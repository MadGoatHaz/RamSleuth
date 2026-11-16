//! Shared MCLK / gear / uclk decoders (used by all three decode paths):
//! the generation-dependent `MC_BIOS_REQ` clock decode, the gear-ratio
//! decode to [`GearMode`], the SA:MEM divisor, and the uclk computation.
//!
//! Dependencies: [`super::constants`] (the reference clocks),
//! [`super::gates`] (the clock sanity gate), [`crate::intel_gen`]
//! (the gear cap), [`crate::amd_readout::GearMode`],
//! [`crate::cpuid::IntelGen`], [`crate::error`] (`Section` / `NaReason`).

use crate::amd_readout::GearMode;
use crate::cpuid::IntelGen;
use crate::error::{NaReason, Section};
use crate::intel_gen::GearCap;

use super::constants::{REF_CLK_100_MHZ, REF_CLK_133_MHZ};
use super::gates::clock_section;

/// The `MC_BIOS_REQ` analog DRAM clock (MCLK) in MHz — the ratio
/// semantics are generation-dependent (the Alder Lake MC_BIOS_REQ
/// decode):
///
/// - **Skylake–Comet Lake (6th–10th gen)**: `CLK_RATIO` (bits [7:0])
///   is a physical DRAM-clock multiplier → `mclk = ratio × refclk`
///   (e.g. ratio 18 × 133.3333 = 2400 MHz; for DDR, MT/s = MCLK × 2
///   = 4800).
/// - **11th gen (Rocket Lake) and later**: Intel redefined
///   `CLK_RATIO` as the effective DDR data-rate multiplier in
///   half-cycle steps (one step = one data transfer beat — the
///   restructuring that lets every bin on both reference clocks map
///   to a whole integer), so the true core clock is `mclk = ratio ×
///   refclk / 2` (e.g. the 12600KF DDR4-3600 word 0x79E00124:
///   36 × 100 MHz / 2 = 1800 MHz → 3600 MT/s).
///
/// The bit-8 reference-clock select is unchanged across generations
/// (1 = 100 MHz, 0 = 133.3333 MHz — the PLL base inputs); the result
/// is sanity-gated to [1, 5333] MHz.
///
/// Containment: an absent register → `Na(ParseError)`; a ratio of 0 =
/// unconfigured → `Na(ParseError)`; the GEAR_RATIO bits (17:16,
/// Rocket+ only) do not affect the MCLK decode — on the Rocket Lake
/// profile they feed the gear cells via [`mcbios_gear`], not MCLK.
pub(crate) fn decode_mclk(reg: Option<u32>, gen: IntelGen) -> Section<f64> {
    match reg {
        None => Section::na(NaReason::ParseError(
            "MC_BIOS_REQ: register read failed".to_owned(),
        )),
        Some(raw) => {
            let ratio = raw & 0xFF;
            if ratio == 0 {
                return Section::na(NaReason::ParseError(
                    "MC_BIOS_REQ: CLK_RATIO unconfigured (0)".to_owned(),
                ));
            }
            let refclk = if mcbios_refclk_100(raw) {
                REF_CLK_100_MHZ
            } else {
                REF_CLK_133_MHZ
            };
            let product = f64::from(ratio) * refclk;
            // 11th gen and later: the ratio is the data-rate
            // multiplier (half-cycle steps) — halve it to the true
            // core clock (the Alder Lake MC_BIOS_REQ decode).
            let mclk = if mclk_half_step(gen) {
                product / 2.0
            } else {
                product
            };
            clock_section(mclk)
        }
    }
}

/// The 11th-gen (Rocket Lake) and later `CLK_RATIO` semantics: the
/// field is the effective DDR data-rate multiplier in half-cycle
/// steps (the Alder Lake MC_BIOS_REQ decode), so the true MCLK is
/// half the `ratio × refclk` product. Skylake–Comet Lake keep the
/// legacy physical-clock multiplier (the full product); the unprofiled
/// generations never reach [`decode_mclk`] from [`super::decode`] (they
/// degrade all-Na before any MCLK decode).
fn mclk_half_step(gen: IntelGen) -> bool {
    matches!(
        gen,
        IntelGen::RocketLake
            | IntelGen::AlderLake
            | IntelGen::RaptorLake
            | IntelGen::MeteorLake
            | IntelGen::ArrowLake
    )
}

/// The `MC_BIOS_REQ` reference-clock select (bit 8): `true` = 100 MHz,
/// `false` = 133.3333 MHz.
fn mcbios_refclk_100(raw: u32) -> bool {
    raw & 0x0100 != 0
}

/// The `MC_BIOS_REQ` gear-ratio field (bits [17:16] — Rocket+ only)
/// decoded to a [`GearMode`], verbatim per Breakdown §3D:
///
/// - bits `[17:16] = 00b` → [`GearMode::One`] (1:1, the synchronous
///   default) **for every cap, including `GearCap::None`** (L-3): `00b`
///   means "no gear divider set" = the 1× mode, the normal valid state on
///   a Tier-1 platform that has no gear register. This is distinct from a
///   *set* gear bit on a `GearCap::None` platform (below), which is
///   over-cap → `None`;
/// - bit 16 = 1b → [`GearMode::Two`] (the `Gear2` and `Gear4` caps);
/// - bit 17 = 1b (with bit 16 = 0b) → [`GearMode::Four`], allowed ONLY
///   when `cap == GearCap::Gear4` (Alder Lake and later); on a `Gear2`
///   cap (Rocket Lake) a set bit 17 is over-cap → `None` (documented);
/// - any other reserved / over-cap encoding (notably a gear bit *set* on
///   a `GearCap::None` Tier-1 platform, which has no gear register) →
///   `None`.
///
/// A `11b` field follows the bit-16 rule (gear 2) — the pinned
/// Breakdown §4 example D reading. Pure and cap-parameterized (IG-11);
/// wired into [`super::decode`] by IG-12 for the Rocket Lake profile.
pub fn mcbios_gear(raw: u32, cap: GearCap) -> Option<GearMode> {
    if raw & 0x0001_0000 != 0 {
        // Gear 2 (bit 16 = 1b): every 2×-gear-capable platform.
        matches!(cap, GearCap::Gear2 | GearCap::Gear4).then_some(GearMode::Two)
    } else if raw & 0x0002_0000 != 0 {
        // Gear 4 (bit 17 = 1b, bit 16 = 0b): only the 4× cap (Alder+).
        (cap == GearCap::Gear4).then_some(GearMode::Four)
    } else {
        // Gear 1 (bits [17:16] = 00b): 1:1 — every cap.
        Some(GearMode::One)
    }
}

/// The SA:MEM divisor for a decoded [`GearMode`] (`uclk = mclk /
/// gear`).
fn gear_divisor(gear: GearMode) -> f64 {
    match gear {
        GearMode::One => 1.0,
        GearMode::Two => 2.0,
        GearMode::Four => 4.0,
    }
}

/// The internal controller clock (f_UCLK) for the decoded gear, per
/// Breakdown §3D (f_UCLK = MCLK, MCLK/2, MCLK/4 for gears 1, 2, 4):
/// `uclk = mclk / gear_divisor(gear)`, sanity-gated to the [1, 5333]
/// MHz clock band via [`clock_section`].
///
/// Containment mirrors the legacy uclk rule: an `Na` gear cell (a
/// failed register read, a reserved, or an over-cap encoding) →
/// `Na(ParseError)` — never a bogus clock. Pure (IG-11); wired into
/// [`super::decode`] by IG-12 for the Rocket Lake profile.
pub fn decode_uclk(mclk: f64, gear: &Section<GearMode>) -> Section<f64> {
    match gear.value().copied() {
        Some(g) => clock_section(mclk / gear_divisor(g)),
        None => Section::na(NaReason::ParseError(
            "uclk = mclk / gear: gear unavailable or reserved encoding".to_owned(),
        )),
    }
}

/// The gear + uclk cells shared across channels (the same `MC_BIOS_REQ`
/// word for every channel): decode once, layer on every channel.
pub(crate) fn gear_uclk_cells(
    gear_cap: GearCap,
    mclk: &Section<f64>,
    mcbios_req: Option<u32>,
) -> (Section<GearMode>, Section<f64>) {
    let gear = mcbios_req.and_then(|raw| mcbios_gear(raw, gear_cap));
    let gear_mode = match gear {
        Some(g) => Section::Value(g),
        None => Section::na(NaReason::ParseError(
            "MC_BIOS_REQ: gear unavailable (register read failed) or reserved/over-cap encoding"
                .to_owned(),
        )),
    };
    let uclk = match (gear, mclk.value().copied()) {
        (Some(g), Some(m)) => decode_uclk(m, &Section::Value(g)),
        (None, _) => Section::na(NaReason::ParseError(
            "uclk = mclk / gear: gear unavailable or reserved encoding".to_owned(),
        )),
        (_, None) => Section::na(NaReason::ParseError(
            "uclk = mclk / gear: mclk unavailable".to_owned(),
        )),
    };
    (gear_mode, uclk)
}
