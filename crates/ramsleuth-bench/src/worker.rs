//! Pinned multi-threaded worker dispatch — one barrier-synced worker per
//! physical core.
//!
//! **Chunk P1-08 — [COUPLED-TO: P1-02, P1-04, P1-05, P1-06, P1-07].**
//! Consumes the frozen [`CpuTopology`] (P1-02; `physical_cores` is
//! already SMT-filtered, so one worker per entry is exactly one per
//! physical core) and the frozen bandwidth kernels (P1-04/P1-05/P1-06
//! AVX2; P1-07 AVX-512 upgrade path) to run a single Read/Write/Copy
//! pass over caller-provided aligned buffers.
//!
//! # Dispatch model
//!
//! 1. **Pre-flight pin probe** (Linux/Android only): the calling thread
//!    briefly pins itself to the first representative CPU via
//!    `libc::sched_setaffinity` and immediately restores its original
//!    affinity mask. If the probe fails — or the platform has no
//!    affinity syscall — the pass runs on a **single unpinned thread**
//!    over the whole buffer, with [`WorkerResult::pinned`] set to
//!    `false`.
//! 2. **Fan-out**: otherwise one `std::thread` per `physical_cores`
//!    entry. Each worker pins itself to its representative CPU *before*
//!    all workers cross a [`std::sync::Barrier`], so the kernel loops
//!    start in lockstep. A worker whose own pin fails (its CPU is
//!    outside the process's allowed set) continues **unpinned** and the
//!    aggregate [`WorkerResult::pinned`] reports `false`.
//! 3. **Work**: the buffer is partitioned into `n` contiguous,
//!    block-aligned slices (block = 32 B for AVX2, 64 B for AVX-512):
//!    workers `0..n-1` each receive an equal base chunk of
//!    `floor(total / n)` blocks and the **last worker receives the
//!    remainder tail**, so every byte is covered exactly once (see
//!    [`slice_boundaries`]). Each worker runs the requested kernel
//!    (`avx2_*` or `avx512_*` per `use_avx512`; the 512 kernels carry
//!    their own AVX2 fallback per P1-07) on its slice.
//! 4. **Aggregation**: the joiner sums the per-worker byte counters
//!    (which equals the full buffer length) and wrapping-sums the
//!    per-slice checksums. The word-sum checksum is an order-independent
//!    wrapping addition over 64-bit words and every slice is a whole
//!    number of blocks (hence of words), so the aggregate exactly equals
//!    the checksum of running the kernel over the whole buffer.
//!
//! # Caller contract (checked; violations are a [`WorkerError`], never UB)
//!
//! - `src.len() == dst.len()`;
//! - both bases aligned to the kernel block: **32 bytes** when
//!   `use_avx512 == false`, **64 bytes** when `true`;
//! - length a multiple of that block (an empty buffer is admitted and
//!   checksums to 0);
//! - for `Copy`, `src` and `dst` must not alias (the kernels' frozen
//!   precondition; the partition preserves disjointness).
//!
//! The caller supplies already-aligned buffers (e.g. allocated to
//! [`BufferPlan`](crate::buffers::BufferPlan) sizes, all 64-byte
//! aligned); nothing is allocated here.
//!
//! # No timing here
//!
//! Wall-clock measurement belongs to the orchestrator (P1-10). This
//! module moves data and reports bytes + checksum only; the checksum is
//! the dead-code-elimination sink, exactly as for the kernels.

use std::fmt;
use std::sync::Barrier;

use crate::kernel_512::{avx512_copy, avx512_read, avx512_write};
use crate::kernel_copy::avx2_copy;
use crate::kernel_read::avx2_read;
use crate::kernel_write::avx2_write;
use crate::topology::CpuTopology;

/// AVX2 kernel block: 32-byte alignment and length granularity
/// (frozen P1-04/P1-05/P1-06 preconditions).
const AVX2_BLOCK_BYTES: usize = 32;

/// AVX-512 kernel block: 64-byte alignment and length granularity
/// (frozen P1-07 preconditions).
const AVX512_BLOCK_BYTES: usize = 64;

/// Fixed pattern fed to the write kernel. Arbitrary constant: the write
/// kernels' frozen return is the byte counter, not a content checksum,
/// so the pattern value only matters for reproducing the stored bytes.
const WRITE_PATTERN: u64 = 0xA5A5_5AA5_5AA5_A55A;

/// Bandwidth operation a worker pool runs over the caller's buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BenchOp {
    /// Stream-read `src`; the checksum covers the read data.
    Read,
    /// Non-temporal write of a fixed pattern into `dst`; the checksum
    /// carries the write kernel's byte counter.
    Write,
    /// Copy `src` into `dst`; the checksum covers `dst` after the copy.
    Copy,
}

/// Aggregated outcome of one [`run_pinned`] pass over the full buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkerResult {
    /// The operation that was run.
    pub op: BenchOp,
    /// Total bytes moved: the sum over the workers' slices, which equals
    /// the full buffer length (the partition covers it exactly once).
    pub total_bytes: u64,
    /// Combined checksum. For `Read`/`Copy`: the wrapping sum of the
    /// per-slice word-sum checksums — exactly the checksum of running
    /// the kernel over the whole buffer. For `Write`: the byte counter
    /// (the write kernels' frozen return), so `checksum == total_bytes`.
    pub checksum: u64,
    /// `true` when **every** worker was pinned to its physical core via
    /// `sched_setaffinity`. `false` records that pinning was skipped:
    /// either the platform exposes no affinity syscall or the pre-flight
    /// probe failed (whole pass on a single unpinned thread), or one
    /// worker's CPU was outside the process's allowed set (that worker
    /// ran unpinned). The pass still succeeded; this only reports how it
    /// was scheduled.
    pub pinned: bool,
}

/// Failure of a [`run_pinned`] pass.
///
/// Pinning failure is deliberately **not** an error: an unpinned run
/// still completes and [`WorkerResult::pinned`] records the skip.
#[derive(Debug)]
pub enum WorkerError {
    /// `src` and `dst` have different lengths.
    LengthMismatch {
        /// Length of `src` in bytes.
        src: usize,
        /// Length of `dst` in bytes.
        dst: usize,
    },
    /// A buffer base is not aligned to the kernel's block.
    Misaligned {
        /// Which buffer: `"src"` or `"dst"`.
        buffer: &'static str,
        /// Required alignment (32 or 64 bytes).
        required: usize,
    },
    /// The buffer length is not a multiple of the kernel's block.
    NotBlockMultiple {
        /// The offending length in bytes.
        len: usize,
        /// The kernel block (32 or 64 bytes).
        block: usize,
    },
    /// A worker thread panicked while running its slice.
    WorkerPanic {
        /// Zero-based index of the worker that panicked.
        index: usize,
    },
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthMismatch { src, dst } => {
                write!(f, "src ({src} bytes) and dst ({dst} bytes) lengths differ")
            }
            Self::Misaligned { buffer, required } => {
                write!(f, "{buffer} base is not {required}-byte aligned")
            }
            Self::NotBlockMultiple { len, block } => write!(
                f,
                "buffer length {len} is not a multiple of the {block}-byte kernel block"
            ),
            Self::WorkerPanic { index } => write!(f, "worker {index} panicked"),
        }
    }
}

impl std::error::Error for WorkerError {}

/// Run one Read/Write/Copy pass over the whole buffer with one
/// barrier-synced worker per physical core. See the module docs for the
/// dispatch model, the caller contract, and the no-timing boundary.
///
/// # Errors
///
/// [`WorkerError`] when the caller violates the buffer contract (length
/// mismatch, misaligned base, non-block-multiple length) or when a
/// worker thread panics. Pinning failure is *not* an error: the pass
/// completes and [`WorkerResult::pinned`] records the skip.
pub fn run_pinned(
    topo: &CpuTopology,
    op: BenchOp,
    src: &[u8],
    dst: &mut [u8],
    use_avx512: bool,
) -> Result<WorkerResult, WorkerError> {
    let total = src.len();
    if total != dst.len() {
        return Err(WorkerError::LengthMismatch {
            src: total,
            dst: dst.len(),
        });
    }
    let block = if use_avx512 { AVX512_BLOCK_BYTES } else { AVX2_BLOCK_BYTES };
    if src.as_ptr() as usize % block != 0 {
        return Err(WorkerError::Misaligned {
            buffer: "src",
            required: block,
        });
    }
    if dst.as_ptr() as usize % block != 0 {
        return Err(WorkerError::Misaligned {
            buffer: "dst",
            required: block,
        });
    }
    if total % block != 0 {
        return Err(WorkerError::NotBlockMultiple { len: total, block });
    }

    // An empty buffer needs no kernel pass at all: report zero work and
    // no pinning (nothing ran) instead of feeding empty, possibly
    // unaligned slices into the kernels' alignment preconditions.
    if total == 0 {
        return Ok(WorkerResult {
            op,
            total_bytes: 0,
            checksum: 0,
            pinned: false,
        });
    }

    // Pre-flight probe: prove that affinity works (and that the first
    // representative CPU is inside the process's allowed set) before
    // fanning out. Any failure degrades the whole pass to a single
    // unpinned thread over the full buffer (still a success; `pinned`
    // records the skip). An empty topology has nothing to pin to either.
    let cores = &topo.physical_cores;
    let pin_usable = match cores.first().copied() {
        Some(cpu) => preflight_pin(cpu),
        None => false,
    };
    let n_workers = if pin_usable { cores.len() } else { 1 };

    // Contiguous block-aligned partition: equal base chunks for workers
    // 0..n-1, remainder tail on the last worker. Sliced sequentially
    // with `split_at(_mut)` so disjointness is proven by construction.
    let bounds = slice_boundaries(total, block, n_workers);
    let mut src_slices: Vec<&[u8]> = Vec::with_capacity(n_workers);
    let mut dst_slices: Vec<&mut [u8]> = Vec::with_capacity(n_workers);
    let mut src_rest = src;
    let mut dst_rest = dst;
    for &(_, len) in &bounds {
        let (slice, src_next) = src_rest.split_at(len);
        src_slices.push(slice);
        src_rest = src_next;
        let (slice, dst_next) = dst_rest.split_at_mut(len);
        dst_slices.push(slice);
        dst_rest = dst_next;
    }
    debug_assert!(src_rest.is_empty() && dst_rest.is_empty());

    // One scoped thread per worker: scoped threads may borrow the
    // (non-`'static`) slices, and the scope joins them at the end.
    let barrier = Barrier::new(n_workers);
    let outcomes = std::thread::scope(|s| -> Result<Vec<(bool, u64, u64)>, WorkerError> {
        let mut handles = Vec::with_capacity(n_workers);
        for (i, (src_slice, dst_slice)) in
            src_slices.into_iter().zip(dst_slices).enumerate()
        {
            let pin_target = if pin_usable { Some(cores[i]) } else { None };
            let gate = &barrier;
            handles.push(s.spawn(move || {
                // Pin before the gate so every worker sits on its own
                // physical core when the barrier releases the kernel
                // loops in lockstep. A failed pin (that CPU outside the
                // process's allowed set) does not abort the worker — it
                // runs unpinned and the aggregate `pinned` records it.
                let pinned = match pin_target {
                    Some(cpu) => pin_to_cpu(cpu),
                    None => false,
                };
                gate.wait();
                let (bytes, checksum) = run_kernel(op, src_slice, dst_slice, use_avx512);
                (pinned, bytes, checksum)
            }));
        }
        let mut outcomes = Vec::with_capacity(n_workers);
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.join() {
                Ok(outcome) => outcomes.push(outcome),
                Err(_) => return Err(WorkerError::WorkerPanic { index: i }),
            }
        }
        Ok(outcomes)
    })?;

    let mut total_bytes = 0u64;
    let mut checksum = 0u64;
    let mut pinned = true;
    for (worker_pinned, bytes, slice_checksum) in outcomes {
        total_bytes = total_bytes.wrapping_add(bytes);
        checksum = checksum.wrapping_add(slice_checksum);
        pinned &= worker_pinned;
    }
    debug_assert_eq!(
        total_bytes,
        total as u64,
        "the partition must cover the buffer exactly once"
    );
    Ok(WorkerResult {
        op,
        total_bytes,
        checksum,
        pinned,
    })
}

/// Run one bandwidth kernel over one worker's slice; returns
/// `(bytes moved, checksum)`.
///
/// - `Read`: bytes = slice length; checksum = the kernel's word sum.
/// - `Write`: bytes = checksum = the kernel's byte counter (the write
///   kernels' frozen return is `dst.len()`, not a content checksum).
/// - `Copy`: bytes = slice length; checksum = the kernel's word sum of
///   `dst` after the copy.
fn run_kernel(op: BenchOp, src: &[u8], dst: &mut [u8], use_avx512: bool) -> (u64, u64) {
    match op {
        BenchOp::Read => {
            let checksum = if use_avx512 { avx512_read(src) } else { avx2_read(src) };
            (src.len() as u64, checksum)
        }
        BenchOp::Write => {
            let written = if use_avx512 {
                avx512_write(dst, WRITE_PATTERN)
            } else {
                avx2_write(dst, WRITE_PATTERN)
            };
            (written, written)
        }
        BenchOp::Copy => {
            let checksum = if use_avx512 { avx512_copy(src, dst) } else { avx2_copy(src, dst) };
            (src.len() as u64, checksum)
        }
    }
}

/// Split `total` bytes into `n_workers` contiguous slices of
/// `(offset, len)`, each `block`-aligned and a whole number of blocks:
/// workers `0..n-1` get `floor(total / n)` blocks each and the last
/// worker gets the remainder tail, so the slices partition the buffer
/// exactly once.
///
/// # Panics
///
/// Debug builds assert `total % block == 0` and `n_workers >= 1`.
fn slice_boundaries(total: usize, block: usize, n_workers: usize) -> Vec<(usize, usize)> {
    debug_assert!(total % block == 0, "slice_boundaries: total must be a block multiple");
    debug_assert!(n_workers >= 1, "slice_boundaries: need at least one worker");
    let units = total / block;
    let base_units = units / n_workers;
    let mut bounds = Vec::with_capacity(n_workers);
    let mut unit_offset = 0usize;
    for i in 0..n_workers {
        let unit_len = if i + 1 == n_workers {
            units - unit_offset // remainder tail on the last worker
        } else {
            base_units
        };
        bounds.push((unit_offset * block, unit_len * block));
        unit_offset += unit_len;
    }
    bounds
}

/// Pin the calling thread to one CPU; `true` on success.
///
/// Platforms whose `libc` has no `sched_setaffinity` report failure so
/// the caller degrades to a single unpinned thread.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn pin_to_cpu(_cpu: usize) -> bool {
    false
}

/// Size of a Linux `cpu_set_t` in bytes (128: 1024 CPU bits).
#[cfg(any(target_os = "linux", target_os = "android"))]
const CPU_SET_BYTES: usize = std::mem::size_of::<libc::cpu_set_t>();

/// Set bit `cpu` in a raw Linux CPU-set byte buffer.
///
/// A CPU set is an array of 64-bit words with bit `bit` placed
/// `word << bit` in *native* endianness; this maps that placement onto
/// the raw bytes so the kernel reads the correct bit on either
/// endianness.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn set_cpu_bit(mask: &mut [u8; CPU_SET_BYTES], cpu: usize) {
    let word = cpu / 64;
    let bit = cpu % 64;
    let (byte_idx, bit_idx) = if cfg!(target_endian = "little") {
        (word * 8 + bit / 8, bit % 8)
    } else {
        (word * 8 + 7 - bit / 8, 7 - bit % 8)
    };
    mask[byte_idx] |= 1u8 << bit_idx;
}

/// Pin the calling thread to one CPU; `true` on success.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn pin_to_cpu(cpu: usize) -> bool {
    // The kernel's CPU set holds this many bits; a larger index cannot
    // be encoded and is reported as a pin failure (no real host has
    // more than 1024 CPUs).
    if cpu >= CPU_SET_BYTES * 8 {
        return false;
    }
    let mut mask = [0u8; CPU_SET_BYTES];
    set_cpu_bit(&mut mask, cpu);
    // SAFETY: `0` addresses the calling thread; `mask` is exactly the
    // size of a Linux `cpu_set_t` — the kernel's opaque CPU-bit-array
    // ABI — and is passed as a raw pointer. No Rust reference to the
    // struct is ever created, so the type's (possibly private) field
    // layout is irrelevant.
    unsafe {
        libc::sched_setaffinity(0, CPU_SET_BYTES, mask.as_ptr().cast::<libc::cpu_set_t>())
            == 0
    }
}

/// Probe whether pinning works for `cpu`: pin the calling thread to it
/// and immediately restore its original affinity mask. Returns `true`
/// only if both syscalls succeeded.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn preflight_pin(_cpu: usize) -> bool {
    false
}

/// Probe whether pinning works for `cpu` (Linux/Android): pin the
/// calling thread to it, then restore the pre-probe affinity mask.
/// Returns `true` only if both syscalls succeeded.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn preflight_pin(cpu: usize) -> bool {
    let mut original = [0u8; CPU_SET_BYTES];
    // SAFETY: `0` is the calling thread; `original` is valid for
    // `CPU_SET_BYTES` bytes — the size of a Linux `cpu_set_t` — and is
    // passed as a raw pointer (no Rust reference is created).
    if unsafe {
        libc::sched_getaffinity(
            0,
            CPU_SET_BYTES,
            original.as_mut_ptr().cast::<libc::cpu_set_t>(),
        )
    } != 0
    {
        return false;
    }
    if !pin_to_cpu(cpu) {
        return false;
    }
    // Restore the pre-probe mask. If the restore itself fails the
    // calling thread stays restricted to `cpu` — still functional, but
    // we report pinning as unavailable so the pass degrades to the
    // single unpinned thread.
    // SAFETY: as above; `original` holds the mask read before the probe.
    unsafe {
        libc::sched_setaffinity(
            0,
            CPU_SET_BYTES,
            original.as_ptr().cast::<libc::cpu_set_t>(),
        ) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{CpuTopology, detect};
    use core::alloc::Layout;

    /// Allocate `len` zeroed bytes with guaranteed 64-byte alignment
    /// (satisfies both the AVX2 and AVX-512 preconditions at once).
    /// The caller must release it with [`dealloc_aligned`].
    fn aligned_zeroed(len: usize) -> *mut u8 {
        // SAFETY: `len` with a 64-byte (power-of-two) alignment is a
        // well-formed layout; the allocation is fresh and released by
        // the caller.
        unsafe { std::alloc::alloc_zeroed(Layout::from_size_align(len, 64).unwrap()) }
    }

    /// Release a pointer from [`aligned_zeroed`] with the matching layout.
    fn dealloc_aligned(ptr: *mut u8, len: usize) {
        // SAFETY: `ptr` came from `aligned_zeroed(len)` and is still
        // alive with unchanged length and alignment.
        unsafe { std::alloc::dealloc(ptr, Layout::from_size_align(len, 64).unwrap()) };
    }

    /// Independent reference checksum: wrapping sum of the little-endian
    /// 64-bit words (a final partial word zero-extended).
    fn word_sum(data: &[u8]) -> u64 {
        let mut sum = 0u64;
        for chunk in data.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u64::from_le_bytes(w));
        }
        sum
    }

    /// Deterministic, data-sensitive fill.
    fn fill_pattern(buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
    }

    /// `slice_boundaries` partitions any block-multiple length into
    /// contiguous, block-aligned slices that cover it exactly once —
    /// including lengths that do not divide evenly across the workers
    /// (tail on the last) and lengths smaller than the worker count
    /// (base chunk 0, everything on the tail worker).
    #[test]
    fn slice_boundaries_cover_the_buffer_exactly_once() {
        for n in [1usize, 2, 3, 5, 8, 16] {
            for total in [64usize, 128, 4096, 1024 * 1024, 64 * (n * 32 + 1), 64 * (n - 1) + 64] {
                let bounds = slice_boundaries(total, 64, n);
                assert_eq!(bounds.len(), n, "n workers, n slices");
                let mut offset = 0usize;
                for (i, (off, len)) in bounds.iter().enumerate() {
                    assert_eq!(*off, offset, "slices must be contiguous (worker {i}, n={n}, total={total})");
                    assert!(*off % 64 == 0, "slice bases must be 64-aligned");
                    assert!(*len % 64 == 0, "slice lengths must be block multiples");
                    offset += len;
                }
                assert_eq!(offset, total, "slices must cover the buffer exactly once (n={n}, total={total})");
            }
        }
    }

    /// (a)+(c): a 1 MiB aligned buffer, every op × both kernel families:
    /// no panic, `total_bytes` equals the full buffer length, the
    /// checksum matches the kernel's semantics, and the run is fully
    /// pinned on this Linux host.
    #[test]
    fn run_pinned_covers_buffer_once_per_op() {
        const LEN: usize = 1024 * 1024;
        let topo = detect().expect("detect() succeeds on the test host");
        assert!(!topo.physical_cores.is_empty());

        let src_ptr = aligned_zeroed(LEN);
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is a fresh 64-aligned allocation of LEN bytes;
        // the mutable view is moved into `fill_pattern` and dead before
        // the shared view below is created (no aliasing).
        let src = unsafe {
            let view = std::slice::from_raw_parts_mut(src_ptr, LEN);
            fill_pattern(view);
            std::slice::from_raw_parts(src_ptr, LEN)
        };
        let dst_view = || unsafe {
            // SAFETY: `dst_ptr` is a fresh 64-aligned allocation of LEN
            // bytes; every view is disjoint from `src` (the Copy
            // kernel's non-aliasing precondition).
            std::slice::from_raw_parts_mut(dst_ptr, LEN)
        };
        let expected = word_sum(src);

        for op in [BenchOp::Read, BenchOp::Write, BenchOp::Copy] {
            for use_avx512 in [false, true] {
                let res = run_pinned(&topo, op, src, dst_view(), use_avx512)
                    .unwrap_or_else(|e| panic!("run_pinned {op:?} (512={use_avx512}) failed: {e}"));
                assert_eq!(res.op, op);
                assert_eq!(
                    res.total_bytes,
                    LEN as u64,
                    "{op:?} 512={use_avx512}: whole buffer covered exactly once"
                );
                // (c) sched_setaffinity is available on Linux → the
                // pool must report itself as pinned.
                #[cfg(any(target_os = "linux", target_os = "android"))]
                assert!(
                    res.pinned,
                    "{op:?} 512={use_avx512}: pinning must succeed on a Linux host"
                );
                let expected_checksum = match op {
                    BenchOp::Read | BenchOp::Copy => expected,
                    BenchOp::Write => LEN as u64, // write checksum = byte counter
                };
                assert_eq!(
                    res.checksum,
                    expected_checksum,
                    "{op:?} 512={use_avx512}: combined checksum"
                );
            }
        }
        // The last pass was Copy: `dst` must be byte-identical to `src`.
        assert!(
            src.iter().zip(dst_view().iter()).all(|(a, b)| a == b),
            "Copy must reproduce src in dst"
        );
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (b): the aggregated Read checksum equals the kernel's checksum
    /// of the *whole* buffer — proving the slice partition has neither
    /// overlap nor gap (word sums are additive over an exact partition).
    #[test]
    fn read_aggregate_checksum_matches_whole_buffer() {
        const LEN: usize = 1024 * 1024;
        let topo = detect().expect("detect() succeeds on the test host");

        let src_ptr = aligned_zeroed(LEN);
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is a fresh 64-aligned allocation of LEN bytes;
        // the mutable view is dead before the shared view is created.
        let src = unsafe {
            let view = std::slice::from_raw_parts_mut(src_ptr, LEN);
            fill_pattern(view);
            std::slice::from_raw_parts(src_ptr, LEN)
        };
        let dst_view = || unsafe {
            // SAFETY: `dst_ptr` is a fresh 64-aligned allocation of LEN bytes.
            std::slice::from_raw_parts_mut(dst_ptr, LEN)
        };

        let r32 = run_pinned(&topo, BenchOp::Read, src, dst_view(), false).expect("Read/AVX2");
        assert_eq!(
            r32.checksum,
            avx2_read(src),
            "AVX2 aggregate must equal whole-buffer avx2_read"
        );
        assert_eq!(
            r32.checksum,
            word_sum(src),
            "AVX2 aggregate must equal the word-sum reference"
        );

        let r512 = run_pinned(&topo, BenchOp::Read, src, dst_view(), true).expect("Read/AVX-512");
        assert_eq!(
            r512.checksum,
            avx512_read(src),
            "AVX-512 aggregate must equal whole-buffer avx512_read"
        );
        // On a host without AVX-512F, `avx512_read` falls back to the
        // AVX2 kernel → both aggregates must agree either way.
        assert_eq!(r512.checksum, r32.checksum, "512-fallback aggregate must equal the AVX2 one");
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (d): a length that is NOT a clean multiple of the per-worker
    /// chunk size: the remainder tail lands on the last worker and the
    /// buffer is still covered exactly once (every op, both families).
    #[test]
    fn tail_buffer_is_covered_exactly_once() {
        let topo = detect().expect("detect() succeeds on the test host");
        let n = topo.physical_cores.len().max(1);

        for use_avx512 in [false, true] {
            let block = if use_avx512 { AVX512_BLOCK_BYTES } else { AVX2_BLOCK_BYTES };
            // `m % n != 0` for n > 1 → uneven split; base chunk is
            // `floor(m / n) == 32` blocks, tail worker gets 33.
            let m = n * 32 + 1;
            let len = m * block;
            if n > 1 {
                assert_ne!(
                    len % (32 * block),
                    0,
                    "test length must not divide evenly into per-worker chunks"
                );
            }
            let src_ptr = aligned_zeroed(len);
            let dst_ptr = aligned_zeroed(len);
            // SAFETY: `src_ptr` is a fresh 64-aligned allocation of len bytes;
            // the mutable view is dead before the shared view is created.
            let src = unsafe {
                let view = std::slice::from_raw_parts_mut(src_ptr, len);
                fill_pattern(view);
                std::slice::from_raw_parts(src_ptr, len)
            };
            let dst_view = || unsafe {
                // SAFETY: `dst_ptr` is a fresh 64-aligned allocation of len
                // bytes; every view is disjoint from `src`.
                std::slice::from_raw_parts_mut(dst_ptr, len)
            };
            let expected = word_sum(src);

            for op in [BenchOp::Read, BenchOp::Write, BenchOp::Copy] {
                let res = run_pinned(&topo, op, src, dst_view(), use_avx512)
                    .unwrap_or_else(|e| panic!("run_pinned {op:?} (512={use_avx512}) failed: {e}"));
                assert_eq!(
                    res.total_bytes,
                    len as u64,
                    "{op:?} 512={use_avx512}: tail handled, whole buffer covered once"
                );
                let expected_checksum = match op {
                    BenchOp::Read | BenchOp::Copy => expected,
                    BenchOp::Write => len as u64,
                };
                assert_eq!(
                    res.checksum,
                    expected_checksum,
                    "{op:?} 512={use_avx512}: combined checksum"
                );
            }
            assert!(
                src.iter().zip(dst_view().iter()).all(|(a, b)| a == b),
                "Copy must reproduce src in dst (512={use_avx512})"
            );
            dealloc_aligned(src_ptr, len);
            dealloc_aligned(dst_ptr, len);
        }
    }

    /// A topology with no physical cores degrades to the single unpinned
    /// thread; the pass still succeeds, fully covered.
    #[test]
    fn empty_topology_falls_back_to_single_unpinned_thread() {
        const LEN: usize = 1024 * 1024;
        let topo = CpuTopology {
            physical_cores: vec![],
            logical_cpus: vec![0],
            total_l3_bytes: 4096,
            ccd_l3_bytes: 4096,
            has_smt: false,
        };
        let src_ptr = aligned_zeroed(LEN);
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: `src_ptr` is a fresh 64-aligned allocation of LEN bytes;
        // the mutable view is dead before the shared view is created.
        let src = unsafe {
            let view = std::slice::from_raw_parts_mut(src_ptr, LEN);
            fill_pattern(view);
            std::slice::from_raw_parts(src_ptr, LEN)
        };
        let dst_view = || unsafe {
            // SAFETY: `dst_ptr` is a fresh 64-aligned allocation of LEN bytes.
            std::slice::from_raw_parts_mut(dst_ptr, LEN)
        };

        let res = run_pinned(&topo, BenchOp::Read, src, dst_view(), false).expect("fallback run");
        assert!(!res.pinned, "no physical cores → no pinning to report");
        assert_eq!(res.total_bytes, LEN as u64);
        assert_eq!(res.checksum, avx2_read(src), "fallback run must checksum the whole buffer");
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// Contract violations surface as `WorkerError` — never UB, even
    /// in release builds (the checks precede any kernel call).
    #[test]
    fn precondition_violations_are_rejected() {
        let topo = detect().expect("detect() succeeds on the test host");

        // Length mismatch.
        let a_ptr = aligned_zeroed(64);
        let b_ptr = aligned_zeroed(128);
        // SAFETY: fresh allocations of the stated sizes.
        let a = unsafe { std::slice::from_raw_parts(a_ptr, 64) };
        let b = unsafe { std::slice::from_raw_parts_mut(b_ptr, 128) };
        assert!(
            matches!(
                run_pinned(&topo, BenchOp::Read, a, b, false),
                Err(WorkerError::LengthMismatch { .. })
            )
        );

        // Misaligned base: a 127-byte window starting at byte 1 of a
        // 64-aligned 128-byte allocation.
        let c_ptr = aligned_zeroed(128);
        // SAFETY: the window lies inside the allocation; the Read kernel
        // is never reached (the error returns before any kernel call).
        let c_src = unsafe { std::slice::from_raw_parts(c_ptr.add(1), 127) };
        let c_dst = unsafe { std::slice::from_raw_parts_mut(c_ptr.add(1), 127) };
        assert!(
            matches!(
                run_pinned(&topo, BenchOp::Read, c_src, c_dst, false),
                Err(WorkerError::Misaligned { buffer: "src", .. })
            )
        );

        // Aligned bases but a length that is not a block multiple.
        let d_ptr = aligned_zeroed(128);
        let e_ptr = aligned_zeroed(128);
        // SAFETY: 120-byte windows inside the allocations; the Read
        // kernel is never reached.
        let d = unsafe { std::slice::from_raw_parts(d_ptr, 120) };
        let e = unsafe { std::slice::from_raw_parts_mut(e_ptr, 120) };
        assert!(
            matches!(
                run_pinned(&topo, BenchOp::Read, d, e, false),
                Err(WorkerError::NotBlockMultiple { .. })
            )
        );

        dealloc_aligned(a_ptr, 64);
        dealloc_aligned(b_ptr, 128);
        dealloc_aligned(c_ptr, 128);
        dealloc_aligned(d_ptr, 128);
        dealloc_aligned(e_ptr, 128);
    }

    /// The pre-flight probe leaves the calling thread's affinity mask
    /// unchanged (it restores whatever it read).
    #[test]
    fn preflight_pin_restores_the_original_mask() {
        let topo = detect().expect("detect() succeeds on the test host");
        let cpu = topo.physical_cores[0];

        // If the probe's restore ever leaked, the mask would shrink to
        // the probed CPU and the pool's own pinning of the other
        // representatives would fail — verify the probe succeeds and
        // that a follow-up `run_pinned` still pins every worker.
        assert!(preflight_pin(cpu), "probe must succeed for a valid core");
        const LEN: usize = 64;
        let src_ptr = aligned_zeroed(LEN);
        let dst_ptr = aligned_zeroed(LEN);
        // SAFETY: fresh 64-aligned allocations of LEN bytes each; the
        // mutable view is dead before the shared view is created.
        let src = unsafe { std::slice::from_raw_parts(src_ptr, LEN) };
        let dst_view = || unsafe {
            // SAFETY: `dst_ptr` is a fresh 64-aligned allocation of LEN bytes.
            std::slice::from_raw_parts_mut(dst_ptr, LEN)
        };
        let res = run_pinned(&topo, BenchOp::Read, src, dst_view(), false).expect("post-probe run");
        assert!(res.pinned, "run must still be fully pinned after the probe");
        assert_eq!(res.total_bytes, LEN as u64);
        assert_eq!(res.checksum, 0, "zeroed buffer checksums to 0");
        dealloc_aligned(src_ptr, LEN);
        dealloc_aligned(dst_ptr, LEN);
    }

    /// (P3-07) Every `BenchOp` arm round-trips through bincode (the Phase 3
    /// frame codec, plan D3), proving the op tag is wire-serializable.
    #[test]
    fn bench_op_bincode_round_trip() {
        let ops = vec![BenchOp::Read, BenchOp::Write, BenchOp::Copy];
        let bytes = bincode::serialize(&ops).expect("BenchOp must serialize");
        let back: Vec<BenchOp> = bincode::deserialize(&bytes).expect("BenchOp must deserialize");
        assert_eq!(ops, back);
    }

    /// (P3-07) Representative `WorkerResult`s (one per op; mixed pinning;
    /// the write arm keeps its `checksum == total_bytes` invariant)
    /// round-trip through bincode - the aggregated pass outcome is
    /// wire-serializable (P3-07 exit criterion).
    #[test]
    fn worker_result_bincode_round_trip() {
        let results = vec![
            WorkerResult {
                op: BenchOp::Read,
                total_bytes: 1024,
                checksum: 0xDEAD_BEEF,
                pinned: true,
            },
            WorkerResult {
                op: BenchOp::Write,
                total_bytes: 65536,
                checksum: 65536,
                pinned: false,
            },
            WorkerResult {
                op: BenchOp::Copy,
                total_bytes: 0,
                checksum: 0,
                pinned: true,
            },
        ];
        let bytes = bincode::serialize(&results).expect("WorkerResult must serialize");
        let back: Vec<WorkerResult> =
            bincode::deserialize(&bytes).expect("WorkerResult must deserialize");
        assert_eq!(results, back);
    }
}
