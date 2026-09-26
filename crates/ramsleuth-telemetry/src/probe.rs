//! Probe-report wire types — the consent-gated "Submit Probe Report"
//! payload (chunk-probe-1a, CRITICAL-PATH foundation).
//!
//! This module defines the three serde wire structs that ride
//! `Request::GetProbeReport` / `Response::ProbeReport` (append-only arms
//! in the `ramsleuth-protocol` `Request` / `Response` enums, the OQ-10
//! append pattern):
//!
//! - [`ProbeReport`] — the top-level report: the full
//!   [`SystemMemoryTelemetry`] snapshot, the optional vendor raw-register
//!   dump ([`ProbeRaw`]), and the system identity ([`ProbeSystem`]).
//! - [`ProbeRaw`] — a serde-friendly **flattened** representation of the
//!   vendor raw registers: the Intel raw IMC registers (the 51
//!   `Option<u32>` slots of `IntelImcRegs` re-laid flat — the 4
//!   per-channel `TC_*` blocks, the 2 native `MCL_*` blocks, the global
//!   `MC_BIOS_REQ`, the 7 global MAD registers — plus the MCHBAR
//!   diagnostics) and the AMD raw section (the 13 SMN DRAM words + the
//!   PM-table version / blob length + the five key f32 values). It is a
//!   **flat, stable wire format** (no nesting that would break the
//!   bincode round-trip) — the existing `IntelImcRegs` / `MclRegs` /
//!   `ChannelRegs` deliberately keep no serde derives and are NOT changed.
//! - [`ProbeSystem`] — the operator-facing system identity: CPU brand /
//!   vendor / generation, the PCI host-bridge id (when known), kernel /
//!   OS / arch, the RamSleuth version, and the telemetry source.
//!
//! The daemon-side builder (chunk 1b) populates these from the live
//! snapshot + raw sources; [`render_probe_report_md`] (chunk 2) turns a
//! report into the copy-paste-ready markdown document (the GitHub
//! issue / clipboard / email form); the frontends (chunks 3/4) embed
//! that renderer output.
//!
//! **No-panic contract (D5):** all three structs are plain data — no I/O,
//! no panics on construction. Every field is serde-serializable so the
//! whole report crosses the wire verbatim (plan D3).

use crate::amd_readout::{
    AmdReadout, CadBus, ClockReadout, CommandRate, DivMode, GearMode, RttValue, TimingSet,
    VoltageSet,
};
use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{NaReason, Section};
use crate::intel_readout::IntelReadout;
use crate::spd_decode::SpdModule;
use crate::SystemMemoryTelemetry;

/// The top-level probe report — the `Response::ProbeReport` payload.
///
/// - `telemetry`: the full [`SystemMemoryTelemetry`] snapshot (the daemon
///   `facade::collect()` output, verbatim).
/// - `raw`: the optional Intel raw-register dump ([`ProbeRaw`]) —
///   populated only when an Intel raw source (the `ramsleuth_intel`
///   kobject or the `/dev/mem` MCHBAR fallback) yields data; `None` on
///   AMD / unknown silicon or when both raw sources are unavailable.
/// - `system`: the operator-facing system identity ([`ProbeSystem`]).
///
/// Derives `PartialEq` (not `Eq` — the embedded snapshot carries `f64`
/// cells) so the round-trip test can assert equality.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProbeReport {
    /// The full system memory telemetry snapshot.
    pub telemetry: SystemMemoryTelemetry,
    /// The vendor raw-register dump (flattened — the Intel IMC registers
    /// and/or the AMD SMN/PM section), when available.
    pub raw: Option<ProbeRaw>,
    /// The operator-facing system identity.
    pub system: ProbeSystem,
}

/// A serde-friendly **flattened** representation of the vendor raw
/// registers — the flat, stable wire form of the 51-slot `IntelImcRegs`
/// plus the 7 global MAD registers plus the MCHBAR diagnostics, and the
/// AMD raw section (the 13 SMN DRAM words + the PM-table version / blob
/// length + the five key f32 values).
///
/// The existing `IntelImcRegs` / `MclRegs` / `ChannelRegs` deliberately
/// carry **no** serde derives (they are decode inputs, not wire types);
/// this struct re-lays them flat so the raw set crosses the wire as a
/// stable, nesting-free shape. bincode serializes a struct's fields in
/// declaration order — **the field order below is the wire contract**
/// (the AMD section is appended after every Intel field, OQ-10
/// append-only).
///
/// Every Intel register field is `Option<u32>` (per-register containment:
/// `None` when the underlying read failed / was absent). The MCHBAR
/// diagnostics: `mchbar_base` is `Option<u64>` (the physical base, `None`
/// when absent / malformed) and `mchbar_enabled` is a plain `bool`
/// (`false` when absent / malformed). The AMD fields: `amd_smn_regs` is a
/// `Vec<Option<u32>>` (empty when no `smn` attribute was available) and
/// the PM fields are `Option<u32>` (`None` when absent). An Intel report
/// leaves every AMD field absent, and an AMD report leaves every Intel
/// field absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeRaw {
    // --- global --------------------------------------------------------
    /// `MC_BIOS_REQ` @ `0x5E00` (the global DRAM clock word).
    pub mcbios_req: Option<u32>,
    // --- channel 0 (`0x4000` block) ------------------------------------
    pub tc_ch0_dbp: Option<u32>,
    pub tc_ch0_rap: Option<u32>,
    pub tc_ch0_rfp: Option<u32>,
    pub tc_ch0_rap2: Option<u32>,
    pub tc_ch0_rdrd: Option<u32>,
    pub tc_ch0_rdwr: Option<u32>,
    pub tc_ch0_wrrd: Option<u32>,
    pub tc_ch0_wrwr: Option<u32>,
    // --- channel 1 (`0x4400` block) ------------------------------------
    pub tc_ch1_dbp: Option<u32>,
    pub tc_ch1_rap: Option<u32>,
    pub tc_ch1_rfp: Option<u32>,
    pub tc_ch1_rap2: Option<u32>,
    pub tc_ch1_rdrd: Option<u32>,
    pub tc_ch1_rdwr: Option<u32>,
    pub tc_ch1_wrrd: Option<u32>,
    pub tc_ch1_wrwr: Option<u32>,
    // --- channel 2 (`0x4800` mirror block — Tier-3 subchannel 2) -------
    pub tc_ch2_dbp: Option<u32>,
    pub tc_ch2_rap: Option<u32>,
    pub tc_ch2_rfp: Option<u32>,
    pub tc_ch2_rap2: Option<u32>,
    pub tc_ch2_rdrd: Option<u32>,
    pub tc_ch2_rdwr: Option<u32>,
    pub tc_ch2_wrrd: Option<u32>,
    pub tc_ch2_wrwr: Option<u32>,
    // --- channel 3 (`0x4C00` mirror block — Tier-3 subchannel 3) -------
    pub tc_ch3_dbp: Option<u32>,
    pub tc_ch3_rap: Option<u32>,
    pub tc_ch3_rfp: Option<u32>,
    pub tc_ch3_rap2: Option<u32>,
    pub tc_ch3_rdrd: Option<u32>,
    pub tc_ch3_rdwr: Option<u32>,
    pub tc_ch3_wrrd: Option<u32>,
    pub tc_ch3_wrwr: Option<u32>,
    // --- MC0 MCL block (`0xD000` — Tier-3 native-uncore fallback) ------
    pub mcl0_pre: Option<u32>,
    pub mcl0_act: Option<u32>,
    pub mcl0_act2: Option<u32>,
    pub mcl0_wtr: Option<u32>,
    pub mcl0_rfp: Option<u32>,
    pub mcl0_rfp2: Option<u32>,
    pub mcl0_rdrd: Option<u32>,
    pub mcl0_wrwr: Option<u32>,
    // --- MC1 MCL block (`0xD800` — Tier-3 native-uncore fallback) ------
    pub mcl1_pre: Option<u32>,
    pub mcl1_act: Option<u32>,
    pub mcl1_act2: Option<u32>,
    pub mcl1_wtr: Option<u32>,
    pub mcl1_rfp: Option<u32>,
    pub mcl1_rfp2: Option<u32>,
    pub mcl1_rdrd: Option<u32>,
    pub mcl1_wrwr: Option<u32>,
    // --- global MAD channel/geometry (24-attr module builds) -----------
    pub mad_inter_channel: Option<u32>,
    pub mad_intra_ch0: Option<u32>,
    pub mad_intra_ch1: Option<u32>,
    pub mad_dimm_ch0: Option<u32>,
    pub mad_dimm_ch1: Option<u32>,
    pub mad_dimm_ch2: Option<u32>,
    pub mad_dimm_ch3: Option<u32>,
    // --- MCHBAR diagnostics --------------------------------------------
    /// MCHBAR physical base (`None` when absent / malformed).
    pub mchbar_base: Option<u64>,
    /// MCHBAR enable bit (`false` when absent / malformed).
    pub mchbar_enabled: bool,
    // --- AMD raw section (appended after all Intel fields — OQ-10
    //     append-only; the flat declaration order is the wire contract) ---
    /// The 13 SMN DRAM timing register words in address order (base
    /// addresses, per the offset rule): `Some(word)` on a successful read,
    /// `None` when that read failed. Empty when no `smn` attribute was
    /// available (the whole read is a no-op).
    pub amd_smn_regs: Vec<Option<u32>>,
    /// The PM-table `TableVersionId` (the sibling `pm_table_version` value,
    /// or the legacy first word of the blob).
    pub amd_pm_version: Option<u32>,
    /// The PM-table blob length in bytes (`None` when no blob was acquired).
    pub amd_pm_blob_len: Option<u32>,
    /// `VDDCR_VDD` (Vcore) f32 bit pattern @ `0x0A0`.
    pub amd_pm_vddcr_vdd: Option<u32>,
    /// `VDDCR_SOC` f32 bit pattern @ `0x0B0`.
    pub amd_pm_vddcr_soc: Option<u32>,
    /// `FCLK` f32 bit pattern @ `0x0C0`.
    pub amd_pm_fclk: Option<u32>,
    /// `UCLK` f32 bit pattern @ `0x0C8`.
    pub amd_pm_uclk: Option<u32>,
    /// `MCLK` f32 bit pattern @ `0x0CC`.
    pub amd_pm_mclk: Option<u32>,
}

/// The operator-facing system identity for a probe report: the CPU
/// identity, the PCI host-bridge id (when known), the OS / kernel /
/// arch, the RamSleuth version, and the telemetry source that produced
/// the report.
///
/// All fields are plain strings (or `Option<String>` for the host
/// bridge, which is not resolvable on every platform) so the shape is
/// stable across RamSleuth versions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeSystem {
    /// CPU brand string (e.g. "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz").
    pub cpu_brand: String,
    /// CPU vendor (e.g. "Intel", "AMD", "unknown").
    pub cpu_vendor: String,
    /// CPU generation (e.g. "RocketLake", "Zen3", "Unknown").
    pub cpu_gen: String,
    /// PCI host-bridge id (`vendor:device`), when known.
    pub pci_host_bridge: Option<String>,
    /// Kernel release string (e.g. "6.6.0-1-cachyos").
    pub kernel: String,
    /// OS name / distro (e.g. "Linux / Arch").
    pub os: String,
    /// CPU architecture (e.g. "x86_64").
    pub arch: String,
    /// The RamSleuth version that produced this report (e.g. "2.4.6").
    pub ramsleuth_version: String,
    /// The telemetry source (e.g. "ramsleuth_intel", "ryzen_smu", "devmem").
    pub telemetry_source: String,
}

// ---------------------------------------------------------------------------
// Markdown renderer (chunk-probe-2): the GitHub issue / clipboard / email
// form of a [`ProbeReport`].
// ---------------------------------------------------------------------------

/// Render a [`ProbeReport`] as a copy-paste-ready markdown document.
///
/// The section layout mirrors the GitHub issue template
/// (`.github/ISSUE_TEMPLATE/probe-report.md`, chunk-probe-5), which the
/// frontends pre-fill from this renderer:
///
/// 1. `# RamSleuth Probe Report` — the header + a one-line summary
///    (CPU brand + generation + OS + kernel + RamSleuth version).
/// 2. `## System` — the [`ProbeSystem`] identity table (the template's
///    nine fields, verbatim).
/// 3. `## Decoded Telemetry` — the [`SystemMemoryTelemetry`] in the
///    dashboard's canonical display form (the same cell formatting the
///    TUI / GUI / CLI `dump` renderers use: MHz to two decimals, the
///    derived MT/s = MCLK × 2, ticks as integers, mV→V to three
///    decimals, ohms to one decimal, and `N/A (<reason>)` for every
///    degraded cell): the CPU identity, the AMD readout (the clocks &
///    ratios, all 27 timings, the CAD bus, the voltages), the Intel
///    readout (the readout-level channel mode + the same display sets
///    per decoded channel, plus the channel-level RTL), the SPD
///    modules (the module identity + the factory-rated profiles), and
///    the platform branch (incl. the derived per-DIMM / total
///    capacities).
/// 4. `## Raw Registers` — the [`ProbeRaw`] table (every register as
///    `0x…`; `N/A (absent)` per unread register), or a not-captured
///    note when `raw` is `None`.
/// 5. `## N/A Reasons` — every `Na` cell of the decoded telemetry (and
///    every absent raw register when `raw` is present) with its
///    reason — the "why is this N/A" development detail.
/// 6. The footer — the generating RamSleuth version + the
///    no-personal-information note (no username, hostname, IP, MAC, or
///    serial numbers).
///
/// Pure (no I/O, D5): the same report always yields the same string,
/// and no field value can make the renderer panic.
pub fn render_probe_report_md(report: &ProbeReport) -> String {
    let mut out = String::new();
    // 1. The header: the title + the one-line summary.
    out.push_str("# RamSleuth Probe Report\n\n");
    out.push_str(&format!(
        "> **{}** — {} on {} (kernel {}), RamSleuth v{}\n\n",
        report.system.cpu_brand, report.system.cpu_gen, report.system.os,
        report.system.kernel, report.system.ramsleuth_version,
    ));
    // 2. The System table (the issue template's nine fields).
    out.push_str("## System\n\n");
    out.push_str("| Field | Value |\n|---|---|\n");
    for (field, value) in system_rows(&report.system) {
        out.push_str(&format!("| {} | {} |\n", md_escape(field), md_escape(&value)));
    }
    out.push('\n');
    // 3. The decoded telemetry in the dashboard display form.
    out.push_str("## Decoded Telemetry\n\n");
    render_cpu(&mut out, &report.telemetry.cpu);
    render_amd(&mut out, &report.telemetry.amd);
    render_intel(&mut out, &report.telemetry.intel);
    render_spd(&mut out, &report.telemetry.spd);
    render_platform(&mut out, &report.telemetry);
    // 4. The raw registers: the Intel IMC table when Intel raw is the
    //    active source, the AMD SMN/PM table when AMD raw is, or the
    //    vendor-neutral not-captured note when neither.
    out.push_str("## Raw Registers\n\n");
    match &report.raw {
        Some(raw) if has_intel_raw(raw) => render_raw_table(&mut out, raw),
        Some(raw) if has_amd_raw(raw) => render_amd_raw_table(&mut out, raw),
        _ => out.push_str("_not captured (no raw source available)_\n\n"),
    }
    // 5. The N/A reasons (the "why is this N/A" development detail).
    out.push_str("## N/A Reasons\n\n");
    let mut reasons: Vec<(String, String)> = Vec::new();
    collect_na_reasons(report, &mut reasons);
    if reasons.is_empty() {
        out.push_str("_none — every decoded field carried a value_\n\n");
    } else {
        for (path, text) in &reasons {
            out.push_str(&format!("- `{path}` — {text}\n"));
        }
        out.push('\n');
    }
    // 6. The footer: the generating version + the no-personal-info note.
    out.push_str("---\n\n");
    out.push_str(&format!(
        "_This report was auto-generated by RamSleuth v{}. It contains no personal information (no username, hostname, IP, MAC, or serial numbers)._\n",
        report.system.ramsleuth_version,
    ));
    out
}

/// The [`ProbeSystem`] rows of the System table (the issue template's
/// nine fields, verbatim; the host bridge renders `N/A` when unknown).
fn system_rows(system: &ProbeSystem) -> Vec<(&str, String)> {
    vec![
        ("CPU", system.cpu_brand.clone()),
        ("Vendor", system.cpu_vendor.clone()),
        ("Generation", system.cpu_gen.clone()),
        (
            "PCI Host Bridge",
            system.pci_host_bridge.clone().unwrap_or_else(|| "N/A".to_owned()),
        ),
        ("Kernel", system.kernel.clone()),
        ("OS", system.os.clone()),
        ("Arch", system.arch.clone()),
        ("RamSleuth Version", system.ramsleuth_version.clone()),
        ("Telemetry Source", system.telemetry_source.clone()),
    ]
}

/// Escape the one markdown-table-breaking character (`|`) in a cell so
/// the table stays well-formed when pasted into GitHub.
fn md_escape(value: &str) -> String {
    value.replace('|', "\\|")
}

/// The CPU vendor as a display string (the same form the CLI `dump`
/// renderer prints; the snapshot carries the vendor + brand only).
fn vendor_text(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Amd(zen) => format!("AMD {zen:?}"),
        CpuVendor::Intel(gen) => format!("Intel {gen:?}"),
        CpuVendor::Unknown => "unknown".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Cell formatters: one per value kind. Each renders a [`Section`] cell
// as its formatted value or `N/A (<reason>)` — the same compact,
// user-presentable forms the TUI / GUI / CLI `dump` renderers print
// (mirrored here: this crate sits below the display crates, so it owns
// its copy of the frozen display forms; [`NaReason`] has no `Display`
// impl, so the renderer owns the reason text).
// ---------------------------------------------------------------------------

/// The human-readable text of an absent cell: `N/A (<reason>)`.
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

/// The DDR data rate derived from the memory clock: MT/s = MCLK × 2
/// (the DDR double-pumping rule — the `intel_readout` module docs);
/// `N/A` when MCLK itself is `Na` (never a fabricated rate).
fn derived_mts(mclk: &Section<f64>) -> String {
    match mclk {
        Section::Value(value) => format!("{:.0} MT/s", value * 2.0),
        Section::Na(_) => "N/A (derived from MCLK)".to_owned(),
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

/// The DRAM command rate: `1T` / `2T` or `N/A (<reason>)`.
fn command_rate(section: &Section<CommandRate>) -> String {
    match section {
        Section::Value(CommandRate::OneT) => "1T".to_owned(),
        Section::Value(CommandRate::TwoT) => "2T".to_owned(),
        Section::Na(reason) => na_cell(reason),
    }
}

/// A memory-rail cell in volts (the frozen mV value over 1000, three
/// decimals — the plan's mV→V display rule).
fn volts(section: &Section<u16>) -> String {
    match section {
        Section::Value(mv) => format!("{:.3} V", f64::from(*mv) / 1000.0),
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

/// An absent raw-register cell: `0x…` when read, `N/A (absent)` when the
/// underlying read failed / was unavailable.
fn hex32(value: Option<u32>) -> String {
    match value {
        Some(word) => format!("0x{word:08X}"),
        None => "N/A (absent)".to_owned(),
    }
}

/// The MCHBAR base in hex (`0x…`), `N/A (absent)` when unread.
fn hex64(value: Option<u64>) -> String {
    match value {
        Some(base) => format!("0x{base:X}"),
        None => "N/A (absent)".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Per-display-set row builders (the shared display sets of the AMD
// readout and each Intel channel).
// ---------------------------------------------------------------------------

/// The clocks & ratios rows of a [`ClockReadout`] (the dashboard order,
/// with the derived MT/s = MCLK × 2 row).
fn clocks_rows(clocks: &ClockReadout) -> Vec<(&str, String)> {
    vec![
        ("MCLK", mhz(&clocks.mclk_mhz)),
        ("MT/s", derived_mts(&clocks.mclk_mhz)),
        ("UCLK", mhz(&clocks.uclk_mhz)),
        ("FCLK", mhz(&clocks.fclk_mhz)),
        ("UCLK:MCLK", div_mode(&clocks.div_mode)),
        ("gear mode", gear_mode(&clocks.gear_mode)),
        ("GDM", flag(&clocks.gdm)),
        ("PDM", flag(&clocks.pdm)),
        ("command rate", command_rate(&clocks.command_rate)),
    ]
}

/// All 27 DRAM subtiming rows of a [`TimingSet`], in canonical order
/// (primary first, then secondary, then tertiary + turnarounds — the
/// same order the TUI / CLI `dump` renderers print).
fn timings_rows(timings: &TimingSet) -> Vec<(&str, String)> {
    vec![
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
fn cad_rows(cad: &CadBus) -> Vec<(&str, String)> {
    vec![
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

/// The five memory-rail rows of a [`VoltageSet`] (volts; the Vcore
/// primary rail leads, the TUI order).
fn voltages_rows(voltages: &VoltageSet) -> Vec<(&str, String)> {
    vec![
        ("VDDCR_VDD (Vcore)", volts(&voltages.vcore_mv)),
        ("VDDCR_SOC", volts(&voltages.vddcr_soc_mv)),
        ("VDDIO_MEM", volts(&voltages.vddio_mem_mv)),
        ("VDD_MISC", volts(&voltages.vdd_misc_mv)),
        ("VPP", volts(&voltages.vpp_mv)),
    ]
}

/// One markdown `| <h1> | <h2> |` table (the two-column form the issue
/// template uses for every section).
fn md_table(out: &mut String, h1: &str, h2: &str, rows: &[(&str, String)]) {
    out.push_str(&format!("| {h1} | {h2} |\n|---|---|\n"));
    for (key, value) in rows {
        out.push_str(&format!("| {} | {} |\n", md_escape(key), md_escape(value)));
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Telemetry sections (the `## Decoded Telemetry` subsections).
// ---------------------------------------------------------------------------

/// The four display-set tables of one readout (the AMD readout, or one
/// Intel channel): the clocks & ratios, all 27 timings, the CAD bus,
/// and the voltages — plus the channel-level RTL row on Intel.
fn render_display_sets(
    out: &mut String,
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad: &CadBus,
    voltages: &VoltageSet,
    rtl: Option<&Section<u16>>,
) {
    md_table(out, "Clock", "Value", &clocks_rows(clocks));
    md_table(out, "Timing (ticks)", "Value", &timings_rows(timings));
    md_table(out, "CAD bus (ohms)", "Value", &cad_rows(cad));
    md_table(out, "Rail (V)", "Value", &voltages_rows(voltages));
    if let Some(rtl) = rtl {
        md_table(out, "RTL (ticks)", "Value", &[("RTL", cell(rtl))]);
    }
}

/// The `### CPU` subsection: vendor + brand (the snapshot root carries
/// the detected CPU — always populated).
fn render_cpu(out: &mut String, cpu: &CpuInfo) {
    out.push_str("### CPU\n\n");
    md_table(
        out,
        "Field",
        "Value",
        &[
            ("vendor", vendor_text(&cpu.vendor)),
            ("brand", cpu.brand.clone()),
        ],
    );
}

/// The `### AMD` subsection: the four display sets, or one
/// `N/A (<reason>)` line when the branch degraded.
fn render_amd(out: &mut String, section: &Section<AmdReadout>) {
    out.push_str("### AMD\n\n");
    match section {
        Section::Na(reason) => out.push_str(&format!("{}\n\n", na_cell(reason))),
        Section::Value(readout) => render_display_sets(
            out,
            &readout.clocks,
            &readout.timings,
            &readout.cad_bus,
            &readout.voltages,
            None,
        ),
    }
}

/// The `### Intel` subsection: the readout-level `channel mode` row
/// (the hardware `MAD_INTER_CHANNEL` mode — `N/A` on the `/dev/mem`
/// fallback where no mode is carried), then one `#### Channel n` block
/// per decoded IMC channel, or one `N/A (<reason>)` line when the
/// branch degraded.
fn render_intel(out: &mut String, section: &Section<IntelReadout>) {
    out.push_str("### Intel\n\n");
    match section {
        Section::Na(reason) => out.push_str(&format!("{}\n\n", na_cell(reason))),
        Section::Value(readout) => {
            let mode = readout
                .channel_mode
                .map(|m| m.label().to_owned())
                .unwrap_or_else(|| "N/A".to_owned());
            md_table(out, "Field", "Value", &[("channel mode", mode)]);
            if readout.channels.is_empty() {
                out.push_str("_no channels decoded_\n\n");
                return;
            }
            for channel in &readout.channels {
                out.push_str(&format!("#### Channel {}\n\n", channel.index));
                render_display_sets(
                    out,
                    &channel.clocks,
                    &channel.timings,
                    &channel.cad_bus,
                    &channel.voltages,
                    Some(&channel.rtl),
                );
            }
        }
    }
}

/// The `### SPD` subsection: one `#### Module 0xNN (DDRn)` block per
/// decoded module (the module identity + the factory-rated profiles),
/// or a no-modules note when the list is empty.
fn render_spd(out: &mut String, modules: &[SpdModule]) {
    out.push_str("### SPD\n\n");
    if modules.is_empty() {
        out.push_str("_no modules (ee1004 driver absent or no device bound)_\n\n");
        return;
    }
    for module in modules {
        let gen = if module.is_ddr5 { "DDR5" } else { "DDR4" };
        out.push_str(&format!("#### Module 0x{:02X} ({gen})\n\n", module.index));
        md_table(
            out,
            "Field",
            "Value",
            &[
                ("maker", cell(&module.maker)),
                ("die maker", cell(&module.die_maker)),
                ("die type", cell(&module.die_type)),
                ("devices", cell(&module.devices)),
                ("part", cell(&module.part)),
                ("serial", cell(&module.serial)),
                ("rank", cell(&module.rank)),
                ("density", density(&module.density_mbit)),
                ("speed", mts(&module.speed_mts)),
            ],
        );
        if module.profiles.is_empty() {
            out.push_str("_no factory-rated profiles_\n\n");
        } else {
            for profile in &module.profiles {
                let scheme = if module.is_ddr5 { "EXPO" } else { "XMP" };
                out.push_str(&format!(
                    "- profile {} ({scheme}): {}, {}-{}-{}-{} @ {}\n",
                    profile.index,
                    mts(&profile.speed_mts),
                    cell(&profile.cas),
                    cell(&profile.trcd),
                    cell(&profile.trp),
                    cell(&profile.tras),
                    volts(&profile.voltage),
                ));
            }
            out.push('\n');
        }
    }
}

/// The `### Platform` subsection: the vendor-neutral identity fields +
/// the derived per-DIMM / total capacities.
fn render_platform(out: &mut String, telemetry: &SystemMemoryTelemetry) {
    let platform = &telemetry.platform;
    out.push_str("### Platform\n\n");
    let dimm_sizes = if telemetry.dimm_sizes.is_empty() {
        "none".to_owned()
    } else {
        telemetry
            .dimm_sizes
            .iter()
            .map(|size| match size {
                Section::Value(gib) => format!("{gib:.1} GiB"),
                Section::Na(_) => "N/A".to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    md_table(
        out,
        "Field",
        "Value",
        &[
            ("CPU clock (MHz)", mhz(&platform.cpu_clock_mhz)),
            ("motherboard", cell(&platform.motherboard)),
            ("BIOS", cell(&platform.bios)),
            ("AGESA", cell(&platform.agesa)),
            ("SMU version", cell(&platform.smu_version)),
            ("total capacity", capacity(&telemetry.total_capacity)),
            ("per-DIMM sizes", dimm_sizes),
        ],
    );
}

/// A GiB capacity cell (one decimal), or the branch's `Na` text.
fn capacity(section: &Section<f64>) -> String {
    match section {
        Section::Value(gib) => format!("{gib:.1} GiB"),
        Section::Na(reason) => na_cell(reason),
    }
}

// ---------------------------------------------------------------------------
// The raw-register table (the `## Raw Registers` section).
// ---------------------------------------------------------------------------

/// The full [`ProbeRaw`] table — every register in the wire (field)
/// order as `0x…` (or `N/A (absent)`), plus the MCHBAR diagnostics.
fn render_raw_table(out: &mut String, raw: &ProbeRaw) {
    md_table(
        out,
        "Register",
        "Value",
        &[
            ("MC_BIOS_REQ", hex32(raw.mcbios_req)),
            ("TC_CH0_DBP", hex32(raw.tc_ch0_dbp)),
            ("TC_CH0_RAP", hex32(raw.tc_ch0_rap)),
            ("TC_CH0_RFP", hex32(raw.tc_ch0_rfp)),
            ("TC_CH0_RAP2", hex32(raw.tc_ch0_rap2)),
            ("TC_CH0_RDRD", hex32(raw.tc_ch0_rdrd)),
            ("TC_CH0_RDWR", hex32(raw.tc_ch0_rdwr)),
            ("TC_CH0_WRRD", hex32(raw.tc_ch0_wrrd)),
            ("TC_CH0_WRWR", hex32(raw.tc_ch0_wrwr)),
            ("TC_CH1_DBP", hex32(raw.tc_ch1_dbp)),
            ("TC_CH1_RAP", hex32(raw.tc_ch1_rap)),
            ("TC_CH1_RFP", hex32(raw.tc_ch1_rfp)),
            ("TC_CH1_RAP2", hex32(raw.tc_ch1_rap2)),
            ("TC_CH1_RDRD", hex32(raw.tc_ch1_rdrd)),
            ("TC_CH1_RDWR", hex32(raw.tc_ch1_rdwr)),
            ("TC_CH1_WRRD", hex32(raw.tc_ch1_wrrd)),
            ("TC_CH1_WRWR", hex32(raw.tc_ch1_wrwr)),
            ("TC_CH2_DBP", hex32(raw.tc_ch2_dbp)),
            ("TC_CH2_RAP", hex32(raw.tc_ch2_rap)),
            ("TC_CH2_RFP", hex32(raw.tc_ch2_rfp)),
            ("TC_CH2_RAP2", hex32(raw.tc_ch2_rap2)),
            ("TC_CH2_RDRD", hex32(raw.tc_ch2_rdrd)),
            ("TC_CH2_RDWR", hex32(raw.tc_ch2_rdwr)),
            ("TC_CH2_WRRD", hex32(raw.tc_ch2_wrrd)),
            ("TC_CH2_WRWR", hex32(raw.tc_ch2_wrwr)),
            ("TC_CH3_DBP", hex32(raw.tc_ch3_dbp)),
            ("TC_CH3_RAP", hex32(raw.tc_ch3_rap)),
            ("TC_CH3_RFP", hex32(raw.tc_ch3_rfp)),
            ("TC_CH3_RAP2", hex32(raw.tc_ch3_rap2)),
            ("TC_CH3_RDRD", hex32(raw.tc_ch3_rdrd)),
            ("TC_CH3_RDWR", hex32(raw.tc_ch3_rdwr)),
            ("TC_CH3_WRRD", hex32(raw.tc_ch3_wrrd)),
            ("TC_CH3_WRWR", hex32(raw.tc_ch3_wrwr)),
            ("MCL0_PRE", hex32(raw.mcl0_pre)),
            ("MCL0_ACT", hex32(raw.mcl0_act)),
            ("MCL0_ACT2", hex32(raw.mcl0_act2)),
            ("MCL0_WTR", hex32(raw.mcl0_wtr)),
            ("MCL0_RFP", hex32(raw.mcl0_rfp)),
            ("MCL0_RFP2", hex32(raw.mcl0_rfp2)),
            ("MCL0_RDRD", hex32(raw.mcl0_rdrd)),
            ("MCL0_WRWR", hex32(raw.mcl0_wrwr)),
            ("MCL1_PRE", hex32(raw.mcl1_pre)),
            ("MCL1_ACT", hex32(raw.mcl1_act)),
            ("MCL1_ACT2", hex32(raw.mcl1_act2)),
            ("MCL1_WTR", hex32(raw.mcl1_wtr)),
            ("MCL1_RFP", hex32(raw.mcl1_rfp)),
            ("MCL1_RFP2", hex32(raw.mcl1_rfp2)),
            ("MCL1_RDRD", hex32(raw.mcl1_rdrd)),
            ("MCL1_WRWR", hex32(raw.mcl1_wrwr)),
            ("MAD_INTER_CHANNEL", hex32(raw.mad_inter_channel)),
            ("MAD_INTRA_CH0", hex32(raw.mad_intra_ch0)),
            ("MAD_INTRA_CH1", hex32(raw.mad_intra_ch1)),
            ("MAD_DIMM_CH0", hex32(raw.mad_dimm_ch0)),
            ("MAD_DIMM_CH1", hex32(raw.mad_dimm_ch1)),
            ("MAD_DIMM_CH2", hex32(raw.mad_dimm_ch2)),
            ("MAD_DIMM_CH3", hex32(raw.mad_dimm_ch3)),
            ("MCHBAR_BASE", hex64(raw.mchbar_base)),
            (
                "MCHBAR_ENABLED",
                if raw.mchbar_enabled { "true".to_owned() } else { "false".to_owned() },
            ),
        ],
    );
}

/// `true` when the Intel raw slots are the active source (at least one of
/// the 56 register words is read, or the MCHBAR diagnostics carry data).
fn has_intel_raw(raw: &ProbeRaw) -> bool {
    raw_u32_sections(raw)
        .iter()
        .any(|(_, value)| value.is_some())
        || raw.mchbar_base.is_some()
        || raw.mchbar_enabled
}

/// `true` when the AMD raw section is populated (a non-empty SMN register
/// list, or a PM-table version).
fn has_amd_raw(raw: &ProbeRaw) -> bool {
    !raw.amd_smn_regs.is_empty() || raw.amd_pm_version.is_some()
}

/// The AMD raw section of the `## Raw Registers` table: the 13 SMN DRAM
/// register words (named by their base address) + the PM-table key values
/// (version, blob length, and the five f32 bit patterns).
fn render_amd_raw_table(out: &mut String, raw: &ProbeRaw) {
    // The 13 SMN register words, in address order.
    out.push_str("### AMD SMN Registers\n\n");
    out.push_str("| Register | Value |\n|---|---|\n");
    for (index, address) in crate::amd_smn::SMN_REGISTER_SET.iter().enumerate() {
        let value = raw.amd_smn_regs.get(index).copied().flatten();
        out.push_str(&format!(
            "| SMN_{:#X} | {} |\n",
            address,
            match value {
                Some(word) => format!("0x{word:08X}"),
                None => "N/A (absent)".to_owned(),
            }
        ));
    }
    out.push('\n');
    // The PM-table key values.
    out.push_str("### AMD PM Table\n\n");
    out.push_str("| Field | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| PM Version | {} |\n",
        match raw.amd_pm_version {
            Some(version) => format!("0x{version:08X}"),
            None => "N/A (absent)".to_owned(),
        }
    ));
    out.push_str(&format!(
        "| PM Blob Length | {} |\n",
        match raw.amd_pm_blob_len {
            Some(len) => format!("{len} bytes"),
            None => "N/A (absent)".to_owned(),
        }
    ));
    for (name, value) in amd_pm_sections(raw) {
        out.push_str(&format!(
            "| {} | {} |\n",
            md_escape(name),
            match value {
                Some(bits) => format!("0x{bits:08X} (f32 bits)"),
                None => "N/A (absent)".to_owned(),
            }
        ));
    }
    out.push('\n');
}

/// The five AMD PM-table key-value cells as (field name, value) pairs, in
/// display order (VDDCR_VDD, VDDCR_SOC, FCLK, UCLK, MCLK).
fn amd_pm_sections(raw: &ProbeRaw) -> [(&str, Option<u32>); 5] {
    [
        ("VDDCR_VDD", raw.amd_pm_vddcr_vdd),
        ("VDDCR_SOC", raw.amd_pm_vddcr_soc),
        ("FCLK", raw.amd_pm_fclk),
        ("UCLK", raw.amd_pm_uclk),
        ("MCLK", raw.amd_pm_mclk),
    ]
}

// ---------------------------------------------------------------------------
// The N/A-reason collection (the `## N/A Reasons` section).
// ---------------------------------------------------------------------------

/// Push one `Na` cell as a (`<field path>`, `N/A (<reason>)`) pair; a
/// `Value` cell contributes nothing.
fn push_na<T>(out: &mut Vec<(String, String)>, path: &str, section: &Section<T>) {
    if let Section::Na(reason) = section {
        out.push((path.to_owned(), na_cell(reason)));
    }
}

/// The 27 [`TimingSet`] cells as (field name, section) pairs (the raw
/// field names, for the N/A-reason paths).
fn timings_sections(timings: &TimingSet) -> [(&str, &Section<u16>); 27] {
    [
        ("cl", &timings.cl),
        ("rcwdwr", &timings.rcwdwr),
        ("rcdrd", &timings.rcdrd),
        ("rp", &timings.rp),
        ("ras", &timings.ras),
        ("rc", &timings.rc),
        ("rrds", &timings.rrds),
        ("rrld", &timings.rrld),
        ("faw", &timings.faw),
        ("wtrs", &timings.wtrs),
        ("wtrl", &timings.wtrl),
        ("wr", &timings.wr),
        ("rfc1", &timings.rfc1),
        ("rfc2", &timings.rfc2),
        ("rfcsb", &timings.rfcsb),
        ("cwl", &timings.cwl),
        ("rtp", &timings.rtp),
        ("rdwr", &timings.rdwr),
        ("wrrd", &timings.wrrd),
        ("rdrd_sd", &timings.rdrd_sd),
        ("rdrd_dd", &timings.rdrd_dd),
        ("rdrd_scl", &timings.rdrd_scl),
        ("rdrd_sc", &timings.rdrd_sc),
        ("wrwr_sd", &timings.wrwr_sd),
        ("wrwr_dd", &timings.wrwr_dd),
        ("wrwr_scl", &timings.wrwr_scl),
        ("wrwr_sc", &timings.wrwr_sc),
    ]
}

/// The five [`VoltageSet`] cells as (field name, section) pairs.
fn voltages_sections(voltages: &VoltageSet) -> [(&str, &Section<u16>); 5] {
    [
        ("vcore", &voltages.vcore_mv),
        ("vddcr_soc", &voltages.vddcr_soc_mv),
        ("vddio_mem", &voltages.vddio_mem_mv),
        ("vdd_misc", &voltages.vdd_misc_mv),
        ("vpp", &voltages.vpp_mv),
    ]
}

/// Push the `Na` cells of one display readout (the shared clock /
/// timing / CAD / voltage cell sets) under `<prefix>`.
fn push_display_na(
    prefix: &str,
    out: &mut Vec<(String, String)>,
    clocks: &ClockReadout,
    timings: &TimingSet,
    cad: &CadBus,
    voltages: &VoltageSet,
) {
    push_na(out, &format!("{prefix}.clocks.mclk"), &clocks.mclk_mhz);
    push_na(out, &format!("{prefix}.clocks.uclk"), &clocks.uclk_mhz);
    push_na(out, &format!("{prefix}.clocks.fclk"), &clocks.fclk_mhz);
    push_na(out, &format!("{prefix}.clocks.div_mode"), &clocks.div_mode);
    push_na(out, &format!("{prefix}.clocks.gear_mode"), &clocks.gear_mode);
    push_na(out, &format!("{prefix}.clocks.gdm"), &clocks.gdm);
    push_na(out, &format!("{prefix}.clocks.pdm"), &clocks.pdm);
    push_na(
        out,
        &format!("{prefix}.clocks.command_rate"),
        &clocks.command_rate,
    );
    for (name, section) in timings_sections(timings) {
        push_na(out, &format!("{prefix}.timings.{name}"), section);
    }
    push_na(out, &format!("{prefix}.cad_bus.proc_odt"), &cad.proc_odt);
    push_na(out, &format!("{prefix}.cad_bus.rtt_nom"), &cad.rtt_nom);
    push_na(out, &format!("{prefix}.cad_bus.rtt_wr"), &cad.rtt_wr);
    push_na(out, &format!("{prefix}.cad_bus.rtt_park"), &cad.rtt_park);
    push_na(out, &format!("{prefix}.cad_bus.clk_drv"), &cad.clk_drv);
    push_na(out, &format!("{prefix}.cad_bus.addr_cmd_drv"), &cad.addr_cmd_drv);
    push_na(out, &format!("{prefix}.cad_bus.cs_odt_drv"), &cad.cs_odt_drv);
    push_na(out, &format!("{prefix}.cad_bus.cke_drv"), &cad.cke_drv);
    for (name, section) in voltages_sections(voltages) {
        push_na(out, &format!("{prefix}.voltages.{name}"), section);
    }
}

/// The 56 [`ProbeRaw`] `Option<u32>` slots as (field path, value)
/// pairs, in the wire (declaration) order.
fn raw_u32_sections(raw: &ProbeRaw) -> [(&str, Option<u32>); 56] {
    [
        ("raw.mcbios_req", raw.mcbios_req),
        ("raw.tc_ch0_dbp", raw.tc_ch0_dbp),
        ("raw.tc_ch0_rap", raw.tc_ch0_rap),
        ("raw.tc_ch0_rfp", raw.tc_ch0_rfp),
        ("raw.tc_ch0_rap2", raw.tc_ch0_rap2),
        ("raw.tc_ch0_rdrd", raw.tc_ch0_rdrd),
        ("raw.tc_ch0_rdwr", raw.tc_ch0_rdwr),
        ("raw.tc_ch0_wrrd", raw.tc_ch0_wrrd),
        ("raw.tc_ch0_wrwr", raw.tc_ch0_wrwr),
        ("raw.tc_ch1_dbp", raw.tc_ch1_dbp),
        ("raw.tc_ch1_rap", raw.tc_ch1_rap),
        ("raw.tc_ch1_rfp", raw.tc_ch1_rfp),
        ("raw.tc_ch1_rap2", raw.tc_ch1_rap2),
        ("raw.tc_ch1_rdrd", raw.tc_ch1_rdrd),
        ("raw.tc_ch1_rdwr", raw.tc_ch1_rdwr),
        ("raw.tc_ch1_wrrd", raw.tc_ch1_wrrd),
        ("raw.tc_ch1_wrwr", raw.tc_ch1_wrwr),
        ("raw.tc_ch2_dbp", raw.tc_ch2_dbp),
        ("raw.tc_ch2_rap", raw.tc_ch2_rap),
        ("raw.tc_ch2_rfp", raw.tc_ch2_rfp),
        ("raw.tc_ch2_rap2", raw.tc_ch2_rap2),
        ("raw.tc_ch2_rdrd", raw.tc_ch2_rdrd),
        ("raw.tc_ch2_rdwr", raw.tc_ch2_rdwr),
        ("raw.tc_ch2_wrrd", raw.tc_ch2_wrrd),
        ("raw.tc_ch2_wrwr", raw.tc_ch2_wrwr),
        ("raw.tc_ch3_dbp", raw.tc_ch3_dbp),
        ("raw.tc_ch3_rap", raw.tc_ch3_rap),
        ("raw.tc_ch3_rfp", raw.tc_ch3_rfp),
        ("raw.tc_ch3_rap2", raw.tc_ch3_rap2),
        ("raw.tc_ch3_rdrd", raw.tc_ch3_rdrd),
        ("raw.tc_ch3_rdwr", raw.tc_ch3_rdwr),
        ("raw.tc_ch3_wrrd", raw.tc_ch3_wrrd),
        ("raw.tc_ch3_wrwr", raw.tc_ch3_wrwr),
        ("raw.mcl0_pre", raw.mcl0_pre),
        ("raw.mcl0_act", raw.mcl0_act),
        ("raw.mcl0_act2", raw.mcl0_act2),
        ("raw.mcl0_wtr", raw.mcl0_wtr),
        ("raw.mcl0_rfp", raw.mcl0_rfp),
        ("raw.mcl0_rfp2", raw.mcl0_rfp2),
        ("raw.mcl0_rdrd", raw.mcl0_rdrd),
        ("raw.mcl0_wrwr", raw.mcl0_wrwr),
        ("raw.mcl1_pre", raw.mcl1_pre),
        ("raw.mcl1_act", raw.mcl1_act),
        ("raw.mcl1_act2", raw.mcl1_act2),
        ("raw.mcl1_wtr", raw.mcl1_wtr),
        ("raw.mcl1_rfp", raw.mcl1_rfp),
        ("raw.mcl1_rfp2", raw.mcl1_rfp2),
        ("raw.mcl1_rdrd", raw.mcl1_rdrd),
        ("raw.mcl1_wrwr", raw.mcl1_wrwr),
        ("raw.mad_inter_channel", raw.mad_inter_channel),
        ("raw.mad_intra_ch0", raw.mad_intra_ch0),
        ("raw.mad_intra_ch1", raw.mad_intra_ch1),
        ("raw.mad_dimm_ch0", raw.mad_dimm_ch0),
        ("raw.mad_dimm_ch1", raw.mad_dimm_ch1),
        ("raw.mad_dimm_ch2", raw.mad_dimm_ch2),
        ("raw.mad_dimm_ch3", raw.mad_dimm_ch3),
    ]
}

/// Collect every `Na` cell of the decoded telemetry (and every absent
/// raw register when `raw` is present) as a (`<field path>`, `N/A
/// (<reason>)`) pair — the "why is this N/A" development detail.
fn collect_na_reasons(report: &ProbeReport, out: &mut Vec<(String, String)>) {
    let telemetry = &report.telemetry;
    match &telemetry.amd {
        Section::Na(reason) => out.push((
            "amd (whole branch)".to_owned(),
            na_cell(reason),
        )),
        Section::Value(readout) => push_display_na(
            "amd",
            out,
            &readout.clocks,
            &readout.timings,
            &readout.cad_bus,
            &readout.voltages,
        ),
    }
    match &telemetry.intel {
        Section::Na(reason) => out.push((
            "intel (whole branch)".to_owned(),
            na_cell(reason),
        )),
        Section::Value(readout) => {
            if readout.channel_mode.is_none() {
                out.push((
                    "intel.channel_mode".to_owned(),
                    "N/A (not decoded: /dev/mem fallback or a pre-24-attr module)".to_owned(),
                ));
            }
            for channel in &readout.channels {
                let prefix = format!("intel.ch{}", channel.index);
                push_display_na(
                    &prefix,
                    out,
                    &channel.clocks,
                    &channel.timings,
                    &channel.cad_bus,
                    &channel.voltages,
                );
                push_na(out, &format!("{prefix}.rtl"), &channel.rtl);
            }
        }
    }
    for module in &telemetry.spd {
        let prefix = format!("spd[0x{:02X}]", module.index);
        push_na(out, &format!("{prefix}.maker"), &module.maker);
        push_na(out, &format!("{prefix}.die_maker"), &module.die_maker);
        push_na(out, &format!("{prefix}.die_type"), &module.die_type);
        push_na(out, &format!("{prefix}.devices"), &module.devices);
        push_na(out, &format!("{prefix}.part"), &module.part);
        push_na(out, &format!("{prefix}.serial"), &module.serial);
        push_na(out, &format!("{prefix}.rank"), &module.rank);
        push_na(
            out,
            &format!("{prefix}.density_mbit"),
            &module.density_mbit,
        );
        push_na(out, &format!("{prefix}.speed_mts"), &module.speed_mts);
        for (profile_index, profile) in module.profiles.iter().enumerate() {
            let pp = format!("{prefix}.profiles[{profile_index}]");
            push_na(out, &format!("{pp}.speed_mts"), &profile.speed_mts);
            push_na(out, &format!("{pp}.cas"), &profile.cas);
            push_na(out, &format!("{pp}.trcd"), &profile.trcd);
            push_na(out, &format!("{pp}.trp"), &profile.trp);
            push_na(out, &format!("{pp}.tras"), &profile.tras);
            push_na(out, &format!("{pp}.voltage"), &profile.voltage);
        }
    }
    let platform = &telemetry.platform;
    push_na(
        out,
        "platform.cpu_clock_mhz",
        &platform.cpu_clock_mhz,
    );
    push_na(out, "platform.motherboard", &platform.motherboard);
    push_na(out, "platform.bios", &platform.bios);
    push_na(out, "platform.agesa", &platform.agesa);
    push_na(out, "platform.smu_version", &platform.smu_version);
    push_na(out, "total_capacity", &telemetry.total_capacity);
    for (index, size) in telemetry.dimm_sizes.iter().enumerate() {
        push_na(out, &format!("dimm_sizes[{index}]"), size);
    }
    match &report.raw {
        None => out.push((
            "raw (whole dump)".to_owned(),
            "not captured (no raw source available)".to_owned(),
        )),
        Some(raw) => {
            // The Intel raw slots — only when Intel is the active source
            // (an AMD report carries all-`None` Intel fields, which are not
            // "absent" but "not this vendor's source").
            if has_intel_raw(raw) {
                for (name, value) in raw_u32_sections(raw) {
                    if value.is_none() {
                        out.push((
                            name.to_owned(),
                            "N/A (register absent / read failed)".to_owned(),
                        ));
                    }
                }
                if raw.mchbar_base.is_none() {
                    out.push((
                        "raw.mchbar_base".to_owned(),
                        "N/A (register absent / read failed)".to_owned(),
                    ));
                }
            }
            // The AMD raw section.
            if has_amd_raw(raw) {
                // No `smn` attribute (a PM-only capture) → the whole SMN
                // block is not captured.
                if raw.amd_smn_regs.is_empty() {
                    out.push((
                        "amd.smn (whole block)".to_owned(),
                        "AMD SMN: not captured (no ryzen_smu driver)".to_owned(),
                    ));
                }
                // Per-register N/A entries for the failed SMN words.
                for (index, value) in raw.amd_smn_regs.iter().enumerate() {
                    if value.is_none() {
                        let address = crate::amd_smn::SMN_REGISTER_SET[index];
                        out.push((
                            format!("amd.smn.smn_{address:#X}"),
                            "N/A (register absent / read failed)".to_owned(),
                        ));
                    }
                }
                // The PM-table key values.
                if raw.amd_pm_version.is_none() {
                    out.push((
                        "amd.pm.version".to_owned(),
                        "N/A (absent / read failed)".to_owned(),
                    ));
                }
                if raw.amd_pm_blob_len.is_none() {
                    out.push((
                        "amd.pm.blob_len".to_owned(),
                        "N/A (absent / read failed)".to_owned(),
                    ));
                }
                for (name, value) in amd_pm_sections(raw) {
                    if value.is_none() {
                        out.push((
                            format!("amd.pm.{name}"),
                            "N/A (absent / read failed)".to_owned(),
                        ));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (chunk-probe-1a: the wire-shape pins).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::{AmdZen, CpuInfo, CpuVendor, IntelGen};
    use crate::error::{NaReason, Section};
    use crate::platform::SystemPlatform;
    use crate::spd_decode::SpdModule;

    /// A fully-populated [`ProbeRaw`] fixture (representative Intel raw
    /// dump: populated ch0/ch1/ch2 + mcl0/mcl1 + all MAD raws, absent
    /// ch3, MCHBAR base + enable — host-independent).
    fn fixture_raw() -> ProbeRaw {
        ProbeRaw {
            mcbios_req: Some(0x0000_0012),
            tc_ch0_dbp: Some(0x1111_0F11),
            tc_ch0_rap: Some(0x2718_0204),
            tc_ch0_rfp: Some(0x0000_01A4),
            tc_ch0_rap2: Some(0x0000_0C0A),
            tc_ch0_rdrd: Some(0x0048_C286),
            tc_ch0_rdwr: Some(0x0000_0280),
            tc_ch0_wrrd: Some(0x0000_0308),
            tc_ch0_wrwr: Some(0x0040_C204),
            tc_ch1_dbp: Some(0x1111_0F11),
            tc_ch1_rap: Some(0x2718_0204),
            tc_ch1_rfp: Some(0x0000_01A4),
            tc_ch1_rap2: Some(0x0000_0C0A),
            tc_ch1_rdrd: Some(0x0048_C286),
            tc_ch1_rdwr: Some(0x0000_0280),
            tc_ch1_wrrd: Some(0x0000_0308),
            tc_ch1_wrwr: Some(0x0040_C204),
            tc_ch2_dbp: Some(0x2222_1F22),
            tc_ch2_rap: Some(0x3333_1111),
            tc_ch2_rfp: Some(0x0000_01B4),
            tc_ch2_rap2: Some(0x0000_0C1A),
            tc_ch2_rdrd: Some(0x0048_C287),
            tc_ch2_rdwr: Some(0x0000_0284),
            tc_ch2_wrrd: Some(0x0000_0318),
            tc_ch2_wrwr: Some(0x0040_C214),
            // ch3 (the Alder subchannel-3 mirror) is absent here.
            tc_ch3_dbp: None,
            tc_ch3_rap: None,
            tc_ch3_rfp: None,
            tc_ch3_rap2: None,
            tc_ch3_rdrd: None,
            tc_ch3_rdwr: None,
            tc_ch3_wrrd: None,
            tc_ch3_wrwr: None,
            mcl0_pre: Some(0x0028_8410),
            mcl0_act: Some(0x1228_4D28),
            mcl0_act2: Some(0x0000_0008),
            mcl0_wtr: Some(0x0828_0E0C),
            mcl0_rfp: Some(0x0000_04B0),
            mcl0_rfp2: Some(0x0000_0087),
            mcl0_rdrd: Some(0x0048_C286),
            mcl0_wrwr: Some(0x0000_0286),
            mcl1_pre: Some(0x0028_8410),
            mcl1_act: Some(0x1228_4D28),
            mcl1_act2: Some(0x0000_0008),
            mcl1_wtr: Some(0x0828_0E0C),
            mcl1_rfp: Some(0x0000_04B0),
            mcl1_rfp2: Some(0x0000_0087),
            mcl1_rdrd: Some(0x0048_C286),
            mcl1_wrwr: Some(0x0000_0286),
            mad_inter_channel: Some(0x0000_0003),
            mad_intra_ch0: Some(0x0000_0005),
            mad_intra_ch1: Some(0x0000_0007),
            mad_dimm_ch0: Some(0x0000_0008),
            mad_dimm_ch1: Some(0x0000_000C),
            mad_dimm_ch2: Some(0x0000_0010),
            mad_dimm_ch3: Some(0x0000_0014),
            mchbar_base: Some(0xFED1_0000),
            mchbar_enabled: true,
            // AMD raw: absent (the Intel fixture carries no AMD data).
            ..Default::default()
        }
    }

    /// The [`ProbeSystem`] fixture (host-independent strings).
    fn fixture_system() -> ProbeSystem {
        ProbeSystem {
            cpu_brand: "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz".to_owned(),
            cpu_vendor: "Intel".to_owned(),
            cpu_gen: "RocketLake".to_owned(),
            pci_host_bridge: Some("8086:4250".to_owned()),
            kernel: "6.6.0-1-cachyos".to_owned(),
            os: "Linux / Arch".to_owned(),
            arch: "x86_64".to_owned(),
            ramsleuth_version: "2.4.6".to_owned(),
            telemetry_source: "ramsleuth_intel".to_owned(),
        }
    }

    /// A representative [`SystemMemoryTelemetry`] fixture (an Intel
    /// snapshot, `Na` vendor branches, one SPD module, mixed platform —
    /// host-independent).
    fn fixture_telemetry() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::RocketLake),
                brand: "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz".to_owned(),
            },
            amd: Section::na(NaReason::UnsupportedHardware),
            intel: Section::na(NaReason::DriverMissing),
            spd: vec![SpdModule {
                index: 0x52,
                is_ddr5: false,
                maker: Section::Value("0xC1".to_owned()),
                die_maker: Section::na(NaReason::NotApplicable),
                die_type: Section::na(NaReason::NotApplicable),
                devices: Section::Value(8),
                part: Section::na(NaReason::NotApplicable),
                serial: Section::na(NaReason::NotApplicable),
                rank: Section::Value(1),
                density_mbit: Section::Value(16_384),
                speed_mts: Section::Value(3_200),
                profiles: Vec::new(),
            }],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3600.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
        }
    }

    /// (1) A fully-populated [`ProbeReport`] (raw `Some` + system)
    /// round-trips through bincode and compares equal — every field of
    /// the report crosses the wire (the wire-shape pin).
    #[test]
    fn probe_report_bincode_round_trip_populated() {
        let report = ProbeReport {
            telemetry: fixture_telemetry(),
            raw: Some(fixture_raw()),
            system: fixture_system(),
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    /// (2) A [`ProbeReport`] with `raw: None` (the AMD / unknown-silicon
    /// arm) round-trips identically — the `Option<ProbeRaw>` arm crosses
    /// the wire in both states.
    #[test]
    fn probe_report_bincode_round_trip_no_raw() {
        let report = ProbeReport {
            telemetry: SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Amd(AmdZen::Zen3),
                    brand: "AMD Ryzen 9 5950X".to_owned(),
                },
                amd: Section::na(NaReason::DriverMissing),
                intel: Section::na(NaReason::UnsupportedHardware),
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
            },
            raw: None,
            system: ProbeSystem {
                cpu_brand: "AMD Ryzen 9 5950X".to_owned(),
                cpu_vendor: "AMD".to_owned(),
                cpu_gen: "Zen3".to_owned(),
                pci_host_bridge: None,
                kernel: "6.6.0-1-cachyos".to_owned(),
                os: "Linux / Arch".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.6".to_owned(),
                telemetry_source: "ryzen_smu".to_owned(),
            },
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    /// (3) The live `collect()` snapshot, wrapped in a [`ProbeReport`]
    /// with a `None` raw, bincode-serializes, round-trips, and compares
    /// equal — the whole report (snapshot + identity) is wire-safe on
    /// this host (no-panic contract).
    #[test]
    fn probe_report_live_telemetry_round_trip() {
        let report = ProbeReport {
            telemetry: crate::collect(),
            raw: None,
            system: ProbeSystem {
                cpu_brand: "live".to_owned(),
                cpu_vendor: "live".to_owned(),
                cpu_gen: "live".to_owned(),
                pci_host_bridge: None,
                kernel: "live".to_owned(),
                os: "live".to_owned(),
                arch: "live".to_owned(),
                ramsleuth_version: "2.4.6".to_owned(),
                telemetry_source: "live".to_owned(),
            },
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    // ------------------------------------------------------------------
    // (chunk-probe-2: the markdown renderer pins).
    // ------------------------------------------------------------------

    use crate::amd_readout::EccStatus;
    use crate::intel_readout::{ChannelMode, IntelChannel};

    /// (4) The sample (fully-populated) [`ProbeReport`] renders to the
    /// six issue-template sections with the expected headers + key
    /// values: the summary line, the System table, the branch `N/A`
    /// cells, the SPD module, the platform identity + derived
    /// capacities, the hex raw table (incl. the absent ch3 block), the
    /// N/A-reason list, and the footer.
    #[test]
    fn render_probe_report_md_has_all_sections_and_key_values() {
        let report = ProbeReport {
            telemetry: fixture_telemetry(),
            raw: Some(fixture_raw()),
            system: fixture_system(),
        };
        let md = render_probe_report_md(&report);

        // The six section headers (the issue-template mirror).
        assert!(md.contains("# RamSleuth Probe Report"), "{md}");
        assert!(md.contains("## System"), "{md}");
        assert!(md.contains("## Decoded Telemetry"), "{md}");
        assert!(md.contains("## Raw Registers"), "{md}");
        assert!(md.contains("## N/A Reasons"), "{md}");
        assert!(md.contains("---"), "{md}");

        // The one-line summary (CPU brand + gen + OS + kernel + version).
        assert!(
            md.contains(
                "> **Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz** — RocketLake on Linux / Arch (kernel 6.6.0-1-cachyos), RamSleuth v2.4.6"
            ),
            "{md}"
        );

        // The System table (the template's nine fields).
        assert!(md.contains("| CPU | Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz |"), "{md}");
        assert!(md.contains("| Vendor | Intel |"), "{md}");
        assert!(md.contains("| Generation | RocketLake |"), "{md}");
        assert!(md.contains("| PCI Host Bridge | 8086:4250 |"), "{md}");
        assert!(md.contains("| Kernel | 6.6.0-1-cachyos |"), "{md}");
        assert!(md.contains("| OS | Linux / Arch |"), "{md}");
        assert!(md.contains("| Arch | x86_64 |"), "{md}");
        assert!(md.contains("| RamSleuth Version | 2.4.6 |"), "{md}");
        assert!(md.contains("| Telemetry Source | ramsleuth_intel |"), "{md}");

        // Decoded Telemetry: the branch N/A cells, the SPD module, the
        // platform identity + derived capacities.
        assert!(md.contains("N/A (unsupported hardware)"), "{md}");
        assert!(md.contains("N/A (driver missing)"), "{md}");
        assert!(md.contains("#### Module 0x52 (DDR4)"), "{md}");
        assert!(md.contains("| maker | 0xC1 |"), "{md}");
        assert!(md.contains("| rank | 1 |"), "{md}");
        assert!(md.contains("| density | 16384 Mbit |"), "{md}");
        assert!(md.contains("| speed | 3200 MT/s |"), "{md}");
        assert!(md.contains("| motherboard | Test Board |"), "{md}");
        assert!(md.contains("| BIOS | 1.0 |"), "{md}");
        assert!(md.contains("16.0 GiB"), "{md}");

        // Raw Registers: the hex values in 0x form + the absent ch3
        // block + the MCHBAR diagnostics.
        assert!(md.contains("| MC_BIOS_REQ | 0x00000012 |"), "{md}");
        assert!(md.contains("| TC_CH0_DBP | 0x11110F11 |"), "{md}");
        assert!(md.contains("| TC_CH1_WRWR | 0x0040C204 |"), "{md}");
        assert!(md.contains("| MCL0_ACT | 0x12284D28 |"), "{md}");
        assert!(md.contains("| MAD_DIMM_CH3 | 0x00000014 |"), "{md}");
        assert!(md.contains("| TC_CH3_DBP | N/A (absent) |"), "{md}");
        assert!(md.contains("| MCHBAR_BASE | 0xFED10000 |"), "{md}");
        assert!(md.contains("| MCHBAR_ENABLED | true |"), "{md}");

        // N/A Reasons: the branch reasons, the SPD module N/A fields,
        // the absent ch3 raws, and the platform N/A fields.
        assert!(md.contains("`amd (whole branch)` — N/A (unsupported hardware)"), "{md}");
        assert!(md.contains("`intel (whole branch)` — N/A (driver missing)"), "{md}");
        assert!(md.contains("`spd[0x52].die_maker` — N/A (not applicable)"), "{md}");
        assert!(
            md.contains("`raw.tc_ch3_dbp` — N/A (register absent / read failed)"),
            "{md}"
        );
        assert!(md.contains("`platform.agesa` — N/A (not applicable)"), "{md}");

        // The footer: the generating version + the no-personal-info note.
        assert!(
            md.contains(
                "_This report was auto-generated by RamSleuth v2.4.6. It contains no personal information (no username, hostname, IP, MAC, or serial numbers)._"
            ),
            "{md}"
        );
    }

    /// (5) A populated two-channel Intel readout renders the readout-
    /// level channel mode, each channel's clocks (MCLK + the derived
    /// MT/s = MCLK × 2 + UCLK + gear), all 27 timings, the CAD bus,
    /// the voltages, and the channel RTL; `raw: None` renders the
    /// not-captured note.
    #[test]
    fn render_probe_report_md_intel_readout_renders_all_display_sets() {
        fn na<T>() -> Section<T> {
            Section::na(NaReason::NotApplicable)
        }
        fn populated_channel(index: u8) -> IntelChannel {
            let v = |x: u16| Section::Value(x);
            IntelChannel {
                index,
                clocks: ClockReadout {
                    mclk_mhz: Section::Value(2400.0),
                    uclk_mhz: Section::Value(1200.0),
                    fclk_mhz: na(),
                    div_mode: na(),
                    gear_mode: Section::Value(GearMode::Two),
                    gdm: na(),
                    pdm: na(),
                    command_rate: na(),
                },
                timings: TimingSet {
                    cl: v(16),
                    rcwdwr: v(16),
                    rcdrd: v(16),
                    rp: v(16),
                    ras: v(36),
                    rc: v(52),
                    rrds: v(4),
                    rrld: v(8),
                    faw: v(16),
                    wtrs: v(4),
                    wtrl: v(12),
                    wr: v(20),
                    rfc1: v(75),
                    rfc2: na(),
                    rfcsb: v(38),
                    cwl: v(12),
                    rtp: v(8),
                    rdwr: v(8),
                    wrrd: v(4),
                    rdrd_sd: v(4),
                    rdrd_dd: v(8),
                    rdrd_scl: v(8),
                    rdrd_sc: v(8),
                    wrwr_sd: v(4),
                    wrwr_dd: v(8),
                    wrwr_scl: v(8),
                    wrwr_sc: v(8),
                },
                cad_bus: CadBus {
                    proc_odt: na(),
                    rtt_nom: na(),
                    rtt_wr: na(),
                    rtt_park: na(),
                    clk_drv: na(),
                    addr_cmd_drv: na(),
                    cs_odt_drv: na(),
                    cke_drv: na(),
                },
                voltages: VoltageSet {
                    vddcr_soc_mv: na(),
                    vddio_mem_mv: na(),
                    vdd_misc_mv: na(),
                    vpp_mv: na(),
                    vcore_mv: na(),
                },
                rtl: na(),
            }
        }
        let report = ProbeReport {
            telemetry: SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Intel(IntelGen::AlderLake),
                    brand: "Intel(R) Core(TM) i7-12700K CPU @ 3.60GHz".to_owned(),
                },
                amd: Section::na(NaReason::UnsupportedHardware),
                intel: Section::Value(IntelReadout {
                    channels: vec![populated_channel(0), populated_channel(1)],
                    channel_mode: Some(ChannelMode::DualSymmetric),
                    ecc_status: EccStatus::Unknown,
                }),
                spd: Vec::new(),
                platform: SystemPlatform {
                    cpu_clock_mhz: Section::Value(3600.0),
                    motherboard: Section::Value("Test Board".to_owned()),
                    bios: Section::Value("1.0".to_owned()),
                    agesa: Section::na(NaReason::NotApplicable),
                    smu_version: Section::na(NaReason::NotApplicable),
                },
                total_capacity: Section::Value(32.0),
                dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
            },
            raw: None,
            system: ProbeSystem {
                cpu_brand: "Intel(R) Core(TM) i7-12700K CPU @ 3.60GHz".to_owned(),
                cpu_vendor: "Intel".to_owned(),
                cpu_gen: "AlderLake".to_owned(),
                pci_host_bridge: None,
                kernel: "6.6.0-1-cachyos".to_owned(),
                os: "Linux / Arch".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.6".to_owned(),
                telemetry_source: "unavailable".to_owned(),
            },
        };
        let md = render_probe_report_md(&report);

        // The readout-level channel mode leads the Intel section.
        assert!(md.contains("| channel mode | Dual-Channel (Symmetric) |"), "{md}");
        // Both channels render their own blocks.
        assert!(md.contains("#### Channel 0"), "{md}");
        assert!(md.contains("#### Channel 1"), "{md}");
        // Clocks + ratios incl. the derived MT/s = MCLK × 2.
        assert!(md.contains("| MCLK | 2400.00 MHz |"), "{md}");
        assert!(md.contains("| MT/s | 4800 MT/s |"), "{md}");
        assert!(md.contains("| UCLK | 1200.00 MHz |"), "{md}");
        assert!(md.contains("| gear mode | 2x |"), "{md}");
        // All 27 timings per channel (the canonical display names).
        for name in [
            "tCL", "tRCDWR", "tRCDRD", "tRP", "tRAS", "tRC", "tRRDS", "tRRLD", "tFAW",
            "tWTRS", "tWTRL", "tWR", "tRFC1", "tRFC2", "tRFCsb", "tCWL", "tRTP", "tRDWR",
            "tWRRD", "tRDRD(SD)", "tRDRD(CCD)", "tRDRD(SCL)", "tRDRD(SC)", "tWRWR(SD)",
            "tWRWR(CCD)", "tWRWR(SCL)", "tWRWR(SC)",
        ] {
            assert!(md.contains(&format!("| {name} |")), "{md}");
        }
        // The populated values + the Na cells.
        assert!(md.contains("| tCL | 16 |"), "{md}");
        assert!(md.contains("| tRC | 52 |"), "{md}");
        assert!(md.contains("N/A (not applicable)"), "{md}");
        // The N/A-reason paths for one channel's degraded cells.
        assert!(md.contains("`intel.ch0.clocks.fclk` — N/A (not applicable)"), "{md}");
        assert!(md.contains("`intel.ch0.timings.rfc2` — N/A (not applicable)"), "{md}");
        assert!(md.contains("`intel.ch1.rtl` — N/A (not applicable)"), "{md}");
        // raw: None renders the not-captured note + the raw N/A reason.
        assert!(md.contains("not captured"), "{md}");
        assert!(md.contains("`raw (whole dump)` — not captured"), "{md}");
        // The empty SPD list note.
        assert!(md.contains("no modules"), "{md}");
    }

    /// An AMD [`ProbeRaw`] fixture: the 13 SMN words (the last a failed
    /// read) + the PM version / blob length + the five key f32 bit patterns;
    /// every Intel field absent (an AMD report carries none).
    fn amd_fixture_raw() -> ProbeRaw {
        ProbeRaw {
            amd_smn_regs: vec![
                Some(0x0000_1539),
                Some(0x1010_2410),
                Some(0x0010_0030),
                Some(0x0400_0404),
                Some(0x0000_0010),
                Some(0x0008_0410),
                Some(0x0000_0010),
                Some(0x0504_0302),
                Some(0x0908_0706),
                Some(0x0000_0602),
                Some(0x0400_0000),
                Some(0x7E08_20A0),
                None, // 0x50264: a failed read
            ],
            amd_pm_version: Some(0x0038_0805),
            amd_pm_blob_len: Some(2288),
            amd_pm_vddcr_vdd: Some(0x3E4C_CCCD),
            amd_pm_vddcr_soc: Some(0x3EF2_CCCC),
            amd_pm_fclk: Some(0x40F0_0000),
            amd_pm_uclk: Some(0x40C8_0000),
            amd_pm_mclk: Some(0x40C8_0000),
            // All Intel raw: absent (an AMD report carries none).
            ..Default::default()
        }
    }

    /// An AMD [`ProbeReport`] fixture: the AMD SMN/PM raw section populated,
    /// the Intel raw absent, an AMD system identity (host-independent).
    fn amd_fixture_report() -> ProbeReport {
        ProbeReport {
            telemetry: SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Amd(AmdZen::Zen3),
                    brand: "AMD Ryzen 9 5950X".to_owned(),
                },
                amd: Section::na(NaReason::DriverMissing),
                intel: Section::na(NaReason::UnsupportedHardware),
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
            },
            raw: Some(amd_fixture_raw()),
            system: ProbeSystem {
                cpu_brand: "AMD Ryzen 9 5950X".to_owned(),
                cpu_vendor: "AMD".to_owned(),
                cpu_gen: "Zen3".to_owned(),
                pci_host_bridge: None,
                kernel: "6.6.0-1-cachyos".to_owned(),
                os: "Linux / Arch".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.6".to_owned(),
                telemetry_source: "ryzen_smu".to_owned(),
            },
        }
    }

    /// (7) A [`ProbeReport`] with an AMD raw section (the 13 SMN words + the
    /// PM key values; all Intel raw `None`) round-trips through bincode and
    /// compares equal — the appended AMD fields cross the wire (OQ-10
    /// append-only).
    #[test]
    fn probe_report_bincode_round_trip_amd_raw() {
        let report = amd_fixture_report();
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    /// (8) An AMD [`ProbeReport`] (raw = the AMD SMN/PM section, Intel raw
    /// absent) renders the AMD table in the `## Raw Registers` section: the
    /// 13 SMN register rows (named by address, the failed word as N/A), the
    /// PM version / blob length, and the five f32 bit patterns — and the
    /// N/A-reason list carries the AMD per-register entry for the failed
    /// word.
    #[test]
    fn render_probe_report_md_amd_raw_section() {
        let report = amd_fixture_report();
        let md = render_probe_report_md(&report);

        // The AMD SMN table is present (not the Intel IMC table).
        assert!(md.contains("### AMD SMN Registers"), "{md}");
        assert!(
            !md.contains("MC_BIOS_REQ"),
            "an AMD report must not render the Intel IMC table: {md}"
        );
        // The 13 SMN rows, named by base address; the failed word is N/A.
        assert!(md.contains("| SMN_0x50200 | 0x00001539 |"), "{md}");
        assert!(md.contains("| SMN_0x50204 | 0x10102410 |"), "{md}");
        assert!(md.contains("| SMN_0x50264 | N/A (absent) |"), "{md}");
        // The PM table: version, blob length, the five f32 bit patterns.
        assert!(md.contains("### AMD PM Table"), "{md}");
        assert!(md.contains("| PM Version | 0x00380805 |"), "{md}");
        assert!(md.contains("| PM Blob Length | 2288 bytes |"), "{md}");
        assert!(md.contains("| VDDCR_VDD | 0x3E4CCCCD (f32 bits) |"), "{md}");
        assert!(md.contains("| MCLK | 0x40C80000 (f32 bits) |"), "{md}");
        // The N/A reasons carry the AMD per-register entry for the failed
        // word.
        assert!(
            md.contains("`amd.smn.smn_0x50264` — N/A (register absent / read failed)"),
            "{md}"
        );
    }
}
