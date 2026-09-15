//! Zone 1 renderer: the live memory-controller & subtimings matrix
//! (P3-27, regrouped per Grand Design §3.1 in C6-21).
//!
//! Grand Design §3.1 left panel: every cell of the AMD (and Intel, if
//! present) readout as a `label / value` row — clocks & ratios
//! (MCLK / UCLK / FCLK, UCLK:MCLK, gear, GDM / CR, PDM), the 27 DRAM
//! subtimings (primary / secondary / tertiary + turnarounds, ticks),
//! the CAD bus (drive / termination, ohms), the voltages (mV→V) —
//! each [`Section::Value`] printed in CYAN, each [`Section::Na`]
//! printed `N/A (<reason>)` in CRIMSON, and the two semantic warnings
//! in AMBER (a 1:2 UCLK:MCLK divide = gear desync; a SOC rail above
//! 1.30 V = out of spec on AM5).
//!
//! **Grouped layout (C6-21, items 2+3; C7-12 3×2):** the flat
//! single-column grid is now the §3.1 3-column × 2-row matrix —
//! column 1 = `[Clocks & Ratios]` over `[Primary Timings]`,
//! column 2 = `[Secondary Timings]` over `[Tertiary & Turnarounds]`,
//! column 3 = `[CAD Bus Drive & Termination]` over `[Active System
//! Voltages]` (the six sections, related content vertically adjacent)
//! — each section a bold CYAN title row over its own 2-column
//! `egui::Grid` (label/value, the compact 8.0 / 60.0 spacing so the
//! three columns fit the zone width), and the new `GDM / CR` row in
//! `[Clocks & Ratios]` (gear down mode + DRAM command rate — e.g.
//! `Gear 1 / 1T` or `Disabled / 2T`; an Intel channel's
//! not-applicable cells degrade it to a crimson N/A pair). The zone
//! lays out at its natural height — no scroll area: at the default
//! 1400×900 window the 3×2 grid (worst column ≈ 25 rows) fits the
//! left column without vertical scrolling (C7-12); a whole `Na`
//! vendor branch collapses to a single row (`AMD` / `Intel` + the
//! reason), and no telemetry at all renders one crimson placeholder —
//! never a panic (the no-panic contract, plan D5).
//!
//! **Pure core:** [`timing_cells`] is I/O-free and deterministic (the
//! unit tests exercise it without an egui context);
//! [`render_telemetry_zone`] is the thin `egui` surface over it (the
//! live render is verified in the QA phase).

use ramsleuth_telemetry::amd_readout::{
    CadBus, ClockReadout, CommandRate, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::update::TelemetryData;
use crate::{AMBER, CRIMSON, CYAN, SLATE};

/// The zone title (Grand Design §3.1, left panel).
const ZONE_TITLE: &str = "1 · MEMORY CONTROLLER & SUBTIMINGS";
/// The AMD vendor-section label (also the whole-section N/A row's label).
const AMD: &str = "AMD";
/// The Intel vendor-section label (the same role as [`AMD`]).
const INTEL: &str = "Intel";
/// The compact section-grid spacing (C7-12): the 8.0 pt label↔value
/// gap (down from 12.0) + the 1.0 pt row pitch.
const SECTION_SPACING: egui::Vec2 = egui::vec2(8.0, 1.0);
/// The compact section-grid minimum column width (C7-12: down from
/// 80.0 so the three columns fit the zone width at the default size).
const MIN_COL_WIDTH: f32 = 60.0;
/// The gap between the two sections stacked in one 3×2 column
/// (C7-12: down from the old breathing row).
const SECTION_GAP: f32 = 4.0;
/// The 3×2 column map (C7-12): the six stored sections (the §3.1
/// pair-row order — Clocks, Tertiary, Primary, CAD, Secondary,
/// Voltages) read as three columns of two stacked sections: column 1
/// = `[Clocks & Ratios]` over `[Primary Timings]`, column 2 =
/// `[Secondary Timings]` over `[Tertiary & Turnarounds]`, column 3 =
/// `[CAD Bus Drive & Termination]` over `[Active System Voltages]`.
const COLUMN_SECTIONS: [(usize, usize); 3] = [(0, 2), (4, 1), (3, 5)];
/// The AM5 SOC-rail limit (volts): a VDDCR_SOC reading above it is an
/// out-of-spec warning (AMBER).
const SOC_MAX_VOLTS: f64 = 1.30;

/// The six §3.1 section titles, in layout order: the three left-
/// subcolumn sections interleaved with the three right-subcolumn
/// sections (row pair i = left `2i` | right `2i + 1`).
const CLOCK_RATIOS: &str = "Clocks & Ratios";
const TERTIARY_TURNAROUNDS: &str = "Tertiary & Turnarounds";
const PRIMARY_TIMINGS: &str = "Primary Timings";
const CAD_BUS: &str = "CAD Bus Drive & Termination";
const SECONDARY_TIMINGS: &str = "Secondary Timings";
const ACTIVE_VOLTAGES: &str = "Active System Voltages";

// ---------------------------------------------------------------------
// The cell model (the §3.1 grouped layout).
// ---------------------------------------------------------------------

/// One labeled group of key/value rows — a single section of the §3.1
/// 2-subcolumn matrix (e.g. `[Clocks & Ratios]`).
#[derive(Debug, Clone, PartialEq)]
pub struct TimingSection {
    /// The section title (the bold CYAN title row of its 3×2 column,
    /// above the section's own grid).
    pub title: String,
    /// The section's key/value rows, in canonical order.
    pub rows: Vec<(String, String)>,
}

/// One vendor block of the §3.1 matrix: the header label (AMD / Intel
/// ch N) and its six grouped sections, or a single whole-block N/A
/// display when the vendor branch degraded entire.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorTiming {
    /// The block header label (AMD / Intel ch N).
    pub header: String,
    /// The six sections in storage order (the §3.1 pair rows
    /// interleaved: Clocks, Tertiary, Primary, CAD, Secondary,
    /// Voltages — the 3×2 layout reads them by column,
    /// [`COLUMN_SECTIONS`]); empty when the block is degraded.
    pub sections: Vec<TimingSection>,
    /// The whole-block `N/A (<reason>)` display (the degraded state);
    /// `None` when the sections are present.
    pub degraded: Option<String>,
}

// ---------------------------------------------------------------------
// The pure block builder (testable: no I/O, no egui context).
// ---------------------------------------------------------------------

/// The zone-1 vendor blocks, in canonical dump order: the AMD block
/// (a header + the six grouped sections, or one degraded N/A row),
/// then the Intel block (one header + section block per decoded
/// channel, including the channel-level RTL — or one degraded row for
/// an empty / absent readout).
///
/// Pure and deterministic: the same snapshot always yields the same
/// `Vec`. Each row's display is the formatted text of its
/// [`Section::Value`] (two-decimal MHz, one-decimal Ω, three-decimal
/// V, bare ticks, `1:1` / `1:2`, `1x`…`4x`, `on` / `off`,
/// `Gear 1` / `Disabled` + `1T` / `2T`, `RZQ/N (x.x Ω)`) or
/// `N/A (<reason>)` for a [`Section::Na`] (the dump renderer's form —
/// [`NaReason`] carries no `Display`). A vendor branch that degraded
/// whole collapses to one block with empty `sections` + the N/A
/// display (and an Intel readout with no decoded channels does the
/// same with `not applicable`), so an all-Na snapshot still renders
/// the complete matrix and never panics.
pub fn timing_cells(telemetry: &SystemMemoryTelemetry) -> Vec<VendorTiming> {
    let mut blocks: Vec<VendorTiming> = Vec::new();

    match &telemetry.amd {
        Section::Value(readout) => {
            blocks.push(VendorTiming {
                header: AMD.to_owned(),
                sections: readout_sections(
                    &readout.clocks,
                    &readout.timings,
                    &readout.cad_bus,
                    &readout.voltages,
                    None,
                ),
                degraded: None,
            });
        }
        Section::Na(reason) => blocks.push(degraded_block(AMD, reason)),
    }

    match &telemetry.intel {
        Section::Value(readout) => {
            if readout.channels.is_empty() {
                blocks.push(degraded_block(INTEL, &NaReason::NotApplicable));
            } else {
                for channel in &readout.channels {
                    blocks.push(VendorTiming {
                        header: format!("{INTEL} ch {}", channel.index),
                        sections: readout_sections(
                            &channel.clocks,
                            &channel.timings,
                            &channel.cad_bus,
                            &channel.voltages,
                            Some(&channel.rtl),
                        ),
                        degraded: None,
                    });
                }
            }
        }
        Section::Na(reason) => blocks.push(degraded_block(INTEL, reason)),
    }

    blocks
}

/// One degraded vendor block: an empty section list + the whole-block
/// N/A display (the single crimson row the renderer draws beneath the
/// bold header).
fn degraded_block(header: &str, reason: &NaReason) -> VendorTiming {
    VendorTiming {
        header: header.to_owned(),
        sections: Vec::new(),
        degraded: Some(na_text(reason)),
    }
}

/// The six §3.1 sections of one readout (the AMD block or one Intel
/// channel), in storage order (the §3.1 pair rows interleaved):
/// `[Clocks & Ratios]` (MCLK / UCLK / FCLK / UCLK:MCLK / gear /
/// `GDM / CR` / PDM), `[Tertiary & Turnarounds]`, `[Primary Timings]`,
/// `[CAD Bus Drive & Termination]`, `[Secondary Timings]`, `[Active
/// System Voltages]` — the 3×2 layout (C7-12) reads them as columns
/// of two stacked sections ([`COLUMN_SECTIONS`]). The `GDM / CR` row
/// (C6-21, item 3)
/// shows the gear down mode + the DRAM command rate (e.g.
/// `Gear 1 / 1T`, `Disabled / 2T`); an Intel channel's not-applicable
/// GDM + command-rate cells degrade it to a crimson N/A pair. The
/// channel-level RTL (Intel only — the frozen `TimingSet` has no
/// slot) is appended to `[Clocks & Ratios]`.
fn readout_sections(
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad_bus: &CadBus,
    voltages: &VoltageSet,
    rtl: Option<&Section<u16>>,
) -> Vec<TimingSection> {
    // The first three stored sections.
    let mut clocks_rows: Vec<(String, String)> = vec![
        row("MCLK", mhz(&clocks.mclk_mhz)),
        row("UCLK", mhz(&clocks.uclk_mhz)),
        row("FCLK", mhz(&clocks.fclk_mhz)),
        row("UCLK:MCLK", div(&clocks.div_mode)),
        row("gear", gear(&clocks.gear_mode)),
        row("GDM / CR", gdm_cr(&clocks.gdm, &clocks.command_rate)),
        row("PDM", flag(&clocks.pdm)),
    ];
    // Channel-level RTL (Intel only; the frozen `TimingSet` has no slot).
    if let Some(rtl) = rtl {
        clocks_rows.push(row("RTL", ticks(rtl)));
    }

    let primary_rows: Vec<(String, String)> = vec![
        row("tCL", ticks(&timings.cl)),
        row("tRCDWR", ticks(&timings.rcwdwr)),
        row("tRCDRD", ticks(&timings.rcdrd)),
        row("tRP", ticks(&timings.rp)),
    ];

    let secondary_rows: Vec<(String, String)> = vec![
        row("tRAS", ticks(&timings.ras)),
        row("tRC", ticks(&timings.rc)),
        row("tRRDS", ticks(&timings.rrds)),
        row("tRRLD", ticks(&timings.rrld)),
        row("tFAW", ticks(&timings.faw)),
    ];

    // The last three stored sections.
    let tertiary_rows: Vec<(String, String)> = vec![
        row("tWTRS", ticks(&timings.wtrs)),
        row("tWTRL", ticks(&timings.wtrl)),
        row("tWR", ticks(&timings.wr)),
        row("tRFC1", ticks(&timings.rfc1)),
        row("tRFC2", ticks(&timings.rfc2)),
        row("tRFCsb", ticks(&timings.rfcsb)),
        row("tCWL", ticks(&timings.cwl)),
        row("tRTP", ticks(&timings.rtp)),
        row("tRDWR", ticks(&timings.rdwr)),
        row("tWRRD", ticks(&timings.wrrd)),
        row("tRDRD(SD)", ticks(&timings.rdrd_sd)),
        row("tRDRD(CCD)", ticks(&timings.rdrd_dd)),
        row("tRDRD(SCL)", ticks(&timings.rdrd_scl)),
        row("tRDRD(SC)", ticks(&timings.rdrd_sc)),
        row("tWRWR(SD)", ticks(&timings.wrwr_sd)),
        row("tWRWR(CCD)", ticks(&timings.wrwr_dd)),
        row("tWRWR(SCL)", ticks(&timings.wrwr_scl)),
        row("tWRWR(SC)", ticks(&timings.wrwr_sc)),
    ];

    let cad_rows: Vec<(String, String)> = vec![
        row("proc ODT", ohms(&cad_bus.proc_odt)),
        row("RTT nom", rtt(&cad_bus.rtt_nom)),
        row("RTT wr", rtt(&cad_bus.rtt_wr)),
        row("RTT park", rtt(&cad_bus.rtt_park)),
        row("CLK drive", ohms(&cad_bus.clk_drv)),
        row("ADD/CMD drive", ohms(&cad_bus.addr_cmd_drv)),
        row("CS/ODT drive", ohms(&cad_bus.cs_odt_drv)),
        row("CKE drive", ohms(&cad_bus.cke_drv)),
    ];

    let voltage_rows: Vec<(String, String)> = vec![
        row("VDDCR_SOC", volts(&voltages.vddcr_soc_mv)),
        row("VDDIO_MEM", volts(&voltages.vddio_mem_mv)),
        row("VDD_MISC", volts(&voltages.vdd_misc_mv)),
        row("VPP", volts(&voltages.vpp_mv)),
    ];

    vec![
        TimingSection {
            title: CLOCK_RATIOS.to_owned(),
            rows: clocks_rows,
        },
        TimingSection {
            title: TERTIARY_TURNAROUNDS.to_owned(),
            rows: tertiary_rows,
        },
        TimingSection {
            title: PRIMARY_TIMINGS.to_owned(),
            rows: primary_rows,
        },
        TimingSection {
            title: CAD_BUS.to_owned(),
            rows: cad_rows,
        },
        TimingSection {
            title: SECONDARY_TIMINGS.to_owned(),
            rows: secondary_rows,
        },
        TimingSection {
            title: ACTIVE_VOLTAGES.to_owned(),
            rows: voltage_rows,
        },
    ]
}

/// One `(label, display)` row.
fn row(label: &str, display: String) -> (String, String) {
    (label.to_owned(), display)
}

// ---------------------------------------------------------------------
// Cell formatters: one per value kind (the dump renderer's form, so the
// GUI matrix reads exactly like the CLI `dump`).
// ---------------------------------------------------------------------

/// The human text of an absent cell: `N/A (<reason>)` (the renderer
/// owns this form — [`NaReason`] carries no `Display`).
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

/// A memory-clock cell in megahertz (two decimals).
fn mhz(section: &Section<f64>) -> String {
    match section {
        Section::Value(value) => format!("{value:.2} MHz"),
        Section::Na(reason) => na_text(reason),
    }
}

/// A DRAM-subtiming cell in ticks (the plan's display unit).
fn ticks(section: &Section<u16>) -> String {
    match section {
        Section::Value(value) => value.to_string(),
        Section::Na(reason) => na_text(reason),
    }
}

/// A CAD drive / ODT resistance cell (one decimal + Ω).
fn ohms(section: &Section<f64>) -> String {
    match section {
        Section::Value(value) => format!("{value:.1} Ω"),
        Section::Na(reason) => na_text(reason),
    }
}

/// An RTT cell: disabled / RZQ divisor (with the resolved ohms) / ohms.
fn rtt(section: &Section<RttValue>) -> String {
    match section {
        Section::Value(RttValue::Disabled) => "disabled".to_owned(),
        Section::Value(RttValue::Rzq(code)) => match RttValue::Rzq(*code).ohms() {
            Some(value) => format!("RZQ/{code} ({value:.1} Ω)"),
            None => format!("RZQ/{code}"),
        },
        Section::Value(RttValue::Ohms(value)) => format!("{value:.1} Ω"),
        Section::Na(reason) => na_text(reason),
    }
}

/// The UCLK:MCLK divide mode: `1:1` / `1:2`.
fn div(section: &Section<DivMode>) -> String {
    match section {
        Section::Value(DivMode::OneToOne) => "1:1".to_owned(),
        Section::Value(DivMode::OneToTwo) => "1:2".to_owned(),
        Section::Na(reason) => na_text(reason),
    }
}

/// The SA:MEM gear multiplier: `1x` / `2x` / `4x`.
fn gear(section: &Section<GearMode>) -> String {
    match section {
        Section::Value(GearMode::One) => "1x".to_owned(),
        Section::Value(GearMode::Two) => "2x".to_owned(),
        Section::Value(GearMode::Four) => "4x".to_owned(),
        Section::Na(reason) => na_text(reason),
    }
}

/// A boolean mode flag (GDM / PDM): `on` / `off`.
fn flag(section: &Section<bool>) -> String {
    match section {
        Section::Value(true) => "on".to_owned(),
        Section::Value(false) => "off".to_owned(),
        Section::Na(reason) => na_text(reason),
    }
}

/// The `GDM / CR` row (C6-21, item 3): the gear down mode + the DRAM
/// command rate (e.g. `Gear 1 / 1T`, `Disabled / 2T`) — the §3.1
/// mockup row `GDM / CR: Disabled / 1T` (synchronous 1:1, GDM off).
/// GDM on → `Gear 1`, off → `Disabled`; a not-applicable / failed
/// cell degrades its own token to `N/A (<reason>)` (the combined row
/// colors CRIMSON when either token is absent — see [`cell_color`]).
/// No panic on any Na combination.
fn gdm_cr(gdm: &Section<bool>, command_rate: &Section<CommandRate>) -> String {
    let gdm = match gdm {
        Section::Value(true) => "Gear 1".to_owned(),
        Section::Value(false) => "Disabled".to_owned(),
        Section::Na(reason) => na_text(reason),
    };
    let rate = match command_rate {
        Section::Value(CommandRate::OneT) => "1T".to_owned(),
        Section::Value(CommandRate::TwoT) => "2T".to_owned(),
        Section::Na(reason) => na_text(reason),
    };
    format!("{gdm} / {rate}")
}

/// A memory-rail cell in volts (the frozen mV over 1000, three
/// decimals — the plan's mV→V display rule).
fn volts(section: &Section<u16>) -> String {
    match section {
        Section::Value(mv) => format!("{:.3} V", f64::from(*mv) / 1000.0),
        Section::Na(reason) => na_text(reason),
    }
}

// ---------------------------------------------------------------------
// The egui surface (compile-checked here; the live render is verified
// in the QA phase).
// ---------------------------------------------------------------------

/// Zone 1: render the timing matrix from `data` — a titled SLATE
/// frame holding the §3.1 3-column × 2-row section grid per vendor
/// block (C7-12): the zone lays out at its natural height — no
/// scroll area — and at the default 1400×900 window the grid (worst
/// column ≈ 25 rows) fits the left column without vertical scrolling.
///
/// Rows: the label in default text, the value in CYAN, an absent cell
/// (`N/A (…)`) in CRIMSON, and the two warnings in AMBER — a 1:2
/// UCLK:MCLK divide (gear desync) and a VDDCR_SOC reading above 1.30 V
/// (out of spec on AM5). No telemetry renders one crimson placeholder
/// line — never a panic (plan D5).
pub fn render_telemetry_zone(ui: &mut egui::Ui, data: &TelemetryData) {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0));
    let _ = frame.show(ui, |ui| {
        ui.label(egui::RichText::new(ZONE_TITLE).strong().color(CYAN));
        ui.add_space(4.0);
        match &data.telemetry {
            Some(telemetry) => {
                let blocks = timing_cells(telemetry);
                for (index, block) in blocks.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(6.0);
                    }
                    render_vendor_block(ui, index, block);
                }
            }
            None => {
                ui.label(egui::RichText::new("N/A (no telemetry)").color(CRIMSON));
            }
        }
    });
}

/// One vendor block: the bold CYAN header (AMD / Intel ch N) and
/// either the 3-column × 2-row section grid, or the single N/A row
/// of a degraded whole branch (CRIMSON via [`cell_color`]).
fn render_vendor_block(ui: &mut egui::Ui, index: usize, block: &VendorTiming) {
    ui.add(egui::Label::new(
        egui::RichText::new(block.header.as_str()).strong().color(CYAN),
    ));
    match &block.degraded {
        Some(display) => {
            ui.add(egui::Label::new(
                egui::RichText::new(display.as_str()).color(cell_color(&block.header, display)),
            ));
        }
        None => {
            ui.add_space(2.0);
            render_section_grid(ui, index, &block.sections);
        }
    }
}

/// The §3.1 3-column × 2-row body (C7-12): the block's six sections
/// (stored in the pair-row order) read as three columns of two
/// stacked sections each — column 1 = `[Clocks & Ratios]` over
/// `[Primary Timings]`, column 2 = `[Secondary Timings]` over
/// `[Tertiary & Turnarounds]`, column 3 = `[CAD Bus Drive &
/// Termination]` over `[Active System Voltages]` — each section a
/// bold CYAN title row over its own 2-column `egui::Grid` (label /
/// value) with the compact [`SECTION_SPACING`] / [`MIN_COL_WIDTH`],
/// a [`SECTION_GAP`] between the stacked sections, and the columns
/// separated by the surrounding layout's item spacing.
fn render_section_grid(ui: &mut egui::Ui, index: usize, sections: &[TimingSection]) {
    ui.horizontal(|ui| {
        for (column, (top, bottom)) in COLUMN_SECTIONS.iter().enumerate() {
            ui.vertical(|ui| {
                render_section(ui, index, column * 2, sections.get(*top));
                ui.add_space(SECTION_GAP);
                render_section(ui, index, column * 2 + 1, sections.get(*bottom));
            });
        }
    });
}

/// One §3.1 section (C7-12): the bold CYAN title row + its own
/// 2-column `egui::Grid` (label/value), each value in its semantic
/// color ([`cell_color`]); a missing section renders nothing —
/// [`timing_cells`] returns exactly six (or zero, whose degraded path
/// never reaches the grid), so this is the no-panic guard for any
/// future shorter section list.
fn render_section(
    ui: &mut egui::Ui,
    block_index: usize,
    slot: usize,
    section: Option<&TimingSection>,
) {
    let Some(section) = section else {
        return;
    };
    ui.add(egui::Label::new(
        egui::RichText::new(section.title.as_str()).strong().color(CYAN),
    ));
    let _ = egui::Grid::new(format!("ramsleuth_telemetry_section_{block_index}_{slot}"))
        .spacing(SECTION_SPACING)
        .min_col_width(MIN_COL_WIDTH)
        .show(ui, |ui| {
            for (label, display) in &section.rows {
                ui.add(egui::Label::new(egui::RichText::new(label.as_str())));
                ui.add(egui::Label::new(
                    egui::RichText::new(display.as_str())
                        .color(cell_color(label.as_str(), display.as_str())),
                ));
                ui.end_row();
            }
        });
}

/// The semantic color of one grid cell: CYAN for values, CRIMSON for
/// absent cells (`N/A (…)`), AMBER for the two warning conditions — a
/// 1:2 UCLK:MCLK divide (gear desync) and a VDDCR_SOC reading above
/// [`SOC_MAX_VOLTS`] (out of spec on AM5). The combined `GDM / CR`
/// row is CRIMSON when either of its tokens is absent (e.g.
/// `Disabled / N/A (driver missing)`); a fully-absent display already
/// hits the `starts_with("N/A")` pick above.
fn cell_color(label: &str, display: &str) -> egui::Color32 {
    if display.starts_with("N/A") {
        return CRIMSON;
    }
    match label {
        "UCLK:MCLK" if display == "1:2" => AMBER,
        "GDM / CR" if display.contains("N/A") => CRIMSON,
        "VDDCR_SOC" => {
            match display
                .strip_suffix(" V")
                .and_then(|value| value.parse::<f64>().ok())
            {
                Some(volts) if volts > SOC_MAX_VOLTS => AMBER,
                _ => CYAN,
            }
        }
        _ => CYAN,
    }
}

// ---------------------------------------------------------------------
// Tests (headless: `timing_cells` + `cell_color` + `gdm_cr` are pure —
// no egui context, no I/O; the render path runs no-panic in a
// headless `egui::Context` (C7-12) + the row-depth no-scroll
// assertion, with the live render verified in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_telemetry::amd_readout::{
        AmdReadout, CadBus, ClockReadout, CommandRate, DivMode, RttValue, TimingSet, VoltageSet,
    };
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor, IntelGen};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::intel_readout::{decode_channel, IntelReadout};
    use ramsleuth_telemetry::SystemMemoryTelemetry;
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// A representative AMD readout: `Value` cells across all four sets
    /// with a few `Na` cells (mirrors the client / TUI fixtures): the
    /// AMD-not-applicable gear mode, a 1:2 divide (the desync case), one
    /// out-of-band refresh timing, one not-applicable RTT park.
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

    /// A two-channel Intel readout built through the public decode
    /// core: channel 0 fully populated (DDR4-3200 class), channel 1 the
    /// all-Na degradation state (every register absent).
    fn fixture_intel() -> IntelReadout {
        // MCS_COMMAND_0: tCL / tRCD / tRP / tRAS (ticks).
        let cmd0: u32 = 16 | (16 << 8) | (16 << 16) | (32 << 24);
        // MCS_COMMAND_1: 1N command rate + gear 1 + RTL 6 ticks.
        let cmd1: u32 = 6 << 4;
        // MCS_COMMAND_2: tCCD_S / tCCD_L (ticks).
        let cmd2: u32 = 4 | (12 << 8);
        // MCS_COMMAND_3: tRDRD / tRDWR / tWRWR / tWRRD (ticks).
        let cmd3: u32 = 10 | (8 << 8) | (12 << 16) | (4 << 24);
        IntelReadout {
            channels: vec![
                decode_channel(0, Some(160), [Some(cmd0), Some(cmd1), Some(cmd2), Some(cmd3)]),
                decode_channel(1, None, [None; 4]),
            ],
        }
    }

    /// A representative snapshot: a `Value` AMD readout, a `Na` Intel
    /// branch (an AMD host).
    fn representative() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(fixture_amd()),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(32.0),
            dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
        }
    }

    /// An Intel-host snapshot: a `Na` AMD branch, a `Value` two-channel
    /// Intel readout.
    fn intel_populated() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::AlderLake),
                brand: "Intel Core i7-12700K".to_owned(),
            },
            amd: Section::na(NaReason::UnsupportedHardware),
            intel: Section::Value(fixture_intel()),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(32.0),
            dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
        }
    }

    /// The fully degraded snapshot: every branch `Na`, no SPD modules.
    fn all_na() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Unknown".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::InsufficientPrivilege),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// Every key/value row of every section of every block, in layout
    /// order (the flat dump order the assertions read).
    fn all_rows(blocks: &[VendorTiming]) -> Vec<(String, String)> {
        blocks
            .iter()
            .flat_map(|block| block.sections.iter().flat_map(|section| section.rows.iter().cloned()))
            .collect()
    }

    /// The display strings of every row labelled `key` (in order).
    fn displays(rows: &[(String, String)], key: &str) -> Vec<String> {
        rows.iter()
            .filter(|(label, _)| label == key)
            .map(|(_, display)| display.clone())
            .collect()
    }

    /// The §3.1 section titles of one block, in layout order.
    fn section_titles(block: &VendorTiming) -> Vec<String> {
        block.sections.iter().map(|section| section.title.clone()).collect()
    }

    /// (a) A representative snapshot: the §3.1 grouped layout — six
    /// sections in the canonical pair order, the formatted values
    /// (not all N/A) present — clocks, the 1:2 ratio, tick timings,
    /// RZQ Ω, volts — and the new `GDM / CR` row.
    #[test]
    fn representative_cells_carry_formatted_values() {
        let blocks = timing_cells(&representative());
        let rows = all_rows(&blocks);
        assert!(!rows.is_empty(), "the rows must not be empty");
        assert!(
            rows.iter().any(|(_, display)| !display.is_empty() && !display.contains("N/A")),
            "at least one formatted (non-N/A) value is expected: {rows:?}"
        );

        // The §3.1 grouped layout: the six sections in pair order
        // (left 1 | right 1, left 2 | right 2, left 3 | right 3).
        assert_eq!(
            section_titles(&blocks[0]),
            vec![
                "Clocks & Ratios",
                "Tertiary & Turnarounds",
                "Primary Timings",
                "CAD Bus Drive & Termination",
                "Secondary Timings",
                "Active System Voltages",
            ]
        );
        let row_counts: Vec<usize> =
            blocks[0].sections.iter().map(|section| section.rows.len()).collect();
        assert_eq!(
            row_counts,
            vec![7, 18, 4, 8, 5, 4],
            "7 = clocks (+ GDM / CR), 18 = tertiary, 4 = primary, 8 = CAD, 5 = secondary, 4 = voltages"
        );

        assert_eq!(displays(&rows, "MCLK"), vec!["1600.00 MHz"]);
        assert_eq!(displays(&rows, "FCLK"), vec!["1800.00 MHz"]);
        assert_eq!(displays(&rows, "UCLK:MCLK"), vec!["1:2"]);
        assert_eq!(displays(&rows, "gear"), vec!["N/A (not applicable)"]);
        assert_eq!(displays(&rows, "GDM / CR"), vec!["Gear 1 / 1T"]);
        assert_eq!(displays(&rows, "PDM"), vec!["off"]);
        assert_eq!(displays(&rows, "tCL"), vec!["16"]);
        assert_eq!(displays(&rows, "tFAW"), vec!["16"]);
        assert_eq!(displays(&rows, "tRFC2"), vec!["N/A (parse error: fixture)"]);
        assert_eq!(displays(&rows, "RTT nom"), vec!["RZQ/10 (24.0 Ω)"]);
        assert_eq!(displays(&rows, "RTT wr"), vec!["45.0 Ω"]);
        assert_eq!(displays(&rows, "RTT park"), vec!["N/A (not applicable)"]);
        assert_eq!(displays(&rows, "VDDCR_SOC"), vec!["1.150 V"]);
        assert_eq!(displays(&rows, "VPP"), vec!["1.800 V"]);
    }

    /// (b) The fully all-Na snapshot: one degraded block per vendor
    /// branch (empty sections + the whole-block N/A display) — no
    /// panic.
    #[test]
    fn all_na_blocks_degrade_to_single_rows_panic_free() {
        let blocks = timing_cells(&all_na());
        assert_eq!(blocks.len(), 2, "one block per vendor branch");
        assert_eq!(blocks[0].header, "AMD");
        assert!(blocks[0].sections.is_empty());
        assert_eq!(blocks[0].degraded.as_deref(), Some("N/A (driver missing)"));
        assert_eq!(blocks[1].header, "Intel");
        assert!(blocks[1].sections.is_empty());
        assert_eq!(blocks[1].degraded.as_deref(), Some("N/A (insufficient privilege)"));
        assert!(all_rows(&blocks).is_empty(), "a degraded block carries no section rows");
    }

    /// (c) Deterministic: two calls on the same snapshot are equal (all
    /// three fixture shapes).
    #[test]
    fn cells_are_deterministic() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            assert_eq!(timing_cells(&snapshot), timing_cells(&snapshot));
        }
    }

    /// (d) The vendor block headers are always present: a populated
    /// section emits its header, a degraded one its single N/A row
    /// (all three fixture shapes).
    #[test]
    fn vendor_headers_are_always_present() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            let blocks = timing_cells(&snapshot);
            assert!(
                blocks.iter().any(|block| block.header == "AMD"),
                "the AMD block header is expected: {blocks:?}"
            );
            assert!(
                blocks.iter().any(|block| block.header == "Intel" || block.header.starts_with("Intel ch ")),
                "the Intel block header is expected: {blocks:?}"
            );
        }
    }

    /// (e) A populated Intel branch renders per channel: one `Intel ch
    /// N` block per channel, the decoded channel 0 with its readings
    /// (incl. the channel-level RTL appended to `[Clocks & Ratios]`),
    /// the degraded channel 1 all-N/A; the Intel `GDM / CR` row
    /// degrades to a crimson N/A pair (D-C11 not-applicable cells).
    #[test]
    fn intel_blocks_are_per_channel() {
        let blocks = timing_cells(&intel_populated());
        let headers: Vec<String> = blocks.iter().map(|block| block.header.clone()).collect();
        assert_eq!(headers, vec!["AMD", "Intel ch 0", "Intel ch 1"]);

        // Channel 0: the Intel channel-level RTL row (8 clocks rows vs
        // the AMD 7); the rest of the six sections are unchanged.
        let counts: Vec<usize> = blocks[1].sections.iter().map(|s| s.rows.len()).collect();
        assert_eq!(counts, vec![8, 18, 4, 8, 5, 4]);

        let rows = all_rows(&blocks);
        let mclk = displays(&rows, "MCLK");
        // channel 0 decodes; channel 1's frequency-ratio read failed.
        assert_eq!(
            mclk,
            vec![
                "1600.00 MHz",
                "N/A (parse error: DRAM frequency ratio absent (register read failed or unconfigured))",
            ]
        );

        let gear = displays(&rows, "gear");
        assert_eq!(gear[0], "1x", "channel 0 decodes gear 1");
        assert!(gear[1].contains("N/A"), "channel 1 is degraded");

        let rtl = displays(&rows, "RTL");
        assert_eq!(rtl.len(), 2, "one RTL row per channel");
        assert_eq!(rtl[0], "6", "channel 0's decoded RTL (ticks)");
        assert!(rtl[1].contains("N/A"), "channel 1's degraded RTL");

        // The GDM / CR row across all three blocks: the AMD block
        // (the host's Na branch — degraded, no rows), then the two
        // Intel channels' not-applicable N/A pairs.
        let gdm_cr = displays(&rows, "GDM / CR");
        assert_eq!(gdm_cr.len(), 2, "one GDM / CR row per decoded channel");
        assert_eq!(
            gdm_cr[0],
            "N/A (not applicable) / N/A (not applicable)",
            "Intel channel 0's honest D-C11 not-applicable cells"
        );
        assert!(gdm_cr[1].contains("N/A"), "channel 1 is degraded");
    }

    /// (f) The `GDM / CR` combined row: gear down mode + DRAM command
    /// rate (e.g. `Gear 1 / 1T`, `Disabled / 2T`); each absent cell
    /// degrades its own token to `N/A (<reason>)` — no panic on any
    /// Na combination.
    #[test]
    fn gdm_cr_row_formats_the_command_rate() {
        let on = Section::Value(true);
        let off = Section::Value(false);
        let gdm_na: Section<bool> = Section::na(NaReason::NotApplicable);
        let rate_na: Section<CommandRate> = Section::na(NaReason::NotApplicable);
        let one_t = Section::Value(CommandRate::OneT);
        let two_t = Section::Value(CommandRate::TwoT);

        assert_eq!(gdm_cr(&on, &one_t), "Gear 1 / 1T");
        assert_eq!(gdm_cr(&off, &two_t), "Disabled / 2T");
        assert_eq!(gdm_cr(&off, &one_t), "Disabled / 1T");
        assert!(gdm_cr(&gdm_na, &rate_na).starts_with("N/A"));
        assert!(gdm_cr(&off, &rate_na).contains("N/A"));
        assert!(gdm_cr(&gdm_na, &two_t).contains("N/A"));
    }

    /// (g) The semantic color picks (the renderer's cell coloring):
    /// CYAN for values, CRIMSON for N/A, AMBER for the 1:2 divide and
    /// a VDDCR_SOC reading above 1.30 V; the combined `GDM / CR` row
    /// is CRIMSON when either token is absent.
    #[test]
    fn cell_color_semantics() {
        assert_eq!(cell_color("MCLK", "1600.00 MHz"), CYAN);
        assert_eq!(cell_color("tCL", "16"), CYAN);
        assert_eq!(cell_color("MCLK", "N/A (driver missing)"), CRIMSON);
        assert_eq!(cell_color("UCLK:MCLK", "1:1"), CYAN);
        assert_eq!(cell_color("UCLK:MCLK", "1:2"), AMBER);
        assert_eq!(cell_color("VDDCR_SOC", "1.150 V"), CYAN);
        assert_eq!(cell_color("VDDCR_SOC", "1.300 V"), CYAN);
        assert_eq!(cell_color("VDDCR_SOC", "1.450 V"), AMBER);
        assert_eq!(cell_color("VDDCR_SOC", "N/A (not applicable)"), CRIMSON);
        assert_eq!(cell_color("GDM / CR", "Gear 1 / 1T"), CYAN);
        assert_eq!(cell_color("GDM / CR", "Disabled / 2T"), CYAN);
        assert_eq!(cell_color("GDM / CR", "N/A (not applicable) / N/A (not applicable)"), CRIMSON);
        assert_eq!(cell_color("GDM / CR", "Disabled / N/A (driver missing)"), CRIMSON);
    }

    /// The zone's content budget at the default 1400×900 size
    /// (C7-12): the 900 pt window minus the header strip (~70 pt),
    /// the column-row bottom gap (8 pt), the zone frame's inner
    /// margin (2×6 pt), and the zone title row + space (~26 pt)
    /// ≈ 784 pt — the full column, since the history strip's removal
    /// (C7-19) returns its ~165 pt budget to the columns.
    const ZONE_CONTENT_BUDGET: f32 = 900.0 - 70.0 - 8.0 - 12.0 - 26.0;

    /// One headless frame on a fresh context (the `begin_frame`
    /// pattern from the `egui` docs — the fonts load there; the
    /// `history` module's `run_headless_frame` precedent), running
    /// `draw` inside a central panel.
    fn run_headless_frame(draw: impl FnOnce(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| draw(ui));
    }

    /// (h) Row depth (C7-12, the no-scroll gate's unit stand-in): the
    /// worst column of the 3×2 layout — two stacked sections + their
    /// two titles — stays within [`ZONE_CONTENT_BUDGET`] at the
    /// default 1400×900 size. The worst column (5 + 18 rows + 2
    /// titles = 25) at the 18 pt row pitch is ≈ 450 pt (the plan's
    /// number) — no vertical scroll (the QA live gate measures the
    /// rendered window).
    #[test]
    fn worst_column_depth_fits_the_default_size() {
        const ROW_PITCH: f32 = 18.0;
        for snapshot in [representative(), intel_populated(), all_na()] {
            for block in &timing_cells(&snapshot) {
                let depth = if block.sections.is_empty() {
                    1 // a degraded block renders one N/A row
                } else {
                    COLUMN_SECTIONS
                        .iter()
                        .map(|(top, bottom)| {
                            let top_rows = block.sections.get(*top).map_or(0, |s| s.rows.len());
                            let bottom_rows =
                                block.sections.get(*bottom).map_or(0, |s| s.rows.len());
                            top_rows + bottom_rows + 2 // + the two section titles
                        })
                        .max()
                        .unwrap_or(0)
                };
                assert!(
                    depth as f32 * ROW_PITCH <= ZONE_CONTENT_BUDGET,
                    "the worst column ({depth} rows ≈ {} pt) must fit the {ZONE_CONTENT_BUDGET:.0} pt budget",
                    depth as f32 * ROW_PITCH
                );
            }
        }
    }

    /// (i) The render path is no-panic headless (C7-12): every fixture
    /// snapshot (plus the no-telemetry state) renders the zone into a
    /// headless context at its natural height — no scroll area, no
    /// panic — and every non-degraded vendor block's rendered height
    /// (the header + the 3-column grid = the worst column) stays
    /// within [`ZONE_CONTENT_BUDGET`].
    #[test]
    fn renders_headlessly_without_panic_and_within_budget() {
        // The zone's column allocation (main.rs: 55% of the panel
        // width, clamped): 1400 × 0.55 ≈ 770 pt; the full 900 pt
        // height — the zone lays out at its natural height (no
        // scroll area).
        const ZONE_W: f32 = 770.0;
        const ZONE_H: f32 = 900.0;
        for telemetry in [
            Some(representative()),
            Some(intel_populated()),
            Some(all_na()),
            None,
        ] {
            let blocks = telemetry.as_ref().map(timing_cells);
            let data = TelemetryData { telemetry, ..Default::default() };
            run_headless_frame(|ui| {
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(ZONE_W, ZONE_H),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| render_telemetry_zone(ui, &data),
                );
            });
            for block in blocks.iter().flatten() {
                if block.sections.is_empty() {
                    continue; // a degraded block is one row — trivially in budget
                }
                let mut height = 0.0_f32;
                run_headless_frame(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(ZONE_W, ZONE_H),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            render_vendor_block(ui, 0, block);
                            height = ui.cursor().min.y - ui.max_rect().min.y;
                        },
                    );
                });
                assert!(
                    height <= ZONE_CONTENT_BUDGET,
                    "the {header} block renders {height:.1} pt tall — the {ZONE_CONTENT_BUDGET:.0} pt budget",
                    header = block.header
                );
            }
        }
    }
}
