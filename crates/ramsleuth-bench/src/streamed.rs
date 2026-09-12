//! Targeted streaming benchmark runs — the wire-ready streaming contract
//! behind the daemon's single-flight benchmark job (P3-15).
//!
//! **Chunk P3-09 — [CRITICAL-PATH, COUPLED-TO: P3-08].** The daemon runs
//! [`run_streamed`] on a blocking thread and forwards every
//! [`StreamProgress`] event to the owning client connection; a set
//! [`StreamOptions::cancel`] flag halts the run cleanly at the next gate
//! with [`StreamError::Cancelled`], and the terminal [`StreamError`]
//! crosses the wire verbatim (bincode, plan D3).
//!
//! # Cell model
//!
//! A *cell* is one (tier, [`BenchOp`]) bandwidth pair — the grid's 12
//! bandwidth cells (`Memory/L1/L2/L3 × Read/Write/Copy`), listed tier-
//! major. [`StreamTarget`] selects which cells run:
//!
//! - [`StreamTarget::Full`] — all 12 cells plus the four per-tier
//!   pointer-chase latency passes (the complete 4×4 grid);
//! - [`StreamTarget::Tier`] — the tier's three cells plus that tier's
//!   latency pass;
//! - [`StreamTarget::Cell`] — exactly one bandwidth cell.
//!
//! Unrequested cells stay `0.0` in the returned [`BenchmarkGrid`]. The
//! latency pass is the widened median-of-3 kernel from P3-08
//! (`bench_latency`, the same size and seed as `run_all_sized`): a
//! per-tier measurement, not a cell, so it runs only for whole-tier
//! targets, lands in the final grid, and carries no progress event of
//! its own (the [`StreamProgress::op`] tag stays truthful).
//!
//! # Sizing, kernels, threads
//!
//! `run_streamed` detects the host topology itself (a detection failure
//! maps to [`StreamError::Other`]), sizes every tier via the frozen
//! `plan` (P1-03), and picks the kernel family at runtime
//! (`CpuFeatures`, P1-01; the AVX-512 kernels carry their own AVX2
//! fallback). `StreamOptions::threads` bounds the pinned worker count:
//! `0` runs one worker per detected physical core, `n > 0` truncates
//! the dispatch to the first `n` cores (clamped to the detected count).
//!
//! # Cancellation
//!
//! The cancel gate is a relaxed load, checked **before the run starts**
//! and **between cells** (before every bandwidth pass and every latency
//! pass). A hit stops the run cleanly — no panic, no leaked threads
//! (every pass is a synchronous, self-joining scoped dispatch) — and
//! returns [`StreamError::Cancelled`] with the partial grid discarded.
//! The in-flight pass (milliseconds) always completes first.
//!
//! # Progress
//!
//! After each bandwidth cell completes, one [`StreamProgress`] is sent
//! through the caller's channel when one is provided; a dropped
//! receiver is ignored (the `.ok()` contract). `cell_index` /
//! `total_cells` count within the target's cell list.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use crate::buffers::{BufferPlan, plan};
use crate::features::CpuFeatures;
use crate::orchestrator::{
    AlignedBuf, BenchmarkGrid, OrchestratorError, Tier, bench_bandwidth, bench_latency,
    fill_pattern, normalize_size,
};
use crate::topology::{CpuTopology, detect};
use crate::worker::{BenchOp, WorkerError};

/// Grid row order: the [`Tier`] discriminants.
const TIER_ORDER: [Tier; 4] = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];

/// Bandwidth-column order: the [`BenchOp`] arms.
const OP_ORDER: [BenchOp; 3] = [BenchOp::Read, BenchOp::Write, BenchOp::Copy];

/// Which cells a streamed run executes.
///
/// A *cell* is one (tier, [`BenchOp`]) bandwidth pair; see the module
/// docs for the full model (latency is a per-tier bonus of whole-tier
/// targets, never a cell). It is the wire payload of the protocol's
/// `Request::StartBenchmark` arm (P3-10), so it is bincode-serializable
/// (P3-10 contract addendum, plan D2 — no duplication).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StreamTarget {
    /// All 12 bandwidth cells plus the four per-tier latency passes
    /// (the complete 4×4 grid).
    Full,
    /// One tier's three bandwidth cells plus that tier's latency pass.
    Tier(Tier),
    /// Exactly one bandwidth cell.
    Cell(Tier, BenchOp),
}

/// Configuration for one streamed run.
///
/// A local type: it holds the cancel [`Arc`] and never crosses the wire
/// itself — only its [`StreamProgress`] / [`StreamError`] outcomes do.
#[derive(Debug, Clone)]
pub struct StreamOptions {
    /// Which cells to run (see the [`StreamTarget`] docs).
    pub target: StreamTarget,
    /// Pinned worker count: `0` = auto (one per detected physical
    /// core); `n > 0` truncates the dispatch to the first `n` cores,
    /// clamped to the detected count.
    pub threads: usize,
    /// The cancel token: relaxed-loaded at the gates (before the run
    /// starts, between cells); set → the run halts cleanly with
    /// [`StreamError::Cancelled`].
    pub cancel: Arc<AtomicBool>,
}

/// One progress event: a completed bandwidth cell.
///
/// Crosses the wire to the owning client connection (bincode, plan D3);
/// the protocol wraps it verbatim with a `run_id`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StreamProgress {
    /// Zero-based index of the cell within the target's cell list.
    pub cell_index: u32,
    /// Total cells in the target's cell list.
    pub total_cells: u32,
    /// The cell's tier.
    pub tier: Tier,
    /// The cell's bandwidth op.
    pub op: BenchOp,
    /// The measured value: GB/s for a bandwidth cell.
    pub value: f64,
    /// Short human-readable label (e.g. `L1 · Read (GB/s)`).
    pub label: String,
}

/// Failure of a streamed run.
///
/// The daemon forwards it to the client verbatim, so every arm is
/// bincode-serializable: worker failures keep the crate's
/// [`WorkerError`] payload, and everything else (topology detection,
/// unexpected I/O) maps to [`Other`](Self::Other) text — never a raw
/// non-serializable error type.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StreamError {
    /// The cancel flag was set at a gate: the run halted cleanly and the
    /// partial grid was discarded.
    Cancelled,
    /// A bandwidth pass failed (worker dispatch contract violation or a
    /// worker-thread panic).
    Worker(WorkerError),
    /// Anything else, as a plain message.
    Other(String),
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "benchmark run cancelled"),
            Self::Worker(err) => write!(f, "benchmark worker pass failed: {err}"),
            Self::Other(msg) => write!(f, "benchmark run failed: {msg}"),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Worker(err) => Some(err),
            Self::Cancelled | Self::Other(_) => None,
        }
    }
}

/// Run the benchmark for `options.target` and return the grid for the
/// cells that ran (unrequested cells stay `0.0`).
///
/// Topology detection, tier sizing, kernel-family selection, and thread
/// pinning are resolved from the host (see the module docs); one
/// [`StreamProgress`] is sent per completed bandwidth cell through
/// `progress_tx` when provided (a dropped receiver is ignored). The
/// cancel gate is checked before the run starts and between cells: a
/// set flag stops the run cleanly with [`StreamError::Cancelled`] — no
/// panic, no leaked threads.
///
/// # Errors
///
/// - [`StreamError::Cancelled`] when the cancel flag is set at any gate;
/// - [`StreamError::Worker`] when a bandwidth pass fails;
/// - [`StreamError::Other`] when topology detection fails.
pub fn run_streamed(
    options: &StreamOptions,
    progress_tx: Option<mpsc::Sender<StreamProgress>>,
) -> Result<BenchmarkGrid, StreamError> {
    // Gate 0: before any work (detection, allocation, passes).
    ensure_not_cancelled(&options.cancel)?;

    let topo = detect().map_err(|e| StreamError::Other(e.to_string()))?;
    let use_avx512 = CpuFeatures::detect().avx512f;
    let effective = effective_topology(&topo, options.threads);
    let plan = plan(&topo);

    let cells = cells_for_target(&options.target);
    let total_cells = cells.len() as u32;
    let mut grid = zero_grid();
    let mut cell_index = 0u32;

    for &tier in &TIER_ORDER {
        // This tier's cells in global order (the list is tier-major).
        let tier_ops: Vec<BenchOp> = cells
            .iter()
            .filter_map(|(t, op)| (*t == tier).then_some(*op))
            .collect();
        if tier_ops.is_empty() {
            continue; // the target never touches this tier
        }
        let slot = tier as usize;
        let size = normalize_size(tier_size(&plan, tier));

        // Bandwidth cells: one shared aligned src/dst window pair per
        // tier (mirroring `run_all_sized`: one allocation, one fill).
        let mut src = AlignedBuf::new(size);
        let mut dst = AlignedBuf::new(size);
        fill_pattern(src.as_mut_slice());
        for &op in &tier_ops {
            // Gate: between cells.
            ensure_not_cancelled(&options.cancel)?;
            let value =
                bench_bandwidth(&effective, op, src.as_slice(), dst.as_mut_slice(), use_avx512)
                    .map_err(map_orchestrator_error)?;
            set_bandwidth_cell(&mut grid, slot, op, value);
            if let Some(ref tx) = progress_tx {
                let label = format!("{} · {} (GB/s)", tier_name(tier), op_name(op));
                let _ = tx
                    .send(StreamProgress {
                        cell_index,
                        total_cells,
                        tier,
                        op,
                        value,
                        label,
                    })
                    .ok(); // dropped receiver: ignored, per contract
            }
            cell_index += 1;
        }
        // Release both windows before the chase buffer so the per-tier
        // peak stays at ~1× the tier size.
        drop(src);
        drop(dst);

        // Latency: a per-tier measurement (not a cell) that runs only
        // when the target covers the whole tier.
        if covers_full_tier(&options.target, tier) {
            // Gate: between the last bandwidth cell and the latency pass.
            ensure_not_cancelled(&options.cancel)?;
            grid.latency_ns[slot] = bench_latency(size, tier as u64 + 1);
        }
    }
    Ok(grid)
}

/// The zero grid: every cell `0.0` (unrequested cells stay zero).
fn zero_grid() -> BenchmarkGrid {
    BenchmarkGrid {
        read_gbps: [0.0; 4],
        write_gbps: [0.0; 4],
        copy_gbps: [0.0; 4],
        latency_ns: [0.0; 4],
    }
}

/// The `BufferPlan` bandwidth size for one tier (grid row order).
fn tier_size(plan: &BufferPlan, tier: Tier) -> usize {
    match tier {
        Tier::Memory => plan.dram,
        Tier::L1 => plan.l1,
        Tier::L2 => plan.l2,
        Tier::L3 => plan.l3,
    }
}

/// The target's cell list, tier-major, op-ordered (`Full` = 12,
/// `Tier` = 3, `Cell` = 1).
fn cells_for_target(target: &StreamTarget) -> Vec<(Tier, BenchOp)> {
    match target {
        StreamTarget::Full => TIER_ORDER
            .iter()
            .flat_map(|&tier| OP_ORDER.iter().map(move |&op| (tier, op)))
            .collect(),
        StreamTarget::Tier(tier) => OP_ORDER.iter().map(|&op| (*tier, op)).collect(),
        StreamTarget::Cell(tier, op) => vec![(*tier, *op)],
    }
}

/// `true` when the target runs the whole tier (its three cells plus the
/// latency pass): `Full` and `Tier(t)`; a single `Cell` never does.
fn covers_full_tier(target: &StreamTarget, tier: Tier) -> bool {
    match target {
        StreamTarget::Full => true,
        StreamTarget::Tier(t) => *t == tier,
        StreamTarget::Cell(..) => false,
    }
}

/// The topology the worker dispatch sees: `threads == 0` keeps every
/// detected physical core; `n > 0` truncates to the first `n` (clamped
/// to the detected count — asking for more cores than the host has just
/// runs the host's cores).
fn effective_topology(topo: &CpuTopology, threads: usize) -> CpuTopology {
    if threads == 0 {
        return topo.clone();
    }
    let mut effective = topo.clone();
    effective.physical_cores.truncate(threads.min(effective.physical_cores.len()));
    effective
}

/// Write one measured bandwidth cell into the grid.
fn set_bandwidth_cell(grid: &mut BenchmarkGrid, slot: usize, op: BenchOp, value: f64) {
    match op {
        BenchOp::Read => grid.read_gbps[slot] = value,
        BenchOp::Write => grid.write_gbps[slot] = value,
        BenchOp::Copy => grid.copy_gbps[slot] = value,
    }
}

/// The cancel gate: a relaxed load; a set flag halts the run cleanly
/// (no partial grid is delivered).
fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), StreamError> {
    if cancel.load(Ordering::Relaxed) {
        Err(StreamError::Cancelled)
    } else {
        Ok(())
    }
}

/// Map the widened passes' [`OrchestratorError`] onto the wire-ready
/// [`StreamError`] (a topology failure can't occur inside a pass — the
/// host was detected up front — but is mapped for completeness).
fn map_orchestrator_error(err: OrchestratorError) -> StreamError {
    match err {
        OrchestratorError::Worker(e) => StreamError::Worker(e),
        OrchestratorError::Topology(e) => StreamError::Other(e.to_string()),
    }
}

/// Short display name for a tier (progress labels).
fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Memory => "Memory",
        Tier::L1 => "L1",
        Tier::L2 => "L2",
        Tier::L3 => "L3",
    }
}

/// Short display name for a bandwidth op (progress labels).
fn op_name(op: BenchOp) -> &'static str {
    match op {
        BenchOp::Read => "Read",
        BenchOp::Write => "Write",
        BenchOp::Copy => "Copy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::Metric;

    fn options(target: StreamTarget) -> StreamOptions {
        StreamOptions {
            target,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// (a) A minimal run — a single bandwidth cell — returns `Ok` with a
    /// grid in which exactly that cell is filled (finite, positive) and
    /// every other cell stays `0.0` (a `Cell` target never runs the
    /// tier's latency pass).
    #[test]
    fn single_cell_run_fills_exactly_its_cell() {
        let grid =
            run_streamed(&options(StreamTarget::Cell(Tier::L1, BenchOp::Read)), None)
                .expect("single-cell run must succeed");
        for &tier in &TIER_ORDER {
            for &metric in &[Metric::Read, Metric::Write, Metric::Copy, Metric::Latency] {
                let cell = grid.cell(tier, metric);
                assert!(cell.is_finite(), "{tier:?}/{metric:?} = {cell} not finite");
                if (tier, metric) == (Tier::L1, Metric::Read) {
                    assert!(cell > 0.0, "the requested cell must be positive, got {cell}");
                } else {
                    assert_eq!(cell, 0.0, "unrequested {tier:?}/{metric:?} must stay 0.0");
                }
            }
        }
    }

    /// (b) Progress: a whole tier emits exactly one event per bandwidth
    /// cell — ascending indices, a constant `total_cells`, the ops in
    /// grid-column order, finite positive GB/s — and the tier's latency
    /// pass lands in the grid.
    #[test]
    fn tier_run_emits_one_progress_event_per_cell() {
        let (tx, rx) = mpsc::channel();
        let grid = run_streamed(&options(StreamTarget::Tier(Tier::L2)), Some(tx))
            .expect("tier run must succeed");

        let events: Vec<StreamProgress> = rx.iter().collect();
        assert_eq!(events.len(), 3, "three bandwidth cells → three events");
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(ev.cell_index, i as u32, "ascending cell indices");
            assert_eq!(ev.total_cells, 3, "total_cells = the target's cell count");
            assert_eq!(ev.tier, Tier::L2);
            assert!(ev.value.is_finite() && ev.value > 0.0, "measured GB/s: {}", ev.value);
        }
        let ops: Vec<BenchOp> = events.iter().map(|ev| ev.op).collect();
        assert_eq!(ops, vec![BenchOp::Read, BenchOp::Write, BenchOp::Copy]);
        // Whole tier → the latency pass ran too (in the grid, no event).
        assert!(grid.cell(Tier::L2, Metric::Latency) > 0.0, "tier latency must be measured");
    }

    /// (c) Cancel pre-set before the call: the run halts at gate 0 —
    /// `Err(StreamError::Cancelled)` with no panic and zero progress
    /// events (nothing ran).
    #[test]
    fn pre_set_cancel_halts_before_any_work() {
        let (tx, rx) = mpsc::channel();
        let opts = StreamOptions {
            target: StreamTarget::Tier(Tier::L1),
            threads: 0,
            cancel: Arc::new(AtomicBool::new(true)),
        };
        let err = run_streamed(&opts, Some(tx)).expect_err("pre-set cancel must fail the run");
        assert!(matches!(err, StreamError::Cancelled), "got {err:?}");
        assert_eq!(err.to_string(), "benchmark run cancelled");
        assert_eq!(rx.try_iter().count(), 0, "no progress before the first gate");
    }

    /// (d1) `StreamProgress` round-trips through bincode (the wire
    /// codec, plan D3) — it crosses to the client verbatim.
    #[test]
    fn stream_progress_bincode_round_trip() {
        let progress = StreamProgress {
            cell_index: 7,
            total_cells: 12,
            tier: Tier::L3,
            op: BenchOp::Write,
            value: 42.5,
            label: "L3 · Write (GB/s)".to_string(),
        };
        let bytes = bincode::serialize(&progress).expect("StreamProgress must serialize");
        let back: StreamProgress =
            bincode::deserialize(&bytes).expect("StreamProgress must deserialize");
        assert_eq!(back, progress);
    }

    /// (d2) Every `StreamError` arm round-trips through bincode —
    /// including `Worker`, whose payload is the crate's `WorkerError`
    /// verbatim.
    #[test]
    fn stream_error_bincode_round_trip() {
        let errors = vec![
            StreamError::Cancelled,
            StreamError::Worker(WorkerError::LengthMismatch { src: 1, dst: 2 }),
            StreamError::Worker(WorkerError::Misaligned { buffer: "src", required: 32 }),
            StreamError::Worker(WorkerError::NotBlockMultiple { len: 120, block: 32 }),
            StreamError::Worker(WorkerError::WorkerPanic { index: 5 }),
            StreamError::Other("topology detection failed".to_string()),
        ];
        let bytes = bincode::serialize(&errors).expect("StreamError must serialize");
        let back: Vec<StreamError> =
            bincode::deserialize(&bytes).expect("StreamError must deserialize");
        assert_eq!(back, errors);
    }

    /// (e) A dropped progress receiver is ignored (the `.ok()`
    /// contract): the run still completes `Ok` with the measured cell.
    #[test]
    fn dropped_progress_receiver_does_not_fail_the_run() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let grid = run_streamed(&options(StreamTarget::Cell(Tier::L1, BenchOp::Copy)), Some(tx))
            .expect("a dropped receiver must not fail the run");
        assert!(grid.cell(Tier::L1, Metric::Copy) > 0.0);
    }

    /// (f) `StreamError` displays and wraps its worker cause (crate
    /// error-idiom parity with `OrchestratorError`).
    #[test]
    fn error_display_and_source_wrap_the_cause() {
        let err = StreamError::Worker(WorkerError::WorkerPanic { index: 2 });
        assert!(err.to_string().contains("benchmark worker pass failed"));
        assert!(std::error::Error::source(&err).is_some());
        assert!(std::error::Error::source(&StreamError::Cancelled).is_none());
        assert!(std::error::Error::source(&StreamError::Other("x".into())).is_none());
    }
}
