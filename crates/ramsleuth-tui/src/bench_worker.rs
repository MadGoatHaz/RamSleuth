//! Benchmark / burn-in workers (extracted from `main.rs`).
//!
//! [`TuiBenchCmd`] is one benchmark request from the key handler to the
//! background updater. [`run_bench`] (a normal single-pass benchmark)
//! and [`run_burn_in`] (a burn-in with a duration) each connect to the
//! daemon, send the start request, and drain the reply stream into the
//! shared state — locking only briefly, per mutation (C14-03), so the
//! render thread stays responsive. The no-panic contract (D5): every
//! failure class is recorded in `AppState`, never a panic.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::Instant;

use ramsleuth_bench::{BenchOp, BurnInTick, StreamTarget};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response};
use ramsleuth_tui::AppState;

use super::{state_read, state_write, BENCH_READ_TIMEOUT};

/// One benchmark request from the key handler to the background
/// updater (the TUI-local mirror of the GUI `update.rs` `BenchCmd`):
/// the cell target + the scope + the run-class discriminator —
/// `None` = a normal single-pass benchmark ([`run_bench`]);
/// `Some(n)` = a burn-in with a duration of `n` minutes
/// ([`run_burn_in`]).
#[derive(Debug, Clone, Copy)]
pub struct TuiBenchCmd {
    /// Which cells run (`Full` / one `Tier` / one `Cell`).
    pub target: StreamTarget,
    /// The run scope (the daemon clamps it onto the target, P3-15).
    pub mode: BenchMode,
    /// The run-class discriminator: `None` = a normal run
    /// ([`run_bench`]); `Some(n)` = a burn-in with a duration of
    /// `n` minutes.
    pub duration_minutes: Option<u32>,
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
pub fn run_bench(
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
pub fn run_burn_in(
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
