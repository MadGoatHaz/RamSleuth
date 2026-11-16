//! ramsleuth-tui — the live terminal dashboard (Phase 3, P3-24).
//!
//! This is the binary entry point (the library — `events` P3-22, `ui`
//! P3-23 — is consumed from the crate root). It wires the frozen pieces
//! into the running TUI:
//!
//! - **Terminal init** — crossterm raw mode + alternate screen (the
//!   cursor hidden, the screen cleared on entry — the ghosting
//!   baseline), wrapped in a `Drop` guard ([`TerminalGuard`]) that
//!   restores everything (disable raw mode, leave the alternate screen,
//!   show the cursor) on *every* exit path: a normal `[Q]`uit, an early
//!   return, or an unwind — the terminal is never left in raw mode. The
//!   main loop re-clears the screen on any strip visible→hidden
//!   transition (the ghosting guard — a closing strip's stale pixels
//!   would otherwise linger where the re-laid-out frame does not
//!   overwrite them).
//! - **Background updater** — a `std::thread` poller loop (the GUI
//!   `update.rs` `spawn_poller` precedent adapted to the TUI's fixed
//!   socket): each tick it re-reads the live [`TuiSettings`] from the
//!   shared `Arc<RwLock<AppState>>` (the render-side key writes are the
//!   one permitted main-thread mutation — no I/O, the D6 settings-panel
//!   precedent), clamps `poll_interval_ms` into the sane
//!   [`MIN_POLL_INTERVAL_MS`]…[`MAX_POLL_INTERVAL_MS`] range, and
//!   sleeps the live value — a changed knob takes effect on the next
//!   tick, no restart. The one connect per tick is both the
//!   connectivity probe (a fixed-socket TUI can only observe a daemon
//!   restart by trying to connect) and, when this tick fetches, the
//!   carrier of the `GetTelemetry` request (P3-18) — so a one-shot
//!   **baseline** (the startup snapshot and the disconnected→connected
//!   reconnect baseline — the C7-08/09 mechanism, so a refresh-off TUI
//!   is not dead on arrival) is exactly one connection. With the
//!   `refresh` gate on the poller runs the continuous cadence (the
//!   current 2 s live poll); off, only the baselines run and the
//!   snapshot stays frozen. Each successful `GetTelemetry` also
//!   records one Na-guarded graphs sample into the shared graphs ring
//!   (the five-series `[g]` overlay data — the poller is the only
//!   writer, plan D6: the bandwidth is the latest `Memory · Read`
//!   figure — the newest streamed progress event, else the terminal
//!   grid row 0, else NaN — and the CPU-temp source scan runs on this
//!   thread) and clears the ring on a disconnected→connected
//!   reconnect transition (the stale pre-outage samples would
//!   straddle the gap). It also services at most one bench command
//!   per tick from the key handler's `mpsc` channel (a queued
//!   [`TuiBenchCmd`] takes priority over this tick's telemetry
//!   step — the run streams to its terminal before the next poll,
//!   the GUI poller's exact structure; telemetry polling pauses
//!   for the run's duration), dispatching on the cmd's
//!   `duration_minutes` discriminator (the GUI C7-16 rule):
//!   `None` → [`run_bench`] (a normal run: a fresh connection,
//!   the `StartBenchmark`, and the stream drained into
//!   `state.bench` — the `BenchStarted` ack's run id, the
//!   `BenchProgress` events (cleared at run start), and exactly
//!   one terminal (`BenchResult` → the grid, `BenchCancelled`,
//!   or the daemon's structured `Error` — including the
//!   single-flight `Error` for a second start while a run is in
//!   flight)); `Some(n)` → [`run_burn_in`] (a burn-in: the
//!   `StartBurnIn`, the `BurnInProgress` ticks into
//!   `state.bench.burn_in` — the newest iteration / elapsed +
//!   the per-cell `latest` — and the same terminal shapes, the
//!   `BenchResult` grid also landing in `state.bench.grid`). The
//!   two run classes share only the `run_id` + the shared `cancel`
//!   flag (the GUI invariant): each run resets the flag at its
//!   start and checks it between frames; once set (the `[C]` key,
//!   TUI-20), the run stops daemon-side with a best-effort
//!   `CancelBenchmark` on its own connection (sent only once the
//!   `BenchStarted` ack has landed — a pre-ack cancel only sets
//!   the flag) and ends with `running` / `burn_in.running =
//!   false`. Both workers lock `state` only briefly, per mutation
//!   (C14-03 — the render thread's reads never park across the
//!   drain) and run the 120 s stream read timeout (C7-16: a run's
//!   frame gaps far exceed the client's 5 s transport default). It
//!   stops on the quit flag (or the channel disconnecting — the
//!   session is ending) and is joined (bounded) before exit.
//! - **Main loop** — each tick: the `[d]` screen's (the requirements
//!   strip's) auto-open-until-closed rule (TUI-22: while `diagnose`
//!   reports a requirement and the `[d]` key has not dismissed it, the
//!   `requirements_open` flag is force-true — a new requirement
//!   re-opens the screen on its own; a dismissed screen stays closed
//!   until the user toggles it open again), then
//!   `terminal.draw(render)` (the P3-23 three-zone dashboard over
//!   the shared state) + `events::poll_event(250 ms)` → the frozen
//!   `key_to_action` table (P3-22 + TUI-01/02) → [`dispatch_action`]:
//!   the state-only actions mutate the shared state under one brief
//!   write scope (no I/O while the lock is held, C14-03) — the
//!   `[g]`/`[t]`/`[d]` toggles write the `settings.*_open` flags
//!   (the `[d]` key toggles the info screen — the requirements strip,
//!   presence-driven, + the controls block — and drives the
//!   `requirements_dismissed` latch: closing the screen dismisses the
//!   auto-open, reopening clears it), the `[p]` poll / `[w]` window
//!   cycles advance the presets
//!   (the renderer reads `settings.graph_window_min` — the `[w]`
//!   cycle is where that value is supplied), the `[u]`/`[k]` unit +
//!   the `[a]` refresh toggles flip, the `[b]`/`[m]`/`[x]` run keys
//!   queue a `TuiBenchCmd` into the updater's bench channel (the run
//!   itself streams on the updater thread; the send is skipped + a
//!   dim status note recorded while any run — a normal bench or a
//!   burn-in — is in flight, the compound single-flight guard), and
//!   `[C]`ancel sets the shared cancel flag (no I/O on the key —
//!   the in-flight run's worker sends the `CancelBenchmark` on its
//!   own connection) — and they return the I/O side effect the loop
//!   performs with the lock released: `[R]`efresh forces an
//!   immediate `poll_once` on the main thread (one fast RPC),
//!   `[S]`napshot writes the dashboard text (the client's pure
//!   `render`, P3-19) to a timestamped file in the CWD and records
//!   the path in `daemon_status` (transient — the next poll
//!   overwrites it), `[E]`xport writes the current telemetry
//!   snapshot + the terminal bench grid (the `bench` key — the JSON
//!   `null` before the first completed run) as pretty JSON to
//!   `$HOME/ramsleuth-export-<unix-ts>.json` (the CWD fallback when
//!   `HOME` is unset, the GUI F3 rule) — one file write on the main
//!   thread — and records the written path (or the failure, or the
//!   no-telemetry hint) in the state the same transient way,
//!   `[F]` probe-report (chunk probe-4) performs the consent-gated
//!   `GetProbeReport` fetch on a fresh connection (one request —
//!   not a telemetry poll) and, on a report, opens the modal
//!   consent overlay (the chunk-2-rendered markdown rides along in
//!   the state); while the overlay is open the main loop routes its
//!   keys to it, bypassing the global key table — `[y]` moves the
//!   markdown into the scrollable preview, `[q]`/`[n]`/`Esc` close
//!   it, and in the preview `[w]` writes `~/.ramsleuth/probe-report.md`,
//!   `[c]` copies to the clipboard (`wl-copy` / `xclip` / `xsel`),
//!   and `[q]`/`Esc` return to the dashboard (a `[q]` there closes
//!   the preview, it does not quit the TUI) — and `[Q]`uit breaks
//!   the loop.
//!
//! **No-panic contract (plan D5):** errors never end the TUI — a daemon
//! down, a timeout, a protocol violation, a draw failure, or an event
//! stream failure is recorded in `AppState` (`error` + `daemon_status`
//! `disconnected`) where the status zone shows it, and the loop keeps
//! running.
//!
//! **Exit codes:** `0` a normal quit (`--help`/`-h` and
//! `--version`/`-V` short-circuit to `0` before parsing too); `1` the
//! terminal could not be initialized (friendly message; nothing to
//! restore); `2` a usage error (unknown flag / positional / missing
//! value, with the usage text — the ramsleuth-daemon P3-17 /
//! ramsleuth-client P3-21 precedent).
//!
//! Manual verification (live TTY, the QA phase): with the dev daemon
//! running (`cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock`),
//! `cargo run -p ramsleuth-tui -- --socket /tmp/ramsleuth.sock` renders
//! the three zones from the live snapshot (values update every ~2 s,
//! `[R]` forces one now, `[S]` writes `ramsleuth-tui-<unix-ts>.txt` in
//! the CWD, `[E]` writes `ramsleuth-export-<unix-ts>.json` in `$HOME`
//! (the CWD when `HOME` is unset), `[F]` fetches the probe report
//! (the consent prompt → the markdown preview → `[w]` writes
//! `~/.ramsleuth/probe-report.md`, `[c]` copies to the clipboard,
//! `[q]` returns to the dashboard), `[Q]` restores the terminal +
//! exit 0); with no daemon the
//! dashboard stays responsive and shows `disconnected` + the
//! daemon-start hint.
//!
//! ```text
//! Usage: ramsleuth-tui [OPTIONS]
//!
//! Keys:
//!   [R]efresh        force one poll (works with refresh off)
//!   [S]napshot       write a .txt dashboard snapshot to the CWD
//!   [Q]uit           exit (exit code 0)
//!   [B]ench (full)   start a full benchmark run
//!   [M]emory         start a memory-only benchmark run
//!   [X] burn-in      start a 5-minute burn-in soak
//!   [C]ancel         cancel the in-flight run
//!   [E]xport         write { telemetry, bench } JSON to $HOME
//!   [G]raphs         toggle the graphs overlay panel
//!   [T]settings      toggle the settings strip
//!   [D]requirements  toggle the setup requirements strip
//!   [P]oll           cycle the poll interval (100 ms → 60 s, wrap)
//!   [U]nits          toggle the capacity units (GiB ↔ GB)
//!   [K]lock          toggle the clock units (MHz ↔ GHz)
//!   [A]uto refresh   toggle the periodic data poll (on ↔ off)
//!   [W]indow         cycle the graphs window (1 → 5 → 15 → 60 min)
//!   [F] probe report  consent → preview → write/copy the report
//!
//! Options:
//!   --socket <path>   Daemon Unix socket
//!                     (default: /run/ramsleuth/ramsleuth.sock)
//!   -h, --help        Print this help and exit
//!   -V, --version     Print the version and exit
//!
//! Exit codes: 0 quit, 1 terminal init failure, 2 usage error
//! ```

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ramsleuth_bench::{BenchOp, BenchmarkGrid, BurnInTick, StreamTarget, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::cli_common::{short_circuit, version_line, ShortCircuit, USAGE_COMMON};
use ramsleuth_protocol::{BenchMode, Request, Response, DEFAULT_SOCKET_PATH};
use ramsleuth_telemetry::probe::render_probe_report_md;
use ramsleuth_telemetry::SystemMemoryTelemetry;
use ramsleuth_tui::events;
use ramsleuth_tui::probe::{
    copy_to_clipboard, probe_report_default_path, write_probe_report, NoticeTone, ProbeOverlayState,
};
use ramsleuth_tui::{key_to_action, render, Action, AppState, BenchState};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
// ---------------------------------------------------------------------------
// Poison-tolerant shared-state guard acquisition (FE-3).
//
// The shared state lives behind an `RwLock` held by both threads (the
// updater writes, the render / poll thread reads). A stray panic that drops
// a guard poisons the lock; a bare `.read().unwrap()` / `.write().unwrap()`
// on the next acquisition would re-panic, cascading one bad read across
// both threads and taking the whole UI down. These two helpers are the
// single acquisition idiom: `unwrap_or_else(|p| p.into_inner())` recovers
// the inner value from a poisoned guard, so a panic degrades (the last-good
// state is read) instead of cascading (the no-panic contract, D5).
// ---------------------------------------------------------------------------

/// Acquire a read guard on the shared state, recovering a poisoned lock
/// (FE-3): `into_inner` yields the last-good inner value instead of
/// re-panicking, so one stray panic degrades a single read, never the
/// whole UI.
fn state_read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|p| p.into_inner())
}

/// Acquire a write guard on the shared state, recovering a poisoned lock
/// (FE-3): the same idiom as [`state_read`], for the mutation path.
fn state_write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|p| p.into_inner())
}

/// The poll interval's sane lower bound in milliseconds (the GUI
/// `update.rs` `MIN_POLL_INTERVAL_MS` rule, re-stated locally): a
/// degenerate stored knob can never make the loop hot-spin — the
/// no-panic contract (D5). The [`TuiSettings::poll_interval_ms`] field
/// doc (TUI-09) pins the clamp to this read site.
const MIN_POLL_INTERVAL_MS: u64 = 100;

/// The poll interval's sane upper bound in milliseconds (the GUI
/// `update.rs` `MAX_POLL_INTERVAL_MS` rule, re-stated locally): an
/// absurd stored knob can never stall the loop (D5).
const MAX_POLL_INTERVAL_MS: u64 = 60_000;

/// Clamp a configured poll interval (milliseconds) to the sane range
/// [`MIN_POLL_INTERVAL_MS`]…[`MAX_POLL_INTERVAL_MS`] (the GUI
/// `update.rs` `clamp_poll_interval` mirror, re-applied defensively at
/// the read site): a zero / absurd stored knob can never make the loop
/// hot-spin or stall — the no-panic contract (D5).
fn clamp_poll_interval(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS))
}

/// The main loop's event-poll timeout (plan P3-24: a 250 ms tick — the
/// redraw cadence, so the `…s ago` stamp and progress line advance live).
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// How long to wait for the updater after quit: it finishes within one
/// update cycle (a live clamped-interval `sleep` + at most one
/// connect-retry window), so this deadline is generous; if it is
/// exceeded the thread is detached
/// (the process is exiting — returning from `main` terminates it, and
/// the terminal is already restored by then).
const JOIN_DEADLINE: Duration = Duration::from_secs(5);

/// The sleep granularity while waiting for the updater to finish.
const JOIN_POLL: Duration = Duration::from_millis(50);

/// The bench / burn-in stream read timeout (the GUI `update.rs`
/// `BENCH_READ_TIMEOUT` mirror, C7-16): 120 s *between frames* —
/// a run streams its progress over minutes, so a legitimate gap
/// between frames can far exceed the client's 5 s transport default
/// (the CLI precedent); the deadline only bounds a silently wedged
/// daemon.
const BENCH_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Usage text printed for `--help`/`-h` and on parse errors (exit 2) —
/// the ramsleuth-daemon P3-17 / ramsleuth-client P3-21 precedent. The
/// default socket path is the protocol's frozen `DEFAULT_SOCKET_PATH`
/// value (P3-10) as a literal: `concat!` only accepts literals, and
/// the protocol freeze test pins the string.
const USAGE: &str = concat!(
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

/// One benchmark request from the key handler to the background
/// updater (the TUI-local mirror of the GUI `update.rs` `BenchCmd`):
/// the cell target + the scope + the run-class discriminator —
/// `None` = a normal single-pass benchmark ([`run_bench`]);
/// `Some(n)` = a burn-in with a duration of `n` minutes
/// ([`run_burn_in`]).
#[derive(Debug, Clone, Copy)]
struct TuiBenchCmd {
    /// Which cells run (`Full` / one `Tier` / one `Cell`).
    target: StreamTarget,
    /// The run scope (the daemon clamps it onto the target, P3-15).
    mode: BenchMode,
    /// The run-class discriminator: `None` = a normal run
    /// ([`run_bench`]); `Some(n)` = a burn-in with a duration of
    /// `n` minutes.
    duration_minutes: Option<u32>,
}

/// One telemetry poll cycle: connect to the daemon at `socket`, request
/// the current snapshot (`GetTelemetry`), and update `state` — the single
/// unit both the background updater thread and the `[R]`efresh key share.
///
/// Testable, no terminal. The no-panic contract (plan D5): every failure
/// class (a missing / refused socket → the `ClientError::DaemonDown`
/// friendly "start it with …" text, a timeout, a protocol violation, an
/// i/o failure) is recorded in `state.error` with `daemon_status`
/// degrading to `disconnected`; a successful `Telemetry` arm stores the
/// snapshot, stamps `last_update`, reports `connected: <socket>`, and
/// clears the error; a structured `Error` reply records its message. The
/// function always returns `Ok(())` — errors are reported through the
/// state, not a `Result` error, so the caller's loop keeps running
/// against a flapping daemon.
pub fn poll_once(socket: &Path, state: &mut AppState) -> Result<(), String> {
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state.error = Some(error.to_string());
            state.daemon_status = "disconnected".to_owned();
            return Ok(());
        }
    };
    poll_with_client(&mut client, socket, state);
    Ok(())
}

/// The shared `GetTelemetry` dispatch over an already-open `client`:
/// the state updates [`poll_once`] documents (the no-panic contract,
/// D5). Split out so the poller's one connect per tick can carry the
/// request — a baseline / reconnect / cadence fetch is exactly one
/// connection, never a probe connect plus a fetch connect.
///
/// TUI-18: a successful `Telemetry` arm also records one Na-guarded
/// graphs sample for the just-landed snapshot ([`record_graph_sample`]
/// — the poller is the only writer, plan D6) and clears the graphs
/// ring on a disconnected→connected reconnect transition (the
/// previous status was the `disconnected` marker — the GUI C6-25
/// reconnect-prime precedent, TUI-adapted to the TUI's status
/// vocabulary so the `[S]`napshot's transient `snapshot: …` status
/// never reads as a reconnect).
fn poll_with_client(client: &mut Client, socket: &Path, state: &mut AppState) {
    match client.request(&Request::GetTelemetry) {
        Ok(Response::Telemetry(telemetry)) => {
            // Reconnect prime (TUI-18, the GUI C6-25 precedent): the
            // previous cycle recorded the `disconnected` marker (the
            // daemon came back after an outage — the first poll has an
            // empty status, the `[S]`napshot transient a `snapshot: …`
            // one, neither a reconnect) — clear the stale ring so the
            // sparkline restarts flat instead of straddling the outage
            // with a gap.
            let reconnected = state.daemon_status == "disconnected";
            state.telemetry = Some(telemetry);
            state.last_update = Some(Instant::now());
            state.daemon_status = format!("connected: {}", socket.display());
            state.error = None;
            if reconnected {
                state.graph.samples.clear();
            }
            record_graph_sample(state);
        }
        Ok(Response::Error(message)) => {
            // The daemon was reachable but rejected the request
            // (a structured wire error, not a transport failure): record
            // the message, keep the current daemon status.
            state.error = Some(message);
        }
        Ok(
            Response::BenchStarted { .. }
            | Response::BenchProgress(_)
            | Response::BenchResult { .. }
            | Response::BenchCancelled { .. }
            | Response::BurnInProgress(_)
            | Response::ProbeReport(_),
        ) => {
            // A benchmark / burn-in frame in reply to `GetTelemetry`
            // violates the wire contract (the daemon streams those
            // only to the owning benchmark / burn-in connection,
            // P3-16): record a structured error.
            state.error = Some("unexpected response to GetTelemetry".to_owned());
        }
        Err(error) => {
            state.error = Some(error.to_string());
            state.daemon_status = "disconnected".to_owned();
        }
    }
}

/// Append one graphs-panel sample (TUI-18) for the just-landed
/// snapshot — the TUI-local mirror of the GUI `update.rs` D-4 / D-5
/// source map:
///
/// - the CPU core frequency + the VDDCR rails ride inside
///   [`ramsleuth_tui::graphs::record_graph_sample`]'s Na guard (an
///   absent / non-finite source degrades its own field to NaN —
///   0 ≠ N/A);
/// - the CPU temperature (°C) comes from the runtime
///   [`ramsleuth_tui::graphs::read_cpu_temp_c`] source scan — the
///   caller runs the I/O on its own thread (the poller; plan D6:
///   never the render thread);
/// - the memory-read bandwidth (GB/s) comes from
///   [`latest_memory_read_bw`] — its `0.0` no-figure sentinel maps to
///   NaN here, so the row stays its no-source note until the first
///   bench / burn-in sample (D-4: a flat 0 line would be a lie).
///
/// The Na guard (the no-panic contract, D5): a sample lands only when
/// ≥ 1 field is finite — an all-NaN poll appends nothing (the
/// graphs.rs `record_graph_sample` precedent).
fn record_graph_sample(state: &mut AppState) {
    let bandwidth = latest_memory_read_bw(&state.bench);
    let bandwidth_gbps = if bandwidth > 0.0 { bandwidth } else { f64::NAN };
    ramsleuth_tui::graphs::record_graph_sample(
        &mut state.graph,
        &state.telemetry,
        ramsleuth_tui::read_cpu_temp_c(),
        bandwidth_gbps,
    );
}

/// The latest memory-read bandwidth figure (GB/s) for the graphs
/// bandwidth series (TUI-18 — the TUI-local mirror of the GUI
/// `update.rs` D-5 rule over the frozen [`BenchState`]): the newest
/// streamed `Memory · Read` progress event when it carries a finite,
/// positive value (a live run), else the terminal grid's memory-read
/// cell (row 0 — the [`Tier::Memory`] slot), else `0.0` (the no-figure
/// sentinel — [`record_graph_sample`] maps it to NaN; a flat 0 line
/// would be a lie, D-4: 0 ≠ N/A). Non-finite / non-positive values
/// never count as a figure (the bench zone's live-cell rule).
fn latest_memory_read_bw(bench: &BenchState) -> f64 {
    if let Some(event) = bench.progress.iter().rev().find(|e| {
        e.tier == Tier::Memory && e.op == BenchOp::Read && e.value.is_finite() && e.value > 0.0
    }) {
        return event.value;
    }
    if let Some(grid) = &bench.grid {
        let value = grid.read_gbps[0];
        if value.is_finite() && value > 0.0 {
            return value;
        }
    }
    0.0
}

/// The compound single-flight guard for the run keys (TUI-20):
/// true while **any** run class is in flight — a normal bench
/// (`running`) or a burn-in (`burn_in.running`). The two classes
/// never touch each other's flag (the GUI invariant: they share
/// only the `run_id` + the cancel flag), so the `[b]` / `[m]` /
/// `[x]` send-skip must consider both — the daemon is
/// single-flight (a second start gets its structured `Error`), and
/// the guard keeps the skip UX-side: a skipped send records a dim
/// status note (TUI-22).
fn run_in_flight(bench: &BenchState) -> bool {
    bench.running || bench.burn_in.running
}

/// One benchmark run (TUI-19 — the GUI `update.rs` `run_bench`
/// mirror): connect to the daemon at `socket`, send the
/// `StartBenchmark` for `cmd`, and drain the reply stream — a
/// `BenchStarted` ack, the `BenchProgress` events, and exactly one
/// terminal — into `state.bench`.
///
/// Testable, no thread: it takes the shared `&RwLock<AppState>` and
/// locks it only briefly, per mutation — the guard is always
/// released before the next stream `recv()` (C14-03: the render
/// thread's reads never park across the drain, so the dashboard
/// stays responsive during a long run). The run starts with
/// `running = true`, the progress list cleared, and a stale `run_id`
/// dropped (a stale run's events never mix into a new one); the
/// terminal frame (`BenchResult` → the grid, `BenchCancelled`, or
/// the daemon's `Error`) sets `running = false`; a transport failure
/// (a closed stream, a timeout, …) or a contract-violating frame (a
/// `Telemetry` or `BurnInProgress` frame in this stream) records
/// `state.error` and the same. Always returns `Ok(())` (the
/// no-panic contract, as in [`poll_once`]) — the updater loop must
/// survive every failure.
///
/// **Cancel (the `[C]` key, TUI-20):** `cancel` is the shared flag
/// the Cancel key sets from the main thread — the
/// `CancelBenchmark` itself goes out on the run's own connection
/// (a pre-ack cancel only sets the flag). It is reset to `false`
/// at the start of every run (a stale cancel never kills a new
/// one) and checked before each `recv()`: once set, the run is
/// stopped daemon-side with a best-effort `CancelBenchmark` for
/// the current `run_id` (only once the `BenchStarted` ack has
/// landed — the daemon's reply is never read) and the loop breaks
/// with `running = false` (the clean stop, plan D6: the in-flight
/// pass finishes, the run ends at the next gate).
fn run_bench(
    socket: &Path,
    cmd: TuiBenchCmd,
    state: &RwLock<AppState>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // The shared cancel flag is reset per run: a stale `true` from a
    // cancelled run must not abort this one before its first frame.
    cancel.store(false, Ordering::Relaxed);
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            // A brief per-mutation lock scope (C14-03) — never held
            // across the stream drain.
            let mut s = state_write(state);
            s.bench.running = false;
            s.error = Some(error.to_string());
            return Ok(());
        }
    };
    // The stream read timeout (C7-16): the client's 5 s default would
    // kill a long run's frame gap mid-stream (the 120 s CLI
    // precedent).
    if let Err(error) = client.set_read_timeout(BENCH_READ_TIMEOUT) {
        let mut s = state_write(state);
        s.bench.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    {
        // The run starts: `running = true`, the progress list cleared,
        // and a stale `run_id` dropped (a stale run's events never
        // mix into a new one) — one brief scope. The normal-bench clock
        // (FE-9) is stamped here by the updater, not the render pass:
        // the status line reads `run_started` (a pure function of
        // `&AppState`), so the render never mutates a global.
        let mut s = state_write(state);
        s.bench.running = true;
        s.bench.progress.clear();
        s.bench.run_started = Some(Instant::now());
        // The new run's id arrives with the `BenchStarted` ack.
        s.bench.run_id = None;
    }
    if let Err(error) = client.send(&Request::StartBenchmark {
        target: cmd.target,
        mode: cmd.mode,
    }) {
        let mut s = state_write(state);
        s.bench.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    loop {
        // The shared flag is set (the `[C]` wiring lands in TUI-20):
        // ask the daemon for a clean stop (best-effort — the reply is
        // never read) and break before the next frame.
        if cancel.load(Ordering::Relaxed) {
            // The run id is read under a brief scope; the
            // `CancelBenchmark` itself goes out with no lock held.
            let run_id = state_read(state).bench.run_id;
            if let Some(run_id) = run_id {
                let _ = client.send(&Request::CancelBenchmark { run_id });
            }
            state_write(state).bench.running = false;
            break;
        }
        match client.recv() {
            Ok(Response::BenchStarted { run_id }) => {
                // The run's id lands (the Cancel key addresses the
                // run with it, TUI-20) — a brief scope.
                state_write(state).bench.run_id = Some(run_id);
            }
            Ok(Response::BenchProgress(progress)) => {
                // One streamed progress event (latest last) — a
                // brief scope.
                state_write(state).bench.progress.push(progress);
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal frame: the grid + the clean stop in
                // one brief scope.
                let mut s = state_write(state);
                s.bench.grid = Some(grid);
                s.bench.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                // The clean-stop ack (the run ended at its cancel
                // gate) — a brief scope.
                state_write(state).bench.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                // A structured daemon reply — including the
                // single-flight `Error` for a second start while a
                // run is in flight (P3-15/D-6): record + end, never
                // a panic (D5).
                let mut s = state_write(state);
                s.error = Some(message);
                s.bench.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the bench stream violates the
                // wire contract (`GetTelemetry` is served on its own
                // connection, P3-16): stop the run and record it.
                let mut s = state_write(state);
                s.error = Some("unexpected response during benchmark".to_owned());
                s.bench.running = false;
                break;
            }
            Ok(Response::BurnInProgress(_)) => {
                // A burn-in frame in a normal-bench stream violates
                // the wire contract (burn-in ticks stream only on the
                // owning `StartBurnIn` connection, D-1/D-2): stop
                // the run and record it (the GUI `run_bench` mirror).
                let mut s = state_write(state);
                s.error = Some("unexpected burn-in frame during benchmark".to_owned());
                s.bench.running = false;
                break;
            }
            Ok(Response::ProbeReport(_)) => {
                // A probe-report frame in a normal-bench stream
                // violates the wire contract (probe reports reply
                // only to `GetProbeReport`, chunk probe-1a): stop
                // the run and record it.
                let mut s = state_write(state);
                s.error = Some("unexpected probe report during benchmark".to_owned());
                s.bench.running = false;
                break;
            }
            Err(error) => {
                let mut s = state_write(state);
                s.error = Some(error.to_string());
                s.bench.running = false;
                break;
            }
        }
    }
    Ok(())
}

/// One burn-in run (TUI-20 — the GUI `update.rs` `run_burn_in`
/// mirror): connect to the daemon at `socket`, send the
/// `StartBurnIn` for `cmd`, and drain the reply stream — a
/// `BenchStarted` ack, the `BurnInProgress` ticks, and exactly one
/// terminal — into `state.bench` (the `burn_in` state + the
/// terminal grid).
///
/// Testable, no thread: it takes the shared `&RwLock<AppState>` and
/// locks it only briefly, per mutation — the guard is always
/// released before the next stream `recv()` (C14-03: the render
/// thread's reads never park across the drain, so the dashboard
/// stays responsive during a long soak). The run starts with
/// `burn_in.running = true`, a fresh zero `latest` grid + zeroed
/// iteration / elapsed (a stale run's values never mix into a new
/// one), and a stale `run_id` dropped (the new run's id arrives
/// with the `BenchStarted` ack); each tick updates the newest
/// iteration / elapsed and writes its cell / latency into `latest`
/// (the newest value per cell wins — the bench zone's `live_grid`
/// rule); the terminal frame (`BenchResult` → also the terminal
/// grid, `BenchCancelled`, or the daemon's `Error`) sets
/// `burn_in.running = false`. The run never touches
/// `BenchState::running` / `progress` (those are the normal-bench
/// state; the two run classes share only the `run_id` + the cancel
/// flag, the GUI invariant).
///
/// **Cancel (the `[C]` key, TUI-20):** the shared `cancel` flag is
/// reset to `false` at the start of every run (a stale cancel never
/// kills a new one) and checked before each `recv()`: once set, the
/// run is stopped daemon-side with a best-effort
/// `CancelBenchmark` for the current `run_id` (only once the
/// `BenchStarted` ack has landed — a pre-ack cancel only sets the
/// flag; the daemon's reply is never read) and the loop breaks with
/// `burn_in.running = false` (the clean stop, plan D6: the
/// in-flight pass finishes, the run ends at the next gate).
///
/// A contract-violating frame (a `BenchProgress` or `Telemetry`
/// frame in this stream) or a transport failure (a closed stream,
/// a timeout, …) records `state.error` and ends the run the same
/// way. Always returns `Ok(())` (the no-panic contract, as in
/// [`run_bench`]) — the updater loop must survive every failure.
fn run_burn_in(
    socket: &Path,
    cmd: TuiBenchCmd,
    state: &RwLock<AppState>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // The shared cancel flag is reset per run: a stale `true` from a
    // cancelled run must not abort this one before its first frame.
    cancel.store(false, Ordering::Relaxed);
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            // A brief per-mutation lock scope (C14-03) — never held
            // across the stream drain.
            let mut s = state_write(state);
            s.bench.burn_in.running = false;
            s.error = Some(error.to_string());
            return Ok(());
        }
    };
    // The stream read timeout (C7-16): the client's 5 s default
    // would kill a long soak's frame gap mid-stream (the 120 s CLI
    // precedent).
    if let Err(error) = client.set_read_timeout(BENCH_READ_TIMEOUT) {
        let mut s = state_write(state);
        s.bench.burn_in.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    {
        // A fresh run starts from the idle burn-in state: a stale
        // run's tick bookkeeping (iteration / elapsed / `latest`)
        // never mixes into a new one — one brief scope.
        let mut s = state_write(state);
        s.bench.burn_in = ramsleuth_tui::ui::BurnInState::default();
        s.bench.burn_in.running = true;
        // The new run's id arrives with the `BenchStarted` ack.
        s.bench.run_id = None;
    }
    // The updater dispatches `duration_minutes.is_some()` to this
    // path; a `None` cmd reaching it is a dispatch contract
    // violation (never a normal-bench run): record it and stop —
    // no panic (D5).
    let Some(duration_minutes) = cmd.duration_minutes else {
        let mut s = state_write(state);
        s.bench.burn_in.running = false;
        s.error = Some("burn-in command without a duration_minutes discriminator".to_owned());
        return Ok(());
    };
    if let Err(error) = client.send(&Request::StartBurnIn {
        target: cmd.target,
        duration_minutes,
    }) {
        let mut s = state_write(state);
        s.bench.burn_in.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    loop {
        // The `[C]` key set the shared flag: ask the daemon for a
        // clean stop (best-effort — the reply is never read) and
        // break before the next frame. A pre-ack cancel (no
        // `run_id` yet) only sets the flag: the run ends at this
        // gate with no request sent.
        if cancel.load(Ordering::Relaxed) {
            // The run id is read under a brief scope; the
            // `CancelBenchmark` itself goes out with no lock held.
            let run_id = state_read(state).bench.run_id;
            if let Some(run_id) = run_id {
                let _ = client.send(&Request::CancelBenchmark { run_id });
            }
            state_write(state).bench.burn_in.running = false;
            break;
        }
        match client.recv() {
            Ok(Response::BenchStarted { run_id }) => {
                // The shared run id lands (the `[C]` key addresses
                // the run with it) — a brief scope.
                state_write(state).bench.run_id = Some(run_id);
            }
            Ok(Response::BurnInProgress(tick)) => {
                // The tick's bookkeeping lands in the burn-in state
                // under one brief write scope. The daemon is the single
                // source of the clock (FE-1): the newest tick's
                // `elapsed_secs` wins (monotonic — a re-stamped /
                // elapsed-update tick never moves it backwards), and its
                // cell / latency (when it carries one) is written into
                // `latest` (the newest value per cell wins — the bench
                // zone's `live_grid` rule; a non-finite / non-positive
                // reading never renders as data).
                let BurnInTick {
                    iteration,
                    elapsed_secs,
                    tier,
                    bandwidth,
                    latency_ns,
                } = tick;
                let mut s = state_write(state);
                s.bench.burn_in.elapsed_secs = s.bench.burn_in.elapsed_secs.max(elapsed_secs);
                let slot = tier as usize;
                match (bandwidth, latency_ns) {
                    (Some((op, value)), _) => {
                        // A data tick: the newest iteration + its cell.
                        s.bench.burn_in.iteration = iteration;
                        if value.is_finite() && value > 0.0 {
                            match op {
                                BenchOp::Read => {
                                    s.bench.burn_in.latest.read_gbps[slot] = value;
                                }
                                BenchOp::Write => {
                                    s.bench.burn_in.latest.write_gbps[slot] = value;
                                }
                                BenchOp::Copy => {
                                    s.bench.burn_in.latest.copy_gbps[slot] = value;
                                }
                            }
                        }
                    }
                    (None, Some(ns)) => {
                        // A latency data tick: the newest iteration +
                        // its latency.
                        s.bench.burn_in.iteration = iteration;
                        if ns.is_finite() && ns > 0.0 {
                            s.bench.burn_in.latest.latency_ns[slot] = ns;
                        }
                    }
                    (None, None) => {
                        // An elapsed-update tick (FE-1): the daemon's
                        // ~1 s clock — it carries no cell / latency, so
                        // only `elapsed_secs` (set above) is updated and
                        // the run continues (the smooth live display).
                    }
                }
            }
            Ok(Response::BenchProgress(_)) => {
                // A normal-bench progress frame in a burn-in stream
                // violates the wire contract (`BenchProgress`
                // streams only on the owning `StartBenchmark`
                // connection, D-1/D-2): record the structured
                // error and stop the run — the permanent mirror of
                // `run_bench`'s `BurnInProgress` contract guard.
                let mut s = state_write(state);
                s.error = Some("unexpected benchmark frame during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal grid is the last completed pass: it
                // lands in the shared terminal grid (the zone-2
                // table + the later F3 export pick it up for free)
                // + the clean stop in one brief scope.
                let mut s = state_write(state);
                s.bench.grid = Some(grid);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                // The clean-stop ack (the run ended at its cancel
                // gate) — a brief scope.
                state_write(state).bench.burn_in.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                // A structured daemon reply — including the
                // single-flight `Error` for a second start while a
                // run is in flight (P3-15/D-6): record + end, never
                // a panic (D5).
                let mut s = state_write(state);
                s.error = Some(message);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the burn-in stream violates
                // the wire contract (`GetTelemetry` is served on its
                // own connection, P3-16): stop the run and record
                // it.
                let mut s = state_write(state);
                s.error = Some("unexpected response during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::ProbeReport(_)) => {
                // A probe-report frame in a burn-in stream
                // violates the wire contract (probe reports reply
                // only to `GetProbeReport`, chunk probe-1a): stop
                // the run and record it.
                let mut s = state_write(state);
                s.error = Some("unexpected probe report during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Err(error) => {
                let mut s = state_write(state);
                s.error = Some(error.to_string());
                s.bench.burn_in.running = false;
                break;
            }
        }
    }
    Ok(())
}

/// A `Drop` guard around the initialized terminal.
///
/// Owns the `Terminal<CrosstermBackend>` after raw mode + the alternate
/// screen are enabled, and restores everything on drop: disable raw mode,
/// leave the alternate screen, show the cursor. Because the restore is a
/// `Drop` impl, the terminal is restored on *every* exit path — a normal
/// `[Q]`uit, an early `return`, or an unwind — and the restore errors
/// are swallowed (a half-dead terminal is still better than a raw-mode
/// zombie; the workspace no-panic contract expects neither anyway).
struct TerminalGuard {
    terminal: Option<Terminal<CrosstermBackend<io::Stdout>>>,
}

impl TerminalGuard {
    /// Enable raw mode, enter the alternate screen (hiding the cursor),
    /// create the ratatui terminal, and clear it. Every partial failure
    /// unwinds what it already changed before returning the error — raw
    /// mode is never left on. The entry clear is the ghosting baseline:
    /// the alternate screen arrives with whatever pixels it last held
    /// (the main screen's content, a stale prior TUI's buffer), and the
    /// first frame must draw onto a blank surface.
    fn new() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| format!("failed to enable raw mode: {e}"))?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(format!("failed to enter the alternate screen: {e}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(e) => {
                let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                return Err(format!("failed to initialize the terminal: {e}"));
            }
        };
        if let Err(e) = terminal.clear() {
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
            let _ = disable_raw_mode();
            return Err(format!("failed to clear the terminal: {e}"));
        }
        Ok(Self {
            terminal: Some(terminal),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.take() {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
        }
    }
}

/// Spawn the background updater: the GUI-style poller loop (the
/// `update.rs` `spawn_poller` precedent adapted to the TUI's fixed
/// socket). Each tick it re-reads the live [`TuiSettings`] from the
/// shared state (the render-side key writes are the one permitted
/// main-thread mutation — no I/O, the D6 settings-panel precedent),
/// clamps `poll_interval_ms` to [`clamp_poll_interval`], and sleeps the
/// live value — a changed knob takes effect on the next tick, no
/// restart. The one connect per tick is both the connectivity probe
/// (a fixed-socket TUI can only observe a daemon restart by trying to
/// connect) and, when this tick fetches, the carrier of the
/// `GetTelemetry` request — so a fetch is exactly one connection.
///
/// The fetch decision per tick (the C7-08/09 refresh gating): the
/// one-shot **baseline** runs on the first tick (the startup snapshot)
/// and on every disconnected→connected transition (the reconnect
/// baseline) — always, refresh on or off, so a refresh-off TUI is not
/// dead on arrival; the **cadence** (the continuous data poll, the
/// current 2 s live poll) runs only while the `refresh` gate is on and
/// the last fetch is at least the clamped interval old. With the gate
/// off and the daemon steadily up, the probe connect is dropped without
/// a fetch — the snapshot stays frozen (the `…s ago` stamp advances).
///
/// TUI-19/20: the loop first services at most one bench command
/// from `bench_rx` per tick (a queued command takes priority over
/// this tick's telemetry step, and a run streams to its terminal
/// before the next poll — the GUI poller's exact structure;
/// telemetry polling pauses for the run's duration; runs are
/// single-flight daemon-side anyway, P3-15), dispatching on the
/// cmd's `duration_minutes` discriminator to [`run_bench`] (a
/// normal run) or [`run_burn_in`] (a burn-in), and handing the
/// shared `cancel` flag to the run (each run resets it and checks
/// it between frames; once set by the `[C]` key, the run stops
/// daemon-side with a best-effort `CancelBenchmark` on its own
/// connection — sent only after the `BenchStarted` ack has landed;
/// a pre-ack cancel only sets the flag).
///
/// The thread runs until `stop` is set (quit) or the bench command
/// channel disconnects (the sender dropped — the session is ending):
/// a telemetry tick is bounded (the connect-retry window ~300 ms +
/// the transport's 5 s read timeout), and a bench tick by the run's
/// stream (the 120 s frame-gap deadline — C7-16), so a wedged daemon
/// cannot hold it forever.
fn spawn_updater(
    state: Arc<RwLock<AppState>>,
    stop: Arc<AtomicBool>,
    socket: PathBuf,
    bench_rx: mpsc::Receiver<TuiBenchCmd>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Was the daemon connected at the end of the last tick: `false`
        // before the first, so the first successful connect is the
        // one-shot startup baseline (D-8); every later
        // disconnected→connected transition is a reconnect baseline.
        let mut prev_connected = false;
        // When the last fetch ran: with the `refresh` gate on, the
        // cadence polls once the live clamped interval has elapsed.
        let mut last_poll = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // The live settings knobs (C6-27 / the D6 precedent):
            // re-read every tick so a changed interval (clamped) or
            // refresh gate takes effect without a restart.
            let (refresh, interval) = {
                let app = state_read(&state);
                (
                    app.settings.refresh,
                    clamp_poll_interval(app.settings.poll_interval_ms),
                )
            };
            // TUI-19: a queued bench command takes priority over this
            // tick's telemetry step — at most one per tick, and a run
            // streams to its terminal before the next poll (the GUI
            // poller's exact structure; telemetry polling pauses for
            // the run's duration).
            match bench_rx.try_recv() {
                Ok(cmd) => {
                    // The run-class dispatch (the GUI C7-16 rule):
                    // the cmd's `duration_minutes` is the
                    // discriminator — `Some` routes to
                    // `run_burn_in` (a burn-in), `None` to
                    // `run_bench` (a normal run); both lock `state`
                    // only briefly, per mutation — never across
                    // their stream drain (C14-03: the render
                    // thread's reads stay responsive for the whole
                    // run), and both take the shared `cancel` flag
                    // (reset per run, checked between frames).
                    if cmd.duration_minutes.is_some() {
                        let _ = run_burn_in(&socket, cmd, &state, &cancel);
                    } else {
                        let _ = run_bench(&socket, cmd, &state, &cancel);
                    }
                }
                Err(TryRecvError::Empty) => {
                    match Client::connect(&socket) {
                        Err(error) => {
                            // Daemon down: the friendly disconnected
                            // state (the no-panic contract, D5); the
                            // next tick re-probes.
                            let mut snapshot = state_write(&state);
                            snapshot.error = Some(error.to_string());
                            snapshot.daemon_status = "disconnected".to_owned();
                            prev_connected = false;
                        }
                        Ok(mut client) => {
                            // The disconnected→connected transition
                            // (the first tick is one): the one-shot
                            // baseline — startup or reconnect — runs
                            // regardless of the `refresh` gate (D-8,
                            // the C7-08/09 mechanism).
                            let reconnected = !prev_connected;
                            let due = reconnected || (refresh && last_poll.elapsed() >= interval);
                            if due {
                                // Baseline / reconnect / cadence: send
                                // `GetTelemetry` on this connection (one
                                // connection total) and update the state.
                                poll_with_client(&mut client, &socket, &mut state_write(&state));
                                last_poll = Instant::now();
                            }
                            // Not due (refresh off, steady state): drop
                            // the probe connection without a fetch —
                            // the snapshot stays frozen, no periodic
                            // data poll.
                            prev_connected = true;
                        }
                    }
                }
                // The sender dropped: the session is ending (`run`
                // keeps it alive until the updater is joined) — no
                // more bench commands, the loop ends (the GUI
                // `spawn_poller` precedent).
                Err(TryRecvError::Disconnected) => break,
            }
            // Sleep the live value: the next tick re-reads the knobs.
            thread::sleep(interval);
        }
    })
}

/// Join `updater` within [`JOIN_DEADLINE`]: it finishes within one update
/// cycle after `stop` is set, so this is normally near-instant; if the
/// deadline is exceeded the thread is detached (the process is exiting —
/// returning from `main` terminates it, and the terminal was restored
/// before this call).
fn join_updater_bounded(updater: JoinHandle<()>) {
    let deadline = Instant::now() + JOIN_DEADLINE;
    while !updater.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(JOIN_POLL);
    }
    let _ = updater.join();
}

/// The snapshot file path: `ramsleuth-tui-<unix-ts>.txt` in the current
/// directory (plan P3-24: the timestamped text snapshot in the CWD; the
/// file name follows the `ramsleuth-tui-<unix-ts>` brief template). A
/// pathological pre-epoch clock or a missing CWD degrades to timestamp
/// `0` / `.` — never a panic.
fn snapshot_path() -> PathBuf {
    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    dir.join(format!("ramsleuth-tui-{unix_ts}.txt"))
}

/// The `[S]`napshot action: write the current dashboard as text to
/// [`snapshot_path`] and record the written path in `daemon_status`
/// (transient — the next poll overwrites it with the connection state;
/// `AppState` has no spare field and this chunk is scoped to `main.rs`).
///
/// The content is the client's pure `dump::render` (P3-19) of the current
/// telemetry — the same dashboard text the CLI `dump` prints — or a small
/// placeholder when no snapshot has arrived yet. A write failure is
/// recorded in `state.error`; the TUI never dies on a snapshot.
fn write_snapshot(state: &Arc<RwLock<AppState>>) {
    let text = {
        let snapshot = state_read(state);
        match &snapshot.telemetry {
            Some(telemetry) => ramsleuth_client::render(telemetry),
            None => {
                let status = if snapshot.daemon_status.is_empty() {
                    "not connected".to_owned()
                } else {
                    snapshot.daemon_status.clone()
                };
                format!(
                    "RamSleuth TUI snapshot\n\ndaemon: {status}\n\nno telemetry yet (daemon down?)\n"
                )
            }
        }
    };
    let path = snapshot_path();
    match std::fs::write(&path, text) {
        Ok(()) => {
            state_write(state).daemon_status = format!("snapshot: {}", path.display());
        }
        Err(e) => {
            state_write(state).error = Some(format!("snapshot failed: {e}"));
        }
    }
}

/// The F3 wire snapshot (TUI-21 — the GUI `style.rs` C6-29 mirror):
/// the frozen [`SystemMemoryTelemetry`] root flattened into the top
/// level (its seven wire keys, unchanged — no type duplication, D2)
/// plus the `bench` key.
#[derive(serde::Serialize)]
struct ExportSnapshot<'a> {
    /// The telemetry snapshot, flattened into the top level of the
    /// file (the seven wire keys: `cpu` / `amd` / `intel` / `spd` /
    /// `platform` / `total_capacity` / `dimm_sizes`).
    #[serde(flatten)]
    telemetry: &'a SystemMemoryTelemetry,
    /// The terminal 4×4 benchmark grid (the wire unmeasured `N/A`
    /// cells are the honest `0.0`); `null` before the first completed
    /// run.
    bench: Option<&'a BenchmarkGrid>,
}

/// The wall-clock unix timestamp (seconds) for the export file name
/// (the GUI F2/F3 `unix_timestamp` mirror). A pre-epoch clock
/// (impossible on Linux) degrades to 0 — never a panic.
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The `$HOME` → CWD-fallback rule in its pure form (the GUI rule,
/// testable without env access): a set `HOME` is the export
/// destination; an unset `HOME` degrades to the CWD (`.`) — never a
/// panic.
fn out_dir_from_home(home: Option<&OsStr>) -> PathBuf {
    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The export destination dir (the GUI F2/F3 rule): `$HOME`, falling
/// back to the CWD when `HOME` is unset — the one env read the
/// `[E]`xport handler makes (the GUI F3 side-effect precedent).
fn export_out_dir() -> PathBuf {
    out_dir_from_home(std::env::var_os("HOME").as_deref())
}

/// F3: write one telemetry + benchmark snapshot to `path` as pretty
/// JSON (the GUI `style.rs::export_json` mirror, TUI-local — there is
/// no `GuiError` in the TUI crate: a JSON failure and a file write
/// map to a `String` the caller records in `state.error`).
///
/// The wire root ([`SystemMemoryTelemetry`], serde-derived since
/// P3-06) serializes under its own seven wire keys, and the terminal
/// [`BenchmarkGrid`] rides alongside it under the `bench` key (C6-29,
/// item 8b): the grid object when a run has completed, the JSON
/// `null` before the first run. Both absent-grid shapes — no run yet
/// (`None`) and the all-`0.0`/`N/A` grid — export as-is, never a
/// panic (D5).
fn export_json(
    telemetry: &SystemMemoryTelemetry,
    bench: Option<&BenchmarkGrid>,
    path: &Path,
) -> Result<(), String> {
    let snapshot = ExportSnapshot { telemetry, bench };
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("export JSON serialization failed: {err}"))?;
    std::fs::write(path, json).map_err(|err| format!("export file write failed: {err}"))?;
    Ok(())
}

/// The `[E]`xport action (TUI-21 — the GUI `main.rs` `perform_export`
/// `ExportJson` arm, TUI-local): write the current telemetry
/// snapshot + the terminal bench grid (the `bench` key — the JSON
/// `null` before the first completed run) to
/// `ramsleuth-export-<unix-ts>.json` in `out_dir`, returning the
/// written path. No telemetry yet → `Ok(None)` (no file — the GUI
/// rule). The only I/O is the one file write (no socket, no daemon,
/// no window) — testable against a temp `out_dir`.
fn perform_export(state: &AppState, out_dir: &Path) -> Result<Option<PathBuf>, String> {
    match &state.telemetry {
        Some(telemetry) => {
            let ts = unix_timestamp();
            let path = out_dir.join(format!("ramsleuth-export-{ts}.json"));
            export_json(telemetry, state.bench.grid.as_ref(), &path)?;
            Ok(Some(path))
        }
        None => Ok(None),
    }
}

/// Record one `[E]`xport outcome in the state (the GUI transient
/// notice, the `[S]`napshot mechanism): the written path lands in
/// `daemon_status` (transient — the next poll overwrites it, as the
/// snapshot path does), the no-telemetry hint lands there too, and a
/// write failure (a missing dir, a read-only `$HOME`, …) lands in
/// `error` — the TUI never dies on an export (the no-panic contract,
/// D5).
fn record_export_result(state: &mut AppState, result: Result<Option<PathBuf>, String>) {
    match result {
        Ok(Some(path)) => {
            state.daemon_status = format!("export: {}", path.display());
        }
        Ok(None) => {
            state.daemon_status = "nothing to export yet (no telemetry)".to_owned();
        }
        Err(message) => {
            state.error = Some(format!("export failed: {message}"));
        }
    }
}

/// The `[E]`xport key handler (TUI-21 — the GUI F3 side-effect
/// precedent): the one file write of the key, on the main thread —
/// the brief read scope takes the snapshot data (the file write runs
/// with no lock held, C14-03), into the `$HOME`/CWD-fallback out dir
/// (the GUI rule), and the outcome is recorded via
/// [`record_export_result`].
fn write_export(state: &Arc<RwLock<AppState>>) {
    let result = {
        let snapshot = state_read(state);
        perform_export(&snapshot, &export_out_dir())
    };
    record_export_result(&mut state_write(state), result);
}

// ------------------------------------------------------------------
// chunk probe-4: the probe-report flow — the one-shot
// `GetProbeReport` fetch (the `[F]` key's I/O side effect) + the
// modal overlay key handler (the consent keys, the preview keys,
// the preview scroll).
// ------------------------------------------------------------------

/// The `[F]` probe-report fetch (chunk probe-4): a one-shot
/// `GetProbeReport` over a fresh connection (the poller's
/// one-connection-per-request precedent — the probe report is its
/// own request class, not a telemetry poll): a
/// `Response::ProbeReport` renders to markdown (chunk 2's
/// `render_probe_report_md`) and opens the consent overlay (the
/// submission is consent-gated — the `[y]` answer moves the
/// markdown into the preview, no second fetch). Every failure class
/// (daemon down, a structured daemon `Error`, a contract-violating
/// frame, a transport error) is recorded in `state.error` — the
/// no-panic contract (D5); the dashboard keeps running, the overlay
/// never opens on a failed fetch.
fn probe_report_fetch(socket: &Path, state: &Arc<RwLock<AppState>>) {
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state_write(state).error = Some(error.to_string());
            return;
        }
    };
    match client.request(&Request::GetProbeReport) {
        Ok(Response::ProbeReport(report)) => {
            let markdown = render_probe_report_md(&report);
            state_write(state).probe = ProbeOverlayState::Consent { markdown };
        }
        Ok(Response::Error(message)) => {
            // The daemon was reachable but rejected the request
            // (a structured wire error, not a transport failure).
            state_write(state).error = Some(message);
        }
        Ok(_) => {
            // Any other frame in reply to `GetProbeReport` violates
            // the wire contract (probe reports reply only to
            // `GetProbeReport`, chunk probe-1a): record it (D5).
            state.write().unwrap().error = Some("unexpected response to GetProbeReport".to_owned());
        }
        Err(error) => {
            state_write(state).error = Some(error.to_string());
        }
    }
}

/// The preview's markdown, when the overlay is in the preview phase
/// (the `[w]`/`[c]` actions' guard — the consent phase has no write
/// / copy): a one-time clone (an intentional action, never a
/// per-keystroke cost).
fn preview_markdown(state: &Arc<RwLock<AppState>>) -> Option<String> {
    let app = state_read(state);
    match &app.probe {
        ProbeOverlayState::Preview { markdown, .. } => Some(markdown.clone()),
        _ => None,
    }
}

/// Set the preview's transient notice line (the `[w]`/`[c]`
/// outcome): the tone colors it (the render maps `Ok` → green,
/// `Warn` → amber, `Error` → crimson); a closed overlay is a
/// no-op (the callers' phase guard makes that unreachable — the
/// fn stays total, D5).
fn set_preview_notice(state: &Arc<RwLock<AppState>>, tone: NoticeTone, text: String) {
    if let ProbeOverlayState::Preview { notice, .. } = &mut state_write(state).probe {
        *notice = Some((tone, text));
    }
}

/// Scroll the preview's markdown by `delta` lines (the
/// `Up`/`Down`/`PageUp`/`PageDown` keys): clamped to the probe
/// module's `preview_max_scroll` over the current terminal `area`
/// (the same layout the render draws) — a short markdown never
/// scrolls, the end is never overshot.
fn scroll_probe_preview(state: &Arc<RwLock<AppState>>, area: Rect, delta: i32) {
    let (current, max) = {
        let app = state_read(state);
        match &app.probe {
            ProbeOverlayState::Preview {
                markdown, scroll, ..
            } => (
                *scroll,
                ramsleuth_tui::probe::preview_max_scroll(markdown, area),
            ),
            _ => return,
        }
    };
    let next = (i64::from(current) + i64::from(delta)).clamp(0, i64::from(max)) as u16;
    if next != current {
        if let ProbeOverlayState::Preview { scroll, .. } = &mut state_write(state).probe {
            *scroll = next;
        }
    }
}

/// The modal overlay's key handler (chunk probe-4): the consent
/// keys (`[y]` → the preview, `[q]`/`[n]`/`Esc` → close), the
/// preview keys (`[w]` → write `write_path`, `[c]` → the
/// `clipboard` closure, `[q]`/`[n]`/`Esc` → close), and the preview
/// scroll
/// (`Up`/`Down`/`PageUp`/`PageDown` over `area`) — every other key
/// (including `[f]` itself) is inert while the overlay is open.
/// The caller (the main loop) routes a key here only while the
/// overlay is open and bypasses the global key table (a `[q]` in
/// the preview closes it — it does not quit the TUI).
fn handle_probe_overlay_key(
    key: KeyEvent,
    state: &Arc<RwLock<AppState>>,
    area: Rect,
    write_path: &Path,
    clipboard: &dyn Fn(&str) -> Result<String, String>,
) {
    match key.code {
        // `[y]` (consent only): move the carried markdown into the
        // preview (the one-time clone; a `[y]` in the preview is a
        // no-op — the answer keys are consumed).
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let mut app = state_write(state);
            if let ProbeOverlayState::Consent { markdown } =
                std::mem::replace(&mut app.probe, ProbeOverlayState::None)
            {
                app.probe = ProbeOverlayState::Preview {
                    markdown,
                    notice: None,
                    scroll: 0,
                };
            }
        }
        // `[q]` / `[n]` / `Esc` (both phases): close the overlay —
        // cancel the consent / quit the preview (the natural
        // `[q]`uit key must never be trapped by the modal; it
        // closes it, it does not quit the TUI).
        KeyCode::Char('q')
        | KeyCode::Char('Q')
        | KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Esc => {
            state_write(state).probe = ProbeOverlayState::None;
        }
        // `[w]` (preview only): write the report to `write_path`
        // (the live loop passes `~/.ramsleuth/probe-report.md`; the
        // tests pass a temp dir) — the outcome is the notice line.
        KeyCode::Char('w') | KeyCode::Char('W') => {
            let Some(markdown) = preview_markdown(state) else {
                return;
            };
            let (tone, text) = match write_probe_report(write_path, &markdown) {
                Ok(path) => (
                    NoticeTone::Ok,
                    format!("Report written to {}", path.display()),
                ),
                Err(error) => (NoticeTone::Error, format!("write failed: {error}")),
            };
            set_preview_notice(state, tone, text);
        }
        // `[c]` (preview only): copy the report to the clipboard —
        // the outcome (a success note naming the utility, or the
        // fixed no-utility warning) is the notice line.
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let Some(markdown) = preview_markdown(state) else {
                return;
            };
            let (tone, text) = match clipboard(&markdown) {
                Ok(note) => (NoticeTone::Ok, note),
                Err(_) => (
                    NoticeTone::Warn,
                    "No clipboard utility found — use [w] to write to file".to_owned(),
                ),
            };
            set_preview_notice(state, tone, text);
        }
        // The preview scroll (the `area` clamp — the render's
        // layout).
        KeyCode::Up => scroll_probe_preview(state, area, -1),
        KeyCode::Down => scroll_probe_preview(state, area, 1),
        KeyCode::PageUp => scroll_probe_preview(state, area, -10),
        KeyCode::PageDown => scroll_probe_preview(state, area, 10),
        // Every other key is inert while the overlay is open.
        _ => {}
    }
}

// ------------------------------------------------------------------
// TUI-22 + chunk probe-4: the action dispatch — the frozen 17-key
// table (P3-22 + TUI-01/02 + the probe-report action) over the
// shared state: the state-only actions (the toggles, the cycles,
// the run keys, the cancel) mutate the state and report no side
// effect; the I/O actions report the effect the main loop performs
// with the lock released.
// ------------------------------------------------------------------

/// The I/O side effect one dispatched action asks the main loop to
/// perform with the state lock released (TUI-22 — the dispatch
/// split that makes the key `match` unit-testable without a TTY;
/// the state-only actions report [`None`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideEffect {
    /// No side effect: the action mutated the state only (the
    /// toggles, the cycles, the run keys, the cancel).
    None,
    /// `[R]`: force one immediate `poll_once` on the main thread.
    Refresh,
    /// `[S]`: write the timestamped text snapshot to the CWD.
    Snapshot,
    /// `[E]`: write the JSON export to `$HOME` (the CWD fallback).
    Export,
    /// `[F]`: fetch the probe report (one RPC on a fresh
    /// connection) + open the consent overlay (chunk probe-4).
    ProbeReport,
    /// `[Q]`: quit the TUI (exit 0).
    Quit,
}

/// The poll-interval presets in cycle order (plan §2.2 `[p]`):
/// 100 ms → 500 ms → 1 s → 2 s → 5 s → 10 s → 30 s → 60 s — the
/// 2 s default sits mid-list (the current TUI behavior).
const POLL_PRESETS_MS: [u64; 8] = [100, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000];

/// The graphs window presets in cycle order (plan §2.2 `[w]`,
/// minutes): 1 → 5 → 15 → 60 — the 5-min default (the current TUI
/// behavior).
const WINDOW_PRESETS_MIN: [u32; 4] = [1, 5, 15, 60];

/// The dim status note a skipped run key records (TUI-22 — the GUI
/// single-flight UX over the transient `daemon_status` mechanism):
/// the next poll overwrites it, as the `[S]`napshot path does.
const RUN_IN_FLIGHT_NOTE: &str = "a run is already in flight (one at a time)";

/// The next poll-interval preset after `ms` (the `[p]` cycle, plan
/// §2.2): the first preset strictly above the current value — an
/// off-preset stored value (a hand-edited default) advances to the
/// next preset above it, and past the last (60 s) the cycle wraps to
/// the first (100 ms). Total — never panics (D5).
fn cycle_poll(ms: u64) -> u64 {
    POLL_PRESETS_MS
        .iter()
        .copied()
        .find(|preset| *preset > ms)
        .unwrap_or(POLL_PRESETS_MS[0])
}

/// The next graphs window preset after `min` (the `[w]` cycle, plan
/// §2.2): the first preset strictly above the current value,
/// wrapping past 60 min to 1 min — total (D5).
fn cycle_window(min: u32) -> u32 {
    WINDOW_PRESETS_MIN
        .iter()
        .copied()
        .find(|preset| *preset > min)
        .unwrap_or(WINDOW_PRESETS_MIN[0])
}

/// The `TuiBenchCmd` one run key starts (plan §2.2): the cell
/// target + the scope + the run-class discriminator — `[b]` a full
/// run, `[m]` a memory-only run, `[x]` a 5-minute burn-in (the GUI
/// default duration; `0` = infinite is not key-reachable, plan
/// §5.6). The non-run actions carry no command.
fn bench_cmd(action: Action) -> Option<TuiBenchCmd> {
    match action {
        Action::BenchFull => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: None,
        }),
        Action::BenchMemory => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::MemoryOnly,
            duration_minutes: None,
        }),
        Action::BurnIn => Some(TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(5),
        }),
        _ => None,
    }
}

/// The one-action dispatch over the shared state (TUI-22 — the
/// main loop's key `match` split into this pure, unit-testable fn +
/// the I/O arms the loop performs from its return value, so the
/// dispatch is testable without a TTY): the state-only actions
/// mutate `state` and return [`SideEffect::None`] — the `[g]`/`[t]`
/// toggles flip the `settings.*_open` flags, the `[d]` key toggles
/// the requirements strip *and* drives the `requirements_dismissed`
/// latch (closing dismisses the auto-open — a new requirement stays
/// hidden until the user toggles the strip open again; reopening
/// clears the dismissal — the auto-open applies again), the `[p]`
/// poll / `[w]` window cycles advance the presets (the renderer
/// reads `settings.graph_window_min` — the `[w]` cycle is where
/// that value is supplied), the `[u]`/`[k]`/`[a]` toggles flip the
/// unit / refresh knobs, the `[b]`/`[m]`/`[x]` run keys queue a
/// `TuiBenchCmd` into the updater's bench channel (a channel send —
/// no I/O; skipped + a dim status note while any run is in flight,
/// the compound single-flight guard), and `[c]` sets the shared
/// cancel flag; the I/O actions return their effect for the loop to
/// perform with the lock released: `[R]` → [`SideEffect::Refresh`],
/// `[S]` → [`SideEffect::Snapshot`], `[E]` → [`SideEffect::Export`],
/// `[F]` → [`SideEffect::ProbeReport`], `[Q]` →
/// [`SideEffect::Quit`].
fn dispatch_action(
    action: Action,
    state: &mut AppState,
    requirements_dismissed: &mut bool,
    bench_tx: &mpsc::Sender<TuiBenchCmd>,
    cancel: &AtomicBool,
) -> SideEffect {
    match action {
        Action::Refresh => SideEffect::Refresh,
        Action::Snapshot => SideEffect::Snapshot,
        Action::Quit => SideEffect::Quit,
        Action::BenchFull | Action::BenchMemory | Action::BurnIn => {
            if run_in_flight(&state.bench) {
                // The compound single-flight guard (TUI-19/20): a
                // second start is skipped (the daemon would answer
                // with its structured `Error` anyway) + a dim
                // status note recorded (the GUI single-flight UX,
                // TUI-22 — the transient `daemon_status`
                // mechanism, as the `[S]`napshot path).
                state.daemon_status = RUN_IN_FLIGHT_NOTE.to_owned();
            } else if let Some(cmd) = bench_cmd(action) {
                // The command goes to the updater (a channel send —
                // no I/O, the one permitted main-thread mutation);
                // the run streams on the updater thread.
                let _ = bench_tx.send(cmd);
            }
            SideEffect::None
        }
        Action::Cancel => {
            // The shared flag (TUI-19/20): the in-flight run's
            // worker checks it between frames and stops the run
            // daemon-side. No I/O on the key.
            cancel.store(true, Ordering::Relaxed);
            SideEffect::None
        }
        Action::ExportJson => SideEffect::Export,
        Action::ProbeReport => SideEffect::ProbeReport,
        Action::ToggleGraphs => {
            state.settings.graphs_open = !state.settings.graphs_open;
            SideEffect::None
        }
        Action::ToggleSettings => {
            state.settings.settings_open = !state.settings.settings_open;
            SideEffect::None
        }
        Action::ToggleRequirements => {
            // The auto-open-until-closed rule (the plan's
            // `requirements_dismissed` latch): closing the strip
            // dismisses the auto-open (a later requirement
            // re-appearance stays hidden until the user toggles it
            // open again); reopening clears the dismissal (the
            // auto-open applies again while a requirement exists).
            if state.settings.requirements_open {
                state.settings.requirements_open = false;
                *requirements_dismissed = true;
            } else {
                state.settings.requirements_open = true;
                *requirements_dismissed = false;
            }
            SideEffect::None
        }
        Action::CyclePoll => {
            state.settings.poll_interval_ms = cycle_poll(state.settings.poll_interval_ms);
            SideEffect::None
        }
        Action::ToggleCapacity => {
            state.settings.capacity_gib = !state.settings.capacity_gib;
            SideEffect::None
        }
        Action::ToggleClock => {
            state.settings.clock_mhz = !state.settings.clock_mhz;
            SideEffect::None
        }
        Action::ToggleRefresh => {
            state.settings.refresh = !state.settings.refresh;
            SideEffect::None
        }
        Action::CycleWindow => {
            // The renderer reads `settings.graph_window_min` (ui.rs
            // `render` → `render_graphs_panel`): this cycle is where
            // that value is supplied.
            state.settings.graph_window_min = cycle_window(state.settings.graph_window_min);
            SideEffect::None
        }
    }
}

/// The requirements strip's auto-open-until-closed rule (TUI-22 —
/// the main-thread half of the plan's TUI-16 presence rule; the
/// renderer only reads the flag): while
/// [`ramsleuth_tui::requirements::diagnose`] reports a requirement
/// and the user has not dismissed the strip (the `[d]` key closed
/// it — the `requirements_dismissed` latch), the
/// `requirements_open` flag is force-true — a new requirement
/// re-opens the strip on its own; a dismissed strip stays closed
/// until the user toggles it open again. The read scope decides,
/// the (rare) write sets one field — never held long (C14-03).
fn apply_requirements_auto_open(state: &RwLock<AppState>, dismissed: bool) {
    if dismissed {
        return;
    }
    let needs_open = {
        let app = state_read(state);
        !app.settings.requirements_open && !ramsleuth_tui::requirements::diagnose(&app).is_empty()
    };
    if needs_open {
        state_write(state).settings.requirements_open = true;
    }
}

/// The `[d]` screen's + the settings strip's visibility [`render`]
/// draws: the controls block (the 17-key map) its `requirements_open`
/// flag alone (the screen's informational companion — drawn while the
/// screen is open, regardless of the strip's presence), the
/// requirements strip presence-driven — its `requirements_open` flag
/// AND a non-empty [`ramsleuth_tui::requirements::diagnose`] (the
/// strip auto-vanishes on the daemon-connect transition — `diagnose`
/// empties — so the flag alone would miss it) — and the settings strip
/// its `settings_open` flag alone. `render` applies this rule inline;
/// this helper is retained (test-only, FE-4) to pin the presence-driven
/// auto-vanish invariant.
#[cfg(test)]
fn strip_visibility(state: &AppState) -> (bool, bool, bool) {
    (
        state.settings.requirements_open,
        !ramsleuth_tui::requirements::diagnose(state).is_empty()
            && state.settings.requirements_open,
        state.settings.settings_open,
    )
}

/// The terminal event loop (P3-24): initialize the terminal (the
/// [`TerminalGuard`] restores it on the way out of this function), spawn
/// the background updater, then tick — draw the dashboard (each surface
/// erases its own region, FE-4 — no global screen-clear), poll events
/// for 250 ms, dispatch the frozen `Action` table. Returns
/// the process exit code: `0` on a normal quit, `1` when the terminal
/// could not be initialized.
fn run(args: TuiArgs) -> ExitCode {
    let mut guard = match TerminalGuard::new() {
        Ok(guard) => guard,
        Err(message) => {
            eprintln!("ramsleuth-tui: {message}");
            return ExitCode::FAILURE;
        }
    };

    // Shared presentation state: the updater writes, the draw loop reads.
    let state = Arc::new(RwLock::new(AppState::default()));
    // The quit flag: set on `[Q]` (and again on loop exit), read by the
    // updater between cycles.
    let stop = Arc::new(AtomicBool::new(false));
    // The bench command channel (TUI-19): the key handler sends a
    // `TuiBenchCmd` into it; the updater services at most one per tick.
    // The sender's binding lives for the whole session — dropping it
    // would disconnect the updater's receiver (and end its loop).
    let (bench_tx, bench_rx) = mpsc::channel();
    // The shared cancel flag (TUI-19/20): each run (a normal
    // bench or a burn-in) resets it at its start and checks it
    // between frames; the `[C]` key sets it (the run's worker then
    // sends the `CancelBenchmark` on its own connection — the key
    // does no I/O).
    let cancel = Arc::new(AtomicBool::new(false));
    // The `[d]` auto-open latch (TUI-22): `true` once the user has
    // closed the requirements strip — the auto-open rule stops
    // force-opening it (a new requirement stays hidden until the
    // user toggles the strip open again); `false` at startup and
    // after an explicit reopen.
    let mut requirements_dismissed = false;
    // FE-4: no global screen-clear ghosting guard. Each surface erases
    // its own region at the top of its paint (the single `blank_area`
    // idiom in `render`), and the zones expand to absorb a closed strip's
    // freed rows — so a visible→hidden transition never clears the whole
    // screen (no flicker). The one-time startup `terminal.clear()` (the
    // first frame draws onto a blank surface) is the only clear.
    let updater = spawn_updater(
        Arc::clone(&state),
        Arc::clone(&stop),
        args.socket.clone(),
        bench_rx,
        Arc::clone(&cancel),
    );

    while let Some(terminal) = guard.terminal.as_mut() {
        // The requirements strip's auto-open-until-closed rule
        // (TUI-22): a new requirement re-opens the strip on its own
        // until the `[d]` key dismisses it (a dismissed strip stays
        // closed until the user toggles it open again).
        apply_requirements_auto_open(&state, requirements_dismissed);

        // One draw per tick (250 ms cadence): the read lock is held only
        // for the frame render (pure over `&AppState`, P3-23). Each
        // surface erases its own region inside `render` (FE-4) — no
        // global screen-clear on a visible→hidden transition.
        let draw_state = Arc::clone(&state);
        if let Err(message) = terminal.draw(|frame| {
            let snapshot = state_read(&draw_state);
            render(frame, &snapshot);
        }) {
            // A failed draw (e.g. the terminal was detached) is recorded,
            // not fatal: the loop keeps running, the status zone shows it.
            state_write(&state).error = Some(format!("terminal draw failed: {message}"));
        }

        match events::poll_event(POLL_TIMEOUT) {
            Ok(Some(Event::Key(key))) => {
                // The modal probe-report overlay (chunk probe-4):
                // while one is open, it consumes the key (the
                // global key table below is bypassed — a `[q]` in
                // the preview closes it, it does not quit the TUI;
                // `[f]` does not re-fetch).
                if state_read(&state).probe.is_open() {
                    // The overlay's key handler takes the
                    // terminal's area (the scroll clamp's layout —
                    // the render draws the preview into the same
                    // whole-frame rect); a failed size read
                    // degrades to a zero rect (the clamp is total,
                    // D5).
                    let area = terminal
                        .size()
                        .map(|s| Rect::new(0, 0, s.width, s.height))
                        .unwrap_or(Rect::new(0, 0, 0, 0));
                    handle_probe_overlay_key(
                        key,
                        &state,
                        area,
                        &probe_report_default_path(),
                        &copy_to_clipboard,
                    );
                    // The overlay consumed the key: skip the global
                    // dispatch this tick (the next draw shows the
                    // overlay's update).
                    continue;
                }
                if let Some(action) = key_to_action(key) {
                    // The action's state mutations (the toggles, the
                    // cycles, the run-key channel sends, the cancel
                    // flag, the `[d]` latch) run under one brief
                    // write scope — no I/O while the lock is held
                    // (C14-03); the returned side effect (the `[R]`
                    // poll, the `[S]`/`[E]` file writes, the `[Q]`
                    // break) is performed with the lock released.
                    let side = {
                        let mut s = state_write(&state);
                        dispatch_action(
                            action,
                            &mut s,
                            &mut requirements_dismissed,
                            &bench_tx,
                            &cancel,
                        )
                    };
                    match side {
                        // The state-only actions (the toggles, the
                        // cycles, the run keys, the cancel): the
                        // mutation is done, nothing else to do.
                        SideEffect::None => {}
                        // `[R]`: force an immediate poll on the main
                        // thread (one fast RPC — bounded by the
                        // connect-retry window + the 5 s read
                        // timeout).
                        SideEffect::Refresh => {
                            let _ = poll_once(&args.socket, &mut state_write(&state));
                        }
                        // `[S]`: write the timestamped text snapshot
                        // to the CWD + record its path in
                        // daemon_status (transient — the next poll
                        // overwrites it).
                        SideEffect::Snapshot => write_snapshot(&state),
                        // `[E]`: write the JSON export (TUI-21 — the
                        // F3 parity): one file write on the main
                        // thread; the written path / the failure /
                        // the no-telemetry hint is recorded in the
                        // state (transient, as the `[S]`napshot
                        // path).
                        SideEffect::Export => write_export(&state),
                        // `[F]`: fetch the probe report (one RPC on
                        // a fresh connection) + open the consent
                        // overlay (chunk probe-4 — the preview is
                        // gated on the user's `[y]`; the overlay
                        // keys are consumed modal-style above).
                        SideEffect::ProbeReport => probe_report_fetch(&args.socket, &state),
                        // `[Q]`: stop the updater, break, restore
                        // the terminal (guard drop), exit 0.
                        SideEffect::Quit => {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
            // Resize / mouse / other events are ignored: the terminal
            // re-reads the surface size every frame (P3-22 contract).
            Ok(Some(_)) => {}
            // A clean 250 ms timeout: just redraw on the next tick.
            Ok(None) => {}
            Err(message) => {
                // An event-stream failure (e.g. a raw-mode teardown) is
                // recorded in the state — the loop keeps running.
                state_write(&state).error = Some(format!("event stream: {message}"));
            }
        }
    }

    // Belt and braces: the `[Q]` arm already set the flag.
    stop.store(true, Ordering::Relaxed);
    join_updater_bounded(updater);

    // The guard drops here → disable raw mode + leave the alternate
    // screen + show the cursor, before the exit code is returned.
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // `--help`/`-h` and `--version`/`-V` short-circuit before parsing
    // (standard, exit 0): help prints the usage text, version the
    // [`version_line`].
    match short_circuit(std::env::args().skip(1)) {
        ShortCircuit::Help => {
            print!("{USAGE}{USAGE_COMMON}");
            return ExitCode::SUCCESS;
        }
        ShortCircuit::Version => {
            println!("{}", version_line("ramsleuth-tui"));
            return ExitCode::SUCCESS;
        }
        ShortCircuit::None => {}
    }

    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("ramsleuth-tui: {message}");
            eprint!("{USAGE}{USAGE_COMMON}");
            return ExitCode::from(2);
        }
    };
    run(args)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
