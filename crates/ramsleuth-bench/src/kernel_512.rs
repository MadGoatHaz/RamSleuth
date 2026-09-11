//! 512-bit (AVX-512F) variants of the read / write / copy bandwidth kernels.
//!
//! **Chunk P1-07 — [COUPLED-TO: P1-01].** The 512-bit companions to the
//! P1-04 / P1-05 / P1-06 AVX2 kernels, reusing their exact shapes at
//! double width: `_mm512_load_si512` aligned loads (the P1-04 load
//! shape), `_mm512_stream_si512` non-temporal stores (the P1-05 store
//! shape — a store that bypasses the cache hierarchy and goes straight
//! to DRAM), a single trailing `_mm_sfence()` (the P1-05 tail), and the
//! P1-04 word-sum checksum as the dead-code-elimination sink.
//!
//! **Dispatch (consumes the frozen P1-01 interface):** all three public
//! fns are *always* callable. On an x86_64 host reporting AVX-512F
//! (`CpuFeatures::detect().avx512f`) the 512-bit body runs (gated
//! `#[target_feature(enable = "avx512f")]`); otherwise the fn falls back
//! to the corresponding AVX2 kernel (`avx2_read` / `avx2_write` /
//! `avx2_copy`), which itself falls back to scalar on non-AVX2 hosts.
//! No scalar fallback is reimplemented in this module.
//!
//! **MSRV note:** the AVX-512F `core::arch` intrinsics stabilized in
//! Rust 1.89, newer than this crate's declared MSRV (1.75, set at
//! workspace level). Each 512-bit body is annotated
//! `#[clippy::msrv = "1.89"]` so clippy's MSRV accounting treats those
//! items at the toolchain version their intrinsics require. Compiling
//! this module on x86_64 therefore needs rustc >= 1.89; aligning the
//! workspace `rust-version` with that is a follow-up plan-level decision
//! (outside this chunk's file scope). The *callable* API degrades
//! gracefully per host: without AVX-512F the dispatch runs the AVX2
//! kernels instead.
//!
//! **No timing here** — wall-clock measurement belongs to the
//! worker/orchestrator chunks (P1-08 / P1-10), exactly as in
//! P1-04…P1-06; this module is a kernel only.
//!
//! # Checksum definition (frozen — reuses P1-04)
//!
//! [`avx512_read`] returns the wrapping sum, modulo 2^64, of the buffer
//! interpreted as little-endian 64-bit words — the P1-04 convention,
//! identical to `avx2_read` for the same data regardless of which SIMD
//! path ran. The 512-bit accumulator folds the eight lanes of every
//! aligned chunk via `_mm512_add_epi64` and horizontally reduces them
//! (addition is order-independent, so the result equals the word sum).
//! [`avx512_copy`] returns that checksum of the destination after the
//! copy, computed with the frozen `avx2_read`, matching `avx2_copy`.
//!
//! # Return value (frozen)
//!
//! [`avx512_write`] returns the number of bytes written —
//! `dst.len()` — identical to `avx2_write`'s return.
//!
//! # Precondition (frozen — no realignment)
//!
//! Buffers must be **64-byte aligned** with a **length that is a
//! multiple of 64 bytes**; the `src`/`dst` ranges of [`avx512_copy`]
//! must be **non-aliasing** (a non-temporal copy with overlap is not
//! defined by this kernel). Because 64-alignment implies the AVX2
//! kernels' 32-byte alignment and 64-multiples imply 32-multiples, the
//! fallback paths' preconditions are satisfied by the same guarantee.
//! The kernels never realign internally: each public fn `debug_assert`s
//! the precondition, and violating it in a release build is undefined
//! behavior. Tests allocate via `Layout::from_size_align(len, 64)`.

// The AVX-512F `core::arch` intrinsics used by the 512-bit bodies below
// stabilized in Rust 1.89, which is newer than this crate's declared
// MSRV (1.75, set at workspace level). Each 512-bit body is therefore
// annotated `#[clippy::msrv = "1.89"]` so clippy's MSRV accounting treats
// those items at the toolchain version their intrinsics require; see the
// module docs' MSRV note. Compiling this module on x86_64 needs rustc
// >= 1.89 (aligning the workspace MSRV is a follow-up decision, outside
// this chunk's file scope).
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    _mm512_add_epi64, _mm512_castsi512_si256, _mm512_extracti64x4_epi64, _mm512_load_si512,
    _mm512_set_epi64, _mm512_setzero_si512, _mm512_stream_si512, _mm_add_epi64, _mm256_castsi256_si128,
    _mm_cvtsi128_si64, _mm256_extracti128_si256, _mm_sfence, _mm_unpackhi_epi64, __m512i,
};

use crate::features::CpuFeatures;
use crate::kernel_copy::avx2_copy;
use crate::kernel_read::avx2_read;
use crate::kernel_write::avx2_write;

/// Stream-read an entire buffer at 512-bit width and return its
/// checksum: the P1-04 wrapping sum (mod 2^64) of the buffer's
/// little-endian 64-bit words — the identical convention to
/// [`avx2_read`] (a deterministic, data-sensitive value usable to
/// defeat dead-code elimination; `0` for a zeroed buffer). See the
/// module docs for dispatch and the frozen precondition.
///
/// # Precondition
///
/// `src` must be 64-byte aligned with a length that is a multiple of
/// 64 bytes (asserted in debug builds; undefined behavior in release
/// builds otherwise). The kernel does not realign internally.
pub fn avx512_read(src: &[u8]) -> u64 {
    debug_assert!(
        (src.as_ptr() as usize) % 64 == 0,
        "avx512_read: src must be 64-byte aligned",
    );
    debug_assert!(
        src.len() % 64 == 0,
        "avx512_read: src.len() must be a multiple of 64",
    );
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx512f {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 64-byte alignment and a
            // 64-multiple length; the slice keeps the buffer valid for
            // the read.
            return unsafe { avx512_read_simd(src.as_ptr(), src.len()) };
        }
    }
    // Fallback: the AVX2 read kernel (which itself falls back to
    // scalar off-AVX2). The 64-alignment / 64-multiple precondition
    // implies the AVX2 kernel's 32-alignment / 32-multiple precondition,
    // so the same buffer is valid for both dispatches.
    avx2_read(src)
}

/// Fill an entire buffer with non-temporal 512-bit stores of eight
/// little-endian copies of `pattern`, terminated by a single
/// `_mm_sfence()`, and return the number of bytes written
/// (`dst.len()`) — the identical return to [`avx2_write`]. See the
/// module docs for dispatch and the frozen precondition.
///
/// # Precondition
///
/// `dst` must be 64-byte aligned with a length that is a multiple of
/// 64 bytes (asserted in debug builds; undefined behavior in release
/// builds otherwise). The kernel does not realign internally.
pub fn avx512_write(dst: &mut [u8], pattern: u64) -> u64 {
    debug_assert!(
        (dst.as_ptr() as usize) % 64 == 0,
        "avx512_write: dst must be 64-byte aligned",
    );
    debug_assert!(
        dst.len() % 64 == 0,
        "avx512_write: dst.len() must be a multiple of 64",
    );
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx512f {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 64-byte alignment and a
            // 64-multiple length; the slice keeps the buffer valid for
            // the writes.
            return unsafe { avx512_write_simd(dst.as_mut_ptr(), dst.len(), pattern) };
        }
    }
    // Fallback: the AVX2 write kernel (which itself falls back to
    // scalar off-AVX2) fills with identical contents and returns the
    // same byte count.
    avx2_write(dst, pattern)
}

/// Copy the whole of `src` into `dst` at 512-bit width (aligned loads +
/// non-temporal stores + a single trailing `_mm_sfence()`) and return
/// the checksum of the destination after the copy: the P1-04 wrapping
/// word-sum (mod 2^64) of `dst`'s little-endian 64-bit words, computed
/// with the frozen `avx2_read` kernel — the identical semantics to
/// [`avx2_copy`] (`0` for an empty/zeroed destination). See the module
/// docs for dispatch and the frozen precondition.
///
/// # Precondition
///
/// `src` and `dst` must each be 64-byte aligned, both lengths a
/// multiple of 64 and equal, and the ranges must not alias (asserted in
/// debug builds; undefined behavior in release builds otherwise). The
/// kernel does not realign internally.
pub fn avx512_copy(src: &[u8], dst: &mut [u8]) -> u64 {
    debug_assert!(src.len() == dst.len(), "avx512_copy: src.len() must equal dst.len()");
    debug_assert!(
        (src.as_ptr() as usize) % 64 == 0,
        "avx512_copy: src must be 64-byte aligned",
    );
    debug_assert!(
        (dst.as_ptr() as usize) % 64 == 0,
        "avx512_copy: dst must be 64-byte aligned",
    );
    debug_assert!(
        src.len() % 64 == 0,
        "avx512_copy: src.len() must be a multiple of 64",
    );
    debug_assert!(
        src.as_ptr() as usize + src.len() <= dst.as_ptr() as usize
            || dst.as_ptr() as usize + dst.len() <= src.as_ptr() as usize,
        "avx512_copy: src and dst must not alias",
    );
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx512f {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 64-byte alignment on both sides,
            // equal 64-multiple lengths, and disjoint ranges; `src`
            // keeps its buffer valid for reads and `dst` for writes.
            unsafe {
                avx512_copy_simd(src.as_ptr(), dst.as_mut_ptr(), src.len());
            }
            return avx2_read(dst);
        }
    }
    // Fallback: the AVX2 copy kernel (which itself falls back to
    // scalar off-AVX2) produces byte-identical destination contents and
    // the same P1-04 checksum.
    avx2_copy(src, dst)
}

/// AVX-512F body: unrolled 512-bit aligned loads accumulating into a
/// 512-bit running sum, horizontally reduced to the P1-04 word-sum
/// checksum.
///
/// # Safety
///
/// `ptr` must be 64-byte aligned, valid for reads of `len` bytes, and
/// `len` a multiple of 64 (enforced by [`avx512_read`]'s debug asserts
/// and documented precondition); every load reads strictly inside that
/// span.
#[cfg(target_arch = "x86_64")]
#[clippy::msrv = "1.89"]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_read_simd(ptr: *const u8, len: usize) -> u64 {
    // SAFETY: the caller (`avx512_read`) guarantees `ptr` is 64-byte
    // aligned, valid for reads of `len` bytes, with `len % 64 == 0`;
    // each `_mm512_load_si512` below therefore reads one 64-byte chunk
    // strictly inside the valid, alignment-correct span.
    let base = ptr as *const __m512i;
    let mut acc = _mm512_setzero_si512();
    let chunks = len / 64;
    let mut i = 0;
    // Unrolled main loop: four aligned loads per iteration, all folded
    // into the same accumulator so every word reaches the checksum.
    while i + 4 <= chunks {
        let v0 = _mm512_load_si512(base.add(i));
        let v1 = _mm512_load_si512(base.add(i + 1));
        let v2 = _mm512_load_si512(base.add(i + 2));
        let v3 = _mm512_load_si512(base.add(i + 3));
        acc = _mm512_add_epi64(acc, v0);
        acc = _mm512_add_epi64(acc, v1);
        acc = _mm512_add_epi64(acc, v2);
        acc = _mm512_add_epi64(acc, v3);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        acc = _mm512_add_epi64(acc, _mm512_load_si512(base.add(i)));
        i += 1;
    }
    // Horizontal-reduce the eight wrapping u64 lanes to one u64: split
    // the accumulator into its two 256-bit halves, reduce each half the
    // same way the AVX2 read does (`[l0, l1] + [l2, l3]` per half), add
    // the two 128-bit partial-pair sums, then combine the final two
    // lanes. 64-bit addition is order-independent (mod 2^64), so this
    // equals the wrapping sum of every 64-bit word of the buffer — the
    // P1-04 checksum.
    let lo = _mm512_castsi512_si256(acc); // lanes 0-3
    let hi = _mm512_extracti64x4_epi64(acc, 1); // lanes 4-7
    let lo_pair = _mm_add_epi64(_mm256_castsi256_si128(lo), _mm256_extracti128_si256(lo, 1));
    let hi_pair = _mm_add_epi64(_mm256_castsi256_si128(hi), _mm256_extracti128_si256(hi, 1));
    let total = _mm_add_epi64(lo_pair, hi_pair); // [w0+w2+w4+w6, w1+w3+w5+w7]
    let a = _mm_cvtsi128_si64(total) as u64;
    let b = _mm_cvtsi128_si64(_mm_unpackhi_epi64(total, total)) as u64;
    a.wrapping_add(b)
}

/// AVX-512F body: one 512-bit vector (eight LE copies of `pattern`)
/// written by unrolled `_mm512_stream_si512` non-temporal stores,
/// followed by a single `_mm_sfence()` so the stores are globally
/// visible.
///
/// # Safety
///
/// `ptr` must be 64-byte aligned, valid for writes of `len` bytes, and
/// `len` a multiple of 64 (enforced by [`avx512_write`]'s debug asserts
/// and documented precondition); every store writes strictly inside
/// that span.
#[cfg(target_arch = "x86_64")]
#[clippy::msrv = "1.89"]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_write_simd(ptr: *mut u8, len: usize, pattern: u64) -> u64 {
    // SAFETY: the caller (`avx512_write`) guarantees `ptr` is 64-byte
    // aligned, valid for writes of `len` bytes, with `len % 64 == 0`;
    // each `_mm512_stream_si512` below therefore writes one 64-byte
    // chunk strictly inside the valid, alignment-correct span.
    let base = ptr as *mut __m512i;
    let word = pattern as i64;
    let v = _mm512_set_epi64(word, word, word, word, word, word, word, word);
    let chunks = len / 64;
    let mut i = 0;
    // Unrolled main loop: four non-temporal stores per iteration.
    while i + 4 <= chunks {
        _mm512_stream_si512(base.add(i), v);
        _mm512_stream_si512(base.add(i + 1), v);
        _mm512_stream_si512(base.add(i + 2), v);
        _mm512_stream_si512(base.add(i + 3), v);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        _mm512_stream_si512(base.add(i), v);
        i += 1;
    }
    // Store fence: make every non-temporal store above globally visible
    // before the caller proceeds (placed after the last store, per plan).
    _mm_sfence();
    len as u64
}

/// AVX-512F body: unrolled 512-bit aligned loads from `src`
/// interleaved with non-temporal 512-bit stores into `dst`, followed by
/// a single `_mm_sfence()` so every store is globally visible before
/// the caller checksums `dst`.
///
/// # Safety
///
/// `src` must be 64-byte aligned and valid for reads of `len` bytes;
/// `dst` must be 64-byte aligned and valid for writes of `len` bytes;
/// `len` a multiple of 64; and the two spans must not alias (enforced
/// by [`avx512_copy`]'s debug asserts and documented precondition).
/// Every load/store touches one 64-byte chunk strictly inside its span.
#[cfg(target_arch = "x86_64")]
#[clippy::msrv = "1.89"]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_copy_simd(src: *const u8, dst: *mut u8, len: usize) {
    // SAFETY: the caller (`avx512_copy`) guarantees both pointers are
    // 64-byte aligned, `len % 64 == 0`, `src` is valid for reads of
    // `len` bytes, `dst` is valid for writes of `len` bytes, and the
    // spans are disjoint; each intrinsic below therefore touches one
    // 64-byte chunk strictly inside its valid, alignment-correct span.
    let s = src as *const __m512i;
    let d = dst as *mut __m512i;
    let chunks = len / 64;
    let mut i = 0;
    // Unrolled main loop: four aligned loads and four non-temporal
    // stores per iteration, interleaved chunk by chunk.
    while i + 4 <= chunks {
        let v0 = _mm512_load_si512(s.add(i));
        let v1 = _mm512_load_si512(s.add(i + 1));
        let v2 = _mm512_load_si512(s.add(i + 2));
        let v3 = _mm512_load_si512(s.add(i + 3));
        _mm512_stream_si512(d.add(i), v0);
        _mm512_stream_si512(d.add(i + 1), v1);
        _mm512_stream_si512(d.add(i + 2), v2);
        _mm512_stream_si512(d.add(i + 3), v3);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        _mm512_stream_si512(d.add(i), _mm512_load_si512(s.add(i)));
        i += 1;
    }
    // Store fence: make every non-temporal store above globally visible
    // before the caller observes `dst` (placed after the last store,
    // mirroring the P1-05 tail).
    _mm_sfence();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    /// Arbitrary fixed pattern for the content tests (shared with the
    /// AVX2 write kernel's test constant).
    const PATTERN: u64 = 0xA50F_1234_5678_9ABC;

    /// Allocate `len` zeroed bytes with a guaranteed 64-byte alignment
    /// (the kernel precondition). The caller must release the pointer
    /// with [`dealloc_aligned`] using the same length.
    fn aligned_zeroed(len: usize) -> *mut u8 {
        assert!(len % 64 == 0);
        // SAFETY: `len` is a multiple of the 64-byte alignment, so the
        // layout is well-formed (power-of-two align, no overflow).
        unsafe { std::alloc::alloc_zeroed(Layout::from_size_align(len, 64).unwrap()) }
    }

    /// Release a pointer from [`aligned_zeroed`] with the matching layout.
    fn dealloc_aligned(ptr: *mut u8, len: usize) {
        // SAFETY: `ptr` came from `aligned_zeroed(len)` and is still
        // alive with unchanged length and alignment.
        unsafe { std::alloc::dealloc(ptr, Layout::from_size_align(len, 64).unwrap()) };
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

    /// Expected contents after filling `len` bytes with `pattern`:
    /// little-endian copies of the 64-bit pattern word, repeated.
    fn expected(len: usize, pattern: u64) -> Vec<u8> {
        let word = pattern.to_le_bytes();
        let mut out = Vec::with_capacity(len);
        for _ in 0..len / 8 {
            out.extend_from_slice(&word);
        }
        out
    }

    /// (a) On an 8 KiB aligned buffer the dispatched `avx512_read`
    /// returns the same checksum as `avx2_read` — the P1-04 word-sum
    /// convention holds regardless of which SIMD path ran (and matches
    /// the independent word-sum reference).
    #[test]
    fn read_checksum_matches_avx2_on_8kib() {
        const LEN: usize = 8 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        for (i, b) in slice.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        assert_eq!(
            avx512_read(slice),
            avx2_read(slice),
            "the 512 dispatch must preserve the P1-04 checksum",
        );
        assert_eq!(avx512_read(slice), word_sum(slice), "must equal the word-sum reference");
        dealloc_aligned(ptr, LEN);
    }

    /// (b) `avx512_write` fills the 8 KiB buffer with the pattern
    /// (spot-checked and fully compared) and returns `dst.len()`,
    /// matching `avx2_write`'s return.
    #[test]
    fn write_fills_pattern_and_returns_len_on_8kib() {
        const LEN: usize = 8 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        let want = expected(LEN, PATTERN);
        assert_eq!(avx512_write(slice, PATTERN), LEN as u64);
        assert_eq!(
            avx512_write(slice, PATTERN),
            avx2_write(slice, PATTERN),
            "the 512 dispatch must return the same byte count",
        );
        // Spot-check several 64-byte chunks (first, second, middle, last).
        for off in [0usize, 64, LEN / 2, LEN - 64] {
            assert_eq!(&slice[off..off + 64], &want[off..off + 64]);
        }
        assert_eq!(&*slice, want.as_slice());
        dealloc_aligned(ptr, LEN);
    }

    /// (c) `avx512_copy` over an 8 KiB aligned pair leaves the
    /// destination byte-identical to the source (a direct
    /// `copy_from_slice` result) and returns the same checksum as
    /// `avx2_copy` on the same inputs.
    #[test]
    fn copy_is_byte_identical_and_checksum_matches_avx2_on_8kib() {
        const LEN: usize = 8 * 1024;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        let checksum = avx512_copy(src, dst);
        assert_eq!(dst, src, "destination must be byte-identical to the source");
        // A direct `copy_from_slice` over the same pair yields the same
        // contents and therefore the same checksum.
        dst.fill(0);
        dst.copy_from_slice(src);
        assert_eq!(dst, src, "destination must match a direct copy_from_slice");
        assert_eq!(avx2_read(dst), checksum, "checksum must match a direct copy_from_slice result");
        // The checksum also equals `avx2_copy`'s on the same inputs.
        dst.fill(0);
        assert_eq!(
            avx2_copy(src, dst),
            checksum,
            "the 512 dispatch must preserve the P1-04 destination checksum",
        );
        assert_eq!(dst, src);
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (d) The minimal 64-byte buffer (one 512-bit load / NT store)
    /// works for all three kernels.
    #[test]
    fn minimal_64_byte_buffers() {
        // Read: a zeroed 64-byte buffer checksums to 0; a buffer of
        // all-1 bytes is eight identical LE u64 words.
        let ptr = aligned_zeroed(64);
        // SAFETY: `ptr` is valid for 64 writes (fresh allocation).
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, 64) };
        assert_eq!(avx512_read(buf), 0, "a zeroed buffer must checksum to 0");
        buf.fill(1);
        assert_eq!(avx512_read(buf), 8 * 0x0101_0101_0101_0101);
        // Write: returns the byte count and fills eight LE pattern words.
        buf.fill(0);
        assert_eq!(avx512_write(buf, PATTERN), 64);
        assert_eq!(&*buf, expected(64, PATTERN).as_slice());
        // Copy: 0xA5A5...A5 source; the wrapping word sum is 8 * word.
        let src_ptr = aligned_zeroed(64);
        // SAFETY: `src_ptr` is valid for 64 reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, 64) };
        src.fill(0xA5);
        let word = 0xA5A5_A5A5_A5A5_A5A5u64;
        assert_eq!(avx512_copy(src, buf), word.wrapping_mul(8));
        assert_eq!(buf, src, "destination must be byte-identical to the source");
        dealloc_aligned(ptr, 64);
        dealloc_aligned(src_ptr, 64);
    }

    /// (e) If the host reports AVX-512F, the `#[target_feature]` 512-bit
    /// bodies are called directly to prove they compile and run without
    /// panicking; otherwise the dispatch is verified to have fallen back
    /// to the AVX2 kernels, with results equal to the AVX2 fns.
    #[test]
    fn direct_512_bodies_or_verified_avx2_fallback() {
        const LEN: usize = 8 * 1024;
        let src_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is valid for `LEN` reads/writes (fresh allocation).
        let src = unsafe { core::slice::from_raw_parts_mut(src_ptr, LEN) };
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `dst_ptr` is valid for `LEN` writes (fresh allocation).
        let dst = unsafe { core::slice::from_raw_parts_mut(dst_ptr, LEN) };
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx512f {
            // SAFETY: all preconditions hold by construction of `src`/`dst`.
            assert_eq!(unsafe { avx512_read_simd(src.as_ptr(), LEN) }, word_sum(src));
            // SAFETY: all preconditions hold by construction of `dst`.
            let n = unsafe { avx512_write_simd(dst.as_mut_ptr(), LEN, PATTERN) };
            assert_eq!(n, LEN as u64);
            assert_eq!(&*dst, expected(LEN, PATTERN).as_slice());
            dst.fill(0);
            // SAFETY: all preconditions hold by construction of `src`/`dst`.
            unsafe { avx512_copy_simd(src.as_ptr(), dst.as_mut_ptr(), LEN) };
            assert_eq!(dst, src, "the 512-bit body must copy byte-identically");
            assert_eq!(avx512_read(dst), word_sum(src));
        } else {
            // No AVX-512F on this host: dispatch must fall back to the
            // AVX2 kernels, and the results must equal them exactly.
            assert_eq!(avx512_read(src), avx2_read(src));
            assert_eq!(avx512_write(dst, PATTERN), avx2_write(dst, PATTERN));
            dst.fill(0);
            assert_eq!(avx512_copy(src, dst), avx2_copy(src, dst));
            assert_eq!(dst, src);
        }
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }
}
