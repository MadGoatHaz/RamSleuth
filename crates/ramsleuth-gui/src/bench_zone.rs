//! Zone 2 renderer: the AIDA64-style 4×4 benchmark grid + run controls
//! + live progress (P3-28).
//!
//! Grand Design §3.1 right panel: the 4×4 grid — Memory / L1 / L2 / L3
//! rows × Read / Write / Copy / Latency columns — from the terminal
//! result grid of the live [`BenchState`], every cell its formatted
//! value (two-decimal GB/s for a bandwidth metric, ns for latency) or
//! `N/A` for an unmeasured cell (0.0 on the wire), in an
//! `egui_extras::TableBuilder` table (bandwidth values CYAN, latency
//! cells AMBER — the palette's low-latency accent — `N/A` CRIMSON); a
//! progress bar driven by the last streamed [`StreamProgress`] event
//! (`cell_index + 1` of `total_cells` — `cell_index` is the 0-based
//! index of the cell just completed) with a `running…` / `done` /
//! `idle` label; and the run controls — `Run Full` + `Memory Only`
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
//! absent daemon degrades to the placeholder + `idle` — never a
//! panic.
//!
//! **Pure core:** [`grid_cells`] is I/O-free and deterministic (the
//! 16 `(tier · op, value-or-N/A)` pairs the table renders — the unit
//! tests exercise it without an egui context); [`render_bench_zone`]
//! is the thin `egui` surface over it (the live render is verified in
//! the QA phase).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use ramsleuth_bench::{BenchmarkGrid, Metric, StreamProgress, StreamTarget, Tier};
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
/// The table renderer consumes these verbatim (4 per row, in order).
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

/// The progress bar's 0.0–1.0 fraction of one streamed progress
/// event: the event's cell just completed, so `cell_index + 1` of
/// `total_cells` cells are done (a zero `total_cells` guards the
/// division → 0.0).
fn progress_fraction(progress: &StreamProgress) -> f32 {
    if progress.total_cells == 0 {
        return 0.0;
    }
    ((progress.cell_index + 1) as f32 / progress.total_cells as f32).clamp(0.0, 1.0)
}

/// The progress line's label: `running…` (with the last event's own
/// label) while a run is in flight, `done` after a run finished (the
/// progress events are kept), and `idle` when no run has started.
fn progress_label(data: &TelemetryData) -> String {
    if data.bench.running {
        return match data.bench.progress.last() {
            Some(progress) => format!("running… ({})", progress.label),
            None => "running…".to_owned(),
        };
    }
    if data.bench.progress.is_empty() {
        "idle".to_owned()
    } else {
        "done".to_owned()
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

// ---------------------------------------------------------------------
// The egui surface (compile-checked here; the live render is verified
// in the QA phase).
// ---------------------------------------------------------------------

/// Zone 2: render the benchmark grid + progress + run controls from
/// `data` — a titled SLATE frame (the zone 1 precedent) with the
/// `egui_extras::TableBuilder` 4×4 grid, the progress bar + label,
/// and the `Run Full` / `Memory Only` / `Cancel` buttons.
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
        render_grid_table(ui, data.bench.grid.as_ref());
        ui.add_space(4.0);
        render_progress(ui, data);
        ui.add_space(4.0);
        render_controls(ui, data, bench_tx, cancel);
    });
}

/// The 4×4 `egui_extras::TableBuilder` table: a header row (the four
/// metric names) + one row per tier, every cell its [`grid_cells`]
/// display in its semantic color. No terminal grid yet renders the
/// all-`N/A` placeholder (the same 16 labels — the layout never shifts
/// when the result lands).
fn render_grid_table(ui: &mut egui::Ui, grid: Option<&BenchmarkGrid>) {
    let cells = match grid {
        Some(grid) => grid_cells(grid),
        None => {
            let empty = BenchmarkGrid {
                read_gbps: [0.0; 4],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            };
            grid_cells(&empty)
        }
    };
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
                    for (metric, (_, display)) in METRICS.iter().zip(row_cells.iter()) {
                        let text = display.clone();
                        let color = cell_color(*metric, &text);
                        row.col(move |ui| {
                            ui.label(egui::RichText::new(text).color(color));
                        });
                    }
                });
            }
        });
}

/// The progress bar + its label: the fraction from the last streamed
/// event, the label per the run state (`running…` / `done` / `idle`).
fn render_progress(ui: &mut egui::Ui, data: &TelemetryData) {
    let fraction = data
        .bench
        .progress
        .last()
        .map(progress_fraction)
        .unwrap_or(0.0);
    let label = progress_label(data);
    let _ = ui.add(
        egui::ProgressBar::new(fraction)
            .text(label)
            .desired_width(280.0),
    );
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
            });
        }
        if ui.button("Memory Only").clicked() {
            let _ = bench_tx.send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::MemoryOnly,
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

    /// (d) `progress_fraction`: the event's cell just completed, so
    /// `cell_index + 1` of `total_cells` — a zero total guards the
    /// division.
    #[test]
    fn progress_fraction_counts_completed_cells() {
        fn event(cell_index: u32, total_cells: u32) -> StreamProgress {
            StreamProgress {
                cell_index,
                total_cells,
                tier: Tier::Memory,
                op: BenchOp::Read,
                value: 26.0,
                label: "Memory · Read (GB/s)".to_owned(),
            }
        }
        assert_eq!(progress_fraction(&event(0, 12)), 1.0 / 12.0);
        assert_eq!(progress_fraction(&event(11, 12)), 1.0);
        assert_eq!(progress_fraction(&event(3, 4)), 1.0);
        assert_eq!(progress_fraction(&event(0, 0)), 0.0);
    }

    /// (e) `progress_label`: the three run states — `idle` (no run,
    /// no events), `running…` (+ the last event's label), `done` (a
    /// finished run keeps its events).
    #[test]
    fn progress_label_tracks_the_run_states() {
        let mut data = TelemetryData::default();
        assert_eq!(progress_label(&data), "idle");

        data.bench.running = true;
        assert_eq!(progress_label(&data), "running…");

        data.bench.progress.push(StreamProgress {
            cell_index: 3,
            total_cells: 12,
            tier: Tier::L1,
            op: BenchOp::Copy,
            value: 31.8,
            label: "L1 · Copy (GB/s)".to_owned(),
        });
        assert_eq!(progress_label(&data), "running… (L1 · Copy (GB/s))");

        data.bench.running = false;
        assert_eq!(progress_label(&data), "done");
    }

    /// (f) The semantic cell color: CYAN for the bandwidth values,
    /// AMBER for a latency cell, CRIMSON for an unmeasured cell.
    #[test]
    fn cell_color_semantics() {
        assert_eq!(cell_color(Metric::Read, "26.35 GB/s"), CYAN);
        assert_eq!(cell_color(Metric::Latency, "86.84 ns"), AMBER);
        assert_eq!(cell_color(Metric::Copy, "N/A"), CRIMSON);
    }
}
