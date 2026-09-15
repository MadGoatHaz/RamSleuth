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
//! - **Background updater** — a `std::thread` that every 2 s runs
//!   [`poll_once`] (a fresh `Client::connect` + `GetTelemetry` over
//!   `ramsleuth-client`, P3-18) into the shared `Arc<RwLock<AppState>>`;
//!   it stops on the quit flag and is joined (bounded) before exit.
//! - **Main loop** — each tick: `terminal.draw(render)` (the P3-23
//!   three-zone dashboard over the shared state) +
//!   `events::poll_event(250 ms)` → the frozen `key_to_action` table
//!   (P3-22): `[R]`efresh forces an immediate `poll_once` on the main
//!   thread (one fast RPC), `[S]`napshot writes the dashboard text (the
//!   client's pure `render`, P3-19) to a timestamped file in the CWD and
//!   records the path in `daemon_status` (transient — the next poll
//!   overwrites it), `[Q]`uit breaks the loop.
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
use ramsleuth_client::Client;
use ramsleuth_protocol::{Request, Response, DEFAULT_SOCKET_PATH};
use ramsleuth_tui::events;
use ramsleuth_tui::{key_to_action, render, Action, AppState};

/// How often the background updater polls the daemon (plan P3-24: 2 s).
const UPDATE_INTERVAL: Duration = Duration::from_millis(2000);

/// The main loop's event-poll timeout (plan P3-24: a 250 ms tick — the
/// redraw cadence, so the `…s ago` stamp and progress line advance live).
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// How long to wait for the updater after quit: it finishes within one
/// update cycle (a 2 s `sleep` + at most one connect-retry window), so
/// this deadline is generous; if it is exceeded the thread is detached
/// (the process is exiting — returning from `main` terminates it, and
/// the terminal is already restored by then).
const JOIN_DEADLINE: Duration = Duration::from_secs(5);

/// The sleep granularity while waiting for the updater to finish.
const JOIN_POLL: Duration = Duration::from_millis(50);

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
    match client.request(&Request::GetTelemetry) {
        Ok(Response::Telemetry(telemetry)) => {
            state.telemetry = Some(telemetry);
            state.last_update = Some(Instant::now());
            state.daemon_status = format!("connected: {}", socket.display());
            state.error = None;
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

/// Spawn the background updater: every [`UPDATE_INTERVAL`] it runs
/// [`poll_once`] against `socket`, storing the result into `state` (a
/// fresh connect each cycle — survives daemon restarts). The thread runs
/// until `stop` is set (quit); one cycle is bounded (the connect-retry
/// window ~300 ms + the transport's 5 s read timeout), so a wedged
/// daemon cannot hold it forever.
fn spawn_updater(
    state: Arc<RwLock<AppState>>,
    stop: Arc<AtomicBool>,
    socket: PathBuf,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let _ = poll_once(&socket, &mut state.write().unwrap());
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(UPDATE_INTERVAL);
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

    let updater = spawn_updater(Arc::clone(&state), Arc::clone(&stop), args.socket.clone());

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

    use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message};
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
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
}
