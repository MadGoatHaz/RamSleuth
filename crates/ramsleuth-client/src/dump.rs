//! The `dump` command — the dashboard-style telemetry renderer (P3-19).
//!
//! This is the Phase 3 exit-criterion presentation: [`render`] walks one
//! [`SystemMemoryTelemetry`] snapshot and formats **every** cell — a
//! [`Section::Value`] prints its formatted value, a [`Section::Na`] prints
//! `N/A (<reason>)` — so a fully degraded snapshot (the all-Na state on a
//! host without the `ryzen_smu` driver) still renders the complete
//! dashboard and never panics (the no-panic contract, plan D5). It is
//! pure: no I/O, no client, no socket state — a fixed snapshot always
//! yields the same text (the TUI snapshot export and the GUI reuse it).
//!
//! Section layout (Grand Design §3.1):
//!
//! ```text
//! === CPU ===     vendor + brand (the dispatch key of the snapshot)
//! === AMD ===     clocks & ratios (MCLK/UCLK/FCLK, divide mode, gear,
//!                 GDM/PDM), primary / secondary / tertiary + turnaround
//!                 timings (ticks), CAD bus (ohms), voltages (volts)
//! === Intel ===   per-channel IMC decode (the same display sets, plus RTL)
//! === SPD ===     per-slot module (maker, part, serial, rank, density,
//!                 speed) + XMP / EXPO profiles
//! ```
//!
//! A section whose branch degraded to `Na` prints its header followed by a
//! single `N/A (<reason>)` line — the section is never omitted. The SPD
//! section (a module list, not a `Section`) notes an empty list the same
//! way.
//!
//! [`dump`] is the thin RPC half: one `GetTelemetry` round trip over
//! [`Client`], then [`render`] to stdout.

use ramsleuth_protocol::{Request, Response};
use ramsleuth_telemetry::amd_readout::{
    AmdReadout, CadBus, ClockReadout, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::intel_readout::{IntelChannel, IntelReadout};
use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::client::{Client, ClientError};

/// The report title line (ASCII only: the `=` rule below is exactly
/// `len()` wide, so no multi-byte characters in the title).
const TITLE: &str = "RamSleuth - system memory telemetry";

// ---------------------------------------------------------------------------
// Cell formatters: one per value kind. Each renders a [`Section`] cell as
// its formatted value or `N/A (<reason>)` — the no-panic contract (D5).
// ---------------------------------------------------------------------------

/// The human-readable text of an absent cell: `N/A (<reason>)`.
///
/// [`NaReason`] carries no `Display` impl (the telemetry crate shows it via
/// `Debug`), so the renderer owns this compact, user-presentable form.
fn na_cell(reason: &NaReason) -> String {
    match reason {
        NaReason::UnsupportedHardware => "N/A (unsupported hardware)".to_owned(),
        NaReason::DriverMissing => "N/A (driver missing)".to_owned(),
        NaReason::InsufficientPrivilege => "N/A (insufficient privilege)".to_owned(),
        NaReason::UnknownPmTableVersion => "N/A (unknown PM table version)".to_owned(),
        NaReason::NotApplicable => "N/A (not applicable)".to_owned(),
        NaReason::ParseError(detail) => format!("N/A (parse error: {detail})"),
    }
}

/// A bare displayable cell: the formatted value, or `N/A (<reason>)`.
fn cell<T: std::fmt::Display>(section: &Section<T>) -> String {
    match section {
        Section::Value(value) => value.to_string(),
        Section::Na(reason) => na_cell(reason),
    }
}

/// A memory-clock cell in megahertz (two decimals).
fn mhz(section: &Section<f64>) -> String {
    match section {
        Section::Value(value) => format!("{value:.2} MHz"),
        Section::Na(reason) => na_cell(reason),
    }
}

/// An ohm cell (one decimal + Ω).
fn ohms(section: &Section<f64>) -> String {
    match section {
        Section::Value(value) => format!("{value:.1} Ω"),
        Section::Na(reason) => na_cell(reason),
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
        Section::Na(reason) => na_cell(reason),
    }
}

/// The UCLK:MCLK divide mode: `1:1` / `1:2` or `N/A (<reason>)`.
fn div_mode(section: &Section<DivMode>) -> String {
    match section {
        Section::Value(DivMode::OneToOne) => "1:1".to_owned(),
        Section::Value(DivMode::OneToTwo) => "1:2".to_owned(),
        Section::Na(reason) => na_cell(reason),
    }
}

/// The SA:MEM gear multiplier: `1x` / `2x` / `4x` or `N/A (<reason>)`.
fn gear_mode(section: &Section<GearMode>) -> String {
    match section {
        Section::Value(GearMode::One) => "1x".to_owned(),
        Section::Value(GearMode::Two) => "2x".to_owned(),
        Section::Value(GearMode::Four) => "4x".to_owned(),
        Section::Na(reason) => na_cell(reason),
    }
}

/// A boolean mode flag (GDM / PDM): `on` / `off` or `N/A (<reason>)`.
fn flag(section: &Section<bool>) -> String {
    match section {
        Section::Value(true) => "on".to_owned(),
        Section::Value(false) => "off".to_owned(),
        Section::Na(reason) => na_cell(reason),
    }
}

/// A memory-rail cell in volts (the frozen mV value over 1000, three
/// decimals — the plan's mV→V display rule).
fn volts(section: &Section<u16>) -> String {
    match section {
        Section::Value(mv) => {
            let value = f64::from(*mv) / 1000.0;
            format!("{value:.3} V")
        }
        Section::Na(reason) => na_cell(reason),
    }
}

/// A module data-rate cell in megatransfers per second.
fn mts(section: &Section<u16>) -> String {
    match section {
        Section::Value(value) => format!("{value} MT/s"),
        Section::Na(reason) => na_cell(reason),
    }
}

/// A DRAM die density cell in mebibits.
fn density(section: &Section<u16>) -> String {
    match section {
        Section::Value(value) => format!("{value} Mbit"),
        Section::Na(reason) => na_cell(reason),
    }
}

// ---------------------------------------------------------------------------
// Layout helpers: section headers, subgroup headers, aligned key/value rows.
// ---------------------------------------------------------------------------

/// One `=== SECTION ===` header line.
fn header(out: &mut String, title: &str) {
    out.push_str(&format!("=== {title} ===\n"));
}

/// One `  --- subgroup ---` header line (two-space indent).
fn subheader(out: &mut String, title: &str) {
    out.push_str(&format!("  --- {title} ---\n"));
}

/// One `  N/A (<reason>)` line under a section header (two-space indent).
fn na_line(out: &mut String, reason: &NaReason) {
    let text = na_cell(reason);
    out.push_str("  ");
    out.push_str(&text);
    out.push('\n');
}

/// Append one aligned `key: value` row group: every key pads to the group's
/// widest key, two spaces before the value, at the given indent.
fn rows(out: &mut String, indent: &str, pairs: &[(&str, String)]) {
    let key_pad = pairs.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    for (key, value) in pairs {
        out.push_str(&format!("{indent}{key:<key_pad$}  {value}\n", key_pad = key_pad));
    }
}

// ---------------------------------------------------------------------------
// Per-display-set row builders (shared by the AMD and Intel sections).
// ---------------------------------------------------------------------------

/// The seven clocks & ratios rows of a [`ClockReadout`].
fn clocks_rows(clocks: &ClockReadout) -> [(&str, String); 7] {
    [
        ("MCLK", mhz(&clocks.mclk_mhz)),
        ("UCLK", mhz(&clocks.uclk_mhz)),
        ("FCLK", mhz(&clocks.fclk_mhz)),
        ("UCLK:MCLK", div_mode(&clocks.div_mode)),
        ("gear", gear_mode(&clocks.gear_mode)),
        ("GDM", flag(&clocks.gdm)),
        ("PDM", flag(&clocks.pdm)),
    ]
}

/// All 27 DRAM subtiming rows of a [`TimingSet`], in canonical order
/// (primary first, then secondary, then tertiary + turnarounds).
fn timings_rows(timings: &TimingSet) -> [(&str, String); 27] {
    [
        ("tCL", cell(&timings.cl)),
        ("tRCDWR", cell(&timings.rcwdwr)),
        ("tRCDRD", cell(&timings.rcdrd)),
        ("tRP", cell(&timings.rp)),
        ("tRAS", cell(&timings.ras)),
        ("tRC", cell(&timings.rc)),
        ("tRRDS", cell(&timings.rrds)),
        ("tRRLD", cell(&timings.rrld)),
        ("tFAW", cell(&timings.faw)),
        ("tWTRS", cell(&timings.wtrs)),
        ("tWTRL", cell(&timings.wtrl)),
        ("tWR", cell(&timings.wr)),
        ("tRFC1", cell(&timings.rfc1)),
        ("tRFC2", cell(&timings.rfc2)),
        ("tRFCsb", cell(&timings.rfcsb)),
        ("tCWL", cell(&timings.cwl)),
        ("tRTP", cell(&timings.rtp)),
        ("tRDWR", cell(&timings.rdwr)),
        ("tWRRD", cell(&timings.wrrd)),
        ("tRDRD(SD)", cell(&timings.rdrd_sd)),
        ("tRDRD(CCD)", cell(&timings.rdrd_dd)),
        ("tRDRD(SCL)", cell(&timings.rdrd_scl)),
        ("tRDRD(SC)", cell(&timings.rdrd_sc)),
        ("tWRWR(SD)", cell(&timings.wrwr_sd)),
        ("tWRWR(CCD)", cell(&timings.wrwr_dd)),
        ("tWRWR(SCL)", cell(&timings.wrwr_scl)),
        ("tWRWR(SC)", cell(&timings.wrwr_sc)),
    ]
}

/// The eight CAD-bus rows of a [`CadBus`] (ohms + the three RTT fields).
fn cad_rows(cad: &CadBus) -> [(&str, String); 8] {
    [
        ("proc ODT", ohms(&cad.proc_odt)),
        ("RTT nom", rtt(&cad.rtt_nom)),
        ("RTT wr", rtt(&cad.rtt_wr)),
        ("RTT park", rtt(&cad.rtt_park)),
        ("CLK drive", ohms(&cad.clk_drv)),
        ("ADD/CMD drive", ohms(&cad.addr_cmd_drv)),
        ("CS/ODT drive", ohms(&cad.cs_odt_drv)),
        ("CKE drive", ohms(&cad.cke_drv)),
    ]
}

/// The four memory-rail rows of a [`VoltageSet`] (volts).
fn voltages_rows(voltages: &VoltageSet) -> [(&str, String); 4] {
    [
        ("VDDCR_SOC", volts(&voltages.vddcr_soc_mv)),
        ("VDDIO_MEM", volts(&voltages.vddio_mem_mv)),
        ("VDD_MISC", volts(&voltages.vdd_misc_mv)),
        ("VPP", volts(&voltages.vpp_mv)),
    ]
}

// ---------------------------------------------------------------------------
// Sections.
// ---------------------------------------------------------------------------

/// The CPU vendor as a display string (the telemetry `CpuInfo` carries the
/// vendor + brand only — family / stepping / feature flags are not part of
/// the frozen snapshot, so nothing else is printed here).
fn vendor_text(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Amd(zen) => format!("AMD {zen:?}"),
        CpuVendor::Intel(gen) => format!("Intel {gen:?}"),
        CpuVendor::Unknown => "unknown".to_owned(),
    }
}

/// The `=== CPU ===` section: vendor + brand (always populated — the
/// snapshot root carries the detected CPU).
fn cpu_section(out: &mut String, cpu: &CpuInfo) {
    header(out, "CPU");
    rows(out, "  ", &[("vendor", vendor_text(&cpu.vendor)), ("brand", cpu.brand.clone())]);
    out.push('\n');
}

/// The `=== AMD ===` section: the four display sets in dashboard order, or
/// one `N/A (<reason>)` line when the branch degraded.
fn amd_section(out: &mut String, section: &Section<AmdReadout>) {
    header(out, "AMD");
    match section {
        Section::Na(reason) => {
            na_line(out, reason);
            out.push('\n');
        }
        Section::Value(readout) => {
            subheader(out, "Clocks & ratios");
            rows(out, "  ", &clocks_rows(&readout.clocks));
            let timings = timings_rows(&readout.timings);
            subheader(out, "Primary timings (ticks)");
            rows(out, "  ", &timings[0..4]);
            subheader(out, "Secondary timings (ticks)");
            rows(out, "  ", &timings[4..9]);
            subheader(out, "Tertiary & turnarounds (ticks)");
            rows(out, "  ", &timings[9..27]);
            subheader(out, "CAD bus (ohms)");
            rows(out, "  ", &cad_rows(&readout.cad_bus));
            subheader(out, "Voltages");
            rows(out, "  ", &voltages_rows(&readout.voltages));
            out.push('\n');
        }
    }
}

/// The `=== Intel ===` section: one `--- Channel n ---` block per decoded
/// IMC channel (clocks + all 27 timings + CAD + voltages, then the
/// channel-level RTL row group), or one `N/A (<reason>)` line when the
/// branch degraded.
fn intel_section(out: &mut String, section: &Section<IntelReadout>) {
    header(out, "Intel");
    match section {
        Section::Na(reason) => {
            na_line(out, reason);
            out.push('\n');
        }
        Section::Value(readout) => {
            if readout.channels.is_empty() {
                out.push_str("  (no channels decoded)\n\n");
                return;
            }
            for channel in &readout.channels {
                intel_channel(out, channel);
            }
            out.push('\n');
        }
    }
}

/// One `--- Channel n ---` block: the shared display sets plus the
/// channel-level RTL row.
fn intel_channel(out: &mut String, channel: &IntelChannel) {
    let index = channel.index;
    subheader(out, &format!("Channel {index}"));
    rows(out, "  ", &clocks_rows(&channel.clocks));
    let timings = timings_rows(&channel.timings);
    subheader(out, "Primary timings (ticks)");
    rows(out, "  ", &timings[0..4]);
    subheader(out, "Secondary timings (ticks)");
    rows(out, "  ", &timings[4..9]);
    subheader(out, "Tertiary & turnarounds (ticks)");
    rows(out, "  ", &timings[9..27]);
    subheader(out, "CAD bus (ohms)");
    rows(out, "  ", &cad_rows(&channel.cad_bus));
    subheader(out, "Voltages");
    rows(out, "  ", &voltages_rows(&channel.voltages));
    subheader(out, "RTL");
    rows(out, "  ", &[("RTL (ticks)", cell(&channel.rtl))]);
}

/// The `=== SPD ===` section: one `--- Module 0xNN ---` block per decoded
/// module (maker / part / serial / rank / density / speed + the XMP / EXPO
/// profile lines), or a no-modules note when the list is empty.
fn spd_section(out: &mut String, modules: &[SpdModule]) {
    header(out, "SPD");
    if modules.is_empty() {
        out.push_str("  (no modules: ee1004 driver absent or no device bound)\n\n");
        return;
    }
    for module in modules {
        spd_module(out, module);
    }
    out.push('\n');
}

/// One `--- Module 0xNN (DDRn) ---` block: the module identity rows plus
/// one line per factory-rated profile.
fn spd_module(out: &mut String, module: &SpdModule) {
    let index = module.index;
    let gen = if module.is_ddr5 { "DDR5" } else { "DDR4" };
    subheader(out, &format!("Module 0x{index:02X} ({gen})"));
    rows(
        out,
        "  ",
        &[
            ("maker", cell(&module.maker)),
            ("part", cell(&module.part)),
            ("serial", cell(&module.serial)),
            ("rank", cell(&module.rank)),
            ("density", density(&module.density_mbit)),
            ("speed", mts(&module.speed_mts)),
        ],
    );
    if module.profiles.is_empty() {
        out.push_str("  profiles  (none)\n");
        return;
    }
    for profile in &module.profiles {
        spd_profile(out, module.is_ddr5, profile);
    }
}

/// One profile line: `profile <n> (XMP|EXPO): <speed>, <tCL>-<tRCD>-<tRP>-<tRAS> @ <voltage>`.
fn spd_profile(out: &mut String, is_ddr5: bool, profile: &SpdProfile) {
    let scheme = if is_ddr5 { "EXPO" } else { "XMP" };
    let index = profile.index;
    let speed = mts(&profile.speed_mts);
    let cas = cell(&profile.cas);
    let trcd = cell(&profile.trcd);
    let trp = cell(&profile.trp);
    let tras = cell(&profile.tras);
    let voltage = volts(&profile.voltage);
    out.push_str(&format!(
        "  profile {index} ({scheme}): {speed}, {cas}-{trcd}-{trp}-{tras} @ {voltage}\n"
    ));
}

// ---------------------------------------------------------------------------
// The public surface.
// ---------------------------------------------------------------------------

/// Render one [`SystemMemoryTelemetry`] snapshot as the dashboard-style
/// text report.
///
/// Pure and deterministic: no I/O, no client — the same snapshot always
/// yields the same `String` (the TUI snapshot export and the GUI reuse it).
/// Every cell prints its formatted value or `N/A (<reason>)`; every section
/// header prints whether or not the branch populated it (the no-panic
/// contract, plan D5).
pub fn render(telemetry: &SystemMemoryTelemetry) -> String {
    let mut out = String::new();
    out.push_str(TITLE);
    out.push('\n');
    out.push_str(&"=".repeat(TITLE.len()));
    out.push('\n');
    out.push('\n');
    cpu_section(&mut out, &telemetry.cpu);
    amd_section(&mut out, &telemetry.amd);
    intel_section(&mut out, &telemetry.intel);
    spd_section(&mut out, &telemetry.spd);
    out
}

/// Fetch the live snapshot and print the dashboard to stdout.
///
/// One `GetTelemetry` round trip over [`Client`]: the `Telemetry` reply is
/// rendered by [`render`] and printed (`Ok`); a structured `Error` reply
/// maps onto [`ClientError::Protocol`]; any other reply class is a
/// protocol violation for this request (`ClientError::Protocol`).
pub fn dump(client: &mut Client) -> Result<(), ClientError> {
    let response = client.request(&Request::GetTelemetry)?;
    match response {
        Response::Telemetry(telemetry) => {
            println!("{}", render(&telemetry));
            Ok(())
        }
        Response::Error(msg) => Err(ClientError::Protocol(msg)),
        _ => Err(ClientError::Protocol("expected telemetry".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    //! Fixture-driven tests: every snapshot is built through the
    //! telemetry crate's public API (host-independent), mirroring the
    //! facade P2-10 and intel_readout P2-07 fixture style.

    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process;
    use std::thread;

    use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message};
    use ramsleuth_telemetry::cpuid::{AmdZen, IntelGen};
    use ramsleuth_telemetry::intel_readout::decode_channel;

    use super::*;

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

    /// A two-channel Intel readout built through the public decode core:
    /// channel 0 fully populated (DDR4-3200 class), channel 1 the
    /// all-Na degradation state (every register absent).
    fn fixture_intel() -> IntelReadout {
        // MCS_COMMAND_0: tCL / tRCD / tRP / tRAS (ticks).
        let cmd0: u32 = 16 | (16 << 8) | (16 << 16) | (32 << 24);
        // MCS_COMMAND_1: 1N command rate (bits 1:0 = 0) + gear 1 (bits
        // 3:2 = 0) + RTL 6 ticks (bits 6:4) = 6 << 4.
        let cmd1: u32 = 6 << 4;
        // MCS_COMMAND_2: tCCD_S / tCCD_L (ticks).
        let cmd2: u32 = 4 | (12 << 8);
        // MCS_COMMAND_3: tRDRD / tRDWR / tWRWR / tWRRD (ticks).
        let cmd3: u32 = 10 | (8 << 8) | (12 << 16) | (4 << 24);
        IntelReadout {
            channels: vec![
                decode_channel(0, Some(160), [Some(cmd0), Some(cmd1), Some(cmd2), Some(cmd3)]),
                decode_channel(1, None, [None, None, None, None]),
            ],
        }
    }

    /// One SPD module with mixed `Value` cells and one XMP/EXPO profile.
    fn spd_module(index: u8, is_ddr5: bool) -> SpdModule {
        SpdModule {
            index,
            is_ddr5,
            maker: Section::Value("Samsung".to_owned()),
            part: Section::Value("M391A2K40DB".to_owned()),
            serial: Section::Value("S064531ABC".to_owned()),
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

    /// A representative snapshot: a `Value` AMD readout, a `Na` Intel
    /// branch (an AMD host), two SPD modules (DDR4 + DDR5).
    fn representative() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(fixture_amd()),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: vec![spd_module(0x52, false), spd_module(0x53, true)],
        }
    }

    /// An Intel-host snapshot: a `Na` AMD branch, a `Value` two-channel
    /// Intel readout, one all-Na SPD module.
    fn intel_populated() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::AlderLake),
                brand: "Intel Core i7-12700K".to_owned(),
            },
            amd: Section::na(NaReason::UnsupportedHardware),
            intel: Section::Value(fixture_intel()),
            spd: vec![SpdModule {
                index: 0x50,
                is_ddr5: false,
                maker: Section::na(NaReason::DriverMissing),
                part: Section::na(NaReason::NotApplicable),
                serial: Section::na(NaReason::NotApplicable),
                rank: Section::na(NaReason::ParseError("fixture".to_owned())),
                density_mbit: Section::na(NaReason::ParseError("fixture".to_owned())),
                speed_mts: Section::na(NaReason::ParseError("fixture".to_owned())),
                profiles: Vec::new(),
            }],
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
        }
    }

    /// (a) `render` on a representative snapshot: non-empty, carries the
    /// section headers, and formats the populated values (MHz / Ω / V /
    /// MT/s) plus the `Na` cells as `N/A (<reason>)`.
    #[test]
    fn representative_render_has_headers_and_formatted_values() {
        let text = render(&representative());

        assert!(!text.is_empty(), "render must not be empty");
        // section headers
        assert!(text.contains("=== CPU ==="));
        assert!(text.contains("=== AMD ==="));
        assert!(text.contains("=== Intel ==="));
        assert!(text.contains("=== SPD ==="));
        // CPU identity
        assert!(text.contains("Ryzen 9 5950X"));
        // AMD clocks & ratios (1:2 divide, the AMD-not-applicable gear)
        assert!(text.contains("1600.00 MHz"));
        assert!(text.contains("1800.00 MHz"));
        assert!(text.contains("1:2"));
        assert!(text.contains("N/A (not applicable)"));
        // AMD primary + tertiary timings
        assert!(text.contains("tCL     16"));
        assert!(text.contains("N/A (parse error: fixture)"));
        // AMD CAD bus (RZQ code resolved to ohms + a direct-ohms RTT)
        assert!(text.contains("RZQ/10 (24.0 Ω)"));
        assert!(text.contains("45.0 Ω"));
        assert!(text.contains("33.0 Ω"));
        // AMD voltages (mV→V display)
        assert!(text.contains("1.150 V"));
        assert!(text.contains("1.350 V"));
        assert!(text.contains("1.800 V"));
        // Intel branch degraded on this AMD host
        assert!(text.contains("N/A (unsupported hardware)"));
        // SPD modules (one DDR4 + one DDR5) with their profile lines
        assert!(text.contains("Module 0x52 (DDR4)"));
        assert!(text.contains("Module 0x53 (DDR5)"));
        assert!(text.contains("profile 1 (XMP): 3600 MT/s, 18-18-18-36 @ 1.350 V"));
        assert!(text.contains("profile 1 (EXPO): 3600 MT/s, 18-18-18-36 @ 1.350 V"));
    }

    /// (b) `render` on the fully all-Na snapshot: every section prints
    /// its header plus an `N/A (<reason>)` line (empty SPD prints its
    /// no-modules note) — no panic, no omitted section.
    #[test]
    fn all_na_render_is_complete_and_panic_free() {
        let text = render(&all_na());

        assert!(text.contains("=== CPU ==="));
        assert!(text.contains("=== AMD ==="));
        assert!(text.contains("=== Intel ==="));
        assert!(text.contains("=== SPD ==="));
        assert!(text.contains("N/A (driver missing)"));
        assert!(text.contains("N/A (insufficient privilege)"));
        assert!(text.contains("(no modules: ee1004 driver absent or no device bound)"));
        // a fully degraded snapshot must not print any formatted reading
        assert!(!text.contains("MHz"), "all-Na snapshot prints no clock values: {text}");
    }

    /// (c) `render` is deterministic: the same snapshot always yields the
    /// same string (checked on all three fixture shapes).
    #[test]
    fn render_is_deterministic() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            assert_eq!(render(&snapshot), render(&snapshot));
        }
    }

    /// (d) The AMD, Intel, and SPD section headers are always present in
    /// the output — even when the branch behind them is `Na` or the SPD
    /// list is empty (all three fixture shapes).
    #[test]
    fn vendor_section_headers_are_always_present() {
        for snapshot in [representative(), intel_populated(), all_na()] {
            let text = render(&snapshot);
            assert!(text.contains("=== AMD ==="), "{text}");
            assert!(text.contains("=== Intel ==="), "{text}");
            assert!(text.contains("=== SPD ==="), "{text}");
        }
    }

    /// (e) The Intel branch renders per channel: channel 0 with its
    /// decoded readings + RTL, channel 1 the all-Na degradation (every
    /// cell `N/A`, including the derived-uclk parse errors).
    #[test]
    fn intel_render_is_per_channel() {
        let text = render(&intel_populated());

        assert!(text.contains("Intel AlderLake"));
        assert!(text.contains("--- Channel 0 ---"));
        assert!(text.contains("--- Channel 1 ---"));
        // channel 0: decoded clocks (ratio 160 → 1600 MHz) + RTL ticks
        assert!(text.contains("1600.00 MHz"));
        assert!(text.contains("1x")); // gear 1 (cmd1 bits 3:2 = 0)
        assert!(text.contains("N/A (parse error: DRAM frequency ratio absent"));
        // channel 0 RTL
        assert!(text.contains("RTL (ticks)  6"));
    }

    /// (f) `dump` over a live in-process daemon stand-in: a `Telemetry`
    /// reply prints the dashboard and returns `Ok`; a structured `Error`
    /// reply maps onto `ClientError::Protocol` (the P3-18 stand-in
    /// pattern: spawned thread, pid-qualified temp socket).
    struct TempSocket {
        path: PathBuf,
    }

    impl TempSocket {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(format!(
                "/tmp/ramsleuth-dump-{name}-{}.sock",
                process::id()
            ));
            let _ = std::fs::remove_file(&path); // stale file from a crashed earlier run
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Server-side single-frame reader (the P3-11 contract, other end).
    fn read_one_message(stream: &mut UnixStream) -> Option<Message> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match decode_frame(&buf) {
                Err(FrameError::Incomplete) => {}
                other => return other.ok().map(|frame| frame.message),
            }
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Spawn the stand-in; returns the `JoinHandle` to join *after* the
    /// client round trip (joining earlier would block on `accept`).
    fn spawn_standin(sock: &TempSocket, reply: Message) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                if let Some(Message::Request(_)) = read_one_message(&mut stream) {
                    let bytes = encode_frame(&reply).expect("reply must encode");
                    let _ = stream.write_all(&bytes);
                }
            }
        })
    }

    #[test]
    fn dump_prints_dashboard_and_succeeds() {
        let sock = TempSocket::new("ok");
        let handle = spawn_standin(
            &sock,
            Message::Response(Response::Telemetry(representative())),
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        dump(&mut client).expect("dump must succeed on a Telemetry reply");
        handle.join().expect("stand-in thread must not panic");
    }

    /// (f′) A structured `Error` reply maps onto `ClientError::Protocol`
    /// with the daemon's text verbatim.
    #[test]
    fn dump_maps_structured_error_reply() {
        let sock = TempSocket::new("err");
        let handle = spawn_standin(
            &sock,
            Message::Response(Response::Error("daemon said no".to_owned())),
        );
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = dump(&mut client).expect_err("an Error reply must fail dump");
        assert_eq!(err, ClientError::Protocol("daemon said no".to_owned()));
        handle.join().expect("stand-in thread must not panic");
    }
}
