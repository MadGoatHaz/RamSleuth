//! Generation/platform gate functions for the Intel IMC decode: the
//! vendor gate, the profile-support predicates, the rich branch-level
//! gates, the tile-routing probe, and the static channel-count model.
//!
//! Distinct from [`super::gates`] (the per-cell sanity gates /
//! degenerate-word screening helpers) — this module is about *which*
//! generation to decode and *whether* it is profiled at all.
//!
//! Dependencies: [`crate::cpuid`] (vendor + generation),
//! [`crate::error`] (the structured `UnsupportedHardware`),
//! [`crate::intel_gen`] (the profile table).

use crate::cpuid::{CpuInfo, CpuVendor, IntelGen};
use crate::error::{TelemetryError, TelemetryResult};
use crate::intel_gen::profile_for;

/// The pure vendor gate for the Intel branch: Intel passes with its
/// [`IntelGen`]; AMD and unknown vendors yield
/// [`TelemetryError::UnsupportedHardware`] (no register access is
/// possible or attempted once this returns `Err`).
pub fn intel_gen_gate(info: &CpuInfo) -> TelemetryResult<IntelGen> {
    match info.vendor {
        CpuVendor::Intel(gen) => Ok(gen),
        CpuVendor::Amd(_) => Err(TelemetryError::UnsupportedHardware {
            vendor: "AMD (Intel IMC decode requires an Intel CPU)".to_owned(),
        }),
        CpuVendor::Unknown => Err(TelemetryError::UnsupportedHardware {
            vendor: "unknown CPU vendor".to_owned(),
        }),
    }
}

/// The Tier-1 set: the generations the hardware-authoritative register
/// map is verified for. A compatibility alias kept from the
/// pre-dispatch name — use [`gen_supported`] for "does this generation
/// have a decode profile" (Tier 1 plus Rocket Lake as of this build).
pub fn tier1_supported(gen: IntelGen) -> bool {
    matches!(
        gen,
        IntelGen::Skylake | IntelGen::KabyLake | IntelGen::CoffeeLake | IntelGen::CometLake
    )
}

/// Profile dispatch: `true` for every generation that
/// [`intel_gen::profile_for`] resolves to a decode profile (Tier 1 —
/// Skylake / Kaby Lake / Coffee Lake / Comet Lake — plus Rocket Lake
/// (Tier 2) and the Tier-3 Alder family — Alder / Raptor / Meteor /
/// Arrow Lake — as of IG-03).
pub fn gen_supported(gen: IntelGen) -> bool {
    profile_for(gen).is_some()
}

/// The rich v1 Tier-1 generation gate (the facade's branch-level
/// gate): `Ok(())` for Tier 1 only, else
/// [`TelemetryError::UnsupportedHardware`] whose `vendor` detail names
/// the detected generation and its research-roadmap tier (Rocket Lake =
/// Tier 2; Alder/Raptor/Arrow = Tier 3) — the facade surfaces this as
/// the branch-level `Na(UnsupportedHardware)` (plan §3.4: never garbage
/// data from a mismatched register map).
pub fn tier1_gate(gen: IntelGen) -> TelemetryResult<()> {
    if tier1_supported(gen) {
        return Ok(());
    }
    let tier = match gen {
        IntelGen::AlderLake | IntelGen::RaptorLake => {
            "Tier 3 (dual-MC DDR4/DDR5, decoded on the Alder profile — outside this Tier-1 gate)"
        }
        IntelGen::MeteorLake | IntelGen::ArrowLake => {
            "Tier 3 (DDR5, decoded on the Alder profile — outside this Tier-1 gate)"
        }
        IntelGen::RocketLake => "Tier 2 (Gear Mode + turnaround parity; outside this Tier-1 gate)",
        _ => "beyond Tier 1",
    };
    Err(TelemetryError::UnsupportedHardware {
        vendor: format!(
            "Intel {gen:?} is {tier}; v1 decodes Tier 1 (Skylake/Kaby Lake/Coffee Lake/Comet Lake) only"
        ),
    })
}

/// The profile-based generation gate (the facade's branch-level gate
/// as of IG-16): `Ok(())` for every generation [`gen_supported`]
/// admits — Tier 1 (Skylake / Kaby Lake / Coffee Lake / Comet Lake /
/// Ice Lake / Tiger Lake), Rocket Lake (Tier 2), the Tier-3 Alder
/// family (Alder / Raptor / Meteor / Arrow Lake), and the Server
/// profile (Sapphire Rapids) — else
/// [`TelemetryError::UnsupportedHardware`] whose `vendor` detail names
/// the detected generation and the profiled set it is absent from (the
/// unprofiled generations — Jasper Lake / Lunar Lake / Core Series 1
/// Mobile / `Unrecognized`) — the facade surfaces this as the
/// branch-level `Na(UnsupportedHardware)` (plan §3.4: never garbage
/// data from a mismatched register map).
///
/// This is the gate that lets Rocket Lake reach [`super::decode`]'s
/// profile dispatch for the first time (IG-14/IG-15 make `detect()`
/// emit it; IG-12 wires the profile). [`tier1_gate`] is retained as
/// the Tier-1-only compatibility alias for other callers.
pub fn gen_gate(gen: IntelGen) -> TelemetryResult<()> {
    if gen_supported(gen) {
        return Ok(());
    }
    Err(TelemetryError::UnsupportedHardware {
        vendor: format!(
            "Intel {gen:?} has no decode profile; v1 decodes Tier 1 (Skylake/Kaby Lake/Coffee Lake/Comet Lake/Ice Lake/Tiger Lake), Tier 2 (Rocket Lake), Tier 3 (Alder/Raptor/Meteor/Arrow Lake), and Server (Sapphire Rapids) only"
        ),
    })
}

/// OQ-5's branch-level tile-routing detail (IG-28): the
/// [`TelemetryError::UnsupportedHardware`] that accompanies a Meteor /
/// Arrow Lake readout whose primary window is fully degenerate
/// ([`super::IntelImcRegs::primary_window_degenerate`]). Per Research
/// line 328, on disaggregated SoC/compute tiles the physical IMC can
/// sit behind the secondary uncore range (MCHBAR + 0x180000) that the
/// module does not expose; the honest degradation is the all-Na
/// readout ([`super::decode`] returns the all-`Na(UnsupportedHardware)`
/// shape at the generation's channel count) with this message — never
/// a value fabricated from the secondary range.
pub fn tile_routing_suspected(gen: IntelGen) -> TelemetryError {
    TelemetryError::UnsupportedHardware {
        vendor: format!(
            "Intel {gen:?}: SoC-tile IMC routing suspected (MCHBAR + 0x180000 secondary range) — primary window empty; see OQ-5"
        ),
    }
}

/// The fixed channel-count model for an [`IntelGen`] (plan: per-channel
/// 0–3): the profiled generations take their count from their
/// [`intel_gen::GenProfile`] (Tier 1 and Rocket Lake: 2; the Alder
/// family: 2 for Alder/Raptor Lake — the DDR4 default — and 4 for
/// Meteor/Arrow Lake, as of IG-03); the non-profiled generations keep
/// the legacy model — DDR4-class generations expose 2 channels and an
/// unrecognized family-6 Intel falls back to the common 2-channel
/// desktop layout. Same values as before this dispatch, so no wire
/// change. (The Tier-3 *decode* may expose a different decode-time
/// subchannel count for the Alder family — OQ-11's DDR5 condition
/// flips [`super::decode`] to the 4-subchannel geometry; this function
/// stays the static model.)
pub fn channel_count(gen: IntelGen) -> u8 {
    if let Some(p) = profile_for(gen) {
        return p.channel_count;
    }
    // No decode profile: the common 2-channel desktop fallback. Every
    // profiled generation (Tier 1 + Rocket Lake + the Alder family + the
    // server gens) returns above via `profile_for`, so the specific-gen
    // arms that used to live here are dead (C-4 made Arrow / Meteor
    // Lake 2-channel; the profile is now the single source of truth for
    // the count).
    2
}
