//! The Graphs window: the five-series graph state + the hand-rolled
//! static render (C7-20, plan D-3 / D-4 / D-5).
//!
//! The dedicated monitoring window (C7-21 spawns it as the eframe
//! 0.27.2 deferred child viewport, C7-22 makes it interactive)
//! renders five series over the newest five minutes:
//!
//! - `CPU FREQ (MHz)` — the live core frequency
//!   (`SystemPlatform.cpu_clock_mhz`).
//! - `VDDCR_CPU` — no source anywhere (the unprivileged SMU surface
//!   carries no VDDCR_CPU cell): a permanent label-only row with the
//!   `N/A (no source)` note, no fake geometry (D-4).
//! - `VDDCR_SOC (mV)` — the AMD SOC rail
//!   (`AmdReadout.voltages.vddcr_soc_mv`).
//! - `CPU TEMP (°C)` — the runtime unprivileged thermal-zone scan
//!   ([`read_cpu_temp_c`] — a `cpu_thermal` zone), else an honest
//!   no-source row (D-4).
//! - `MEM BANDWIDTH (GB/s)` — a step series from the latest bench /
//!   burn-in `Memory · Read` figure (D-5 — it rides the existing
//!   bench state, never a continuous sampler): a no-source row
//!   until the first sample.
//!
//! The state + render:
//!
//! - [`GraphSample`] / [`GraphState`] — one timestamped five-field
//!   sample per successful poll (an absent value = `f64::NAN`, the
//!   plot's non-finite-skip rule) in the [`GRAPH_CAPACITY`]-deep
//!   ring (1800 = 60 min at the 2 s poll cadence — the
//!   `history.rs` [`RingBuffer`] reused, generic, bounded). The
//!   background poller is the only writer (D6 — the Na-guarded
//!   [`record_graph_sample`] hook appends one sample per successful
//!   poll); the render thread + the child window are pure readers.
//! - [`read_cpu_temp_c`] — the direct unprivileged thermal-zone scan
//!   (D-4's runtime source): every failure class (no dir, no
//!   matching zone, an unreadable / non-numeric `temp`) degrades to
//!   `f64::NAN`; nothing panics (std `fs` only — poller-thread I/O,
//!   D6: it runs in the poller, never the render thread).
//! - [`render_graphs_window`] — the basic render: a `CentralPanel`
//!   (SLATE fill, the zone idiom) with the title + a dim subtitle
//!   and the five series rows in fixed order — each a hand-rolled
//!   time-windowed plot (the pure [`window_points`] helper over the
//!   newest five minutes: non-finite samples skipped, a flat window
//!   on the midline, every division guarded — the D5 no-panic
//!   plot contract); a series whose window holds no finite sample
//!   draws a label + the `N/A (no source)` note and no geometry
//!   (D-4 — a flat 0 line would be a lie, 0 ≠ N/A).
//!
//! **No new dependency (D-3):** the plot is a few dozen
//! `egui::Painter` calls (the `history.rs` idiom) — no chart crate,
//! the MSRV-1.75 lockfile stays untouched.
//!
//! **Pure core:** [`window_points`] / [`finite_min_max`] are I/O-free
//! and deterministic (the unit tests exercise them without an egui
//! context); [`render_graphs_window`] is the thin `egui` surface
//! over them (the live render is C7-21's gate).

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::history::RingBuffer;
use crate::{AMBER, CYAN, SLATE};

/// The graph depth: 1800 samples = 60 minutes at the default 2 s
/// poll cadence (C7-20 — bounded memory, the `HISTORY_CAPACITY`
/// precedent: the `history.rs` `RingBuffer` reused, generic).
pub const GRAPH_CAPACITY: usize = 1800;

/// The static render's time window: the newest five minutes (C7-22
/// makes it interactive: 1 / 5 / 15 / 60 min + pan).
const WINDOW_SECONDS: f64 = 300.0;

/// One series row's height (points) (the `PLOT_ROW_HEIGHT` idiom).
const ROW_HEIGHT: f32 = 40.0;
/// The polyline stroke width (points).
const LINE_WIDTH: f32 = 1.5;
/// The single-sample dot radius (points).
const DOT_RADIUS: f32 = 2.0;
/// The corner margin for the label / note / readout text (points).
const TEXT_MARGIN: f32 = 3.0;
/// The row background (a step darker than the SLATE panel fill).
const ROW_BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1C);
/// The window title (the dedicated graphs window, C7-20/21).
const WINDOW_TITLE: &str = "RAM & SYSTEM GRAPHS";
/// The no-source note (D-4): a source-less series never fakes a
/// 0 line (0 ≠ N/A) — the label + this note, no geometry.
const NO_SOURCE_NOTE: &str = "N/A (no source)";

// ---------------------------------------------------------------------
// The state (the §3 frozen shapes: one ring of timestamped samples).
// ---------------------------------------------------------------------

/// One graphs-window sample (the §3 frozen shape): the poll's
/// timestamp + the carried series (an absent value = `f64::NAN`, the
/// plot's non-finite-skip rule):
///
/// - `t` — unix seconds (fractional; [`unix_now`]).
/// - `cpu_freq_mhz` — the live core frequency
///   (`SystemPlatform.cpu_clock_mhz`), else NaN.
/// - `vddcr_soc_mv` — the AMD SOC rail (`vddcr_soc_mv` — a `u16`, so
///   a present cell is always finite), else NaN.
/// - `cpu_temp_c` — the thermal-zone scan ([`read_cpu_temp_c`]),
///   else NaN.
/// - `bandwidth_gbps` — the latest bench / burn-in `Memory · Read`
///   figure (D-5), else NaN (no sample yet).
///
/// There is deliberately no VDDCR_CPU field: the series has no
/// source anywhere (D-4) — its row is a permanent no-source note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphSample {
    /// The poll's unix seconds (fractional).
    pub t: f64,
    /// CPU core frequency (MHz), else NaN.
    pub cpu_freq_mhz: f64,
    /// VDDCR_SOC rail (mV), else NaN.
    pub vddcr_soc_mv: f64,
    /// CPU temperature (°C — the thermal-zone scan), else NaN.
    pub cpu_temp_c: f64,
    /// Memory-read bandwidth (GB/s — the D-5 step series), else NaN.
    pub bandwidth_gbps: f64,
}

/// The graphs-window state (the §3 frozen shape): one ring of
/// [`GraphSample`] at the [`GRAPH_CAPACITY`] depth — lockstep-free
/// (one struct ring, unlike the three-series `HistoryState`).
///
/// The background poller is the only writer (D6): it appends one
/// sample per successful poll (the Na-guarded
/// [`record_graph_sample`]); the render thread + the child window
/// are pure readers.
#[derive(Debug, Clone)]
pub struct GraphState {
    /// The samples, oldest → newest (the ring evicts the oldest
    /// past the capacity).
    pub samples: RingBuffer<GraphSample>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self { samples: RingBuffer::with_capacity(GRAPH_CAPACITY) }
    }
}

impl GraphState {
    /// The number of stored samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no samples have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

// ---------------------------------------------------------------------
// The sample recording (the poller is the only writer, D6).
// ---------------------------------------------------------------------

/// The current unix seconds (fractional): `SystemTime` over
/// `UNIX_EPOCH` — the sample's `t`. A pre-epoch clock (never on a
/// sane host) degrades to 0.0, not a panic.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Append one graphs-window sample for the just-landed snapshot (one
/// per successful poll — the poller is the only writer, D6).
///
/// The sources (the plan's C7-20 source map):
///
/// - `t` = unix seconds now ([`unix_now`]);
/// - `cpu_freq_mhz` = `platform.cpu_clock_mhz` (finite or NaN);
/// - `vddcr_soc_mv` = `amd.voltages.vddcr_soc_mv` (finite or NaN —
///   a `u16`, so a present cell is always finite);
/// - `cpu_temp_c` = the passed scan value ([`read_cpu_temp_c`] —
///   the caller runs the I/O on the poller thread, D6);
/// - `bandwidth_gbps` = the passed latest `Memory · Read` figure
///   (D-5 — the caller maps the no-figure sentinel to NaN so the
///   row stays its no-source note until the first bench / burn-in).
///
/// The Na guard (the no-panic contract, D5 — the
/// `record_history_sample` precedent): a sample is appended only
/// when ≥ 1 field is finite. An all-NaN sample (no snapshot, an
/// all-Na readout + no temp + no bandwidth yet, or only non-finite
/// readings) is a hole, not a point — it never lands in the ring.
pub fn record_graph_sample(
    graph: &mut GraphState,
    telemetry: &Option<SystemMemoryTelemetry>,
    cpu_temp_c: f64,
    bandwidth_gbps: f64,
) {
    let mut freq = f64::NAN;
    let mut soc = f64::NAN;
    if let Some(snapshot) = telemetry {
        if let Some(value) = snapshot
            .platform
            .cpu_clock_mhz
            .value()
            .copied()
            .filter(|value| value.is_finite())
        {
            freq = value;
        }
        if let Some(value) = snapshot
            .amd
            .value()
            .and_then(|readout| readout.voltages.vddcr_soc_mv.value().copied())
        {
            soc = f64::from(value);
        }
    }
    // The no-hole rule: an all-NaN sample never lands in the ring.
    if !freq.is_finite() && !soc.is_finite() && !cpu_temp_c.is_finite() && !bandwidth_gbps.is_finite() {
        return;
    }
    graph.samples.push(GraphSample {
        t: unix_now(),
        cpu_freq_mhz: freq,
        vddcr_soc_mv: soc,
        cpu_temp_c,
        bandwidth_gbps,
    });
}

// ---------------------------------------------------------------------
// The unprivileged CPU-temperature scan (D-4's runtime source).
// ---------------------------------------------------------------------

/// The unprivileged CPU-temperature scan (D-4's runtime source):
/// walk `/sys/class/thermal` and return, in °C, the first
/// `thermal_zone*` whose `type` (trimmed) equals `cpu_thermal`
/// (case-insensitive — the AMD CPU zone name); its `temp` file is
/// millidegrees.
///
/// Every failure class degrades to `f64::NAN`: no thermal class at
/// all, no matching zone (e.g. the host's iwlwifi-only thermal set),
/// an unreadable `type`, or an unreadable / non-numeric `temp` (the
/// ENODATA sensor case). Nothing panics — std `fs` reads only, no
/// parsing that can divide. Runs on the poller thread (D6 — never
/// the render thread).
pub fn read_cpu_temp_c() -> f64 {
    let Ok(zones) = fs::read_dir("/sys/class/thermal") else {
        return f64::NAN; // no thermal class at all.
    };
    for entry in zones.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue; // a non-UTF-8 zone name: not the CPU zone.
        };
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let dir = entry.path();
        let Ok(zone_type) = fs::read_to_string(dir.join("type")) else {
            continue; // the zone's type is unreadable: not it.
        };
        if zone_type.trim().eq_ignore_ascii_case("cpu_thermal") {
            // The first matching zone wins.
            return read_zone_temp_c(&dir);
        }
    }
    f64::NAN // no `cpu_thermal` zone (the host's iwlwifi-only case).
}

/// One zone's `temp` (millidegrees) in °C; every failure class
/// (missing / unreadable / non-numeric / non-finite) degrades to
/// NaN (the no-panic contract).
fn read_zone_temp_c(zone_dir: &Path) -> f64 {
    let Ok(raw) = fs::read_to_string(zone_dir.join("temp")) else {
        return f64::NAN; // missing (ENODATA sensor) or unreadable.
    };
    match raw.trim().parse::<f64>() {
        Ok(millidegrees) if millidegrees.is_finite() => millidegrees / 1000.0,
        _ => f64::NAN, // a non-numeric reading: not data.
    }
}

// ---------------------------------------------------------------------
// The pure plot geometry (testable: no egui context, no I/O).
// ---------------------------------------------------------------------

/// The (min, max) of the finite values, or `None` when no value is
/// finite (an empty / all-non-finite set draws nothing — the
/// `finite_min_max` idiom from `history.rs`).
fn finite_min_max(values: &[f64]) -> Option<(f64, f64)> {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for value in values {
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }
    if min == f64::INFINITY {
        // No finite value (empty or all non-finite).
        None
    } else {
        Some((min, max))
    }
}

/// Map the in-window samples of one series onto `rect` as pixel
/// points (the time-windowed generalization of `history.rs`'s
/// `sample_points`):
///
/// - x = the sample's `t` normalized over `[t_start, t_end]` (a
///   sample at the newest edge sits at the rect's right edge);
/// - y = the value normalized over the in-window finite values'
///   min/max (bottom → top);
/// - samples outside the window and non-finite values are skipped;
/// - a flat window (min == max) sits on the rect's midline.
///
/// Degenerate inputs yield an empty list (the caller draws no
/// geometry): a collapsed window (`t_end <= t_start`), no in-window
/// finite sample, or a zero / 1-pt rect — every division is
/// guarded, so nothing panics and nothing divides by zero (D5).
fn window_points(
    samples: &[GraphSample],
    field: impl Fn(&GraphSample) -> f64,
    t_start: f64,
    t_end: f64,
    rect: Rect,
) -> Vec<Pos2> {
    if t_end <= t_start {
        return Vec::new();
    }
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return Vec::new();
    }
    let in_window: Vec<(f64, f64)> = samples
        .iter()
        .filter(|s| s.t >= t_start && s.t <= t_end)
        .map(|s| (s.t, field(s)))
        .filter(|(_, value)| value.is_finite())
        .collect();
    let values: Vec<f64> = in_window.iter().map(|(_, value)| *value).collect();
    let Some((min, max)) = finite_min_max(&values) else {
        return Vec::new();
    };
    let span = max - min; // ≥ 0.0; 0.0 = a flat window (the midline).
    in_window
        .iter()
        .map(|(t, value)| {
            let x =
                rect.left() + rect.width() * (((*t - t_start) / (t_end - t_start)) as f32);
            let y = if span == 0.0 {
                rect.center().y
            } else {
                rect.bottom() - rect.height() * (((value - min) / span) as f32)
            };
            Pos2::new(x, y)
        })
        .collect()
}

// ---------------------------------------------------------------------
// The egui surface (the hand-rolled immediate-mode plot, D-3).
// ---------------------------------------------------------------------

/// One series row (the `plot_series` idiom): a fixed-height row in
/// `ui` — the row background, the dim series label (top-left), and
/// either (a) the time-windowed geometry from [`window_points`] (a
/// filled dot for a single in-window sample, a polyline for more,
/// a dim newest-value readout top-right) or (b) when the window
/// holds no finite sample, the `N/A (no source)` note (D-4) —
/// never fake geometry, never a panic.
fn graph_row(
    ui: &mut egui::Ui,
    samples: &[GraphSample],
    field: impl Fn(&GraphSample) -> f64,
    label: &str,
    color: Color32,
    t_start: f64,
    t_end: f64,
) -> egui::Response {
    let row_width = ui.available_width().max(1.0);
    let desired = Rect::from_min_size(ui.cursor().min, Vec2::new(row_width, ROW_HEIGHT));
    let rect = desired.intersect(ui.available_rect_before_wrap());
    let response = ui.allocate_rect(rect, Sense::hover());
    let dim = ui.visuals().weak_text_color();
    let painter = ui.painter();

    // The row background (always, so a no-source row still shows its
    // label + the plot area's extent).
    painter.rect_filled(rect, 3.0, ROW_BG);
    // The series label (always, over the plot).
    painter.text(
        rect.left_top() + Vec2::new(TEXT_MARGIN, TEXT_MARGIN),
        Align2::LEFT_TOP,
        label,
        FontId::monospace(11.0),
        dim,
    );
    if rect.width() > 1.0 && rect.height() > 1.0 {
        let points = window_points(samples, &field, t_start, t_end, rect);
        if points.is_empty() {
            // No finite sample in the window: the no-source note
            // (D-4) — no geometry, no fake 0 line.
            painter.text(
                rect.right_top() + Vec2::new(-TEXT_MARGIN, TEXT_MARGIN),
                Align2::RIGHT_TOP,
                NO_SOURCE_NOTE,
                FontId::monospace(11.0),
                dim,
            );
        } else {
            match points.len() {
                1 => {
                    // A single in-window sample: one dot at its time
                    // position.
                    painter.circle_filled(points[0], DOT_RADIUS, color);
                }
                _ => {
                    // The polyline: one segment between consecutive
                    // points.
                    for pair in points.windows(2) {
                        painter.line_segment([pair[0], pair[1]], Stroke::new(LINE_WIDTH, color));
                    }
                }
            }
            // The newest in-window value, dim, top-right.
            let newest = samples
                .iter()
                .rev()
                .filter(|s| s.t >= t_start && s.t <= t_end)
                .find(|s| field(s).is_finite());
            if let Some(sample) = newest {
                painter.text(
                    rect.right_top() + Vec2::new(-TEXT_MARGIN, TEXT_MARGIN),
                    Align2::RIGHT_TOP,
                    format!("{:.1}", field(sample)),
                    FontId::monospace(11.0),
                    dim,
                );
            }
        }
    }
    response
}

/// The dedicated graphs window's basic render (C7-20; C7-21 spawns
/// this as the deferred child viewport's body, C7-22 layers the
/// interactivity on): a `CentralPanel` (SLATE fill, the zone idiom)
/// with the title + a dim subtitle and the five series rows in
/// fixed order over the newest five minutes (the static default
/// view):
///
/// 1. `CPU FREQ (MHz)` — the live core frequency.
/// 2. `VDDCR_CPU` — the permanent no-source row (D-4: the series
///    has no source anywhere — the label + the `N/A (no source)`
///    note, no geometry; it self-populates if a source appears, no
///    layout change — the row is data-driven).
/// 3. `VDDCR_SOC (mV)` — the AMD SOC rail.
/// 4. `CPU TEMP (°C)` — the thermal-zone scan; a no-source row
///    while the window holds no finite temp (D-4).
/// 5. `MEM BANDWIDTH (GB/s)` — the bench / burn-in step series
///    (D-5); a no-source row until the first sample.
///
/// Pure read over [`GraphState`] (no I/O, D6 — the poller is the
/// only writer): an empty state renders the five no-source rows
/// (the VDDCR_CPU row is permanent) and never panics (the D5
/// contract; the `render_history` headless-test precedent).
pub fn render_graphs_window(ctx: &egui::Context, graph: &GraphState) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(SLATE))
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(WINDOW_TITLE).strong().color(CYAN));
            // The dim subtitle: the static window + the sample count.
            ui.label(
                RichText::new(format!("last 5 minutes · {} samples", graph.samples.len()))
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(8.0);
            // One pass over the ring (the transient `Vec` is a
            // render-only copy — the `render_history` idiom; the
            // ring itself never grows, D-C4).
            let samples: Vec<GraphSample> = graph.samples.iter().copied().collect();
            // The shared static window: anchored at the newest
            // sample (C7-22's `pan_offset_s = 0` default) and
            // reaching five minutes back — collapsed (0..0) while
            // the state is empty, so every row degrades to its
            // no-source note.
            let t_end = samples.last().map(|s| s.t).unwrap_or(0.0);
            let t_start = t_end - WINDOW_SECONDS;
            // Palette semantics (style.rs): clocks + voltages
            // AMBER, bandwidth + temperature CYAN.
            graph_row(ui, &samples, |s| s.cpu_freq_mhz, "CPU FREQ (MHz)", AMBER, t_start, t_end);
            ui.add_space(2.0);
            // The permanent no-source row (D-4): the series has no
            // source anywhere — an empty slice keeps it data-driven
            // (it self-populates if one ever appears, no layout
            // change).
            graph_row(ui, &[], |_s| f64::NAN, "VDDCR_CPU", AMBER, t_start, t_end);
            ui.add_space(2.0);
            graph_row(ui, &samples, |s| s.vddcr_soc_mv, "VDDCR_SOC (mV)", AMBER, t_start, t_end);
            ui.add_space(2.0);
            graph_row(ui, &samples, |s| s.cpu_temp_c, "CPU TEMP (°C)", CYAN, t_start, t_end);
            ui.add_space(2.0);
            graph_row(
                ui,
                &samples,
                |s| s.bandwidth_gbps,
                "MEM BANDWIDTH (GB/s)",
                CYAN,
                t_start,
                t_end,
            );
        });
}

// ---------------------------------------------------------------------
// Tests (headless: no window, no display).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
    use ramsleuth_telemetry::amd_readout::map_amd;
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemPlatform;

    /// Approximate f32 equality (the geometry is exact for the test
    /// values, but the comparison stays robust).
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3
    }

    /// One sample with explicit fields (the test geometry).
    fn sample(t: f64, freq: f64, soc: f64, temp: f64, bw: f64) -> GraphSample {
        GraphSample {
            t,
            cpu_freq_mhz: freq,
            vddcr_soc_mv: soc,
            cpu_temp_c: temp,
            bandwidth_gbps: bw,
        }
    }

    /// An AMD readout with VDDCR_SOC = `soc_mv` and every other cell
    /// sanity-mapped (the `populated_snapshot` fixture from
    /// `update.rs` — the DDR4-3200-class synthetic PM-table
    /// snapshot).
    fn readout_with_soc(soc_mv: u16) -> ramsleuth_telemetry::amd_readout::AmdReadout {
        map_amd(&AmdPmSnapshot {
            version: 0x0007_0B02,
            mclk_mhz: 1800,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 16,
                rcwdwr: 16,
                rcdrd: 16,
                rp: 16,
                ras: 32,
                rc: 48,
                rrds: 4,
                rrld: 4,
                faw: 16,
                wtrs: 8,
                wtrl: 8,
                wr: 8,
                rfc1: 160,
                rfc2: 160,
                rfcsb: 160,
                cwl: 16,
                rtp: 8,
                rdwr: 8,
                wrrd: 4,
                rdrd_sd: 100,
                rdrd_dd: 101,
                rdrd_scl: 102,
                rdrd_sc: 103,
                wrwr_sd: 104,
                wrwr_dd: 105,
                wrwr_scl: 106,
                wrwr_sc: 107,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 5,
                rtt_nom: 2,
                rtt_wr: 0,
                rtt_park: 4,
                clk_drv: 6,
                addr_cmd_drv: 8,
                cs_odt_drv: 10,
                cke_drv: 12,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: soc_mv,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
            },
        })
    }

    /// A snapshot with the given platform core frequency (or an all-Na
    /// platform when `None`) + the given VDDCR_SOC (or an all-Na AMD
    /// readout when `None`) — every other cell Na.
    fn snapshot(freq_mhz: Option<f64>, soc_mv: Option<u16>) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: match soc_mv {
                Some(mv) => Section::Value(readout_with_soc(mv)),
                None => Section::na(NaReason::NotApplicable),
            },
            intel: Section::na(NaReason::NotApplicable),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: match freq_mhz {
                    Some(mhz) => Section::Value(mhz),
                    None => Section::na(NaReason::NotApplicable),
                },
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// A 100×50 rect at the origin for the geometry tests.
    fn plot_rect() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 50.0))
    }

    /// The test time window (unix seconds): `[1000, 1100]`.
    fn window() -> (f64, f64) {
        (1000.0, 1100.0)
    }

    // ------------------------------------------------------------------
    // record_graph_sample — the Na guard (the plan's test list).
    // ------------------------------------------------------------------

    /// (a) A populated snapshot (finite clock + VDDCR_SOC) + a finite
    /// temp + a finite bandwidth appends exactly one sample with the
    /// right fields (the `t` stamp is unix seconds, finite + > 0).
    #[test]
    fn record_graph_sample_populated_appends_one_sample_with_the_right_fields() {
        let mut graph = GraphState::default();
        record_graph_sample(&mut graph, &Some(snapshot(Some(3600.0), Some(1150))), 47.3, 26.35);
        assert_eq!(graph.len(), 1, "one recorded poll appends exactly one sample");
        let s = graph.samples.last().expect("the sample landed");
        assert!(s.t.is_finite() && s.t > 0.0, "the t stamp is unix seconds");
        assert_eq!(s.cpu_freq_mhz, 3600.0, "the clock lands in MHz");
        assert_eq!(s.vddcr_soc_mv, 1150.0, "VDDCR_SOC lands in mV");
        assert_eq!(s.cpu_temp_c, 47.3, "the passed temp lands in °C");
        assert_eq!(s.bandwidth_gbps, 26.35, "the passed bandwidth lands in GB/s");
    }

    /// (b) No sample lands when every field is absent: no snapshot
    /// at all, an all-Na AMD readout + all-Na platform + no temp + no
    /// bandwidth, or only non-finite readings (the no-hole rule — an
    /// all-NaN sample is a hole, not a point).
    #[test]
    fn record_graph_sample_all_na_appends_nothing() {
        // No snapshot at all.
        let mut graph = GraphState::default();
        record_graph_sample(&mut graph, &None, f64::NAN, f64::NAN);
        assert!(graph.is_empty(), "no snapshot appends no sample");

        // An all-Na AMD readout + all-Na platform + no temp + no
        // bandwidth.
        let mut graph = GraphState::default();
        record_graph_sample(&mut graph, &Some(snapshot(None, None)), f64::NAN, f64::NAN);
        assert!(graph.is_empty(), "an all-Na snapshot appends no sample");

        // Only non-finite readings never count toward the ≥ 1-finite
        // guard (±inf are as absent as NaN here).
        let mut graph = GraphState::default();
        record_graph_sample(
            &mut graph,
            &Some(snapshot(None, None)),
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        assert!(graph.is_empty(), "non-finite readings never count as a field");
    }

    /// (c) A finite clock alone appends a sample with the rest NaN
    /// (the series are independent — an absent source degrades its
    /// own field, never the whole sample).
    #[test]
    fn record_graph_sample_finite_clock_alone_appends_the_rest_nan() {
        let mut graph = GraphState::default();
        record_graph_sample(&mut graph, &Some(snapshot(Some(3600.0), None)), f64::NAN, f64::NAN);
        assert_eq!(graph.len(), 1, "a finite clock alone appends a sample");
        let s = graph.samples.last().expect("the sample landed");
        assert_eq!(s.cpu_freq_mhz, 3600.0);
        assert!(s.vddcr_soc_mv.is_nan(), "an absent VDDCR_SOC stays NaN");
        assert!(s.cpu_temp_c.is_nan(), "an absent temp stays NaN");
        assert!(s.bandwidth_gbps.is_nan(), "an absent bandwidth stays NaN");
    }

    // ------------------------------------------------------------------
    // GraphState — the ring capacity (the `HISTORY_CAPACITY`
    // precedent, at 1800).
    // ------------------------------------------------------------------

    /// (d) Pushing past the 1800 capacity evicts the oldest: the ring
    /// caps at [`GRAPH_CAPACITY`], the five oldest are gone, the
    /// newest is last, and the order holds.
    #[test]
    fn graph_state_wraps_at_the_capacity() {
        let mut graph = GraphState::default();
        assert_eq!(graph.samples.capacity(), GRAPH_CAPACITY);
        for i in 0..(GRAPH_CAPACITY + 5) {
            graph.samples.push(sample(
                i as f64,
                i as f64,
                i as f64,
                i as f64,
                i as f64,
            ));
        }
        assert_eq!(graph.len(), GRAPH_CAPACITY, "the ring caps at the capacity");
        let newest = graph.samples.last().expect("the newest sample");
        assert_eq!(newest.cpu_freq_mhz, (GRAPH_CAPACITY + 4) as f64);
        let oldest = graph.samples.iter().next().expect("the oldest survivor");
        assert_eq!(oldest.cpu_freq_mhz, 5.0, "the five oldest are evicted");
    }

    // ------------------------------------------------------------------
    // read_cpu_temp_c — the thermal-zone scan (host-dependent).
    // ------------------------------------------------------------------

    /// (e) The scan never panics; a reading is either the honest NaN
    /// (no `cpu_thermal` zone — the host's iwlwifi-only thermal set)
    /// or a finite value in a sane temperature range.
    #[test]
    fn read_cpu_temp_c_never_panics_and_degrades_to_nan() {
        let temp = read_cpu_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    // ------------------------------------------------------------------
    // window_points — the pure time-windowed geometry.
    // ------------------------------------------------------------------

    /// (f) Degenerate inputs draw nothing (no panic, no division by
    /// zero): an empty slice, an all-NaN series, a collapsed window,
    /// and a zero-size rect.
    #[test]
    fn window_points_degenerate_inputs_are_empty() {
        let (t_start, t_end) = window();
        assert!(
            window_points(&[], |s| s.cpu_freq_mhz, t_start, t_end, plot_rect()).is_empty(),
            "an empty slice yields no points"
        );
        let all_nan = vec![
            sample(1010.0, f64::NAN, f64::NAN, f64::NAN, f64::NAN),
            sample(1050.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::NAN),
        ];
        assert!(
            window_points(&all_nan, |s| s.cpu_freq_mhz, t_start, t_end, plot_rect()).is_empty(),
            "an all-NaN series yields no points"
        );
        let data = vec![sample(1010.0, 1.0, 1.0, 1.0, 1.0)];
        assert!(
            window_points(&data, |s| s.cpu_freq_mhz, t_start, t_start, plot_rect()).is_empty(),
            "a collapsed window yields no points"
        );
        assert!(
            window_points(&data, |s| s.cpu_freq_mhz, t_start, t_end, Rect::NOTHING).is_empty(),
            "a zero-size rect yields no points"
        );
    }

    /// (g) A flat window (min == max) sits on the rect's midline —
    /// the span division never fires — with the points at their time
    /// positions across the width.
    #[test]
    fn window_points_flat_series_sits_on_the_midline() {
        let (t_start, t_end) = window();
        let samples = vec![
            sample(1010.0, 42.0, 42.0, 42.0, 42.0),
            sample(1050.0, 42.0, 42.0, 42.0, 42.0),
            sample(1100.0, 42.0, 42.0, 42.0, 42.0),
        ];
        let points = window_points(&samples, |s| s.cpu_freq_mhz, t_start, t_end, plot_rect());
        assert_eq!(points.len(), 3);
        for point in &points {
            assert!(close(point.y, 25.0), "a flat window must sit on the midline");
        }
        assert!(close(points[0].x, 10.0));
        assert!(close(points[1].x, 50.0));
        assert!(close(points[2].x, 100.0), "the newest sample sits at the right edge");
    }

    /// (h) Two extremes normalize onto the rect's corners: the min at
    /// the bottom-left, the max at the top-right.
    #[test]
    fn window_points_extremes_hit_the_corners() {
        let (t_start, t_end) = window();
        let samples = vec![
            sample(1000.0, 0.0, 0.0, 0.0, 0.0),
            sample(1100.0, 10.0, 10.0, 10.0, 10.0),
        ];
        let points = window_points(&samples, |s| s.cpu_freq_mhz, t_start, t_end, plot_rect());
        assert_eq!(points.len(), 2);
        assert!(
            close(points[0].x, 0.0) && close(points[0].y, 50.0),
            "the min sits bottom-left, got ({}, {})",
            points[0].x,
            points[0].y
        );
        assert!(
            close(points[1].x, 100.0) && close(points[1].y, 0.0),
            "the max sits top-right, got ({}, {})",
            points[1].x,
            points[1].y
        );
    }

    /// (i) Samples outside the window are dropped (the window is the
    /// newest five minutes — older points never plot): exactly the
    /// in-window sample remains, at its time position.
    #[test]
    fn window_points_out_of_window_samples_are_dropped() {
        let (t_start, t_end) = window();
        let samples = vec![
            sample(900.0, 1.0, 1.0, 1.0, 1.0), // before the window
            sample(1050.0, 5.0, 5.0, 5.0, 5.0), // in the window
            sample(1200.0, 9.0, 9.0, 9.0, 9.0), // after the window
        ];
        let points = window_points(&samples, |s| s.bandwidth_gbps, t_start, t_end, plot_rect());
        assert_eq!(points.len(), 1, "only the in-window sample plots");
        assert!(close(points[0].x, 50.0));
        assert!(close(points[0].y, 25.0), "a lone in-window value sits on the midline");
    }

    // ------------------------------------------------------------------
    // render_graphs_window — the headless no-panic contract (the
    // `render_history` headless-test precedent).
    // ------------------------------------------------------------------

    /// One headless frame on a fresh context (the `begin_frame`
    /// pattern from the `egui` docs — the fonts load there), running
    /// the window render. The 10000×10000 default screen rect gives
    /// every row a real area, so the full paint path executes — what
    /// is under test is the no-panic contract.
    fn run_headless_frame(draw: impl FnOnce(&egui::Context)) {
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        draw(&ctx);
    }

    /// (j) The full window runs headless without panicking: an empty
    /// state (every row its no-source note), a fully populated state
    /// (four plotted rows + the permanent VDDCR_CPU no-source row),
    /// a sparse state (only a finite clock — the VDDCR_SOC / CPU
    /// TEMP / MEM BANDWIDTH rows their no-source notes), and a
    /// single-sample state (the dot path).
    #[test]
    fn render_graphs_window_runs_headless_without_panicking() {
        // Empty: five no-source rows (the window is collapsed).
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &GraphState::default());
        });

        // Fully populated: 20 samples over two minutes (all in the
        // five-minute window) — the polyline path (the
        // `windows(2)` segments) on four rows + the permanent
        // VDDCR_CPU no-source row.
        let mut full = GraphState::default();
        for i in 0..20 {
            full.samples.push(sample(
                1000.0 + 10.0 * f64::from(i),
                3600.0 + 10.0 * f64::from(i),
                1150.0 + f64::from(i),
                45.0 + 0.1 * f64::from(i),
                26.35,
            ));
        }
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &full);
        });

        // Sparse: only a finite clock (the other three series are
        // their no-source notes; VDDCR_CPU is permanent).
        let mut sparse = GraphState::default();
        sparse.samples.push(sample(1000.0, 3600.0, f64::NAN, f64::NAN, f64::NAN));
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &sparse);
        });

        // A single sample: the dot path (one in-window point per
        // finite series).
        let mut one = GraphState::default();
        one.samples.push(sample(1000.0, 3600.0, 1150.0, 45.0, 26.35));
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &one);
        });
    }
}
