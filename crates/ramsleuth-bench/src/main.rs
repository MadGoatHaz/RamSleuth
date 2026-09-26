//! Standalone verification CLI for the ramsleuth-bench engine.
//!
//! **Chunk P1-11 — [COUPLED-TO: P1-10].** Thin harness around the frozen
//! orchestrator: detects host features and topology, runs [`run_all`],
//! and prints the AIDA64-style 4×4 grid as a fixed-width text table
//! plus (with `--json`) a serde-free JSON object. No benchmark logic
//! lives here — all measurement stays in the library.
//!
//! ```text
//! cargo run -p ramsleuth-bench [--avx512] [--json] [--help] [--version]
//! ```

use ramsleuth_bench::{detect, run_all, BenchmarkGrid, CpuFeatures};

/// Parsed CLI options (std-only parsing — no arg-parsing dependency).
#[derive(Debug)]
pub(crate) struct Opts {
    /// Force the AVX-512 kernel path (the `run_all` argument).
    pub(crate) avx512: bool,
    /// Also print the grid as a JSON object.
    pub(crate) json: bool,
}

/// Usage text printed for `--help`/`--version` and parse errors.
const USAGE: &str = concat!(
    "ramsleuth-bench — the standalone AIDA64-style memory bandwidth &\n",
    "latency grid (Memory/L3/L2/L1 × read/write/copy/latency)\n",
    "\n",
    "Runs the benchmark directly — no daemon needed.\n",
    "\n",
    "Usage: ramsleuth-bench [OPTIONS]\n",
    "\n",
    "Options:\n",
    "  --avx512     Force the AVX-512 kernel path (falls back to AVX2 when\n",
    "               AVX-512F is absent)\n",
    "  --json       Also print the grid as a JSON object\n",
    "  -h, --help   Print this help and exit\n",
    "  -V, --version Print the version and exit\n",
);

/// The `--version`/`-V` output line: `ramSleuth <bin> v<version>` —
/// the version is the crate's `CARGO_PKG_VERSION` (the workspace
/// release, so it tracks it automatically).
pub(crate) fn version_line() -> String {
    format!("ramSleuth ramsleuth-bench v{}", env!("CARGO_PKG_VERSION"))
}

/// The startup short-circuit for a raw argument (checked by `main`
/// *before* the full [`parse_opts`]): `Help` prints the usage text
/// (exit 0), `Version` prints [`version_line`] (exit 0), `None` means
/// the full parse proceeds (a genuinely unknown flag stays a usage
/// error, exit 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortCircuit {
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
pub(crate) fn short_circuit<I, S>(args: I) -> ShortCircuit
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

/// Parse CLI flags from `args` (program name already removed).
///
/// `--help`/`-h` / `--version`/`-V` are recognized here (the entrypoint
/// short-circuits them before running); unknown flags are rejected
/// with an error naming the offending flag.
pub(crate) fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts { avx512: false, json: false };
    for arg in args {
        match arg.as_str() {
            "--avx512" => opts.avx512 = true,
            "--json" => opts.json = true,
            // recognized; the entrypoint short-circuits them (help →
            // usage, version → the version line)
            "--help" | "-h" | "--version" | "-V" => {}
            other => return Err(format!("unknown flag: {other} (see --help)")),
        }
    }
    Ok(opts)
}

/// Format one f64 as a JSON number: shortest round-trip decimal, or
/// `null` for non-finite values (never valid JSON).
fn json_num(value: &f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "null".to_string()
    }
}

/// Format one grid column as a JSON array (Tier order: Memory, L1, L2, L3).
fn json_array(values: &[f64; 4]) -> String {
    let items = values.iter().map(json_num).collect::<Vec<_>>().join(", ");
    format!("[{items}]")
}

/// Serialize the grid as `{"read_gbps":[...],"write_gbps":[...],
/// "copy_gbps":[...],"latency_ns":[...]}` — hand-rolled, no serde.
pub(crate) fn to_json(grid: &BenchmarkGrid) -> String {
    format!(
        "{{\"read_gbps\":{},\"write_gbps\":{},\"copy_gbps\":{},\"latency_ns\":{}}}",
        json_array(&grid.read_gbps),
        json_array(&grid.write_gbps),
        json_array(&grid.copy_gbps),
        json_array(&grid.latency_ns),
    )
}

/// (Row label, grid slot) pairs in display order — top to bottom:
/// `Memory (DRAM)`, `L3`, `L2`, `L1` (slots are the `Tier` discriminants).
const ROWS: [(&str, usize); 4] = [("Memory (DRAM)", 0), ("L3", 3), ("L2", 2), ("L1", 1)];

/// The grid as a fixed-width AIDA64-style text table: bandwidth cells
/// with 2 decimals (GB/s), latency cells with 1 decimal (ns).
pub(crate) fn render_grid(grid: &BenchmarkGrid) -> String {
    const COL: usize = 12;
    let mut out = String::new();
    out.push_str(&format!(
        "{:<15} {:>COL$} {:>COL$} {:>COL$} {:>COL$}\n",
        "Tier", "Read (GB/s)", "Write (GB/s)", "Copy (GB/s)", "Latency (ns)"
    ));
    for (label, slot) in ROWS {
        out.push_str(&format!(
            "{label:<15} {:>COL$.2} {:>COL$.2} {:>COL$.2} {:>COL$.1}\n",
            grid.read_gbps[slot],
            grid.write_gbps[slot],
            grid.copy_gbps[slot],
            grid.latency_ns[slot],
        ));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match short_circuit(&args) {
        ShortCircuit::Help => {
            println!("{USAGE}");
            return;
        }
        ShortCircuit::Version => {
            println!("{}", version_line());
            return;
        }
        ShortCircuit::None => {}
    }
    let opts = match parse_opts(&args) {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("ramsleuth-bench: {message}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    let features = CpuFeatures::detect();
    let topo = match detect() {
        Ok(topo) => topo,
        Err(error) => {
            eprintln!("ramsleuth-bench: topology detection failed: {error}");
            std::process::exit(1);
        }
    };

    let l3_mib = topo.total_l3_bytes / (1024 * 1024);
    println!(
        "ramsleuth-bench: {} physical cores ({} logical, SMT {}), total L3 {} MiB, AVX2 {}, AVX-512 {}",
        topo.physical_cores.len(),
        topo.logical_cpus.len(),
        if topo.has_smt { "on" } else { "off" },
        l3_mib,
        if features.avx2 { "yes" } else { "no" },
        if features.avx512f { "yes" } else { "no" },
    );
    if opts.avx512 && !features.avx512f {
        println!("note: --avx512 requested but AVX-512F is absent; the AVX-512 kernels fall back to AVX2.");
    }

    let grid = match run_all(opts.avx512) {
        Ok(grid) => grid,
        Err(error) => {
            eprintln!("ramsleuth-bench: {error}");
            std::process::exit(1);
        }
    };

    print!("{}", render_grid(&grid));
    if opts.json {
        println!("{}", to_json(&grid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built grid (Tier order: Memory, L1, L2, L3) — no benchmark
    /// runs, no large allocations.
    fn hand_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [1.0, 2.5, 3.0, 4.0],
            write_gbps: [5.0, 6.0, 7.0, 8.0],
            copy_gbps: [9.0, 10.0, 11.0, 12.0],
            latency_ns: [13.0, 14.0, 15.0, 16.0],
        }
    }

    /// (a) `to_json` emits the exact expected object: key names plus
    /// values in Tier order (Memory, L1, L2, L3).
    #[test]
    fn to_json_emits_the_expected_object() {
        let json = to_json(&hand_grid());
        assert_eq!(
            json,
            "{\"read_gbps\":[1, 2.5, 3, 4],\"write_gbps\":[5, 6, 7, 8],\
             \"copy_gbps\":[9, 10, 11, 12],\"latency_ns\":[13, 14, 15, 16]}"
        );
        for key in ["read_gbps", "write_gbps", "copy_gbps", "latency_ns"] {
            assert!(json.contains(&format!("\"{key}\":")), "missing key {key} in {json}");
        }
    }

    /// (b) `render_grid` contains all four row labels and all four
    /// column headers, with 2-decimal bandwidth and 1-decimal latency
    /// cells.
    #[test]
    fn render_grid_contains_all_rows_and_headers() {
        let table = render_grid(&hand_grid());
        for label in ["Memory (DRAM)", "L3", "L2", "L1"] {
            assert!(table.contains(label), "row label {label} missing:\n{table}");
        }
        for header in ["Read (GB/s)", "Write (GB/s)", "Copy (GB/s)", "Latency (ns)"] {
            assert!(table.contains(header), "header {header} missing:\n{table}");
        }
        assert!(table.contains("2.50"), "L1 read cell should be 2.50:\n{table}");
        assert!(table.contains("13.0"), "Memory latency cell should be 13.0:\n{table}");
    }

    /// (c) `parse_opts` accepts `--avx512`/`--json`, recognizes
    /// `--help`/`-h` (without setting a flag), and rejects unknown
    /// flags with an error naming the flag.
    #[test]
    fn parse_opts_handles_known_and_unknown_flags() {
        let none = parse_opts(&[]).expect("no args is valid");
        assert!(!none.avx512 && !none.json);

        let both =
            parse_opts(&["--avx512".into(), "--json".into()]).expect("known flags parse");
        assert!(both.avx512 && both.json);

        for help in ["--help", "-h", "--version", "-V"] {
            let opts = parse_opts(&[help.into()]).expect("--help/-h/--version/-V is recognized");
            assert!(!opts.avx512 && !opts.json);
        }

        let err = parse_opts(&["--nope".into()]).expect_err("unknown flag must fail");
        assert!(err.contains("--nope"), "error should name the flag: {err}");
    }

    /// (d) The `--version`/`-V` line is the package version, exactly
    /// (`env!` — it tracks the workspace release).
    #[test]
    fn version_line_is_the_package_version() {
        assert_eq!(
            version_line(),
            format!("ramSleuth ramsleuth-bench v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// (e) `--help`/`-h` and `--version`/`-V` are the short-circuits:
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
        assert_eq!(short_circuit(["--avx512", "--json"]), ShortCircuit::None);
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
}
