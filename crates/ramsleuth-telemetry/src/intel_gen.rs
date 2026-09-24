//! Per-generation Intel MCHBAR decode profiles (IG-01, CRITICAL-PATH).
//!
//! Pure data + dispatch: maps a detected [`IntelGen`] to a static
//! [`GenProfile`] (datasheet window size, MCHBAR alignment mask, channel
//! count, memory-gear capability, register-map family). No I/O, no
//! unsafe, no CPUID — the module is unit-tested in isolation and is the
//! foundation every later generational-expansion chunk dispatches on.
//!
//! IG-01 lands the Tier-1 (Skylake–Comet Lake) and Rocket Lake profiles
//! only; the `Alder`, `Sandy`, and `Haswell` `GenMap` families exist now
//! and gain their profile entries in later chunks (IG-03+).
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
/// `Tier1` and `Rocket`; the `Alder`, `Sandy`, and `Haswell` entries land
/// in later chunks (IG-03+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenMap {
    /// Skylake–Comet Lake (64 KiB window, no gear register).
    Tier1,
    /// Rocket Lake (64 KiB window, 2× gear).
    Rocket,
    /// Alder/Raptor/Meteor/Arrow Lake (256 KiB window, 4× gear) — IG-03+.
    Alder,
    /// Sandy/Ivy Bridge (32 KiB window) — IG-03+.
    Sandy,
    /// Haswell/Broadwell (64 KiB window) — IG-03+.
    Haswell,
}

/// A static per-generation MCHBAR decode profile.
///
/// All fields are compile-time constants; a profile is resolved by value
/// through [`profile_for`] and never mutated at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenProfile {
    /// The MCHBAR register window size in bytes the profile's register
    /// table is compile-checked against (Tier 1/Rocket: 64 KiB).
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

/// Resolve the static decode profile for a generation.
///
/// Returns the shared Tier-1 profile for the four Tier-1 generations and
/// the Rocket Lake profile for [`IntelGen::RocketLake`]; every other
/// generation (including [`IntelGen::Unrecognized`]) resolves to `None`
/// until its family gains a profile entry in a later chunk (IG-03+).
pub fn profile_for(gen: IntelGen) -> Option<&'static GenProfile> {
    match gen {
        IntelGen::Skylake
        | IntelGen::KabyLake
        | IntelGen::CoffeeLake
        | IntelGen::CometLake => Some(&TIER1),
        IntelGen::RocketLake => Some(&ROCKET),
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

    /// Non-profiled generations resolve to `None` in IG-01: a
    /// non-Tier-1/2 generation (Alder Lake) and `Unrecognized`.
    #[test]
    fn non_profiled_generations_resolve_to_none() {
        assert_eq!(profile_for(IntelGen::AlderLake), None);
        assert_eq!(profile_for(IntelGen::Unrecognized), None);
    }
}
