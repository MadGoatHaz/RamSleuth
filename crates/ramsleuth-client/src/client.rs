//! The synchronous Unix-socket RPC client (P3-18, plan D2/D5).
//!
//! [`Client`] is the transport every unprivileged frontend (the CLI bin
//! in P3-21, the TUI in P3-22+, the GUI in P3-25+) shares: it connects
//! to the daemon's Unix socket — a local `connect` cannot hang (it
//! completes or fails at once), so a starting daemon is given a short
//! retry + backoff window instead — speaks the frozen P3-10 / P3-11
//! frame protocol over a plain `std::os::unix::net::UnixStream` (no
//! tokio — the daemon is the workspace's only tokio consumer, D2), and
//! degrades to **friendly, actionable diagnostics** instead of panics
//! on every failure class (the client's no-panic contract, D5):
//!
//! - daemon absent / refused → [`ClientError::DaemonDown`] with a
//!   "start it with …" hint (retried first — see [`RETRIES`]);
//! - silent / wedged daemon → [`ClientError::Timeout`] (5 s default
//!   read / write timeouts, tunable via [`Client::set_read_timeout`] /
//!   [`Client::set_write_timeout`]);
//! - malformed stream → [`ClientError::Protocol`] (oversized frame,
//!   bincode rejection, or a request frame where a response is due).
//!
//! **Incremental-reader contract (P3-11):** `recv` keeps a growable
//! buffer of leftover bytes — each `read` chunk is appended,
//! [`decode_frame`] is retried while [`FrameError::Incomplete`], and a
//! complete frame is drained by its exact `consumed` count so a partial
//! next frame (or several at once) survives across `recv` calls. That
//! is what makes streamed benchmark progress (P3-20) work over the same
//! connection.
//!
//! **No panics:** every failure path is a structured [`ClientError`];
//! a closed connection mid-frame is [`ClientError::Io`] carrying
//! `ErrorKind::UnexpectedEof`, never a panic.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message, Request, Response};

/// Number of `connect` attempts before giving up (plan P3-18: a daemon
/// may still be binding its socket, so transient connect failures are
/// retried with short backoff; the final failure still maps onto the
/// friendly [`ClientError::DaemonDown`]).
const RETRIES: u32 = 3;

/// Default read / write timeouts applied on connect (plan P3-18): a
/// wedged daemon becomes [`ClientError::Timeout`], never a hang.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// One `recv` read chunk (16 KiB: a real `Message` frame is a few KiB
/// at most — the snapshot arm — and the P3-11 incremental contract
/// re-decodes after every chunk).
const READ_CHUNK: usize = 16 * 1024;

/// Backoff before the next connect attempt: 100 ms · (attempt + 1)
/// (plan P3-18) — 100 ms after the first failure, 200 ms after the
/// second.
fn connect_backoff(attempt: u32) -> Duration {
    Duration::from_millis(100 * (attempt + 1) as u64)
}

/// `std::io::Error` is not `Clone`; preserve the observable identity
/// (`kind` + message text) instead (the daemon P3-13/P3-16 precedent).
fn clone_io_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

/// `std::io::Error` is not `PartialEq`; compare the observable identity
/// (`kind` + message text) instead (the daemon P3-13/P3-16 precedent).
fn same_io_error(a: &io::Error, b: &io::Error) -> bool {
    a.kind() == b.kind() && a.to_string() == b.to_string()
}

/// A mapped read failure: an expired read timeout is
/// [`ClientError::Timeout`], anything else is a plain
/// [`ClientError::Io`] (including the `UnexpectedEof` constructed for
/// a closed connection mid-frame).
///
/// Note the std quirk: `TcpStream` reports an expired read timeout as
/// `TimedOut`, but `UnixStream` runs non-blocking while a timeout is
/// set and surfaces the expired deadline as `WouldBlock` instead — so
/// both kinds map onto [`ClientError::Timeout`] (with a read timeout
/// set — always the case here, defaults land on connect — a pending
/// read only unblocks on data, EOF, or the deadline).
fn map_read_error(error: io::Error) -> ClientError {
    if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
        ClientError::Timeout
    } else {
        ClientError::Io(error)
    }
}

/// Errors the client can hit, mapped from every failure class the
/// transport sees (plan D5: each carries an actionable diagnostic).
///
/// `std::io::Error` is neither `Clone` nor `PartialEq`, so those two
/// traits are implemented by hand below (identity = the variant, the
/// `kind`, and the message text) — the daemon `RpcError` /
/// `SocketSetupError` precedent.
#[derive(Debug)]
pub enum ClientError {
    /// The connect failed with a non-transient i/o error (not a
    /// missing / refused socket — those retry and become
    /// [`DaemonDown`](Self::DaemonDown)).
    Connect(io::Error),
    /// A read or write timed out (the daemon is alive but silent /
    /// wedged — the 5 s default timeouts, or a caller override).
    Timeout,
    /// The byte stream violated the wire protocol: a frame over the
    /// 16 MiB guard, a bincode `Decode` rejection, or a request frame
    /// where a response was due.
    Protocol(String),
    /// A socket i/o failure mid-session (the peer vanished, a write to
    /// a closed end, …). A connection closed before a complete frame
    /// arrives is the `UnexpectedEof` kind.
    Io(io::Error),
    /// The daemon socket is absent / refused after every retry: the
    /// daemon is not running. `msg` is the friendly, actionable text
    /// (the socket path + how to start the daemon).
    DaemonDown(String),
}

impl Clone for ClientError {
    fn clone(&self) -> Self {
        match self {
            Self::Connect(e) => Self::Connect(clone_io_error(e)),
            Self::Timeout => Self::Timeout,
            Self::Protocol(msg) => Self::Protocol(msg.clone()),
            Self::Io(e) => Self::Io(clone_io_error(e)),
            Self::DaemonDown(msg) => Self::DaemonDown(msg.clone()),
        }
    }
}

impl PartialEq for ClientError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Connect(a), Self::Connect(b)) | (Self::Io(a), Self::Io(b)) => {
                same_io_error(a, b)
            }
            (Self::Protocol(a), Self::Protocol(b)) | (Self::DaemonDown(a), Self::DaemonDown(b)) => {
                a == b
            }
            (Self::Timeout, Self::Timeout) => true,
            _ => false,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "failed to connect to the daemon: {e}"),
            Self::Timeout => write!(f, "timed out waiting for the daemon"),
            Self::Protocol(msg) => write!(f, "daemon protocol error: {msg}"),
            Self::Io(e) => write!(f, "i/o error on the daemon socket: {e}"),
            Self::DaemonDown(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) | Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// A synchronous client for one daemon connection (plan D2: no tokio —
/// the daemon is the workspace's only tokio consumer).
///
/// Built by [`Client::connect`] (never constructed directly — the
/// socket path and the leftover receive buffer are transport state);
/// one `Client` serves one connection's worth of round trips,
/// including streamed benchmark progress over the same socket (P3-20).
#[derive(Debug)]
pub struct Client {
    /// The connected unix stream (synchronous I/O; read / write
    /// timeouts are set on connect and tunable afterwards).
    stream: UnixStream,
    /// The socket this connection was made to (diagnostics + the
    /// [`Client::socket_path`] accessor).
    socket_path: PathBuf,
    /// Leftover bytes not yet consumed by a decoded frame (the P3-11
    /// incremental-reader contract): each `read` chunk is appended, a
    /// complete frame is drained by its exact `consumed` count, and
    /// whatever remains is re-decoded on the next `recv`.
    buf: Vec<u8>,
}

impl Client {
    /// Connect to the daemon at `socket_path`.
    ///
    /// A local unix `connect` either completes or fails at once (a
    /// missing path is `NotFound`, a path with no listening daemon is
    /// `ConnectionRefused` — std's `UnixStream` has no
    /// `connect_timeout` because there is no handshake to stall on), so
    /// a daemon that is still starting up is given a short window
    /// instead: transient failures are retried ([`RETRIES`] attempts,
    /// 100 ms · (n + 1) backoff) before the final one becomes the
    /// friendly [`ClientError::DaemonDown`]. On success the 5 s default
    /// read / write timeouts are set (a wedged daemon becomes
    /// [`ClientError::Timeout`], never a hang).
    ///
    /// A missing or refused socket after every retry is
    /// [`ClientError::DaemonDown`] with the "start it with …"
    /// diagnostic; any other connect failure is
    /// [`ClientError::Connect`]. Never panics (D5).
    pub fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        let mut last: Option<io::Error> = None;
        for attempt in 0..RETRIES {
            match UnixStream::connect(socket_path) {
                Ok(stream) => {
                    let client = Self {
                        stream,
                        socket_path: socket_path.to_path_buf(),
                        buf: Vec::new(),
                    };
                    // A wedged daemon must become a `Timeout`, never a
                    // hang (plan P3-18 default 5 s read / write).
                    client.set_read_timeout(DEFAULT_READ_TIMEOUT)?;
                    client.set_write_timeout(DEFAULT_WRITE_TIMEOUT)?;
                    return Ok(client);
                }
                Err(e) => {
                    // Only a missing / refused socket is retryable (the
                    // daemon may still be binding); anything else is a
                    // real connect failure and stops here.
                    if !matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) {
                        return Err(ClientError::Connect(e));
                    }
                    last = Some(e);
                    if attempt + 1 < RETRIES {
                        thread::sleep(connect_backoff(attempt));
                    }
                }
            }
        }
        let e = last.expect("the loop records every transient failure");
        let path = socket_path.display();
        Err(ClientError::DaemonDown(format!(
            "cannot connect to {path}: daemon not running? start it with \
             `cargo run -p ramsleuth-daemon` or `systemctl start ramsleuth` \
             (last error: {e})"
        )))
    }

    /// The socket path this connection was made to (diagnostics).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Override the read timeout (the default is 5 s, set on connect).
    pub fn set_read_timeout(&self, d: Duration) -> Result<(), ClientError> {
        self.stream.set_read_timeout(Some(d)).map_err(ClientError::Io)
    }

    /// Override the write timeout (the default is 5 s, set on connect).
    pub fn set_write_timeout(&self, d: Duration) -> Result<(), ClientError> {
        self.stream.set_write_timeout(Some(d)).map_err(ClientError::Io)
    }

    /// Send one request frame (encode + `write_all`; the frame codec is
    /// the P3-11 contract, `FrameError` → [`ClientError::Protocol`],
    /// i/o failure → [`ClientError::Io`]).
    pub fn send(&mut self, req: &Request) -> Result<(), ClientError> {
        let bytes =
            encode_frame(&Message::Request(*req)).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.stream.write_all(&bytes).map_err(ClientError::Io)?;
        Ok(())
    }

    /// Receive the next response frame.
    ///
    /// Reads into the persistent leftover buffer until a complete frame
    /// decodes (the P3-11 incremental contract): a closed connection
    /// mid-frame is [`ClientError::Io`] with `UnexpectedEof`, a read
    /// timeout is [`ClientError::Timeout`], an oversized / undecodable
    /// frame and a request frame where a response is due are
    /// [`ClientError::Protocol`]. Never panics (D5).
    pub fn recv(&mut self) -> Result<Response, ClientError> {
        loop {
            let message = match decode_frame(&self.buf) {
                Err(FrameError::Incomplete) => {
                    self.read_more()?;
                    continue;
                }
                Err(e) => return Err(ClientError::Protocol(e.to_string())),
                Ok(frame) => {
                    // Drain exactly `consumed` bytes; any remainder is
                    // the start of the next frame (or two frames
                    // arrived in one read).
                    self.buf.drain(..frame.consumed);
                    frame.message
                }
            };
            return match message {
                Message::Response(resp) => Ok(resp),
                Message::Request(_) => {
                    Err(ClientError::Protocol("unexpected request frame".to_owned()))
                }
            };
        }
    }

    /// One request / response round trip (single connection, used for
    /// `GetTelemetry` / `CancelBenchmark`; benchmark runs stream
    /// progress + one terminal over the same connection, P3-20).
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        self.send(req)?;
        self.recv()
    }

    /// Append one read chunk to the leftover buffer; a 0-byte read is
    /// the peer closing mid-frame → `Io(UnexpectedEof)`.
    fn read_more(&mut self) -> Result<(), ClientError> {
        let mut chunk = [0u8; READ_CHUNK];
        let n = self.stream.read(&mut chunk).map_err(map_read_error)?;
        if n == 0 {
            return Err(ClientError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the daemon closed the connection before a complete frame",
            )));
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests run against a real **in-process daemon stand-in**: a
    //! `std::thread` that binds a `UnixListener` on a unique temp socket
    //! path (pid-qualified so parallel runs never collide), accepts one
    //! connection, and speaks the frozen P3-10 / P3-11 frame protocol.

    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process;
    use std::thread;
    use std::time::{Duration, Instant};

    use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message, Request, Response};
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemMemoryTelemetry;

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
                "/tmp/ramsleuth-client-{name}-{}.sock",
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
    /// API (host-independent — mirrors the daemon P3-14 mock).
    fn mock_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Mock CPU".to_owned(),
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

    /// (a) `connect` succeeds against a **live** temp socket (the
    /// stand-in simply accepts and closes).
    #[test]
    fn connect_to_live_temp_socket_succeeds() {
        let sock = TempSocket::new("connect");
        let stand_in = DaemonStandIn::spawn(&sock, |_stream| {});
        let client = Client::connect(sock.path()).expect("must connect to the live stand-in");
        assert_eq!(client.socket_path(), sock.path());
        stand_in.join();
    }

    /// (b) `request(&GetTelemetry)` returns the stand-in's canned
    /// `Response::Telemetry` — one full frame round trip over the frozen
    /// protocol (the stand-in decodes the request frame and answers
    /// with an encoded `Telemetry` frame).
    #[test]
    fn request_get_telemetry_round_trip() {
        let sock = TempSocket::new("telemetry");
        let snapshot = mock_snapshot();
        let stand_in = DaemonStandIn::spawn(&sock, move |mut stream| {
            match read_one_message(&mut stream) {
                Some(Message::Request(Request::GetTelemetry)) => {}
                other => panic!("stand-in expected GetTelemetry, got {other:?}"),
            }
            let bytes =
                encode_frame(&Message::Response(Response::Telemetry(snapshot))).expect("must encode");
            stream.write_all(&bytes).expect("stand-in write must not fail");
        });
        let mut client = Client::connect(sock.path()).expect("must connect");
        let resp = client
            .request(&Request::GetTelemetry)
            .expect("request must round-trip");
        assert_eq!(resp, Response::Telemetry(mock_snapshot()));
        stand_in.join();
    }

    /// (c) `connect` to a **non-existent** socket path returns the
    /// friendly `DaemonDown` diagnostic after `RETRIES` attempts with
    /// backoff (never panics; the ≥300 ms floor proves the 100 + 200 ms
    /// backoffs ran — no hang, no instant give-up either).
    #[test]
    fn connect_to_missing_socket_is_friendly_daemon_down() {
        let sock = TempSocket::new("missing");
        let start = Instant::now();
        let err = Client::connect(sock.path()).expect_err("no listener: must fail");
        let elapsed = start.elapsed();
        let ClientError::DaemonDown(msg) = err else {
            panic!("expected DaemonDown, got {err:?}");
        };
        let path_str = sock.path().to_string_lossy().into_owned();
        assert!(msg.contains("cannot connect to"), "friendly message, got: {msg}");
        assert!(msg.contains(&path_str), "names the socket path, got: {msg}");
        assert!(msg.contains("daemon not running"), "actionable hint, got: {msg}");
        assert!(
            elapsed >= Duration::from_millis(300),
            "RETRIES={RETRIES} with 100 + 200 ms backoff must be observable, took {elapsed:?}"
        );
    }

    /// (d) `recv` on a stream the daemon already closed returns a
    /// structured `Io` error (`UnexpectedEof`) — no panic, no hang.
    #[test]
    fn recv_on_closed_stream_is_io_error() {
        let sock = TempSocket::new("closed");
        // The stand-in accepts and immediately closes (no frame).
        let stand_in = DaemonStandIn::spawn(&sock, |_stream| {});
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = client.recv().expect_err("closed stream: must fail");
        let ClientError::Io(e) = err else {
            panic!("expected Io, got {err:?}");
        };
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
        stand_in.join();
    }

    /// (e) A **silent** daemon (accepts, never answers) turns the read
    /// timeout into `ClientError::Timeout` — the no-hang guarantee
    /// (plan P3-18 quality gate).
    #[test]
    fn recv_times_out_on_silent_daemon() {
        let sock = TempSocket::new("silent");
        let stand_in = DaemonStandIn::spawn(&sock, |_stream| {
            thread::sleep(Duration::from_millis(500));
        });
        let mut client = Client::connect(sock.path()).expect("must connect");
        client.set_read_timeout(Duration::from_millis(200)).expect("timeout setter");
        let err = client
            .request(&Request::GetTelemetry)
            .expect_err("silent daemon: must time out");
        assert_eq!(err, ClientError::Timeout);
        stand_in.join();
    }

    /// (f) A stand-in that answers a request with a *request* frame is
    /// a protocol violation: `recv` maps it to `Protocol` (the
    /// daemon-side mirror closes the connection the same way, P3-16).
    #[test]
    fn recv_on_request_frame_is_protocol_error() {
        let sock = TempSocket::new("echo");
        let stand_in = DaemonStandIn::spawn(&sock, |mut stream| {
            if let Some(Message::Request(req)) = read_one_message(&mut stream) {
                // Echo the request frame back (a protocol violation).
                let bytes = encode_frame(&Message::Request(req)).expect("must encode");
                stream.write_all(&bytes).expect("stand-in write must not fail");
            }
        });
        let mut client = Client::connect(sock.path()).expect("must connect");
        let err = client.request(&Request::GetTelemetry).expect_err("must fail");
        let ClientError::Protocol(msg) = err else {
            panic!("expected Protocol, got {err:?}");
        };
        assert_eq!(msg, "unexpected request frame");
        stand_in.join();
    }
}
