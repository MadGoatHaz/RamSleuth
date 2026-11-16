//! Shared CLI boilerplate for all RamSleuth binaries.
//!
//! Extracted from the 6 byte-identical copies in each `main.rs`
//! (v2.4.33+ enhancement, CHUNK-C). All binaries import from this
//! module instead of defining their own copies of the version line,
//! the `--help`/`--version` short-circuit scanner, and the shared
//! "Related tools" block.

/// The shared "Related tools" block appended to every binary's help
/// text. Each binary keeps its own specific usage / options / subcommand
/// documentation local; only this cross-tool index is shared.
pub const USAGE_COMMON: &str = "\
Related tools:
  ramsleuth              Desktop GUI (egui)
  ramsleuth-tui          Terminal dashboard (ratatui)
  ramsleuth-client       CLI client (probe, dump, status, bench, burn)
  ramsleuth-bench        Standalone memory & latency benchmark
  ramsleuth-telemetry    Standalone telemetry snapshot (no daemon)
  ramsleuth-daemon       Privileged root daemon (CAP_SYS_RAWIO)

Run '<tool> --help' for per-tool usage.
";

/// Format the version line: `ramSleuth <bin_name> v<version>`.
///
/// The version is the workspace package version (injected at build time
/// via `CARGO_PKG_VERSION` — since this module lives in the
/// `ramsleuth-protocol` crate, it tracks the workspace release
/// automatically, exactly as the per-binary `env!` did before).
///
/// `bin_name` is the binary's full name (e.g. `"ramsleuth-daemon"`,
/// `"ramsleuth"` for the GUI) — the shared function no longer knows
/// which binary it is in, so the caller passes it explicitly.
pub fn version_line(bin_name: &str) -> String {
    format!("ramSleuth {bin_name} v{}", env!("CARGO_PKG_VERSION"))
}

/// Short-circuit enum for `--help` / `--version` handling.
///
/// Checked by each binary's `main` *before* the full argument parse:
/// `Help` prints the usage text (exit 0), `Version` prints the
/// [`version_line`] (exit 0), `None` means the full parse proceeds
/// (a genuinely unknown flag stays a usage error, exit 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortCircuit {
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
pub fn short_circuit<I, S>(args: I) -> ShortCircuit
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

/// Suggest a known name for an unknown one (Levenshtein distance ≤ 2).
///
/// `known` is the list of valid names for this binary (e.g. its
/// subcommands). Returns the closest match, or `None` if no suggestion
/// is plausible.
///
/// Example: `suggest_for_unknown("dum", &["dump", "bench", …])` →
/// `Some("dump")`.
pub fn suggest_for_unknown<'a>(unknown: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;
    for &k in known {
        let dist = levenshtein(unknown, k);
        if dist <= 2 {
            match best {
                Some((_, best_dist)) if dist < best_dist => best = Some((k, dist)),
                None => best = Some((k, dist)),
                _ => {}
            }
        }
    }
    best.map(|(s, _)| s)
}

/// Minimal Levenshtein edit distance (no external dependency).
///
/// The standard dynamic-programming algorithm with a two-row rolling
/// buffer: `O(m·n)` time, `O(n)` space.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) The `--version`/`-V` line carries the binary name and the
    /// package version exactly (`env!` — it tracks the workspace
    /// release).
    #[test]
    fn version_line_is_the_package_version() {
        let line = version_line("ramsleuth-daemon");
        assert!(
            line.starts_with("ramSleuth ramsleuth-daemon v"),
            "must carry the binary name: {line}"
        );
        assert!(
            line.contains(env!("CARGO_PKG_VERSION")),
            "must carry the package version: {line}"
        );
    }

    /// (b) `--help`/`-h` and `--version`/`-V` are the short-circuits:
    /// the first occurrence, in argv order, wins; anything else
    /// (including the plain flags list) is `None`.
    #[test]
    fn short_circuit_classifies_help_version_and_none() {
        for h in ["--help", "-h"] {
            assert_eq!(short_circuit([h]), ShortCircuit::Help, "{h} → Help");
        }
        for v in ["--version", "-V"] {
            assert_eq!(short_circuit([v]), ShortCircuit::Version, "{v} → Version");
        }
        assert_eq!(short_circuit(Vec::<&str>::new()), ShortCircuit::None);
        assert_eq!(
            short_circuit(["--socket", "/tmp/x", "--max-age", "4"]),
            ShortCircuit::None,
            "a plain flags list is None"
        );
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

    /// (c) `suggest_for_unknown` finds the closest known name when the
    /// edit distance is ≤ 2, and returns `None` otherwise.
    #[test]
    fn suggest_for_unknown_finds_close_match() {
        let known = ["dump", "bench", "status", "probe", "burn"];
        assert_eq!(suggest_for_unknown("dum", &known), Some("dump"));
        assert_eq!(suggest_for_unknown("benchh", &known), Some("bench"));
        assert_eq!(suggest_for_unknown("xyz", &known), None);
        // An exact match is distance 0.
        assert_eq!(suggest_for_unknown("dump", &known), Some("dump"));
    }
}
