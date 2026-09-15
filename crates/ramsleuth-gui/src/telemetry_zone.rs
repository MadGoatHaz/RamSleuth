//! Zone 1 renderer: the live memory-controller & subtimings matrix (P3-27).
//!
//! Grand Design §3.1 left panel: every cell of the AMD (and Intel, if
//! present) readout as a `label / value` row of an `egui::Grid` — clocks &
//! ratios (MCLK / UCLK / FCLK, UCLK:MCLK, gear, GDM / PDM), the 27 DRAM
//! subtimings (primary / secondary / tertiary + turnarounds, ticks), the
//! CAD bus (drive / termination, ohms), the voltages (mV→V) — each
//! [`Section::Value`] printed in CYAN, each [`Section::Na`] printed
//! `N/A (<reason>)` in CRIMSON, and the two semantic warnings in AMBER
//! (a 1:2 UCLK:MCLK divide = gear desync; a SOC rail above 1.30 V = out of
//! spec on AM5). The zone sits in a titled, bounded-height
//! `egui::ScrollArea` so it fits the non-scrolling dashboard; a whole
//! `Na` branch collapses to a single row (`AMD` / `Intel` + the reason),
//! and no telemetry at all renders one crimson placeholder — never a
//! panic (the no-panic contract, plan D5).
//!
//! **Pure core:** [`timing_cells`] is I/O-free and deterministic (the unit
//! tests exercise it without an egui context); [`render_telemetry_zone`]
//! is the thin `egui` surface over it (the live render is verified in the
//! QA phase).

use ramsleuth_telemetry::amd_readout::{
    CadBus, ClockReadout, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
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
/// The bounded height (pixels) of the zone's scroll area: the dashboard
/// is non-scrolling, so the matrix scrolls inside this bound.
const ZONE_MAX_HEIGHT: f32 = 420.0;
/// The AM5 SOC-rail limit (volts): a VDDCR_SOC reading above it is an
/// out-of-spec warning (AMBER).
const SOC_MAX_VOLTS: f64 = 1.30;

// ---------------------------------------------------------------------
// The pure cell builder (testable: no I/O, no egui context).
// ---------------------------------------------------------------------

/// The zone-1 content cells, in canonical dump order: the AMD section
/// (a header row, then clocks & ratios, the 27 subtimings, the CAD bus,
/// the voltages), then the Intel section (one header row + field block
/// per decoded channel, including the channel-level RTL).
///
/// Pure and deterministic: the same snapshot always yields the same
/// `Vec`. Each cell's display is the formatted text of its
/// [`Section::Value`] (two-decimal MHz, one-decimal Ω, three-decimal V,
/// bare ticks, `1:1` / `1:2`, `1x`…`4x`, `on` / `off`, `RZQ/N (x.x Ω)`)
/// or `N/A (<reason>)` for a [`Section::Na`] (the dump renderer's form —
/// [`NaReason`] carries no `Display`). A section that degraded whole
/// collapses to one `(AMD | Intel, "N/A (<reason>)")` row (and an Intel
/// readout with no decoded channels does the same with `not
/// applicable`), so an all-Na snapshot still renders the complete matrix
/// and never panics. A cell with an **empty display** is a section /
/// channel **header** row (rendered bold by [`render_telemetry_zone`]).
pub fn timing_cells(telemetry: &SystemMemoryTelemetry) -> Vec<(String, String)> {
    let mut cells: Vec<(String, String)> = Vec::new();

    match &telemetry.amd {
        Section::Value(readout) => {
            cells.push((AMD.to_owned(), String::new())); // the section header row
            push_readout(
                &mut cells,
                &readout.clocks,
                &readout.timings,
                &readout.cad_bus,
                &readout.voltages,
                None,
            );
        }
        Section::Na(reason) => cells.push((AMD.to_owned(), na_text(reason))),
    }

    match &telemetry.intel {
        Section::Value(readout) => {
            if readout.channels.is_empty() {
                cells.push((INTEL.to_owned(), na_text(&NaReason::NotApplicable)));
            } else {
                for channel in &readout.channels {
                    cells.push((format!("{INTEL} ch {}", channel.index), String::new()));
                    push_readout(
                        &mut cells,
                        &channel.clocks,
                        &channel.timings,
                        &channel.cad_bus,
                        &channel.voltages,
                        Some(&channel.rtl),
                    );
                }
            }
        }
        Section::Na(reason) => cells.push((INTEL.to_owned(), na_text(reason))),
    }

    cells
}

/// Appends one readout's field rows (the AMD branch or one Intel
/// channel) in canonical order: clocks & ratios, the primary /
/// secondary / tertiary + turnaround timings, the CAD drive /
/// termination, the voltages, and (Intel only) the channel-level RTL.
fn push_readout(
    cells: &mut Vec<(String, String)>,
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad_bus: &CadBus,
    voltages: &VoltageSet,
    rtl: Option<&Section<u16>>,
) {
    // Clocks & ratios (MHz).
    push(cells, "MCLK", mhz(&clocks.mclk_mhz));
    push(cells, "UCLK", mhz(&clocks.uclk_mhz));
    push(cells, "FCLK", mhz(&clocks.fclk_mhz));
    push(cells, "UCLK:MCLK", div(&clocks.div_mode));
    push(cells, "gear", gear(&clocks.gear_mode));
    push(cells, "GDM", flag(&clocks.gdm));
    push(cells, "PDM", flag(&clocks.pdm));

    // Primary timings (ticks).
    push(cells, "tCL", ticks(&timings.cl));
    push(cells, "tRCDWR", ticks(&timings.rcwdwr));
    push(cells, "tRCDRD", ticks(&timings.rcdrd));
    push(cells, "tRP", ticks(&timings.rp));

    // Secondary timings (ticks).
    push(cells, "tRAS", ticks(&timings.ras));
    push(cells, "tRC", ticks(&timings.rc));
    push(cells, "tRRDS", ticks(&timings.rrds));
    push(cells, "tRRLD", ticks(&timings.rrld));
    push(cells, "tFAW", ticks(&timings.faw));

    // Tertiary & turnarounds (ticks).
    push(cells, "tWTRS", ticks(&timings.wtrs));
    push(cells, "tWTRL", ticks(&timings.wtrl));
    push(cells, "tWR", ticks(&timings.wr));
    push(cells, "tRFC1", ticks(&timings.rfc1));
    push(cells, "tRFC2", ticks(&timings.rfc2));
    push(cells, "tRFCsb", ticks(&timings.rfcsb));
    push(cells, "tCWL", ticks(&timings.cwl));
    push(cells, "tRTP", ticks(&timings.rtp));
    push(cells, "tRDWR", ticks(&timings.rdwr));
    push(cells, "tWRRD", ticks(&timings.wrrd));
    push(cells, "tRDRD(SD)", ticks(&timings.rdrd_sd));
    push(cells, "tRDRD(CCD)", ticks(&timings.rdrd_dd));
    push(cells, "tRDRD(SCL)", ticks(&timings.rdrd_scl));
    push(cells, "tRDRD(SC)", ticks(&timings.rdrd_sc));
    push(cells, "tWRWR(SD)", ticks(&timings.wrwr_sd));
    push(cells, "tWRWR(CCD)", ticks(&timings.wrwr_dd));
    push(cells, "tWRWR(SCL)", ticks(&timings.wrwr_scl));
    push(cells, "tWRWR(SC)", ticks(&timings.wrwr_sc));

    // CAD bus: drive / termination (ohms).
    push(cells, "proc ODT", ohms(&cad_bus.proc_odt));
    push(cells, "RTT nom", rtt(&cad_bus.rtt_nom));
    push(cells, "RTT wr", rtt(&cad_bus.rtt_wr));
    push(cells, "RTT park", rtt(&cad_bus.rtt_park));
    push(cells, "CLK drive", ohms(&cad_bus.clk_drv));
    push(cells, "ADD/CMD drive", ohms(&cad_bus.addr_cmd_drv));
    push(cells, "CS/ODT drive", ohms(&cad_bus.cs_odt_drv));
    push(cells, "CKE drive", ohms(&cad_bus.cke_drv));

    // Voltages (mV → V display).
    push(cells, "VDDCR_SOC", volts(&voltages.vddcr_soc_mv));
    push(cells, "VDDIO_MEM", volts(&voltages.vddio_mem_mv));
    push(cells, "VDD_MISC", volts(&voltages.vdd_misc_mv));
    push(cells, "VPP", volts(&voltages.vpp_mv));

    // Channel-level RTL (Intel only; the frozen `TimingSet` has no slot).
    if let Some(rtl) = rtl {
        push(cells, "RTL", ticks(rtl));
    }
}

/// Appends one `(label, display)` row.
fn push(cells: &mut Vec<(String, String)>, label: &str, display: String) {
    cells.push((label.to_owned(), display));
}

// ---------------------------------------------------------------------
// Cell formatters: one per value kind (the dump renderer's form, so the
// GUI matrix reads exactly like the CLI `dump`).
// ---------------------------------------------------------------------

/// The human text of an absent cell: `N/A (<reason>)` (the renderer owns
/// this form — [`NaReason`] carries no `Display`).
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

/// Zone 1: render the timing matrix from `data` — a titled SLATE frame
/// with a bounded-height vertical scroll area of one `egui::Grid` (it
/// fits the non-scrolling dashboard; the grid scrolls inside the bound).
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
                let cells = timing_cells(telemetry);
                let _ = egui::ScrollArea::vertical()
                    .max_height(ZONE_MAX_HEIGHT)
                    .show(ui, |ui| render_cell_grid(ui, &cells));
            }
            None => {
                ui.label(egui::RichText::new("N/A (no telemetry)").color(CRIMSON));
            }
        }
    });
}

/// The `egui::Grid` body: one row per cell — a cell with an empty
/// display is a single bold section / channel header row, otherwise a
/// default-text label plus its semantic-color value.
fn render_cell_grid(ui: &mut egui::Ui, cells: &[(String, String)]) {
    let _ = egui::Grid::new("ramsleuth_telemetry_zone")
        .spacing(egui::vec2(12.0, 1.0))
        .min_col_width(80.0)
        .show(ui, |ui| {
            for (label, display) in cells {
                if display.is_empty() {
                    ui.add(egui::Label::new(
                        egui::RichText::new(label.as_str()).strong().color(CYAN),
                    ));
                } else {
                    ui.add(egui::Label::new(egui::RichText::new(label.as_str())));
                    ui.add(egui::Label::new(
                        egui::RichText::new(display.as_str())
                            .color(cell_color(label.as_str(), display.as_str())),
                    ));
                }
                ui.end_row();
            }
        });
}

/// The semantic color of one grid cell: CYAN for values, CRIMSON for
/// absent cells (`N/A (…)`), AMBER for the two warning conditions — a
/// 1:2 UCLK:MCLK divide (gear desync) and a VDDCR_SOC reading above
/// [`SOC_MAX_VOLTS`] (out of spec on AM5).
fn cell_color(label: &str, display: &str) -> egui::Color32 {
    if display.starts_with("N/A") {
        return CRIMSON;
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
// Tests (headless: `timing_cells` + `cell_color` are pure — no egui
// context, no I/O; the render path is compile-checked and verified
// live in the QA phase).
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

    /// The display strings of every row labelled `key` (in order).
    fn displays(cells: &[(String, String)], key: &str) -> Vec<String> {
        cells
            .iter()
            .filter(|(label, _)| label == key)
            .map(|(_, display)| display.clone())
            .collect()
    }

    /// (a) A representative snapshot: non-empty, with formatted values
    /// present (not all N/A) — clocks, the 1:2 ratio, tick timings,
    /// RZQ Ω, and volts.
    #[test]
    fn representative_cells_carry_formatted_values() {
        let cells = timing_cells(&representative());
        assert!(!cells.is_empty(), "the cells must not be empty");
        assert!(
            cells.iter().any(|(_, display)| !display.is_empty() && !display.contains("N/A")),
            "at least one formatted (non-N/A) value is expected: {cells:?}"
        );

        assert_eq!(displays(&cells, "MCLK"), vec!["1600.00 MHz"]);
        assert_eq!(displays(&cells, "FCLK"), vec!["1800.00 MHz"]);
        assert_eq!(displays(&cells, "UCLK:MCLK"), vec!["1:2"]);
        assert_eq!(displays(&cells, "gear"), vec!["N/A (not applicable)"]);
        assert_eq!(displays(&cells, "GDM"), vec!["on"]);
        assert_eq!(displays(&cells, "tCL"), vec!["16"]);
        assert_eq!(displays(&cells, "tFAW"), vec!["16"]);
        assert_eq!(displays(&cells, "tRFC2"), vec!["N/A (parse error: fixture)"]);
        assert_eq!(displays(&cells, "RTT nom"), vec!["RZQ/10 (24.0 Ω)"]);
        assert_eq!(displays(&cells, "RTT wr"), vec!["45.0 Ω"]);
        assert_eq!(displays(&cells, "RTT park"), vec!["N/A (not applicable)"]);
        assert_eq!(displays(&cells, "VDDCR_SOC"), vec!["1.150 V"]);
        assert_eq!(displays(&cells, "VPP"), vec!["1.800 V"]);
    }

    /// (b) The fully all-Na snapshot: one row per degraded section and
    /// every display is an `N/A (…)` — no panic.
    #[test]
    fn all_na_cells_are_complete_and_panic_free() {
        let cells = timing_cells(&all_na());
        assert!(!cells.is_empty(), "all-Na must still render the complete matrix");
        assert!(
            cells.iter().all(|(_, display)| display.contains("N/A")),
            "every all-Na cell must carry an N/A display: {cells:?}"
        );
        assert_eq!(displays(&cells, "AMD"), vec!["N/A (driver missing)"]);
        assert_eq!(displays(&cells, "Intel"), vec!["N/A (insufficient privilege)"]);
    }

    /// (c) Deterministic: two calls on the same snapshot are equal (all
    /// three fixture shapes).
    #[test]
    fn cells_are_deterministic() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            assert_eq!(timing_cells(&snapshot), timing_cells(&snapshot));
        }
    }

    /// (d) The vendor section labels are always present: a populated
    /// section emits its header row, a degraded one its single N/A row
    /// (all three fixture shapes).
    #[test]
    fn section_labels_are_always_present() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            let cells = timing_cells(&snapshot);
            assert!(
                cells.iter().any(|(label, _)| label == "AMD"),
                "the AMD section label is expected: {cells:?}"
            );
            assert!(
                cells.iter().any(|(label, _)| label == "Intel" || label.starts_with("Intel ch ")),
                "the Intel section label is expected: {cells:?}"
            );
        }
    }

    /// (e) A populated Intel branch renders per channel: one `Intel ch
    /// N` header row per channel, the decoded channel 0 with its
    /// readings (incl. the channel-level RTL), the degraded channel 1
    /// all-N/A.
    #[test]
    fn intel_cells_are_per_channel() {
        let cells = timing_cells(&intel_populated());
        assert!(cells.iter().any(|(label, _)| label == "Intel ch 0"));
        assert!(cells.iter().any(|(label, _)| label == "Intel ch 1"));

        let mclk = displays(&cells, "MCLK");
        // channel 0 decodes; channel 1's frequency-ratio read failed.
        assert_eq!(
            mclk,
            vec![
                "1600.00 MHz",
                "N/A (parse error: DRAM frequency ratio absent (register read failed or unconfigured))",
            ]
        );

        let gear = displays(&cells, "gear");
        assert_eq!(gear[0], "1x", "channel 0 decodes gear 1");
        assert!(gear[1].contains("N/A"), "channel 1 is degraded");

        let rtl = displays(&cells, "RTL");
        assert_eq!(rtl.len(), 2, "one RTL row per channel");
        assert_eq!(rtl[0], "6", "channel 0's decoded RTL (ticks)");
        assert!(rtl[1].contains("N/A"), "channel 1's degraded RTL");
    }

    /// (f) The semantic color picks (the renderer's cell coloring):
    /// CYAN for values, CRIMSON for N/A, AMBER for the 1:2 divide and
    /// a VDDCR_SOC reading above 1.30 V.
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
    }
}
