//! The frozen type shapes for the Intel readout (step 1 of the
//! `intel_readout` directory-module split — moved verbatim from
//! `intel_readout.rs`).
//!
//! Pure data shapes only: the raw IMC register sets the decode consumes
//! and the wire-safe display types it produces (the same frozen shapes
//! the AMD readout shares — [`ClockReadout`], [`TimingSet`], [`CadBus`],
//! [`VoltageSet`]). No decode logic lives here: the register readers
//! ([`IntelImcRegs::from_bar`] / [`IntelImcRegs::from_bar_wide`]), the
//! pure decode core ([`super::decode`]), and the 4-tier channel
//! resolution ([`super::resolve_channel`]) all stay in the parent
//! module.

use crate::amd_readout::{CadBus, ClockReadout, EccStatus, TimingSet, VoltageSet};
use crate::error::Section;

// ---------------------------------------------------------------------------
// Raw register types (the module exposes raw values; [`super`] decodes).
// ---------------------------------------------------------------------------

/// One channel's raw IMC registers (8 registers; `None` when the
/// underlying read failed — per-register containment).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelRegs {
    /// `TC_DBP` (tCL / tCWL / tRCD / tRP).
    pub tc_dbp: Option<u32>,
    /// `TC_RAP` (tRRD_S / tRTP / tCKE / tFAW / tRAS).
    pub tc_rap: Option<u32>,
    /// `TC_RFP` (tRFC / tREFI).
    pub tc_rfp: Option<u32>,
    /// `TC_RAP2` (tRRD_L / tWR).
    pub tc_rap2: Option<u32>,
    /// `TC_RDRD` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_rdrd: Option<u32>,
    /// `TC_RDWR` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_rdwr: Option<u32>,
    /// `TC_WRRD` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_wrrd: Option<u32>,
    /// `TC_WRWR` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_wrwr: Option<u32>,
}

/// The raw registers of one MCL (native uncore memory-controller)
/// block: the eight `TC_*` registers of a Tier-3 (Alder/Raptor) native
/// MCL block (MC0 @ `0xD000`, MC1 @ `0xD800`; register set per the
/// Research Alder/Raptor table, offsets relative to the MCL base).
/// OQ-4: retail desktops mirror MC0/MC1 into the legacy 64 KiB space
/// (0x4000/0x4400/0x4800/0x4C00); the MCL blocks are the native-uncore
/// fallback where the mirror is disabled (per-controller; the doc
/// gives no MCL bases for subchannel 1/3). `None` when the underlying
/// read failed — per-register containment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MclRegs {
    /// `TC_PRE` @ +0x00 (tRCD / tRP / tRAS / tCWL — widened).
    pub tc_pre: Option<u32>,
    /// `TC_ACT` @ +0x04 (tCL / tFAW / tRRD_S / tRRD_L — widened).
    pub tc_act: Option<u32>,
    /// `TC_ACT2` @ +0x08 (tPPD — DDR5-specific).
    pub tc_act2: Option<u32>,
    /// `TC_WTR` @ +0x10 (tWTR_S / tWTR_L / tWR / tRTP — widened).
    pub tc_wtr: Option<u32>,
    /// `TC_RFP` @ +0x14 (tRFC1 12-bit / tREFI).
    pub tc_rfp: Option<u32>,
    /// `TC_RFP2` @ +0x18 (tRFCsb — DDR5-specific).
    pub tc_rfp2: Option<u32>,
    /// `TC_RDRD` @ +0x20 (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_rdrd: Option<u32>,
    /// `TC_WRWR` @ +0x28 (tWRWR sg / dg — the two documented entries).
    pub tc_wrwr: Option<u32>,
}

/// The raw registers of one Alder Lake **DDR4** memory controller's TC
/// block (Phase 1): the nine `TC_*` registers at `MCHBAR + 0xE000` (MC0) /
/// `MCHBAR + 0x1E000` (MC1). `TC_PRE` is a 64-bit register, read as a low
/// word (`tc_pre_lo` @ `+0x00`) and a high word (`tc_pre_hi` @ `+0x04`);
/// every other register is a single 32-bit word. `None` when the
/// underlying read failed — per-register containment (D5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlderDdr4TcRegs {
    /// `TC_PRE` low word @ +0x00 (tRP [7:0] / tRDPRE [19:13] / tPPD [23:20]).
    pub tc_pre_lo: Option<u32>,
    /// `TC_PRE` high word @ +0x04 (tWRPRE [9:0] / tRAS [18:10] /
    /// tRCD [26:19] — the 64-bit positions [41:32] / [50:42] / [58:51]).
    pub tc_pre_hi: Option<u32>,
    /// `TC_ACT` @ +0x08 (tFAW / tRRD_SG / tRRD_DG).
    pub tc_act: Option<u32>,
    /// `TC_RDRD` @ +0x0C (sg / dg / dr / dd turnaround).
    pub tc_rdrd: Option<u32>,
    /// `TC_RDWR` @ +0x10 (sg / dg / dr / dd turnaround).
    pub tc_rdwr: Option<u32>,
    /// `TC_WRRD` @ +0x14 (sg / dg / dr / dd turnaround).
    pub tc_wrrd: Option<u32>,
    /// `TC_WRWR` @ +0x18 (sg / dg / dr / dd turnaround).
    pub tc_wrwr: Option<u32>,
    /// `TC_ODT` @ +0x70 (tCL [22:16] / tCWL [31:24]).
    pub tc_odt: Option<u32>,
    /// `TC_RFP` @ +0x438 (tREFI9 [31:24] / OREF_RI [7:0]).
    pub tc_rfp: Option<u32>,
    /// `TC_RFTP` @ +0x43C (tRFC [30:18] / tREFI [17:0]).
    pub tc_rftp: Option<u32>,
}

/// The full raw IMC register set the decode consumes: one global
/// register, the four Tier-1/Tier-3 channel blocks (`ch0`–`ch3`), the
/// two native MCL blocks (`mcl0` @ `0xD000` / `mcl1` @ `0xD800`, OQ-4),
/// the five `MAD_*` channel/geometry raws (`mad_inter_channel` /
/// `mad_dimm_ch0`–`ch3` — all decode inputs, L-10), and the two Alder
/// Lake DDR4 TC blocks (`alk_d4_tc0` / `alk_d4_tc1`) — 74
/// `Option<u32>` slots in total (`mchbar_base` / `mchbar_enabled` are
/// diagnostics carried by the producer, not decode inputs).
///
/// Every Tier-3 field defaults to `None` (containment, IG-22): no
/// Tier-1/2 producer sets them, so the existing raw-set comparisons
/// and fixture tests stay valid unchanged.
///
/// Producers: `intel_sysfs::acquire()` (the `ramsleuth_intel` kobject)
/// and the `/dev/mem` fallback readers ([`IntelImcRegs::from_bar`] —
/// Tier 1 / Rocket Lake, 64 KiB; [`IntelImcRegs::from_bar_wide`] —
/// the Tier-3 Alder family, 256 KiB; both routed by
/// [`super::read_intel`]). All producers feed the same pure decode core
/// ([`super::decode`] / [`super::read_regs`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IntelImcRegs {
    /// `MC_BIOS_REQ` @ `0x5E00` (the global DRAM clock word).
    pub mcbios_req: Option<u32>,
    /// Channel 0 (`0x4000` block).
    pub ch0: ChannelRegs,
    /// Channel 1 (`0x4400` block).
    pub ch1: ChannelRegs,
    /// Channel 2 (`0x4800` mirror block — Tier-3 subchannel 2).
    pub ch2: ChannelRegs,
    /// Channel 3 (`0x4C00` mirror block — Tier-3 subchannel 3).
    pub ch3: ChannelRegs,
    /// MC0 MCL block raws (`0xD000` — Tier-3 native-uncore fallback,
    /// OQ-4).
    pub mcl0: MclRegs,
    /// MC1 MCL block raws (`0xD800` — Tier-3 native-uncore fallback,
    /// OQ-4).
    pub mcl1: MclRegs,
    /// `MAD_INTER_CHANNEL` (the global channel-mode / DDR-type word —
    /// decode input, L-10: the base is gen-keyed, so the `/dev/mem`
    /// producer sets it; the sysfs producer reads the `mad_inter_channel`
    /// attribute).
    pub mad_inter_channel: Option<u32>,
    /// `MAD_DIMM_CH0` (channel 0 DIMM capacity — decode input, L-10).
    pub mad_dimm_ch0: Option<u32>,
    /// `MAD_DIMM_CH1` (channel 1 DIMM capacity — decode input, L-10).
    pub mad_dimm_ch1: Option<u32>,
    /// `MAD_DIMM_CH2` @ `0x5060` (Alder/DDR5 subchannel 2 geometry,
    /// raw).
    pub mad_dimm_ch2: Option<u32>,
    /// `MAD_DIMM_CH3` @ `0x5064` (Alder/DDR5 subchannel 3 geometry,
    /// raw).
    pub mad_dimm_ch3: Option<u32>,
    /// MC0 Alder Lake DDR4 TC block raws (`0xE000` — Phase 1; the 64-bit
    /// `TC_PRE` is split into `tc_pre_lo` / `tc_pre_hi`). Default (all
    /// `None`) on the 64 KiB `from_bar` set and on module builds that do
    /// not expose the `0xE000+` range.
    pub alk_d4_tc0: AlderDdr4TcRegs,
    /// MC1 Alder Lake DDR4 TC block raws (`0x1E000` = `0xE000 + 0x10000`
    /// — Phase 1). See [`alk_d4_tc0`].
    pub alk_d4_tc1: AlderDdr4TcRegs,
}

/// Which DDR4 turnaround register a [`super::alk_d4_turnaround`]
/// extraction targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhichAlkD4Turnaround {
    Rdrd,
    Rdwr,
    Wrrd,
    Wrwr,
}

// ---------------------------------------------------------------------------
// Frozen display types (shape unchanged — wire-safe, bincode round-trips
// stay valid; only which cells carry `Value` vs `Na` changes).
// ---------------------------------------------------------------------------

/// One decoded memory channel of an Intel platform.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelChannel {
    /// 0-based channel index (`index < channel_count(gen)`).
    pub index: u8,
    /// Clocks and ratios: in the live decode `mclk` comes from
    /// `MC_BIOS_REQ`; the Rocket Lake profile additionally decodes
    /// `gear_mode` + `uclk_mhz` from `MC_BIOS_REQ[17:16]`, while on
    /// Tier 1 (and for the AMD-fabric slots) those cells are
    /// `Na(NotApplicable)`. The legacy compat path may populate the
    /// gear / uclk cells.
    pub clocks: ClockReadout,
    /// DRAM subtimings in integer DRAM clock cycles: 24 populated /
    /// derived slots + 3 structural `Na(NotApplicable)` in the live
    /// Tier-1 decode (the full map in the module docs).
    pub timings: TimingSet,
    /// CAD bus: every field `Na(NotApplicable)` on Intel.
    pub cad_bus: CadBus,
    /// Memory rails: every field `Na(NotApplicable)` on Intel.
    pub voltages: VoltageSet,
    /// RTL (ticks): `Na(NotApplicable)` in the live Tier-1 decode — the
    /// legacy skeleton's "RTL" register was fiction; the legacy compat
    /// path may carry a `Value`.
    pub rtl: Section<u16>,
}

/// The hardware channel / interleave mode.
///
/// **Chunk 2 correction:** the mode is *not* a 2-bit field of
/// `MAD_INTER_CHANNEL` — bits `[2:0]` of that register are the `DDR_TYPE`
/// (the physical memory technology, see [`MemoryTech`]), not a mode. The
/// true operating mode is derived from the *capacity* mapped to each
/// channel: [`super::channel_mode_from_capacity`] computes it from the
/// two `MAD_DIMM_CH0/1` register sums (there is no `CH_L_SIZE` field in
/// `MAD_INTER_CHANNEL`). A Flex-Mode box (asymmetric DIMMs, e.g. 16 + 8 GB)
/// may bind only a single SPD yet run in Dual-Flex, so the SPD-count label
/// is authoritative only as a frontend fallback.
///
/// Wire-safe (serde + bincode). The mode is population-conditional, and the
/// Skylake-family firmware is not reliable in maintaining it, so the decode
/// cross-checks it against the `MAD_DIMM_CH0/1` population **and** the
/// SMBIOS Type 17 topology ([`super::resolve_channel`], the 4-tier trust
/// procedure — the L3 safety override is the core fix for the
/// misreported-single-channel bug).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelMode {
    /// Dual-channel, symmetric interleaving (both channels equal capacity).
    DualSymmetric,
    /// Dual-channel, flex mode (asymmetric DIMMs).
    DualFlex,
    /// Single-channel.
    Single,
}

/// The physical DRAM technology the IMC is running (decoded from
/// `MAD_INTER_CHANNEL[2:0]` — the `DDR_TYPE` field, **not** a channel mode
/// — or, when MMIO access is blocked, from the SMBIOS Type 17 memory-type
/// field).
///
/// Wire-safe (serde + bincode). [`MemoryTech::Unknown`] is the honest
/// fallback for a reserved / unrecognizable encoding; [`None`] (on the
/// [`IntelReadout`]) means no source carried a technology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MemoryTech {
    /// DDR3 / DDR3L (`001b`).
    Ddr3,
    /// DDR4 (`000b`).
    Ddr4,
    /// LPDDR4 (`010b`).
    Lpddr4,
    /// LPDDR5 (`011b`).
    Lpddr5,
    /// DDR5 (`100b`).
    Ddr5,
    /// A reserved / unrecognizable encoding.
    Unknown,
}

/// The trust tier of a resolved channel mode (the 4-tier decision
/// procedure, research "Recommended Multi-Tier Decision Procedure").
///
/// Wire-safe (serde + bincode). [`ChannelTrust::None`] (the variant,
/// carried as `Option<ChannelTrust>` on [`IntelReadout`]) is the terminal
/// "unresolvable" state — both MMIO and SMBIOS yielded nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelTrust {
    /// L1 — the MMIO decode is readable, internally consistent, and matches
    /// SMBIOS. The hardware read is the primary authority.
    VerifiedHardware,
    /// L2 — MMIO is inaccessible; the topology is verified from SMBIOS
    /// Type 17 alone.
    VerifiedFirmware,
    /// L3 — SAFETY OVERRIDE: MMIO decoded single-channel but SMBIOS
    /// confirms two DIMMs across channels; the MMIO read is marked
    /// untrusted and the result is reported dual-channel.
    Override,
    /// L4 — MMIO is inaccessible and the SMBIOS locators are ambiguous; a
    /// dual-channel heuristic is applied (the "unverified" annotation).
    Heuristic,
    /// Terminal — neither MMIO nor SMBIOS identified the topology.
    None,
}

/// The resolved channel topology: the trust-resolved mode, the active
/// memory technology, the trust tier, a human annotation (the
/// "untrusted-MMIO mismatch" / "heuristic" note), and the SMBIOS-confirmed
/// populated DIMM count. Wire-safe fields (see [`IntelReadout`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelResolution {
    /// The trust-resolved channel mode (`None` for the terminal state).
    pub mode: Option<ChannelMode>,
    /// The active memory technology (MMIO `DDR_TYPE` else SMBIOS).
    pub tech: Option<MemoryTech>,
    /// The trust tier (`None` for the terminal state).
    pub trust: Option<ChannelTrust>,
    /// A human annotation (empty for the clean L1 cases).
    pub annotation: String,
    /// The SMBIOS-confirmed populated DIMM count.
    pub dimm_count: u8,
}

/// The full Intel readout: one [`IntelChannel`] per detected channel.
///
/// Wire-safe (serde + bincode); the struct shape is append-only (OQ-10
/// pattern) — the three original field positions are frozen, and the
/// Chunk 2 fields (`memory_tech` / `channel_trust` / `channel_annotation`
/// / `dimm_count`) are appended after `ecc_status`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelReadout {
    /// Decoded channels (0-based indices, `channels.len() == channel_count`).
    pub channels: Vec<IntelChannel>,
    /// The trust-resolved channel mode ([`super::resolve_channel`], the
    /// 4-tier procedure): the capacity-derived MMIO mode cross-checked
    /// against the SMBIOS Type 17 topology. `None` on the terminal /
    /// all-Na degradation.
    pub channel_mode: Option<ChannelMode>,
    /// ECC state (the CAPID0_A bit-17 decode). OQ-10 wire append.
    pub ecc_status: EccStatus,
    /// The active memory technology (Chunk 2): the MMIO `DDR_TYPE`
    /// (`MAD_INTER_CHANNEL[2:0]`) else the SMBIOS Type 17 memory type.
    /// `None` when no source carried a technology.
    pub memory_tech: Option<MemoryTech>,
    /// The trust tier of the resolved channel mode (Chunk 2): `L1`
    /// verified-hardware, `L2` verified-firmware, `L3` override, `L4`
    /// heuristic, or `None` (terminal). `None` (the `Option` outer) on the
    /// all-Na degradation.
    pub channel_trust: Option<ChannelTrust>,
    /// A human annotation for the resolution (the "untrusted-MMIO
    /// mismatch" / "heuristic" note); empty for the clean L1 cases.
    pub channel_annotation: String,
    /// The SMBIOS-confirmed populated DIMM count.
    pub dimm_count: u8,
}
