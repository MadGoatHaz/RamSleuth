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
//!   (<reason>)` — clocks/ratios (MCLK/UCLK/FCLK), GEAR_DOWN/CR, primary /
//!   secondary / tertiary + turnaround timings, CAD drive/termination
//!   (Ω), voltages (the VDDCR_VDD primary rail first).
//! - **Zone 2 — AIDA-style benchmark engine:** the 4×4 grid (tier rows ×
//!   Read/Write/Copy/Latency columns) — live during a run: a normal
//!   bench's [`live_grid`] accumulates the streamed progress events
//!   (the newest value per cell wins; a non-finite / non-positive
//!   reading never counts) and a burn-in shows `burn_in.latest` (its
//!   newest per-cell values), the measured cells dimmed with a `…`
//!   suffix (the GUI C7-17/18 convention), the unstarted cells `N/A`;
//!   not in flight, the terminal grid of the last completed run. Below
//!   the grid, the flat C7-17 status line (`Status: Idle` /
//!   `Status: Running… <m:ss>` / `Status: Running… (burn-in <m:ss>,
//!   iter <n>)` / `Status: Done`) — it replaces the pre-parity `bench:`
//!   progress line (the parity goal's intentional re-render of that
//!   line). Below the status line, the controls line (`[B] Full` /
//!   `[M] Mem` / `[X] Burn-in(5m)` — the run keys dimmed while any run
//!   is in flight, the GUI single-flight rule — plus `[C] Cancel`,
//!   shown only while a run is in flight) and the live burn-in row
//!   (`Burn-in: iteration <n> · <m:ss> elapsed · Memory Read <…> ·
//!   <latency>`, present only while a burn-in runs — the GUI
//!   `burn_in_row` form).
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
//! a placeholder (`N/A`, `Status: Idle`, `not connected`); an all-`Na`
//! or the default [`AppState`] draws without panicking (the tests assert
//! through ratatui's in-memory `TestBackend`).

use std::sync::Mutex;
use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table};
use ratatui::Frame;

use ramsleuth_bench::{BenchOp, BenchmarkGrid, Metric, StreamProgress, Tier};
use ramsleuth_telemetry::amd_readout::{
    CadBus, ClockReadout, CommandRate, DivMode, RttValue, TimingSet, VoltageSet,
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
/// grid + the `Status: Idle` line for it.
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
/// CPU/platform identity, line 3 the RAM summary — the capacity,
/// per-DIMM breakdown, SPD speed, channel mode, and UCLK:MCLK sync
/// mode) and the three zones side by side. Every zone is a
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

/// Header line 3 (TUI-11 — the RAM summary, the GUI C6-20/C9-01
/// line-3 mirror): `RAM: <total> (<summary>) <max SPD MT/s> |
/// <channel> | <note> | Mode: <sync>` — the total capacity in the
/// selected capacity unit (`settings.capacity_gib`), the per-DIMM
/// breakdown ([`dimm_summary`], with the rank word), the max SPD speed
/// (omitted when no module carries one), the channel mode
/// ([`channel_mode`]), the total-vs-breakdown slot note
/// ([`slot_note`], when the OS total strictly exceeds the SPD sum),
/// and the UCLK:MCLK sync mode ([`sync_mode`] — `Synchronous 1:1`
/// amber, `Asynchronous 1:2` crimson, `N/A` dim). The capacity + clock
/// segments honor the frozen unit knobs. No telemetry degrades to the
/// dim `RAM: —` placeholder (never a panic).
fn header_line3(state: &AppState) -> Line<'static> {
    let Some(telemetry) = &state.telemetry else {
        return Line::from(Span::styled("RAM: —", Style::default().fg(DIM)));
    };
    let (total, summary, speed, channel, note, (mode, mode_color)) = ram_line3_parts(
        telemetry,
        state.settings.capacity_gib,
        state.settings.clock_mhz,
    );
    let value = Style::default().fg(CYAN);
    let label = Style::default().fg(DIM);
    let mut spans = vec![
        Span::styled("RAM: ", label),
        Span::styled(total, value),
        Span::styled(format!(" ({summary})"), value),
    ];
    if let Some(speed) = speed {
        spans.push(Span::styled(format!(" {speed}"), value));
    }
    spans.push(Span::styled(" | ", label));
    spans.push(Span::styled(channel, value));
    if let Some(note) = note {
        spans.push(Span::styled(" | ", label));
        spans.push(Span::styled(note, Style::default().fg(AMBER)));
    }
    spans.push(Span::styled(" | Mode: ", label));
    spans.push(Span::styled(mode, Style::default().fg(mode_color)));
    Line::from(spans)
}

/// The binary → decimal capacity conversion factor (1 GiB =
/// 1.073741824 GB; the GUI `GIB_TO_GB` mirror).
const GIB_TO_GB: f64 = 1.073741824;

/// One capacity readout (the carried GiB wire value) as display text in
/// the selected capacity unit (the GUI `format_capacity`/`trim` mirror,
/// C7-11): the GiB arm keeps the value (`16 GiB`), the GB arm converts
/// × [`GIB_TO_GB`] (`17.2 GB`); a whole number renders without
/// decimals, one decimal otherwise; a non-finite value degrades to the
/// honest `N/A`.
fn format_capacity(gib: f64, capacity_gib: bool) -> String {
    if !gib.is_finite() {
        return "N/A".to_owned();
    }
    let trim = |v: f64| {
        if (v - v.round()).abs() < 0.05 {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        }
    };
    if capacity_gib {
        format!("{} GiB", trim(gib))
    } else {
        format!("{} GB", trim(gib * GIB_TO_GB))
    }
}

/// The rank word for the header breakdown (the GUI `rank_word` mirror,
/// C9-01/D-1): `1` → `Single-Rank`, `2` → `Dual-Rank`, `n > 0` →
/// `<n>-Rank`; a `Na`/`0` rank yields `None` (the word is omitted from
/// the group, never printed as `N/A`).
fn rank_word(rank: &Section<u8>) -> Option<String> {
    match rank {
        Section::Value(0) | Section::Na(_) => None,
        Section::Value(1) => Some("Single-Rank".to_owned()),
        Section::Value(2) => Some("Dual-Rank".to_owned()),
        Section::Value(other) => Some(format!("{other}-Rank")),
    }
}

/// The per-DIMM capacity summary (the GUI `dimm_summary` mirror,
/// C9-01/D-1): one `<count>x<size>` group per distinct carried
/// `(size, rank word)` (first-seen order, ` + `-joined, the size in the
/// selected capacity unit — [`format_capacity`]); the rank word from
/// the parallel `spd` slice (positionally aligned with `sizes`, the
/// facade contract) is appended when present (`2x16 GiB Single-Rank`);
/// a `Na` size contributes nothing; an all-`Na` / empty list degrades
/// to `N/A`.
fn dimm_summary(sizes: &[Section<f64>], spd: &[SpdModule], capacity_gib: bool) -> String {
    let mut groups: Vec<(f64, Option<String>, usize)> = Vec::new();
    for (slot, cell) in sizes.iter().enumerate() {
        if let Some(gib) = cell.value() {
            let rank = spd.get(slot).and_then(|module| rank_word(&module.rank));
            match groups
                .iter_mut()
                .find(|(value, word, _)| (value - gib).abs() < 0.05 && *word == rank)
            {
                Some(group) => group.2 += 1,
                None => groups.push((*gib, rank, 1)),
            }
        }
    }
    if groups.is_empty() {
        "N/A".to_owned()
    } else {
        groups
            .iter()
            .map(|(gib, rank, count)| match rank {
                Some(word) => format!("{count}x{} {word}", format_capacity(*gib, capacity_gib)),
                None => format!("{count}x{}", format_capacity(*gib, capacity_gib)),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// The channel mode from the DIMM count (the GUI `channel_mode` mirror,
/// D-C8): 1 / 2 / 4 → Single- / Dual- / Quad-Channel; any other count
/// (0, odd) degrades to `N/A`.
fn channel_mode(dimm_count: usize) -> String {
    match dimm_count {
        1 => "Single-Channel".to_owned(),
        2 => "Dual-Channel".to_owned(),
        4 => "Quad-Channel".to_owned(),
        _ => "N/A".to_owned(),
    }
}

/// The total-vs-breakdown slot note (the GUI `slot_note` mirror,
/// C9-01/D-1): the OS total can strictly exceed the SPD-visible sum
/// when the host installs more DIMMs than the SPD bus binds (the live
/// host: 4×16 GiB installed, 2 bound). A uniform per-module size whose
/// quotient `total / u` lands within a tenth of a module of an integer
/// `n` above the visible count → `<visible> of <n> slots SPD-visible`
/// (the OS reserves a fraction of the installed capacity, so the
/// quotient sits just below the integer); any other excess → `SPD sees
/// <visible> of the installed capacity`. A `Na` / non-finite total, no
/// visible modules, or `total ≤ sum` → `None` (no false alarm).
/// Unit-agnostic: counts, not GiB.
fn slot_note(total: &Section<f64>, sizes: &[Section<f64>]) -> Option<String> {
    let total = total.value().copied().filter(|value| value.is_finite())?;
    let visible: Vec<f64> = sizes.iter().filter_map(|cell| cell.value().copied()).collect();
    if visible.is_empty() {
        return None;
    }
    let sum = visible.iter().sum::<f64>();
    if total <= sum {
        return None;
    }
    let uniform = visible.iter().all(|value| (value - visible[0]).abs() < 0.05);
    if uniform {
        let unit = visible[0];
        if unit > 0.0 {
            let quotient = total / unit;
            let slots = quotient.round();
            if (quotient - slots).abs() <= 0.1 && slots > visible.len() as f64 {
                return Some(format!("{} of {slots:.0} slots SPD-visible", visible.len()));
            }
        }
    }
    Some(format!("SPD sees {} of the installed capacity", visible.len()))
}

/// Line 3's mode segment over one clock readout (the GUI
/// `sync_mode_from_clocks` mirror, D-C8): the UCLK:MCLK ratio + its
/// semantic color — AMBER for `Synchronous 1:1` (with the MCLK in the
/// selected clock unit when it carries one — [`format_clock`]),
/// CRIMSON for `Asynchronous 1:2`, and DIM for the honest `N/A`.
fn sync_mode_from_clocks(clocks: &ClockReadout, clock_mhz: bool) -> (String, Color) {
    match clocks.div_mode.value() {
        Some(DivMode::OneToOne) => {
            let text = match clocks.mclk_mhz.value() {
                Some(mhz) => {
                    format!("Synchronous 1:1 (UCLK = MCLK = {})", format_clock(*mhz, clock_mhz))
                }
                None => "Synchronous 1:1".to_owned(),
            };
            (text, AMBER)
        }
        Some(DivMode::OneToTwo) => ("Asynchronous 1:2".to_owned(), CRIMSON),
        None => ("N/A".to_owned(), DIM),
    }
}

/// Line 3's mode segment over a whole snapshot (the GUI `sync_mode`
/// mirror): the AMD branch must carry a value whose `div_mode` is
/// usable, else the honest `N/A` (DIM) — the Intel / driver-missing /
/// degraded states all degrade here (never a panic).
fn sync_mode(t: &SystemMemoryTelemetry, clock_mhz: bool) -> (String, Color) {
    match t.amd.value() {
        Some(readout) => sync_mode_from_clocks(&readout.clocks, clock_mhz),
        None => ("N/A".to_owned(), DIM),
    }
}

/// The line-3 pieces — the single source of truth for the flat
/// [`ram_line_prefix`] text and the per-segment-coloured
/// [`header_line3`]: the total (selected capacity unit), the
/// breakdown, the optional max-SPD speed, the channel mode, the
/// optional slot note, and the mode segment (text + color).
fn ram_line3_parts(
    t: &SystemMemoryTelemetry,
    capacity_gib: bool,
    clock_mhz: bool,
) -> (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    (String, Color),
) {
    let total = match t.total_capacity.value() {
        Some(gib) => format_capacity(*gib, capacity_gib),
        None => "N/A".to_owned(),
    };
    let summary = dimm_summary(&t.dimm_sizes, &t.spd, capacity_gib);
    let speed = t
        .spd
        .iter()
        .filter_map(|m| m.speed_mts.value().copied())
        .max()
        .map(|mts| format!("{mts} MT/s"));
    let channel = channel_mode(t.dimm_sizes.len());
    let note = slot_note(&t.total_capacity, &t.dimm_sizes);
    let mode = sync_mode(t, clock_mhz);
    (total, summary, speed, channel, note, mode)
}

/// Line 3's non-mode part (the GUI `ram_line_prefix` mirror, C6-20/
/// C9-01): the total capacity in the selected capacity unit, the
/// per-DIMM breakdown (with the rank word), the max SPD speed (omitted
/// when no module carries one), the channel mode, the slot note when
/// the OS total strictly exceeds the SPD sum, and the `Mode: ` lead-in
/// the mode segment completes.
///
/// Test-only: [`header_line3`] builds its per-segment-coloured spans
/// from [`ram_line3_parts`] directly; this flat-text form exists so the
/// composed line is assertable string-for-string (the GUI mirror).
#[cfg(test)]
fn ram_line_prefix(t: &SystemMemoryTelemetry, capacity_gib: bool) -> String {
    let (total, summary, speed, channel, note, _) = ram_line3_parts(t, capacity_gib, true);
    let mut line = format!("RAM: {total} ({summary})");
    if let Some(speed) = speed {
        line.push_str(&format!(" {speed}"));
    }
    line.push_str(&format!(" | {channel}"));
    if let Some(note) = note {
        line.push_str(&format!(" | {note}"));
    }
    line.push_str(" | Mode: ");
    line
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
    // The `GDM / CR` split (the GUI's D-6 form): the gear-down mode and
    // the DRAM command rate each own a bare-value row; an absent cell
    // degrades its row to the GUI's bare `N/A` (D-4).
    items.push(bare_na_row("GEAR_DOWN", &clocks.gdm, CYAN, fmt_gear_down));
    items.push(bare_na_row("CR", &clocks.command_rate, CYAN, fmt_cr));
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
    // The Vcore primary rail (the C12 frozen wire field) leads the
    // section exactly as the GUI; on Intel it degrades to a bare N/A.
    items.push(bare_na_row("VDDCR_VDD", &voltages.vcore_mv, CYAN, fmt_volts));
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

/// The `Status: Done` line's green (the GUI `bench_zone` C7-17 zone-local
/// const — a dark-slate-compatible green; the §3.2 palette stays frozen:
/// the status colors are zone-local, not a palette entry).
const STATUS_DONE: Color = Color::Rgb(0x2E, 0x9E, 0x5B);

/// Zone 2: the 4×4 grid (five rows: header + four tiers) + the flat
/// status line + the controls line + the burn-in row (blank when no
/// burn-in runs) + slack — clipped, never scrolled. The fixed
/// 5 + 1 + 1 + 1 row slots keep the zone's layout stable as the rows
/// appear (the GUI's no-layout-shift rule, the terminal adaptation:
/// an absent row leaves its slot blank, the rows below it never move).
fn render_zone2(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = zone_block("2 · BENCH (GB/s)");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // the grid (header + four tier rows)
            Constraint::Length(1), // the flat status line (C7-17)
            Constraint::Length(1), // the controls line (TUI-14)
            Constraint::Length(1), // the burn-in row (TUI-14, blank when absent)
            Constraint::Fill(1),   // slack
        ])
        .split(inner);
    frame.render_widget(bench_table(&state.bench), parts[0]);
    let (text, color) = bench_status(&state.bench);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(&text, Style::default().fg(color)))),
        parts[1],
    );
    frame.render_widget(Paragraph::new(controls_line(&state.bench)), parts[2]);
    if let Some(row) = burn_in_row_line(&state.bench.burn_in) {
        frame.render_widget(Paragraph::new(row), parts[3]);
    }
}

/// The live in-flight grid for a **normal** bench (the GUI
/// `bench_zone::live_grid` rule): accumulate each streamed
/// [`StreamProgress`] event's (tier, op, value) into a zero grid — the
/// latest event per cell wins. The row is the tier (its discriminant is
/// the grid-array slot: `Memory = 0, L1 = 1, L2 = 2, L3 = 3`) and the
/// column is the op (`Read` / `Write` / `Copy`); the stream carries
/// bandwidth ops only (no latency events — the streamed contract), so
/// the `latency_ns` column stays 0.0. A non-finite or non-positive value
/// is ignored (never renders as data), and unmeasured cells stay 0.0
/// (which renders `N/A`). A burn-in's live grid is `burn_in.latest`
/// instead (C7-18 — see [`table_live_grid`]).
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

/// The 4×4 grid's live in-flight grid for the current run (the GUI
/// `table_live_grid`, C7-18): a burn-in in flight shows `burn_in.latest`
/// (the newest per-cell values seen this burn-in — the cells update per
/// iteration), a normal bench in flight the accumulated
/// [`live_grid`]; neither in flight → the (unused) progress
/// accumulation is returned harmlessly (the terminal grid renders).
fn table_live_grid(bench: &BenchState) -> BenchmarkGrid {
    if bench.burn_in.running {
        bench.burn_in.latest.clone()
    } else {
        live_grid(&bench.progress)
    }
}

/// One grid cell's render phase within the zone's current run state (the
/// GUI `bench_zone` C7-17/18 mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellPhase {
    /// The run is not in flight: the cell shows its terminal value from
    /// the result grid (or the `N/A` placeholder when unmeasured).
    Terminal,
    /// A run is in flight and the cell has a live value: the dimmed
    /// in-flight fill + a `…` suffix.
    Live,
    /// A run is in flight and the cell has no live value yet — not
    /// started, or a latency cell of a normal bench (no progress
    /// events): the `N/A` placeholder.
    NotStarted,
}

/// One cell's render [`CellPhase`]: not in flight → `Terminal` (the
/// result grid); in flight → `Live` when the live grid carries a
/// (finite, positive) value for the cell, else `NotStarted` (a cell not
/// started yet, or a normal-bench latency cell — the stream carries no
/// latency events).
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

/// One cell's display for its render [`CellPhase`] (the bare
/// terminal-width form): the terminal value in the existing two-decimal
/// shape — cyan for a measured reading (a non-finite / unmeasured cell
/// degrades to the crimson `N/A`, the GUI's guard), the live in-flight
/// value with a `…` suffix (dimmed — the terminal adaptation of the
/// GUI's dimmed-cyan fill), or the `N/A` placeholder.
fn bench_cell_text(
    phase: CellPhase,
    terminal: &BenchmarkGrid,
    live: &BenchmarkGrid,
    tier: Tier,
    metric: Metric,
) -> (String, Color) {
    match phase {
        CellPhase::Terminal => {
            let value = terminal.cell(tier, metric);
            if value.is_finite() && value > 0.0 {
                (format!("{value:.2}"), CYAN)
            } else {
                ("N/A".to_owned(), CRIMSON)
            }
        }
        CellPhase::Live => {
            let value = live.cell(tier, metric);
            (format!("{value:.2}…"), DIM)
        }
        CellPhase::NotStarted => ("N/A".to_owned(), CRIMSON),
    }
}

/// The 4×4 grid: tier rows × Read/Write/Copy/Latency columns, in its
/// current run state (the GUI `render_grid_table` mirror, the bare
/// terminal-width form: the Read/Write/Copy columns are GB/s — named in
/// the zone title — and the last column is ns/hop, the AIDA64
/// convention). **Not in flight** — the terminal result grid of the
/// last completed run (or the all-`N/A` zero grid when none has landed
/// yet — the layout never shifts when the result lands); **a run in
/// flight** — the [`table_live_grid`] overlay: the live cells dimmed
/// with a `…` suffix, the unstarted cells (and a normal bench's latency
/// column — the stream carries no latency events) keep `N/A`, the
/// terminal grid hidden until the run settles.
fn bench_table(bench: &BenchState) -> Table<'static> {
    // The terminal result grid of the last completed run, or the
    // all-`N/A` zero grid when none has landed yet.
    let terminal = match bench.grid.as_ref() {
        Some(grid) => grid.clone(),
        None => BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        },
    };
    // Any run in flight (a normal bench or a burn-in) → the live cells
    // render the in-flight fill.
    let in_flight = bench.running || bench.burn_in.running;
    let live = table_live_grid(bench);
    let header = Row::new(
        ["", "Read", "Write", "Copy", "ns/hop"]
            .iter()
            .map(|name| Cell::from(*name).style(Style::default().fg(DIM))),
    );
    let mut rows = vec![header];
    for &tier in &[Tier::Memory, Tier::L1, Tier::L2, Tier::L3] {
        let mut cells = vec![Cell::from(tier_name(tier)).style(Style::default().fg(CYAN))];
        for &metric in &[Metric::Read, Metric::Write, Metric::Copy, Metric::Latency] {
            let (text, color) = bench_cell_text(
                cell_phase(in_flight, &live, tier, metric),
                &terminal,
                &live,
                tier,
                metric,
            );
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

/// The bench zone's flat status line state (the GUI `bench_zone` C7-17
/// mirror — the progress line's pill retired): the idle rest state, a
/// run in flight (with the run's elapsed), and a finished run (a
/// terminal grid or kept streamed progress present).
#[derive(Debug, Clone, Copy, PartialEq)]
enum StatusState {
    /// No run in flight and no run has finished: the dim `Status: Idle`
    /// line.
    Idle,
    /// A run (a normal bench or a burn-in) is in flight: the CYAN
    /// running line; `elapsed_secs` is the run's elapsed (the newest
    /// tick's for a burn-in, the zone's start clock's for a normal
    /// bench). `burn_in_iteration` is `Some(n)` for a burn-in (the
    /// `Status: Running… (burn-in <m:ss>, iter <n>)` line) and `None`
    /// for a normal bench (the `Status: Running… <m:ss>` line).
    Running { elapsed_secs: f64, burn_in_iteration: Option<u32> },
    /// A run finished (a terminal grid landed, or streamed progress is
    /// kept): the green `Status: Done` line.
    Done,
}

/// The status state of one run bookkeeping (the GUI `status_state`
/// mirror): a burn-in in flight → `Running` (the newest tick's elapsed +
/// iteration — the normal-bench flag stays false for a burn-in, C7-16),
/// a normal bench in flight → `Running` (the zone's start clock's
/// elapsed, no burn-in iteration), neither in flight but a terminal
/// grid or kept streamed progress present → `Done`, the fresh state (no
/// grid, no progress) → `Idle`. The `Done` corner deliberately includes
/// the kept-progress-only shape (a cancelled normal bench with no
/// terminal grid): a non-empty progress list with no run in flight is a
/// finished run, not an idle one.
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

/// One run's elapsed in the status line's `m:ss` form (the GUI
/// `format_elapsed` mirror — the plan's `0:42`): the minutes un-padded,
/// the seconds two-digit; a non-finite / non-positive reading renders
/// `0:00` (a bad elapsed never panics the line, the no-panic contract).
fn format_elapsed(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    format!("{}:{:02}", total / 60, total % 60)
}

/// The flat status line's text (the GUI `status_text` mirror):
/// `Status: Idle`, `Status: Running… <m:ss>` (a normal bench — the `…`
/// kept static, the repaint animates the elapsed), `Status: Running…
/// (burn-in <m:ss>, iter <n>)` (a burn-in, C7-18), `Status: Done`.
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

/// The flat status line's color (the GUI `status_color` mirror, the
/// terminal palette): the idle line is dim, the running line CYAN (a
/// normal bench or a burn-in), the done line the zone-local green
/// [`STATUS_DONE`].
fn status_color(state: StatusState) -> Color {
    match state {
        StatusState::Idle => DIM,
        StatusState::Running { .. } => CYAN,
        StatusState::Done => STATUS_DONE,
    }
}

/// The zone-local start clock for a normal bench run (the GUI
/// `bench_zone` C7-17 precedent): the moment the zone first saw a normal
/// bench in flight (a repaint within one frame of the run's start). The
/// burn-in needs no clock — its newest tick carries the elapsed
/// (`burn_in.elapsed_secs`). Stamped on the first in-flight frame,
/// cleared on the terminal; a poisoned lock is recovered in place (the
/// render thread is the sole user — the no-panic contract).
static NORMAL_RUN_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Stamp / clear the zone-local normal-run start clock for this frame's
/// run state, returning the start instant while a normal bench is in
/// flight (`None` outside one — the terminal cleared it).
fn track_run_start(running: bool) -> Option<Instant> {
    let mut guard = NORMAL_RUN_START.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if running {
        Some(*guard.get_or_insert_with(Instant::now))
    } else {
        *guard = None;
        None
    }
}

/// The bench zone's flat status line (the GUI C7-17 mirror): a single
/// text line below the grid — `Status: Idle` dim, `Status: Running…
/// <m:ss>` CYAN (a normal bench — elapsed from the zone's start clock),
/// `Status: Running… (burn-in <m:ss>, iter <n>)` CYAN (a burn-in — the
/// newest tick's elapsed + iteration), `Status: Done` in the
/// zone-local green.
fn bench_status(bench: &BenchState) -> (String, Color) {
    let burn_in = &bench.burn_in;
    let running_elapsed = if burn_in.running {
        burn_in.elapsed_secs
    } else {
        track_run_start(bench.running).map_or(0.0, |start| start.elapsed().as_secs_f64())
    };
    let state = status_state(
        bench.running,
        burn_in.running,
        bench.grid.is_some(),
        !bench.progress.is_empty(),
        running_elapsed,
        burn_in.iteration,
    );
    (status_text(state), status_color(state))
}

/// The controls line's flat text (the GUI `render_controls` terminal
/// form, TUI-14): the three run keys — `[B] Full` (a full-scope
/// bench), `[M] Mem` (a memory-only bench), `[X] Burn-in(5m)` (the
/// GUI's 5-minute burn-in preset) — always present, plus `[C] Cancel`
/// shown only while a run (a normal bench or a burn-in) is in flight
/// (the GUI single-flight rule: the run keys disable while one is in
/// flight, and `Cancel` stops it).
///
/// Test-only: [`controls_line`] builds its per-segment-coloured spans
/// directly; this flat-text form exists so the composed line is
/// assertable string-for-string (the TUI-11 `ram_line_prefix`
/// precedent).
#[cfg(test)]
fn controls_text(bench: &BenchState) -> String {
    const RUN_KEYS: &str = "[B] Full  [M] Mem  [X] Burn-in(5m)";
    if bench.running || bench.burn_in.running {
        format!("{RUN_KEYS}  [C] Cancel")
    } else {
        RUN_KEYS.to_owned()
    }
}

/// The controls line as a rendered line (TUI-14): each run key is
/// CYAN (the actionable part) with its label dim when the run keys
/// are enabled; while any run is in flight (a normal bench or a
/// burn-in) the run key spans dim (the GUI disabled-button form) and
/// the CYAN `[C] Cancel` entry appears to stop it.
fn controls_line(bench: &BenchState) -> Line<'static> {
    let in_flight = bench.running || bench.burn_in.running;
    let run_key = if in_flight { DIM } else { CYAN };
    let mut spans = vec![
        Span::styled("[B]", Style::default().fg(run_key)),
        Span::styled(" Full  ", Style::default().fg(DIM)),
        Span::styled("[M]", Style::default().fg(run_key)),
        Span::styled(" Mem  ", Style::default().fg(DIM)),
        Span::styled("[X]", Style::default().fg(run_key)),
        Span::styled(" Burn-in(5m)", Style::default().fg(DIM)),
    ];
    if in_flight {
        spans.push(Span::styled("  [C]", Style::default().fg(CYAN)));
        spans.push(Span::styled(" Cancel", Style::default().fg(DIM)));
    }
    Line::from(spans)
}

/// The live burn-in row's flat text (the GUI `burn_in_row` mirror,
/// C7-18 / TUI-14): the newest tick's bookkeeping + the headline
/// `latest` values — `Burn-in: iteration <n> · <m:ss> elapsed ·
/// Memory Read <…> · <latency>`; the cells in the plain grid form
/// (the [`bench_cell_text`] Terminal arm — unit-less, the zone title
/// carries the GB/s unit and the `ns/hop` column header the ns one),
/// a cell not yet streamed (0.0 on the wire) or a non-finite reading
/// reads `N/A`. `None` when no burn-in is in flight (the row is
/// absent — its fixed layout slot stays blank, the no-layout-shift
/// rule).
///
/// Test-only: [`burn_in_row_line`] builds its per-segment-coloured
/// spans from the same pieces; this flat-text form exists so the
/// composed line is assertable string-for-string (the TUI-11
/// `ram_line_prefix` precedent).
#[cfg(test)]
fn burn_in_row_text(burn_in: &BurnInState) -> Option<String> {
    if !burn_in.running {
        return None;
    }
    let grid = &burn_in.latest;
    let read = bench_cell_text(CellPhase::Terminal, grid, grid, Tier::Memory, Metric::Read).0;
    let latency =
        bench_cell_text(CellPhase::Terminal, grid, grid, Tier::Memory, Metric::Latency).0;
    Some(format!(
        "Burn-in: iteration {} · {} elapsed · Memory Read {} · {}",
        burn_in.iteration,
        format_elapsed(burn_in.elapsed_secs),
        read,
        latency
    ))
}

/// The live burn-in row as a rendered line (TUI-14): the dim
/// `Burn-in: iteration <n> · <m:ss> elapsed · Memory Read` prefix,
/// then the two `latest` headline cells in their own measured /
/// `N/A` colors (the [`bench_cell_text`] Terminal arm).
fn burn_in_row_line(burn_in: &BurnInState) -> Option<Line<'static>> {
    if !burn_in.running {
        return None;
    }
    let grid = &burn_in.latest;
    let (read, read_color) =
        bench_cell_text(CellPhase::Terminal, grid, grid, Tier::Memory, Metric::Read);
    let (latency, latency_color) =
        bench_cell_text(CellPhase::Terminal, grid, grid, Tier::Memory, Metric::Latency);
    Some(Line::from(vec![
        Span::styled(
            format!(
                "Burn-in: iteration {} · {} elapsed · Memory Read ",
                burn_in.iteration,
                format_elapsed(burn_in.elapsed_secs)
            ),
            Style::default().fg(DIM),
        ),
        Span::styled(read, Style::default().fg(read_color)),
        Span::styled(" · ", Style::default().fg(DIM)),
        Span::styled(latency, Style::default().fg(latency_color)),
    ]))
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

/// A cell row in the GUI's bare-`N/A` form (D-4): the value in `color`,
/// or a bare crimson `N/A` (the reason stays on the wire, never a
/// parenthetical). The zone-1 `GEAR_DOWN` / `CR` / `VDDCR_VDD` rows
/// mirror the GUI's per-row degradation exactly.
fn bare_na_row<T>(
    key: &str,
    section: &Section<T>,
    color: Color,
    fmt: impl Fn(&T) -> String,
) -> ListItem<'static> {
    match section {
        Section::Value(value) => row(key, &fmt(value), color),
        Section::Na(_) => row(key, "N/A", CRIMSON),
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

/// The `GEAR_DOWN` row value (the GUI's D-6 form): the gear-down
/// mode's bare `Enabled` / `Disabled`.
fn fmt_gear_down(v: &bool) -> String {
    if *v {
        "Enabled".to_owned()
    } else {
        "Disabled".to_owned()
    }
}

/// The `CR` row value (the GUI's D-6 form): the DRAM command rate's
/// bare `1T` / `2T`.
fn fmt_cr(v: &CommandRate) -> String {
    match v {
        CommandRate::OneT => "1T".to_owned(),
        CommandRate::TwoT => "2T".to_owned(),
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
        // zone 2: the grid renders (idle, no run yet -> N/A cells) +
        // the flat status line (the TUI-13 re-render of the progress line)
        assert!(text.contains("Status: Idle"), "{text}");
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
    /// `N/A` grid, and the `Status: Idle` line.
    #[test]
    fn default_state_renders_placeholders_without_panic() {
        let text = draw(&AppState::default());

        // all sixteen grid cells plus the zone-1/zone-3 placeholders
        assert!(
            text.matches("N/A").count() >= 18,
            "expected >= 18 N/A placeholders, got:\n{text}"
        );
        assert!(text.contains("Status: Idle"), "{text}");
        assert!(text.contains("not connected"), "{text}");
        assert!(text.contains("no telemetry"), "{text}");
    }

    /// (c) A running bench renders the grid's live overlay — the
    /// streamed cell dimmed with a `…` suffix, the terminal grid
    /// hidden until the run settles (the unstarted cells — including
    /// the terminal L1 latency, which a normal bench's stream never
    /// carries — stay `N/A`) — and the flat `Status: Running… <m:ss>`
    /// line (the TUI-13 re-render of the pre-parity `bench:` progress
    /// line; the run-start clock is stamped within this one frame, so
    /// the elapsed renders `0:00`).
    #[test]
    fn running_bench_renders_live_grid_and_status() {
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
        // 100×62: the full zone-1 surface (the 30-row default clips
        // the rtt_park / VDDCR_VDD N/A rows out of the count).
        let text = draw_at(&state, 100, 62);

        assert!(text.contains("Status: Running… 0:00"), "{text}");
        // the streamed cell: its live value dimmed with the `…` suffix
        assert!(text.contains("42.50…"), "{text}");
        assert!(text.contains("ns/hop"), "{text}");
        // the unmeasured cells stay N/A (15 grid cells — the terminal
        // L1 latency included, the live grid carries no latency — +
        // the zone-1 Na cells: rfc2, rtt_park, the bare-N-A VDDCR_VDD)
        assert!(text.matches("N/A").count() >= 18, "{text}");
    }

    /// (c′) A completed run (not running, grid present) renders the
    /// flat `Status: Done` line (the TUI-13 re-render of the pre-parity
    /// `bench: done`).
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

        assert!(text.contains("Status: Done"), "{text}");
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
        // (channel 0 is 55 rows), just enough to reach the second
        // channel's label; a shorter terminal clips it.
        let text = draw_at(&state, 100, 62);

        assert!(text.contains("Intel ch 0"), "{text}");
        assert!(text.contains("Intel ch 1"), "{text}");
        assert!(text.contains("1600.00 MHz"), "{text}");
        assert!(text.contains("SPD: N/A"), "{text}");
    }

    /// (f) The 3-line header — line 1 the title + platform tag + daemon
    /// status, line 2 the CPU/platform identity (the representative's
    /// Zen 3 / 3500 MHz / Test Board / BIOS 1.0 / all-Na AGESA+SMU
    /// shape), line 3 the composed RAM summary (the TUI-11 capacity /
    /// breakdown / SPD speed / channel / sync mode).
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
        // Line 3 (TUI-11): the composed RAM summary — the 16 GiB total,
        // the single Dual-Rank 16 GiB DIMM, the 3200 MT/s SPD speed, the
        // Single-Channel mode (one bound module), and the Asynchronous
        // 1:2 UCLK:MCLK (the fixture's div mode).
        assert_eq!(
            lines[2],
            "RAM: 16 GiB (1x16 GiB Dual-Rank) 3200 MT/s | Single-Channel | Mode: Asynchronous 1:2",
            "{}",
            lines[2]
        );
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

    // ------------------------------------------------------------------
    // TUI-11 — the header line-3 (RAM summary) pure helpers.
    // ------------------------------------------------------------------

    /// A minimal SPD module fixture for the line-3 tests (every cell
    /// `Na` except the `rank` / `speed_mts` the helpers consume).
    fn module(rank: Section<u8>, speed: Option<u16>) -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::na(NaReason::NotApplicable),
            die_maker: Section::na(NaReason::NotApplicable),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::na(NaReason::NotApplicable),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank,
            density_mbit: Section::na(NaReason::NotApplicable),
            speed_mts: match speed {
                Some(value) => Section::Value(value),
                None => Section::na(NaReason::NotApplicable),
            },
            profiles: Vec::new(),
        }
    }

    /// A line-3 test snapshot: the configurable capacity tail + SPD
    /// list over an Na AMD / Intel branch (the mode segment degrades;
    /// the helpers under test here do not read it).
    fn telemetry(
        total_capacity: Section<f64>,
        dimm_sizes: Vec<Section<f64>>,
        spd: Vec<SpdModule>,
    ) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "synthetic".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd,
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3600.0),
                motherboard: Section::Value("Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity,
            dimm_sizes,
        }
    }

    /// A clock-readout fixture (every cell `Na` except the two the
    /// mode segment consumes).
    fn clocks_with(div_mode: Section<DivMode>, mclk_mhz: Section<f64>) -> ClockReadout {
        ClockReadout {
            mclk_mhz,
            uclk_mhz: Section::na(NaReason::NotApplicable),
            fclk_mhz: Section::na(NaReason::NotApplicable),
            div_mode,
            gear_mode: Section::na(NaReason::NotApplicable),
            gdm: Section::na(NaReason::NotApplicable),
            pdm: Section::na(NaReason::NotApplicable),
            command_rate: Section::na(NaReason::NotApplicable),
        }
    }

    /// (l) `format_capacity`: the GiB arm keeps the wire value, the GB
    /// arm converts × [`GIB_TO_GB`]; a whole number renders without
    /// decimals, one decimal otherwise; non-finite → `N/A`.
    #[test]
    fn format_capacity_arms() {
        assert_eq!(format_capacity(16.0, true), "16 GiB");
        assert_eq!(format_capacity(32.0, true), "32 GiB");
        assert_eq!(format_capacity(4.5, true), "4.5 GiB");
        assert_eq!(format_capacity(16.0, false), "17.2 GB");
        assert_eq!(format_capacity(32.0, false), "34.4 GB");
        assert_eq!(format_capacity(f64::NAN, true), "N/A");
        assert_eq!(format_capacity(f64::INFINITY, false), "N/A");
    }

    /// (m) `rank_word` (the GUI C9-01/D-1 mirror): `1` → Single-Rank,
    /// `2` → Dual-Rank, `n > 0` → `<n>-Rank`; a `Na`/`0` rank yields
    /// `None` (the word is omitted, never `N/A`).
    #[test]
    fn rank_word_arms() {
        assert_eq!(rank_word(&Section::Value(1)), Some("Single-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(2)), Some("Dual-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(4)), Some("4-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(0)), None);
        assert_eq!(rank_word(&Section::na(NaReason::NotApplicable)), None);
    }

    /// (n) `dimm_summary`: distinct-size grouping in the selected
    /// capacity unit (the GB knob converts × [`GIB_TO_GB`]), the Na
    /// entry contributes nothing, all-Na / empty → `N/A`, and the rank
    /// word from the parallel SPD slice groups with the size (same-size
    /// same-rank stays one group with the word, same-size different-rank
    /// splits, a `Na` rank omits the word).
    #[test]
    fn dimm_summary_groups_and_degrades() {
        // No rank words (the empty SPD list): grouped by size alone.
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[], true),
            "2x16 GiB"
        );
        // A Na entry contributes nothing; the mixed kit → two groups.
        assert_eq!(
            dimm_summary(
                &[
                    Section::Value(16.0),
                    Section::na(NaReason::NotApplicable),
                    Section::Value(32.0),
                ],
                &[],
                true,
            ),
            "1x16 GiB + 1x32 GiB"
        );
        // All-Na / empty → N/A; a non-whole size keeps one decimal.
        assert_eq!(dimm_summary(&[Section::na(NaReason::NotApplicable)], &[], true), "N/A");
        assert_eq!(dimm_summary(&[], &[], true), "N/A");
        assert_eq!(dimm_summary(&[Section::Value(4.5)], &[], true), "1x4.5 GiB");
        // The GB knob: 16 GiB → 17.2 GB per group.
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[], false),
            "2x17.2 GB"
        );
        // The rank word groups with the size.
        let single = module(Section::Value(1), Some(3200));
        let dual = module(Section::Value(2), Some(3200));
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(16.0)],
                &[single.clone(), single.clone()],
                true,
            ),
            "2x16 GiB Single-Rank"
        );
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[single, dual], true),
            "1x16 GiB Single-Rank + 1x16 GiB Dual-Rank"
        );
        // A `Na` rank omits the word: the ranked + unranked pair splits
        // into two groups (first-seen order).
        let na_rank = module(Section::na(NaReason::NotApplicable), Some(3200));
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(16.0)],
                &[module(Section::Value(1), Some(3200)), na_rank],
                true,
            ),
            "1x16 GiB Single-Rank + 1x16 GiB"
        );
    }

    /// (o) `channel_mode`: 1 / 2 / 4 → Single / Dual / Quad, every
    /// other count (0, odd) → `N/A`.
    #[test]
    fn channel_mode_from_dimm_count() {
        assert_eq!(channel_mode(1), "Single-Channel");
        assert_eq!(channel_mode(2), "Dual-Channel");
        assert_eq!(channel_mode(4), "Quad-Channel");
        assert_eq!(channel_mode(0), "N/A");
        assert_eq!(channel_mode(3), "N/A");
    }

    /// (p) `slot_note`: the total-vs-breakdown note — total > SPD sum
    /// with a uniform module size near-dividing the total → `<visible>
    /// of <n> slots SPD-visible`; a non-integer quotient → the generic
    /// note; total ≤ sum, a `Na` total, or no visible modules → `None`
    /// (no false alarm).
    #[test]
    fn slot_note_arms() {
        // The live-host shape: 4×16 GiB installed (62.68 GiB OS total),
        // 2 bound to the SPD bus (32 GiB sum).
        assert_eq!(
            slot_note(&Section::Value(62.68), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("2 of 4 slots SPD-visible".to_owned())
        );
        // An exact multiple (no reserved fraction): 3 slots, 2 visible.
        assert_eq!(
            slot_note(&Section::Value(48.0), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("2 of 3 slots SPD-visible".to_owned())
        );
        // A non-integer quotient (50/16 = 3.125) → the generic note.
        assert_eq!(
            slot_note(&Section::Value(50.0), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("SPD sees 2 of the installed capacity".to_owned())
        );
        // total ≤ sum → no note.
        assert_eq!(
            slot_note(&Section::Value(32.0), &[Section::Value(16.0), Section::Value(16.0)]),
            None
        );
        assert_eq!(
            slot_note(&Section::Value(16.0), &[Section::Value(16.0), Section::Value(16.0)]),
            None
        );
        // A `Na` total → no note; no visible modules → no note.
        assert_eq!(
            slot_note(&Section::na(NaReason::NotApplicable), &[Section::Value(16.0)]),
            None
        );
        assert_eq!(
            slot_note(&Section::Value(62.68), &[Section::na(NaReason::NotApplicable)]),
            None
        );
    }

    /// (q) line 3's composed non-mode text ([`ram_line_prefix`]):
    /// populated (the total + breakdown + speed + channel + the
    /// `Mode: ` lead-in), the speed omitted when no module carries
    /// one, the single-Na-DIMM and all-Na degradations, the GB knob,
    /// and the live-host shape (the rank word + the slot note).
    #[test]
    fn ram_line_prefix_populated_and_degraded() {
        let t = telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![module(Section::na(NaReason::NotApplicable), Some(3200))],
        );
        assert_eq!(
            ram_line_prefix(&t, true),
            "RAM: 32 GiB (2x16 GiB) 3200 MT/s | Dual-Channel | Mode: "
        );
        // No module carries a speed: the segment is omitted entirely.
        let no_speed = telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![module(Section::na(NaReason::NotApplicable), None)],
        );
        assert_eq!(
            ram_line_prefix(&no_speed, true),
            "RAM: 32 GiB (2x16 GiB) | Dual-Channel | Mode: "
        );
        // A single Na DIMM still counts as one bound module (the
        // channel is the count, not the sizes): Single-Channel.
        let degraded = telemetry(
            Section::na(NaReason::NotApplicable),
            vec![Section::na(NaReason::NotApplicable)],
            Vec::new(),
        );
        assert_eq!(
            ram_line_prefix(&degraded, true),
            "RAM: N/A (N/A) | Single-Channel | Mode: "
        );
        // No bound modules at all: every segment degrades to N/A.
        let empty = telemetry(Section::na(NaReason::NotApplicable), Vec::new(), Vec::new());
        assert_eq!(
            ram_line_prefix(&empty, true),
            "RAM: N/A (N/A) | N/A | Mode: "
        );
        // The GB knob: 32 GiB → 34.4 GB total, 16 GiB → 17.2 GB per
        // group.
        assert_eq!(
            ram_line_prefix(&t, false),
            "RAM: 34.4 GB (2x17.2 GB) 3200 MT/s | Dual-Channel | Mode: "
        );
        // The live-host shape: single-rank modules + the OS total
        // (62.68 GiB) above the SPD sum (32 GiB) → the rank word in
        // the breakdown + the slot-note segment.
        let single = module(Section::Value(1), Some(3200));
        let ranked = telemetry(
            Section::Value(62.68),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![single.clone(), single],
        );
        assert_eq!(
            ram_line_prefix(&ranked, true),
            "RAM: 62.7 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | 2 of 4 slots SPD-visible | Mode: "
        );
        // The rank word with total ≤ the SPD sum: no note (no false
        // alarm).
        let single2 = module(Section::Value(1), Some(3200));
        let balanced = telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![single2.clone(), single2],
        );
        assert_eq!(
            ram_line_prefix(&balanced, true),
            "RAM: 32 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | Mode: "
        );
    }

    /// (r) the sync-mode segment matrix: 1:1 with an MCLK (AMBER; the
    /// MCLK in the selected clock unit — the GHz knob ÷1000), 1:1
    /// without (AMBER, the bare text), 1:2 (CRIMSON), a Na ratio (the
    /// honest N/A, DIM), and the Na-AMD-branch degradation (independent
    /// of the clock unit).
    #[test]
    fn sync_mode_matrix() {
        assert_eq!(
            sync_mode_from_clocks(
                &clocks_with(Section::Value(DivMode::OneToOne), Section::Value(1800.0)),
                true,
            ),
            ("Synchronous 1:1 (UCLK = MCLK = 1800 MHz)".to_owned(), AMBER)
        );
        assert_eq!(
            sync_mode_from_clocks(
                &clocks_with(Section::Value(DivMode::OneToOne), Section::na(NaReason::NotApplicable)),
                true,
            ),
            ("Synchronous 1:1".to_owned(), AMBER)
        );
        assert_eq!(
            sync_mode_from_clocks(
                &clocks_with(Section::Value(DivMode::OneToTwo), Section::Value(1800.0)),
                true,
            ),
            ("Asynchronous 1:2".to_owned(), CRIMSON)
        );
        assert_eq!(
            sync_mode_from_clocks(
                &clocks_with(Section::na(NaReason::NotApplicable), Section::Value(1800.0)),
                true,
            ),
            ("N/A".to_owned(), DIM)
        );
        // The GHz knob: 1800 MHz → 1.8 GHz.
        assert_eq!(
            sync_mode_from_clocks(
                &clocks_with(Section::Value(DivMode::OneToOne), Section::Value(1800.0)),
                false,
            ),
            ("Synchronous 1:1 (UCLK = MCLK = 1.8 GHz)".to_owned(), AMBER)
        );
        // A Na AMD branch (Intel silicon / the driver missing) degrades
        // the whole segment (independent of the clock unit).
        let t = telemetry(Section::Value(32.0), Vec::new(), Vec::new());
        assert_eq!(sync_mode(&t, true), ("N/A".to_owned(), DIM));
        assert_eq!(sync_mode(&t, false), ("N/A".to_owned(), DIM));
    }

    /// (s) line 3 end-to-end over the composed spans: the unit knobs
    /// apply (the GB capacity + the GHz clock unit) and the mode
    /// segment colors (the 1:1 amber, the 1:2 crimson) — asserted
    /// through the rendered buffer text of a 1:1 synchronous snapshot.
    #[test]
    fn line3_unit_knobs_and_mode_text() {
        // A 1:1 synchronous AMD readout (the mode segment's MCLK in
        // the selected clock unit).
        let mut state = representative();
        if let Some(ref mut t) = state.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.clocks.div_mode = Section::Value(DivMode::OneToOne);
                readout.clocks.mclk_mhz = Section::Value(1800.0);
            }
        }
        // A wide surface — the composed 1:1 line with the MCLK is
        // ~108 columns, beyond the 100-col default.
        let text = draw_at(&state, 130, 30);
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        assert_eq!(
            lines[2],
            "RAM: 16 GiB (1x16 GiB Dual-Rank) 3200 MT/s | Single-Channel | Mode: Synchronous 1:1 (UCLK = MCLK = 1800 MHz)",
            "{}",
            lines[2]
        );
        // The GB capacity knob + the GHz clock knob.
        state.settings.capacity_gib = false;
        state.settings.clock_mhz = false;
        let text = draw_at(&state, 130, 30);
        let lines = text.split('\n').take(3).collect::<Vec<_>>();
        assert_eq!(
            lines[2],
            "RAM: 17.2 GB (1x17.2 GB Dual-Rank) 3200 MT/s | Single-Channel | Mode: Synchronous 1:1 (UCLK = MCLK = 1.8 GHz)",
            "{}",
            lines[2]
        );
    }

    // ------------------------------------------------------------------
    // TUI-12 — the zone-1 VDDCR_VDD + GEAR_DOWN/CR rows.
    // ------------------------------------------------------------------

    /// (t) The zone-1 clocks section is the GUI's 7-row shape (D-6):
    /// MCLK / UCLK / FCLK / UCLK:MCLK / GEAR_DOWN / CR / PDM (the old
    /// `gear` row and the combined `GDM` row gone), and the voltages
    /// section is the GUI's 5-row shape with the VDDCR_VDD primary rail
    /// first (C12).
    #[test]
    fn zone1_clocks_and_voltages_row_shape() {
        // 100×62: the 55-line AMD readout (incl. the voltages section)
        // needs the taller surface — the 30-row default clips it.
        let text = draw_at(&representative(), 100, 62);
        // Zone 1 occupies the leftmost screen columns: extract its
        // inner segment per row (the text between the first two
        // vertical borders), skipping the frame corners.
        let z1: Vec<String> = text
            .split('\n')
            .filter_map(|line| {
                line.split('│')
                    .nth(1)
                    .map(|segment| segment.trim_end().to_owned())
            })
            .collect();

        let rows_between = |from: &str, to: &str| -> Vec<&str> {
            let start = z1
                .iter()
                .position(|line| line == from)
                .expect("the start subheader must render");
            let end = z1
                .iter()
                .position(|line| line == to)
                .expect("the end marker must render");
            z1[start + 1..end].iter().map(|line| line.as_str()).collect()
        };

        let clocks = rows_between("--- clocks & ratios ---", "--- primary timings ---");
        assert_eq!(
            clocks,
            vec![
                "MCLK: 1600.00 MHz",
                "UCLK: 1600.00 MHz",
                "FCLK: 1800.00 MHz",
                "UCLK:MCLK: 1:2",
                "GEAR_DOWN: Enabled",
                "CR: 1T",
                "PDM: off",
            ],
            "the 7 clocks rows (the GUI's AMD shape, D-6)"
        );

        let voltages = rows_between("--- voltages ---", "Intel: N/A (unsupported hardware)");
        assert_eq!(
            voltages,
            vec![
                "VDDCR_VDD: N/A",
                "VDDCR_SOC: 1.150 V",
                "VDDIO_MEM: 1.350 V",
                "VDD_MISC: 1.100 V",
                "VPP: 1.800 V",
            ],
            "the 5 voltages rows (VDDCR_VDD first, C12)"
        );

        // The old rows are gone entirely (no `gear` label, no `GDM`
        // row — the combined form is split, D-6).
        assert!(!z1.iter().any(|line| line.starts_with("gear:")));
        assert!(!z1.iter().any(|line| line.starts_with("GDM:")));
    }

    /// (u) The VDDCR_VDD row (the C12 frozen wire field — the Vcore
    /// primary rail): a measured value renders in volts (the mV→V
    /// display rule); the Na cell (the Intel-shape cell) degrades to
    /// the GUI's bare `N/A`.
    #[test]
    fn vddcr_vdd_value_and_na() {
        // The fixture's vcore is Na (the Intel-shape cell) -> bare N/A.
        let text = draw_at(&representative(), 100, 62);
        assert!(text.contains("VDDCR_VDD: N/A"), "{text}");

        // A measured Vcore rail: 1150 mV -> 1.150 V.
        let mut state = representative();
        if let Some(ref mut t) = state.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.voltages.vcore_mv = Section::Value(1150);
            }
        }
        let text = draw_at(&state, 100, 62);
        assert!(text.contains("VDDCR_VDD: 1.150 V"), "{text}");
    }

    /// (v) The CR row (the GUI's D-6 form): the command rate's bare
    /// `1T` / `2T`; the Na cell degrades to the bare `N/A`.
    #[test]
    fn cr_value_and_na() {
        // The fixture's command rate is 1T.
        let text = draw_at(&representative(), 100, 62);
        assert!(text.contains("CR: 1T"), "{text}");

        let mut state = representative();
        if let Some(ref mut t) = state.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.clocks.command_rate = Section::Value(CommandRate::TwoT);
            }
        }
        let text = draw_at(&state, 100, 62);
        assert!(text.contains("CR: 2T"), "{text}");

        // The Na degradation (a driver-missing command rate).
        let mut na = representative();
        if let Some(ref mut t) = na.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.clocks.command_rate = Section::na(NaReason::DriverMissing);
            }
        }
        let text = draw_at(&na, 100, 62);
        assert!(text.contains("CR: N/A"), "{text}");
    }

    /// (w) The GEAR_DOWN row (the GUI's D-6 form): the gear-down flag's
    /// bare `Enabled` / `Disabled`; the Na cell degrades to the bare
    /// `N/A` (the value semantics — the `gdm` flag — unchanged).
    #[test]
    fn gear_down_text_matrix() {
        // The fixture carries gdm = true.
        let text = draw_at(&representative(), 100, 62);
        assert!(text.contains("GEAR_DOWN: Enabled"), "{text}");

        let mut off = representative();
        if let Some(ref mut t) = off.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.clocks.gdm = Section::Value(false);
            }
        }
        let text = draw_at(&off, 100, 62);
        assert!(text.contains("GEAR_DOWN: Disabled"), "{text}");

        let mut na = representative();
        if let Some(ref mut t) = na.telemetry {
            if let Section::Value(ref mut readout) = t.amd {
                readout.clocks.gdm = Section::na(NaReason::NotApplicable);
            }
        }
        let text = draw_at(&na, 100, 62);
        assert!(text.contains("GEAR_DOWN: N/A"), "{text}");
    }

    // -----------------------------------------------------------------
    // TUI-13 — the zone-2 pure core (the GUI `bench_zone` C7-17/18
    // mirror): `live_grid`, the cell phases, the flat status line,
    // and the `m:ss` formatter. Headless — no TTY, no I/O.
    // -----------------------------------------------------------------

    /// One synthetic streamed progress event (the GUI `bench_zone`
    /// test helper — the label is unused by the pure core).
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

    /// (x) `live_grid` (the GUI (i) mirror): each streamed event fills
    /// its (tier, op) cell; the latest event per cell wins; the
    /// latency column and the unmeasured cells stay 0.0.
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
        // Unmeasured cells — and the whole latency column (no progress
        // events) — stay 0.0.
        assert_eq!(grid.cell(Tier::Memory, Metric::Copy), 0.0);
        assert_eq!(grid.cell(Tier::L2, Metric::Read), 0.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Copy), 0.0);
        assert_eq!(grid.cell(Tier::L1, Metric::Latency), 0.0);
        assert_eq!(grid.cell(Tier::L3, Metric::Latency), 0.0);
    }

    /// (y) `live_grid` (the GUI (j) mirror): an empty stream and a
    /// stream of malformed (non-finite / non-positive) values never
    /// panic and never render as data — every cell stays 0.0.
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

    /// (z) `cell_phase` (the GUI (k) mirror): not in flight →
    /// `Terminal` for every cell; in flight → `Live` only for the
    /// cells the live grid carries a value for, `NotStarted`
    /// otherwise (a cell not started yet, or a latency cell — no
    /// progress events).
    #[test]
    fn cell_phase_tracks_running_and_streamed_values() {
        let live = live_grid(&[
            progress_event(Tier::Memory, BenchOp::Read, 26.0),
            progress_event(Tier::L1, BenchOp::Copy, 31.8),
        ]);
        // Not in flight: every cell renders its terminal value.
        for &tier in &[Tier::Memory, Tier::L1, Tier::L2, Tier::L3] {
            for &metric in &[Metric::Read, Metric::Write, Metric::Copy, Metric::Latency] {
                assert_eq!(cell_phase(false, &live, tier, metric), CellPhase::Terminal);
            }
        }
        // In flight: the streamed cells are live, the rest not started.
        assert_eq!(cell_phase(true, &live, Tier::Memory, Metric::Read), CellPhase::Live);
        assert_eq!(cell_phase(true, &live, Tier::L1, Metric::Copy), CellPhase::Live);
        assert_eq!(
            cell_phase(true, &live, Tier::Memory, Metric::Write),
            CellPhase::NotStarted
        );
        assert_eq!(cell_phase(true, &live, Tier::L2, Metric::Read), CellPhase::NotStarted);
        assert_eq!(cell_phase(true, &live, Tier::L3, Metric::Latency), CellPhase::NotStarted);
    }

    /// (z′) The cell phases' text + color (the GUI (l) mirror, the
    /// bare terminal-width form): terminal cells keep the existing
    /// two-decimal / `N/A` semantics (a non-finite reading degrades to
    /// `N/A` too — the GUI's guard), live cells show the in-flight
    /// value + a `…` suffix dimmed, not-started cells keep `N/A`.
    #[test]
    fn phase_cell_text_renders_live_and_terminal_cells() {
        let terminal = BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 0.0],
            write_gbps: [43.63, 38.20, 0.0, 14.02],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        };
        let live = live_grid(&[progress_event(Tier::Memory, BenchOp::Read, 26.0)]);

        // Terminal: the existing semantics (a measured cell cyan, an
        // unmeasured cell N/A).
        assert_eq!(
            bench_cell_text(CellPhase::Terminal, &terminal, &live, Tier::Memory, Metric::Read),
            ("26.35".to_owned(), CYAN)
        );
        assert_eq!(
            bench_cell_text(CellPhase::Terminal, &terminal, &live, Tier::Memory, Metric::Latency),
            ("86.84".to_owned(), CYAN)
        );
        assert_eq!(
            bench_cell_text(CellPhase::Terminal, &terminal, &live, Tier::L2, Metric::Write),
            ("N/A".to_owned(), CRIMSON)
        );
        // A non-finite terminal reading degrades to N/A (the GUI's
        // guard — no "NaN" text).
        let broken = BenchmarkGrid {
            read_gbps: [f64::NAN; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        assert_eq!(
            bench_cell_text(CellPhase::Terminal, &broken, &live, Tier::Memory, Metric::Read),
            ("N/A".to_owned(), CRIMSON)
        );

        // Live: the in-flight value + the `…` suffix, dimmed.
        assert_eq!(
            bench_cell_text(CellPhase::Live, &terminal, &live, Tier::Memory, Metric::Read),
            ("26.00…".to_owned(), DIM)
        );
        // A burn-in's `latest` also carries per-tier latency ticks →
        // the live latency cell shows the `…` suffix too.
        let live_lat = BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [86.84, 0.0, 0.0, 0.0],
        };
        assert_eq!(
            bench_cell_text(CellPhase::Live, &terminal, &live_lat, Tier::Memory, Metric::Latency),
            ("86.84…".to_owned(), DIM)
        );

        // NotStarted: the `N/A` placeholder.
        assert_eq!(
            bench_cell_text(CellPhase::NotStarted, &terminal, &live, Tier::L3, Metric::Read),
            ("N/A".to_owned(), CRIMSON)
        );
    }

    /// (aa) `status_state` (the GUI (e) mirror): a burn-in in flight →
    /// `Running` (the newest tick's elapsed + iteration — the
    /// normal-bench flag stays false for a burn-in, C7-16), a normal
    /// bench in flight → `Running` (the zone's start clock's elapsed,
    /// no iteration), a finished run (a terminal grid or kept
    /// streamed progress) → `Done`, the fresh state → `Idle`.
    #[test]
    fn status_state_matrix() {
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
        // false, C7-16).
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
        assert_eq!(status_state(false, false, true, false, 0.0, 0), StatusState::Done);
        // A finished run: the streamed progress is kept with no grid
        // (a cancelled normal bench) → `Done`.
        assert_eq!(status_state(false, false, false, true, 0.0, 0), StatusState::Done);
    }

    /// (ab) `status_text` (the GUI (f) mirror): the flat line's exact
    /// strings — `Status: Idle`, `Status: Running… <m:ss>` (a normal
    /// bench), `Status: Running… (burn-in <m:ss>, iter <n>)` (a
    /// burn-in, C7-18), `Status: Done`.
    #[test]
    fn status_text_renders_the_flat_lines() {
        assert_eq!(status_text(StatusState::Idle), "Status: Idle");
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

    /// (ac) `status_color` (the GUI (g) mirror, the terminal palette):
    /// the idle line is dim, the running line CYAN (a normal bench or
    /// a burn-in), the done line the zone-local green (the §3.2
    /// palette stays frozen).
    #[test]
    fn status_color_tracks_the_state() {
        assert_eq!(status_color(StatusState::Idle), DIM, "the idle line is dim");
        assert_eq!(
            status_color(StatusState::Running {
                elapsed_secs: 42.0,
                burn_in_iteration: None,
            }),
            CYAN
        );
        assert_eq!(
            status_color(StatusState::Running {
                elapsed_secs: 125.0,
                burn_in_iteration: Some(3),
            }),
            CYAN
        );
        assert_eq!(status_color(StatusState::Done), STATUS_DONE);
        assert_eq!(
            STATUS_DONE,
            Color::Rgb(0x2E, 0x9E, 0x5B),
            "the done green is the zone-local const"
        );
    }

    /// (ad) `format_elapsed` (the GUI (d) mirror): the `m:ss` form
    /// (the plan's `0:42`) — the minutes un-padded, the seconds
    /// two-digit; a non-finite / non-positive reading renders `0:00`
    /// (a bad elapsed never panics the line).
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

    /// (ae) `table_live_grid` (the GUI (q) mirror): a burn-in in
    /// flight shows `burn_in.latest` (the cells update per
    /// iteration), a normal bench in flight shows the accumulated
    /// `StreamProgress` events, neither shows the (unused) progress
    /// accumulation.
    #[test]
    fn table_live_grid_selects_the_source() {
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

        // Neither in flight: the progress accumulation (harmless —
        // the terminal grid renders).
        let bench = BenchState::default();
        let live = table_live_grid(&bench);
        assert_eq!(live.cell(Tier::Memory, Metric::Read), 0.0);
    }

    // -----------------------------------------------------------------
    // TUI-14 — the zone-2 render wiring: the controls line + the live
    // burn-in row (the GUI C7-17/18 controls / burn-in-row mirror).
    // Headless — no TTY, no I/O.
    // -----------------------------------------------------------------

    /// The rendered text of a line (its spans concatenated — the test's
    /// flat-text surface for the styled line builders).
    fn line_text(line: &Line<'_>) -> String {
        line.iter().map(|span| span.content.as_ref()).collect()
    }

    /// One span's fg color (the test's style surface for the controls
    /// line's enabled / dimmed entries).
    fn span_fg(line: &Line<'_>, index: usize) -> Color {
        line.iter()
            .nth(index)
            .expect("the span index must exist")
            .style
            .fg
            .expect("every controls span is explicitly styled")
    }

    /// (af) The controls line visibility matrix (the GUI single-flight
    /// rule, TUI-14): the three run keys (`[B] Full`, `[M] Mem`,
    /// `[X] Burn-in(5m)`) are always present; `Cancel` appears only
    /// while a run (a normal bench or a burn-in) is in flight; the run
    /// key spans are CYAN when enabled and DIM while a run is in
    /// flight (the GUI disabled-button form), the `Cancel` key CYAN
    /// whenever it appears.
    #[test]
    fn controls_line_visibility_matrix() {
        // Not in flight: the run keys, no Cancel.
        let idle = BenchState::default();
        assert_eq!(controls_text(&idle), "[B] Full  [M] Mem  [X] Burn-in(5m)");
        assert_eq!(line_text(&controls_line(&idle)), controls_text(&idle));
        assert_eq!(span_fg(&controls_line(&idle), 0), CYAN, "the enabled [B] key is cyan");

        // A normal bench in flight: Cancel appears, the run keys dim.
        let running = BenchState {
            running: true,
            ..Default::default()
        };
        assert_eq!(
            controls_text(&running),
            "[B] Full  [M] Mem  [X] Burn-in(5m)  [C] Cancel"
        );
        let line = controls_line(&running);
        assert_eq!(line_text(&line), controls_text(&running));
        assert_eq!(span_fg(&line, 0), DIM, "the in-flight [B] key dims");
        assert_eq!(span_fg(&line, 6), CYAN, "the in-flight [C] key is cyan");

        // A burn-in in flight: the identical shape (the two run
        // classes share the single-flight rule).
        let burn = BenchState {
            burn_in: BurnInState {
                running: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(controls_text(&burn), controls_text(&running));

        // Over the rendered buffer: the run keys render in the zone;
        // the `Cancel` text appears only while a run is in flight
        // (a wide surface — the in-flight line is 45 columns, beyond
        // the 100-col zone-2 inner width of 30).
        let mut state = AppState::default();
        let text = draw(&state);
        assert!(text.contains("[B] Full"), "{text}");
        assert!(!text.contains("Cancel"), "{text}");
        state.bench = running;
        let text = draw_at(&state, 200, 30);
        assert!(text.contains("[C] Cancel"), "{text}");
    }

    /// (ag) The burn-in row text (the GUI `burn_in_row` mirror,
    /// C7-18 / TUI-14): the newest tick's bookkeeping + the headline
    /// `latest` cells in the plain grid form (the [`bench_cell_text`]
    /// Terminal arm — unit-less, the zone title carries GB/s and the
    /// `ns/hop` header the ns); a cell not yet streamed (0.0 on the
    /// wire) or a non-finite reading reads `N/A`.
    #[test]
    fn burn_in_row_text_and_na_cells() {
        // A burn-in in flight with measured headline cells: the exact
        // GUI form.
        let burn = BurnInState {
            running: true,
            iteration: 2,
            elapsed_secs: 90.0,
            latest: BenchmarkGrid {
                read_gbps: [26.35, 35.10, 0.0, 0.0],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [86.84, 0.0, 0.0, 0.0],
            },
        };
        assert_eq!(
            burn_in_row_text(&burn),
            Some("Burn-in: iteration 2 · 1:30 elapsed · Memory Read 26.35 · 86.84".to_owned())
        );
        // The rendered line carries the same flat text.
        assert_eq!(
            burn_in_row_line(&burn).map(|line| line_text(&line)),
            burn_in_row_text(&burn)
        );

        // A fresh tick (no `latest` cells yet): both headline cells
        // read N/A — the row is present (the burn-in is in flight).
        let fresh = BurnInState {
            running: true,
            iteration: 1,
            elapsed_secs: 5.0,
            ..Default::default()
        };
        assert_eq!(
            burn_in_row_text(&fresh),
            Some("Burn-in: iteration 1 · 0:05 elapsed · Memory Read N/A · N/A".to_owned())
        );

        // A non-finite reading degrades to N/A too (the plain cell
        // form's guard).
        let broken = BurnInState {
            running: true,
            latest: BenchmarkGrid {
                read_gbps: [f64::NAN, 0.0, 0.0, 0.0],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            },
            ..Default::default()
        };
        assert_eq!(
            burn_in_row_text(&broken),
            Some("Burn-in: iteration 0 · 0:00 elapsed · Memory Read N/A · N/A".to_owned())
        );
    }

    /// (ah) The burn-in row's absence (the GUI `burn_in_row` rule,
    /// TUI-14): `None` before a burn-in starts and after it ends (the
    /// row is present only while a burn-in is in flight — the fixed
    /// layout slot stays blank, no shift), and the rendered zone
    /// carries no burn-in text in the idle state.
    #[test]
    fn burn_in_row_absent_without_burn_in() {
        // No burn-in ever: absent.
        assert_eq!(burn_in_row_text(&BurnInState::default()), None);
        assert_eq!(burn_in_row_line(&BurnInState::default()), None);
        // A finished burn-in (the newest tick kept, the run ended):
        // absent too (the row is not a result summary).
        let finished = BurnInState {
            running: false,
            iteration: 12,
            elapsed_secs: 300.0,
            ..Default::default()
        };
        assert_eq!(burn_in_row_text(&finished), None);

        // Over the rendered buffer: the idle zone carries no burn-in
        // text.
        let text = draw(&AppState::default());
        assert!(!text.contains("Burn-in:"), "{text}");
    }

    /// (ai) Clipping at a small height (TUI-14 — the zone-2
    /// constraint grows 5 + 1 + 1 + 1 + slack): the dashboard never
    /// panics as the terminal shrinks. The layout solver keeps the
    /// three line rows (status / controls / burn-in) and trims the
    /// grid's tier rows first (the table keeps its header, the tier
    /// rows drop one by one, the controls line is the last of the
    /// lines to go at the smallest inner); at the full surface all
    /// four zone-2 lines render with the full 5-row grid.
    #[test]
    fn zone2_clips_at_small_height() {
        // An in-flight burn-in (the full zone-2 content: the live
        // grid + the status line + the controls line + the burn-in
        // row) on shrinking surfaces (inner = total - 3 header - 2
        // zone borders).
        let state = AppState {
            bench: BenchState {
                burn_in: BurnInState {
                    running: true,
                    iteration: 3,
                    elapsed_secs: 90.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        // 200×8 (3 inner rows): the grid keeps its header row only
        // (no tier rows — the L1/L2/L3 tier names are absent); the
        // status + burn-in lines render; the controls line is
        // clipped out.
        let text = draw_at(&state, 200, 8);
        assert!(text.contains("2 · BENCH (GB/s)"), "{text}");
        assert!(text.contains("Status: Running…"), "{text}");
        assert!(text.contains("Burn-in: iteration 3"), "{text}");
        assert!(!text.contains("L1"), "{text}");
        assert!(!text.contains("L2"), "{text}");
        assert!(!text.contains("L3"), "{text}");
        assert!(!text.contains("[B] Full"), "{text}");
        // 200×10 (5 inner rows): the grid's first tier row (the
        // Memory tier) renders, the second (L1) clips out; all three
        // lines render.
        let text = draw_at(&state, 200, 10);
        assert!(!text.contains("L1"), "{text}");
        assert!(text.contains("Status: Running…"), "{text}");
        assert!(text.contains("[B] Full"), "{text}");
        assert!(text.contains("Burn-in: iteration 3"), "{text}");
        // 200×13 (8 inner rows): the full 5-row grid + all three
        // lines — the exact 5 + 1 + 1 + 1 fit, every line whole.
        let text = draw_at(&state, 200, 13);
        for tier in ["L1", "L2", "L3"] {
            assert!(text.contains(tier), "{text}");
        }
        assert!(
            text.contains("Status: Running… (burn-in 1:30, iter 3)"),
            "{text}"
        );
        assert!(
            text.contains("[B] Full  [M] Mem  [X] Burn-in(5m)  [C] Cancel"),
            "{text}"
        );
        assert!(
            text.contains("Burn-in: iteration 3 · 1:30 elapsed · Memory Read N/A · N/A"),
            "{text}"
        );
        // The 100-col default surface (30-col zone-2 inner): the
        // lines render width-clipped at the budget (the no-panic clip
        // contract) — the burn-in line's prefix + the controls
        // prefix survive.
        let text = draw(&state);
        assert!(text.contains("Burn-in: iteration 3"), "{text}");
        assert!(text.contains("[B] Full"), "{text}");
    }
}
