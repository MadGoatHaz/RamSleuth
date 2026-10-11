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
//!
//! **Forward-compatibility contract (C6 / DAEMON-1, enforced):** the wire
//! is versioned by [`crate::PROTOCOL_VERSION`] (the frame header) and the
//! payload layouts are **byte-pinned** (the `TC-DAEMON1` tests below).
//! The payload structs — the telemetry crate's `SystemMemoryTelemetry` /
//! `ProbeReport` / `ProbeRaw` / `SpdModule` and the bench crate's
//! `StreamTarget` / `StreamProgress` / `BenchmarkGrid` / `BurnInTick` —
//! are **bincode-positional**, so their fields serialize in declaration
//! order with no per-field names. The sanctioned evolution rule is
//! **append-only-at-the-tail**: a new field is added at the *end* of a
//! struct (the OQ-10 pattern the enum discriminant pins already follow),
//! never reordered, removed, or re-typed in place. Any such layout change
//! is a *schema change*: it changes the pinned bytes (the `TC-DAEMON1`
//! test fails) and MUST bump [`crate::PROTOCOL_VERSION`] in lockstep, so
//! a peer on a different generation is rejected by
//! [`crate::frame::decode_frame`] instead of silently mis-parsing. A
//! reorder is therefore a version bump — never a silent change.

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
    use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
    use ramsleuth_telemetry::{ProbeRaw, ProbeSystem, SystemPlatform};

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
            vcore: Section::na(NaReason::NotApplicable),
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

    // ------------------------------------------------------------------
    // TC-DAEMON1 (c): payload-layout byte-pinning (the forward-compat
    // anchor). The payload structs are bincode-positional and
    // append-only-at-the-tail; pinning each canonical encoding means any
    // field add / reorder / type change alters the bytes and fails this
    // test — forcing a deliberate `PROTOCOL_VERSION` bump.
    // ------------------------------------------------------------------

    /// Encode a byte slice to a lowercase hex string (for pinning).
    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Canonical `SystemMemoryTelemetry` (host-independent, fixed values).
    fn canonical_system_memory_telemetry() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::UnsupportedHardware),
            spd: vec![canonical_spd_module()],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0)],
            vcore: Section::na(NaReason::NotApplicable),
        }
    }

    /// Canonical `SpdModule` (one DDR4 DIMM, one XMP profile).
    fn canonical_spd_module() -> SpdModule {
        SpdModule {
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
            profiles: vec![SpdProfile {
                index: 1,
                speed_mts: Section::Value(3_200),
                cas: Section::Value(14),
                trcd: Section::Value(14),
                trp: Section::Value(14),
                tras: Section::Value(28),
                voltage: Section::Value(1_200),
            }],
        }
    }

    /// Canonical `StreamProgress` (one completed bandwidth cell).
    fn canonical_stream_progress() -> StreamProgress {
        StreamProgress {
            cell_index: 0,
            total_cells: 12,
            tier: Tier::Memory,
            op: BenchOp::Read,
            value: 512.0,
            label: "Memory \u{b7} Read (GB/s)".to_owned(),
        }
    }

    /// Canonical `BenchmarkGrid` (the 4\u{d7}4 grid, fixed cells).
    fn canonical_benchmark_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [512.0, 897.5, 402.0, 198.5],
            write_gbps: [410.0, 823.0, 311.5, 152.0],
            copy_gbps: [455.0, 851.5, 349.0, 176.5],
            latency_ns: [88.0, 1.1, 3.4, 12.7],
        }
    }

    /// Canonical `BurnInTick` (one completed bandwidth cell).
    fn canonical_burn_in_tick() -> BurnInTick {
        BurnInTick {
            iteration: 1,
            elapsed_secs: 1.5,
            tier: Tier::Memory,
            bandwidth: Some((BenchOp::Read, 512.0)),
            latency_ns: None,
        }
    }

    /// Canonical `ProbeRaw` (a representative Intel subset populated, the
    /// rest at their `Default` containment).
    fn canonical_probe_raw() -> ProbeRaw {
        ProbeRaw {
            mcbios_req: Some(0x0000_0012),
            tc_ch0_dbp: Some(0x1111_0F11),
            tc_ch0_rap: Some(0x2718_0204),
            mad_inter_channel: Some(0x0000_0003),
            mchbar_base: Some(0xFED1_0000),
            mchbar_enabled: true,
            ..Default::default()
        }
    }

    /// Canonical `ProbeReport` (snapshot + raw + system identity).
    fn canonical_probe_report() -> ProbeReport {
        ProbeReport {
            telemetry: canonical_system_memory_telemetry(),
            raw: Some(canonical_probe_raw()),
            system: ProbeSystem {
                cpu_brand: "Ryzen 9 5950X".to_owned(),
                cpu_vendor: "AMD".to_owned(),
                cpu_gen: "Zen 3".to_owned(),
                pci_host_bridge: Some("0x1234".to_owned()),
                kernel: "6.6.0-1-cachyos".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.15".to_owned(),
                telemetry_source: "ryzen_smu".to_owned(),
            },
        }
    }

    /// TC-DAEMON1 (c, generation helper): print each canonical payload's
    /// bincode encoding as hex so the pins below can be (re)generated.
    /// Run with `cargo test -p ramsleuth-protocol tc_daemon1_generate --
    /// --nocapture`. **Not a pass/fail assertion** — it only prints.
    #[test]
    fn tc_daemon1_generate_payload_pins() {
        let cases: [(&str, String); 7] = [
            (
                "SystemMemoryTelemetry",
                to_hex(&bincode::serialize(&canonical_system_memory_telemetry()).unwrap()),
            ),
            (
                "SpdModule",
                to_hex(&bincode::serialize(&canonical_spd_module()).unwrap()),
            ),
            (
                "StreamProgress",
                to_hex(&bincode::serialize(&canonical_stream_progress()).unwrap()),
            ),
            (
                "BenchmarkGrid",
                to_hex(&bincode::serialize(&canonical_benchmark_grid()).unwrap()),
            ),
            (
                "BurnInTick",
                to_hex(&bincode::serialize(&canonical_burn_in_tick()).unwrap()),
            ),
            (
                "ProbeRaw",
                to_hex(&bincode::serialize(&canonical_probe_raw()).unwrap()),
            ),
            (
                "ProbeReport",
                to_hex(&bincode::serialize(&canonical_probe_report()).unwrap()),
            ),
        ];
        for (name, hex) in &cases {
            println!("PIN {name} = \"{hex}\";");
        }
    }

    /// TC-DAEMON1 (c): **byte-pin** each payload struct's canonical
    /// bincode encoding. The payload layouts are bincode-positional and
    /// append-only-at-the-tail; any field add / reorder / type change
    /// alters these bytes and fails this test — forcing a deliberate
    /// `PROTOCOL_VERSION` bump (the forward-compat guarantee, C6).
    ///
    /// To regenerate the pins after a sanctioned schema change: bump
    /// `PROTOCOL_VERSION`, run `cargo test -p ramsleuth-protocol
    /// tc_daemon1_generate -- --nocapture`, and paste the printed hex into
    /// the literals below.
    #[test]
    fn tc_daemon1_payload_layout_byte_pins() {
        let pins: [(&str, &str, &str); 7] = [
            (
                "SystemMemoryTelemetry",
                &to_hex(&bincode::serialize(&canonical_system_memory_telemetry()).unwrap()),
                "00000000020000000d0000000000000052797a656e2039203539353058010000000100000001000000000000000100000000000000520000000000040000000000000030784331000000000800000000000000534b2068796e69780100000004000000000000000801000000040000000100000004000000000000000100000000004000000000800c01000000000000000100000000800c000000000e000000000e000000000e000000001c00000000b00400000000000000000058ab40000000000a000000000000005465737420426f617264000000000300000000000000312e300100000004000000010000000400000000000000000000000000304001000000000000000000000000000000000030400100000004000000",
            ),
            (
                "SpdModule",
                &to_hex(&bincode::serialize(&canonical_spd_module()).unwrap()),
                "520000000000040000000000000030784331000000000800000000000000534b2068796e69780100000004000000000000000801000000040000000100000004000000000000000100000000004000000000800c01000000000000000100000000800c000000000e000000000e000000000e000000001c00000000b004",
            ),
            (
                "StreamProgress",
                &to_hex(&bincode::serialize(&canonical_stream_progress()).unwrap()),
                "000000000c0000000000000000000000000000000000804015000000000000004d656d6f727920c2b72052656164202847422f7329",
            ),
            (
                "BenchmarkGrid",
                &to_hex(&bincode::serialize(&canonical_benchmark_grid()).unwrap()),
                "000000000000804000000000000c8c4000000000002079400000000000d068400000000000a079400000000000b88940000000000078734000000000000063400000000000707c4000000000009c8a400000000000d07540000000000010664000000000000056409a9999999999f13f3333333333330b406666666666662940",
            ),
            (
                "BurnInTick",
                &to_hex(&bincode::serialize(&canonical_burn_in_tick()).unwrap()),
                "01000000000000000000f83f000000000100000000000000000000804000",
            ),
            (
                "ProbeRaw",
                &to_hex(&bincode::serialize(&canonical_probe_raw()).unwrap()),
                "011200000001110f11110104021827000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000103000000000000000000010000d1fe0000000001000000000000000000000000000000",
            ),
            (
                "ProbeReport",
                &to_hex(&bincode::serialize(&canonical_probe_report()).unwrap()),
                "00000000020000000d0000000000000052797a656e2039203539353058010000000100000001000000000000000100000000000000520000000000040000000000000030784331000000000800000000000000534b2068796e69780100000004000000000000000801000000040000000100000004000000000000000100000000004000000000800c01000000000000000100000000800c000000000e000000000e000000000e000000001c00000000b00400000000000000000058ab40000000000a000000000000005465737420426f617264000000000300000000000000312e30010000000400000001000000040000000000000000000000000030400100000000000000000000000000000000003040010000000400000001011200000001110f11110104021827000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000103000000000000000000010000d1fe00000000010000000000000000000000000000000d0000000000000052797a656e20392035393530580300000000000000414d4405000000000000005a656e20330106000000000000003078313233340f00000000000000362e362e302d312d63616368796f7305000000000000006c696e757806000000000000007838365f36340600000000000000322e342e3135090000000000000072797a656e5f736d75",
            ),
        ];
        for (name, actual, pinned) in pins {
            assert_eq!(
                actual,
                pinned,
                "{name} payload layout changed — this is a schema change: bump \
                 PROTOCOL_VERSION (C6 / DAEMON-1) and regenerate the pin"
            );
        }
    }
}
