//! Wire RPC messages — the frozen message contract (P3-10).
//!
//! One [`Message`] per frame, per direction: a client sends
//! [`Message::Request`], the daemon replies [`Message::Response`] (plus
//! streamed progress frames to the owning benchmark connection, plan D6).
//!
//! **Payloads are reused, never duplicated (plan D2):** [`Request`] /
//! [`Response`] embed the telemetry crate's `SystemMemoryTelemetry` and
//! the bench crate's `StreamTarget` / `StreamProgress` /
//! `BenchmarkGrid` verbatim — the single source of truth for each is its
//! owning crate. This module owns only the four wire enums and the
//! socket-path constant.
//!
//! **No-panic contract:** every arm is bincode-serializable (plan D3);
//! failures cross the wire as structured payloads — [`Response::Error`]
//! and the `Na(reason)` sections inside a snapshot — never as panics.

use ramsleuth_bench::{BenchmarkGrid, StreamProgress, StreamTarget};
use ramsleuth_telemetry::SystemMemoryTelemetry;

/// The default daemon Unix-socket path: the single socket-path source
/// for the daemon and every client (plan D5). The daemon accepts a
/// `--socket` override so it can run unprivileged for local dev
/// (e.g. `--socket /tmp/ramsleuth.sock`).
pub const DEFAULT_SOCKET_PATH: &str = "/run/ramsleuth/ramsleuth.sock";

/// The benchmark scope a client asks for (Grand Design §3: "Run Full /
/// Memory Only"). The daemon maps `(target, mode)` onto the bench
/// crate's `StreamOptions` (plan D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BenchMode {
    /// The complete 4×4 grid: all 12 bandwidth cells plus the four
    /// per-tier latency passes.
    Full,
    /// The Memory tier only: its three bandwidth cells plus its latency
    /// pass.
    MemoryOnly,
}

/// A client → daemon RPC request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Request {
    /// Fetch the current telemetry snapshot (served from the daemon's
    /// TTL cache, plan D6; stays servable while a benchmark runs).
    GetTelemetry,
    /// Start a benchmark run: `target` selects the cells, `mode` the
    /// scope (single-flight — a second start while one is active gets
    /// [`Response::Error`], plan D6).
    StartBenchmark { target: StreamTarget, mode: BenchMode },
    /// Cancel the active run `run_id`: the in-flight pass finishes, the
    /// run stops at the next gate (clean cancel, plan D6).
    CancelBenchmark { run_id: u64 },
}

/// A daemon → client RPC response.
///
/// `Telemetry` carries the ~1.2 KiB `SystemMemoryTelemetry` snapshot
/// inline; the `large_enum_variant` allow is deliberate — boxing would
/// change the frozen P3-10 wire-contract shape (and every downstream
/// construction/match site: daemon P3-16, clients P3-18+) and gains
/// nothing, since each `Response` is a transient per-frame value living
/// beside the heap-allocated bincode frame itself (`Vec<u8>`, plan D3).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Response {
    /// The full telemetry snapshot.
    Telemetry(SystemMemoryTelemetry),
    /// The requested run was accepted and started; `run_id` tags its
    /// progress + terminal frames.
    BenchStarted { run_id: u64 },
    /// One streamed progress event for the owning connection (plan D6).
    BenchProgress(StreamProgress),
    /// The terminal result grid for a completed run.
    BenchResult { run_id: u64, grid: BenchmarkGrid },
    /// The terminal ack for a cancelled run (the owner also receives it
    /// as its single terminal frame, plan D6).
    BenchCancelled { run_id: u64 },
    /// A structured error reply — the wire-safe arm of the no-panic
    /// contract.
    Error(String),
}

/// The top-level wire frame payload: one request or one response per
/// frame (the P3-11 codec encodes/decodes exactly one `Message` per
/// u32-LE-length-prefixed Bincode frame, plan D3).
///
/// The same deliberate `large_enum_variant` allow as `Response`: the
/// `Response` arm carries the snapshot transitively, the frame shape is
/// frozen, and the value is transient per frame beside its heap bincode
/// payload.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Message {
    /// Client → daemon.
    Request(Request),
    /// Daemon → client.
    Response(Response),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_bench::{BenchOp, Tier};
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::spd_decode::SpdModule;

    /// A representative snapshot: a `Value` CPU branch, structured-`Na`
    /// vendor branches, one SPD module with mixed `Value`/`Na` cells
    /// (host-independent).
    fn fixture_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: vec![SpdModule {
                index: 0x52,
                is_ddr5: false,
                maker: Section::Value("0xC1".to_owned()),
                part: Section::na(NaReason::NotApplicable),
                serial: Section::na(NaReason::NotApplicable),
                rank: Section::Value(1),
                density_mbit: Section::Value(16_384),
                speed_mts: Section::Value(3_200),
                profiles: Vec::new(),
            }],
        }
    }

    /// (a1) `Request::StartBenchmark` round-trips through bincode.
    #[test]
    fn start_benchmark_request_bincode_round_trip() {
        let req = Request::StartBenchmark {
            target: StreamTarget::Full,
            mode: BenchMode::MemoryOnly,
        };
        let bytes = bincode::serialize(&req).expect("Request must serialize");
        let back: Request = bincode::deserialize(&bytes).expect("Request must deserialize");
        assert_eq!(back, req);
    }

    /// (a2) `Response::Telemetry` round-trips with a representative
    /// snapshot (the P3-06 payload root crosses the wire verbatim).
    #[test]
    fn telemetry_response_bincode_round_trip() {
        let snap = fixture_snapshot();
        let resp = Response::Telemetry(snap.clone());
        let bytes = bincode::serialize(&resp).expect("Response must serialize");
        let back: Response = bincode::deserialize(&bytes).expect("Response must deserialize");
        assert_eq!(back, resp);
        assert_eq!(back, Response::Telemetry(snap));
    }

    /// (a3) Every remaining `Request` / `Response` arm round-trips —
    /// all `StreamTarget` variants, the progress/result payloads, and
    /// the structured `Error` arm.
    #[test]
    fn all_remaining_arms_bincode_round_trip() {
        let requests = vec![
            Request::GetTelemetry,
            Request::StartBenchmark {
                target: StreamTarget::Tier(Tier::L2),
                mode: BenchMode::Full,
            },
            Request::StartBenchmark {
                target: StreamTarget::Cell(Tier::Memory, BenchOp::Read),
                mode: BenchMode::MemoryOnly,
            },
            Request::CancelBenchmark { run_id: 42 },
        ];
        let bytes = bincode::serialize(&requests).expect("requests must serialize");
        let back: Vec<Request> = bincode::deserialize(&bytes).expect("requests must deserialize");
        assert_eq!(back, requests);

        let grid = BenchmarkGrid {
            read_gbps: [512.0, 897.5, 402.0, 198.5],
            write_gbps: [410.0, 823.0, 311.5, 152.0],
            copy_gbps: [455.0, 851.5, 349.0, 176.5],
            latency_ns: [88.0, 1.1, 3.4, 12.7],
        };
        let responses = vec![
            Response::BenchStarted { run_id: 1 },
            Response::BenchProgress(StreamProgress {
                cell_index: 2,
                total_cells: 12,
                tier: Tier::Memory,
                op: BenchOp::Write,
                value: 410.0,
                label: "Memory · Write (GB/s)".to_owned(),
            }),
            Response::BenchResult { run_id: 1, grid },
            Response::BenchCancelled { run_id: 1 },
            Response::Error("benchmark already running".to_owned()),
        ];
        let bytes = bincode::serialize(&responses).expect("responses must serialize");
        let back: Vec<Response> = bincode::deserialize(&bytes).expect("responses must deserialize");
        assert_eq!(back, responses);
    }

    /// (b) The `Message` frame payload round-trips in both directions.
    #[test]
    fn message_wrapper_bincode_round_trip() {
        let req_msg = Message::Request(Request::CancelBenchmark { run_id: 7 });
        let bytes = bincode::serialize(&req_msg).expect("Message must serialize");
        let back: Message = bincode::deserialize(&bytes).expect("Message must deserialize");
        assert_eq!(back, req_msg);

        let resp_msg = Message::Response(Response::BenchStarted { run_id: 3 });
        let bytes = bincode::serialize(&resp_msg).expect("Message must serialize");
        let back: Message = bincode::deserialize(&bytes).expect("Message must deserialize");
        assert_eq!(back, resp_msg);
    }

    /// (c) The socket-path constant is the frozen default.
    #[test]
    fn default_socket_path_is_frozen() {
        assert_eq!(DEFAULT_SOCKET_PATH, "/run/ramsleuth/ramsleuth.sock");
    }
}
