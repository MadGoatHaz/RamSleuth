//! Per-generation Intel MCHBAR decode profiles (IG-01, CRITICAL-PATH).
//!
//! Pure data + dispatch: maps a detected [`IntelGen`] to a static
//! [`GenProfile`] (datasheet window size, MCHBAR alignment mask, channel
//! count, memory-gear capability, register-map family). No I/O, no
//! unsafe, no CPUID — the module is unit-tested in isolation and is the
//! foundation every later generational-expansion chunk dispatches on.
//!
//! IG-01 lands the Tier-1 (Skylake–Comet Lake) and Rocket Lake profiles;
//! IG-03 adds the Tier-3 `Alder` family entries (Alder/Raptor Lake at the
//! DDR4-default 2-channel count, Meteor/Arrow Lake at 4 channels). The
//! `Sandy` and `Haswell` `GenMap` families exist now and gain their
//! profile entries in later chunks once their `IntelGen` variants land;
//! IG-20 adds the Alder/Raptor Lake (Tier 3) register table: the four
//! DDR5 subchannel descriptors ([`ALDER_CHANNELS`]) with their legacy
//! mirror and uncore-MCL bases, and the widened DDR5 timing bitfield
//! constants ([`ALDER_FIELDS`]).
//!
//! The `mchbar_mask` values here are **diagnostic-only** for the
//! dispatcher (OQ-8): they document the alignment contract each profile
//! decodes against and do NOT alter the merged kernel module's ioremap
//! mask — `kernel/ramsleuth-intel/` is untouched by this module.
//!
//! Register references: Research lines 15–17, 107, 111–132, 329,
//! Breakdown §2 (`plans/PLAN-INTEL-GENERIC.md`).

use crate::cpuid::IntelGen;

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
/// `Tier1` and `Rocket`; IG-03 populates `Alder`; the `Sandy` and
/// `Haswell` entries land in later chunks once their `IntelGen` variants
/// exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenMap {
    /// Skylake–Comet Lake (64 KiB window, no gear register).
    Tier1,
    /// Rocket Lake (64 KiB window, 2× gear).
    Rocket,
    /// Alder/Raptor/Meteor/Arrow Lake (256 KiB window, 4× gear) — IG-03.
    Alder,
    /// Sandy/Ivy Bridge (32 KiB window) — later chunk (variants pending).
    Sandy,
    /// Haswell/Broadwell (64 KiB window) — later chunk (variants pending).
    Haswell,
}

/// A static per-generation MCHBAR decode profile.
///
/// All fields are compile-time constants; a profile is resolved by value
/// through [`profile_for`] and never mutated at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenProfile {
    /// The MCHBAR register window size in bytes the profile's register
    /// table is compile-checked against (Tier 1/Rocket: 64 KiB; the
    /// Alder family: 256 KiB).
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

/// The shared Tier-1 profile: Skylake, Kaby Lake, Coffee Lake, Comet Lake
/// (Breakdown §2: 64 KiB window, diagnostic-only mask, 2 channels, no
/// gear register).
const TIER1: GenProfile = GenProfile {
    window_size: 0x10000,
    mchbar_mask: 0x0000007FFFFFFE0000,
    channel_count: 2,
    gear: GearCap::None,
    map: GenMap::Tier1,
};

/// The Rocket Lake profile: the Tier-1 64 KiB window with the 2×-gear
/// register set (Breakdown §2; Research lines 15–17).
const ROCKET: GenProfile = GenProfile {
    window_size: 0x10000,
    mchbar_mask: 0x0000007FFFFFFE0000,
    channel_count: 2,
    gear: GearCap::Gear2,
    map: GenMap::Rocket,
};

/// The Alder/Raptor Lake profile: the 256 KiB window with the 4×-gear
/// register set (Breakdown §2; Research line 19). `channel_count` = 2
/// documents the DDR4 default — OQ-11's DDR5 detection (IG-26) flips the
/// decode-time count to 4; the profile field itself is untouched.
const ALDER_2CH: GenProfile = GenProfile {
    window_size: 0x40000,
    mchbar_mask: 0x0000007FFFFFC0000,
    channel_count: 2,
    gear: GearCap::Gear4,
    map: GenMap::Alder,
};

/// The Meteor/Arrow Lake profile: the 256 KiB window with the 4×-gear
/// register set and the native 4-channel DDR5 count (Breakdown §2;
/// Research lines 20–21; the tile-routing caveat OQ-5 is handled in
/// IG-28).
const ALDER_4CH: GenProfile = GenProfile {
    window_size: 0x40000,
    mchbar_mask: 0x0000007FFFFFC0000,
    channel_count: 4,
    gear: GearCap::Gear4,
    map: GenMap::Alder,
};

/// Resolve the static decode profile for a generation.
///
/// Returns the shared Tier-1 profile for the four Tier-1 generations,
/// the Rocket Lake profile for [`IntelGen::RocketLake`], the 2-channel
/// Alder profile for [`IntelGen::AlderLake`] / [`IntelGen::RaptorLake`]
/// and the 4-channel profile for [`IntelGen::MeteorLake`] /
/// [`IntelGen::ArrowLake`] (IG-03); every other generation (Ice Lake,
/// Tiger Lake, and [`IntelGen::Unrecognized`]) resolves to `None` until
/// its family gains a profile entry in a later chunk.
pub fn profile_for(gen: IntelGen) -> Option<&'static GenProfile> {
    match gen {
        IntelGen::Skylake
        | IntelGen::KabyLake
        | IntelGen::CoffeeLake
        | IntelGen::CometLake => Some(&TIER1),
        IntelGen::RocketLake => Some(&ROCKET),
        IntelGen::AlderLake | IntelGen::RaptorLake => Some(&ALDER_2CH),
        IntelGen::MeteorLake | IntelGen::ArrowLake => Some(&ALDER_4CH),
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
    mcl_base: Some(0xD000),
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
    mcl_base: Some(0xD800),
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
#[cfg(test)]
mod tests {
    use super::*;

    /// The four Tier-1 generations all resolve to the shared Tier-1
    /// profile with the exact IG-01 values (64 KiB window, diagnostic
    /// mask, 2 channels, no gear, Tier-1 map).
    #[test]
    fn tier1_generations_resolve_to_shared_profile() {
        for gen in [
            IntelGen::Skylake,
            IntelGen::KabyLake,
            IntelGen::CoffeeLake,
            IntelGen::CometLake,
        ] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the Tier-1 profile"));
            assert_eq!(p, &TIER1, "{gen:?}");
            assert_eq!(p.window_size, 0x10000, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0x0000007FFFFFFE0000, "{gen:?}");
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
        assert_eq!(p.mchbar_mask, 0x0000007FFFFFFE0000);
        assert_eq!(p.channel_count, 2);
        assert_eq!(p.gear, GearCap::Gear2);
        assert_eq!(p.map, GenMap::Rocket);
    }

    /// Non-profiled generations resolve to `None` in IG-03: Ice Lake
    /// and Tiger Lake (no family entry yet) and `Unrecognized`.
    #[test]
    fn non_profiled_generations_resolve_to_none() {
        assert_eq!(profile_for(IntelGen::IceLake), None);
        assert_eq!(profile_for(IntelGen::TigerLake), None);
        assert_eq!(profile_for(IntelGen::Unrecognized), None);
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
            assert_eq!(p.mchbar_mask, 0x0000007FFFFFC0000, "{gen:?}");
            assert_eq!(p.channel_count, 2, "{gen:?}");
            assert_eq!(p.gear, GearCap::Gear4, "{gen:?}");
            assert_eq!(p.map, GenMap::Alder, "{gen:?}");
        }
    }

    /// Meteor Lake and Arrow Lake resolve to the 4-channel Tier-3
    /// profile (IG-03): the same 256 KiB window, mask, 4×-gear
    /// register set, and Alder map as the dual-channel profile, with
    /// the native 4-channel DDR5 count (Breakdown §2; Research lines
    /// 20–21).
    #[test]
    fn tier3_quadruple_channel_generations_resolve_to_alder_profile() {
        for gen in [IntelGen::MeteorLake, IntelGen::ArrowLake] {
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must resolve to the 4-channel Alder profile"));
            assert_eq!(p, &ALDER_4CH, "{gen:?}");
            assert_eq!(p.window_size, 0x40000, "{gen:?}");
            assert_eq!(p.mchbar_mask, 0x0000007FFFFFC0000, "{gen:?}");
            assert_eq!(p.channel_count, 4, "{gen:?}");
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
}
