//! Zone 1 renderer: the live memory-controller & subtimings matrix
//! (P3-27, regrouped per Grand Design §3.1 in C6-21).
//!
//! Grand Design §3.1 left panel: every cell of the detected
//! vendor's readout (C7-14: the AMD block on AMD silicon, the Intel
//! per-channel blocks on Intel silicon, both blocks when the vendor
//! is `Unknown`) as a `label / value` row — clocks & ratios
//! (MCLK / UCLK / FCLK, UCLK:MCLK, gear (Intel only), GEAR_DOWN, CR,
//! PDM — the C8-10 `GDM / CR` split, D-6), the 27 DRAM
//! subtimings (primary / secondary / tertiary + turnarounds, ticks),
//! the CAD bus (drive / termination, ohms), the voltages (mV→V) —
//! each [`Section::Value`] printed in CYAN, each [`Section::Na`]
//! printed as bare `N/A` in muted gray (NA_GRAY — the reason stays on
//! the wire, D-4), and the two semantic warnings in AMBER (a 1:2
//! UCLK:MCLK divide = gear desync; a SOC rail above 1.30 V = out of
//! spec on AM5). The MCLK / UCLK / FCLK rows follow
//! the settings panel's clock-unit knob (C7-15): the default `MHz`
//! keeps the two-decimal form, the `GHz` knob re-renders them
//! through [`format_clock`]'s GHz form (÷1000, trimmed).
//!
//! **Grouped layout (C6-21, items 2+3; C7-12 3×2):** the flat
//! single-column grid is now the §3.1 3-column × 2-row matrix —
//! column 1 = `[Clocks & Ratios]` over `[Primary Timings]`,
//! column 2 = `[Secondary Timings]` over `[Tertiary & Turnarounds]`,
//! column 3 = `[CAD Bus Drive & Termination]` over `[Active System
//! Voltages]` (the six sections, related content vertically adjacent)
//! — each section a bold CYAN title row over its own 2-column
//! `egui::Grid` (label/value, the compact 8.0 / 60.0 spacing so the
//! three columns fit the zone width), and the `GEAR_DOWN` + `CR` rows
//! in `[Clocks & Ratios]` (the C8-10 split of the `GDM / CR` row,
//! D-6: the gear down mode `Enabled` / `Disabled` + the DRAM command
//! rate `1T` / `2T`, each its own row; a not-applicable cell degrades
//! its own row to a bare gray N/A) — the `gear` row (the SA:MEM
//! multiplier, an Intel concept) renders only on the Intel channels;
//! the AMD block omits it (D-6). The zone
//! lays out at its natural height — no scroll area: at the default
//! 968×600 window the 3×2 grid (worst column ≈ 25 rows) fits the
//! left column without vertical scrolling (C7-12). The vendor
//! blocks are conditional on the detected CPU (C7-14): an `Amd` host
//! renders only the AMD block, an `Intel` host only the Intel
//! per-channel blocks — the off-vendor bare-`N/A` row is omitted
//! entirely — and an `Unknown` vendor keeps both; a rendered
//! whole-`Na` branch collapses to a single bare-`N/A` row, and no
//! telemetry at all renders one gray placeholder — never a panic
//! (the no-panic contract, plan D5).
//!
//! **Pure core:** [`timing_cells`] is I/O-free and deterministic (the
//! unit tests exercise it without an egui context);
//! [`render_telemetry_zone`] is the thin `egui` surface over it (the
//! live render is verified in the QA phase).

use ramsleuth_telemetry::amd_readout::{
    CadBus, ClockReadout, CommandRate, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::cpuid::CpuVendor;
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::update::TelemetryData;
use crate::{format_clock, ClockUnit, Units, AMBER, CYAN, NA_GRAY, SLATE};

/// The zone title (Grand Design §3.1, left panel).
const ZONE_TITLE: &str = "1 · MEMORY CONTROLLER & SUBTIMINGS";
/// The AMD vendor-section label (also the whole-section N/A row's label).
const AMD: &str = "AMD";
/// The Intel vendor-section label (the same role as [`AMD`]).
const INTEL: &str = "Intel";
/// The compact section-grid spacing (C7-12; C13-03): the 8.0 pt
/// label↔value gap (down from 12.0) + the 0.0 pt row pitch (down
/// from 1.0 — at the 968×600 default the equal-width columns wrap
/// their longest section titles onto a second line, and the zero
/// pitch reclaims that ~23 pt of the worst column, keeping it within
/// the zone's content budget).
const SECTION_SPACING: egui::Vec2 = egui::vec2(8.0, 0.0);
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
    /// The whole-block bare `N/A` display (the degraded state — the
    /// reason stays on the wire, D-4); `None` when the sections are
    /// present.
    pub degraded: Option<String>,
}

// ---------------------------------------------------------------------
// The pure block builder (testable: no I/O, no egui context).
// ---------------------------------------------------------------------

/// The zone-1 vendor blocks, filtered by the detected CPU vendor
/// (C7-14): `CpuVendor::Amd(…)` renders only the AMD block (a
/// header + the six grouped sections, or one degraded bare-`N/A`
/// row — an AMD host with a missing driver still shows
/// its own block, never an Intel one); `CpuVendor::Intel(…)` only
/// the Intel per-channel blocks (one header + section block per
/// decoded channel, including the channel-level RTL — or one
/// degraded row for an empty / absent readout); `CpuVendor::Unknown`
/// both blocks (the vendor-claim-free behavior — each branch
/// degrades to its own single N/A row).
///
/// Pure and deterministic: the same snapshot + clock unit always
/// yields the same `Vec`. Each row's display is the formatted text
/// of its [`Section::Value`] (the clock unit's form — the `MHz`
/// default keeps its two-decimal, the `GHz` knob is
/// [`format_clock`]'s ÷1000-trimmed — one-decimal Ω, three-decimal
/// V, bare ticks, `1:1` / `1:2`, `1x`…`4x` (the `gear` row, Intel
/// only), `on` / `off`, the `GEAR_DOWN` and `CR` row values
/// (`Enabled` / `Disabled`, `1T` / `2T` — the C8-10 split, D-6),
/// `RZQ/N (x.x Ω)`) or
/// bare `N/A` for a [`Section::Na`] (the reason stays on the wire;
/// [`NaReason`] carries no `Display`, and the GUI drops the
/// `(<reason>)` parenthetical — D-4). A rendered vendor branch that
/// degraded whole collapses to one block with empty `sections` + the
/// N/A display (and an Intel readout with no decoded channels does
/// the same with `not applicable`), so an all-Na snapshot still
/// renders the complete matrix and never panics.
pub fn timing_cells(telemetry: &SystemMemoryTelemetry, units: &Units) -> Vec<VendorTiming> {
    // The platform-conditional visibility (C7-14, item 3b): each
    // block renders on its own vendor — the off-vendor branch is
    // omitted even in its degraded bare-`N/A` form
    // — while an `Unknown` vendor (no honest vendor claim) keeps
    // both.
    let show_amd = !matches!(telemetry.cpu.vendor, CpuVendor::Intel(_));
    let show_intel = !matches!(telemetry.cpu.vendor, CpuVendor::Amd(_));
    let mut blocks: Vec<VendorTiming> = Vec::new();

    if show_amd {
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
                        false, // D-6: the AMD block omits the `gear` row
                        units,
                    ),
                    degraded: None,
                });
            }
            Section::Na(reason) => blocks.push(degraded_block(AMD, reason)),
        }
    }

    if show_intel {
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
                                true, // D-6: the Intel channels keep the `gear` row
                                units,
                            ),
                            degraded: None,
                        });
                    }
                }
            }
            Section::Na(reason) => blocks.push(degraded_block(INTEL, reason)),
        }
    }

    blocks
}

/// One degraded vendor block: an empty section list + the whole-block
/// bare N/A display (the single gray row the renderer draws beneath
/// the bold header, D-5).
fn degraded_block(header: &str, reason: &NaReason) -> VendorTiming {
    VendorTiming {
        header: header.to_owned(),
        sections: Vec::new(),
        degraded: Some(na_text(reason)),
    }
}

/// The six §3.1 sections of one readout (the AMD block or one Intel
/// channel), in storage order (the §3.1 pair rows interleaved):
/// `[Clocks & Ratios]` (MCLK / UCLK / FCLK / UCLK:MCLK / gear (Intel
/// only) / `GEAR_DOWN` / `CR` / PDM), `[Tertiary & Turnarounds]`,
/// `[Primary Timings]`, `[CAD Bus Drive & Termination]`, `[Secondary
/// Timings]`, `[Active System Voltages]` — the 3×2 layout (C7-12)
/// reads them as columns of two stacked sections
/// ([`COLUMN_SECTIONS`]). The `GEAR_DOWN` + `CR` rows (C8-10, D-6 —
/// the C6-21 `GDM / CR` combined row, D-7 format C7-13, split into
/// two) show the gear down mode (`Enabled` / `Disabled`) + the DRAM
/// command rate (`1T` / `2T`), each token its own row with a bare
/// value; a not-applicable cell degrades its own row to a bare gray
/// N/A (D-4/D-5). The `gear` row (the SA:MEM multiplier, an Intel
/// concept — the AMD `gear_mode` is always not-applicable) renders
/// only when `gear_row` is `true` (the Intel channels); the AMD
/// block passes `false` and omits it (D-6). The channel-level RTL
/// (Intel only — the frozen `TimingSet` has no slot) is appended to
/// `[Clocks & Ratios]`.
fn readout_sections(
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad_bus: &CadBus,
    voltages: &VoltageSet,
    rtl: Option<&Section<u16>>,
    gear_row: bool,
    units: &Units,
) -> Vec<TimingSection> {
    // The first three stored sections.
    let mut clocks_rows: Vec<(String, String)> = vec![
        row("MCLK", mhz(&clocks.mclk_mhz, units)),
        row("UCLK", mhz(&clocks.uclk_mhz, units)),
        row("FCLK", mhz(&clocks.fclk_mhz, units)),
        row("UCLK:MCLK", div(&clocks.div_mode)),
    ];
    // The SA:MEM gear multiplier row — Intel only (the C8-10
    // `gear_row` vendor param, D-6): the AMD block omits it (its
    // `gear_mode` is always not-applicable — the row would read bare
    // `N/A` on every AMD host).
    if gear_row {
        clocks_rows.push(row("gear", gear(&clocks.gear_mode)));
    }
    // The `GDM / CR` split (C8-10, D-6): the gear down mode + the
    // DRAM command rate, each its own row (the D-7 token names
    // survive as the row labels, the values bare).
    clocks_rows.push(row("GEAR_DOWN", gdm_text(&clocks.gdm)));
    clocks_rows.push(row("CR", cr_text(&clocks.command_rate)));
    clocks_rows.push(row("PDM", flag(&clocks.pdm)));
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

    // The Vcore row (C12: the C12-01 frozen additive `vcore_mv`
    // field, SMU PM table 0x0A0 — Vcore is the primary rail) is
    // prepended before VDDCR_SOC; on Intel it renders bare `N/A`
    // (the `Na(NotApplicable)` cell, D-4 per-row degradation).
    let voltage_rows: Vec<(String, String)> = vec![
        row("VDDCR_VDD", volts(&voltages.vcore_mv)),
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

/// The human text of an absent cell: bare `N/A` (D-4 — the reason
/// stays on the wire; [`NaReason`] carries no `Display`, and the GUI
/// drops the verbose `(<reason>)` parenthetical).
fn na_text(_reason: &NaReason) -> String {
    "N/A".to_owned()
}

/// A memory-clock cell in the selected clock unit (C7-15): the
/// default `MHz` arm keeps the current two-decimal form, the `GHz`
/// arm is [`format_clock`]'s GHz form (the carried MHz ÷ 1000,
/// trimmed — a non-finite value degrades to the honest `N/A`).
fn mhz(section: &Section<f64>, units: &Units) -> String {
    match section {
        Section::Value(value) => match units.clock {
            ClockUnit::MHz => format!("{value:.2} MHz"),
            ClockUnit::GHz => format_clock(*value, units),
        },
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

/// The `GEAR_DOWN` row (C8-10, D-6 — the C6-21 `GDM / CR` combined
/// row, D-7 format C7-13, split into two): the gear down mode's bare
/// value — `Enabled` (gdm `true`) / `Disabled` (gdm `false`); a
/// not-applicable / failed cell degrades its own row to bare `N/A`
/// (the C8-06 form, D-4 — the row colors gray via [`cell_color`]).
/// No panic on any Na.
fn gdm_text(gdm: &Section<bool>) -> String {
    match gdm {
        Section::Value(true) => "Enabled".to_owned(),
        Section::Value(false) => "Disabled".to_owned(),
        Section::Na(reason) => na_text(reason),
    }
}

/// The `CR` row (C8-10, D-6 — the C6-21 `GDM / CR` combined row,
/// D-7 format C7-13, split into two): the DRAM command rate's bare
/// value — `1T` / `2T`; a not-applicable / failed cell degrades its
/// own row to bare `N/A` (the C8-06 form, D-4 — the row colors gray
/// via [`cell_color`]). No panic on any Na.
fn cr_text(command_rate: &Section<CommandRate>) -> String {
    match command_rate {
        Section::Value(CommandRate::OneT) => "1T".to_owned(),
        Section::Value(CommandRate::TwoT) => "2T".to_owned(),
        Section::Na(reason) => na_text(reason),
    }
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
/// scroll area — and at the default 968×600 window the grid (worst
/// column ≈ 25 rows) fits the left column without vertical scrolling.
///
/// Rows: the label in default text, the value in CYAN, an absent cell
/// (bare `N/A`) in muted gray (NA_GRAY — D-5), and the two warnings
/// in AMBER — a 1:2 UCLK:MCLK divide (gear desync) and a VDDCR_SOC
/// reading above 1.30 V (out of spec on AM5); the MCLK / UCLK / FCLK
/// rows follow the
/// settings panel's clock-unit knob (C7-15). No telemetry renders one
/// gray placeholder line — never a panic (plan D5).
pub fn render_telemetry_zone(ui: &mut egui::Ui, data: &TelemetryData) {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0));
    let _ = frame.show(ui, |ui| {
        // D-4a width fill: force the content — and hence this frame's
        // border — to span the full allocated column width; a `Frame`
        // otherwise shrinks to its content's natural width (the
        // 3×2 grid's), leaving dead space to the right of the border.
        ui.set_min_width(ui.available_width());
        ui.label(egui::RichText::new(ZONE_TITLE).strong().color(CYAN));
        ui.add_space(4.0);
        match &data.telemetry {
            Some(telemetry) => {
                let blocks = timing_cells(telemetry, &data.settings.units);
                for (index, block) in blocks.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(6.0);
                    }
                    render_vendor_block(ui, index, block);
                }
            }
            None => {
                ui.label(egui::RichText::new("N/A").color(NA_GRAY));
            }
        }
    });
}

/// One vendor block: the bold CYAN header (AMD / Intel ch N) and
/// either the 3-column × 2-row section grid, or the single N/A row
/// of a degraded whole branch (NA_GRAY via [`cell_color`], D-5).
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

/// The §3.1 3-column × 2-row body (C7-12; equal-width columns
/// D-13.2 / C13-03): the block's six sections (stored in the
/// pair-row order) read as three **equal-width** columns of two
/// stacked sections each — column 1 = `[Clocks & Ratios]` over
/// `[Primary Timings]`, column 2 = `[Secondary Timings]` over
/// `[Tertiary & Turnarounds]`, column 3 = `[CAD Bus Drive &
/// Termination]` over `[Active System Voltages]` — each section a
/// bold CYAN title row over its own 2-column `egui::Grid` (label /
/// value) with the compact [`SECTION_SPACING`] / [`MIN_COL_WIDTH`],
/// a [`SECTION_GAP`] between the stacked sections, and the columns
/// separated by the surrounding layout's item spacing. Each column
/// is allocated `(available − 2·gap) / 3` (D-13.2), so
/// `3·col_w + 2·gap == available` exactly — the three columns
/// span the full parent width and the dead void right of column 3
/// disappears; the row is top-aligned (`Align::TOP`) so unequal-
/// height columns line up at the top, not center-staggered.
fn render_section_grid(ui: &mut egui::Ui, index: usize, sections: &[TimingSection]) {
    // D-13.2: equal-width columns — each of the three columns gets
    // a zero-height allocation of `(available − 2·item_spacing) / 3`;
    // the layout's own item spacing lands the two gaps, so the sum
    // is the full available width exactly.
    let gap = ui.spacing().item_spacing.x;
    let col_w = (ui.available_width() - 2.0 * gap) / 3.0;
    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
        for (column, (top, bottom)) in COLUMN_SECTIONS.iter().enumerate() {
            ui.allocate_ui_with_layout(
                egui::Vec2::new(col_w, 0.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    render_section(ui, index, column * 2, sections.get(*top));
                    ui.add_space(SECTION_GAP);
                    render_section(ui, index, column * 2 + 1, sections.get(*bottom));
                },
            );
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
    // The title wraps (D-13.2): at the 968×600 default each column
    // is ≈ (504 − 20 − 16) / 3 ≈ 156 pt wide, and the `CAD Bus
    // Drive & Termination` title would otherwise paint into the next
    // column.
    ui.add(
        egui::Label::new(
            egui::RichText::new(section.title.as_str()).strong().color(CYAN),
        )
        .wrap(true),
    );
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

/// The semantic color of one grid cell: CYAN for values, muted gray
/// (NA_GRAY) for absent cells (bare `N/A` — D-5: unavailable, not a
/// critical fault), AMBER for the two warning conditions — a 1:2
/// UCLK:MCLK divide (gear desync) and a VDDCR_SOC reading above
/// [`SOC_MAX_VOLTS`] (out of spec on AM5). The split `GEAR_DOWN` /
/// `CR` rows (D-6) degrade their own row to bare `N/A` when their
/// cell is absent — that display hits the `starts_with("N/A")` pick
/// above; their value rows (`Enabled` / `Disabled` / `1T` / `2T`)
/// fall through to CYAN.
fn cell_color(label: &str, display: &str) -> egui::Color32 {
    if display.starts_with("N/A") {
        return NA_GRAY;
    }
    match label {
        "UCLK:MCLK" if display == "1:2" => AMBER,
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
// Tests (headless: `timing_cells` + `cell_color` + `gdm_text` /
// `cr_text` are pure — no egui context, no I/O; the render path runs
// no-panic in a headless `egui::Context` (C7-12) + the row-depth
// no-scroll assertion, with the live render verified in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_telemetry::amd_readout::{
        AmdReadout, CadBus, ClockReadout, CommandRate, DivMode, EccStatus, MemoryChannelMode,
        RttValue, TimingSet, VoltageSet,
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
                vcore_mv: Section::Value(1150),
            },
            channel_mode: MemoryChannelMode::DualSymmetric,
            ecc_status: EccStatus::CapableButDisabled,
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
            channel_mode: None,
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
                smu_version: Section::na(NaReason::NotApplicable),
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
                smu_version: Section::na(NaReason::NotApplicable),
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
                smu_version: Section::na(NaReason::NotApplicable),
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

    /// (a) A representative snapshot: the AMD-detected host renders
    /// exactly one block — the §3.1 grouped layout of the AMD
    /// readout (the off-vendor Intel bare-`N/A` block is omitted,
    /// C7-14) — six sections in the canonical pair
    /// order, the formatted values (not all N/A) present — clocks,
    /// the 1:2 ratio, tick timings, RZQ Ω, volts — and the split
    /// `GEAR_DOWN` / `CR` rows (the AMD `gear` row omitted, D-6).
    #[test]
    fn representative_cells_carry_formatted_values() {
        let blocks = timing_cells(&representative(), &Units::default());
        assert_eq!(
            blocks.len(),
            1,
            "an AMD-detected host renders only the AMD block — the Intel N/A block is omitted (C7-14)"
        );
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
            vec![7, 18, 4, 8, 5, 5],
            "7 = clocks (the `gear` row omitted, `GDM / CR` split into `GEAR_DOWN` + `CR` — net 0, D-6), 18 = tertiary, 4 = primary, 8 = CAD, 5 = secondary, 5 = voltages (the VDDCR_VDD row prepended, C12)"
        );

        assert_eq!(displays(&rows, "MCLK"), vec!["1600.00 MHz"]);
        assert_eq!(displays(&rows, "FCLK"), vec!["1800.00 MHz"]);
        assert_eq!(displays(&rows, "UCLK:MCLK"), vec!["1:2"]);
        // The AMD `gear` row is omitted entirely (D-6): no `gear`
        // label survives in the AMD block.
        assert!(displays(&rows, "gear").is_empty(), "the AMD block omits the `gear` row (D-6)");
        assert_eq!(displays(&rows, "GEAR_DOWN"), vec!["Enabled"]);
        assert_eq!(displays(&rows, "CR"), vec!["1T"]);
        assert_eq!(displays(&rows, "PDM"), vec!["off"]);
        assert_eq!(displays(&rows, "tCL"), vec!["16"]);
        assert_eq!(displays(&rows, "tFAW"), vec!["16"]);
        assert_eq!(displays(&rows, "tRFC2"), vec!["N/A"]);
        assert_eq!(displays(&rows, "RTT nom"), vec!["RZQ/10 (24.0 Ω)"]);
        assert_eq!(displays(&rows, "RTT wr"), vec!["45.0 Ω"]);
        assert_eq!(displays(&rows, "RTT park"), vec!["N/A"]);
        assert_eq!(displays(&rows, "VDDCR_VDD"), vec!["1.150 V"]);
        assert_eq!(displays(&rows, "VDDCR_SOC"), vec!["1.150 V"]);
        // VDDIO_MEM value path (C12-05): the fixture carries
        // `Value(1350)`, and the `volts()` formatter renders `1.350 V`.
        assert_eq!(displays(&rows, "VDDIO_MEM"), vec!["1.350 V"]);
        assert_eq!(displays(&rows, "VPP"), vec!["1.800 V"]);
    }

    /// (b) The fully all-Na snapshot with an `Unknown` vendor: one
    /// degraded block per vendor branch (empty sections + the
    /// whole-block N/A display) — the vendor-claim-free state keeps
    /// both blocks (C7-14) — no panic.
    #[test]
    fn all_na_blocks_degrade_to_single_rows_panic_free() {
        let blocks = timing_cells(&all_na(), &Units::default());
        assert_eq!(
            blocks.len(),
            2,
            "the Unknown vendor keeps both blocks (C7-14)"
        );
        assert_eq!(blocks[0].header, "AMD");
        assert!(blocks[0].sections.is_empty());
        assert_eq!(blocks[0].degraded.as_deref(), Some("N/A"));
        assert_eq!(blocks[1].header, "Intel");
        assert!(blocks[1].sections.is_empty());
        assert_eq!(blocks[1].degraded.as_deref(), Some("N/A"));
        assert!(all_rows(&blocks).is_empty(), "a degraded block carries no section rows");
    }

    /// (c) Deterministic: two calls on the same snapshot are equal (all
    /// three fixture shapes).
    #[test]
    fn cells_are_deterministic() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            assert_eq!(
            timing_cells(&snapshot, &Units::default()),
            timing_cells(&snapshot, &Units::default())
        );
        }
    }

    /// (d) Vendor-conditional block visibility (C7-14): the detected
    /// vendor's block header is always present (a populated section
    /// emits its header, a degraded branch its single N/A row), and
    /// the off-vendor block is omitted entirely — an `Amd` host
    /// renders no Intel block, an `Intel` host no AMD block, an
    /// `Unknown` vendor keeps both.
    #[test]
    fn vendor_blocks_are_conditional_on_the_detected_vendor() {
        // AMD host (a populated AMD branch): the AMD header renders,
        // the off-vendor Intel block — even its degraded N/A form —
        // is omitted.
        let blocks = timing_cells(&representative(), &Units::default());
        assert!(
            blocks.iter().any(|block| block.header == "AMD"),
            "the AMD block header is expected: {blocks:?}"
        );
        assert!(
            !blocks
                .iter()
                .any(|block| block.header == "Intel" || block.header.starts_with("Intel ch ")),
            "an AMD-detected host must not render an Intel block: {blocks:?}"
        );

        // Intel host (a populated two-channel Intel branch): the
        // Intel channel headers render, the off-vendor AMD block is
        // omitted.
        let blocks = timing_cells(&intel_populated(), &Units::default());
        assert!(
            blocks.iter().any(|block| block.header.starts_with("Intel ch ")),
            "the Intel channel block headers are expected: {blocks:?}"
        );
        assert!(
            !blocks.iter().any(|block| block.header == "AMD"),
            "an Intel-detected host must not render an AMD block: {blocks:?}"
        );

        // Unknown vendor (both branches degraded): both blocks
        // render — the current vendor-claim-free behavior.
        let blocks = timing_cells(&all_na(), &Units::default());
        assert!(
            blocks.iter().any(|block| block.header == "AMD"),
            "the AMD block header is expected: {blocks:?}"
        );
        assert!(
            blocks.iter().any(|block| block.header == "Intel"),
            "the Intel block header is expected: {blocks:?}"
        );
    }

    /// (e) A populated Intel branch renders per channel: one `Intel ch
    /// N` block per channel (the off-vendor AMD block is omitted,
    /// C7-14), the decoded channel 0 with its readings (incl. the
    /// channel-level RTL appended to `[Clocks & Ratios]`), the
    /// degraded channel 1 all-N/A; the split `GEAR_DOWN` / `CR` rows
    /// degrade to bare gray N/A (D-C11 not-applicable cells, D-4/D-5)
    /// and the `gear` row survives on the Intel channels (D-6).
    #[test]
    fn intel_blocks_are_per_channel() {
        let blocks = timing_cells(&intel_populated(), &Units::default());
        let headers: Vec<String> = blocks.iter().map(|block| block.header.clone()).collect();
        assert_eq!(
            headers,
            vec!["Intel ch 0", "Intel ch 1"],
            "an Intel-detected host renders only the Intel channel blocks — the AMD block is omitted (C7-14)"
        );

        // Channel 0: the Intel channels keep the `gear` row (D-6) and
        // the `GDM / CR` split adds one row (`GEAR_DOWN` + `CR` vs the
        // single combined row) + the channel-level RTL — 9 clocks
        // rows vs the AMD 7; the voltages section carries the
        // VDDCR_VDD row (C12 — bare `N/A` on Intel, D-4), the rest of
        // the six sections are unchanged.
        let counts: Vec<usize> = blocks[0].sections.iter().map(|s| s.rows.len()).collect();
        assert_eq!(counts, vec![9, 18, 4, 8, 5, 5]);

        let rows = all_rows(&blocks);
        let mclk = displays(&rows, "MCLK");
        // channel 0 decodes; channel 1's frequency-ratio read failed.
        assert_eq!(
            mclk,
            vec![
                "1600.00 MHz",
                "N/A",
            ]
        );

        let gear = displays(&rows, "gear");
        assert_eq!(gear[0], "1x", "channel 0 decodes gear 1");
        assert!(gear[1].contains("N/A"), "channel 1 is degraded");

        let rtl = displays(&rows, "RTL");
        assert_eq!(rtl.len(), 2, "one RTL row per channel");
        assert_eq!(rtl[0], "6", "channel 0's decoded RTL (ticks)");
        assert!(rtl[1].contains("N/A"), "channel 1's degraded RTL");

        // The split GEAR_DOWN / CR rows across the two Intel channel
        // blocks (the off-vendor AMD block is omitted, C7-14): both
        // channels' not-applicable cells degrade their own row to
        // bare `N/A` (D-4).
        let gdm = displays(&rows, "GEAR_DOWN");
        assert_eq!(
            gdm,
            vec!["N/A", "N/A"],
            "one GEAR_DOWN row per channel, both not-applicable (D-C11)"
        );
        let cr = displays(&rows, "CR");
        assert_eq!(
            cr,
            vec!["N/A", "N/A"],
            "one CR row per channel, both not-applicable (D-C11)"
        );

        // VDDIO_MEM Na path (C12-05): the Intel voltage is structurally
        // all-Na, so each VDDIO_MEM row renders bare `N/A` (one per
        // channel) — the all-Na state the C12-04-fed slot degrades to.
        assert_eq!(
            displays(&rows, "VDDIO_MEM"),
            vec!["N/A", "N/A"],
            "one VDDIO_MEM row per channel, both bare N/A (all-Na voltage, D-4)"
        );
    }

    /// (f) The split `GEAR_DOWN` + `CR` rows (D-6): each row carries
    /// its own bare value — the `GEAR_DOWN` value forms `Enabled` /
    /// `Disabled` (gdm `true` / `false`) + the `CR` value forms
    /// `1T` / `2T` (the D-7 token names survive as the row labels) —
    /// and each absent cell degrades its own row to bare `N/A` (D-4,
    /// per row, not the other); no panic on any Na combination.
    #[test]
    fn gdm_and_cr_rows_format_their_own_tokens() {
        let on = Section::Value(true);
        let off = Section::Value(false);
        let gdm_na: Section<bool> = Section::na(NaReason::NotApplicable);
        let rate_na: Section<CommandRate> = Section::na(NaReason::NotApplicable);
        let one_t = Section::Value(CommandRate::OneT);
        let two_t = Section::Value(CommandRate::TwoT);

        // The two value forms per row (the bare D-7 token values).
        assert_eq!(gdm_text(&on), "Enabled");
        assert_eq!(gdm_text(&off), "Disabled");
        assert_eq!(cr_text(&one_t), "1T");
        assert_eq!(cr_text(&two_t), "2T");

        // Per-row N/A degradation: the absent token renders its own
        // row as bare `N/A` (D-4), the other row keeps its value.
        assert_eq!(gdm_text(&gdm_na), "N/A", "the gear-down row degrades on its own");
        assert_eq!(cr_text(&rate_na), "N/A", "the command-rate row degrades on its own");
    }

    /// (g) The semantic color picks (the renderer's cell coloring):
    /// CYAN for values, NA_GRAY for absent cells (bare `N/A`, D-5),
    /// AMBER for the 1:2 divide and a VDDCR_SOC reading above 1.30 V;
    /// the split `GEAR_DOWN` / `CR` rows are NA_GRAY when their cell
    /// is absent (bare `N/A`) and CYAN for their value forms (D-6).
    #[test]
    fn cell_color_semantics() {
        assert_eq!(cell_color("MCLK", "1600.00 MHz"), CYAN);
        assert_eq!(cell_color("tCL", "16"), CYAN);
        assert_eq!(cell_color("MCLK", "N/A"), NA_GRAY);
        assert_eq!(cell_color("UCLK:MCLK", "1:1"), CYAN);
        assert_eq!(cell_color("UCLK:MCLK", "1:2"), AMBER);
        assert_eq!(cell_color("VDDCR_SOC", "1.150 V"), CYAN);
        assert_eq!(cell_color("VDDCR_SOC", "1.300 V"), CYAN);
        assert_eq!(cell_color("VDDCR_SOC", "1.450 V"), AMBER);
        assert_eq!(cell_color("VDDCR_SOC", "N/A"), NA_GRAY);
        assert_eq!(
            cell_color("GEAR_DOWN", "Enabled"),
            CYAN,
            "the gear-down value row is CYAN"
        );
        assert_eq!(
            cell_color("CR", "1T"),
            CYAN,
            "the command-rate value row is CYAN"
        );
        assert_eq!(
            cell_color("GEAR_DOWN", "N/A"),
            NA_GRAY,
            "the gear-down row degrades gray (bare N/A)"
        );
        assert_eq!(
            cell_color("CR", "N/A"),
            NA_GRAY,
            "the command-rate row degrades gray (bare N/A)"
        );
    }

    /// The zone's content budget at the default 968×600 size
    /// (C13-03, re-anchored from C7-12's 1400×900 pin): the 600 pt
    /// window (`DEFAULT_WINDOW_SIZE[1]`) minus the header strip
    /// (~70 pt), the column-row bottom gap (8 pt), the zone frame's
    /// inner margin (2×6 pt), and the zone title row + space (~26 pt)
    /// ≈ 484 pt — the full column, since the history strip's removal
    /// (C7-19) returns its ~165 pt budget to the columns. The 600 pt
    /// window height is `DEFAULT_WINDOW_SIZE[1]` (main.rs, C13-01):
    /// this module is a lib and cannot reference the bin's constant,
    /// so the value is mirrored as a literal.
    const ZONE_CONTENT_BUDGET: f32 = 600.0 - 70.0 - 8.0 - 12.0 - 26.0;

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
    /// default 968×600 size. The worst column (5 + 18 rows + 2
    /// titles = 25) at the 18 pt row pitch is ≈ 450 pt (the plan's
    /// number) — no vertical scroll (the QA live gate measures the
    /// rendered window).
    #[test]
    fn worst_column_depth_fits_the_default_size() {
        const ROW_PITCH: f32 = 18.0;
        for snapshot in [representative(), intel_populated(), all_na()] {
            for block in &timing_cells(&snapshot, &Units::default()) {
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
        // width, clamped to `max_left` = avail − MIN_RIGHT_W −
        // COLUMN_GAP): 952 × 0.55 ≈ 524 pt → 504 pt at the 968 pt
        // default window (right column = MIN_RIGHT_W 440); the full
        // 600 pt height = `DEFAULT_WINDOW_SIZE[1]` (main.rs, C13-01 —
        // mirrored literal: a lib module cannot reference the bin's
        // constant) — the zone lays out at its natural height (no
        // scroll area).
        const ZONE_W: f32 = 504.0;
        const ZONE_H: f32 = 600.0;
        for telemetry in [
            Some(representative()),
            Some(intel_populated()),
            Some(all_na()),
            None,
        ] {
            let blocks = telemetry.as_ref().map(|t| timing_cells(t, &Units::default()));
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

    /// (j) The clock-unit knob (C7-15, item 2c): the MCLK / UCLK /
    /// FCLK rows render in the selected clock unit — the default
    /// `MHz` keeps the current two-decimal form (the `representative`
    /// assertions cover it), the `GHz` knob re-renders the same rows
    /// through [`format_clock`]'s GHz form (the carried MHz ÷ 1000,
    /// trimmed); every other row is unit-agnostic and unchanged under
    /// the knob.
    #[test]
    fn clock_rows_follow_the_units_knob() {
        // The default unit (MHz) keeps the current two-decimal form.
        let blocks = timing_cells(&representative(), &Units::default());
        let rows = all_rows(&blocks);
        assert_eq!(displays(&rows, "MCLK"), vec!["1600.00 MHz"]);
        assert_eq!(displays(&rows, "UCLK"), vec!["1600.00 MHz"]);
        assert_eq!(displays(&rows, "FCLK"), vec!["1800.00 MHz"]);

        // The GHz knob re-renders the clock rows in GHz: 1600 MHz →
        // `1.6 GHz`, 1800 MHz → `1.8 GHz` (format_clock's trim: a
        // whole number drops its decimals, else one decimal).
        let ghz = Units {
            clock: ClockUnit::GHz,
            ..Units::default()
        };
        let blocks = timing_cells(&representative(), &ghz);
        let rows = all_rows(&blocks);
        assert_eq!(displays(&rows, "MCLK"), vec!["1.6 GHz"]);
        assert_eq!(displays(&rows, "UCLK"), vec!["1.6 GHz"]);
        assert_eq!(displays(&rows, "FCLK"), vec!["1.8 GHz"]);

        // The unit-agnostic rows are unchanged under the knob (the AMD
        // `gear` row omitted, D-6; the split `GEAR_DOWN` / `CR` rows
        // carry their bare values).
        assert_eq!(displays(&rows, "UCLK:MCLK"), vec!["1:2"]);
        assert!(displays(&rows, "gear").is_empty(), "the AMD block omits the `gear` row (D-6)");
        assert_eq!(displays(&rows, "GEAR_DOWN"), vec!["Enabled"]);
        assert_eq!(displays(&rows, "CR"), vec!["1T"]);
        assert_eq!(displays(&rows, "PDM"), vec!["off"]);
        assert_eq!(displays(&rows, "tCL"), vec!["16"]);
        assert_eq!(displays(&rows, "VDDCR_SOC"), vec!["1.150 V"]);
    }

    /// (k) The bare N/A form (D-4): every `na_text` arm collapses to
    /// exactly `N/A` (the reason stays on the wire), and no row
    /// display or degraded whole-block display across all three
    /// fixture snapshots carries a `N/A (<reason>)` parenthetical.
    #[test]
    fn na_text_arms_and_all_displays_render_bare() {
        // The six arms of na_text all collapse to the one bare form.
        for reason in [
            NaReason::UnsupportedHardware,
            NaReason::DriverMissing,
            NaReason::InsufficientPrivilege,
            NaReason::UnknownPmTableVersion,
            NaReason::NotApplicable,
            NaReason::ParseError("fixture".to_owned()),
        ] {
            assert_eq!(na_text(&reason), "N/A", "na_text({reason:?}) must be bare (D-4)");
        }

        // The full matrix over all three fixture shapes: any display
        // containing `N/A` is exactly the bare form (no
        // parenthetical survives into the GUI), and the degraded
        // whole-block displays are bare too.
        for snapshot in [representative(), intel_populated(), all_na()] {
            let blocks = timing_cells(&snapshot, &Units::default());
            for (label, display) in all_rows(&blocks) {
                assert!(
                    !display.contains("N/A ("),
                    "the {label} display must not carry an N/A parenthetical (D-4), got {display:?}"
                );
            }
            for block in &blocks {
                if let Some(display) = &block.degraded {
                    assert_eq!(display, "N/A", "the degraded display must be bare (D-4)");
                }
            }
        }
    }

    /// (l) The no-telemetry placeholder renders bare `N/A` in the
    /// muted gray (D-4/D-5 — the `(no telemetry)` parenthetical is
    /// stripped, and the placeholder is unavailable-gray, not
    /// fault-red).
    #[test]
    fn no_telemetry_placeholder_renders_bare_gray_na() {
        let data = TelemetryData { telemetry: None, ..Default::default() };
        let ctx = egui::Context::default();
        ctx.begin_frame(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| render_telemetry_zone(ui, &data));
        let out = ctx.end_frame();
        let texts = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            texts.contains(&"N/A"),
            "the placeholder must paint bare N/A, got {texts:?}"
        );
        assert!(
            texts.iter().all(|t| !t.contains("no telemetry")),
            "no parenthetical survives in the painted text, got {texts:?}"
        );
        // Every section of the placeholder galley is painted in the
        // muted gray (unavailable, not a fault).
        let colors = out
            .shapes
            .iter()
            .filter(|cs| matches!(&cs.shape, egui::Shape::Text(t) if t.galley.text() == "N/A"))
            .flat_map(|cs| match &cs.shape {
                egui::Shape::Text(t) => t
                    .galley
                    .job
                    .sections
                    .iter()
                    .map(|sec| sec.format.color)
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        assert!(!colors.is_empty(), "the placeholder galley must carry sections");
        assert!(
            colors.iter().all(|c| *c == NA_GRAY),
            "the placeholder N/A must be muted gray, got {colors:?}"
        );
    }

    /// (m) The D-4a width fill (the left-column balancing half): the
    /// frame's painted border spans the full available width of the
    /// parent ui. `set_min_width(available_width)` as the first line
    /// inside the frame closure forces the content — and hence this
    /// `Frame`'s border — to the allocated column width; a `Frame`
    /// otherwise shrinks to its content's natural width (the 3×2
    /// grid's, well under the panel width), leaving dead space to
    /// the right of the border. Asserted by rendering the zone into
    /// a bounded central panel and measuring the one painted
    /// CYAN-stroked rect (the frame's border itself — the labels are
    /// `Shape::Text`, so the filter is unique): its width must equal
    /// the panel's available width, for the populated matrix and the
    /// no-telemetry placeholder alike.
    #[test]
    fn render_telemetry_zone_frame_fills_available_width() {
        // A bounded screen (a left-column-shaped allocation, not the
        // default headless size): the central panel's content ui is
        // the parent the frame allocates into.
        let screen = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(600.0, 500.0));
        for telemetry in [Some(representative()), None] {
            let data = TelemetryData { telemetry, ..Default::default() };
            let ctx = egui::Context::default();
            ctx.begin_frame(egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            });
            let mut available = 0.0_f32;
            egui::CentralPanel::default().show(&ctx, |ui| {
                available = ui.available_width();
                assert!(
                    available > 400.0,
                    "a bounded panel to measure against, got {available}"
                );
                render_telemetry_zone(ui, &data);
            });
            let out = ctx.end_frame();
            // The zone's frame is the only CYAN-stroked rect painted.
            let frame_rects: Vec<egui::Rect> = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r) if r.stroke.color == CYAN => Some(r.rect),
                    _ => None,
                })
                .collect();
            assert_eq!(
                frame_rects.len(),
                1,
                "the zone must paint exactly one CYAN-stroked frame rect, got {frame_rects:?}"
            );
            let width = frame_rects[0].width();
            assert!(
                (width - available).abs() < 1.0,
                "the frame border must fill the available width: {width} != {available}"
            );
        }
    }
}
