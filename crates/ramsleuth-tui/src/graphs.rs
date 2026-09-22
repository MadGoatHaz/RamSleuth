//! TUI-05/06/07 — the graphs core, parts 1+2+3: the sample +
//! state + record hook, the CPU-temp source scan, and the window
//! filter + block sparkline panel (plan `PLAN-TUI-PARITY` §3).
//!
//! A self-contained mirror of the GUI `graph.rs` sample core (the
//! five-series graphs window), re-implemented TUI-local: the TUI
//! must not depend on `ramsleuth-gui` (plan §1 dependency facts —
//! the egui/eframe pull would be the wrong direction), and this
//! module adds no new dependency (std + the already-present
//! `ramsleuth-telemetry` snapshot type).
//!
//! - [`GraphSample`] — one timestamped six-field sample per
//!   successful poll (an absent value = `f64::NAN`, the plot's
//!   non-finite-skip rule): `t` (unix seconds, [`unix_now`]) +
//!   `cpu_freq_mhz` + `vddcr_cpu_mv` + `vddcr_soc_mv` +
//!   `cpu_temp_c` + `bandwidth_gbps`.
//! - [`GraphState`] — one [`ring::RingBuffer`] of samples at
//!   [`GRAPH_CAPACITY`] (1800 = 60 min at the 2 s poll cadence —
//!   the GUI's graph depth; TUI-03's ring via `with_capacity(1800)`)
//!   + `len` / `is_empty` over it.
//! - [`record_graph_sample`] — the Na-guarded poller hook (the GUI
//!   C7-20 source map): one sample per successful poll, appended
//!   only when ≥ 1 field is finite — an all-NaN reading is a hole,
//!   never a point (the `record_history_sample` precedent).
//! - [`read_cpu_temp_c`] — the ordered CPU-temp source scan
//!   (TUI-06 — the runtime source of the `cpu_temp_c` series; the
//!   TUI-local copy of the GUI's C9-04 scan: the `k10temp` /
//!   `zenpower` hwmon `temp1_input` first, the `cpu_thermal`
//!   thermal-zone fallback; every failure class → `f64::NAN`, std
//!   `fs` only, poller-thread — plan D6).
//! - [`window_samples`] — the `[w]` time-window filter: the
//!   samples with `t ≥ newest_t − window_min·60` (the GUI presets
//!   1/5/15/60 min; the newest stamp is the ring's last — `t` is
//!   non-decreasing).
//! - [`render_graphs_panel`] — the `[g]` graphs overlay (TUI-16
//!   draws it last, topmost, over the whole zone area — the
//!   C21-36 modal idiom): a titled block on the slate background,
//!   one block-bar sparkline row per series in the GUI's fixed
//!   order (CPU FREQ MHz, VDDCR_CPU mV, VDDCR_SOC mV, CPU TEMP °C,
//!   MEM BW GB/s — a dim label + a `▁…█` bar plot quantized to the
//!   row's finite min/max span, NaN = a gap, + a dim min/max; a
//!   series with no finite sample in the window draws label + the
//!   N/A note only), a bottom sample-count line; zero-area /
//!   empty / one-sample never panics.
//!
//! **Poller-only writer:** the background updater (TUI-17/18) is
//! the only caller of [`record_graph_sample`]; the render path is
//! a pure reader (plan D6).
//!
//! **No-panic contract:** every non-finite source cell degrades to
//! `f64::NAN` (an absent value — 0 ≠ N/A), the ring is bounded at
//! [`GRAPH_CAPACITY`] (no unbounded growth), and nothing here
//! allocates per call except the sample push.
//!
//! The module was declared in `lib.rs` by TUI-05 (the minimal
//! wiring the crate needs to compile this file) — TUI-23 does the
//! final module-map update + the root re-exports.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ramsleuth_telemetry::SystemMemoryTelemetry;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ring::RingBuffer;

/// The graphs depth: 1800 samples = 60 minutes at the default 2 s
/// poll cadence (the GUI `GRAPH_CAPACITY` — bounded memory, the
/// `history.rs` ring reused, generic).
pub const GRAPH_CAPACITY: usize = 1800;

/// One graphs-panel sample (plan §2.3 frozen shape): the poll's
/// timestamp + the five carried series (an absent value =
/// `f64::NAN`, the plot's non-finite-skip rule):
///
/// - `t` — unix seconds (fractional; [`unix_now`]).
/// - `cpu_freq_mhz` — the live core frequency
///   (`SystemPlatform.cpu_clock_mhz`), else NaN.
/// - `vddcr_cpu_mv` — the Vcore / VDDCR_VDD rail (the C12-01
///   frozen `VoltageSet.vcore_mv`), else NaN.
/// - `vddcr_soc_mv` — the AMD SOC rail (`VoltageSet.vddcr_soc_mv`
///   — a `u16`, so a present cell is always finite), else NaN.
/// - `cpu_temp_c` — the CPU-temp source scan (TUI-06), else NaN.
/// - `bandwidth_gbps` — the latest bench / burn-in `Memory · Read`
///   figure (TUI-18), else NaN (no sample yet).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphSample {
    /// The poll's unix seconds (fractional).
    pub t: f64,
    /// CPU core frequency (MHz), else NaN.
    pub cpu_freq_mhz: f64,
    /// Vcore / VDDCR_VDD rail (mV), else NaN.
    pub vddcr_cpu_mv: f64,
    /// VDDCR_SOC rail (mV), else NaN.
    pub vddcr_soc_mv: f64,
    /// CPU temperature (°C — the TUI-06 source scan), else NaN.
    pub cpu_temp_c: f64,
    /// Memory-read bandwidth (GB/s — the TUI-18 step series),
    /// else NaN.
    pub bandwidth_gbps: f64,
}

/// The graphs-panel state (plan §2.3 frozen shape): one
/// [`RingBuffer`] of [`GraphSample`] at the [`GRAPH_CAPACITY`]
/// depth — lockstep-free (one struct ring, unlike the GUI's
/// three-series `HistoryState`).
///
/// The background poller is the only writer (plan D6): it appends
/// one sample per successful poll (the Na-guarded
/// [`record_graph_sample`]); the render path is a pure reader.
#[derive(Debug, Clone)]
pub struct GraphState {
    /// The samples, oldest → newest (the ring evicts the oldest
    /// past the capacity).
    pub samples: RingBuffer<GraphSample>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            samples: RingBuffer::with_capacity(GRAPH_CAPACITY),
        }
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

/// The current unix seconds (fractional): `SystemTime` over
/// `UNIX_EPOCH` — the sample's `t`. A pre-epoch clock (never on a
/// sane host) degrades to 0.0, not a panic.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Append one graphs-panel sample for the just-landed snapshot (one
/// per successful poll — the poller is the only writer, plan D6).
///
/// The sources (the GUI's C7-20 source map, TUI-adapted):
///
/// - `t` = unix seconds now ([`unix_now`]);
/// - `cpu_freq_mhz` = `platform.cpu_clock_mhz` (finite or NaN);
/// - `vddcr_cpu_mv` = `amd.voltages.vcore_mv` (the C12-01 frozen
///   field, PM table 0x0A0 — a `u16`, so a present cell is always
///   finite), else NaN;
/// - `vddcr_soc_mv` = `amd.voltages.vddcr_soc_mv` (a `u16`, so a
///   present cell is always finite), else NaN;
/// - `cpu_temp_c` = the passed scan value (TUI-06's
///   `read_cpu_temp_c` — the caller runs the I/O on the poller
///   thread, plan D6);
/// - `bandwidth_gbps` = the passed latest `Memory · Read` figure
///   (TUI-18 — the caller maps the no-figure sentinel to NaN so
///   the row stays its no-source note until the first bench /
///   burn-in).
///
/// The Na guard (the no-panic contract — the GUI's
/// `record_graph_sample` precedent): a sample is appended only
/// when ≥ 1 field is finite. An all-NaN sample (no snapshot, an
/// all-Na readout + no temp + no bandwidth yet, or only
/// non-finite readings) is a hole, not a point — it never lands
/// in the ring.
pub fn record_graph_sample(
    graph: &mut GraphState,
    telemetry: &Option<SystemMemoryTelemetry>,
    cpu_temp_c: f64,
    bandwidth_gbps: f64,
) {
    let mut freq = f64::NAN;
    let mut soc = f64::NAN;
    let mut vcore = f64::NAN;
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
        // The C12-01 frozen Vcore field (PM table 0x0A0, board-
        // agnostic): a `u16`, so a present cell is always finite.
        if let Some(value) = snapshot
            .amd
            .value()
            .and_then(|readout| readout.voltages.vcore_mv.value().copied())
        {
            vcore = f64::from(value);
        }
    }
    // The no-hole rule: an all-NaN sample never lands in the ring.
    if !freq.is_finite()
        && !soc.is_finite()
        && !vcore.is_finite()
        && !cpu_temp_c.is_finite()
        && !bandwidth_gbps.is_finite()
    {
        return;
    }
    graph.samples.push(GraphSample {
        t: unix_now(),
        cpu_freq_mhz: freq,
        vddcr_cpu_mv: vcore,
        vddcr_soc_mv: soc,
        cpu_temp_c,
        bandwidth_gbps,
    });
}

// ---------------------------------------------------------------------
// The CPU-temperature source scan (TUI-06 — the runtime source of
// the `cpu_temp_c` series; the GUI's C9-04 ordered scan: hwmon
// first, thermal-zone fallback).
// ---------------------------------------------------------------------

/// The ordered CPU-temperature source scan (the runtime source of
/// the `cpu_temp_c` series — the TUI-local copy of the GUI
/// `graph.rs` C9-04 scan): first the hwmon sensor
/// ([`read_hwmon_temp_c`] — the AMD `k10temp` / `zenpower`
/// `temp1_input`), then the `cpu_thermal` thermal-zone scan (the
/// pre-C9-04 fallback, kept verbatim). The first finite reading
/// wins.
///
/// Every failure class degrades to `f64::NAN`: no hwmon class at
/// all, no matching sensor name, no matching `cpu_thermal` zone
/// (e.g. the host's iwlwifi-only thermal set), an unreadable
/// `name` / `type`, or an unreadable / non-numeric reading (the
/// ENODATA sensor case). Nothing panics — std `fs` reads only, no
/// parsing that can divide. Runs on the poller thread (plan D6 —
/// never the render thread).
pub fn read_cpu_temp_c() -> f64 {
    let hwmon = read_hwmon_temp_c();
    if hwmon.is_finite() {
        return hwmon; // the hwmon source (k10temp/zenpower) wins.
    }
    // The fallback: the `cpu_thermal` thermal-zone scan (the
    // pre-C9-04 form, kept verbatim).
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

/// The hwmon CPU-temperature scan (the GUI's C9-04/D-3): walk
/// `/sys/class/hwmon/`, read each `hwmon*` device's `name` (the
/// kernel sensor name, trimmed + case-insensitive), select the
/// preferred one by name only ([`select_hwmon_dir`] — `k10temp`
/// first, else `zenpower`, never a fixed `hwmonN` index — the
/// index is unstable across boots/CPUs), and return that device's
/// `temp1_input` (millidegrees ÷ 1000) in °C.
///
/// Every failure class degrades to `f64::NAN`: no hwmon class at
/// all, no `hwmon*` device with a readable `name`, no matching
/// sensor name, or an unreadable / non-numeric / non-finite
/// `temp1_input` (the ENODATA sensor case — e.g. the host's `asus`
/// / `iwlwifi` hwmons). Nothing panics — std `fs` reads only.
fn read_hwmon_temp_c() -> f64 {
    let Ok(devices) = fs::read_dir("/sys/class/hwmon") else {
        return f64::NAN; // no hwmon class at all.
    };
    let mut names: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in devices.flatten() {
        let Ok(raw_name) = entry.file_name().into_string() else {
            continue; // a non-UTF-8 device name: skip.
        };
        if !raw_name.starts_with("hwmon") {
            continue; // not an hwmon device dir.
        }
        let dir = entry.path();
        let Ok(name) = fs::read_to_string(dir.join("name")) else {
            continue; // the device's `name` is unreadable: not it.
        };
        names.push(name.trim().to_string());
        dirs.push(dir);
    }
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let Some(selected) = select_hwmon_dir(&name_refs) else {
        return f64::NAN; // no `k10temp` / `zenpower` device.
    };
    let Some(dir) = names.iter().position(|name| name == selected).and_then(|i| dirs.get(i))
    else {
        // `selected` came from `names`: not expected, still degrades.
        return f64::NAN;
    };
    read_hwmon_temp1_c(dir)
}

/// The hwmon name selection (the GUI's C9-04/D-3 — pure, I/O-free):
/// prefer `k10temp` (the AMD classic CPU sensor) over `zenpower`
/// (the newer AMD sensor) over no match. Case-insensitive on the
/// trimmed name; by name only — a fixed `hwmonN` index is never
/// matched (it is unstable across boots/CPUs).
fn select_hwmon_dir<'a>(names: &'a [&'a str]) -> Option<&'a str> {
    let mut zenpower: Option<&str> = None;
    for name in names {
        let trimmed = name.trim();
        if trimmed.eq_ignore_ascii_case("k10temp") {
            return Some(trimmed); // the preferred sensor wins.
        }
        if trimmed.eq_ignore_ascii_case("zenpower") {
            zenpower.get_or_insert(trimmed); // the fallback: first wins.
        }
    }
    zenpower
}

/// One hwmon device's `temp1_input` (millidegrees) in °C; every
/// failure class (missing / unreadable / non-numeric / non-finite)
/// degrades to NaN (the no-panic contract — the ENODATA sensor
/// case, e.g. the host's `asus` / `iwlwifi` hwmons).
fn read_hwmon_temp1_c(dir: &Path) -> f64 {
    let Ok(raw) = fs::read_to_string(dir.join("temp1_input")) else {
        return f64::NAN; // missing (ENODATA sensor) or unreadable.
    };
    // A non-numeric / non-finite reading degrades to NaN (no panic).
    parse_millidegrees(&raw).unwrap_or(f64::NAN)
}

/// One thermal zone's `temp` (millidegrees) in °C; every failure
/// class (missing / unreadable / non-numeric / non-finite)
/// degrades to NaN (the no-panic contract).
fn read_zone_temp_c(zone_dir: &Path) -> f64 {
    let Ok(raw) = fs::read_to_string(zone_dir.join("temp")) else {
        return f64::NAN; // missing (ENODATA sensor) or unreadable.
    };
    // A non-numeric / non-finite reading degrades to NaN (no panic).
    parse_millidegrees(&raw).unwrap_or(f64::NAN)
}

/// One millidegree reading (the kernel's `temp` / `temp1_input`
/// encoding) in °C; a non-numeric or non-finite reading yields
/// `None` (the no-panic contract).
fn parse_millidegrees(raw: &str) -> Option<f64> {
    match raw.trim().parse::<f64>() {
        Ok(millidegrees) if millidegrees.is_finite() => Some(millidegrees / 1000.0),
        _ => None, // a non-numeric reading: not data.
    }
}

// ---------------------------------------------------------------------
// The window filter + the graphs overlay panel (TUI-07 — part 3:
// the `[w]` window cycle + the `[g]` topmost block sparkline
// panel; TUI-16 composes it over the zones).
// ---------------------------------------------------------------------

/// The semantic palette (Grand Design §3.2, the exact values — the
/// ui.rs palette mirror, only the members this panel uses).
///
/// Cyan: the plotted bar values.
const CYAN: Color = Color::Rgb(0x00, 0xD4, 0xFF);
/// Grey: the N/A notes (0 ≠ N/A — an absent value is never a zero
/// bar; a degradation, never an alarm).
const NA_GRAY: Color = Color::Rgb(0x8A, 0x8A, 0x94);
/// Slate: the panel background.
const SLATE: Color = Color::Rgb(0x1E, 0x1E, 0x24);
/// Dim grey: the labels, the min/max, the sample-count line.
const DIM: Color = Color::Rgb(0x8A, 0x8A, 0x96);

/// The window filter (the `[w]` action's pure core): the samples
/// with `t ≥ newest_t − window_min·60`, oldest → newest.
///
/// The newest stamp is the last element of the source — `t` is
/// non-decreasing (TUI-05) and the ring iterates oldest → newest.
/// The filter works over any sample iterator (not just the ring);
/// an empty source yields an empty window, and a 0-minute window
/// collapses to the newest stamp's equal-stamp run — never a
/// panic.
pub fn window_samples<'a>(
    samples: impl Iterator<Item = &'a GraphSample>,
    window_min: u32,
) -> Vec<&'a GraphSample> {
    let samples: Vec<&'a GraphSample> = samples.collect();
    let Some(newest) = samples.last().copied() else {
        return Vec::new(); // no samples: an empty window.
    };
    let cutoff = newest.t - f64::from(window_min) * 60.0;
    samples
        .into_iter()
        .filter(|sample| sample.t >= cutoff)
        .collect()
}

/// One plotted series: its display label + its field projection
/// off the sample.
type Series = (&'static str, fn(&GraphSample) -> f64);

/// The row's label cell: a leading space + the 13-char padded
/// label (all five series labels are ≤ 12 chars).
const LABEL_CELL: usize = 14;

/// Render the graphs overlay panel (the `[g]` toggle — TUI-16
/// draws it last, topmost, over the whole zone area, the C21-36
/// modal idiom): a titled block on the slate background, one
/// block-bar sparkline row per series in the GUI's fixed order,
/// then a sample-count line.
///
/// The rows (the plan's frozen order): CPU FREQ MHz, VDDCR_CPU
/// mV, VDDCR_SOC mV, CPU TEMP °C, MEM BW GB/s. Each row is a dim
/// label (left) + an ASCII bar plot (one column per windowed
/// sample, right-aligned — newest right, the time axis; a
/// bucketed max per column when the samples outnumber the
/// columns; each finite value quantized to the row's finite
/// min/max span over the 8 block elements, a NaN sample = a gap)
/// and a dim min/max (right). A series with no finite sample in
/// the window draws its label + an N/A note only (the GUI's
/// 0 ≠ N/A rule).
///
/// No-panic contract: every division is span-guarded (a flat
/// series quantizes to a full bar, not a NaN level), a zero /
/// too-small `area` renders nothing, and the empty + one-sample
/// states render without any special-case panic path.
pub fn render_graphs_panel(
    frame: &mut Frame,
    graph: &GraphState,
    window_min: u32,
    area: Rect,
) {
    // A zero / too-small rect cannot host the borders + a row:
    // draw nothing (the no-panic contract).
    if area.is_empty() || area.width < 3 || area.height < 3 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title(format!(
            "GRAPHS (last {window_min} min — [w] window · [g] close)"
        ))
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return; // the borders consumed the rect: nothing to plot.
    }

    let windowed = window_samples(graph.samples.iter(), window_min);
    // The GUI's fixed series order (plan §2.3).
    let series: [Series; 5] = [
        ("CPU FREQ MHz", |s: &GraphSample| s.cpu_freq_mhz),
        ("VDDCR_CPU mV", |s: &GraphSample| s.vddcr_cpu_mv),
        ("VDDCR_SOC mV", |s: &GraphSample| s.vddcr_soc_mv),
        ("CPU TEMP °C", |s: &GraphSample| s.cpu_temp_c),
        ("MEM BW GB/s", |s: &GraphSample| s.bandwidth_gbps),
    ];
    let mut constraints = vec![Constraint::Length(1); series.len()];
    constraints.push(Constraint::Length(1)); // the sample-count line
    constraints.push(Constraint::Fill(1)); // slack (clipped, never scrolled)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    for (i, (label, field)) in series.iter().enumerate() {
        render_series_row(frame, label, *field, &windowed, rows[i]);
    }
    let count = windowed.len();
    let unit = if count == 1 { "sample" } else { "samples" };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{count} {unit} in window"),
            Style::default().fg(DIM),
        ))),
        rows[series.len()],
    );
}

/// One series row: the dim label, the cyan bar plot, the dim
/// min/max — or the label + the grey N/A note when the window
/// carries no finite sample (the 0 ≠ N/A rule — no bar, no
/// min/max).
fn render_series_row(
    frame: &mut Frame,
    label: &str,
    field: fn(&GraphSample) -> f64,
    windowed: &[&GraphSample],
    area: Rect,
) {
    if area.is_empty() {
        return; // a clipped row: nothing to draw.
    }
    let label_span = Span::styled(format!(" {label:<13}"), Style::default().fg(DIM));
    let line = {
        let values: Vec<f64> = windowed
            .iter()
            .map(|s| field(s))
            .filter(|v| v.is_finite())
            .collect();
        if values.is_empty() {
            // No finite sample in the window: label + the honest
            // N/A note only (0 ≠ N/A).
            Line::from(vec![
                label_span,
                Span::styled(" N/A", Style::default().fg(NA_GRAY)),
            ])
        } else {
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let right = format!(" {min:.1} / {max:.1}");
            // The plot takes what the row has left after the label
            // column + the right min/max (zero width = no bars).
            let plot_width = usize::from(area.width)
                .saturating_sub(LABEL_CELL)
                .saturating_sub(right.len());
            let plot_values: Vec<f64> = windowed.iter().map(|s| field(s)).collect();
            Line::from(vec![
                label_span,
                Span::styled(
                    sparkline(&plot_values, plot_width, min, max),
                    Style::default().fg(CYAN),
                ),
                Span::styled(right, Style::default().fg(DIM)),
            ])
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The 8 block elements, shortest → tallest: a one-row bar plot
/// quantizes each column's value to these 8 levels over the row's
/// finite min/max span.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// One series' bar plot as a single row of block elements.
///
/// The columns: one per sample (right-aligned — newest right, the
/// time axis) when the samples fit in `width`, else a bucketed max
/// per column (the peaks survive the downsize). A NaN sample (or a
/// NaN-only bucket) is a gap. Each finite value quantizes to the 8
/// [`BLOCKS`] levels over `min` / `max` (the row's finite span); a
/// flat span (min == max) quantizes to a full bar — the guarded
/// division, never NaN.
fn sparkline(values: &[f64], width: usize, min: f64, max: f64) -> String {
    let mut plot = String::with_capacity(width);
    if width == 0 {
        return plot;
    }
    if values.len() <= width {
        for _ in 0..(width - values.len()) {
            plot.push(' ');
        }
        for value in values {
            plot.push(if value.is_finite() {
                BLOCKS[quantize(*value, min, max)]
            } else {
                ' ' // a NaN sample: a gap, never a zero bar.
            });
        }
    } else {
        for column in 0..width {
            let lo = column * values.len() / width;
            let hi = (column + 1) * values.len() / width;
            let peak = values[lo..hi]
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(f64::NEG_INFINITY, f64::max);
            plot.push(if peak.is_finite() {
                BLOCKS[quantize(peak, min, max)]
            } else {
                ' '
            });
        }
    }
    plot
}

/// Quantize a finite `value` to a 0..=7 [`BLOCKS`] level over the
/// row's finite min/max span (guarded: a flat span maps to a full
/// bar, not a division by zero).
fn quantize(value: f64, min: f64, max: f64) -> usize {
    if max == min {
        return BLOCKS.len() - 1; // a flat series: a full bar.
    }
    let ratio = (value - min) / (max - min); // value in [min, max]: [0, 1]
    (ratio * 7.0).round().clamp(0.0, 7.0) as usize
}

// ---------------------------------------------------------------------
// Tests (headless: no TTY — the record fixtures are synthetic
// snapshots, the record core is pure; the scan helpers are pure,
// the two full-scan tests host-dependent (no-panic + a sane
// reading, never a fixed value); the window filter + panel tests
// use ratatui's in-memory TestBackend).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
    use ramsleuth_telemetry::amd_readout::{map_amd, AmdReadout};
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemPlatform;

    /// An AMD readout with VDDCR_SOC = `soc_mv` + Vcore =
    /// `vcore_mv` (the `u16` voltage mapping: in-band → `Value`,
    /// 0 → `Na(ParseError)`) and every other cell sanity-mapped
    /// (the GUI graph.rs `readout_with_soc` fixture shape).
    fn fixture_amd(soc_mv: u16, vcore_mv: u16) -> AmdReadout {
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
                vcore_mv,
            },
        })
    }

    /// A snapshot with the given platform core frequency (or an
    /// all-Na platform when `None`) + the given AMD readout (or an
    /// all-Na AMD branch when `None`) — every other cell Na
    /// (host-independent).
    fn snapshot(freq_mhz: Option<f64>, amd: Option<AmdReadout>) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: match amd {
                Some(readout) => Section::Value(readout),
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
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// A fully-populated snapshot (every source finite) for the
    /// repeat-record tests.
    fn populated() -> SystemMemoryTelemetry {
        snapshot(Some(1800.0), Some(fixture_amd(1100, 1150)))
    }

    /// (a) A fresh state is empty, at the graphs capacity (1800 —
    /// not the ring's 300 default), and not full.
    #[test]
    fn fresh_state_is_empty_at_graph_capacity() {
        let state = GraphState::default();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
        assert_eq!(state.samples.capacity(), GRAPH_CAPACITY);
        assert_eq!(
            state.samples.capacity(),
            1800,
            "the graphs ring is 1800-deep, not the 300 default"
        );
        assert!(!state.samples.is_full());
    }

    /// (b) A populated poll records one sample with every field
    /// verbatim + a finite, non-negative `t`.
    #[test]
    fn record_populates_a_finite_sample() {
        let mut state = GraphState::default();
        record_graph_sample(&mut state, &Some(populated()), 47.3, 26.35);
        assert_eq!(state.len(), 1);
        let sample = state.samples.last().expect("the recorded sample");
        assert!(
            sample.t.is_finite() && sample.t >= 0.0,
            "t must be unix seconds: {}",
            sample.t
        );
        assert_eq!(Some(sample.cpu_freq_mhz), Some(1800.0));
        assert_eq!(Some(sample.vddcr_cpu_mv), Some(1150.0));
        assert_eq!(Some(sample.vddcr_soc_mv), Some(1100.0));
        assert_eq!(Some(sample.cpu_temp_c), Some(47.3));
        assert_eq!(Some(sample.bandwidth_gbps), Some(26.35));
    }

    /// (c) The Na guard: `Na` cells degrade to NaN (never a fake 0)
    /// while the finite cells land verbatim — a sample with ≥ 1
    /// finite field is appended.
    #[test]
    fn record_na_guards_degrade_to_nan() {
        // freq Na (no platform clock), SOC Na (the raw 0 maps to
        // Na(ParseError)), Vcore finite, temp finite, bandwidth NaN.
        let snap = snapshot(None, Some(fixture_amd(0, 1150)));
        let mut state = GraphState::default();
        record_graph_sample(&mut state, &Some(snap), 42.0, f64::NAN);
        assert_eq!(state.len(), 1);
        let sample = state.samples.last().expect("the recorded sample");
        assert!(
            sample.cpu_freq_mhz.is_nan(),
            "an Na clock must be NaN, not 0.0"
        );
        assert!(
            sample.vddcr_soc_mv.is_nan(),
            "an Na SOC rail must be NaN, not 0.0"
        );
        assert_eq!(Some(sample.vddcr_cpu_mv), Some(1150.0));
        assert_eq!(Some(sample.cpu_temp_c), Some(42.0));
        assert!(sample.bandwidth_gbps.is_nan());
    }

    /// (c′) Without a snapshot at all, the passed scan / bandwidth
    /// values are the only possible finite fields.
    #[test]
    fn record_without_snapshot_keeps_passed_values() {
        let mut state = GraphState::default();
        record_graph_sample(&mut state, &None, 39.5, 12.75);
        assert_eq!(state.len(), 1);
        let sample = state.samples.last().expect("the recorded sample");
        assert!(sample.cpu_freq_mhz.is_nan());
        assert!(sample.vddcr_cpu_mv.is_nan());
        assert!(sample.vddcr_soc_mv.is_nan());
        assert_eq!(Some(sample.cpu_temp_c), Some(39.5));
        assert_eq!(Some(sample.bandwidth_gbps), Some(12.75));
    }

    /// (d) The no-hole rule: an all-NaN reading (no snapshot + no
    /// temp + no bandwidth, or an all-Na snapshot) appends nothing.
    #[test]
    fn all_nan_appends_nothing() {
        let mut state = GraphState::default();
        // No snapshot, no temp, no bandwidth.
        record_graph_sample(&mut state, &None, f64::NAN, f64::NAN);
        // An all-Na snapshot (no clock, no readout), no temp, no
        // bandwidth.
        record_graph_sample(&mut state, &Some(snapshot(None, None)), f64::NAN, f64::NAN);
        assert!(
            state.is_empty(),
            "an all-NaN sample is a hole, never a point"
        );
        assert_eq!(state.len(), 0);
    }

    /// (e) The ring wraps at the graphs capacity: push 1801
    /// populated samples — the oldest is evicted, 1800 remain in
    /// order, and the capacity never grows.
    #[test]
    fn record_wraps_at_capacity() {
        let mut state = GraphState::default();
        // One distinguishable sample per poll (the freq carries the
        // poll index).
        for i in 0..=GRAPH_CAPACITY {
            let snap = snapshot(Some(i as f64), Some(fixture_amd(1100, 1150)));
            record_graph_sample(&mut state, &Some(snap), f64::NAN, f64::NAN);
        }
        assert!(state.samples.is_full());
        assert_eq!(state.len(), GRAPH_CAPACITY, "the depth must stay 1800");
        assert_eq!(
            state.samples.capacity(),
            GRAPH_CAPACITY,
            "the capacity must never grow"
        );
        assert_eq!(
            Some(state.samples.iter().next().expect("oldest").cpu_freq_mhz),
            Some(1.0),
            "the oldest sample (poll 0) must be evicted"
        );
        assert_eq!(
            Some(state.samples.last().expect("newest").cpu_freq_mhz),
            Some(GRAPH_CAPACITY as f64),
            "the newest sample (poll 1800) must be kept"
        );
    }

    /// (f) `t` is non-decreasing across consecutive records (the
    /// wall-clock stamp: equal is allowed, a regression is not).
    #[test]
    fn record_timestamps_are_non_decreasing() {
        let mut state = GraphState::default();
        for _ in 0..3 {
            record_graph_sample(&mut state, &Some(populated()), f64::NAN, f64::NAN);
        }
        let stamps: Vec<f64> = state.samples.iter().map(|s| s.t).collect();
        assert_eq!(stamps.len(), 3);
        for pair in stamps.windows(2) {
            assert!(pair[0] <= pair[1], "t must be non-decreasing: {pair:?}");
        }
    }

    // ------------------------------------------------------------------
    // read_cpu_temp_c — the CPU-temperature source scan (TUI-06 —
    // the helper level is pure; the full scan is host-dependent,
    // QA eyeballs the live value).
    // ------------------------------------------------------------------

    /// (g) The ordered scan never panics; a reading is either the
    /// honest NaN (no hwmon sensor + no `cpu_thermal` zone) or a
    /// finite value in a sane temperature range (this host: the
    /// `k10temp` hwmon source).
    #[test]
    fn read_cpu_temp_c_never_panics_and_degrades_to_nan() {
        let temp = read_cpu_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    /// (h) The hwmon scan never panics: a reading is either the
    /// honest NaN (no `k10temp` / `zenpower` device — a host
    /// without the AMD sensor) or a finite value in a sane
    /// temperature range (this host's `k10temp` `temp1_input`).
    #[test]
    fn read_hwmon_temp_c_never_panics_and_degrades_to_nan() {
        let temp = read_hwmon_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    /// (i) The name selection prefers `k10temp` over `zenpower`,
    /// falls back to `zenpower`, and matches by name only (a fixed
    /// `hwmonN` index is never a sensor name) — case-insensitive,
    /// trimmed.
    #[test]
    fn select_hwmon_dir_prefers_k10temp_over_zenpower() {
        assert_eq!(select_hwmon_dir(&["zenpower", "k10temp"]), Some("k10temp"));
        assert_eq!(select_hwmon_dir(&["k10temp", "zenpower"]), Some("k10temp"));
        assert_eq!(select_hwmon_dir(&["zenpower"]), Some("zenpower"));
        assert_eq!(select_hwmon_dir(&[]), None, "no devices: no source");
        assert_eq!(
            select_hwmon_dir(&["acpi", "nvme", "asus", "iwlwifi_1"]),
            None,
            "non-CPU sensor names: no source"
        );
        assert_eq!(
            select_hwmon_dir(&["hwmon4"]),
            None,
            "a fixed hwmonN index is not a sensor name"
        );
        assert_eq!(
            select_hwmon_dir(&[" K10TEMP "]),
            Some("K10TEMP"),
            "the match is case-insensitive + trimmed"
        );
    }

    /// (j) The millidegree parse (shared by the zone + hwmon
    /// readers) maps `33125` → `33.125` °C (trimmed — the kernel's
    /// trailing newline) and rejects non-numeric / non-finite
    /// readings.
    #[test]
    fn parse_millidegrees_maps_and_rejects() {
        assert_eq!(parse_millidegrees("33125"), Some(33.125));
        assert_eq!(parse_millidegrees(" 29125\n"), Some(29.125));
        assert_eq!(parse_millidegrees("garbage"), None, "non-numeric: not data");
        assert_eq!(parse_millidegrees(""), None, "empty: not data");
        assert_eq!(parse_millidegrees("inf"), None, "non-finite: not data");
        assert_eq!(parse_millidegrees("1e999"), None, "overflow: not data");
    }

    /// (k) Every source reader degrades to NaN on a missing source
    /// (no `temp1_input` on the hwmon device, no `temp` on the zone
    /// — the no-source classes never panic).
    #[test]
    fn temp_source_readers_degrade_to_nan_on_missing_sources() {
        let missing = Path::new("/nonexistent/ramsleuth_tui_06");
        assert!(read_hwmon_temp1_c(missing).is_nan(), "a missing hwmon device: NaN");
        assert!(read_zone_temp_c(missing).is_nan(), "a missing thermal zone: NaN");
    }

    // ------------------------------------------------------------------
    // window_samples + render_graphs_panel — the window filter and
    // the graphs overlay panel (TUI-07 part 3 — the render tests
    // use ratatui's in-memory TestBackend, no TTY).
    // ------------------------------------------------------------------

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// One all-finite sample (the panel tests' base case; the
    /// frequency is the distinguishing value).
    fn sample(t: f64, freq_mhz: f64) -> GraphSample {
        GraphSample {
            t,
            cpu_freq_mhz: freq_mhz,
            vddcr_cpu_mv: 1150.0,
            vddcr_soc_mv: 1100.0,
            cpu_temp_c: 45.0,
            bandwidth_gbps: 26.0,
        }
    }

    /// Draw `graph` at `window_min` into an in-memory terminal of
    /// the given size and return the buffer as text (one line per
    /// row, trailing spaces trimmed) — the ui.rs draw pattern.
    fn draw_panel(graph: &GraphState, window_min: u32, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render_graphs_panel(f, graph, window_min, Rect::new(0, 0, width, height)))
            .expect("render must not panic");
        let buffer = completed.buffer;
        let width = usize::from(buffer.area().width);
        buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// (l) The window filter keeps only the samples with
    /// `t ≥ newest − window_min·60` (oldest → newest), keeps
    /// everything for a wide window, collapses to the newest stamp
    /// alone for a 0-minute window, and yields an empty window for
    /// an empty source (never a panic).
    #[test]
    fn window_samples_filters_to_the_window() {
        let mut ring = RingBuffer::<GraphSample>::with_capacity(10);
        ring.push(sample(1000.0, 100.0));
        ring.push(sample(1030.0, 200.0));
        ring.push(sample(1060.0, 300.0));
        ring.push(sample(1100.0, 400.0));
        let state = GraphState { samples: ring };

        // 1 min = 60 s: newest 1100, cutoff 1040 → the last two,
        // oldest → newest.
        let windowed = window_samples(state.samples.iter(), 1);
        assert_eq!(
            windowed.iter().map(|s| s.t).collect::<Vec<_>>(),
            vec![1060.0, 1100.0]
        );
        // A wide window keeps everything.
        let windowed = window_samples(state.samples.iter(), 10);
        assert_eq!(windowed.len(), 4);
        // A 0-minute window collapses to the newest stamp alone.
        let windowed = window_samples(state.samples.iter(), 0);
        assert_eq!(
            windowed.iter().map(|s| s.t).collect::<Vec<_>>(),
            vec![1100.0]
        );
        // An empty source: an empty window (no panic).
        let empty = GraphState::default();
        assert!(window_samples(empty.samples.iter(), 5).is_empty());
    }

    /// (m) An empty state renders the titled panel with every
    /// series label + the honest N/A note (0 ≠ N/A) and the zero
    /// sample-count line — no bars, no panic.
    #[test]
    fn render_empty_state() {
        let state = GraphState::default();
        let text = draw_panel(&state, 5, 100, 12);

        // the panel title: the window minutes + the key hints
        assert!(text.contains("GRAPHS (last 5 min"), "{text}");
        assert!(text.contains("[w] window"), "{text}");
        assert!(text.contains("[g] close"), "{text}");
        // every series label + the honest N/A note (0 ≠ N/A)
        for label in [
            "CPU FREQ MHz",
            "VDDCR_CPU mV",
            "VDDCR_SOC mV",
            "CPU TEMP °C",
            "MEM BW GB/s",
        ] {
            assert!(text.contains(label), "missing {label}:\n{text}");
        }
        assert_eq!(text.matches("N/A").count(), 5, "{text}");
        // the bottom sample-count line
        assert!(text.contains("0 samples in window"), "{text}");
    }

    /// (n) One all-finite sample renders every series as its flat
    /// full bar (min == max → the quantize guard) with the
    /// identical min/max — no N/A anywhere.
    #[test]
    fn render_one_sample() {
        let mut state = GraphState::default();
        state.samples.push(sample(1000.0, 1800.0));
        let text = draw_panel(&state, 5, 100, 12);

        assert!(text.contains("1 sample in window"), "{text}");
        assert!(text.contains("1800.0 / 1800.0"), "{text}");
        assert!(text.contains("1150.0 / 1150.0"), "{text}");
        assert!(text.contains("1100.0 / 1100.0"), "{text}");
        assert!(text.contains("45.0 / 45.0"), "{text}");
        assert!(text.contains("26.0 / 26.0"), "{text}");
        // the flat span quantizes to a full block
        assert!(text.contains('█'), "{text}");
        assert_eq!(text.matches("N/A").count(), 0, "{text}");
    }

    /// (o) The full 1800-deep ring renders without panicking: the
    /// 60-min window keeps everything, the bucketed plot survives
    /// the downsize, and the newest (max) column is a full block.
    #[test]
    fn render_full_ring() {
        let mut state = GraphState::default();
        for i in 0..GRAPH_CAPACITY {
            state.samples.push(sample(i as f64 * 2.0, 1000.0 + i as f64));
        }
        assert!(state.samples.is_full());
        let text = draw_panel(&state, 60, 100, 12);

        assert!(text.contains("GRAPHS (last 60 min"), "{text}");
        assert!(text.contains("1800 samples in window"), "{text}");
        // the freq span 1000.0 → 2799.0 (the newest sample's value)
        assert!(text.contains("1000.0 / 2799.0"), "{text}");
        // the newest (max) column quantizes to a full block
        assert!(text.contains('█'), "{text}");
    }

    /// (p) A series with no finite sample in the window draws its
    /// label + the N/A note only (the 0 ≠ N/A rule) while the
    /// finite series plots its bars.
    #[test]
    fn render_all_nan_series() {
        let mut state = GraphState::default();
        // Only the frequency is finite; the other four series are
        // all-NaN in the window.
        for i in 0..3 {
            state.samples.push(GraphSample {
                t: 1000.0 + i as f64 * 2.0,
                cpu_freq_mhz: 1800.0 + i as f64 * 100.0,
                vddcr_cpu_mv: f64::NAN,
                vddcr_soc_mv: f64::NAN,
                cpu_temp_c: f64::NAN,
                bandwidth_gbps: f64::NAN,
            });
        }
        let text = draw_panel(&state, 5, 100, 12);

        // the finite series plots its bars with its min/max...
        assert!(text.contains("1800.0 / 2000.0"), "{text}");
        assert!(text.contains('█'), "{text}");
        // ...the four all-NaN series draw label + N/A note only
        assert_eq!(text.matches("N/A").count(), 4, "{text}");
    }

    /// (q) The window clips: of two samples 200 s apart, a 1-min
    /// window plots only the newest — the clipped sample's value
    /// appears nowhere.
    #[test]
    fn render_window_clipping() {
        let mut state = GraphState::default();
        state.samples.push(sample(1000.0, 123.4));
        state.samples.push(sample(1200.0, 678.9));
        let text = draw_panel(&state, 1, 100, 12);

        assert!(text.contains("1 sample in window"), "{text}");
        // only the newest value plots (min == max = the newest)
        assert!(text.contains("678.9 / 678.9"), "{text}");
        assert!(
            !text.contains("123.4"),
            "the clipped sample's value must appear nowhere:\n{text}"
        );
    }

    /// (r) A zero rect (and a rect too small for the borders) draws
    /// nothing, never panics.
    #[test]
    fn render_zero_area_never_panics() {
        let mut state = GraphState::default();
        state.samples.push(sample(1000.0, 1800.0));
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|f| {
                // a zero rect + a rect too small for the borders
                render_graphs_panel(f, &state, 5, Rect::new(0, 0, 0, 0));
                render_graphs_panel(f, &state, 5, Rect::new(0, 0, 2, 2));
            })
            .expect("render must not panic");
    }
}
