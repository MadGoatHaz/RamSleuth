//! Unit tests for the pure / testable parts of P3-24 (no real
//! terminal — the draw/poll loop needs a TTY and is verified in the
//! QA phase): `parse_args` driven directly, and `poll_once` against
//! an in-process daemon stand-in (a `std::thread` binding a
//! `UnixListener` on a unique temp socket, the ramsleuth-client
//! P3-18 precedent) plus a non-existent socket path.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process;
use std::sync::atomic::AtomicUsize;
use std::thread;

use ramsleuth_bench::{BenchmarkGrid, StreamProgress};
use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message};
use ramsleuth_telemetry::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
use ramsleuth_telemetry::amd_readout::map_amd;
use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::SystemMemoryTelemetry;
use ramsleuth_telemetry::SystemPlatform;
use ramsleuth_telemetry::{RailConfidence, VoltageTelemetry};

use super::*;

/// TC-FE3 (the poison-cascade confirming test): poison the shared-state
/// lock (a panic while holding a write guard), then a subsequent read and
/// write **through the [`state_read`] / [`state_write`] helpers** recover
/// the guard via `into_inner` **without** re-panicking — one stray panic
/// degrades a single access, it never cascades across both threads and
/// takes the whole UI down. Pre-fix the bare `.read().unwrap()` /
/// `.write().unwrap()` panicked on the poisoned lock (the cascade).
#[test]
fn tc_fe3_poisoned_lock_recovers_without_cascade() {
    use std::panic;
    let state = Arc::new(RwLock::new(AppState::default()));
    // Poison the lock: panic while holding a write guard (the value is
    // set first so the recovery has something meaningful to assert).
    let poisoned = Arc::clone(&state);
    let _ = panic::catch_unwind(panic::AssertUnwindSafe(move || {
        let mut guard = poisoned.write().unwrap();
        guard.error = Some("panic-while-holding".to_owned());
        panic!("poison the lock");
    }));
    assert!(
        state.is_poisoned(),
        "the lock must be poisoned after the panic"
    );
    // A read through the helper recovers the guard (no cascade panic) and
    // sees the value the panicking writer set.
    let app = state_read(&state);
    assert_eq!(
        app.error.as_deref(),
        Some("panic-while-holding"),
        "read recovers the last-good value"
    );
    drop(app);
    // A write through the helper also recovers (no cascade panic).
    let mut app = state_write(&state);
    app.error = Some("recovered".to_owned());
    drop(app);
    // The recovered value is readable and mutated as expected.
    assert_eq!(state_read(&state).error.as_deref(), Some("recovered"));
}

/// A unique temp socket path owned by a guard that removes the file
/// on drop (best-effort cleanup; the path is pid-qualified so
/// parallel test runs / processes never collide).
struct TempSocket {
    path: PathBuf,
}

impl TempSocket {
    fn new(name: &str) -> Self {
        let path = PathBuf::from(format!("/tmp/ramsleuth-tui-{name}-{}.sock", process::id()));
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
        vcore: Section::na(NaReason::NotApplicable),
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
        pm_exposed: true,
        smn_setpoint_unverified: false,
        threadripper: false,
        siod: false,
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
        // The vCore (Chunk 3): the AMD readout's 1150 mV vcore,
        // promoted to the snapshot-root's `vcore` field (mirroring
        // the facade's AMD promotion) so the vcore series lands.
        vcore: Section::Value(VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: 1.150,
            requested_vid: None,
            delta_volts: None,
            confidence: RailConfidence::VerifiedProfile,
        }),
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
    assert!(
        err.contains("--bogus"),
        "the error must name the flag: {err}"
    );
}

/// (d) A positional argument is rejected with its name (the TUI
/// takes no subcommand).
#[test]
fn positional_is_rejected() {
    let err = parse_args(["dump"]).expect_err("a positional must be rejected");
    assert!(
        err.contains("dump"),
        "the error must name the argument: {err}"
    );
}

/// (e) `--socket` with a missing value is rejected with a message
/// naming the flag.
#[test]
fn missing_socket_value_is_rejected() {
    let err = parse_args(["--socket"]).expect_err("`--socket` without a value must be rejected");
    assert!(
        err.contains("--socket"),
        "the error must name the flag: {err}"
    );
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
    let args: Vec<String> = ["--socket", "/tmp/y.sock"]
        .iter()
        .copied()
        .map(str::to_owned)
        .collect();
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
        let bytes = encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
            .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
    });

    // A pre-existing error (initialized here, not reassigned after)
    // must be cleared by a successful poll.
    let mut state = AppState {
        error: Some("stale error".to_owned()),
        ..Default::default()
    };
    poll_once(sock.path(), &mut state).expect("poll_once must not error");

    assert_eq!(
        state.telemetry.as_ref().map(|t| t.cpu.brand.as_str()),
        Some("TUI Test CPU"),
        "the stand-in's canned snapshot must land in the state"
    );
    assert!(
        state.error.is_none(),
        "a successful poll must clear the error"
    );
    let status = &state.daemon_status;
    assert!(
        status.contains("connected"),
        "the status must report the connection, got: {status}"
    );
    assert!(
        state.last_update.is_some(),
        "a successful poll must stamp last_update"
    );
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
            let bytes = encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
                .expect("must encode");
            stream
                .write_all(&bytes)
                .expect("stand-in write must not fail");
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
        let status = state
            .read()
            .expect("the updater must not poison the lock")
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
        let bytes = encode_frame(&Message::Response(
            Response::Telemetry(populated_snapshot()),
        ))
        .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
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
            sample.vddcr_soc_mv, 1050.0,
            "the fixture's SOC rail lands in mV on every sample"
        );
        assert_eq!(
            sample.vddcr_cpu_mv, 1150.0,
            "the fixture's vcore rail lands in mV on every sample"
        );
        assert!(
            sample.cpu_freq_mhz.is_nan(),
            "the all-Na platform keeps the freq series no-source (D-4)"
        );
        assert_eq!(
            sample.bandwidth_gbps, 26.35,
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
        let bytes = encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
            .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
    });
    let mut state = AppState::default();
    poll_once(sock.path(), &mut state).expect("poll_once must not error");
    stand_in.join();

    // This host's temp scan (the same I/O the poll ran — the
    // sensor state is stable across the two reads): it decides
    // the sample's shape.
    let temp = ramsleuth_tui::read_cpu_temp_c();
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
            sample.cpu_temp_c.is_finite() && sample.cpu_temp_c > -50.0 && sample.cpu_temp_c < 150.0,
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
        let bytes = encode_frame(&Message::Response(
            Response::Telemetry(populated_snapshot()),
        ))
        .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
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
        let bytes = encode_frame(&Message::Response(
            Response::Telemetry(populated_snapshot()),
        ))
        .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
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
    assert_eq!(
        latest_memory_read_bw(&idle),
        0.0,
        "no bench data at all → 0.0"
    );

    // (2) Terminal grid only (row 0 = the Memory read cell).
    let grid_only = BenchState {
        grid: Some(grid.clone()),
        ..Default::default()
    };
    assert_eq!(
        latest_memory_read_bw(&grid_only),
        26.35,
        "the terminal grid cell"
    );

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
            stream
                .write_all(&bytes)
                .expect("stand-in write must not fail");
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
    assert!(
        !state.bench.running,
        "the terminal result must clear running"
    );
    assert_eq!(
        state.bench.run_id,
        Some(1),
        "the ack's run_id must be recorded"
    );
    assert_eq!(
        state.bench.progress, expected_progress,
        "exactly the streamed events; the stale entry is cleared"
    );
    assert_eq!(
        state.bench.grid,
        Some(expected_grid),
        "the terminal grid must land in the state"
    );
    assert!(
        state.error.is_none(),
        "a clean run must not record an error"
    );
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
        let started = encode_frame(&Message::Response(Response::BenchStarted { run_id: 42 }))
            .expect("must encode");
        stream
            .write_all(&started)
            .expect("stand-in write must not fail");
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
            run_bench(&socket, cmd, &state, &cancel).expect("run_bench must not error");
        })
    };

    // Wait for the run to start (the ack's run_id lands in the
    // state), then set the shared cancel flag.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if state
            .read()
            .expect("the updater must not poison the lock")
            .bench
            .run_id
            .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the run never started (no BenchStarted within 5 s)"
        );
        thread::sleep(Duration::from_millis(10));
    }
    cancel.store(true, Ordering::Relaxed);
    worker
        .join()
        .expect("run_bench must return on cancel (no hang)");

    let state = state.read().expect("the updater must not poison the lock");
    assert!(!state.bench.running, "the cancel must clear running");
    assert_eq!(
        state.bench.run_id,
        Some(42),
        "the run id must stay recorded"
    );
    assert!(
        state.error.is_none(),
        "a clean cancel must not record an error"
    );
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
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
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
    assert!(
        state.bench.grid.is_none(),
        "an errored run lands no terminal grid"
    );
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
    assert!(
        !state.bench.running,
        "a failed connect must not leave a run in flight"
    );
    assert!(state.bench.progress.is_empty(), "no progress without a run");
    let error = state
        .error
        .clone()
        .expect("a friendly error must be recorded");
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
        let started = encode_frame(&Message::Response(Response::BenchStarted { run_id: 7 }))
            .expect("must encode");
        stream
            .write_all(&started)
            .expect("stand-in write must not fail");
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
            stream
                .write_all(&tick)
                .expect("stand-in write must not fail");
            thread::sleep(Duration::from_millis(100));
        }
        let result = encode_frame(&Message::Response(Response::BenchResult {
            run_id: 7,
            grid,
        }))
        .expect("must encode");
        stream
            .write_all(&result)
            .expect("stand-in write must not fail");
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
            run_bench(&socket, cmd, &state, &cancel).expect("run_bench must not error");
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
    worker
        .join()
        .expect("run_bench must return at its terminal (no hang)");
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
            Some(Message::Request(Request::StartBurnIn {
                target,
                duration_minutes,
            })) => {
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
            // F-06: (None, None) elapsed-update tick — the burn-in
            // fix arm. `iteration` unchanged, `elapsed_secs` advances,
            // no error, stream continues.
            Response::BurnInProgress(BurnInTick {
                iteration: 1,
                elapsed_secs: 2.5,
                tier: Tier::Memory,
                bandwidth: None,
                latency_ns: None,
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
            stream
                .write_all(&bytes)
                .expect("stand-in write must not fail");
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
    assert!(
        !state.bench.burn_in.running,
        "the terminal must clear burn_in.running"
    );
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
    assert_eq!(
        state.bench.run_id,
        Some(9),
        "the ack's run_id must be recorded"
    );
    assert_eq!(
        state.bench.burn_in.iteration, 2,
        "the newest tick's iteration"
    );
    assert_eq!(
        state.bench.burn_in.elapsed_secs, 3.5,
        "the newest tick's elapsed"
    );
    assert_eq!(
        state.bench.burn_in.latest.read_gbps[0], 52.5,
        "the newest value per cell wins (the stale 7.0 is reset)"
    );
    assert_eq!(
        state.bench.burn_in.latest.latency_ns[1], 1.2,
        "the latency tick lands in the tier's latency cell (L1 = slot 1)"
    );
    assert_eq!(
        state.bench.burn_in.latest.write_gbps, [0.0; 4],
        "an unticked column stays zero"
    );
    assert_eq!(
        state.bench.burn_in.latest.copy_gbps, [0.0; 4],
        "an unticked column stays zero"
    );
    assert_eq!(
        state.bench.grid,
        Some(expected_grid),
        "the terminal grid must land in the state"
    );
    assert!(
        state.error.is_none(),
        "a clean run must not record an error"
    );
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
            Some(Message::Request(Request::StartBurnIn {
                target,
                duration_minutes,
            })) => {
                assert_eq!(
                    target,
                    StreamTarget::Full,
                    "the cmd's target must ride the wire"
                );
                assert_eq!(
                    duration_minutes, 0,
                    "the infinite (0) duration must ride the wire"
                );
            }
            other => panic!("stand-in expected StartBurnIn, got {other:?}"),
        }
        let started = encode_frame(&Message::Response(Response::BenchStarted { run_id: 42 }))
            .expect("must encode");
        stream
            .write_all(&started)
            .expect("stand-in write must not fail");
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
            run_burn_in(&socket, cmd, &state, &cancel).expect("run_burn_in must not error");
        })
    };

    // Wait for the run to start (the ack's run_id lands in the
    // state), then set the shared cancel flag.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if state
            .read()
            .expect("the updater must not poison the lock")
            .bench
            .run_id
            .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the run never started (no BenchStarted within 5 s)"
        );
        thread::sleep(Duration::from_millis(10));
    }
    cancel.store(true, Ordering::Relaxed);
    worker
        .join()
        .expect("run_burn_in must return on cancel (no hang)");

    let state = state.read().expect("the updater must not poison the lock");
    assert!(
        !state.bench.burn_in.running,
        "the cancel must clear burn_in.running"
    );
    assert_eq!(
        state.bench.run_id,
        Some(42),
        "the run id must stay recorded"
    );
    assert!(
        state.error.is_none(),
        "a clean cancel must not record an error"
    );
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
                assert!(Instant::now() < deadline, "the cancel flag was never set");
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
            run_burn_in(&socket, cmd, &state, &cancel).expect("run_burn_in must not error");
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
    worker
        .join()
        .expect("run_burn_in must return on the pre-ack cancel (no hang)");
    stand_in.join();

    assert!(
        !seen_cancel.load(Ordering::Relaxed),
        "a pre-ack cancel must send no CancelBenchmark (no run id yet)"
    );
    let state = state.read().expect("the updater must not poison the lock");
    assert!(
        !state.bench.burn_in.running,
        "the cancel must clear burn_in.running"
    );
    assert_eq!(state.bench.run_id, None, "the ack never landed — no run id");
    assert!(
        state.error.is_none(),
        "a clean cancel must not record an error"
    );
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
        let started = encode_frame(&Message::Response(Response::BenchStarted { run_id: 3 }))
            .expect("must encode");
        stream
            .write_all(&started)
            .expect("stand-in write must not fail");
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
        stream
            .write_all(&progress)
            .expect("stand-in write must not fail");
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
    assert!(
        !state.bench.burn_in.running,
        "the violation must clear burn_in.running"
    );
    let error = state
        .error
        .as_ref()
        .expect("the violation must record a structured error");
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
    assert!(
        !state.bench.burn_in.running,
        "a failed connect must not leave a burn-in in flight"
    );
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
    let error = state
        .error
        .clone()
        .expect("a friendly error must be recorded");
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
    assert!(
        !run_in_flight(&BenchState::default()),
        "idle: no run in flight"
    );
    let bench = BenchState {
        running: true,
        ..Default::default()
    };
    assert!(run_in_flight(&bench), "a normal bench in flight");
    let burn = BenchState {
        burn_in: ramsleuth_tui::ui::BurnInState {
            running: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(run_in_flight(&burn), "a burn-in in flight");
    let both = BenchState {
        running: true,
        burn_in: ramsleuth_tui::ui::BurnInState {
            running: true,
            ..Default::default()
        },
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
                        let started =
                            encode_frame(&Message::Response(Response::BenchStarted { run_id: 1 }))
                                .expect("must encode");
                        stream
                            .write_all(&started)
                            .expect("stand-in write must not fail");
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
                        stream
                            .write_all(&result)
                            .expect("stand-in write must not fail");
                    }
                    Some(Message::Request(Request::StartBurnIn {
                        target,
                        duration_minutes,
                    })) => {
                        assert_eq!(
                            target,
                            StreamTarget::Full,
                            "the burn-in cmd's target must ride the wire"
                        );
                        assert_eq!(
                            duration_minutes, 7,
                            "the burn-in cmd's duration must ride the wire"
                        );
                        seen.fetch_or(2, Ordering::Relaxed);
                        let started =
                            encode_frame(&Message::Response(Response::BenchStarted { run_id: 2 }))
                                .expect("must encode");
                        stream
                            .write_all(&started)
                            .expect("stand-in write must not fail");
                        let cancelled =
                            encode_frame(&Message::Response(Response::BenchCancelled {
                                run_id: 2,
                            }))
                            .expect("must encode");
                        stream
                            .write_all(&cancelled)
                            .expect("stand-in write must not fail");
                    }
                    Some(Message::Request(Request::GetTelemetry)) => {
                        let bytes =
                            encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
                                .expect("must encode");
                        stream
                            .write_all(&bytes)
                            .expect("stand-in write must not fail");
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
    assert!(
        !state.bench.running,
        "the normal run must end at its terminal"
    );
    assert!(
        !state.bench.burn_in.running,
        "the burn-in must end at its terminal"
    );
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
        let path = std::env::temp_dir().join(format!("ramsleuth-tui-{name}-{}", process::id()));
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
    let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
    assert!(
        value.is_object(),
        "a snapshot must serialize to a JSON object"
    );
    let keys = value
        .as_object()
        .expect("checked is_object")
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let expected: std::collections::BTreeSet<String> = [
        "amd",
        "bench",
        "cpu",
        "intel",
        "spd",
        "platform",
        "total_capacity",
        "dimm_sizes",
        "vcore",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        keys, expected,
        "the eight telemetry wire keys (vcore appended, Chunk 3) + the bench key"
    );

    // The C6-29 `bench` key carries the grid verbatim.
    let back_grid: BenchmarkGrid =
        serde_json::from_value(value["bench"].clone()).expect("bench must re-parse into the grid");
    assert_eq!(back_grid, grid, "the grid round-trips");

    // The flattened telemetry sub-object is byte-compatible:
    // every one of the seven wire keys matches the plain wire
    // root serialization.
    let base = serde_json::to_value(&telemetry).expect("the wire root must serialize");
    for key in [
        "cpu",
        "amd",
        "intel",
        "spd",
        "platform",
        "total_capacity",
        "dimm_sizes",
    ] {
        assert_eq!(
            value[key], base[key],
            "the {key} key must stay byte-compatible"
        );
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
    let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
    assert!(
        value.is_object(),
        "a snapshot must serialize to a JSON object"
    );
    assert!(
        value["bench"].is_null(),
        "no run yet: the bench key is the JSON null"
    );
    let base = serde_json::to_value(&telemetry).expect("the wire root must serialize");
    for key in [
        "cpu",
        "amd",
        "intel",
        "spd",
        "platform",
        "total_capacity",
        "dimm_sizes",
    ] {
        assert_eq!(
            value[key], base[key],
            "the {key} key must stay byte-compatible"
        );
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
    let value: serde_json::Value = serde_json::from_str(&text).expect("export must be valid JSON");
    let back_grid: BenchmarkGrid =
        serde_json::from_value(value["bench"].clone()).expect("bench must re-parse into the grid");
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
    let err = export_json(&telemetry, None, &path).expect_err("a missing parent dir must fail");
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
        bench: BenchState {
            grid: Some(grid.clone()),
            ..Default::default()
        },
        ..Default::default()
    };

    let path = perform_export(&state, dir.path())
        .expect("the export must not fail")
        .expect("telemetry set must produce a file");
    assert!(
        path.starts_with(dir.path()),
        "the file must land in the out dir: {path:?}"
    );
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .expect("the file must be named");
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
    assert!(
        value.is_object(),
        "a snapshot must serialize to a JSON object"
    );
    let back_grid: BenchmarkGrid =
        serde_json::from_value(value["bench"].clone()).expect("bench must re-parse into the grid");
    assert_eq!(back_grid, grid, "the terminal grid is exported");
}

/// (bg) `perform_export` with no telemetry → `Ok(None)` and no
/// file is written (the GUI `main.rs` (d) mirror).
#[test]
fn perform_export_without_telemetry_is_none() {
    let dir = TempOutDir::new("perform-export-none");
    let state = AppState::default(); // no snapshot yet

    let result = perform_export(&state, dir.path()).expect("no telemetry must not fail");
    assert!(result.is_none(), "no telemetry: no file (the GUI rule)");
    let count = std::fs::read_dir(dir.path())
        .expect("the dir must be readable")
        .count();
    assert_eq!(count, 0, "no telemetry: nothing was written to the out dir");
}

/// (bh) `perform_export` maps a write failure to a `String` (a
/// missing parent dir under the out dir): `Err`, no file, no
/// panic — the `GuiError::Io` class without the enum.
#[test]
fn perform_export_maps_write_failure_to_a_string() {
    let dir = TempOutDir::new("perform-export-fail");
    let state = AppState {
        telemetry: Some(mock_snapshot()),
        ..Default::default()
    };
    // Nest the out dir one level below the temp dir: the missing
    // parent makes the write fail.
    let nested = dir.path().join("no-such-dir");
    let err = perform_export(&state, &nested).expect_err("a missing parent dir must fail");
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
        state.daemon_status, "export: /tmp/ramsleuth-export-1.json",
        "the written path lands in daemon_status (transient)"
    );
    assert!(
        state.error.is_none(),
        "a written export must not record an error"
    );

    record_export_result(&mut state, Ok(None));
    assert_eq!(
        state.daemon_status, "nothing to export yet (no telemetry)",
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
    assert_eq!(
        cycle_poll(1_500),
        2_000,
        "between 1 s and 2 s it lands on 2 s"
    );
    assert_eq!(
        cycle_poll(59_000),
        60_000,
        "between 30 s and 60 s it lands on 60 s"
    );
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
    assert_eq!(
        cycle_window(2),
        5,
        "an off-preset value lands on the next preset above"
    );
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
        dispatch_action(
            Action::ToggleCapacity,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
        SideEffect::None
    );
    assert!(!state.settings.capacity_gib, "GiB → GB");
    assert_eq!(
        dispatch_action(
            Action::ToggleCapacity,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
        SideEffect::None
    );
    assert!(state.settings.capacity_gib, "GB → GiB (round-trip)");

    assert!(state.settings.clock_mhz, "the MHz default");
    assert_eq!(
        dispatch_action(
            Action::ToggleClock,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
        SideEffect::None
    );
    assert!(!state.settings.clock_mhz, "MHz → GHz");
    assert_eq!(
        dispatch_action(
            Action::ToggleClock,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
        SideEffect::None
    );
    assert!(state.settings.clock_mhz, "GHz → MHz (round-trip)");

    assert!(state.settings.refresh, "the TUI's refresh-on default");
    assert_eq!(
        dispatch_action(
            Action::ToggleRefresh,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
        SideEffect::None
    );
    assert!(
        !state.settings.refresh,
        "refresh on → off (the poller freezes the cadence)"
    );
    assert_eq!(
        dispatch_action(
            Action::ToggleRefresh,
            &mut state,
            &mut dismissed,
            &tx,
            &cancel
        ),
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
        state_read(&state).settings.requirements_open,
        "the auto-open opens the strip while a requirement is present"
    );

    // `[d]` (the strip is open): close + dismiss.
    {
        let mut s = state_write(&state);
        assert_eq!(
            dispatch_action(
                Action::ToggleRequirements,
                &mut s,
                &mut dismissed,
                &tx,
                &cancel
            ),
            SideEffect::None
        );
    }
    assert!(dismissed, "closing dismisses the auto-open (the latch)");
    assert!(
        !state_read(&state).settings.requirements_open,
        "the strip is closed"
    );
    apply_requirements_auto_open(&state, dismissed);
    assert!(
        !state_read(&state).settings.requirements_open,
        "a dismissed strip stays closed while the requirement persists"
    );

    // `[d]` again (the strip is closed): reopen + clear the
    // dismissal (the auto-open applies again).
    {
        let mut s = state_write(&state);
        assert_eq!(
            dispatch_action(
                Action::ToggleRequirements,
                &mut s,
                &mut dismissed,
                &tx,
                &cancel
            ),
            SideEffect::None
        );
    }
    assert!(!dismissed, "reopening clears the dismissal");
    assert!(
        state_read(&state).settings.requirements_open,
        "the strip is open again"
    );
    apply_requirements_auto_open(&state, dismissed);
    assert!(
        state_read(&state).settings.requirements_open,
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
        !state_read(&clean).settings.requirements_open,
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
        bench: BenchState {
            running: true,
            ..Default::default()
        },
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
    assert!(
        rx.try_recv().is_err(),
        "no command was queued while a run is in flight"
    );

    // A burn-in in flight (the compound guard's other class):
    // the same skip.
    let mut state = AppState {
        bench: BenchState {
            burn_in: ramsleuth_tui::ui::BurnInState {
                running: true,
                ..Default::default()
            },
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
    let memory = rx
        .try_recv()
        .expect("the `[m]` memory-bench cmd was queued");
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
        let action = key_to_action(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE))?;
        apply_requirements_auto_open(state, *dismissed);
        Some({
            let mut s = state_write(state);
            dispatch_action(action, &mut s, dismissed, tx, cancel)
        })
    }

    let keys = [
        'g', 'p', 'u', 'k', 'a', 'w', 't', 'd', 'b', 'm', 'x', 'c', 'e', 's', 'r', 'z', 'q',
    ];
    let mut sides = Vec::new();
    for key in keys {
        sides.push(step(&state, key, &mut dismissed, &tx, &cancel));
    }
    assert_eq!(
        sides,
        vec![
            // g p u k a w t d b m x c: the state-only keys
            // (`Some(SideEffect::None)` — they dispatched, no I/O)
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
            Some(SideEffect::None),
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

    let s = state_read(&state);
    assert!(s.settings.graphs_open, "one `[g]` opens the graphs overlay");
    assert_eq!(
        s.settings.poll_interval_ms, 5_000,
        "one `[p]` advances 2 s → 5 s"
    );
    assert!(!s.settings.capacity_gib, "one `[u]` flips GiB → GB");
    assert!(!s.settings.clock_mhz, "one `[k]` flips MHz → GHz");
    assert!(!s.settings.refresh, "one `[a]` freezes the periodic poll");
    assert_eq!(
        s.settings.graph_window_min, 15,
        "one `[w]` advances 5 → 15 min"
    );
    assert!(
        s.settings.settings_open,
        "one `[t]` opens the settings strip"
    );
    assert!(
        !s.settings.requirements_open,
        "the `[d]` closed the strip the auto-open had opened"
    );
    assert!(
        dismissed,
        "the `[d]` close dismissed the auto-open (the latch)"
    );

    // The three run keys queued exactly their commands (in
    // order) — the state stayed idle, so none was skipped.
    let full = rx.try_recv().expect("the `[b]` full-bench cmd was queued");
    assert_eq!(
        (full.target, full.mode, full.duration_minutes),
        (StreamTarget::Full, BenchMode::Full, None)
    );
    let memory = rx
        .try_recv()
        .expect("the `[m]` memory-bench cmd was queued");
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
    assert!(
        cancel.load(Ordering::Relaxed),
        "the `[c]` key set the shared cancel flag"
    );
}

/// (bq) Every one of the 17 actions dispatches a defined effect
/// (the no-op placeholder arms are gone — the plan's exit
/// criterion): the four I/O actions report their side effect,
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
        (Action::ProbeReport, SideEffect::ProbeReport),
    ];
    assert_eq!(
        expected.len(),
        17,
        "the full frozen table (P3-22 + TUI-01/02 + chunk probe-4)"
    );
    for (action, side_effect) in expected {
        let side = dispatch_action(action, &mut state, &mut dismissed, &tx, &cancel);
        assert_eq!(
            side, side_effect,
            "{action:?} must dispatch its defined effect"
        );
    }
}

/// (br) The USAGE text carries the full 17-key table (plan
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
        "[D]info",
        "[P]oll",
        "[U]nits",
        "[K]lock",
        "[A]uto refresh",
        "[W]indow",
        "[F] probe report",
    ] {
        assert!(USAGE.contains(entry), "the USAGE must list {entry}");
    }
    assert!(
        USAGE.contains("Exit codes: 0 quit, 1 terminal init failure, 2 usage error"),
        "the exit-code note is unchanged"
    );
    // The capability description + the new options surface.
    for entry in [
        "17-key",
        "ramsleuth-daemon over the Unix",
        "CAP_SYS_RAWIO",
        "-h, --help",
        "-V, --version",
    ] {
        assert!(USAGE.contains(entry), "the USAGE must list {entry}");
    }
}

// ------------------------------------------------------------------
// chunk probe-4: the probe-report fetch + the modal overlay keys.
// ------------------------------------------------------------------

/// A probe-report fixture (host-independent — the chunk-1a wire
/// shapes; the assertions target the renderer's header, not its
/// full body).
fn probe_fixture() -> ramsleuth_telemetry::ProbeReport {
    ramsleuth_telemetry::ProbeReport {
        telemetry: mock_snapshot(),
        raw: None,
        system: ramsleuth_telemetry::ProbeSystem {
            cpu_brand: "TUI Test CPU".to_owned(),
            cpu_vendor: "unknown".to_owned(),
            cpu_gen: "Unknown".to_owned(),
            pci_host_bridge: None,
            kernel: "6.6.0-test".to_owned(),
            os: "Linux / Test".to_owned(),
            arch: "x86_64".to_owned(),
            ramsleuth_version: "2.4.13".to_owned(),
            telemetry_source: "unavailable".to_owned(),
        },
    }
}

/// A unique temp dir (pid-qualified — parallel test runs never
/// collide; removed best-effort on drop, the `TempSocket`
/// precedent).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ramsleuth-tui-probe-{name}-{}", process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("test dir must create");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// One plain (no-modifier) key event (the events.rs helper shape).
fn overlay_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// (ps) The fetch against a stand-in that answers
/// `GetProbeReport` with a canned report opens the consent
/// overlay with the rendered markdown (the `[y]` move needs no
/// second fetch — the markdown rides the state).
#[test]
fn probe_report_fetch_opens_consent_overlay() {
    let sock = TempSocket::new("probe-fetch");
    let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
        match read_one_message(&mut stream) {
            Some(Message::Request(Request::GetProbeReport)) => {}
            other => panic!("stand-in expected GetProbeReport, got {other:?}"),
        }
        let bytes = encode_frame(&Message::Response(Response::ProbeReport(probe_fixture())))
            .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
    });

    let state = Arc::new(RwLock::new(AppState::default()));
    probe_report_fetch(sock.path(), &state);

    let app = state_read(&state);
    match &app.probe {
        ProbeOverlayState::Consent { markdown } => {
            assert!(
                markdown.starts_with("# RamSleuth Probe Report"),
                "{markdown}"
            );
            assert!(markdown.contains("TUI Test CPU"), "{markdown}");
        }
        other => panic!("the consent overlay must open, got {other:?}"),
    }
    assert!(
        app.error.is_none(),
        "a successful fetch must not record an error"
    );
    stand_in.join();
}

/// (pt) The fetch against a missing socket records the friendly
/// error (the `DaemonDown` hint) and never opens the overlay
/// (D5).
#[test]
fn probe_report_fetch_missing_socket_never_opens_overlay() {
    let sock = TempSocket::new("probe-missing");
    let state = Arc::new(RwLock::new(AppState::default()));
    probe_report_fetch(sock.path(), &state);
    let app = state_read(&state);
    assert_eq!(
        app.probe,
        ProbeOverlayState::None,
        "no overlay on a failed fetch"
    );
    let error = app
        .error
        .clone()
        .expect("a friendly error must be recorded");
    assert!(error.contains("daemon not running"), "{error}");
}

/// (pu) A contract-violating reply (a `Telemetry` frame to
/// `GetProbeReport`) records the structured error, never the
/// overlay (D5).
#[test]
fn probe_report_fetch_contract_violation_records_error() {
    let sock = TempSocket::new("probe-violation");
    let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
        match read_one_message(&mut stream) {
            Some(Message::Request(Request::GetProbeReport)) => {}
            other => panic!("stand-in expected GetProbeReport, got {other:?}"),
        }
        let bytes = encode_frame(&Message::Response(Response::Telemetry(mock_snapshot())))
            .expect("must encode");
        stream
            .write_all(&bytes)
            .expect("stand-in write must not fail");
    });
    let state = Arc::new(RwLock::new(AppState::default()));
    probe_report_fetch(sock.path(), &state);
    let app = state_read(&state);
    assert_eq!(
        app.probe,
        ProbeOverlayState::None,
        "no overlay on a contract violation"
    );
    assert_eq!(
        app.error.as_deref(),
        Some("unexpected response to GetProbeReport")
    );
    stand_in.join();
}

/// (pv) The consent keys: `[q]` and `[n]` close; every other key
/// (including `[f]`) leaves the consent open (the consent's
/// answer key is `[y]`).
#[test]
fn overlay_consent_keys() {
    let dir = TempDir::new("overlay-consent");
    let area = Rect::new(0, 0, 80, 24);
    let no_clip = |_: &str| Ok("copied".to_owned());

    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Consent {
            markdown: "# the report".to_owned(),
        },
        ..Default::default()
    }));
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('f')),
        &state,
        area,
        &dir.path,
        &no_clip,
    );
    assert!(
        matches!(state_read(&state).probe, ProbeOverlayState::Consent { .. }),
        "[f] must not close the consent"
    );
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('q')),
        &state,
        area,
        &dir.path,
        &no_clip,
    );
    assert_eq!(
        state_read(&state).probe,
        ProbeOverlayState::None,
        "[q] must close the consent"
    );
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('n')),
        &state,
        area,
        &dir.path,
        &no_clip,
    );
    assert_eq!(
        state_read(&state).probe,
        ProbeOverlayState::None,
        "[n] stays closed after the `[q]`"
    );
}

/// (pw) `[y]` on the consent moves the carried markdown into the
/// preview (scroll 0, no notice) — no second fetch.
#[test]
fn overlay_consent_yes_opens_preview() {
    let dir = TempDir::new("overlay-yes");
    let area = Rect::new(0, 0, 80, 24);
    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Consent {
            markdown: "# the report".to_owned(),
        },
        ..Default::default()
    }));
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('y')),
        &state,
        area,
        &dir.path,
        &|_: &str| Ok("copied".to_owned()),
    );
    let app = state_read(&state);
    match &app.probe {
        ProbeOverlayState::Preview {
            markdown,
            notice,
            scroll,
        } => {
            assert_eq!(
                markdown, "# the report",
                "the carried markdown must move over"
            );
            assert!(notice.is_none(), "the preview opens with no notice");
            assert_eq!(*scroll, 0, "the preview opens at the top");
        }
        other => panic!("the preview must open, got {other:?}"),
    }
}

/// (px) The preview `[w]`: writes the markdown to `write_path`
/// (the test's temp dir — the live default path is never
/// touched) + records the green success notice; `[q]` closes
/// the preview.
#[test]
fn overlay_preview_write_and_quit() {
    let dir = TempDir::new("overlay-write");
    let path = dir.path.join(".ramsleuth").join("probe-report.md");
    let area = Rect::new(0, 0, 80, 24);
    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Preview {
            markdown: "the markdown".to_owned(),
            notice: None,
            scroll: 0,
        },
        ..Default::default()
    }));
    let no_clip = |_: &str| Ok("copied".to_owned());
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('w')),
        &state,
        area,
        &path,
        &no_clip,
    );
    let back = std::fs::read_to_string(&path).expect("the report must be written");
    assert_eq!(back, "the markdown", "the markdown must land verbatim");
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('q')),
        &state,
        area,
        &path,
        &no_clip,
    );
    assert_eq!(
        state_read(&state).probe,
        ProbeOverlayState::None,
        "[q] must close the preview"
    );
}
/// (py) The preview `[c]`: a clipboard success records the
/// success notice; a failure (the no-utility class) records the
/// fixed "use [w]" warning — the markdown and the overlay
/// survive (D5).
#[test]
fn overlay_preview_copy_outcomes() {
    let dir = TempDir::new("overlay-copy");
    let area = Rect::new(0, 0, 80, 24);

    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Preview {
            markdown: "md".to_owned(),
            notice: None,
            scroll: 0,
        },
        ..Default::default()
    }));
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('c')),
        &state,
        area,
        &dir.path,
        &|md: &str| {
            assert_eq!(md, "md", "the preview markdown is the clipboard payload");
            Ok("Copied to clipboard (wl-copy)".to_owned())
        },
    );
    let app = state_read(&state);
    match &app.probe {
        ProbeOverlayState::Preview {
            notice: Some((tone, text)),
            ..
        } => {
            assert_eq!(*tone, NoticeTone::Ok);
            assert_eq!(text, "Copied to clipboard (wl-copy)");
        }
        other => panic!("the preview must stay open after [c], got {other:?}"),
    }

    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Preview {
            markdown: "md".to_owned(),
            notice: None,
            scroll: 0,
        },
        ..Default::default()
    }));
    handle_probe_overlay_key(
        overlay_key(KeyCode::Char('c')),
        &state,
        area,
        &dir.path,
        &|_: &str| Err("no clipboard utility found".to_owned()),
    );
    let app = state_read(&state);
    match &app.probe {
        ProbeOverlayState::Preview {
            notice: Some((tone, text)),
            ..
        } => {
            assert_eq!(
                *tone,
                NoticeTone::Warn,
                "no utility is a warning, not a failure"
            );
            assert_eq!(
                text,
                "No clipboard utility found — use [w] to write to file"
            );
        }
        other => panic!("the preview must stay open after [c], got {other:?}"),
    }
}

/// (pz) The preview scroll: clamped to `lines − visible` over
/// the passed `area` (the render's layout) — the end is never
/// overshot, the top never undershot.
#[test]
fn overlay_preview_scroll_clamps() {
    let dir = TempDir::new("overlay-scroll");
    // 14 rows: the borders (2) + the footer (2) leave 10 visible.
    let area = Rect::new(0, 0, 80, 14);
    let markdown: String = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let state = Arc::new(RwLock::new(AppState {
        probe: ProbeOverlayState::Preview {
            markdown,
            notice: None,
            scroll: 0,
        },
        ..Default::default()
    }));
    let no_clip = |_: &str| Ok("copied".to_owned());

    for _ in 0..15 {
        handle_probe_overlay_key(
            overlay_key(KeyCode::Down),
            &state,
            area,
            &dir.path,
            &no_clip,
        );
    }
    assert_eq!(
        scroll_of(&state),
        10,
        "the scroll must clamp at max (20 − 10)"
    );
    handle_probe_overlay_key(
        overlay_key(KeyCode::Down),
        &state,
        area,
        &dir.path,
        &no_clip,
    );
    assert_eq!(scroll_of(&state), 10, "the end must never be overshot");
    for _ in 0..15 {
        handle_probe_overlay_key(overlay_key(KeyCode::Up), &state, area, &dir.path, &no_clip);
    }
    assert_eq!(scroll_of(&state), 0, "the top must never be undershot");
    handle_probe_overlay_key(overlay_key(KeyCode::Up), &state, area, &dir.path, &no_clip);
    assert_eq!(scroll_of(&state), 0, "the top must never be undershot");
}

/// (qa) The ghosting guard's visibility predicate: the controls
/// block (the 17-key map) its `requirements_open` flag alone, the
/// requirements strip presence-driven — its flag AND a non-empty
/// `diagnose` (the daemon-connect auto-vanish: a connected clean
/// state hides the strip even with the flag open, while the
/// controls block stays shown on the flag) — and the settings
/// strip its flag alone.
#[test]
fn strip_visibility_is_presence_driven() {
    // The daemon-less default: the daemon-down requirement is
    // present. Closed flag → the whole `[d]` screen hidden; open
    // flag → shown (the controls block on the flag, the strip on the
    // presence rule).
    let mut state = AppState::default();
    assert_eq!(strip_visibility(&state), (false, false, false));
    state.settings.requirements_open = true;
    assert_eq!(strip_visibility(&state), (true, true, false));

    // The settings strip: its flag alone.
    state.settings.settings_open = true;
    assert_eq!(strip_visibility(&state), (true, true, true));

    // The daemon-connect transition (the ghosting case): connected,
    // no telemetry, no error → `diagnose` empties → the
    // requirements strip auto-vanishes, while the controls block
    // stays shown on its flag and the settings strip on its.
    state.daemon_status = "connected: /run/ramsleuth/ramsleuth.sock".to_owned();
    assert_eq!(strip_visibility(&state), (true, false, true));
}

/// The current preview scroll offset (the test assertion helper;
/// panics if the overlay is not in the preview phase).
fn scroll_of(state: &Arc<RwLock<AppState>>) -> u16 {
    let app = state_read(state);
    match &app.probe {
        ProbeOverlayState::Preview { scroll, .. } => *scroll,
        other => panic!("the preview must stay open, got {other:?}"),
    }
}
