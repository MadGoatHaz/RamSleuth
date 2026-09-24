//! Probe-report wire types — the consent-gated "Submit Probe Report"
//! payload (chunk-probe-1a, CRITICAL-PATH foundation).
//!
//! This module defines the three serde wire structs that ride
//! `Request::GetProbeReport` / `Response::ProbeReport` (append-only arms
//! in the `ramsleuth-protocol` `Request` / `Response` enums, the OQ-10
//! append pattern):
//!
//! - [`ProbeReport`] — the top-level report: the full
//!   [`SystemMemoryTelemetry`] snapshot, the optional Intel raw-register
//!   dump ([`ProbeRaw`]), and the system identity ([`ProbeSystem`]).
//! - [`ProbeRaw`] — a serde-friendly **flattened** representation of the
//!   Intel raw IMC registers: the 51 `Option<u32>` slots of
//!   `IntelImcRegs` re-laid flat (the 4 per-channel `TC_*` blocks, the 2
//!   native `MCL_*` blocks, the global `MC_BIOS_REQ`, the 7 global MAD
//!   registers) plus the MCHBAR diagnostics. It is a **flat, stable wire
//!   format** (no nesting that would break the bincode round-trip) — the
//!   existing `IntelImcRegs` / `MclRegs` / `ChannelRegs` deliberately keep
//!   no serde derives and are NOT changed.
//! - [`ProbeSystem`] — the operator-facing system identity: CPU brand /
//!   vendor / generation, the PCI host-bridge id (when known), kernel /
//!   OS / arch, the RamSleuth version, and the telemetry source.
//!
//! **This chunk defines the wire shapes only.** The daemon-side builder
//! (chunk 1b) populates these from the live snapshot + raw sources; the
//! frontends (chunks 3/4) render them.
//!
//! **No-panic contract (D5):** all three structs are plain data — no I/O,
//! no panics on construction. Every field is serde-serializable so the
//! whole report crosses the wire verbatim (plan D3).

use crate::SystemMemoryTelemetry;

/// The top-level probe report — the `Response::ProbeReport` payload.
///
/// - `telemetry`: the full [`SystemMemoryTelemetry`] snapshot (the daemon
///   `facade::collect()` output, verbatim).
/// - `raw`: the optional Intel raw-register dump ([`ProbeRaw`]) —
///   populated only when an Intel raw source (the `ramsleuth_intel`
///   kobject or the `/dev/mem` MCHBAR fallback) yields data; `None` on
///   AMD / unknown silicon or when both raw sources are unavailable.
/// - `system`: the operator-facing system identity ([`ProbeSystem`]).
///
/// Derives `PartialEq` (not `Eq` — the embedded snapshot carries `f64`
/// cells) so the round-trip test can assert equality.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProbeReport {
    /// The full system memory telemetry snapshot.
    pub telemetry: SystemMemoryTelemetry,
    /// The Intel raw-register dump (flattened), when available.
    pub raw: Option<ProbeRaw>,
    /// The operator-facing system identity.
    pub system: ProbeSystem,
}

/// A serde-friendly **flattened** representation of the Intel raw IMC
/// registers — the flat, stable wire form of the 51-slot `IntelImcRegs`
/// plus the 7 global MAD registers plus the MCHBAR diagnostics.
///
/// The existing `IntelImcRegs` / `MclRegs` / `ChannelRegs` deliberately
/// carry **no** serde derives (they are decode inputs, not wire types);
/// this struct re-lays them flat so the raw set crosses the wire as a
/// stable, nesting-free shape. bincode serializes a struct's fields in
/// declaration order — **the field order below is the wire contract**.
///
/// Every register field is `Option<u32>` (per-register containment:
/// `None` when the underlying read failed / was absent). The MCHBAR
/// diagnostics: `mchbar_base` is `Option<u64>` (the physical base, `None`
/// when absent / malformed) and `mchbar_enabled` is a plain `bool`
/// (`false` when absent / malformed).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeRaw {
    // --- global --------------------------------------------------------
    /// `MC_BIOS_REQ` @ `0x5E00` (the global DRAM clock word).
    pub mcbios_req: Option<u32>,
    // --- channel 0 (`0x4000` block) ------------------------------------
    pub tc_ch0_dbp: Option<u32>,
    pub tc_ch0_rap: Option<u32>,
    pub tc_ch0_rfp: Option<u32>,
    pub tc_ch0_rap2: Option<u32>,
    pub tc_ch0_rdrd: Option<u32>,
    pub tc_ch0_rdwr: Option<u32>,
    pub tc_ch0_wrrd: Option<u32>,
    pub tc_ch0_wrwr: Option<u32>,
    // --- channel 1 (`0x4400` block) ------------------------------------
    pub tc_ch1_dbp: Option<u32>,
    pub tc_ch1_rap: Option<u32>,
    pub tc_ch1_rfp: Option<u32>,
    pub tc_ch1_rap2: Option<u32>,
    pub tc_ch1_rdrd: Option<u32>,
    pub tc_ch1_rdwr: Option<u32>,
    pub tc_ch1_wrrd: Option<u32>,
    pub tc_ch1_wrwr: Option<u32>,
    // --- channel 2 (`0x4800` mirror block — Tier-3 subchannel 2) -------
    pub tc_ch2_dbp: Option<u32>,
    pub tc_ch2_rap: Option<u32>,
    pub tc_ch2_rfp: Option<u32>,
    pub tc_ch2_rap2: Option<u32>,
    pub tc_ch2_rdrd: Option<u32>,
    pub tc_ch2_rdwr: Option<u32>,
    pub tc_ch2_wrrd: Option<u32>,
    pub tc_ch2_wrwr: Option<u32>,
    // --- channel 3 (`0x4C00` mirror block — Tier-3 subchannel 3) -------
    pub tc_ch3_dbp: Option<u32>,
    pub tc_ch3_rap: Option<u32>,
    pub tc_ch3_rfp: Option<u32>,
    pub tc_ch3_rap2: Option<u32>,
    pub tc_ch3_rdrd: Option<u32>,
    pub tc_ch3_rdwr: Option<u32>,
    pub tc_ch3_wrrd: Option<u32>,
    pub tc_ch3_wrwr: Option<u32>,
    // --- MC0 MCL block (`0xD000` — Tier-3 native-uncore fallback) ------
    pub mcl0_pre: Option<u32>,
    pub mcl0_act: Option<u32>,
    pub mcl0_act2: Option<u32>,
    pub mcl0_wtr: Option<u32>,
    pub mcl0_rfp: Option<u32>,
    pub mcl0_rfp2: Option<u32>,
    pub mcl0_rdrd: Option<u32>,
    pub mcl0_wrwr: Option<u32>,
    // --- MC1 MCL block (`0xD800` — Tier-3 native-uncore fallback) ------
    pub mcl1_pre: Option<u32>,
    pub mcl1_act: Option<u32>,
    pub mcl1_act2: Option<u32>,
    pub mcl1_wtr: Option<u32>,
    pub mcl1_rfp: Option<u32>,
    pub mcl1_rfp2: Option<u32>,
    pub mcl1_rdrd: Option<u32>,
    pub mcl1_wrwr: Option<u32>,
    // --- global MAD channel/geometry (24-attr module builds) -----------
    pub mad_inter_channel: Option<u32>,
    pub mad_intra_ch0: Option<u32>,
    pub mad_intra_ch1: Option<u32>,
    pub mad_dimm_ch0: Option<u32>,
    pub mad_dimm_ch1: Option<u32>,
    pub mad_dimm_ch2: Option<u32>,
    pub mad_dimm_ch3: Option<u32>,
    // --- MCHBAR diagnostics --------------------------------------------
    /// MCHBAR physical base (`None` when absent / malformed).
    pub mchbar_base: Option<u64>,
    /// MCHBAR enable bit (`false` when absent / malformed).
    pub mchbar_enabled: bool,
}

/// The operator-facing system identity for a probe report: the CPU
/// identity, the PCI host-bridge id (when known), the OS / kernel /
/// arch, the RamSleuth version, and the telemetry source that produced
/// the report.
///
/// All fields are plain strings (or `Option<String>` for the host
/// bridge, which is not resolvable on every platform) so the shape is
/// stable across RamSleuth versions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeSystem {
    /// CPU brand string (e.g. "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz").
    pub cpu_brand: String,
    /// CPU vendor (e.g. "Intel", "AMD", "unknown").
    pub cpu_vendor: String,
    /// CPU generation (e.g. "RocketLake", "Zen3", "Unknown").
    pub cpu_gen: String,
    /// PCI host-bridge id (`vendor:device`), when known.
    pub pci_host_bridge: Option<String>,
    /// Kernel release string (e.g. "6.6.0-1-cachyos").
    pub kernel: String,
    /// OS name / distro (e.g. "Linux / Arch").
    pub os: String,
    /// CPU architecture (e.g. "x86_64").
    pub arch: String,
    /// The RamSleuth version that produced this report (e.g. "2.4.0").
    pub ramsleuth_version: String,
    /// The telemetry source (e.g. "ramsleuth_intel", "ryzen_smu", "devmem").
    pub telemetry_source: String,
}

// ---------------------------------------------------------------------------
// Tests (chunk-probe-1a: the wire-shape pins).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::{AmdZen, CpuInfo, CpuVendor, IntelGen};
    use crate::error::{NaReason, Section};
    use crate::platform::SystemPlatform;
    use crate::spd_decode::SpdModule;

    /// A fully-populated [`ProbeRaw`] fixture (representative Intel raw
    /// dump: populated ch0/ch1/ch2 + mcl0/mcl1 + all MAD raws, absent
    /// ch3, MCHBAR base + enable — host-independent).
    fn fixture_raw() -> ProbeRaw {
        ProbeRaw {
            mcbios_req: Some(0x0000_0012),
            tc_ch0_dbp: Some(0x1111_0F11),
            tc_ch0_rap: Some(0x2718_0204),
            tc_ch0_rfp: Some(0x0000_01A4),
            tc_ch0_rap2: Some(0x0000_0C0A),
            tc_ch0_rdrd: Some(0x0048_C286),
            tc_ch0_rdwr: Some(0x0000_0280),
            tc_ch0_wrrd: Some(0x0000_0308),
            tc_ch0_wrwr: Some(0x0040_C204),
            tc_ch1_dbp: Some(0x1111_0F11),
            tc_ch1_rap: Some(0x2718_0204),
            tc_ch1_rfp: Some(0x0000_01A4),
            tc_ch1_rap2: Some(0x0000_0C0A),
            tc_ch1_rdrd: Some(0x0048_C286),
            tc_ch1_rdwr: Some(0x0000_0280),
            tc_ch1_wrrd: Some(0x0000_0308),
            tc_ch1_wrwr: Some(0x0040_C204),
            tc_ch2_dbp: Some(0x2222_1F22),
            tc_ch2_rap: Some(0x3333_1111),
            tc_ch2_rfp: Some(0x0000_01B4),
            tc_ch2_rap2: Some(0x0000_0C1A),
            tc_ch2_rdrd: Some(0x0048_C287),
            tc_ch2_rdwr: Some(0x0000_0284),
            tc_ch2_wrrd: Some(0x0000_0318),
            tc_ch2_wrwr: Some(0x0040_C214),
            // ch3 (the Alder subchannel-3 mirror) is absent here.
            tc_ch3_dbp: None,
            tc_ch3_rap: None,
            tc_ch3_rfp: None,
            tc_ch3_rap2: None,
            tc_ch3_rdrd: None,
            tc_ch3_rdwr: None,
            tc_ch3_wrrd: None,
            tc_ch3_wrwr: None,
            mcl0_pre: Some(0x0028_8410),
            mcl0_act: Some(0x1228_4D28),
            mcl0_act2: Some(0x0000_0008),
            mcl0_wtr: Some(0x0828_0E0C),
            mcl0_rfp: Some(0x0000_04B0),
            mcl0_rfp2: Some(0x0000_0087),
            mcl0_rdrd: Some(0x0048_C286),
            mcl0_wrwr: Some(0x0000_0286),
            mcl1_pre: Some(0x0028_8410),
            mcl1_act: Some(0x1228_4D28),
            mcl1_act2: Some(0x0000_0008),
            mcl1_wtr: Some(0x0828_0E0C),
            mcl1_rfp: Some(0x0000_04B0),
            mcl1_rfp2: Some(0x0000_0087),
            mcl1_rdrd: Some(0x0048_C286),
            mcl1_wrwr: Some(0x0000_0286),
            mad_inter_channel: Some(0x0000_0003),
            mad_intra_ch0: Some(0x0000_0005),
            mad_intra_ch1: Some(0x0000_0007),
            mad_dimm_ch0: Some(0x0000_0008),
            mad_dimm_ch1: Some(0x0000_000C),
            mad_dimm_ch2: Some(0x0000_0010),
            mad_dimm_ch3: Some(0x0000_0014),
            mchbar_base: Some(0xFED1_0000),
            mchbar_enabled: true,
        }
    }

    /// The [`ProbeSystem`] fixture (host-independent strings).
    fn fixture_system() -> ProbeSystem {
        ProbeSystem {
            cpu_brand: "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz".to_owned(),
            cpu_vendor: "Intel".to_owned(),
            cpu_gen: "RocketLake".to_owned(),
            pci_host_bridge: Some("8086:4250".to_owned()),
            kernel: "6.6.0-1-cachyos".to_owned(),
            os: "Linux / Arch".to_owned(),
            arch: "x86_64".to_owned(),
            ramsleuth_version: "2.4.0".to_owned(),
            telemetry_source: "ramsleuth_intel".to_owned(),
        }
    }

    /// A representative [`SystemMemoryTelemetry`] fixture (an Intel
    /// snapshot, `Na` vendor branches, one SPD module, mixed platform —
    /// host-independent).
    fn fixture_telemetry() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Intel(IntelGen::RocketLake),
                brand: "Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz".to_owned(),
            },
            amd: Section::na(NaReason::UnsupportedHardware),
            intel: Section::na(NaReason::DriverMissing),
            spd: vec![SpdModule {
                index: 0x52,
                is_ddr5: false,
                maker: Section::Value("0xC1".to_owned()),
                die_maker: Section::na(NaReason::NotApplicable),
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

    /// (1) A fully-populated [`ProbeReport`] (raw `Some` + system)
    /// round-trips through bincode and compares equal — every field of
    /// the report crosses the wire (the wire-shape pin).
    #[test]
    fn probe_report_bincode_round_trip_populated() {
        let report = ProbeReport {
            telemetry: fixture_telemetry(),
            raw: Some(fixture_raw()),
            system: fixture_system(),
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    /// (2) A [`ProbeReport`] with `raw: None` (the AMD / unknown-silicon
    /// arm) round-trips identically — the `Option<ProbeRaw>` arm crosses
    /// the wire in both states.
    #[test]
    fn probe_report_bincode_round_trip_no_raw() {
        let report = ProbeReport {
            telemetry: SystemMemoryTelemetry {
                cpu: CpuInfo {
                    vendor: CpuVendor::Amd(AmdZen::Zen3),
                    brand: "AMD Ryzen 9 5950X".to_owned(),
                },
                amd: Section::na(NaReason::DriverMissing),
                intel: Section::na(NaReason::UnsupportedHardware),
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
            },
            raw: None,
            system: ProbeSystem {
                cpu_brand: "AMD Ryzen 9 5950X".to_owned(),
                cpu_vendor: "AMD".to_owned(),
                cpu_gen: "Zen3".to_owned(),
                pci_host_bridge: None,
                kernel: "6.6.0-1-cachyos".to_owned(),
                os: "Linux / Arch".to_owned(),
                arch: "x86_64".to_owned(),
                ramsleuth_version: "2.4.0".to_owned(),
                telemetry_source: "ryzen_smu".to_owned(),
            },
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }

    /// (3) The live `collect()` snapshot, wrapped in a [`ProbeReport`]
    /// with a `None` raw, bincode-serializes, round-trips, and compares
    /// equal — the whole report (snapshot + identity) is wire-safe on
    /// this host (no-panic contract).
    #[test]
    fn probe_report_live_telemetry_round_trip() {
        let report = ProbeReport {
            telemetry: crate::collect(),
            raw: None,
            system: ProbeSystem {
                cpu_brand: "live".to_owned(),
                cpu_vendor: "live".to_owned(),
                cpu_gen: "live".to_owned(),
                pci_host_bridge: None,
                kernel: "live".to_owned(),
                os: "live".to_owned(),
                arch: "live".to_owned(),
                ramsleuth_version: "2.4.0".to_owned(),
                telemetry_source: "live".to_owned(),
            },
        };
        let bytes =
            bincode::serialize(&report).expect("ProbeReport must serialize (no-panic contract)");
        let back: ProbeReport =
            bincode::deserialize(&bytes).expect("ProbeReport must deserialize");
        assert_eq!(report, back);
    }
}
