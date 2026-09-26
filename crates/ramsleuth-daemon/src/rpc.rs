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
//! **Probe report:** [`Request::GetProbeReport`] → the chunk 1b daemon
//! builder: take the P3-14 TTL cache (offloaded to the blocking pool,
//! same as [`Request::GetTelemetry`]), then assemble the
//! [`ProbeReport`] — the decoded snapshot + the vendor raw dump (Intel:
//! module-first, `/dev/mem` fallback, plan §3.5; AMD: the `ryzen_smu`
//! SMN/PM section) + the system identity — all inside one
//! `spawn_blocking` (the raw acquisition may `mmap` / read sysfs) →
//! [`Response::ProbeReport`]; a failed raw acquisition degrades to
//! `raw: None` + `telemetry_source: "unavailable"` (no-panic, D5) — the
//! arm never errors the connection.
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
use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::TelemetryError;
use ramsleuth_telemetry::intel_gen::{self, GenMap};
use ramsleuth_telemetry::intel_readout::IntelImcRegs;
use ramsleuth_telemetry::intel_sysfs::SysfsRegs;
use ramsleuth_telemetry::{ProbeRaw, ProbeReport, ProbeSystem, SystemMemoryTelemetry};
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
            // wire types; chunk 1b daemon builder). Take the shared P3-14
            // TTL cache (the same offloaded `get` as the `GetTelemetry`
            // arm — `get` is `&mut self` and may call the slow
            // collector, so it runs on the blocking pool behind the
            // mutex), then assemble the [`ProbeReport`]: the decoded
            // snapshot + the vendor raw dump (Intel: module-first,
            // `/dev/mem` fallback — plan §3.5; AMD: the `ryzen_smu`
            // SMN/PM section) + the system identity. The raw
            // acquisition may `mmap` / read sysfs, so it runs in the same
            // `spawn_blocking`. A failed raw acquisition degrades to
            // `raw: None` + `telemetry_source: "unavailable"` (no-panic
            // contract, D5) — the arm never errors the connection and
            // never sends a partial / malformed payload.
            let cache = Arc::clone(&ctx.cache);
            let report = tokio::task::spawn_blocking(move || {
                let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
                let telemetry = c.get();
                build_probe_report(&telemetry, acquire_raw(), SystemIdentity::detect())
            })
            .await
            .map_err(|e| RpcError::Join(e.to_string()))?;
            write_frame(writer, &Message::Response(Response::ProbeReport(report))).await
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

// ---------------------------------------------------------------------------
// chunk-probe-1b: the `GetProbeReport` daemon-side builder.
// ---------------------------------------------------------------------------

/// The raw Intel register set, normalized across the two acquisition
/// sources (the `ramsleuth_intel` sysfs kobject and the `/dev/mem` MCHBAR
/// map) so a single flatten ([`probe_raw_from_source`]) builds the wire
/// [`ProbeRaw`] from either (plan §3.5: module-first, devmem-fallback).
#[derive(Debug, Clone)]
struct ProbeRawSource {
    /// The 51-slot raw register set.
    regs: IntelImcRegs,
    /// MCHBAR physical base (`None` when absent / malformed).
    mchbar_base: Option<u64>,
    /// MCHBAR enable bit (`false` when absent / malformed).
    mchbar_enabled: bool,
    /// The 7 global MAD channel/geometry raws — module-sysfs-only; the
    /// `/dev/mem` path carries none of them (`None`).
    mad_inter_channel: Option<u32>,
    mad_intra_ch0: Option<u32>,
    mad_intra_ch1: Option<u32>,
    mad_dimm_ch0: Option<u32>,
    mad_dimm_ch1: Option<u32>,
    mad_dimm_ch2: Option<u32>,
    mad_dimm_ch3: Option<u32>,
}

/// The raw AMD register set, normalized from the `ryzen_smu` driver: the
/// 13 SMN DRAM timing words + the PM-table blob (from which the key f32
/// values are extracted) + the PM version.
#[derive(Debug, Clone)]
struct ProbeAmdSource {
    /// The 13 SMN DRAM register words in address order (`None` per failed
    /// read); empty when no `smn` attribute was available.
    smn_regs: Vec<Option<u32>>,
    /// The PM-table `TableVersionId` (`None` when no blob was acquired).
    pm_version: Option<u32>,
    /// The raw PM-table blob (headerless f32 array); empty when unavailable.
    pm_blob: Vec<u8>,
}

/// The raw-acquisition outcome: which source produced the register set
/// (drives the `telemetry_source` field) — the Intel sysfs kobject, the
/// Intel `/dev/mem` MCHBAR map, the AMD `ryzen_smu` driver, or none.
#[derive(Debug)]
enum RawAcquisition {
    /// The `ramsleuth_intel` sysfs kobject (the primary, world-readable
    /// path).
    Sysfs(ProbeRawSource),
    /// The `/dev/mem` MCHBAR map (the root + `CAP_SYS_RAWIO` fallback).
    DevMem(ProbeRawSource),
    /// The `ryzen_smu` driver (AMD): the SMN DRAM words + the PM-table blob
    /// (the AMD raw section).
    Amd(ProbeAmdSource),
    /// No raw source was available (unknown silicon, or every source for
    /// the detected vendor failed).
    Unavailable,
}

/// The OS / system identity facts that do not come from the telemetry
/// snapshot (the host-I/O half of [`ProbeSystem`]): kept separate and
/// injectable so [`build_probe_report`] is a pure, unit-testable
/// function (no I/O).
#[derive(Debug, Clone)]
struct SystemIdentity {
    /// The PCI host-bridge device id (e.g. "0x066C"); `None` when the
    /// sysfs file is absent.
    pci_host_bridge: Option<String>,
    /// The kernel release (`/proc/sys/kernel/osrelease`).
    kernel: String,
    /// The OS name (`std::env::consts::OS`).
    os: String,
    /// The CPU architecture (`std::env::consts::ARCH`).
    arch: String,
    /// The RamSleuth version that produced the report.
    ramsleuth_version: String,
}

impl SystemIdentity {
    /// Read the host identity facts (the production path — the daemon
    /// is root, so these sysfs / proc reads succeed on Linux).
    fn detect() -> Self {
        Self {
            pci_host_bridge: read_pci_host_bridge_device(),
            kernel: read_kernel_release(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            ramsleuth_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Assemble a [`ProbeReport`] from a decoded telemetry snapshot, a raw
/// acquisition, and the system identity.
///
/// **Pure (no I/O):** the raw acquisition ([`RawAcquisition`]) and the
/// system identity ([`SystemIdentity`]) are precomputed and passed in
/// — the production caller builds them via [`acquire_raw`] and
/// [`SystemIdentity::detect`] inside the `spawn_blocking` closure. This
/// keeps the builder unit-testable with mock inputs (the chunk 1b
/// quality gate).
fn build_probe_report(
    telemetry: &SystemMemoryTelemetry,
    raw: RawAcquisition,
    identity: SystemIdentity,
) -> ProbeReport {
    let (raw, telemetry_source) = match raw {
        RawAcquisition::Sysfs(source) => (Some(probe_raw_from_source(&source)), "sysfs-module"),
        RawAcquisition::DevMem(source) => (Some(probe_raw_from_source(&source)), "dev-mem"),
        RawAcquisition::Amd(source) => (Some(probe_raw_from_amd_source(&source)), "ryzen_smu"),
        RawAcquisition::Unavailable => (None, "unavailable"),
    };
    let system = ProbeSystem {
        cpu_brand: telemetry.cpu.brand.clone(),
        cpu_vendor: cpu_vendor_string(&telemetry.cpu.vendor),
        cpu_gen: cpu_gen_string(&telemetry.cpu.vendor),
        pci_host_bridge: identity.pci_host_bridge,
        kernel: identity.kernel,
        os: identity.os,
        arch: identity.arch,
        ramsleuth_version: identity.ramsleuth_version,
        telemetry_source: telemetry_source.to_owned(),
    };
    ProbeReport {
        telemetry: telemetry.clone(),
        raw,
        system,
    }
}

/// Flatten one [`ProbeRawSource`] into the wire [`ProbeRaw`].
///
/// The field mapping is one-to-one and **order-stable**: the [`ProbeRaw`]
/// declaration order (the chunk 1a wire contract) is reproduced exactly,
/// so the flattened set crosses the wire verbatim. Every `Option<u32>`
/// register carries the source's per-register containment (`None` on a
/// failed / absent read); the MCHBAR diagnostics ride as-is.
fn probe_raw_from_source(source: &ProbeRawSource) -> ProbeRaw {
    let r = &source.regs;
    ProbeRaw {
        mcbios_req: r.mcbios_req,
        tc_ch0_dbp: r.ch0.tc_dbp,
        tc_ch0_rap: r.ch0.tc_rap,
        tc_ch0_rfp: r.ch0.tc_rfp,
        tc_ch0_rap2: r.ch0.tc_rap2,
        tc_ch0_rdrd: r.ch0.tc_rdrd,
        tc_ch0_rdwr: r.ch0.tc_rdwr,
        tc_ch0_wrrd: r.ch0.tc_wrrd,
        tc_ch0_wrwr: r.ch0.tc_wrwr,
        tc_ch1_dbp: r.ch1.tc_dbp,
        tc_ch1_rap: r.ch1.tc_rap,
        tc_ch1_rfp: r.ch1.tc_rfp,
        tc_ch1_rap2: r.ch1.tc_rap2,
        tc_ch1_rdrd: r.ch1.tc_rdrd,
        tc_ch1_rdwr: r.ch1.tc_rdwr,
        tc_ch1_wrrd: r.ch1.tc_wrrd,
        tc_ch1_wrwr: r.ch1.tc_wrwr,
        tc_ch2_dbp: r.ch2.tc_dbp,
        tc_ch2_rap: r.ch2.tc_rap,
        tc_ch2_rfp: r.ch2.tc_rfp,
        tc_ch2_rap2: r.ch2.tc_rap2,
        tc_ch2_rdrd: r.ch2.tc_rdrd,
        tc_ch2_rdwr: r.ch2.tc_rdwr,
        tc_ch2_wrrd: r.ch2.tc_wrrd,
        tc_ch2_wrwr: r.ch2.tc_wrwr,
        tc_ch3_dbp: r.ch3.tc_dbp,
        tc_ch3_rap: r.ch3.tc_rap,
        tc_ch3_rfp: r.ch3.tc_rfp,
        tc_ch3_rap2: r.ch3.tc_rap2,
        tc_ch3_rdrd: r.ch3.tc_rdrd,
        tc_ch3_rdwr: r.ch3.tc_rdwr,
        tc_ch3_wrrd: r.ch3.tc_wrrd,
        tc_ch3_wrwr: r.ch3.tc_wrwr,
        mcl0_pre: r.mcl0.tc_pre,
        mcl0_act: r.mcl0.tc_act,
        mcl0_act2: r.mcl0.tc_act2,
        mcl0_wtr: r.mcl0.tc_wtr,
        mcl0_rfp: r.mcl0.tc_rfp,
        mcl0_rfp2: r.mcl0.tc_rfp2,
        mcl0_rdrd: r.mcl0.tc_rdrd,
        mcl0_wrwr: r.mcl0.tc_wrwr,
        mcl1_pre: r.mcl1.tc_pre,
        mcl1_act: r.mcl1.tc_act,
        mcl1_act2: r.mcl1.tc_act2,
        mcl1_wtr: r.mcl1.tc_wtr,
        mcl1_rfp: r.mcl1.tc_rfp,
        mcl1_rfp2: r.mcl1.tc_rfp2,
        mcl1_rdrd: r.mcl1.tc_rdrd,
        mcl1_wrwr: r.mcl1.tc_wrwr,
        mad_inter_channel: source.mad_inter_channel,
        mad_intra_ch0: source.mad_intra_ch0,
        mad_intra_ch1: source.mad_intra_ch1,
        mad_dimm_ch0: source.mad_dimm_ch0,
        mad_dimm_ch1: source.mad_dimm_ch1,
        mad_dimm_ch2: source.mad_dimm_ch2,
        mad_dimm_ch3: source.mad_dimm_ch3,
        mchbar_base: source.mchbar_base,
        mchbar_enabled: source.mchbar_enabled,
        // AMD raw: absent (the Intel source carries no AMD data).
        ..Default::default()
    }
}

/// Flatten one [`ProbeAmdSource`] into the wire [`ProbeRaw`] AMD section:
/// the 13 SMN words + the PM version / blob length + the five key f32
/// values (extracted from the blob). Every Intel field is absent (an AMD
/// report carries no Intel raw).
fn probe_raw_from_amd_source(source: &ProbeAmdSource) -> ProbeRaw {
    let key_values = ramsleuth_telemetry::amd_smu::read_pm_key_values(&source.pm_blob);
    ProbeRaw {
        amd_smn_regs: source.smn_regs.clone(),
        amd_pm_version: source.pm_version,
        amd_pm_blob_len: (!source.pm_blob.is_empty())
            .then(|| u32::try_from(source.pm_blob.len()).unwrap_or(u32::MAX)),
        amd_pm_vddcr_vdd: key_values[0],
        amd_pm_vddcr_soc: key_values[1],
        amd_pm_fclk: key_values[2],
        amd_pm_uclk: key_values[3],
        amd_pm_mclk: key_values[4],
        // All Intel raw: absent (an AMD report carries none).
        ..Default::default()
    }
}

/// Normalize the `ramsleuth_intel` sysfs kobject raw set into a
/// [`ProbeRawSource`] (the MCHBAR diagnostics + the 7 global MAD raws
/// ride the module's sysfs surface).
fn raw_source_from_sysfs(sysfs: &SysfsRegs) -> ProbeRawSource {
    ProbeRawSource {
        regs: sysfs.regs.clone(),
        mchbar_base: sysfs.mchbar.base,
        mchbar_enabled: sysfs.mchbar.enabled.unwrap_or(false),
        mad_inter_channel: sysfs.mad_inter_channel,
        mad_intra_ch0: sysfs.mad_intra_ch0,
        mad_intra_ch1: sysfs.mad_intra_ch1,
        mad_dimm_ch0: sysfs.mad_dimm_ch0,
        mad_dimm_ch1: sysfs.mad_dimm_ch1,
        mad_dimm_ch2: sysfs.mad_dimm_ch2,
        mad_dimm_ch3: sysfs.mad_dimm_ch3,
    }
}

/// The CPU vendor as the wire string ("Intel" / "AMD" / "Unknown").
fn cpu_vendor_string(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Intel(_) => "Intel".to_owned(),
        CpuVendor::Amd(_) => "AMD".to_owned(),
        CpuVendor::Unknown => "Unknown".to_owned(),
    }
}

/// The CPU generation as its variant name (e.g. "RocketLake", "Zen5";
/// "Unknown" for an unrecognized vendor).
fn cpu_gen_string(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Intel(gen) => format!("{gen:?}"),
        CpuVendor::Amd(zen) => format!("{zen:?}"),
        CpuVendor::Unknown => "Unknown".to_owned(),
    }
}

/// The PCI host-bridge device id from sysfs
/// (`/sys/bus/pci/devices/0000:00:00.0/device`, e.g. "0x066C"); `None`
/// when the file is absent or empty (the daemon is root, so a present
/// file is readable).
fn read_pci_host_bridge_device() -> Option<String> {
    std::fs::read_to_string("/sys/bus/pci/devices/0000:00:00.0/device")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The kernel release string from `/proc/sys/kernel/osrelease` (empty
/// when the file is absent — the daemon runs on Linux, so it is normally
/// present).
fn read_kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Acquire the raw register set from the production sources for the
/// detected vendor: Intel (module-first, devmem-fallback — the plan §3.5 /
/// facade contract) and AMD (the `ryzen_smu` driver — the SMN DRAM words +
/// the PM-table blob).
///
/// Returns [`RawAcquisition`]: for Intel, the sysfs kobject when loaded
/// (the primary, world-readable path) or the `/dev/mem` MCHBAR map when the
/// sysfs outcome is exactly `DriverMissing` (kobject absent — the root +
/// `CAP_SYS_RAWIO` fallback); for AMD, the `ryzen_smu` driver when it
/// yields a PM blob and/or the SMN register set; and
/// [`RawAcquisition::Unavailable`] when no source for the detected vendor
/// yields a set (unknown silicon — the vendor gate fires before any I/O; a
/// non-`DriverMissing` Intel sysfs fault; a failed devmem map; an AMD
/// driver that yields neither source). Never panics (the no-panic
/// contract, D5).
fn acquire_raw() -> RawAcquisition {
    // The frozen facade vendor gate: reject before any I/O (zero I/O for an
    // unrecognized vendor).
    let info = CpuInfo::detect();
    match info.vendor {
        // Intel: module-first, devmem-fallback (plan §3.5).
        CpuVendor::Intel(_) => match ramsleuth_telemetry::intel_sysfs::acquire() {
            Ok(sysfs) => RawAcquisition::Sysfs(raw_source_from_sysfs(&sysfs)),
            // The frozen fallthrough: ONLY a `DriverMissing` (kobject
            // absent — module not loaded) takes the `/dev/mem` path. Any
            // other sysfs outcome means the module is present (hardware
            // reachable) — do not silently switch sources (facade §3.5).
            Err(TelemetryError::DriverMissing { .. }) => acquire_devmem(&info),
            Err(_) => RawAcquisition::Unavailable,
        },
        // AMD: the `ryzen_smu` driver (the PM-table blob + the SMN DRAM
        // words — the AMD raw section).
        CpuVendor::Amd(_) => acquire_amd_raw(),
        // Unknown silicon: no raw source (zero I/O).
        CpuVendor::Unknown => RawAcquisition::Unavailable,
    }
}

/// The `/dev/mem` MCHBAR fallback: map the window read-only and read the
/// register set (the `read_intel` routing contract: the Tier-3 Alder
/// family takes the 256 KiB wide set; Tier 1 / Rocket Lake keep the
/// 64 KiB set).
fn acquire_devmem(info: &CpuInfo) -> RawAcquisition {
    let bar = match ramsleuth_telemetry::intel_mchbar::acquire() {
        Ok(bar) => bar,
        Err(_) => return RawAcquisition::Unavailable,
    };
    let gen = match ramsleuth_telemetry::intel_readout::intel_gen_gate(info) {
        Ok(gen) => gen,
        Err(_) => return RawAcquisition::Unavailable,
    };
    let regs = match intel_gen::profile_for(gen).map(|p| p.map) {
        Some(GenMap::Alder) => IntelImcRegs::from_bar_wide(&bar),
        _ => IntelImcRegs::from_bar(&bar),
    };
    RawAcquisition::DevMem(ProbeRawSource {
        regs,
        // A successfully mapped, decoded MCHBAR is enabled by definition
        // (a disabled / zero base fails `acquire` with
        // `UnsupportedHardware` before this point).
        mchbar_base: Some(bar.base()),
        mchbar_enabled: true,
        // The 7 MAD raws are module-sysfs-only — the `/dev/mem` path
        // carries none of them.
        mad_inter_channel: None,
        mad_intra_ch0: None,
        mad_intra_ch1: None,
        mad_dimm_ch0: None,
        mad_dimm_ch1: None,
        mad_dimm_ch2: None,
        mad_dimm_ch3: None,
    })
}

/// Acquire the AMD raw section from the `ryzen_smu` driver: the PM-table
/// blob (version + the five key f32 values) + the 13 SMN DRAM words.
///
/// Returns [`RawAcquisition::Amd`] when at least one source yields data
/// (a PM blob, or a non-empty SMN register list), and
/// [`RawAcquisition::Unavailable`] when both are absent. Never panics
/// (no-panic contract, D5): each source degrades to its structured error,
/// collapsed here.
fn acquire_amd_raw() -> RawAcquisition {
    // The PM-table blob (the `ryzen_smu` `pm_table`): `None` when the
    // driver is absent / the read fails.
    let pm = ramsleuth_telemetry::amd_smu::acquire().ok();
    // The 13 SMN DRAM words: empty when no `smn` attribute is available.
    let smn_regs = ramsleuth_telemetry::amd_smn::read_all_dram_registers();

    if pm.is_some() || !smn_regs.is_empty() {
        RawAcquisition::Amd(ProbeAmdSource {
            smn_regs,
            pm_version: pm.as_ref().map(|ctx| ctx.version),
            pm_blob: pm.map(|ctx| ctx.pm).unwrap_or_default(),
        })
    } else {
        RawAcquisition::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ramsleuth_bench::{BenchOp, Metric, StreamTarget, Tier};
    use ramsleuth_protocol::BenchMode;
    use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor, IntelGen};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::intel_readout::ChannelRegs;
    use ramsleuth_telemetry::intel_sysfs::{MchBarInfo, SysfsRegs};
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

    // ------------------------------------------------------------------
    // (i) chunk-probe-1b: the daemon-side `ProbeReport` builder.
    // ------------------------------------------------------------------

    /// A mock `ramsleuth_intel` sysfs raw set (host-independent): the
    /// Skylake acceptance `ch0` block + the MCHBAR diagnostics + the 5
    /// global MAD raws; every other register / the Tier-3 extension
    /// fields stay `None` (the all-`Default` containment shape).
    fn mock_sysfs_regs() -> SysfsRegs {
        let regs = IntelImcRegs {
            mcbios_req: Some(0x0000_0012),
            ch0: ChannelRegs {
                tc_dbp: Some(0x1111_0F11),
                tc_rap: Some(0x2718_0204),
                tc_rfp: Some(0x0000_01A4),
                tc_rap2: Some(0x0000_0C0A),
                tc_rdrd: Some(0x0048_C286),
                tc_rdwr: Some(0x0000_0280),
                tc_wrrd: Some(0x0000_0308),
                tc_wrwr: Some(0x0040_C204),
            },
            ..IntelImcRegs::default()
        };
        SysfsRegs {
            regs,
            mchbar: MchBarInfo {
                base: Some(0xFED1_0000),
                enabled: Some(true),
            },
            mad_inter_channel: Some(0x0000_0003),
            mad_intra_ch0: Some(0x0000_0005),
            mad_intra_ch1: Some(0x0000_0007),
            mad_dimm_ch0: Some(0x0000_0008),
            mad_dimm_ch1: Some(0x0000_000C),
            mad_dimm_ch2: None,
            mad_dimm_ch3: None,
            capid0a: None,
        }
    }

    /// A mock Intel [`SystemMemoryTelemetry`] (Rocket Lake) carrying the
    /// vendor identity fields the builder reads; the vendor branches /
    /// capacities are host-independent `Na` / `Value` placeholders.
    fn mock_intel_telemetry() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::RocketLake),
                brand: "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz".to_owned(),
            },
            amd: Section::na(NaReason::UnsupportedHardware),
            intel: Section::na(NaReason::DriverMissing),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3600.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
        }
    }

    /// A mock [`SystemIdentity`] (host-independent, fixed values — the
    /// injectable half that keeps the builder pure).
    fn mock_identity() -> SystemIdentity {
        SystemIdentity {
            pci_host_bridge: Some("0x066C".to_owned()),
            kernel: "6.6.0-1-cachyos".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            ramsleuth_version: "2.4.6".to_owned(),
        }
    }

    /// (i1) The builder assembles the [`ProbeReport`] from a mock
    /// snapshot + mock sysfs raws + mock identity: the `raw` is the
    /// flattened [`ProbeRaw`] (the populated `ch0` block + `MC_BIOS_REQ`
    /// land in their flat slots, the MAD raws + MCHBAR diagnostics ride,
    /// and the absent / Tier-3 fields stay `None`), and the `system` is
    /// the probe identity (the Intel vendor / "RocketLake" generation
    /// strings, the injected host facts, and `telemetry_source` =
    /// "sysfs-module").
    #[test]
    fn build_probe_report_from_sysfs_raws() {
        let telemetry = mock_intel_telemetry();
        let sysfs = mock_sysfs_regs();
        let report = build_probe_report(
            &telemetry,
            RawAcquisition::Sysfs(raw_source_from_sysfs(&sysfs)),
            mock_identity(),
        );

        // The telemetry snapshot is carried verbatim.
        assert_eq!(report.telemetry, telemetry);

        // The raw is present (the sysfs path) with the flattened fields.
        let raw = report.raw.expect("the sysfs path must populate the raw");
        assert_eq!(raw.mcbios_req, Some(0x0000_0012));
        assert_eq!(raw.tc_ch0_dbp, Some(0x1111_0F11));
        assert_eq!(raw.tc_ch0_rap, Some(0x2718_0204));
        assert_eq!(raw.tc_ch0_rfp, Some(0x0000_01A4));
        assert_eq!(raw.tc_ch0_rap2, Some(0x0000_0C0A));
        assert_eq!(raw.tc_ch0_rdrd, Some(0x0048_C286));
        assert_eq!(raw.tc_ch0_rdwr, Some(0x0000_0280));
        assert_eq!(raw.tc_ch0_wrrd, Some(0x0000_0308));
        assert_eq!(raw.tc_ch0_wrwr, Some(0x0040_C204));
        // The unpopulated `ch1` block stays all-`None`.
        assert_eq!(raw.tc_ch1_dbp, None);
        assert_eq!(raw.tc_ch1_rap, None);
        // The Tier-3 extension fields stay `None`.
        assert_eq!(raw.tc_ch2_dbp, None);
        assert_eq!(raw.tc_ch3_dbp, None);
        assert_eq!(raw.mcl0_pre, None);
        assert_eq!(raw.mcl1_pre, None);
        // The global MAD raws ride.
        assert_eq!(raw.mad_inter_channel, Some(0x0000_0003));
        assert_eq!(raw.mad_intra_ch0, Some(0x0000_0005));
        assert_eq!(raw.mad_intra_ch1, Some(0x0000_0007));
        assert_eq!(raw.mad_dimm_ch0, Some(0x0000_0008));
        assert_eq!(raw.mad_dimm_ch1, Some(0x0000_000C));
        assert_eq!(raw.mad_dimm_ch2, None);
        assert_eq!(raw.mad_dimm_ch3, None);
        // The MCHBAR diagnostics ride.
        assert_eq!(raw.mchbar_base, Some(0xFED1_0000));
        assert!(raw.mchbar_enabled);

        // The system identity.
        assert_eq!(
            report.system.cpu_brand,
            "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz"
        );
        assert_eq!(report.system.cpu_vendor, "Intel");
        assert_eq!(report.system.cpu_gen, "RocketLake");
        assert_eq!(report.system.pci_host_bridge, Some("0x066C".to_owned()));
        assert_eq!(report.system.kernel, "6.6.0-1-cachyos");
        assert_eq!(report.system.os, "linux");
        assert_eq!(report.system.arch, "x86_64");
        assert_eq!(report.system.ramsleuth_version, "2.4.6");
        assert_eq!(report.system.telemetry_source, "sysfs-module");
    }

    /// (i2) The unavailable arm: neither raw source (AMD / unknown
    /// silicon, or both paths failing) yields `raw: None` +
    /// `telemetry_source` = "unavailable"; the snapshot + identity still
    /// assemble.
    #[test]
    fn build_probe_report_unavailable_raw() {
        let telemetry = mock_intel_telemetry();
        let report = build_probe_report(&telemetry, RawAcquisition::Unavailable, mock_identity());
        assert_eq!(report.telemetry, telemetry);
        assert_eq!(report.raw, None);
        assert_eq!(report.system.telemetry_source, "unavailable");
        assert_eq!(report.system.cpu_vendor, "Intel");
        assert_eq!(report.system.cpu_gen, "RocketLake");
    }

    /// (i3) The built report (raw `Some`) is wire-safe: the
    /// `Response::ProbeReport` frame round-trips through the P3-11
    /// frame codec (length-prefixed bincode) and decodes back to an
    /// identical report (the chunk 1a wire contract, exercised
    /// end-to-end from the daemon builder).
    #[test]
    fn build_probe_report_round_trips_through_the_frame_codec() {
        let telemetry = mock_intel_telemetry();
        let sysfs = mock_sysfs_regs();
        let report = build_probe_report(
            &telemetry,
            RawAcquisition::Sysfs(raw_source_from_sysfs(&sysfs)),
            mock_identity(),
        );
        let frame = encode_frame(&Message::Response(Response::ProbeReport(report.clone())))
            .expect("the built report frame must encode");
        let decoded = decode_frame(&frame).expect("the built report frame must decode");
        let Message::Response(Response::ProbeReport(back)) = decoded.message else {
            panic!("the decoded frame must be the ProbeReport response: {decoded:?}")
        };
        assert_eq!(back, report);
    }

    /// A mock AMD [`ProbeAmdSource`] (host-independent): the 13 SMN words
    /// (the last a failed read) + the PM version + a PM blob long enough for
    /// the five key f32 values (seeded with distinct bit patterns).
    fn mock_amd_source() -> ProbeAmdSource {
        // A blob long enough for every key offset (>= 0x0C8 + 4 = 208 bytes).
        let mut blob = vec![0u8; 0x0D0];
        let patterns = [
            0x3E4C_CCCDu32, // VDDCR_VDD @ 0x0A0
            0x3EF2_CCCCu32, // VDDCR_SOC @ 0x0B0
            0x40F0_0000u32, // FCLK @ 0x0C0
            0x40C8_0000u32, // UCLK @ 0x0C8
            0x40C8_0000u32, // MCLK @ 0x0CC
        ];
        let offsets = [0x0A0, 0x0B0, 0x0C0, 0x0C8, 0x0CC];
        for (pattern, off) in patterns.iter().zip(offsets.iter()) {
            blob[*off..*off + 4].copy_from_slice(&pattern.to_le_bytes());
        }
        ProbeAmdSource {
            smn_regs: vec![
                Some(0x0000_1539),
                Some(0x1010_2410),
                Some(0x0010_0030),
                Some(0x0400_0404),
                Some(0x0000_0010),
                Some(0x0008_0410),
                Some(0x0000_0010),
                Some(0x0504_0302),
                Some(0x0908_0706),
                Some(0x0000_0602),
                Some(0x0400_0000),
                Some(0x7E08_20A0),
                None, // 0x50264: a failed read
            ],
            pm_version: Some(0x0038_0805),
            pm_blob: blob,
        }
    }

    /// (i4) The AMD arm: `build_probe_report` assembles the report from a
    /// mock AMD source: `telemetry_source` = "ryzen_smu", the AMD raw
    /// section populated (the 13 SMN words, the PM version / blob length,
    /// the five key f32 values), and every Intel raw field absent.
    #[test]
    fn build_probe_report_from_amd_source() {
        let telemetry = mock_snapshot();
        let report = build_probe_report(
            &telemetry,
            RawAcquisition::Amd(mock_amd_source()),
            mock_identity(),
        );

        // The AMD source's identity.
        assert_eq!(report.system.telemetry_source, "ryzen_smu");

        let raw = report.raw.expect("the AMD path must populate the raw");
        // The 13 SMN words (the last is a failed read).
        assert_eq!(raw.amd_smn_regs.len(), 13);
        assert_eq!(raw.amd_smn_regs[0], Some(0x0000_1539));
        assert_eq!(raw.amd_smn_regs[11], Some(0x7E08_20A0));
        assert_eq!(raw.amd_smn_regs[12], None);
        // The PM version + blob length.
        assert_eq!(raw.amd_pm_version, Some(0x0038_0805));
        assert_eq!(raw.amd_pm_blob_len, Some(0x0D0));
        // The five key f32 values (the seeded bit patterns).
        assert_eq!(raw.amd_pm_vddcr_vdd, Some(0x3E4C_CCCD));
        assert_eq!(raw.amd_pm_vddcr_soc, Some(0x3EF2_CCCC));
        assert_eq!(raw.amd_pm_fclk, Some(0x40F0_0000));
        assert_eq!(raw.amd_pm_uclk, Some(0x40C8_0000));
        assert_eq!(raw.amd_pm_mclk, Some(0x40C8_0000));
        // Every Intel raw field is absent (an AMD report carries none).
        assert_eq!(raw.mcbios_req, None);
        assert_eq!(raw.tc_ch0_dbp, None);
        assert_eq!(raw.mad_dimm_ch3, None);
        assert_eq!(raw.mchbar_base, None);
        assert!(!raw.mchbar_enabled);
    }

    /// (i5) An AMD-built report is wire-safe: the `Response::ProbeReport`
    /// frame round-trips through the P3-11 frame codec and decodes back to
    /// an identical report (the appended AMD fields cross the wire).
    #[test]
    fn build_probe_report_amd_round_trips_through_the_frame_codec() {
        let telemetry = mock_snapshot();
        let report = build_probe_report(
            &telemetry,
            RawAcquisition::Amd(mock_amd_source()),
            mock_identity(),
        );
        let frame = encode_frame(&Message::Response(Response::ProbeReport(
            report.clone(),
        )))
        .expect("the built report frame must encode");
        let decoded = decode_frame(&frame).expect("the built report frame must decode");
        let Message::Response(Response::ProbeReport(back)) = decoded.message else {
            panic!("the decoded frame must be the ProbeReport response: {decoded:?}")
        };
        assert_eq!(back, report);
    }
}
