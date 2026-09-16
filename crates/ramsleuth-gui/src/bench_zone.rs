//! Zone 2 renderer: the AIDA64-style 4×4 benchmark grid + run controls
//! + a flat status line (C7-17 — the P3-28 progress pill retired).
//!
//! Grand Design §3.1 right panel: the 4×4 grid — Memory / L1 / L2 / L3
//! rows × Read / Write / Copy / Latency columns. The grid is **live
//! during a run**: [`live_grid`] accumulates each streamed
//! [`StreamProgress`] event's (tier, op, value) — the latest per cell
//! wins — and every measured cell renders its in-flight value dimmed
//! with a `…` suffix as the events arrive (egui repaints every
//! frame); the cells not started yet — and the latency column, the
//! stream carrying no latency events — keep the `N/A` placeholder.
//! Once the run is not in flight, the grid shows the terminal result
//! grid of the last completed run, every cell its formatted value
//! (two-decimal GB/s for a bandwidth metric, ns for latency) or `N/A`
//! for an unmeasured cell (0.0 on the wire), in an
//! `egui_extras::TableBuilder` table (bandwidth values CYAN, latency
//! cells AMBER — the palette's low-latency accent — `N/A` CRIMSON); a
//! flat status line (C7-17 — the progress bar's pill retired):
//! `Status: Idle` dim at rest, `Status: Running… <m:ss>` CYAN while a
//! run is in flight (a burn-in's elapsed from the newest tick, a
//! normal bench's from the zone's local start clock — the `…` kept
//! static, the 60 FPS repaint animates the elapsed), `Status: Done`
//! in a zone-local green after a run finished (a terminal grid or
//! kept progress); and the run controls — `Run Full` + `Memory Only`
//! send a [`BenchCmd`] to the background poller (the poller owns the
//! socket, D6: no render-thread I/O), and `Cancel` — shown only while
//! a run is in flight — sets the shared cancel flag the poller's
//! `run_bench` checks between frames (P3-28).
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
//! pairs of the terminal result grid), [`live_grid`] (the in-flight
//! grid accumulated from the streamed events — the latest value per
//! cell wins), and the flat status line's view (C7-17:
//! [`status_state`] / [`status_text`] / [`status_color`] /
//! [`format_elapsed`]) are I/O-free and deterministic — the unit
//! tests exercise them without an egui context; [`render_bench_zone`]
//! is the thin `egui` surface over them (the live render is verified
//! in the QA phase).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Instant;

use ramsleuth_bench::{BenchOp, BenchmarkGrid, Metric, StreamProgress, StreamTarget, Tier};
use ramsleuth_protocol::BenchMode;

use crate::update::{BenchCmd, TelemetryData};
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
    /// `Status: Running… <m:ss>` line; `elapsed_secs` is the run's
    /// elapsed (the newest tick's for a burn-in, the zone's start
    /// clock's for a normal bench).
    Running { elapsed_secs: f64 },
    /// A run finished (a terminal grid landed, or streamed progress
    /// is kept): the green `Status: Done` line.
    Done,
}

/// The status state of one run bookkeeping: a burn-in in flight →
/// `Running` (elapsed from the newest tick), a normal bench in
/// flight → `Running` (elapsed from the zone's start clock), neither
/// in flight but a terminal grid or kept streamed progress present
/// → `Done`, the fresh state (no grid, no progress) → `Idle`.
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
) -> StatusState {
    if burn_in_running || running {
        StatusState::Running { elapsed_secs: running_elapsed_secs }
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
/// <m:ss>` (the `…` kept static — the 60 FPS repaint animates the
/// elapsed, C7-17), `Status: Done`.
fn status_text(state: StatusState) -> String {
    match state {
        StatusState::Idle => "Status: Idle".to_owned(),
        StatusState::Running { elapsed_secs } => {
            format!("Status: Running… {}", format_elapsed(elapsed_secs))
        }
        StatusState::Done => "Status: Done".to_owned(),
    }
}

/// The flat status line's color: the idle line is dim (the caller
/// supplies the UI's theme-relative `weak_text_color` — the status
/// color is not a palette const), the running line CYAN, the done
/// line the zone-local green [`STATUS_DONE`].
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

/// The live in-flight grid: accumulate each streamed
/// [`StreamProgress`] event's (tier, op, value) into a zero grid —
/// the latest event per cell wins. The (tier, op) → grid-cell
/// mapping: the row is the tier (its discriminant is the grid-array
/// slot: `Memory = 0, L1 = 1, L2 = 2, L3 = 3`) and the column is the
/// op (`Read` → `read_gbps`, `Write` → `write_gbps`, `Copy` →
/// `copy_gbps`); the stream carries bandwidth ops only (no latency
/// events — the streamed.rs contract), so the `latency_ns` column
/// stays 0.0. A non-finite or non-positive value is ignored (never
/// renders as data), and unmeasured cells stay 0.0 (which
/// [`cell_text`] renders as `N/A`).
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
    /// A run is in flight and the cell has a streamed value: the live
    /// in-flight fill (dimmed CYAN + a `…` suffix).
    Live,
    /// A run is in flight and the cell has no streamed value yet —
    /// not started, or a latency cell (no progress events): the
    /// `N/A` placeholder.
    NotStarted,
}

/// One cell's render [`CellPhase`]: not running → `Terminal` (the
/// result grid); running → `Live` when the live grid carries a
/// (finite, positive) streamed value for the cell, else
/// `NotStarted` (a cell not started yet, or a latency cell — the
/// stream carries no latency events).
fn cell_phase(running: bool, live: &BenchmarkGrid, tier: Tier, metric: Metric) -> CellPhase {
    if !running {
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
/// same GB/s form + a `…` suffix — the run is still settling the
/// rest of the grid), or the `N/A` placeholder.
fn phase_cell_text(
    phase: CellPhase,
    grid: &BenchmarkGrid,
    live: &BenchmarkGrid,
    tier: Tier,
    metric: Metric,
) -> String {
    match phase {
        CellPhase::Terminal => cell_text(grid, tier, metric),
        CellPhase::Live => format!("{:.2} GB/s…", live.cell(tier, metric)),
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

// ---------------------------------------------------------------------
// The egui surface (compile-checked here; the live render is verified
// in the QA phase).
// ---------------------------------------------------------------------

/// Zone 2: render the benchmark grid + status line + run controls
/// from `data` — a titled SLATE frame (the zone 1 precedent) with the
/// `egui_extras::TableBuilder` 4×4 grid, the flat status line
/// (C7-17), and the `Run Full` / `Memory Only` / `Cancel` buttons.
///
/// `bench_tx` is the poller's [`BenchCmd`] channel (the run buttons
/// send; the poller owns the socket — no render-thread I/O, D6);
/// `cancel` is the shared cancel flag the `Cancel` button sets (shown
/// only while `data.bench.running`; `run_bench` resets it per run).
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
        ui.add_space(4.0);
        render_controls(ui, data, bench_tx, cancel);
    });
}

/// The 4×4 `egui_extras::TableBuilder` table: a header row (the four
/// metric names) + one row per tier. Every cell renders its phase:
/// while a run is in flight, a cell with a streamed value shows the
/// live in-flight fill (dimmed CYAN + a `…` suffix, [`live_grid`])
/// and the rest keep the `N/A` placeholder; once the run is not in
/// flight, the cells show the terminal result grid's values (CYAN /
/// AMBER / CRIMSON). No terminal grid yet renders the all-`N/A`
/// placeholder (the layout never shifts when the result lands).
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
    // The live in-flight grid: the streamed events' latest value per
    // cell (bandwidth ops only — the latency column stays 0.0).
    let live = live_grid(&data.bench.progress);
    // The 16 per-cell render views (text + color), row-major.
    let mut cells = Vec::with_capacity(16);
    for tier in &TIERS {
        for metric in &METRICS {
            let phase = cell_phase(data.bench.running, &live, *tier, *metric);
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
/// burn-in in flight elapses from the newest tick, a normal bench in
/// flight elapses from the zone's start clock (the frame the run was
/// first seen in flight — ≤ one repaint of the true start).
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
    )
}

/// The flat status line (C7-17 — the progress bar's pill retired): a
/// single text line, no frame, no border, no button shape —
/// `Status: Idle` dim, `Status: Running… <m:ss>` CYAN (the `…` kept
/// static — the 60 FPS repaint animates the elapsed), `Status: Done`
/// in the zone-local green.
fn render_status(ui: &mut egui::Ui, data: &TelemetryData) {
    let state = bench_status_state(data);
    let text = status_text(state);
    let color = status_color(state, ui.visuals().weak_text_color());
    ui.label(egui::RichText::new(text).color(color));
}

/// The run controls: `Run Full` / `Memory Only` send a [`BenchCmd`]
/// to the poller (fire-and-forget — the poller owns the socket, D6),
/// and `Cancel` — shown only while a run is in flight — sets the
/// shared cancel flag `run_bench` checks between frames (P3-28).
fn render_controls(
    ui: &mut egui::Ui,
    data: &TelemetryData,
    bench_tx: &Sender<BenchCmd>,
    cancel: &AtomicBool,
) {
    ui.horizontal(|ui| {
        if ui.button("Run Full").clicked() {
            let _ = bench_tx.send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
                duration_minutes: None,
            });
        }
        if ui.button("Memory Only").clicked() {
            let _ = bench_tx.send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::MemoryOnly,
                duration_minutes: None,
            });
        }
        if data.bench.running && ui.button("Cancel").clicked() {
            cancel.store(true, Ordering::Relaxed);
        }
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
    /// — a burn-in in flight → `Running` (the newest tick's
    /// elapsed — the normal-bench flag stays false for a burn-in,
    /// C7-16), a normal bench in flight → `Running` (the zone's
    /// start clock elapsed), a finished run (a terminal grid or kept
    /// streamed progress) → `Done`, the fresh state → `Idle`.
    #[test]
    fn status_state_tracks_the_run_states() {
        // The fresh state: no run in flight, no grid, no progress.
        assert_eq!(
            status_state(false, false, false, false, 0.0),
            StatusState::Idle
        );
        // A normal bench in flight (progress streaming): `Running`
        // with the start clock's elapsed — 42 s renders `0:42`.
        assert_eq!(
            status_state(true, false, false, true, 42.0),
            StatusState::Running { elapsed_secs: 42.0 }
        );
        // A burn-in in flight: `Running` with the newest tick's
        // elapsed — 125 s (the normal-bench flag stays false, C7-16).
        assert_eq!(
            status_state(false, true, false, false, 125.0),
            StatusState::Running { elapsed_secs: 125.0 }
        );
        // A burn-in still in flight wins over the kept terminal grid
        // of an earlier run.
        assert_eq!(
            status_state(false, true, true, false, 9.0),
            StatusState::Running { elapsed_secs: 9.0 }
        );
        // A finished run: the terminal grid landed (the burn-in
        // terminal leaves the progress empty — C7-16) → `Done`.
        assert_eq!(status_state(false, false, true, false, 0.0), StatusState::Done);
        // A finished run: the streamed progress is kept with no grid
        // (a cancelled normal bench) → `Done` (the old `done`
        // label's corner).
        assert_eq!(status_state(false, false, false, true, 0.0), StatusState::Done);
    }

    /// (f) `status_text`: the flat line's exact strings —
    /// `Status: Idle`, `Status: Running… <m:ss>` (the `…` kept
    /// static — the 60 FPS repaint animates the elapsed),
    /// `Status: Done`.
    #[test]
    fn status_text_renders_the_flat_lines() {
        assert_eq!(status_text(StatusState::Idle), "Status: Idle");
        assert_eq!(
            status_text(StatusState::Running { elapsed_secs: 42.0 }),
            "Status: Running… 0:42"
        );
        assert_eq!(
            status_text(StatusState::Running { elapsed_secs: 125.0 }),
            "Status: Running… 2:05"
        );
        assert_eq!(
            status_text(StatusState::Running { elapsed_secs: 0.0 }),
            "Status: Running… 0:00"
        );
        assert_eq!(status_text(StatusState::Done), "Status: Done");
    }

    /// (g) `status_color`: the idle line is the caller's dim
    /// (theme-relative `weak_text_color`, passed through), the
    /// running line CYAN, the done line the zone-local green (the
    /// `style.rs` palette stays frozen — C7-17).
    #[test]
    fn status_color_tracks_the_state() {
        let dim = egui::Color32::from_rgb(0x80, 0x80, 0x80);
        assert_eq!(
            status_color(StatusState::Idle, dim),
            dim,
            "the idle line is the dim color"
        );
        assert_eq!(status_color(StatusState::Running { elapsed_secs: 42.0 }, dim), CYAN);
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

    /// (k) `cell_phase`: not running → `Terminal` for every cell (the
    /// result grid); running → `Live` only for the cells the live
    /// grid carries a value for, `NotStarted` otherwise (a cell not
    /// started yet, or a latency cell — no progress events).
    #[test]
    fn cell_phase_tracks_running_and_streamed_values() {
        let live = live_grid(&[
            progress_event(Tier::Memory, BenchOp::Read, 26.0),
            progress_event(Tier::L1, BenchOp::Copy, 31.8),
        ]);
        // Not running: every cell renders its terminal value.
        for tier in &TIERS {
            for metric in &METRICS {
                assert_eq!(
                    cell_phase(false, &live, *tier, *metric),
                    CellPhase::Terminal
                );
            }
        }
        // Running: the streamed cells are live, the rest not started.
        assert_eq!(cell_phase(true, &live, Tier::Memory, Metric::Read), CellPhase::Live);
        assert_eq!(cell_phase(true, &live, Tier::L1, Metric::Copy), CellPhase::Live);
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
    /// in-flight GB/s value + a `…` suffix in a dimmed CYAN (distinct
    /// from the final value's full CYAN); not-started cells keep the
    /// `N/A` placeholder in CRIMSON.
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

        // NotStarted: the `N/A` placeholder.
        assert_eq!(
            phase_cell_text(CellPhase::NotStarted, &grid, &live, Tier::L3, Metric::Read),
            "N/A"
        );
        assert_eq!(phase_cell_color(CellPhase::NotStarted, Metric::Read, "N/A"), CRIMSON);
    }
}
