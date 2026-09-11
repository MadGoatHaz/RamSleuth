//! Non-temporal streaming WRITE bandwidth kernel (AVX2, 256-bit NT stores).
//!
//! **Chunk P1-05 — [COUPLED-TO: P1-01].** A pure write loop over an
//! already-aligned buffer: every 32-byte chunk is written with one
//! `_mm256_stream_si256` — a *non-temporal* store that bypasses the cache
//! hierarchy and goes straight to DRAM (intentional: this kernel
//! benchmarks DRAM write bandwidth, and skipping the RFO /
//! read-for-ownership is exactly the signature the orchestrator later
//! distinguishes) — repeating a single 256-bit vector built from
//! `pattern` (four little-endian copies of the same u64 word). A single
//! `_mm_sfence()` issued after the last store makes all the non-temporal
//! stores globally visible before the caller observes the buffer.
//!
//! **Dispatch (consumes the frozen P1-01 interface):** on an x86_64 host
//! reporting AVX2 ([`CpuFeatures::detect`]) the SIMD body runs; on
//! non-x86_64 targets or an x86_64 host without AVX2 the *same* function
//! falls back to a safe scalar path that fills the buffer with
//! *identical* contents, so `avx2_write` is always callable and produces
//! the same result on every path.
//!
//! **No timing here** — wall-clock measurement belongs to the
//! worker/orchestrator chunks (P1-08 / P1-10); this module is a kernel
//! only. The returned byte count plus the caller's subsequent use of the
//! buffer (e.g. via `std::hint::black_box`) defeats dead-code elimination.
//!
//! # Return value (frozen)
//!
//! The number of bytes written — `dst.len()` — a deterministic, non-zero
//! counter for any non-empty buffer, identical on the SIMD and scalar
//! paths (see the module precondition).
//!
//! # Precondition (frozen)
//!
//! `dst` must be **32-byte aligned** and its **length a multiple of 32
//! bytes**. The kernel never realigns internally: `avx2_write`
//! `debug_assert`s both, and violating them in a release build is
//! undefined behavior. Tests allocate via `Layout::from_size_align(len, 32)`.

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{_mm256_set_epi64x, _mm256_stream_si256, _mm_sfence, __m256i};

use crate::features::CpuFeatures;

/// Fill an entire buffer with non-temporal 256-bit stores of four
/// little-endian copies of `pattern`, terminated by a single
/// `_mm_sfence()`, and return the number of bytes written (`dst.len()`).
/// See the module docs for dispatch, the return definition, and the
/// frozen precondition.
///
/// # Precondition
///
/// `dst` must be 32-byte aligned with a length that is a multiple of 32
/// bytes (asserted in debug builds; undefined behavior in release builds
/// otherwise). The kernel does not realign internally.
pub fn avx2_write(dst: &mut [u8], pattern: u64) -> u64 {
    debug_assert!((dst.as_ptr() as usize) % 32 == 0, "avx2_write: dst must be 32-byte aligned");
    debug_assert!(dst.len() % 32 == 0, "avx2_write: dst.len() must be a multiple of 32");
    #[cfg(target_arch = "x86_64")]
    {
        if CpuFeatures::detect().avx2 {
            // SAFETY: the debug_asserts above (and the documented
            // precondition) guarantee 32-byte alignment and a 32-multiple
            // length; the slice keeps the buffer valid for the writes.
            return unsafe { avx2_write_simd(dst.as_mut_ptr(), dst.len(), pattern) };
        }
    }
    scalar_write(dst, pattern);
    dst.len() as u64
}

/// AVX2 body: one 256-bit vector (four LE copies of `pattern`) written
/// by unrolled `_mm256_stream_si256` non-temporal stores, followed by a
/// single `_mm_sfence()` so the stores are globally visible.
///
/// # Safety
///
/// `ptr` must be 32-byte aligned, valid for writes of `len` bytes, and
/// `len` a multiple of 32 (enforced by [`avx2_write`]'s debug asserts and
/// documented precondition); every store writes strictly inside that span.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_write_simd(ptr: *mut u8, len: usize, pattern: u64) -> u64 {
    // SAFETY: the caller (`avx2_write`) guarantees `ptr` is 32-byte
    // aligned, valid for writes of `len` bytes, with `len % 32 == 0`;
    // each `_mm256_stream_si256` below therefore writes one 32-byte chunk
    // strictly inside the valid, alignment-correct span.
    let base = ptr as *mut __m256i;
    let word = pattern as i64;
    let v = _mm256_set_epi64x(word, word, word, word);
    let chunks = len / 32;
    let mut i = 0;
    // Unrolled main loop: four non-temporal stores per iteration.
    while i + 4 <= chunks {
        _mm256_stream_si256(base.add(i), v);
        _mm256_stream_si256(base.add(i + 1), v);
        _mm256_stream_si256(base.add(i + 2), v);
        _mm256_stream_si256(base.add(i + 3), v);
        i += 4;
    }
    // Tail: the remaining 0-3 chunks.
    while i < chunks {
        _mm256_stream_si256(base.add(i), v);
        i += 1;
    }
    // Store fence: make every non-temporal store above globally visible
    // before the caller proceeds (placed after the last store, per plan).
    _mm_sfence();
    len as u64
}

/// Safe scalar fallback filling the buffer with contents identical to
/// [`avx2_write_simd`]: repeated little-endian copies of the 64-bit
/// `pattern` word (a final partial word, when the length is not a
/// multiple of 8, is truncated).
///
/// Works on *any* slice (no alignment precondition); used both as the
/// dispatch fallback and as the reference implementation in the tests.
fn scalar_write(dst: &mut [u8], pattern: u64) {
    let word = pattern.to_le_bytes();
    let (head, tail) = dst.split_at_mut(dst.len() - dst.len() % 8);
    for chunk in head.chunks_exact_mut(8) {
        chunk.copy_from_slice(&word);
    }
    if !tail.is_empty() {
        tail.copy_from_slice(&word[..tail.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    /// Arbitrary fixed pattern for the content tests.
    const PATTERN: u64 = 0xA50F_1234_5678_9ABC;

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

    /// (a) A 4 KiB zeroed, 32-byte-aligned buffer is fully filled with
    /// the pattern and does not panic — including a direct run of the
    /// SIMD body on an AVX2 host.
    #[test]
    fn pattern_fills_4kib_buffer_without_panicking() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        let want = expected(LEN, PATTERN);
        assert_eq!(avx2_write(slice, PATTERN), LEN as u64);
        // Spot-check several 32-byte chunks (first, second, middle, last).
        for off in [0usize, 32, LEN / 2, LEN - 32] {
            assert_eq!(&slice[off..off + 32], &want[off..off + 32]);
        }
        assert_eq!(&*slice, want.as_slice());
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx2 {
            slice.fill(0);
            // SAFETY: the precondition holds by construction of `slice`.
            let n = unsafe { avx2_write_simd(slice.as_mut_ptr(), LEN, PATTERN) };
            assert_eq!(n, LEN as u64);
            assert_eq!(&*slice, want.as_slice());
        }
        dealloc_aligned(ptr, LEN);
    }

    /// (b) The returned byte count is stable across two calls and
    /// non-zero (a deterministic DCE-defeating counter).
    #[test]
    fn return_value_is_stable_and_nonzero() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        let first = avx2_write(slice, PATTERN);
        let second = avx2_write(slice, PATTERN);
        assert_ne!(first, 0, "a non-empty buffer must report a non-zero byte count");
        assert_eq!(first, second, "the byte count must be stable across calls");
        assert_eq!(first, LEN as u64);
        dealloc_aligned(ptr, LEN);
    }

    /// (c) The minimal 32-byte buffer (a single 256-bit non-temporal
    /// store) works.
    #[test]
    fn minimal_32_byte_buffer() {
        const LEN: usize = 32;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        assert_eq!(avx2_write(slice, PATTERN), LEN as u64);
        let word = PATTERN.to_le_bytes();
        assert_eq!(&slice[0..8], &word[..], "first 64-bit word must be the LE pattern");
        assert_eq!(&*slice, expected(LEN, PATTERN).as_slice());
        dealloc_aligned(ptr, LEN);
    }

    /// (d) The scalar fallback is directly callable and fills the same
    /// buffer with exactly the same contents as the dispatched (SIMD on
    /// an AVX2 host) path.
    #[test]
    fn scalar_fallback_matches_simd_contents() {
        const LEN: usize = 4 * 1024;
        let ptr = aligned_zeroed(LEN);
        // SAFETY: `ptr` is valid for `LEN` writes (fresh allocation).
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, LEN) };
        let want = expected(LEN, PATTERN);
        avx2_write(slice, PATTERN); // dispatched path (SIMD on AVX2 hosts)
        assert_eq!(&*slice, want.as_slice());
        scalar_write(slice, PATTERN); // same buffer via the safe fallback
        assert_eq!(&*slice, want.as_slice());
        #[cfg(target_arch = "x86_64")]
        if CpuFeatures::detect().avx2 {
            slice.fill(0);
            // SAFETY: the precondition holds by construction of `slice`.
            unsafe { avx2_write_simd(slice.as_mut_ptr(), LEN, PATTERN) };
            assert_eq!(&*slice, want.as_slice());
        }
        dealloc_aligned(ptr, LEN);
    }
}
