//! Wire RPC messages — the frozen message contract (P3-10).
//!
//! One [`Message`] per frame, per direction: a client sends
//! [`Message::Request`], the daemon replies [`Message::Response`] (plus
//! streamed progress frames to the owning benchmark connection, plan D6).
//!
//! **Payloads are reused, never duplicated (plan D2):** [`Request`] /
//! [`Response`] embed the telemetry crate's `SystemMemoryTelemetry` and
//! the bench crate's `StreamTarget` / `StreamProgress` /
//! `BenchmarkGrid` / `BurnInTick` verbatim — the single source of truth
//! for each is its owning crate. This module owns only the four wire
//! enums and the socket-path constant.
//!
//! **No-panic contract:** every arm is bincode-serializable (plan D3);
//! failures cross the wire as structured payloads — [`Response::Error`]
//! and the `Na(reason)` sections inside a snapshot — never as panics.

use ramsleuth_bench::{BenchmarkGrid, BurnInTick, StreamProgress, StreamTarget};
use ramsleuth_telemetry::{ProbeReport, SystemMemoryTelemetry};

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
    /// Start a burn-in run (D-1): `target` selects the cells every pass
    /// runs, `duration_minutes` the duration — `0` means *infinite*
    /// (stop only via [`Request::CancelBenchmark`]), `n > 0` stops once
    /// a pass has completed and the run elapsed is `n × 60` seconds. A
    /// burn-in is a distinct run class (a multi-pass duration run, not
    /// the single-pass [`Request::StartBenchmark`]), so it gets its own
    /// arm: `StartBenchmark` / `BenchMode` stay byte-frozen.
    /// Single-flight with `StartBenchmark` (a second start while any
    /// run is active gets [`Response::Error`], plan D6); its progress
    /// streams on [`Response::BurnInProgress`].
    StartBurnIn { target: StreamTarget, duration_minutes: u32 },
    /// Fetch the consent-gated "Submit Probe Report": the full telemetry
    /// snapshot + the Intel raw-register dump (when available) + the
    /// system identity — the [`Response::ProbeReport`] payload (the
    /// telemetry crate's `ProbeReport`, chunk probe-1a). The daemon-side
    /// builder that fills it is chunk 1b; this arm is append-only (OQ-10:
    /// it lands at the next discriminant after `StartBurnIn`, so every
    /// pre-existing variant's wire encoding is unchanged).
    GetProbeReport,
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
    /// One streamed burn-in tick for the owning `StartBurnIn`
    /// connection (D-2): the bench crate's [`BurnInTick`] verbatim —
    /// one per completed bandwidth cell and per completed per-tier
    /// latency pass of each burn-in iteration. The normal-bench
    /// [`Response::BenchProgress`] arm stays byte-identical.
    BurnInProgress(BurnInTick),
    /// The consent-gated "Submit Probe Report" payload (the
    /// [`Request::GetProbeReport`] reply): the full snapshot + the Intel
    /// raw-register dump (when available) + the system identity — the
    /// telemetry crate's `ProbeReport` (chunk probe-1a). Append-only
    /// (OQ-10: it lands at the next discriminant after `BurnInProgress`,
    /// so every pre-existing variant's wire encoding is unchanged).
    ProbeReport(ProbeReport),
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
    use ramsleuth_telemetry::SystemPlatform;

    /// A representative snapshot: a `Value` CPU branch, structured-`Na`
    /// vendor branches, one SPD module with mixed `Value`/`Na` cells, a
    /// mixed `Value`/`Na` platform, and per-DIMM capacity parallel to `spd`
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
                die_maker: Section::Value("SK hynix".to_owned()),
                die_type: Section::na(NaReason::NotApplicable),
                devices: Section::Value(8),
                part: Section::na(NaReason::NotApplicable),
                serial: Section::na(NaReason::NotApplicable),
                rank: Section::Value(1),
                density_mbit: Section::Value(16_384),
                speed_mts: Section::Value(3_200),
                profiles: Vec::new(),
            }],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            // 16_384 Mbit x 8 devices / 8192 = 16 GiB per DIMM, parallel
            // to `spd`; total = the sum of the `Value` entries.
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
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

    /// (a1b) `Request::StartBurnIn` round-trips through bincode (D-1:
    /// the appended wire arm — both the infinite (`0`) and a finite
    /// duration, over the full target range).
    #[test]
    fn start_burn_in_request_bincode_round_trip() {
        for target in [
            StreamTarget::Full,
            StreamTarget::Tier(Tier::L2),
            StreamTarget::Cell(Tier::Memory, BenchOp::Write),
        ] {
            for duration_minutes in [0u32, 1, 60, u32::MAX] {
                let req = Request::StartBurnIn {
                    target,
                    duration_minutes,
                };
                let bytes = bincode::serialize(&req).expect("Request must serialize");
                let back: Request =
                    bincode::deserialize(&bytes).expect("Request must deserialize");
                assert_eq!(back, req);
            }
        }
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
            Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 0,
            },
            Request::StartBurnIn {
                target: StreamTarget::Tier(Tier::L3),
                duration_minutes: 30,
            },
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
            Response::BurnInProgress(BurnInTick {
                iteration: 3,
                elapsed_secs: 42.5,
                tier: Tier::Memory,
                bandwidth: Some((BenchOp::Read, 512.0)),
                latency_ns: None,
            }),
            Response::BurnInProgress(BurnInTick {
                iteration: 3,
                elapsed_secs: 42.9,
                tier: Tier::L1,
                bandwidth: None,
                latency_ns: Some(1.1),
            }),
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

    /// (d) OQ-10 wire pinning: appending `Request::GetProbeReport` and
    /// `Response::ProbeReport` must not shift any pre-existing variant's
    /// bincode discriminant. bincode 1.x encodes an enum's leading u32
    /// discriminant little-endian, so each pre-existing variant keeps its
    /// original leading bytes and the appended arms land at the next
    /// discriminant only (the OQ-10 append rule, byte-for-byte).
    #[test]
    fn request_response_bincode_byte_pinning_oq10() {
        use ramsleuth_telemetry::ProbeSystem;

        // Pin the leading 4 bytes (the u32-LE discriminant) of a payload.
        let lead = |v: &[u8], disc: u32, name: &str| {
            assert_eq!(
                &v[..4],
                &disc.to_le_bytes()[..],
                "{name} discriminant must be unchanged (OQ-10)"
            );
        };

        // --- Request: pre-existing discriminants unchanged. ---------
        // The unit variant serializes to exactly its u32 discriminant.
        assert_eq!(
            bincode::serialize(&Request::GetTelemetry).unwrap(),
            0u32.to_le_bytes().to_vec(),
            "Request::GetTelemetry wire encoding must be unchanged (OQ-10)"
        );
        lead(
            &bincode::serialize(&Request::StartBenchmark {
                target: StreamTarget::Full,
                mode: BenchMode::Full,
            })
            .unwrap(),
            1,
            "Request::StartBenchmark",
        );
        lead(
            &bincode::serialize(&Request::CancelBenchmark { run_id: 1 }).unwrap(),
            2,
            "Request::CancelBenchmark",
        );
        lead(
            &bincode::serialize(&Request::StartBurnIn {
                target: StreamTarget::Full,
                duration_minutes: 0,
            })
            .unwrap(),
            3,
            "Request::StartBurnIn",
        );
        // The appended arm lands at the next discriminant (unit variant).
        assert_eq!(
            bincode::serialize(&Request::GetProbeReport).unwrap(),
            4u32.to_le_bytes().to_vec(),
            "Request::GetProbeReport must land at discriminant 4 (OQ-10 append)"
        );

        // --- Response: pre-existing discriminants unchanged. --------
        let resp_lead = |r: &Response, disc: u32, name: &str| {
            lead(&bincode::serialize(r).unwrap(), disc, name);
        };
        resp_lead(&Response::Telemetry(fixture_snapshot()), 0, "Response::Telemetry");
        resp_lead(&Response::BenchStarted { run_id: 1 }, 1, "Response::BenchStarted");
        resp_lead(
            &Response::BenchProgress(StreamProgress {
                cell_index: 0,
                total_cells: 0,
                tier: Tier::L1,
                op: BenchOp::Read,
                value: 0.0,
                label: String::new(),
            }),
            2,
            "Response::BenchProgress",
        );
        resp_lead(
            &Response::BenchResult {
                run_id: 1,
                grid: BenchmarkGrid {
                    read_gbps: [0.0; 4],
                    write_gbps: [0.0; 4],
                    copy_gbps: [0.0; 4],
                    latency_ns: [0.0; 4],
                },
            },
            3,
            "Response::BenchResult",
        );
        resp_lead(&Response::BenchCancelled { run_id: 1 }, 4, "Response::BenchCancelled");
        resp_lead(&Response::Error(String::new()), 5, "Response::Error");
        resp_lead(
            &Response::BurnInProgress(BurnInTick {
                iteration: 0,
                elapsed_secs: 0.0,
                tier: Tier::L1,
                bandwidth: None,
                latency_ns: None,
            }),
            6,
            "Response::BurnInProgress",
        );
        // The appended arm lands at the next discriminant.
        let minimal_report = ProbeReport {
            telemetry: fixture_snapshot(),
            raw: None,
            system: ProbeSystem {
                cpu_brand: String::new(),
                cpu_vendor: String::new(),
                cpu_gen: String::new(),
                pci_host_bridge: None,
                kernel: String::new(),
                os: String::new(),
                arch: String::new(),
                ramsleuth_version: String::new(),
                telemetry_source: String::new(),
            },
        };
        lead(
            &bincode::serialize(&Response::ProbeReport(minimal_report)).unwrap(),
            7,
            "Response::ProbeReport",
        );
    }
}
