//! Per-connection async RPC loop (P3-16, plan D6): the daemon's async
//! surface for one accepted client connection — read P3-11 frames
//! incrementally (u32-LE length prefix + Bincode), dispatch each
//! [`Message::Request`], and write the matching
//! [`Message::Response`] frame(s) back:
//!
//! - [`Request::GetTelemetry`] → the P3-14 TTL cache: `get` takes
//!   `&mut self` and may call the **slow** collector, so it runs on the
//!   blocking pool behind the shared mutex → [`Response::Telemetry`];
//! - [`Request::StartBenchmark { target, mode }`] → the P3-15
//!   single-flight [`BenchJobManager`] (`threads = 0`: auto, the P3-09
//!   contract) → reply [`Response::BenchStarted { run_id }] and become
//!   the run's **owner**: every [`JobEvent`] is forwarded in order
//!   (`Progress` → [`Response::BenchProgress`]; exactly one terminal →
//!   [`Response::BenchResult`] / [`Response::BenchCancelled`] /
//!   [`Response::Error`]), each blocking std-`mpsc` `recv` offloaded
//!   via `spawn_blocking` so no runtime worker is ever parked;
//! - [`Request::StartBurnIn { target, duration_minutes }`] → the same
//!   single-flight slot via `start_burn_in` (`threads = 0`: auto — a
//!   burn-in and a benchmark are mutually exclusive, D-1) → reply
//!   [`Response::BenchStarted { run_id }] and become the run's
//!   **owner**: every [`JobEvent`] is forwarded in order
//!   (`BurnInTick` → [`Response::BurnInProgress`], D-2; exactly one
//!   terminal → [`Response::BenchResult`] /
//!   [`Response::BenchCancelled`] / [`Response::Error`]), the same
//!   blocking-`recv`-offloaded forwarding as the benchmark arm;
//! - [`Request::CancelBenchmark { run_id }`] →
//!   [`BenchJobManager::cancel`] (any connection may cancel, D6) → ack
//!   [`Response::BenchCancelled { run_id }] when the run was active,
//!   [`Response::Error`] when no such run is active (the owner still
//!   receives the terminal `Cancelled`/`Result` exactly once);
//! - a client-sent [`Message::Response`] is a protocol violation →
//!   close the connection.
//!
//! **Connection lifecycle:** [`handle_connection`] returns `Ok(())` on
//! clean EOF (the client closed) and [`RpcError`] on I/O failure or a
//! protocol violation (`Oversized` / `Decode` frame — the connection
//! is closed). A dying owner tears down nothing shared: single-flight
//! state lives in the [`BenchJobManager`] (D6), and an unfinished run
//! simply runs to its terminal and releases the slot.
//!
//! **No panics (plan D5):** poisoned locks are recovered (the
//! P3-15 `BenchJobManager` precedent), a run channel that closes
//! without a terminal ends the owner's forwarding, and every failure
//! is a structured [`RpcError`].

use std::fmt;
use std::io;
use std::marker::Unpin;
use std::sync::{Arc, Mutex, PoisonError};

use ramsleuth_protocol::{decode_frame, encode_frame, FrameError, Message, Request, Response};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{BenchJobManager, JobEvent, TelemetryCache};

/// The daemon's shared per-connection state, handed (cloned `Arc`) to
/// every [`handle_connection`] by the P3-17 accept loop:
///
/// - `cache`: the P3-14 TTL telemetry cache behind a `Mutex` —
///   `get` is `&mut self` and may call the slow collector, so access
///   is serialized across connections and each RPC takes the lock
///   only inside `spawn_blocking`;
/// - `jobs`: the P3-15 single-flight [`BenchJobManager`] (inherently
///   share-safe: all of its state is `Arc`-backed; a dying owner
///   strands nothing — the run releases its own slot, plan D6).
pub struct DaemonContext {
    pub cache: Arc<Mutex<TelemetryCache>>,
    pub jobs: Arc<BenchJobManager>,
}

/// Failure of one connection's RPC loop.
///
/// `std::io::Error` is neither `Clone` nor `PartialEq`, so those two
/// traits are implemented by hand below (identity = variant + `kind`
/// + message text) — the P3-13 `SocketSetupError` precedent.
#[derive(Debug)]
pub enum RpcError {
    /// A socket read / write / flush failed (the peer vanished
    /// mid-frame, a write to a closed end, ...).
    Io(io::Error),
    /// The byte stream violated the wire protocol: a frame over the
    /// 16 MiB `MAX_FRAME_SIZE` guard, a Bincode `Decode` rejection, or
    /// a client-sent `Response` frame — the connection is closed.
    Protocol(String),
    /// The benchmark job failed to start (the P3-15 `JobError` text —
    /// e.g. single-flight `Busy`, which the P3-15 contract already
    /// displays as "benchmark already running").
    Job(String),
    /// A `spawn_blocking` pump failed to join (a worker panicked —
    /// caught and structured, never propagated as a panic; plan D5).
    Join(String),
}

/// `std::io::Error` is not `Clone`; preserve the observable identity
/// (`kind` + message text) instead.
fn clone_io_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

impl Clone for RpcError {
    fn clone(&self) -> Self {
        match self {
            Self::Io(e) => Self::Io(clone_io_error(e)),
            Self::Protocol(msg) => Self::Protocol(msg.clone()),
            Self::Job(msg) => Self::Job(msg.clone()),
            Self::Join(msg) => Self::Join(msg.clone()),
        }
    }
}

/// `std::io::Error` is not `PartialEq`; compare the observable identity
/// (`kind` + message text) instead.
fn same_io_error(a: &io::Error, b: &io::Error) -> bool {
    a.kind() == b.kind() && a.to_string() == b.to_string()
}

impl PartialEq for RpcError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => same_io_error(a, b),
            (Self::Protocol(a), Self::Protocol(b))
            | (Self::Job(a), Self::Job(b))
            | (Self::Join(a), Self::Join(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "rpc i/o error: {e}"),
            Self::Protocol(msg) => write!(f, "rpc protocol violation: {msg}"),
            Self::Job(msg) => write!(f, "benchmark job error: {msg}"),
            Self::Join(msg) => write!(f, "blocking pump join failed: {msg}"),
        }
    }
}

impl std::error::Error for RpcError {}

/// Serve one accepted connection to completion (clean EOF or error).
///
/// Splits the stream into its read / write halves, keeps a growable
/// frame buffer, and loops: read a chunk (0 = EOF → `Ok(())`), append
/// it, [`decode_frame`] — a complete frame is drained by its exact
/// `consumed` count (any remainder stays for the next decode, the
/// P3-11 incremental-reader contract) and dispatched; `Incomplete`
/// reads more; `Oversized` / `Decode` close the connection with
/// [`RpcError::Protocol`].
pub async fn handle_connection(
    stream: tokio::net::UnixStream,
    ctx: Arc<DaemonContext>,
) -> Result<(), RpcError> {
    let (mut reader, mut writer) = stream.into_split();
    let mut buf: Vec<u8> = Vec::new();
    // A 16 KiB stack buffer per read: real `Message` frames are at
    // most a few hundred bytes (the 16 MiB guard only bounds hostile
    // prefixes), so the hot loop stays allocation-free.
    let mut chunk = [0u8; 16 * 1024];

    loop {
        let n = reader.read(&mut chunk).await.map_err(RpcError::Io)?;
        if n == 0 {
            // The client closed: a clean end. Nothing shared is
            // touched — an unfinished run keeps running to its
            // terminal and releases its own slot (plan D6).
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);

        match decode_frame(&buf) {
            Ok(frame) => {
                buf.drain(..frame.consumed);
                dispatch(&frame.message, &mut writer, &ctx).await?;
            }
            Err(FrameError::Incomplete) => {
                // A partial frame is buffered: read more.
            }
            Err(FrameError::Oversized) => {
                return Err(RpcError::Protocol(
                    "frame payload length exceeds the 16 MiB maximum".to_owned(),
                ));
            }
            Err(FrameError::Decode(msg)) => {
                return Err(RpcError::Protocol(format!("frame failed to decode: {msg}")));
            }
        }
    }
}

/// Dispatch one decoded request frame, writing its response frame(s).
///
/// The writer is `&mut impl AsyncWrite` (the connection's split write
/// half in production; the same code serves any async sink in tests).
/// Every blocking operation — the cache `get` (it may call the
/// collector) and the run's std-`mpsc` `recv` (blocking by design) —
/// is offloaded to the blocking pool via `spawn_blocking`, so no
/// async runtime worker is ever parked on a blocking call (plan D6).
async fn dispatch(
    message: &Message,
    writer: &mut (impl AsyncWrite + Unpin),
    ctx: &DaemonContext,
) -> Result<(), RpcError> {
    match message {
        Message::Request(Request::GetTelemetry) => {
            // `get` is `&mut self` and may call the (slow) collector:
            // take the shared mutex inside the pool, never on a
            // runtime worker. A poisoned lock is recovered (the
            // collector is no-panic by the Phase 2 contract, so this
            // is defensive only — plan D5).
            let cache = Arc::clone(&ctx.cache);
            let telemetry = tokio::task::spawn_blocking(move || {
                let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
                c.get()
            })
            .await
            .map_err(|e| RpcError::Join(e.to_string()))?;
            write_frame(writer, &Message::Response(Response::Telemetry(telemetry))).await
        }
        Message::Request(Request::StartBenchmark { target, mode }) => {
            // `threads = 0`: auto — one pinned worker per detected
            // physical core (the P3-09 contract). `JobError::Busy`
            // (single-flight, plan D6) arrives with its wire-ready
            // "benchmark already running" text.
            let handle = ctx
                .jobs
                .start(*target, *mode, 0)
                .await
                .map_err(|e| RpcError::Job(e.to_string()))?;
            let run_id = handle.run_id;
            write_frame(writer, &Message::Response(Response::BenchStarted { run_id })).await?;

            // Owner forwarding: the run's std `mpsc` receiver blocks
            // in `recv`, so each pull is one `spawn_blocking`. The
            // `Mutex` keeps the receiver alive across pulls (one pull
            // at a time — the lock is never contended). P3-15
            // guarantees every progress in order plus exactly one
            // terminal, after which the channel closes.
            let events = Arc::new(Mutex::new(handle.events));
            loop {
                let events = Arc::clone(&events);
                let event = tokio::task::spawn_blocking(move || {
                    let guard = events.lock().unwrap_or_else(PoisonError::into_inner);
                    guard.recv()
                })
                .await
                .map_err(|e| RpcError::Join(e.to_string()))?;
                match event {
                    Ok(JobEvent::Progress(progress)) => {
                        write_frame(writer, &Message::Response(Response::BenchProgress(progress)))
                            .await?;
                    }
                    // Defensive: a benchmark run never emits a
                    // burn-in tick (that is the `StartBurnIn` arm's
                    // stream) — end the stream cleanly.
                    Ok(JobEvent::BurnInTick(_)) => break,
                    Ok(JobEvent::Result(grid)) => {
                        write_frame(
                            writer,
                            &Message::Response(Response::BenchResult { run_id, grid }),
                        )
                        .await?;
                        break;
                    }
                    Ok(JobEvent::Cancelled) => {
                        write_frame(writer, &Message::Response(Response::BenchCancelled { run_id }))
                            .await?;
                        break;
                    }
                    Ok(JobEvent::Error(msg)) => {
                        write_frame(writer, &Message::Response(Response::Error(msg))).await?;
                        break;
                    }
                    // Defensive: the run dropped every sender without
                    // a terminal (P3-15 sends exactly one before
                    // closing) — end the stream cleanly.
                    Err(_) => break,
                }
            }
            Ok(())
        }
        Message::Request(Request::StartBurnIn { target, duration_minutes }) => {
            // `threads = 0`: auto — one pinned worker per detected
            // physical core (the C7-06 contract). The burn-in shares
            // the single-flight slot with `StartBenchmark` (D-1):
            // `JobError::Busy` arrives with its wire-ready "benchmark
            // already running" text.
            let handle = ctx
                .jobs
                .start_burn_in(*target, *duration_minutes, 0)
                .await
                .map_err(|e| RpcError::Job(e.to_string()))?;
            let run_id = handle.run_id;
            write_frame(writer, &Message::Response(Response::BenchStarted { run_id })).await?;

            // Owner forwarding: the same pattern as the `StartBenchmark`
            // arm (the run's std `mpsc` receiver blocks in `recv`, so
            // each pull is one `spawn_blocking`; the `Mutex` keeps the
            // receiver alive across pulls). The job guarantees every
            // tick in order plus exactly one terminal, after which the
            // channel closes.
            let events = Arc::new(Mutex::new(handle.events));
            loop {
                let events = Arc::clone(&events);
                let event = tokio::task::spawn_blocking(move || {
                    let guard = events.lock().unwrap_or_else(PoisonError::into_inner);
                    guard.recv()
                })
                .await
                .map_err(|e| RpcError::Join(e.to_string()))?;
                match event {
                    Ok(JobEvent::BurnInTick(tick)) => {
                        write_frame(writer, &Message::Response(Response::BurnInProgress(tick)))
                            .await?;
                    }
                    Ok(JobEvent::Result(grid)) => {
                        write_frame(
                            writer,
                            &Message::Response(Response::BenchResult { run_id, grid }),
                        )
                        .await?;
                        break;
                    }
                    Ok(JobEvent::Cancelled) => {
                        write_frame(writer, &Message::Response(Response::BenchCancelled { run_id }))
                            .await?;
                        break;
                    }
                    Ok(JobEvent::Error(msg)) => {
                        write_frame(writer, &Message::Response(Response::Error(msg))).await?;
                        break;
                    }
                    // Defensive: a burn-in run never emits a
                    // `Progress` event (that is the benchmark arm's
                    // stream) — end the stream cleanly.
                    Ok(JobEvent::Progress(_)) => break,
                    // Defensive: the run dropped every sender without
                    // a terminal (P3-15 sends exactly one before
                    // closing) — end the stream cleanly.
                    Err(_) => break,
                }
            }
            Ok(())
        }
        Message::Request(Request::CancelBenchmark { run_id }) => {
            // Any connection may cancel (plan D6): `true` when the
            // run was still active (its owner receives the terminal
            // `Cancelled`); `false` for an unknown id or a run that
            // already terminated and released the slot.
            if ctx.jobs.cancel(*run_id) {
                write_frame(
                    writer,
                    &Message::Response(Response::BenchCancelled { run_id: *run_id }),
                )
                .await
            } else {
                write_frame(
                    writer,
                    &Message::Response(Response::Error(format!(
                        "no active run with id {run_id}"
                    ))),
                )
                .await
            }
        }
        Message::Request(Request::GetProbeReport) => {
            // The consent-gated "Submit Probe Report" (chunk probe-1a
            // wire types). The daemon-side builder that assembles the
            // `ProbeReport` payload (the live snapshot + the Intel raw
            // dump + the system identity) lands in chunk 1b — until
            // then this arm returns a structured, wire-safe
            // "not wired yet" (the no-panic contract, D5: a plain
            // `Response::Error`, never a panic, never a partial
            // payload).
            write_frame(
                writer,
                &Message::Response(Response::Error(
                    "probe report not wired (chunk 1b)".to_owned(),
                )),
            )
            .await
        }
        // A client must never send a `Response` frame: protocol
        // violation, close the connection.
        Message::Response(_) => {
            Err(RpcError::Protocol("unexpected response frame".to_owned()))
        }
    }
}

/// Encode one message as a P3-11 frame and write it — `write_all` +
/// `flush` — to the connection's write half.
async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), message: &Message) -> Result<(), RpcError> {
    let frame = encode_frame(message).map_err(|e| RpcError::Protocol(e.to_string()))?;
    writer.write_all(&frame).await.map_err(RpcError::Io)?;
    writer.flush().await.map_err(RpcError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ramsleuth_bench::{BenchOp, Metric, StreamTarget, Tier};
    use ramsleuth_protocol::BenchMode;
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::SystemMemoryTelemetry;
    use ramsleuth_telemetry::SystemPlatform;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Mock snapshot built via the telemetry crate's public API
    /// (host-independent — the real `collect()` is never exercised in
    /// these tests): vendor branches all-`Na` + empty SPD,
    /// representative platform / capacity values (C6-13/14).
    fn mock_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Mock CPU".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(32.0),
            dimm_sizes: vec![Section::Value(16.0), Section::Value(16.0)],
        }
    }

    /// The MOCK `DaemonContext` every test shares: a `TelemetryCache`
    /// with an injectable mock collector (the fixed snapshot above,
    /// TTL 5 s) plus a real single-flight `BenchJobManager`.
    fn mock_ctx() -> Arc<DaemonContext> {
        let cache = TelemetryCache::new(mock_snapshot, Duration::from_secs(5));
        Arc::new(DaemonContext {
            cache: Arc::new(Mutex::new(cache)),
            jobs: Arc::new(BenchJobManager::new()),
        })
    }

    /// The test side of a `UnixStream::pair()` (in-memory connected
    /// pair — no socket file): a buffered frame reader plus an
    /// encoder, mirroring what the P3-18 client will do over std I/O.
    struct Conn {
        stream: tokio::net::UnixStream,
        buf: Vec<u8>,
    }

    impl Conn {
        fn new(stream: tokio::net::UnixStream) -> Self {
            Self { stream, buf: Vec::new() }
        }

        /// Encode + write one frame.
        async fn send(&mut self, message: &Message) {
            let frame = encode_frame(message).expect("test frame must encode");
            self.stream
                .write_all(&frame)
                .await
                .expect("test write must succeed");
            self.stream.flush().await.expect("test flush must succeed");
        }

        /// Read the next complete frame (incremental-reader contract:
        /// buffer, decode, drain by `consumed`); `None` on clean EOF.
        async fn recv(&mut self) -> Option<Message> {
            loop {
                if let Ok(frame) = decode_frame(&self.buf) {
                    self.buf.drain(..frame.consumed);
                    return Some(frame.message);
                }
                let mut tmp = [0u8; 8192];
                let n = self
                    .stream
                    .read(&mut tmp)
                    .await
                    .expect("test read must succeed");
                if n == 0 {
                    return None;
                }
                self.buf.extend_from_slice(&tmp[..n]);
            }
        }

        /// Receive frames until one whose `Response` satisfies
        /// `matches` (skipping streamed progress frames).
        async fn recv_until(&mut self, mut matches: impl FnMut(&Response) -> bool) -> Response {
            loop {
                let message = self.recv().await.expect("the stream must stay open");
                let Message::Response(response) = message else {
                    panic!("the daemon must never send a Request frame to a client")
                };
                if matches(&response) {
                    return response;
                }
            }
        }
    }

    /// (a) `GetTelemetry` is served from the mock cache (offloaded via
    /// `spawn_blocking`): the reply decodes to `Response::Telemetry`
    /// carrying the mock's fixed snapshot, and clean EOF ends the
    /// connection with `Ok(())`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_telemetry_serves_the_mock_snapshot() {
        let ctx = mock_ctx();
        let (client, server) = tokio::net::UnixStream::pair().expect("pair must connect");
        let task = tokio::spawn(handle_connection(server, Arc::clone(&ctx)));

        let mut conn = Conn::new(client);
        conn.send(&Message::Request(Request::GetTelemetry)).await;
        let message = conn.recv().await.expect("the telemetry reply frame");
        let Message::Response(Response::Telemetry(snap)) = message else {
            panic!("the first reply must be the Telemetry response: {message:?}")
        };
        assert_eq!(
            snap,
            mock_snapshot(),
            "the served snapshot must be the mock's fixed snapshot"
        );

        // Clean EOF: dropping the client end makes the reader see 0.
        drop(conn);
        task.await
            .expect("handle_connection must join")
            .expect("clean EOF must end the connection with Ok(())");
    }

    /// (b) `StartBenchmark` with a minimal target (one L1 Read cell,
    /// auto threads) streams `BenchStarted { run_id }`, then at least
    /// one `BenchProgress`, then the terminal `BenchResult` grid with
    /// the measured cell filled; clean EOF ends with `Ok(())`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_benchmark_streams_started_progress_and_result() {
        let ctx = mock_ctx();
        let (client, server) = tokio::net::UnixStream::pair().expect("pair must connect");
        let task = tokio::spawn(handle_connection(server, Arc::clone(&ctx)));

        let mut conn = Conn::new(client);
        conn.send(&Message::Request(Request::StartBenchmark {
            target: StreamTarget::Cell(Tier::L1, BenchOp::Read),
            mode: BenchMode::Full,
        }))
        .await;

        let message = conn.recv().await.expect("the started frame");
        let Message::Response(Response::BenchStarted { run_id }) = message else {
            panic!("the first reply must be BenchStarted: {message:?}")
        };
        assert!(run_id > 0, "the run id must be daemon-assigned");

        // The single cell streams exactly one progress event, in
        // order, before the terminal.
        let progress = conn
            .recv_until(|r| matches!(r, Response::BenchProgress(_)))
            .await;
        let Response::BenchProgress(p) = progress else {
            unreachable!("recv_until matched a BenchProgress")
        };
        assert_eq!(p.tier, Tier::L1);
        assert_eq!(p.op, BenchOp::Read);
        assert!(p.value.is_finite() && p.value > 0.0, "the cell must measure > 0: {p:?}");

        // The uncancelled run terminates with its measured grid.
        let terminal = conn
            .recv_until(|r| {
                matches!(
                    r,
                    Response::BenchResult { .. } | Response::BenchCancelled { .. }
                        | Response::Error(_)
                )
            })
            .await;
        let Response::BenchResult {
            run_id: result_id,
            grid,
        } = terminal
        else {
            panic!("an uncancelled single-cell run must end with BenchResult: {terminal:?}")
        };
        assert_eq!(result_id, run_id, "the terminal must tag the same run");
        assert!(
            grid.cell(Tier::L1, Metric::Read) > 0.0,
            "the requested cell must be measured in the grid: {grid:?}"
        );

        drop(conn);
        task.await
            .expect("handle_connection must join")
            .expect("clean EOF must end the connection with Ok(())");
    }

    /// (c) `CancelBenchmark` for the active run's id, sent from a
    /// **second** connection (the owner is busy forwarding its run):
    /// the reply is `BenchCancelled { run_id }` when the tiny run is
    /// still active, or the structured `Error` ack when it already
    /// finished — and the owner still receives its single terminal
    /// exactly once (plan D6).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_from_a_second_connection_acks_and_the_owner_gets_a_terminal() {
        let ctx = mock_ctx();
        let (client_owner, server_owner) = tokio::net::UnixStream::pair().expect("pair must connect");
        let (client_other, server_other) = tokio::net::UnixStream::pair().expect("pair must connect");
        let owner_task = tokio::spawn(handle_connection(server_owner, Arc::clone(&ctx)));
        let other_task = tokio::spawn(handle_connection(server_other, Arc::clone(&ctx)));

        let mut owner = Conn::new(client_owner);
        owner.send(&Message::Request(Request::StartBenchmark {
            target: StreamTarget::Cell(Tier::L1, BenchOp::Read),
            mode: BenchMode::Full,
        }))
        .await;
        let message = owner.recv().await.expect("the started frame");
        let Message::Response(Response::BenchStarted { run_id }) = message else {
            panic!("the first reply must be BenchStarted: {message:?}")
        };

        // A second connection cancels the run by its id.
        let mut other = Conn::new(client_other);
        other.send(&Message::Request(Request::CancelBenchmark { run_id })).await;
        let message = other.recv().await.expect("the cancel reply frame");
        let Message::Response(ack) = message else {
            panic!("the cancel reply must be a Response: {message:?}")
        };
        assert!(
            matches!(ack, Response::BenchCancelled { run_id: rid } if rid == run_id)
                || matches!(ack, Response::Error(_)),
            "the cancel ack must be BenchCancelled or the no-such-run Error: {ack:?}"
        );

        // The owner drains to its single terminal: `BenchCancelled`
        // when the cancel won the race, `BenchResult` when the tiny
        // run finished first.
        let terminal = owner
            .recv_until(|r| {
                matches!(
                    r,
                    Response::BenchResult { .. } | Response::BenchCancelled { .. }
                        | Response::Error(_)
                )
            })
            .await;
        assert!(
            matches!(terminal, Response::BenchResult { .. } | Response::BenchCancelled { .. }),
            "the owner's terminal must be the run's result or cancellation: {terminal:?}"
        );

        drop(owner);
        drop(other);
        owner_task
            .await
            .expect("owner handle_connection must join")
            .expect("clean EOF must end the owner with Ok(())");
        other_task
            .await
            .expect("other handle_connection must join")
            .expect("clean EOF must end the other connection with Ok(())");
    }

    /// (h) `StartBurnIn` (C7-07, D-1) over the wire: the reply is
    /// `BenchStarted { run_id }`; the run (infinite — `duration_minutes
    /// = 0`) is cancelled from a second connection, whose ack is the
    /// run's `BenchCancelled`; the owner then receives the buffered
    /// `BurnInProgress` tick burst (the drain-then-terminal contract —
    /// ticks land when the run ends) followed by the run's
    /// `BenchCancelled` terminal exactly once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_burn_in_streams_ticks_and_cancels_from_a_second_connection() {
        let ctx = mock_ctx();
        let (client_owner, server_owner) = tokio::net::UnixStream::pair().expect("pair must connect");
        let (client_other, server_other) = tokio::net::UnixStream::pair().expect("pair must connect");
        let owner_task = tokio::spawn(handle_connection(server_owner, Arc::clone(&ctx)));
        let other_task = tokio::spawn(handle_connection(server_other, Arc::clone(&ctx)));

        let mut owner = Conn::new(client_owner);
        owner.send(&Message::Request(Request::StartBurnIn {
            target: StreamTarget::Tier(Tier::L1),
            duration_minutes: 0,
        }))
        .await;
        let message = owner.recv().await.expect("the started frame");
        let Message::Response(Response::BenchStarted { run_id }) = message else {
            panic!("the first reply must be BenchStarted: {message:?}")
        };
        assert!(run_id > 0, "the run id must be daemon-assigned");

        // Let the run complete at least one iteration (L1 passes are
        // fast), then cancel the (infinite) run from a second
        // connection by its id.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let mut other = Conn::new(client_other);
        other.send(&Message::Request(Request::CancelBenchmark { run_id })).await;
        let message = other.recv().await.expect("the cancel reply frame");
        let Message::Response(ack) = message else {
            panic!("the cancel reply must be a Response: {message:?}")
        };
        assert!(
            matches!(ack, Response::BenchCancelled { run_id: rid } if rid == run_id),
            "the cancel ack must be the run's BenchCancelled: {ack:?}"
        );

        // The owner receives the buffered tick burst (a second of L1
        // iterations emitted ticks), then the run's terminal.
        let tick = owner
            .recv_until(|r| matches!(r, Response::BurnInProgress(_)))
            .await;
        let Response::BurnInProgress(t) = tick else {
            unreachable!("recv_until matched a BurnInProgress")
        };
        assert!(t.iteration >= 1, "iterations are 1-based");
        assert_eq!(t.tier, Tier::L1, "a tier target only ticks its tier");
        assert_eq!(
            t.bandwidth.is_some(),
            t.latency_ns.is_none(),
            "exactly one of bandwidth / latency per tick"
        );
        assert!(
            t.elapsed_secs.is_finite() && t.elapsed_secs >= 0.0,
            "the elapsed must be finite and non-negative: {}",
            t.elapsed_secs
        );

        let terminal = owner
            .recv_until(|r| {
                matches!(
                    r,
                    Response::BenchResult { .. } | Response::BenchCancelled { .. }
                        | Response::Error(_)
                )
            })
            .await;
        assert!(
            matches!(terminal, Response::BenchCancelled { run_id: rid } if rid == run_id),
            "the owner's terminal must be the run's BenchCancelled: {terminal:?}"
        );

        drop(owner);
        drop(other);
        owner_task
            .await
            .expect("owner handle_connection must join")
            .expect("clean EOF must end the owner with Ok(())");
        other_task
            .await
            .expect("other handle_connection must join")
            .expect("clean EOF must end the other connection with Ok(())");
    }

    /// (d) Protocol violation: a forged 4-byte LE length prefix above
    /// `MAX_FRAME_SIZE` (no payload needed — the guard fires before
    /// the payload is awaited) makes `handle_connection` return
    /// `Err(RpcError::Protocol)` and close the connection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_length_prefix_closes_with_protocol_error() {
        let ctx = mock_ctx();
        let (client, server) = tokio::net::UnixStream::pair().expect("pair must connect");
        let task = tokio::spawn(handle_connection(server, Arc::clone(&ctx)));

        let mut conn = Conn::new(client);
        conn.stream
            .write_all(&u32::MAX.to_le_bytes())
            .await
            .expect("test write must succeed");
        conn.stream.flush().await.expect("test flush must succeed");

        let result = task.await.expect("handle_connection must join");
        let RpcError::Protocol(msg) = result.as_ref().expect_err("the oversized frame must fail the connection")
        else {
            panic!("expected a Protocol error, got {result:?}")
        };
        assert!(
            msg.contains("maximum"),
            "the Protocol error must name the size guard: {msg}"
        );
    }

    /// (e) Protocol violation: a present-but-corrupt payload (the
    /// `Message` variant byte flipped to 0xFF — Bincode rejects it)
    /// makes `handle_connection` return `Err(RpcError::Protocol)`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn corrupt_payload_closes_with_protocol_error() {
        let ctx = mock_ctx();
        let (client, server) = tokio::net::UnixStream::pair().expect("pair must connect");
        let task = tokio::spawn(handle_connection(server, Arc::clone(&ctx)));

        let mut frame = encode_frame(&Message::Request(Request::GetTelemetry))
            .expect("the frame must encode");
        // `Message::Request(GetTelemetry)` serializes to two variant
        // bytes; the first payload byte (offset 4) at 0xFF is not a
        // variant of `Message`, so Bincode rejects the frame.
        frame[4] = 0xFF;
        let mut conn = Conn::new(client);
        conn.stream.write_all(&frame).await.expect("test write must succeed");
        conn.stream.flush().await.expect("test flush must succeed");

        let result = task.await.expect("handle_connection must join");
        let RpcError::Protocol(msg) = result.as_ref().expect_err("the corrupt frame must fail the connection")
        else {
            panic!("expected a Protocol error, got {result:?}")
        };
        assert!(
            msg.contains("decode"),
            "the Protocol error must name the decode failure: {msg}"
        );
    }

    /// (f) Protocol violation: a client-sent `Response` frame is
    /// rejected with the "unexpected response frame" Protocol error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_sent_response_frame_closes_with_protocol_error() {
        let ctx = mock_ctx();
        let (client, server) = tokio::net::UnixStream::pair().expect("pair must connect");
        let task = tokio::spawn(handle_connection(server, Arc::clone(&ctx)));

        let mut conn = Conn::new(client);
        conn.send(&Message::Response(Response::Error(
            "a client must not send responses".to_owned(),
        )))
        .await;

        let result = task.await.expect("handle_connection must join");
        let RpcError::Protocol(msg) = result.as_ref().expect_err("the response frame must fail the connection")
        else {
            panic!("expected a Protocol error, got {result:?}")
        };
        assert!(
            msg.contains("unexpected response frame"),
            "the Protocol error must say so: {msg}"
        );
    }

    /// (g) `RpcError`'s hand-implemented `Clone` / `PartialEq`
    /// (identity = variant + `kind` + text for the `io::Error` arm —
    /// the P3-13 precedent), plus `Display` and `std::error::Error`,
    /// behave across every arm.
    #[test]
    fn rpc_error_display_clone_partial_eq_and_error() {
        let io_err = RpcError::Io(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
        assert_eq!(io_err.clone(), io_err, "io arms compare by kind + text");
        assert!(io_err.to_string().contains("gone"));
        assert_ne!(
            io_err,
            RpcError::Io(io::Error::new(io::ErrorKind::BrokenPipe, "other")),
            "different text, different error"
        );
        assert_ne!(
            io_err,
            RpcError::Io(io::Error::new(io::ErrorKind::PermissionDenied, "gone")),
            "different kind, different error"
        );

        let protocol = RpcError::Protocol("unexpected response frame".to_owned());
        assert_eq!(protocol.clone(), protocol);
        assert_eq!(
            RpcError::Protocol("a".to_owned()),
            RpcError::Protocol("a".to_owned())
        );
        assert_ne!(protocol, RpcError::Protocol("other".to_owned()));
        assert_eq!(RpcError::Job("busy".to_owned()), RpcError::Job("busy".to_owned()));
        assert_eq!(RpcError::Join("panic".to_owned()), RpcError::Join("panic".to_owned()));
        assert_ne!(
            RpcError::Job("x".to_owned()),
            RpcError::Join("x".to_owned()),
            "different arms are never equal"
        );

        for err in [
            &protocol,
            &RpcError::Job("benchmark already running".to_owned()),
            &RpcError::Join("worker panicked".to_owned()),
        ] {
            let as_error: &dyn std::error::Error = err;
            assert!(!as_error.to_string().is_empty());
        }
        assert!(
            protocol.to_string().contains("protocol violation"),
            "the Protocol display must name the class: {protocol}"
        );
    }
}
