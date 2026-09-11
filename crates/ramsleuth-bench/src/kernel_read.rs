//! Streaming READ bandwidth kernel (AVX2, 256-bit aligned loads).
//!
//! **Chunk P1-04 — [COUPLED-TO: P1-01].** A pure read loop over an
//! already-aligned buffer: every 32-byte chunk is consumed with one
//! `_mm256_load_si256` and folded into a 256-bit running sum via
//! `_mm256_add_epi64`, so the loop is a plain "saturate the load pipe"
//! benchmark with no stores and no temporaries.
//!
//! **Dispatch (consumes the frozen P1-01 interface):** on an x86_64 host
//! reporting AVX2 ([`CpuFeatures::detect`]) the SIMD body runs; on
//! non-x86_64 targets or an x86_64 host without AVX2 the *same* function
//! falls back to a safe scalar path with *identical checksum semantics*,
//! so `avx2_read` is always callable and returns the same value for the
//! same input on every path.
//!
//! **No timing here** — wall-clock measurement belongs to the
//! worker/orchestrator chunks (P1-08 / P1-10); this module is a kernel
//! only, and its returned checksum is the dead-code-elimination sink the
//! caller keeps honest (e.g. via `std::hint::black_box`).
//!
//! # Checksum definition (frozen)
//!
//! The returned value is the wrapping sum, modulo 2^64, of the buffer
//! interpreted as **little-endian 64-bit words**:
//! `Σ⌊word_i⌋ mod 2^64` over `len / 8` words (a final partial word, when
//! the length is not a multiple of 8, is zero-extended to 8 bytes). The
//! SIMD path computes exactly this: each 256-bit register holds four
//! u64 words, the per-lane wrapping sums over all chunks are the
//! per-word-position sums, and the horizontal lane reduction
//! `(l0+l2)+(l1+l3) mod 2^64` is order-independent, hence equal to the
//! scalar word sum. A zeroed buffer checksums to `0`.
//!
//! # Precondition (frozen)
//!
//! `src` must be **32-byte aligned** and its **length a multiple of 32
//! bytes**. The kernel never realigns internally: `avx2_read`
//! `debug_assert`s both, and violating them in a release build is
//! undefined behavior. Tests allocate via `Layout::from_size_align(len, 32)`.

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    _mm256_add_epi64, _mm256_castsi256_si128, _mm256_extracti128_si256, _mm256_load_si256,
    _mm256_setzero_si256, _mm_add_epi64, _mm_cvtsi128_si64, _mm_unpackhi_epi64, __m256i,
};

use crate::features::CpuFeatures;

/// Stream-read an entire buffer and return its checksum: the wrapping
/// sum (mod 2^64) of the buffer's little-endian 64-bit words — a
/// deterministic, data-sensitive value usable to defeat dead-code
/// elimination (`0` for a zeroed buffer). See the module docs for the
/// exact definition; it is identical on the SIMD and scalar paths.
///
/// # Precondition
///
/// `src` must be 32-byte aligned with a length that is a multiple of 32
/// bytes (asserted in debug builds; undefined behavior in release builds
/// otherwise). The kernel does not realign internally.
pub fn avx2_read(src: &[u8]) -> u64 {
    debug_assert!((src.as_ptr() as usize) % 32 == 0, "avx2_read: src must be 32-byte aligned");
    debug_assert!(src.len() % 32 == 0, "avx2_read: src.len() must be a multiple of 32");
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx2 {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 32-byte alignment and a 32-multiple
            // length; the slice keeps the buffer valid for the read.
            return unsafe { avx2_read_simd(src.as_ptr(), src.len()) };
        }
    }
    scalar_read(src)
}

/// AVX2 body: unrolled 256-bit aligned loads accumulating into a
/// 256-bit running sum, horizontally reduced to the word-sum checksum.
///
/// # Safety
///
/// `ptr` must be 32-byte aligned, valid for reads of `len` bytes, and
/// `len` a multiple of 32 (enforced by [`avx2_read`]'s debug asserts and
/// documented precondition); every load reads strictly inside that span.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_read_simd(ptr: *const u8, len: usize) -> u64 {
    // SAFETY: the caller (`avx2_read`) guarantees `ptr` is 32-byte
    // aligned, valid for reads of `len` bytes, with `len % 32 == 0`;
    // each `_mm256_load_si256` below therefore reads one 32-byte chunk
    // strictly inside the valid, alignment-correct span.
    let base = ptr as *const __m256i;
    let mut acc = _mm256_setzero_si256();
    let chunks = len / 32;
    let mut i = 0;
    // Unrolled main loop: four aligned loads per iteration, all folded
    // into the same accumulator so every word reaches the checksum.
    while i + 4 <= chunks {
        let v0 = _mm256_load_si256(base.add(i));
        let v1 = _mm256_load_si256(base.add(i + 1));
        let v2 = _mm256_load_si256(base.add(i + 2));
        let v3 = _mm256_load_si256(base.add(i + 3));
        acc = _mm256_add_epi64(acc, v0);
        acc = _mm256_add_epi64(acc, v1);
        acc = _mm256_add_epi64(acc, v2);
        acc = _mm256_add_epi64(acc, v3);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        acc = _mm256_add_epi64(acc, _mm256_load_si256(base.add(i)));
        i += 1;
    }
    // Horizontal-reduce the four wrapping u64 lanes to one u64:
    // [l0, l1, l2, l3] -> (l0 + l2) + (l1 + l3)  (mod 2^64; addition is
    // order-independent, so this equals the wrapping sum of every 64-bit
    // word of the buffer — the documented checksum).
    let lo = _mm256_castsi256_si128(acc); // [l0, l1]
    let hi = _mm256_extracti128_si256(acc, 1); // [l2, l3]
    let pair = _mm_add_epi64(lo, hi); // [l0+l2, l1+l3]
    let a = _mm_cvtsi128_si64(pair) as u64;
    let b = _mm_cvtsi128_si64(_mm_unpackhi_epi64(pair, pair)) as u64;
    a.wrapping_add(b)
}

/// Safe scalar fallback with checksum semantics identical to
/// [`avx2_read_simd`]: the wrapping sum of the buffer's little-endian
/// 64-bit words (a final partial word is zero-extended).
///
/// Works on *any* slice (no alignment precondition); used both as the
/// dispatch fallback and as the reference implementation in the tests.
fn scalar_read(src: &[u8]) -> u64 {
    let (head, tail) = src.split_at(src.len() - src.len() % 8);
    let mut sum: u64 = 0;
    for chunk in head.chunks_exact(8) {
        // `chunk` is exactly 8 bytes; `try_into` cannot fail.
        sum = sum.wrapping_add(u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    if !tail.is_empty() {
        let mut w = [0u8; 8];
        w[..tail.len()].copy_from_slice(tail);
        sum = sum.wrapping_add(u64::from_le_bytes(w));
    }
    sum
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
    /// 64-bit words, via `from_le_bytes` (deliberately different
    /// construction than [`scalar_read`]'s path).
    fn word_sum(data: &[u8]) -> u64 {
        let mut sum: u64 = 0;
        for chunk in data.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u64::from_le_bytes(w));
        }
        sum
    }

    /// (a) A 4 KiB zeroed, 32-byte-aligned buffer yields the
    /// deterministic checksum `0` and does not panic — including a
    /// direct run of the SIMD body on an AVX2 host.
    #[test]
    fn zeroed_4kib_buffer_is_deterministic_and_safe() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        assert_eq!(avx2_read(slice), 0, "a zeroed buffer must checksum to 0");
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx2 {
            // SAFETY: the precondition holds by construction of `slice`.
            assert_eq!(unsafe { avx2_read_simd(slice.as_ptr(), LEN) }, 0);
        }
        dealloc_aligned(ptr, LEN);
    }

    /// (b) The checksum is stable across two calls on identical data,
    /// matches the independent word-sum reference, and changes when the
    /// data changes (defeats dead-code elimination).
    #[test]
    fn checksum_is_stable_and_data_sensitive() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        for (i, b) in slice.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let expected = word_sum(slice);
        assert_eq!(avx2_read(slice), expected, "must equal the word-sum reference");
        assert_eq!(avx2_read(slice), expected, "two calls must agree");
        slice[0] = 250; // original byte 0 was `0`
        assert_ne!(avx2_read(slice), expected, "different data must differ");
        dealloc_aligned(ptr, LEN);
    }

    /// (c) The minimal 32-byte buffer (a single 256-bit load) works.
    #[test]
    fn minimal_32_byte_buffer() {
        const LEN: usize = 32;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        assert_eq!(avx2_read(slice), 0);
        slice.fill(1);
        // 4 LE u64 words of 0x0101010101010101.
        assert_eq!(avx2_read(slice), 4 * 0x0101_0101_0101_0101);
        dealloc_aligned(ptr, LEN);
    }

    /// (d) The scalar fallback is directly callable, compiles on every
    /// target, matches the dispatched checksum on the same data, and
    /// tolerates slices the SIMD path would reject (any alignment/length).
    #[test]
    fn scalar_fallback_matches_dispatch() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        for (i, b) in slice.iter_mut().enumerate() {
            *b = (i * 7 % 256) as u8;
        }
        assert_eq!(
            avx2_read(slice),
            scalar_read(slice),
            "dispatch must preserve checksum semantics"
        );
        // The scalar path has no alignment precondition.
        let unaligned: [u8; 64] = [3u8; 64];
        assert_eq!(scalar_read(&unaligned), 8 * 0x0303_0303_0303_0303);
        // A non-8-multiple length still checksums via zero extension.
        let partial: [u8; 10] = [5u8; 10];
        assert_eq!(scalar_read(&partial), word_sum(&partial));
        dealloc_aligned(ptr, LEN);
    }
}
