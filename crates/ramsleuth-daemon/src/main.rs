//! ramsleuth-daemon — the privileged ramsleuth service (Phase 3, P3-17).
//!
//! The binary entry point: CLI parsing, the SOFT privilege probe
//! (P3-12, plan D5 — warnings to stderr, never an exit), Unix-socket
//! listener setup (P3-13), the shared [`DaemonContext`] (P3-16: the
//! P3-14 TTL telemetry cache over the real no-panic
//! `ramsleuth_telemetry::collect` plus the P3-15 single-flight
//! benchmark job manager), and the async accept loop — one
//! `handle_connection` task (P3-16) per accepted stream until a
//! SIGTERM/SIGINT stops the daemon gracefully (stop accepting,
//! best-effort socket removal, exit 0).
//!
//! C10 (D-2): the injected collector is spike-wrapped — every
//! re-collect (a cold cache or a TTL-expired `get()`) runs a ≤ 250 ms
//! `spike()` DRAM load, then the 150 ms `CLOCK_SETTLE` settle delay
//! (C14, M1), immediately before `collect()` re-reads the SMU PM
//! table, so the `MCLK` sample is taken at the operating frequency,
//! not the idle frequency; the in-TTL clone path never spikes (the
//! collector is not called).
//!
//! The collector's first line is the guarded SPD EEPROM auto-bind
//! fallback (`spd_bind`, disabled with `--no-spd-autobind`): on an
//! Intel box where the kernel's `ee1004` driver bound fewer EEPROMs
//! than the platform has active channels, the missing standard
//! addresses are attempted via the sysfs `new_device` mechanism before
//! the spike + settle + `collect()`, so a freshly bound EEPROM lands in
//! the same snapshot.
//!
//! **The daemon never panics** (plan D5): missing root or
//! `CAP_SYS_RAWIO` only warns — the daemon keeps serving with its
//! privileged fields degraded to `N/A`. The only permitted exits are
//! `2` (a CLI/parse error, with usage text) and `1` (a fatal
//! socket-setup or signal-install failure); signals stop it
//! gracefully and the process exits `0`.
//!
//! Manual verification (no unit installed):
//! - unprivileged dev run (expect the non-root / `CAP_SYS_RAWIO`
//!   warnings on stderr; the RPC still serves, privileged fields
//!   degrade to `N/A`):
//!   `cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock`
//! - privileged / production: install `systemd/ramsleuth.service`
//!   (this chunk; Phase 5 finalizes packaging) and
//!   `systemctl start ramsleuth`.
//!
//! ```text
//! Usage: ramsleuth-daemon [OPTIONS]
//!   --socket <path>    Unix socket to listen on (default: /run/ramsleuth/ramsleuth.sock)
//!   --max-age <secs>   telemetry cache TTL in seconds (default: 2)
//!   --no-spd-autobind  disable the guarded SPD EEPROM auto-bind fallback
//!   -h, --help         print usage and exit
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ramsleuth_daemon::{
    ensure_spd_eeproms_bound, handle_connection, probe, setup_listener, spike, BenchJobManager,
    DaemonContext, TelemetryCache,
};
use ramsleuth_protocol::DEFAULT_SOCKET_PATH;
use tokio::signal::unix::SignalKind;

/// Settle window between the DRAM spike and the PM-table read (C14, M1):
/// after the spike wakes the memory controller out of idle, give the SMU
/// 150 ms before sampling MCLK/UCLK/FCLK so first (and TTL-expired) reads
/// settle at the operating frequency, not a transient idle one.
const CLOCK_SETTLE: Duration = Duration::from_millis(150);

/// The daemon's parsed command-line arguments (P3-17).
///
/// Pure data — produced by [`parse_args`] (no env access, no I/O,
/// unit-tested) and consumed once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonArgs {
    /// The Unix socket path to listen on (`--socket`); defaults to
    /// [`DEFAULT_SOCKET_PATH`] (the single socket-path source, P3-10).
    pub socket_path: PathBuf,
    /// The telemetry cache TTL (`--max-age`, a non-negative number of
    /// seconds); defaults to 2 s. Inside the TTL, `GetTelemetry`
    /// returns the cached snapshot instead of re-collecting (P3-14).
    pub max_age: Duration,
    /// Whether the guarded SPD EEPROM auto-bind fallback is enabled
    /// (default: `true`; the `--no-spd-autobind` flag turns it off).
    /// When on, an Intel box where `ee1004` bound fewer EEPROMs than
    /// active channels gets the missing standard addresses attempted
    /// via sysfs `new_device` before each collect (root-only,
    /// non-fatal — see `spd_bind`).
    pub spd_autobind: bool,
}

impl Default for DaemonArgs {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            max_age: Duration::from_secs(2),
            spd_autobind: true,
        }
    }
}

/// Usage text printed for `--help` and on parse errors (the
/// ramsleuth-bench CLI precedent). The default socket path is the
/// protocol's frozen `DEFAULT_SOCKET_PATH` value (P3-10) as a literal:
/// `concat!` only accepts literals, and the protocol's freeze test
/// pins the string.
const USAGE: &str = concat!(
    "ramsleuth-daemon — the privileged ramsleuth telemetry/benchmark daemon\n",
    "\n",
    "Usage: ramsleuth-daemon [OPTIONS]\n",
    "\n",
    "Options:\n",
    "  --socket <path>    Unix socket to listen on (default: /run/ramsleuth/ramsleuth.sock)\n",
    "  --max-age <secs>   Telemetry cache TTL in seconds, a non-negative\n",
    "                     integer (default: 2)\n",
    "  --no-spd-autobind  Disable the guarded SPD EEPROM auto-bind\n",
    "                     fallback (default: enabled; Intel-only,\n",
    "                     root-only, non-fatal)\n",
    "  -h, --help         Print this help and exit\n",
    "\n",
    "Signals: SIGTERM and SIGINT stop the daemon gracefully (stop\n",
    "accepting, remove the socket file, exit 0).\n",
);

/// Parse `--socket <path>`, `--max-age <secs>`, and
/// `--no-spd-autobind` from `args` (the program name already removed).
///
/// Pure and testable: no env access, no I/O, no panic. Both value-taking
/// flags may repeat (the last value wins) and all flags may appear in
/// any order. A missing flag value, a non-numeric or negative
/// `--max-age`, or any unknown argument is a `String` error naming the
/// problem; a valid parse yields [`DaemonArgs`] with defaults for every
/// absent flag (the SPD auto-bind fallback defaults to enabled).
pub fn parse_args<I, S>(args: I) -> Result<DaemonArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = DaemonArgs::default();
    let mut iter = args.into_iter();
    while let Some(raw) = iter.next() {
        match raw.as_ref() {
            "--socket" => {
                let value = iter
                    .next()
                    .map(|v| v.as_ref().to_owned())
                    .ok_or_else(|| "--socket requires a value (the Unix socket path)".to_owned())?;
                parsed.socket_path = PathBuf::from(value);
            }
            "--max-age" => {
                let value = iter
                    .next()
                    .map(|v| v.as_ref().to_owned())
                    .ok_or_else(|| "--max-age requires a value (seconds)".to_owned())?;
                // A `u64` parse rejects negatives and non-numerics.
                // `Duration` holds nanoseconds in a `u64`, so a
                // seconds value whose nanosecond product would
                // overflow is rejected before the conversion
                // (no-panic contract).
                let seconds: u64 = value
                    .parse()
                    .map_err(|_| format!("--max-age must be a non-negative integer number of seconds, got `{value}`"))?;
                if seconds > u64::MAX / 1_000_000_000 {
                    return Err(format!(
                        "--max-age {seconds} s is too large to represent as a duration"
                    ));
                }
                parsed.max_age = Duration::from_secs(seconds);
            }
            "--no-spd-autobind" => {
                parsed.spd_autobind = false;
            }
            other => {
                return Err(format!(
                    "unknown argument `{other}` (expected `--socket <path>`, `--max-age <secs>`, `--no-spd-autobind`, or `--help`)"
                ));
            }
        }
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() {
    // `--help` is recognized before parsing (the ramsleuth-bench CLI
    // precedent): print usage and exit 0.
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }

    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("ramsleuth-daemon: {message}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    // SOFT privilege probe (P3-12, plan D5): missing root or
    // `CAP_SYS_RAWIO` only warns — the daemon keeps serving (its
    // privileged fields degrade to `N/A`), so no exit here.
    for warning in probe().warnings {
        eprintln!("warning: {warning}");
    }

    // The SPD auto-bind gate (default on; `--no-spd-autobind` off) —
    // copied out of `args` before the closure captures it (`args` is
    // still consumed by the listener setup and the graceful stop
    // below; the `bool` is `Copy`).
    let spd_autobind = args.spd_autobind;

    // The shared per-daemon state (P3-16): the P3-14 TTL telemetry
    // cache over the real no-panic collector (the `--max-age` is the
    // cache TTL) + the P3-15 single-flight benchmark job manager.
    let ctx = Arc::new(DaemonContext {
        cache: Arc::new(Mutex::new(TelemetryCache::new(
            move || {
                // First line (before the spike + settle + collect): the
                // guarded SPD auto-bind fallback — a freshly bound
                // EEPROM lands in the same snapshot. (`move` only
                // captures the `Copy` `spd_autobind` flag; `args.max_age`
                // is a separate argument below.)
                ensure_spd_eeproms_bound(spd_autobind);
                spike();
                std::thread::sleep(CLOCK_SETTLE);
                ramsleuth_telemetry::collect()
            },
            args.max_age,
        ))),
        jobs: Arc::new(BenchJobManager::new()),
    });

    // Listener setup (P3-13) — from inside the runtime (its final
    // `from_std` registers with the runtime's IO driver). A fatal
    // socket-setup failure is the only hard startup error: log and
    // exit 1 (never a panic, plan D5).
    let listener = match setup_listener(&args.socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("ramsleuth-daemon: socket setup failed: {error}");
            std::process::exit(1);
        }
    };

    println!("ramsleuth-daemon listening on {}", args.socket_path.display());

    // Shutdown handlers (plan D5: graceful stop, never a panic).
    // Handler installation is part of startup: a failure is fatal.
    let mut sigterm = match tokio::signal::unix::signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("ramsleuth-daemon: could not install the SIGTERM handler: {error}");
            std::process::exit(1);
        }
    };
    let mut sigint = match tokio::signal::unix::signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("ramsleuth-daemon: could not install the SIGINT handler: {error}");
            std::process::exit(1);
        }
    };

    // Accept loop: one `handle_connection` task (P3-16) per accepted
    // stream; a SIGTERM/SIGINT breaks the loop for a graceful stop.
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _addr)) => {
                    let ctx = Arc::clone(&ctx);
                    tokio::spawn(async move {
                        // `Ok(())` = a clean client disconnect (no
                        // log); an `RpcError` = an I/O or protocol
                        // failure (log and move on — one bad client
                        // must not affect the others).
                        if let Err(error) = handle_connection(stream, ctx).await {
                            eprintln!("ramsleuth-daemon: connection closed: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("warning: accept failed: {error}"),
            },
            _ = sigterm.recv() => {
                eprintln!("ramsleuth-daemon: received SIGTERM, shutting down");
                break;
            }
            _ = sigint.recv() => {
                eprintln!("ramsleuth-daemon: received SIGINT, shutting down");
                break;
            }
        }
    }

    // Graceful stop: best-effort socket-file removal (a failure — e.g.
    // the file already gone — only warns). Returning from
    // `#[tokio::main]` drops the runtime (with it any in-flight
    // connection tasks) and the process exits 0.
    if let Err(error) = std::fs::remove_file(&args.socket_path) {
        eprintln!(
            "warning: could not remove the socket file {}: {error}",
            args.socket_path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_protocol::DEFAULT_SOCKET_PATH;

    /// (a) No args → the defaults: the protocol's default socket path
    /// and a 2 s cache TTL.
    #[test]
    fn no_args_yields_defaults() {
        let args = parse_args(Vec::<&str>::new()).expect("no args must parse");
        assert_eq!(args, DaemonArgs::default());
        assert_eq!(args.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
        assert_eq!(args.max_age, Duration::from_secs(2));
    }

    /// (b) Both flags: the socket path and a 5 s TTL are honored.
    #[test]
    fn socket_and_max_age_flags_are_honored() {
        let args = parse_args(["--socket", "/tmp/x.sock", "--max-age", "5"])
            .expect("both flags must parse");
        assert_eq!(args.socket_path, PathBuf::from("/tmp/x.sock"));
        assert_eq!(args.max_age, Duration::from_secs(5));
    }

    /// (c) A zero TTL is a valid (non-negative) max age.
    #[test]
    fn zero_max_age_is_valid() {
        let args = parse_args(["--max-age", "0"]).expect("0 s must parse");
        assert_eq!(args.max_age, Duration::from_secs(0));
    }

    /// (d) A negative max age is rejected with a message naming the
    /// flag (the `u64` parse fails on the leading `-`).
    #[test]
    fn negative_max_age_is_rejected() {
        let err = parse_args(["--max-age", "-1"]).expect_err("a negative TTL must be rejected");
        assert!(
            err.contains("--max-age"),
            "the error must name the flag: {err}"
        );
    }

    /// (e) A non-numeric max age is rejected with a message naming
    /// the offending value.
    #[test]
    fn non_numeric_max_age_is_rejected() {
        let err = parse_args(["--max-age", "abc"]).expect_err("a non-numeric TTL must be rejected");
        assert!(
            err.contains("--max-age") && err.contains("abc"),
            "the error must name the flag and the value: {err}"
        );
    }

    /// (f) An unknown argument is rejected with its name.
    #[test]
    fn unknown_argument_is_rejected() {
        let err = parse_args(["--bogus"]).expect_err("an unknown argument must be rejected");
        assert!(
            err.contains("--bogus"),
            "the error must name the offending argument: {err}"
        );
    }

    /// (g) A flag missing its value is rejected with a message naming
    /// the flag (checked for both value-taking flags).
    #[test]
    fn missing_flag_values_are_rejected() {
        let err = parse_args(["--socket"]).expect_err("`--socket` without a value must be rejected");
        assert!(
            err.contains("--socket"),
            "the error must name the flag: {err}"
        );
        let err = parse_args(["--max-age"]).expect_err("`--max-age` without a value must be rejected");
        assert!(
            err.contains("--max-age"),
            "the error must name the flag: {err}"
        );
    }

    /// (h) The SPD auto-bind fallback defaults to enabled (absent
    /// flag).
    #[test]
    fn spd_autobind_defaults_on() {
        let args = parse_args(Vec::<&str>::new()).expect("no args must parse");
        assert!(args.spd_autobind);
    }

    /// (i) `--no-spd-autobind` disables the SPD auto-bind fallback
    /// (a flag-only argument; repeating it stays disabled).
    #[test]
    fn no_spd_autobind_flag_disables_autobind() {
        let args = parse_args(["--no-spd-autobind"]).expect("the flag must parse");
        assert!(!args.spd_autobind);
        let args = parse_args(["--no-spd-autobind", "--no-spd-autobind"])
            .expect("the repeated flag must parse");
        assert!(!args.spd_autobind);
    }

    /// Repeated flags: the last value wins, in any order.
    #[test]
    fn repeated_flags_last_wins() {
        let args = parse_args([
            "--max-age",
            "3",
            "--socket",
            "/tmp/a.sock",
            "--socket",
            "/tmp/b.sock",
            "--max-age",
            "7",
        ])
        .expect("repeated flags must parse");
        assert_eq!(args.socket_path, PathBuf::from("/tmp/b.sock"));
        assert_eq!(args.max_age, Duration::from_secs(7));
    }

    /// `String` iterators parse exactly like `&str` ones (the generic
    /// bound is exercised with owned values, as `env::args` yields).
    #[test]
    fn owned_string_args_parse() {
        let args: Vec<String> = ["--socket", "/tmp/y.sock", "--max-age", "4"]
            .iter()
            .copied()
            .map(str::to_owned)
            .collect();
        let parsed = parse_args(args).expect("owned String args must parse");
        assert_eq!(parsed.socket_path, PathBuf::from("/tmp/y.sock"));
        assert_eq!(parsed.max_age, Duration::from_secs(4));
    }
}
