//! Background updater thread (extracted from `main.rs`).
//!
//! [`spawn_updater`] is the GUI-style poller loop adapted to the TUI's
//! fixed socket: each tick it re-reads the live settings from the shared
//! state, services at most one bench command from the key handler's
//! channel (dispatching to [`run_bench`] or [`run_burn_in`]), and
//! otherwise probes / fetches telemetry (`[`poll_with_client`]`).
//! [`join_updater_bounded`] waits for the thread to finish within a
//! deadline after quit, detaching it if the deadline is exceeded.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use ramsleuth_client::Client;
use ramsleuth_tui::AppState;

use super::{clamp_poll_interval, state_read, state_write, JOIN_DEADLINE, JOIN_POLL};
use crate::bench_worker::{run_bench, run_burn_in, TuiBenchCmd};
use crate::polling::poll_with_client;

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
pub fn spawn_updater(
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
pub fn join_updater_bounded(updater: JoinHandle<()>) {
    let deadline = Instant::now() + JOIN_DEADLINE;
    while !updater.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(JOIN_POLL);
    }
    let _ = updater.join();
}
