//! The GUI's shared state + the background poller thread (P3-26, plan D6).
//!
//! The render thread (the eframe app shell, P3-30) never does I/O: it
//! reads snapshots of [`TelemetryData`] out of the `Arc<RwLock<…>>` that
//! [`spawn_poller`] hands it, and a single background thread is the only
//! place the GUI talks to the daemon (plan D6: no render-thread blocking
//! — 60 FPS stays feasible). That thread:
//!
//! - at the live settings cadence (default 2 s — the
//!   `settings.poll_interval_ms` knob, C6-27) runs one
//!   [`poll_telemetry`] cycle (a fresh [`Client::connect`] +
//!   `GetTelemetry` — a new connection each cycle survives a daemon
//!   restart, the TUI P3-24 precedent), appending one trend-history
//!   sample to `state.history` per successful poll (the Na-guarded
//!   [`record_history_sample`] — C6-25) and clearing the series when
//!   the daemon reconnects;
//! - serves benchmark requests from the [`BenchCmd`] channel (the app
//!   shell's bench-zone run buttons, P3-28) with [`run_bench`]: one
//!   `StartBenchmark` send, then the reply stream — a `BenchStarted`
//!   ack, the `BenchProgress` events, and exactly one terminal — into
//!   `state.bench`;
//! - watches the shared `AtomicBool` cancel flag (the bench zone's
//!   Cancel button, P3-28): [`run_bench`] checks it before each
//!   frame — once set, the run is stopped daemon-side with a
//!   best-effort `CancelBenchmark` and ends with `running = false`;
//! - re-reads the live settings knobs (`poll_interval_ms` +
//!   `refresh_enabled`) every tick (C6-27) — a changed interval or
//!   the refresh gate takes effect without a poller restart; a
//!   disabled refresh idles the loop (no polling);
//! - ticks every 200 ms so an in-flight run's progress frames stay
//!   responsive, and stops on the shared `AtomicBool` (or when the
//!   channel disconnects).
//!
//! **No-panic contract (plan D5):** [`poll_telemetry`] and [`run_bench`]
//! take `&mut TelemetryData` (no thread, no lock — testable in
//! isolation) and **always return `Ok(())`**: every failure class
//! (a missing / refused socket → the `ClientError::DaemonDown`
//! "start it with …" text, a read timeout, a protocol violation, a
//! closed stream) is recorded in the state (`error` +
//! `daemon_status`) instead of propagating, so the caller's loop keeps
//! running against a flapping daemon. The `RwLock`'s data fields are
//! written from that one background thread only; the one
//! render-thread write is the settings panel mutating
//! `state.settings` (no I/O, D6 — C6-27), which the poller re-reads
//! every tick.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ramsleuth_bench::{BenchOp, BenchmarkGrid, StreamProgress, StreamTarget, Tier};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response};
use ramsleuth_telemetry::SystemMemoryTelemetry;

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

// ---------------------------------------------------------------------
// Shared state (the render thread reads only; one writer: the poller).
// ---------------------------------------------------------------------

/// The live benchmark state: a run in flight, its streamed progress
/// events, and the terminal result grid.
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
    /// until the first `BenchResult`).
    pub grid: Option<BenchmarkGrid>,
}

/// The GUI's presentation state: the current telemetry snapshot, the
/// bench state, the daemon connection status, the 10-minute
/// trend history (C6-25), and the in-memory settings knobs
/// (C6-26 / C6-27).
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
    /// The in-memory settings knobs (C6-26): the poll cadence
    /// (`poll_interval_ms`, default 2 s) + the refresh gate
    /// (`refresh_enabled`) the poller re-reads every tick (C6-27),
    /// and the units / theme knobs the zones consume (the settings
    /// panel wiring lands in C6-30). The render thread's one
    /// permitted write is the settings panel mutating these knobs
    /// (no I/O, D6); the poller is the only writer of every other
    /// field.
    pub settings: GuiSettings,
}

/// One benchmark request from the UI to the background poller (the
/// bench zone's run buttons, P3-28): the cell target + the scope.
#[derive(Debug, Clone, Copy)]
pub struct BenchCmd {
    /// Which cells run (`Full` / one `Tier` / one `Cell`).
    pub target: StreamTarget,
    /// The run scope (the daemon clamps it onto the target, P3-15).
    pub mode: BenchMode,
}

// ---------------------------------------------------------------------
// The two poll units (testable: no thread, no lock, always `Ok(())`).
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
            | Response::BenchCancelled { .. },
        ) => {
            // A benchmark frame in reply to `GetTelemetry` violates the
            // wire contract (the daemon streams those only to the owning
            // benchmark connection, P3-16): record a structured error.
            state.error = Some("unexpected response to GetTelemetry".to_owned());
        }
        Err(error) => {
            state.error = Some(error.to_string());
            state.daemon_status = "disconnected".to_owned();
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
/// Testable, no thread. The run starts with `running = true`, the
/// progress list cleared, and a stale `run_id` dropped (a stale run's
/// events never mix into a new one); the terminal frame (`BenchResult`
/// → the grid, `BenchCancelled`, or the daemon's `Error`) sets
/// `running = false`; a transport failure (a closed stream, a timeout,
/// …) or a contract-violating frame records `state.error` and the
/// same. Always returns `Ok(())` (the no-panic contract, as in
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
    state: &mut TelemetryData,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // The shared cancel flag is reset per run: a stale `true` from a
    // cancelled run must not abort this one before its first frame.
    cancel.store(false, Ordering::Relaxed);
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state.bench.running = false;
            state.error = Some(error.to_string());
            return Ok(());
        }
    };
    state.bench.running = true;
    state.bench.progress.clear();
    // The new run's id arrives with the `BenchStarted` ack.
    state.bench.run_id = None;
    if let Err(error) = client.send(&Request::StartBenchmark {
        target: cmd.target,
        mode: cmd.mode,
    }) {
        state.bench.running = false;
        state.error = Some(error.to_string());
        return Ok(());
    }
    loop {
        // The Cancel button set the shared flag: ask the daemon for a
        // clean stop (best-effort — the reply is never read) and break
        // before the next frame.
        if cancel.load(Ordering::Relaxed) {
            if let Some(run_id) = state.bench.run_id {
                let _ = client.send(&Request::CancelBenchmark { run_id });
            }
            state.bench.running = false;
            break;
        }
        match client.recv() {
            Ok(Response::BenchStarted { run_id }) => {
                state.bench.run_id = Some(run_id);
            }
            Ok(Response::BenchProgress(progress)) => {
                state.bench.progress.push(progress);
            }
            Ok(Response::BenchResult { grid, .. }) => {
                state.bench.grid = Some(grid);
                state.bench.running = false;
                break;
            }
            Ok(Response::BenchCancelled { .. }) => {
                state.bench.running = false;
                break;
            }
            Ok(Response::Error(message)) => {
                state.error = Some(message);
                state.bench.running = false;
                break;
            }
            Ok(Response::Telemetry(_)) => {
                // A telemetry frame in the bench stream violates the
                // wire contract (`GetTelemetry` is served on its own
                // connection, P3-16): stop the run and record it.
                state.error = Some("unexpected response during benchmark".to_owned());
                state.bench.running = false;
                break;
            }
            Err(error) => {
                state.error = Some(error.to_string());
                state.bench.running = false;
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
/// runs are single-flight daemon-side anyway, P3-15) handing the
/// shared `cancel` flag to [`run_bench`] (the bench zone's Cancel
/// button sets it, P3-28; `run_bench` resets it per run); otherwise
/// re-read the live settings knobs (C6-27) — the clamped
/// `poll_interval_ms` + the `refresh_enabled` gate — and poll
/// telemetry when the last poll is ≥ the clamped interval old (a
/// thread-local stamp, so a flapping daemon polls on the configured
/// cadence instead of every tick; a disabled refresh idles the loop,
/// and a changed knob takes effect on the next tick — no restart);
/// tick [`POLLER_TICK`] (200 ms, so bench progress stays responsive);
/// exit when `stop` is set or the channel disconnects. The `RwLock`'s
/// data fields are written from this thread only, so the `unwrap` is
/// the workspace's one-writer precedent (TUI P3-24).
pub fn spawn_poller(
    socket: PathBuf,
    state: Arc<RwLock<TelemetryData>>,
    bench_rx: Receiver<BenchCmd>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Primed due at the default cadence: the first telemetry
        // poll runs at once (the live knob read below governs every
        // subsequent due-check — C6-27).
        let mut last_poll =
            Instant::now() - Duration::from_millis(DEFAULT_POLL_INTERVAL_MS);
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match bench_rx.try_recv() {
                Ok(cmd) => {
                    let _ = run_bench(&socket, cmd, &mut state.write().unwrap(), &cancel);
                }
                Err(TryRecvError::Empty) => {
                    // The live settings knobs (C6-27): re-read every
                    // tick — a changed interval (clamped) or the
                    // refresh gate takes effect without a restart.
                    let (interval, refresh_enabled) = {
                        let settings = state.read().unwrap();
                        (
                            clamp_poll_interval(settings.settings.poll_interval_ms),
                            settings.settings.refresh_enabled,
                        )
                    };
                    if refresh_enabled && last_poll.elapsed() >= interval {
                        last_poll = Instant::now();
                        let _ = poll_telemetry(&socket, &mut state.write().unwrap());
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
    use std::process;
    use std::sync::mpsc;

    use ramsleuth_bench::{BenchOp, Tier};
    use ramsleuth_protocol::{FrameError, Message, decode_frame, encode_frame};
    use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
    use ramsleuth_telemetry::amd_readout::map_amd;
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemPlatform;

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
                "ramsleuth-gui-{name}-{}.sock",
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

        let cmd = BenchCmd { target: StreamTarget::Tier(Tier::Memory), mode: BenchMode::Full };
        // A false cancel flag: this run is never cancelled (it is
        // reset per run anyway).
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = TelemetryData::default();
        // A stale pre-run progress entry must be cleared at run start.
        state.bench.progress.push(StreamProgress {
            cell_index: 9,
            total_cells: 12,
            tier: Tier::L3,
            op: BenchOp::Copy,
            value: 1.0,
            label: "stale".to_owned(),
        });
        // A stale pre-run id must be dropped at run start.
        state.bench.run_id = Some(99);
        run_bench(sock.path(), cmd, &mut state, &cancel).expect("run_bench must not error");

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
    /// tests above — no extensive real-thread assertions).
    #[test]
    fn spawn_poller_returns_a_handle_that_stops_cleanly() {
        let sock = TempSocket::new("poller");
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let (_tx, rx) = mpsc::channel::<BenchCmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));

        let handle = spawn_poller(
            sock.path().to_path_buf(),
            state.clone(),
            rx,
            stop.clone(),
            cancel.clone(),
        );
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
                let cmd = BenchCmd { target: StreamTarget::Full, mode: BenchMode::Full };
                run_bench(&socket, cmd, &mut state.write().unwrap(), &cancel)
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

        // The shared state starts with a short (clamp-floor)
        // interval + refresh on — the poller reads these live, per
        // tick (the settings panel's one render-thread write, D6).
        let state = Arc::new(RwLock::new(TelemetryData {
            settings: GuiSettings {
                poll_interval_ms: MIN_POLL_INTERVAL_MS,
                ..Default::default()
            },
            ..Default::default()
        }));
        let (bench_tx, bench_rx) = mpsc::channel::<BenchCmd>();
        let cancel = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(
            sock.path().to_path_buf(),
            state.clone(),
            bench_rx,
            stop.clone(),
            cancel,
        );

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
}
