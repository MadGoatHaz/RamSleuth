//! Fixture tests — the CI stand-in for the hardware acceptance
//! (plan §6): the Skylake acceptance raws (`mcbios_req = 0x12` →
//! ratio 18 @ 133.3333 MHz = 2400 MHz MCLK / 4800 MT/s) pin the
//! decoded numbers exactly, plus degradation / containment / gate /
//! wire-safety coverage, all without hardware, root, or I/O. The
//! downstream fixture constructor ([`fixture_intel_readout`]) is
//! pinned by TC-TEL4 (the legacy skeleton is gone; the fixtures
//! build through the live [`decode`]).

use super::*;
use crate::cpuid::AmdZen;
use crate::intel_gen::ALDER_CHANNELS;
use crate::intel_readout::fixtures::*;
use crate::intel_smbios::DerivedMode;

// -----------------------------------------------------------------
// The §6.1 acceptance fixture (Skylake i5-6600T, ratio 18 @
// 133.3333 MHz → 2400 MHz MCLK / 4800 MT/s, 17-17-17-39,
// dual-channel symmetric).
// -----------------------------------------------------------------

/// The plan §6.1 acceptance raws: `mcbios_req = 0x00000012`,
/// `ch0_tc_dbp = 0x11110F11`, `ch0_tc_rap = 0x27180204`,
/// `ch0_tc_rfp = 0x000001A4`, ch1 symmetric — plus in-range
/// canonical values for the remaining `TC_*` registers (their
/// decoded slots are pinned by the acceptance test below).
fn acceptance_regs() -> IntelImcRegs {
    let ch0 = ChannelRegs {
        tc_dbp: Some(0x1111_0F11), // tCL 17, tCWL 15, tRCD 17, tRP 17
        // tRRD_S 4, tRTP 8, tCKE 0 (4-bit; not displayed), tFAW 24,
        // tRAS 39.
        tc_rap: Some(0x2718_0204),
        tc_rfp: Some(0x0000_01A4),  // tRFC 420, tREFI 0 (not displayed)
        tc_rap2: Some(0x0000_0C0A), // tRRD_L 10, tWR 12
        tc_rdrd: Some(0x0048_C286), // sg 6, dg 10, dr 12, dd 18
        tc_rdwr: Some(0x0000_0280), // dg 10 (the representative turnaround)
        tc_wrrd: Some(0x0000_0308), // sg 8 (tWTR_L), dg 12 (tWTR_S)
        tc_wrwr: Some(0x0040_C204), // sg 4, dg 8, dr 12, dd 16
    };
    IntelImcRegs {
        // ratio 18, REF_CLK = 133.3333 MHz → 2400 MHz MCLK.
        mcbios_req: Some(0x0000_0012),
        ch1: ch0.clone(),
        ch0,
        // The Tier-3 extension fields stay default (all `None` —
        // containment, IG-22; no Tier-1/2 producer sets them).
        ..IntelImcRegs::default()
    }
}

/// §6.1 step 4 (Tier 1 — the legacy physical-multiplier
/// semantics): `mcbios_req = 0x00000012` → ratio 18 × 133.3333 MHz
/// = **2400 MHz** MCLK, **4800 MT/s** (for DDR, MT/s = MCLK × 2) —
/// pinned exactly.
#[test]
fn acceptance_mcbios_req_decodes_2400_mhz_4800_mts() {
    let mclk = decode_mclk(Some(0x0000_0012), IntelGen::Skylake);
    assert_eq!(mclk, Section::Value(2400.0));
    assert_eq!(
        mclk.value().copied().map(|v| v * 2.0),
        Some(4800.0),
        "DDR: two transfers per DRAM clock = 4800 MT/s"
    );
}

/// Live-hardware anchor (Skylake i5-6600T, DDR4-2133):
/// `mcbios_req = 0x08` → ratio 8 × 133.3333 MHz = **1066.67 MHz**
/// MCLK (the analog DRAM clock; MT/s = MCLK × 2 = 2133.33 MT/s =
/// DDR4-2133) — within 0.01 MHz of the decimal value.
#[test]
fn decode_mclk_live_ddr4_2133_anchor() {
    let mclk = decode_mclk(Some(0x0000_0008), IntelGen::Skylake);
    assert_eq!(mclk, Section::Value(8.0 * REF_CLK_133_MHZ));
    let mhz = mclk.value().copied().expect("ratio 8 decodes in-band");
    assert!(
        (mhz - 1066.67).abs() < 0.01,
        "MCLK ≈ 1066.67 MHz, got {mhz}"
    );
}

/// The §6.1 acceptance decode, pinned end to end: ch0 every decoded
/// slot at its acceptance number, ch1 symmetric (identical raws →
/// identical decoded cells), the v1 not-applicable cells
/// not-applicable, the CAD bus / voltages not exposed.
#[test]
fn acceptance_skylake_readout_pins_the_section_numbers() {
    let mut regs = acceptance_regs();
    regs.mad_inter_channel = Some(0);
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "Tier 1: two channels");
    // No `MAD_DIMM_CH0/1` population raws + no SMBIOS -> the 4-tier
    // terminal state (the acceptance fixture pins the *timings*, not the
    // channel mode).
    assert_eq!(ro.channel_mode, None, "no population evidence -> terminal");
    let ch0 = &ro.channels[0];

    // --- clocks (v1: mclk only) ------------------------------------
    assert_eq!(ch0.clocks.mclk_mhz, Section::Value(2400.0));
    assert_eq!(ch0.clocks.uclk_mhz, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.fclk_mhz, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.div_mode, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.gear_mode, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.gdm, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.pdm, Section::na(NaReason::NotApplicable));
    assert_eq!(
        ch0.clocks.command_rate,
        Section::na(NaReason::NotApplicable)
    );
    assert_eq!(ch0.rtl, Section::na(NaReason::NotApplicable));

    // --- timings (the acceptance numbers, pinned exactly) ----------
    assert_eq!(ch0.timings.cl, Section::Value(17));
    assert_eq!(ch0.timings.cwl, Section::Value(15));
    assert_eq!(ch0.timings.rcdrd, Section::Value(17));
    assert_eq!(
        ch0.timings.rcwdwr,
        Section::Value(17),
        "the unified symmetric tRCD feeds both rcdrd and rcwdwr"
    );
    assert_eq!(ch0.timings.rp, Section::Value(17));
    assert_eq!(ch0.timings.ras, Section::Value(39));
    assert_eq!(
        ch0.timings.rc,
        Section::Value(56),
        "tRAS + tRP = 39 + 17 (synthesized — Intel exposes no tRC)"
    );
    assert_eq!(ch0.timings.rrds, Section::Value(4));
    assert_eq!(ch0.timings.rrld, Section::Value(10));
    assert_eq!(ch0.timings.faw, Section::Value(24));
    assert_eq!(ch0.timings.rtp, Section::Value(8));
    assert_eq!(ch0.timings.wr, Section::Value(12));
    assert_eq!(ch0.timings.rfc1, Section::Value(420));
    assert_eq!(ch0.timings.rdwr, Section::Value(10));
    assert_eq!(ch0.timings.wtrs, Section::Value(12));
    assert_eq!(ch0.timings.wtrl, Section::Value(8));
    assert_eq!(ch0.timings.rdrd_scl, Section::Value(6));
    assert_eq!(ch0.timings.rdrd_sc, Section::Value(10));
    assert_eq!(ch0.timings.rdrd_sd, Section::Value(12));
    assert_eq!(ch0.timings.rdrd_dd, Section::Value(18));
    assert_eq!(ch0.timings.wrwr_scl, Section::Value(4));
    assert_eq!(ch0.timings.wrwr_sc, Section::Value(8));
    assert_eq!(ch0.timings.wrwr_sd, Section::Value(12));
    assert_eq!(ch0.timings.wrwr_dd, Section::Value(16));
    for s in [&ch0.timings.rfc2, &ch0.timings.rfcsb, &ch0.timings.wrrd] {
        assert_eq!(*s, Section::na(NaReason::NotApplicable), "{s:?}");
    }

    // --- CAD bus + voltages: never exposed by the IMC window --------
    for s in [
        &ch0.cad_bus.proc_odt,
        &ch0.cad_bus.clk_drv,
        &ch0.cad_bus.addr_cmd_drv,
        &ch0.cad_bus.cs_odt_drv,
        &ch0.cad_bus.cke_drv,
    ] {
        assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
    }
    for s in [
        &ch0.cad_bus.rtt_nom,
        &ch0.cad_bus.rtt_wr,
        &ch0.cad_bus.rtt_park,
    ] {
        assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
    }
    for s in [
        &ch0.voltages.vddcr_soc_mv,
        &ch0.voltages.vddio_mem_mv,
        &ch0.voltages.vdd_misc_mv,
        &ch0.voltages.vpp_mv,
        &ch0.voltages.vcore_mv,
    ] {
        assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
    }

    // ch1 symmetric: identical raws → identical decoded cells (only
    // the channel index differs).
    let mut ch1 = ro.channels[1].clone();
    ch1.index = 0;
    assert_eq!(ch1, ro.channels[0].clone(), "ch1 is symmetric with ch0");
}

// -----------------------------------------------------------------
// Clock decode: reference clock, unconfigured, reserved bits, the
// [1, 5333] MHz sanity gate.
// -----------------------------------------------------------------

/// REF_CLK = 1 (100 MHz), Tier 1 (the legacy physical multiplier —
/// e.g. pre-Alder DDR4-3600 runs 18 × 100 MHz = 1800 MHz core):
/// ratio 18 × 100 = 1800 MHz, exact.
#[test]
fn decode_mclk_refclk_100_mhz() {
    assert_eq!(
        decode_mclk(Some(0x0112), IntelGen::CoffeeLake),
        Section::Value(1800.0)
    );
    // ratio 31 with the 100 MHz ref: 3100.0 MHz (in-band, exact).
    assert_eq!(
        decode_mclk(Some(0x011F), IntelGen::CoffeeLake),
        Section::Value(3100.0)
    );
}

/// A ratio of 0 is unconfigured → `Na(ParseError)`, never a 0 MHz
/// value; an absent register degrades the same way.
#[test]
fn decode_mclk_zero_ratio_and_absent_register() {
    // The containment is generation-independent (both Na).
    assert!(matches!(
        decode_mclk(Some(0), IntelGen::Skylake),
        Section::Na(NaReason::ParseError(_))
    ));
    assert!(matches!(
        decode_mclk(None, IntelGen::Skylake),
        Section::Na(NaReason::ParseError(_))
    ));
}

/// The reserved GEAR_RATIO bits (17:16 — Rocket+ only, Tier 2) do
/// not affect the v1 mclk decode.
#[test]
fn decode_mclk_ignores_reserved_gear_bits_in_v1() {
    assert_eq!(
        decode_mclk(Some(0x0003_0012), IntelGen::Skylake),
        Section::Value(2400.0)
    );
}

/// The [1, 5333] MHz sanity gate bounds the displayed mclk: ratio
/// 40 at the 133.3333 MHz ref (≈ 5333.33 MHz) / ratio 255 (≈ 34000
/// MHz at the 133 ref; 25500 MHz at the 100 ref) degrade to
/// `Na(ParseError)`; ratio 39 at the 133 ref (≈ 5200 MHz), ratio
/// 30 (≈ 4000 MHz), and ratio 40 at the 100 ref (4000 MHz) stay
/// in-band. H-6 raised the ceiling from 4096 to 5333 MHz to cover
/// DDR5-10666.
#[test]
fn decode_mclk_sanity_gate() {
    // Tier 1 (the legacy full product sets the [1, 5333] bounds).
    // ratio 31 @ 133.3333 MHz ≈ 4133 MHz → in-band (H-6).
    assert!(matches!(
        decode_mclk(Some(31), IntelGen::Skylake),
        Section::Value(_)
    ));
    // ratio 41 @ 100 MHz = 4100 MHz → in-band (H-6).
    assert!(matches!(
        decode_mclk(Some(0x0129), IntelGen::Skylake),
        Section::Value(_)
    ));
    // ratio 255 @ 133.3333 MHz ≈ 34000 MHz → out-of-band.
    assert!(matches!(
        decode_mclk(Some(0x00FF), IntelGen::Skylake),
        Section::Na(NaReason::ParseError(_))
    ));
    // ratio 255 @ 100 MHz = 25500 MHz → out-of-band.
    assert!(matches!(
        decode_mclk(Some(0x01FF), IntelGen::Skylake),
        Section::Na(NaReason::ParseError(_))
    ));
    // ratio 30 @ 133.3333 MHz = 4000 MHz → in-band.
    assert!(matches!(
        decode_mclk(Some(30), IntelGen::Skylake),
        Section::Value(_)
    ));
    // ratio 40 @ 100 MHz = 4000 MHz → in-band.
    assert!(matches!(
        decode_mclk(Some(0x0128), IntelGen::Skylake),
        Section::Value(_)
    ));
    // ratio 39 @ 133.3333 MHz ≈ 5200 MHz → in-band.
    assert!(matches!(
        decode_mclk(Some(39), IntelGen::Skylake),
        Section::Value(_)
    ));
    // ratio 40 @ 133.3333 MHz ≈ 5333.33 MHz → just over the 5333 limit.
    assert!(matches!(
        decode_mclk(Some(40), IntelGen::Skylake),
        Section::Na(NaReason::ParseError(_))
    ));
}

// -----------------------------------------------------------------
// The 11th-gen-plus MC_BIOS_REQ decode anchors (half-step
// semantics). Generation-level: the representative fixture is the
// documented Alder Lake i5-12600KF word (the hardware-confirmed
// reference device); the cross-generation invariance of the
// formula is pinned in the two parameterized tests that follow.
// -----------------------------------------------------------------

/// The 11th-gen-plus half-step anchor. The representative word is
/// the **documented Alder Lake i5-12600KF fixture** (the reference
/// device — hardware-confirmed): `MC_BIOS_REQ` 0x79E00124 (ratio
/// 36, bit 8 = 1 → 100 MHz refclk, bits [17:16] = 00b → gear 1;
/// the upper PLL-tracking word is ignored by the decode) resolves,
/// under the 11th-gen-and-later half-step semantics, to the true
/// core clock 36 × 100 / 2 = **1800 MHz** — and the reporters'
/// standard DDR × 2 then yields **3600 MT/s** (not the pre-fix
/// 7200). The anchor is generation-level: it pins the half-step
/// formula on a confirmed word; the formula's invariance across all
/// five 11th-gen+ gens is pinned separately below.
#[test]
fn decode_mclk_11th_gen_plus_half_step_anchor() {
    let mclk = decode_mclk(Some(0x79E0_0124), IntelGen::AlderLake);
    assert_eq!(mclk, Section::Value(1800.0));
    assert_eq!(
        mclk.value().copied().map(|v| v * 2.0),
        Some(3600.0),
        "DDR: two transfers per DRAM clock = 3600 MT/s"
    );
}

/// The Alder family-representative word (the documented 12600KF
/// fixture 0x79E00124) through the full [`decode`] dispatch (Alder
/// Lake, the 2-channel DDR4 default): every decoded subchannel
/// carries 1800 MHz MCLK, gear One, UCLK 1800 — the reporters'
/// × 2 yields 3600 MT/s end to end. Generation-level: the Alder
/// family is the hardware-confirmed anchor for the Tier-3 decode
/// path; the sibling 11th-gen+ gens share the same MCLK formula
/// (pinned below).
#[test]
fn decode_alder_family_representative_word_end_to_end() {
    let regs = IntelImcRegs {
        mcbios_req: Some(0x79E0_0124),
        mad_inter_channel: Some(0),
        ..IntelImcRegs::default()
    };
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "Alder Lake DDR4: two subchannels");
    for ch in &ro.channels {
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1800.0));
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(1800.0));
    }
}

// -----------------------------------------------------------------
// Cross-generation MCLK anchors: the half-step formula is
// generation-invariant across the whole 11th-gen+ set, and the
// Tier-1 full-step formula is unchanged as the control.
// -----------------------------------------------------------------

/// The 11th-gen-and-later half-step MCLK formula (`ratio × refclk /
/// 2`) is **generation-invariant** across the whole half-step set
/// {Rocket, Alder, Raptor, Meteor, Arrow}: one representative
/// in-band `MC_BIOS_REQ` word per gen, each decoding to
/// `ratio × refclk / 2`.
#[test]
fn decode_mclk_11th_gen_plus_half_step_is_generation_invariant() {
    let cases = [
        (IntelGen::RocketLake, 0x0000_0120),
        (IntelGen::AlderLake, 0x79E0_0124),
        (IntelGen::RaptorLake, 0x0001_012C),
        (IntelGen::MeteorLake, 0x0002_0130),
        (IntelGen::ArrowLake, 0x0003_0134),
    ];
    for (gen, word) in cases {
        let ratio = f64::from(word & 0xFF);
        let refclk = if word & 0x0100 != 0 {
            REF_CLK_100_MHZ
        } else {
            REF_CLK_133_MHZ
        };
        let expected = ratio * refclk / 2.0;
        assert_eq!(
            decode_mclk(Some(word), gen),
            Section::Value(expected),
            "{gen:?} {word:#010x}: half-step ratio × refclk / 2 = {expected}"
        );
    }
}

/// The Tier-1 (Skylake–Comet Lake, 6th–10th gen) **full-step**
/// MCLK formula (`ratio × refclk` — the legacy physical-clock
/// multiplier, no halving) is unchanged across the set {Skylake,
/// Kaby, Coffee, Comet}: the control proving the 11th-gen+
/// half-step restructure did not leak into the legacy generations.
#[test]
fn decode_mclk_tier1_full_step_is_unchanged() {
    let cases = [
        (IntelGen::Skylake, 0x0000_0012),
        (IntelGen::KabyLake, 0x0000_0012),
        (IntelGen::CoffeeLake, 0x0000_0010),
        (IntelGen::CometLake, 0x0000_0010),
    ];
    for (gen, word) in cases {
        let ratio = f64::from(word & 0xFF);
        let refclk = if word & 0x0100 != 0 {
            REF_CLK_100_MHZ
        } else {
            REF_CLK_133_MHZ
        };
        let expected = ratio * refclk;
        assert_eq!(
            decode_mclk(Some(word), gen),
            Section::Value(expected),
            "{gen:?} {word:#010x}: full-step ratio × refclk = {expected}"
        );
    }
}

// -----------------------------------------------------------------
// Gear + UCLK decoders (IG-11): the Breakdown §3D gear map and the
// §4 worked examples A–D pinned end to end (raw word → gear → uclk).
// -----------------------------------------------------------------

/// The §3D gear map over all three caps: 00b → One everywhere;
/// 01b → Two (Gear2/Gear4); 10b → Four (Gear4 only — over-cap on
/// the Rocket Gear2 cap and on Tier-1 no-gear, documented); 11b →
/// Two (the bit-16 rule — the pinned example D reading). Junk in
/// the other bits never affects the decode.
#[test]
fn mcbios_gear_maps_the_3d_field() {
    for cap in [GearCap::None, GearCap::Gear2, GearCap::Gear4] {
        assert_eq!(
            mcbios_gear(0x0000_0000, cap),
            Some(GearMode::One),
            "00b, {cap:?}"
        );
        assert_eq!(
            mcbios_gear(0x0112, cap),
            Some(GearMode::One),
            "00b + junk bits, {cap:?}"
        );
    }
    assert_eq!(
        mcbios_gear(0x0001_0000, GearCap::None),
        None,
        "01b over-cap (Tier 1)"
    );
    assert_eq!(
        mcbios_gear(0x0001_0000, GearCap::Gear2),
        Some(GearMode::Two)
    );
    assert_eq!(
        mcbios_gear(0x0001_0000, GearCap::Gear4),
        Some(GearMode::Two)
    );
    assert_eq!(
        mcbios_gear(0x0002_0000, GearCap::None),
        None,
        "10b over-cap (Tier 1)"
    );
    assert_eq!(
        mcbios_gear(0x0002_0000, GearCap::Gear2),
        None,
        "10b over-cap (Rocket, documented)"
    );
    assert_eq!(
        mcbios_gear(0x0002_0000, GearCap::Gear4),
        Some(GearMode::Four)
    );
    assert_eq!(
        mcbios_gear(0x0003_0000, GearCap::None),
        None,
        "11b over-cap (Tier 1)"
    );
    assert_eq!(
        mcbios_gear(0x0003_0000, GearCap::Gear2),
        Some(GearMode::Two),
        "11b (Gear2)"
    );
    assert_eq!(
        mcbios_gear(0x0003_0000, GearCap::Gear4),
        Some(GearMode::Two),
        "11b (Gear4)"
    );
}

/// Breakdown §4 example A, re-pinned per the Alder Lake
/// MC_BIOS_REQ decode: DDR4-3200, gear 1 on a 100 MHz base on an
/// 11th-gen-and-later IMC (Rocket Lake).
#[test]
fn breakdown_example_a_gear1_100_mhz_base() {
    let raw = 0x0000_0120;
    assert_eq!(mcbios_gear(raw, GearCap::Gear2), Some(GearMode::One));
    let mclk = decode_mclk(Some(raw), IntelGen::RocketLake)
        .value()
        .copied()
        .expect("ratio 32 @ 100 MHz in-band");
    let uclk = decode_uclk(mclk, &Section::Value(GearMode::One));
    assert!((mclk - 1600.0).abs() < 0.01, "mclk = 1600 MHz, got {mclk}");
    assert!(
        (uclk.value().copied().unwrap_or(0.0) - 1600.0).abs() < 0.01,
        "uclk = mclk"
    );
}

/// Breakdown §4 example B, re-pinned per the Alder Lake
/// MC_BIOS_REQ decode: DDR4-3200, gear 1 on a 133.3333 MHz base.
#[test]
fn breakdown_example_b_gear1_133_mhz_base() {
    let raw = 0x0000_0018;
    assert_eq!(mcbios_gear(raw, GearCap::Gear2), Some(GearMode::One));
    let mclk = decode_mclk(Some(raw), IntelGen::RocketLake)
        .value()
        .copied()
        .expect("ratio 24 @ 133.3333 in-band");
    let uclk = decode_uclk(mclk, &Section::Value(GearMode::One));
    assert!((mclk - 1600.0).abs() < 0.01, "mclk = 1600 MHz, got {mclk}");
    assert!(
        (uclk.value().copied().unwrap_or(0.0) - 1600.0).abs() < 0.01,
        "uclk = mclk"
    );
}

/// Breakdown §4 example C, re-pinned per the Alder Lake
/// MC_BIOS_REQ decode: DDR5-4800, gear 2 on a 133.3333 MHz base.
#[test]
fn breakdown_example_c_gear2_133_mhz_base() {
    let raw = 0x0001_0024;
    assert_eq!(mcbios_gear(raw, GearCap::Gear4), Some(GearMode::Two));
    let mclk = decode_mclk(Some(raw), IntelGen::AlderLake)
        .value()
        .copied()
        .expect("ratio 36 @ 133.3333 in-band");
    let uclk = decode_uclk(mclk, &Section::Value(GearMode::Two));
    assert!((mclk - 2400.0).abs() < 0.01, "mclk = 2400 MHz, got {mclk}");
    assert!(
        (uclk.value().copied().unwrap_or(0.0) - 1200.0).abs() < 0.01,
        "uclk = mclk / 2"
    );
}

/// Breakdown §4 example D, pinned raw word `mcbios_req =
/// 0x0003001E`, re-pinned per the Alder Lake MC_BIOS_REQ decode.
#[test]
fn breakdown_example_d_gear2_11b_field() {
    let raw = 0x0003_001E;
    assert_eq!(
        mcbios_gear(raw, GearCap::Gear4),
        Some(GearMode::Two),
        "11b → gear 2"
    );
    let mclk = decode_mclk(Some(raw), IntelGen::AlderLake)
        .value()
        .copied()
        .expect("ratio 30 @ 133.3333 in-band");
    let uclk = decode_uclk(mclk, &Section::Value(GearMode::Two));
    assert!((mclk - 2000.0).abs() < 0.01, "mclk = 2000 MHz, got {mclk}");
    assert!(
        (mclk / 2.0 - uclk.value().copied().unwrap_or(0.0)).abs() < 0.01,
        "uclk = mclk / 2"
    );
    assert!(
        (uclk.value().copied().unwrap_or(0.0) - 1000.0).abs() < 0.01,
        "uclk = 1000 MHz"
    );
}

/// Breakdown §4 example D narrative register settings (DDR5-6000,
/// gear 2 on a 100 MHz base).
#[test]
fn breakdown_example_d_narrative_100_mhz_base() {
    let raw = 0x0003_013C;
    assert_eq!(mcbios_gear(raw, GearCap::Gear4), Some(GearMode::Two));
    let mclk = decode_mclk(Some(raw), IntelGen::AlderLake)
        .value()
        .copied()
        .expect("ratio 60 @ 100 MHz in-band");
    let uclk = decode_uclk(mclk, &Section::Value(GearMode::Two));
    assert!((mclk - 3000.0).abs() < 0.01, "mclk = 3000 MHz, got {mclk}");
    assert!(
        (uclk.value().copied().unwrap_or(0.0) - 1500.0).abs() < 0.01,
        "uclk = mclk / 2 = 1500 MHz"
    );
}

/// `decode_uclk` containment + the gear-divisor semantics.
#[test]
fn decode_uclk_divides_and_degrades() {
    assert_eq!(
        decode_uclk(2400.0, &Section::Value(GearMode::One)),
        Section::Value(2400.0)
    );
    assert_eq!(
        decode_uclk(2400.0, &Section::Value(GearMode::Two)),
        Section::Value(1200.0)
    );
    assert_eq!(
        decode_uclk(2400.0, &Section::Value(GearMode::Four)),
        Section::Value(600.0)
    );
    let na = Section::na(NaReason::ParseError("reserved gear encoding".to_owned()));
    let uclk = decode_uclk(2400.0, &na);
    assert!(matches!(uclk, Section::Na(NaReason::ParseError(_))));
    // 1.0 / 4 = 0.25 MHz is outside the [1, 5333] band.
    let uclk = decode_uclk(1.0, &Section::Value(GearMode::Four));
    assert!(matches!(uclk, Section::Na(NaReason::ParseError(_))));
}

// -----------------------------------------------------------------
// The 4-bit tCKE field + the 11-bit tRFC bound.
// -----------------------------------------------------------------

/// IG-22 containment: the Tier-3 extension fields all default
/// to `None`.
#[test]
fn ig22_extended_fields_default_to_all_none() {
    let regs = IntelImcRegs::default();
    assert_eq!(regs.ch2, ChannelRegs::default());
    assert_eq!(regs.ch3, ChannelRegs::default());
    assert_eq!(regs.mcl0, MclRegs::default());
    assert_eq!(regs.mcl1, MclRegs::default());
    assert!(regs.mad_dimm_ch2.is_none());
    assert!(regs.mad_dimm_ch3.is_none());
}

// -----------------------------------------------------------------
// The wide `/dev/mem` Tier-3 offset set (IG-25).
// -----------------------------------------------------------------

/// The wide `/dev/mem` Tier-3 offset set (IG-25): the 256 KiB
/// window constant matches the Alder profile window (IG-03,
/// `window_size` from `profile_for`) and every Tier-3 offset fits it.
#[test]
fn devmem_wide_map_is_256_kib_for_tier3() {
    assert_eq!(
        MCHBAR_WINDOW_TIER3, 0x40000,
        "the Alder profile window (IG-03)"
    );
    assert_eq!(
        MCHBAR_WINDOW_TIER3,
        profile_for(IntelGen::AlderLake).unwrap().window_size,
        "window size from profile_for"
    );
    assert_eq!(
        MCHBAR_WINDOW_TIER3,
        profile_for(IntelGen::MeteorLake).unwrap().window_size
    );
    for off in [
        MAX_MCL_OFFSET,
        MAX_MAD_DIMM_OFFSET,
        MAX_IMC_OFFSET,
        MAX_CHANNEL_OFFSET,
    ] {
        assert!(
            off <= MCHBAR_WINDOW_TIER3,
            "offset {off:#x} must fit the 256 KiB Tier-3 window"
        );
    }
    for map in ALDER_CHANNELS {
        assert!(
            map.mirror_base as usize + 4 <= MCHBAR_WINDOW_TIER3,
            "mirror base {:#x}",
            map.mirror_base
        );
    }
}

// -----------------------------------------------------------------
// The /dev/mem reader cores (IG-26 fix): `from_bar` /
// `from_bar_wide` over an abstract register reader.
// -----------------------------------------------------------------

/// The wide `/dev/mem` read set (IG-25 + the IG-26 wiring).
#[test]
fn from_bar_wide_reads_the_full_alder_offset_set() {
    let mut buf = vec![0u8; MCHBAR_WINDOW_TIER3];
    seed_word(&mut buf, MC_BIOS_REQ_OFFSET, 0x0001_0012);
    let ch_offsets = [
        TC_DBP_OFFSET,
        TC_RAP_OFFSET,
        TC_RFP_OFFSET,
        TC_RAP2_OFFSET,
        TC_RDRD_OFFSET,
        TC_RDWR_OFFSET,
        TC_WRRD_OFFSET,
        TC_WRWR_OFFSET,
    ];
    for (i, base) in [CHANNEL0_BASE, CHANNEL1_BASE, 0x4800, 0x4C00]
        .into_iter()
        .enumerate()
    {
        for (j, off) in ch_offsets.iter().enumerate() {
            seed_word(
                &mut buf,
                base + off,
                0xA000_0000 | (i as u32) << 12 | (j as u32) << 8,
            );
        }
    }
    let mcl_offsets = [
        MCL_TC_PRE_OFFSET,
        MCL_TC_ACT_OFFSET,
        MCL_TC_ACT2_OFFSET,
        MCL_TC_WTR_OFFSET,
        MCL_TC_RFP_OFFSET,
        MCL_TC_RFP2_OFFSET,
        MCL_TC_RDRD_OFFSET,
        MCL_TC_WRWR_OFFSET,
    ];
    for (i, base) in [MCL0_BASE, MCL1_BASE].into_iter().enumerate() {
        for (j, off) in mcl_offsets.iter().enumerate() {
            seed_word(
                &mut buf,
                base + off,
                0xB000_0000 | (i as u32) << 12 | (j as u32) << 8,
            );
        }
    }
    let gen = IntelGen::AlderLake;
    seed_word(&mut buf, imc_base_for(gen) + MAD_DIMM_CH2_INTRA, 1);
    seed_word(&mut buf, imc_base_for(gen) + MAD_DIMM_CH3_INTRA, 2);
    let alk_d4_offsets = [
        ALK_D4_TC_PRE,
        ALK_D4_TC_PRE_HI,
        ALK_D4_TC_ACT,
        ALK_D4_TC_RDRD,
        ALK_D4_TC_RDWR,
        ALK_D4_TC_WRRD,
        ALK_D4_TC_WRWR,
        ALK_D4_TC_ODT,
        ALK_D4_TC_RFP,
        ALK_D4_TC_RFTP,
    ];
    for (i, base) in [ALK_D4_TC0_BASE, ALK_D4_TC1_BASE].into_iter().enumerate() {
        for (j, off) in alk_d4_offsets.iter().enumerate() {
            seed_word(
                &mut buf,
                base + off,
                0xD000_0000 | (i as u32) << 12 | (j as u32) << 8,
            );
        }
    }

    let touched = std::cell::RefCell::new(Vec::new());
    let regs = IntelImcRegs::from_wide_reader(imc_base_for(gen), buffered_reader(buf, &touched));

    let expect_ch = |i: u32| {
        let w = |j: u32| 0xA000_0000 | (i << 12) | (j << 8);
        ChannelRegs {
            tc_dbp: Some(w(0)),
            tc_rap: Some(w(1)),
            tc_rfp: Some(w(2)),
            tc_rap2: Some(w(3)),
            tc_rdrd: Some(w(4)),
            tc_rdwr: Some(w(5)),
            tc_wrrd: Some(w(6)),
            tc_wrwr: Some(w(7)),
        }
    };
    let expect_mcl = |i: u32| {
        let w = |j: u32| 0xB000_0000 | (i << 12) | (j << 8);
        MclRegs {
            tc_pre: Some(w(0)),
            tc_act: Some(w(1)),
            tc_act2: Some(w(2)),
            tc_wtr: Some(w(3)),
            tc_rfp: Some(w(4)),
            tc_rfp2: Some(w(5)),
            tc_rdrd: Some(w(6)),
            tc_wrwr: Some(w(7)),
        }
    };
    let expect_alk_d4 = |i: u32| {
        let w = |j: u32| 0xD000_0000 | (i << 12) | (j << 8);
        AlderDdr4TcRegs {
            tc_pre_lo: Some(w(0)),
            tc_pre_hi: Some(w(1)),
            tc_act: Some(w(2)),
            tc_rdrd: Some(w(3)),
            tc_rdwr: Some(w(4)),
            tc_wrrd: Some(w(5)),
            tc_wrwr: Some(w(6)),
            tc_odt: Some(w(7)),
            tc_rfp: Some(w(8)),
            tc_rftp: Some(w(9)),
        }
    };
    assert_eq!(
        regs,
        IntelImcRegs {
            mcbios_req: Some(0x0001_0012),
            ch0: expect_ch(0),
            ch1: expect_ch(1),
            ch2: expect_ch(2),
            ch3: expect_ch(3),
            mcl0: expect_mcl(0),
            mcl1: expect_mcl(1),
            mad_inter_channel: None,
            mad_dimm_ch0: None,
            mad_dimm_ch1: None,
            mad_dimm_ch2: Some(1),
            mad_dimm_ch3: Some(2),
            alk_d4_tc0: expect_alk_d4(0),
            alk_d4_tc1: expect_alk_d4(1),
        },
        "the 256 KiB map populates the whole Tier-3 set"
    );

    let mut set = touched.into_inner();
    set.sort_unstable();
    set.dedup();
    assert_eq!(
        set.len(),
        71,
        "exactly the full 71-slot Alder read set (51 + 20 DDR4 TC)"
    );
    for &off in &set {
        assert!(
            off + 4 <= MCHBAR_WINDOW_TIER3,
            "every read fits the 256 KiB wide window: {off:#x}"
        );
    }
    for base in [0x4800usize, 0x4C00] {
        assert!(
            (0..8).all(|j| set.contains(&(base + ch_offsets[j]))),
            "the ch2/ch3 mirror block @ {base:#x} is fully read"
        );
    }
    for base in [MCL0_BASE, MCL1_BASE] {
        assert!(
            (0..8).all(|j| set.contains(&(base + mcl_offsets[j]))),
            "the MCL block @ {base:#x} is fully read"
        );
    }
    for base in [ALK_D4_TC0_BASE, ALK_D4_TC1_BASE] {
        assert!(
            (0..10).all(|j| set.contains(&(base + alk_d4_offsets[j]))),
            "the DDR4 TC block @ {base:#x} is fully read (Phase 1)"
        );
    }
    assert!(
        set.contains(&(imc_base_for(gen) + MAD_DIMM_CH2_INTRA))
            && set.contains(&(imc_base_for(gen) + MAD_DIMM_CH3_INTRA)),
        "the gen-keyed MAD_DIMM_CH2/CH3 raws are read"
    );
}

/// The 64 KiB `/dev/mem` read set is unchanged by the IG-26
/// wiring.
#[test]
fn from_bar_keeps_the_64_kib_nine_register_set() {
    let mut buf = vec![0u8; MCHBAR_WINDOW];
    seed_word(&mut buf, MC_BIOS_REQ_OFFSET, 0x0000_0012);
    let ch_offsets = [
        TC_DBP_OFFSET,
        TC_RAP_OFFSET,
        TC_RFP_OFFSET,
        TC_RAP2_OFFSET,
        TC_RDRD_OFFSET,
        TC_RDWR_OFFSET,
        TC_WRRD_OFFSET,
        TC_WRWR_OFFSET,
    ];
    for (i, base) in [CHANNEL0_BASE, CHANNEL1_BASE].into_iter().enumerate() {
        for (j, off) in ch_offsets.iter().enumerate() {
            seed_word(
                &mut buf,
                base + off,
                0xC000_0000 | (i as u32) << 12 | (j as u32) << 8,
            );
        }
    }
    seed_word(&mut buf, MAD_DIMM_CH2_OFFSET, 0xDEAD_BEEF);
    seed_word(&mut buf, MAD_DIMM_CH3_OFFSET, 0xDEAD_BEEF);

    let touched = std::cell::RefCell::new(Vec::new());
    let regs = IntelImcRegs::from_reader(buffered_reader(buf, &touched));

    let expect_ch = |i: u32| {
        let w = |j: u32| 0xC000_0000 | (i << 12) | (j << 8);
        ChannelRegs {
            tc_dbp: Some(w(0)),
            tc_rap: Some(w(1)),
            tc_rfp: Some(w(2)),
            tc_rap2: Some(w(3)),
            tc_rdrd: Some(w(4)),
            tc_rdwr: Some(w(5)),
            tc_wrrd: Some(w(6)),
            tc_wrwr: Some(w(7)),
        }
    };
    assert_eq!(
        regs,
        IntelImcRegs {
            mcbios_req: Some(0x0000_0012),
            ch0: expect_ch(0),
            ch1: expect_ch(1),
            ..IntelImcRegs::default()
        },
        "from_bar = the 9-register Tier-1 set only; every Tier-3 field stays None"
    );

    let mut set = touched.into_inner();
    set.sort_unstable();
    set.dedup();
    assert_eq!(
        set.len(),
        17,
        "exactly the 9-register Tier-1 read set (1 global + 2 x 8)"
    );
    for &off in &set {
        assert!(
            off + 4 <= MCHBAR_WINDOW,
            "every read fits the 64 KiB Tier-1 window: {off:#x}"
        );
    }
    assert!(
        !set.iter().any(|&off| off >= MCHBAR_WINDOW),
        "no read crosses 64 KiB"
    );
}

/// Containment of the wide read set on a map shorter than the
/// highest wide offsets.
#[test]
fn from_bar_wide_on_a_short_map_contains_the_wide_offsets() {
    const SHORT_WINDOW: usize = 1 << 15;
    let mut buf = vec![0u8; SHORT_WINDOW];
    seed_word(&mut buf, MC_BIOS_REQ_OFFSET, 0x0001_0012);
    seed_word(&mut buf, MAD_DIMM_CH2_OFFSET, 7);
    seed_word(&mut buf, MAD_DIMM_CH3_OFFSET, 9);
    seed_word(&mut buf, 0x4800 + TC_DBP_OFFSET, 0x284D_2828);

    let gen = IntelGen::Skylake;
    let touched = std::cell::RefCell::new(Vec::new());
    let regs = IntelImcRegs::from_wide_reader(imc_base_for(gen), buffered_reader(buf, &touched));

    assert_eq!(regs.mcbios_req, Some(0x0001_0012));
    assert_eq!(regs.mad_dimm_ch2, Some(7));
    assert_eq!(regs.mad_dimm_ch3, Some(9));
    assert_eq!(regs.ch2.tc_dbp, Some(0x284D_2828));
    assert_eq!(regs.mcl0, MclRegs::default());
    assert_eq!(regs.mcl1, MclRegs::default());
    let mut set = touched.into_inner();
    set.sort_unstable();
    set.dedup();
    assert!(
        set.iter().any(|&off| off >= SHORT_WINDOW),
        "the MCL offsets were attempted on the short map"
    );
}

// -----------------------------------------------------------------
// The Tier-3 decode dispatch (IG-26).
// -----------------------------------------------------------------

/// OQ-11's DDR5 condition met → 4-subchannel geometry.
#[test]
fn decode_tier3_ddr5_condition_exposes_four_subchannels() {
    for gen in [
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
    ] {
        let mut regs = tier3_ddr5_condition_regs();
        regs.mad_inter_channel = Some(0);
        let ro = decode(&regs, gen, &SmbiosTopology::empty());
        assert_eq!(
            ro.channels.len(),
            4,
            "{gen:?}: DDR5 condition met -> 4 subchannels"
        );
        assert_eq!(
            ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3],
            "{gen:?}"
        );
        assert_eq!(ro.channel_mode, None, "{gen:?}: no population evidence");
        for ch in &ro.channels {
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?}");
            assert_eq!(
                ch.clocks.gear_mode,
                Section::Value(GearMode::Two),
                "{gen:?}"
            );
            assert_eq!(
                ch.clocks.uclk_mhz,
                Section::Value(1200.0),
                "{gen:?}: uclk = mclk / 2"
            );
            assert_eq!(ch.timings.cl, Section::Value(40), "{gen:?}");
            assert_eq!(ch.timings.cwl, Section::Value(40), "{gen:?}");
            assert_eq!(ch.timings.rc, Section::Value(117), "{gen:?}");
            assert_eq!(ch.timings.rdrd_scl, Section::Value(6), "{gen:?}");
            assert_eq!(ch.timings.wrwr_scl, Section::Value(6), "{gen:?}");
            assert!(
                matches!(ch.timings.rfc1, Section::Na(NaReason::ParseError(_))),
                "{gen:?}"
            );
            assert!(
                matches!(ch.timings.rfcsb, Section::Na(NaReason::ParseError(_))),
                "{gen:?}"
            );
        }
    }
}

/// OQ-11's DDR5 condition NOT met → 2-channel DDR4 default.
#[test]
fn decode_tier3_ddr4_default_exposes_two_channels() {
    for gen in [
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
    ] {
        for (label, mad2, mad3) in [
            ("raws absent", None, None),
            ("raws zero", Some(0u32), Some(0u32)),
        ] {
            let mut regs = tier3_ddr5_condition_regs();
            regs.mad_dimm_ch2 = mad2;
            regs.mad_dimm_ch3 = mad3;
            regs.mad_inter_channel = Some(1);
            let ro = decode(&regs, gen, &SmbiosTopology::empty());
            assert_eq!(ro.channels.len(), 2, "{gen:?} ({label}): DDR4 default");
            assert_eq!(
                ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
                vec![0, 1],
                "{gen:?} ({label}): sequential display indices"
            );
            assert_eq!(
                ro.channel_mode, None,
                "{gen:?} ({label}): no population evidence"
            );
            for ch in &ro.channels {
                assert_eq!(
                    ch.clocks.mclk_mhz,
                    Section::Value(2400.0),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.clocks.gear_mode,
                    Section::Value(GearMode::Two),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.clocks.uclk_mhz,
                    Section::Value(1200.0),
                    "{gen:?} ({label})"
                );
                assert_eq!(ch.timings.cl, Section::Value(40), "{gen:?} ({label})");
            }
        }
    }
}

/// Gear 4 wires the `GearCap::Gear4` cap into the Tier-3 dispatch.
#[test]
fn decode_tier3_gear4_uclk_is_mclk_over_four() {
    let mut regs = tier3_ddr5_condition_regs();
    regs.mcbios_req = Some(0x0002_0024);
    for gen in [
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
    ] {
        let ro = decode(&regs, gen, &SmbiosTopology::empty());
        for ch in &ro.channels {
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?}");
            assert_eq!(
                ch.clocks.gear_mode,
                Section::Value(GearMode::Four),
                "{gen:?}"
            );
            assert_eq!(
                ch.clocks.uclk_mhz,
                Section::Value(600.0),
                "{gen:?}: uclk = mclk / 4"
            );
        }
    }
}

/// The 4-subchannel Tier-3 readout round-trips through bincode.
#[test]
fn decode_tier3_four_channel_readout_bincode_round_trip() {
    let mut regs = tier3_ddr5_condition_regs();
    regs.mad_inter_channel = Some(0);
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 4, "the 4-subchannel DDR5 shape");
    let bytes = bincode::serialize(&ro).expect("IntelReadout must serialize (no-panic contract)");
    let back: IntelReadout = bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
    assert_eq!(ro, back);
}

// -----------------------------------------------------------------
// Tier 3 acceptance fixtures (IG-27).
// -----------------------------------------------------------------

/// Breakdown §4 Example C, pinned under AlderLake through the full
/// Tier-3 `decode()` dispatch.
#[test]
fn tier3_acceptance_example_c_pinned_under_alderlake() {
    assert_eq!(
        tier3_ddr5_condition_regs().mcbios_req,
        Some(0x0001_0024),
        "Example C register settings: 133.3333 MHz base, ratio 36, Gear 2"
    );
    let ro = decode(
        &tier3_ddr5_condition_regs(),
        IntelGen::AlderLake,
        &SmbiosTopology::empty(),
    );
    assert_eq!(ro.channels.len(), 4, "DDR5 condition met -> 4 subchannels");
    for ch in &ro.channels {
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "MCLK 2400 MHz");
        assert_eq!(
            ch.clocks.mclk_mhz.value().copied().map(|v| v * 2.0),
            Some(4800.0),
            "4800 MT/s"
        );
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Two), "Gear 2");
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "uclk 1200 MHz");
    }
}

/// The DDR5 widened-field acceptance raws through the MCL fallback arm.
#[test]
fn tier3_acceptance_widened_fields_via_mcl_fallback() {
    let regs = IntelImcRegs {
        mcbios_req: Some(0x0001_0024),
        ch0: ChannelRegs {
            tc_rap: Some(0xFFFF_FFFF),
            ..ChannelRegs::default()
        },
        mcl0: MclRegs {
            tc_act: Some(0x0028_8410),
            tc_rfp: Some(0x0000_04B0),
            tc_rfp2: Some(0x0000_0087),
            ..MclRegs::default()
        },
        ..IntelImcRegs::default()
    };
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(
        ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
        vec![0, 1],
        "DDR4 default geometry (no MAD raws) — M-7: sequential display indices"
    );
    let ch0 = &ro.channels[0];
    assert_eq!(
        ch0.timings.cl,
        Section::Value(40),
        "tCL 40 in the 8-bit field, 0xD004 winning over the disabled 0x4004"
    );
    assert_eq!(
        ch0.timings.faw,
        Section::Value(16),
        "tFAW 16 in the 8-bit field"
    );
    assert_eq!(
        ch0.timings.rfc1,
        Section::Value(1200),
        "tRFC1 1200 in the 12-bit field"
    );
    assert_eq!(
        ch0.timings.rfcsb,
        Section::Value(135),
        "tRFCsb 135 in the 11-bit field -> the rfcsb slot"
    );
    assert!(
        matches!(ch0.timings.rcdrd, Section::Na(NaReason::ParseError(_))),
        "tc_dbp absent -> tRCD Na"
    );
}

/// The widened-width degradation arms through the full dispatch.
#[test]
fn tier3_acceptance_widened_zero_and_reserved_degrade() {
    let zero_mcl = MclRegs {
        tc_pre: Some(0),
        tc_act: Some(0),
        tc_act2: Some(0),
        tc_wtr: Some(0),
        tc_rfp: Some(0),
        tc_rfp2: Some(0),
        tc_rdrd: Some(0),
        tc_wrwr: Some(0),
    };
    let regs = IntelImcRegs {
        mcbios_req: Some(0x0001_0024),
        ch0: ChannelRegs {
            tc_dbp: Some(0xFFFF_FFFF),
            tc_rap: Some(0xFFFF_FFFF),
            tc_rfp: Some(0xFFFF_FFFF),
            tc_rdrd: Some(0xFFFF_FFFF),
            tc_wrrd: Some(0xFFFF_FFFF),
            ..ChannelRegs::default()
        },
        ch1: ChannelRegs {
            tc_rdrd: Some(0xFFFF_FFFF),
            ..ChannelRegs::default()
        },
        ch3: ChannelRegs {
            tc_rdrd: Some(0xFFFF_FFFF),
            ..ChannelRegs::default()
        },
        mcl0: zero_mcl,
        mad_dimm_ch2: Some(1),
        mad_dimm_ch3: Some(1),
        ..IntelImcRegs::default()
    };
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 4, "DDR5 condition met -> 4 subchannels");
    assert_eq!(
        ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    for ch in &ro.channels {
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0));
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0));
    }
    fn assert_all_sourced_na(ch: &IntelChannel, why: &str) {
        let cells: [&Section<u16>; 18] = [
            &ch.timings.cl,
            &ch.timings.rcwdwr,
            &ch.timings.rcdrd,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.wtrs,
            &ch.timings.wtrl,
            &ch.timings.wr,
            &ch.timings.rfc1,
            &ch.timings.rfcsb,
            &ch.timings.cwl,
            &ch.timings.rtp,
            &ch.timings.rdrd_scl,
            &ch.timings.rdrd_sc,
            &ch.timings.rdrd_sd,
            &ch.timings.rdrd_dd,
            &ch.timings.wrwr_scl,
            &ch.timings.wrwr_sc,
        ];
        for s in &cells {
            assert!(s.is_na(), "{why}: {s:?}");
        }
    }
    assert_all_sourced_na(
        &ro.channels[0],
        "subch0: zero MCL words win (mirrors disabled)",
    );
    assert_all_sourced_na(
        &ro.channels[1],
        "subch1: mirror-only, reserved TC_RDRD word",
    );
    assert_all_sourced_na(&ro.channels[2], "subch2: all registers absent");
    assert_all_sourced_na(
        &ro.channels[3],
        "subch3: mirror-only, reserved TC_RDRD word",
    );
    assert_eq!(
        ro.channels[0].timings.rc,
        Section::na(NaReason::NotApplicable)
    );
}

// -----------------------------------------------------------------
// OQ-5 tile-routing probe (IG-28): Meteor / Arrow Lake.
// -----------------------------------------------------------------

/// All-degenerate primary-window raws on Meteor / Arrow Lake →
/// the all-`Na(UnsupportedHardware)` readout.
#[test]
fn ig28_all_degenerate_window_degrades_all_na_with_tile_detail() {
    type Fill = fn(usize) -> Option<u32>;
    let fills: [(&str, Fill); 3] = [
        ("all-zero raws", |_off: usize| Some(0u32)),
        ("all-bus-fault raws", |_off: usize| Some(0xFFFF_FFFFu32)),
        ("all-absent raws", |_off: usize| None),
    ];
    for gen in [IntelGen::MeteorLake, IntelGen::ArrowLake] {
        for (label, fill) in fills {
            let regs = IntelImcRegs::from_wide_reader(imc_base_for(gen), fill);
            assert!(regs.primary_window_degenerate(), "{gen:?} ({label})");
            let ro = decode(&regs, gen, &SmbiosTopology::empty());
            assert_eq!(
                ro.channels.len(),
                2,
                "{gen:?} ({label}) (C-4: dual-channel)"
            );
            for ch in &ro.channels {
                assert_eq!(
                    ch.clocks.mclk_mhz,
                    Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.clocks.gear_mode,
                    Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.clocks.uclk_mhz,
                    Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.timings.cl,
                    Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} ({label})"
                );
                assert_eq!(
                    ch.rtl,
                    Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} ({label})"
                );
            }
            assert_eq!(ro.channel_mode, None, "{gen:?} ({label})");
        }
        match tile_routing_suspected(gen) {
            TelemetryError::UnsupportedHardware { vendor } => {
                assert!(
                        vendor.contains(
                            "SoC-tile IMC routing suspected (MCHBAR + 0x180000 secondary range) — primary window empty; see OQ-5"
                        ),
                        "{gen:?}: {vendor}"
                    );
            }
            other => unreachable!("{gen:?}: expected UnsupportedHardware, got {other:?}"),
        }
    }
}

/// One valid primary-window register keeps the normal decode path.
#[test]
fn ig28_one_valid_register_keeps_the_normal_decode_path() {
    for gen in [IntelGen::MeteorLake, IntelGen::ArrowLake] {
        let regs = IntelImcRegs::from_wide_reader(imc_base_for(gen), |off: usize| {
            if off == MC_BIOS_REQ_OFFSET {
                Some(0x0001_0024)
            } else {
                Some(0u32)
            }
        });
        assert!(!regs.primary_window_degenerate(), "{gen:?}");
        let ro = decode(&regs, gen, &SmbiosTopology::empty());
        assert_eq!(
            ro.channels.len(),
            2,
            "{gen:?}: DDR4 default (MAD raws zero)"
        );
        for ch in &ro.channels {
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?}");
            assert_eq!(
                ch.clocks.gear_mode,
                Section::Value(GearMode::Two),
                "{gen:?}"
            );
            assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "{gen:?}");
            assert!(
                matches!(ch.timings.cl, Section::Na(NaReason::ParseError(_))),
                "{gen:?}: untrained zero field degrades per-register"
            );
        }
    }
}

// Degradation + containment (the no-panic contract, D5).
// -----------------------------------------------------------------

/// Every register absent (all-`None` raws, Tier 1) degrades the
/// whole channel to honest `Na` — no panic, no garbage values.
#[test]
fn decode_all_absent_degrades_without_panic() {
    let ro = decode(
        &IntelImcRegs::default(),
        IntelGen::Skylake,
        &SmbiosTopology::empty(),
    );
    for ch in &ro.channels {
        assert!(ch.clocks.mclk_mhz.is_na(), "{:?}", ch.clocks.mclk_mhz);
        for s in [&ch.clocks.uclk_mhz, &ch.clocks.fclk_mhz] {
            assert!(s.is_na(), "{s:?}");
        }
        assert!(ch.clocks.gdm.is_na(), "{:?}", ch.clocks.gdm);
        assert!(ch.clocks.pdm.is_na(), "{:?}", ch.clocks.pdm);
        assert!(ch.clocks.div_mode.is_na(), "{:?}", ch.clocks.div_mode);
        assert!(ch.clocks.gear_mode.is_na(), "{:?}", ch.clocks.gear_mode);
        assert!(
            ch.clocks.command_rate.is_na(),
            "{:?}",
            ch.clocks.command_rate
        );
        assert!(ch.rtl.is_na());
        let cells: [&Section<u16>; 27] = [
            &ch.timings.cl,
            &ch.timings.rcwdwr,
            &ch.timings.rcdrd,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.rc,
            &ch.timings.rrds,
            &ch.timings.rrld,
            &ch.timings.faw,
            &ch.timings.wtrs,
            &ch.timings.wtrl,
            &ch.timings.wr,
            &ch.timings.rfc1,
            &ch.timings.rfc2,
            &ch.timings.rfcsb,
            &ch.timings.cwl,
            &ch.timings.rtp,
            &ch.timings.rdwr,
            &ch.timings.wrrd,
            &ch.timings.rdrd_sd,
            &ch.timings.rdrd_dd,
            &ch.timings.rdrd_scl,
            &ch.timings.rdrd_sc,
            &ch.timings.wrwr_sd,
            &ch.timings.wrwr_dd,
            &ch.timings.wrwr_scl,
            &ch.timings.wrwr_sc,
        ];
        for s in &cells {
            assert!(s.is_na(), "{s:?}");
        }
        for s in [
            &ch.cad_bus.proc_odt,
            &ch.cad_bus.clk_drv,
            &ch.cad_bus.addr_cmd_drv,
            &ch.cad_bus.cs_odt_drv,
            &ch.cad_bus.cke_drv,
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }
        for s in [
            &ch.voltages.vddcr_soc_mv,
            &ch.voltages.vddio_mem_mv,
            &ch.voltages.vdd_misc_mv,
            &ch.voltages.vpp_mv,
            &ch.voltages.vcore_mv,
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }
    }
}

/// Per-register containment: only `ch0.tc_rap` unreadable.
#[test]
fn decode_per_register_containment() {
    let mut regs = acceptance_regs();
    regs.ch0.tc_rap = None;
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    let ch0 = &ro.channels[0];

    for s in [
        &ch0.timings.ras,
        &ch0.timings.rrds,
        &ch0.timings.rtp,
        &ch0.timings.faw,
    ] {
        assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
    }
    assert_eq!(ch0.timings.rc, Section::na(NaReason::NotApplicable));
    assert_eq!(ch0.clocks.mclk_mhz, Section::Value(2400.0));
    assert_eq!(ch0.timings.cl, Section::Value(17));
    assert_eq!(ch0.timings.cwl, Section::Value(15));
    assert_eq!(ch0.timings.rcdrd, Section::Value(17));
    assert_eq!(ch0.timings.rcwdwr, Section::Value(17));
    assert_eq!(ch0.timings.rp, Section::Value(17));
    assert_eq!(ch0.timings.rfc1, Section::Value(420));
    assert_eq!(ch0.timings.rrld, Section::Value(10));
    assert_eq!(ch0.timings.wr, Section::Value(12));
    assert_eq!(ch0.timings.rdwr, Section::Value(10));
    assert_eq!(ch0.timings.wtrs, Section::Value(12));
    assert_eq!(ch0.timings.wtrl, Section::Value(8));
    assert_eq!(ch0.timings.rdrd_dd, Section::Value(18));
    assert_eq!(ch0.timings.wrwr_dd, Section::Value(16));

    let ch1 = &ro.channels[1];
    assert_eq!(ch1.timings.ras, Section::Value(39));
    assert_eq!(ch1.timings.rc, Section::Value(56));
}

/// A decoded field of `0` is untrained → `Na(ParseError)` per field.
#[test]
fn decode_zero_fields_degrade_to_parse_error() {
    let mut regs = acceptance_regs();
    regs.ch0.tc_dbp = Some(0);
    regs.ch0.tc_rfp = Some(0);
    regs.ch1.tc_rap = Some(0);
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());

    let ch0 = &ro.channels[0];
    for s in [
        &ch0.timings.cl,
        &ch0.timings.cwl,
        &ch0.timings.rcdrd,
        &ch0.timings.rcwdwr,
        &ch0.timings.rp,
        &ch0.timings.rfc1,
    ] {
        assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
    }
    assert_eq!(ch0.timings.ras, Section::Value(39));
    assert_eq!(ch0.timings.rc, Section::na(NaReason::NotApplicable));

    let ch1 = &ro.channels[1];
    for s in [
        &ch1.timings.ras,
        &ch1.timings.rrds,
        &ch1.timings.rtp,
        &ch1.timings.faw,
    ] {
        assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
    }
    assert_eq!(ch1.timings.cl, Section::Value(17));
}

// -----------------------------------------------------------------
// The v1 Tier-1 generation gate.
// -----------------------------------------------------------------

/// Every *unprofiled* generation degrades the whole readout to
/// `Na(UnsupportedHardware)` channels.
#[test]
fn decode_non_tier1_gen_degrades_all_na() {
    let regs = acceptance_regs();
    for (gen, count) in [
        (IntelGen::JasperLake, 2u8),
        (IntelGen::LunarLake, 2),
        (IntelGen::CoreSeries1Mobile, 2),
        (IntelGen::Unrecognized, 2),
    ] {
        let ro = decode(&regs, gen, &SmbiosTopology::empty());
        assert_eq!(ro.channels.len(), usize::from(count), "{gen:?}");
        for (i, ch) in ro.channels.iter().enumerate() {
            assert_eq!(ch.index, i as u8, "{gen:?}");
            assert_eq!(
                ch.clocks.mclk_mhz,
                Section::na(NaReason::UnsupportedHardware),
                "{gen:?}"
            );
            assert_eq!(
                ch.rtl,
                Section::na(NaReason::UnsupportedHardware),
                "{gen:?}"
            );
            let cells: [&Section<u16>; 27] = [
                &ch.timings.cl,
                &ch.timings.rcwdwr,
                &ch.timings.rcdrd,
                &ch.timings.rp,
                &ch.timings.ras,
                &ch.timings.rc,
                &ch.timings.rrds,
                &ch.timings.rrld,
                &ch.timings.faw,
                &ch.timings.wtrs,
                &ch.timings.wtrl,
                &ch.timings.wr,
                &ch.timings.rfc1,
                &ch.timings.rfc2,
                &ch.timings.rfcsb,
                &ch.timings.cwl,
                &ch.timings.rtp,
                &ch.timings.rdwr,
                &ch.timings.wrrd,
                &ch.timings.rdrd_sd,
                &ch.timings.rdrd_dd,
                &ch.timings.rdrd_scl,
                &ch.timings.rdrd_sc,
                &ch.timings.wrwr_sd,
                &ch.timings.wrwr_dd,
                &ch.timings.wrwr_scl,
                &ch.timings.wrwr_sc,
            ];
            for s in &cells {
                assert_eq!(
                    *s,
                    &Section::na(NaReason::UnsupportedHardware),
                    "{gen:?} {s:?}"
                );
            }
        }
    }
}

/// [`tier1_gate`]: Tier 1 passes; every other generation yields
/// `UnsupportedHardware`.
#[test]
fn tier1_gate_dispatches_by_generation() {
    for gen in [
        IntelGen::Skylake,
        IntelGen::KabyLake,
        IntelGen::CoffeeLake,
        IntelGen::CometLake,
    ] {
        assert_eq!(tier1_gate(gen), Ok(()), "{gen:?}");
        assert!(tier1_supported(gen), "{gen:?}");
    }
    let ald = tier1_gate(IntelGen::AlderLake).unwrap_err();
    assert!(matches!(ald, TelemetryError::UnsupportedHardware { .. }));
    let ald_msg = ald.to_string();
    assert!(ald_msg.contains("AlderLake"), "{ald_msg}");
    assert!(ald_msg.contains("Tier 3"), "{ald_msg}");
    let met = tier1_gate(IntelGen::MeteorLake).unwrap_err();
    assert!(met.to_string().contains("Tier 3"), "{met}");
    let rock = tier1_gate(IntelGen::RocketLake).unwrap_err();
    assert!(rock.to_string().contains("RocketLake"), "{rock}");
    assert!(rock.to_string().contains("Tier 2"), "{rock}");
    let ice = tier1_gate(IntelGen::IceLake).unwrap_err();
    assert!(ice.to_string().contains("IceLake"), "{ice}");
    assert!(!tier1_supported(IntelGen::Unrecognized));
}

/// [`gen_gate`] (the facade's branch-level gate, IG-16).
#[test]
fn gen_gate_dispatches_by_generation() {
    for gen in [
        IntelGen::Skylake,
        IntelGen::KabyLake,
        IntelGen::CoffeeLake,
        IntelGen::CometLake,
        IntelGen::IceLake,
        IntelGen::TigerLake,
        IntelGen::Haswell,
        IntelGen::Broadwell,
        IntelGen::RocketLake,
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
        IntelGen::HaswellE,
        IntelGen::BroadwellE,
        IntelGen::SkylakeX,
        IntelGen::CascadeLake,
        IntelGen::SapphireRapids,
    ] {
        assert_eq!(gen_gate(gen), Ok(()), "{gen:?}");
    }
    for gen in [
        IntelGen::JasperLake,
        IntelGen::LunarLake,
        IntelGen::CoreSeries1Mobile,
        IntelGen::Unrecognized,
    ] {
        let msg = gen_gate(gen).unwrap_err().to_string();
        assert!(msg.contains(&format!("{gen:?}")), "{msg}");
        assert!(msg.contains("has no decode profile"), "{msg}");
        assert!(
            msg.contains("Tier 3 (Alder/Raptor/Meteor/Arrow Lake)"),
            "{msg}"
        );
        assert!(msg.contains("Server (Sapphire Rapids)"), "{msg}");
    }
}

// -----------------------------------------------------------------
// Profile dispatch (IG-12): Tier 1 is one case of the dispatcher.
// -----------------------------------------------------------------

/// [`gen_supported`] mirrors [`intel_gen::profile_for`].
#[test]
fn gen_supported_matches_profile_for() {
    for gen in [
        IntelGen::Skylake,
        IntelGen::KabyLake,
        IntelGen::CoffeeLake,
        IntelGen::CometLake,
        IntelGen::IceLake,
        IntelGen::TigerLake,
        IntelGen::Haswell,
        IntelGen::Broadwell,
        IntelGen::RocketLake,
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
        IntelGen::HaswellE,
        IntelGen::BroadwellE,
        IntelGen::SkylakeX,
        IntelGen::CascadeLake,
        IntelGen::SapphireRapids,
    ] {
        assert!(gen_supported(gen), "{gen:?}");
    }
    for gen in [
        IntelGen::JasperLake,
        IntelGen::LunarLake,
        IntelGen::CoreSeries1Mobile,
        IntelGen::Unrecognized,
    ] {
        assert!(!gen_supported(gen), "{gen:?}");
    }
}

/// Rocket Lake through [`decode`]: the shared 64 KiB map decodes
/// identically for the same ratio raws, and the gear cells layer on.
#[test]
fn decode_rocket_lake_is_tier1_map_plus_gear() {
    let tier1 = decode(
        &acceptance_regs(),
        IntelGen::Skylake,
        &SmbiosTopology::empty(),
    );
    let mut regs = acceptance_regs();
    regs.mcbios_req = Some(0x0001_0012);
    let rocket = decode(&regs, IntelGen::RocketLake, &SmbiosTopology::empty());
    assert_eq!(rocket.channels.len(), 2, "Rocket Lake: two channels");
    for (t, r) in tier1.channels.iter().zip(&rocket.channels) {
        assert_eq!(
            t.clocks.mclk_mhz,
            Section::Value(2400.0),
            "Tier 1: ratio 18 full-step"
        );
        assert_eq!(
            r.clocks.mclk_mhz,
            Section::Value(1200.0),
            "Rocket: ratio 18 half-step"
        );
        assert_eq!(t.clocks.fclk_mhz, r.clocks.fclk_mhz);
        assert_eq!(t.clocks.div_mode, r.clocks.div_mode);
        assert_eq!(t.clocks.gdm, r.clocks.gdm);
        assert_eq!(t.clocks.pdm, r.clocks.pdm);
        assert_eq!(t.clocks.command_rate, r.clocks.command_rate);
        assert_eq!(t.timings, r.timings, "all timings identical");
        assert_eq!(t.cad_bus, r.cad_bus);
        assert_eq!(t.voltages, r.voltages);
        assert_eq!(t.rtl, r.rtl);
        assert_eq!(t.clocks.gear_mode, Section::na(NaReason::NotApplicable));
        assert_eq!(t.clocks.uclk_mhz, Section::na(NaReason::NotApplicable));
        assert_eq!(r.clocks.gear_mode, Section::Value(GearMode::Two));
        assert_eq!(r.clocks.uclk_mhz, Section::Value(600.0), "uclk = 1200 / 2");
    }
}

/// Rocket Lake gear containment.
#[test]
fn decode_rocket_lake_gear_containment() {
    let sync = decode(
        &acceptance_regs(),
        IntelGen::RocketLake,
        &SmbiosTopology::empty(),
    );
    for ch in &sync.channels {
        assert_eq!(
            ch.clocks.mclk_mhz,
            Section::Value(1200.0),
            "ratio 18 half-step"
        );
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
        assert_eq!(
            ch.clocks.uclk_mhz,
            Section::Value(1200.0),
            "uclk = mclk (gear 1)"
        );
    }
    let absent = decode(
        &IntelImcRegs::default(),
        IntelGen::RocketLake,
        &SmbiosTopology::empty(),
    );
    for ch in &absent.channels {
        assert!(matches!(
            ch.clocks.mclk_mhz,
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            ch.clocks.gear_mode,
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            ch.clocks.uclk_mhz,
            Section::Na(NaReason::ParseError(_))
        ));
    }
}

/// [`channel_count`] delegates the profiled generations to their
/// profile.
#[test]
fn channel_count_delegates_to_profile() {
    for gen in [
        IntelGen::Skylake,
        IntelGen::KabyLake,
        IntelGen::CoffeeLake,
        IntelGen::CometLake,
        IntelGen::RocketLake,
    ] {
        let p = profile_for(gen).unwrap_or_else(|| panic!("{gen:?} must be profiled"));
        assert_eq!(channel_count(gen), p.channel_count, "{gen:?}");
    }
}

// -----------------------------------------------------------------
// Tier 2 acceptance fixtures (IG-13).
// -----------------------------------------------------------------

/// Breakdown §4 Example A, end-to-end under Rocket Lake.
#[test]
fn tier2_example_a_gear1_100_mhz_base_end_to_end() {
    let mut regs = acceptance_regs();
    regs.mcbios_req = Some(0x0000_0120);
    let ro = decode(&regs, IntelGen::RocketLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "Rocket Lake: two channels");
    for ch in &ro.channels {
        assert_eq!(
            ch.clocks.mclk_mhz,
            Section::Value(1600.0),
            "mclk = 16 × 100 MHz"
        );
        assert_eq!(
            ch.clocks.mclk_mhz.value().copied().map(|v| v * 2.0),
            Some(3200.0),
            "DDR: two transfers per DRAM clock = 3200 MT/s"
        );
        assert_eq!(
            ch.clocks.gear_mode,
            Section::Value(GearMode::One),
            "bits [17:16] = 00b → gear 1"
        );
        assert_eq!(
            ch.clocks.uclk_mhz,
            Section::Value(1600.0),
            "gear 1: uclk = mclk (1:1)"
        );
    }
}

/// Breakdown §4 Example D, end-to-end under Rocket Lake.
#[test]
fn tier2_example_d_gear2_100_mhz_base_end_to_end() {
    let mut regs = acceptance_regs();
    regs.mcbios_req = Some(0x0001_013C);
    let ro = decode(&regs, IntelGen::RocketLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "Rocket Lake: two channels");
    for ch in &ro.channels {
        assert_eq!(
            ch.clocks.mclk_mhz,
            Section::Value(3000.0),
            "mclk = 60 × 100 / 2"
        );
        assert_eq!(
            ch.clocks.mclk_mhz.value().copied().map(|v| v * 2.0),
            Some(6000.0),
            "DDR: two transfers per DRAM clock = 6000 MT/s"
        );
        assert_eq!(
            ch.clocks.gear_mode,
            Section::Value(GearMode::Two),
            "bits [17:16] = 01b → gear 2"
        );
        assert_eq!(
            ch.clocks.uclk_mhz,
            Section::Value(1500.0),
            "gear 2: uclk = mclk / 2"
        );
    }
}

/// The Rocket Lake turnaround quartets decode end-to-end through `decode`.
#[test]
fn tier2_rocket_turnaround_quartets_end_to_end() {
    let regs = acceptance_regs();
    assert_eq!(
        legacy_ddr4::turnaround_quartet(0x0048_C286),
        [6, 10, 12, 18],
        "TC_RDRD"
    );
    assert_eq!(
        legacy_ddr4::turnaround_quartet(0x0000_0280),
        [0, 10, 0, 0],
        "TC_RDWR"
    );
    assert_eq!(
        legacy_ddr4::turnaround_quartet(0x0000_0308),
        [8, 12, 0, 0],
        "TC_WRRD"
    );
    assert_eq!(
        legacy_ddr4::turnaround_quartet(0x0040_C204),
        [4, 8, 12, 16],
        "TC_WRWR"
    );

    let ro = decode(&regs, IntelGen::RocketLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "Rocket Lake: two channels");
    for ch in &ro.channels {
        assert_eq!(ch.timings.rdrd_scl, Section::Value(6), "sg");
        assert_eq!(ch.timings.rdrd_sc, Section::Value(10), "dg");
        assert_eq!(ch.timings.rdrd_sd, Section::Value(12), "dr");
        assert_eq!(ch.timings.rdrd_dd, Section::Value(18), "dd");
        assert_eq!(ch.timings.rdwr, Section::Value(10), "dg");
        assert_eq!(ch.timings.wtrs, Section::Value(12), "dg");
        assert_eq!(ch.timings.wtrl, Section::Value(8), "sg");
        assert_eq!(ch.timings.wrwr_scl, Section::Value(4), "sg");
        assert_eq!(ch.timings.wrwr_sc, Section::Value(8), "dg");
        assert_eq!(ch.timings.wrwr_sd, Section::Value(12), "dr");
        assert_eq!(ch.timings.wrwr_dd, Section::Value(16), "dd");
        assert_eq!(
            ch.clocks.mclk_mhz,
            Section::Value(1200.0),
            "ratio 18 half-step"
        );
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
        assert_eq!(
            ch.clocks.uclk_mhz,
            Section::Value(1200.0),
            "gear 1: uclk = mclk"
        );
    }
    let tier1 = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    for (r, t) in ro.channels.iter().zip(&tier1.channels) {
        assert_eq!(r.timings, t.timings, "all shared-map timings identical");
    }
}

// -----------------------------------------------------------------
// Entry points: `read_regs` total + panic-free on this host.
// -----------------------------------------------------------------

/// `read_regs` over all-absent raws is structural and panic-free.
#[test]
fn read_regs_on_this_host_is_structural_and_panic_free() {
    let ro = read_regs(&IntelImcRegs::default());
    let expected: Vec<u8> = match CpuInfo::detect().vendor {
        CpuVendor::Intel(gen) => {
            assert_eq!(
                ro.channels.len(),
                usize::from(channel_count(gen)),
                "Intel host: the channel count follows the detected generation"
            );
            if matches!(gen, IntelGen::AlderLake | IntelGen::RaptorLake) {
                vec![0, 2]
            } else {
                (0..channel_count(gen)).collect()
            }
        }
        _ => {
            assert_eq!(
                ro.channels.len(),
                2,
                "non-Intel host: the degraded readout carries the common two-channel shape"
            );
            vec![0, 1]
        }
    };
    for (i, ch) in ro.channels.iter().enumerate() {
        assert_eq!(ch.index, expected[i]);
        assert!(
            ch.clocks.mclk_mhz.is_na(),
            "absent MC_BIOS_REQ: mclk must be an honest Na: {:?}",
            ch.clocks.mclk_mhz
        );
        for s in [&ch.clocks.uclk_mhz, &ch.clocks.fclk_mhz] {
            assert!(s.is_na(), "{s:?}");
        }
        assert!(ch.clocks.gdm.is_na(), "{:?}", ch.clocks.gdm);
        assert!(ch.clocks.pdm.is_na(), "{:?}", ch.clocks.pdm);
        assert!(ch.clocks.div_mode.is_na(), "{:?}", ch.clocks.div_mode);
        assert!(ch.clocks.gear_mode.is_na(), "{:?}", ch.clocks.gear_mode);
        assert!(
            ch.clocks.command_rate.is_na(),
            "{:?}",
            ch.clocks.command_rate
        );
        assert!(ch.rtl.is_na());
        for s in [
            &ch.timings.cl,
            &ch.timings.rcwdwr,
            &ch.timings.rcdrd,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.rc,
            &ch.timings.rrds,
            &ch.timings.rrld,
            &ch.timings.faw,
            &ch.timings.wtrs,
            &ch.timings.wtrl,
            &ch.timings.wr,
            &ch.timings.rfc1,
            &ch.timings.rfc2,
            &ch.timings.rfcsb,
            &ch.timings.cwl,
            &ch.timings.rtp,
            &ch.timings.rdwr,
            &ch.timings.wrrd,
            &ch.timings.rdrd_sd,
            &ch.timings.rdrd_dd,
            &ch.timings.rdrd_scl,
            &ch.timings.rdrd_sc,
            &ch.timings.wrwr_sd,
            &ch.timings.wrwr_dd,
            &ch.timings.wrwr_scl,
            &ch.timings.wrwr_sc,
        ] {
            assert!(s.is_na(), "{s:?}");
        }
    }
}

/// The decoded readout is wire-safe: populated, fully degraded, and
/// gen-gated shapes all round-trip through bincode.
#[test]
fn decode_readout_bincode_round_trip() {
    for ro in [
        {
            let mut regs = acceptance_regs();
            regs.mad_inter_channel = Some(1);
            decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty())
        },
        decode(
            &IntelImcRegs::default(),
            IntelGen::Skylake,
            &SmbiosTopology::empty(),
        ),
        decode(
            &acceptance_regs(),
            IntelGen::AlderLake,
            &SmbiosTopology::empty(),
        ),
    ] {
        let bytes =
            bincode::serialize(&ro).expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
        assert_eq!(ro, back);
    }
}

/// (c) The fixed channel-count model per [`IntelGen`].
#[test]
fn channel_count_model() {
    let two = [
        IntelGen::Skylake,
        IntelGen::KabyLake,
        IntelGen::CoffeeLake,
        IntelGen::CometLake,
        IntelGen::IceLake,
        IntelGen::TigerLake,
        IntelGen::Haswell,
        IntelGen::Broadwell,
        IntelGen::AlderLake,
        IntelGen::RaptorLake,
        IntelGen::MeteorLake,
        IntelGen::ArrowLake,
        IntelGen::RocketLake,
        IntelGen::Unrecognized,
    ];
    for g in two {
        assert_eq!(channel_count(g), 2, "{g:?}");
    }
}

/// The `/dev/mem` fallback map size is the 64 KiB Tier-1 window.
#[test]
fn devmem_fallback_map_is_64_kib() {
    assert_eq!(MCHBAR_WINDOW, 0x10000, "Tier-1 MCHBAR window is 64 KiB");
    for off in [MAX_IMC_OFFSET, MAX_CHANNEL_OFFSET] {
        assert!(
            off <= MCHBAR_WINDOW,
            "offset {off:#x} must fit the 64 KiB window"
        );
    }
}

/// (e) The pure gate: Intel passes with its generation; AMD /
/// unknown yield `UnsupportedHardware`.
#[test]
fn intel_gen_gate_dispatches_by_vendor() {
    fn info(vendor: CpuVendor) -> CpuInfo {
        CpuInfo {
            vendor,
            brand: "synthetic".to_owned(),
        }
    }
    assert_eq!(
        intel_gen_gate(&info(CpuVendor::Intel(IntelGen::AlderLake))),
        Ok(IntelGen::AlderLake)
    );
    assert_eq!(
        intel_gen_gate(&info(CpuVendor::Unknown)),
        Err(TelemetryError::UnsupportedHardware {
            vendor: "unknown CPU vendor".to_owned()
        })
    );
    assert!(matches!(
        intel_gen_gate(&info(CpuVendor::Amd(AmdZen::Zen3))),
        Err(TelemetryError::UnsupportedHardware { .. })
    ));
}

/// (e) The live gate behavior is vendor-consistent.
#[test]
fn intel_gen_gate_behavior_is_vendor_consistent() {
    let detected = CpuInfo::detect();
    match detected.vendor {
        CpuVendor::Intel(gen) => {
            assert_eq!(intel_gen_gate(&detected), Ok(gen));
        }
        _ => {
            assert!(matches!(
                intel_gen_gate(&detected),
                Err(TelemetryError::UnsupportedHardware { .. })
            ));
        }
    }
}

/// The clock sanity gate bounds the displayed value range.
#[test]
fn clock_section_sanity_gate() {
    assert_eq!(clock_section(1600.0), Section::Value(1600.0));
    assert_eq!(clock_section(1.0), Section::Value(1.0));
    assert_eq!(clock_section(4096.0), Section::Value(4096.0));
    assert_eq!(
        clock_section(5333.0),
        Section::Value(5333.0),
        "H-6: DDR5-10666 upper bound"
    );
    for mhz in [0.0, 0.5, 5333.1, 65535.0] {
        assert!(
            matches!(clock_section(mhz), Section::Na(NaReason::ParseError(_))),
            "{mhz}"
        );
    }
}

/// The frozen readout types are `Clone` + `Debug` + `PartialEq`.
#[test]
fn readout_and_channel_traits() {
    fn bound<T: Clone + std::fmt::Debug + PartialEq>() {}
    bound::<IntelChannel>();
    bound::<IntelReadout>();

    let ro = fixture_intel_readout(
        IntelGen::Skylake,
        Some(0x0000_000C),
        &fixture_channel_regs(),
        &ChannelRegs::default(),
        false,
    );
    let a = ro.channels[0].clone();
    let b = a.clone();
    assert_eq!(a, b);
    assert!(!format!("{a:?}").is_empty());
    assert_eq!(
        a,
        fixture_intel_readout(
            IntelGen::Skylake,
            Some(0x0000_000C),
            &fixture_channel_regs(),
            &ChannelRegs::default(),
            false,
        )
        .channels[0]
    );

    assert_eq!(ro.channels.len(), 2);
    assert_eq!(ro.channels[0].index, 0);
    assert_eq!(ro.channels[1].index, 1);
}

/// (f) P3-04: a multi-channel `IntelReadout` round-trips through bincode.
#[test]
fn intel_readout_bincode_round_trip() {
    let mut ro = fixture_intel_readout(
        IntelGen::Skylake,
        Some(0x0000_000C),
        &fixture_channel_regs(),
        &ChannelRegs::default(),
        false,
    );
    ro.channel_mode = Some(ChannelMode::DualFlex);
    ro.ecc_status = EccStatus::CapableButDisabled;
    ro.memory_tech = Some(MemoryTech::Ddr4);
    ro.channel_trust = Some(ChannelTrust::VerifiedHardware);
    ro.dimm_count = 2;

    let bytes = bincode::serialize(&ro).expect("IntelReadout must serialize (no-panic contract)");
    let back: IntelReadout = bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
    assert_eq!(ro, back);
    assert_eq!(back.ecc_status, EccStatus::CapableButDisabled);
    assert_eq!(back.memory_tech, Some(MemoryTech::Ddr4));
    assert_eq!(back.channel_trust, Some(ChannelTrust::VerifiedHardware));
    assert_eq!(back.dimm_count, 2);

    let all_na = fixture_intel_readout(
        IntelGen::Skylake,
        None,
        &ChannelRegs::default(),
        &ChannelRegs::default(),
        false,
    );
    let bytes =
        bincode::serialize(&all_na).expect("IntelReadout must serialize (no-panic contract)");
    let back: IntelReadout = bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
    assert_eq!(all_na, back);
}

/// Phase 1 regression guard (TC-ALK-DDR5): a DDR5 Alder Lake system
/// still takes the 4-subchannel DDR5 path.
#[test]
fn tc_alk_ddr5_still_routes_to_the_subchannel_path() {
    let mut regs = tier3_ddr5_condition_regs();
    regs.mad_inter_channel = Some(4);
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 4, "DDR5 Alder Lake = 4 subchannels");
}

/// TC-TEL4 (TEL-4): the known-wrong pre-v2.2.1 IMC skeleton is gone.
#[test]
fn tc_tel4_legacy_skeleton_is_gone_and_fixtures_use_the_live_decode() {
    let src = include_str!("mod.rs");
    let gone: Vec<String> = [
        ("decode", "channel"),
        ("decode", "freq_ratio"),
        ("decode", "tcl"),
        ("decode", "trcd"),
        ("decode", "trp"),
        ("decode", "tras"),
        ("decode", "cmd_rate"),
        ("decode", "gear"),
        ("decode", "rtl"),
        ("decode", "tccd_s"),
        ("decode", "tccd_l"),
        ("decode", "trdrd"),
        ("decode", "trdwr"),
        ("decode", "twrwr"),
        ("decode", "twrrd"),
        ("channel", "offsets"),
        ("LEGACY", "_MCS"),
        ("LEGACY", "_FREQ_RATIO"),
        ("FREQ_RATIO_STEP", "_MHZ"),
    ]
    .iter()
    .map(|(a, b)| format!("{a}{b}"))
    .collect();
    for gone in &gone {
        assert!(
            !src.contains(gone.as_str()),
            "the legacy skeleton identifier `{gone}` must be gone (TEL-4)"
        );
    }

    let ro = fixture_intel_readout(
        IntelGen::Skylake,
        Some(0x0000_000C),
        &fixture_channel_regs(),
        &ChannelRegs::default(),
        false,
    );
    assert_eq!(ro.channels.len(), 2);
    assert_eq!(ro.channels[0].clocks.mclk_mhz, Section::Value(1600.0));
    assert_eq!(ro.channels[0].timings.cl, Section::Value(17));
    assert_eq!(ro.channels[1].clocks.mclk_mhz, Section::Value(1600.0));
    assert!(ro.channels[1].timings.cl.is_na());
    assert!(ro.channels[1].timings.ras.is_na());
    assert_eq!(ro.channel_mode, None);
    assert_eq!(ro.channel_trust, None);
    assert!(ro.channel_annotation.is_empty());
    assert_eq!(ro.dimm_count, 0);
    assert_eq!(ro.ecc_status, EccStatus::Unknown);
}

// -----------------------------------------------------------------
// C-3 aliasing policy tests.
// -----------------------------------------------------------------

/// (C-3) When the resolved tech is DDR4, the `0xD800` page is the MAD
/// block, so the MCL1 timing words are **not** decoded.
#[test]
fn c3_ddr4_mcl1_timing_not_decoded() {
    let mad_inter = 0x100u32;
    let regs = IntelImcRegs {
        ch2: ChannelRegs {
            tc_dbp: Some(0xFFFF_FFFF),
            tc_rap: Some(0xFFFF_FFFF),
            tc_rfp: Some(0xFFFF_FFFF),
            tc_rap2: Some(0xFFFF_FFFF),
            tc_rdrd: Some(0xFFFF_FFFF),
            tc_rdwr: Some(0xFFFF_FFFF),
            tc_wrrd: Some(0xFFFF_FFFF),
            tc_wrwr: Some(0xFFFF_FFFF),
        },
        mcl1: MclRegs {
            tc_pre: Some(0x0028_8410),
            tc_act: Some(0x0010_0020),
            tc_act2: Some(0x0000_0001),
            tc_wtr: Some(0x0000_0002),
            tc_rfp: Some(0x0000_0004),
            tc_rfp2: Some(0x0000_0008),
            tc_rdrd: Some(0x0000_0010),
            tc_wrwr: Some(0x0000_0020),
        },
        mad_inter_channel: Some(mad_inter),
        ..Default::default()
    };

    let readout = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());

    assert_eq!(readout.memory_tech, Some(MemoryTech::Ddr4));
    let ch2 = readout
        .channels
        .iter()
        .find(|c| c.index == 1)
        .expect("second channel (subchannel 2) present");
    assert!(
        matches!(ch2.timings.rcdrd, Section::Na(_)),
        "MCL1 tRCD must not be decoded for DDR4: {:?}",
        ch2.timings.rcdrd
    );
    assert!(
        matches!(ch2.timings.cl, Section::Na(_)),
        "MCL1 tCL must not be decoded for DDR4: {:?}",
        ch2.timings.cl
    );
}

/// N-04: the C-3 circularity de-risk.
#[test]
fn c3_smbios_crosscheck_trusts_smbios_on_disagreement() {
    let smbios = SmbiosTopology {
        memory_type: Some(MemoryTech::Ddr4),
        ..SmbiosTopology::empty()
    };
    let regs = IntelImcRegs {
        ch2: ChannelRegs {
            tc_rap: Some(0xFFFF_FFFF),
            ..ChannelRegs::default()
        },
        mcl1: MclRegs {
            tc_act: Some(0x0028_8410),
            ..MclRegs::default()
        },
        mad_inter_channel: Some(4),
        ..IntelImcRegs::default()
    };
    let ro = decode(&regs, IntelGen::AlderLake, &smbios);
    assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr5));
    assert!(
        ro.channel_annotation.contains("disagrees with SMBIOS"),
        "the C-3 cross-check annotates the disagreement: {}",
        ro.channel_annotation
    );
    let ch2 = ro
        .channels
        .iter()
        .find(|c| c.index == 1)
        .expect("second channel (subchannel 2) present");
    assert!(
        matches!(ch2.timings.cl, Section::Na(_)),
        "MCL1 tCL must not be decoded on an SMBIOS-DDR4 platform: {:?}",
        ch2.timings.cl
    );
}

/// N-04: when the MAD-derived tech and the SMBIOS Type 17 tech
/// AGREE, the cross-check is silent.
#[test]
fn c3_smbios_crosscheck_silent_when_agreeing() {
    let smbios = SmbiosTopology {
        memory_type: Some(MemoryTech::Ddr4),
        ..SmbiosTopology::empty()
    };
    let mut regs = tier3_ddr5_condition_regs();
    regs.mad_inter_channel = Some(0);
    let ro = decode(&regs, IntelGen::AlderLake, &smbios);
    assert_eq!(
        ro.channels.len(),
        4,
        "OQ-11 DDR5 condition met -> 4 subchannels"
    );
    assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr4));
    assert!(
        !ro.channel_annotation.contains("disagrees with SMBIOS"),
        "agreement is silent: {}",
        ro.channel_annotation
    );
}

// -----------------------------------------------------------------
// 4-tier channel resolution end-to-end.
// -----------------------------------------------------------------

/// The exact user bug, end to end: a dual-channel 2×16 GiB system
/// whose MMIO is blocked.
#[test]
fn decode_l3_override_single_mmio_dual_smbios() {
    let smbios = SmbiosTopology {
        populated_count: 2,
        dimm_sizes: vec![16384, 16384],
        total_capacity_mib: 32768,
        memory_type: Some(MemoryTech::Ddr4),
        speed_mts: None,
        locators: vec!["Ch0".to_owned(), "Ch1".to_owned()],
        derived_mode: DerivedMode::Dual,
    };
    let mut regs = acceptance_regs();
    regs.mad_inter_channel = Some(0x0);
    regs.mad_dimm_ch0 = Some(0x20);
    regs.mad_dimm_ch1 = Some(0x0);
    let ro = decode(&regs, IntelGen::AlderLake, &smbios);
    assert_eq!(
        ro.channel_mode,
        Some(ChannelMode::DualSymmetric),
        "MMIO blocked + SMBIOS dual -> Dual (L2 firmware-verified)"
    );
    assert_eq!(ro.channel_trust, Some(ChannelTrust::VerifiedFirmware));
    assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr4));
    assert_eq!(ro.dimm_count, 2);
    assert_eq!(
        ro.channel_display_label(),
        Some("Dual-Channel (Symmetric)".to_owned())
    );
}

/// End to end (the 4-tier model): the Tier-1 decode resolves the
/// channel mode through `resolve_channel`.
#[test]
fn decode_tier1_channel_mode_4_tier() {
    let mut regs = acceptance_regs();
    regs.mad_inter_channel = Some(0x0);
    regs.mad_dimm_ch0 = Some(0x20);
    regs.mad_dimm_ch1 = Some(0x20);
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    assert_eq!(ro.channel_mode, Some(ChannelMode::DualSymmetric));
    assert_eq!(ro.channel_trust, Some(ChannelTrust::VerifiedHardware));

    let mut regs = acceptance_regs();
    regs.mad_inter_channel = Some(0x0);
    regs.mad_dimm_ch0 = Some(0x20);
    regs.mad_dimm_ch1 = Some(0x0);
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    assert_eq!(ro.channel_mode, None);
    assert_eq!(ro.channel_trust, Some(ChannelTrust::None));

    let mut regs = acceptance_regs();
    regs.mad_inter_channel = Some(0x0);
    let ro = decode(&regs, IntelGen::Skylake, &SmbiosTopology::empty());
    assert_eq!(ro.channel_mode, None);
    assert_eq!(ro.channel_trust, Some(ChannelTrust::None));
}

// -----------------------------------------------------------------
// C-2 degenerate-word screening.
// -----------------------------------------------------------------

/// The degenerate-word screening helpers.
#[test]
fn degenerate_word_helpers() {
    assert!(!is_present_word(None), "None is not present");
    assert!(!is_present_word(Some(0)), "zero is not present");
    assert!(
        !is_present_word(Some(0xFFFF_FFFF)),
        "bus-fault is not present"
    );
    assert!(is_present_word(Some(1)), "non-zero non-fault is present");
    assert!(
        is_present_word(Some(0x1234_5678)),
        "arbitrary valid word is present"
    );

    assert!(is_degenerate_word(0), "zero is degenerate");
    assert!(is_degenerate_word(0xFFFF_FFFF), "bus-fault is degenerate");
    assert!(
        !is_degenerate_word(1),
        "non-zero non-fault is not degenerate"
    );
    assert!(
        !is_degenerate_word(0x1234_5678),
        "arbitrary valid word is not degenerate"
    );
}

/// C-2: OQ-11's DDR5 condition with bus-fault words.
#[test]
fn c2_bus_fault_mad_words_are_not_ddr5_evidence() {
    let mut regs = tier3_ddr5_condition_regs();
    regs.mad_dimm_ch2 = Some(0xFFFF_FFFF);
    regs.mad_dimm_ch3 = Some(0xFFFF_FFFF);
    regs.mad_inter_channel = Some(0);
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "bus-fault MAD words -> DDR4 default");
    assert_eq!(
        ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
        vec![0, 1],
    );
}

/// C-2: OQ-11's DDR5 condition with zero MAD words.
#[test]
fn c2_zero_mad_words_are_not_ddr5_evidence() {
    let mut regs = tier3_ddr5_condition_regs();
    regs.mad_dimm_ch2 = Some(0);
    regs.mad_dimm_ch3 = Some(0);
    regs.mad_inter_channel = Some(0);
    let ro = decode(&regs, IntelGen::AlderLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "zero MAD words -> DDR4 default");
}

/// M-7: the Tier-3 2-channel default decodes subchannels `[0, 2]`,
/// but the display index is the channel's position in the output vector.
#[test]
fn tier3_two_channel_default_indexes_are_sequential() {
    let regs = IntelImcRegs {
        mcbios_req: Some(0x0001_0012),
        ..IntelImcRegs::default()
    };
    let ro = decode(&regs, IntelGen::MeteorLake, &SmbiosTopology::empty());
    assert_eq!(ro.channels.len(), 2, "the 2-channel default");
    assert_eq!(ro.channels[0].index, 0, "first channel is display index 0");
    assert_eq!(
        ro.channels[1].index, 1,
        "second channel is display index 1 (not subchannel 2)"
    );
}
