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
//! profile entries in later chunks once their `IntelGen` variants land.
//!
//! The `mchbar_mask` values here are **diagnostic-only** for the
//! dispatcher (OQ-8): they document the alignment contract each profile
//! decodes against and do NOT alter the merged kernel module's ioremap
//! mask — `kernel/ramsleuth-intel/` is untouched by this module.
//!
//! Register references: Research lines 15–17, Breakdown §2
//! (`plans/PLAN-INTEL-GENERIC.md`).

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
}
