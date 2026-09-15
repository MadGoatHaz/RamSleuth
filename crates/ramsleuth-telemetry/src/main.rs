//! Verification CLI (P2-11) — the thin front end for [`collect`].
//!
//! Renders the full [`SystemMemoryTelemetry`] snapshot as either a
//! dashboard-style text listing or a hand-rolled JSON block (no serde —
//! P1-11 precedent). Every section renders as its value's `Debug` text or
//! a structured `N/A (<reason>)`, so the process **exits 0 even when
//! everything is N/A** (N/A is a valid, structured outcome, not a
//! failure). The only non-zero exit is a bad command-line flag (2). No
//! clap; `std::env::args` only.

use ramsleuth_telemetry::error::Section;
use ramsleuth_telemetry::{collect, SystemMemoryTelemetry};

/// Command-line options (parsed by hand — no clap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Opts {
    /// Emit the snapshot as JSON instead of the dashboard text.
    json: bool,
}

/// Usage text (printed for `--help` / `-h`).
const USAGE: &str = "\
ramsleuth-telemetry — RamSleuth system memory telemetry snapshot

Usage: ramsleuth-telemetry [OPTIONS]

Options:
  --json      Print the snapshot as a hand-rolled JSON block
  -h, --help  Print this help and exit

Exit codes:
  0  snapshot rendered (even if every section is N/A)
  2  unknown flag
";

/// Parse argv (index 0 is the program name). `--json` sets `json`;
/// `-h`/`--help` are recognized (not an error — `main` short-circuits
/// them before parsing); any other argument is an error naming that
/// argument.
pub(crate) fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts { json: false };
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--json" => opts.json = true,
            "-h" | "--help" => {} // recognized; help is handled in `main`
            bad => return Err(bad.to_owned()),
        }
    }
    Ok(opts)
}

/// Render one `Section` cell: the value's `Debug` text, or
/// `N/A (<reason>)`.
pub(crate) fn render_section<T: std::fmt::Debug>(s: &Section<T>) -> String {
    match s {
        Section::Value(v) => format!("{v:?}"),
        Section::Na(r) => format!("N/A ({r:?})"),
    }
}

/// Render the dashboard listing: one CPU line, one AMD line, one Intel
/// line, then one line per SPD module (or a single N/A line when the SPD
/// branch yielded nothing).
pub(crate) fn render_dashboard(t: &SystemMemoryTelemetry) -> String {
    let mut out = String::new();
    out.push_str(&format!("CPU: {:?} — {}\n", t.cpu.vendor, t.cpu.brand));
    out.push_str(&format!("AMD: {}\n", render_section(&t.amd)));
    out.push_str(&format!("Intel: {}\n", render_section(&t.intel)));
    if t.spd.is_empty() {
        out.push_str("SPD: N/A (no SPD data)\n");
    } else {
        for (i, module) in t.spd.iter().enumerate() {
            out.push_str(&format!("SPD[{i}]: {module:?}\n"));
        }
    }
    out
}

/// Render one section for JSON: `null` when `Na`, otherwise the
/// section's `Debug` text as a JSON string.
fn section_json<T: std::fmt::Debug>(s: &Section<T>) -> String {
    match s {
        Section::Na(_) => "null".to_owned(),
        Section::Value(v) => format!("\"{}\"", json_escape(&format!("{v:?}"))),
    }
}

/// Hand-rolled JSON for the whole snapshot (no serde — P1-11 precedent).
///
/// Shape: `{"cpu":{"vendor":…,"brand":…},"amd":…,"intel":…,"spd":[…]}`.
/// A section is `null` when `Na`, else its `Debug` text as a JSON string;
/// each SPD module is its `Debug` text as a JSON string.
pub(crate) fn to_json(t: &SystemMemoryTelemetry) -> String {
    let spd = t
        .spd
        .iter()
        .map(|m| format!("\"{}\"", json_escape(&format!("{m:?}"))))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"cpu\":{{\"vendor\":\"{}\",\"brand\":\"{}\"}},\"amd\":{},\"intel\":{},\"spd\":[{}]}}",
        json_escape(&format!("{:?}", t.cpu.vendor)),
        json_escape(&t.cpu.brand),
        section_json(&t.amd),
        section_json(&t.intel),
        spd,
    )
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

/// CLI entry: parse flags, collect the snapshot, render it, exit 0.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Help short-circuits before parsing: print usage, exit 0.
    if args.iter().skip(1).any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        std::process::exit(0);
    }
    match parse_opts(&args) {
        Err(bad) => {
            eprintln!("unknown flag: {bad}");
            std::process::exit(2);
        }
        Ok(opts) => {
            let t = collect();
            if opts.json {
                println!("{}", to_json(&t));
            } else {
                print!("{}", render_dashboard(&t));
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
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::NaReason;
    use ramsleuth_telemetry::spd_decode::SpdModule;
    use ramsleuth_telemetry::SystemPlatform;

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
            // The C6-06 fields: a fully-degraded platform, no total
            // (the synthetic host carries no meminfo source), and no
            // DIMM sizes (parallel to the empty SPD list).
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

    /// A small `SpdModule` fixture (host-independent fields).
    fn fixture_module(index: u8) -> SpdModule {
        SpdModule {
            index,
            is_ddr5: false,
            maker: Section::Value("0xC1".to_owned()),
            die_maker: Section::na(NaReason::NotApplicable),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::na(NaReason::ParseError("fixture".to_owned())),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(1),
            density_mbit: Section::na(NaReason::ParseError("fixture".to_owned())),
            speed_mts: Section::Value(3200),
            profiles: Vec::new(),
        }
    }

    /// (a) `render_section`: `Value(x)` shows x's `Debug` text; `Na(r)`
    /// is exactly `N/A (<r Debug>)`.
    #[test]
    fn render_section_value_and_na() {
        let v: Section<u32> = Section::Value(3200);
        assert_eq!(render_section(&v), format!("{:?}", 3200u32));

        let n: Section<u32> = Section::Na(NaReason::DriverMissing);
        assert_eq!(render_section(&n), format!("N/A ({:?})", NaReason::DriverMissing));
        assert_eq!(render_section(&n), "N/A (DriverMissing)");
    }

    /// (b) `render_dashboard` on an all-Na snapshot: both vendor sections
    /// render their structured `N/A` reasons, the CPU line carries the
    /// brand, and the empty SPD list renders its single N/A line.
    #[test]
    fn dashboard_all_na_renders_structured() {
        let out = render_dashboard(&all_na());
        assert!(out.contains("CPU: Amd(Zen3) — Ryzen 9 5950X"));
        assert!(out.contains("AMD: N/A (DriverMissing)"));
        assert!(out.contains("Intel: N/A (UnsupportedHardware)"));
        assert!(out.contains("SPD: N/A (no SPD data)"));
    }

    /// (b') `render_dashboard` with SPD modules: one `SPD[i]: <Debug>`
    /// line per module, no N/A line.
    #[test]
    fn dashboard_renders_spd_modules() {
        let mut t = all_na();
        t.spd = vec![fixture_module(0x52), fixture_module(0x53)];
        let out = render_dashboard(&t);
        assert!(out.contains("SPD[0]: SpdModule"));
        assert!(out.contains("SPD[1]: SpdModule"));
        assert!(!out.contains("no SPD data"));
    }

    /// (c) `to_json`: structurally valid — starts with `{`, carries all
    /// four top-level keys (plus vendor/brand), an `Na` section is
    /// `null`, and a module is its escaped `Debug` string.
    #[test]
    fn to_json_is_structurally_valid() {
        let mut t = all_na();
        t.spd = vec![fixture_module(0x52)];
        let out = to_json(&t);
        assert!(out.starts_with('{'));
        assert!(out.ends_with('}'));
        for key in ["\"cpu\"", "\"vendor\"", "\"brand\"", "\"amd\"", "\"intel\"", "\"spd\""] {
            assert!(out.contains(key), "missing key {key} in: {out}");
        }
        assert!(out.contains("\"amd\":null"));
        assert!(out.contains("\"intel\":null"));
        // The module renders as a JSON string carrying its Debug text.
        assert!(out.contains("\"spd\":[\"SpdModule"));
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

        let bogus_args = [prog.clone(), "--bogus".to_owned()];
        assert_eq!(parse_opts(&bogus_args), Err("--bogus".to_owned()));
    }
}
