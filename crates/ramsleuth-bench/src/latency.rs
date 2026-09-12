//! Pointer-chase latency kernel.
//!
//! **Chunk P1-09 — [COUPLED-TO: P1-03].** Builds a prefetcher-defeating
//! pointer-chase *ring* and measures the latency of its dependent
//! traversal.
//!
//! The ring is a **single cycle** of `N = size_bytes / 64` slots: slot `i`
//! stores the index of the *next* hop, so following `ring[i]` from any start
//! (e.g. `0`) visits every slot in `0..N` exactly once and then returns to
//! the start. When the orchestrator (P1-10) materializes slot `i` at
//! physical offset `64 * i` of the [`BufferPlan.latency_ring`] (128 MiB)
//! buffer, consecutive hops are exactly one cache line (64 B) apart and land
//! in pseudo-random order — defeating hardware stream/spatial prefetchers so
//! every hop is a true dependent memory access (L1/L3/DRAM latency, not
//! prefetch latency).
//!
//! This chunk is **index-ring + measurement only**: it allocates the compact
//! `Vec<usize>` ring and provides the timing kernel. The 128 MiB physical
//! buffer is sized by P1-03 ([`BufferPlan.latency_ring`]) and allocated by the
//! orchestrator — not here.
//!
//! [`BufferPlan`]: crate::buffers::BufferPlan
//!
//! ## Measurement
//! [`chase_latency_ns`] walks the ring as a strictly dependent load chain
//! (`idx = ring[idx]` — each load's address is fixed by the previous load's
//! data, so the CPU cannot overlap or prefetch hops) and times it:
//!
//! - **x86_64:** serialized [`core::arch::x86_64::__rdtscp`] at start and
//!   end. `RDTSCP` waits for every preceding instruction (the full dependent
//!   chain) to complete before reading the TSC, so the delta is the true
//!   chain time in TSC cycles. The cycle count is converted to ns by
//!   *self-calibrating* against `std::time::Instant` over the same run — no
//!   hardcoded TSC frequency, robust to power/thermal scaling of the
//!   invariant TSC.
//! - **non-x86_64:** `std::time::Instant` only — **reduced precision** (no
//!   cycle counter, coarser clock), but still a valid dependent-chase
//!   latency estimate.
//!
//! The hop count is `max(ring.len(), 1_000_000)`: always at least one full
//! traversal of the cycle, and at least a million hops for a stable average.

use std::hint::black_box;
use std::time::Instant;

/// One cache line: the physical chase stride between consecutive hops (bytes).
///
/// This is a property of how the orchestrator *materializes* the ring (slot
/// `i` at offset `STRIDE_BYTES * i`), not of the compact `Vec<usize>` ring
/// this module returns.
pub const STRIDE_BYTES: usize = 64;

/// Minimum number of chase hops for a stable average.
///
/// [`chase_latency_ns`] performs `max(ring.len(), HOPS)` hops.
const HOPS: usize = 1_000_000;

/// Deterministic seed → 64-bit PRNG (SplitMix64).
///
/// SplitMix64 is tiny, fast, seedable, and statistically strong enough for a
/// Fisher-Yates shuffle. Seeding with the same `seed` always reproduces the
/// same ring (reproducible benchmarks).
struct SplitMix64(u64);

impl SplitMix64 {
    /// SplitMix64 increment / mixing constants.
    const INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;
    const MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;
    const MIX_B: u64 = 0x94D0_49BB_1331_11EB;

    /// Create PRNG state from `seed`.
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Advance the state and return the next pseudo-random 64-bit value.
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(Self::INCREMENT);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(Self::MIX_A);
        z = (z ^ (z >> 27)).wrapping_mul(Self::MIX_B);
        z ^ (z >> 31)
    }
}

/// Build the pointer-chase ring for a `size_bytes` chase buffer.
///
/// Returns a `Vec<usize>` of `N = size_bytes / 64` entries (one per
/// 64-byte cache-line slot). Entry `i` is the index of the **next** hop when
/// the chase is at slot `i`. The ring is a **single cycle** of length `N`:
/// following `ring[i]` from any start (e.g. `0`) visits every slot in `0..N`
/// exactly once and then returns to the start. The cycle order is a
/// deterministic SplitMix64-seeded Fisher-Yates permutation of `0..N`, so:
///
/// - the ring is reproducible (same `seed` → same ring);
/// - consecutive hops land in pseudo-random slot order, which — once the
///   orchestrator materializes slot `i` at physical offset `64 * i` — makes
///   every hop a 64-byte-stride dependent load that defeats prefetchers.
///
/// # Notes
/// - `size_bytes` is the *total* chase-buffer size in bytes; the slot count
///   is `size_bytes / 64` (truncated). If `size_bytes < 64` the result is an
///   empty ring (no slots).
/// - This returns the compact index ring only; the 128 MiB physical buffer
///   is the orchestrator's allocation (see the module docs).
pub fn build_chase_ring(size_bytes: usize, seed: u64) -> Vec<usize> {
    let n = size_bytes / STRIDE_BYTES;
    if n == 0 {
        return Vec::new();
    }

    // 1. Identity sequence `0..n` — one entry per cache-line slot.
    let mut seq: Vec<usize> = (0..n).collect();

    // 2. Deterministic Fisher-Yates shuffle (SplitMix64, seeded).
    let mut rng = SplitMix64::new(seed);
    for i in (1..n).rev() {
        let j = (rng.next_u64() % (i + 1) as u64) as usize;
        seq.swap(i, j);
    }

    // 3. Wire each position to the next (cyclically) → a single `n`-cycle.
    //    `seq[i]` is followed by `seq[(i+1) % n]`, ..., then back to `seq[i]`.
    let mut ring = vec![0; n];
    for i in 0..n {
        ring[seq[i]] = seq[(i + 1) % n];
    }
    ring
}

/// Measure the average nanoseconds per hop of a dependent pointer-chase over
/// `ring`.
///
/// `ring` must be a valid chase ring: every entry in `0..ring.len()`. (Rings
/// from [`build_chase_ring`] satisfy this by construction; a `debug_assert`
/// guards it.) The chase starts at slot `0` and performs
/// `max(ring.len(), 1_000_000)` strictly dependent hops
/// (`idx = ring[idx]`), so no hop can issue before the previous hop's data
/// has landed — that dependency chain *is* the latency being measured.
///
/// Returns the average **ns per hop** (`0.0` for an empty ring).
///
/// # Precision
/// - **x86_64:** `RDTSCP`-based cycle timing, self-calibrated to ns via
///   `Instant` over the same run (see the module docs). High precision.
/// - **non-x86_64:** `Instant`-only wall timing — **reduced precision** (no
///   cycle counter, coarser clock), but a valid dependent-chase estimate.
pub fn chase_latency_ns(ring: &[usize]) -> f64 {
    let n = ring.len();
    if n == 0 {
        return 0.0;
    }
    debug_assert!(
        ring.iter().all(|&i| i < n),
        "chase ring must reference only valid indices in [0, {n})"
    );

    // At least one full traversal of the cycle, and a stable minimum hop
    // count. `hops` is always `>= n` (a full pass) and `>= HOPS`.
    let hops = n.max(HOPS);
    chase(ring, hops)
}

/// Architecture-specific dependent-chase + timing core. Returns ns/hop.
fn chase(ring: &[usize], hops: usize) -> f64 {
    // x86_64: RDTSCP cycle timing, self-calibrated to ns.
    #[cfg(target_arch = "x86_64")]
    {
        let mut aux = 0u32;
        let mut idx = 0usize;

        // Start timestamp: RDTSCP waits for all preceding setup to complete.
        let t0 = unsafe {
            // SAFETY: `aux` is a valid, aligned stack `u32`; `__rdtscp`
            // writes exactly one 32-bit AUX value through this pointer.
            core::arch::x86_64::__rdtscp(&mut aux as *mut u32)
        };
        let w0 = Instant::now();

        // The dependent chase: each load's address depends on the previous
        // load's data, so hops serialize and cannot be prefetched/overlapped.
        //
        // SAFETY: `idx` stays in `[0, ring.len())` — it starts at `0` and is
        // only ever reassigned to `ring[idx]`, and every ring entry is a
        // valid index (checked by the caller's `debug_assert`).
        // `get_unchecked` performs the same read as `ring[idx]` minus the
        // bounds check, keeping the hot loop minimal.
        let _ = unsafe {
            for _ in 0..hops {
                idx = *ring.get_unchecked(idx);
            }
            // `black_box` defeats dead-store elimination of the chain.
            black_box(idx)
        };

        // End timestamp: RDTSCP waits for the entire dependent chain (all
        // `hops` loads) to complete before reading the TSC.
        let t1 = unsafe {
            // SAFETY: same contract as the start-timestamp `__rdtscp` call.
            core::arch::x86_64::__rdtscp(&mut aux as *mut u32)
        };
        let w1 = Instant::now();

        let cycles = t1.wrapping_sub(t0);
        let wall_ns = w1.duration_since(w0).as_secs_f64() * 1e9;
        if cycles == 0 || wall_ns <= 0.0 {
            return 0.0;
        }
        // Self-calibrate: TSC cycles per ns over this run (scaling-robust).
        let cycles_per_ns = cycles as f64 / wall_ns;
        // ns per hop = (avg cycles per hop) / (cycles per ns).
        cycles as f64 / hops as f64 / cycles_per_ns
    }

    // non-x86_64: Instant-only fallback (reduced precision, documented).
    #[cfg(not(target_arch = "x86_64"))]
    {
        let mut idx = 0usize;
        let w0 = Instant::now();
        // SAFETY: same indexing contract as the x86_64 path above.
        let _ = unsafe {
            for _ in 0..hops {
                idx = *ring.get_unchecked(idx);
            }
            black_box(idx)
        };
        let w1 = Instant::now();
        let wall_ns = w1.duration_since(w0).as_secs_f64() * 1e9;
        if wall_ns <= 0.0 {
            return 0.0;
        }
        wall_ns / hops as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) 64 KiB ring → exactly 1024 entries, all in `[0, 1024)`, each value
    /// unique (a permutation), and reproducible for a fixed seed.
    #[test]
    fn build_chase_ring_64k_is_reproducible_permutation() {
        let ring = build_chase_ring(64 * 1024, 42);
        assert_eq!(ring.len(), 1024);
        for &i in &ring {
            assert!((0..1024).contains(&i), "index {i} out of range");
        }
        let mut seen = vec![false; 1024];
        for &i in &ring {
            assert!(!seen[i], "duplicate index {i}");
            seen[i] = true;
        }
        // Same seed → identical ring.
        assert_eq!(ring, build_chase_ring(64 * 1024, 42));
    }

    /// (b) Different seeds produce different rings.
    #[test]
    fn build_chase_ring_seed_dependence() {
        let a = build_chase_ring(1024 * 1024, 1);
        let b = build_chase_ring(1024 * 1024, 2);
        assert_ne!(a, b);
    }

    /// (c) `chase_latency_ns` on a small ring returns a finite, positive
    /// value and does not panic. (No exact-ns assertion — hardware
    /// dependent.)
    #[test]
    fn chase_latency_ns_small_ring_is_finite_positive() {
        let ring = build_chase_ring(64 * 1024, 42);
        let ns = chase_latency_ns(&ring);
        assert!(ns.is_finite(), "ns {ns} not finite");
        assert!(ns > 0.0, "ns {ns} not positive");
    }

    /// (d) The ring is a *single* cycle: following `ring[i]` from `0` for
    /// `n` steps returns to `0` and visits every slot exactly once.
    #[test]
    fn build_chase_ring_is_single_cycle() {
        let n = 1024;
        let ring = build_chase_ring(n * 64, 7);
        assert_eq!(ring.len(), n);
        let mut visited = vec![false; n];
        let mut cur = 0usize;
        for _ in 0..n {
            assert!(!visited[cur], "slot {cur} visited twice → multi-cycle");
            visited[cur] = true;
            cur = ring[cur];
        }
        assert!(cur == 0, "chase from 0 must return to 0 after {n} steps");
        assert!(visited.iter().all(|&v| v), "every slot must be visited once");
    }

    /// Sub-line sizes yield an empty ring; a 64-byte ring yields one slot
    /// (self-loop, still a valid single cycle of length 1).
    #[test]
    fn build_chase_ring_edge_sizes() {
        assert!(build_chase_ring(0, 0).is_empty());
        assert!(build_chase_ring(63, 0).is_empty());
        let one = build_chase_ring(64, 0);
        assert_eq!(one, vec![0]);
    }

    /// The full public-API pair round-trips: build a ring, chase it.
    #[test]
    fn build_then_chase_does_not_panic() {
        let ring = build_chase_ring(256 * 1024, 1234);
        let ns = chase_latency_ns(&ring);
        assert!(ns.is_finite() && ns > 0.0);
    }
}
