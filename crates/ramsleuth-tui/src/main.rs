//! ramsleuth-tui — the live terminal dashboard (Phase 3, P3-24).
//!
//! This is the binary entry point (the library — `events` P3-22, `ui`
//! P3-23 — is consumed from the crate root). It wires the frozen pieces
//! into the running TUI:
//!
//! - **Terminal init** — crossterm raw mode + alternate screen, wrapped in
//!   a `Drop` guard ([`TerminalGuard`]) that restores everything (disable
//!   raw mode, leave the alternate screen, show the cursor) on *every*
//!   exit path: a normal `[Q]`uit, an early return, or an unwind — the
//!   terminal is never left in raw mode.
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
//!   for the run's duration): [`run_bench`] opens a fresh
//!   connection, sends the `StartBenchmark`, and drains the stream
//!   into `state.bench` — the `BenchStarted` ack's run id, the
//!   `BenchProgress` events (cleared at run start), and exactly one
//!   terminal (`BenchResult` → the grid, `BenchCancelled`, or the
//!   daemon's structured `Error` — including the single-flight
//!   `Error` for a second start while a run is in flight) — with
//!   the C14-03 brief-lock discipline (a write lock per mutation,
//!   always released before the next `recv`, so the render
//!   thread's reads never park across the drain) and the 120 s
//!   stream read timeout (C7-16: a run's frame gaps far exceed the
//!   client's 5 s transport default). It stops on the quit flag (or
//!   the channel disconnecting — the session is ending) and is
//!   joined (bounded) before exit.
//! - **Main loop** — each tick: `terminal.draw(render)` (the P3-23
//!   three-zone dashboard over the shared state) +
//!   `events::poll_event(250 ms)` → the frozen `key_to_action` table
//!   (P3-22): `[R]`efresh forces an immediate `poll_once` on the main
//!   thread (one fast RPC), `[S]`napshot writes the dashboard text (the
//!   client's pure `render`, P3-19) to a timestamped file in the CWD and
//!   records the path in `daemon_status` (transient — the next poll
//!   overwrites it), `[b]` / `[m]` send a `TuiBenchCmd` into the
//!   updater's bench channel (TUI-19 — the run itself streams on the
//!   updater thread; the send is skipped while a run is in flight),
//!   `[Q]`uit breaks the loop.
//!
//! **No-panic contract (plan D5):** errors never end the TUI — a daemon
//! down, a timeout, a protocol violation, a draw failure, or an event
//! stream failure is recorded in `AppState` (`error` + `daemon_status`
//! `disconnected`) where the status zone shows it, and the loop keeps
//! running.
//!
//! **Exit codes:** `0` a normal quit; `1` the terminal could not be
//! initialized (friendly message; nothing to restore); `2` a usage error
//! (unknown flag / positional / missing value, with the usage text — the
//! ramsleuth-daemon P3-17 / ramsleuth-client P3-21 precedent).
//!
//! Manual verification (live TTY, the QA phase): with the dev daemon
//! running (`cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock`),
//! `cargo run -p ramsleuth-tui -- --socket /tmp/ramsleuth.sock` renders
//! the three zones from the live snapshot (values update every ~2 s,
//! `[R]` forces one now, `[S]` writes `ramsleuth-tui-<unix-ts>.txt` in
//! the CWD, `[Q]` restores the terminal + exit 0); with no daemon the
//! dashboard stays responsive and shows `disconnected` + the
//! daemon-start hint.
//!
//! ```text
//! Usage: ramsleuth-tui [OPTIONS]
//!
//! Options:
//!   --socket <path>   Daemon Unix socket
//!                     (default: /run/ramsleuth/ramsleuth.sock)
//!
//! Keys: [R]efresh  [S]napshot  [Q]uit
//! Exit codes: 0 quit, 1 terminal init failure, 2 usage error
//! ```

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::cursor::Show;
use crossterm::event::Event;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ramsleuth_bench::{BenchOp, StreamTarget, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response, DEFAULT_SOCKET_PATH};
use ramsleuth_tui::events;
use ramsleuth_tui::{key_to_action, render, Action, AppState, BenchState};

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

/// Usage text printed on parse errors (exit 2) — the ramsleuth-daemon
/// P3-17 / ramsleuth-client P3-21 precedent. The default socket path is
/// the protocol's frozen `DEFAULT_SOCKET_PATH` value (P3-10) as a
/// literal: `concat!` only accepts literals, and the protocol freeze
/// test pins the string.
const USAGE: &str = concat!(
    "ramsleuth-tui — the live terminal dashboard (R/S/Q)\n",
    "\n",
    "Usage: ramsleuth-tui [OPTIONS]\n",
    "\n",
    "Options:\n",
    "  --socket <path>   Daemon Unix socket\n",
    "                    (default: /run/ramsleuth/ramsleuth.sock)\n",
    "\n",
    "Keys: [R]efresh  [S]napshot  [Q]uit\n",
    "Exit codes: 0 quit, 1 terminal init failure, 2 usage error\n",
);

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
/// ramsleuth-client P3-21 precedent.
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
            let value = iter
                .next()
                .map(|v| v.as_ref().to_owned())
                .ok_or_else(|| {
                    "--socket requires a value (the daemon's Unix socket path)".to_owned()
                })?;
            parsed.socket = PathBuf::from(value);
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag `{arg}` (expected `--socket <path>`)"));
        } else {
            return Err(format!(
                "unexpected argument `{arg}` (no positional arguments are accepted)"
            ));
        }
    }
    Ok(parsed)
}

/// One benchmark request from the key handler to the background
/// updater (the TUI-local mirror of the GUI `update.rs` `BenchCmd`):
/// the cell target + the scope + the run-class discriminator —
/// `None` = a normal single-pass benchmark ([`run_bench`]);
/// `Some(n)` = a burn-in with a duration of `n` minutes (its
/// `run_burn_in` worker lands in TUI-20).
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
            | Response::BurnInProgress(_),
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
        ramsleuth_tui::graphs::read_cpu_temp_c(),
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
    if let Some(event) = bench
        .progress
        .iter()
        .rev()
        .find(|e| e.tier == Tier::Memory && e.op == BenchOp::Read && e.value.is_finite() && e.value > 0.0)
    {
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
/// **Cancel (TUI-20):** `cancel` is the shared flag the Cancel key
/// sets (wired in TUI-20, along with the `CancelBenchmark` send on
/// the run's own connection — a pre-ack cancel only sets the flag).
/// It is reset to `false` at the start of every run (a stale cancel
/// never kills a new one) and checked before each `recv()`: once
/// set, the run is stopped daemon-side with a best-effort
/// `CancelBenchmark` for the current `run_id` (only once the
/// `BenchStarted` ack has landed — the daemon's reply is never read)
/// and the loop breaks with `running = false` (the clean stop, plan
/// D6: the in-flight pass finishes, the run ends at the next gate).
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
            let mut s = state.write().unwrap();
            s.bench.running = false;
            s.error = Some(error.to_string());
            return Ok(());
        }
    };
    // The stream read timeout (C7-16): the client's 5 s default would
    // kill a long run's frame gap mid-stream (the 120 s CLI
    // precedent).
    if let Err(error) = client.set_read_timeout(BENCH_READ_TIMEOUT) {
        let mut s = state.write().unwrap();
        s.bench.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    {
        // The run starts: `running = true`, the progress list cleared,
        // and a stale `run_id` dropped (a stale run's events never
        // mix into a new one) — one brief scope.
        let mut s = state.write().unwrap();
        s.bench.running = true;
        s.bench.progress.clear();
        // The new run's id arrives with the `BenchStarted` ack.
        s.bench.run_id = None;
    }
    if let Err(error) = client.send(&Request::StartBenchmark {
        target: cmd.target,
        mode: cmd.mode,
    }) {
        let mut s = state.write().unwrap();
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
            let run_id = state.read().unwrap().bench.run_id;
            if let Some(run_id) = run_id {
                let _ = client.send(&Request::CancelBenchmark { run_id });
            }
            state.write().unwrap().bench.running = false;
            break;
        }
        match client.recv() {
            Ok(Response::BenchStarted { run_id }) => {
                // The run's id lands (the Cancel key addresses the
                // run with it, TUI-20) — a brief scope.
                state.write().unwrap().bench.run_id = Some(run_id);
            }
            Ok(Response::BenchProgress(progress)) => {
                // One streamed progress event (latest last) — a
                // brief scope.
                state.write().unwrap().bench.progress.push(progress);
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal frame: the grid + the clean stop in
                // one brief scope.
                let mut s = state.write().unwrap();
                s.bench.grid = Some(grid);
                s.bench.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                // The clean-stop ack (the run ended at its cancel
                // gate) — a brief scope.
                state.write().unwrap().bench.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                // A structured daemon reply — including the
                // single-flight `Error` for a second start while a
                // run is in flight (P3-15/D-6): record + end, never
                // a panic (D5).
                let mut s = state.write().unwrap();
                s.error = Some(message);
                s.bench.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the bench stream violates the
                // wire contract (`GetTelemetry` is served on its own
                // connection, P3-16): stop the run and record it.
                let mut s = state.write().unwrap();
                s.error = Some("unexpected response during benchmark".to_owned());
                s.bench.running = false;
                break;
            }
            Ok(Response::BurnInProgress(_)) => {
                // A burn-in frame in a normal-bench stream violates
                // the wire contract (burn-in ticks stream only on the
                // owning `StartBurnIn` connection, D-1/D-2): stop
                // the run and record it (the GUI `run_bench` mirror).
                let mut s = state.write().unwrap();
                s.error = Some("unexpected burn-in frame during benchmark".to_owned());
                s.bench.running = false;
                break;
            }
            Err(error) => {
                let mut s = state.write().unwrap();
                s.error = Some(error.to_string());
                s.bench.running = false;
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
    /// Enable raw mode, enter the alternate screen, and create the
    /// ratatui terminal. Every partial failure unwinds what it already
    /// changed before returning the error — raw mode is never left on.
    fn new() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| format!("failed to enable raw mode: {e}"))?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(format!("failed to enter the alternate screen: {e}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(e) => {
                let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
                let _ = disable_raw_mode();
                return Err(format!("failed to initialize the terminal: {e}"));
            }
        };
        Ok(Self { terminal: Some(terminal) })
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
/// TUI-19: the loop first services at most one bench command from
/// `bench_rx` per tick (a queued command takes priority over this
/// tick's telemetry step, and a run streams to its terminal before the
/// next poll — the GUI poller's exact structure; telemetry polling
/// pauses for the run's duration; runs are single-flight daemon-side
/// anyway, P3-15), handing the shared `cancel` flag to [`run_bench`]
/// (each run resets it and checks it between frames).
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
                let app = state.read().unwrap();
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
                    if cmd.duration_minutes.is_some() {
                        // TUI-20: the burn-in worker — not wired until
                        // that chunk; a burn-in command landing before
                        // it is recorded, never a panic (D5) (the key
                        // that sends one — `[x]` — lands with
                        // TUI-20/22).
                        state.write().unwrap().error =
                            Some("burn-in is not available in this build".to_owned());
                    } else {
                        // A normal run: `run_bench` locks `state` only
                        // briefly, per mutation — never across its
                        // stream drain (C14-03: the render thread's
                        // reads stay responsive for the whole run).
                        let _ = run_bench(&socket, cmd, &state, &cancel);
                    }
                }
                Err(TryRecvError::Empty) => {
                    match Client::connect(&socket) {
                        Err(error) => {
                            // Daemon down: the friendly disconnected
                            // state (the no-panic contract, D5); the
                            // next tick re-probes.
                            let mut snapshot = state.write().unwrap();
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
                                poll_with_client(&mut client, &socket, &mut state.write().unwrap());
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
        let snapshot = state.read().unwrap();
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
            state.write().unwrap().daemon_status = format!("snapshot: {}", path.display());
        }
        Err(e) => {
            state.write().unwrap().error = Some(format!("snapshot failed: {e}"));
        }
    }
}

/// The terminal event loop (P3-24): initialize the terminal (the
/// [`TerminalGuard`] restores it on the way out of this function), spawn
/// the background updater, then tick — draw the dashboard, poll events
/// for 250 ms, dispatch the frozen `Action` table. Returns the process
/// exit code: `0` on a normal quit, `1` when the terminal could not be
/// initialized.
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
    // The shared cancel flag (TUI-19/20): each run resets it and
    // checks it between frames; the `[C]` key's wiring lands in
    // TUI-20.
    let cancel = Arc::new(AtomicBool::new(false));

    let updater = spawn_updater(
        Arc::clone(&state),
        Arc::clone(&stop),
        args.socket.clone(),
        bench_rx,
        Arc::clone(&cancel),
    );

    while let Some(terminal) = guard.terminal.as_mut() {
        // One draw per tick (250 ms cadence): the read lock is held only
        // for the frame render (pure over `&AppState`, P3-23).
        let draw_state = Arc::clone(&state);
        if let Err(message) = terminal.draw(|frame| {
            let snapshot = draw_state.read().unwrap();
            render(frame, &snapshot);
        }) {
            // A failed draw (e.g. the terminal was detached) is recorded,
            // not fatal: the loop keeps running, the status zone shows it.
            state.write().unwrap().error = Some(format!("terminal draw failed: {message}"));
        }

        match events::poll_event(POLL_TIMEOUT) {
            Ok(Some(Event::Key(key))) => {
                if let Some(action) = key_to_action(key) {
                    match action {
                        // `[R]`: force an immediate poll on the main
                        // thread (one fast RPC — bounded by the
                        // connect-retry window + the 5 s read timeout).
                        Action::Refresh => {
                            let _ = poll_once(&args.socket, &mut state.write().unwrap());
                        }
                        // `[S]`: write the timestamped text snapshot to
                        // the CWD + record its path in daemon_status.
                        Action::Snapshot => write_snapshot(&state),
                        // `[Q]`: stop the updater, break, restore the
                        // terminal (guard drop), exit 0.
                        Action::Quit => {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                        // `[b]`: a full benchmark run (TUI-19) — the
                        // command goes to the updater (a channel send,
                        // no I/O — the key handler's one permitted
                        // main-thread mutation). The send is skipped
                        // while a run is in flight (the GUI
                        // single-flight UX; the daemon would answer a
                        // second start with its structured `Error`
                        // anyway — TUI-22 refines the skip with a
                        // status note).
                        Action::BenchFull => {
                            if !state.read().unwrap().bench.running {
                                let _ = bench_tx.send(TuiBenchCmd {
                                    target: StreamTarget::Full,
                                    mode: BenchMode::Full,
                                    duration_minutes: None,
                                });
                            }
                        }
                        // `[m]`: a memory-only benchmark run (TUI-19) —
                        // the single-flight skip as above.
                        Action::BenchMemory => {
                            if !state.read().unwrap().bench.running {
                                let _ = bench_tx.send(TuiBenchCmd {
                                    target: StreamTarget::Full,
                                    mode: BenchMode::MemoryOnly,
                                    duration_minutes: None,
                                });
                            }
                        }
                        // TUI-20: wire the burn-in + cancel class;
                        // TUI-22: wire the view class
                        Action::BurnIn | Action::Cancel | Action::ToggleGraphs
                            | Action::ToggleSettings | Action::ToggleRequirements
                            | Action::ExportJson | Action::CyclePoll
                            | Action::ToggleCapacity | Action::ToggleClock
                            | Action::ToggleRefresh | Action::CycleWindow => {}
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
                state.write().unwrap().error = Some(format!("event stream: {message}"));
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
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("ramsleuth-tui: {message}");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    run(args)
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure / testable parts of P3-24 (no real
    //! terminal — the draw/poll loop needs a TTY and is verified in the
    //! QA phase): `parse_args` driven directly, and `poll_once` against
    //! an in-process daemon stand-in (a `std::thread` binding a
    //! `UnixListener` on a unique temp socket, the ramsleuth-client
    //! P3-18 precedent) plus a non-existent socket path.

    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process;
    use std::thread;
    use std::sync::atomic::AtomicUsize;

    use ramsleuth_bench::{BenchmarkGrid, StreamProgress};
    use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message};
    use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
    use ramsleuth_telemetry::amd_readout::map_amd;
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemMemoryTelemetry;
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// A unique temp socket path owned by a guard that removes the file
    /// on drop (best-effort cleanup; the path is pid-qualified so
    /// parallel test runs / processes never collide).
    struct TempSocket {
        path: PathBuf,
    }

    impl TempSocket {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(format!(
                "/tmp/ramsleuth-tui-{name}-{}.sock",
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

    /// A distinctive all-`Na` snapshot built through the telemetry
    /// crate's public API (host-independent — mirrors the daemon P3-14 /
    /// client P3-18 mocks).
    fn mock_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "TUI Test CPU".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
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
        }
    }

    /// A host-independent snapshot with finite AMD rails (the TUI-18
    /// record fixture — mirrors the GUI `update.rs`
    /// `populated_snapshot`): the `u16` voltages map in-band →
    /// `Value` (a present cell is always finite), the platform clock
    /// stays Na (the freq series keeps its no-source note — the
    /// sample lands via the rails). One such poll appends a graph
    /// sample on every host, no temp scan required.
    fn populated_snapshot() -> SystemMemoryTelemetry {
        let readout = map_amd(&AmdPmSnapshot {
            version: 0x0007_0B02,
            mclk_mhz: 1800,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 16,
                rcwdwr: 16,
                rcdrd: 16,
                rp: 16,
                ras: 32,
                rc: 48,
                rrds: 4,
                rrld: 4,
                faw: 16,
                wtrs: 8,
                wtrl: 8,
                wr: 8,
                rfc1: 160,
                rfc2: 160,
                rfcsb: 160,
                cwl: 16,
                rtp: 8,
                rdwr: 8,
                wrrd: 4,
                rdrd_sd: 100,
                rdrd_dd: 101,
                rdrd_scl: 102,
                rdrd_sc: 103,
                wrwr_sd: 104,
                wrwr_dd: 105,
                wrwr_scl: 106,
                wrwr_sc: 107,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 5,
                rtt_nom: 2,
                rtt_wr: 0,
                rtt_park: 4,
                clk_drv: 6,
                addr_cmd_drv: 8,
                cs_odt_drv: 10,
                cke_drv: 12,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1050,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
                vcore_mv: 1150,
            },
        });
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(readout),
            intel: Section::na(NaReason::NotApplicable),
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
        }
    }

    /// Server-side incremental frame reader (the P3-11 contract on the
    /// other end): append every received byte and decode until one frame
    /// is complete (`None` on a clean EOF before a frame).
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

    /// One in-process daemon stand-in: binds the temp socket, accepts
    /// exactly one connection in a spawned thread, and hands it to
    /// `handler`. `join` reaps the thread (a handler panic fails the
    /// test instead of hanging it).
    struct DaemonStandIn {
        handle: thread::JoinHandle<()>,
    }

    impl DaemonStandIn {
        fn spawn(sock: &TempSocket, handler: impl FnOnce(UnixStream) + Send + 'static) -> Self {
            let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
            let handle = thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    handler(stream);
                }
            });
            Self { handle }
        }

        /// A multi-connection stand-in (the GUI `update.rs`
        /// precedent): binds `sock` and accepts up to `max_conns`
        /// connections in a spawned thread (one per poll — the
        /// fresh-connection-per-cycle contract), handing each to
        /// `handler` with its 1-based index. `join` reaps the thread
        /// (a handler panic fails the test instead of hanging it).
        fn spawn_multi(
            sock: &TempSocket,
            max_conns: usize,
            mut handler: impl FnMut(usize, UnixStream) + Send + 'static,
        ) -> Self {
            let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
            let handle = thread::spawn(move || {
                for index in 1..=max_conns {
                    if let Ok((stream, _)) = listener.accept() {
                        handler(index, stream);
                    }
                }
            });
            Self { handle }
        }

        fn join(self) {
            self.handle.join().expect("stand-in thread must not panic");
        }
    }

    // --- parse_args -------------------------------------------------------

    /// (a) No args → the default socket (the protocol's frozen
    /// `DEFAULT_SOCKET_PATH`).
    #[test]
    fn no_args_yields_default_socket() {
        let args = parse_args(Vec::<&str>::new()).expect("no args must parse");
        assert_eq!(args, TuiArgs::default());
        assert_eq!(args.socket, PathBuf::from(DEFAULT_SOCKET_PATH));
    }

    /// (b) `--socket /tmp/x` → the overridden socket path.
    #[test]
    fn socket_override() {
        let args = parse_args(["--socket", "/tmp/x"]).expect("must parse");
        assert_eq!(args.socket, PathBuf::from("/tmp/x"));
    }

    /// (c) An unknown flag is rejected with its name.
    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse_args(["--bogus"]).expect_err("an unknown flag must be rejected");
        assert!(err.contains("--bogus"), "the error must name the flag: {err}");
    }

    /// (d) A positional argument is rejected with its name (the TUI
    /// takes no subcommand).
    #[test]
    fn positional_is_rejected() {
        let err = parse_args(["dump"]).expect_err("a positional must be rejected");
        assert!(err.contains("dump"), "the error must name the argument: {err}");
    }

    /// (e) `--socket` with a missing value is rejected with a message
    /// naming the flag.
    #[test]
    fn missing_socket_value_is_rejected() {
        let err =
            parse_args(["--socket"]).expect_err("`--socket` without a value must be rejected");
        assert!(err.contains("--socket"), "the error must name the flag: {err}");
    }

    /// (f) A repeated `--socket` — the last value wins.
    #[test]
    fn repeated_socket_last_wins() {
        let args = parse_args(["--socket", "/tmp/a", "--socket", "/tmp/b"]).expect("must parse");
        assert_eq!(args.socket, PathBuf::from("/tmp/b"));
    }

    /// (g) `String` iterators parse exactly like `&str` ones (the
    /// generic bound is exercised with owned values, as `env::args`
    /// yields — the ramsleuth-client P3-21 test precedent).
    #[test]
    fn owned_string_args_parse() {
        let args: Vec<String> = ["--socket", "/tmp/y.sock"].iter().copied().map(str::to_owned).collect();
        let parsed = parse_args(args).expect("owned String args must parse");
        assert_eq!(parsed.socket, PathBuf::from("/tmp/y.sock"));
    }

    // --- poll_once --------------------------------------------------------

    /// (b) `poll_once` against an in-process daemon stand-in (a `GetTelemetry`
    /// request answered with a canned `Response::Telemetry`) fills the
    /// state: `telemetry` becomes `Some` (the stand-in's snapshot), the
    /// pre-existing error is cleared, `daemon_status` reports
    /// `connected`, and `last_update` is stamped.
    #[test]
    fn poll_once_against_live_stand_in_updates_state() {
        let sock = TempSocket::new("poll-live");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
                    .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        // A pre-existing error (initialized here, not reassigned after)
        // must be cleared by a successful poll.
        let mut state =
            AppState { error: Some("stale error".to_owned()), ..Default::default() };
        poll_once(sock.path(), &mut state).expect("poll_once must not error");

        assert_eq!(
            state.telemetry.as_ref().map(|t| t.cpu.brand.as_str()),
            Some("TUI Test CPU"),
            "the stand-in's canned snapshot must land in the state"
        );
        assert!(state.error.is_none(), "a successful poll must clear the error");
        let status = &state.daemon_status;
        assert!(
            status.contains("connected"),
            "the status must report the connection, got: {status}"
        );
        assert!(state.last_update.is_some(), "a successful poll must stamp last_update");
        stand_in.join();
    }

    /// (c) `poll_once` against a **non-existent** socket path records a
    /// friendly error (the `DaemonDown` "start it with …" hint) + the
    /// `disconnected` status, and never panics (the no-panic contract,
    /// plan D5).
    #[test]
    fn poll_once_against_missing_socket_is_friendly() {
        let sock = TempSocket::new("poll-missing");
        let mut state = AppState::default();

        poll_once(sock.path(), &mut state).expect("poll_once must not error");

        assert!(state.telemetry.is_none(), "no telemetry without a daemon");
        assert_eq!(state.daemon_status, "disconnected");
        let error = state.error.expect("a friendly error must be recorded");
        assert!(
            error.contains("daemon not running"),
            "the error must carry the DaemonDown hint, got: {error}"
        );
    }

    // --- TUI-17: the poller rework (live interval + refresh gating) ---

    /// (h) The clamp (the TUI-local mirror of the GUI `update.rs`
    /// `clamp_poll_interval`): below the floor → 100 ms; inside the
    /// range → unchanged; above the ceiling → 60 000 ms.
    #[test]
    fn clamp_poll_interval_arms() {
        assert_eq!(clamp_poll_interval(0), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(50), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(100), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(2_000), Duration::from_millis(2_000));
        assert_eq!(clamp_poll_interval(30_000), Duration::from_millis(30_000));
        assert_eq!(clamp_poll_interval(60_000), Duration::from_millis(60_000));
        assert_eq!(clamp_poll_interval(90_000), Duration::from_millis(60_000));
        assert_eq!(clamp_poll_interval(u64::MAX), Duration::from_millis(60_000));
    }

    /// A persistent counting stand-in: binds `sock` (removing any stale
    /// file first — the kill/restart phases re-bind the same path),
    /// accepts until `stop`, and increments `count` per accepted
    /// connection. A served `GetTelemetry` gets a canned snapshot; a
    /// clean EOF before a frame (the poller's no-fetch probe connect —
    /// refresh off, steady state) is tolerated and the loop keeps
    /// accepting. The stop-waker's throwaway connect is never counted
    /// (it is sent only after `stop` is set, and both stop checks pass
    /// before a connection is counted).
    fn tolerant_counting_stand_in(
        sock: &TempSocket,
        stop: &Arc<AtomicBool>,
        count: &Arc<AtomicUsize>,
    ) -> thread::JoinHandle<()> {
        let _ = std::fs::remove_file(sock.path()); // stale file (re-bind phase)
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        let stop = Arc::clone(stop);
        let count = Arc::clone(count);
        thread::spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                if stop.load(Ordering::Relaxed) {
                    break; // the waker (stop was set first)
                }
                count.fetch_add(1, Ordering::Relaxed);
                match read_one_message(&mut stream) {
                    Some(Message::Request(Request::GetTelemetry)) => {}
                    Some(other) => panic!("stand-in expected GetTelemetry, got {other:?}"),
                    None => continue, // a no-fetch probe connect (EOF)
                }
                let bytes =
                    encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
                        .expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        })
    }

    /// Stop a stand-in cleanly: set its `stop` flag, wake a blocked
    /// `accept` with a throwaway connect (immediate EOF; a failed
    /// connect means the stand-in already left), and join it.
    fn stop_stand_in(handle: thread::JoinHandle<()>, stop: &AtomicBool, sock: &Path) {
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock);
        handle.join().expect("stand-in thread must not panic");
    }

    /// Wait (bounded by a 10 s deadline) until `daemon_status` contains
    /// `needle`; the test fails with the current status instead of
    /// hanging (the GUI `update.rs` precedent).
    fn wait_for_status(state: &Arc<RwLock<AppState>>, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status =
                state.read().expect("the updater must not poison the lock").daemon_status.clone();
            if status.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for a status containing {needle:?}, got: {status}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// (i) Refresh gating (the C7-08/09 mechanism): with `refresh` off,
    /// the updater performs exactly one baseline fetch at startup (one
    /// connection), none while the daemon is down (several ticks pass —
    /// the probes fail against the dead socket, no accepted
    /// connections), and one reconnect baseline when the daemon comes
    /// back (one connection — the probe connect is reused for the
    /// fetch). The snapshot stays frozen (no periodic data poll).
    #[test]
    fn refresh_off_runs_only_the_startup_and_reconnect_baselines() {
        let sock = TempSocket::new("refresh-off");
        let count = Arc::new(AtomicUsize::new(0));

        // Phase A: the daemon is up; the startup baseline fires (one
        // connection). Refresh off, a 200 ms interval.
        let stop_a = Arc::new(AtomicBool::new(false));
        let stand_in_a = tolerant_counting_stand_in(&sock, &stop_a, &count);
        let mut app = AppState::default();
        app.settings.poll_interval_ms = 200;
        app.settings.refresh = false;
        let state = Arc::new(RwLock::new(app));
        let updater_stop = Arc::new(AtomicBool::new(false));
        // TUI-19: the new `spawn_updater` parameters — a fresh bench
        // command channel (no commands are sent in this test; the
        // sender stays alive for the test's duration) + the shared
        // cancel flag.
        let (_bench_tx, bench_rx) = mpsc::channel::<TuiBenchCmd>();
        let bench_cancel = Arc::new(AtomicBool::new(false));
        let updater = spawn_updater(
            Arc::clone(&state),
            Arc::clone(&updater_stop),
            sock.path().to_path_buf(),
            bench_rx,
            Arc::clone(&bench_cancel),
        );

        // Exactly one connection for the startup baseline (checked
        // immediately — the next probe tick is ~200 ms away). The
        // needle keeps the `:` so a `disconnected` status never
        // matches as a substring.
        wait_for_status(&state, "connected:");
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "exactly one startup baseline connection (got {})",
            count.load(Ordering::Relaxed)
        );

        // Phase B: the daemon goes down; several ticks pass with no
        // accepted connections (the probes fail against the dead socket
        // — no listener).
        stop_stand_in(stand_in_a, &stop_a, sock.path());
        let _ = std::fs::remove_file(sock.path()); // no listener, no file
        thread::sleep(Duration::from_millis(800)); // several 200 ms ticks
        wait_for_status(&state, "disconnected");
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "no accepted connections while the daemon is down (got {})",
            count.load(Ordering::Relaxed)
        );

        // Phase C: the daemon comes back; exactly one reconnect baseline
        // (the probe connect is reused for the fetch — one connection).
        let stop_c = Arc::new(AtomicBool::new(false));
        let stand_in_c = tolerant_counting_stand_in(&sock, &stop_c, &count);
        wait_for_status(&state, "connected:");
        assert_eq!(
            count.load(Ordering::Relaxed),
            2,
            "exactly one reconnect baseline connection (got {})",
            count.load(Ordering::Relaxed)
        );

        // Teardown: stop the updater + the stand-in.
        updater_stop.store(true, Ordering::Relaxed);
        stop_stand_in(stand_in_c, &stop_c, sock.path());
        updater.join().expect("updater thread must not panic");
    }

    /// (j) The live interval (C6-27): with `refresh` on, the poller
    /// sleeps the live clamped value — changing `poll_interval_ms` at
    /// runtime (no restart) changes the cadence on the next tick. Over
    /// the same wall window, the shorter interval produces more
    /// connections than the longer one.
    #[test]
    fn live_poll_interval_change_takes_effect_without_restart() {
        let sock = TempSocket::new("live-interval");
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let stand_in = tolerant_counting_stand_in(&sock, &stop, &count);

        // Refresh on, a long 400 ms interval.
        let mut app = AppState::default();
        app.settings.poll_interval_ms = 400;
        app.settings.refresh = true;
        let state = Arc::new(RwLock::new(app));
        let updater_stop = Arc::new(AtomicBool::new(false));
        // TUI-19: the new `spawn_updater` parameters — a fresh bench
        // command channel (no commands are sent in this test; the
        // sender stays alive for the test's duration) + the shared
        // cancel flag.
        let (_bench_tx, bench_rx) = mpsc::channel::<TuiBenchCmd>();
        let bench_cancel = Arc::new(AtomicBool::new(false));
        let updater = spawn_updater(
            Arc::clone(&state),
            Arc::clone(&updater_stop),
            sock.path().to_path_buf(),
            bench_rx,
            Arc::clone(&bench_cancel),
        );

        // Phase A: let the 400 ms cadence run over a 1 s window.
        wait_for_status(&state, "connected:");
        thread::sleep(Duration::from_millis(200)); // let the baseline settle
        let a0 = count.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(1_000));
        let slow_delta = count.load(Ordering::Relaxed) - a0;

        // Phase B: the panel shrinks the interval to the 100 ms floor;
        // the same 1 s window yields more polls (no restart).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .poll_interval_ms = 100;
        let b0 = count.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(1_000));
        let fast_delta = count.load(Ordering::Relaxed) - b0;

        assert!(
            fast_delta > slow_delta,
            "the 100 ms interval must poll more often than 400 ms over the same window (slow={slow_delta}, fast={fast_delta})"
        );

        // Teardown.
        updater_stop.store(true, Ordering::Relaxed);
        stop_stand_in(stand_in, &stop, sock.path());
        updater.join().expect("updater thread must not panic");
    }

    // --- TUI-18: per-poll graph sample recording + ring clear ---

    /// (k) Two continuous successful polls append two Na-guarded
    /// graph samples (one per poll — the poller stays the only
    /// writer, D6; no clear — the status stays `connected`): the
    /// fixture's finite AMD rails land on every sample, and the
    /// terminal grid's memory-read cell (row 0) feeds the bandwidth
    /// series (D-5).
    #[test]
    fn two_continuous_polls_record_two_graph_samples() {
        let sock = TempSocket::new("graph-two");
        let stand_in = DaemonStandIn::spawn_multi(&sock, 2, |_index, mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(populated_snapshot())))
                    .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        let mut state = AppState::default();
        // A terminal grid from an earlier run: the memory-read cell
        // (row 0) feeds the bandwidth series (D-5).
        state.bench.grid = Some(BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        });
        poll_once(sock.path(), &mut state).expect("first poll must not error");
        poll_once(sock.path(), &mut state).expect("second poll must not error");
        stand_in.join();

        assert_eq!(
            state.graph.len(),
            2,
            "two continuous successful polls append two samples (no clear)"
        );
        for sample in state.graph.samples.iter() {
            assert!(
                sample.t.is_finite() && sample.t > 0.0,
                "the sample is stamped with unix seconds"
            );
            assert_eq!(
                sample.vddcr_soc_mv,
                1050.0,
                "the fixture's SOC rail lands in mV on every sample"
            );
            assert_eq!(
                sample.vddcr_cpu_mv,
                1150.0,
                "the fixture's vcore rail lands in mV on every sample"
            );
            assert!(
                sample.cpu_freq_mhz.is_nan(),
                "the all-Na platform keeps the freq series no-source (D-4)"
            );
            assert_eq!(
                sample.bandwidth_gbps,
                26.35,
                "the bandwidth is the terminal grid's memory-read cell (D-5)"
            );
        }
    }

    /// (l) An all-Na poll appends no hole (the no-panic contract,
    /// D5): the all-Na snapshot carries no finite field, so the only
    /// possible finite source is this host's temp scan (host-
    /// dependent — the graphs.rs (g) precedent: NaN on a sensor-less
    /// host, a sane reading here). With the scan finite, exactly one
    /// temp-only sample lands (every snapshot-derived field NaN, no
    /// fake 0.0); with the scan NaN, the ring stays empty.
    #[test]
    fn all_na_poll_records_no_hole() {
        let sock = TempSocket::new("graph-allna");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
                    .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        let mut state = AppState::default();
        poll_once(sock.path(), &mut state).expect("poll_once must not error");
        stand_in.join();

        // This host's temp scan (the same I/O the poll ran — the
        // sensor state is stable across the two reads): it decides
        // the sample's shape.
        let temp = ramsleuth_tui::graphs::read_cpu_temp_c();
        if temp.is_finite() {
            assert_eq!(
                state.graph.len(),
                1,
                "a finite temp scan lands exactly one sample"
            );
            let sample = state.graph.samples.last().expect("the sample");
            assert!(
                sample.cpu_freq_mhz.is_nan(),
                "an Na clock stays NaN, not a fake 0.0"
            );
            assert!(sample.vddcr_cpu_mv.is_nan(), "an Na vcore stays NaN");
            assert!(sample.vddcr_soc_mv.is_nan(), "an Na SOC rail stays NaN");
            assert!(
                sample.cpu_temp_c.is_finite()
                    && sample.cpu_temp_c > -50.0
                    && sample.cpu_temp_c < 150.0,
                "the sample's only finite field is a sane temp reading"
            );
            assert!(
                sample.bandwidth_gbps.is_nan(),
                "no bench data: the no-figure sentinel maps to NaN (0 ≠ N/A)"
            );
        } else {
            assert!(
                state.graph.is_empty(),
                "an all-NaN sample is a hole, never a point"
            );
        }
    }

    /// (m) Reconnect prime (the GUI C6-25 precedent, the TUI-18
    /// plan): two continuous successful polls append two samples
    /// (no clear — the status stays `connected`); the daemon drop
    /// records `disconnected` (a failed poll appends nothing,
    /// clears nothing); the next successful poll — the
    /// disconnected→connected transition — clears the stale ring
    /// before appending one fresh sample.
    #[test]
    fn graph_ring_clears_on_reconnect_not_on_continuous_polls() {
        // Phase 1: two continuous successful polls (a fresh
        // connection each) → two samples, no clear.
        let sock = TempSocket::new("graph-reconnect");
        let stand_in = DaemonStandIn::spawn_multi(&sock, 2, |_index, mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(populated_snapshot())))
                    .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        let mut state = AppState::default();
        poll_once(sock.path(), &mut state).expect("first poll must not error");
        poll_once(sock.path(), &mut state).expect("second poll must not error");
        stand_in.join();
        assert_eq!(
            state.graph.len(),
            2,
            "two continuous polls append two samples (no clear)"
        );

        // Phase 2: the daemon drops (a missing socket records
        // `disconnected`) — a failed poll appends nothing, clears
        // nothing.
        let down_sock = TempSocket::new("graph-reconnect-down");
        poll_once(down_sock.path(), &mut state).expect("the down poll must not error");
        assert_eq!(state.daemon_status, "disconnected");
        assert_eq!(
            state.graph.len(),
            2,
            "a failed poll appends nothing (no clear)"
        );

        // Phase 3: the daemon comes back → the stale ring is cleared
        // and the reconnect poll appends exactly one fresh sample.
        let back_sock = TempSocket::new("graph-reconnect-back");
        let stand_in = DaemonStandIn::spawn(&back_sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(populated_snapshot())))
                    .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        poll_once(back_sock.path(), &mut state).expect("the reconnect poll must not error");
        stand_in.join();
        assert_eq!(
            state.graph.len(),
            1,
            "the reconnect clears the stale ring before the fresh sample"
        );
        assert_eq!(
            state.daemon_status,
            format!("connected: {}", back_sock.path().display()),
            "the reconnect reports the new connection"
        );
    }

    /// (n) The bandwidth source priority (the GUI `update.rs`
    /// `latest_memory_read_bw` 4-case matrix, the TUI-local mirror
    /// over the frozen [`BenchState`]): (1) no progress, no grid →
    /// the `0.0` no-figure sentinel (mapped to NaN at the record
    /// site — the row keeps its no-source note, D-4: 0 ≠ N/A);
    /// (2) the terminal grid's memory-read cell (row 0); (3) the
    /// newest streamed `Memory · Read` event beats the (older)
    /// grid, a newer non-Memory / non-Read event is ignored; (4) a
    /// non-finite `Memory · Read` + a `Memory · Write` event are
    /// both ignored → the figure falls back to the grid.
    #[test]
    fn latest_memory_read_bw_source_priority() {
        let grid = BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        // (1) Neither: no progress, no grid → the no-figure sentinel.
        let idle = BenchState::default();
        assert_eq!(latest_memory_read_bw(&idle), 0.0, "no bench data at all → 0.0");

        // (2) Terminal grid only (row 0 = the Memory read cell).
        let grid_only = BenchState { grid: Some(grid.clone()), ..Default::default() };
        assert_eq!(latest_memory_read_bw(&grid_only), 26.35, "the terminal grid cell");

        // (3) A live `Memory · Read` event beats the (older) terminal
        // grid; a newer non-Memory / non-Read event is ignored.
        let live = BenchState {
            progress: vec![
                StreamProgress {
                    cell_index: 1,
                    total_cells: 3,
                    tier: Tier::L1,
                    op: BenchOp::Read,
                    value: 99.0,
                    label: "L1 · Read (GB/s)".to_owned(),
                },
                StreamProgress {
                    cell_index: 0,
                    total_cells: 3,
                    tier: Tier::Memory,
                    op: BenchOp::Read,
                    value: 42.0,
                    label: "Memory · Read (GB/s)".to_owned(),
                },
            ],
            grid: Some(grid.clone()),
            ..Default::default()
        };
        assert_eq!(
            latest_memory_read_bw(&live),
            42.0,
            "the newest Memory · Read event wins over the grid"
        );

        // (4) A non-finite `Memory · Read` value + a `Memory · Write`
        // event are both ignored: the figure falls back to the grid.
        let bad_live = BenchState {
            progress: vec![
                StreamProgress {
                    cell_index: 0,
                    total_cells: 3,
                    tier: Tier::Memory,
                    op: BenchOp::Read,
                    value: f64::NAN,
                    label: "Memory · Read (GB/s)".to_owned(),
                },
                StreamProgress {
                    cell_index: 0,
                    total_cells: 3,
                    tier: Tier::Memory,
                    op: BenchOp::Write,
                    value: 42.0,
                    label: "Memory · Write (GB/s)".to_owned(),
                },
            ],
            grid: Some(grid),
            ..Default::default()
        };
        assert_eq!(
            latest_memory_read_bw(&bad_live),
            26.35,
            "a NaN read event + a write event fall back to the grid"
        );
    }
    // ------------------------------------------------------------------
    // TUI-19: the bench worker (normal runs) — `run_bench` against an
    // in-process daemon stand-in (the GUI `update.rs` `run_bench`
    // stand-in suite, adapted to the TUI state / command types).
    // ------------------------------------------------------------------

    /// (o) `run_bench` against a stand-in (a `StartBenchmark` answered
    /// with `BenchStarted` + two `BenchProgress` + the terminal
    /// `BenchResult`) streams into `state.bench`: the cmd's target /
    /// mode ride the wire, exactly the two progress events (a stale
    /// pre-run entry is cleared at run start), the ack's `run_id` is
    /// recorded, the terminal grid lands, `running = false` on the
    /// terminal — with no transport error.
    #[test]
    fn run_bench_streams_started_progress_result() {
        let sock = TempSocket::new("bench");
        let progress_a = StreamProgress {
            cell_index: 0,
            total_cells: 3,
            tier: Tier::Memory,
            op: BenchOp::Read,
            value: 26.35,
            label: "Memory · Read (GB/s)".to_owned(),
        };
        let progress_b = StreamProgress {
            cell_index: 1,
            total_cells: 3,
            tier: Tier::Memory,
            op: BenchOp::Write,
            value: 43.63,
            label: "Memory · Write (GB/s)".to_owned(),
        };
        let grid = BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [43.63, 0.0, 0.0, 0.0],
            copy_gbps: [12.11, 0.0, 0.0, 0.0],
            latency_ns: [86.84, 0.0, 0.0, 0.0],
        };
        let expected_progress = vec![progress_a.clone(), progress_b.clone()];
        let expected_grid = grid.clone();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBenchmark { target, mode })) => {
                    assert_eq!(
                        target,
                        StreamTarget::Full,
                        "the cmd's target must ride the wire"
                    );
                    assert_eq!(mode, BenchMode::Full, "the cmd's mode must ride the wire");
                }
                other => panic!("stand-in expected StartBenchmark, got {other:?}"),
            }
            for response in [
                Response::BenchStarted { run_id: 1 },
                Response::BenchProgress(progress_a),
                Response::BenchProgress(progress_b),
                Response::BenchResult { run_id: 1, grid },
            ] {
                let bytes = encode_frame(&Message::Response(response)).expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        });

        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: None,
        };
        // A false cancel flag: this run is never cancelled (it is
        // reset per run anyway).
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        {
            let mut s = state.write().expect("the updater must not poison the lock");
            // A stale pre-run progress entry must be cleared at run
            // start.
            s.bench.progress.push(StreamProgress {
                cell_index: 9,
                total_cells: 12,
                tier: Tier::L3,
                op: BenchOp::Copy,
                value: 1.0,
                label: "stale".to_owned(),
            });
            // A stale pre-run id must be dropped at run start.
            s.bench.run_id = Some(99);
        }
        run_bench(sock.path(), cmd, &state, &cancel).expect("run_bench must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.running, "the terminal result must clear running");
        assert_eq!(state.bench.run_id, Some(1), "the ack's run_id must be recorded");
        assert_eq!(
            state.bench.progress, expected_progress,
            "exactly the streamed events; the stale entry is cleared"
        );
        assert_eq!(
            state.bench.grid,
            Some(expected_grid),
            "the terminal grid must land in the state"
        );
        assert!(state.error.is_none(), "a clean run must not record an error");
        stand_in.join();
    }

    /// (p) CANCEL (the shared flag the `[C]` key sets — its wiring
    /// lands in TUI-20; `run_bench` already checks it between frames,
    /// as in the GUI P3-28): after the `BenchStarted` ack lands in
    /// the state, the shared `cancel` flag is set — `run_bench` ends
    /// the run with `running = false` (sending the daemon a
    /// best-effort `CancelBenchmark` when the run has an id) — no
    /// hang, no panic, no error recorded.
    #[test]
    fn run_bench_cancel_flag_stops_the_run() {
        let sock = TempSocket::new("bench-cancel");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBenchmark { .. })) => {}
                other => panic!("stand-in expected StartBenchmark, got {other:?}"),
            }
            let started =
                encode_frame(&Message::Response(Response::BenchStarted { run_id: 42 }))
                    .expect("must encode");
            stream.write_all(&started).expect("stand-in write must not fail");
            // Mimic the daemon: end the run after a beat — the worker
            // either sees the cancel flag before the terminal frame
            // (sends a `CancelBenchmark` and breaks) or receives the
            // terminal `BenchCancelled` while blocked in `recv`.
            thread::sleep(Duration::from_millis(200));
            let _ = stream.write_all(
                &encode_frame(&Message::Response(Response::BenchCancelled { run_id: 42 }))
                    .expect("must encode"),
            );
            let _ = read_one_message(&mut stream); // drain the cancel (best-effort)
        });

        let socket = sock.path().to_path_buf();
        let state = Arc::new(RwLock::new(AppState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = TuiBenchCmd {
                    target: StreamTarget::Full,
                    mode: BenchMode::Full,
                    duration_minutes: None,
                };
                // The function locks the shared state only per
                // mutation — never across the stream drain (C14-03).
                run_bench(&socket, cmd, &state, &cancel)
                    .expect("run_bench must not error");
            })
        };

        // Wait for the run to start (the ack's run_id lands in the
        // state), then set the shared cancel flag.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if state.read().expect("the updater must not poison the lock").bench.run_id.is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the run never started (no BenchStarted within 5 s)"
            );
            thread::sleep(Duration::from_millis(10));
        }
        cancel.store(true, Ordering::Relaxed);
        worker.join().expect("run_bench must return on cancel (no hang)");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.running, "the cancel must clear running");
        assert_eq!(state.bench.run_id, Some(42), "the run id must stay recorded");
        assert!(state.error.is_none(), "a clean cancel must not record an error");
        stand_in.join();
    }

    /// (q) A structured daemon `Error` in the bench stream — including
    /// the single-flight `Error` for a second start while a run is in
    /// flight (P3-15/D-6) — is recorded in `state.error` and ends the
    /// run (`running = false`) — never a panic (the no-panic contract,
    /// D5); an errored run lands no terminal grid.
    #[test]
    fn run_bench_records_structured_error() {
        let sock = TempSocket::new("bench-error");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBenchmark { .. })) => {}
                other => panic!("stand-in expected StartBenchmark, got {other:?}"),
            }
            // The daemon's single-flight reply for a second start
            // while a run is active: a structured `Error`.
            let bytes = encode_frame(&Message::Response(Response::Error(
                "a benchmark is already running".to_owned(),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        run_bench(sock.path(), cmd, &state, &cancel).expect("run_bench must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.running, "a structured error must end the run");
        assert_eq!(
            state.error.as_deref(),
            Some("a benchmark is already running"),
            "the daemon's structured message must be recorded"
        );
        assert!(state.bench.grid.is_none(), "an errored run lands no terminal grid");
        stand_in.join();
    }

    /// (r) `run_bench` against a **non-existent** socket records the
    /// friendly `DaemonDown` text, leaves `running = false`, and never
    /// panics (the no-panic contract, D5) — the run's connect failure
    /// is a transport failure reported through the state.
    #[test]
    fn run_bench_missing_socket_is_friendly() {
        let sock = TempSocket::new("bench-missing");
        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        run_bench(sock.path(), cmd, &state, &cancel).expect("run_bench must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.running, "a failed connect must not leave a run in flight");
        assert!(state.bench.progress.is_empty(), "no progress without a run");
        let error = state.error.clone().expect("a friendly error must be recorded");
        assert!(
            error.contains("daemon not running"),
            "the error must carry the DaemonDown hint, got: {error}"
        );
    }

    /// (s) The bench stream read timeout is the raised 120 s value
    /// (the GUI C7-16 / the CLI precedent): a long frame gap can no
    /// longer trip the client's 5 s transport default mid-stream — a
    /// wedged daemon is bounded by the 120 s deadline, not a hang.
    #[test]
    fn bench_stream_read_timeout_is_the_raised_cli_value() {
        assert_eq!(
            BENCH_READ_TIMEOUT,
            Duration::from_secs(120),
            "the stream read timeout must be the 120 s CLI precedent"
        );
    }

    /// (t) C14-03: a long, in-flight bench run keeps the shared
    /// state's lock brief for readers. The stand-in streams
    /// `BenchStarted`, then 15 steady `BenchProgress` ticks at 100 ms
    /// intervals, then the terminal grid — the stream stays open for
    /// the whole run, so under a pre-fix lock scope (the write guard
    /// held across the whole drain) every `state.read()` on this
    /// thread would park for the entire run: here, every read taken
    /// every ~10 ms must complete promptly (no blocking), the mid-run
    /// reads must observe the live progress growing, and the run must
    /// end at its terminal (the grid recorded, no error).
    #[test]
    fn run_in_flight_bench_keeps_the_lock_brief_for_readers() {
        let sock = TempSocket::new("bench-inflight");
        let grid = BenchmarkGrid {
            read_gbps: [10.5, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        let expected_grid = grid.clone();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBenchmark { .. })) => {}
                other => panic!("stand-in expected StartBenchmark, got {other:?}"),
            }
            let started =
                encode_frame(&Message::Response(Response::BenchStarted { run_id: 7 }))
                    .expect("must encode");
            stream.write_all(&started).expect("stand-in write must not fail");
            for cell_index in 0u32..15 {
                let tick = encode_frame(&Message::Response(Response::BenchProgress(
                    StreamProgress {
                        cell_index,
                        total_cells: 15,
                        tier: Tier::Memory,
                        op: BenchOp::Read,
                        value: f64::from(cell_index + 1),
                        label: "Memory · Read (GB/s)".to_owned(),
                    },
                )))
                .expect("must encode");
                stream.write_all(&tick).expect("stand-in write must not fail");
                thread::sleep(Duration::from_millis(100));
            }
            let result = encode_frame(&Message::Response(Response::BenchResult {
                run_id: 7,
                grid,
            }))
            .expect("must encode");
            stream.write_all(&result).expect("stand-in write must not fail");
        });

        let state = Arc::new(RwLock::new(AppState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let socket = sock.path().to_path_buf();
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = TuiBenchCmd {
                    target: StreamTarget::Full,
                    mode: BenchMode::Full,
                    duration_minutes: None,
                };
                run_bench(&socket, cmd, &state, &cancel)
                    .expect("run_bench must not error");
            })
        };

        // The main test thread (a stand-in for the render thread):
        // read continuously every ~10 ms during the run — every read
        // must complete promptly (the writer's critical section is
        // per-frame, µs — C14-03), and the mid-run reads must observe
        // the live progress.
        let mut max_mid_run_progress = 0usize;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let read_started = Instant::now();
            let (running, progress_len, done) = {
                let s = state.read().expect("the updater must not poison the lock");
                (
                    s.bench.running,
                    s.bench.progress.len(),
                    s.bench.grid.is_some() && !s.bench.running,
                )
            };
            let read_elapsed = read_started.elapsed();
            assert!(
                read_elapsed < Duration::from_millis(50),
                "a mid-run read must not block on the writer (took {read_elapsed:?})"
            );
            if running {
                max_mid_run_progress = max_mid_run_progress.max(progress_len);
            }
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the in-flight bench never reached its terminal"
            );
            thread::sleep(Duration::from_millis(10));
        }
        worker.join().expect("run_bench must return at its terminal (no hang)");
        stand_in.join();

        // The terminal assertions take one brief, fully-scoped read
        // (the guard must not outlive this block — a long-lived
        // reader starves the next run's writer, C14-03).
        assert!(
            max_mid_run_progress >= 2,
            "a mid-run read must observe at least two streamed progress events, saw {max_mid_run_progress}"
        );
        {
            let s = state.read().expect("the updater must not poison the lock");
            assert!(!s.bench.running, "the terminal must clear running");
            assert_eq!(s.bench.run_id, Some(7), "the ack's run id must be recorded");
            assert_eq!(
                s.bench.grid,
                Some(expected_grid),
                "the terminal grid must land in the state"
            );
            assert!(s.error.is_none(), "a clean run must not record an error");
        }
    }
}
