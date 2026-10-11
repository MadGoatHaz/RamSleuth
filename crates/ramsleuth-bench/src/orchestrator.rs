//! Benchmark orchestrator & aggregation — runs all tiers × ops and fills
//! the 4×4 grid.
//!
//! **Chunk P1-10 — [COUPLED-TO: P1-08, P1-09].** Consumes the frozen
//! topology (P1-02), buffer sizing (P1-03), pinned worker dispatch
//! (P1-08), and pointer-chase ring builder (P1-09) to produce the full
//! [`BenchmarkGrid`]:
//!
//! |             | Read   | Write   | Copy   | Latency |
//! |-------------|--------|---------|--------|---------|
//! | Memory      | GB/s   | GB/s    | GB/s   | ns/hop  |
//! | L1          | GB/s   | GB/s    | GB/s   | ns/hop  |
//! | L2          | GB/s   | GB/s    | GB/s   | ns/hop  |
//! | L3          | GB/s   | GB/s    | GB/s   | ns/hop  |
//!
//! # Timing model
//!
//! - **Bandwidth (Read/Write/Copy):** `BENCH_ITERATIONS` [`run_pinned`]
//!   passes per tier, each wall-timed with [`Instant`]; the grid cell is
//!   `total_bytes / best_elapsed` — best-of-3, which trims the first
//!   run's cold start (page faults, TLB warmup, thread spin-up).
//! - **Latency:** the tier's chase buffer is **materialized** — slot `i`
//!   holds the ring entry as a little-endian `u64` at physical byte
//!   offset `64·i` — and chased as a strictly dependent load chain timed
//!   with serialized `__rdtscp` (x86_64), self-calibrated against
//!   `Instant`; non-x86_64 falls back to `Instant` only (reduced
//!   precision, documented). `BENCH_ITERATIONS` runs; the grid cell is
//!   the **median** ns/hop over a **fixed** [`CHASE_HOPS`] (1M) chase
//!   (BENCH-2: independent of the buffer size — a partial traversal of a
//!   large DRAM ring is fine for latency and avoids a ~30-minute full
//!   traversal). Chasing the materialized tier buffer (rather than
//!   P1-09's compact 8-byte-entry ring) puts the Memory-tier working set
//!   beyond L3 — true DRAM row latency, not an L3 one.
//!
//! # Buffers & safety
//!
//! Every tier buffer is a 64-byte-aligned window into owned `Vec`
//! storage (see [`AlignedBuf`]). The only `unsafe` in this module is the
//! two raw-pointer sites documented inline: the aligned-window views
//! and the dependent chase loads.

use std::fmt;
use std::hint::black_box;
use std::time::Instant;

use crate::buffers::plan;
use crate::latency::{build_chase_ring, STRIDE_BYTES};
use crate::topology::{CpuTopology, TopologyError, detect};
use crate::worker::{BenchOp, WorkerError, run_pinned};

/// Iterations per pass: bandwidth takes the best (min), latency the median.
const BENCH_ITERATIONS: usize = 3;

/// The fixed chase hop count for every tier (BENCH-2): a million hops.
/// A partial traversal of a large (DRAM-sized) ring is statistically
/// robust for latency; a full traversal of a 256–384 MiB ring (≈ 4M–6.3M
/// hops) would take ~30 minutes and make a 5-minute burn-in unable to
/// finish a pass. Cache tiers (rings < 1M) are unaffected by the floor.
const CHASE_HOPS: usize = 1_000_000;

/// The four benchmark tiers — the grid's row order.
///
/// The discriminants are the grid-array indices: `Memory = 0, L1 = 1,
/// L2 = 2, L3 = 3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tier {
    /// Main memory (DRAM).
    Memory,
    /// L1 cache.
    L1,
    /// L2 cache.
    L2,
    /// L3 cache.
    L3,
}

/// The four benchmark metrics — the grid's column order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Metric {
    /// Read bandwidth (GB/s).
    Read,
    /// Write bandwidth (GB/s).
    Write,
    /// Copy bandwidth (GB/s).
    Copy,
    /// Pointer-chase latency (ns/hop).
    Latency,
}

/// The 4×4 benchmark grid: four tiers × four metrics.
///
/// Every array is indexed by [`Tier`] (discriminant order
/// `Memory, L1, L2, L3`); [`BenchmarkGrid::cell`] is the lookup.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkGrid {
    /// Read bandwidth, GB/s, per tier.
    pub read_gbps: [f64; 4],
    /// Write bandwidth, GB/s, per tier.
    pub write_gbps: [f64; 4],
    /// Copy bandwidth, GB/s, per tier.
    pub copy_gbps: [f64; 4],
    /// Pointer-chase latency, ns/hop, per tier.
    pub latency_ns: [f64; 4],
}

impl BenchmarkGrid {
    /// Look up one cell: `metric` for `tier`.
    pub fn cell(&self, tier: Tier, metric: Metric) -> f64 {
        let slot = tier as usize;
        match metric {
            Metric::Read => self.read_gbps[slot],
            Metric::Write => self.write_gbps[slot],
            Metric::Copy => self.copy_gbps[slot],
            Metric::Latency => self.latency_ns[slot],
        }
    }
}

/// Failure of an orchestrator pass.
#[derive(Debug)]
pub enum OrchestratorError {
    /// CPU topology detection failed.
    Topology(TopologyError),
    /// A bandwidth pass (worker dispatch) failed.
    Worker(WorkerError),
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Topology(err) => write!(f, "cpu topology detection failed: {err}"),
            Self::Worker(err) => write!(f, "bandwidth pass failed: {err}"),
        }
    }
}

impl std::error::Error for OrchestratorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Topology(err) => Some(err),
            Self::Worker(err) => Some(err),
        }
    }
}

/// Detect the host topology, size every tier buffer via the frozen
/// [`plan`], and run all tiers × ops into the grid.
///
/// `use_avx512` selects the AVX-512 kernel family (the kernels carry
/// their own AVX2 fallback per P1-07).
///
/// # Errors
///
/// [`OrchestratorError`] when topology detection or any bandwidth pass
/// fails.
pub fn run_all(use_avx512: bool) -> Result<BenchmarkGrid, OrchestratorError> {
    let topo = detect().map_err(OrchestratorError::Topology)?;
    let plan = plan(&topo);
    // Tier order matches the `Tier` discriminants: Memory, L1, L2, L3.
    run_all_sized(&topo, [plan.dram, plan.l1, plan.l2, plan.l3], use_avx512)
}

/// Run the full grid for explicit per-tier byte sizes (`[Memory, L1,
/// L2, L3]`, the [`Tier`] order).
///
/// `pub(crate)`: the testable entry point behind [`run_all`] (tests
/// feed tiny sizes; the CLI feeds the `BufferPlan` sizes).
///
/// # Errors
///
/// [`OrchestratorError`] when any bandwidth pass fails.
pub(crate) fn run_all_sized(
    topo: &CpuTopology,
    sizes: [usize; 4],
    use_avx512: bool,
) -> Result<BenchmarkGrid, OrchestratorError> {
    let mut grid = BenchmarkGrid {
        read_gbps: [0.0; 4],
        write_gbps: [0.0; 4],
        copy_gbps: [0.0; 4],
        latency_ns: [0.0; 4],
    };
    let tiers = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];
    for (slot, &tier) in tiers.iter().enumerate() {
        let size = normalize_size(sizes[slot]);

        // Bandwidth buffers: two 64-aligned `size`-byte windows; `src`
        // carries a deterministic data pattern.
        let mut src = AlignedBuf::new(size);
        let mut dst = AlignedBuf::new(size);
        fill_pattern(src.as_mut_slice());

        grid.read_gbps[slot] =
            bench_bandwidth(topo, BenchOp::Read, tier, src.as_slice(), dst.as_mut_slice(), use_avx512)?;
        grid.write_gbps[slot] = bench_bandwidth(
            topo,
            BenchOp::Write,
            tier,
            src.as_slice(),
            dst.as_mut_slice(),
            use_avx512,
        )?;
        grid.copy_gbps[slot] =
            bench_bandwidth(topo, BenchOp::Copy, tier, src.as_slice(), dst.as_mut_slice(), use_avx512)?;

        // The latency pass needs a single buffer: release `dst` before
        // allocating the chase buffer to keep the per-tier peak at
        // ~2× size instead of 3×.
        drop(dst);
        grid.latency_ns[slot] = bench_latency(size, tier as u64 + 1);
    }
    Ok(grid)
}

/// Round `size` up to a 64-byte (cache-line) multiple, enforcing a
/// 64-byte minimum (one slot is the smallest usable chase unit).
///
/// `pub(crate)`: the streaming run (`streamed::run_streamed`, P3-09)
/// sizes per-tier windows with it.
pub(crate) fn normalize_size(size: usize) -> usize {
    let rem = size % STRIDE_BYTES;
    let aligned = if rem == 0 { size } else { size + STRIDE_BYTES - rem };
    aligned.max(STRIDE_BYTES)
}

/// A 64-byte-aligned window of exactly `size` bytes backed by owned
/// storage.
///
/// Safe alignment strategy: allocate `size + 64` zeroed bytes — at most
/// 64 bytes of documented waste (one extra cache line of padding that
/// guarantees a 64-byte-aligned sub-window exists for any `size`) — then
/// take the aligned window `[offset, offset + size)`: since `offset ≤ 63`,
/// `offset + size ≤ size + 64 = owner.len()`. The owning `Vec` keeps the
/// storage alive for the window's lifetime.
///
/// `pub(crate)`: the streaming run (`streamed::run_streamed`, P3-09)
/// allocates per-tier bandwidth windows with it.
pub(crate) struct AlignedBuf {
    owner: Vec<u8>,
    offset: usize,
    size: usize,
}

impl AlignedBuf {
    /// Allocate a zeroed `size`-byte window with a guaranteed 64-byte
    /// aligned base (see the struct docs for the padding strategy).
    pub(crate) fn new(size: usize) -> Self {
        let owner = vec![0u8; size.checked_add(STRIDE_BYTES).expect("aligned buffer size overflow")];
        let offset = (STRIDE_BYTES - (owner.as_ptr() as usize % STRIDE_BYTES)) % STRIDE_BYTES;
        debug_assert!(offset + size <= owner.len());
        Self { owner, offset, size }
    }

    /// The aligned window as a shared slice.
    pub(crate) fn as_slice(&self) -> &[u8] {
        // SAFETY: the window `[offset, offset + size)` lies inside
        // `owner` (the 64-byte pad guarantees `offset + size ≤
        // owner.len()`), its base is 64-byte aligned by construction,
        // `owner` keeps the storage alive for the view's lifetime, and a
        // shared view derived from `&self` aliases no other live view.
        unsafe { std::slice::from_raw_parts(self.owner.as_ptr().add(self.offset), self.size) }
    }

    /// The aligned window as a mutable slice.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as in [`AlignedBuf::as_slice`]; the exclusive
        // `&mut self` guarantees no other view of the window is live.
        unsafe { std::slice::from_raw_parts_mut(self.owner.as_mut_ptr().add(self.offset), self.size) }
    }
}

/// Fill `buf` (a whole number of 64-byte slots) with a deterministic,
/// data-dependent pattern: slot `i` holds the little-endian encoding of
/// `i` repeated in every 8-byte word. The read checksum is non-zero for
/// any buffer beyond the first slot, and the fill is reproducible.
///
/// `pub(crate)`: the streaming run (`streamed::run_streamed`, P3-09)
/// arms `src` with the same deterministic pattern.
pub(crate) fn fill_pattern(buf: &mut [u8]) {
    debug_assert!(buf.len() % STRIDE_BYTES == 0);
    for (slot, chunk) in buf.chunks_exact_mut(STRIDE_BYTES).enumerate() {
        let word = slot as u64;
        for word_bytes in chunk.chunks_exact_mut(8) {
            word_bytes.copy_from_slice(&word.to_le_bytes());
        }
    }
}

/// Run one Read/Write/Copy pass `BENCH_ITERATIONS` times over the tier's
/// aligned `src`/`dst` windows, wall-timing each with [`Instant`];
/// report GB/s as `total_bytes / best_elapsed` (the fastest run —
/// best-of-3 trims the first run's cold start).
///
/// `tier` selects the store policy for the `Write`/`Copy` ops (BENCH-1/3):
/// the cache tiers use cache-resident stores, the Memory tier uses
/// non-temporal stores. `Read` ignores it.
///
/// `pub(crate)`: the streaming run (`streamed::run_streamed`, P3-09)
/// reuses this best-of-3 bandwidth kernel.
pub(crate) fn bench_bandwidth(
    topo: &CpuTopology,
    op: BenchOp,
    tier: Tier,
    src: &[u8],
    dst: &mut [u8],
    use_avx512: bool,
) -> Result<f64, OrchestratorError> {
    let mut best_secs = f64::INFINITY;
    let mut bytes = 0u64;
    for _ in 0..BENCH_ITERATIONS {
        let t0 = Instant::now();
        let result =
            run_pinned(topo, op, tier, src, dst, use_avx512).map_err(OrchestratorError::Worker)?;
        best_secs = best_secs.min(t0.elapsed().as_secs_f64());
        bytes = result.total_bytes;
    }
    if bytes == 0 || best_secs <= 0.0 {
        return Ok(0.0);
    }
    Ok(bytes as f64 / best_secs / 1e9)
}

/// Latency pass for one tier: materialize the tier-sized chase buffer
/// (P1-09 ring, seed `tier_idx + 1`), then chase it
/// `BENCH_ITERATIONS` times and report the **median** ns/hop.
///
/// `pub(crate)`: the streaming run (`streamed::run_streamed`, P3-09)
/// reuses this median-of-3 latency kernel.
pub(crate) fn bench_latency(size: usize, seed: u64) -> f64 {
    let ring = build_chase_ring(size, seed);
    let buf = materialize_chase(&ring, size);
    // A fixed hop count (BENCH-2): independent of the ring length, so a
    // DRAM-sized ring is a partial (fast) traversal, not a ~30-minute
    // full one.
    let hops = chase_hops(ring.len());
    let mut samples = [0.0f64; BENCH_ITERATIONS];
    for sample in &mut samples {
        *sample = chase_materialized(buf.as_slice(), hops).0;
    }
    samples.sort_by(f64::total_cmp);
    samples[BENCH_ITERATIONS / 2]
}

/// The number of chase hops for a ring of `n` slots (BENCH-2): the fixed
/// [`CHASE_HOPS`] for every tier, **independent of `n`**. Pre-fix this was
/// `n.max(CHASE_HOPS)`, so a DRAM-sized ring (≈ 4M–6.3M slots) chased a
/// *full* traversal (~30 minutes); a partial traversal of a large ring is
/// statistically robust for latency, and a small cache ring (< 1M slots)
/// is floored to the same million hops either way.
pub(crate) fn chase_hops(_n: usize) -> usize {
    CHASE_HOPS
}

/// Materialize the compact chase `ring` into a 64-byte-aligned
/// `size`-byte buffer: slot `i` (byte offset `64·i`) holds `ring[i]` as
/// a little-endian `u64` in its first 8 bytes (the rest of the line
/// stays zero). A dependent chase over this buffer then hops 64 bytes
/// apart in pseudo-random order — the true DRAM/cache-line latency, not
/// the compact ring's 8-byte-entry latency.
fn materialize_chase(ring: &[usize], size: usize) -> AlignedBuf {
    let slots = size / STRIDE_BYTES;
    debug_assert_eq!(ring.len(), slots, "ring must have one entry per slot");
    let mut buf = AlignedBuf::new(size);
    for (i, chunk) in buf.as_mut_slice().chunks_exact_mut(STRIDE_BYTES).enumerate() {
        debug_assert!(ring[i] < slots);
        chunk[..8].copy_from_slice(&ring[i].to_le_bytes());
    }
    buf
}

/// The strictly dependent chase over a materialized buffer: each hop
/// reads the LE `u64` at slot `idx` (byte offset `64·idx`) as the next
/// index. Every load's address is fixed by the previous load's data, so
/// the CPU cannot overlap or prefetch hops.
///
/// Returns the final slot index after `hops`, or `None` if a corrupted
/// slot index took the chase out of bounds (BENCH-6). The **release**
/// path bound-checks `idx < slots` before every dereference, so a
/// corrupted ring degrades to a graceful early-exit (a non-counted
/// reading in [`chase_materialized`]) instead of silent out-of-bounds UB;
/// the `get_unchecked` is kept only behind that verified invariant.
fn chase_loop(buf: &[u8], hops: usize) -> Option<usize> {
    debug_assert!(
        buf.len() % STRIDE_BYTES == 0,
        "chase buffer must hold whole {STRIDE_BYTES}-byte slots"
    );
    if buf.is_empty() {
        return Some(0); // no slots: nothing to follow
    }
    let slots = buf.len() / STRIDE_BYTES;
    // SAFETY: `buf` is a 64-byte-aligned window of owned, initialized
    // storage with a length that is a multiple of 64, so reinterpreting
    // its bytes as `buf.len() / 8` `u64` words is a valid, 8-aligned,
    // in-bounds view of the same storage (no other live references).
    let words = unsafe {
        std::slice::from_raw_parts(buf.as_ptr().cast::<u64>(), buf.len() / 8)
    };
    let mut idx = 0usize;
    for _ in 0..hops {
        // Release-path bound check (BENCH-6): a corrupted ring entry
        // (`idx >= slots`) would make `8 * idx` out of bounds. Check it
        // explicitly and stop the chase (graceful early-exit) instead of
        // dereferencing out of bounds (silent UB). `words.len() ==
        // 8 * slots`, so `idx < slots` ⇒ `8 * idx < words.len()`.
        if idx >= slots {
            return None;
        }
        // SAFETY: `idx < slots` (checked above) ⇒ `8 * idx < words.len()`,
        // so `get_unchecked(8 * idx)` is in bounds.
        // `to_ne_bytes` + `from_le_bytes` normalizes the word to its
        // little-endian value on both endiannesses.
        let word = u64::from_le_bytes(unsafe { words.get_unchecked(8 * idx).to_ne_bytes() });
        idx = word as usize;
    }
    Some(black_box(idx))
}

/// Chase a materialized buffer and return `(ns/hop, final slot index)`.
///
/// - **x86_64:** serialized `__rdtscp` brackets the chain; the cycle
///   delta converts to ns by self-calibrating against `Instant` over the
///   same run (no hardcoded TSC frequency; scaling-robust).
/// - **non-x86_64:** `Instant` only — reduced precision (no cycle
///   counter, coarser clock), but still a valid dependent-chase
///   latency estimate.
fn chase_materialized(buf: &[u8], hops: usize) -> (f64, usize) {
    if hops == 0 {
        return (0.0, 0);
    }
    // x86_64: RDTSCP cycle timing, self-calibrated to ns.
    #[cfg(target_arch = "x86_64")]
    {
        let mut aux = 0u32;
        let t0 = unsafe {
            // SAFETY: `aux` is a valid, aligned stack `u32`; `__rdtscp`
            // writes exactly one 32-bit AUX value through this pointer.
            core::arch::x86_64::__rdtscp(&mut aux as *mut u32)
        };
        let w0 = Instant::now();
        let final_idx = match chase_loop(buf, hops) {
            Some(idx) => idx,
            // A corrupted ring took the chase out of bounds: degrade to a
            // non-counted reading (0.0), not silent OOB UB (BENCH-6).
            None => return (0.0, 0),
        };
        let t1 = unsafe {
            // SAFETY: same contract as the start-timestamp `__rdtscp` call.
            core::arch::x86_64::__rdtscp(&mut aux as *mut u32)
        };
        let w1 = Instant::now();

        let cycles = t1.wrapping_sub(t0);
        let wall_ns = w1.duration_since(w0).as_secs_f64() * 1e9;
        if cycles == 0 || wall_ns <= 0.0 {
            (0.0, final_idx)
        } else {
            // Self-calibrate: TSC cycles per ns over this run.
            let cycles_per_ns = cycles as f64 / wall_ns;
            // ns/hop = (average cycles per hop) / (cycles per ns).
            (cycles as f64 / hops as f64 / cycles_per_ns, final_idx)
        }
    }

    // non-x86_64: Instant-only fallback (reduced precision, documented).
    #[cfg(not(target_arch = "x86_64"))]
    {
        let w0 = Instant::now();
        let final_idx = match chase_loop(buf, hops) {
            Some(idx) => idx,
            None => return (0.0, 0),
        };
        let w1 = Instant::now();
        let wall_ns = w1.duration_since(w0).as_secs_f64() * 1e9;
        (if wall_ns <= 0.0 { 0.0 } else { wall_ns / hops as f64 }, final_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic small topology: 2 physical cores (4 SMT logical CPUs).
    fn synthetic_topo() -> CpuTopology {
        CpuTopology {
            physical_cores: vec![0, 1],
            logical_cpus: vec![0, 1, 2, 3],
            total_l3_bytes: 8 * 1024 * 1024,
            ccd_l3_bytes: 4 * 1024 * 1024,
            has_smt: true,
        }
    }

    /// (a) `run_all_sized` with a synthetic topology and tiny sizes
    /// (4 KiB per tier — never the 128 MiB+ DRAM run) completes without
    /// panicking and fills all 16 cells with finite, non-negative values,
    /// for both kernel families.
    #[test]
    fn run_all_sized_tiny_grid_is_finite_and_non_negative() {
        let topo = synthetic_topo();
        for use_avx512 in [false, true] {
            let grid = run_all_sized(&topo, [4096, 4096, 4096, 4096], use_avx512)
                .unwrap_or_else(|e| panic!("run_all_sized (avx512={use_avx512}) failed: {e}"));
            for tier in [Tier::Memory, Tier::L1, Tier::L2, Tier::L3] {
                for metric in [Metric::Read, Metric::Write, Metric::Copy, Metric::Latency] {
                    let cell = grid.cell(tier, metric);
                    assert!(
                        cell.is_finite(),
                        "avx512={use_avx512} {tier:?}/{metric:?} = {cell} not finite"
                    );
                    assert!(
                        cell >= 0.0,
                        "avx512={use_avx512} {tier:?}/{metric:?} = {cell} negative"
                    );
                }
            }
        }
    }

    /// (b) The safe aligned-buffer helper yields a window whose base is
    /// 64-byte aligned and whose length equals the requested size.
    #[test]
    fn aligned_window_is_64_byte_aligned_with_exact_length() {
        for size in [64usize, 65, 128, 4096, 3 * 64 + 32, 1024 * 1024] {
            let buf = AlignedBuf::new(size);
            let slice = buf.as_slice();
            assert_eq!(slice.len(), size, "window length must be {size}");
            assert_eq!(
                slice.as_ptr() as usize % STRIDE_BYTES,
                0,
                "window base must be 64-byte aligned (size {size})"
            );
        }
    }

    /// (c) The materialized chase buffer is a valid cycle: chasing
    /// exactly N hops from slot 0 returns to slot 0, every slot holds
    /// its ring entry as a LE u64, and the measured ns/hop is finite and
    /// positive.
    #[test]
    fn materialized_chase_is_a_valid_cycle() {
        let size = 4096; // 64 slots
        let slots = size / STRIDE_BYTES;
        let ring = build_chase_ring(size, 42);
        assert_eq!(ring.len(), slots);
        let buf = materialize_chase(&ring, size);

        // Content: slot `i` holds `ring[i]` as a little-endian u64.
        let bytes = buf.as_slice();
        for i in [0usize, 1, 5, slots - 1] {
            let start = STRIDE_BYTES * i;
            let word = u64::from_le_bytes(
                bytes[start..start + 8].try_into().expect("8-byte slot prefix"),
            );
            assert_eq!(word as usize, ring[i], "slot {i} must hold ring[{i}]");
        }

        let (ns, final_idx) = chase_materialized(bytes, slots);
        assert_eq!(
            final_idx, 0,
            "exactly {slots} hops from slot 0 must return to slot 0"
        );
        assert!(ns.is_finite(), "ns/hop {ns} not finite");
        assert!(ns > 0.0, "ns/hop {ns} not positive");
    }

    /// (d) `cell()` maps every (tier, metric) pair to the right slot of
    /// a hand-built grid.
    #[test]
    fn cell_returns_the_right_grid_values() {
        let grid = BenchmarkGrid {
            read_gbps: [1.0, 2.0, 3.0, 4.0],
            write_gbps: [5.0, 6.0, 7.0, 8.0],
            copy_gbps: [9.0, 10.0, 11.0, 12.0],
            latency_ns: [13.0, 14.0, 15.0, 16.0],
        };
        assert_eq!(grid.cell(Tier::Memory, Metric::Read), 1.0);
        assert_eq!(grid.cell(Tier::L1, Metric::Read), 2.0);
        assert_eq!(grid.cell(Tier::L2, Metric::Copy), 11.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Write), 8.0);
        assert_eq!(grid.cell(Tier::Memory, Metric::Latency), 13.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Latency), 16.0);
    }

    /// The grid layout depends on the discriminants being `Memory = 0,
    /// L1 = 1, L2 = 2, L3 = 3`.
    #[test]
    fn tier_discriminants_match_the_grid_layout() {
        assert_eq!(Tier::Memory as usize, 0);
        assert_eq!(Tier::L1 as usize, 1);
        assert_eq!(Tier::L2 as usize, 2);
        assert_eq!(Tier::L3 as usize, 3);
    }

    /// `normalize_size` rounds up to a 64-byte multiple with a 64-byte
    /// floor.
    #[test]
    fn normalize_size_rounds_up_with_64_byte_floor() {
        assert_eq!(normalize_size(0), 64);
        assert_eq!(normalize_size(1), 64);
        assert_eq!(normalize_size(63), 64);
        assert_eq!(normalize_size(64), 64);
        assert_eq!(normalize_size(65), 128);
        assert_eq!(normalize_size(4096), 4096);
        assert_eq!(normalize_size(4097), 4160);
    }

    /// `OrchestratorError` displays and wraps its cause.
    #[test]
    fn error_display_and_source_wrap_the_cause() {
        let worker = OrchestratorError::Worker(WorkerError::LengthMismatch { src: 1, dst: 2 });
        assert!(format!("{worker}").contains("bandwidth pass failed"));
        assert!(std::error::Error::source(&worker).is_some());

        let topo = OrchestratorError::Topology(TopologyError::NoCpus);
        assert!(format!("{topo}").contains("cpu topology detection failed"));
        assert!(std::error::Error::source(&topo).is_some());
    }

    /// (P3-08) A small representative `BenchmarkGrid` round-trips through
    /// bincode (plan D3: the wire codec) — the grid (and the `Tier`/
    /// `Metric` enums it indexes) is wire-serializable (P3-08 exit
    /// criterion). Bincode is bit-exact on `f64`, so each array compares
    /// equal after the round-trip.
    #[test]
    fn grid_bincode_round_trip() {
        let grid = BenchmarkGrid {
            read_gbps: [1.5, 2.5, 3.5, 4.5],
            write_gbps: [5.5, 6.5, 7.5, 8.5],
            copy_gbps: [9.5, 10.5, 11.5, 12.5],
            latency_ns: [13.5, 14.5, 15.5, 16.5],
        };
        let bytes = bincode::serialize(&grid).expect("BenchmarkGrid must serialize");
        let back: BenchmarkGrid =
            bincode::deserialize(&bytes).expect("BenchmarkGrid must deserialize");
        assert_eq!(back.read_gbps, grid.read_gbps);
        assert_eq!(back.write_gbps, grid.write_gbps);
        assert_eq!(back.copy_gbps, grid.copy_gbps);
        assert_eq!(back.latency_ns, grid.latency_ns);
    }

    /// TC-BENCH2 (BENCH-2): the latency hop count is the fixed
    /// `CHASE_HOPS` (1M) for every tier, independent of the ring length.
    /// Pre-fix it was `ring.len().max(CHASE_HOPS)`, so a DRAM-sized ring
    /// (≈ 4M–6.3M slots) chased a *full* traversal (~30 min) and a 5-min
    /// burn-in could never finish a pass; a small cache ring was floored
    /// to 1M. Now both are exactly 1M.
    #[test]
    fn tc_bench2_latency_hops_are_the_fixed_1m_floor() {
        // A DRAM-sized ring (384 MiB → ≈ 6.3M slots): the hop count is the
        // fixed 1M, NOT the ring length.
        let dram_ring_slots = (384 * 1024 * 1024) / STRIDE_BYTES;
        assert!(
            dram_ring_slots > CHASE_HOPS,
            "the DRAM ring ({dram_ring_slots} slots) is larger than 1M"
        );
        assert_eq!(
            chase_hops(dram_ring_slots),
            CHASE_HOPS,
            "a DRAM-sized ring must chase the fixed 1M hops, not a full traversal"
        );
        // A small cache ring (< 1M) is also the fixed 1M.
        assert_eq!(chase_hops(1024), CHASE_HOPS, "a small cache ring is floored to 1M");
        assert_eq!(chase_hops(0), CHASE_HOPS, "the hop count is independent of ring length");
    }

    /// TC-BENCH6 (BENCH-6): a corrupted ring (one slot index `>= slots`)
    /// makes the chase degrade to a non-counted `0.0` without panicking /
    /// UB — the release-path bound check (`idx < slots`) takes the
    /// early-exit. Pre-fix the release path used `get_unchecked` with only
    /// a `debug_assert`, so a corrupted ring was silent out-of-bounds UB.
    #[test]
    fn tc_bench6_corrupted_ring_degrades_to_zero() {
        let size = 4096; // 64 slots
        let slots = size / STRIDE_BYTES;
        // Build a valid chase buffer directly (bypassing
        // `materialize_chase`'s debug_assert so the corruption is visible
        // in both debug and release builds), then corrupt slot 0 to point
        // to `slots` (out of bounds).
        let ring = build_chase_ring(size, 42);
        let mut buf = AlignedBuf::new(size);
        for (i, chunk) in buf.as_mut_slice().chunks_exact_mut(STRIDE_BYTES).enumerate() {
            chunk[..8].copy_from_slice(&ring[i].to_le_bytes());
        }
        let bytes = buf.as_mut_slice();
        bytes[..8].copy_from_slice(&slots.to_le_bytes()); // slot 0 → OOB index
        // The chase must degrade to a non-counted 0.0, not panic/UB.
        let (ns, _) = chase_materialized(bytes, CHASE_HOPS);
        assert_eq!(ns, 0.0, "a corrupted ring degrades to a non-counted 0.0");
    }
}
