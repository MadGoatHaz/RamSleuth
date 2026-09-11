//! COPY bandwidth kernel (AVX2, 256-bit aligned loads + non-temporal stores).
//!
//! **Chunk P1-06 — [COUPLED-TO: P1-01].** A pure copy loop over an
//! already-aligned buffer pair: every 32-byte chunk is read from `src`
//! with one aligned `_mm256_load_si256` (the P1-04 load shape) and
//! written to `dst` with one `_mm256_stream_si256` (the P1-05
//! non-temporal store shape — a store that bypasses the cache hierarchy
//! and goes straight to DRAM, exactly the destination-write policy a
//! copy-bandwidth benchmark wants), with a single trailing
//! `_mm_sfence()` (the P1-05 tail) making every non-temporal store
//! globally visible before the checksum pass reads `dst`.
//!
//! **Dispatch (consumes the frozen P1-01 interface):** on an x86_64 host
//! reporting AVX2 ([`CpuFeatures::detect`]) the SIMD body runs; on
//! non-x86_64 targets or an x86_64 host without AVX2 the *same* function
//! falls back to a safe scalar `copy_from_slice` path that produces
//! *identical* destination contents, so `avx2_copy` is always callable
//! and returns the same checksum on every path.
//!
//! **No timing here** — wall-clock measurement belongs to the
//! worker/orchestrator chunks (P1-08 / P1-10); this module is a kernel
//! only.
//!
//! # Checksum definition (frozen — reuses P1-04)
//!
//! The returned value is the **P1-04 word-sum checksum of the
//! destination buffer after the copy**: the wrapping sum, modulo 2^64,
//! of `dst` interpreted as little-endian 64-bit words. It is computed by
//! calling the frozen [`avx2_read`] kernel, so the definition has a
//! single shared source of truth — the SIMD and scalar copy paths return
//! the identical checksum for identical contents (`0` for a zeroed
//! destination), and the data-sensitive checksum defeats dead-code
//! elimination of the copy itself.
//!
//! # Precondition (frozen)
//!
//! `src` and `dst` must each be **32-byte aligned** with a **length that
//! is a multiple of 32 bytes**, the lengths must be **equal**, and the
//! two ranges must **not alias** (a non-temporal copy with overlap is
//! not defined by this kernel; callers pass disjoint buffers). The
//! kernel never realigns internally: `avx2_copy` `debug_assert`s all of
//! this, and violating it in a release build is undefined behavior.
//! Tests allocate via `Layout::from_size_align(len, 32)`.

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{_mm256_load_si256, _mm256_stream_si256, _mm_sfence, __m256i};

use crate::features::CpuFeatures;
use crate::kernel_read::avx2_read;

/// Copy the whole of `src` into `dst` and return the checksum of the
/// destination after the copy: the P1-04 wrapping word-sum (mod 2^64)
/// of `dst`'s little-endian 64-bit words, computed with the frozen
/// [`avx2_read`] kernel (identical semantics on the SIMD and scalar
/// paths; `0` for an empty/zeroed destination). See the module docs for
/// dispatch, the checksum definition, and the frozen precondition.
///
/// # Precondition
///
/// `src` and `dst` must each be 32-byte aligned, both lengths a
/// multiple of 32 and equal, and the ranges must not alias (asserted in
/// debug builds; undefined behavior in release builds otherwise). The
/// kernel does not realign internally.
pub fn avx2_copy(src: &[u8], dst: &mut [u8]) -> u64 {
    debug_assert!(src.len() == dst.len(), "avx2_copy: src.len() must equal dst.len()");
    debug_assert!((src.as_ptr() as usize) % 32 == 0, "avx2_copy: src must be 32-byte aligned");
    debug_assert!((dst.as_ptr() as usize) % 32 == 0, "avx2_copy: dst must be 32-byte aligned");
    debug_assert!(src.len() % 32 == 0, "avx2_copy: src.len() must be a multiple of 32");
    debug_assert!(
        src.as_ptr() as usize + src.len() <= dst.as_ptr() as usize
            || dst.as_ptr() as usize + dst.len() <= src.as_ptr() as usize,
        "avx2_copy: src and dst must not alias",
    );
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx2 {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 32-byte alignment on both sides,
            // equal 32-multiple lengths, and disjoint ranges; `src`
            // keeps its buffer valid for reads and `dst` for writes.
            unsafe {
                avx2_copy_simd(src.as_ptr(), dst.as_mut_ptr(), src.len());
            }
            return avx2_read(dst);
        }
    }
    // Scalar fallback: byte-identical destination contents; the
    // checksum is the same P1-04 word sum (`avx2_read` itself falls
    // back to its scalar form off-AVX2, so both paths agree).
    scalar_copy(src, dst);
    avx2_read(dst)
}

/// AVX2 body: unrolled 256-bit aligned loads from `src` interleaved
/// with non-temporal 256-bit stores into `dst`, followed by a single
/// `_mm_sfence()` so every store is globally visible before the caller
/// checksums `dst`.
///
/// # Safety
///
/// `src` must be 32-byte aligned and valid for reads of `len` bytes;
/// `dst` must be 32-byte aligned and valid for writes of `len` bytes;
/// `len` a multiple of 32; and the two spans must not alias (enforced
/// by [`avx2_copy`]'s debug asserts and documented precondition). Every
/// load/store touches one 32-byte chunk strictly inside its span.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_copy_simd(src: *const u8, dst: *mut u8, len: usize) {
    // SAFETY: the caller (`avx2_copy`) guarantees both pointers are
    // 32-byte aligned, `len % 32 == 0`, `src` is valid for reads of
    // `len` bytes, `dst` is valid for writes of `len` bytes, and the
    // spans are disjoint; each intrinsic below therefore touches one
    // 32-byte chunk strictly inside its valid, alignment-correct span.
    let s = src as *const __m256i;
    let d = dst as *mut __m256i;
    let chunks = len / 32;
    let mut i = 0;
    // Unrolled main loop: four aligned loads and four non-temporal
    // stores per iteration, interleaved chunk by chunk.
    while i + 4 <= chunks {
        let v0 = _mm256_load_si256(s.add(i));
        let v1 = _mm256_load_si256(s.add(i + 1));
        let v2 = _mm256_load_si256(s.add(i + 2));
        let v3 = _mm256_load_si256(s.add(i + 3));
        _mm256_stream_si256(d.add(i), v0);
        _mm256_stream_si256(d.add(i + 1), v1);
        _mm256_stream_si256(d.add(i + 2), v2);
        _mm256_stream_si256(d.add(i + 3), v3);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        _mm256_stream_si256(d.add(i), _mm256_load_si256(s.add(i)));
        i += 1;
    }
    // Store fence: make every non-temporal store above globally visible
    // before the caller observes `dst` (placed after the last store,
    // mirroring the P1-05 tail).
    _mm_sfence();
}

/// Safe scalar fallback copying `src` into `dst` with contents
/// identical to [`avx2_copy_simd`] (a plain `copy_from_slice`).
///
/// Works on *any* pair of equal-length slices (no alignment
/// precondition); used both as the dispatch fallback and as the
/// reference implementation in the tests.
fn scalar_copy(src: &[u8], dst: &mut [u8]) {
    dst.copy_from_slice(src);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    /// Allocate `len` zeroed bytes with a guaranteed 32-byte alignment
    /// (the kernel precondition). The caller must release the pointer
    /// with [`dealloc_aligned`] using the same length.
    fn aligned_zeroed(len: usize) -> *mut u8 {
        assert!(len % 32 == 0);
        // SAFETY: `len` is a multiple of the 32-byte alignment, so the
        // layout is well-formed (power-of-two align, no overflow).
        unsafe { std::alloc::alloc_zeroed(Layout::from_size_align(len, 32).unwrap()) }
    }

    /// Release a pointer from [`aligned_zeroed`] with the matching layout.
    fn dealloc_aligned(ptr: *mut u8, len: usize) {
        // SAFETY: `ptr` came from `aligned_zeroed(len)` and is still
        // alive with unchanged length and alignment.
        unsafe { std::alloc::dealloc(ptr, Layout::from_size_align(len, 32).unwrap()) };
    }

    /// Independent reference checksum: wrapping sum of the little-endian
    /// 64-bit words (the P1-04 word-sum convention), via `from_le_bytes`
    /// (deliberately different construction than the kernel paths).
    fn word_sum(data: &[u8]) -> u64 {
        let mut sum: u64 = 0;
        for chunk in data.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u64::from_le_bytes(w));
        }
        sum
    }

    /// (a) Copying a 4 KiB patterned, 32-byte-aligned source into an
    /// aligned destination leaves the destination byte-identical to
    /// the source, returns the P1-04 word-sum checksum of it (matching
    /// a direct `copy_from_slice` result), and does not panic —
    /// including a direct run of the SIMD body on an AVX2 host.
    #[test]
    fn copy_4kib_patterned_buffer_is_byte_identical() {
        const LEN: usize = 4 * 1024;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        let checksum = avx2_copy(src, dst);
        assert_eq!(dst, src, "destination must be byte-identical to the source");
        assert_eq!(checksum, word_sum(src), "checksum must match the P1-04 word sum");
        // A direct `copy_from_slice` over the same pair yields the
        // same contents and therefore the same checksum.
        dst.fill(0);
        dst.copy_from_slice(src);
        assert_eq!(avx2_read(dst), checksum, "checksum must match a direct copy_from_slice result");
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx2 {
            dst.fill(0);
            // SAFETY: the precondition holds by construction of `src`/`dst`.
            unsafe { avx2_copy_simd(src.as_ptr(), dst.as_mut_ptr(), LEN) };
            assert_eq!(dst, src, "SIMD body must copy byte-identically");
            assert_eq!(avx2_read(dst), checksum);
        }
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (b) The returned checksum is stable across two calls on the
    /// same source and changes when the source data changes (defeats
    /// dead-code elimination of the copy).
    #[test]
    fn checksum_is_stable_and_data_sensitive() {
        const LEN: usize = 4 * 1024;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i * 7 % 256) as u8;
        }
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        let first = avx2_copy(src, dst);
        let second = avx2_copy(src, dst);
        assert_eq!(first, second, "two calls on the same source must agree");
        assert_ne!(first, 0, "a non-zero pattern must not checksum to 0");
        src[0] = src[0].wrapping_add(1); // mutate the source (byte 0 was 0)
        let third = avx2_copy(src, dst);
        assert_ne!(third, first, "different source data must change the checksum");
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (c) The minimal 32-byte buffer (one 256-bit aligned load + one
    /// non-temporal store) works.
    #[test]
    fn minimal_32_byte_buffer() {
        const LEN: usize = 32;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        src.fill(0xA5);
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        // Four identical LE u64 words of 0xA5A5A5A5A5A5A5A5: the
        // wrapping word sum is `4 * word` (mod 2^64).
        let word = 0xA5A5_A5A5_A5A5_A5A5u64;
        assert_eq!(avx2_copy(src, dst), word.wrapping_mul(4));
        assert_eq!(dst, src, "destination must be byte-identical to the source");
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (d) The scalar fallback is directly callable and produces a
    /// destination byte-identical to the dispatched (SIMD on an AVX2
    /// host) path for the same source pair.
    #[test]
    fn scalar_fallback_matches_simd_destination() {
        const LEN: usize = 4 * 1024;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        avx2_copy(src, dst); // dispatched path (SIMD on AVX2 hosts)
        assert_eq!(dst, src);
        dst.fill(0);
        scalar_copy(src, dst); // same pair via the safe fallback
        assert_eq!(dst, src, "scalar fallback must produce identical contents");
        assert_eq!(avx2_read(dst), word_sum(src));
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx2 {
            dst.fill(0);
            // SAFETY: the precondition holds by construction of `src`/`dst`.
            unsafe { avx2_copy_simd(src.as_ptr(), dst.as_mut_ptr(), LEN) };
            assert_eq!(dst, src, "SIMD body must produce identical contents");
        }
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }
}
