//! Telemetry polling (extracted from `main.rs`).
//!
//! The single unit both the background updater thread and the `[R]`efresh
//! key share: [`poll_once`] (connect + fetch) and its internals
//! ([`poll_with_client`], the graphs sample recorder, and the compound
//! [`run_in_flight`] single-flight guard). The no-panic contract (D5):
//! every failure class is recorded in `AppState`, never a panic.

use std::path::Path;
use std::time::Instant;

use ramsleuth_bench::{BenchOp, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::{Request, Response};
use ramsleuth_tui::{AppState, BenchState};

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
pub fn poll_with_client(client: &mut Client, socket: &Path, state: &mut AppState) {
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
pub fn latest_memory_read_bw(bench: &BenchState) -> f64 {
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
pub fn run_in_flight(bench: &BenchState) -> bool {
    bench.running || bench.burn_in.running
}
