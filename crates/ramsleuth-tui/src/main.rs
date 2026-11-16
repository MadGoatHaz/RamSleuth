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

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crossterm::event::Event;
use ramsleuth_protocol::cli_common::{short_circuit, version_line, ShortCircuit, USAGE_COMMON};
use ramsleuth_tui::events;
use ramsleuth_tui::probe::{copy_to_clipboard, probe_report_default_path};
use ramsleuth_tui::{key_to_action, render, AppState};
use ratatui::layout::Rect;

mod bench_worker;
mod cli;
mod dispatch;
mod export;
mod polling;
mod probe_overlay;
mod terminal;
mod updater;

use cli::{parse_args, TuiArgs, USAGE};
use dispatch::{apply_requirements_auto_open, dispatch_action, SideEffect};
use export::{write_export, write_snapshot};
use polling::poll_once;
use probe_overlay::{handle_probe_overlay_key, probe_report_fetch};
use terminal::TerminalGuard;
use updater::{join_updater_bounded, spawn_updater};

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
