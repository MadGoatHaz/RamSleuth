//! Verification CLI (P2-11) — the thin front end for [`collect`].
//!
//! Renders the full [`SystemMemoryTelemetry`] snapshot as either a
//! **human-readable** dashboard (CLI-4) or a JSON block (no serde —
//! P1-11 precedent). Every section renders its formatted values: a
//! [`Section::Value`] shows the formatted reading, a [`Section::Na`] shows
//! the **human-readable** `N/A (<reason>)` phrase (CLI-3 — the [`NaReason`]
//! `Display` source, DAEMON-2), so a fully degraded snapshot still renders
//! the complete dashboard and the process **exits 0 even when everything
//! is N/A** (N/A is a valid, structured outcome, not a failure).
//! `-h`/`--help` and `--version`/`-V` short-circuit (exit `0`) before the
//! snapshot is rendered; the only non-zero exit is a bad command-line flag
//! (2). No clap; `std::env::args` only.
//!
//! **Local renderer (CLI-4, a documented, bounded micro-duplication):**
//! this diagnostic bin **cannot** depend on `ramsleuth-display` (a cycle:
//! `display -> telemetry`), so it carries a small local human-readable
//! section renderer adapted from the CLI `dump` formatters. The three real
//! frontends (TUI / GUI / client) are fully single-sourced via
//! `ramsleuth-display`; only this standalone bin keeps the local copy.

use ramsleuth_telemetry::amd_readout::{
    AmdReadout, CadBus, ClockReadout, DivMode, GearMode, RttValue, TimingSet, VoltageSet,
};
use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::intel_readout::{IntelChannel, IntelReadout};
use ramsleuth_telemetry::intel_smbios::{self, SmbiosTopology};
use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
use ramsleuth_telemetry::{collect, SystemMemoryTelemetry, VoltageTelemetry};

/// Command-line options (parsed by hand — no clap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Opts {
    /// Emit the snapshot as JSON instead of the dashboard text.
    json: bool,
}

/// Usage text (printed for `--help` / `-h`).
const USAGE: &str = "\
ramsleuth-telemetry — the standalone RamSleuth system memory telemetry
snapshot

CPU vendor/brand, the vCore reading, the AMD SMU section, the Intel IMC
section, and the SPD modules (including XMP/EXPO profiles). Reads the
hardware directly — no daemon needed.

Usage: ramsleuth-telemetry [OPTIONS]

Options:
  --json       Print the snapshot as a hand-rolled JSON block
  -h, --help   Print this help and exit
  -V, --version Print the version and exit

Exit codes:
  0  snapshot rendered (even if every section is N/A)
  2  unknown flag
";

/// The `--version`/`-V` output line: `ramSleuth <bin> v<version>` —
/// the version is the crate's `CARGO_PKG_VERSION` (the workspace
/// release, so it tracks it automatically).
fn version_line() -> String {
    format!("ramSleuth ramsleuth-telemetry v{}", env!("CARGO_PKG_VERSION"))
}

/// The startup short-circuit for a raw argument (checked by `main`
/// *before* the full [`parse_opts`]): `Help` prints the usage text
/// (exit 0), `Version` prints [`version_line`] (exit 0), `None` means
/// the full parse proceeds (a genuinely unknown flag stays a usage
/// error, exit 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortCircuit {
    /// `--help` / `-h` — print the usage text.
    Help,
    /// `--version` / `-V` — print [`version_line`].
    Version,
    /// Neither — continue to the full parse.
    None,
}

/// Classify a raw argument list (the program name already removed) for
/// the startup short-circuit: the *first* `--help`/`-h` or
/// `--version`/`-V`, in argv order, wins; a list without either is
/// `None`.
fn short_circuit<I, S>(args: I) -> ShortCircuit
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for arg in args {
        match arg.as_ref() {
            "--help" | "-h" => return ShortCircuit::Help,
            "--version" | "-V" => return ShortCircuit::Version,
            _ => {}
        }
    }
    ShortCircuit::None
}

/// Parse argv (index 0 is the program name). `--json` sets `json`;
/// `-h`/`--help` / `-V`/`--version` are recognized (not an error —
/// `main` short-circuits them before parsing); any other argument is
/// an error naming that argument.
pub(crate) fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts { json: false };
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--json" => opts.json = true,
            // recognized; help / version are handled in `main`
            "-h" | "--help" | "-V" | "--version" => {}
            bad => return Err(bad.to_owned()),
        }
    }
    Ok(opts)
}

// ---------------------------------------------------------------------------
// Cell formatters (CLI-3 / CLI-4): one per value kind. Each renders a
// [`Section`] cell as its formatted value, or the **human-readable**
// `N/A (<reason>)` phrase (the [`NaReason`] `Display` source — DAEMON-2).
// A structurally not-applicable cell keeps the one bare `N/A`.
// ---------------------------------------------------------------------------

/// The human-readable text of an absent cell (CLI-3): the [`NaReason`]
/// phrase in the compact `N/A (<reason>)` form — `N/A (driver missing)`,
/// `N/A (unsupported CPU)`, …; a structurally not-applicable cell
/// ([`NaReason::NotApplicable`]) keeps the one bare `N/A`. Rendered locally
/// (this bin cannot depend on `ramsleuth-display` — a cycle).
///
/// Unlike the three real frontends (which keep the terse `decode failed`
/// tag for a [`NaReason::ParseError`] per the D-4 dense-grid rule), this
/// **diagnostic** bin renders a `ParseError`'s detail verbatim when it is
/// non-empty — the *true* reason (e.g. the server-uncore platform note),
/// never a mislabel of a structural absence as a decode failure — falling
/// back to `decode failed` for an empty detail.
fn na_cell(reason: &NaReason) -> String {
    match reason {
        NaReason::NotApplicable => "N/A".to_owned(),
        NaReason::ParseError(detail) if !detail.is_empty() => format!("N/A ({detail})"),
        r => format!("N/A ({r})"),
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

/// A derived data-rate cell: MCLK × 2 (the DDR double-pumping rule) in
/// MT/s, or the N/A reason text when the MCLK is absent.
fn mts_from_mclk(mclk: &Section<f64>) -> String {
    match mclk {
        Section::Value(value) => format!("{:.0} MT/s", value * 2.0),
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
/// decimals — the mV→V display rule).
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
    out.push_str("  ");
    out.push_str(&na_cell(reason));
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

/// The eight clocks & ratios rows of a [`ClockReadout`] (MCLK, the derived
/// MT/s data rate, UCLK, FCLK, and the mode / gear / power flags).
fn clocks_rows(clocks: &ClockReadout) -> [(&str, String); 8] {
    [
        ("MCLK", mhz(&clocks.mclk_mhz)),
        ("MT/s", mts_from_mclk(&clocks.mclk_mhz)),
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
// Section renderers (CLI-4): each returns the human-readable text of one
// `=== SECTION ===` block — shared by the dashboard and the JSON so the
// two never diverge (a `Na` branch is its reason line, never a Debug wall).
// ---------------------------------------------------------------------------

/// The CPU vendor as a display string (the **stable** name — DAEMON-2:
/// `AMD Zen 3`, `Intel Alder Lake`, `unknown` — not the `Debug` form).
fn vendor_text(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Amd(zen) => format!("AMD {zen}"),
        CpuVendor::Intel(gen) => format!("Intel {gen}"),
        CpuVendor::Unknown => "unknown".to_owned(),
    }
}

/// The `=== CPU ===` section: vendor + brand (always populated — the
/// snapshot root carries the detected CPU).
fn cpu_section_text(cpu: &CpuInfo) -> String {
    let mut out = String::new();
    header(&mut out, "CPU");
    rows(&mut out, "  ", &[("vendor", vendor_text(&cpu.vendor)), ("brand", cpu.brand.clone())]);
    out.push('\n');
    out
}

/// The single vCore reading (the snapshot-root's `vcore` field) as the CLI
/// layout-matrix form: the delivered value at mV precision, the canonical
/// `[est]` badge (the shared [`RailConfidence::badge`] source) for a
/// heuristic (vendor-fallback) reading, the optional VID + signed delta
/// (CLI-2: the wire's `delta_volts` when present, the `delivered − vid`
/// identity only when the wire omits it) when a VID is present, and a plain
/// `N/A` for the absent / enterprise / out-of-bounds states.
fn vcore_display(vcore: &Section<VoltageTelemetry>) -> String {
    match vcore {
        Section::Na(_) => "N/A".to_owned(),
        Section::Value(v) => {
            let mut s = format!("{:.3} V", v.delivered_volts);
            if let Some(badge) = v.confidence.badge() {
                s.push(' ');
                s.push_str(badge);
            }
            if let Some(vid) = v.requested_vid {
                let delta_mv = v.delta_volts.unwrap_or(v.delivered_volts - vid) * 1000.0;
                s.push_str(&format!(" (VID: {:.3} V, Δ {:+.0} mV)", vid, delta_mv));
            }
            s
        }
    }
}

/// The `=== Vcore ===` section: the single vCore reading (the vendor-aware
/// source of truth), one line.
fn vcore_section_text(vcore: &Section<VoltageTelemetry>) -> String {
    let mut out = String::new();
    header(&mut out, "Vcore");
    rows(&mut out, "  ", &[("CPU Core Voltage", vcore_display(vcore))]);
    out.push('\n');
    out
}

/// The `=== AMD ===` section: the four display sets in dashboard order, or
/// one `N/A (<reason>)` line when the branch degraded.
fn amd_section_text(section: &Section<AmdReadout>) -> String {
    let mut out = String::new();
    header(&mut out, "AMD");
    match section {
        Section::Na(reason) => {
            na_line(&mut out, reason);
            out.push('\n');
        }
        Section::Value(readout) => {
            subheader(&mut out, "Clocks & ratios");
            rows(&mut out, "  ", &clocks_rows(&readout.clocks));
            let timings = timings_rows(&readout.timings);
            subheader(&mut out, "Primary timings (ticks)");
            rows(&mut out, "  ", &timings[0..4]);
            subheader(&mut out, "Secondary timings (ticks)");
            rows(&mut out, "  ", &timings[4..9]);
            subheader(&mut out, "Tertiary & turnarounds (ticks)");
            rows(&mut out, "  ", &timings[9..27]);
            subheader(&mut out, "CAD bus (ohms)");
            rows(&mut out, "  ", &cad_rows(&readout.cad_bus));
            subheader(&mut out, "Voltages");
            rows(&mut out, "  ", &voltages_rows(&readout.voltages));
            out.push('\n');
        }
    }
    out
}

/// The per-DIMM capacity line: a uniform kit renders `<size> GiB × <count>`,
/// a mixed kit joins the per-DIMM sizes with ` + `.
fn dimm_line(sizes: &[u64]) -> String {
    if sizes.is_empty() {
        return "N/A".to_owned();
    }
    let first = sizes[0];
    if sizes.iter().all(|&s| s == first) {
        format!("{:.1} GiB × {}", first as f64 / 1024.0, sizes.len())
    } else {
        sizes
            .iter()
            .map(|&s| format!("{:.1} GiB", s as f64 / 1024.0))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// The `=== Intel ===` section: a readout-level `channel mode` line plus the
/// SMBIOS Type 17 capacity (total + per-DIMM — the firmware ground truth),
/// then one `--- Channel n ---` block per decoded IMC channel, or one
/// `N/A (<reason>)` line when the branch degraded.
fn intel_section_text(section: &Section<IntelReadout>, smbios: &SmbiosTopology) -> String {
    let mut out = String::new();
    header(&mut out, "Intel");
    match section {
        Section::Na(reason) => {
            na_line(&mut out, reason);
            out.push('\n');
        }
        Section::Value(readout) => {
            let mode = readout
                .channel_mode
                .map(|m| m.label().to_owned())
                .unwrap_or_else(|| "N/A".to_owned());
            // The SMBIOS Type 17 capacity (total + per-DIMM), shown
            // alongside the resolved channel mode.
            let total_text = if smbios.populated_count > 0 {
                format!("{:.1} GiB", smbios.total_capacity_mib as f64 / 1024.0)
            } else {
                "N/A".to_owned()
            };
            let per_dimm_text = if smbios.populated_count > 0 {
                dimm_line(&smbios.dimm_sizes)
            } else {
                "N/A".to_owned()
            };
            rows(
                &mut out,
                "  ",
                &[
                    ("channel mode", mode),
                    ("total capacity", total_text),
                    ("per-DIMM", per_dimm_text),
                ],
            );
            if readout.channels.is_empty() {
                out.push_str("  (no channels decoded)\n\n");
                return out;
            }
            for channel in &readout.channels {
                out.push_str(&intel_channel_text(channel));
            }
            out.push('\n');
        }
    }
    out
}

/// One `--- Channel n ---` block: the shared display sets plus the
/// channel-level RTL row.
fn intel_channel_text(channel: &IntelChannel) -> String {
    let index = channel.index;
    let mut out = String::new();
    subheader(&mut out, &format!("Channel {index}"));
    rows(&mut out, "  ", &clocks_rows(&channel.clocks));
    let timings = timings_rows(&channel.timings);
    subheader(&mut out, "Primary timings (ticks)");
    rows(&mut out, "  ", &timings[0..4]);
    subheader(&mut out, "Secondary timings (ticks)");
    rows(&mut out, "  ", &timings[4..9]);
    subheader(&mut out, "Tertiary & turnarounds (ticks)");
    rows(&mut out, "  ", &timings[9..27]);
    subheader(&mut out, "CAD bus (ohms)");
    rows(&mut out, "  ", &cad_rows(&channel.cad_bus));
    subheader(&mut out, "Voltages");
    rows(&mut out, "  ", &voltages_rows(&channel.voltages));
    subheader(&mut out, "RTL");
    rows(&mut out, "  ", &[("RTL (ticks)", cell(&channel.rtl))]);
    out
}

/// One `--- Module 0xNN (DDRn) ---` block: the module identity rows plus
/// one line per factory-rated profile.
fn spd_module_text(module: &SpdModule) -> String {
    let index = module.index;
    let gen = if module.is_ddr5 { "DDR5" } else { "DDR4" };
    let mut out = String::new();
    subheader(&mut out, &format!("Module 0x{index:02X} ({gen})"));
    rows(
        &mut out,
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
        return out;
    }
    for profile in &module.profiles {
        out.push_str(&spd_profile_text(module.is_ddr5, profile));
    }
    out
}

/// One profile line: `profile <n> (XMP|EXPO): <speed>, <tCL>-<tRCD>-<tRP>-<tRAS> @ <voltage>`.
fn spd_profile_text(is_ddr5: bool, profile: &SpdProfile) -> String {
    let scheme = if is_ddr5 { "EXPO" } else { "XMP" };
    let index = profile.index;
    let speed = mts(&profile.speed_mts);
    let cas = cell(&profile.cas);
    let trcd = cell(&profile.trcd);
    let trp = cell(&profile.trp);
    let tras = cell(&profile.tras);
    let voltage = volts(&profile.voltage);
    format!(
        "  profile {index} ({scheme}): {speed}, {cas}-{trcd}-{trp}-{tras} @ {voltage}\n"
    )
}

/// The `=== SPD ===` section: one `--- Module 0xNN ---` block per decoded
/// module, or a no-modules note when the list is empty.
fn spd_section_text(modules: &[SpdModule]) -> String {
    let mut out = String::new();
    header(&mut out, "SPD");
    if modules.is_empty() {
        out.push_str("  (no modules: ee1004 driver absent or no device bound)\n\n");
        return out;
    }
    for module in modules {
        out.push_str(&spd_module_text(module));
    }
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// The public surface.
// ---------------------------------------------------------------------------

/// The report title line (ASCII only: the `=` rule below is exactly
/// `len()` wide, so no multi-byte characters in the title).
const TITLE: &str = "RamSleuth - system memory telemetry";

/// Render one [`SystemMemoryTelemetry`] snapshot as the human-readable
/// dashboard (CLI-4): the CPU, vCore, AMD, Intel, and SPD sections —
/// every cell its formatted value or the human-readable `N/A (<reason>)`
/// phrase, never a `Debug` wall. Pure and deterministic: the same
/// snapshot always yields the same `String`; a fully degraded snapshot
/// renders the complete dashboard without panicking (the no-panic
/// contract, D5).
pub(crate) fn render_dashboard(t: &SystemMemoryTelemetry, smbios: &SmbiosTopology) -> String {
    let mut out = String::new();
    out.push_str(TITLE);
    out.push('\n');
    out.push_str(&"=".repeat(TITLE.len()));
    out.push('\n');
    out.push('\n');
    out.push_str(&cpu_section_text(&t.cpu));
    out.push_str(&vcore_section_text(&t.vcore));
    out.push_str(&amd_section_text(&t.amd));
    out.push_str(&intel_section_text(&t.intel, smbios));
    out.push_str(&spd_section_text(&t.spd));
    out
}

/// Escape a string for embedding in a JSON double-quoted literal:
/// backslash, double-quote, newline, tab, and carriage-return by name,
/// and every other control character as `\u00XX`.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Hand-rolled JSON for the whole snapshot (no serde — P1-11 precedent).
///
/// CLI-4: carries the **same fields** the dashboard shows — `cpu`
/// (vendor + brand), `vcore`, `amd`, `intel`, and `spd` — so the two forms
/// never diverge. A degraded vendor section is `null`; the `vcore` field is
/// the same human-readable string the dashboard's `=== Vcore ===` section
/// prints (`vcore_display`).
pub(crate) fn to_json(t: &SystemMemoryTelemetry, smbios: &SmbiosTopology) -> String {
    let amd = match &t.amd {
        Section::Na(_) => "null".to_owned(),
        _ => format!("\"{}\"", json_escape(&amd_section_text(&t.amd))),
    };
    let intel = match &t.intel {
        Section::Na(_) => "null".to_owned(),
        _ => format!("\"{}\"", json_escape(&intel_section_text(&t.intel, smbios))),
    };
    let spd = t
        .spd
        .iter()
        .map(|m| format!("\"{}\"", json_escape(&spd_module_text(m))))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"cpu\":{{\"vendor\":\"{}\",\"brand\":\"{}\"}},\"vcore\":\"{}\",\"amd\":{},\"intel\":{},\"spd\":[{}]}}",
        json_escape(&vendor_text(&t.cpu.vendor)),
        json_escape(&t.cpu.brand),
        json_escape(&vcore_display(&t.vcore)),
        amd,
        intel,
        spd,
    )
}

/// CLI entry: parse flags, collect the snapshot, render it, exit 0.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Help / version short-circuit before parsing: print the usage /
    // the version line, exit 0.
    match short_circuit(args.iter().skip(1)) {
        ShortCircuit::Help => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        ShortCircuit::Version => {
            println!("{}", version_line());
            std::process::exit(0);
        }
        ShortCircuit::None => {}
    }
    match parse_opts(&args) {
        Err(bad) => {
            eprintln!("unknown flag: {bad}");
            std::process::exit(2);
        }
        Ok(opts) => {
            let t = collect();
            // The SMBIOS Type 17 topology (unprivileged dmi read) feeds the
            // Intel section's capacity line and the server-uncore speed
            // fallback (the SPD EEPROM is not OS-visible on those platforms).
            let smbios = intel_smbios::collect();
            if opts.json {
                println!("{}", to_json(&t, &smbios));
            } else {
                print!("{}", render_dashboard(&t, &smbios));
            }
            // N/A is a valid, structured outcome: exit 0 in every
            // reachable render state (no-panic contract, plan D5).
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_telemetry::amd_readout::{CommandRate, EccStatus, MemoryChannelMode};
    use ramsleuth_telemetry::cpuid::{AmdZen, IntelGen};
    use ramsleuth_telemetry::intel_readout::{
        ChannelMode, ChannelRegs, MemoryTech, fixture_channel_regs, fixture_intel_readout,
    };
    use ramsleuth_telemetry::intel_smbios::DerivedMode;
    use ramsleuth_telemetry::{RailConfidence, SystemPlatform};

    /// A synthetic all-Na snapshot (pure — never calls `collect()`).
    fn all_na() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Na(NaReason::DriverMissing),
            intel: Section::Na(NaReason::UnsupportedHardware),
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
            vcore: Section::na(NaReason::NotApplicable),
        }
    }

    /// A representative AMD-host snapshot: a populated `AmdReadout`, an
    /// `Na` Intel branch (an AMD host), two SPD modules (DDR4 + DDR5), a
    /// verified vCore. (Host-independent — no `collect()`.)
    fn representative() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(AmdReadout {
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
                    rfc2: Section::Value(40),
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
                channel_mode: MemoryChannelMode::DualSymmetric,
                ecc_status: EccStatus::CapableButDisabled,
            }),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: vec![
                SpdModule {
                    index: 0x52,
                    is_ddr5: false,
                    maker: Section::Value("Samsung".to_owned()),
                    die_maker: Section::Value("SK hynix".to_owned()),
                    die_type: Section::na(NaReason::NotApplicable),
                    devices: Section::Value(8),
                    part: Section::Value("M391A2K40DB".to_owned()),
                    serial: Section::Value("S064531ABC".to_owned()),
                    rank: Section::Value(2),
                    density_mbit: Section::Value(16_384),
                    speed_mts: Section::Value(3_200),
                    profiles: Vec::new(),
                },
                SpdModule {
                    index: 0x53,
                    is_ddr5: true,
                    maker: Section::Value("Micron".to_owned()),
                    die_maker: Section::Value("Micron".to_owned()),
                    die_type: Section::na(NaReason::NotApplicable),
                    devices: Section::Value(16),
                    part: Section::Value("MTA36ANT".to_owned()),
                    serial: Section::Value("S064531XYZ".to_owned()),
                    rank: Section::Value(2),
                    density_mbit: Section::Value(16_384),
                    speed_mts: Section::Value(4_800),
                    profiles: vec![SpdProfile {
                        index: 1,
                        speed_mts: Section::Value(6_000),
                        cas: Section::Value(36),
                        trcd: Section::Value(36),
                        trp: Section::Value(36),
                        tras: Section::Value(76),
                        voltage: Section::Value(1_350),
                    }],
                },
            ],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::Value("PI 1.2.0.12".to_owned()),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(32.0),
            dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
            vcore: Section::Value(VoltageTelemetry {
                rail_name: "CPU Vcore".to_owned(),
                delivered_volts: 1.150,
                requested_vid: None,
                delta_volts: None,
                confidence: RailConfidence::VerifiedProfile,
            }),
        }
    }

    /// TC-CLI3 (the standalone N/A human-readable confirming test): a
    /// `Section::Na(DriverMissing)` renders the **human-readable phrase**
    /// `N/A (driver missing)` via the [`NaReason`] `Display` impl (DAEMON-2)
    /// — NOT the `Debug` form `N/A (DriverMissing)`. A structurally
    /// not-applicable cell keeps the one bare `N/A`. Pre-fix the `{:?}`
    /// printed the CamelCase variant name.
    #[test]
    fn tc_cli3_na_renders_the_human_phrase_not_debug() {
        assert_eq!(na_cell(&NaReason::DriverMissing), "N/A (driver missing)");
        assert_eq!(na_cell(&NaReason::UnsupportedHardware), "N/A (unsupported CPU)");
        assert_eq!(na_cell(&NaReason::InsufficientPrivilege), "N/A (insufficient privilege)");
        assert_eq!(na_cell(&NaReason::UnknownPmTableVersion), "N/A (new PM table)");
        // A non-empty `ParseError` detail renders the true reason verbatim;
        // an empty one falls back to the generic "decode failed" phrase.
        assert_eq!(na_cell(&NaReason::ParseError(String::new())), "N/A (decode failed)");
        assert_eq!(
            na_cell(&NaReason::ParseError(
                "server uncore: no OS-accessible timing source".to_owned()
            )),
            "N/A (server uncore: no OS-accessible timing source)"
        );
        // NotApplicable keeps the one bare N/A.
        assert_eq!(na_cell(&NaReason::NotApplicable), "N/A");
        // The Dashboard carries the phrase, never the Debug variant name.
        let out = render_dashboard(&all_na(), &SmbiosTopology::empty());
        assert!(out.contains("N/A (driver missing)"), "{out}");
        assert!(out.contains("N/A (unsupported CPU)"), "{out}");
        assert!(!out.contains("DriverMissing"), "no Debug form: {out}");
        assert!(!out.contains("UnsupportedHardware"), "no Debug form: {out}");
    }

    /// TC-CLI4 (the standalone dashboard human-readable + JSON consistency
    /// confirming test): `render_dashboard` for a canned snapshot shows the
    /// human-readable formatted values (NO `SpdModule {` / `ClockReadout {` /
    /// `AmdReadout {` `Debug` walls), and `to_json` carries the **same
    /// fields** the dashboard shows — including a `vcore` field matching the
    /// dashboard's `=== Vcore ===` line. Pre-fix the dashboard was a raw
    /// `{:?}` dump and `to_json` omitted `vcore` entirely.
    #[test]
    fn tc_cli4_dashboard_human_readable_and_json_carries_vcore() {
        let t = representative();
        let dash = render_dashboard(&t, &SmbiosTopology::empty());
        // No Debug walls anywhere in the dashboard.
        assert!(!dash.contains("SpdModule {"), "no SpdModule Debug wall: {dash}");
        assert!(!dash.contains("ClockReadout {"), "no ClockReadout Debug wall: {dash}");
        assert!(!dash.contains("AmdReadout {"), "no AmdReadout Debug wall: {dash}");
        assert!(!dash.contains("TimingSet {"), "no TimingSet Debug wall: {dash}");
        // The human-readable formatted values appear.
        for header in ["=== CPU ===", "=== Vcore ===", "=== AMD ===", "=== Intel ===", "=== SPD ==="] {
            assert!(dash.contains(header), "missing {header}: {dash}");
        }
        assert!(dash.contains("1600.00 MHz"), "formatted MCLK: {dash}");
        assert!(dash.contains("1.350 V"), "formatted voltage: {dash}");
        assert!(dash.contains("3200 MT/s"), "formatted speed: {dash}");
        assert!(dash.contains("Module 0x52 (DDR4)"), "SPD module block: {dash}");
        assert!(dash.contains("profile 1 (EXPO)"), "EXPO profile line: {dash}");
        assert!(dash.contains("CPU Core Voltage  1.150 V"), "vCore line: {dash}");
        // The JSON carries the same fields, including a vcore field that
        // matches the dashboard's vCore display (dashboard/JSON consistency).
        let json = to_json(&t, &SmbiosTopology::empty());
        for key in ["\"cpu\"", "\"vendor\"", "\"brand\"", "\"vcore\"", "\"amd\"", "\"intel\"", "\"spd\""] {
            assert!(json.contains(key), "missing {key} in: {json}");
        }
        let expected_vcore = format!("\"vcore\":\"{}\"", json_escape(&vcore_display(&t.vcore)));
        assert!(json.contains(&expected_vcore), "JSON vcore must match the dashboard's: {json}");
        // The degraded Intel branch is `null` in the JSON.
        assert!(json.contains("\"intel\":null"), "a Na branch is null: {json}");
    }

    /// (a) `render_dashboard` on an all-Na snapshot: every section prints
    /// its header plus its human-readable `N/A (<reason>)` line (empty SPD
    /// prints its no-modules note) — no panic, no omitted section, no
    /// Debug wall.
    #[test]
    fn dashboard_all_na_renders_structured() {
        let out = render_dashboard(&all_na(), &SmbiosTopology::empty());
        assert!(out.contains("=== CPU ==="));
        assert!(out.contains("=== Vcore ==="));
        assert!(out.contains("=== AMD ==="));
        assert!(out.contains("=== Intel ==="));
        assert!(out.contains("=== SPD ==="));
        assert!(out.contains("Ryzen 9 5950X"));
        assert!(out.contains("N/A (driver missing)"));
        assert!(out.contains("N/A (unsupported CPU)"));
        assert!(out.contains("(no modules: ee1004 driver absent or no device bound)"));
    }

    /// (b) `render_dashboard` with SPD modules: one `--- Module 0xNN ---`
    /// block per module, no no-modules note.
    #[test]
    fn dashboard_renders_spd_modules() {
        let out = render_dashboard(&representative(), &SmbiosTopology::empty());
        assert!(out.contains("Module 0x52 (DDR4)"));
        assert!(out.contains("Module 0x53 (DDR5)"));
        assert!(!out.contains("no modules"));
    }

    /// (b'') The vCore section: an absent vCore renders `N/A`, a heuristic
    /// (vendor-fallback) reading renders the mV-precision value + the
    /// `[est]` badge, and a verified reading with a synthetic VID renders the
    /// value + the signed (negative) delta.
    #[test]
    fn dashboard_vcore_renders_per_confidence() {
        // Absent → N/A.
        assert!(render_dashboard(&all_na(), &SmbiosTopology::empty())
            .contains("CPU Core Voltage  N/A"));

        // Heuristic (no VID) → the value + the `[est]` badge.
        let mut t = all_na();
        t.vcore = Section::Value(VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: 1.216,
            requested_vid: None,
            delta_volts: None,
            confidence: RailConfidence::HeuristicDefault,
        });
        assert!(render_dashboard(&t, &SmbiosTopology::empty())
            .contains("CPU Core Voltage  1.216 V [est]"));

        // Verified + a synthetic VID → the signed negative delta
        // (1.216 − 1.280 = −64 mV).
        let mut t = all_na();
        t.vcore = Section::Value(VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: 1.216,
            requested_vid: Some(1.280),
            delta_volts: Some(1.216 - 1.280),
            confidence: RailConfidence::VerifiedProfile,
        });
        let out = render_dashboard(&t, &SmbiosTopology::empty());
        assert!(out.contains("CPU Core Voltage  1.216 V (VID: 1.280 V, Δ -64 mV)"), "{out}");
        assert!(!out.contains("[est]"), "a verified vCore has no badge: {out}");
    }

    /// (c) `to_json` is structurally valid — carries all the dashboard's
    /// fields (cpu/vendor/brand/vcore/amd/intel/spd), a `Na` vendor branch
    /// is `null`, and a module is its human-readable escaped text.
    #[test]
    fn to_json_is_structurally_valid() {
        let t = representative();
        let out = to_json(&t, &SmbiosTopology::empty());
        assert!(out.starts_with('{'));
        assert!(out.ends_with('}'));
        for key in ["\"cpu\"", "\"vendor\"", "\"brand\"", "\"vcore\"", "\"amd\"", "\"intel\"", "\"spd\""] {
            assert!(out.contains(key), "missing key {key} in: {out}");
        }
        // The vcore field carries the same human-readable string the
        // dashboard's Vcore section prints.
        assert!(
            out.contains(&format!("\"vcore\":\"{}\"", json_escape(&vcore_display(&t.vcore)))),
            "vcore field must match the dashboard: {out}"
        );
    }

    /// (c') `json_escape` covers the whole escape table, including the
    /// `\u00XX` control-character arm.
    #[test]
    fn json_escape_escapes_specials_and_controls() {
        let raw = "a\"b\\c\nd\te\r\u{1}";
        assert_eq!(json_escape(raw), "a\\\"b\\\\c\\nd\\te\\r\\u0001");
        // Ordinary text (incl. non-ASCII) passes through untouched.
        assert_eq!(json_escape("Ryzen 9 5950X — ok"), "Ryzen 9 5950X — ok");
    }

    /// (d) `parse_opts`: `--json` sets the flag; `-h`/`--help` are
    /// recognized (no error — `main` short-circuits them); any other
    /// argument is an error naming that argument verbatim.
    #[test]
    fn parse_opts_flags() {
        let prog = "ramsleuth-telemetry".to_owned();
        let text = Opts { json: false };
        let json = Opts { json: true };

        assert_eq!(parse_opts(std::slice::from_ref(&prog)), Ok(text));

        let json_args = [prog.clone(), "--json".to_owned()];
        assert_eq!(parse_opts(&json_args), Ok(json));

        let h_args = [prog.clone(), "-h".to_owned()];
        assert_eq!(parse_opts(&h_args), Ok(text));

        let help_args = [prog.clone(), "--help".to_owned()];
        assert_eq!(parse_opts(&help_args), Ok(text));

        let v_args = [prog.clone(), "-V".to_owned()];
        assert_eq!(parse_opts(&v_args), Ok(text));

        let version_args = [prog.clone(), "--version".to_owned()];
        assert_eq!(parse_opts(&version_args), Ok(text));

        let bogus_args = [prog.clone(), "--bogus".to_owned()];
        assert_eq!(parse_opts(&bogus_args), Err("--bogus".to_owned()));
    }

    /// (e) The `--version`/`-V` line is the package version, exactly
    /// (`env!` — it tracks the workspace release).
    #[test]
    fn version_line_is_the_package_version() {
        assert_eq!(
            version_line(),
            format!("ramSleuth ramsleuth-telemetry v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// (f) `--help`/`-h` and `--version`/`-V` are the short-circuits:
    /// the first occurrence, in argv order, wins; anything else is
    /// `None`.
    #[test]
    fn short_circuit_classifies_help_version_and_none() {
        for h in ["--help", "-h"] {
            assert_eq!(short_circuit([h]), ShortCircuit::Help, "{h} → Help");
        }
        for v in ["--version", "-V"] {
            assert_eq!(short_circuit([v]), ShortCircuit::Version, "{v} → Version");
        }
        assert_eq!(short_circuit(Vec::<&str>::new()), ShortCircuit::None);
        assert_eq!(short_circuit(["--json"]), ShortCircuit::None);
        assert_eq!(
            short_circuit(["--help", "--version"]),
            ShortCircuit::Help,
            "the first occurrence wins"
        );
        assert_eq!(
            short_circuit(["--version", "--help"]),
            ShortCircuit::Version,
            "the first occurrence wins"
        );
    }

    /// (g) `vendor_text` is the stable name (DAEMON-2), not the `Debug`
    /// form — an `Amd(Zen3)` is `AMD Zen 3`, an `Intel(AlderLake)` is
    /// `Intel Alder Lake`, `Unknown` is `unknown`.
    #[test]
    fn vendor_text_is_the_stable_name() {
        assert_eq!(vendor_text(&CpuVendor::Amd(AmdZen::Zen3)), "AMD Zen 3");
        assert_eq!(vendor_text(&CpuVendor::Intel(IntelGen::AlderLake)), "Intel Alder Lake");
        assert_eq!(vendor_text(&CpuVendor::Unknown), "unknown");
    }

    /// (h) The Intel section renders per-channel (built through the
    /// **live** decode — the TEL-4 fixture constructor) — no Debug
    /// wall, the channel mode line + the decoded readings.
    #[test]
    fn intel_section_renders_per_channel() {
        let mut t = all_na();
        let mut ro = fixture_intel_readout(
            IntelGen::Skylake,
            Some(0x0000_000C), // ratio 12 @ 133.3333 MHz → 1600 MHz
            &fixture_channel_regs(),
            &ChannelRegs::default(),
            true, // the single-channel fixture shape
        );
        ro.channel_mode = Some(ChannelMode::DualFlex);
        t.intel = Section::Value(ro);
        let out = render_dashboard(&t, &SmbiosTopology::empty());
        assert!(!out.contains("IntelReadout {"), "no Debug wall: {out}");
        assert!(out.contains("--- Channel 0 ---"), "{out}");
        assert!(out.contains("1600.00 MHz"), "{out}");
        assert!(out.contains("channel mode"), "{out}");
    }

    /// (i) The Intel section renders the SMBIOS Type 17 capacity (total +
    /// per-DIMM): a 3 × 8 GiB kit → "24.0 GiB" total and "8.0 GiB × 3"
    /// per-DIMM (the x99 Broadwell-E reference shape).
    #[test]
    fn intel_section_renders_smbios_capacity() {
        let mut t = all_na();
        let mut ro = fixture_intel_readout(
            IntelGen::Skylake,
            Some(0x0000_000C),
            &fixture_channel_regs(),
            &ChannelRegs::default(),
            true,
        );
        ro.channel_mode = Some(ChannelMode::DualSymmetric);
        t.intel = Section::Value(ro);
        let smbios = SmbiosTopology {
            populated_count: 3,
            dimm_sizes: vec![8192, 8192, 8192],
            total_capacity_mib: 24576,
            memory_type: Some(MemoryTech::Ddr4),
            speed_mts: Some(2400),
            locators: vec!["B1".to_owned(), "D1".to_owned(), "H1".to_owned()],
            derived_mode: DerivedMode::Dual,
        };
        let out = render_dashboard(&t, &smbios);
        assert!(out.contains("total capacity"), "{out}");
        assert!(out.contains("24.0 GiB"), "total 24 GiB: {out}");
        assert!(out.contains("8.0 GiB × 3"), "per-DIMM 8 GiB × 3: {out}");
    }
}
