//! The GUI's shared state + the background poller thread (P3-26, plan D6).
//!
//! The render thread (the eframe app shell, P3-30) never does I/O: it
//! reads snapshots of [`TelemetryData`] out of the `Arc<RwLock<…>>` that
//! [`spawn_poller`] hands it, and a single background thread is the only
//! place the GUI talks to the daemon (plan D6: no render-thread blocking
//! — 60 FPS stays feasible). That thread:
//!
//! - performs exactly one **baseline** [`poll_telemetry`] fetch per
//!   distinct `settings.socket` value (D-8 — the one-shot startup
//!   snapshot, independent of the `refresh_enabled` gate; a socket
//!   edit re-triggers exactly one fetch for the new socket, and a
//!   failed baseline never retries on its own — the next re-trigger
//!   is a socket change or the refresh cadence);
//! - at the live settings cadence (default 2 s — the
//!   `settings.poll_interval_ms` knob, C6-27, while
//!   `refresh_enabled` is on) runs one
//!   [`poll_telemetry`] cycle against the live settings socket
//!   (`settings.socket` — seeded from the CLI `--socket` by the app
//!   shell; a panel edit retargets the next cycle, C6-30): a fresh
//!   [`Client::connect`] + `GetTelemetry` — a new connection each
//!   cycle survives a daemon restart, the TUI P3-24 precedent —
//!   appending one trend-history sample to `state.history` per
//!   successful poll (the Na-guarded [`record_history_sample`] —
//!   C6-25) + one graphs-window sample to `state.graph` (the
//!   Na-guarded [`graph::record_graph_sample`] — C7-20, the D-4 /
//!   D-5 sources) and clearing the series when the daemon
//!   reconnects;
//! - serves benchmark requests from the [`BenchCmd`] channel (the app
//!   shell's bench-zone run buttons, P3-28) against the same live
//!   settings socket (C6-30), dispatching on the cmd's
//!   `duration_minutes` discriminator (C7-16): a normal run (`None`)
//!   goes to [`run_bench`] — one `StartBenchmark` send, then the
//!   reply stream (a `BenchStarted` ack, the `BenchProgress` events,
//!   and exactly one terminal) into `state.bench`; a burn-in run
//!   (`Some`) goes to [`run_burn_in`] — one `StartBurnIn` send, then
//!   the reply stream (a `BenchStarted` ack, the `BurnInProgress`
//!   ticks, and exactly one terminal) into `state.bench` (the
//!   `burn_in` state + the terminal grid);
//! - watches the shared `AtomicBool` cancel flag (the bench zone's
//!   Cancel button, P3-28): [`run_bench`] / [`run_burn_in`] check it
//!   before each frame — once set, the run is stopped daemon-side
//!   with a best-effort `CancelBenchmark` and ends with
//!   `running` / `burn_in.running = false`;
//! - re-reads the live settings knobs (`poll_interval_ms` +
//!   `refresh_enabled` every tick — C6-27, the `socket` per poll /
//!   bench cycle — C6-30) — a changed knob takes effect without a
//!   poller restart; a disabled refresh idles the loop after the
//!   one-shot baseline per distinct socket (no continuous polling);
//! - ticks every 200 ms so an in-flight run's progress frames stay
//!   responsive, and stops on the shared `AtomicBool` (or when the
//!   channel disconnects).
//!
//! **No-panic contract (plan D5):** [`poll_telemetry`] takes
//! `&mut TelemetryData` (no thread, no lock — testable in
//! isolation), and [`run_bench`] / [`run_burn_in`] take the shared
//! `&RwLock<TelemetryData>` — the lock is held only briefly, per
//! mutation, and is always released before the next stream `recv()`
//! (C14-03: the render thread’s per-frame reads — and the
//! Graphs child frame’s per-frame write — never park across a
//! long run’s drain, so both viewports stay responsive during a
//! run). All three **always return `Ok(())`**: every
//! failure class
//! (a missing / refused socket → the `ClientError::DaemonDown`
//! "start it with …" text, a read timeout, a protocol violation, a
//! closed stream) is recorded in the state (`error` +
//! `daemon_status`) instead of propagating, so the caller's loop keeps
//! running against a flapping daemon. The `RwLock`'s data fields are
//! written from that one background thread only; the one
//! render-thread write is the settings panel mutating
//! `state.settings` (no I/O, D6 — C6-27), which the poller re-reads
//! every tick (including the socket knob, C6-30).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ramsleuth_bench::{BenchOp, BenchmarkGrid, BurnInTick, StreamProgress, StreamTarget, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response};
use ramsleuth_telemetry::{ProbeReport, SystemMemoryTelemetry};

use crate::graph::GraphState;
use crate::history::HistoryState;
use crate::settings::{DEFAULT_POLL_INTERVAL_MS, GuiSettings};

/// Background-loop tick: short enough to keep an in-flight bench run's
/// progress frames responsive, cheap when idle.
const POLLER_TICK: Duration = Duration::from_millis(200);
/// Sane lower bound of the poll interval — the settings panel's
/// DragValue range floor, re-clamped defensively by the poller (a
/// degenerate stored knob can never make the loop hot-spin, D5).
const MIN_POLL_INTERVAL_MS: u64 = 100;
/// Sane upper bound of the poll interval — the settings panel's
/// DragValue range ceiling (60 s), re-clamped defensively by the
/// poller (an absurd stored knob can never stall the loop, D5).
const MAX_POLL_INTERVAL_MS: u64 = 60_000;
/// Read timeout for the bench / burn-in streams (C7-16): 120 s
/// *between frames* — a run streams its progress (the
/// `BenchProgress` events or the `BurnInProgress` ticks) over
/// minutes, so a legitimate gap between frames can far exceed the
/// client's 5 s transport default (the CLI precedent,
/// `client/src/main.rs`'s `BENCH_READ_TIMEOUT`); the deadline only
/// bounds a silently wedged daemon.
const BENCH_READ_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------
// Shared state (the render thread reads only; one writer: the poller).
// ---------------------------------------------------------------------

/// The live benchmark state: a run in flight, its streamed progress
/// events, the terminal result grid, and the live burn-in state
/// (C7-16).
#[derive(Debug, Clone, Default)]
pub struct BenchState {
    /// A benchmark run is in flight (the status zone + the bench
    /// zone's progress bar key off this).
    pub running: bool,
    /// The in-flight run's daemon-assigned id (the `BenchStarted`
    /// ack; `None` between runs — the Cancel button addresses the run
    /// with it, P3-28).
    pub run_id: Option<u64>,
    /// Streamed [`StreamProgress`] events of the current / last run
    /// (cleared when a new run starts).
    pub progress: Vec<StreamProgress>,
    /// The terminal result grid of the last completed run (`None`
    /// until the first `BenchResult` — a burn-in's terminal lands
    /// here too: its last completed pass, C7-16).
    pub grid: Option<BenchmarkGrid>,
    /// The live burn-in state (C7-16): a burn-in in flight, the
    /// newest tick's iteration / elapsed, and the newest per-cell
    /// values seen this burn-in (`latest`).
    pub burn_in: BurnInState,
}

/// The live burn-in state (C7-16): a burn-in run in flight, the
/// newest tick's iteration / elapsed, and the newest per-cell values
/// seen this burn-in.
///
/// `latest` accumulates each streamed `BurnInProgress` tick the same
/// way the bench zone's `live_grid` accumulates `StreamProgress`
/// events (the newest value per cell wins, a non-finite / non-positive
/// reading never renders as data), except a burn-in also streams
/// per-tier latency ticks (written into the `latency_ns` column). The
/// 4×4 table keeps showing the *terminal* grid
/// ([`BenchState::grid`]) during a run; the burn-in row shows
/// `latest` (C7-18).
#[derive(Debug, Clone)]
pub struct BurnInState {
    /// A burn-in run is in flight (the status / controls key off this
    /// alongside [`BenchState::running`]).
    pub running: bool,
    /// The 1-based iteration of the newest tick seen this run (0
    /// before the first tick).
    pub iteration: u32,
    /// The run elapsed, in seconds, from the newest tick (0.0 before
    /// the first tick).
    pub elapsed_secs: f64,
    /// The newest per-cell values seen this burn-in (the `live_grid`
    /// rule over the ticks: the newest value per cell wins).
    pub latest: BenchmarkGrid,
}

impl Default for BurnInState {
    fn default() -> Self {
        Self {
            running: false,
            iteration: 0,
            elapsed_secs: 0.0,
            // A zero grid: unmeasured cells stay 0.0 (the `live_grid`
            // form — the bench zone renders them `N/A`).
            latest: BenchmarkGrid {
                read_gbps: [0.0; 4],
                write_gbps: [0.0; 4],
                copy_gbps: [0.0; 4],
                latency_ns: [0.0; 4],
            },
        }
    }
}

/// The GUI's presentation state: the current telemetry snapshot, the
/// bench state, the daemon connection status, the 10-minute
/// trend history (C6-25), the Graphs-window series (C7-20), and the
/// in-memory settings knobs (C6-26 / C6-27).
///
/// One `TelemetryData` lives behind an `Arc<RwLock<…>>` shared with the
/// render thread (P3-30); the background poller is the only writer
/// of the data fields, and the render thread's one permitted write
/// is the settings panel mutating the `settings` knobs (no I/O,
/// D6 — C6-27), which the poller re-reads every tick.
#[derive(Debug, Clone, Default)]
pub struct TelemetryData {
    /// The latest telemetry snapshot (`None` until the first successful
    /// poll; the stale snapshot is kept while the daemon is down — the
    /// zones render it with the `disconnected` status).
    pub telemetry: Option<SystemMemoryTelemetry>,
    /// The live benchmark state (run flag, progress events, result grid).
    pub bench: BenchState,
    /// The human-readable daemon status line for the status zone:
    /// `connected: <socket>` on success, `disconnected` on a transport
    /// failure (the zones color it).
    pub daemon_status: String,
    /// When the last successful telemetry poll landed (`None` until
    /// then — the status zone's "…s ago" clock).
    pub last_update: Option<Instant>,
    /// The structured error of the last failed poll / run (the
    /// `ClientError` text, or the daemon's wire `Error` message);
    /// cleared by the next success. `None` = healthy.
    pub error: Option<String>,
    /// The 10-minute trend series (C6-24 / C6-25): MCLK (MHz),
    /// VDDCR_SOC (mV), and the memory-read bandwidth (GB/s). The
    /// background poller is the only writer (D6): it appends one
    /// sample per successful poll (the Na-guarded
    /// [`record_history_sample`]) and clears the series on a daemon
    /// reconnect; the render thread reads it for
    /// [`crate::history::render_history`].
    pub history: HistoryState,
    /// The Graphs-window series (C7-20, the D-3 / D-4 / D-5
    /// sources): one timestamped sample per successful poll — CPU
    /// core frequency (MHz, `platform.cpu_clock_mhz`), VDDCR_SOC
    /// (mV, the AMD readout), the CPU temperature (°C — the runtime
    /// [`graph::read_cpu_temp_c`] thermal-zone scan), and the
    /// memory-read bandwidth (GB/s — the D-5 bench / burn-in step
    /// series) — in the 1800-deep ring (60 min at the 2 s poll).
    /// The background poller is the only writer (D6): it appends the
    /// Na-guarded [`graph::record_graph_sample`] per poll; the
    /// render thread + the Graphs window (C7-21) read it.
    pub graph: GraphState,
    /// The in-memory settings knobs (C6-26 / C6-30): the poll
    /// cadence (`poll_interval_ms`, default 2 s) + the refresh gate
    /// (`refresh_enabled`) the poller re-reads every tick (C6-27),
    /// the daemon socket (`socket` — seeded from the CLI `--socket`
    /// by the app shell; the poller re-reads it live per poll /
    /// bench cycle, C6-30), and the units / theme knobs the settings
    /// panel edits. The render thread's one permitted write is the
    /// settings panel mutating these knobs (no I/O, D6); the poller
    /// is the only writer of every other field.
    pub settings: GuiSettings,
    /// The consent-gated "Submit Probe Report" request edge (chunk
    /// probe-3): the consent dialog's [Allow] button sets it (the
    /// render thread's one probe write — no I/O, D6); the poller
    /// clears it and runs [`request_probe_report`] once per tick.
    pub probe_requested: bool,
    /// The probe-report request outcome (chunk probe-3): the poller
    /// writes [`ProbeResult::Pending`] → `Ok(report)` / `Err(text)`
    /// (the only daemon I/O off the render thread, D6); the render
    /// thread consumes it (the consent → preview transition) and
    /// resets it to `None`.
    pub probe_result: ProbeResult,
}

/// One benchmark request from the UI to the background poller (the
/// bench zone's run buttons, P3-28): the cell target + the scope +
/// the run-class discriminator (C7-16).
#[derive(Debug, Clone, Copy)]
pub struct BenchCmd {
    /// Which cells run (`Full` / one `Tier` / one `Cell`).
    pub target: StreamTarget,
    /// The run scope (the daemon clamps it onto the target, P3-15).
    pub mode: BenchMode,
    /// The run-class discriminator (C7-16): `None` = a normal
    /// single-pass benchmark ([`run_bench`]); `Some(n)` = a burn-in
    /// ([`run_burn_in`]) with a duration of `n` minutes (`n = 0`
    /// infinite — stop only via the Cancel flag /
    /// [`Request::CancelBenchmark`]).
    pub duration_minutes: Option<u32>,
}

/// The consent-gated "Submit Probe Report" request outcome (chunk
/// probe-3): shared between the background poller (the only writer —
/// it does the daemon I/O off the render thread, D6) and the render
/// thread (the reader/consumer — the consent → preview transition).
///
/// - `None` — no probe request in flight / no result (the idle
///   default).
/// - `Pending` — a request is in flight (the consent dialog shows a
///   progress indicator).
/// - `Ok(report)` — the report landed (the render thread renders it
///   to markdown and moves to the preview).
/// - `Err(text)` — the request failed (the structured error text; the
///   render thread flashes it as a notice and returns to idle).
///
/// `ProbeReport` is not `Eq` (it embeds `f64` cells), so this derives
/// `PartialEq` only.
///
/// The same deliberate `large_enum_variant` allow as the protocol's
/// [`Response`]: the `Ok` arm carries the transient per-request
/// [`ProbeReport`] (the daemon's `GetProbeReport` reply, consumed
/// once by the render thread) — boxing it would gain nothing, since
/// the value is a short-lived one-shot, not a long-lived field.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProbeResult {
    #[default]
    None,
    Pending,
    Ok(ProbeReport),
    Err(String),
}

// ---------------------------------------------------------------------
// The two poll units (testable: no thread, always `Ok(())`).
// `poll_telemetry` takes `&mut TelemetryData` (no lock); the run
// units take the shared `&RwLock<TelemetryData>` and lock it only
// per mutation — never across a stream `recv()` (C14-03).
// ---------------------------------------------------------------------

/// One telemetry poll cycle: connect to the daemon at `socket`, request
/// the current snapshot (`GetTelemetry`), and update `state` — the
/// single unit the background poller runs at the live settings
/// cadence (default 2 s, C6-27).
///
/// Testable, no thread. The no-panic contract (plan D5): every failure
/// class (a missing / refused socket → the `ClientError::DaemonDown`
/// friendly "start it with …" text, a timeout, a protocol violation, an
/// i/o failure) is recorded in `state.error` with `daemon_status`
/// degrading to `disconnected`; a successful `Telemetry` arm stores the
/// snapshot, stamps `last_update`, reports `connected: <socket>`,
/// clears the error, and appends one trend-history sample (C6-25) —
/// clearing the series first when the daemon reconnected; a
/// structured `Error` reply records its message.
/// The function always returns `Ok(())` — errors are reported through
/// the state, not a `Result` error, so the caller's loop keeps running
/// against a flapping daemon.
pub fn poll_telemetry(socket: &Path, state: &mut TelemetryData) -> Result<(), String> {
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state.error = Some(error.to_string());
            state.daemon_status = "disconnected".to_owned();
            return Ok(());
        }
    };
    match client.request(&Request::GetTelemetry) {
        Ok(Response::Telemetry(telemetry)) => {
            // Reconnect prime (C6-25): the previous cycle recorded a
            // non-connected status (the first poll, or the daemon
            // came back after an outage) — clear the stale series so
            // the sparkline restarts cleanly instead of drawing a gap
            // across the outage.
            let reconnected = !state.daemon_status.starts_with("connected");
            state.telemetry = Some(telemetry);
            state.last_update = Some(Instant::now());
            state.daemon_status = format!("connected: {}", socket.display());
            state.error = None;
            if reconnected {
                state.history.clear();
            }
            record_history_sample(state);
            record_graph_sample(state);
        }
        Ok(Response::Error(message)) => {
            // The daemon was reachable but rejected the request (a
            // structured wire error, not a transport failure): record
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
    Ok(())
}

/// One consent-gated probe-report request (chunk probe-3): connect to
/// the daemon at `socket`, send `GetProbeReport`, and land the reply
/// into `state.probe_result` — the single daemon I/O unit the
/// background poller runs when the consent dialog's [Allow] edge sets
/// `probe_requested`.
///
/// Testable, no thread: it takes the shared `&RwLock<TelemetryData>`
/// and locks it only briefly, per mutation (C14-03). The no-panic
/// contract (plan D5), as in [`poll_telemetry`]: every failure class
/// (a missing / refused socket → the `ClientError::DaemonDown`
/// "start it with …" text, a timeout, a protocol violation, a closed
/// stream, a structured wire `Error`) is recorded in
/// `state.probe_result` as `Err(…)`, and the function always returns
/// `Ok(())` — the poller's loop keeps running against a flapping
/// daemon.
pub fn request_probe_report(socket: &Path, state: &RwLock<TelemetryData>) -> Result<(), String> {
    // The in-flight marker (the consent dialog's progress indicator).
    state.write().unwrap().probe_result = ProbeResult::Pending;
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state
                .write()
                .unwrap()
                .probe_result = ProbeResult::Err(error.to_string());
            return Ok(());
        }
    };
    match client.request(&Request::GetProbeReport) {
        Ok(Response::ProbeReport(report)) => {
            state.write().unwrap().probe_result = ProbeResult::Ok(report);
        }
        Ok(Response::Error(message)) => {
            state.write().unwrap().probe_result = ProbeResult::Err(message);
        }
        Ok(_) => {
            // Any other frame in reply to `GetProbeReport` violates
            // the wire contract (P3-16: the daemon answers one
            // request with exactly one response).
            state.write().unwrap().probe_result =
                ProbeResult::Err("unexpected response to GetProbeReport".to_owned());
        }
        Err(error) => {
            state.write().unwrap().probe_result = ProbeResult::Err(error.to_string());
        }
    }
    Ok(())
}

/// Append one trend-history sample (C6-25) for the just-landed
/// snapshot: MCLK (MHz) + VDDCR_SOC (mV) from the AMD clock /
/// voltage readout, and the bandwidth (GB/s) from
/// [`latest_memory_read_bw`].
///
/// The Na guard (the no-panic contract, D5): a sample is appended
/// only when the AMD branch carries both cells as values — an Intel
/// snapshot, a driver-missing / degraded AMD readout, or a
/// non-finite MCLK pushes nothing (the series gains no holes; the
/// sparkline simply holds its last shape). `vddcr_soc` is a `u16`, so
/// a present cell is always finite.
fn record_history_sample(state: &mut TelemetryData) {
    let Some(readout) = state.telemetry.as_ref().and_then(|t| t.amd.value()) else {
        return; // Intel silicon / the driver missing: no MCLK / VDDCR_SOC.
    };
    let Some(mclk_mhz) = readout
        .clocks
        .mclk_mhz
        .value()
        .copied()
        .filter(|v| v.is_finite())
    else {
        return; // MCLK absent / non-finite: skip the whole sample.
    };
    let Some(vddcr_soc_mv) = readout.voltages.vddcr_soc_mv.value().copied() else {
        return; // VDDCR_SOC absent: skip the whole sample.
    };
    state
        .history
        .push(mclk_mhz, f64::from(vddcr_soc_mv), latest_memory_read_bw(state));
}

/// Append one graphs-window sample (C7-20) for the just-landed
/// snapshot — the D-4 / D-5 source map:
///
/// - the CPU core frequency (MHz) from `platform.cpu_clock_mhz` and
///   the VDDCR_SOC (mV) from the AMD readout ride inside
///   [`graph::record_graph_sample`]'s Na guard (an absent /
///   non-finite source degrades its own field to NaN);
/// - the CPU temperature (°C) comes from the runtime thermal-zone
///   scan ([`graph::read_cpu_temp_c`] — poller-thread I/O only,
///   D6: never the render thread);
/// - the memory-read bandwidth (GB/s) comes from
///   [`latest_memory_read_bw`] (D-5: the newest streamed / burn-in
///   `Memory · Read` event, else the terminal grid's cell) — its
///   0.0 no-figure sentinel maps to NaN, so the row stays its
///   no-source note until the first bench / burn-in sample (D-4: a
///   flat 0 line would be a lie, 0 ≠ N/A).
///
/// The Na guard itself (the no-panic contract, D5): a sample lands
/// only when ≥ 1 field is finite — an all-NaN poll appends
/// nothing (the `record_history_sample` precedent).
fn record_graph_sample(state: &mut TelemetryData) {
    let bandwidth = latest_memory_read_bw(state);
    let bandwidth_gbps = if bandwidth > 0.0 { bandwidth } else { f64::NAN };
    crate::graph::record_graph_sample(
        &mut state.graph,
        &state.telemetry,
        crate::graph::read_cpu_temp_c(),
        bandwidth_gbps,
    );
}

/// The latest memory-read bandwidth figure (GB/s) for the history
/// series: the newest streamed `Memory · Read` progress event when it
/// carries a finite, positive value (a live run), else the terminal
/// grid's memory-read cell (row 0 — the [`Tier::Memory`] slot, the
/// C6-22 mapping), else `0.0` (no benchmark data yet — the row plots
/// zero rather than a hole). Non-finite / non-positive values never
/// count as a figure (the bench zone's live-cell rule).
fn latest_memory_read_bw(state: &TelemetryData) -> f64 {
    if let Some(event) = state
        .bench
        .progress
        .iter()
        .rev()
        .find(|e| e.tier == Tier::Memory && e.op == BenchOp::Read && e.value.is_finite() && e.value > 0.0)
    {
        return event.value;
    }
    if let Some(grid) = &state.bench.grid {
        let value = grid.read_gbps[0];
        if value.is_finite() && value > 0.0 {
            return value;
        }
    }
    0.0
}

/// One benchmark run: connect to the daemon at `socket`, send the
/// `StartBenchmark` for `cmd`, and drain the reply stream — a
/// `BenchStarted` ack, the `BenchProgress` events, and exactly one
/// terminal — into `state.bench`.
///
/// Testable, no thread: it takes the shared
/// `&RwLock<TelemetryData>` and locks it only briefly, per mutation —
/// the guard is always released before the next stream `recv()`
/// (C14-03: the render thread's reads never park across the drain,
/// so the viewports stay responsive during a long run). The run
/// starts with `running = true`, the
/// progress list cleared, and a stale `run_id` dropped (a stale run's
/// events never mix into a new one); the terminal frame (`BenchResult`
/// → the grid, `BenchCancelled`, or the daemon's `Error`) sets
/// `running = false`; a transport failure (a closed stream, a timeout,
/// …) or a contract-violating frame (a `Telemetry` or `BurnInProgress`
/// frame in this stream) records `state.error` and the same. Always
/// returns `Ok(())` (the no-panic contract, as in
/// [`poll_telemetry`]) — the poller loop must survive every failure.
///
/// **Cancel (P3-28):** `cancel` is the flag the bench zone's Cancel
/// button sets (shared with the poller). It is reset to `false` at the
/// start of every run (a stale cancel never kills a new one) and
/// checked before each `recv()`: once set, the run is stopped
/// daemon-side with a best-effort `CancelBenchmark` for the current
/// `run_id` (only once the `BenchStarted` ack has landed — the
/// daemon's reply is never read) and the loop breaks with
/// `running = false` (the clean stop, plan D6: the in-flight pass
/// finishes, the run ends at the next gate).
pub fn run_bench(
    socket: &Path,
    cmd: BenchCmd,
    state: &RwLock<TelemetryData>,
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
        // The Cancel button set the shared flag: ask the daemon for a
        // clean stop (best-effort — the reply is never read) and break
        // before the next frame.
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
                state.write().unwrap().bench.run_id = Some(run_id);
            }
            Ok(Response::BenchProgress(progress)) => {
                state.write().unwrap().bench.progress.push(progress);
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal frame: the grid + the clean stop in one
                // brief scope.
                let mut s = state.write().unwrap();
                s.bench.grid = Some(grid);
                s.bench.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                state.write().unwrap().bench.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
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
                // owning `StartBurnIn` connection, D-1/D-2): stop the
                // run and record it — the permanent mirror of
                // `run_burn_in`'s `BenchProgress` contract guard
                // (C7-16 landed the real burn-in consumption).
                let mut s = state.write().unwrap();
                s.error =
                    Some("unexpected burn-in frame during benchmark".to_owned());
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

/// One burn-in run (C7-16): connect to the daemon at `socket`, send
/// the `StartBurnIn` for `cmd`, and drain the reply stream — a
/// `BenchStarted` ack, the `BurnInProgress` ticks, and exactly one
/// terminal — into `state.bench` (the `burn_in` state + the terminal
/// grid).
///
/// Testable, no thread: it takes the shared
/// `&RwLock<TelemetryData>` and locks it only briefly, per mutation —
/// the guard is always released before the next stream `recv()`
/// (C14-03: the render thread's reads never park across the drain,
/// so the viewports stay responsive during a long burn-in). The run
/// starts with `burn_in.running = true`,
/// a fresh zero `latest` grid + zeroed iteration / elapsed (a stale
/// run's values never mix into a new one), and a stale `run_id`
/// dropped (the new run's id arrives with the `BenchStarted` ack);
/// each tick updates the newest iteration / elapsed and writes its
/// cell / latency into `latest` (the newest value per cell wins — the
/// bench zone's `live_grid` rule); the terminal frame (`BenchResult`
/// → the grid, `BenchCancelled`, or the daemon's `Error`) sets
/// `burn_in.running = false`. The run never touches
/// `BenchState::running` / `progress` (those are the normal-bench
/// state; the two run classes share only the `run_id` + the cancel
/// flag).
///
/// **Cancel (P3-28, the brief's "stop via existing Cancel"):** the
/// shared `cancel` flag is reset to `false` at the start of every run
/// (a stale cancel never kills a new one) and checked before each
/// `recv()`: once set, the run is stopped daemon-side with a
/// best-effort `CancelBenchmark` for the current `run_id` (only once
/// the `BenchStarted` ack has landed — the daemon's reply is never
/// read) and the loop breaks with `burn_in.running = false` (the
/// clean stop, plan D6: the in-flight pass finishes, the run ends at
/// the next gate).
///
/// A contract-violating frame (a `BenchProgress` or `Telemetry` frame
/// in this stream) or a transport failure (a closed stream, a
/// timeout, …) records `state.error` and ends the run the same way.
/// Always returns `Ok(())` (the no-panic contract, as in
/// [`poll_telemetry`]) — the poller loop must survive every failure.
pub fn run_burn_in(
    socket: &Path,
    cmd: BenchCmd,
    state: &RwLock<TelemetryData>,
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
    // The stream read timeout (C7-16): the client's 5 s default would
    // kill a long burn-in's frame gap mid-stream (the 120 s CLI
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
        s.bench.burn_in = BurnInState::default();
        s.bench.burn_in.running = true;
        // The new run's id arrives with the `BenchStarted` ack.
        s.bench.run_id = None;
    }
    // The poller dispatches `duration_minutes.is_some()` to this
    // path; a `None` cmd reaching it is a dispatch contract violation
    // (never a normal-bench run): record it and stop — no panic (D5).
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
        // The Cancel button set the shared flag: ask the daemon for a
        // clean stop (best-effort — the reply is never read) and break
        // before the next frame.
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
                state.write().unwrap().bench.run_id = Some(run_id);
            }
            Ok(Response::BurnInProgress(tick)) => {
                // The tick's bookkeeping lands in the burn-in state
                // under one brief write scope: the newest iteration /
                // elapsed, and its cell / latency written into
                // `latest` (the newest value per cell wins — the
                // `live_grid` rule; a non-finite / non-positive
                // reading never renders as data).
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
                // violates the wire contract (`BenchProgress` streams
                // only on the owning `StartBenchmark` connection,
                // D-1/D-2): record the structured error and stop the
                // run — the permanent mirror of `run_bench`'s
                // `BurnInProgress` contract guard.
                let mut s = state.write().unwrap();
                s.error = Some("unexpected benchmark frame during burn-in".to_owned());
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchResult { grid, .. }) => {
                // The terminal grid is the last completed pass: the
                // 4×4 table + the F2/F3 exports pick it up for free.
                let mut s = state.write().unwrap();
                s.bench.grid = Some(grid);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                state.write().unwrap().bench.burn_in.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                let mut s = state.write().unwrap();
                s.error = Some(message);
                s.bench.burn_in.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the burn-in stream violates the
                // wire contract (`GetTelemetry` is served on its own
                // connection, P3-16): stop the run and record it.
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

// ---------------------------------------------------------------------
// The background poller thread (the data fields' only writer — the
// settings knobs' one render-thread write is the exception, C6-27).
// ---------------------------------------------------------------------

/// Clamp a configured poll interval (milliseconds) to the sane range
/// [`MIN_POLL_INTERVAL_MS`] ..= [`MAX_POLL_INTERVAL_MS`] (the
/// settings panel's DragValue bounds, re-applied defensively): a zero
/// / absurd stored knob can never make the loop hot-spin or stall —
/// the no-panic contract (D5).
fn clamp_poll_interval(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS))
}

/// Spawn the background poller thread behind `state` (the plan D6
/// contract: the render thread only ever reads
/// `Arc<RwLock<TelemetryData>>` — the one exception is the settings
/// panel mutating `state.settings`, no I/O, C6-27).
///
/// The loop: check `stop`; service at most one [`BenchCmd`] from
/// `bench_rx` (a run streams to its terminal before the next tick —
/// runs are single-flight daemon-side anyway, P3-15) against the
/// live settings socket (C6-30), dispatching on the cmd's
/// `duration_minutes` discriminator to [`run_bench`] (a normal run)
/// or [`run_burn_in`] (a burn-in, C7-16), and handing the shared
/// `cancel` flag to it (the bench zone's Cancel button sets it,
/// P3-28; each run resets it per run); otherwise re-read the live
/// settings knobs (C6-27 / C6-30) — the clamped `poll_interval_ms` +
/// the `refresh_enabled` gate + the `socket` — and: (a) run exactly
/// one **baseline** [`poll_telemetry`] fetch for a `socket` value the
/// thread-local `baseline_for` stamp has not seen yet (D-8 —
/// regardless of `refresh_enabled`, the one-shot startup snapshot; a
/// socket edit re-triggers exactly one fetch for the new socket, and
/// a failed baseline never retries on its own — no retry storm: the
/// stamp is set after the fetch, which cannot panic (D5), so the
/// next re-trigger is a socket change or the refresh cadence); it
/// stamps the due clock, so the (b) cadence starts a full interval
/// after it; (b) else, when the `refresh_enabled` gate is on and the
/// last poll is ≥ the clamped interval old (the thread-local
/// `last_poll` stamp, so a flapping daemon polls on the configured
/// cadence instead of every tick; a disabled refresh idles the loop,
/// and a changed knob takes effect on the next tick — no restart),
/// poll telemetry; tick [`POLLER_TICK`] (200 ms, so bench progress
/// stays responsive); exit when `stop` is set or the channel
/// disconnects. The `RwLock`'s data fields are written from this
/// thread only, so the `unwrap` is the workspace's one-writer
/// precedent (TUI P3-24).
pub fn spawn_poller(
    state: Arc<RwLock<TelemetryData>>,
    bench_rx: Receiver<BenchCmd>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Primed due at the default cadence: with the refresh gate
        // on, the first continuous poll runs at once (the live knob
        // read below governs every subsequent due-check — C6-27);
        // with it off the stamp only matters after a later
        // refresh-enable (the baseline fetch is due-gate-independent
        // — D-8).
        let mut last_poll =
            Instant::now() - Duration::from_millis(DEFAULT_POLL_INTERVAL_MS);
        // Baseline-once (D-8): the last `settings.socket` value a
        // one-shot baseline fetch ran against (`None` before the
        // first tick). A distinct value triggers exactly one
        // [`poll_telemetry`] regardless of `refresh_enabled`; the
        // stamp is set after the fetch (which cannot panic, D5), so
        // a failed baseline never retries on its own (no retry storm
        // — the next re-trigger is a socket change or the refresh
        // cadence).
        let mut baseline_for: Option<String> = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // The consent-gated probe-report request (chunk probe-3):
            // the consent dialog's [Allow] edge sets `probe_requested`
            // (the render thread's one probe write, no I/O — D6); the
            // poller is the only place the GUI talks to the daemon, so
            // it clears the flag and runs the one-shot request here,
            // before the bench dispatch (a probe never blocks a run).
            let (probe_requested, socket) = {
                let s = state.read().unwrap();
                (s.probe_requested, s.settings.socket.clone())
            };
            if probe_requested {
                state.write().unwrap().probe_requested = false;
                let _ = request_probe_report(Path::new(&socket), &state);
            }
            match bench_rx.try_recv() {
                Ok(cmd) => {
                    // The live settings socket (C6-30): a panel edit
                    // retargets the next run (the fresh connection
                    // per cycle rides the change — D6).
                    let socket = state.read().unwrap().settings.socket.clone();
                    // The run-class dispatch (C7-16): the cmd's
                    // `duration_minutes` is the discriminator —
                    // `None` routes to `run_bench` (a normal run),
                    // `Some` to `run_burn_in` (a burn-in); both share
                    // the same socket read, the same cancel-flag
                    // hand-off, and the same `Ok(())`-always
                    // contract.
                    if cmd.duration_minutes.is_some() {
                        // The run locks `state` only briefly, per
                        // mutation — never across its stream drain
                        // (C14-03: the render thread's reads stay
                        // responsive for the whole run).
                        let _ = run_burn_in(Path::new(&socket), cmd, &state, &cancel);
                    } else {
                        let _ = run_bench(Path::new(&socket), cmd, &state, &cancel);
                    }
                }
                Err(TryRecvError::Empty) => {
                    // The live settings knobs (C6-27 / C6-30): re-read
                    // every tick — a changed interval (clamped), the
                    // refresh gate, or the socket takes effect
                    // without a restart.
                    let (interval, refresh_enabled, socket) = {
                        let settings = state.read().unwrap();
                        (
                            clamp_poll_interval(settings.settings.poll_interval_ms),
                            settings.settings.refresh_enabled,
                            settings.settings.socket.clone(),
                        )
                    };
                    if baseline_for.as_deref() != Some(socket.as_str()) {
                        // The one-shot baseline fetch for this socket
                        // value (D-8): exactly one per distinct
                        // socket, regardless of `refresh_enabled` —
                        // the startup snapshot. Stamping `baseline_for`
                        // + the due clock after the fetch (which cannot
                        // panic, D5) — no retry storm: a failed
                        // baseline is not re-attempted on its own — so
                        // the (b) cadence starts a full interval after
                        // it.
                        let _ = poll_telemetry(Path::new(&socket), &mut state.write().unwrap());
                        baseline_for = Some(socket);
                        last_poll = Instant::now();
                    } else if refresh_enabled && last_poll.elapsed() >= interval {
                        last_poll = Instant::now();
                        let _ = poll_telemetry(Path::new(&socket), &mut state.write().unwrap());
                    }
                }
                Err(TryRecvError::Disconnected) => break,
            }
            thread::sleep(POLLER_TICK);
        }
    })
}

// ---------------------------------------------------------------------
// Tests (in-process daemon stand-in: a thread binding a `UnixListener`
// on a unique temp socket that speaks the frozen P3-10/P3-11 frame
// protocol — the client P3-18 / TUI P3-24 test precedent).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use ramsleuth_bench::{BenchOp, Tier};
    use ramsleuth_protocol::{FrameError, Message, decode_frame, encode_frame};
    use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
    use ramsleuth_telemetry::amd_readout::map_amd;
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::{ProbeSystem, SystemPlatform};

    use crate::history::HISTORY_CAPACITY;

    use super::*;

    /// A unique temp socket path owned by a guard that removes the file
    /// on drop (best-effort cleanup; the path is pid-qualified so
    /// parallel test runs / processes never collide).
    struct TempSocket {
        path: PathBuf,
    }

    impl TempSocket {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "ramsleuth-{name}-{}.sock",
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

    /// An all-`Na` snapshot built through the telemetry crate's public
    /// API (host-independent — mirrors the client / TUI stand-in mock).
    fn mock_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "GUI Test CPU".to_owned(),
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

    /// Server-side incremental frame reader (the P3-11 contract on the
    /// other end): append every received byte and decode until one
    /// frame is complete (`None` on a clean EOF before a frame).
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

        /// A multi-connection variant: accepts up to `max_conns`
        /// connections in order, handing each its 1-based index +
        /// stream to `handler` (the poller opens a fresh connection
        /// per cycle — the history accumulation test).
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

    /// (d) `TelemetryData::default()` is the idle, never-polled state:
    /// no run in flight, no snapshot, no status, no error.
    #[test]
    fn default_state_is_idle_and_never_polled() {
        let state = TelemetryData::default();
        assert!(!state.bench.running, "a fresh state must not run a bench");
        assert!(state.bench.run_id.is_none(), "a fresh state has no run id");
        assert!(state.bench.progress.is_empty());
        assert!(state.bench.grid.is_none());
        assert!(state.telemetry.is_none());
        assert!(state.daemon_status.is_empty());
        assert!(state.last_update.is_none());
        assert!(state.error.is_none());
        assert!(state.graph.is_empty(), "a fresh state has no graph samples");
    }

    /// (a) `poll_telemetry` against a live stand-in (a `GetTelemetry`
    /// request answered with a canned `Response::Telemetry`) fills the
    /// state: `telemetry` becomes `Some` (the stand-in's snapshot), the
    /// pre-existing error is cleared, `daemon_status` reports
    /// `connected: <socket>`, and `last_update` is stamped.
    #[test]
    fn poll_telemetry_against_live_stand_in_updates_state() {
        let sock = TempSocket::new("telemetry");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                mock_snapshot(),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        // A pre-existing error (initialized here, not reassigned after)
        // must be cleared by a successful poll.
        let mut state =
            TelemetryData { error: Some("stale error".to_owned()), ..Default::default() };
        poll_telemetry(sock.path(), &mut state).expect("poll_telemetry must not error");

        assert_eq!(
            state.telemetry.as_ref().map(|t| t.cpu.brand.as_str()),
            Some("GUI Test CPU"),
            "the stand-in's canned snapshot must land in the state"
        );
        assert!(state.error.is_none(), "a successful poll must clear the error");
        let status = &state.daemon_status;
        assert!(
            status.contains("connected"),
            "the status must report the connection, got: {status}"
        );
        assert!(
            status.contains(&sock.path().display().to_string()),
            "the status must name the socket, got: {status}"
        );
        assert!(state.last_update.is_some(), "a successful poll must stamp last_update");
        // The stand-in snapshot is all-Na AMD (a driver-missing
        // host): the Na guard appends no history sample — no panic
        // (the no-panic contract, D5).
        assert!(
            state.history.is_empty(),
            "an all-Na AMD readout must append no history sample"
        );
        stand_in.join();
    }

    /// (b) `poll_telemetry` against a **non-existent** socket path
    /// records a friendly error (the `DaemonDown` "start it with …"
    /// hint) + the `disconnected` status, and never panics (the
    /// no-panic contract, plan D5).
    #[test]
    fn poll_telemetry_against_missing_socket_is_friendly() {
        let sock = TempSocket::new("missing");
        let mut state = TelemetryData::default();

        poll_telemetry(sock.path(), &mut state).expect("poll_telemetry must not error");

        assert!(state.telemetry.is_none(), "no telemetry without a daemon");
        assert_eq!(state.daemon_status, "disconnected");
        let error = state.error.expect("a friendly error must be recorded");
        assert!(
            error.contains("daemon not running"),
            "the error must carry the DaemonDown hint, got: {error}"
        );
    }

    /// (c) `run_bench` against a stand-in (a `StartBenchmark` answered
    /// with `BenchStarted` + one `BenchProgress` + the terminal
    /// `BenchResult`) streams into `state.bench`: exactly the one
    /// progress event (a stale pre-run entry is cleared), the small
    /// result grid, and `running = false` on the terminal — with no
    /// transport error.
    #[test]
    fn run_bench_streams_started_progress_result() {
        let sock = TempSocket::new("bench");
        let progress = StreamProgress {
            cell_index: 0,
            total_cells: 3,
            tier: Tier::Memory,
            op: BenchOp::Read,
            value: 26.35,
            label: "Memory · Read (GB/s)".to_owned(),
        };
        let grid = BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [43.63, 0.0, 0.0, 0.0],
            copy_gbps: [12.11, 0.0, 0.0, 0.0],
            latency_ns: [86.84, 0.0, 0.0, 0.0],
        };
        let expected_progress = progress.clone();
        let expected_grid = grid.clone();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBenchmark { target, mode })) => {
                    assert_eq!(
                        target,
                        StreamTarget::Tier(Tier::Memory),
                        "the cmd's target must ride the wire"
                    );
                    assert_eq!(mode, BenchMode::Full, "the cmd's mode must ride the wire");
                }
                other => panic!("stand-in expected StartBenchmark, got {other:?}"),
            }
            for response in [
                Response::BenchStarted { run_id: 1 },
                Response::BenchProgress(progress),
                Response::BenchResult { run_id: 1, grid },
            ] {
                let bytes = encode_frame(&Message::Response(response)).expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        });

        let cmd = BenchCmd {
            target: StreamTarget::Tier(Tier::Memory),
            mode: BenchMode::Full,
            duration_minutes: None,
        };
        // A false cancel flag: this run is never cancelled (it is
        // reset per run anyway).
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        {
            let mut s = state.write().expect("the poller must not poison the lock");
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

        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running, "the terminal result must clear running");
        assert_eq!(state.bench.run_id, Some(1), "the ack's run_id must be recorded");
        assert_eq!(
            state.bench.progress.len(),
            1,
            "exactly the streamed event; the stale entry is cleared"
        );
        assert_eq!(
            state.bench.progress[0],
            expected_progress,
            "the streamed event must land verbatim"
        );
        assert_eq!(
            state.bench.grid,
            Some(expected_grid),
            "the terminal grid must land in the state"
        );
        assert!(state.error.is_none(), "a clean run must not record an error");
        stand_in.join();
    }

    /// (e) `spawn_poller` smoke: it returns a `JoinHandle` whose thread
    /// stops cleanly on the `stop` flag and leaves the lock usable (the
    /// heavy logic is covered by the `poll_telemetry` / `run_bench`
    /// tests above — no extensive real-thread assertions). The poller
    /// reads its socket from `state.settings` (C6-30): the default
    /// (unbound) socket records a friendly error on the one-shot
    /// baseline fetch (D-8) before the immediate stop.
    #[test]
    fn spawn_poller_returns_a_handle_that_stops_cleanly() {
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let (_tx, rx) = mpsc::channel::<BenchCmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));

        let handle = spawn_poller(state.clone(), rx, stop.clone(), cancel.clone());
        stop.store(true, Ordering::Relaxed);
        handle.join().expect("the poller thread must not panic");

        // The lock is usable after the thread exits and no run is
        // left in flight.
        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running);
    }

    /// (f) CANCEL (P3-28): after the `BenchStarted` ack lands in the
    /// state, the shared `cancel` flag is set — `run_bench` ends the
    /// run with `running = false` (sending the daemon a best-effort
    /// `CancelBenchmark` when the run has an id) — no hang (the
    /// stand-in ends the run within a beat either way), no panic, no
    /// error recorded.
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
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = BenchCmd {
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
            if state.read().expect("the poller must not poison the lock").bench.run_id.is_some() {
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

        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running, "the cancel must clear running");
        assert_eq!(state.bench.run_id, Some(42), "the run id must stay recorded");
        assert!(state.error.is_none(), "a clean cancel must not record an error");
        stand_in.join();
    }

    // ------------------------------------------------------------------
    // C6-25: the trend-history wiring — one sample per successful
    // poll, the Na guard, the capacity wrap, the reconnect prime.
    // ------------------------------------------------------------------

    /// A snapshot with a populated AMD readout: the [`map_amd`]
    /// output over an in-range synthetic PM-table snapshot (the
    /// DDR4-3200-class fixture the telemetry crate's `good_snapshot`
    /// test uses) — MCLK = `mclk_mhz` (the wrap test steps it; every
    /// value stays inside the clock range gate) and VDDCR_SOC = 1050
    /// mV. The host (Intel branch, platform, SPD) stays all-Na.
    fn populated_snapshot(mclk_mhz: u16) -> SystemMemoryTelemetry {
        let readout = map_amd(&AmdPmSnapshot {
            version: 0x0007_0B02,
            mclk_mhz,
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

    /// (g1) One successful poll with a populated AMD readout appends
    /// exactly one history sample: MCLK (MHz) + VDDCR_SOC (mV) from
    /// the readout, the bandwidth from the terminal bench grid's
    /// memory-read cell (row 0).
    #[test]
    fn poll_telemetry_pushes_one_history_sample_per_success() {
        let sock = TempSocket::new("history-once");
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                populated_snapshot(1800),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        let mut state = TelemetryData::default();
        // A terminal grid from an earlier run: the memory-read cell
        // (row 0) feeds the bandwidth series.
        state.bench.grid = Some(BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [0.0, 0.0, 0.0, 0.0],
            copy_gbps: [0.0, 0.0, 0.0, 0.0],
            latency_ns: [0.0, 0.0, 0.0, 0.0],
        });

        poll_telemetry(sock.path(), &mut state).expect("poll_telemetry must not error");
        stand_in.join();

        assert_eq!(state.history.len(), 1, "one successful poll appends exactly one sample");
        assert_eq!(
            *state.history.mclk.last().expect("the mclk series has the sample"),
            1800.0,
            "MCLK lands in MHz"
        );
        assert_eq!(
            *state.history.vddcr_soc.last().expect("the vddcr_soc series has the sample"),
            1050.0,
            "VDDCR_SOC lands in mV"
        );
        assert_eq!(
            *state.history
                .bandwidth
                .last()
                .expect("the bandwidth series has the sample"),
            26.35,
            "the bandwidth is the terminal grid's memory-read cell"
        );
    }

    /// (g2) The bandwidth source picks: the newest streamed
    /// `Memory · Read` event (a live run) over the terminal grid, the
    /// grid when no progress streams, `0.0` when neither carries a
    /// value — a non-finite / non-positive event never counts.
    #[test]
    fn latest_memory_read_bw_source_priority() {
        // Neither: no progress, no grid → 0.0.
        let idle = TelemetryData::default();
        assert_eq!(latest_memory_read_bw(&idle), 0.0, "no bench data at all → 0.0");

        // Terminal grid only (row 0 = the Memory read cell).
        let grid_only = TelemetryData {
            bench: BenchState {
                grid: Some(BenchmarkGrid {
                    read_gbps: [26.35, 0.0, 0.0, 0.0],
                    write_gbps: [0.0, 0.0, 0.0, 0.0],
                    copy_gbps: [0.0, 0.0, 0.0, 0.0],
                    latency_ns: [0.0, 0.0, 0.0, 0.0],
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(latest_memory_read_bw(&grid_only), 26.35, "the terminal grid cell");

        // A live `Memory · Read` event beats the (older) terminal
        // grid; a newer non-Memory / non-Read event is ignored.
        let live = TelemetryData {
            bench: BenchState {
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
                grid: Some(BenchmarkGrid {
                    read_gbps: [26.35, 0.0, 0.0, 0.0],
                    write_gbps: [0.0, 0.0, 0.0, 0.0],
                    copy_gbps: [0.0, 0.0, 0.0, 0.0],
                    latency_ns: [0.0, 0.0, 0.0, 0.0],
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            latest_memory_read_bw(&live),
            42.0,
            "the newest Memory · Read event wins over the grid"
        );

        // A non-finite `Memory · Read` value + a `Memory · Write`
        // event are both ignored: the figure falls back to the grid.
        let bad_live = TelemetryData {
            bench: BenchState {
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
                grid: Some(BenchmarkGrid {
                    read_gbps: [26.35, 0.0, 0.0, 0.0],
                    write_gbps: [0.0, 0.0, 0.0, 0.0],
                    copy_gbps: [0.0, 0.0, 0.0, 0.0],
                    latency_ns: [0.0, 0.0, 0.0, 0.0],
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            latest_memory_read_bw(&bad_live),
            26.35,
            "a NaN read event + a write event fall back to the grid"
        );
    }

    /// (g3) N successful polls append N samples (one per poll — the
    /// poller stays the only writer, D6), and past the 300-sample
    /// capacity the oldest is evicted: the series caps at
    /// [`HISTORY_CAPACITY`], the five oldest samples are gone, the
    /// newest is last, and the order holds (a reconnect never fires —
    /// the status stays `connected` throughout, so no clear).
    #[test]
    fn poll_telemetry_appends_per_poll_and_wraps_at_capacity() {
        let total = HISTORY_CAPACITY + 5;
        let sock = TempSocket::new("history-wrap");
        let stand_in = DaemonStandIn::spawn_multi(&sock, total, |index, mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            // One sample per connection: MCLK = the 1-based index (so
            // the eviction is observable); VDDCR_SOC is constant.
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                populated_snapshot(index as u16),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        let mut state = TelemetryData::default();
        for _ in 0..total {
            poll_telemetry(sock.path(), &mut state).expect("poll_telemetry must not error");
        }
        stand_in.join();

        assert_eq!(
            state.history.len(),
            HISTORY_CAPACITY,
            "the series caps at the ring capacity"
        );
        let mclk = state.history.mclk.iter().copied().collect::<Vec<_>>();
        assert_eq!(
            mclk.first().copied(),
            Some(6.0),
            "the five oldest samples (MCLK 1..=5) are evicted"
        );
        assert_eq!(mclk.last().copied(), Some(total as f64), "the newest sample is last");
        assert!(
            mclk.windows(2).all(|w| w[1] > w[0]),
            "the samples stay in poll order (oldest → newest)"
        );
        assert!(
            state.history.vddcr_soc.iter().all(|v| *v == 1050.0),
            "the constant VDDCR_SOC rail survives the wrap"
        );
    }

    /// (g4) Reconnect prime: two continuous successful polls append
    /// two samples (no clear — the status stays `connected`); a
    /// daemon drop records `disconnected`, and the next successful
    /// poll clears the stale series before appending one fresh
    /// sample.
    #[test]
    fn history_clears_on_reconnect_not_on_continuous_polls() {
        // Phase 1: two continuous successful polls (a fresh
        // connection each) → two samples, no clear.
        let sock = TempSocket::new("history-reconnect");
        let stand_in = DaemonStandIn::spawn_multi(&sock, 2, |_index, mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                populated_snapshot(1800),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        let mut state = TelemetryData::default();
        poll_telemetry(sock.path(), &mut state).expect("first poll must not error");
        poll_telemetry(sock.path(), &mut state).expect("second poll must not error");
        stand_in.join();
        assert_eq!(state.history.len(), 2, "two continuous polls append two samples");

        // Phase 2: the daemon drops (a missing socket records
        // `disconnected`), then comes back → the stale series is
        // cleared and the reconnect poll appends exactly one fresh
        // sample.
        let down_sock = TempSocket::new("history-reconnect-down");
        poll_telemetry(down_sock.path(), &mut state).expect("the down poll must not error");
        assert_eq!(state.daemon_status, "disconnected");

        let back_sock = TempSocket::new("history-reconnect-back");
        let stand_in = DaemonStandIn::spawn(&back_sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                populated_snapshot(1800),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        poll_telemetry(back_sock.path(), &mut state).expect("the reconnect poll must not error");
        stand_in.join();
        assert_eq!(state.history.len(), 1, "the reconnect clears the stale series");
        assert_eq!(*state.history.mclk.last().expect("the fresh sample"), 1800.0);
    }

    /// (g5) C7-20: five successful polls append five graphs-window
    /// samples (one per poll — the poller stays the only writer,
    /// D6): the VDDCR_SOC (mV) + the terminal grid's memory-read
    /// bandwidth (GB/s, D-5) land finite on every sample, and the
    /// no-source series (the CPU freq — this fixture's platform is
    /// all-Na) stays NaN (D-4: it self-populates if a source
    /// appears — the row is data-driven, no layout change).
    #[test]
    fn poll_telemetry_appends_one_graph_sample_per_success() {
        let sock = TempSocket::new("graph-once");
        let stand_in = DaemonStandIn::spawn_multi(&sock, 5, |index, mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes = encode_frame(&Message::Response(Response::Telemetry(
                populated_snapshot(index as u16),
            )))
            .expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });

        let mut state = TelemetryData::default();
        // A terminal grid from an earlier run: the memory-read cell
        // (row 0) feeds the bandwidth series (D-5).
        state.bench.grid = Some(BenchmarkGrid {
            read_gbps: [26.35, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        });

        for _ in 0..5 {
            poll_telemetry(sock.path(), &mut state).expect("poll_telemetry must not error");
        }
        stand_in.join();

        assert_eq!(state.graph.len(), 5, "five successful polls append five graph samples");
        for sample in state.graph.samples.iter() {
            assert!(sample.t.is_finite() && sample.t > 0.0, "the sample is stamped with unix seconds");
            assert_eq!(sample.vddcr_soc_mv, 1050.0, "VDDCR_SOC lands in mV");
            assert_eq!(
                sample.bandwidth_gbps, 26.35,
                "the bandwidth is the terminal grid's memory-read cell (D-5)"
            );
            assert!(
                sample.cpu_freq_mhz.is_nan(),
                "the all-Na platform keeps the freq series no-source (D-4)"
            );
        }
    }

    // ------------------------------------------------------------------
    // C6-27: the live settings knobs — the poller re-reads the poll
    // interval + the refresh gate every tick (no restart).
    // ------------------------------------------------------------------

    /// (i1) `clamp_poll_interval`: the sane range (100 ms – 60 s, the
    /// settings panel's DragValue bounds) — a zero / absurd stored
    /// knob degrades to the bound, never a hot-spin or stall (D5).
    #[test]
    fn clamp_poll_interval_bounds() {
        assert_eq!(clamp_poll_interval(0), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(50), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(100), Duration::from_millis(100));
        assert_eq!(clamp_poll_interval(2_000), Duration::from_millis(2_000));
        assert_eq!(clamp_poll_interval(60_000), Duration::from_millis(60_000));
        assert_eq!(clamp_poll_interval(60_001), Duration::from_millis(60_000));
        assert_eq!(clamp_poll_interval(u64::MAX), Duration::from_millis(60_000));
    }

    /// (i2) The poller honors the **live** settings knobs (no
    /// poller restart): a short `poll_interval_ms` (100 ms — the
    /// clamp floor) produces several samples over the window;
    /// changing the knob live to the clamp ceiling (60 s) freezes
    /// the cadence in the same running poller; flipping
    /// `refresh_enabled` off pauses polling even with a short
    /// interval, and back on resumes it (the stale due-stamp polls
    /// on the next tick). The stand-in accepts one connection per
    /// poll (the fresh-connection-per-cycle contract) and serves
    /// one populated snapshot (one history sample per poll); a
    /// waker connection (immediate EOF) unblocks its final accept
    /// on stop.
    #[test]
    fn spawn_poller_honors_the_live_settings_knobs() {
        let sock = TempSocket::new("poller-settings");
        let stop = Arc::new(AtomicBool::new(false));
        // The stand-in: accept until `stop`, serve one populated
        // snapshot per connection; the final accept is unblocked by
        // the waker (an EOF before a frame ends it — no panic).
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        let stand_in = {
            let stop = Arc::clone(&stop);
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
                        Some(Message::Request(Request::GetTelemetry)) => {}
                        Some(other) => {
                            panic!("stand-in expected GetTelemetry, got {other:?}")
                        }
                        None => break, // the waker (an EOF before a frame)
                    }
                    let bytes = encode_frame(&Message::Response(Response::Telemetry(
                        populated_snapshot(1800),
                    )))
                    .expect("must encode");
                    stream.write_all(&bytes).expect("stand-in write must not fail");
                }
            })
        };

        // The shared state starts with the live socket (the
        // stand-in's path — C6-30) + a short (clamp-floor) interval
        // + refresh on — the poller reads all of these live, per
        // tick (the settings panel's one render-thread write, D6).
        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: sock.path().to_string_lossy().into_owned(),
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                // Explicit (C7-08): the pre-C7-09 default is `true`;
                // the test's semantics are unchanged.
                refresh_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: the short interval — several samples over the
        // window (one per poll; a 100 ms knob polls nearly every
        // 200 ms tick).
        thread::sleep(Duration::from_millis(1_500));
        let a = state.read().expect("the poller must not poison the lock").history.len();
        assert!(a >= 2, "the short interval must produce several samples, got {a}");

        // Phase B: change the knob live (no restart) to the clamp
        // ceiling — the cadence freezes (60 s cannot elapse in the
        // window): no new samples.
        {
            let mut guard = state.write().expect("the panel write must not fail");
            guard.settings.poll_interval_ms = MAX_POLL_INTERVAL_MS;
        }
        thread::sleep(Duration::from_millis(1_200));
        let b = state.read().expect("the poller must not poison the lock").history.len();
        assert_eq!(b, a, "a live interval change to 60 s must freeze the cadence");

        // Phase C: the refresh gate — a short interval again, but
        // refresh disabled: polling stays paused (the gate wins over
        // a due stamp).
        {
            let mut guard = state.write().expect("the panel write must not fail");
            guard.settings.poll_interval_ms = MIN_POLL_INTERVAL_MS;
            guard.settings.refresh_enabled = false;
        }
        thread::sleep(Duration::from_millis(1_200));
        let c = state.read().expect("the poller must not poison the lock").history.len();
        assert_eq!(
            c,
            a,
            "a disabled refresh must pause polling even with a short interval"
        );

        // Phase D: resume — refresh back on (the short interval is
        // kept): the stale due-stamp polls on the next tick
        // (polling resumed without a restart).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .refresh_enabled = true;
        thread::sleep(Duration::from_millis(1_000));
        let d = state.read().expect("the poller must not poison the lock").history.len();
        assert!(d > a, "re-enabling the refresh must resume polling");

        // Stop: drop the bench sender (the poller's channel
        // disconnects), set the shared flag, and wake the stand-in's
        // final accept with a throwaway connect (immediate EOF; a
        // failed connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock.path());
        stand_in.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");

        // The samples are the stand-in's MCLK (one per poll): the
        // cadence changes never lost the series (no reconnect — the
        // status stayed `connected` throughout), and no run was
        // left in flight.
        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running);
        assert!(
            state.history.mclk.iter().all(|v| *v == 1800.0),
            "every sample is the stand-in's MCLK"
        );
    }

    // ------------------------------------------------------------------
    // C6-30: the dynamic socket — the poller reads the live
    // `settings.socket` (a panel edit retargets the next cycle, no
    // poller restart).
    // ------------------------------------------------------------------

    /// Wait until `daemon_status` contains `needle` (bounded by a 10 s
    /// deadline: the test fails with the current status instead of
    /// hanging).
    fn wait_for_status(state: &Arc<RwLock<TelemetryData>>, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = state
                .read()
                .expect("the poller must not poison the lock")
                .daemon_status
                .clone();
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

    /// (i3) The poller follows the **live** settings socket (no
    /// restart): it starts against socket A (the stand-in), a panel
    /// edit of `settings.socket` to a missing path B retargets the
    /// next cycle (the friendly `disconnected`), and a second edit
    /// back to A reconnects the same running poller (the fresh
    /// connection per cycle rides each change — C6-30).
    #[test]
    fn spawn_poller_follows_the_live_settings_socket() {
        let sock_a = TempSocket::new("socket-live-a");
        let sock_b = TempSocket::new("socket-live-b"); // never bound: missing
        let stop = Arc::new(AtomicBool::new(false));
        // The stand-in on A: accept until `stop`, serve one mock
        // snapshot per fresh connection (one per poll cycle); the
        // final accept is unblocked by the waker (an EOF before a
        // frame ends it — no panic).
        let listener = UnixListener::bind(sock_a.path()).expect("test socket must bind");
        let stand_in = {
            let stop = Arc::clone(&stop);
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
                        Some(Message::Request(Request::GetTelemetry)) => {}
                        Some(other) => {
                            panic!("stand-in expected GetTelemetry, got {other:?}")
                        }
                        None => break, // the waker (an EOF before a frame)
                    }
                    let bytes =
                        encode_frame(&Message::Response(Response::Telemetry(
                            mock_snapshot(),
                        )))
                        .expect("must encode");
                    stream.write_all(&bytes).expect("stand-in write must not fail");
                }
            })
        };

        // The shared state: the socket starts at A, the short
        // (clamp-floor) interval polls nearly every tick.
        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: sock_a.path().to_string_lossy().into_owned(),
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                // Explicit (C7-08): the pre-C7-09 default is `true`;
                // the test's semantics are unchanged.
                refresh_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: connected to A (the CLI-seeded / current socket).
        wait_for_status(&state, &format!("connected: {}", sock_a.path().display()));

        // Phase B: the panel edits the socket to the missing B → the
        // next cycle retargets (the friendly disconnect).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .socket = sock_b.path().to_string_lossy().into_owned();
        wait_for_status(&state, "disconnected");

        // Phase C: the panel edits the socket back to A → the same
        // running poller reconnects (no restart).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .socket = sock_a.path().to_string_lossy().into_owned();
        wait_for_status(&state, &format!("connected: {}", sock_a.path().display()));

        // Stop: set the shared flag and wake the stand-in's final
        // accept with a throwaway connect (immediate EOF; a failed
        // connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock_a.path());
        stand_in.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");
    }

    // ------------------------------------------------------------------
    // C7-08 (D-8): the baseline-once mechanism — exactly one
    // `poll_telemetry` fetch per distinct `settings.socket` value,
    // independent of `refresh_enabled`; a socket edit re-triggers one
    // fetch; a failed baseline never retries on its own.
    // ------------------------------------------------------------------

    /// A counting stand-in: accepts connections until `stop`, serves
    /// one mock snapshot per connection, and increments `count` per
    /// served connection. The stop-waker's throwaway connect is never
    /// counted (it is only sent after `stop` is set, and both stop
    /// checks pass before a served connection is counted).
    fn counting_stand_in(
        sock: &TempSocket,
        stop: &Arc<AtomicBool>,
        count: &Arc<AtomicUsize>,
    ) -> JoinHandle<()> {
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
                    Some(other) => {
                        panic!("stand-in expected GetTelemetry, got {other:?}")
                    }
                    None => break, // an unexpected EOF before a frame
                }
                let bytes = encode_frame(&Message::Response(Response::Telemetry(
                    mock_snapshot(),
                )))
                .expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        })
    }

    /// A flapping-daemon stand-in: accepts until `stop`, drops each
    /// connection immediately (immediate EOF — the client's request
    /// fails on the read), and increments `count` per accepted
    /// connection (the stop-waker's connect is never counted, as
    /// above).
    fn dropping_stand_in(
        sock: &TempSocket,
        stop: &Arc<AtomicBool>,
        count: &Arc<AtomicUsize>,
    ) -> JoinHandle<()> {
        let listener = UnixListener::bind(sock.path()).expect("test socket must bind");
        let stop = Arc::clone(stop);
        let count = Arc::clone(count);
        thread::spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                if stop.load(Ordering::Relaxed) {
                    break; // the waker (stop was set first)
                }
                count.fetch_add(1, Ordering::Relaxed);
                drop(stream); // immediate EOF: the client's request fails
            }
        })
    }

    /// (j1) Baseline-once (D-8): with `refresh_enabled` off, the
    /// poller performs **exactly one** fetch per distinct
    /// `settings.socket` value — the startup baseline against A (one
    /// connection), a socket edit to B fires exactly one more (one
    /// connection for B), and a second edit back to A fires exactly
    /// one more for A — no continuous polling (the gate is off) and
    /// no retry (an unchanged socket is never re-fetched).
    #[test]
    fn baseline_fires_exactly_once_per_distinct_socket_with_refresh_off() {
        let sock_a = TempSocket::new("baseline-a");
        let sock_b = TempSocket::new("baseline-b");
        let stop = Arc::new(AtomicBool::new(false));
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));
        let stand_in_a = counting_stand_in(&sock_a, &stop, &count_a);
        let stand_in_b = counting_stand_in(&sock_b, &stop, &count_b);

        // Refresh explicitly off (the pre-C7-09 default is still on —
        // C7-09 flips it): the baseline must not depend on the
        // default; it is the only thing that may fetch.
        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: sock_a.path().to_string_lossy().into_owned(),
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                refresh_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: the startup baseline fetch against A — exactly one
        // connection, then nothing (the gate is off, the socket is
        // unchanged — several ticks pass).
        wait_for_status(&state, &format!("connected: {}", sock_a.path().display()));
        thread::sleep(Duration::from_millis(600));
        assert_eq!(
            count_a.load(Ordering::Relaxed),
            1,
            "exactly one baseline fetch for the startup socket A"
        );

        // Phase B: a socket edit to B re-triggers exactly one
        // baseline fetch for the new socket.
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .socket = sock_b.path().to_string_lossy().into_owned();
        wait_for_status(&state, &format!("connected: {}", sock_b.path().display()));
        thread::sleep(Duration::from_millis(600));
        assert_eq!(
            count_b.load(Ordering::Relaxed),
            1,
            "exactly one baseline fetch for the edited socket B"
        );
        assert_eq!(
            count_a.load(Ordering::Relaxed),
            1,
            "A is never re-fetched without another socket change"
        );

        // Phase C: a second socket edit (back to A) fires exactly one
        // more baseline fetch for A.
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .socket = sock_a.path().to_string_lossy().into_owned();
        wait_for_status(&state, &format!("connected: {}", sock_a.path().display()));
        thread::sleep(Duration::from_millis(600));
        assert_eq!(
            count_a.load(Ordering::Relaxed),
            2,
            "the edit back to A re-triggers exactly one more baseline"
        );
        assert_eq!(
            count_b.load(Ordering::Relaxed),
            1,
            "B stays at its one baseline fetch"
        );

        // Stop: set the shared flag and wake both stand-ins' final
        // accepts with throwaway connects (immediate EOF; a failed
        // connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock_a.path());
        let _ = UnixStream::connect(sock_b.path());
        stand_in_a.join().expect("the stand-in must not panic");
        stand_in_b.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");
    }

    /// (j2) A **failed** baseline (the daemon drops the connection on
    /// the fetch) records the friendly `disconnected` state + a
    /// structured error exactly once and never retries on its own
    /// (no retry storm — several ticks pass with the gate off): the
    /// next re-trigger is either the refresh gate (the cadence then
    /// polls on the configured interval) or a socket edit to a live
    /// daemon (one baseline fetch for it).
    #[test]
    fn failed_baseline_records_disconnected_without_retrying() {
        let dead = TempSocket::new("baseline-dead");
        let live = TempSocket::new("baseline-live");
        let stop = Arc::new(AtomicBool::new(false));
        let dead_count = Arc::new(AtomicUsize::new(0));
        let live_count = Arc::new(AtomicUsize::new(0));
        let stand_in_dead = dropping_stand_in(&dead, &stop, &dead_count);
        let stand_in_live = counting_stand_in(&live, &stop, &live_count);

        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: dead.path().to_string_lossy().into_owned(),
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                refresh_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: the one-shot baseline fetch against the flapping
        // daemon fails — the friendly `disconnected` state + a
        // structured error.
        wait_for_status(&state, "disconnected");
        {
            let s = state.read().expect("the poller must not poison the lock");
            assert!(s.error.is_some(), "the failed baseline must record a structured error");
        }
        // No retry storm: several ticks pass with the gate off — the
        // failed baseline is never re-attempted on its own.
        thread::sleep(Duration::from_millis(800));
        assert_eq!(
            dead_count.load(Ordering::Relaxed),
            1,
            "a failed baseline must not retry on its own (no retry storm)"
        );

        // Phase B: enabling the refresh gate re-triggers the cadence
        // (the due clock polls the still-dead socket on the
        // configured interval).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .refresh_enabled = true;
        thread::sleep(Duration::from_millis(600));
        assert!(
            dead_count.load(Ordering::Relaxed) > 1,
            "an enabled refresh must resume the (failing) cadence"
        );

        // Phase C: the gate is frozen again, and a socket edit to a
        // live daemon re-triggers exactly one baseline fetch for the
        // new socket (the settings panel is never a dead end).
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .refresh_enabled = false;
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .socket = live.path().to_string_lossy().into_owned();
        wait_for_status(&state, &format!("connected: {}", live.path().display()));
        thread::sleep(Duration::from_millis(600));
        assert_eq!(
            live_count.load(Ordering::Relaxed),
            1,
            "exactly one baseline fetch for the new live socket"
        );

        // Stop: set the shared flag and wake both stand-ins' final
        // accepts with throwaway connects (immediate EOF; a failed
        // connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(dead.path());
        let _ = UnixStream::connect(live.path());
        stand_in_dead.join().expect("the stand-in must not panic");
        stand_in_live.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");
    }

    /// (j3) With the refresh gate off, the one-shot baseline is the
    /// only fetch (one connection, frozen afterward — several ticks
    /// pass); **enabling the refresh gate** resumes the continuous
    /// cadence one interval after the baseline's stamp — the
    /// connections grow again against the same socket.
    #[test]
    fn enabling_refresh_after_baseline_resumes_the_cadence() {
        let sock = TempSocket::new("baseline-resume");
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicUsize::new(0));
        let stand_in = counting_stand_in(&sock, &stop, &count);

        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: sock.path().to_string_lossy().into_owned(),
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                refresh_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: the startup baseline fetch (one connection); with
        // the gate off the cadence never starts — the count freezes
        // over several ticks.
        wait_for_status(&state, &format!("connected: {}", sock.path().display()));
        thread::sleep(Duration::from_millis(800));
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "with refresh off, only the one-shot baseline fetch happens"
        );

        // Phase B: the panel enables the refresh gate (the short
        // clamp-floor interval): the baseline's due stamp is now well
        // past the interval, so the cadence resumes on the next tick
        // — the connections grow again.
        state
            .write()
            .expect("the panel write must not fail")
            .settings
            .refresh_enabled = true;
        thread::sleep(Duration::from_millis(1_200));
        assert!(
            count.load(Ordering::Relaxed) >= 3,
            "enabling refresh must resume the continuous cadence (got {} connections)",
            count.load(Ordering::Relaxed)
        );

        // Stop: set the shared flag and wake the stand-in's final
        // accept with a throwaway connect (immediate EOF; a failed
        // connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock.path());
        stand_in.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");
    }

    // ------------------------------------------------------------------
    // C7-16: the burn-in path — `run_burn_in` consumption, the
    // `BenchCmd` dispatch, the raised stream read timeout.
    // ------------------------------------------------------------------

    /// (k0) The bench / burn-in stream read timeout is the raised
    /// 120 s value (the CLI precedent): a long frame gap can no
    /// longer trip the client's 5 s default mid-stream.
    #[test]
    fn bench_stream_read_timeout_is_the_raised_cli_value() {
        assert_eq!(
            BENCH_READ_TIMEOUT,
            Duration::from_secs(120),
            "the stream read timeout must be the 120 s CLI precedent"
        );
    }

    /// (k1) `run_burn_in` against a stand-in (a `StartBurnIn`
    /// answered with `BenchStarted` + `BurnInProgress` ticks — one
    /// bandwidth kind and one latency kind — + the terminal
    /// `BenchResult`) consumes the stream into `state.bench`: the
    /// request rides the wire with the cmd's target / duration, the
    /// `BenchStarted` ack's `run_id` is recorded, each tick updates
    /// `burn_in` (the newest `iteration` / `elapsed_secs`, and the
    /// `latest` grid — the newest value per cell wins, the
    /// `live_grid` rule), and the terminal lands its grid in
    /// `state.bench.grid` + clears `burn_in.running` — with no
    /// transport error and the normal-bench `running` flag untouched.
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

        let cmd = BenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(5),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = TelemetryData::default();
        // A stale pre-run burn-in state must be reset at run start
        // (iteration / elapsed / `latest`).
        state.bench.burn_in.running = true;
        state.bench.burn_in.iteration = 7;
        state.bench.burn_in.elapsed_secs = 99.0;
        state.bench.burn_in.latest.read_gbps[0] = 7.0;
        // A stale pre-run id must be dropped at run start.
        state.bench.run_id = Some(99);
        let state = Arc::new(RwLock::new(state));
        run_burn_in(sock.path(), cmd, &state, &cancel).expect("run_burn_in must not error");

        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.burn_in.running, "the terminal must clear burn_in.running");
        assert!(
            !state.bench.running,
            "a burn-in run must not touch the normal-bench running flag"
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
        assert_eq!(state.bench.burn_in.latest.write_gbps, [0.0; 4], "an unticked column stays zero");
        assert_eq!(state.bench.burn_in.latest.copy_gbps, [0.0; 4], "an unticked column stays zero");
        assert_eq!(
            state.bench.grid,
            Some(expected_grid),
            "the terminal grid must land in the state"
        );
        assert!(state.error.is_none(), "a clean run must not record an error");
        stand_in.join();
    }

    /// (k2) CANCEL (P3-28, the brief's "stop via existing Cancel"):
    /// after the burn-in's `BenchStarted` ack lands in the state, the
    /// shared `cancel` flag is set — `run_burn_in` ends the run with
    /// `burn_in.running = false` (sending the daemon a best-effort
    /// `CancelBenchmark` for the recorded run id) — no hang (the
    /// stand-in ends the run within a beat either way), no panic, no
    /// error recorded.
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
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = BenchCmd {
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
            if state.read().expect("the poller must not poison the lock").bench.run_id.is_some() {
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

        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.burn_in.running, "the cancel must clear burn_in.running");
        assert_eq!(state.bench.run_id, Some(42), "the run id must stay recorded");
        assert!(state.error.is_none(), "a clean cancel must not record an error");
        stand_in.join();
    }

    /// (k3) A `BenchProgress` frame in a burn-in stream violates the
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

        let cmd = BenchCmd {
            target: StreamTarget::Full,
            mode: BenchMode::Full,
            duration_minutes: Some(2),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        run_burn_in(sock.path(), cmd, &state, &cancel).expect("run_burn_in must not error");

        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.burn_in.running, "the violation must clear burn_in.running");
        let error = state.error.as_ref().expect("the violation must record a structured error");
        assert!(
            error.contains("unexpected benchmark frame during burn-in"),
            "the error must name the violation, got: {error}"
        );
        stand_in.join();
    }

    /// (k4) The poller dispatches on the cmd's `duration_minutes`
    /// discriminator (C7-16): a default (`None`) cmd routes to
    /// `run_bench` (a `StartBenchmark` on the wire) and a burn-in
    /// (`Some(7)`) cmd routes to `run_burn_in` (a `StartBurnIn` on
    /// the wire) — same live settings socket, both runs stream to
    /// their terminal cleanly (the baseline poll may interleave; the
    /// stand-in serves every request arm it sees).
    #[test]
    fn poller_dispatches_bench_cmds_by_the_duration_discriminator() {
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
                            let started =
                                encode_frame(&Message::Response(Response::BenchStarted { run_id: 1 }))
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
                            let started =
                                encode_frame(&Message::Response(Response::BenchStarted { run_id: 2 }))
                                    .expect("must encode");
                            stream.write_all(&started).expect("stand-in write must not fail");
                            let cancelled =
                                encode_frame(&Message::Response(Response::BenchCancelled { run_id: 2 }))
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

        // Refresh explicitly off (the pre-C7-09 default is still on —
        // C7-09 flips it): only the dispatched runs + the one-shot
        // baseline may fetch.
        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                socket: sock.path().to_string_lossy().into_owned(),
                refresh_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel);

        // Phase A: the default (`duration_minutes = None`) cmd — the
        // dispatch must route it to `run_bench` (a `StartBenchmark`
        // on the wire).
        bench_tx
            .send(BenchCmd {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
                duration_minutes: None,
            })
            .expect("the channel must accept the cmd");
        // Phase B: the burn-in (`duration_minutes = Some(7)`) cmd —
        // the dispatch must route it to `run_burn_in` (a
        // `StartBurnIn` on the wire).
        bench_tx
            .send(BenchCmd {
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
        assert_eq!(seen.load(Ordering::Relaxed), 3, "exactly one StartBenchmark + one StartBurnIn");

        // Stop: drop the bench sender (the poller's channel
        // disconnects), set the shared flag, and wake the stand-in's
        // final accept with a throwaway connect (immediate EOF; a
        // failed connect means the stand-in is already out).
        drop(bench_tx);
        stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(sock.path());
        stand_in.join().expect("the stand-in must not panic");
        poller.join().expect("the poller thread must not panic");

        // Both runs ended cleanly: nothing left in flight (either
        // class), no error recorded.
        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running, "the normal run must end at its terminal");
        assert!(!state.bench.burn_in.running, "the burn-in must end at its terminal");
        assert!(state.error.is_none(), "clean runs must not record an error");
    }

    // ------------------------------------------------------------------
    // C14-03: the missing regression net (BUG-2) — the render
    // thread's reads must stay brief while a burn-in is in flight.
    // ------------------------------------------------------------------

    /// (k5) C14-03: a **long, non-self-terminating** in-flight
    /// burn-in keeps the shared state's lock brief for readers.
    /// Phase 1: a ~1.5 s run (15 steady `BurnInProgress` ticks, then
    /// the terminal grid) — the stream stays open until the run
    /// ends, so under the pre-fix lock scope (the write guard held
    /// across the whole drain) every `state.read()` on this thread
    /// would park for the entire run: here, every read taken every
    /// ~10 ms must complete promptly (no blocking), the mid-run
    /// reads must observe the live tick state (`burn_in.running ==
    /// true` with the `iteration` increasing), and the run must end
    /// at its terminal (the grid recorded, no error). Phase 2: the
    /// Cancel flag stops a non-self-terminating run (the stand-in
    /// ticks for as long as the stream stays open) —
    /// `burn_in.running` is cleared with no error (the clean stop,
    /// P3-28).
    #[test]
    fn run_in_flight_burn_in_keeps_the_lock_brief_for_readers() {
        // Phase 1: the stand-in stays open until the run's terminal
        // (~1.5 s of ticks — deliberately not self-terminating
        // within 200 ms like the (k2) cancel stand-in).
        let sock = TempSocket::new("burnin-inflight");
        let grid = BenchmarkGrid {
            read_gbps: [10.5, 0.0, 0.0, 0.0],
            write_gbps: [0.0; 4],
            copy_gbps: [0.0; 4],
            latency_ns: [0.0; 4],
        };
        let expected_grid = grid.clone();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBurnIn { .. })) => {}
                other => panic!("stand-in expected StartBurnIn, got {other:?}"),
            }
            let started =
                encode_frame(&Message::Response(Response::BenchStarted { run_id: 7 }))
                    .expect("must encode");
            stream.write_all(&started).expect("stand-in write must not fail");
            for iteration in 1u32..=15 {
                let tick = encode_frame(&Message::Response(Response::BurnInProgress(
                    BurnInTick {
                        iteration,
                        elapsed_secs: f64::from(iteration) * 0.1,
                        tier: Tier::Memory,
                        bandwidth: Some((BenchOp::Read, f64::from(iteration))),
                        latency_ns: None,
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

        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let socket = sock.path().to_path_buf();
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = BenchCmd {
                    target: StreamTarget::Full,
                    mode: BenchMode::Full,
                    duration_minutes: Some(0),
                };
                run_burn_in(&socket, cmd, &state, &cancel)
                    .expect("run_burn_in must not error");
            })
        };

        // The main test thread (a stand-in for the egui render
        // thread): read continuously every ~10 ms during the run —
        // every read must complete promptly (the writer's critical
        // section is per-frame, µs — C14-03), and the mid-run
        // reads must observe the live tick state.
        let mut max_mid_run_iteration = 0u32;
        let mut last_iteration = 0u32;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let read_started = Instant::now();
            let (running, iteration, done) = {
                let s = state.read().expect("the poller must not poison the lock");
                (
                    s.bench.burn_in.running,
                    s.bench.burn_in.iteration,
                    s.bench.grid.is_some() && !s.bench.burn_in.running,
                )
            };
            let read_elapsed = read_started.elapsed();
            assert!(
                read_elapsed < Duration::from_millis(50),
                "a mid-run read must not block on the writer (took {read_elapsed:?})"
            );
            if running {
                assert!(
                    iteration >= last_iteration,
                    "the tick iteration must not go backwards mid-run"
                );
                if iteration > last_iteration {
                    max_mid_run_iteration = iteration;
                }
                last_iteration = iteration;
            }
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the in-flight burn-in never reached its terminal"
            );
            thread::sleep(Duration::from_millis(10));
        }
        worker.join().expect("run_burn_in must return at its terminal (no hang)");
        stand_in.join();

        // The phase-1 assertions take one brief, fully-scoped read
        // (the guard must not outlive this block — a long-lived
        // reader starves the next run's writer, C14-03).
        assert!(
            max_mid_run_iteration >= 2,
            "a mid-run read must observe at least two distinct ticks, saw {max_mid_run_iteration}"
        );
        {
            let s = state.read().expect("the poller must not poison the lock");
            assert!(!s.bench.burn_in.running, "the terminal must clear burn_in.running");
            assert_eq!(
                s.bench.grid,
                Some(expected_grid),
                "the terminal grid must land in the state"
            );
            assert!(s.error.is_none(), "a clean run must not record an error");
        }

        // Phase 2: the Cancel flag stops a non-self-terminating run —
        // the stand-in ticks for as long as the stream stays open
        // (its loop ends on the client's drop after the clean stop).
        let sock2 = TempSocket::new("burnin-inflight-cancel");
        let stand_in2 = DaemonStandIn::spawn(&sock2, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::StartBurnIn { .. })) => {}
                other => panic!("stand-in expected StartBurnIn, got {other:?}"),
            }
            let started =
                encode_frame(&Message::Response(Response::BenchStarted { run_id: 11 }))
                    .expect("must encode");
            stream.write_all(&started).expect("stand-in write must not fail");
            for iteration in 1u32..=60 {
                let tick = encode_frame(&Message::Response(Response::BurnInProgress(
                    BurnInTick {
                        iteration,
                        elapsed_secs: f64::from(iteration) * 0.1,
                        tier: Tier::Memory,
                        bandwidth: Some((BenchOp::Read, f64::from(iteration))),
                        latency_ns: None,
                    },
                )))
                .expect("must encode");
                if stream.write_all(&tick).is_err() {
                    break; // the client dropped (the run ended)
                }
                thread::sleep(Duration::from_millis(100));
            }
        });

        let socket2 = sock2.path().to_path_buf();
        let worker = {
            let state = Arc::clone(&state);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                let cmd = BenchCmd {
                    target: StreamTarget::Full,
                    mode: BenchMode::Full,
                    duration_minutes: Some(0),
                };
                run_burn_in(&socket2, cmd, &state, &cancel)
                    .expect("run_burn_in must not error");
            })
        };

        // Wait for the run to start (the ack's run_id + burn-in
        // state), then set the shared cancel flag. The read guard is
        // scoped to the check only (dropped before the sleep — a
        // continuous re-reading guard would starve the worker's
        // write request, which must land at run start).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let started = {
                let s = state.read().expect("the poller must not poison the lock");
                s.bench.burn_in.running && s.bench.run_id == Some(11)
            };
            if started {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the run never started (no BenchStarted within the deadline)"
            );
            thread::sleep(Duration::from_millis(10));
        }
        cancel.store(true, Ordering::Relaxed);
        worker.join().expect("run_burn_in must return on cancel (no hang)");
        stand_in2.join();

        let s = state.read().expect("the poller must not poison the lock");
        assert!(!s.bench.burn_in.running, "the cancel must clear burn_in.running");
        assert!(s.error.is_none(), "a clean cancel must not record an error");
    }

    // -----------------------------------------------------------------
    // The consent-gated probe-report request (chunk probe-3).
    // -----------------------------------------------------------------

    /// A minimal probe-report fixture (the `request_probe_report`
    /// stand-in tests' payload).
    fn fixture_probe_report() -> ProbeReport {
        ProbeReport {
            telemetry: mock_snapshot(),
            raw: None,
            system: ProbeSystem {
                cpu_brand: "GUI Test CPU".to_owned(),
                cpu_vendor: "unknown".to_owned(),
                cpu_gen: "Unknown".to_owned(),
                pci_host_bridge: None,
                kernel: "6.6.0-test".to_owned(),
                os: "Linux / Test".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.2".to_owned(),
                telemetry_source: "unavailable".to_owned(),
            },
        }
    }

    /// (q6) `request_probe_report` against a live stand-in (a
    /// `GetProbeReport` answered with a canned
    /// `Response::ProbeReport`) lands `probe_result = Ok(report)`.
    #[test]
    fn request_probe_report_against_a_live_stand_in_updates_state() {
        let sock = TempSocket::new("probe-report");
        let report = fixture_probe_report();
        let expected = report.clone();
        // Spawn (no join before the request — the existing stand-in
        // pattern: the handle detaches when the test ends, after the
        // handler has served the one connection).
        let _stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            let msg = read_one_message(&mut stream).expect("the request must arrive");
            assert!(
                matches!(msg, Message::Request(Request::GetProbeReport)),
                "the stand-in must receive a GetProbeReport"
            );
            let frame = encode_frame(&Message::Response(Response::ProbeReport(report)))
                .expect("must encode");
            stream.write_all(&frame).expect("the reply must write");
        });
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let _ = request_probe_report(sock.path(), &state);
        let guard = state.read().unwrap();
        match &guard.probe_result {
            ProbeResult::Ok(r) => {
                assert_eq!(*r, expected, "the landed report must match")
            }
            other => panic!("expected Ok(report), got {other:?}"),
        }
    }

    /// (q7) `request_probe_report` against a missing socket records a
    /// structured `Err` (the no-panic contract — the poller's loop
    /// survives a flapping daemon).
    #[test]
    fn request_probe_report_against_missing_socket_is_friendly() {
        let sock = TempSocket::new("probe-report-missing");
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let _ = request_probe_report(sock.path(), &state);
        let guard = state.read().unwrap();
        match &guard.probe_result {
            ProbeResult::Err(msg) => {
                assert!(!msg.is_empty(), "a friendly error text")
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
