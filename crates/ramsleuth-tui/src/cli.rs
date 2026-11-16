//! CLI argument parsing for the TUI binary (extracted from `main.rs`).
//!
//! Pure, testable, no I/O: the usage text, the related-binary suggestion,
//! the [`TuiArgs`] struct, and [`parse_args`] (the ramsleuth-daemon P3-17 /
//! ramsleuth-client P3-21 precedent).

use std::fmt::Write as _;
use std::path::PathBuf;

use ramsleuth_protocol::DEFAULT_SOCKET_PATH;

/// Usage text printed for `--help`/`-h` and on parse errors (exit 2) —
/// the ramsleuth-daemon P3-17 / ramsleuth-client P3-21 precedent. The
/// default socket path is the protocol's frozen `DEFAULT_SOCKET_PATH`
/// value (P3-10) as a literal: `concat!` only accepts literals, and
/// the protocol freeze test pins the string.
pub const USAGE: &str = concat!(
    "ramsleuth-tui — the RamSleuth live terminal dashboard (17-key\n",
    "parity with the GUI)\n",
    "\n",
    "Live RAM telemetry (AMD SMU / Intel IMC), SPD module details with\n",
    "XMP/EXPO profiles, channel mode + ECC status, and the memory\n",
    "benchmark. Talks to the privileged ramsleuth-daemon over the Unix\n",
    "socket (the daemon needs root / CAP_SYS_RAWIO; without it the\n",
    "privileged fields degrade to N/A).\n",
    "\n",
    "Usage: ramsleuth-tui [OPTIONS]\n",
    "\n",
    "Keys:\n",
    "  [R]efresh        force one poll (works with refresh off)\n",
    "  [S]napshot       write a .txt dashboard snapshot to the CWD\n",
    "  [Q]uit           exit (exit code 0)\n",
    "  [B]ench (full)   start a full benchmark run\n",
    "  [M]emory         start a memory-only benchmark run\n",
    "  [X] burn-in      start a 5-minute burn-in soak\n",
    "  [C]ancel         cancel the in-flight run\n",
    "  [E]xport         write { telemetry, bench } JSON to $HOME\n",
    "  [G]raphs         toggle the graphs overlay panel\n",
    "  [T]settings      toggle the settings strip\n",
    "  [D]info          toggle the requirements + controls screen\n",
    "  [P]oll           cycle the poll interval (100 ms → 60 s, wrap)\n",
    "  [U]nits          toggle the capacity units (GiB ↔ GB)\n",
    "  [K]lock          toggle the clock units (MHz ↔ GHz)\n",
    "  [A]uto refresh   toggle the periodic data poll (on ↔ off)\n",
    "  [W]indow         cycle the graphs window (1 → 5 → 15 → 60 min)\n",
    "  [F] probe report  consent → preview → write/copy the report\n",
    "\n",
    "Options:\n",
    "  --socket <path>   Daemon Unix socket\n",
    "                    (default: /run/ramsleuth/ramsleuth.sock)\n",
    "  -h, --help        Print this help and exit\n",
    "  -V, --version     Print the version and exit\n",
    "\n",
    "Exit codes: 0 quit, 1 terminal init failure, 2 usage error\n",
    "\n",
);

/// Suggest a related RamSleuth binary when the user types a flag or
/// positional that looks like they meant to invoke a different tool.
/// (The shared `suggest_for_unknown` in `ramsleuth_protocol::cli_common`
/// is a Levenshtein-based subcommand suggester; this binary-name alias
/// lookup stays local because its result text is tool-specific.)
fn suggest_related_binary(arg: &str) -> Option<&'static str> {
    let normalized = arg.trim_start_matches('-').to_ascii_lowercase();
    match normalized.as_str() {
        "tui" => Some("Did you mean `ramsleuth-tui`? (run `ramsleuth-tui --help`)"),
        "client" => Some("Did you mean `ramsleuth-client`? (run `ramsleuth-client --help`)"),
        "bench" => Some("Did you mean `ramsleuth-bench`? (run `ramsleuth-bench --help`)"),
        "telemetry" => {
            Some("Did you mean `ramsleuth-telemetry`? (run `ramsleuth-telemetry --help`)")
        }
        "daemon" => Some("Did you mean `ramsleuth-daemon`? (run `ramsleuth-daemon --help`)"),
        "gui" => Some("Did you mean `ramsleuth` (the GUI)? (run `ramsleuth --help`)"),
        _ => None,
    }
}

/// The TUI's parsed command line (P3-24).
///
/// Pure data — produced by [`parse_args`] (no env access, no I/O,
/// unit-tested) and consumed once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct TuiArgs {
    /// The daemon Unix socket to connect to (`--socket`); defaults to
    /// [`DEFAULT_SOCKET_PATH`] (the single socket-path source, P3-10).
    pub socket: PathBuf,
}

impl Default for TuiArgs {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_SOCKET_PATH),
        }
    }
}

/// Parse the TUI command line (the program name already removed).
///
/// Pure and testable: no env access, no I/O, no panic. The one flag
/// `--socket <path>` sets the daemon Unix socket (default
/// [`DEFAULT_SOCKET_PATH`]); it may repeat (the last value wins). Any
/// other flag, any positional argument, or a missing flag value is a
/// `String` error naming the problem (exit `2` at startup) — the
/// ramsleuth-client P3-21 precedent. (`--help`/`-h` and
/// `--version`/`-V` never reach this parser — `main` short-circuits
/// them first, exit `0`.)
pub fn parse_args<I, S>(args: I) -> Result<TuiArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = TuiArgs::default();
    let mut iter = args.into_iter();
    while let Some(raw) = iter.next() {
        let arg = raw.as_ref();
        if arg == "--socket" {
            let value = iter.next().map(|v| v.as_ref().to_owned()).ok_or_else(|| {
                "--socket requires a value (the daemon's Unix socket path)".to_owned()
            })?;
            parsed.socket = PathBuf::from(value);
        } else if arg.starts_with("--") {
            let mut msg = format!("unknown flag `{arg}` (expected `--socket <path>`)");
            if let Some(sug) = suggest_related_binary(arg) {
                let _ = write!(msg, "\n  {sug}");
            }
            return Err(msg);
        } else {
            let mut msg =
                format!("unexpected argument `{arg}` (no positional arguments are accepted)");
            if let Some(sug) = suggest_related_binary(arg) {
                let _ = write!(msg, "\n  {sug}");
            }
            return Err(msg);
        }
    }
    Ok(parsed)
}
