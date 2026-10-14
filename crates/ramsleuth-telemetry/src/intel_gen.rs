//! Per-generation Intel MCHBAR decode profiles (IG-01, CRITICAL-PATH).
//!
//! Pure data + dispatch: maps a detected [`IntelGen`] to a static
//! [`GenProfile`] (datasheet window size, MCHBAR alignment mask, channel
//! count, memory-gear capability, register-map family). No I/O, no
//! unsafe, no CPUID — the module is unit-tested in isolation and is the
//! foundation every later generational-expansion chunk dispatches on.
//!
//! IG-01 lands the Tier-1 (Skylake–Comet Lake) and Rocket Lake profiles
//! (Ice Lake / Tiger Lake join the shared Tier-1 profile — H-1); IG-03
//! adds the Tier-3 `Alder` family entries (Alder/Raptor Lake at the
//! DDR4-default 2-channel count, Meteor/Arrow Lake at the DDR5
//! 2-channel count — C-4: both are dual-channel, not 4-channel). The
//! The client Haswell / Broadwell `IntelGen` variants (L-8) resolve to
//! the shared Tier-1 profile (the same 64 KiB register map). The
//! server-uncore `GenMap::Server` line (Haswell-E / Broadwell-E /
//! Skylake-X / Cascade Lake / Sapphire Rapids — H-9) carries the
//! no-MMIO profiles (4 / 6 / 8 channels). IG-20 adds the
//! Alder/Raptor Lake (Tier 3) register table: the four DDR5 subchannel
//! descriptors ([`ALDER_CHANNELS`]) with their legacy mirror and
//! uncore-MCL bases, and the widened DDR5 timing bitfield constants
//! ([`ALDER_FIELDS`]).
//!
//! Phase 1 (Intel MC fix) adds the **Alder/Raptor Lake DDR4** register
//! map: the two per-controller channel descriptors ([`ALDER_DDR4_CHANNELS`])
//! whose TC register blocks live at `MCHBAR + 0xE000` / `0x1E000` (NOT the
//! legacy `0x4000` mirror window Tier 1 uses), and the DDR4 timing
//! bitfield constants ([`ALDER_DDR4_FIELDS`]). DDR4 Alder Lake is 2 memory
//! controllers × 1 channel each (no subchannels), distinct from both the
//! Tier-1 (Skylake) and the DDR5 ([`ALDER_FIELDS`]) layouts.
//!
//! The canonical hardware constants (MCHBAR window sizes, uncore-MCL
//! bases, Alder Lake DDR4 TC bases) are declared in this module as the
//! single source of truth (M-2) so the consumers (intel_readout.rs /
//! intel_mchbar.rs) import them rather than re-declaring independent
//! copies.
//!
//! The `mchbar_mask` values here are **diagnostic-only** for the
//! dispatcher (OQ-8): they document the alignment contract each profile
//! decodes against and do NOT alter the merged kernel module's ioremap
//! mask — `kernel/ramsleuth-intel/` is untouched by this module.
//!
//! Register references: Research lines 15–17, 107, 111–132, 329,
//! Breakdown §2 (`plans/PLAN-INTEL-GENERIC.md`).

use crate::cpuid::IntelGen;

// ---------------------------------------------------------------------
// Canonical hardware constants (single source of truth — M-2)
// ---------------------------------------------------------------------
//
// The shared MCHBAR window sizes, uncore-MCL bases, and Alder Lake DDR4
// TC bases. Declared here (intel_gen.rs) so the consumers (intel_readout.rs
// / intel_mchbar.rs) import them instead of re-declaring independent
// copies. All are `pub` (an additive change); the consumer-side `use`
// statements are a follow-up task.

/// MCHBAR window size for Tier-1 (64 KiB).
pub const WINDOW_TIER1: u32 = 0x10000;
/// MCHBAR window size for Tier-3/Alder (256 KiB).
pub const WINDOW_ALDER: u32 = 0x40000;
/// MCL0 base offset (Tier 3 subchannel 0 / MC0).
pub const MCL0_BASE: u32 = 0xD000;
/// MCL1 base offset (Tier 3 subchannel 2 / MC1).
pub const MCL1_BASE: u32 = 0xD800;
/// ALK DDR4 MC0 TC base.
pub const ALK_D4_TC0_BASE: u32 = 0xE000;
/// ALK DDR4 MC1 TC base.
pub const ALK_D4_TC1_BASE: u32 = 0x1E000;

/// Memory-gear capability of a generation's MCHBAR decode set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GearCap {
    /// No gear register (Tier 1).
    None,
    /// 2×-gear register set (Rocket Lake).
    Gear2,
    /// 4×-gear register set (Alder Lake and later DDR5 generations).
    Gear4,
}

/// The register-map family a generation's MCHBAR offsets belong to.
///
/// The families the generational expansion knows about. IG-01 populates
/// `Tier1` and `Rocket`; IG-03 populates `Alder`. Pre-Skylake families
/// (Sandy Bridge, Haswell/Broadwell) are out of scope — the project
/// supports Skylake (6th) through Arrow Lake plus server platforms — so
/// they carry no `GenMap` entry (M-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenMap {
    /// Skylake–Comet Lake (64 KiB window, no gear register).
    Tier1,
    /// Rocket Lake (64 KiB window, 2× gear).
    Rocket,
    /// Alder/Raptor/Meteor/Arrow Lake (256 KiB window, 4× gear) — IG-03.
    Alder,
    /// Server uncore (Haswell-E / Broadwell-E / Skylake-X / Cascade
    /// Lake / Sapphire Rapids): no OS-accessible MC timing registers —
    /// the decode sources SMBIOS Type 17 + the SPD EEPROM and never
    /// reads the MCHBAR MMIO window.
    Server,
}

/// A static per-generation MCHBAR decode profile.
///
/// All fields are compile-time constants; a profile is resolved by value
/// through [`profile_for`] and never mutated at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenProfile {
    /// Minimum mapped window size covering this profile's register table
    /// (Tier 1/Rocket: 64 KiB; the Alder family: 256 KiB). NOT a claim
    /// about the actual hardware BAR size (which may be larger). The
    /// runtime mapper in `intel_mchbar.rs` uses its own mask.
    pub window_size: usize,
    /// The MCHBAR base-address alignment mask (diagnostic-only, OQ-8).
    pub mchbar_mask: u64,
    /// The fixed channel count for the generation.
    pub channel_count: u8,
    /// The memory-gear capability of the profile's decode set.
    pub gear: GearCap,
    /// The register-map family the profile belongs to.
    pub map: GenMap,
}

/// The shared Tier-1 profile: Skylake, Kaby Lake, Coffee Lake, Comet
/// Lake, Ice Lake, Tiger Lake (H-1: the 10th/11th-gen derivatives reuse
/// the same 64 KiB register map — Breakdown §2: diagnostic-only mask,
/// 2 channels, no gear register).
const TIER1: GenProfile = GenProfile {
    window_size: WINDOW_TIER1 as usize,
    mchbar_mask: 0x0000_007F_FFFF_0000,
    channel_count: 2,
    gear: GearCap::None,
    map: GenMap::Tier1,
};

/// The Rocket Lake profile: the Tier-1 64 KiB window with the 2×-gear
/// register set (Breakdown §2; Research lines 15–17).
const ROCKET: GenProfile = GenProfile {
    window_size: WINDOW_TIER1 as usize,
    mchbar_mask: 0x0000_007F_FFFF_0000,
    channel_count: 2,
    gear: GearCap::Gear2,
    map: GenMap::Rocket,
};

/// The Alder/Raptor Lake profile: the 256 KiB window with the 4×-gear
/// register set (Breakdown §2; Research line 19). `channel_count` = 2
/// documents the DDR4 default — OQ-11's DDR5 detection (IG-26) flips the
/// decode-time count to 4; the profile field itself is untouched.
const ALDER_2CH: GenProfile = GenProfile {
    window_size: WINDOW_ALDER as usize,
    mchbar_mask: 0x0000_007F_FFFC_0000,
    channel_count: 2,
    gear: GearCap::Gear4,
    map: GenMap::Alder,
};

/// The Meteor/Arrow Lake profile: the 256 KiB window with the 4×-gear
/// register set and the **dual-channel** DDR5 count (C-4: both Arrow
/// Lake-S and Meteor Lake are dual-channel DDR5 — TechPowerUp: 102.4
/// GB/s = 2×64-bit×6400 for Arrow Lake, 89.6 GB/s = 2×64-bit×5600 for
/// Meteor Lake; Breakdown §2; Research lines 20–21; the tile-routing
/// caveat OQ-5 is handled in IG-28).
///
/// Meteor Lake has a mixed memory topology (2× DDR5 off-package +
/// LPDDR5X on-package); this profile models the off-package DDR5
/// portion only — the LPDDR5X on-package channels are not modeled.
const ALDER_DDR5: GenProfile = GenProfile {
    window_size: WINDOW_ALDER as usize,
    mchbar_mask: 0x0000_007F_FFFC_0000,
    channel_count: 2,
    gear: GearCap::Gear4,
    map: GenMap::Alder,
};

/// The server-uncore profile (Haswell-E / Broadwell-E / Skylake-X):
/// quad-channel DDR4. `mchbar_mask` is `0` — there is **no** valid
/// OS-accessible MCHBAR: the memory-controller timing registers live in
/// CPU-internal uncore space that is not PCI-BAR-mapped and is unreachable
/// via `/dev/mem` (the 16 MiB MCH-window sweep on the x99 reference found
/// only a sparse page-0 cluster). The decode for this profile never reads
/// the MMIO window: all memory telemetry sources from SMBIOS Type 17
/// (topology / capacity / technology) + the SPD EEPROM (rated timings /
/// speed). `window_size` is a page multiple kept only for the `mmap`
/// contract (the width is irrelevant — no MMIO read happens).
const SERVER_4CH: GenProfile = GenProfile {
    window_size: 0x10000,
    mchbar_mask: 0,
    channel_count: 4,
    gear: GearCap::None,
    map: GenMap::Server,
};

/// The server-uncore profile (Cascade Lake, Xeon Scalable 1st gen):
/// hex-channel DDR4 (the 6-channel count). Same no-MMIO contract as
/// [`SERVER_4CH`].
const SERVER_6CH: GenProfile = GenProfile {
    window_size: 0x10000,
    mchbar_mask: 0,
    channel_count: 6,
    gear: GearCap::None,
    map: GenMap::Server,
};

/// The server-uncore profile (Sapphire Rapids, Xeon Scalable 4th gen):
/// 8-channel DDR5 (H-9). Same no-MMIO contract as [`SERVER_4CH`] (no
/// OS-accessible MCHBAR — the decode sources SMBIOS + SPD, never the
/// MCHBAR MMIO window).
const SERVER_8CH: GenProfile = GenProfile {
    window_size: 0x40000,
    mchbar_mask: 0,
    channel_count: 8,
    gear: GearCap::None,
    map: GenMap::Server,
};

/// Resolve the static decode profile for a generation.
///
/// Returns the shared Tier-1 profile for the Tier-1 generations
/// (Skylake / Kaby Lake / Coffee Lake / Comet Lake, plus Ice Lake /
/// Tiger Lake — H-1: the same 64 KiB register map, 2 channels, no
/// gear), the Rocket Lake profile for [`IntelGen::RocketLake`], the
/// 2-channel Alder profile for [`IntelGen::AlderLake`] /
/// [`IntelGen::RaptorLake`] and the 2-channel DDR5 profile for
/// [`IntelGen::MeteorLake`] / [`IntelGen::ArrowLake`] (IG-03, C-4);
/// the server-uncore profiles (4 / 6 / 8 channels) for the Xeon / HEDT
/// line; every other generation (Jasper Lake, Lunar Lake, Core Series
/// 1 Mobile, and [`IntelGen::Unrecognized`]) resolves to `None` until
/// its family gains a profile entry in a later chunk.
pub fn profile_for(gen: IntelGen) -> Option<&'static GenProfile> {
    match gen {
        // The client Haswell / Broadwell (L-8) share the identical 64 KiB
        // Tier-1 register map — the same profile as the rest of the
        // pre-8th / 6th-gen line.
        IntelGen::Skylake
        | IntelGen::KabyLake
        | IntelGen::CoffeeLake
        | IntelGen::CometLake
        | IntelGen::IceLake
        | IntelGen::TigerLake
        | IntelGen::Haswell
        | IntelGen::Broadwell => Some(&TIER1),
        IntelGen::RocketLake => Some(&ROCKET),
        IntelGen::AlderLake | IntelGen::RaptorLake => Some(&ALDER_2CH),
        IntelGen::MeteorLake | IntelGen::ArrowLake => Some(&ALDER_DDR5),
        // Server uncore: no OS-accessible MC timing registers — the decode
        // sources SMBIOS + SPD, never the MCHBAR MMIO window.
        IntelGen::HaswellE | IntelGen::BroadwellE | IntelGen::SkylakeX => Some(&SERVER_4CH),
        IntelGen::CascadeLake => Some(&SERVER_6CH),
        IntelGen::SapphireRapids => Some(&SERVER_8CH),
        // Lunar Lake (Core Ultra 200V) is on-package LPDDR5X only (no off-package DIMMs).
        // It cannot reuse any existing profile (no DDR5 DIMM channel map applies).
        // Deliberately out of scope for current Intel telemetry support.
        // If added in the future, requires a dedicated on-package-memory profile.
        IntelGen::LunarLake => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------
// IG-20: the Alder/Raptor Lake (Tier 3) register table
// ---------------------------------------------------------------------

/// An Alder/Raptor Lake (Tier 3) channel descriptor (IG-20).
///
/// DDR5 mode runs four 32-bit subchannels: MC0 hosts subchannels 0 and
/// 1, MC1 hosts subchannels 2 and 3 (Research line 107). Retail desktop
/// boards mirror all four into the legacy 64 KiB window at 0x4000 /
/// 0x4400 / 0x4800 / 0x4C00; some mobile/OEM boards disable the
/// mirror, exposing the timings through the native uncore MCL blocks at
/// 0xD000 (MC0) and 0xD800 (MC1) (Research line 329). The MCL bases
/// are documented **per controller**, not per subchannel: subchannels
/// 1 and 3 therefore carry no MCL base (`mcl_base: None`) and are
/// mirror-only — the 0xD400/0xDC00 analogs are not in the research
/// doc and must not be hard-coded (plan OQ-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlderChannelMap {
    /// The legacy-compatibility mirror base, present on every Tier 3
    /// subchannel (Research lines 107, 329).
    pub mirror_base: u32,
    /// The native uncore MCL fallback base: `Some(0xD000)` for MC0
    /// subchannel 0, `Some(0xD800)` for MC1 subchannel 2, and
    /// `None` for subchannels 1 and 3 (no documented base — OQ-4).
    pub mcl_base: Option<u32>,
    /// All four Tier 3 DDR5 channels are 32-bit subchannels (Research
    /// line 107).
    pub is_subch: bool,
}

/// MC0 subchannel 0: mirror 0x4000, MCL 0xD000 (Research lines 107,
/// 329).
pub const ALDER_MC0_SUBCH0: AlderChannelMap = AlderChannelMap {
    mirror_base: 0x4000,
    mcl_base: Some(MCL0_BASE),
    is_subch: true,
};

/// MC0 subchannel 1: mirror 0x4400, no documented MCL base (OQ-4 —
/// mirror-only; Research lines 107, 329).
pub const ALDER_MC0_SUBCH1: AlderChannelMap = AlderChannelMap {
    mirror_base: 0x4400,
    mcl_base: None,
    is_subch: true,
};

/// MC1 subchannel 2: mirror 0x4800, MCL 0xD800 (Research lines 107,
/// 329).
pub const ALDER_MC1_SUBCH2: AlderChannelMap = AlderChannelMap {
    mirror_base: 0x4800,
    mcl_base: Some(MCL1_BASE),
    is_subch: true,
};

/// MC1 subchannel 3: mirror 0x4C00, no documented MCL base (OQ-4 —
/// mirror-only; Research lines 107, 329).
pub const ALDER_MC1_SUBCH3: AlderChannelMap = AlderChannelMap {
    mirror_base: 0x4C00,
    mcl_base: None,
    is_subch: true,
};

/// The four Alder/Raptor Lake (Tier 3) DDR5 subchannel descriptors in
/// subchannel order (Research lines 107, 329).
pub static ALDER_CHANNELS: &[AlderChannelMap] = &[
    ALDER_MC0_SUBCH0,
    ALDER_MC0_SUBCH1,
    ALDER_MC1_SUBCH2,
    ALDER_MC1_SUBCH3,
];

/// An inclusive `(hi, lo)` bit range (MSB index first, LSB index
/// second) of a single timing field inside its per-controller TC
/// register.
pub type BitRange = (u32, u32);

/// The widened Alder/Raptor Lake (Tier 3, DDR5) timing bitfields (IG-20).
///
/// Transcribed verbatim from the Research Alder/Raptor table (lines
/// 111–132, header line 109), which applies to MC0 / subchannel 0 and
/// MC1 / subchannel 2 (Research line 107). Each field is the inclusive
/// `(hi, lo)` bit range inside its TC register:
///
/// - `TC_PRE`  @ +0x00 — tRCD, tRP, tRAS, tCWL (lines 112–115)
/// - `TC_ACT`  @ +0x04 — tCL, tFAW, tRRD_S, tRRD_L (lines 111, 116–118)
/// - `TC_ACT2` @ +0x08 — tPPD (line 125)
/// - `TC_WTR`  @ +0x10 — tWTR_S, tWTR_L, tWR, tRTP (lines 119–122)
/// - `TC_RFP`  @ +0x14 — tRFC1, tREFI (lines 123, 126)
/// - `TC_RFP2` @ +0x18 — tRFCsb (line 124)
/// - `TC_RDRD` @ +0x20 — the tRDRD sg/dg/dr/dd quartet (lines 127–130)
/// - `TC_WRWR` @ +0x28 — tWRWR sg/dg (lines 131–132); only those two
///   entries are documented for Alder — do not extrapolate the Tier-1
///   _dr/_dd rows
///
/// The widenings vs. Tier 1: tCL/tRAS/tWR from 6 to 8 bits, tRCD/tRP
/// from 6 to 7 bits, tRFC1 to 12 bits, plus the DDR5-specific tRFCsb
/// and tPPD fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlderFields {
    /// tRCD [14:8] — TC_PRE +0x00 (line 112).
    pub trcd: BitRange,
    /// tRP [6:0] — TC_PRE +0x00 (line 113).
    pub trp: BitRange,
    /// tRAS [23:16] — TC_PRE +0x00 (line 114).
    pub tras: BitRange,
    /// tCWL [30:24] — TC_PRE +0x00 (line 115).
    pub tcwl: BitRange,
    /// tCL [23:16] — TC_ACT +0x04 (line 111).
    pub tcl: BitRange,
    /// tFAW [7:0] — TC_ACT +0x04 (line 116).
    pub tfaw: BitRange,
    /// tRRD_S [11:8] — TC_ACT +0x04 (line 117).
    pub trrd_s: BitRange,
    /// tRRD_L [15:12] — TC_ACT +0x04 (line 118).
    pub trrd_l: BitRange,
    /// tPPD [3:0] — TC_ACT2 +0x08 (line 125), DDR5-specific.
    pub tppd: BitRange,
    /// tWTR_S [6:0] — TC_WTR +0x10 (line 119).
    pub twtr_s: BitRange,
    /// tWTR_L [14:8] — TC_WTR +0x10 (line 120).
    pub twtr_l: BitRange,
    /// tWR [23:16] — TC_WTR +0x10 (line 121).
    pub twr: BitRange,
    /// tRTP [30:24] — TC_WTR +0x10 (line 122).
    pub trtp: BitRange,
    /// tRFC1 [11:0] — TC_RFP +0x14 (line 123).
    pub trfc1: BitRange,
    /// tREFI [31:16] — TC_RFP +0x14 (line 126); multiplier, not DRAM
    /// clocks.
    pub trefi: BitRange,
    /// tRFCsb [10:0] — TC_RFP2 +0x18 (line 124), DDR5-specific.
    pub trfcsb: BitRange,
    /// tRDRD_sg [5:0] — TC_RDRD +0x20 (line 127).
    pub trdrd_sg: BitRange,
    /// tRDRD_dg [11:6] — TC_RDRD +0x20 (line 128).
    pub trdrd_dg: BitRange,
    /// tRDRD_dr [17:12] — TC_RDRD +0x20 (line 129).
    pub trdrd_dr: BitRange,
    /// tRDRD_dd [23:18] — TC_RDRD +0x20 (line 130).
    pub trdrd_dd: BitRange,
    /// tWRWR_sg [5:0] — TC_WRWR +0x28 (line 131).
    pub twrwr_sg: BitRange,
    /// tWRWR_dg [11:6] — TC_WRWR +0x28 (line 132).
    pub twrwr_dg: BitRange,
}

/// The single widened Alder/Raptor Lake (Tier 3, DDR5) timing field
/// table (IG-20), per Research lines 111–132.
pub const ALDER_FIELDS: AlderFields = AlderFields {
    trcd: (14, 8),
    trp: (6, 0),
    tras: (23, 16),
    tcwl: (30, 24),
    tcl: (23, 16),
    tfaw: (7, 0),
    trrd_s: (11, 8),
    trrd_l: (15, 12),
    tppd: (3, 0),
    twtr_s: (6, 0),
    twtr_l: (14, 8),
    twr: (23, 16),
    trtp: (30, 24),
    trfc1: (11, 0),
    trefi: (31, 16),
    trfcsb: (10, 0),
    trdrd_sg: (5, 0),
    trdrd_dg: (11, 6),
    trdrd_dr: (17, 12),
    trdrd_dd: (23, 18),
    twrwr_sg: (5, 0),
    twrwr_dg: (11, 6),
};

// ---------------------------------------------------------------------
// Phase 1 (Intel MC fix): the Alder/Raptor Lake DDR4 register map
// ---------------------------------------------------------------------

/// The Alder/Raptor Lake **DDR4** timing bitfields (Phase 1a).
///
/// Transcribed from the Intel 12th Gen Core Datasheet Vol 2
/// (doc 710723-003), §3.2.66–3.2.87. DDR4 Alder Lake is 2 memory
/// controllers × 1 channel each (no subchannels). The TC register block
/// is at `MCHBAR + 0xE000` (MC0) / `MCHBAR + 0x1E000` (MC1) — **not** the
/// legacy `0x4000` mirror window the Tier-1 map reads (which returns
/// `0xFFFFFFFF` on DDR4 Alder Lake).
///
/// `TC_PRE` is a **64-bit** register: its `[58:51]`-class fields (tRCD /
/// tRAS / tWRPRE) exceed a single 32-bit word, so the decode reads it as
/// two 32-bit words — a low word (`[31:0]`, `+0x00`) and a high word
/// (`[63:32]`, `+0x04`). The table records the full 64-bit `[hi:lo]`
/// positions; a field is in the high word when `lo >= 32` (word-relative
/// position `[hi-32 : lo-32]`), else in the low word at `[hi:lo]`.
/// Every other `TC_*` register here is a plain 32-bit word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlderDdr4Fields {
    // `TC_PRE` @ +0xE000 (64-bit; high word @ +0x04, low word @ +0x00).
    /// tRCD — ACT→CAS same bank, [58:51] (high word [26:19]).
    pub trcd: BitRange,
    /// tRAS — ACT→PRE same bank, [50:42] (high word [18:10]).
    pub tras: BitRange,
    /// tWRPRE — WR→PRE same bank, [41:32] (high word [9:0]).
    pub twrpre: BitRange,
    /// tPPD — PRE→PRE same rank, [23:20] (low word).
    pub tppd: BitRange,
    /// tRDPRE — RD→PRE same bank, [19:13] (low word).
    pub trdpre: BitRange,
    /// tRP — PRE→ACT same bank, [7:0] (low word).
    pub trp: BitRange,
    // `TC_ACT` @ +0xE008 (32-bit).
    /// tRRD_DG — ACT→ACT diff bank group, [21:15].
    pub trrd_dg: BitRange,
    /// tRRD_SG — ACT→ACT same bank group (tRRD_L for DDR4), [14:9].
    pub trrd_sg: BitRange,
    /// tFAW — four-activate window, [8:0].
    pub tfaw: BitRange,
    // `TC_RDRD` @ +0xE00C (32-bit).
    /// tRDRD_DD — RD→RD other DIMM, [31:24].
    pub rdrd_dd: BitRange,
    /// tRDRD_DR — RD→RD other rank, [23:16].
    pub rdrd_dr: BitRange,
    /// tRDRD_DG — RD→RD diff bank group, [14:8].
    pub rdrd_dg: BitRange,
    /// tRDRD_SG — RD→RD same bank group, [6:0].
    pub rdrd_sg: BitRange,
    // `TC_RDWR` @ +0xE010 (32-bit).
    /// tRDWR_DD — RD→WR other DIMM, [31:24].
    pub rdwr_dd: BitRange,
    /// tRDWR_DR — RD→WR other rank, [23:16].
    pub rdwr_dr: BitRange,
    /// tRDWR_DG — RD→WR diff bank group, [15:8].
    pub rdwr_dg: BitRange,
    /// tRDWR_SG — RD→WR same bank group, [7:0].
    pub rdwr_sg: BitRange,
    // `TC_WRRD` @ +0xE014 (32-bit).
    /// tWRRD_DD — WR→RD other DIMM, [31:25].
    pub wrrd_dd: BitRange,
    /// tWRRD_DR — WR→RD other rank, [24:18].
    pub wrrd_dr: BitRange,
    /// tWRRD_DG — WR→RD diff bank group, [17:9].
    pub wrrd_dg: BitRange,
    /// tWRRD_SG — WR→RD same bank group, [8:0].
    pub wrrd_sg: BitRange,
    // `TC_WRWR` @ +0xE018 (32-bit).
    /// tWRWR_DD — WR→WR other DIMM, [31:24].
    pub wrwr_dd: BitRange,
    /// tWRWR_DR — WR→WR other rank, [22:16].
    pub wrwr_dr: BitRange,
    /// tWRWR_DG — WR→WR diff bank group, [14:8].
    pub wrwr_dg: BitRange,
    /// tWRWR_SG — WR→WR same bank group, [6:0].
    pub wrwr_sg: BitRange,
    // `TC_ODT` @ +0xE070 (32-bit).
    /// tCWL — WR command→data, [31:24].
    pub tcwl: BitRange,
    /// tCL — RD command→data, [22:16].
    pub tcl: BitRange,
    // `TC_RFP` @ +0xE438 (32-bit).
    /// tREFI×9 (1024-DCLK units), [31:24].
    pub trefi9: BitRange,
    /// OREF_RI — rank-idle period (DCLK), [7:0].
    pub oref_ri: BitRange,
    // `TC_RFTP` @ +0xE43C (32-bit).
    /// tRFC — refresh time, [30:18].
    pub trfc: BitRange,
    /// tREFI — average refresh interval (tCK), [17:0].
    pub trefi: BitRange,
}

/// The Alder/Raptor Lake **DDR4** timing field table (Phase 1a), per the
/// Intel 12th Gen Core Datasheet Vol 2 §3.2.66–3.2.87.
pub const ALDER_DDR4_FIELDS: AlderDdr4Fields = AlderDdr4Fields {
    // TC_PRE @ +0xE000 (64-bit).
    trcd: (58, 51),
    tras: (50, 42),
    twrpre: (41, 32),
    tppd: (23, 20),
    trdpre: (19, 13),
    trp: (7, 0),
    // TC_ACT @ +0xE008 (32-bit).
    trrd_dg: (21, 15),
    trrd_sg: (14, 9),
    tfaw: (8, 0),
    // TC_RDRD @ +0xE00C (32-bit).
    rdrd_dd: (31, 24),
    rdrd_dr: (23, 16),
    rdrd_dg: (14, 8),
    rdrd_sg: (6, 0),
    // TC_RDWR @ +0xE010 (32-bit).
    rdwr_dd: (31, 24),
    rdwr_dr: (23, 16),
    rdwr_dg: (15, 8),
    rdwr_sg: (7, 0),
    // TC_WRRD @ +0xE014 (32-bit).
    wrrd_dd: (31, 25),
    wrrd_dr: (24, 18),
    wrrd_dg: (17, 9),
    wrrd_sg: (8, 0),
    // TC_WRWR @ +0xE018 (32-bit).
    wrwr_dd: (31, 24),
    wrwr_dr: (22, 16),
    wrwr_dg: (14, 8),
    wrwr_sg: (6, 0),
    // TC_ODT @ +0xE070 (32-bit).
    tcwl: (31, 24),
    tcl: (22, 16),
    // TC_RFP @ +0xE438 (32-bit).
    trefi9: (31, 24),
    oref_ri: (7, 0),
    // TC_RFTP @ +0xE43C (32-bit).
    trfc: (30, 18),
    trefi: (17, 0),
};

/// An Alder/Raptor Lake **DDR4** channel descriptor (Phase 1b): one 64-bit
/// channel per memory controller (2 MCs total, no subchannels).
///
/// The TC register block is at `tc_base` from MCHBAR; the MCL register
/// block at `mcl_base`. MC1 is MC0's block offset by the inter-controller
/// stride `0x10000` (the DDR4 TC block: `0xE000 + 0x10000 = 0x1E000`; the
/// MCL block: `0xD000 + 0x800 = 0xD800`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlderDdr4ChannelMap {
    /// The TC register block base offset from MCHBAR (MC0 @ `0xE000`,
    /// MC1 @ `0x1E000`).
    pub tc_base: u32,
    /// The MCL register block base offset from MCHBAR (MC0 @ `0xD000`,
    /// MC1 @ `0xD800`).
    pub mcl_base: u32,
}

/// MC0 DDR4 channel: TC block @ `0xE000`, MCL block @ `0xD000`.
pub const ALDER_DDR4_MC0: AlderDdr4ChannelMap = AlderDdr4ChannelMap {
    tc_base: 0xE000,
    mcl_base: 0xD000,
};

/// MC1 DDR4 channel: TC block @ `0x1E000` (`0xE000 + 0x10000`), MCL block
/// @ `0xD800`.
pub const ALDER_DDR4_MC1: AlderDdr4ChannelMap = AlderDdr4ChannelMap {
    tc_base: 0x1E000,
    mcl_base: 0xD800,
};

/// The two Alder/Raptor Lake **DDR4** channel descriptors in controller
/// order (MC0, MC1) — 2 channels, no subchannels (Phase 1b).
pub static ALDER_DDR4_CHANNELS: &[AlderDdr4ChannelMap] = &[ALDER_DDR4_MC0, ALDER_DDR4_MC1];

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tier-1 generations (the original four plus Ice Lake /
    /// Tiger Lake — H-1, and the client Haswell / Broadwell — L-8) all
    /// resolve to the shared Tier-1 profile with the exact IG-01 values
    /// (64 KiB window, diagnostic mask, 2 channels, no gear, Tier-1 map).
    #[test]
    fn tier1_generations_resolve_to_shared_profile() {
        for gen in [
            IntelGen::Skylake,
            IntelGen::KabyLake,
            IntelGen::CoffeeLake,
            IntelGen::CometLake,
            IntelGen::IceLake,
            IntelGen::TigerLake,
            IntelGen::Haswell,
            IntelGen::Broadwell,
        ] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the Tier-1 profile"));
            assert_eq!(p, &TIER1, "{gen:?}");
            assert_eq!(p.window_size, 0x10000, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0x0000_007F_FFFF_0000, "{gen:?}");
            assert_eq!(p.channel_count, 2, "{gen:?}");
            assert_eq!(p.gear, GearCap::None, "{gen:?}");
            assert_eq!(p.map, GenMap::Tier1, "{gen:?}");
        }
    }

    /// Rocket Lake resolves to its own profile: the same 64 KiB window and
    /// diagnostic mask as Tier 1, but the 2×-gear register set and the
    /// Rocket map.
    #[test]
    fn rocket_lake_resolves_to_rocket_profile() {
        let p = profile_for(IntelGen::RocketLake)
            .unwrap_or_else(|| panic!("RocketLake must resolve to the Rocket profile"));
        assert_eq!(p, &ROCKET);
        assert_eq!(p.window_size, 0x10000);
        assert_eq!(p.mchbar_mask, 0x0000_007F_FFFF_0000);
        assert_eq!(p.channel_count, 2);
        assert_eq!(p.gear, GearCap::Gear2);
        assert_eq!(p.map, GenMap::Rocket);
    }

    /// Non-profiled generations resolve to `None`: `Unrecognized`
    /// (Ice Lake / Tiger Lake now carry the shared Tier-1 profile,
    /// H-1; the remaining appended variants have their own test below).
    #[test]
    fn non_profiled_generations_resolve_to_none() {
        assert_eq!(profile_for(IntelGen::Unrecognized), None);
    }

    /// The chunk-1 appended variants (Jasper Lake, Lunar Lake, Core
    /// Series 1 Mobile) carry no decode profile → honest `N/A`
    /// (research "desktop impact nil" for the mobile / Atom collapse).
    /// Sapphire Rapids carries the 8-channel server-uncore profile
    /// (H-9) and is covered by the server test below.
    #[test]
    fn non_profiled_appended_variants_resolve_to_none() {
        assert_eq!(profile_for(IntelGen::JasperLake), None);
        assert_eq!(profile_for(IntelGen::LunarLake), None);
        assert_eq!(profile_for(IntelGen::CoreSeries1Mobile), None);
    }

    /// The server-uncore generations resolve to the `Server` profile:
    /// Haswell-E / Broadwell-E / Skylake-X at the quad-channel count,
    /// Cascade Lake at the hex-channel count, and Sapphire Rapids at the
    /// 8-channel count (H-9) — all with `mchbar_mask = 0` (no
    /// OS-accessible MCHBAR), no gear register, and the `Server` map.
    #[test]
    fn server_generations_resolve_to_server_profile() {
        for gen in [IntelGen::HaswellE, IntelGen::BroadwellE, IntelGen::SkylakeX] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the Server profile"));
            assert_eq!(p, &SERVER_4CH, "{gen:?}");
            assert_eq!(p.map, GenMap::Server, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0, "{gen:?} (no OS-accessible MCHBAR)");
            assert_eq!(p.channel_count, 4, "{gen:?} (quad-channel)");
            assert_eq!(p.gear, GearCap::None, "{gen:?} (DDR4, no gear register)");
        }
        let p = profile_for(IntelGen::CascadeLake)
            .unwrap_or_else(|| panic!("CascadeLake must resolve to the Server profile"));
        assert_eq!(p, &SERVER_6CH, "Cascade Lake");
        assert_eq!(p.map, GenMap::Server, "Cascade Lake");
        assert_eq!(p.channel_count, 6, "Cascade Lake is hex-channel");
        assert_eq!(p.mchbar_mask, 0, "Cascade Lake (no OS-accessible MCHBAR)");
        let p = profile_for(IntelGen::SapphireRapids)
            .unwrap_or_else(|| panic!("SapphireRapids must resolve to the Server profile (H-9)"));
        assert_eq!(p, &SERVER_8CH, "Sapphire Rapids");
        assert_eq!(p.map, GenMap::Server, "Sapphire Rapids");
        assert_eq!(p.channel_count, 8, "Sapphire Rapids is 8-channel DDR5 (H-9)");
        assert_eq!(p.mchbar_mask, 0, "Sapphire Rapids (no OS-accessible MCHBAR)");
        assert_eq!(p.gear, GearCap::None, "Sapphire Rapids (no gear register)");
    }

    /// Alder Lake and Raptor Lake resolve to the shared 2-channel
    /// Tier-3 profile (IG-03): 256 KiB window, the 256 KiB alignment
    /// mask, the DDR4-default channel count, the 4×-gear register set,
    /// the Alder map (Breakdown §2; Research line 19).
    #[test]
    fn tier3_dual_channel_generations_resolve_to_alder_profile() {
        for gen in [IntelGen::AlderLake, IntelGen::RaptorLake] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the 2-channel Alder profile"));
            assert_eq!(p, &ALDER_2CH, "{gen:?}");
            assert_eq!(p.window_size, 0x40000, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0x0000_007F_FFFC_0000, "{gen:?}");
            assert_eq!(p.channel_count, 2, "{gen:?}");
            assert_eq!(p.gear, GearCap::Gear4, "{gen:?}");
            assert_eq!(p.map, GenMap::Alder, "{gen:?}");
        }
    }

    /// Meteor Lake and Arrow Lake resolve to the 2-channel DDR5 Tier-3
    /// profile (IG-03, C-4: both are **dual-channel** DDR5, not
    /// 4-channel): the same 256 KiB window, mask, 4×-gear register
    /// set, and Alder map as the DDR4 profile, with the dual-channel
    /// DDR5 count (Breakdown §2; Research lines 20–21).
    #[test]
    fn tier3_ddr5_dual_channel_generations_resolve_to_alder_profile() {
        for gen in [IntelGen::MeteorLake, IntelGen::ArrowLake] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the 2-channel DDR5 Alder profile"));
            assert_eq!(p, &ALDER_DDR5, "{gen:?}");
            assert_eq!(p.window_size, 0x40000, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0x0000_007F_FFFC_0000, "{gen:?}");
            assert_eq!(p.channel_count, 2, "{gen:?}");
            assert_eq!(p.gear, GearCap::Gear4, "{gen:?}");
            assert_eq!(p.map, GenMap::Alder, "{gen:?}");
        }
    }

    /// The four Alder/Raptor Lake (Tier 3) channel descriptors pin the
    /// legacy-mirror bases 0x4000/0x4400/0x4800/0x4C00, the
    /// per-controller MCL fallback bases 0xD000 (MC0) and 0xD800 (MC1)
    /// on subchannels 0 and 2 only, and no MCL base on subchannels 1
    /// and 3 (OQ-4 — mirror-only); all four are 32-bit DDR5
    /// subchannels (Research lines 107, 329).
    #[test]
    fn alder_channels_pin_mirror_and_mcl_bases() {
        assert_eq!(ALDER_CHANNELS.len(), 4, "exactly four Tier 3 subchannels");
        let expected: [(u32, Option<u32>); 4] = [
            (0x4000, Some(0xD000)),
            (0x4400, None),
            (0x4800, Some(0xD800)),
            (0x4C00, None),
        ];
        for (i, ch) in ALDER_CHANNELS.iter().enumerate() {
            assert_eq!(ch.mirror_base, expected[i].0, "subch {i}");
            assert_eq!(ch.mcl_base, expected[i].1, "subch {i}");
            assert!(ch.is_subch, "subch {i} is a 32-bit DDR5 subchannel");
        }
        // OQ-4: each documented per-controller MCL base appears exactly
        // once; subchannels 1 and 3 carry none.
        assert_eq!(
            ALDER_CHANNELS.iter().filter(|c| c.mcl_base == Some(0xD000)).count(),
            1,
        );
        assert_eq!(
            ALDER_CHANNELS.iter().filter(|c| c.mcl_base == Some(0xD800)).count(),
            1,
        );
        assert_eq!(
            ALDER_CHANNELS.iter().filter(|c| c.mcl_base.is_none()).count(),
            2,
        );
    }

    /// The widened DDR5 timing bitfields pin the exact `(hi, lo)`
    /// ranges of the Research Alder/Raptor table (lines 111–132),
    /// grouped by their TC register.
    #[test]
    fn alder_widened_fields_match_research_lines_111_132() {
        let f = ALDER_FIELDS;
        // TC_PRE @ +0x00 (lines 112–115).
        assert_eq!(f.trcd, (14, 8));
        assert_eq!(f.trp, (6, 0));
        assert_eq!(f.tras, (23, 16));
        assert_eq!(f.tcwl, (30, 24));
        // TC_ACT @ +0x04 (lines 111, 116–118).
        assert_eq!(f.tcl, (23, 16));
        assert_eq!(f.tfaw, (7, 0));
        assert_eq!(f.trrd_s, (11, 8));
        assert_eq!(f.trrd_l, (15, 12));
        // TC_ACT2 @ +0x08 (line 125).
        assert_eq!(f.tppd, (3, 0));
        // TC_WTR @ +0x10 (lines 119–122).
        assert_eq!(f.twtr_s, (6, 0));
        assert_eq!(f.twtr_l, (14, 8));
        assert_eq!(f.twr, (23, 16));
        assert_eq!(f.trtp, (30, 24));
        // TC_RFP @ +0x14 (lines 123, 126).
        assert_eq!(f.trfc1, (11, 0));
        assert_eq!(f.trefi, (31, 16));
        // TC_RFP2 @ +0x18 (line 124).
        assert_eq!(f.trfcsb, (10, 0));
        // TC_RDRD @ +0x20 (lines 127–130); TC_WRWR @ +0x28
        // (lines 131–132).
        assert_eq!(f.trdrd_sg, (5, 0));
        assert_eq!(f.trdrd_dg, (11, 6));
        assert_eq!(f.trdrd_dr, (17, 12));
        assert_eq!(f.trdrd_dd, (23, 18));
        assert_eq!(f.twrwr_sg, (5, 0));
        assert_eq!(f.twrwr_dg, (11, 6));
        // Every documented range is well-formed: 0 <= lo <= hi <= 31.
        let ranges = [
            f.trcd, f.trp, f.tras, f.tcwl, f.tcl, f.tfaw, f.trrd_s,
            f.trrd_l, f.tppd, f.twtr_s, f.twtr_l, f.twr, f.trtp, f.trfc1,
            f.trefi, f.trfcsb, f.trdrd_sg, f.trdrd_dg, f.trdrd_dr,
            f.trdrd_dd, f.twrwr_sg, f.twrwr_dg,
        ];
        assert_eq!(ranges.len(), 22);
        for (hi, lo) in ranges {
            assert!(lo <= hi && hi <= 31, "malformed range ({hi}, {lo})");
        }
    }

    /// The two Alder Lake DDR4 channel descriptors pin the per-controller
    /// TC block bases (`0xE000` MC0, `0x1E000` MC1 = `0xE000 + 0x10000`)
    /// and the MCL block bases (`0xD000` / `0xD800`) — 2 channels, no
    /// subchannels (Phase 1b).
    #[test]
    fn alder_ddr4_channels_pin_mc_bases() {
        assert_eq!(ALDER_DDR4_CHANNELS.len(), 2, "DDR4 Alder Lake = 2 MCs");
        let expected: [(u32, u32); 2] = [
            (0xE000, 0xD000), // MC0
            (0x1E000, 0xD800), // MC1 (0xE000 + 0x10000)
        ];
        for (i, ch) in ALDER_DDR4_CHANNELS.iter().enumerate() {
            assert_eq!(ch.tc_base, expected[i].0, "MC{i} tc_base");
            assert_eq!(ch.mcl_base, expected[i].1, "MC{i} mcl_base");
        }
        // The inter-MC stride is 0x10000 for the TC block.
        assert_eq!(
            ALDER_DDR4_CHANNELS[1].tc_base - ALDER_DDR4_CHANNELS[0].tc_base,
            0x10000,
        );
    }

    /// The Alder Lake DDR4 timing fields pin the exact `(hi, lo)` ranges
    /// of the Intel 12th Gen Datasheet Vol 2 §3.2.66–3.2.87 table,
    /// grouped by their TC register (Phase 1a). `TC_PRE` is the 64-bit
    /// register (high-word fields reach bit 58); every other register is
    /// 32-bit.
    #[test]
    fn alder_ddr4_fields_match_datasheet() {
        let f = ALDER_DDR4_FIELDS;
        // TC_PRE @ +0xE000 (64-bit).
        assert_eq!(f.trcd, (58, 51));
        assert_eq!(f.tras, (50, 42));
        assert_eq!(f.twrpre, (41, 32));
        assert_eq!(f.tppd, (23, 20));
        assert_eq!(f.trdpre, (19, 13));
        assert_eq!(f.trp, (7, 0));
        // TC_ACT @ +0xE008 (32-bit).
        assert_eq!(f.trrd_dg, (21, 15));
        assert_eq!(f.trrd_sg, (14, 9));
        assert_eq!(f.tfaw, (8, 0));
        // TC_RDRD @ +0xE00C (32-bit).
        assert_eq!(f.rdrd_dd, (31, 24));
        assert_eq!(f.rdrd_dr, (23, 16));
        assert_eq!(f.rdrd_dg, (14, 8));
        assert_eq!(f.rdrd_sg, (6, 0));
        // TC_RDWR @ +0xE010 (32-bit).
        assert_eq!(f.rdwr_dd, (31, 24));
        assert_eq!(f.rdwr_dr, (23, 16));
        assert_eq!(f.rdwr_dg, (15, 8));
        assert_eq!(f.rdwr_sg, (7, 0));
        // TC_WRRD @ +0xE014 (32-bit).
        assert_eq!(f.wrrd_dd, (31, 25));
        assert_eq!(f.wrrd_dr, (24, 18));
        assert_eq!(f.wrrd_dg, (17, 9));
        assert_eq!(f.wrrd_sg, (8, 0));
        // TC_WRWR @ +0xE018 (32-bit).
        assert_eq!(f.wrwr_dd, (31, 24));
        assert_eq!(f.wrwr_dr, (22, 16));
        assert_eq!(f.wrwr_dg, (14, 8));
        assert_eq!(f.wrwr_sg, (6, 0));
        // TC_ODT @ +0xE070 (32-bit).
        assert_eq!(f.tcwl, (31, 24));
        assert_eq!(f.tcl, (22, 16));
        // TC_RFP @ +0xE438 (32-bit).
        assert_eq!(f.trefi9, (31, 24));
        assert_eq!(f.oref_ri, (7, 0));
        // TC_RFTP @ +0xE43C (32-bit).
        assert_eq!(f.trfc, (30, 18));
        assert_eq!(f.trefi, (17, 0));

        // Every range is well-formed: lo <= hi. The 64-bit TC_PRE fields
        // may reach bit 63; the 32-bit register fields stay <= 31.
        let all = [
            f.trcd, f.tras, f.twrpre, f.tppd, f.trdpre, f.trp, f.trrd_dg,
            f.trrd_sg, f.tfaw, f.rdrd_dd, f.rdrd_dr, f.rdrd_dg, f.rdrd_sg,
            f.rdwr_dd, f.rdwr_dr, f.rdwr_dg, f.rdwr_sg, f.wrrd_dd,
            f.wrrd_dr, f.wrrd_dg, f.wrrd_sg, f.wrwr_dd, f.wrwr_dr,
            f.wrwr_dg, f.wrwr_sg, f.tcwl, f.tcl, f.trefi9, f.oref_ri,
            f.trfc, f.trefi,
        ];
        assert_eq!(all.len(), 31);
        for (hi, lo) in all {
            assert!(lo <= hi, "malformed range ({hi}, {lo})");
            assert!(hi <= 63, "range exceeds 64-bit space ({hi}, {lo})");
        }
        // The 32-bit registers never exceed bit 31.
        let thirty_two_bit = [
            f.tppd, f.trdpre, f.trp, f.trrd_dg, f.trrd_sg, f.tfaw, f.rdrd_dd,
            f.rdrd_dr, f.rdrd_dg, f.rdrd_sg, f.rdwr_dd, f.rdwr_dr, f.rdwr_dg,
            f.rdwr_sg, f.wrrd_dd, f.wrrd_dr, f.wrrd_dg, f.wrrd_sg, f.wrwr_dd,
            f.wrwr_dr, f.wrwr_dg, f.wrwr_sg, f.tcwl, f.tcl, f.trefi9,
            f.oref_ri, f.trfc, f.trefi,
        ];
        for (hi, lo) in thirty_two_bit {
            assert!(hi <= 31, "32-bit field exceeds word ({hi}, {lo})");
        }
        // The high-word TC_PRE fields sit in [32, 63]; the low-word in
        // [0, 31].
        for (hi, lo) in [f.trcd, f.tras, f.twrpre] {
            assert!(lo >= 32 && hi <= 63, "high-word field ({hi}, {lo})");
        }
        for (hi, lo) in [f.tppd, f.trdpre, f.trp] {
            assert!(hi <= 31, "low-word field ({hi}, {lo})");
        }
    }

    /// The canonical hardware constants (M-2 single source of truth) match
    /// the values the consumers (intel_readout.rs / intel_mchbar.rs) expect.
    /// If a value here is ever changed, this test — and the consumer `use`
    /// imports it guards — keep the two in sync.
    #[test]
    fn constants_match_consumer_expectations() {
        assert_eq!(WINDOW_TIER1, 0x10000);
        assert_eq!(WINDOW_ALDER, 0x40000);
        assert_eq!(MCL0_BASE, 0xD000);
        assert_eq!(MCL1_BASE, 0xD800);
        assert_eq!(ALK_D4_TC0_BASE, 0xE000);
        assert_eq!(ALK_D4_TC1_BASE, 0x1E000);
    }
}
