//! P3-23 — the three-zone dashboard renderer (Grand Design §3.1).
//!
//! [`AppState`] is the TUI's presentation state and wraps the wire types
//! **verbatim** (D2 — no duplication): a [`SystemMemoryTelemetry`] snapshot,
//! the benchmark [`BenchState`], the daemon status line, a last-update
//! timestamp, an optional error, the five-series graphs ring
//! ([`GraphState`]), and the in-memory settings knobs
//! ([`TuiSettings`]). [`render`] draws the dense, non-scrolling dark
//! dashboard into one `ratatui::Frame`:
//!
//! - **Zone 1 — live memory controller & subtimings:** every cell of the
//!   AMD (and Intel, if present) readout as `key: value` or `key: N/A
//!   (<reason>)` — clocks/ratios (MCLK/UCLK/FCLK), gear/GDM, primary /
//!   secondary / tertiary + turnaround timings, CAD drive/termination
//!   (Ω), voltages.
//! - **Zone 2 — AIDA-style benchmark engine:** the 4×4 grid (tier rows ×
//!   Read/Write/Copy/Latency columns) with metric values or `N/A`, plus a
//!   progress line (`idle` / `running` / `[i/total] …` / `done`).
//! - **Zone 3 — hardware & SPD telemetry:** per-slot module lines (maker /
//!   part / rank / density / speed + XMP/EXPO profiles), the daemon status
//!   line, and the error line when present.
//!
//! The frozen state shapes also carry the parity surfaces the render
//! chain adds over these zones (TUI-10…16): the 3-line header, the
//! settings strip (`settings.settings_open`), the requirements strip
//! (`settings.requirements_open` — driven by the TUI-08 diagnose
//! presence rule), and the graphs overlay (`settings.graphs_open`,
//! drawn over the zone area from `graph`).
//!
//! The semantic palette (Grand Design §3.2) is exact: values in cyan
//! `#00D4FF`, warnings in amber `#FFB300`, N/A/alarms in crimson
//! `#FF3B30`, background slate `#1E1E24`.
//!
//! **No-panic contract:** absent, degraded, or empty data always renders
//! a placeholder (`N/A`, `idle`, `not connected`); an all-`Na` snapshot
//! or the default [`AppState`] draws without panicking (the tests assert
//! through ratatui's in-memory `TestBackend`).

use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table};
use ratatui::Frame;

use ramsleuth_bench::{BenchmarkGrid, Metric, StreamProgress, Tier};
use ramsleuth_telemetry::amd_readout::{
    CadBus, ClockReadout, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::cpuid::{AmdZen, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
use ramsleuth_telemetry::{SystemMemoryTelemetry, SystemPlatform};

use crate::graphs::GraphState;

// ---------------------------------------------------------------------------
// Semantic palette (Grand Design §3.2, exact values).
// ---------------------------------------------------------------------------

/// Cyan: measured values (primary timings / bandwidth).
const CYAN: Color = Color::Rgb(0x00, 0xD4, 0xFF);
/// Amber: warnings (1:2 desync, out-of-spec voltages, not connected).
const AMBER: Color = Color::Rgb(0xFF, 0xB3, 0x00);
/// Crimson: N/A cells, alarms, errors.
const CRIMSON: Color = Color::Rgb(0xFF, 0x3B, 0x30);
/// Slate: the zone backgrounds.
const SLATE: Color = Color::Rgb(0x1E, 0x1E, 0x24);
/// Dim grey: key labels, subheaders, static text.
const DIM: Color = Color::Rgb(0x8A, 0x8A, 0x96);

// ---------------------------------------------------------------------------
// Presentation state.
// ---------------------------------------------------------------------------

/// The benchmark engine presentation state (zone 2).
///
/// `Default` is the not-yet-run state: idle, no run id, no progress
/// events, no grid, no burn-in — the dashboard renders a full `N/A`
/// grid + an `idle` line for it.
#[derive(Debug, Clone, Default)]
pub struct BenchState {
    /// A run is in flight (progress events are streaming).
    pub running: bool,
    /// The in-flight run's daemon-assigned id (`None` between runs —
    /// the cancel action addresses the run with it).
    pub run_id: Option<u64>,
    /// The streamed progress events (latest last; `cell_index` /
    /// `total_cells` count within the run's cell list).
    pub progress: Vec<StreamProgress>,
    /// The terminal grid (`Some` after a completed run; unmeasured cells
    /// carry `0.0` on the wire and render as `N/A`).
    pub grid: Option<BenchmarkGrid>,
    /// The live burn-in state: a burn-in in flight, the newest tick's
    /// iteration / elapsed, and the newest per-cell values.
    pub burn_in: BurnInState,
}

/// The live burn-in state (the zone-2 burn-in row): a burn-in run in
/// flight, the newest tick's iteration / elapsed, and the newest
/// per-cell values seen this burn-in.
///
/// `latest` accumulates each streamed `BurnInProgress` tick the same
/// way the live grid accumulates progress events (the newest value per
/// cell wins; a non-finite / non-positive reading never counts). The
/// 4×4 table keeps showing the terminal grid
/// ([`BenchState::grid`]) during a run; the burn-in row shows `latest`.
///
/// Hand-written `Default` (the GUI `update.rs` precedent): no run,
/// iteration 0, zero elapsed, and a zero grid — `BenchmarkGrid` derives
/// no `Default`, so the unmeasured cells stay `0.0` (they render `N/A`).
#[derive(Debug, Clone)]
pub struct BurnInState {
    /// A burn-in run is in flight.
    pub running: bool,
    /// The 1-based iteration of the newest tick seen this run (0
    /// before the first tick).
    pub iteration: u32,
    /// The run elapsed, in seconds, from the newest tick (0.0 before
    /// the first tick).
    pub elapsed_secs: f64,
    /// The newest per-cell values seen this burn-in.
    pub latest: BenchmarkGrid,
}

impl Default for BurnInState {
    fn default() -> Self {
        Self {
            running: false,
            iteration: 0,
            elapsed_secs: 0.0,
            // A zero grid: unmeasured cells stay 0.0 (they render N/A).
            latest: BenchmarkGrid {
                read_gbps: [0.0; 4],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            },
        }
    }
}

/// The TUI's in-memory settings knobs (the GUI `update.rs` settings
/// precedent — no persistence; the `[t]` strip + the cycle keys mutate
/// them on the main thread, the poller re-reads them each tick).
///
/// `Default` is the current TUI behavior: the 2 s live poll, refresh
/// on (the TUI-specific inversion of the GUI's off default — the
/// existing live poll is kept), GiB capacity, MHz clocks, all panels
/// closed, and the 5-min graphs window.
#[derive(Debug, Clone)]
pub struct TuiSettings {
    /// The telemetry poll interval in milliseconds (default 2000;
    /// floor 100, max 60 000 — the clamp is applied at the read site).
    pub poll_interval_ms: u64,
    /// Periodic polling is on (`false` freezes the view; `[R]` and the
    /// reconnect baseline fetch still run).
    pub refresh: bool,
    /// Capacity displays in GiB (`false` = GB).
    pub capacity_gib: bool,
    /// Clocks display in MHz (`false` = GHz).
    pub clock_mhz: bool,
    /// The graphs overlay panel is open (`[g]`).
    pub graphs_open: bool,
    /// The settings strip is open (`[t]`).
    pub settings_open: bool,
    /// The requirements strip is open (`[d]` — the renderer keeps it
    /// force-true while diagnose is non-empty, until explicitly
    /// closed).
    pub requirements_open: bool,
    /// The graphs window in minutes (default 5; presets 1 / 5 / 15 /
    /// 60).
    pub graph_window_min: u32,
}

impl Default for TuiSettings {
    fn default() -> Self {
        Self {
            poll_interval_ms: 2_000,
            refresh: true,
            capacity_gib: true,
            clock_mhz: true,
            graphs_open: false,
            settings_open: false,
            requirements_open: false,
            graph_window_min: 5,
        }
    }
}

/// The TUI's presentation state: the reused wire types verbatim (D2),
/// the daemon status line, the graphs ring, and the settings knobs.
///
/// P3-24's main loop builds it from `GetTelemetry` / benchmark frames
/// behind an `Arc<RwLock<_>>` and calls [`render`] each tick; `Default` is
/// the not-yet-connected state (no telemetry, idle bench, empty ring,
/// default settings, no error) and renders pure placeholders.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    /// The latest telemetry snapshot (`None` before the first successful
    /// `GetTelemetry`).
    pub telemetry: Option<SystemMemoryTelemetry>,
    /// The benchmark engine state (zone 2).
    pub bench: BenchState,
    /// The daemon status line (e.g. `up · /run/ramsleuth/ramsleuth.sock`);
    /// empty until the first connection attempt.
    pub daemon_status: String,
    /// When the snapshot was last updated (renders the `…s ago` stamp on
    /// the daemon status line).
    pub last_update: Option<Instant>,
    /// The latest structured error (daemon down, protocol violation);
    /// `None` when healthy.
    pub error: Option<String>,
    /// The five-series graphs ring (1800 samples = 60 min at the 2 s
    /// cadence — the `[g]` overlay's data source).
    pub graph: GraphState,
    /// The in-memory settings knobs (the `[t]` strip + the cycle keys).
    pub settings: TuiSettings,
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

/// Render the three-zone dashboard into `frame` from `state`.
///
/// The screen splits vertically into a three-row header strip (line 1
/// the title + platform tag + daemon status + key legend, line 2 the
/// CPU/platform identity, line 3 the RAM summary slot — TUI-11) and the
/// three zones side by side. Every zone is a
/// titled `Block` on a slate background; content that does not fit is
/// clipped, never wrapped or scrolled. Safe at any terminal size — the
/// all-`Na`, empty-SPD, daemon-down, and default states all render without
/// panicking.
pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Fill(1)])
        .split(area);

    let width = usize::from(outer[0].width);
    frame.render_widget(
        Paragraph::new(vec![
            header_line1(state, width),
            header_line2(state),
            header_line3(state),
        ]),
        outer[0],
    );

    let zones = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(32),
            Constraint::Percentage(28),
        ])
        .split(outer[1]);

    render_zone1(frame, state, zones[0]);
    render_zone2(frame, state, zones[1]);
    render_zone3(frame, state, zones[2]);
}

/// The shared zone decoration: a titled border on a slate background.
fn zone_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title(title)
        .style(Style::default().bg(SLATE))
}

/// Header line 1 (Grand Design §3.1, TUI-10 — the GUI's line-1 mirror):
/// `RamSleuth v<version> [<platform tag>] · daemon: <status> · <key
/// legend>`. The title is bold cyan; the bracketed platform tag
/// ([`platform_tag`]) is dim (the honest bare `Platform` without
/// telemetry); the daemon status keeps the existing color rule (crimson
/// on error, dim otherwise — the empty-status states degrade to `down`
/// on error, `—` without); the full §2.2 key legend is dim and truncated
/// to the `width` budget on entry boundaries ([`key_legend`]).
fn header_line1(state: &AppState, width: usize) -> Line<'static> {
    let title = format!("RamSleuth v{}", env!("CARGO_PKG_VERSION"));
    let tag = platform_tag(state.telemetry.as_ref().map(|t| &t.cpu.vendor));
    let status = if state.daemon_status.is_empty() {
        if state.error.is_some() {
            "down"
        } else {
            "—"
        }
    } else {
        state.daemon_status.as_str()
    };
    let status_color = if state.error.is_some() { CRIMSON } else { DIM };
    // The legend budget: the fixed prefix measured in columns (the em
    // dash is the only non-ASCII cell; `chars()` counts it as one) plus
    // the ` · ` separator that precedes the legend.
    let fixed = title.chars().count()
        + format!(" [{tag}]").chars().count()
        + format!(" · daemon: {status}").chars().count();
    let budget = width.saturating_sub(fixed + 3);
    let legend = key_legend(budget);
    let mut spans = vec![
        Span::styled(title, Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" [{tag}]"), Style::default().fg(DIM)),
        Span::styled(format!(" · daemon: {status}"), Style::default().fg(status_color)),
    ];
    if !legend.is_empty() {
        spans.push(Span::styled(format!(" · {legend}"), Style::default().fg(DIM)));
    }
    Line::from(spans)
}

/// Header line 2 (the GUI `cpu_line_text` mirror, C6-20): `CPU: <brand>
/// <clock> | <motherboard> | BIOS <bios> | <AGESA|SMU>` — the CPUID
/// brand + the platform clock in the selected unit (the `clock_mhz`
/// setting, [`format_clock`]), the DMI motherboard / BIOS cells (each
/// `Na` degrades to its `N/A` text), and the AGESA/SMU provenance
/// fragment ([`age_fragment`] — the true-AGESA-suppresses-SMU
/// precedence, C8-11/D-1). No telemetry degrades the cells to `—`
/// placeholders (the fragment stays the honest `AGESA N/A`) — never a
/// panic.
fn header_line2(state: &AppState) -> Line<'static> {
    let Some(telemetry) = &state.telemetry else {
        return Line::from(Span::styled(
            "CPU: — | — | BIOS — | AGESA N/A",
            Style::default().fg(DIM),
        ));
    };
    let platform = &telemetry.platform;
    let clock = match &platform.cpu_clock_mhz {
        Section::Value(mhz) => format_clock(*mhz, state.settings.clock_mhz),
        Section::Na(_) => "N/A".to_owned(),
    };
    let age = age_fragment(platform);
    let age_style = if age == "AGESA N/A" {
        Style::default().fg(DIM)
    } else {
        Style::default().fg(CYAN)
    };
    let value = Style::default().fg(CYAN);
    let label = Style::default().fg(DIM);
    Line::from(vec![
        Span::styled("CPU: ", label),
        Span::styled(format!("{} {}", telemetry.cpu.brand, clock), value),
        Span::styled(" | ", label),
        Span::styled(cell_text(&platform.motherboard), value),
        Span::styled(" | BIOS ", label),
        Span::styled(cell_text(&platform.bios), value),
        Span::styled(" | ", label),
        Span::styled(age, age_style),
    ])
}

/// Header line 3 (the TUI-11 RAM-summary slot): a dim `RAM: —`
/// placeholder until TUI-11 lands the TUI-local `dimm_summary` /
/// `channel_mode` / `sync_mode` composition (`RAM: <total> (<summary>)
/// <max SPD> | <channel> | <note> | Mode: <sync>`).
fn header_line3(_state: &AppState) -> Line<'static> {
    Line::from(Span::styled("RAM: —", Style::default().fg(DIM)))
}

/// Line 1's platform tag (the GUI's line-1 tag mirror, C6-20): the CPU
/// vendor + a best-effort socket family — the Zen 1–3 map to `AM4`,
/// Zen 4/5 to `AM5`, Intel to the generic `LGA` family (a generation
/// does not identify a socket number unambiguously — mobile and desktop
/// share generations), and no telemetry (or an unrecognized vendor)
/// carries the honest bare `Platform`.
fn platform_tag(vendor: Option<&CpuVendor>) -> String {
    match vendor {
        Some(CpuVendor::Amd(AmdZen::Zen1 | AmdZen::Zen2 | AmdZen::Zen3)) => {
            "AMD AM4 Platform".to_owned()
        }
        Some(CpuVendor::Amd(AmdZen::Zen4 | AmdZen::Zen5)) => "AMD AM5 Platform".to_owned(),
        Some(CpuVendor::Intel(_)) => "Intel LGA Platform".to_owned(),
        _ => "Platform".to_owned(),
    }
}

/// The §2.2 frozen key map (canonical R/S/Q-first order) as the compact
/// dim legend line 1 carries: the longest prefix of entries that fits
/// `budget` columns (`" · "` between entries) — an entry that does not
/// fit is dropped whole, never cut mid-token (the width-truncation
/// rule); a zero budget yields the empty legend.
fn key_legend(budget: usize) -> String {
    const ENTRIES: &[&str] = &[
        "R refresh", "S snapshot", "Q quit", "B bench", "M memory", "X burn-in",
        "C cancel", "E export", "G graphs", "T settings", "D reqs", "P poll",
        "U cap", "K clock", "A auto", "W window",
    ];
    let mut text = String::new();
    for entry in ENTRIES {
        let candidate = if text.is_empty() {
            entry.to_string()
        } else {
            format!("{text} · {entry}")
        };
        if candidate.chars().count() > budget {
            break;
        }
        text = candidate;
    }
    text
}

/// One clock readout (the carried MHz wire value) as display text in
/// the selected clock unit (the GUI `format_clock`/`trim` mirror,
/// C7-11): the MHz arm keeps the value (`3500 MHz`), the GHz arm ÷1000
/// (`3.5 GHz`); a whole number renders without decimals, one decimal
/// otherwise; a non-finite value degrades to the honest `N/A`.
fn format_clock(mhz: f64, clock_mhz: bool) -> String {
    if !mhz.is_finite() {
        return "N/A".to_owned();
    }
    let trim = |v: f64| {
        if (v - v.round()).abs() < 0.05 {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        }
    };
    if clock_mhz {
        format!("{} MHz", trim(mhz))
    } else {
        format!("{} GHz", trim(mhz / 1000.0))
    }
}

/// One `Section<String>` cell as display text (the header's honest
/// degradation rule): the contained value, or `N/A`.
fn cell_text(cell: &Section<String>) -> String {
    match cell {
        Section::Value(v) => v.clone(),
        Section::Na(_) => "N/A".to_owned(),
    }
}

/// Line 2's AGESA/SMU provenance fragment (the GUI `age_fragment`
/// mirror, C8-11/D-1): the header labels the value by where it came
/// from — a true AGESA token (the DMI BIOS string scan) renders
/// `AGESA <v>` and suppresses the SMU value (the D-1 precedence); else
/// a shape-checked `ryzen_smu` firmware version renders under its own
/// `SMU` label (never the reported `AGESA <smu value>` mislabel); else
/// the honest `AGESA N/A`. One value, one true label — no hybrid, never
/// fabricated.
fn age_fragment(platform: &SystemPlatform) -> String {
    match (&platform.agesa, &platform.smu_version) {
        (Section::Value(v), _) => format!("AGESA {v}"),
        (Section::Na(_), Section::Value(v)) => format!("SMU {v}"),
        (Section::Na(_), Section::Na(_)) => "AGESA N/A".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Zone 1 — live memory controller & subtimings.
// ---------------------------------------------------------------------------

/// Zone 1: the AMD (and Intel, if present) timing cells, top-aligned and
/// clipped to the block.
fn render_zone1(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = zone_block("1 · MEMORY CONTROLLER");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(List::new(zone1_items(state.telemetry.as_ref())), inner);
}

/// The zone-1 content: every cell of the AMD (and Intel) readout as
/// `key: value` or `key: N/A (<reason>)` in the canonical dump order.
///
/// A section that degraded whole renders a single `N/A (<reason>)` line;
/// no telemetry at all renders a single crimson placeholder — never a
/// panic, never an omitted section.
fn zone1_items(telemetry: Option<&SystemMemoryTelemetry>) -> Vec<ListItem<'static>> {
    let Some(telemetry) = telemetry else {
        return vec![na_item("N/A (no telemetry)")];
    };
    let mut items = Vec::new();
    match &telemetry.amd {
        Section::Na(reason) => items.push(section_na("AMD", reason)),
        Section::Value(readout) => items.extend(readout_items(
            "AMD",
            &readout.clocks,
            &readout.timings,
            &readout.cad_bus,
            &readout.voltages,
            None,
        )),
    }
    match &telemetry.intel {
        Section::Na(reason) => items.push(section_na("Intel", reason)),
        Section::Value(readout) => {
            if readout.channels.is_empty() {
                items.push(section_na("Intel", &NaReason::NotApplicable));
            } else {
                for channel in &readout.channels {
                    let label = format!("Intel ch {}", channel.index);
                    items.extend(readout_items(
                        &label,
                        &channel.clocks,
                        &channel.timings,
                        &channel.cad_bus,
                        &channel.voltages,
                        Some(&channel.rtl),
                    ));
                }
            }
        }
    }
    items
}

/// The rows of one readout (the AMD branch or an Intel channel): a label
/// subheader, then clocks & ratios, the three timing groups + turnarounds,
/// the CAD bus, the voltages, and (Intel only) the channel-level RTL.
fn readout_items(
    label: &str,
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad_bus: &CadBus,
    voltages: &VoltageSet,
    rtl: Option<&Section<u16>>,
) -> Vec<ListItem<'static>> {
    let mut items = vec![sub_item(label)];

    items.push(sub_item("clocks & ratios"));
    items.push(cell_row("MCLK", &clocks.mclk_mhz, CYAN, fmt_mhz));
    items.push(cell_row("UCLK", &clocks.uclk_mhz, CYAN, fmt_mhz));
    items.push(cell_row("FCLK", &clocks.fclk_mhz, CYAN, fmt_mhz));
    // A 1:2 UCLK:MCLK divide is a desync warning (amber); 1:1 stays cyan.
    let div_color = match &clocks.div_mode {
        Section::Value(DivMode::OneToTwo) => AMBER,
        _ => CYAN,
    };
    items.push(cell_row("UCLK:MCLK", &clocks.div_mode, div_color, fmt_div));
    items.push(cell_row("gear", &clocks.gear_mode, CYAN, fmt_gear));
    items.push(cell_row("GDM", &clocks.gdm, CYAN, fmt_flag));
    items.push(cell_row("PDM", &clocks.pdm, CYAN, fmt_flag));

    let primary: &[(&str, &Section<u16>)] = &[
        ("tCL", &timings.cl),
        ("tRCDWR", &timings.rcwdwr),
        ("tRCDRD", &timings.rcdrd),
        ("tRP", &timings.rp),
    ];
    items.push(sub_item("primary timings"));
    tick_rows(&mut items, primary);

    let secondary: &[(&str, &Section<u16>)] = &[
        ("tRAS", &timings.ras),
        ("tRC", &timings.rc),
        ("tRRDS", &timings.rrds),
        ("tRRLD", &timings.rrld),
        ("tFAW", &timings.faw),
    ];
    items.push(sub_item("secondary timings"));
    tick_rows(&mut items, secondary);

    let tertiary: &[(&str, &Section<u16>)] = &[
        ("tWTRS", &timings.wtrs),
        ("tWTRL", &timings.wtrl),
        ("tWR", &timings.wr),
        ("tRFC1", &timings.rfc1),
        ("tRFC2", &timings.rfc2),
        ("tRFCsb", &timings.rfcsb),
        ("tCWL", &timings.cwl),
        ("tRTP", &timings.rtp),
        ("tRDWR", &timings.rdwr),
        ("tWRRD", &timings.wrrd),
        ("tRDRD(SD)", &timings.rdrd_sd),
        ("tRDRD(CCD)", &timings.rdrd_dd),
        ("tRDRD(SCL)", &timings.rdrd_scl),
        ("tRDRD(SC)", &timings.rdrd_sc),
        ("tWRWR(SD)", &timings.wrwr_sd),
        ("tWRWR(CCD)", &timings.wrwr_dd),
        ("tWRWR(SCL)", &timings.wrwr_scl),
        ("tWRWR(SC)", &timings.wrwr_sc),
    ];
    items.push(sub_item("tertiary & turnarounds"));
    tick_rows(&mut items, tertiary);

    items.push(sub_item("CAD bus (ohms)"));
    items.push(cell_row("proc ODT", &cad_bus.proc_odt, CYAN, fmt_ohms));
    items.push(cell_row("RTT nom", &cad_bus.rtt_nom, CYAN, fmt_rtt));
    items.push(cell_row("RTT wr", &cad_bus.rtt_wr, CYAN, fmt_rtt));
    items.push(cell_row("RTT park", &cad_bus.rtt_park, CYAN, fmt_rtt));
    items.push(cell_row("CLK drive", &cad_bus.clk_drv, CYAN, fmt_ohms));
    items.push(cell_row("ADD/CMD drive", &cad_bus.addr_cmd_drv, CYAN, fmt_ohms));
    items.push(cell_row("CS/ODT drive", &cad_bus.cs_odt_drv, CYAN, fmt_ohms));
    items.push(cell_row("CKE drive", &cad_bus.cke_drv, CYAN, fmt_ohms));

    items.push(sub_item("voltages"));
    // A SOC rail above 1.30 V is out of spec on AM5 -> amber warning.
    let soc_color = match &voltages.vddcr_soc_mv {
        Section::Value(mv) if f64::from(*mv) > 1300.0 => AMBER,
        _ => CYAN,
    };
    items.push(cell_row("VDDCR_SOC", &voltages.vddcr_soc_mv, soc_color, fmt_volts));
    items.push(cell_row("VDDIO_MEM", &voltages.vddio_mem_mv, CYAN, fmt_volts));
    items.push(cell_row("VDD_MISC", &voltages.vdd_misc_mv, CYAN, fmt_volts));
    items.push(cell_row("VPP", &voltages.vpp_mv, CYAN, fmt_volts));

    if let Some(rtl) = rtl {
        items.push(cell_row("RTL", rtl, CYAN, fmt_ticks));
    }
    items
}

/// Append one cyan `key: <ticks>` row per pair (crimson `N/A` when the
/// cell degraded).
fn tick_rows(items: &mut Vec<ListItem<'static>>, pairs: &[(&str, &Section<u16>)]) {
    for (key, section) in pairs {
        items.push(cell_row(key, section, CYAN, fmt_ticks));
    }
}

// ---------------------------------------------------------------------------
// Zone 2 — AIDA-style benchmark engine.
// ---------------------------------------------------------------------------

/// Zone 2: the 4×4 grid (five rows: header + four tiers) + one progress
/// line + slack (clipped, never scrolled).
fn render_zone2(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = zone_block("2 · BENCH (GB/s)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // the grid (header + four tier rows)
            Constraint::Length(1), // the progress line
            Constraint::Fill(1),   // slack
        ])
        .split(inner);
    frame.render_widget(bench_table(state.bench.grid.as_ref()), parts[0]);
    let (text, color) = progress_line(&state.bench);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(&text, Style::default().fg(color)))),
        parts[1],
    );
}

/// The 4×4 grid: tier rows × Read/Write/Copy/Latency columns.
///
/// The cells are bare two-decimal numbers: the Read/Write/Copy columns are
/// GB/s (named in the zone title) and the last column is ns/hop (named in
/// its header), the AIDA64 grid convention at terminal width. An
/// unmeasured cell (`0.0` on the wire — the grid carries no `Na` arm) and
/// a missing grid (no run yet) render `N/A` (crimson); measured cells are
/// cyan.
fn bench_table(grid: Option<&BenchmarkGrid>) -> Table<'static> {
    let header = Row::new(
        ["", "Read", "Write", "Copy", "ns/hop"]
            .iter()
            .map(|name| Cell::from(*name).style(Style::default().fg(DIM))),
    );
    let mut rows = vec![header];
    for &tier in &[Tier::Memory, Tier::L1, Tier::L2, Tier::L3] {
        let mut cells = vec![Cell::from(tier_name(tier)).style(Style::default().fg(CYAN))];
        for &metric in &[Metric::Read, Metric::Write, Metric::Copy, Metric::Latency] {
            let value = grid
                .map(|g| g.cell(tier, metric))
                .filter(|v| *v != 0.0);
            let (text, color) = match value {
                Some(v) => (format!("{v:.2}"), CYAN),
                None => ("N/A".to_owned(), CRIMSON),
            };
            cells.push(Cell::from(text).style(Style::default().fg(color)));
        }
        rows.push(Row::new(cells));
    }
    Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Min(6),
            Constraint::Min(6),
            Constraint::Min(8),
        ],
    )
}

/// The one-line benchmark progress indicator: `[i/total] label value`
/// while a run streams, `running` when one just started, `done` after a
/// completed run, `idle` otherwise.
fn progress_line(bench: &BenchState) -> (String, Color) {
    if bench.running {
        match bench.progress.last() {
            Some(p) => (
                format!(
                    "bench: [{}/{}] {} {value:.2}",
                    p.cell_index + 1,
                    p.total_cells,
                    p.label,
                    value = p.value
                ),
                CYAN,
            ),
            None => ("bench: running...".to_owned(), AMBER),
        }
    } else if bench.grid.is_some() {
        ("bench: done".to_owned(), DIM)
    } else {
        ("bench: idle".to_owned(), DIM)
    }
}

// ---------------------------------------------------------------------------
// Zone 3 — hardware & SPD telemetry.
// ---------------------------------------------------------------------------

/// Zone 3: the per-slot SPD modules, the daemon status line (with the
/// last-update stamp), and the error line when present.
fn render_zone3(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = zone_block("3 · HARDWARE & SPD");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(List::new(zone3_items(state)), inner);
}

/// The zone-3 content: one block per SPD slot, then the daemon status
/// line and (when present) the crimson error line.
fn zone3_items(state: &AppState) -> Vec<ListItem<'static>> {
    let mut items = Vec::new();
    match &state.telemetry {
        None => items.push(na_item("N/A (no telemetry)")),
        Some(telemetry) => {
            if telemetry.spd.is_empty() {
                items.push(section_na("SPD", &NaReason::DriverMissing));
            } else {
                for module in &telemetry.spd {
                    items.extend(spd_module_items(module));
                }
            }
        }
    }

    // Daemon status line: the status + the last-update stamp; amber when
    // not connected, crimson when the latest attempt errored.
    let status = if state.daemon_status.is_empty() {
        "not connected".to_owned()
    } else {
        state.daemon_status.clone()
    };
    let mut text = format!("daemon: {status}");
    if let Some(last) = state.last_update {
        text.push_str(&format!(
            " · {secs:.1}s ago",
            secs = last.elapsed().as_secs_f64()
        ));
    }
    let color = if state.error.is_some() {
        CRIMSON
    } else if state.daemon_status.is_empty() {
        AMBER
    } else {
        DIM
    };
    items.push(text_item(&text, color));

    if let Some(error) = &state.error {
        items.push(text_item(&format!("! {error}"), CRIMSON));
    }
    items
}

/// One SPD slot: a subheader + maker / part / rank / density / speed
/// cells + one line per XMP/EXPO profile (or a `none` placeholder).
fn spd_module_items(module: &SpdModule) -> Vec<ListItem<'static>> {
    let gen = if module.is_ddr5 { "DDR5" } else { "DDR4" };
    let mut items = vec![sub_item(&format!(
        "slot 0x{:02X} ({gen})",
        module.index
    ))];
    items.push(cell_row("maker", &module.maker, CYAN, |s| s.clone()));
    items.push(cell_row("part", &module.part, CYAN, |s| s.clone()));
    items.push(cell_row("rank", &module.rank, CYAN, |v: &u8| v.to_string()));
    items.push(cell_row("density", &module.density_mbit, CYAN, fmt_density));
    items.push(cell_row("speed", &module.speed_mts, CYAN, fmt_mts));
    if module.profiles.is_empty() {
        items.push(row("profiles", "none", DIM));
    } else {
        for profile in &module.profiles {
            items.push(spd_profile_item(module.is_ddr5, profile));
        }
    }
    items
}

/// One profile line: `<XMP|EXPO> <n>: <speed> <cl>-<trcd>-<trp>-<tras> @
/// <voltage>` (an N/A field degrades just that field, never the line).
fn spd_profile_item(is_ddr5: bool, profile: &SpdProfile) -> ListItem<'static> {
    let scheme = if is_ddr5 { "EXPO" } else { "XMP" };
    let speed = match &profile.speed_mts {
        Section::Value(v) => fmt_mts(v),
        Section::Na(reason) => na_text(reason),
    };
    let tick = |s: &Section<u8>| match s {
        Section::Value(v) => v.to_string(),
        Section::Na(reason) => na_text(reason),
    };
    let voltage = match &profile.voltage {
        Section::Value(v) => fmt_volts(v),
        Section::Na(reason) => na_text(reason),
    };
    text_item(
        &format!(
            "  {scheme} {}: {speed} {}-{}-{}-{} @ {voltage}",
            profile.index,
            tick(&profile.cas),
            tick(&profile.trcd),
            tick(&profile.trp),
            tick(&profile.tras)
        ),
        CYAN,
    )
}

// ---------------------------------------------------------------------------
// Cell formatters + line primitives.
// ---------------------------------------------------------------------------

/// One `key: value` row: the dim key, the colored value.
fn row(key: &str, value: &str, color: Color) -> ListItem<'static> {
    let line = Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(DIM)),
        Span::styled(value.to_owned(), Style::default().fg(color)),
    ]);
    ListItem::new(line)
}

/// A formatted cell: the value in `color`, or a crimson `N/A (<reason>)`.
fn cell_row<T>(key: &str, section: &Section<T>, color: Color, fmt: impl Fn(&T) -> String) -> ListItem<'static> {
    match section {
        Section::Value(value) => row(key, &fmt(value), color),
        Section::Na(reason) => row(key, &na_text(reason), CRIMSON),
    }
}

/// One plain colored line.
fn text_item(text: &str, color: Color) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(text.to_owned(), Style::default().fg(color))))
}

/// One dim subheader line.
fn sub_item(title: &str) -> ListItem<'static> {
    text_item(&format!("--- {title} ---"), DIM)
}

/// One crimson N/A placeholder line.
fn na_item(text: &str) -> ListItem<'static> {
    text_item(text, CRIMSON)
}

/// A section that degraded whole: one crimson `N/A (<reason>)` line.
fn section_na(section: &str, reason: &NaReason) -> ListItem<'static> {
    text_item(&format!("{section}: {}", na_text(reason)), CRIMSON)
}

/// The human text of an N/A cell (mirrors the dump renderer's form;
/// [`NaReason`] carries no `Display`).
fn na_text(reason: &NaReason) -> String {
    match reason {
        NaReason::UnsupportedHardware => "N/A (unsupported hardware)".to_owned(),
        NaReason::DriverMissing => "N/A (driver missing)".to_owned(),
        NaReason::InsufficientPrivilege => "N/A (insufficient privilege)".to_owned(),
        NaReason::UnknownPmTableVersion => "N/A (unknown PM table version)".to_owned(),
        NaReason::NotApplicable => "N/A (not applicable)".to_owned(),
        NaReason::ParseError(detail) => format!("N/A (parse error: {detail})"),
    }
}

/// The display name of a grid tier row (`Mem` — the AIDA64 grid
/// convention, which also fits the column budget).
fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Memory => "Mem",
        Tier::L1 => "L1",
        Tier::L2 => "L2",
        Tier::L3 => "L3",
    }
}

/// A memory-clock cell in megahertz (two decimals).
fn fmt_mhz(v: &f64) -> String {
    format!("{v:.2} MHz")
}

/// An ohm cell (one decimal + Ω).
fn fmt_ohms(v: &f64) -> String {
    format!("{v:.1} Ω")
}

/// An RTT cell: disabled / RZQ divisor (with the resolved ohms) / ohms.
fn fmt_rtt(v: &RttValue) -> String {
    match v {
        RttValue::Disabled => "disabled".to_owned(),
        RttValue::Rzq(code) => match RttValue::Rzq(*code).ohms() {
            Some(ohms) => format!("RZQ/{code} ({ohms:.1} Ω)"),
            None => format!("RZQ/{code}"),
        },
        RttValue::Ohms(ohms) => format!("{ohms:.1} Ω"),
    }
}

/// The UCLK:MCLK divide mode: `1:1` / `1:2`.
fn fmt_div(v: &DivMode) -> String {
    match v {
        DivMode::OneToOne => "1:1".to_owned(),
        DivMode::OneToTwo => "1:2".to_owned(),
    }
}

/// The SA:MEM gear multiplier: `1x` / `2x` / `4x`.
fn fmt_gear(v: &GearMode) -> String {
    match v {
        GearMode::One => "1x".to_owned(),
        GearMode::Two => "2x".to_owned(),
        GearMode::Four => "4x".to_owned(),
    }
}

/// A boolean mode flag (GDM / PDM): `on` / `off`.
fn fmt_flag(v: &bool) -> String {
    if *v {
        "on".to_owned()
    } else {
        "off".to_owned()
    }
}

/// A memory-rail cell in volts (the frozen mV value over 1000, three
/// decimals — the plan's mV→V display rule).
fn fmt_volts(v: &u16) -> String {
    format!("{:.3} V", f64::from(*v) / 1000.0)
}

/// A module data-rate cell in megatransfers per second.
fn fmt_mts(v: &u16) -> String {
    format!("{v} MT/s")
}

/// A DRAM die density cell in mebibits.
fn fmt_density(v: &u16) -> String {
    format!("{v} Mbit")
}

/// A tick count (the plan's display unit for DRAM subtimings).
fn fmt_ticks(v: &u16) -> String {
    v.to_string()
}

#[cfg(test)]
mod tests {
    //! Renderer tests on ratatui's in-memory backend (`TestBackend` +
    //! `Terminal`): draw an [`AppState`], read the rendered buffer back,
    //! and assert on its text — no TTY required.

    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use ramsleuth_bench::BenchOp;
    use ramsleuth_telemetry::amd_readout::{AmdReadout, CommandRate};
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, IntelGen};
    use ramsleuth_telemetry::intel_readout::{decode_channel, IntelReadout};
    use ramsleuth_telemetry::SystemPlatform;

    /// Draw `state` into a 100×30 in-memory terminal and return the
    /// rendered buffer as text (one line per row, trailing spaces
    /// trimmed) — the single assertion surface for all the tests.
    fn draw(state: &AppState) -> String {
        draw_at(state, 100, 30)
    }

    /// [`draw`] at an explicit terminal size (the per-channel Intel test
    /// needs a taller surface to reach the second channel block).
    fn draw_at(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render(f, state))
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

    /// A representative AMD readout: `Value` cells across all four sets
    /// with a few `Na` cells (the AMD-not-applicable gear mode, one
    /// out-of-band refresh timing, one not-applicable RTT park).
    fn fixture_amd() -> AmdReadout {
        AmdReadout {
            clocks: ClockReadout {
                mclk_mhz: Section::Value(1600.0),
                uclk_mhz: Section::Value(1600.0),
                fclk_mhz: Section::Value(1800.0),
                div_mode: Section::Value(DivMode::OneToTwo),
                gear_mode: Section::na(NaReason::NotApplicable),
                gdm: Section::Value(true),
                pdm: Section::Value(false),
                command_rate: Section::Value(CommandRate::OneT),
            },
            timings: TimingSet {
                cl: Section::Value(16),
                rcwdwr: Section::Value(16),
                rcdrd: Section::Value(16),
                rp: Section::Value(16),
                ras: Section::Value(34),
                rc: Section::Value(50),
                rrds: Section::Value(4),
                rrld: Section::Value(8),
                faw: Section::Value(16),
                wtrs: Section::Value(4),
                wtrl: Section::Value(12),
                wr: Section::Value(20),
                rfc1: Section::Value(75),
                rfc2: Section::na(NaReason::ParseError("fixture".to_owned())),
                rfcsb: Section::Value(38),
                cwl: Section::Value(12),
                rtp: Section::Value(8),
                rdwr: Section::Value(8),
                wrrd: Section::Value(4),
                rdrd_sd: Section::Value(4),
                rdrd_dd: Section::Value(8),
                rdrd_scl: Section::Value(8),
                rdrd_sc: Section::Value(8),
                wrwr_sd: Section::Value(4),
                wrwr_dd: Section::Value(8),
                wrwr_scl: Section::Value(8),
                wrwr_sc: Section::Value(8),
            },
            cad_bus: CadBus {
                proc_odt: Section::Value(33.0),
                rtt_nom: Section::Value(RttValue::Rzq(10)),
                rtt_wr: Section::Value(RttValue::Ohms(45.0)),
                rtt_park: Section::na(NaReason::NotApplicable),
                clk_drv: Section::Value(48.0),
                addr_cmd_drv: Section::Value(48.0),
                cs_odt_drv: Section::Value(33.0),
                cke_drv: Section::Value(48.0),
            },
            voltages: VoltageSet {
                vddcr_soc_mv: Section::Value(1150),
                vddio_mem_mv: Section::Value(1350),
                vdd_misc_mv: Section::Value(1100),
                vpp_mv: Section::Value(1800),
                vcore_mv: Section::na(NaReason::NotApplicable),
            },
        }
    }

    /// One SPD module with populated cells and one XMP profile.
    fn fixture_module() -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::Value("Samsung".to_owned()),
            die_maker: Section::Value("SK hynix".to_owned()),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::Value(8),
            part: Section::Value("M391A2K40DB".to_owned()),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(2),
            density_mbit: Section::Value(16_384),
            speed_mts: Section::Value(3_200),
            profiles: vec![SpdProfile {
                index: 1,
                speed_mts: Section::Value(3_600),
                cas: Section::Value(18),
                trcd: Section::Value(18),
                trp: Section::Value(18),
                tras: Section::Value(36),
                voltage: Section::Value(1_350),
            }],
        }
    }

    /// A representative state: a `Value` AMD readout, an all-`Na` Intel
    /// branch (an AMD host), one SPD module, the daemon up.
    fn representative() -> AppState {
        AppState {
            telemetry: Some(SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Amd(AmdZen::Zen3),
                    brand: "Ryzen 9 5950X".to_owned(),
                },
                amd: Section::Value(fixture_amd()),
                intel: Section::na(NaReason::UnsupportedHardware),
                spd: vec![fixture_module()],
                platform: SystemPlatform {
                    cpu_clock_mhz: Section::Value(3500.0),
                    motherboard: Section::Value("Test Board".to_owned()),
                    bios: Section::Value("1.0".to_owned()),
                    agesa: Section::na(NaReason::NotApplicable),
                    smu_version: Section::na(NaReason::NotApplicable),
                },
                total_capacity: Section::Value(16.0),
                dimm_sizes: vec![Section::Value(16.0)],
            }),
            daemon_status: "up · /tmp/ramsleuth.sock".to_owned(),
            last_update: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// (a) A representative state renders all three zone titles, at
    /// least one formatted value per zone, and the daemon status line.
    #[test]
    fn representative_state_renders_zones_and_values() {
        let text = draw(&representative());

        // the three zone titles
        assert!(text.contains("1 · MEMORY CONTROLLER"), "{text}");
        assert!(text.contains("2 · BENCH (GB/s)"), "{text}");
        assert!(text.contains("3 · HARDWARE & SPD"), "{text}");
        // zone 1: formatted clock + ratio values (the first rows)
        assert!(text.contains("1600.00 MHz"), "{text}");
        assert!(text.contains("1:2"), "{text}");
        assert!(text.contains("tCL: 16"), "{text}");
        // zone 2: the grid renders (idle, no run yet -> N/A cells)
        assert!(text.contains("bench: idle"), "{text}");
        // zone 3: the SPD module + profile + daemon line
        assert!(text.contains("Samsung"), "{text}");
        assert!(text.contains("3200 MT/s"), "{text}");
        assert!(text.contains("16384 Mbit"), "{text}");
        assert!(text.contains("XMP 1"), "{text}");
        // header line 2 (TUI-10): the CPU/platform identity
        assert!(text.contains("CPU: Ryzen 9 5950X"), "{text}");
        assert!(text.contains("daemon: up"), "{text}");
    }

    /// (b) The default state (all-Na, no telemetry, no run) renders
    /// placeholders without panicking: `N/A` in every zone, the full
    /// `N/A` grid, and the `idle` progress line.
    #[test]
    fn default_state_renders_placeholders_without_panic() {
        let text = draw(&AppState::default());

        // all sixteen grid cells plus the zone-1/zone-3 placeholders
        assert!(
            text.matches("N/A").count() >= 18,
            "expected >= 18 N/A placeholders, got:\n{text}"
        );
        assert!(text.contains("idle"), "{text}");
        assert!(text.contains("not connected"), "{text}");
        assert!(text.contains("no telemetry"), "{text}");
    }

    /// (c) A running bench renders the grid with its measured cells and a
    /// live `[i/total]` progress line.
    #[test]
    fn running_bench_renders_grid_and_progress() {
        let mut state = representative();
        state.bench = BenchState {
            running: true,
            progress: vec![StreamProgress {
                cell_index: 0,
                total_cells: 3,
                tier: Tier::L1,
                op: BenchOp::Read,
                value: 42.5,
                label: "L1 · Read (GB/s)".to_owned(),
            }],
            grid: Some(BenchmarkGrid {
                read_gbps: [0.0, 42.5, 0.0, 0.0],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0, 1.1, 0.0, 0.0],
            }),
            ..Default::default()
        };
        let text = draw(&state);

        assert!(text.contains("bench: [1/3]"), "{text}");
        assert!(text.contains("42.50"), "{text}");
        assert!(text.contains("1.10"), "{text}");
        assert!(text.contains("ns/hop"), "{text}");
        // the unmeasured cells stay N/A (14 grid cells + the gear cell)
        assert!(text.matches("N/A").count() >= 13, "{text}");
    }

    /// (c′) A completed run (not running, grid present) renders the
    /// `done` progress line.
    #[test]
    fn completed_bench_renders_done() {
        let state = AppState {
            bench: BenchState {
                running: false,
                progress: Vec::new(),
                grid: Some(BenchmarkGrid {
                    read_gbps: [26.3, 0.0, 0.0, 0.0],
                    write_gbps: [0.0; 4],
                    copy_gbps: [0.0; 4],
                    latency_ns: [86.8, 0.0, 0.0, 0.0],
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = draw(&state);

        assert!(text.contains("bench: done"), "{text}");
        assert!(text.contains("26.30"), "{text}");
        assert!(text.contains("86.80"), "{text}");
    }

    /// (d) An error state renders the crimson error line and the daemon
    /// down marker (with the all-Na telemetry behind it).
    #[test]
    fn error_state_renders_error_line() {
        let state = AppState {
            error: Some("daemon down: cannot connect to /tmp/x.sock".to_owned()),
            ..Default::default()
        };
        let text = draw(&state);

        assert!(text.contains("daemon down"), "{text}");
        assert!(text.contains("daemon: down"), "{text}");
    }

    /// (e) An Intel-host state renders the per-channel block (the decoded
    /// channel plus the all-Na degradation channel) without panicking.
    #[test]
    fn intel_state_renders_per_channel() {
        let cmd0: u32 = 16 | (16 << 8) | (16 << 16) | (32 << 24);
        let cmd1: u32 = 6 << 4;
        let cmd2: u32 = 4 | (12 << 8);
        let cmd3: u32 = 10 | (8 << 8) | (12 << 16) | (4 << 24);
        let state = AppState {
            telemetry: Some(SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Unknown,
                    brand: "synthetic".to_owned(),
                },
                amd: Section::na(NaReason::UnsupportedHardware),
                intel: Section::Value(IntelReadout {
                    channels: vec![
                        decode_channel(0, Some(160), [
                            Some(cmd0),
                            Some(cmd1),
                            Some(cmd2),
                            Some(cmd3),
                        ]),
                        decode_channel(1, None, [None; 4]),
                    ],
                }),
                spd: Vec::new(),
                platform: SystemPlatform {
                    cpu_clock_mhz: Section::Value(2400.0),
                    motherboard: Section::na(NaReason::NotApplicable),
                    bios: Section::na(NaReason::NotApplicable),
                    agesa: Section::na(NaReason::NotApplicable),
                    smu_version: Section::na(NaReason::NotApplicable),
                },
                total_capacity: Section::na(NaReason::NotApplicable),
                dimm_sizes: Vec::new(),
            }),
            daemon_status: "up".to_owned(),
            ..Default::default()
        };
        // 100×62: the 3-row header leaves a 57-row inner zone surface
        // (channel 0 is 54 rows), just enough to reach the second
        // channel's label; a shorter terminal clips it.
        let text = draw_at(&state, 100, 62);

        assert!(text.contains("Intel ch 0"), "{text}");
        assert!(text.contains("Intel ch 1"), "{text}");
        assert!(text.contains("1600.00 MHz"), "{text}");
        assert!(text.contains("SPD: N/A"), "{text}");
    }

    /// (f) The 3-line header (TUI-10): line 1 the title + platform tag
    /// + daemon status, line 2 the CPU/platform identity (the
    /// representative's Zen 3 / 3500 MHz / Test Board / BIOS 1.0 /
    /// all-Na AGESA+SMU shape), line 3 the RAM summary slot (the
    /// TUI-11 placeholder).
    #[test]
    fn header_is_three_lines_with_values() {
        let text = draw(&representative());
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        assert_eq!(lines.len(), 3, "the header occupies the top three rows");
        let l1 = &lines[0];
        assert!(
            l1.contains(&format!("RamSleuth v{}", env!("CARGO_PKG_VERSION"))),
            "{l1}"
        );
        assert!(l1.contains("[AMD AM4 Platform]"), "{l1}");
        assert!(l1.contains("daemon: up · /tmp/ramsleuth.sock"), "{l1}");
        assert_eq!(
            lines[1],
            "CPU: Ryzen 9 5950X 3500 MHz | Test Board | BIOS 1.0 | AGESA N/A",
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "RAM: —", "{}", lines[2]);
    }

    /// (g) The no-telemetry header degrades to the `—` / `N/A`
    /// placeholders (TUI-10): line 1 the honest bare `Platform` tag +
    /// `daemon: —`, line 2 the full placeholder line, line 3 the RAM
    /// placeholder — never a panic.
    #[test]
    fn header_no_telemetry_placeholders() {
        let text = draw(&AppState::default());
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        let l1 = &lines[0];
        assert!(l1.contains("[Platform]"), "{l1}");
        assert!(l1.contains("daemon: —"), "{l1}");
        assert_eq!(lines[1], "CPU: — | — | BIOS — | AGESA N/A", "{}", lines[1]);
        assert_eq!(lines[2], "RAM: —", "{}", lines[2]);
    }

    /// (h) The AGESA/SMU precedence matrix (the GUI `age_fragment`
    /// mirror, C8-11/D-1): a true AGESA wins and suppresses the SMU
    /// value; else the shape-checked `ryzen_smu` firmware version
    /// renders under its own `SMU` label; else the honest `AGESA N/A`.
    /// No hybrid, never fabricated.
    #[test]
    fn age_fragment_by_provenance() {
        let base = SystemPlatform {
            cpu_clock_mhz: Section::Value(3600.0),
            motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
            bios: Section::Value("5601".to_owned()),
            agesa: Section::na(NaReason::NotApplicable),
            smu_version: Section::na(NaReason::NotApplicable),
        };
        assert_eq!(age_fragment(&base), "AGESA N/A");

        let smu = SystemPlatform {
            smu_version: Section::Value("56.78.0".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&smu), "SMU 56.78.0");

        let agesa = SystemPlatform {
            agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&agesa), "AGESA ComboAm4v2 PI 1.2.0.12");

        // The precedence (D-1): a true AGESA suppresses the SMU value.
        let both = SystemPlatform {
            agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
            smu_version: Section::Value("56.78.0".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&both), "AGESA ComboAm4v2 PI 1.2.0.12");
    }

    /// (i) The platform-tag matrix (the GUI's line-1 tag mirror): Zen
    /// 1–3 → `AM4`, Zen 4/5 → `AM5`, Intel → the generic `LGA` family;
    /// no telemetry (`None`) and the unrecognized vendor carry the
    /// honest bare `Platform`.
    #[test]
    fn platform_tag_matrix() {
        assert_eq!(
            platform_tag(Some(&CpuVendor::Amd(AmdZen::Zen1))),
            "AMD AM4 Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Amd(AmdZen::Zen2))),
            "AMD AM4 Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Amd(AmdZen::Zen3))),
            "AMD AM4 Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Amd(AmdZen::Zen4))),
            "AMD AM5 Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Amd(AmdZen::Zen5))),
            "AMD AM5 Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Intel(IntelGen::Skylake))),
            "Intel LGA Platform"
        );
        assert_eq!(
            platform_tag(Some(&CpuVendor::Intel(IntelGen::Unrecognized))),
            "Intel LGA Platform"
        );
        assert_eq!(platform_tag(Some(&CpuVendor::Unknown)), "Platform");
        assert_eq!(platform_tag(None), "Platform");
    }

    /// (j) The line-2 clock unit knob (the GUI `format_clock`/`trim`
    /// mirror, C7-11): MHz keeps the carried wire value (`3500 MHz`),
    /// GHz ÷1000 (`3.5 GHz`); the trim arms (whole → no decimals,
    /// else one) and the non-finite → `N/A` degradation.
    #[test]
    fn header_line2_clock_unit_knob() {
        let mut state = representative();
        state.settings.clock_mhz = false;
        let text = draw(&state);
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        assert_eq!(
            lines[1],
            "CPU: Ryzen 9 5950X 3.5 GHz | Test Board | BIOS 1.0 | AGESA N/A",
            "{}",
            lines[1]
        );
        assert_eq!(format_clock(3600.0, true), "3600 MHz");
        assert_eq!(format_clock(1800.0, true), "1800 MHz");
        assert_eq!(format_clock(3600.0, false), "3.6 GHz");
        assert_eq!(format_clock(1800.0, false), "1.8 GHz");
        assert_eq!(format_clock(f64::NAN, true), "N/A");
        assert_eq!(format_clock(f64::INFINITY, false), "N/A");
    }

    /// (k) The key legend truncates by width on entry boundaries
    /// (TUI-10): at a wide terminal the full §2.2 map fits (the last
    /// entry `W window` present); at the 100-col test surface a
    /// prefix of it (the entries that no longer fit are dropped
    /// whole, never cut mid-token, and the line stays in budget); at a
    /// narrow surface with a short daemon status only the first entry
    /// survives (the measured truncation points: 250 cols → all 16
    /// entries, 100 cols → `R refresh · S snapshot`, 60 cols →
    /// `R refresh`).
    #[test]
    fn header_legend_truncated_by_width() {
        // Wide (250 cols): the full legend fits.
        let text = draw_at(&representative(), 250, 30);
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        assert!(lines[0].contains("R refresh"), "{}", lines[0]);
        assert!(lines[0].contains("W window"), "{}", lines[0]);

        // 100 cols: the prefix ends before the third entry.
        let text = draw(&representative());
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        let l1 = &lines[0];
        assert!(l1.contains("S snapshot"), "{l1}");
        assert!(!l1.contains("Q quit"), "{l1}");
        assert!(!l1.contains("B bench"), "{l1}");
        assert!(l1.chars().count() <= 100, "{}", l1.chars().count());

        // 60 cols with a short daemon status: only the first entry.
        let mut state = representative();
        state.daemon_status = "up".to_owned();
        let text = draw_at(&state, 60, 30);
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        let l1 = &lines[0];
        assert!(l1.contains("R refresh"), "{l1}");
        assert!(!l1.contains("S snapshot"), "{l1}");
        assert!(l1.chars().count() <= 60, "{}", l1.chars().count());
    }
}
