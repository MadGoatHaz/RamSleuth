//! Zone 2 renderer: the AIDA64-style 4×4 benchmark grid, the run
//! controls (including the burn-in — a duration knob + `Run Burn-In`,
//! C7-18 — and a live per-iteration row), and a flat status line
//! (C7-17 — the P3-28 progress pill retired).
//!
//! Grand Design §3.1 right panel: the 4×4 grid — Memory / L1 / L2 / L3
//! rows × Read / Write / Copy / Latency columns. The grid is **live
//! during a run**: a normal bench's live grid ([`live_grid`])
//! accumulates each streamed [`StreamProgress`] event's (tier, op,
//! value) — the latest per cell wins — and a burn-in's live grid is
//! `BurnInState.latest` (the newest per-cell values seen this burn-in —
//! the cells update per iteration, C7-18); every measured cell renders
//! its in-flight value dimmed with a `…` suffix as the events arrive
//! (egui repaints every frame); the cells not started yet keep the
//! `N/A` placeholder (a normal bench's stream carries no latency
//! events, so its latency column stays `N/A`; a burn-in's `latest` also
//! carries per-tier latency ticks, so its latency cells can be live).
//! Once the run is not in flight, the grid shows the terminal result
//! grid of the last completed run, every cell its formatted value
//! (two-decimal GB/s for a bandwidth metric, ns for latency) or `N/A`
//! for an unmeasured cell (0.0 on the wire), in an
//! `egui_extras::TableBuilder` table (bandwidth values CYAN, latency
//! cells AMBER — the palette's low-latency accent — `N/A` CRIMSON); a
//! flat status line (C7-17 — the progress bar's pill retired):
//! `Status: Idle` dim at rest, `Status: Running… <m:ss>` CYAN while a
//! normal bench is in flight (elapsed from the zone's local start
//! clock) or `Status: Running… (burn-in <m:ss>, iter <n>)` while a
//! burn-in is (elapsed + iteration from the newest tick — the `…` kept
//! static, the 60 FPS repaint animates the elapsed), `Status: Done` in
//! a zone-local green after a run finished (a terminal grid or kept
//! progress); and the run controls — `Run Full` + `Memory Only` send a
//! [`BenchCmd`] to the background poller (the poller owns the socket,
//! D6: no render-thread I/O), the burn-in's minutes knob (`0` =
//! infinite, clamped to [`BURN_IN_MINUTES_MAX`]) + `Run Burn-In` send a
//! burn-in [`BenchCmd`] (C7-18), all the run buttons disable while any
//! run is in flight (single-flight), and `Cancel` — shown while any run
//! is in flight — sets the shared cancel flag the poller checks between
//! frames (P3-28).
//!
//! **No grid yet** renders the full table with every cell `N/A` (the
//! placeholder state — the layout never shifts when the result
//! lands). **No-panic contract (D5):** the zone only reads a
//! `&TelemetryData` snapshot and two `&` flags; every control is a
//! fire-and-forget channel send or an atomic store, so a flapping /
//! absent daemon degrades to the placeholder + the dim
//! `Status: Idle` line — never a panic.
//!
//! **Pure core:** [`grid_cells`] (the 16 `(tier · op, value-or-N/A)`
//! pairs of the terminal result grid), [`live_grid`] (the normal
//! bench's in-flight grid accumulated from the streamed events — the
//! latest value per cell wins), the flat status line's view (C7-17:
//! [`status_state`] / [`status_text`] / [`status_color`] /
//! [`format_elapsed`]), and the burn-in's views (C7-18:
//! [`run_in_flight`] / [`table_live_grid`] / [`burn_in_cmd`] /
//! [`burn_in_row`]) are I/O-free and deterministic — the unit tests
//! exercise them without an egui context; [`render_bench_zone`] is the
//! thin `egui` surface over them (the live render is verified in the
//! QA phase).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Instant;

use ramsleuth_bench::{BenchOp, BenchmarkGrid, Metric, StreamProgress, StreamTarget, Tier};
use ramsleuth_protocol::BenchMode;

use crate::update::{BenchCmd, BenchState, BurnInState, TelemetryData};
use crate::{AMBER, CRIMSON, CYAN, SLATE};

/// The zone title (Grand Design §3.1, right panel).
const ZONE_TITLE: &str = "2 · BENCHMARK ENGINE";
/// The grid's row order (the `Tier` discriminants: `BenchmarkGrid`'s
/// array indices).
const TIERS: [Tier; 4] = [Tier::Memory, Tier::L1, Tier::L2, Tier::L3];
/// The grid's column order (the four metrics).
const METRICS: [Metric; 4] = [Metric::Read, Metric::Write, Metric::Copy, Metric::Latency];
/// One table-row height (pixels).
const ROW_HEIGHT: f32 = 26.0;
/// The `Status: Done` line's green (C7-17): a dark-slate-compatible
/// green, zone-local — the palette in `style.rs` stays frozen (the
/// status colors are zone-local, not a palette entry).
const STATUS_DONE: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x9E, 0x5B);
/// The burn-in duration knob's max (minutes, C7-18): 24 h (the plan's
/// `0..=1440` clamp). `0` = infinite (continuous until manually stopped
/// via `Cancel`).
const BURN_IN_MINUTES_MAX: u32 = 1440;
/// The burn-in duration knob's default (minutes, C7-18): a short,
/// bounded first-run burn-in (the operator drags it up — or to `0` =
/// infinite — before a long soak).
const BURN_IN_MINUTES_DEFAULT: u32 = 5;

// ---------------------------------------------------------------------
// The pure core (testable: no I/O, no egui context).
// ---------------------------------------------------------------------

/// The display name of one grid row.
fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Memory => "Memory",
        Tier::L1 => "L1",
        Tier::L2 => "L2",
        Tier::L3 => "L3",
    }
}

/// The display name of one grid column.
fn metric_name(metric: Metric) -> &'static str {
    match metric {
        Metric::Read => "Read",
        Metric::Write => "Write",
        Metric::Copy => "Copy",
        Metric::Latency => "Latency",
    }
}

/// One cell's display text: the formatted value (two-decimal GB/s for
/// a bandwidth metric, ns for latency) or `N/A` for an unmeasured
/// cell (0.0 on the wire, or a non-finite reading).
fn cell_text(grid: &BenchmarkGrid, tier: Tier, metric: Metric) -> String {
    let value = grid.cell(tier, metric);
    if !value.is_finite() || value <= 0.0 {
        return "N/A".to_owned();
    }
    match metric {
        Metric::Latency => format!("{value:.2} ns"),
        _ => format!("{value:.2} GB/s"),
    }
}

/// The 16 grid cells, in row-major order: four tiers (Memory, L1, L2,
/// L3) × four metrics (Read, Write, Copy, Latency).
///
/// Pure and deterministic: the same grid always yields the same `Vec`.
/// Each pair is the cell's `"tier · op"` label (the progress label's
/// form) and its [`cell_text`] display (a formatted value, or `N/A`).
/// It is the pure 16-cell view of the terminal result grid — the
/// unit tests exercise it without an egui context.
pub fn grid_cells(grid: &BenchmarkGrid) -> Vec<(String, String)> {
    let mut cells = Vec::with_capacity(16);
    for tier in &TIERS {
        for metric in &METRICS {
            cells.push((
                format!("{} · {}", tier_name(*tier), metric_name(*metric)),
                cell_text(grid, *tier, *metric),
            ));
        }
    }
    cells
}

/// The bench zone's flat status line state (C7-17 — the progress
/// bar's pill retired): the idle rest state, a run in flight (with
/// the run's elapsed), and a finished run (a terminal grid or kept
/// streamed progress present).
#[derive(Debug, Clone, Copy, PartialEq)]
enum StatusState {
    /// No run in flight and no run has finished: the dim
    /// `Status: Idle` line.
    Idle,
    /// A run (a normal bench or a burn-in) is in flight: the CYAN
    /// running line; `elapsed_secs` is the run's elapsed (the newest
    /// tick's for a burn-in, the zone's start clock's for a normal
    /// bench). `burn_in_iteration` is `Some(n)` for a burn-in (the
    /// `Status: Running… (burn-in <m:ss>, iter <n>)` line, C7-18) and
    /// `None` for a normal bench (the `Status: Running… <m:ss>` line).
    Running { elapsed_secs: f64, burn_in_iteration: Option<u32> },
    /// A run finished (a terminal grid landed, or streamed progress
    /// is kept): the green `Status: Done` line.
    Done,
}

/// The status state of one run bookkeeping: a burn-in in flight →
/// `Running` (elapsed from the newest tick, the newest tick's
/// iteration), a normal bench in flight → `Running` (elapsed from the
/// zone's start clock, no burn-in iteration), neither in flight but a
/// terminal grid or kept streamed progress present → `Done`, the fresh
/// state (no grid, no progress) → `Idle`.
///
/// The `Done` corner deliberately includes the kept-progress-only
/// shape (a cancelled normal bench with no terminal grid): the
/// plan's idle condition is `progress.is_empty()` — a non-empty
/// progress list with no run in flight is a finished run, not an idle
/// one.
fn status_state(
    running: bool,
    burn_in_running: bool,
    grid_present: bool,
    progress_present: bool,
    running_elapsed_secs: f64,
    burn_in_iteration: u32,
) -> StatusState {
    if burn_in_running {
        StatusState::Running {
            elapsed_secs: running_elapsed_secs,
            burn_in_iteration: Some(burn_in_iteration),
        }
    } else if running {
        StatusState::Running {
            elapsed_secs: running_elapsed_secs,
            burn_in_iteration: None,
        }
    } else if grid_present || progress_present {
        StatusState::Done
    } else {
        StatusState::Idle
    }
}

/// One run's elapsed in the status line's `m:ss` form (the plan's
/// `0:42`): the minutes un-padded, the seconds two-digit; a
/// non-finite / non-positive reading renders `0:00` (a bad elapsed
/// never panics the line, D5).
fn format_elapsed(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    format!("{}:{:02}", total / 60, total % 60)
}

/// The flat status line's text: `Status: Idle`, `Status: Running…
/// <m:ss>` (a normal bench — the `…` kept static, the 60 FPS repaint
/// animates the elapsed, C7-17), `Status: Running… (burn-in <m:ss>,
/// iter <n>)` (a burn-in, C7-18), `Status: Done`.
fn status_text(state: StatusState) -> String {
    match state {
        StatusState::Idle => "Status: Idle".to_owned(),
        StatusState::Running {
            elapsed_secs,
            burn_in_iteration,
        } => match burn_in_iteration {
            Some(iter) => format!(
                "Status: Running… (burn-in {}, iter {})",
                format_elapsed(elapsed_secs),
                iter
            ),
            None => format!("Status: Running… {}", format_elapsed(elapsed_secs)),
        },
        StatusState::Done => "Status: Done".to_owned(),
    }
}

/// The flat status line's color: the idle line is dim (the caller
/// supplies the UI's theme-relative `weak_text_color` — the status
/// color is not a palette const), the running line CYAN (a normal
/// bench or a burn-in), the done line the zone-local green
/// [`STATUS_DONE`].
fn status_color(state: StatusState, idle_color: egui::Color32) -> egui::Color32 {
    match state {
        StatusState::Idle => idle_color,
        StatusState::Running { .. } => CYAN,
        StatusState::Done => STATUS_DONE,
    }
}

/// One grid cell's semantic color: CRIMSON for an unmeasured cell
/// (`N/A`), AMBER for a latency cell (the palette's low-latency
/// accent), CYAN for the bandwidth values.
fn cell_color(metric: Metric, display: &str) -> egui::Color32 {
    if display.starts_with("N/A") {
        return CRIMSON;
    }
    match metric {
        Metric::Latency => AMBER,
        _ => CYAN,
    }
}

/// The live in-flight grid for a **normal** bench: accumulate each
/// streamed [`StreamProgress`] event's (tier, op, value) into a zero
/// grid — the latest event per cell wins. The (tier, op) → grid-cell
/// mapping: the row is the tier (its discriminant is the grid-array
/// slot: `Memory = 0, L1 = 1, L2 = 2, L3 = 3`) and the column is the
/// op (`Read` → `read_gbps`, `Write` → `write_gbps`, `Copy` →
/// `copy_gbps`); the stream carries bandwidth ops only (no latency
/// events — the streamed.rs contract), so the `latency_ns` column
/// stays 0.0. A non-finite or non-positive value is ignored (never
/// renders as data), and unmeasured cells stay 0.0 (which
/// [`cell_text`] renders as `N/A`). A burn-in's live grid is
/// `BurnInState.latest` instead (C7-18 — see [`table_live_grid`]).
fn live_grid(progress: &[StreamProgress]) -> BenchmarkGrid {
    let mut grid = BenchmarkGrid {
        read_gbps: [0.0; 4],
        write_gbps: [0.0; 4],
        copy_gbps: [0.0; 4],
        latency_ns: [0.0; 4],
    };
    for event in progress {
        if !event.value.is_finite() || event.value <= 0.0 {
            continue; // a malformed reading never renders as data
        }
        let slot = event.tier as usize;
        match event.op {
            BenchOp::Read => grid.read_gbps[slot] = event.value,
            BenchOp::Write => grid.write_gbps[slot] = event.value,
            BenchOp::Copy => grid.copy_gbps[slot] = event.value,
        }
    }
    grid
}

/// One cell's render phase within the zone's current run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellPhase {
    /// The run is not in flight: the cell shows its terminal value
    /// from the result grid (or the `N/A` placeholder when
    /// unmeasured).
    Terminal,
    /// A run is in flight and the cell has a live value: the live
    /// in-flight fill (dimmed CYAN + a `…` suffix).
    Live,
    /// A run is in flight and the cell has no live value yet — not
    /// started, or a latency cell of a normal bench (no progress
    /// events): the `N/A` placeholder.
    NotStarted,
}

/// One cell's render [`CellPhase`]: not in flight → `Terminal` (the
/// result grid); in flight → `Live` when the live grid carries a
/// (finite, positive) value for the cell, else `NotStarted` (a cell
/// not started yet, or a normal-bench latency cell — the stream
/// carries no latency events).
fn cell_phase(in_flight: bool, live: &BenchmarkGrid, tier: Tier, metric: Metric) -> CellPhase {
    if !in_flight {
        return CellPhase::Terminal;
    }
    let value = live.cell(tier, metric);
    if value.is_finite() && value > 0.0 {
        CellPhase::Live
    } else {
        CellPhase::NotStarted
    }
}

/// One cell's display text for its render [`CellPhase`]: the terminal
/// value in the [`cell_text`] form, the live in-flight value (the
/// same unit form + a `…` suffix — the run is still settling the rest
/// of the grid; a bandwidth cell is `GB/s`, a latency cell is `ns` —
/// a burn-in's `latest` also carries per-tier latency ticks, C7-16),
/// or the `N/A` placeholder.
fn phase_cell_text(
    phase: CellPhase,
    grid: &BenchmarkGrid,
    live: &BenchmarkGrid,
    tier: Tier,
    metric: Metric,
) -> String {
    match phase {
        CellPhase::Terminal => cell_text(grid, tier, metric),
        CellPhase::Live => {
            let value = live.cell(tier, metric);
            match metric {
                Metric::Latency => format!("{value:.2} ns…"),
                _ => format!("{value:.2} GB/s…"),
            }
        }
        CellPhase::NotStarted => "N/A".to_owned(),
    }
}

/// One cell's color for its render [`CellPhase`]: the terminal
/// semantics (the existing [`cell_color`]: CYAN / AMBER / CRIMSON),
/// the live fill (dimmed CYAN — visually distinct from the final
/// value's full CYAN), or the not-started placeholder (CRIMSON).
fn phase_cell_color(phase: CellPhase, metric: Metric, text: &str) -> egui::Color32 {
    match phase {
        CellPhase::Terminal => cell_color(metric, text),
        CellPhase::Live => CYAN.gamma_multiply(0.6),
        CellPhase::NotStarted => CRIMSON,
    }
}

/// Whether any run is in flight (a normal bench or a burn-in) — the
/// single-flight state the run buttons key off: they disable while one
/// is in flight, and `Cancel` is shown to stop it (C7-16/18).
fn run_in_flight(running: bool, burn_in_running: bool) -> bool {
    running || burn_in_running
}

/// The 4×4 table's live in-flight grid for the current run (C7-18): a
/// burn-in in flight shows `burn_in.latest` (the newest per-cell values
/// seen this burn-in — the cells update per iteration); a normal bench
/// in flight shows the accumulated `StreamProgress` events
/// ([`live_grid`]); neither in flight → the live grid is unused (the
/// terminal grid renders), so the progress accumulation is returned
/// harmlessly.
fn table_live_grid(bench: &BenchState) -> BenchmarkGrid {
    if bench.burn_in.running {
        bench.burn_in.latest.clone()
    } else {
        live_grid(&bench.progress)
    }
}

/// The burn-in duration clamped to [`BURN_IN_MINUTES_MAX`] (a `u32` is
/// never negative, so only the upper bound bites; `0` = infinite).
fn burn_in_minutes_clamped(minutes: u32) -> u32 {
    minutes.min(BURN_IN_MINUTES_MAX)
}

/// The [`BenchCmd`] the `Run Burn-In` trigger sends for a requested
/// duration (C7-18): a full-scope burn-in (the §3 wire shape —
/// `StartBurnIn { target, duration_minutes }`, D-1: a burn-in is a
/// full-scope duration run) with the duration clamped to
/// [`BURN_IN_MINUTES_MAX`] (`0` = infinite / continuous until manually
/// stopped via `Cancel`).
fn burn_in_cmd(minutes: u32) -> BenchCmd {
    BenchCmd {
        target: StreamTarget::Full,
        mode: BenchMode::Full,
        duration_minutes: Some(burn_in_minutes_clamped(minutes)),
    }
}

/// The live per-iteration row's text (C7-18): the newest tick's
/// bookkeeping + the headline `latest` values —
/// `Burn-in: iteration <n> · <m:ss> elapsed · Memory Read <gbps> ·
/// <latency>`; a cell not yet streamed (0.0 on the wire) reads `N/A`
/// (the [`cell_text`] form). `None` when no burn-in is in flight (the
/// row is absent — no layout shift beyond the status line).
fn burn_in_row(burn_in: &BurnInState) -> Option<String> {
    if !burn_in.running {
        return None;
    }
    let grid = &burn_in.latest;
    Some(format!(
        "Burn-in: iteration {} · {} elapsed · Memory Read {} · {}",
        burn_in.iteration,
        format_elapsed(burn_in.elapsed_secs),
        cell_text(grid, Tier::Memory, Metric::Read),
        cell_text(grid, Tier::Memory, Metric::Latency),
    ))
}

// ---------------------------------------------------------------------
// The egui surface (compile-checked here; the live render is verified
// in the QA phase).
// ---------------------------------------------------------------------

/// Zone 2: render the benchmark grid + status line + run controls
/// from `data` — a titled SLATE frame (the zone 1 precedent) with the
/// `egui_extras::TableBuilder` 4×4 grid, the flat status line
/// (C7-17), the live per-iteration row (C7-18), and the `Run Full` /
/// `Memory Only` / burn-in / `Cancel` buttons.
///
/// `bench_tx` is the poller's [`BenchCmd`] channel (the run buttons
/// send; the poller owns the socket — no render-thread I/O, D6);
/// `cancel` is the shared cancel flag the `Cancel` button sets (shown
/// while any run — a normal bench or a burn-in — is in flight;
/// `run_bench` resets it per run).
pub fn render_bench_zone(
    ui: &mut egui::Ui,
    data: &TelemetryData,
    bench_tx: &Sender<BenchCmd>,
    cancel: &AtomicBool,
) {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0));
    let _ = frame.show(ui, |ui| {
        ui.label(egui::RichText::new(ZONE_TITLE).strong().color(CYAN));
        ui.add_space(4.0);
        render_grid_table(ui, data);
        ui.add_space(4.0);
        render_status(ui, data);
        // The live per-iteration row (C7-18): present only while a
        // burn-in is in flight (absent otherwise — no layout shift
        // beyond the status line).
        if let Some(row) = burn_in_row(&data.bench.burn_in) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(row).color(ui.visuals().weak_text_color()));
        }
        ui.add_space(4.0);
        render_controls(ui, data, bench_tx, cancel);
    });
}

/// The 4×4 `egui_extras::TableBuilder` table: a header row (the four
/// metric names) + one row per tier. Every cell renders its phase:
/// while a run is in flight, a cell with a live value shows the live
/// in-flight fill (dimmed CYAN + a `…` suffix — a normal bench's
/// `StreamProgress` accumulation, or a burn-in's `latest`, C7-18) and
/// the rest keep the `N/A` placeholder; once the run is not in flight,
/// the cells show the terminal result grid's values (CYAN / AMBER /
/// CRIMSON). No terminal grid yet renders the all-`N/A` placeholder
/// (the layout never shifts when the result lands).
fn render_grid_table(ui: &mut egui::Ui, data: &TelemetryData) {
    // The terminal result grid of the last completed run, or the
    // all-`N/A` zero grid when none has landed yet.
    let grid = match data.bench.grid.as_ref() {
        Some(grid) => grid.clone(),
        None => BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        },
    };
    // The live in-flight grid (C7-18): a burn-in shows `burn_in.latest`
    // (the cells update per iteration), a normal bench shows the
    // accumulated `StreamProgress` events.
    let live = table_live_grid(&data.bench);
    // Any run in flight (a normal bench or a burn-in) → the live cells
    // render the in-flight fill.
    let in_flight = run_in_flight(data.bench.running, data.bench.burn_in.running);
    // The 16 per-cell render views (text + color), row-major.
    let mut cells = Vec::with_capacity(16);
    for tier in &TIERS {
        for metric in &METRICS {
            let phase = cell_phase(in_flight, &live, *tier, *metric);
            let text = phase_cell_text(phase, &grid, &live, *tier, *metric);
            let color = phase_cell_color(phase, *metric, &text);
            cells.push((text, color));
        }
    }
    egui_extras::TableBuilder::new(ui)
        .striped(false)
        .column(egui_extras::Column::initial(90.0))
        .columns(egui_extras::Column::initial(70.0), METRICS.len())
        .header(ROW_HEIGHT, |mut header| {
            header.col(|ui| {
                ui.label(egui::RichText::new("tier").strong().color(CYAN));
            });
            for metric in &METRICS {
                header.col(|ui| {
                    ui.label(
                        egui::RichText::new(metric_name(*metric)).strong().color(CYAN),
                    );
                });
            }
        })
        .body(|mut body| {
            for (tier, row_cells) in TIERS.iter().zip(cells.chunks_exact(METRICS.len())) {
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(tier_name(*tier)).strong());
                    });
                    for (text, color) in row_cells.iter() {
                        let text = text.clone();
                        let color = *color;
                        row.col(move |ui| {
                            ui.label(egui::RichText::new(text).color(color));
                        });
                    }
                });
            }
        });
}

/// The zone's local start clock for a normal bench run (C7-17): the
/// moment the zone first saw a normal bench in flight (a repaint
/// within one frame of the run's start — the 60 FPS loop). The
/// plan's `BenchState.run_started` field (the shared state in
/// `update.rs`, outside this chunk's scope boundary —
/// `bench_zone.rs` only) is not added here, so the clock lives
/// zone-local: stamped on the first in-flight frame, cleared on the
/// terminal. The burn-in needs no clock — its newest tick carries
/// the elapsed (`burn_in.elapsed_secs`).
static NORMAL_RUN_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Stamp / clear the zone-local normal-run start clock for this
/// frame's run state, returning the start instant while a normal
/// bench is in flight (`None` outside one — the terminal cleared
/// it): a poisoned lock is recovered in place (the render thread is
/// the sole user; the no-panic contract, D5).
fn track_run_start(running: bool) -> Option<Instant> {
    let mut guard = NORMAL_RUN_START.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if running {
        Some(*guard.get_or_insert_with(Instant::now))
    } else {
        *guard = None;
        None
    }
}

/// The bench zone's current [`StatusState`] from the snapshot: a
/// burn-in in flight elapses from the newest tick (and carries its
/// iteration), a normal bench in flight elapses from the zone's start
/// clock (the frame the run was first seen in flight — ≤ one repaint
/// of the true start).
fn bench_status_state(data: &TelemetryData) -> StatusState {
    let running = data.bench.running;
    let burn_in = &data.bench.burn_in;
    let running_elapsed = if burn_in.running {
        burn_in.elapsed_secs
    } else {
        track_run_start(running).map_or(0.0, |start| start.elapsed().as_secs_f64())
    };
    status_state(
        running,
        burn_in.running,
        data.bench.grid.is_some(),
        !data.bench.progress.is_empty(),
        running_elapsed,
        burn_in.iteration,
    )
}

/// The flat status line (C7-17 — the progress bar's pill retired): a
/// single text line, no frame, no border, no button shape —
/// `Status: Idle` dim, `Status: Running… <m:ss>` CYAN (a normal bench
/// — the `…` kept static, the 60 FPS repaint animates the elapsed),
/// `Status: Running… (burn-in <m:ss>, iter <n>)` CYAN (a burn-in,
/// C7-18), `Status: Done` in the zone-local green.
fn render_status(ui: &mut egui::Ui, data: &TelemetryData) {
    let state = bench_status_state(data);
    let text = status_text(state);
    let color = status_color(state, ui.visuals().weak_text_color());
    ui.label(egui::RichText::new(text).color(color));
}

/// The run controls: `Run Full` / `Memory Only` send a [`BenchCmd`] to
/// the poller (fire-and-forget — the poller owns the socket, D6), the
/// burn-in's minutes knob (`0` = infinite, clamped to
/// [`BURN_IN_MINUTES_MAX`]) + `Run Burn-In` send a burn-in
/// [`BenchCmd`] (C7-18), all the run buttons disable while any run is
/// in flight (single-flight), and `Cancel` — shown while any run is in
/// flight (a normal bench or a burn-in) — sets the shared cancel flag
/// the poller checks between frames (P3-28 / C7-16).
fn render_controls(
    ui: &mut egui::Ui,
    data: &TelemetryData,
    bench_tx: &Sender<BenchCmd>,
    cancel: &AtomicBool,
) {
    // Any run in flight (a normal bench or a burn-in) — the run
    // buttons disable while one is in flight (single-flight, C7-18),
    // and `Cancel` is shown to stop it.
    let in_flight = run_in_flight(data.bench.running, data.bench.burn_in.running);
    // The burn-in duration knob (minutes, C7-18): a transient UI value
    // in the context's temp data (per-context, session-local; `0` =
    // infinite, clamped to [`BURN_IN_MINUTES_MAX`]). It is read out
    // (falling back to the default on first use), edited by the
    // `DragValue`, and written back — so the value survives across
    // frames without holding a context borrow across the layout.
    let minutes_id = egui::Id::new("ramsleuth_burnin_minutes");
    let mut minutes: u32 = BURN_IN_MINUTES_DEFAULT;
    ui.data(|d| {
        if let Some(stored) = d.get_temp::<u32>(minutes_id) {
            minutes = stored;
        }
    });

    ui.horizontal(|ui| {
        // The run buttons: disabled while any run is in flight.
        if ui
            .add_enabled(!in_flight, egui::Button::new("Run Full"))
            .clicked()
        {
            let _ = bench_tx.send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
                duration_minutes: None,
            });
        }
        if ui
            .add_enabled(!in_flight, egui::Button::new("Memory Only"))
            .clicked()
        {
            let _ = bench_tx.send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::MemoryOnly,
                duration_minutes: None,
            });
        }
        // The burn-in duration (minutes; `0` = infinite) + the Run
        // Burn-In trigger (disabled while any run is in flight).
        ui.add_enabled(
            !in_flight,
            egui::DragValue::new(&mut minutes)
                .clamp_range(0..=BURN_IN_MINUTES_MAX)
                .suffix(" min (0 = ∞)"),
        );
        if ui
            .add_enabled(!in_flight, egui::Button::new("Run Burn-In"))
            .clicked()
        {
            let _ = bench_tx.send(burn_in_cmd(minutes));
        }
        // Cancel: shown while any run is in flight (a normal bench or
        // a burn-in); it sets the shared flag the poller checks, which
        // stops whichever run is in flight (C7-16/18).
        if in_flight && ui.button("Cancel").clicked() {
            cancel.store(true, Ordering::Relaxed);
        }
    });

    // Persist the (possibly dragged) minutes value back to the context.
    ui.data_mut(|d| {
        d.insert_temp(minutes_id, minutes);
    });
}

// ---------------------------------------------------------------------
// Tests (headless: the pure core — no egui context, no I/O; the
// render path is compile-checked and verified live in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_bench::BenchOp;

    use super::*;

    /// A small mixed grid: most cells measured, two unmeasured
    /// (0.0 on the wire → `N/A`).
    fn fixture_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 0.0],
            write_gbps: [43.63, 38.20, 0.0, 14.02],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        }
    }

    /// (a) `grid_cells` returns exactly the 16 row-major
    /// `(tier · op, value-or-N/A)` pairs: formatted values for the
    /// measured cells, `N/A` for the unmeasured ones, and no panic on
    /// an all-zero grid.
    #[test]
    fn grid_cells_returns_16_value_or_na_pairs() {
        let cells = grid_cells(&fixture_grid());
        assert_eq!(cells.len(), 16, "the 4×4 grid is 16 cells");

        // Row-major: the Memory row first, the four metrics in order.
        assert_eq!(cells[0].0, "Memory · Read");
        assert_eq!(cells[0].1, "26.35 GB/s");
        assert_eq!(cells[1].1, "43.63 GB/s");
        assert_eq!(cells[2].1, "12.11 GB/s");
        assert_eq!(cells[3].1, "86.84 ns");

        // The L1 row (indices 4..8) and the L2 · Write `N/A`.
        assert_eq!(cells[4].0, "L1 · Read");
        assert_eq!(cells[7].0, "L1 · Latency");
        assert_eq!(cells[7].1, "1.12 ns");
        assert_eq!(cells[9], ("L2 · Write".to_owned(), "N/A".to_owned()));

        // The L3 row (indices 12..16): `L3 · Read` unmeasured.
        assert_eq!(cells[12], ("L3 · Read".to_owned(), "N/A".to_owned()));
        assert_eq!(cells[15].0, "L3 · Latency");
        assert_eq!(cells[15].1, "13.90 ns");

        // The all-zero grid: every cell `N/A`, no panic.
        let empty = grid_cells(&BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        });
        assert_eq!(empty.len(), 16);
        assert!(
            empty.iter().all(|(_, display)| display == "N/A"),
            "all-zero grid -> all N/A"
        );
    }

    /// (b) Determinism: two calls on the same grid are equal.
    #[test]
    fn grid_cells_is_deterministic() {
        assert_eq!(grid_cells(&fixture_grid()), grid_cells(&fixture_grid()));
    }

    /// (c) `cell_text`: the unit form per metric (GB/s for bandwidth,
    /// ns for latency), `N/A` for an unmeasured / non-finite reading
    /// (no panic).
    #[test]
    fn cell_text_formats_values_and_na() {
        let grid = fixture_grid();
        assert_eq!(cell_text(&grid, Tier::Memory, Metric::Read), "26.35 GB/s");
        assert_eq!(cell_text(&grid, Tier::L3, Metric::Latency), "13.90 ns");
        assert_eq!(cell_text(&grid, Tier::L2, Metric::Write), "N/A");

        let broken = BenchmarkGrid {
            read_gbps: [f64::NAN; 4],
            write_gbps: [f64::INFINITY; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        assert_eq!(cell_text(&broken, Tier::Memory, Metric::Read), "N/A");
        assert_eq!(cell_text(&broken, Tier::L1, Metric::Write), "N/A");
    }

    /// (d) `format_elapsed`: the `m:ss` form (the plan's `0:42`) —
    /// the minutes un-padded, the seconds two-digit; a non-finite /
    /// non-positive reading renders `0:00` (a bad elapsed never
    /// panics the line, D5).
    #[test]
    fn format_elapsed_renders_minutes_and_seconds() {
        assert_eq!(format_elapsed(0.0), "0:00");
        assert_eq!(format_elapsed(59.9), "0:59");
        assert_eq!(format_elapsed(60.0), "1:00");
        assert_eq!(format_elapsed(125.0), "2:05");
        assert_eq!(format_elapsed(3661.0), "61:01");
        assert_eq!(format_elapsed(f64::NAN), "0:00", "NaN never panics");
        assert_eq!(format_elapsed(-4.0), "0:00", "a negative elapsed renders zero");
        assert_eq!(format_elapsed(f64::INFINITY), "0:00", "+inf never panics");
    }

    /// (e) `status_state`: the flat line's run bookkeeping (C7-17)
    /// — a burn-in in flight → `Running` (the newest tick's elapsed +
    /// iteration — the normal-bench flag stays false for a burn-in,
    /// C7-16), a normal bench in flight → `Running` (the zone's start
    /// clock's elapsed, no burn-in iteration), a finished run (a
    /// terminal grid or kept streamed progress) → `Done`, the fresh
    /// state → `Idle`.
    #[test]
    fn status_state_tracks_the_run_states() {
        // The fresh state: no run in flight, no grid, no progress.
        assert_eq!(
            status_state(false, false, false, false, 0.0, 0),
            StatusState::Idle
        );
        // A normal bench in flight (progress streaming): `Running`
        // with the start clock's elapsed — 42 s renders `0:42` (no
        // burn-in iteration).
        assert_eq!(
            status_state(true, false, false, true, 42.0, 0),
            StatusState::Running {
                elapsed_secs: 42.0,
                burn_in_iteration: None
            }
        );
        // A burn-in in flight: `Running` with the newest tick's
        // elapsed (125 s) + iteration (the normal-bench flag stays
        // false, C7-16) — the `(burn-in <m:ss>, iter <n>)` line.
        assert_eq!(
            status_state(false, true, false, false, 125.0, 3),
            StatusState::Running {
                elapsed_secs: 125.0,
                burn_in_iteration: Some(3)
            }
        );
        // A burn-in still in flight wins over the kept terminal grid
        // of an earlier run.
        assert_eq!(
            status_state(false, true, true, false, 9.0, 2),
            StatusState::Running {
                elapsed_secs: 9.0,
                burn_in_iteration: Some(2)
            }
        );
        // A finished run: the terminal grid landed (the burn-in
        // terminal leaves the progress empty — C7-16) → `Done`.
        assert_eq!(
            status_state(false, false, true, false, 0.0, 0),
            StatusState::Done
        );
        // A finished run: the streamed progress is kept with no grid
        // (a cancelled normal bench) → `Done` (the old `done`
        // label's corner).
        assert_eq!(
            status_state(false, false, false, true, 0.0, 0),
            StatusState::Done
        );
    }

    /// (f) `status_text`: the flat line's exact strings —
    /// `Status: Idle`, `Status: Running… <m:ss>` (a normal bench —
    /// the `…` kept static, the 60 FPS repaint animates the elapsed),
    /// `Status: Running… (burn-in <m:ss>, iter <n>)` (a burn-in,
    /// C7-18), `Status: Done`.
    #[test]
    fn status_text_renders_the_flat_lines() {
        assert_eq!(status_text(StatusState::Idle), "Status: Idle");
        // A normal bench in flight: `Status: Running… <m:ss>`.
        assert_eq!(
            status_text(StatusState::Running {
                elapsed_secs: 42.0,
                burn_in_iteration: None,
            }),
            "Status: Running… 0:42"
        );
        assert_eq!(
            status_text(StatusState::Running {
                elapsed_secs: 125.0,
                burn_in_iteration: None,
            }),
            "Status: Running… 2:05"
        );
        assert_eq!(
            status_text(StatusState::Running {
                elapsed_secs: 0.0,
                burn_in_iteration: None,
            }),
            "Status: Running… 0:00"
        );
        // A burn-in in flight: the `(burn-in <m:ss>, iter <n>)`
        // phrasing (C7-18).
        assert_eq!(
            status_text(StatusState::Running {
                elapsed_secs: 125.0,
                burn_in_iteration: Some(3),
            }),
            "Status: Running… (burn-in 2:05, iter 3)"
        );
        assert_eq!(
            status_text(StatusState::Running {
                elapsed_secs: 0.0,
                burn_in_iteration: Some(0),
            }),
            "Status: Running… (burn-in 0:00, iter 0)"
        );
        assert_eq!(status_text(StatusState::Done), "Status: Done");
    }

    /// (g) `status_color`: the idle line is the caller's dim
    /// (theme-relative `weak_text_color`, passed through), the running
    /// line CYAN (a normal bench or a burn-in), the done line the
    /// zone-local green (the `style.rs` palette stays frozen —
    /// C7-17).
    #[test]
    fn status_color_tracks_the_state() {
        let dim = egui::Color32::from_rgb(0x80, 0x80, 0x80);
        assert_eq!(
            status_color(StatusState::Idle, dim),
            dim,
            "the idle line is the dim color"
        );
        // Both a normal bench and a burn-in in flight render CYAN.
        assert_eq!(
            status_color(
                StatusState::Running {
                    elapsed_secs: 42.0,
                    burn_in_iteration: None,
                },
                dim
            ),
            CYAN
        );
        assert_eq!(
            status_color(
                StatusState::Running {
                    elapsed_secs: 125.0,
                    burn_in_iteration: Some(3),
                },
                dim
            ),
            CYAN
        );
        assert_eq!(status_color(StatusState::Done, dim), STATUS_DONE);
        assert_eq!(
            (STATUS_DONE.r(), STATUS_DONE.g(), STATUS_DONE.b()),
            (0x2E, 0x9E, 0x5B),
            "the done green is the zone-local const"
        );
    }

    /// (h) The semantic cell color: CYAN for the bandwidth values,
    /// AMBER for a latency cell, CRIMSON for an unmeasured cell.
    #[test]
    fn cell_color_semantics() {
        assert_eq!(cell_color(Metric::Read, "26.35 GB/s"), CYAN);
        assert_eq!(cell_color(Metric::Latency, "86.84 ns"), AMBER);
        assert_eq!(cell_color(Metric::Copy, "N/A"), CRIMSON);
    }

    /// One synthetic streamed progress event (the label is unused by
    /// the pure core).
    fn progress_event(tier: Tier, op: BenchOp, value: f64) -> StreamProgress {
        StreamProgress {
            cell_index: 0,
            total_cells: 12,
            tier,
            op,
            value,
            label: "test (GB/s)".to_owned(),
        }
    }

    /// (i) `live_grid`: each streamed event fills its (tier, op)
    /// cell; the latest event per cell wins; the latency column and
    /// the unmeasured cells stay 0.0.
    #[test]
    fn live_grid_fills_streamed_cells_latest_wins() {
        let events = vec![
            progress_event(Tier::Memory, BenchOp::Read, 26.0),
            progress_event(Tier::Memory, BenchOp::Write, 43.0),
            progress_event(Tier::L1, BenchOp::Read, 35.0),
            progress_event(Tier::Memory, BenchOp::Read, 27.5), // latest wins
        ];
        let grid = live_grid(&events);
        assert_eq!(grid.cell(Tier::Memory, Metric::Read), 27.5, "latest per cell wins");
        assert_eq!(grid.cell(Tier::Memory, Metric::Write), 43.0);
        assert_eq!(grid.cell(Tier::L1, Metric::Read), 35.0);
        // Unmeasured cells — and the whole latency column (no
        // progress events) — stay 0.0.
        assert_eq!(grid.cell(Tier::Memory, Metric::Copy), 0.0);
        assert_eq!(grid.cell(Tier::L2, Metric::Read), 0.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Copy), 0.0);
        assert_eq!(grid.cell(Tier::L1, Metric::Latency), 0.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Latency), 0.0);
    }

    /// (j) `live_grid`: an empty stream and a stream of malformed
    /// (non-finite / non-positive) values never panic and never
    /// render as data — every cell stays 0.0.
    #[test]
    fn live_grid_empty_and_malformed_streams_stay_zero() {
        let empty = live_grid(&[]);
        assert_eq!(empty.cell(Tier::Memory, Metric::Read), 0.0);
        assert_eq!(empty.cell(Tier::L3, Metric::Latency), 0.0);

        let broken = vec![
            progress_event(Tier::L1, BenchOp::Read, f64::NAN),
            progress_event(Tier::L2, BenchOp::Write, f64::INFINITY),
            progress_event(Tier::L3, BenchOp::Copy, -4.0),
        ];
        let grid = live_grid(&broken);
        assert_eq!(grid.cell(Tier::L1, Metric::Read), 0.0, "NaN never renders");
        assert_eq!(grid.cell(Tier::L2, Metric::Write), 0.0, "+inf never renders");
        assert_eq!(grid.cell(Tier::L3, Metric::Copy), 0.0, "negative never renders");
    }

    /// (k) `cell_phase`: not in flight → `Terminal` for every cell
    /// (the result grid); in flight → `Live` only for the cells the
    /// live grid carries a value for, `NotStarted` otherwise (a cell
    /// not started yet, or a latency cell — no progress events).
    #[test]
    fn cell_phase_tracks_running_and_streamed_values() {
        let live = live_grid(&[
            progress_event(Tier::Memory, BenchOp::Read, 26.0),
            progress_event(Tier::L1, BenchOp::Copy, 31.8),
        ]);
        // Not in flight: every cell renders its terminal value.
        for tier in &TIERS {
            for metric in &METRICS {
                assert_eq!(
                    cell_phase(false, &live, *tier, *metric),
                    CellPhase::Terminal
                );
            }
        }
        // In flight: the streamed cells are live, the rest not started.
        assert_eq!(
            cell_phase(true, &live, Tier::Memory, Metric::Read),
            CellPhase::Live
        );
        assert_eq!(
            cell_phase(true, &live, Tier::L1, Metric::Copy),
            CellPhase::Live
        );
        assert_eq!(
            cell_phase(true, &live, Tier::Memory, Metric::Write),
            CellPhase::NotStarted
        );
        assert_eq!(
            cell_phase(true, &live, Tier::L2, Metric::Read),
            CellPhase::NotStarted
        );
        assert_eq!(
            cell_phase(true, &live, Tier::L3, Metric::Latency),
            CellPhase::NotStarted
        );
    }

    /// (l) The phase render: terminal cells keep the existing
    /// [`cell_text`] / [`cell_color`] semantics; live cells show the
    /// in-flight value + a `…` suffix in a dimmed CYAN (distinct from
    /// the final value's full CYAN) — a bandwidth cell in `GB/s`, a
    /// latency cell in `ns` (a burn-in's `latest` carries latency
    /// ticks, C7-16); not-started cells keep the `N/A` placeholder in
    /// CRIMSON.
    #[test]
    fn phase_text_and_color_render_live_and_terminal_cells() {
        let grid = fixture_grid();
        let live = live_grid(&[progress_event(Tier::Memory, BenchOp::Read, 26.0)]);

        // Terminal: the existing semantics.
        let text = phase_cell_text(CellPhase::Terminal, &grid, &live, Tier::Memory, Metric::Read);
        assert_eq!(text, "26.35 GB/s");
        assert_eq!(phase_cell_color(CellPhase::Terminal, Metric::Read, &text), CYAN);
        let text =
            phase_cell_text(CellPhase::Terminal, &grid, &live, Tier::Memory, Metric::Latency);
        assert_eq!(text, "86.84 ns");
        assert_eq!(phase_cell_color(CellPhase::Terminal, Metric::Latency, &text), AMBER);
        let text = phase_cell_text(CellPhase::Terminal, &grid, &live, Tier::L2, Metric::Write);
        assert_eq!(text, "N/A");
        assert_eq!(phase_cell_color(CellPhase::Terminal, Metric::Write, &text), CRIMSON);

        // Live: the in-flight value + the `…` suffix, dimmed.
        let text = phase_cell_text(CellPhase::Live, &grid, &live, Tier::Memory, Metric::Read);
        assert_eq!(text, "26.00 GB/s…");
        let color = phase_cell_color(CellPhase::Live, Metric::Read, &text);
        assert_eq!(color, CYAN.gamma_multiply(0.6), "the live fill is dimmed");
        assert_ne!(color, CYAN, "the live fill is distinct from the final value");

        // A burn-in's `latest` also carries per-tier latency ticks →
        // the Live latency cell shows `ns…` (not `GB/s…`).
        let live_lat = BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [86.84, 0.0, 0.0, 0.0],
        };
        assert_eq!(
            phase_cell_text(CellPhase::Live, &grid, &live_lat, Tier::Memory, Metric::Latency),
            "86.84 ns…"
        );

        // NotStarted: the `N/A` placeholder.
        assert_eq!(
            phase_cell_text(CellPhase::NotStarted, &grid, &live, Tier::L3, Metric::Read),
            "N/A"
        );
        assert_eq!(phase_cell_color(CellPhase::NotStarted, Metric::Read, "N/A"), CRIMSON);
    }

    /// (m) `burn_in_cmd`: the `Run Burn-In` trigger's [`BenchCmd`] —
    /// a full-scope burn-in (the §3 wire shape) with the requested
    /// `duration_minutes` (`0` = infinite).
    #[test]
    fn burn_in_cmd_sends_the_duration_bearing_cmd() {
        for minutes in [0, 5, 90, 1440] {
            let cmd = burn_in_cmd(minutes);
            assert_eq!(cmd.target, StreamTarget::Full, "a full-scope burn-in");
            assert_eq!(cmd.mode, BenchMode::Full, "a full-scope burn-in");
            assert_eq!(
                cmd.duration_minutes,
                Some(minutes),
                "the requested minutes ride the cmd"
            );
        }
    }

    /// (n) `burn_in_cmd` / `burn_in_minutes_clamped`: the minutes are
    /// clamped to the `0..=1440` range (a value above the max is
    /// capped; `0` stays `0` = infinite).
    #[test]
    fn burn_in_cmd_clamps_the_minutes() {
        assert_eq!(burn_in_minutes_clamped(0), 0);
        assert_eq!(burn_in_minutes_clamped(1440), 1440);
        assert_eq!(burn_in_minutes_clamped(9999), BURN_IN_MINUTES_MAX);
        assert_eq!(burn_in_cmd(9999).duration_minutes, Some(BURN_IN_MINUTES_MAX));
        assert_eq!(burn_in_cmd(0).duration_minutes, Some(0));
    }

    /// (o) `burn_in_row`: the live per-iteration row over a synthetic
    /// [`BurnInState`] — the newest tick's iteration + elapsed + the
    /// headline `latest` values; absent cells read `N/A`; `None` when
    /// no burn-in is in flight (the row is absent).
    #[test]
    fn burn_in_row_renders_the_per_iteration_values() {
        // No burn-in in flight → the row is absent.
        let idle = BurnInState::default();
        assert!(!idle.running);
        assert_eq!(burn_in_row(&idle), None);

        // A burn-in in flight, no ticks yet (zero `latest`) → the
        // `N/A` parts.
        let fresh = BurnInState {
            running: true,
            iteration: 0,
            elapsed_secs: 0.0,
            latest: BenchmarkGrid {
                read_gbps: [0.0; 4],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            },
        };
        assert_eq!(
            burn_in_row(&fresh),
            Some("Burn-in: iteration 0 · 0:00 elapsed · Memory Read N/A · N/A".to_owned())
        );

        // A burn-in in flight with the headline cells streamed.
        let live = BurnInState {
            running: true,
            iteration: 3,
            elapsed_secs: 125.0,
            latest: BenchmarkGrid {
                read_gbps: [26.35, 0.0, 0.0, 0.0],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [86.84, 0.0, 0.0, 0.0],
            },
        };
        assert_eq!(
            burn_in_row(&live),
            Some(
                "Burn-in: iteration 3 · 2:05 elapsed · Memory Read 26.35 GB/s · 86.84 ns"
                    .to_owned()
            )
        );
    }

    /// (p) `run_in_flight`: the single-flight state the run buttons
    /// key off — a normal bench, a burn-in, or both in flight.
    #[test]
    fn run_in_flight_tracks_the_single_flight_state() {
        assert!(!run_in_flight(false, false), "idle");
        assert!(run_in_flight(true, false), "a normal bench in flight");
        assert!(run_in_flight(false, true), "a burn-in in flight");
        assert!(run_in_flight(true, true), "both in flight");
    }

    /// (q) `table_live_grid`: the 4×4 table's live grid — a burn-in in
    /// flight shows `burn_in.latest` (the cells update per iteration),
    /// a normal bench in flight shows the accumulated `StreamProgress`
    /// events, neither shows the (unused) progress accumulation.
    #[test]
    fn table_live_grid_shows_the_burn_in_latest() {
        // A burn-in in flight: the live grid is `burn_in.latest`.
        let latest = BenchmarkGrid {
            read_gbps: [26.35, 35.10, 0.0, 0.0],
            write_gbps: [43.63, 0.0, 0.0, 0.0],
            copy_gbps: [0.0; 4],
            latency_ns: [86.84, 1.12, 0.0, 0.0],
        };
        let bench = BenchState {
            running: false,
            burn_in: BurnInState {
                running: true,
                iteration: 2,
                elapsed_secs: 90.0,
                latest: latest.clone(),
            },
            ..Default::default()
        };
        assert_eq!(table_live_grid(&bench), latest, "a burn-in shows `latest`");

        // A normal bench in flight: the live grid is the accumulated
        // `StreamProgress` events.
        let bench = BenchState {
            running: true,
            burn_in: BurnInState::default(),
            progress: vec![progress_event(Tier::Memory, BenchOp::Read, 26.0)],
            ..Default::default()
        };
        let live = table_live_grid(&bench);
        assert_eq!(live.cell(Tier::Memory, Metric::Read), 26.0);
        assert_eq!(live.cell(Tier::L1, Metric::Read), 0.0, "unmeasured cells stay 0.0");

        // Neither in flight: the progress accumulation (harmless — the
        // terminal grid renders).
        let bench = BenchState::default();
        let live = table_live_grid(&bench);
        assert_eq!(live.cell(Tier::Memory, Metric::Read), 0.0);
    }

    /// One headless frame rendering the full zone (the `begin_frame`
    /// pattern, the `history` module's precedent): a fresh context + a
    /// fresh `bench_tx` channel + cancel flag, the zone in a central
    /// panel. The 10000×10000 default screen rect gives every widget a
    /// real area, so the full paint path executes — what is under test
    /// is the no-panic contract (D5).
    fn render_bench_headless(data: &TelemetryData) {
        let (tx, _rx) = std::sync::mpsc::channel::<BenchCmd>();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            render_bench_zone(ui, data, &tx, &cancel);
        });
    }

    /// (r) The full zone renders headless without panicking across the
    /// run states (the D5 no-panic contract): idle, a normal bench in
    /// flight, a burn-in in flight (the per-iteration row + the
    /// disabled controls + the burn-in status phrasing), and a
    /// finished burn-in (the terminal grid + `Done`).
    #[test]
    fn render_bench_zone_runs_headless_without_panicking() {
        // Idle: the all-`N/A` placeholder + `Status: Idle`.
        render_bench_headless(&TelemetryData::default());

        // A normal bench in flight: the live `StreamProgress` cells.
        let mut normal = TelemetryData::default();
        normal.bench.running = true;
        normal.bench.progress = vec![
            progress_event(Tier::Memory, BenchOp::Read, 26.0),
            progress_event(Tier::L1, BenchOp::Copy, 31.8),
        ];
        render_bench_headless(&normal);

        // A burn-in in flight: the per-iteration row + the live
        // `latest` cells + the `(burn-in …, iter …)` status phrasing.
        let mut burn = TelemetryData::default();
        burn.bench.burn_in = BurnInState {
            running: true,
            iteration: 4,
            elapsed_secs: 245.0,
            latest: BenchmarkGrid {
                read_gbps: [26.35, 35.10, 30.40, 0.0],
                write_gbps: [43.63, 38.20, 0.0, 0.0],
                copy_gbps: [12.11, 0.0, 0.0, 0.0],
                latency_ns: [86.84, 1.12, 4.20, 0.0],
            },
        };
        render_bench_headless(&burn);

        // A finished burn-in: the terminal grid landed + `Done`.
        let mut done = burn.clone();
        done.bench.burn_in.running = false;
        done.bench.grid = Some(BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 22.0],
            write_gbps: [43.63, 38.20, 41.0, 14.0],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        });
        render_bench_headless(&done);
    }
}
