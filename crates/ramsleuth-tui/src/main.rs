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
//! - **Main loop** — each tick: the requirements strip's
//!   auto-open-until-closed rule (TUI-22: while `diagnose` reports a
//!   requirement and the `[d]` key has not dismissed it, the
//!   `requirements_open` flag is force-true — a new requirement
//!   re-opens the strip on its own; a dismissed strip stays closed
//!   until the user toggles it open again), then
//!   `terminal.draw(render)` (the P3-23 three-zone dashboard over
//!   the shared state) + `events::poll_event(250 ms)` → the frozen
//!   `key_to_action` table (P3-22 + TUI-01/02) → [`dispatch_action`]:
//!   the state-only actions mutate the shared state under one brief
//!   write scope (no I/O while the lock is held, C14-03) — the
//!   `[g]`/`[t]`/`[d]` toggles write the `settings.*_open` flags
//!   (the `[d]` key also drives the `requirements_dismissed` latch —
//!   closing the strip dismisses the auto-open, reopening clears
//!   it), the `[p]` poll / `[w]` window cycles advance the presets
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
//!   no-telemetry hint) in the state the same transient way, and
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
//! the CWD, `[E]` writes `ramsleuth-export-<unix-ts>.json` in `$HOME`
//! (the CWD when `HOME` is unset), `[Q]` restores the terminal + exit
//! 0); with no daemon the
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
//! Exit codes: 0 quit, 1 terminal init failure, 2 usage error
//! ```

use std::ffi::OsStr;
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
use ramsleuth_bench::{BenchmarkGrid, BenchOp, BurnInTick, StreamTarget, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response, DEFAULT_SOCKET_PATH};
use ramsleuth_telemetry::SystemMemoryTelemetry;
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
    "ramsleuth-tui — the live terminal dashboard (16-key parity)\n",
    "\n",
    "Usage: ramsleuth-tui [OPTIONS]\n",
    "\n",
    "Options:\n",
    "  --socket <path>   Daemon Unix socket\n",
    "                    (default: /run/ramsleuth/ramsleuth.sock)\n",
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
    "  [D]requirements  toggle the setup requirements strip\n",
    "  [P]oll           cycle the poll interval (100 ms → 60 s, wrap)\n",
    "  [U]nits          toggle the capacity units (GiB ↔ GB)\n",
    "  [K]lock          toggle the clock units (MHz ↔ GHz)\n",
    "  [A]uto refresh   toggle the periodic data poll (on ↔ off)\n",
    "  [W]indow         cycle the graphs window (1 → 5 → 15 → 60 min)\n",
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
            Ok(Response::ProbeReport(_)) => {
                // A probe-report frame in a normal-bench stream
                // violates the wire contract (probe reports reply
                // only to `GetProbeReport`, chunk probe-1a): stop
                // the run and record it.
                let mut s = state.write().unwrap();
                s.error = Some("unexpected probe report during benchmark".to_owned());
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
            let mut s = state.write().unwrap();
            s.bench.burn_in.running = false;
            s.error = Some(error.to_string());
            return Ok(());
        }
    };
    // The stream read timeout (C7-16): the client's 5 s default
    // would kill a long soak's frame gap mid-stream (the 120 s CLI
    // precedent).
    if let Err(error) = client.set_read_timeout(BENCH_READ_TIMEOUT) {
        let mut s = state.write().unwrap();
        s.bench.burn_in.running = false;
        s.error = Some(error.to_string());
        return Ok(());
    }
    {
        // A fresh run starts from the idle burn-in state: a stale
        // run's tick bookkeeping (iteration / elapsed / `latest`)
        // never mixes into a new one — one brief scope.
        let mut s = state.write().unwrap();
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
        let mut s = state.write().unwrap();
        s.bench.burn_in.running = false;
        s.error =
            Some("burn-in command without a duration_minutes discriminator".to_owned());
        return Ok(());
    };
    if let Err(error) = client.send(&Request::StartBurnIn {
        target: cmd.target,
        duration_minutes,
    }) {
        let mut s = state.write().unwrap();
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
            let run_id = state.read().unwrap().bench.run_id;
            if let Some(run_id) = run_id {
                let _ = client.send(&Request::CancelBenchmark { run_id });
            }
            state.write().unwrap().bench.burn_in.running = false;
            break;
        }
        match client.recv() {
            Ok(Response::BenchStarted { run_id }) => {
                // The shared run id lands (the `[C]` key addresses
                // the run with it) — a brief scope.
                state.write().unwrap().bench.run_id = Some(run_id);
            }
            Ok(Response::BurnInProgress(tick)) => {
                // The tick's bookkeeping lands in the burn-in state
                // under one brief write scope: the newest iteration /
                // elapsed, and its cell / latency written into
                // `latest` (the newest value per cell wins — the
                // bench zone's `live_grid` rule; a non-finite /
                // non-positive reading never renders as data).
                let BurnInTick {
                    iteration,
                    elapsed_secs,
                    tier,
                    bandwidth,
                    latency_ns,
                } = tick;
                let mut s = state.write().unwrap();
                s.bench.burn_in.iteration = iteration;
                s.bench.burn_in.elapsed_secs = elapsed_secs;
                let slot = tier as usize;
                match (bandwidth, latency_ns) {
                    (Some((op, value)), _) => {
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
                        if ns.is_finite() && ns > 0.0 {
                            s.bench.burn_in.latest.latency_ns[slot] = ns;
                        }
                    }
                    (None, None) => {
                        // A tick with neither a cell nor a latency is
                        // off-contract (exactly one of the two is
                        // `Some`, the `BurnInTick` contract): stop
                        // the run and record it.
                        s.error =
                            Some("burn-in tick carried neither a cell nor a latency".to_owned());
                        s.bench.burn_in.running = false;
                        break;
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
                let mut s = state.write().unwrap();
                s.error = Some("unexpected benchmark frame during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal grid is the last completed pass: it
                // lands in the shared terminal grid (the zone-2
                // table + the later F3 export pick it up for free)
                // + the clean stop in one brief scope.
                let mut s = state.write().unwrap();
                s.bench.grid = Some(grid);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                // The clean-stop ack (the run ended at its cancel
                // gate) — a brief scope.
                state.write().unwrap().bench.burn_in.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                // A structured daemon reply — including the
                // single-flight `Error` for a second start while a
                // run is in flight (P3-15/D-6): record + end, never
                // a panic (D5).
                let mut s = state.write().unwrap();
                s.error = Some(message);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the burn-in stream violates
                // the wire contract (`GetTelemetry` is served on its
                // own connection, P3-16): stop the run and record
                // it.
                let mut s = state.write().unwrap();
                s.error = Some("unexpected response during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::ProbeReport(_)) => {
                // A probe-report frame in a burn-in stream
                // violates the wire contract (probe reports reply
                // only to `GetProbeReport`, chunk probe-1a): stop
                // the run and record it.
                let mut s = state.write().unwrap();
                s.error = Some("unexpected probe report during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Err(error) => {
                let mut s = state.write().unwrap();
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
    home.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
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
        let snapshot = state.read().unwrap();
        perform_export(&snapshot, &export_out_dir())
    };
    record_export_result(&mut state.write().unwrap(), result);
}

// ------------------------------------------------------------------
// TUI-22: the action dispatch — the frozen 16-key table (P3-22 +
// TUI-01/02) over the shared state: the state-only actions (the
// toggles, the cycles, the run keys, the cancel) mutate the state
// and report no side effect; the I/O actions report the effect the
// main loop performs with the lock released.
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
/// `[Q]` → [`SideEffect::Quit`].
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
        let app = state.read().unwrap();
        !app.settings.requirements_open && !ramsleuth_tui::requirements::diagnose(&app).is_empty()
    };
    if needs_open {
        state.write().unwrap().settings.requirements_open = true;
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
                    // The action's state mutations (the toggles, the
                    // cycles, the run-key channel sends, the cancel
                    // flag, the `[d]` latch) run under one brief
                    // write scope — no I/O while the lock is held
                    // (C14-03); the returned side effect (the `[R]`
                    // poll, the `[S]`/`[E]` file writes, the `[Q]`
                    // break) is performed with the lock released.
                    let side = {
                        let mut s = state.write().unwrap();
                        dispatch_action(action, &mut s, &mut requirements_dismissed, &bench_tx, &cancel)
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
                            let _ = poll_once(&args.socket, &mut state.write().unwrap());
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

    // ------------------------------------------------------------------
    // TUI-20: the burn-in worker — `run_burn_in` against an in-process
    // daemon stand-in (the GUI `update.rs` C7-16 stand-in suite,
    // adapted to the TUI state / command types), the `[C]` / `[x]`
    // key wiring, and the compound single-flight guard.
    // ------------------------------------------------------------------

    /// (u) `run_burn_in` against a stand-in (a `StartBurnIn` answered
    /// with `BenchStarted` + `BurnInProgress` ticks — one bandwidth
    /// kind and one latency kind — + the terminal `BenchResult`)
    /// consumes the stream into `state.bench`: the request rides the
    /// wire with the cmd's target / duration, the `BenchStarted`
    /// ack's `run_id` is recorded, each tick updates `burn_in` (the
    /// newest `iteration` / `elapsed_secs`, and the `latest` grid —
    /// the newest value per cell wins, the `live_grid` rule), and
    /// the terminal lands its grid in `state.bench.grid` + clears
    /// `burn_in.running` — with no transport error, a stale pre-run
    /// burn-in state reset at run start, and the normal-bench
    /// `running` flag / `progress` untouched (the two run classes
    /// share only the `run_id` + the cancel flag).
    #[test]
    fn run_burn_in_streams_ticks_and_the_terminal_grid() {
        let sock = TempSocket::new("burnin");
        let grid = BenchmarkGrid {
            read_gbps: [51.2, 901.4, 612.7, 240.3],
            write_gbps: [47.8, 870.2, 590.1, 221.9],
            copy_gbps: [28.4, 455.6, 310.8, 130.4],
            latency_ns: [91.2, 1.3, 3.9, 14.1],
        };
        let expected_grid = grid.clone();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBurnIn { target, duration_minutes })) => {
                    assert_eq!(
                        target,
                        StreamTarget::Full,
                        "the cmd's target must ride the wire"
                    );
                    assert_eq!(duration_minutes, 5, "the cmd's duration must ride the wire");
                }
                other => panic!("stand-in expected StartBurnIn, got {other:?}"),
            }
            for response in [
                Response::BenchStarted { run_id: 9 },
                Response::BurnInProgress(BurnInTick {
                    iteration: 1,
                    elapsed_secs: 1.5,
                    tier: Tier::Memory,
                    bandwidth: Some((BenchOp::Read, 50.0)),
                    latency_ns: None,
                }),
                Response::BurnInProgress(BurnInTick {
                    iteration: 1,
                    elapsed_secs: 2.0,
                    tier: Tier::L1,
                    bandwidth: None,
                    latency_ns: Some(1.2),
                }),
                // A later tick over the same cell: the newest value
                // wins in `latest` (the `live_grid` rule).
                Response::BurnInProgress(BurnInTick {
                    iteration: 2,
                    elapsed_secs: 3.5,
                    tier: Tier::Memory,
                    bandwidth: Some((BenchOp::Read, 52.5)),
                    latency_ns: None,
                }),
                Response::BenchResult { run_id: 9, grid },
            ] {
                let bytes = encode_frame(&Message::Response(response)).expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        });

        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(5),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = AppState::default();
        // A stale pre-run burn-in state must be reset at run start
        // (iteration / elapsed / `latest`).
        state.bench.burn_in.running = true;
        state.bench.burn_in.iteration = 7;
        state.bench.burn_in.elapsed_secs = 99.0;
        state.bench.burn_in.latest.read_gbps[0] = 7.0;
        // A stale pre-run id must be dropped at run start.
        state.bench.run_id = Some(99);
        // The normal-bench flag / progress must stay untouched by a
        // burn-in run.
        state.bench.running = true;
        state.bench.progress.push(StreamProgress {
            cell_index: 9,
            total_cells: 12,
            tier: Tier::L3,
            op: BenchOp::Copy,
            value: 1.0,
            label: "stale".to_owned(),
        });
        let state = Arc::new(RwLock::new(state));
        run_burn_in(sock.path(), cmd, &state, &cancel).expect("run_burn_in must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.burn_in.running, "the terminal must clear burn_in.running");
        assert!(
            state.bench.running,
            "a burn-in run must not touch the normal-bench running flag"
        );
        assert_eq!(
            state.bench.progress,
            vec![StreamProgress {
                cell_index: 9,
                total_cells: 12,
                tier: Tier::L3,
                op: BenchOp::Copy,
                value: 1.0,
                label: "stale".to_owned(),
            }],
            "a burn-in run must not touch the normal-bench progress"
        );
        assert_eq!(state.bench.run_id, Some(9), "the ack's run_id must be recorded");
        assert_eq!(state.bench.burn_in.iteration, 2, "the newest tick's iteration");
        assert_eq!(state.bench.burn_in.elapsed_secs, 3.5, "the newest tick's elapsed");
        assert_eq!(
            state.bench.burn_in.latest.read_gbps[0],
            52.5,
            "the newest value per cell wins (the stale 7.0 is reset)"
        );
        assert_eq!(
            state.bench.burn_in.latest.latency_ns[1],
            1.2,
            "the latency tick lands in the tier's latency cell (L1 = slot 1)"
        );
        assert_eq!(
            state.bench.burn_in.latest.write_gbps,
            [0.0; 4],
            "an unticked column stays zero"
        );
        assert_eq!(
            state.bench.burn_in.latest.copy_gbps,
            [0.0; 4],
            "an unticked column stays zero"
        );
        assert_eq!(
            state.bench.grid,
            Some(expected_grid),
            "the terminal grid must land in the state"
        );
        assert!(state.error.is_none(), "a clean run must not record an error");
        stand_in.join();
    }

    /// (v) CANCEL post-ack (the `[C]` key's shared flag, TUI-20 —
    /// the GUI C7-16 (k2) mirror): after the burn-in's
    /// `BenchStarted` ack lands in the state, the shared `cancel`
    /// flag is set — `run_burn_in` ends the run with
    /// `burn_in.running = false` (sending the daemon a best-effort
    /// `CancelBenchmark` for the recorded run id — the stand-in
    /// drains it) — no hang, no panic, no error recorded.
    #[test]
    fn run_burn_in_cancel_flag_stops_the_run() {
        let sock = TempSocket::new("burnin-cancel");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBurnIn { target, duration_minutes })) => {
                    assert_eq!(target, StreamTarget::Full, "the cmd's target must ride the wire");
                    assert_eq!(
                        duration_minutes,
                        0,
                        "the infinite (0) duration must ride the wire"
                    );
                }
                other => panic!("stand-in expected StartBurnIn, got {other:?}"),
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
                    duration_minutes: Some(0),
                };
                // The function locks the shared state only per
                // mutation — never across the stream drain (C14-03).
                run_burn_in(&socket, cmd, &state, &cancel)
                    .expect("run_burn_in must not error");
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
        worker.join().expect("run_burn_in must return on cancel (no hang)");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.burn_in.running, "the cancel must clear burn_in.running");
        assert_eq!(state.bench.run_id, Some(42), "the run id must stay recorded");
        assert!(state.error.is_none(), "a clean cancel must not record an error");
        stand_in.join();
    }

    /// (w) CANCEL pre-ack (TUI-20 — the plan's "a pre-ack cancel
    /// only sets the flag"): the shared `cancel` flag is set before
    /// the `BenchStarted` ack has landed (`run_id` is still `None`)
    /// — `run_burn_in` ends the run at its cancel gate with
    /// `burn_in.running = false` and **no** `CancelBenchmark` sent
    /// (the stand-in observes only the stream's EOF, no request),
    /// no error recorded.
    #[test]
    fn run_burn_in_pre_ack_cancel_sends_no_request() {
        let sock = TempSocket::new("burnin-preack-cancel");
        let seen_cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let stand_in = {
            let cancel = Arc::clone(&cancel);
            let seen_cancel = Arc::clone(&seen_cancel);
            DaemonStandIn::spawn(&sock, move |mut stream| {
                match read_one_message(&mut stream) {
                    Some(Message::Request(Request::StartBurnIn { .. })) => {}
                    other => panic!("stand-in expected StartBurnIn, got {other:?}"),
                }
                // The worker is blocked in `recv()` with no ack
                // sent: wait for the pre-ack cancel flag, then push
                // one tick so the worker's loop returns to its
                // cancel gate (the `run_id` is still `None` — the
                // break must send no request). The write is
                // best-effort: if the worker breaks before reading
                // it (the flag was set before its first gate check),
                // the client is already dropped — the drain below
                // sees the EOF either way.
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel.load(Ordering::Relaxed) {
                    assert!(
                        Instant::now() < deadline,
                        "the cancel flag was never set"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                let tick = encode_frame(&Message::Response(Response::BurnInProgress(BurnInTick {
                    iteration: 1,
                    elapsed_secs: 0.1,
                    tier: Tier::Memory,
                    bandwidth: Some((BenchOp::Read, 5.0)),
                    latency_ns: None,
                })))
                .expect("must encode");
                let _ = stream.write_all(&tick);
                // Drain the rest of the stream: a pre-ack cancel
                // must send no `CancelBenchmark`, so the only
                // thing left is the client drop's EOF.
                loop {
                    match read_one_message(&mut stream) {
                        Some(Message::Request(Request::CancelBenchmark { .. })) => {
                            seen_cancel.store(true, Ordering::Relaxed);
                        }
                        Some(other) => panic!("stand-in saw an unexpected frame: {other:?}"),
                        None => break, // the client dropped
                    }
                }
            })
        };

        let socket = sock.path().to_path_buf();
        let state = Arc::new(RwLock::new(AppState::default()));
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = TuiBenchCmd {
                    target: StreamTarget::Full,
                    mode: BenchMode::Full,
                    duration_minutes: Some(0),
                };
                run_burn_in(&socket, cmd, &state, &cancel)
                    .expect("run_burn_in must not error");
            })
        };

        // Wait for the run to start (its start-scope sets
        // `burn_in.running` before the `StartBurnIn` send — the ack
        // has not landed), then set the shared cancel flag.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if state
                .read()
                .expect("the updater must not poison the lock")
                .bench
                .burn_in
                .running
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the run never started within 5 s"
            );
            thread::sleep(Duration::from_millis(10));
        }
        cancel.store(true, Ordering::Relaxed);
        worker.join().expect("run_burn_in must return on the pre-ack cancel (no hang)");
        stand_in.join();

        assert!(
            !seen_cancel.load(Ordering::Relaxed),
            "a pre-ack cancel must send no CancelBenchmark (no run id yet)"
        );
        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.burn_in.running, "the cancel must clear burn_in.running");
        assert_eq!(state.bench.run_id, None, "the ack never landed — no run id");
        assert!(state.error.is_none(), "a clean cancel must not record an error");
    }

    /// (x) A `BenchProgress` frame in a burn-in stream violates the
    /// wire contract (normal-bench progress streams only on the
    /// owning `StartBenchmark` connection, D-1/D-2): `run_burn_in`
    /// records the structured error and ends the run with
    /// `burn_in.running = false` — the permanent mirror of
    /// `run_bench`'s `BurnInProgress` guard.
    #[test]
    fn run_burn_in_rejects_a_bench_progress_frame() {
        let sock = TempSocket::new("burnin-contract");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBurnIn { .. })) => {}
                other => panic!("stand-in expected StartBurnIn, got {other:?}"),
            }
            let started =
                encode_frame(&Message::Response(Response::BenchStarted { run_id: 3 }))
                    .expect("must encode");
            stream.write_all(&started).expect("stand-in write must not fail");
            let progress = encode_frame(&Message::Response(Response::BenchProgress(
                StreamProgress {
                    cell_index: 0,
                    total_cells: 3,
                    tier: Tier::Memory,
                    op: BenchOp::Read,
                    value: 26.35,
                    label: "Memory · Read (GB/s)".to_owned(),
                },
            )))
            .expect("must encode");
            stream.write_all(&progress).expect("stand-in write must not fail");
        });

        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(2),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        run_burn_in(sock.path(), cmd, &state, &cancel).expect("run_burn_in must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.burn_in.running, "the violation must clear burn_in.running");
        let error = state.error.as_ref().expect("the violation must record a structured error");
        assert!(
            error.contains("unexpected benchmark frame during burn-in"),
            "the error must name the violation, got: {error}"
        );
        stand_in.join();
    }

    /// (y) `run_burn_in` against a **non-existent** socket records
    /// the friendly `DaemonDown` text, leaves `burn_in.running =
    /// false`, and never panics (the no-panic contract, D5) — the
    /// run's connect failure is a transport failure reported
    /// through the state.
    #[test]
    fn run_burn_in_missing_socket_is_friendly() {
        let sock = TempSocket::new("burnin-missing");
        let cmd = TuiBenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(5),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        run_burn_in(sock.path(), cmd, &state, &cancel).expect("run_burn_in must not error");

        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.burn_in.running, "a failed connect must not leave a burn-in in flight");
        assert_eq!(
            state.bench.burn_in.latest,
            BenchmarkGrid {
                read_gbps: [0.0; 4],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            },
            "no tick landed — the latest grid stays zero"
        );
        let error = state.error.clone().expect("a friendly error must be recorded");
        assert!(
            error.contains("daemon not running"),
            "the error must carry the DaemonDown hint, got: {error}"
        );
    }

    /// (z) The compound single-flight guard (TUI-20): true while
    /// **any** run class is in flight — a normal bench only, a
    /// burn-in only, both, and false when neither (the `[b]` /
    /// `[m]` / `[x]` send-skip must consider both classes, since
    /// they never touch each other's flag).
    #[test]
    fn run_in_flight_tracks_both_run_classes() {
        assert!(!run_in_flight(&BenchState::default()), "idle: no run in flight");
        let bench = BenchState { running: true, ..Default::default() };
        assert!(run_in_flight(&bench), "a normal bench in flight");
        let burn = BenchState {
            burn_in: ramsleuth_tui::ui::BurnInState { running: true, ..Default::default() },
            ..Default::default()
        };
        assert!(run_in_flight(&burn), "a burn-in in flight");
        let both = BenchState {
            running: true,
            burn_in: ramsleuth_tui::ui::BurnInState { running: true, ..Default::default() },
            ..Default::default()
        };
        assert!(run_in_flight(&both), "both classes in flight");
    }

    /// (aa) The updater's run-class dispatch (TUI-20 — the GUI
    /// (k4) mirror, the TUI `spawn_updater` form): a default
    /// (`duration_minutes = None`) cmd routes to `run_bench` (a
    /// `StartBenchmark` on the wire) and a burn-in (`Some(7)`) cmd
    /// routes to `run_burn_in` (a `StartBurnIn` on the wire — the
    /// pre-TUI-20 placeholder recorded "not available in this
    /// build" instead) — both runs stream to their terminal cleanly
    /// (the one-shot baseline may interleave; the stand-in serves
    /// every request arm it sees).
    #[test]
    fn spawn_updater_dispatches_burn_in_cmds_by_the_duration_discriminator() {
        let sock = TempSocket::new("burnin-dispatch");
        let stop = Arc::new(AtomicBool::new(false));
        // Bit 0 = a `StartBenchmark` observed, bit 1 = a
        // `StartBurnIn` observed.
        let seen = Arc::new(AtomicUsize::new(0));
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        let stand_in = {
            let stop = Arc::clone(&stop);
            let seen = Arc::clone(&seen);
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
                    match read_one_message(&mut stream) {
                        Some(Message::Request(Request::StartBenchmark { .. })) => {
                            seen.fetch_or(1, Ordering::Relaxed);
                            let started = encode_frame(&Message::Response(
                                Response::BenchStarted { run_id: 1 },
                            ))
                            .expect("must encode");
                            stream.write_all(&started).expect("stand-in write must not fail");
                            let result = encode_frame(&Message::Response(Response::BenchResult {
                                run_id: 1,
                                grid: BenchmarkGrid {
                                    read_gbps: [1.0, 0.0, 0.0, 0.0],
                                    write_gbps: [0.0; 4],
                                    copy_gbps: [0.0; 4],
                                    latency_ns: [0.0; 4],
                                },
                            }))
                            .expect("must encode");
                            stream.write_all(&result).expect("stand-in write must not fail");
                        }
                        Some(Message::Request(Request::StartBurnIn { target, duration_minutes })) => {
                            assert_eq!(
                                target,
                                StreamTarget::Full,
                                "the burn-in cmd's target must ride the wire"
                            );
                            assert_eq!(
                                duration_minutes,
                                7,
                                "the burn-in cmd's duration must ride the wire"
                            );
                            seen.fetch_or(2, Ordering::Relaxed);
                            let started = encode_frame(&Message::Response(
                                Response::BenchStarted { run_id: 2 },
                            ))
                            .expect("must encode");
                            stream.write_all(&started).expect("stand-in write must not fail");
                            let cancelled = encode_frame(&Message::Response(
                                Response::BenchCancelled { run_id: 2 },
                            ))
                            .expect("must encode");
                            stream.write_all(&cancelled).expect("stand-in write must not fail");
                        }
                        Some(Message::Request(Request::GetTelemetry)) => {
                            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                                mock_snapshot(),
                            )))
                            .expect("must encode");
                            stream.write_all(&bytes).expect("stand-in write must not fail");
                        }
                        Some(other) => {
                            panic!("stand-in saw an unexpected frame: {other:?}")
                        }
                        None => break, // the waker (an EOF before a frame)
                    }
                }
            })
        };

        // Refresh explicitly off + the 100 ms clamp floor: only the
        // dispatched runs + the one-shot baseline may fetch, and
        // the 100 ms tick services the queued cmds promptly.
        let mut app = AppState::default();
        app.settings.refresh = false;
        app.settings.poll_interval_ms = 100;
        let state = Arc::new(RwLock::new(app));
        let (bench_tx, bench_rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let updater = spawn_updater(
            Arc::clone(&state),
            Arc::clone(&stop),
            sock.path().to_path_buf(),
            bench_rx,
            cancel,
        );

        // Phase A: the default (`duration_minutes = None`) cmd — the
        // dispatch must route it to `run_bench` (a `StartBenchmark`
        // on the wire).
        bench_tx
            .send(TuiBenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
                duration_minutes: None,
            })
            .expect("the channel must accept the cmd");
        // Phase B: the burn-in (`duration_minutes = Some(7)`) cmd —
        // the dispatch must route it to `run_burn_in` (a
        // `StartBurnIn` on the wire).
        bench_tx
            .send(TuiBenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
                duration_minutes: Some(7),
            })
            .expect("the channel must accept the cmd");

        // Wait until both request arms were observed (bounded by a 10
        // s deadline: the test fails with the current bits instead of
        // hanging).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let bits = seen.load(Ordering::Relaxed);
            if bits == 3 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the dispatch never served both arms, saw {bits:#b}"
            );
            thread::sleep(Duration::from_millis(20));
        }

        // Stop: drop the bench sender (the updater's channel
        // disconnects), set the shared flag, and wake the stand-in's
        // final accept with a throwaway connect (immediate EOF; a
        // failed connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock.path());
        stand_in.join().expect("the stand-in must not panic");
        updater.join().expect("the updater thread must not panic");

        // Both runs ended cleanly: nothing left in flight (either
        // class), no error recorded.
        let state = state.read().expect("the updater must not poison the lock");
        assert!(!state.bench.running, "the normal run must end at its terminal");
        assert!(!state.bench.burn_in.running, "the burn-in must end at its terminal");
        assert!(state.error.is_none(), "clean runs must not record an error");
    }

    // ------------------------------------------------------------------
    // TUI-21: the JSON export (F3 parity) — the GUI `style.rs` /
    // `main.rs` export suite, adapted to the TUI state, the
    // `String`-mapped failure class, and the `$HOME`/CWD-fallback
    // rule. Every file lands in a temp out dir, never `$HOME`.
    // ------------------------------------------------------------------

    /// A unique temp out dir per test (pid-scoped — the GUI
    /// `temp_out_dir` precedent); created now, removed on drop.
    struct TempOutDir {
        path: PathBuf,
    }

    impl TempOutDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("ramsleuth-tui-{name}-{}", process::id()));
            std::fs::create_dir_all(&path).expect("the test out dir must be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempOutDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// The fixture grid (the GUI `fixture_grid` shape): the Memory row
    /// populated, the L1/L2/L3 rows unmeasured (`0.0` = the honest
    /// `N/A`).
    fn fixture_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [43.63, 0.0, 0.0, 0.0],
            copy_gbps: [12.11, 0.0, 0.0, 0.0],
            latency_ns: [86.84, 0.0, 0.0, 0.0],
        }
    }

    /// (bb) `export_json` on a live snapshot + terminal grid writes a
    /// file that parses back as a JSON object with the eight wire
    /// keys (the seven telemetry keys: `cpu` / `amd` / `intel` /
    /// `spd` / `platform` / `total_capacity` / `dimm_sizes` — plus
    /// the `bench` key), the telemetry sub-object byte-compatible
    /// with the plain wire serialization, and the `bench` key
    /// round-tripping into an equal `BenchmarkGrid`; the whole file
    /// re-parses into an equal `SystemMemoryTelemetry` (the wire
    /// root ignores the extra `bench` key — the GUI `style.rs` (b)
    /// mirror).
    #[test]
    fn export_json_writes_parseable_snapshot() {
        let dir = TempOutDir::new("export");
        let telemetry = mock_snapshot();
        let grid = fixture_grid();
        let path = dir.path().join("export.json");
        export_json(&telemetry, Some(&grid), &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        let keys = value
            .as_object()
            .expect("checked is_object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let expected: std::collections::BTreeSet<String> = [
            "amd", "bench", "cpu", "intel", "spd", "platform", "total_capacity", "dimm_sizes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            keys, expected,
            "the seven telemetry wire keys + the bench key"
        );

        // The C6-29 `bench` key carries the grid verbatim.
        let back_grid: BenchmarkGrid = serde_json::from_value(value["bench"].clone())
            .expect("bench must re-parse into the grid");
        assert_eq!(back_grid, grid, "the grid round-trips");

        // The flattened telemetry sub-object is byte-compatible:
        // every one of the seven wire keys matches the plain wire
        // root serialization.
        let base = serde_json::to_value(&telemetry).expect("the wire root must serialize");
        for key in ["cpu", "amd", "intel", "spd", "platform", "total_capacity", "dimm_sizes"] {
            assert_eq!(value[key], base[key], "the {key} key must stay byte-compatible");
        }

        // The wire root is serde round-trip safe (P3-06); the extra
        // `bench` key is ignored on the way back.
        let back: SystemMemoryTelemetry =
            serde_json::from_str(&text).expect("the file must re-parse into the wire root");
        assert_eq!(back, telemetry, "the telemetry round-trips");
    }

    /// (bc) `bench` is `None` (no run has completed yet) → the
    /// `bench` key is the JSON `null`: the snapshot shape stays
    /// stable and parseable, the telemetry sub-object is untouched,
    /// never a panic (the GUI `style.rs` (b2) mirror).
    #[test]
    fn export_json_bench_none_is_null() {
        let dir = TempOutDir::new("export-nobench");
        let telemetry = mock_snapshot();
        let path = dir.path().join("export-nobench.json");
        export_json(&telemetry, None, &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        assert!(value["bench"].is_null(), "no run yet: the bench key is the JSON null");
        let base = serde_json::to_value(&telemetry).expect("the wire root must serialize");
        for key in ["cpu", "amd", "intel", "spd", "platform", "total_capacity", "dimm_sizes"] {
            assert_eq!(value[key], base[key], "the {key} key must stay byte-compatible");
        }
    }

    /// (bd) The all-`N/A` grid (every cell `0.0` — the wire
    /// unmeasured marker) exports as-is: the `bench` object
    /// round-trips into the same zero grid, never a panic (the GUI
    /// `style.rs` (b3) mirror).
    #[test]
    fn export_json_all_na_grid_exports_as_is() {
        let dir = TempOutDir::new("export-nagrid");
        let telemetry = mock_snapshot();
        let grid = BenchmarkGrid {
            read_gbps: [0.0; 4],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        let path = dir.path().join("export-nagrid.json");
        export_json(&telemetry, Some(&grid), &path)
            .expect("export_json must not fail for a writable temp path");

        let text = std::fs::read_to_string(&path).expect("the exported file must exist");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("export must be valid JSON");
        let back_grid: BenchmarkGrid = serde_json::from_value(value["bench"].clone())
            .expect("bench must re-parse into the grid");
        assert_eq!(back_grid, grid, "the all-Na grid exports as-is");
    }

    /// (be) `export_json` maps a write failure to a `String` (the
    /// `GuiError::Io` class without the GUI error enum — the
    /// no-panic contract: a missing parent dir is an error, not a
    /// panic; the GUI `style.rs` (c) mirror).
    #[test]
    fn export_json_maps_write_failure_to_a_string() {
        let dir = TempOutDir::new("export-fail");
        let telemetry = mock_snapshot();
        // A nested path whose parent dir does not exist: `fs::write`
        // fails with a missing-dir I/O error.
        let path = dir.path().join("no-such-dir").join("missing-dir.json");
        let err =
            export_json(&telemetry, None, &path).expect_err("a missing parent dir must fail");
        assert!(
            err.starts_with("export file write failed:"),
            "the write failure must carry the I/O class, got: {err}"
        );
        assert!(!path.exists(), "a failed write must leave no file");
    }

    /// (bf) `perform_export` with telemetry set → a JSON file is
    /// written to the out dir, `Ok(Some(path))`, the file exists +
    /// parses as a JSON object carrying the terminal grid under the
    /// `bench` key (the GUI `main.rs` (b) mirror).
    #[test]
    fn perform_export_with_telemetry_writes_and_parses() {
        let dir = TempOutDir::new("perform-export");
        let grid = fixture_grid();
        let state = AppState {
            telemetry: Some(mock_snapshot()),
            bench: BenchState { grid: Some(grid.clone()), ..Default::default() },
            ..Default::default()
        };

        let path = perform_export(&state, dir.path())
            .expect("the export must not fail")
            .expect("telemetry set must produce a file");
        assert!(
            path.starts_with(dir.path()),
            "the file must land in the out dir: {path:?}"
        );
        let name = path.file_name().and_then(|s| s.to_str()).expect("the file must be named");
        assert!(
            name.starts_with("ramsleuth-export-"),
            "the file must carry the export name prefix, got: {name}"
        );
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("json"),
            "the file must be a .json"
        );

        let text = std::fs::read_to_string(&path).expect("the exported file must be readable");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("the export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        let back_grid: BenchmarkGrid = serde_json::from_value(value["bench"].clone())
            .expect("bench must re-parse into the grid");
        assert_eq!(back_grid, grid, "the terminal grid is exported");
    }

    /// (bg) `perform_export` with no telemetry → `Ok(None)` and no
    /// file is written (the GUI `main.rs` (d) mirror).
    #[test]
    fn perform_export_without_telemetry_is_none() {
        let dir = TempOutDir::new("perform-export-none");
        let state = AppState::default(); // no snapshot yet

        let result =
            perform_export(&state, dir.path()).expect("no telemetry must not fail");
        assert!(result.is_none(), "no telemetry: no file (the GUI rule)");
        let count = std::fs::read_dir(dir.path()).expect("the dir must be readable").count();
        assert_eq!(count, 0, "no telemetry: nothing was written to the out dir");
    }

    /// (bh) `perform_export` maps a write failure to a `String` (a
    /// missing parent dir under the out dir): `Err`, no file, no
    /// panic — the `GuiError::Io` class without the enum.
    #[test]
    fn perform_export_maps_write_failure_to_a_string() {
        let dir = TempOutDir::new("perform-export-fail");
        let state = AppState { telemetry: Some(mock_snapshot()), ..Default::default() };
        // Nest the out dir one level below the temp dir: the missing
        // parent makes the write fail.
        let nested = dir.path().join("no-such-dir");
        let err =
            perform_export(&state, &nested).expect_err("a missing parent dir must fail");
        assert!(
            err.starts_with("export file write failed:"),
            "the write failure must carry the I/O class, got: {err}"
        );
    }

    /// (bi) The `$HOME` → CWD-fallback rule (the GUI rule, its pure
    /// form): a set `HOME` is the destination, an unset `HOME`
    /// degrades to the CWD (`.`) — never a panic.
    #[test]
    fn out_dir_from_home_arms() {
        assert_eq!(
            out_dir_from_home(Some(OsStr::new("/home/x"))),
            PathBuf::from("/home/x"),
            "a set HOME is the destination"
        );
        assert_eq!(
            out_dir_from_home(None),
            PathBuf::from("."),
            "an unset HOME degrades to the CWD"
        );
    }

    /// (bj) The record arms (the GUI transient notice, the
    /// `[S]`napshot mechanism): the written path lands in
    /// `daemon_status` (transient), the no-telemetry hint lands
    /// there too, and a write failure lands in `error` — never a
    /// panic (the no-panic contract, D5).
    #[test]
    fn record_export_result_arms() {
        let mut state = AppState::default();

        let path = PathBuf::from("/tmp/ramsleuth-export-1.json");
        record_export_result(&mut state, Ok(Some(path)));
        assert_eq!(
            state.daemon_status,
            "export: /tmp/ramsleuth-export-1.json",
            "the written path lands in daemon_status (transient)"
        );
        assert!(state.error.is_none(), "a written export must not record an error");

        record_export_result(&mut state, Ok(None));
        assert_eq!(
            state.daemon_status,
            "nothing to export yet (no telemetry)",
            "the no-telemetry hint lands in daemon_status"
        );
        assert!(state.error.is_none(), "a hint must not record an error");

        record_export_result(&mut state, Err("gone".to_owned()));
        assert_eq!(
            state.error.as_deref(),
            Some("export failed: gone"),
            "the write failure lands in error (the GuiError class text)"
        );
    }
    // ------------------------------------------------------------------
    // TUI-22: the dispatch + settings cycles + the `[d]` auto-open
    // latch — the dispatch split (`dispatch_action` + `SideEffect`)
    // makes the whole key `match` unit-testable without a TTY.
    // ------------------------------------------------------------------

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// (bk) The `[p]` poll cycle wraps the 8 presets in order (plan
    /// §2.2): 100 ms → 500 ms → 1 s → 2 s → 5 s → 10 s → 30 s →
    /// 60 s → wrap to 100 ms — the default 2 s is a preset (the
    /// current TUI behavior), and an off-preset stored value
    /// advances to the next preset above it (a past-the-last value
    /// wraps).
    #[test]
    fn cycle_poll_wraps_the_eight_presets() {
        let presets = [100u64, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000];
        // The full loop from the first preset (the wrap is the
        // final step back to the first).
        let mut value = presets[0];
        for expected in &presets[1..] {
            value = cycle_poll(value);
            assert_eq!(value, *expected, "the cycle advances in preset order");
        }
        assert_eq!(
            cycle_poll(value),
            presets[0],
            "past the last preset the cycle wraps"
        );
        // Off-preset values advance to the next preset above them.
        assert_eq!(cycle_poll(200), 500, "a sub-500 ms value lands on 500 ms");
        assert_eq!(cycle_poll(400), 500, "a sub-500 ms value lands on 500 ms");
        assert_eq!(cycle_poll(1_500), 2_000, "between 1 s and 2 s it lands on 2 s");
        assert_eq!(cycle_poll(59_000), 60_000, "between 30 s and 60 s it lands on 60 s");
        assert_eq!(cycle_poll(61_000), 100, "past the last preset it wraps");
    }

    /// (bl) The `[w]` window cycle wraps the 4 presets in order
    /// (plan §2.2): 1 → 5 → 15 → 60 min → wrap — the default 5 min
    /// is a preset, and an off-preset value advances to the next
    /// preset above it.
    #[test]
    fn cycle_window_wraps_the_four_presets() {
        assert_eq!(cycle_window(1), 5);
        assert_eq!(cycle_window(5), 15);
        assert_eq!(cycle_window(15), 60);
        assert_eq!(cycle_window(60), 1, "past the last preset the cycle wraps");
        assert_eq!(cycle_window(2), 5, "an off-preset value lands on the next preset above");
        assert_eq!(cycle_window(7), 15);
        assert_eq!(cycle_window(20), 60);
        assert_eq!(cycle_window(90), 1, "past the last preset it wraps");
    }

    /// (bm) The unit / refresh toggles (the `[u]`/`[k]`/`[a]` keys)
    /// flip their settings field and report no side effect (the
    /// render-side key write — no I/O on the key): each toggle
    /// round-trips to its starting value.
    #[test]
    fn unit_and_refresh_toggles_flip_the_settings() {
        let (tx, _rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut dismissed = false;
        let mut state = AppState::default();

        assert!(state.settings.capacity_gib, "the GiB default");
        assert_eq!(
            dispatch_action(Action::ToggleCapacity, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(!state.settings.capacity_gib, "GiB → GB");
        assert_eq!(
            dispatch_action(Action::ToggleCapacity, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(state.settings.capacity_gib, "GB → GiB (round-trip)");

        assert!(state.settings.clock_mhz, "the MHz default");
        assert_eq!(
            dispatch_action(Action::ToggleClock, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(!state.settings.clock_mhz, "MHz → GHz");
        assert_eq!(
            dispatch_action(Action::ToggleClock, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(state.settings.clock_mhz, "GHz → MHz (round-trip)");

        assert!(state.settings.refresh, "the TUI's refresh-on default");
        assert_eq!(
            dispatch_action(Action::ToggleRefresh, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(!state.settings.refresh, "refresh on → off (the poller freezes the cadence)");
        assert_eq!(
            dispatch_action(Action::ToggleRefresh, &mut state, &mut dismissed, &tx, &cancel),
            SideEffect::None
        );
        assert!(state.settings.refresh, "off → on (round-trip)");
    }

    /// (bn) The `[d]` auto-open latch (the plan's
    /// `requirements_dismissed`): with a requirement present (the
    /// daemon-less default) and no dismissal, the auto-open
    /// force-true opens the strip; `d` closes it + dismisses the
    /// auto-open (it stays closed while the dismissal holds); `d`
    /// again reopens it + clears the dismissal (the auto-open
    /// applies again). A connected clean state (no requirement) is
    /// never auto-opened — `d` is a plain toggle there.
    #[test]
    fn d_key_drives_the_requirements_auto_open_latch() {
        let (tx, _rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut dismissed = false;
        // The daemon-less default has one requirement (the TUI-08
        // `diagnose` case 1).
        let state = Arc::new(RwLock::new(AppState::default()));

        apply_requirements_auto_open(&state, dismissed);
        assert!(
            state.read().unwrap().settings.requirements_open,
            "the auto-open opens the strip while a requirement is present"
        );

        // `[d]` (the strip is open): close + dismiss.
        {
            let mut s = state.write().unwrap();
            assert_eq!(
                dispatch_action(Action::ToggleRequirements, &mut s, &mut dismissed, &tx, &cancel),
                SideEffect::None
            );
        }
        assert!(dismissed, "closing dismisses the auto-open (the latch)");
        assert!(!state.read().unwrap().settings.requirements_open, "the strip is closed");
        apply_requirements_auto_open(&state, dismissed);
        assert!(
            !state.read().unwrap().settings.requirements_open,
            "a dismissed strip stays closed while the requirement persists"
        );

        // `[d]` again (the strip is closed): reopen + clear the
        // dismissal (the auto-open applies again).
        {
            let mut s = state.write().unwrap();
            assert_eq!(
                dispatch_action(Action::ToggleRequirements, &mut s, &mut dismissed, &tx, &cancel),
                SideEffect::None
            );
        }
        assert!(!dismissed, "reopening clears the dismissal");
        assert!(state.read().unwrap().settings.requirements_open, "the strip is open again");
        apply_requirements_auto_open(&state, dismissed);
        assert!(
            state.read().unwrap().settings.requirements_open,
            "the auto-open keeps an open strip open"
        );

        // A connected clean state has no requirement: the auto-open
        // never touches the closed flag.
        let clean = Arc::new(RwLock::new(AppState {
            daemon_status: "connected: /run/ramsleuth/ramsleuth.sock".to_owned(),
            ..Default::default()
        }));
        apply_requirements_auto_open(&clean, false);
        assert!(
            !clean.read().unwrap().settings.requirements_open,
            "no requirement: no auto-open"
        );
    }

    /// (bo) The run keys' in-flight send-skip (TUI-22 over the
    /// TUI-19/20 compound guard): while a normal bench is in
    /// flight, each of the three run keys is skipped (no channel
    /// send) + the dim status note is recorded; the same while
    /// only a burn-in is in flight (the guard's other class); with
    /// no run in flight, each key queues its `TuiBenchCmd` (the
    /// target / mode / duration ride the channel, in key order).
    #[test]
    fn run_keys_skip_the_send_while_a_run_is_in_flight() {
        let (tx, rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));

        // A normal bench in flight: all three run keys skip + the
        // note lands in `daemon_status`.
        let mut state = AppState {
            bench: BenchState { running: true, ..Default::default() },
            ..Default::default()
        };
        for action in [Action::BenchFull, Action::BenchMemory, Action::BurnIn] {
            assert_eq!(
                dispatch_action(action, &mut state, &mut false, &tx, &cancel),
                SideEffect::None
            );
        }
        assert_eq!(
            state.daemon_status, RUN_IN_FLIGHT_NOTE,
            "a skipped send records the dim status note (the GUI single-flight UX)"
        );
        assert!(rx.try_recv().is_err(), "no command was queued while a run is in flight");

        // A burn-in in flight (the compound guard's other class):
        // the same skip.
        let mut state = AppState {
            bench: BenchState {
                burn_in: ramsleuth_tui::ui::BurnInState { running: true, ..Default::default() },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            dispatch_action(Action::BenchFull, &mut state, &mut false, &tx, &cancel),
            SideEffect::None
        );
        assert!(
            rx.try_recv().is_err(),
            "a burn-in in flight skips the bench send too (the compound guard)"
        );

        // Idle: each run key queues its command (in key order) — the
        // daemon-side single-flight is the worker's concern; the
        // UX skip is this guard.
        let mut state = AppState::default();
        for action in [Action::BenchFull, Action::BenchMemory, Action::BurnIn] {
            assert_eq!(
                dispatch_action(action, &mut state, &mut false, &tx, &cancel),
                SideEffect::None
            );
        }
        let full = rx.try_recv().expect("the `[b]` full-bench cmd was queued");
        assert_eq!(
            (full.target, full.mode, full.duration_minutes),
            (StreamTarget::Full, BenchMode::Full, None),
            "`[b]` = a full run (no burn-in duration)"
        );
        let memory = rx.try_recv().expect("the `[m]` memory-bench cmd was queued");
        assert_eq!(
            (memory.target, memory.mode, memory.duration_minutes),
            (StreamTarget::Full, BenchMode::MemoryOnly, None),
            "`[m]` = a memory-only run"
        );
        let burn = rx.try_recv().expect("the `[x]` burn-in cmd was queued");
        assert_eq!(
            (burn.target, burn.mode, burn.duration_minutes),
            (StreamTarget::Full, BenchMode::Full, Some(5)),
            "`[x]` = the GUI default 5-minute burn-in"
        );
        assert!(rx.try_recv().is_err(), "exactly three commands were queued");
    }

    /// (bp) The dispatch over a scripted `KeyEvent` stream (the
    /// plan's TUI-22 test — the pure `key_to_action` +
    /// `dispatch_action` split makes the whole key `match`
    /// unit-testable without a TTY): the 16-key sequence (the
    /// frozen table order, `q` last) + one ignored char, mirroring
    /// the main loop (the auto-open rule applies before each
    /// dispatch) over a daemon-less default state — the side
    /// effects come out in order (the state-only keys `None`, the
    /// `[e]`/`[s]`/`[r]` keys their I/O effect, `[q]` the quit),
    /// the settings land in their post-sequence state (one toggle /
    /// one cycle of each knob), the three run keys queue exactly
    /// their `TuiBenchCmd`s (the state stays idle — no run is in
    /// flight in this test), the cancel flag ends set, and the
    /// `[d]` key dismisses the auto-open (the strip had been
    /// auto-opened before the key landed).
    #[test]
    fn dispatch_over_a_scripted_key_stream() {
        let (tx, rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(AppState::default()));
        let mut dismissed = false;

        /// One main-loop iteration over one key: the auto-open
        /// rule, the dispatch under the write scope, the observed
        /// side effect (`None` for an ignored char — it never
        /// reaches the dispatch).
        fn step(
            state: &RwLock<AppState>,
            key: char,
            dismissed: &mut bool,
            tx: &mpsc::Sender<TuiBenchCmd>,
            cancel: &AtomicBool,
        ) -> Option<SideEffect> {
            let action =
                key_to_action(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE))?;
            apply_requirements_auto_open(state, *dismissed);
            Some({
                let mut s = state.write().unwrap();
                dispatch_action(action, &mut s, dismissed, tx, cancel)
            })
        }

        let keys = ['g', 'p', 'u', 'k', 'a', 'w', 't', 'd', 'b', 'm', 'x', 'c', 'e', 's', 'r', 'z', 'q'];
        let mut sides = Vec::new();
        for key in keys {
            sides.push(step(&state, key, &mut dismissed, &tx, &cancel));
        }
        assert_eq!(
            sides,
            vec![
                // g p u k a w t d b m x c: the state-only keys
                // (`Some(SideEffect::None)` — they dispatched, no I/O)
                Some(SideEffect::None), Some(SideEffect::None), Some(SideEffect::None),
                Some(SideEffect::None), Some(SideEffect::None), Some(SideEffect::None),
                Some(SideEffect::None), Some(SideEffect::None),
                Some(SideEffect::None), Some(SideEffect::None), Some(SideEffect::None),
                Some(SideEffect::None),
                // e s r: the I/O keys report their effect
                Some(SideEffect::Export),
                Some(SideEffect::Snapshot),
                Some(SideEffect::Refresh),
                // z: ignored — never dispatched
                None,
                // q: the quit
                Some(SideEffect::Quit),
            ],
            "the side effects come out in the scripted order"
        );

        let s = state.read().unwrap();
        assert!(s.settings.graphs_open, "one `[g]` opens the graphs overlay");
        assert_eq!(s.settings.poll_interval_ms, 5_000, "one `[p]` advances 2 s → 5 s");
        assert!(!s.settings.capacity_gib, "one `[u]` flips GiB → GB");
        assert!(!s.settings.clock_mhz, "one `[k]` flips MHz → GHz");
        assert!(!s.settings.refresh, "one `[a]` freezes the periodic poll");
        assert_eq!(s.settings.graph_window_min, 15, "one `[w]` advances 5 → 15 min");
        assert!(s.settings.settings_open, "one `[t]` opens the settings strip");
        assert!(
            !s.settings.requirements_open,
            "the `[d]` closed the strip the auto-open had opened"
        );
        assert!(dismissed, "the `[d]` close dismissed the auto-open (the latch)");

        // The three run keys queued exactly their commands (in
        // order) — the state stayed idle, so none was skipped.
        let full = rx.try_recv().expect("the `[b]` full-bench cmd was queued");
        assert_eq!(
            (full.target, full.mode, full.duration_minutes),
            (StreamTarget::Full, BenchMode::Full, None)
        );
        let memory = rx.try_recv().expect("the `[m]` memory-bench cmd was queued");
        assert_eq!(
            (memory.target, memory.mode, memory.duration_minutes),
            (StreamTarget::Full, BenchMode::MemoryOnly, None)
        );
        let burn = rx.try_recv().expect("the `[x]` burn-in cmd was queued");
        assert_eq!(
            (burn.target, burn.mode, burn.duration_minutes),
            (StreamTarget::Full, BenchMode::Full, Some(5))
        );
        assert!(rx.try_recv().is_err(), "exactly three commands were queued");
        assert!(cancel.load(Ordering::Relaxed), "the `[c]` key set the shared cancel flag");
    }

    /// (bq) Every one of the 16 actions dispatches a defined effect
    /// (the no-op placeholder arms are gone — the plan's exit
    /// criterion): the three I/O actions report their side effect,
    /// `[Q]`uit reports the break, and the ten state-only actions
    /// (the five toggles, the two cycles, the three run keys, the
    /// cancel) mutate the state and report `SideEffect::None`.
    #[test]
    fn every_action_dispatches_a_defined_effect() {
        let (tx, _rx) = mpsc::channel::<TuiBenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut dismissed = false;
        let mut state = AppState::default();

        let expected = [
            (Action::Refresh, SideEffect::Refresh),
            (Action::Snapshot, SideEffect::Snapshot),
            (Action::Quit, SideEffect::Quit),
            (Action::BenchFull, SideEffect::None),
            (Action::BenchMemory, SideEffect::None),
            (Action::BurnIn, SideEffect::None),
            (Action::Cancel, SideEffect::None),
            (Action::ToggleGraphs, SideEffect::None),
            (Action::ToggleSettings, SideEffect::None),
            (Action::ToggleRequirements, SideEffect::None),
            (Action::ExportJson, SideEffect::Export),
            (Action::CyclePoll, SideEffect::None),
            (Action::ToggleCapacity, SideEffect::None),
            (Action::ToggleClock, SideEffect::None),
            (Action::ToggleRefresh, SideEffect::None),
            (Action::CycleWindow, SideEffect::None),
        ];
        assert_eq!(expected.len(), 16, "the full frozen table (P3-22 + TUI-01/02)");
        for (action, side_effect) in expected {
            let side = dispatch_action(action, &mut state, &mut dismissed, &tx, &cancel);
            assert_eq!(side, side_effect, "{action:?} must dispatch its defined effect");
        }
    }

    /// (br) The USAGE text carries the full 16-key table (plan
    /// §2.2) + the unchanged exit-code note — the `--help`-style
    /// surface of the parity dashboard.
    #[test]
    fn usage_text_carries_the_full_key_table() {
        for entry in [
            "[R]efresh",
            "[S]napshot",
            "[Q]uit",
            "[B]ench (full)",
            "[M]emory",
            "[X] burn-in",
            "[C]ancel",
            "[E]xport",
            "[G]raphs",
            "[T]settings",
            "[D]requirements",
            "[P]oll",
            "[U]nits",
            "[K]lock",
            "[A]uto refresh",
            "[W]indow",
        ] {
            assert!(USAGE.contains(entry), "the USAGE must list {entry}");
        }
        assert!(
            USAGE.contains("Exit codes: 0 quit, 1 terminal init failure, 2 usage error"),
            "the exit-code note is unchanged"
        );
    }

}
