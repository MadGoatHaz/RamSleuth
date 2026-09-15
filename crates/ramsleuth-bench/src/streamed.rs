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
//!
//! # Burn-in
//!
//! [`run_burn_in`] loops whole passes — one [`run_cell_pass`] per
//! iteration, a fresh zero grid and a zeroed cursor each time — until
//! (a) the cancel gate trips (the in-flight pass completes first; its
//! partial grid is discarded — the [`StreamError::Cancelled`] contract)
//! or (b) for a finite `duration_minutes`, the first completed pass
//! whose run-elapsed time reaches `duration_minutes × 60` seconds (the
//! grid of that last completed pass is returned). `duration_minutes ==
//! 0` runs until cancelled. Each completed bandwidth cell and each
//! completed per-tier latency pass emits one [`BurnInTick`] — the
//! 1-based iteration, the run-elapsed seconds at the emit, and exactly
//! one of the bandwidth `(op, GB/s)` or the latency `ns` value —
//! through the channel the caller provided when one is; a dropped
//! receiver is ignored (the `.ok()` contract).

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// One burn-in tick: a completed bandwidth cell or a completed per-tier
/// latency pass of one [`run_burn_in`] iteration.
///
/// Crosses the wire to the owning client connection (bincode, plan
/// D-2); the protocol wraps it verbatim in its `BurnInProgress`
/// response arm (C7-07). A wire payload, so it is serde-derived — the
/// [`StreamProgress`] precedent. Exactly one of
/// [`bandwidth`](Self::bandwidth) / [`latency_ns`](Self::latency_ns)
/// is `Some` per tick; `iteration` is 1-based and `elapsed_secs` is
/// the run elapsed at the emit, stamped from the run start.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BurnInTick {
    /// The 1-based burn-in iteration this tick belongs to.
    pub iteration: u32,
    /// The run elapsed, in seconds, at the moment the tick was emitted.
    pub elapsed_secs: f64,
    /// The tick tier.
    pub tier: Tier,
    /// The completed bandwidth cell: its op and the measured GB/s.
    /// `Some` for a cell tick, `None` for a latency tick.
    pub bandwidth: Option<(BenchOp, f64)>,
    /// The completed per-tier latency pass: the measured ns. `Some`
    /// for a latency tick, `None` for a cell tick.
    pub latency_ns: Option<f64>,
}

/// Configuration for one burn-in run.
///
/// A local type, like [`StreamOptions`]: it holds the cancel [`Arc`]
/// and never crosses the wire itself — only its [`BurnInTick`]
/// payloads do (the protocol `StartBurnIn` request arm carries just
/// the `target` and `duration_minutes`, C7-07).
#[derive(Debug, Clone)]
pub struct BurnInOptions {
    /// Which cells every pass runs (see the [`StreamTarget`] docs).
    pub target: StreamTarget,
    /// The run duration in minutes: `0` = infinite (stop only via
    /// cancel); `n > 0` = stop once a pass has completed and the run
    /// elapsed is at least `n × 60` seconds.
    pub duration_minutes: u32,
    /// Pinned worker count: same semantics as
    /// [`StreamOptions::threads`] (`0` = auto).
    pub threads: usize,
    /// The cancel token: relaxed-loaded at the gates (before the run
    /// starts, between iterations, and inside every pass); set → the
    /// in-flight pass completes first, its partial grid is discarded,
    /// and the run returns [`StreamError::Cancelled`].
    pub cancel: Arc<AtomicBool>,
}

/// Run the benchmark for `options.target` and return the grid for the
/// cells that ran (unrequested cells stay `0.0`).
///
/// Topology detection, tier sizing, kernel-family selection, and thread
/// pinning are resolved from the host once (see the module docs), and
/// the run is one whole pass over the target's cells via
/// [`run_cell_pass`], wired here to the [`StreamProgress`] emitter: one
/// event per completed bandwidth cell through `progress_tx` when
/// provided (a dropped receiver is ignored), and no event for the
/// per-tier latency pass (it lands in the grid with no progress cell of
/// its own — the `StreamProgress::op` tag stays truthful). The cancel
/// gate is checked before the run starts and between cells: a set flag
/// stops the run cleanly with [`StreamError::Cancelled`] — no panic, no
/// leaked threads.
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
    let setup = PassSetup {
        target: options.target,
        cells,
        total_cells,
        effective,
        use_avx512,
        plan,
    };
    let mut grid = zero_grid();
    let ctx = PassContext { cell_index: 0 };

    // One whole pass over the target's cells. The cell-completed emit
    // forwards to the progress channel (a no-op when none is provided;
    // a dropped receiver is ignored — the `.ok()` contract); the
    // latency emit is a no-op: the latency pass is a per-tier
    // measurement, never a progress cell.
    let mut emit_cell = |tier: Tier,
                        op: BenchOp,
                        value: f64,
                        cell_index: u32,
                        total_cells: u32| {
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
    };
    let mut emit_latency = |_tier: Tier, _ns: f64| {};
    run_cell_pass(
        &mut grid,
        &setup,
        &options.cancel,
        &ctx,
        &mut emit_cell,
        &mut emit_latency,
    )?;
    Ok(grid)
}

/// Run a burn-in: loop whole passes (one [`run_cell_pass`] per
/// iteration) until the cancel gate trips or, for a finite
/// `duration_minutes`, the deadline is reached after a completed pass.
///
/// Same detection / sizing preamble as [`run_streamed`] (shared via the
/// existing helpers), then the pass loop: each iteration runs one full
/// pass over the target cells with a fresh zero grid and a zeroed
/// [`PassContext`], emitting one [`BurnInTick`] per completed bandwidth
/// cell and per completed per-tier latency pass through `tx` when
/// provided (a dropped receiver is ignored — the `.ok()` contract).
/// Each tick carries the 1-based iteration, the run elapsed at the emit
/// (stamped from the run start), and exactly one of the bandwidth or
/// the latency value.
///
/// Stop conditions: (a) the cancel gate trips — the in-flight pass
/// completes first and its partial grid is discarded, the run returns
/// [`StreamError::Cancelled`] (the [`run_streamed`] cancel contract);
/// (b) `duration_minutes > 0` and a pass has completed with the run
/// elapsed at least `duration_minutes × 60` — the run returns `Ok`
/// with the grid of that last completed pass. `duration_minutes == 0`
/// is infinite: the loop runs until cancelled.
///
/// # Errors
///
/// - [`StreamError::Cancelled`] when the cancel flag is set at any gate;
/// - [`StreamError::Worker`] when a bandwidth pass fails;
/// - [`StreamError::Other`] when topology detection fails.
pub fn run_burn_in(
    options: &BurnInOptions,
    tx: Option<mpsc::Sender<BurnInTick>>,
) -> Result<BenchmarkGrid, StreamError> {
    burn_in(options, tx, duration_deadline(options.duration_minutes))
}

/// The burn-in stop deadline: `0` minutes = no deadline (infinite —
/// cancel-only stop); `n > 0` = `n × 60` seconds from the run start.
fn duration_deadline(duration_minutes: u32) -> Option<Duration> {
    if duration_minutes == 0 {
        None
    } else {
        Some(Duration::from_secs(duration_minutes as u64 * 60))
    }
}

/// The burn-in pass loop: the detection / sizing preamble runs once,
/// then every iteration runs one whole pass (fresh zero grid, zeroed
/// cursor) until a stop condition fires.
///
/// # Errors
///
/// - [`StreamError::Cancelled`] at any cancel gate (the in-flight pass
///   completes first; its partial grid is discarded);
/// - [`StreamError::Worker`] when a bandwidth pass fails;
/// - [`StreamError::Other`] when topology detection fails.
fn burn_in(
    options: &BurnInOptions,
    tx: Option<mpsc::Sender<BurnInTick>>,
    deadline: Option<Duration>,
) -> Result<BenchmarkGrid, StreamError> {
    // Gate 0: before any work (detection, allocation, passes).
    ensure_not_cancelled(&options.cancel)?;

    let topo = detect().map_err(|e| StreamError::Other(e.to_string()))?;
    let use_avx512 = CpuFeatures::detect().avx512f;
    let effective = effective_topology(&topo, options.threads);
    let plan = plan(&topo);

    let cells = cells_for_target(&options.target);
    let total_cells = cells.len() as u32;
    let setup = PassSetup {
        target: options.target,
        cells,
        total_cells,
        effective,
        use_avx512,
        plan,
    };

    let start = Instant::now();
    let mut iteration: u32 = 0;
    loop {
        iteration += 1;
        // Gate: between iterations (a pass also gates internally,
        // before every bandwidth and latency pass).
        ensure_not_cancelled(&options.cancel)?;

        let mut grid = zero_grid();
        let ctx = PassContext { cell_index: 0 };

        // One tick per completed bandwidth cell: the iteration, the
        // run elapsed at the emit, and exactly the bandwidth value.
        let mut emit_cell = |tier: Tier,
                            op: BenchOp,
                            value: f64,
                            _cell_index: u32,
                            _total_cells: u32| {
            if let Some(sender) = tx.as_ref() {
                let _ = sender
                    .send(BurnInTick {
                        iteration,
                        elapsed_secs: start.elapsed().as_secs_f64(),
                        tier,
                        bandwidth: Some((op, value)),
                        latency_ns: None,
                    })
                    .ok(); // dropped receiver: ignored, per contract
            }
        };
        // One tick per completed per-tier latency pass: exactly the
        // latency value.
        let mut emit_latency = |tier: Tier, ns: f64| {
            if let Some(sender) = tx.as_ref() {
                let _ = sender
                    .send(BurnInTick {
                        iteration,
                        elapsed_secs: start.elapsed().as_secs_f64(),
                        tier,
                        bandwidth: None,
                        latency_ns: Some(ns),
                    })
                    .ok(); // dropped receiver: ignored, per contract
            }
        };
        // The in-flight pass completes first; on a cancel gate hit the
        // partial grid is discarded (the run_streamed contract), and a
        // worker failure propagates as-is.
        run_cell_pass(
            &mut grid,
            &setup,
            &options.cancel,
            &ctx,
            &mut emit_cell,
            &mut emit_latency,
        )?;
        // Deadline reached after a completed pass: return that pass grid.
        if let Some(limit) = deadline {
            if start.elapsed() >= limit {
                return Ok(grid);
            }
        }
        // No deadline (infinite) or not yet reached: next iteration.
    }
}

/// Everything one pass needs that does not change between passes: the
/// target and its cell list (tier-major, op-ordered), the target's
/// bandwidth cell count, the thread-bounded topology the worker
/// dispatch sees, the kernel-family flag, and the per-tier sizing
/// plan. `run_streamed` resolves it from the host once per run; the
/// burn-in loop (C7-06) reuses the same setup for every iteration.
struct PassSetup {
    /// Which cells the pass runs (see the [`StreamTarget`] docs).
    target: StreamTarget,
    /// The target's cell list, tier-major, op-ordered.
    cells: Vec<(Tier, BenchOp)>,
    /// The target's bandwidth cell count (emitted with every completed
    /// cell).
    total_cells: u32,
    /// The topology the worker dispatch sees (`threads`-bounded).
    effective: CpuTopology,
    /// The runtime kernel-family flag (AVX-512).
    use_avx512: bool,
    /// The per-tier sizing plan.
    plan: BufferPlan,
}

/// The per-pass bookkeeping for [`run_cell_pass`]: where the pass's
/// cell cursor starts. A pass always walks the target's cell list from
/// the beginning (`0` for a fresh pass — every pass is one), and the
/// cursor grows by one per completed bandwidth cell, so a fresh pass
/// emits indices `0 .. total_cells` (the emitted index is the
/// pre-increment value).
struct PassContext {
    /// The pass's starting cell cursor (`0` for a fresh pass).
    cell_index: u32,
}

/// Run one whole pass over the target's cells, writing the measured
/// cells into `grid`.
///
/// For every tier the target touches, in [`TIER_ORDER`]: the tier's
/// cells in global order run against one shared aligned src/dst window
/// pair (one allocation, one fill — mirroring `run_all_sized`), the
/// cancel gate checked before every bandwidth pass; each completed cell
/// is written to the grid and forwarded to `emit_cell` (the cell's
/// tier, op, value, index within the target's list, and the list's
/// total); both windows are released before the chase buffer so the
/// per-tier peak stays at ~1× the tier size. When the target covers the
/// whole tier, the per-tier latency pass runs behind its own gate
/// (between the last bandwidth cell and the chase): a per-tier
/// measurement, not a cell — it lands in the grid and is forwarded to
/// `emit_latency` (tier, ns) with no index bookkeeping.
///
/// `run_streamed` is this helper wired to its [`StreamProgress`]
/// emitter (one pass from a zero cursor); the burn-in loop (C7-06)
/// runs the same helper repeatedly, one pass per iteration.
///
/// # Errors
///
/// - [`StreamError::Cancelled`] when the cancel flag is set at any gate
///   (the in-flight pass completes first; the grid is left as written —
///   the caller discards it);
/// - [`StreamError::Worker`] when a bandwidth pass fails.
fn run_cell_pass(
    grid: &mut BenchmarkGrid,
    setup: &PassSetup,
    cancel: &AtomicBool,
    ctx: &PassContext,
    emit_cell: &mut impl FnMut(Tier, BenchOp, f64, u32, u32),
    emit_latency: &mut impl FnMut(Tier, f64),
) -> Result<(), StreamError> {
    let mut cell_index = ctx.cell_index;
    for &tier in &TIER_ORDER {
        // This tier's cells in global order (the list is tier-major).
        let tier_ops: Vec<BenchOp> = setup
            .cells
            .iter()
            .filter_map(|(t, op)| (*t == tier).then_some(*op))
            .collect();
        if tier_ops.is_empty() {
            continue; // the target never touches this tier
        }
        let slot = tier as usize;
        let size = normalize_size(tier_size(&setup.plan, tier));

        // Bandwidth cells: one shared aligned src/dst window pair per
        // tier (mirroring `run_all_sized`: one allocation, one fill).
        let mut src = AlignedBuf::new(size);
        let mut dst = AlignedBuf::new(size);
        fill_pattern(src.as_mut_slice());
        for &op in &tier_ops {
            // Gate: between cells.
            ensure_not_cancelled(cancel)?;
            let value =
                bench_bandwidth(
                    &setup.effective,
                    op,
                    src.as_slice(),
                    dst.as_mut_slice(),
                    setup.use_avx512,
                )
                .map_err(map_orchestrator_error)?;
            set_bandwidth_cell(grid, slot, op, value);
            emit_cell(tier, op, value, cell_index, setup.total_cells);
            cell_index += 1;
        }
        // Release both windows before the chase buffer so the per-tier
        // peak stays at ~1× the tier size.
        drop(src);
        drop(dst);

        // Latency: a per-tier measurement (not a cell) that runs only
        // when the target covers the whole tier.
        if covers_full_tier(&setup.target, tier) {
            // Gate: between the last bandwidth cell and the latency pass.
            ensure_not_cancelled(cancel)?;
            let ns = bench_latency(size, tier as u64 + 1);
            grid.latency_ns[slot] = ns;
            emit_latency(tier, ns);
        }
    }
    Ok(())
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

    /// (g) Burn-in ticks: a whole-tier target emits one tick per
    /// completed bandwidth cell and one per completed latency pass,
    /// per iteration — iterations ascending from 1, elapsed
    /// non-decreasing, exactly one of bandwidth / latency per tick,
    /// the ops in grid-column order. The finite duration (1 minute,
    /// the minimum) far exceeds the test window, so the run is
    /// stopped via cancel once two full iterations have been observed.
    #[test]
    fn burn_in_emits_ascending_ticks_per_iteration() {
        let (tx, rx) = mpsc::channel();
        let opts = BurnInOptions {
            target: StreamTarget::Tier(Tier::L1),
            duration_minutes: 1,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let cancel = opts.cancel.clone();
        let run = std::thread::spawn(move || run_burn_in(&opts, Some(tx)));
        let mut ticks = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(t) => {
                    ticks.push(t);
                    // The 8th tick is the latency pass of iteration 2:
                    // two full iterations observed.
                    if let Some(last) = ticks.last() {
                        if last.iteration >= 2 && last.latency_ns.is_some() {
                            break;
                        }
                    }
                }
                Err(_) => panic!("no burn-in ticks within the wait window"),
            }
        }
        cancel.store(true, Ordering::Relaxed);
        let err = run
            .join()
            .expect("the run thread must finish")
            .expect_err("a cancelled burn-in must return an error");
        assert!(matches!(err, StreamError::Cancelled), "got {err:?}");
        // Two full tier iterations = 2 x (3 cells + 1 latency) = 8 ticks.
        assert_eq!(ticks.len(), 8, "two full tier iterations = 8 ticks");
        // Iterations are 1-based and non-decreasing.
        assert_eq!(ticks[0].iteration, 1, "the first tick must be iteration 1");
        for w in ticks.windows(2) {
            assert!(w[1].iteration >= w[0].iteration, "iterations must not decrease");
        }
        // Elapsed: finite, non-negative, non-decreasing.
        for t in &ticks {
            assert!(
                t.elapsed_secs.is_finite() && t.elapsed_secs >= 0.0,
                "elapsed = {}",
                t.elapsed_secs
            );
        }
        for w in ticks.windows(2) {
            assert!(
                w[1].elapsed_secs + 0.001 >= w[0].elapsed_secs,
                "elapsed must not decrease"
            );
        }
        // Every tick is the tier tier and carries exactly one of
        // bandwidth / latency.
        for t in &ticks {
            assert_eq!(t.tier, Tier::L1, "a tier target only ticks its tier");
            assert_eq!(
                t.bandwidth.is_some(),
                t.latency_ns.is_none(),
                "exactly one of bandwidth / latency per tick"
            );
        }
        // Per-iteration pattern: Read, Write, Copy, then the latency pass.
        for iter in 1..=2u32 {
            let iter_ticks: Vec<&BurnInTick> = ticks
                .iter()
                .filter(|t| t.iteration == iter)
                .collect();
            assert_eq!(iter_ticks.len(), 4, "iteration {iter} = 3 cells + 1 latency pass");
            let ops: Vec<BenchOp> = iter_ticks
                .iter()
                .filter_map(|t| t.bandwidth.map(|(op, _)| op))
                .collect();
            assert_eq!(
                ops,
                vec![BenchOp::Read, BenchOp::Write, BenchOp::Copy],
                "ops in grid-column order"
            );
            let ns = iter_ticks
                .iter()
                .filter_map(|t| t.latency_ns)
                .next()
                .expect("one latency tick per iteration");
            assert!(ns > 0.0 && ns.is_finite(), "the latency value must be positive, got {ns}");
        }
    }

    /// (h) Deadline stop: a finite duration stops the loop once a pass
    /// has completed with the run elapsed at the deadline — the public
    /// entry derives `duration_minutes × 60` seconds; the loop seam is
    /// driven with a sub-second deadline so the test does not wait a
    /// full minute. The returned grid is the last completed pass:
    /// exactly the target cell measured, everything else 0.0.
    #[test]
    fn burn_in_deadline_stops_after_a_completed_pass() {
        let (tx, rx) = mpsc::channel();
        let opts = BurnInOptions {
            target: StreamTarget::Cell(Tier::L1, BenchOp::Read),
            duration_minutes: 1,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let grid = burn_in(&opts, Some(tx), Some(Duration::from_millis(50)))
            .expect("a deadline burn-in must complete Ok");
        let ticks: Vec<BurnInTick> = rx.iter().collect();
        assert!(!ticks.is_empty(), "at least one completed pass emits a tick");
        for t in &ticks {
            assert!(t.iteration >= 1, "iterations are 1-based");
            assert!(t.elapsed_secs.is_finite() && t.elapsed_secs >= 0.0);
            assert_eq!(t.bandwidth.is_some(), t.latency_ns.is_none(), "exactly one Some per tick");
        }
        // The terminal grid: the target cell measured, everything else 0.0.
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

    /// (h2) The minutes → deadline derivation: `0` = no deadline
    /// (infinite), `n > 0` = `n × 60` seconds.
    #[test]
    fn burn_in_duration_deadline_derivation() {
        assert_eq!(duration_deadline(0), None);
        assert_eq!(duration_deadline(1), Some(Duration::from_secs(60)));
        assert_eq!(duration_deadline(2), Some(Duration::from_secs(120)));
        assert_eq!(duration_deadline(1440), Some(Duration::from_secs(86_400)));
    }

    /// (i) Cancel pre-set before the call: the burn-in halts at gate
    /// 0 — `Err(StreamError::Cancelled)` with no panic and zero ticks
    /// (nothing ran).
    #[test]
    fn burn_in_pre_set_cancel_halts_before_any_work() {
        let (tx, rx) = mpsc::channel();
        let opts = BurnInOptions {
            target: StreamTarget::Tier(Tier::L1),
            duration_minutes: 0,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(true)),
        };
        let err = run_burn_in(&opts, Some(tx)).expect_err("a pre-set cancel must fail the run");
        assert!(matches!(err, StreamError::Cancelled), "got {err:?}");
        assert_eq!(rx.try_iter().count(), 0, "no ticks before the first gate");
    }

    /// (j) Infinite duration (`0` minutes): the loop never stops on the
    /// clock — it runs until the cancel gate trips, returning
    /// `Cancelled` with the partial pass grid discarded. A single-cell
    /// target keeps the loop cheap (one tick per completed pass).
    #[test]
    fn burn_in_zero_duration_runs_until_cancelled() {
        let (tx, rx) = mpsc::channel();
        let opts = BurnInOptions {
            target: StreamTarget::Cell(Tier::L1, BenchOp::Read),
            duration_minutes: 0,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let cancel = opts.cancel.clone();
        let run = std::thread::spawn(move || run_burn_in(&opts, Some(tx)));
        let mut ticks = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(t) => {
                    ticks.push(t);
                    if ticks.len() >= 3 {
                        break;
                    }
                }
                Err(_) => panic!("no burn-in ticks within the wait window"),
            }
        }
        cancel.store(true, Ordering::Relaxed);
        let res = run.join().expect("the run thread must finish");
        assert!(matches!(res, Err(StreamError::Cancelled)), "got {res:?}");
        // Three ticks from a single-cell target = three full passes:
        // the loop ran past the clock until the cancel gate stopped it.
        assert_eq!(ticks.len(), 3, "one tick per completed single-cell pass");
        for (i, t) in ticks.iter().enumerate() {
            assert_eq!(t.iteration, (i + 1) as u32, "consecutive iterations");
        }
    }

    /// (k) A dropped burn-in tick receiver is ignored (the `.ok()`
    /// contract): the run still completes — here via a tiny deadline —
    /// with the measured cell.
    #[test]
    fn burn_in_dropped_receiver_does_not_fail_the_run() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let opts = BurnInOptions {
            target: StreamTarget::Cell(Tier::L1, BenchOp::Copy),
            duration_minutes: 1,
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let grid = burn_in(&opts, Some(tx), Some(Duration::from_millis(50)))
            .expect("a dropped receiver must not fail the run");
        assert!(grid.cell(Tier::L1, Metric::Copy) > 0.0);
    }

    /// (l) `BurnInTick` round-trips through bincode (the wire codec,
    /// plan D-2) — both tick kinds: a bandwidth cell and a latency pass.
    #[test]
    fn burn_in_tick_bincode_round_trip() {
        let ticks = vec![
            BurnInTick {
                iteration: 3,
                elapsed_secs: 42.5,
                tier: Tier::L2,
                bandwidth: Some((BenchOp::Write, 18.75)),
                latency_ns: None,
            },
            BurnInTick {
                iteration: 4,
                elapsed_secs: 61.25,
                tier: Tier::Memory,
                bandwidth: None,
                latency_ns: Some(142.0),
            },
        ];
        let bytes = bincode::serialize(&ticks).expect("BurnInTick must serialize");
        let back: Vec<BurnInTick> =
            bincode::deserialize(&bytes).expect("BurnInTick must deserialize");
        assert_eq!(back, ticks);
    }
}
