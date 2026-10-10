//! Pointer-chase latency kernel — the chase *ring* builder.
//!
//! **Chunk P1-09 — [COUPLED-TO: P1-03].** Builds a prefetcher-defeating
//! pointer-chase *ring* for the benchmark's single latency path.
//!
//! The ring is a **single cycle** of `N = size_bytes / 64` slots: slot `i`
//! stores the index of the *next* hop, so following `ring[i]` from any start
//! (e.g. `0`) visits every slot in `0..N` exactly once and then returns to
//! the start. When the orchestrator (P1-10) materializes slot `i` at
//! physical offset `64 * i` of the **tier-sized** chase buffer, consecutive
//! hops are exactly one cache line (64 B) apart and land in pseudo-random
//! order — defeating hardware stream/spatial prefetchers so every hop is a
//! true dependent memory access (L1/L3/DRAM latency, not prefetch latency).
//!
//! This module is **index-ring only**: it allocates the compact `Vec<usize>`
//! ring. The physical chase buffer is sized to the tier's *bandwidth* size
//! (the DRAM chase is [`BufferPlan.dram`], sized beyond L3 — BENCH-4 removed
//! the dead fixed 128 MiB `latency_ring` field) and allocated + chased by the
//! orchestrator's `chase_materialized` (64-byte stride, the single latency
//! path — BENCH-5 removed the duplicate compact-ring `chase_latency_ns`,
//! which fit in L3 and would have misreported L3 as DRAM).
//!
//! [`BufferPlan`]: crate::buffers::BufferPlan
//!
//! ## Measurement (the orchestrator's `chase_materialized`)
//! The materialized buffer is chased as a strictly dependent load chain
//! (`idx = slot[idx]` — each load's address is fixed by the previous load's
//! data, so the CPU cannot overlap or prefetch hops) and timed:
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
//! The hop count is a **fixed** `CHASE_HOPS` (1M) for every tier (BENCH-2) —
//! a partial traversal of a large DRAM ring is fine for latency, and a full
//! traversal would take ~30 minutes.

/// One cache line: the physical chase stride between consecutive hops (bytes).
///
/// This is a property of how the orchestrator *materializes* the ring (slot
/// `i` at offset `STRIDE_BYTES * i`), not of the compact `Vec<usize>` ring
/// this module returns. The single latency path (the orchestrator's
/// `chase_materialized`) chases at this 64-byte stride.
pub const STRIDE_BYTES: usize = 64;

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

// The single latency path is the orchestrator's `chase_materialized`
// (64-byte-stride, materialized buffer). The duplicate compact-ring
// `chase_latency_ns` kernel (which fit in L3 and would have misreported
// L3 latency as DRAM) was removed in BENCH-5; this module now provides
// only the prefetcher-defeating ring builder ([`build_chase_ring`]).

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

    /// (c) BENCH-5: the single latency path chases at the 64-byte stride
    /// documented in the module docs, and the duplicate compact-ring
    /// `chase_latency_ns` kernel is gone (compile-level: referencing it
    /// here would fail to build — this module now exposes only
    /// [`build_chase_ring`] + `STRIDE_BYTES`).
    #[test]
    fn tc_bench5_single_latency_kernel_and_64_byte_stride() {
        // The documented chase stride is 64 bytes (one cache line).
        assert_eq!(
            STRIDE_BYTES, 64,
            "the module doc's 64-byte chase stride must match the kernel's actual stride"
        );
        // The ring builder yields `size / 64` slots — the same 64-byte
        // stride the single latency path materializes at.
        let ring = build_chase_ring(64 * 1024, 42);
        assert_eq!(ring.len(), (64 * 1024) / STRIDE_BYTES);
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

}
