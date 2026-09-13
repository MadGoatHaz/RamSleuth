//! The GUI's shared state + the background poller thread (P3-26, plan D6).
//!
//! The render thread (the eframe app shell, P3-30) never does I/O: it
//! reads snapshots of [`TelemetryData`] out of the `Arc<RwLock<…>>` that
//! [`spawn_poller`] hands it, and a single background thread is the only
//! place the GUI talks to the daemon (plan D6: no render-thread blocking
//! — 60 FPS stays feasible). That thread:
//!
//! - every 2 s runs one [`poll_telemetry`] cycle (a fresh
//!   [`Client::connect`] + `GetTelemetry` — a new connection each cycle
//!   survives a daemon restart, the TUI P3-24 precedent);
//! - serves benchmark requests from the [`BenchCmd`] channel (the app
//!   shell's bench-zone run buttons, P3-28) with [`run_bench`]: one
//!   `StartBenchmark` send, then the reply stream — a `BenchStarted`
//!   ack, the `BenchProgress` events, and exactly one terminal — into
//!   `state.bench`;
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
//! running against a flapping daemon. The `RwLock` is only ever written
//! from that one background thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ramsleuth_bench::{BenchmarkGrid, StreamProgress, StreamTarget};
use ramsleuth_client::Client;
use ramsleuth_protocol::{BenchMode, Request, Response};
use ramsleuth_telemetry::SystemMemoryTelemetry;

/// Telemetry poll cadence of the background loop (the plan's 2 s; a
/// fresh connection per cycle survives daemon restarts).
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(2);
/// Background-loop tick: short enough to keep an in-flight bench run's
/// progress frames responsive, cheap when idle.
const POLLER_TICK: Duration = Duration::from_millis(200);

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
    /// Streamed [`StreamProgress`] events of the current / last run
    /// (cleared when a new run starts).
    pub progress: Vec<StreamProgress>,
    /// The terminal result grid of the last completed run (`None`
    /// until the first `BenchResult`).
    pub grid: Option<BenchmarkGrid>,
}

/// The GUI's presentation state: the current telemetry snapshot, the
/// bench state, and the daemon connection status.
///
/// One `TelemetryData` lives behind an `Arc<RwLock<…>>` shared with the
/// render thread (P3-30); the background poller is its only writer.
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
/// single unit the background poller runs every 2 s.
///
/// Testable, no thread. The no-panic contract (plan D5): every failure
/// class (a missing / refused socket → the `ClientError::DaemonDown`
/// friendly "start it with …" text, a timeout, a protocol violation, an
/// i/o failure) is recorded in `state.error` with `daemon_status`
/// degrading to `disconnected`; a successful `Telemetry` arm stores the
/// snapshot, stamps `last_update`, reports `connected: <socket>`, and
/// clears the error; a structured `Error` reply records its message.
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
            state.telemetry = Some(telemetry);
            state.last_update = Some(Instant::now());
            state.daemon_status = format!("connected: {}", socket.display());
            state.error = None;
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

/// One benchmark run: connect to the daemon at `socket`, send the
/// `StartBenchmark` for `cmd`, and drain the reply stream — a
/// `BenchStarted` ack, the `BenchProgress` events, and exactly one
/// terminal — into `state.bench`.
///
/// Testable, no thread. The run starts with `running = true` and the
/// progress list cleared (a stale run's events never mix into a new
/// one); the terminal frame (`BenchResult` → the grid, `BenchCancelled`,
/// or the daemon's `Error`) sets `running = false`; a transport failure
/// (a closed stream, a timeout, …) or a contract-violating frame
/// records `state.error` and the same. Always returns `Ok(())` (the
/// no-panic contract, as in [`poll_telemetry`]) — the poller loop must
/// survive every failure.
pub fn run_bench(socket: &Path, cmd: BenchCmd, state: &mut TelemetryData) -> Result<(), String> {
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
    if let Err(error) = client.send(&Request::StartBenchmark {
        target: cmd.target,
        mode: cmd.mode,
    }) {
        state.bench.running = false;
        state.error = Some(error.to_string());
        return Ok(());
    }
    loop {
        match client.recv() {
            Ok(Response::BenchStarted { .. }) => {} // the ack: progress follows
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
// The background poller thread (the state's only writer).
// ---------------------------------------------------------------------

/// Spawn the background poller thread behind `state` (the plan D6
/// contract: the render thread only ever reads
/// `Arc<RwLock<TelemetryData>>`).
///
/// The loop: check `stop`; service at most one [`BenchCmd`] from
/// `bench_rx` (a run streams to its terminal before the next tick —
/// runs are single-flight daemon-side anyway, P3-15); otherwise poll
/// telemetry when the last poll is ≥ [`TELEMETRY_INTERVAL`] old (a
/// thread-local stamp, so a flapping daemon polls on the fixed cadence
/// instead of every tick); tick [`POLLER_TICK`] (200 ms, so bench
/// progress stays responsive); exit when `stop` is set or the channel
/// disconnects. The `RwLock` is written from this thread only, so the
/// `unwrap` is the workspace's one-writer precedent (TUI P3-24).
pub fn spawn_poller(
    socket: PathBuf,
    state: Arc<RwLock<TelemetryData>>,
    bench_rx: Receiver<BenchCmd>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Primed due: the first telemetry poll runs at once.
        let mut last_poll = Instant::now() - TELEMETRY_INTERVAL;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match bench_rx.try_recv() {
                Ok(cmd) => {
                    let _ = run_bench(&socket, cmd, &mut state.write().unwrap());
                }
                Err(TryRecvError::Empty) => {
                    if last_poll.elapsed() >= TELEMETRY_INTERVAL {
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
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};

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
        run_bench(sock.path(), cmd, &mut state).expect("run_bench must not error");

        assert!(!state.bench.running, "the terminal result must clear running");
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

        let handle = spawn_poller(sock.path().to_path_buf(), state.clone(), rx, stop.clone());
        stop.store(true, Ordering::Relaxed);
        handle.join().expect("the poller thread must not panic");

        // The lock is usable after the thread exits and no run is
        // left in flight.
        let state = state.read().expect("the poller must not poison the lock");
        assert!(!state.bench.running);
    }
}
