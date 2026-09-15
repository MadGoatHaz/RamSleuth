//! P3-23 — the three-zone dashboard renderer (Grand Design §3.1).
//!
//! [`AppState`] is the TUI's presentation state and wraps the wire types
//! **verbatim** (D2 — no duplication): a [`SystemMemoryTelemetry`] snapshot,
//! the benchmark [`BenchState`], the daemon status line, a last-update
//! timestamp, and an optional error. [`render`] draws the dense,
//! non-scrolling dark dashboard into one `ratatui::Frame`:
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
use ramsleuth_telemetry::cpuid::CpuVendor;
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
use ramsleuth_telemetry::SystemMemoryTelemetry;

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
/// `Default` is the not-yet-run state: idle, no progress events, no grid —
/// the dashboard renders a full `N/A` grid + an `idle` line for it.
#[derive(Debug, Clone, Default)]
pub struct BenchState {
    /// A run is in flight (progress events are streaming).
    pub running: bool,
    /// The streamed progress events (latest last; `cell_index` /
    /// `total_cells` count within the run's cell list).
    pub progress: Vec<StreamProgress>,
    /// The terminal grid (`Some` after a completed run; unmeasured cells
    /// carry `0.0` on the wire and render as `N/A`).
    pub grid: Option<BenchmarkGrid>,
}

/// The TUI's presentation state: the reused wire types verbatim (D2) plus
/// the daemon status line.
///
/// P3-24's main loop builds it from `GetTelemetry` / benchmark frames
/// behind an `Arc<RwLock<_>>` and calls [`render`] each tick; `Default` is
/// the not-yet-connected state (no telemetry, idle bench, no error) and
/// renders pure placeholders.
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
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

/// Render the three-zone dashboard into `frame` from `state`.
///
/// The screen splits vertically into a one-row header strip (title + CPU +
/// daemon + key legend) and the three zones side by side. Every zone is a
/// titled `Block` on a slate background; content that does not fit is
/// clipped, never wrapped or scrolled. Safe at any terminal size — the
/// all-`Na`, empty-SPD, daemon-down, and default states all render without
/// panicking.
pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(area);

    frame.render_widget(Paragraph::new(header_line(state)), outer[0]);

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

/// The header strip: `RamSleuth` + the CPU identity + the daemon state +
/// the key legend (one line, no wrap).
fn header_line(state: &AppState) -> Line<'static> {
    let cpu = match &state.telemetry {
        Some(telemetry) => format!(
            "{} · {}",
            vendor_text(&telemetry.cpu.vendor),
            telemetry.cpu.brand
        ),
        None => "cpu: --".to_owned(),
    };
    let (daemon, daemon_color) = if state.error.is_some() {
        let status = if state.daemon_status.is_empty() {
            "down"
        } else {
            state.daemon_status.as_str()
        };
        (format!("daemon: {status}"), CRIMSON)
    } else if state.daemon_status.is_empty() {
        ("daemon: --".to_owned(), DIM)
    } else {
        (format!("daemon: {}", state.daemon_status), DIM)
    };
    Line::from(vec![
        Span::styled("RamSleuth", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::styled(" live memory dashboard", Style::default().fg(DIM)),
        Span::styled(format!("  [{cpu}]"), Style::default().fg(DIM)),
        Span::styled(format!("  [{daemon}]"), Style::default().fg(daemon_color)),
        Span::styled("  [R]efresh [S]napshot [Q]uit", Style::default().fg(DIM)),
    ])
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

/// The CPU vendor as a display string (the snapshot carries vendor +
/// brand only — no family / stepping / feature flags).
fn vendor_text(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Amd(zen) => format!("AMD {zen:?}"),
        CpuVendor::Intel(gen) => format!("Intel {gen:?}"),
        CpuVendor::Unknown => "unknown".to_owned(),
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
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo};
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
                },
                total_capacity: Section::Value(16.0),
                dimm_sizes: vec![Section::Value(16.0)],
            }),
            bench: BenchState::default(),
            daemon_status: "up · /tmp/ramsleuth.sock".to_owned(),
            last_update: Some(Instant::now()),
            error: None,
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
            telemetry: None,
            bench: BenchState::default(),
            daemon_status: String::new(),
            last_update: None,
            error: Some("daemon down: cannot connect to /tmp/x.sock".to_owned()),
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
                },
                total_capacity: Section::na(NaReason::NotApplicable),
                dimm_sizes: Vec::new(),
            }),
            bench: BenchState::default(),
            daemon_status: "up".to_owned(),
            last_update: None,
            error: None,
        };
        // 100×60: zone 1's 27-row surface would clip the second channel
        // block; a taller terminal reaches it.
        let text = draw_at(&state, 100, 60);

        assert!(text.contains("Intel ch 0"), "{text}");
        assert!(text.contains("Intel ch 1"), "{text}");
        assert!(text.contains("1600.00 MHz"), "{text}");
        assert!(text.contains("SPD: N/A"), "{text}");
    }
}
