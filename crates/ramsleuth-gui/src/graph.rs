//! The Graphs window: the five-series graph state + the hand-rolled
//! render (C7-20) + the interactive layer (C7-22: the hover
//! crosshair, the horizontal pan, the 1/5/15/60-min window — plan
//! D-3 / D-4 / D-5).
//!
//! The dedicated monitoring window (C7-21 spawns it as the eframe
//! 0.27.2 deferred child viewport, C7-22 makes it interactive)
//! renders five series over a selectable time window (default: the
//! newest five minutes):
//!
//! - `CPU FREQ (MHz)` — the live core frequency
//!   (`SystemPlatform.cpu_clock_mhz`).
//! - `VDDCR_CPU` — no source anywhere (the unprivileged SMU surface
//!   carries no VDDCR_CPU cell): a permanent label-only row whose
//!   label carries the bare `N/A` note, no fake geometry
//!   (D-4).
//! - `VDDCR_SOC (mV)` — the AMD SOC rail
//!   (`AmdReadout.voltages.vddcr_soc_mv`).
//! - `CPU TEMP (°C)` — the ordered CPU-temp source scan
//!   ([`read_cpu_temp_c`] — C9-04, D-3: the `k10temp` / `zenpower`
//!   hwmon `temp1_input` first, the `cpu_thermal` thermal zone as
//!   the fallback), else an honest no-source row (D-4).
//! - `MEM BANDWIDTH (GB/s)` — a step series from the latest bench /
//!   burn-in `Memory · Read` figure (D-5 — it rides the existing
//!   bench state, never a continuous sampler): a no-source row
//!   until the first sample.
//!
//! The state + render:
//!
//! - [`GraphSample`] / [`GraphState`] — one timestamped five-field
//!   sample per successful poll (an absent value = `f64::NAN`, the
//!   non-finite-skip rule of the plot) in the
//!   [`GRAPH_CAPACITY`]-deep ring (1800 = 60 min at the 2 s poll
//!   cadence — the `history.rs` [`RingBuffer`] reused, generic,
//!   bounded). The background poller is the only writer (D6 — the
//!   Na-guarded [`record_graph_sample`] hook appends one sample per
//!   successful poll); the render thread + the child window are
//!   pure readers (the child's one permitted write is the C9-03
//!   `Poll` combo over the shared settings knob — the settings-
//!   panel D6 precedent).
//! - [`read_cpu_temp_c`] — the ordered CPU-temp source scan (the
//!   runtime source of D-4, extended by C9-04 / D-3 — the
//!   `k10temp` / `zenpower` hwmon `temp1_input` first, the
//!   `cpu_thermal` thermal zone as the fallback): every failure
//!   class (no hwmon class, no matching sensor, no matching zone,
//!   an unreadable / non-numeric reading) degrades to `f64::NAN`;
//!   nothing panics (std `fs` only — poller-thread I/O, D6: it
//!   runs in the poller, never the render thread).
//! - [`render_graphs_window`] — the interactive render: a
//!   `CentralPanel` (SLATE fill, the zone idiom) with the title + a
//!   dim subtitle (the live window + the sample count), the
//!   window-resolution row (four selectable `1 / 5 / 15 / 60 min`
//!   buttons — the default 5 min is the static view of C7-20;
//!   selecting one resets the pan) + the `Poll` combo (C9-03, D-2:
//!   the shared poll-interval knob), and the five series rows in
//!   fixed order — each a hand-rolled time-windowed plot (the pure
//!   [`window_points`] helper over the view window: non-finite
//!   samples skipped, a flat window on the midline, every division
//!   guarded — the no-panic plot contract of D5). The interactive
//!   layer of C7-22 over the same rows: (a) a horizontal drag on
//!   any row pans the window (`pan_offset_s += dx ×
//!   seconds_per_px`, clamped to the data by [`clamp_pan`] — never
//!   a negative allocation, panning past the data clamps) + the dim
//!   `« pan »` hint; (b) on hover over any row, a full-height (all
//!   rows) vertical crosshair at the hovered x + a floating
//!   tooltip with the exact value of every series at the hovered
//!   timestamp (the t of the nearest sample — the pure
//!   [`hover_timestamp`] + [`tooltip_lines`] helpers —
//!   `1800 MHz · 1150 mV · 47.3 °C · 26.35 GB/s`-style, missing
//!   fields as `N/A`) + the `HH:MM:SS` timestamp; no hover → no
//!   crosshair. A series whose window holds no finite sample draws
//!   a label + the bare `N/A` note and no geometry (D-4 — a
//!   flat 0 line would be a lie, 0 ≠ N/A): the `VDDCR_CPU` note
//!   lives in the label (permanent), the `CPU TEMP` / `MEM
//!   BANDWIDTH` notes self-clear when data appears.
//!
//! - The render-local view state (`GraphView` — the window length +
//!   the pan offset) lives in the IdTypeMap of the child context
//!   (the D-3 mechanism: `ctx.data_mut` +
//!   `get_temp_mut_or_insert_with` — per-context, session-local,
//!   child-only, never in `TelemetryData` — the poller stays the
//!   only writer, D6), read-modify-write per frame (no context
//!   borrow held across the layout — the C7-18 idiom).
//!
//! **No new dependency (D-3):** the plot is a few dozen
//! `egui::Painter` calls (the `history.rs` idiom) — no chart
//! crate, the MSRV-1.75 lockfile stays untouched.
//!
//! **Pure core:** [`window_points`] / [`finite_min_max`] /
//! [`window_seconds_for`] / [`clamp_pan`] / [`nearest_sample`] /
//! [`hover_timestamp`] / [`tooltip_lines`] are I/O-free and
//! deterministic (the unit tests exercise them without an egui
//! context); [`render_graphs_window`] is the thin `egui` surface
//! over them (the live render is gated by C7-21).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Align2, Area, Color32, FontId, Id, Order, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::history::RingBuffer;
use crate::{AMBER, CYAN, SLATE};

/// The graph depth: 1800 samples = 60 minutes at the default 2 s
/// poll cadence (C7-20 — bounded memory, the `HISTORY_CAPACITY`
/// precedent: the `history.rs` `RingBuffer` reused, generic).
pub const GRAPH_CAPACITY: usize = 1800;


/// The supported time-window lengths (minutes) in the resolution
/// row (C7-22 item 7f): 1 / 5 / 15 / 60 min.
const WINDOW_MINUTES: [u32; 4] = [1, 5, 15, 60];

/// The default window length (minutes): the static five-minute view
/// of C7-20.
const DEFAULT_WINDOW_MINUTES: u32 = 5;

/// The poll-interval presets (ms) for the header's `Poll` combo
/// (C9-03, D-2): 0.5 / 1 / 2 / 5 / 10 s. The combo writes the
/// shared `GuiSettings.poll_interval_ms` knob (the settings panel's
/// drag value, C6-27 — the two always agree); the poller re-reads
/// it live per tick, so a selection takes effect on the next tick
/// without a restart.
const POLL_INTERVAL_PRESETS: [(u64, &str); 5] = [
    (500, "0.5 s"),
    (1000, "1 s"),
    (2000, "2 s"),
    (5000, "5 s"),
    (10000, "10 s"),
];

/// The dim pan hint on the resolution row (C7-22 item 7f): a drag
/// on any row pans the window.
const PAN_HINT: &str = "« pan »";

/// The tooltip background (a step lighter than the row background).
const TOOLTIP_BG: Color32 = Color32::from_rgb(0x24, 0x24, 0x2E);

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
/// The no-source note (D-4): the bare `N/A` — a source-less
/// series never fakes a 0 line (0 ≠ N/A) — the label + this
/// note, no geometry.
const NO_SOURCE_NOTE: &str = "N/A";

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
// The CPU-temperature source scan (D-4's runtime source; the C9-04
// ordered scan of D-3: hwmon first, thermal zone the fallback).
// ---------------------------------------------------------------------

/// The ordered CPU-temperature source scan (the runtime source of
/// D-4, extended by C9-04 / D-3): first the hwmon sensor
/// ([`read_hwmon_temp_c`] — the AMD `k10temp` / `zenpower`
/// `temp1_input`), then the pre-C9-04 `cpu_thermal` thermal-zone
/// scan (kept verbatim as the fallback). The first finite reading
/// wins.
///
/// Every failure class degrades to `f64::NAN`: no hwmon class at
/// all, no matching sensor name, no matching `cpu_thermal` zone
/// (e.g. the host's iwlwifi-only thermal set), an unreadable
/// `name` / `type`, or an unreadable / non-numeric reading (the
/// ENODATA sensor case). Nothing panics — std `fs` reads only, no
/// parsing that can divide. Runs on the poller thread (D6 — never
/// the render thread).
pub fn read_cpu_temp_c() -> f64 {
    let hwmon = read_hwmon_temp_c();
    if hwmon.is_finite() {
        return hwmon; // the hwmon source (k10temp/zenpower) wins.
    }
    // The fallback: the pre-C9-04 `cpu_thermal` thermal-zone scan
    // (kept verbatim).
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

/// The hwmon CPU-temperature scan (C9-04, D-3): walk
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

/// The hwmon name selection (C9-04, D-3 — pure, I/O-free): prefer
/// `k10temp` (the AMD classic CPU sensor) over `zenpower` (the
/// newer AMD sensor) over no match. Case-insensitive on the trimmed
/// name; by name only — a fixed `hwmonN` index is never matched
/// (it is unstable across boots/CPUs).
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
/// class (missing / unreadable / non-numeric / non-finite) degrades
/// to NaN (the no-panic contract).
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
// The render-local view state (C7-22 — the D-3 IdTypeMap mechanism).
// ---------------------------------------------------------------------

/// The interactive view state of the graphs window (C7-22 — the §3
/// GUI-internal shape): the time-window length + the pan offset.
///
/// Render-local (the D-3 mechanism): it lives in the IdTypeMap of
/// the child context (session-local, per-context —
/// `get_temp_mut_or_insert_with` is not cleared between frames, so
/// the view survives across the child repaints) and is touched only
/// by the child callback ([`render_graphs_window`]) — never in
/// [`GraphState`] / `TelemetryData` (the poller stays the only
/// writer, D6). Read-modify-write per frame: no context borrow is
/// held across the layout (the C7-18 idiom).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GraphView {
    /// The time-window length in minutes (one of [`WINDOW_MINUTES`];
    /// the default [`DEFAULT_WINDOW_MINUTES`] is the static view of
    /// C7-20).
    window_minutes: u32,
    /// The pan offset in seconds back from the newest sample
    /// (0 = anchored at the newest sample — the static view).
    /// Always clamped into the data by [`clamp_pan`] (never
    /// negative, never past the oldest sample).
    pan_offset_s: f64,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            window_minutes: DEFAULT_WINDOW_MINUTES,
            pan_offset_s: 0.0,
        }
    }
}

/// The view-state id in the IdTypeMap of the child context (a
/// function, not a `const` — `egui::Id::new` is not const in
/// pinned 0.27.2 — the C7-21 idiom).
fn view_id() -> Id {
    Id::new("ramsleuth_graph_view")
}

/// Read the view state (inserting the default on first use).
fn view_state(ctx: &egui::Context) -> GraphView {
    ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(view_id(), GraphView::default))
}

/// Persist the view state (the store half of the read-modify-write;
/// it runs once per frame, after the interaction).
fn store_view(ctx: &egui::Context, view: GraphView) {
    ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(view_id(), GraphView::default) = view);
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

/// Map a selected window length (minutes) to seconds (C7-22 item
/// 7f): only the four supported resolutions (1 / 5 / 15 / 60 min)
/// are accepted; anything else (a corrupted value read back from
/// the view state) falls back to the default (5 min = 300 s — the
/// static window of C7-20).
fn window_seconds_for(minutes: u32) -> f64 {
    match minutes {
        1 => 60.0,
        5 => 300.0,
        15 => 900.0,
        60 => 3600.0,
        _ => f64::from(DEFAULT_WINDOW_MINUTES) * 60.0,
    }
}

/// The `Poll` combo's selected text (C9-03, D-2): the preset label
/// for a stored value matching one of [`POLL_INTERVAL_PRESETS`], or
/// the exact millisecond figure (`custom` display) when it does not
/// (a hand-edited Settings value — the presets stay selectable, the
/// button shows the live knob).
fn poll_interval_display(ms: u64) -> String {
    match POLL_INTERVAL_PRESETS.iter().find(|(preset, _)| *preset == ms) {
        Some((_, label)) => (*label).to_owned(),
        None => format!("{ms} ms"),
    }
}

/// Clamp a pan offset (seconds back from the newest sample) into
/// the recorded data (C7-22 item 7f): the window may not reach
/// before the oldest sample (max pan = newest − oldest −
/// `window_secs`) and it may never be negative (never a negative
/// allocation). Returns 0.0 when there is nothing to pan (no
/// samples, one sample, a degenerate span, or the whole history
/// fits inside the window); a non-finite pan degrades to 0.0 (the
/// no-panic contract of D5).
fn clamp_pan(pan: f64, samples: &[GraphSample], window_secs: f64) -> f64 {
    if !pan.is_finite() {
        return 0.0;
    }
    let (Some(oldest), Some(newest)) = (samples.first(), samples.last()) else {
        return 0.0;
    };
    let span = newest.t - oldest.t;
    if !span.is_finite() || span <= 0.0 || !window_secs.is_finite() || span <= window_secs {
        // The data fits the window (or is degenerate): no pan.
        return 0.0;
    }
    pan.clamp(0.0, span - window_secs)
}

/// The sample nearest in time to `t` (ties resolve to the earlier
/// sample), or `None` for an empty slice or a non-finite `t` (the
/// no-panic contract of D5).
fn nearest_sample(samples: &[GraphSample], t: f64) -> Option<&GraphSample> {
    if !t.is_finite() {
        return None;
    }
    let mut best: Option<&GraphSample> = None;
    let mut best_dist = f64::INFINITY;
    for s in samples {
        let d = (s.t - t).abs();
        if d.is_finite() && d < best_dist {
            best_dist = d;
            best = Some(s);
        }
    }
    best
}

/// The timestamp of the sample nearest to a hovered x (C7-22 item
/// 7e): the inverse of the [`window_points`] normalization (x → t
/// over the panned window) + the nearest-sample lookup (the
/// crosshair snaps to a real sample, not the raw mapped time).
///
/// - `window` = the base time window `(t_start, t_end)`, anchored
///   at the newest sample (the static view); `pan` (seconds,
///   already clamped by [`clamp_pan`]) shifts it back: the
///   effective window is `(t_start − pan, t_end − pan)`.
/// - `x` / `width` = the hovered x in pixels relative to the left
///   edge of the row, and the width of the row. `x` is clamped to
///   `[0, width]` (a hover just outside the row snaps to the
///   window edge — no panic).
///
/// Returns `None` for no samples, a collapsed / non-finite window,
/// or a zero-width row (the crosshair never shows then).
fn hover_timestamp(
    samples: &[GraphSample],
    window: (f64, f64),
    pan: f64,
    x: f32,
    width: f32,
) -> Option<f64> {
    let (t_start, t_end) = window;
    if samples.is_empty()
        || !t_start.is_finite()
        || !t_end.is_finite()
        || !width.is_finite()
        || width <= 0.0
    {
        return None;
    }
    let eff_start = t_start - pan;
    let eff_end = t_end - pan;
    if eff_end <= eff_start {
        return None; // a collapsed (panned-away) window.
    }
    let frac = (f64::from(x) / f64::from(width)).clamp(0.0, 1.0);
    let t = eff_start + frac * (eff_end - eff_start);
    nearest_sample(samples, t).map(|s| s.t)
}

/// One tooltip value token (C7-22 item 7e): `prec` decimals
/// (`1800`, `1150`, `47.3`, `26.35`-style), or `N/A` when the
/// value is non-finite (D-4 — a missing field is an honest note,
/// never a fake 0).
fn value_token(value: f64, prec: usize) -> String {
    if value.is_finite() {
        format!("{:.*}", prec, value)
    } else {
        "N/A".to_owned()
    }
}

/// The crosshair tooltip lines for the timestamp `t` (C7-22 item
/// 7e): line 1 = the wall-clock time of the nearest sample
/// (`HH:MM:SS` from that unix `t` — the sample the values come
/// from), line 2 = the value of every series at that sample in
/// `1800 MHz · 1150 mV · 47.3 °C · 26.35 GB/s`-style (a
/// non-finite field degrades to `N/A` — D-4). Empty (no lines, no
/// panic) for no samples or a non-finite `t`.
fn tooltip_lines(samples: &[GraphSample], t: f64) -> Vec<String> {
    let Some(sample) = nearest_sample(samples, t) else {
        return Vec::new();
    };
    let secs = sample.t.floor().max(0.0) as u64;
    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    vec![
        format!("{h:02}:{m:02}:{s:02}"),
        format!(
            "{} MHz · {} mV · {} °C · {} GB/s",
            value_token(sample.cpu_freq_mhz, 0),
            value_token(sample.vddcr_soc_mv, 0),
            value_token(sample.cpu_temp_c, 1),
            value_token(sample.bandwidth_gbps, 2),
        ),
    ]
}

// ---------------------------------------------------------------------
// The egui surface (the hand-rolled immediate-mode plot, D-3).
// ---------------------------------------------------------------------

/// One series row (the `plot_series` idiom): a fixed-height row in
/// `ui` — the row background, the dim series label (top-left), and
/// either (a) the time-windowed geometry from [`window_points`] (a
/// filled dot for a single in-window sample, a polyline for more,
/// a dim newest-value readout top-right) or (b) when the window
/// holds no finite sample, the bare `N/A` note (D-4 — it
/// self-clears when data appears; the permanent `VDDCR_CPU` row
/// carries the note in its label instead — `note_in_label`) —
/// never fake geometry, never a panic.
///
/// The row senses drags (`Sense::drag()` — C7-22): the caller reads
/// the returned [`egui::Response`] for the pan (`dragged_by` +
/// `drag_delta`) and for the crosshair (`hover_pos`) — every row is
/// a pan target, and any row raises the crosshair.
fn graph_row(
    ui: &mut egui::Ui,
    samples: &[GraphSample],
    field: impl Fn(&GraphSample) -> f64,
    label: &str,
    color: Color32,
    window: (f64, f64),
    note_in_label: bool,
) -> egui::Response {
    let (t_start, t_end) = window;
    let row_width = ui.available_width().max(1.0);
    let desired = Rect::from_min_size(ui.cursor().min, Vec2::new(row_width, ROW_HEIGHT));
    let rect = desired.intersect(ui.available_rect_before_wrap());
    let response = ui.allocate_rect(rect, Sense::drag());
    let dim = ui.visuals().weak_text_color();
    let painter = ui.painter();

    // The row background (always, so a no-source row still shows
    // its label + the extent of the plot area).
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
            // (D-4) — no geometry, no fake 0 line. The permanent
            // `VDDCR_CPU` row carries the note in its label instead
            // (no double note).
            if !note_in_label {
                painter.text(
                    rect.right_top() + Vec2::new(-TEXT_MARGIN, TEXT_MARGIN),
                    Align2::RIGHT_TOP,
                    NO_SOURCE_NOTE,
                    FontId::monospace(11.0),
                    dim,
                );
            }
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

/// The interactive render of the dedicated graphs window (C7-22
/// over the basic render of C7-20; C7-21 spawns this as the body
/// of the deferred child viewport): a `CentralPanel` (SLATE fill,
/// the zone idiom) with the title + a dim subtitle (the live
/// window + the sample count), the window-resolution row, and the
/// five series rows in fixed order over the view window:
///
/// 1. `CPU FREQ (MHz)` — the live core frequency.
/// 2. `VDDCR_CPU` — the permanent no-source row (D-4: the label
///    carries `— N/A` — the series has no source
///    anywhere; no geometry, it self-populates if one ever
///    appears, no layout change — the row is data-driven).
/// 3. `VDDCR_SOC (mV)` — the AMD SOC rail.
/// 4. `CPU TEMP (°C)` — the thermal-zone scan; a no-source row
///    while the window holds no finite temp (D-4, self-clearing).
/// 5. `MEM BANDWIDTH (GB/s)` — the bench / burn-in step series
///    (D-5); a no-source row until the first sample.
///
/// The interactive layer (C7-22), driven by the render-local
/// [`GraphView`] in the IdTypeMap of the child context (the D-3
/// mechanism — per-context, session-local, child-only, never in
/// `TelemetryData`):
///
/// - the window-resolution row: four selectable `1 / 5 / 15 / 60
///   min` buttons (the default 5 min is the static view of C7-20;
///   selecting one resets the pan to 0) + the `Poll` combo (C9-03,
///   D-2: the poll-interval control — the presets write the shared
///   `settings.poll_interval_ms` knob the poller re-reads live; a
///   non-preset stored value shows its exact millisecond figure);
/// - horizontal pan: a drag on any row pans the window
///   (`pan_offset_s += dx × seconds_per_px`, clamped to the data
///   by [`clamp_pan`]) + the dim `« pan »` hint;
/// - the interactive vertical crosshair: on hover over any row, a
///   full-height (all rows) vertical line at the hovered x + a
///   floating tooltip with the exact value of every series at the
///   hovered timestamp (the t of the nearest sample —
///   [`hover_timestamp`] + [`tooltip_lines`]; missing fields as
///   `N/A`, D-4) + the `HH:MM:SS` timestamp; no hover → no
///   crosshair.
///
/// Pure read over [`GraphState`] (no I/O, D6 — the poller is the
/// only writer of the data fields): an empty state renders the
/// five no-source rows (the VDDCR_CPU row is permanent) and never
/// panics (the D5 contract; the `render_history` headless-test
/// precedent). The one permitted write is the `Poll` combo's
/// `poll_interval_ms: &mut u64` — the shared settings knob the
/// poller re-reads live (the settings-panel D6 write precedent,
/// C9-03, D-2).
pub fn render_graphs_window(ctx: &egui::Context, graph: &GraphState, poll_interval_ms: &mut u64) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(SLATE))
        .show(ctx, |ui| {
            let dim = ui.visuals().weak_text_color();
            // The render-local view state (C7-22, the D-3
            // IdTypeMap mechanism): read-modify-write — no context
            // borrow is held across the layout (the C7-18 idiom).
            let mut view = view_state(ctx);
            // One pass over the ring (the transient `Vec` is a
            // render-only copy — the `render_history` idiom; the
            // ring itself never grows, D-C4).
            let samples: Vec<GraphSample> = graph.samples.iter().copied().collect();
            // The view window: the selected resolution (default 5
            // min = the static view of C7-20), the base
            // [newest − W, newest] shifted back by the (clamped)
            // pan. An empty state collapses to [−W, 0] — every row
            // degrades to its no-source note, as in C7-20.
            let window_secs = window_seconds_for(view.window_minutes);
            let pan = clamp_pan(view.pan_offset_s, &samples, window_secs);
            view.pan_offset_s = pan;
            let t_end_base = samples.last().map(|s| s.t).unwrap_or(0.0);
            let t_start_base = t_end_base - window_secs;
            let (t_start, t_end) = (t_start_base - pan, t_end_base - pan);

            ui.add_space(8.0);
            ui.label(RichText::new(WINDOW_TITLE).strong().color(CYAN));
            // The dim subtitle: the live window + the sample count
            // (the pan offset when it is non-zero).
            let mut subtitle = format!(
                "last {} min · {} samples",
                view.window_minutes,
                samples.len()
            );
            if pan > 0.0 {
                subtitle.push_str(&format!(" · −{pan:.0} s"));
            }
            ui.label(RichText::new(subtitle).color(dim));
            ui.add_space(8.0);
            // The window-resolution row (C7-22 item 7f): four
            // selectable buttons (selecting one resets the pan to
            // 0) + the `Poll` combo (C9-03, D-2: the poll-interval
            // control over the shared settings knob) + the dim pan
            // hint, right-aligned.
            ui.horizontal(|ui| {
                for minutes in WINDOW_MINUTES {
                    if ui
                        .add(
                            egui::Button::new(format!("{minutes} min"))
                                .selected(view.window_minutes == minutes),
                        )
                        .clicked()
                    {
                        view.window_minutes = minutes;
                        view.pan_offset_s = 0.0;
                    }
                }
                // The poll-interval control (C9-03, D-2): the
                // shared `GuiSettings.poll_interval_ms` knob (the
                // settings panel's drag value, C6-27 — the two
                // always agree) — the poller re-reads it live per
                // tick, so a selection takes effect on the next
                // tick without a restart. A preset pick writes the
                // knob; a non-preset stored value shows its exact
                // millisecond figure (the presets stay selectable).
                let _ = egui::ComboBox::from_label("Poll")
                    .selected_text(poll_interval_display(*poll_interval_ms))
                    .show_ui(ui, |ui| {
                        for (ms, label) in POLL_INTERVAL_PRESETS {
                            ui.selectable_value(poll_interval_ms, ms, label);
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(PAN_HINT).color(dim));
                });
            });
            ui.add_space(8.0);

            // The five series rows in fixed order (C7-20) + the
            // interaction (C7-22): a drag on any row pans the
            // window, a hover over any row raises the crosshair.
            let mut row_rects: Vec<Rect> = Vec::new();
            let mut hover_pos: Option<Pos2> = None;
            let mut add_row = |ui: &mut egui::Ui,
                               field: &dyn Fn(&GraphSample) -> f64,
                               label: &str,
                               color: Color32,
                               data: &[GraphSample],
                               note_in_label: bool,
                               window: (f64, f64)| {
                let resp = graph_row(ui, data, field, label, color, window, note_in_label);
                row_rects.push(resp.rect);
                if let Some(pos) = resp.hover_pos() {
                    hover_pos = Some(pos);
                }
                if resp.dragged_by(PointerButton::Primary) {
                    let width = resp.rect.width().max(1.0);
                    view.pan_offset_s += f64::from(resp.drag_delta().x) * (window_secs / f64::from(width));
                }
            };
            add_row(ui, &|s| s.cpu_freq_mhz, "CPU FREQ (MHz)", AMBER, &samples, false, (t_start, t_end));
            ui.add_space(2.0);
            // The permanent no-source row (D-4): the label carries
            // the bare `N/A` note (item 7d); the empty slice
            // keeps it data-driven (it self-populates if a source
            // ever appears, no layout change).
            add_row(ui, &|_s| f64::NAN, "VDDCR_CPU — N/A", AMBER, &[], true, (t_start, t_end));
            ui.add_space(2.0);
            add_row(ui, &|s| s.vddcr_soc_mv, "VDDCR_SOC (mV)", AMBER, &samples, false, (t_start, t_end));
            ui.add_space(2.0);
            add_row(ui, &|s| s.cpu_temp_c, "CPU TEMP (°C)", CYAN, &samples, false, (t_start, t_end));
            ui.add_space(2.0);
            add_row(ui, &|s| s.bandwidth_gbps, "MEM BANDWIDTH (GB/s)", CYAN, &samples, false, (t_start, t_end));
            // Clamp the accumulated pan back into the data (never a
            // negative or past-the-data allocation), then persist
            // the (possibly dragged / re-resolved) view.
            view.pan_offset_s = clamp_pan(view.pan_offset_s, &samples, window_secs);
            store_view(ctx, view);

            // The interactive crosshair (C7-22 item 7e): on hover
            // over any row, a full-height (all rows) vertical line
            // at the hovered x + a floating tooltip with the exact
            // value of every series at the hovered timestamp (the
            // t of the nearest sample). No hover → no crosshair.
            let (Some(pos), Some(row)) = (hover_pos, row_rects.first()) else {
                return;
            };
            let Some(t) = hover_timestamp(
                &samples,
                (t_start_base, t_end_base),
                pan,
                pos.x - row.left(),
                row.width(),
            ) else {
                return;
            };
            let top = row_rects.iter().map(|r| r.top()).fold(f32::INFINITY, f32::min);
            let bottom = row_rects.iter().map(|r| r.bottom()).fold(f32::NEG_INFINITY, f32::max);
            if bottom > top {
                ui.painter().line_segment(
                    [Pos2::new(pos.x, top), Pos2::new(pos.x, bottom)],
                    Stroke::new(1.0_f32, ui.visuals().strong_text_color()),
                );
            }
            let lines = tooltip_lines(&samples, t);
            if !lines.is_empty() {
                // The floating tooltip (the `egui::Area` idiom):
                // offset from the pointer, flipped / clamped into
                // the screen, above everything (Foreground).
                let screen = ctx.screen_rect();
                let est_w = lines
                    .iter()
                    .map(|l| l.chars().count() as f32)
                    .fold(0.0, f32::max)
                    * 7.5
                    + 16.0;
                let est_h = lines.len() as f32 * 18.0 + 12.0;
                let mut x = pos.x + 12.0;
                if x + est_w > screen.right() {
                    x = (pos.x - 12.0 - est_w).max(screen.left());
                }
                let y = (pos.y + 12.0).min(screen.bottom() - est_h).max(screen.top());
                Area::new(Id::new("ramsleuth_graph_tooltip"))
                    .order(Order::Foreground)
                    .fixed_pos(Pos2::new(x, y))
                    .show(ctx, |ui| {
                        let frame = egui::Frame::default()
                            .fill(TOOLTIP_BG)
                            .inner_margin(egui::Margin::same(6.0))
                            .rounding(3.0);
                        frame.show(ui, |ui| {
                            for (i, line) in lines.iter().enumerate() {
                                if i > 0 {
                                    ui.add_space(2.0);
                                }
                                let color = if i == 0 {
                                    dim
                                } else {
                                    ui.visuals().strong_text_color()
                                };
                                ui.label(RichText::new(line).monospace().size(12.0).color(color));
                            }
                        });
                    });
            }
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
                vcore_mv: 1150,
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
                smu_version: Section::na(NaReason::NotApplicable),
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

    /// (e) The ordered scan never panics; a reading is either the
    /// honest NaN (no hwmon sensor + no `cpu_thermal` zone) or a
    /// finite value in a sane temperature range (C9-04: the 5950X
    /// host lands on the `k10temp` hwmon source).
    #[test]
    fn read_cpu_temp_c_never_panics_and_degrades_to_nan() {
        let temp = read_cpu_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    // ------------------------------------------------------------------
    // read_hwmon_temp_c — the C9-04 hwmon source (D-3) + the pure
    // name selection.
    // ------------------------------------------------------------------

    /// (e2) The hwmon scan never panics: a reading is either the
    /// honest NaN (no `k10temp` / `zenpower` device — a host without
    /// the AMD sensor) or a finite value in a sane temperature range
    /// (the 5950X host's `k10temp` `temp1_input`).
    #[test]
    fn read_hwmon_temp_c_never_panics_and_degrades_to_nan() {
        let temp = read_hwmon_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    /// (e3) The name selection prefers `k10temp` over `zenpower`,
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

    /// (e4) The millidegree parse (shared by the zone + hwmon
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

    /// (e5) Every source reader degrades to NaN on a missing source
    /// (no `temp1_input` on the hwmon device, no `temp` on the zone
    /// — the no-source classes never panic).
    #[test]
    fn temp_source_readers_degrade_to_nan_on_missing_sources() {
        let missing = Path::new("/nonexistent/ramsleuth_c9_04");
        assert!(read_hwmon_temp1_c(missing).is_nan(), "a missing hwmon device: NaN");
        assert!(read_zone_temp_c(missing).is_nan(), "a missing thermal zone: NaN");
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
        // The shared poll-interval knob (C9-03): the render's one
        // permitted write (the `Poll` combo, D-2) — the default
        // cadence.
        let mut iv = 2000u64;
        // Empty: five no-source rows (the window is collapsed).
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &GraphState::default(), &mut iv);
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
            render_graphs_window(ctx, &full, &mut iv);
        });

        // Sparse: only a finite clock (the other three series are
        // their no-source notes; VDDCR_CPU is permanent).
        let mut sparse = GraphState::default();
        sparse.samples.push(sample(1000.0, 3600.0, f64::NAN, f64::NAN, f64::NAN));
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &sparse, &mut iv);
        });

        // A single sample: the dot path (one in-window point per
        // finite series).
        let mut one = GraphState::default();
        one.samples.push(sample(1000.0, 3600.0, 1150.0, 45.0, 26.35));
        run_headless_frame(|ctx| {
            render_graphs_window(ctx, &one, &mut iv);
        });
    }

    /// (r) The no-source note renders bare `N/A` (D-4 — the
    /// parenthetical is stripped): the permanent `VDDCR_CPU` row
    /// label reads `VDDCR_CPU — N/A`, each source-less data row
    /// paints the bare note top-right, and no painted text carries
    /// a `no source` parenthetical — in the empty state (all four
    /// data rows no-source) and the populated state (the permanent
    /// label + the self-cleared notes).
    #[test]
    fn render_graphs_window_no_source_note_renders_bare_na() {
        // The shared poll-interval knob (C9-03): the render's one
        // permitted write (the `Poll` combo, D-2) — the default
        // cadence.
        let mut iv = 2000u64;
        // The empty state: every data row its bare note + the
        // permanent VDDCR_CPU label (the note lives in the label).
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &GraphState::default(), &mut iv);
        let out = ctx.end_frame();
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"VDDCR_CPU — N/A"),
            "the permanent row label is bare N/A, got {texts:?}"
        );
        assert!(
            texts.iter().filter(|t| **t == "N/A").count() == 4,
            "each source-less data row shows the bare note, got {texts:?}"
        );
        assert!(
            texts.iter().all(|t| !t.contains("no source")),
            "no painted text carries the parenthetical, got {texts:?}"
        );

        // The populated state: the four data rows their newest-value
        // readouts (the notes self-cleared) + the permanent label.
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
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"VDDCR_CPU — N/A"),
            "the permanent row label stays bare N/A, got {texts:?}"
        );
        assert!(
            !texts.contains(&"N/A"),
            "no note left once data appears (self-cleared), got {texts:?}"
        );
        assert!(
            texts.iter().all(|t| !t.contains("no source")),
            "no painted text carries the parenthetical, got {texts:?}"
        );
    }

    // ------------------------------------------------------------------
    // C7-22 — window_seconds_for (the window-length selection).
    // ------------------------------------------------------------------

    /// (k) The four supported resolutions map to their seconds; an
    /// unsupported value falls back to the default (5 min = 300 s).
    #[test]
    fn window_seconds_for_selects_the_supported_resolutions() {
        assert_eq!(window_seconds_for(1), 60.0);
        assert_eq!(window_seconds_for(5), 300.0);
        assert_eq!(window_seconds_for(15), 900.0);
        assert_eq!(window_seconds_for(60), 3600.0);
        assert_eq!(window_seconds_for(7), 300.0, "an unsupported value falls back to the default");
        assert_eq!(window_seconds_for(0), 300.0);
        assert_eq!(window_seconds_for(u32::MAX), 300.0);
    }

    // ------------------------------------------------------------------
    // C7-22 — clamp_pan (the pan clamping).
    // ------------------------------------------------------------------

    /// (l) The pan stays inside the data: a negative pan clamps to
    /// 0, an in-range pan is kept, a past-the-data pan clamps to
    /// span − window, and there is nothing to pan when the data
    /// fits the window (or is empty / degenerate / non-finite).
    #[test]
    fn clamp_pan_stays_inside_the_data() {
        // A 10-min span (600 s) against a 5-min window: max pan 300.
        let ten = vec![sample(1000.0, 1.0, 1.0, 1.0, 1.0), sample(1600.0, 2.0, 2.0, 2.0, 2.0)];
        assert_eq!(clamp_pan(-50.0, &ten, 300.0), 0.0, "a negative pan clamps to 0");
        assert_eq!(clamp_pan(100.0, &ten, 300.0), 100.0, "an in-range pan is kept");
        assert_eq!(
            clamp_pan(9999.0, &ten, 300.0),
            300.0,
            "a past-the-data pan clamps to span − window"
        );
        // The span fits the window: nothing to pan.
        let two = vec![sample(1000.0, 1.0, 1.0, 1.0, 1.0), sample(1240.0, 2.0, 2.0, 2.0, 2.0)];
        assert_eq!(clamp_pan(999.0, &two, 300.0), 0.0, "a span that fits the window never pans");
        // No data / one sample: no pan.
        assert_eq!(clamp_pan(50.0, &[], 300.0), 0.0, "no samples never pan");
        assert_eq!(
            clamp_pan(50.0, &[sample(1000.0, 1.0, 1.0, 1.0, 1.0)], 300.0),
            0.0,
            "one sample never pans"
        );
        // A non-finite pan degrades to 0 (no panic).
        assert_eq!(clamp_pan(f64::NAN, &ten, 300.0), 0.0, "a NaN pan degrades to 0");
        assert_eq!(clamp_pan(f64::INFINITY, &ten, 300.0), 0.0, "an infinite pan degrades to 0");
    }

    // ------------------------------------------------------------------
    // C7-22 — hover_timestamp (the crosshair nearest-sample lookup).
    // ------------------------------------------------------------------

    /// (m) The hovered x maps across the (panned) window and snaps
    /// to the t of the nearest sample: the window edges, an exact
    /// hit, the clamped out-of-row x, the pan shift, and the
    /// degenerate inputs (no samples → None, zero width → None,
    /// collapsed window → None, NaN pan → None).
    #[test]
    fn hover_timestamp_maps_the_window_and_snaps_to_the_nearest_sample() {
        let samples = vec![
            sample(1010.0, 1.0, 1.0, 1.0, 1.0),
            sample(1050.0, 2.0, 2.0, 2.0, 2.0),
            sample(1090.0, 3.0, 3.0, 3.0, 3.0),
        ];
        let w = (1000.0, 1100.0);
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 0.0, 100.0),
            Some(1010.0),
            "the left edge maps to t = 1000 → the nearest sample"
        );
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 50.0, 100.0),
            Some(1050.0),
            "the mid maps exactly to the 1050 sample"
        );
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 90.0, 100.0),
            Some(1090.0),
            "t = 1090 → the 1090 sample"
        );
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 100.0, 100.0),
            Some(1090.0),
            "the right edge maps to t = 1100 → the nearest sample"
        );
        // The clamped x: a hover outside the row snaps to the edge.
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, -25.0, 100.0),
            Some(1010.0),
            "x < 0 clamps to the left edge"
        );
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 250.0, 100.0),
            Some(1090.0),
            "x > width clamps to the right edge"
        );
        // The pan shifts the window back: the same x maps earlier.
        // [1000, 1100] − 100 → [900, 1000]: x = 100 → t = 1000 → 1010.
        assert_eq!(
            hover_timestamp(&samples, w, 100.0, 100.0, 100.0),
            Some(1010.0),
            "the panned window shifts the mapping back"
        );
        // Degenerate inputs: no panic, None.
        assert_eq!(hover_timestamp(&[], w, 0.0, 50.0, 100.0), None, "no samples → no crosshair");
        assert_eq!(
            hover_timestamp(&samples, w, 0.0, 50.0, 0.0),
            None,
            "a zero-width row → no crosshair"
        );
        assert_eq!(
            hover_timestamp(&samples, (1000.0, 1000.0), 0.0, 50.0, 100.0),
            None,
            "a collapsed window → no crosshair"
        );
        assert_eq!(
            hover_timestamp(&samples, w, f64::NAN, 50.0, 100.0),
            None,
            "a NaN pan → no crosshair (no panic)"
        );
    }

    // ------------------------------------------------------------------
    // C7-22 — tooltip_lines (the crosshair exact values).
    // ------------------------------------------------------------------

    /// (n) The tooltip lists the value of every series at the hover
    /// timestamp in the plan style
    /// (`1800 MHz · 1150 mV · 47.3 °C · 26.35 GB/s`) + the
    /// `HH:MM:SS` timestamp: a fully populated sample, a partial
    /// sample (missing fields as `N/A` — D-4), and the degenerate
    /// inputs (no samples / a NaN t → no lines, no panic).
    #[test]
    fn tooltip_lines_list_every_series_at_the_hover_timestamp() {
        let samples = vec![
            sample(1_700_000_000.0, 3600.0, 1150.0, 47.3, 26.35),
            sample(1_700_000_060.0, 3700.0, 1160.0, 48.0, 26.4),
        ];
        // The first sample (t = 1_700_000_000 → 22:13:20).
        let lines = tooltip_lines(&samples, 1_700_000_000.0);
        assert_eq!(lines.len(), 2, "the timestamp line + the values line");
        assert_eq!(lines[0], "22:13:20");
        assert_eq!(lines[1], "3600 MHz · 1150 mV · 47.3 °C · 26.35 GB/s");
        // The nearest sample wins (t + 31 → the second, 29 s away).
        let lines = tooltip_lines(&samples, 1_700_000_031.0);
        assert_eq!(lines[0], "22:14:20");
        assert_eq!(lines[1], "3700 MHz · 1160 mV · 48.0 °C · 26.40 GB/s");
        // A partial sample: the missing fields degrade to N/A (D-4),
        // the present one keeps its value.
        let partial = vec![sample(1_700_000_000.0, 3600.0, f64::NAN, f64::NAN, f64::NAN)];
        let lines = tooltip_lines(&partial, 1_700_000_000.0);
        assert_eq!(lines[1], "3600 MHz · N/A mV · N/A °C · N/A GB/s");
        // Degenerate inputs: no lines, no panic.
        assert!(tooltip_lines(&[], 1.0).is_empty());
        assert!(tooltip_lines(&samples, f64::NAN).is_empty());
    }

    // ------------------------------------------------------------------
    // C7-22 — the interactive headless render (the synthetic pointer
    // injection — the `render_history` headless pattern extended).
    // ------------------------------------------------------------------

    /// One `RawInput` carrying the given events (+ an optional
    /// screen rect — `None` keeps the default 10000×10000).
    fn pointer_input(events: Vec<egui::Event>, screen: Option<Rect>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: screen,
            events,
            ..Default::default()
        }
    }

    /// A `PointerButton` press / release event at `pos`.
    fn button_event(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The rect of the first series row, from the painted row
    /// backgrounds (`ROW_BG` fills — the rows are the only shapes
    /// painted with that color).
    fn first_row_rect(out: &egui::FullOutput) -> Option<Rect> {
        out.shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == ROW_BG => Some(r.rect),
                _ => None,
            })
    }

    /// Whether the output carries the full-height crosshair line at
    /// x: a vertical line segment (both points at x, a span well
    /// above the single-row height — the line crosses all five
    /// rows).
    fn has_crosshair_line(out: &egui::FullOutput, x: f32) -> bool {
        out.shapes.iter().any(|cs| match &cs.shape {
            egui::Shape::LineSegment {
                points: [p0, p1], ..
            } => {
                (p0.x - x).abs() < 0.5 && (p1.x - x).abs() < 0.5 && (p1.y - p0.y).abs() > 100.0
            }
            _ => false,
        })
    }

    /// The painted text lines (the tooltip assertion surface).
    fn painted_texts(out: &egui::FullOutput) -> Vec<&str> {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect()
    }

    /// (o) The interactive render with a synthetic hover: a hover
    /// over a row raises the full-height crosshair line at the
    /// pointer x + the floating tooltip (the `HH:MM:SS` + the values
    /// lines — egui areas paint from the frame after they first
    /// appear, so the tooltip is asserted on the second hover
    /// frame); the empty / partial states never panic, and the
    /// crosshair drops again on a pointer-gone frame.
    #[test]
    fn render_graphs_window_hover_raises_the_crosshair_without_panicking() {
        // The shared poll-interval knob (C9-03): the render's one
        // permitted write (the `Poll` combo, D-2) — the default
        // cadence.
        let mut iv = 2000u64;
        // 20 samples, 10 s apart (t = 1000..1190): all inside the
        // default 5-min window [890, 1190].
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

        let ctx = egui::Context::default();
        // Frame 1: no pointer — lay out the window + the row rect.
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &GraphState::default(), &mut iv);
        let out = ctx.end_frame();
        let row = first_row_rect(&out).expect("the first row background was painted");
        let hover = Pos2::new(row.center().x, row.center().y);

        // The empty state + hover: no panic, no crosshair (no
        // samples to snap to).
        ctx.begin_frame(pointer_input(vec![egui::Event::PointerMoved(hover)], None));
        render_graphs_window(&ctx, &GraphState::default(), &mut iv);
        let out = ctx.end_frame();
        assert!(!has_crosshair_line(&out, hover.x), "no samples → no crosshair");

        // The full state + hover: the crosshair at the pointer x.
        // The tooltip area is new this frame — not painted yet.
        ctx.begin_frame(pointer_input(vec![egui::Event::PointerMoved(hover)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        assert!(has_crosshair_line(&out, hover.x), "the crosshair line paints at the hover x");

        // The pointer stays: the tooltip now paints (the mid-row
        // hover maps to t = 1040 = the exact 5th sample: 3640 MHz ·
        // 1154 mV · 45.4 °C · 26.35 GB/s @ 00:17:20).
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        assert!(has_crosshair_line(&out, hover.x), "the crosshair persists while hovering");
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"00:17:20"),
            "the tooltip shows the HH:MM:SS timestamp, got {texts:?}"
        );
        assert!(
            texts.contains(&"3640 MHz · 1154 mV · 45.4 °C · 26.35 GB/s"),
            "the tooltip lists every series at that timestamp, got {texts:?}"
        );

        // The pointer goes: the crosshair drops again.
        ctx.begin_frame(pointer_input(vec![egui::Event::PointerGone], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        assert!(!has_crosshair_line(&out, hover.x), "no hover → no crosshair");

        // The partial state (one sample with only a finite clock):
        // the crosshair + the tooltip with the N/A fields (D-4) —
        // the tooltip area reappears fresh, so two hover frames
        // again.
        let mut partial = GraphState::default();
        partial.samples.push(sample(1000.0, 3600.0, f64::NAN, f64::NAN, f64::NAN));
        ctx.begin_frame(pointer_input(vec![egui::Event::PointerMoved(hover)], None));
        render_graphs_window(&ctx, &partial, &mut iv);
        let _out = ctx.end_frame();
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &partial, &mut iv);
        let out = ctx.end_frame();
        assert!(has_crosshair_line(&out, hover.x), "a partial sample still raises the crosshair");
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"3600 MHz · N/A mV · N/A °C · N/A GB/s"),
            "the tooltip degrades the missing fields to N/A, got {texts:?}"
        );
    }

    /// (p) The horizontal pan with a synthetic drag: a press on a
    /// row + a rightward drag pans the window back (the view state
    /// in the IdTypeMap of the child context), clamped to the data
    /// (span − window); the pan persists across the release + the
    /// following pointer-less frames.
    #[test]
    fn render_graphs_window_drag_pans_the_window_and_clamps_to_the_data() {
        // The shared poll-interval knob (C9-03): the render's one
        // permitted write (the `Poll` combo, D-2) — the default
        // cadence.
        let mut iv = 2000u64;
        // 36 samples, 10 s apart (t = 1000..1350): span 350 s >
        // the default 300 s window → max pan = 50 s.
        let mut history = GraphState::default();
        for i in 0..36 {
            history.samples.push(sample(
                1000.0 + 10.0 * f64::from(i),
                3600.0 + f64::from(i),
                1150.0,
                45.0,
                26.35,
            ));
        }
        // A 400×300 screen (a manageable row width for the drag).
        let screen = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));

        let ctx = egui::Context::default();
        ctx.begin_frame(pointer_input(vec![], screen));
        render_graphs_window(&ctx, &history, &mut iv);
        let out = ctx.end_frame();
        let row = first_row_rect(&out).expect("the first row background was painted");

        // Press the primary button on the left edge of the row …
        let press = Pos2::new(row.left() + 1.0, row.center().y);
        ctx.begin_frame(pointer_input(vec![button_event(press, true)], screen));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        // … then drag the pointer to the center of the row.
        let drag_to = Pos2::new(row.center().x, row.center().y);
        ctx.begin_frame(pointer_input(vec![egui::Event::PointerMoved(drag_to)], screen));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        // The pan = the drag × (window / width): far beyond the max
        // (50 s) → clamped to exactly span − window.
        let view = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(view_id(), GraphView::default));
        assert_eq!(view.pan_offset_s, 50.0, "the pan clamps to span − window, got {view:?}");
        assert_eq!(view.window_minutes, 5, "the window resolution is untouched by the pan");

        // Release + a pointer-less frame: the pan persists.
        ctx.begin_frame(pointer_input(vec![button_event(drag_to, false)], screen));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        ctx.begin_frame(pointer_input(vec![], screen));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        let view = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(view_id(), GraphView::default));
        assert_eq!(view.pan_offset_s, 50.0, "the pan persists after the release, got {view:?}");
    }

    /// (q) The window-resolution buttons: clicking `60 min`
    /// re-selects the window and resets the pan to 0 (the plan
    /// selection rule) — over the view state in the IdTypeMap of
    /// the child context (a pre-set in-range pan is cleared by the
    /// click).
    #[test]
    fn render_graphs_window_button_reselects_the_window_and_resets_the_pan() {
        // The shared poll-interval knob (C9-03): the render's one
        // permitted write (the `Poll` combo, D-2) — the default
        // cadence.
        let mut iv = 2000u64;
        // 36 samples, 10 s apart (t = 1000..1350): span 350 s → an
        // in-range pan of 37 s against the 5-min window.
        let mut history = GraphState::default();
        for i in 0..36 {
            history.samples.push(sample(
                1000.0 + 10.0 * f64::from(i),
                3600.0 + f64::from(i),
                1150.0,
                45.0,
                26.35,
            ));
        }
        let ctx = egui::Context::default();
        // Pre-set the view: 5 min + an in-range pan of 37 s.
        ctx.data_mut(|d| {
            *d.get_temp_mut_or_insert_with(view_id(), GraphView::default) = GraphView {
                window_minutes: 5,
                pan_offset_s: 37.0,
            }
        });

        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &history, &mut iv);
        let out = ctx.end_frame();
        // Find the `60 min` button by its label (the only thing
        // painted with that exact text) and click its center.
        let click = out
            .shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == "60 min" => Some(Pos2::new(
                    t.pos.x + t.galley.size().x / 2.0,
                    t.pos.y + t.galley.size().y / 2.0,
                )),
                _ => None,
            })
            .expect("the `60 min` button label was painted");

        // Press …
        ctx.begin_frame(pointer_input(vec![button_event(click, true)], None));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        // … then release on the button: clicked → re-select +
        // reset the pan.
        ctx.begin_frame(pointer_input(vec![button_event(click, false)], None));
        render_graphs_window(&ctx, &history, &mut iv);
        let _out = ctx.end_frame();
        let view = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(view_id(), GraphView::default));
        assert_eq!(
            view,
            GraphView {
                window_minutes: 60,
                pan_offset_s: 0.0
            },
            "selecting the window re-selects it and resets the pan"
        );
    }

    // ------------------------------------------------------------------
    // C9-03 — the Poll combo (the poll-interval control, D-2).
    // ------------------------------------------------------------------

    /// The click point of a painted text shape (its galley center —
    /// the `TextShape` `pos` is the top-left of the laid-out text).
    fn text_click_pos(out: &egui::FullOutput, text: &str) -> Option<Pos2> {
        out.shapes
            .iter()
            .find_map(|cs| match &cs.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(Pos2::new(
                    t.pos.x + t.galley.size().x / 2.0,
                    t.pos.y + t.galley.size().y / 2.0,
                )),
                _ => None,
            })
    }

    /// (s) The `Poll` combo (C9-03, D-2): the resolution row carries
    /// the poll-interval control; a preset selection writes the
    /// shared knob (`*poll_interval_ms`); a non-preset stored value
    /// shows its exact millisecond figure (the `custom` display)
    /// while the presets stay selectable — the settings panel's
    /// drag value and the combo always agree on the same knob.
    #[test]
    fn render_graphs_window_poll_interval_combo_writes_the_shared_knob() {
        // A few samples so the window renders its full layout (the
        // combo sits in the resolution row regardless of the data).
        let mut full = GraphState::default();
        for i in 0..10 {
            full.samples.push(sample(
                1000.0 + 10.0 * f64::from(i),
                3600.0,
                1150.0,
                45.0,
                26.35,
            ));
        }

        let ctx = egui::Context::default();
        // The shared poll-interval knob (the settings panel's drag
        // value, C6-27) — the default cadence.
        let mut iv = 2000u64;

        // Frame 1: the row carries the combo — the button shows the
        // preset label for the stored 2000 ms (`2 s`) + the `Poll`
        // label.
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"Poll"),
            "the `Poll` label paints in the row, got {texts:?}"
        );
        assert!(
            texts.contains(&"2 s"),
            "the stored 2000 ms shows its preset label, got {texts:?}"
        );
        let click =
            text_click_pos(&out, "2 s").expect("the combo button painted its selected text");

        // Frames 2-3: press + release on the button → the popup
        // opens (the combo toggles on the click).
        ctx.begin_frame(pointer_input(vec![button_event(click, true)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        ctx.begin_frame(pointer_input(vec![button_event(click, false)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();

        // Frame 4: the popup paints (egui areas paint from the frame
        // after they first appear — the tooltip precedent): all five
        // presets are selectable.
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        let texts = painted_texts(&out);
        for label in ["0.5 s", "1 s", "2 s", "5 s", "10 s"] {
            assert!(
                texts.contains(&label),
                "the popup lists the preset `{label}`, got {texts:?}"
            );
        }
        let pick = text_click_pos(&out, "5 s").expect("the popup painted the `5 s` preset");

        // Frames 5-6: press + release on `5 s` → the knob writes
        // 5000 (the shared-knob ruling: the settings panel's drag
        // value and the combo always agree).
        ctx.begin_frame(pointer_input(vec![button_event(pick, true)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        ctx.begin_frame(pointer_input(vec![button_event(pick, false)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        assert_eq!(iv, 5000, "selecting the `5 s` preset writes the shared knob");

        // A non-preset stored value (a hand-edited Settings figure):
        // the button shows the exact millisecond figure (the
        // `custom` display), and a preset pick still writes.
        iv = 750;
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        let texts = painted_texts(&out);
        assert!(
            texts.contains(&"750 ms"),
            "a non-preset value shows its exact figure, got {texts:?}"
        );
        let click = text_click_pos(&out, "750 ms")
            .expect("the combo button painted its custom text");
        ctx.begin_frame(pointer_input(vec![button_event(click, true)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        ctx.begin_frame(pointer_input(vec![button_event(click, false)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        ctx.begin_frame(egui::RawInput::default());
        render_graphs_window(&ctx, &full, &mut iv);
        let out = ctx.end_frame();
        let pick = text_click_pos(&out, "10 s").expect("the popup painted the `10 s` preset");
        ctx.begin_frame(pointer_input(vec![button_event(pick, true)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        ctx.begin_frame(pointer_input(vec![button_event(pick, false)], None));
        render_graphs_window(&ctx, &full, &mut iv);
        let _ = ctx.end_frame();
        assert_eq!(iv, 10000, "a preset pick from a non-preset value writes the knob");
    }

    /// (t) The `Poll` combo's selected text (the pure display rule):
    /// a stored preset shows its label; any other value (a
    /// hand-edited Settings figure) shows its exact millisecond
    /// figure — never a wrong preset.
    #[test]
    fn poll_interval_display_maps_presets_and_degrades_to_the_custom_figure() {
        assert_eq!(poll_interval_display(500), "0.5 s");
        assert_eq!(poll_interval_display(1000), "1 s");
        assert_eq!(poll_interval_display(2000), "2 s");
        assert_eq!(poll_interval_display(5000), "5 s");
        assert_eq!(poll_interval_display(10000), "10 s");
        // Non-preset values (a hand-edited Settings figure — the
        // drag value's 100..=60 000 ms range): the exact figure.
        assert_eq!(poll_interval_display(750), "750 ms");
        assert_eq!(poll_interval_display(60_000), "60000 ms");
        assert_eq!(poll_interval_display(0), "0 ms");
    }
}
