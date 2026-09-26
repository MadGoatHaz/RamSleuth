//! ramsleuth-client — the unprivileged ramsleuth CLI (Phase 3, P3-21).
//!
//! The binary entry point: CLI parsing ([`parse_cli`] — pure, no env
//! access, unit-tested), a [`Client`] connect to the daemon's Unix
//! socket (P3-18 — a missing / refused socket is retried, then becomes
//! the friendly `DaemonDown` "start it with …" hint, never a panic,
//! plan D5), a per-subcommand read timeout, and dispatch to the three
//! commands the library exposes (P3-19 / P3-20):
//!
//! - `dump`   → the dashboard-style telemetry listing — the Phase 3
//!   exit-criterion command: full hardware timings against a running
//!   daemon;
//! - `bench`  → a streamed benchmark run (a progress line per completed
//!   cell, then the terminal AIDA64-style 4×4 grid);
//! - `status` → the one-line-per-section health summary.
//!
//! **Exit codes:** `0` success (`--help`/`-h` and `--version`/`-V`
//! short-circuit to `0` before parsing too); `1` a daemon / client
//! error (connect, timeout, protocol, or i/o — each with its
//! structured [`ClientError`] diagnostic); `2` a usage error (unknown
//! subcommand or flag, a missing flag value, or a bad enum value —
//! with the usage text).
//!
//! Manual verification (the CORE GATE command): with a daemon running,
//! `cargo run -p ramsleuth-client -- dump` prints the full hardware
//! timings against the default socket; against the unprivileged dev
//! daemon (`cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock`),
//! run `cargo run -p ramsleuth-client -- --socket /tmp/ramsleuth.sock dump`.
//!
//! ```text
//! Usage: ramsleuth-client [SUBCOMMAND] [OPTIONS]
//!
//! Subcommands:
//!   dump     Print the dashboard-style telemetry listing (default):
//!            CPU, the AMD / Intel sections, and the SPD modules with
//!            XMP/EXPO profiles
//!   bench    Run a streamed AIDA64-style memory bandwidth / latency
//!            benchmark (progress lines + the 4x4 grid)
//!   status   Print the per-section health summary
//!
//! Options:
//!   --socket <path>                  Daemon Unix socket
//!                                    (default: /run/ramsleuth/ramsleuth.sock)
//!   --tier <memory|l1|l2|l3|full>    Benchmark tier scope (default: full)
//!   --mode <full|memory-only>        Benchmark scope (default: full)
//!   -h, --help                       Print this help and exit
//!   -V, --version                    Print the version and exit
//!
//! Exit codes: 0 success, 1 daemon/client error, 2 usage error
//! ```

use std::path::PathBuf;
use std::time::Duration;

use ramsleuth_bench::{StreamTarget, Tier};
use ramsleuth_client::{bench, dump, status, Client, ClientError};
use ramsleuth_protocol::{BenchMode, DEFAULT_SOCKET_PATH};

/// Read timeout for the one-round-trip subcommands (`dump` /
/// `status`): 10 s — more generous than the transport's 5 s default
/// (a slow daemon still gets to answer), yet bounded (a wedged daemon
/// becomes a `Timeout`, never a hang).
const FAST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read timeout for the streamed `bench` subcommand: 120 s *between
/// frames* — a full run streams its progress lines over minutes, so a
/// legitimate gap between frames can far exceed the 5 s transport
/// default; the deadline only bounds a silently wedged daemon.
const BENCH_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// The client's parsed command line (P3-21).
///
/// Pure data — produced by [`parse_cli`] (no env access, no I/O,
/// unit-tested) and consumed once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct CliArgs {
    /// The daemon Unix socket to connect to (`--socket`); defaults to
    /// [`DEFAULT_SOCKET_PATH`] (the single socket-path source, P3-10).
    pub socket: PathBuf,
    /// The subcommand to run (the first positional); defaults to
    /// [`Subcommand::Dump`].
    pub subcommand: Subcommand,
    /// The benchmark tier scope for `bench` (`--tier`); defaults to
    /// [`StreamTarget::Full`] (the complete 4×4 grid).
    pub target: StreamTarget,
    /// The benchmark scope for `bench` (`--mode`); defaults to
    /// [`BenchMode::Full`] (all 12 bandwidth cells plus the four
    /// latency passes).
    pub mode: BenchMode,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_SOCKET_PATH),
            subcommand: Subcommand::Dump,
            target: StreamTarget::Full,
            mode: BenchMode::Full,
        }
    }
}

/// The client's subcommands (the first positional argument; defaults to
/// [`Subcommand::Dump`] when no positional is given).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Subcommand {
    /// Print the dashboard-style telemetry listing (the Phase 3
    /// exit-criterion command).
    Dump,
    /// Run a streamed benchmark with the selected `target` + `mode`.
    Bench,
    /// Print the per-section health summary.
    Status,
}

/// Usage text printed for `--help`/`-h` and on parse errors (exit 2 —
/// the ramsleuth-daemon P3-17 precedent). The default socket path is
/// the protocol's frozen `DEFAULT_SOCKET_PATH` value (P3-10) as a
/// literal: `concat!` only accepts literals, and the protocol freeze
/// test pins the string.
const USAGE: &str = concat!(
    "ramsleuth-client — the RamSleuth unprivileged CLI (dump / bench /\n",
    "status)\n",
    "\n",
    "Talks to the privileged ramsleuth-daemon over the Unix socket (the\n",
    "daemon needs root / CAP_SYS_RAWIO).\n",
    "\n",
    "Usage: ramsleuth-client [SUBCOMMAND] [OPTIONS]\n",
    "\n",
    "Subcommands:\n",
    "  dump     Print the dashboard-style telemetry listing (default):\n",
    "           CPU, the AMD / Intel sections, and the SPD modules with\n",
    "           XMP/EXPO profiles\n",
    "  bench    Run a streamed AIDA64-style memory bandwidth / latency\n",
    "           benchmark (progress lines + the 4x4 grid)\n",
    "  status   Print the per-section health summary\n",
    "\n",
    "Options:\n",
    "  --socket <path>                  Daemon Unix socket\n",
    "                                   (default: /run/ramsleuth/ramsleuth.sock)\n",
    "  --tier <memory|l1|l2|l3|full>    Benchmark tier scope (default: full)\n",
    "  --mode <full|memory-only>        Benchmark scope (default: full)\n",
    "  -h, --help                       Print this help and exit\n",
    "  -V, --version                    Print the version and exit\n",
    "\n",
    "Exit codes: 0 success, 1 daemon/client error, 2 usage error\n",
);

/// The `--version`/`-V` output line: `ramSleuth <bin> v<version>` —
/// the version is the crate's `CARGO_PKG_VERSION` (the workspace
/// release, so it tracks it automatically).
fn version_line() -> String {
    format!("ramSleuth ramsleuth-client v{}", env!("CARGO_PKG_VERSION"))
}

/// The startup short-circuit for a raw argument (checked by `main`
/// *before* the full [`parse_cli`]): `Help` prints the usage text
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

/// One subcommand name (the first positional): `dump` / `bench` /
/// `status`; anything else is an error naming the offending value.
fn parse_subcommand(arg: &str) -> Result<Subcommand, String> {
    match arg {
        "dump" => Ok(Subcommand::Dump),
        "bench" => Ok(Subcommand::Bench),
        "status" => Ok(Subcommand::Status),
        other => Err(format!(
            "unknown subcommand `{other}` (expected `dump`, `bench`, or `status`)"
        )),
    }
}

/// One `--tier` value: `memory` / `l1` / `l2` / `l3` map onto
/// [`StreamTarget::Tier`], `full` onto [`StreamTarget::Full`]; anything
/// else is an error naming the offending value.
fn parse_target(arg: &str) -> Result<StreamTarget, String> {
    match arg {
        "full" => Ok(StreamTarget::Full),
        "memory" => Ok(StreamTarget::Tier(Tier::Memory)),
        "l1" => Ok(StreamTarget::Tier(Tier::L1)),
        "l2" => Ok(StreamTarget::Tier(Tier::L2)),
        "l3" => Ok(StreamTarget::Tier(Tier::L3)),
        other => Err(format!(
            "invalid --tier value `{other}` (expected `memory`, `l1`, `l2`, `l3`, or `full`)"
        )),
    }
}

/// One `--mode` value: `full` / `memory-only`; anything else is an
/// error naming the offending value.
fn parse_mode(arg: &str) -> Result<BenchMode, String> {
    match arg {
        "full" => Ok(BenchMode::Full),
        "memory-only" => Ok(BenchMode::MemoryOnly),
        other => Err(format!(
            "invalid --mode value `{other}` (expected `full` or `memory-only`)"
        )),
    }
}

/// Parse the client's command line (the program name already removed).
///
/// Pure and testable: no env access, no I/O, no panic. The first
/// positional is the subcommand (`dump` | `bench` | `status`; defaults
/// to `dump` when no positional is given). The three flags all take a
/// value, may repeat (the last value wins), and may appear in any
/// order relative to the subcommand:
///
/// - `--socket <path>` — the daemon Unix socket (default
///   [`DEFAULT_SOCKET_PATH`]);
/// - `--tier <memory|l1|l2|l3|full>` — the benchmark tier scope
///   (default `Full`);
/// - `--mode <full|memory-only>` — the benchmark scope (default `Full`).
///
/// An unknown flag, an unknown subcommand, a second positional, or a
/// flag missing its value is a `String` error naming the problem (exit
/// `2` at startup); a valid parse yields [`CliArgs`] with defaults for
/// every absent piece. (`--help`/`-h` and `--version`/`-V` never reach
/// this parser — `main` short-circuits them first, exit `0`.)
pub fn parse_cli<I, S>(args: I) -> Result<CliArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = CliArgs::default();
    let mut subcommand_seen = false;
    let mut iter = args.into_iter();
    while let Some(raw) = iter.next() {
        let arg = raw.as_ref();
        if arg.starts_with("--") {
            match arg {
                "--socket" => {
                    let value = iter
                        .next()
                        .map(|v| v.as_ref().to_owned())
                        .ok_or_else(|| {
                            "--socket requires a value (the daemon's Unix socket path)".to_owned()
                        })?;
                    parsed.socket = PathBuf::from(value);
                }
                "--tier" => {
                    let value = iter
                        .next()
                        .map(|v| v.as_ref().to_owned())
                        .ok_or_else(|| {
                            "--tier requires a value (memory | l1 | l2 | l3 | full)".to_owned()
                        })?;
                    parsed.target = parse_target(&value)?;
                }
                "--mode" => {
                    let value = iter
                        .next()
                        .map(|v| v.as_ref().to_owned())
                        .ok_or_else(|| "--mode requires a value (full | memory-only)".to_owned())?;
                    parsed.mode = parse_mode(&value)?;
                }
                other => {
                    return Err(format!(
                        "unknown flag `{other}` (expected `--socket <path>`, \
                         `--tier <memory|l1|l2|l3|full>`, or `--mode <full|memory-only>`)"
                    ));
                }
            }
        } else {
            if subcommand_seen {
                return Err(format!(
                    "unexpected argument `{arg}` (only one subcommand is accepted: \
                     `dump`, `bench`, or `status`)"
                ));
            }
            parsed.subcommand = parse_subcommand(arg)?;
            subcommand_seen = true;
        }
    }
    Ok(parsed)
}

fn main() {
    // `--help`/`-h` and `--version`/`-V` short-circuit before parsing
    // (standard, exit 0): help prints the usage text, version the
    // [`version_line`].
    match short_circuit(std::env::args().skip(1)) {
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

    // A parse error is a usage error: the message + the usage text,
    // exit 2 (the ramsleuth-daemon P3-17 precedent).
    let args = match parse_cli(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("ramsleuth-client: {message}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    // Connect to the daemon (P3-18): a missing / refused socket is
    // retried, then becomes the friendly `DaemonDown` hint (with the
    // "start it with …" diagnostic) — never a panic (plan D5). A
    // connect failure is a daemon error: exit 1.
    let mut client = match Client::connect(&args.socket) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("ramsleuth-client: {error}");
            std::process::exit(1);
        }
    };

    // The per-subcommand read timeout (the transport default is 5 s):
    // the one-round-trip commands get 10 s, the streamed benchmark gets
    // 120 s between frames (a full run streams over minutes).
    let read_timeout = match args.subcommand {
        Subcommand::Bench => BENCH_READ_TIMEOUT,
        Subcommand::Dump | Subcommand::Status => FAST_READ_TIMEOUT,
    };
    if let Err(error) = client.set_read_timeout(read_timeout) {
        eprintln!("ramsleuth-client: {error}");
        std::process::exit(1);
    }

    // Dispatch: `dump` prints the dashboard (P3-19); `bench` streams
    // the progress + the terminal grid (P3-20); `status` prints the
    // per-section summary (P3-20). `bench` / `status` also *return*
    // the text they printed (the P3-20 contract) — the bin ignores it.
    // Success falls through to exit 0; any client/daemon error exits 1.
    let result: Result<(), ClientError> = match args.subcommand {
        Subcommand::Dump => dump(&mut client),
        Subcommand::Bench => bench(&mut client, args.target, args.mode).map(drop),
        Subcommand::Status => status(&mut client).map(drop),
    };
    if let Err(error) = result {
        eprintln!("ramsleuth-client: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    //! `parse_cli` is pure (no env access, no I/O), so the unit tests
    //! drive it directly with argument iterators — `&str` slices (the
    //! common case) plus one owned-`String` case (the `env::args`
    //! shape; the generic bound is exercised with owned values, the
    //! ramsleuth-daemon P3-17 test precedent).

    use super::*;

    /// (a) No args → the defaults: subcommand `Dump`, the protocol's
    /// default socket path, target `Full`, mode `Full`.
    #[test]
    fn no_args_yields_defaults() {
        let args = parse_cli(Vec::<&str>::new()).expect("no args must parse");
        assert_eq!(args, CliArgs::default());
        assert_eq!(args.subcommand, Subcommand::Dump);
        assert_eq!(args.socket, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert_eq!(args.target, StreamTarget::Full);
        assert_eq!(args.mode, BenchMode::Full);
    }

    /// (b) `dump --socket /tmp/x.sock` → `Dump` with the overridden
    /// socket path (flags may follow the subcommand).
    #[test]
    fn dump_with_socket_override() {
        let args = parse_cli(["dump", "--socket", "/tmp/x.sock"]).expect("must parse");
        assert_eq!(args.subcommand, Subcommand::Dump);
        assert_eq!(args.socket, PathBuf::from("/tmp/x.sock"));
        assert_eq!(args.target, StreamTarget::Full);
        assert_eq!(args.mode, BenchMode::Full);
    }

    /// (c) `bench --tier memory --mode memory-only` → `Bench`,
    /// `Tier(Memory)`, and `MemoryOnly`.
    #[test]
    fn bench_tier_memory_mode_memory_only() {
        let args =
            parse_cli(["bench", "--tier", "memory", "--mode", "memory-only"])
                .expect("must parse");
        assert_eq!(args.subcommand, Subcommand::Bench);
        assert_eq!(args.target, StreamTarget::Tier(Tier::Memory));
        assert_eq!(args.mode, BenchMode::MemoryOnly);
    }

    /// (d) `status` → `Status`.
    #[test]
    fn status_subcommand() {
        let args = parse_cli(["status"]).expect("must parse");
        assert_eq!(args.subcommand, Subcommand::Status);
        assert_eq!(args.target, StreamTarget::Full);
        assert_eq!(args.mode, BenchMode::Full);
    }

    /// (e) `bench --tier full` → `StreamTarget::Full` (the explicit
    /// full-grid spelling of the default).
    #[test]
    fn bench_tier_full() {
        let args = parse_cli(["bench", "--tier", "full"]).expect("must parse");
        assert_eq!(args.subcommand, Subcommand::Bench);
        assert_eq!(args.target, StreamTarget::Full);
    }

    /// (f) An unknown subcommand is rejected with its name.
    #[test]
    fn unknown_subcommand_is_rejected() {
        let err = parse_cli(["frobnicate"])
            .expect_err("an unknown subcommand must be rejected");
        assert!(
            err.contains("frobnicate"),
            "the error must name the subcommand: {err}"
        );
    }

    /// (g) `--tier` with no value is rejected with a message naming the
    /// flag.
    #[test]
    fn missing_tier_value_is_rejected() {
        let err = parse_cli(["bench", "--tier"])
            .expect_err("`--tier` without a value must be rejected");
        assert!(err.contains("--tier"), "the error must name the flag: {err}");
    }

    /// (h) `--tier bogus` is rejected with a message naming the
    /// offending value.
    #[test]
    fn bad_tier_value_is_rejected() {
        let err = parse_cli(["--tier", "bogus"])
            .expect_err("a bad `--tier` value must be rejected");
        assert!(
            err.contains("bogus") && err.contains("--tier"),
            "the error must name the flag and the value: {err}"
        );
    }

    /// (i) `--mode bogus` is rejected with a message naming the
    /// offending value.
    #[test]
    fn bad_mode_value_is_rejected() {
        let err = parse_cli(["--mode", "bogus"])
            .expect_err("a bad `--mode` value must be rejected");
        assert!(
            err.contains("bogus") && err.contains("--mode"),
            "the error must name the flag and the value: {err}"
        );
    }

    /// (j) An unknown flag is rejected with its name.
    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse_cli(["--bogus"]).expect_err("an unknown flag must be rejected");
        assert!(
            err.contains("--bogus"),
            "the error must name the offending flag: {err}"
        );
    }

    /// Every `--tier` string maps onto its `StreamTarget` arm (all
    /// five accepted spellings, checked against the enum).
    #[test]
    fn all_tier_values_map() {
        assert_eq!(
            parse_cli(["--tier", "memory"]).expect("memory must parse").target,
            StreamTarget::Tier(Tier::Memory)
        );
        assert_eq!(
            parse_cli(["--tier", "l1"]).expect("l1 must parse").target,
            StreamTarget::Tier(Tier::L1)
        );
        assert_eq!(
            parse_cli(["--tier", "l2"]).expect("l2 must parse").target,
            StreamTarget::Tier(Tier::L2)
        );
        assert_eq!(
            parse_cli(["--tier", "l3"]).expect("l3 must parse").target,
            StreamTarget::Tier(Tier::L3)
        );
        assert_eq!(
            parse_cli(["--tier", "full"]).expect("full must parse").target,
            StreamTarget::Full
        );
    }

    /// A second positional is rejected with its name (only one
    /// subcommand is accepted — flags do not count as positionals).
    #[test]
    fn extra_positional_is_rejected() {
        let err = parse_cli(["dump", "bench"])
            .expect_err("a second positional must be rejected");
        assert!(
            err.contains("bench"),
            "the error must name the offending argument: {err}"
        );
    }

    /// All three value-taking flags reject a missing value with a
    /// message naming the flag.
    #[test]
    fn missing_flag_values_are_rejected() {
        for flag in ["--socket", "--tier", "--mode"] {
            let err = parse_cli(vec![flag])
                .expect_err(&format!("{flag} without a value must be rejected"));
            assert!(err.contains(flag), "the error must name the flag: {err}");
        }
    }

    /// `String` iterators parse exactly like `&str` ones (the generic
    /// bound is exercised with owned values, as `env::args` yields).
    #[test]
    fn owned_string_args_parse() {
        let args: Vec<String> =
            ["bench", "--socket", "/tmp/y.sock", "--tier", "l2", "--mode", "memory-only"]
                .iter()
                .copied()
                .map(str::to_owned)
                .collect();
        let parsed = parse_cli(args).expect("owned String args must parse");
        assert_eq!(parsed.subcommand, Subcommand::Bench);
        assert_eq!(parsed.socket, PathBuf::from("/tmp/y.sock"));
        assert_eq!(parsed.target, StreamTarget::Tier(Tier::L2));
        assert_eq!(parsed.mode, BenchMode::MemoryOnly);
    }

    /// (k) The `--version`/`-V` line is the package version, exactly
    /// (`env!` — it tracks the workspace release).
    #[test]
    fn version_line_is_the_package_version() {
        assert_eq!(
            version_line(),
            format!("ramSleuth ramsleuth-client v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// (k) `--help`/`-h` and `--version`/`-V` are the short-circuits:
    /// the first occurrence, in argv order, wins; anything else
    /// (including the subcommand + flags) is `None`.
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
            short_circuit(["dump", "--socket", "/tmp/x"]),
            ShortCircuit::None,
            "a plain subcommand + flag list is None"
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
}
