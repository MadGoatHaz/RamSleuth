//! The Tier-3 (Alder/Raptor/Meteor/Arrow Lake) DDR5 decode path: the
//! widened `ALDER_FIELDS` bitfield decoders, the mirror → MCL fallback
//! selector, the per-subchannel decode core, and the OQ-5 tile-routing
//! probe.
//!
//! Step 2 of the `intel_readout` directory-module split — moved verbatim
//! from `mod.rs`.

use super::gates::*;
use super::types::*;

use crate::amd_readout::TimingSet;
use crate::error::{NaReason, Section};
use crate::intel_gen::{AlderChannelMap, ALDER_FIELDS};

// ---------------------------------------------------------------------------
// Tier-3 (Alder/Raptor Lake, DDR5) bitfield decoders — the widened
// `ALDER_FIELDS` ranges transcribed verbatim from the Research Alder/Raptor
// table (lines 111–132, IG-20). The table's bit ranges apply to
// Controller 0 / Subchannel 0 (and Controller 1 / Subchannel 2) and are
// reused per subchannel (Research line 107).
// ---------------------------------------------------------------------------

/// tCL — `TC_ACT` bits [23:16] (**8-bit**, widened to accommodate the DDR5
/// CL40–CL56 latencies; Research line 111).
pub(super) fn tc_act_tcl(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.tcl.0, ALDER_FIELDS.tcl.1)
}

/// tFAW — `TC_ACT` bits [7:0] (8-bit; Research line 116).
pub(super) fn tc_act_tfaw(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.tfaw.0, ALDER_FIELDS.tfaw.1)
}

/// tRRD_S — `TC_ACT` bits [11:8] (4-bit; Research line 117).
pub(super) fn tc_act_trrd_s(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trrd_s.0, ALDER_FIELDS.trrd_s.1)
}

/// tRRD_L — `TC_ACT` bits [15:12] (4-bit; Research line 118).
pub(super) fn tc_act_trrd_l(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trrd_l.0, ALDER_FIELDS.trrd_l.1)
}

/// tPPD — `TC_ACT2` bits [3:0] (4-bit, DDR5-specific; Research line
/// 125). Decoded by the core but has **no frozen display slot** (not
/// shown; the raw value stays available via the sysfs attributes).
pub fn tc_act2_tppd(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.tppd.0, ALDER_FIELDS.tppd.1)
}

/// tRCD — `TC_PRE` bits [14:8] (**7-bit**, widened; Research line 112).
/// Intel enforces a *unified, symmetric* tRCD (as in Tier 1): the same
/// value feeds both the `rcdrd` and `rcwdwr` display slots.
pub(super) fn tc_pre_trcd(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trcd.0, ALDER_FIELDS.trcd.1)
}

/// tRP — `TC_PRE` bits [6:0] (**7-bit**, widened for the DDR5 precharge
/// cycle requirements; Research line 113).
pub(super) fn tc_pre_trp(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trp.0, ALDER_FIELDS.trp.1)
}

/// tRAS — `TC_PRE` bits [23:16] (**8-bit**, widened for the DDR5 row
/// active constraints; Research line 114).
pub(super) fn tc_pre_tras(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.tras.0, ALDER_FIELDS.tras.1)
}

/// tCWL — `TC_PRE` bits [30:24] (7-bit; Research line 115).
pub(super) fn tc_pre_tcwl(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.tcwl.0, ALDER_FIELDS.tcwl.1)
}

/// tWTR_S — `TC_WTR` bits [6:0] (**7-bit**, widened; Research line 119).
pub(super) fn tc_wtr_twtr_s(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.twtr_s.0, ALDER_FIELDS.twtr_s.1)
}

/// tWTR_L — `TC_WTR` bits [14:8] (**7-bit**, widened; Research line 120).
pub(super) fn tc_wtr_twtr_l(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.twtr_l.0, ALDER_FIELDS.twtr_l.1)
}

/// tWR — `TC_WTR` bits [23:16] (**8-bit**, widened to absorb the on-die
/// ECC write recovery penalties; Research line 121).
pub(super) fn tc_wtr_twr(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.twr.0, ALDER_FIELDS.twr.1)
}

/// tRTP — `TC_WTR` bits [30:24] (7-bit; Research line 122).
pub(super) fn tc_wtr_trtp(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trtp.0, ALDER_FIELDS.trtp.1)
}

/// tRFC1 — `TC_RFP` bits [11:0] (**12-bit**, widened for the
/// high-density DRAM refresh cycles; Research line 123).
pub(super) fn tc_rfp_trfc1(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trfc1.0, ALDER_FIELDS.trfc1.1)
}

/// tRFCsb — `TC_RFP2` bits [10:0] (11-bit, **DDR5-specific** same-bank
/// refresh; Research line 124). Feeds the `rfcsb` display slot
/// (populated on Tier 3, unlike Tier 1).
pub(super) fn tc_rfp2_trfcsb(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.trfcsb.0, ALDER_FIELDS.trfcsb.1)
}

/// tWRWR_sg — `TC_WRWR` bits [5:0] (6-bit; Research line 131). Only the
/// two documented Alder entries (sg / dg) are decoded — the Tier-1
/// _dr/_dd rows are not extrapolated (IG-20).
pub(super) fn tc_wrwr_s(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.twrwr_sg.0, ALDER_FIELDS.twrwr_sg.1)
}

/// tWRWR_dg — `TC_WRWR` bits [11:6] (6-bit; Research line 132).
pub(super) fn tc_wrwr_d(reg: u32) -> u16 {
    field16(reg, ALDER_FIELDS.twrwr_dg.0, ALDER_FIELDS.twrwr_dg.1)
}

// ---------------------------------------------------------------------------
// Mirror → MCL fallback (Research line 329).
// ---------------------------------------------------------------------------

/// The mirror → MCL fallback selector (Research line 329, verbatim
/// rule): select `mirror_reg` unless `mirror ∈ {0x0, 0xFFFFFFFF}`, in
/// which case select `mcl_reg` — the research doc's "verify 0xD004 if
/// 0x4004 returns 0x0 or 0xFFFFFFFF" over the `Option<u32>` raws.
///
/// Some mobile / OEM boards disable the legacy mirror window; when a
/// mirrored register then reads the disabled-mirror default `0x0` or
/// the uncleared `0xFFFFFFFF`, the corresponding native-MCL register is
/// selected instead (the research doc's example: 0x4004 → 0xD004).
///
/// Containment: a populated non-default mirror wins; a failed mirror
/// read (`None`) is **not** a disabled mirror → no fallback (`None`);
/// a disabled mirror with an absent MCL read → `None` (honest
/// degradation, never a bogus zero).
pub fn mirror_or_mcl(mirror: Option<u32>, mcl: Option<u32>) -> Option<u32> {
    match mirror {
        // A failed read is not a disabled mirror → no fallback.
        None => None,
        // H-3: A degenerate mirror word (0x0 / 0xFFFF_FFFF) falls back
        // to the MCL register, which is *also* screened for degenerates
        // (a bus-faulted MCL word is not a valid fallback source).
        Some(w) if is_degenerate_word(w) => {
            mcl.filter(|&w| !is_degenerate_word(w))
        }
        // A populated, non-degenerate mirror wins.
        Some(v) => Some(v),
    }
}

/// The effective Tier-3 [`MclRegs`] for one subchannel: the Research
/// line 329 mirror → MCL fallback applied per controller (IG-25).
///
/// The legacy mirror block is the primary source; where the subchannel's
/// controller has a documented MCL base (subchannels 0 and 2 — OQ-4),
/// every mirrored register reading the disabled-mirror default
/// (`0x0` / `0xFFFFFFFF`) falls back to the corresponding native-MCL
/// register. Subchannels 1 and 3 carry no MCL base → mirror-only
/// (their words are taken as-is; a disabled mirror degrades to `None`).
///
/// Register pairing is by MCHBAR-relative offset (the mirror is a
/// legacy alias of the MCL block — the same words, under the Tier-1
/// `ChannelRegs` names vs. the Alder `MclRegs` names): `+0x00` /
/// `+0x04` / `+0x08` / `+0x20` / `+0x28` fall back per register; the
/// MCL-only registers (`+0x10` `TC_WTR`, `+0x14` `TC_RFP`, `+0x18`
/// `TC_RFP2`) have no word in the Tier-1 layout and ride the MCL block
/// as-is (mirror-only subchannels: `None`).
pub fn tier3_mcl_for(index: usize, map: &AlderChannelMap, regs: &IntelImcRegs) -> MclRegs {
    let mirror = match index {
        0 => &regs.ch0,
        1 => &regs.ch1,
        2 => &regs.ch2,
        _ => &regs.ch3,
    };
    // OQ-4: the fallback applies only where the subchannel's controller
    // has a documented MCL base (MC0 for subchannel 0, MC1 for
    // subchannel 2); subchannels 1 and 3 are mirror-only.
    let mcl: Option<&MclRegs> = match (index, map.mcl_base) {
        (0, Some(_)) => Some(&regs.mcl0),
        (2, Some(_)) => Some(&regs.mcl1),
        _ => None,
    };
    MclRegs {
        tc_pre: mirror_or_mcl(mirror.tc_dbp, mcl.and_then(|m| m.tc_pre)),
        tc_act: mirror_or_mcl(mirror.tc_rap, mcl.and_then(|m| m.tc_act)),
        tc_act2: mirror_or_mcl(mirror.tc_rfp, mcl.and_then(|m| m.tc_act2)),
        tc_wtr: mcl.and_then(|m| m.tc_wtr),
        tc_rfp: mcl.and_then(|m| m.tc_rfp),
        tc_rfp2: mcl.and_then(|m| m.tc_rfp2),
        tc_rdrd: mirror_or_mcl(mirror.tc_rdrd, mcl.and_then(|m| m.tc_rdrd)),
        tc_wrwr: mirror_or_mcl(mirror.tc_wrrd, mcl.and_then(|m| m.tc_wrwr)),
    }
}

// ---------------------------------------------------------------------------
// The Tier-3 per-subchannel decode core.
// ---------------------------------------------------------------------------

/// Decode one Tier-3 (Alder/Raptor Lake, DDR5) subchannel from its raw
/// [`MclRegs`] register set + the shared DRAM core clock + the
/// subchannel's [`AlderChannelMap`] descriptor (the per-subchannel heart
/// of the Tier-3 decode path, IG-24). Never panics (D5): every cell is a
/// `Value`, an honest `Na(ParseError)` (a sourced register failed / a
/// field is untrained), or a structural `Na(NotApplicable)`.
///
/// The widened field extracts follow the Research Alder/Raptor table
/// (lines 111–132, pinned in [`ALDER_FIELDS`]): tCL / tRAS / tWR 8-bit,
/// tRCD / tRP 7-bit, tRFC1 12-bit, tRFCsb 11-bit → the `rfcsb` slot
/// (populated on DDR5, unlike Tier 1), tPPD 4-bit (decoded — no display
/// slot), and the `TC_RDRD` 4×6-bit quartet unchanged. The table's bit
/// ranges apply to Controller 0 / Subchannel 0 (Research line 107) and
/// are reused per subchannel — the extraction is map-invariant; the
/// `map` parameter carries the subchannel's provenance (mirror / MCL
/// bases) for the IG-25 fallback selection ([`tier3_mcl_for`]).
///
/// The same [1, 2048] tick / [1, 5333] MHz sanity gates as Tier 1 apply
/// to every decoded cell, and the `rc = ras + rp` synthesis is preserved
/// (Breakdown §6 — Intel exposes no tRC register). The gear cells
/// (`gear_mode` / `uclk_mhz`) stay `Na(NotApplicable)` here, exactly as
/// in [`super::legacy_ddr4::decode_tier1_channel`] — the caller dispatch
/// (IG-26) layers them on from `MC_BIOS_REQ[17:16]` via
/// [`super::gear_uclk_cells`] on the `GearCap::Gear4` Alder profile.
pub fn decode_tier3_channel(
    index: u8,
    mclk: Section<f64>,
    regs: &MclRegs,
    map: &AlderChannelMap,
) -> IntelChannel {
    // H-2: Normalize degenerate words to None so the turnaround-quartet
    // path never fabricates values from a bus-fault readback.
    let regs = MclRegs {
        tc_pre: regs.tc_pre.filter(|&w| !is_degenerate_word(w)),
        tc_act: regs.tc_act.filter(|&w| !is_degenerate_word(w)),
        tc_act2: regs.tc_act2.filter(|&w| !is_degenerate_word(w)),
        tc_wtr: regs.tc_wtr.filter(|&w| !is_degenerate_word(w)),
        tc_rfp: regs.tc_rfp.filter(|&w| !is_degenerate_word(w)),
        tc_rfp2: regs.tc_rfp2.filter(|&w| !is_degenerate_word(w)),
        tc_rdrd: regs.tc_rdrd.filter(|&w| !is_degenerate_word(w)),
        tc_wrwr: regs.tc_wrwr.filter(|&w| !is_degenerate_word(w)),
    };

    // --- timings (integer DRAM clock cycles) ---------------------------
    // TC_PRE: tRCD (the unified symmetric tRCD feeds both the rcdrd and
    // the rcwdwr slot, as in Tier 1) / tRP / tRAS / tCWL.
    let rcdrd = timing_cell(regs.tc_pre, "tRCD", tc_pre_trcd);
    let rcwdwr = rcdrd.clone();
    let rp = timing_cell(regs.tc_pre, "tRP", tc_pre_trp);
    let ras = timing_cell(regs.tc_pre, "tRAS", tc_pre_tras);
    let cwl = timing_cell(regs.tc_pre, "tCWL", tc_pre_tcwl);

    // TC_ACT: tCL / tFAW / tRRD_S / tRRD_L (widened).
    let cl = timing_cell(regs.tc_act, "tCL", tc_act_tcl);
    let faw = timing_cell(regs.tc_act, "tFAW", tc_act_tfaw);
    let rrds = timing_cell(regs.tc_act, "tRRD_S", tc_act_trrd_s);
    let rrld = timing_cell(regs.tc_act, "tRRD_L", tc_act_trrd_l);

    // tRC is synthesized (the JEDEC identity — Intel exposes no tRC
    // register): `tRAS + tRP` when both decode.
    let rc = match (ras.value().copied(), rp.value().copied()) {
        (Some(a), Some(b)) => ticks_section(a.saturating_add(b), "tRC (tRAS + tRP)"),
        _ => Section::na(NaReason::NotApplicable),
    };

    // TC_WTR: tWTR_S / tWTR_L / tWR / tRTP (widened).
    let wtrs = timing_cell(regs.tc_wtr, "tWTR_S", tc_wtr_twtr_s);
    let wtrl = timing_cell(regs.tc_wtr, "tWTR_L", tc_wtr_twtr_l);
    let wr = timing_cell(regs.tc_wtr, "tWR", tc_wtr_twr);
    let rtp = timing_cell(regs.tc_wtr, "tRTP", tc_wtr_trtp);

    // TC_RFP: tRFC1 (12-bit; tREFI is decoded — no display slot).
    // H-4: the 12-bit tRFC1 field can reach 4095 for high-density dies,
    // exceeding the shared TIMING_MAX_TICKS of 2048.
    let rfc1 = timing_cell_max(regs.tc_rfp, "tRFC1", tc_rfp_trfc1, TRFC1_MAX_TICKS);

    // TC_RFP2: tRFCsb — the DDR5-specific same-bank refresh feeds the
    // `rfcsb` display slot (Tier 1 keeps it not-applicable).
    let rfcsb = timing_cell(regs.tc_rfp2, "tRFCsb", tc_rfp2_trfcsb);

    // TC_RDRD: the unchanged 4×6-bit sg / dg / dr / dd quartet.
    let rdrd = regs.tc_rdrd.map(super::legacy_ddr4::turnaround_quartet);
    let rdrd_scl = quartet_cell(rdrd, "tRDRD(sg)", 0);
    let rdrd_sc = quartet_cell(rdrd, "tRDRD(dg)", 1);
    let rdrd_sd = quartet_cell(rdrd, "tRDRD(dr)", 2);
    let rdrd_dd = quartet_cell(rdrd, "tRDRD(dd)", 3);

    // TC_WRWR: only the two documented Alder entries (sg / dg) — the
    // Tier-1 _dr/_dd rows are not extrapolated (IG-20).
    let wrwr_scl = timing_cell(regs.tc_wrwr, "tWRWR(sg)", tc_wrwr_s);
    let wrwr_sc = timing_cell(regs.tc_wrwr, "tWRWR(dg)", tc_wrwr_d);

    // tPPD (TC_ACT2) is decoded by the core but has no frozen display
    // slot (DDR5-specific; the raw stays available via sysfs).

    // The extraction is map-invariant (Research line 107: the bit ranges
    // are reused per subchannel) — `map` carries this subchannel's
    // provenance (mirror / MCL bases) for the IG-25 fallback selection
    // ([`tier3_mcl_for`]).
    let _ = map;

    // --- clocks: the shared map decodes mclk only; the gear cells layer
    // on in the caller dispatch (as in Tier 1 / Rocket Lake). N-06: the
    // structural NAs come from the shared builder.
    let mut clocks = na_clocks(&NaReason::NotApplicable);
    clocks.mclk_mhz = mclk;

    let timings = TimingSet {
        cl,
        rcwdwr,
        rcdrd,
        rp,
        ras,
        rc,
        rrds,
        rrld,
        faw,
        wtrs,
        wtrl,
        wr,
        rfc1,
        rfc2: Section::na(NaReason::NotApplicable),
        rfcsb,
        cwl,
        rtp,
        rdwr: Section::na(NaReason::NotApplicable),
        wrrd: Section::na(NaReason::NotApplicable),
        rdrd_sd,
        rdrd_dd,
        rdrd_scl,
        rdrd_sc,
        wrwr_sd: Section::na(NaReason::NotApplicable),
        wrwr_dd: Section::na(NaReason::NotApplicable),
        wrwr_scl,
        wrwr_sc,
    };

    // The legacy skeleton's "RTL" was fiction — the real IMC table has
    // no RTL. CAD bus + voltages: not exposed by the Intel IMC window
    // (unchanged from the v2.2.1 model). N-06: the structural NAs come
    // from the shared builders.
    let rtl = na_rtl(&NaReason::NotApplicable);
    let cad_bus = na_cad_bus(&NaReason::NotApplicable);
    let voltages = na_voltages(&NaReason::NotApplicable);

    IntelChannel {
        index,
        clocks,
        timings,
        cad_bus,
        voltages,
        rtl,
    }
}

// ---------------------------------------------------------------------------
// OQ-5 tile-routing probe (IG-28).
// ---------------------------------------------------------------------------

impl IntelImcRegs {
    /// OQ-5's tile-routing probe (IG-28): `true` when **every**
    /// primary-window register in this set is degenerate — `None`
    /// (failed / unreadable read) or a present-but-invalid word
    /// (`0x0` = untrained, `0xFFFF_FFFF` = bus-fault readback — the two
    /// degenerate encodings Research line 329 uses for the mirror
    /// window). On Meteor / Arrow Lake this is the runtime signature
    /// that the physical IMC sits behind the secondary SoC-tile uncore
    /// range (MCHBAR + 0x180000) the module does not expose (Research
    /// line 328) — see [`super::tile_routing_suspected`] and the
    /// pre-decode probe in [`super::decode`].
    pub fn primary_window_degenerate(&self) -> bool {
        // N-05: the canonical degenerate screen — a slot is degenerate
        // when it is absent (`None`) OR a present-but-invalid word
        // (`0x0` / `0xFFFF_FFFF`), i.e. the negation of
        // [`is_present_word`].
        let degenerate = |v: &Option<u32>| !is_present_word(*v);
        let channel_all = |c: &ChannelRegs| {
            [&c.tc_dbp,
                &c.tc_rap,
                &c.tc_rfp,
                &c.tc_rap2,
                &c.tc_rdrd,
                &c.tc_rdwr,
                &c.tc_wrrd,
                &c.tc_wrwr]
                .iter()
                .all(|v| degenerate(v))
        };
        let mcl_all = |m: &MclRegs| {
            [&m.tc_pre,
                &m.tc_act,
                &m.tc_act2,
                &m.tc_wtr,
                &m.tc_rfp,
                &m.tc_rfp2,
                &m.tc_rdrd,
                &m.tc_wrwr]
                .iter()
                .all(|v| degenerate(v))
        };
        [&self.mcbios_req, &self.mad_dimm_ch2, &self.mad_dimm_ch3]
            .iter()
            .all(|v| degenerate(v))
            && [&self.ch0, &self.ch1, &self.ch2, &self.ch3].iter().all(|c| channel_all(c))
            && [&self.mcl0, &self.mcl1].iter().all(|m| mcl_all(m))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::IntelGen;
    use crate::intel_gen::ALDER_CHANNELS;
    use crate::intel_readout::fixtures::*;

    /// The DDR5-4800 Tier-3 fixture raws: `mcbios_req = 0x12` (ratio 18
    /// @ 133.3333 MHz → 2400 MHz MCLK / 4800 MT/s — the same word as
    /// the Skylake acceptance test) + the widened DDR5-4800 40-class
    /// timings (CL40-40-40-77 — a tCL 40 that only the 8-bit `TC_ACT`
    /// field carries, and a 1200-class tRFC1 that only the 12-bit
    /// field carries).
    fn tier3_ddr5_4800_mcl() -> MclRegs {
        MclRegs {
            // TC_PRE: tRP 40 [6:0], tRCD 40 [14:8], tRAS 77 [23:16],
            // tCWL 40 [30:24].
            tc_pre: Some(0x284D_2828),
            // TC_ACT: tFAW 16 [7:0], tRRD_S 4 [11:8], tRRD_L 8 [15:12],
            // tCL 40 [23:16].
            tc_act: Some(0x0028_8410),
            // TC_ACT2: tPPD 4 [3:0] (decoded — no display slot).
            tc_act2: Some(0x4),
            // TC_WTR: tWTR_S 12 [6:0], tWTR_L 14 [14:8], tWR 40 [23:16],
            // tRTP 8 [30:24].
            tc_wtr: Some(0x0828_0E0C),
            // TC_RFP: tRFC1 1200 [11:0], tREFI 8192 [31:16] (multiplier —
            // no display slot).
            tc_rfp: Some(0x2000_04B0),
            // TC_RFP2: tRFCsb 135 [10:0] (DDR5-specific → the `rfcsb`
            // slot).
            tc_rfp2: Some(0x87),
            // TC_RDRD: the unchanged 4×6-bit sg 6 / dg 10 / dr 12 / dd 18.
            tc_rdrd: Some(0x0048_C286),
            // TC_WRWR: only the two documented entries — sg 6 / dg 10.
            tc_wrwr: Some(0x286),
        }
    }

    /// The DDR5-4800 widened decode, pinned end to end: the 40-class
    /// tCL / tCWL / tRCD / tRP / tWR across the widened fields, tRFC1
    /// 1200 in the 12-bit field, tRFCsb 135 in the `rfcsb` slot, the
    /// unchanged `TC_RDRD` quartet, the two documented `TC_WRWR`
    /// entries (the _dr/_dd rows not-applicable), and `rc = tRAS + tRP
    /// = 117`.
    #[test]
    fn tier3_ddr5_4800_pins_the_widened_40_class_values() {
        let mcl = tier3_ddr5_4800_mcl();
        // DDR5-4800 on the 133.3333 MHz ref (the 11th-gen-and-later
        // half-step encoding): ratio 36 = 4800 / 133.3333 →
        // 36 × 133.3333 / 2 = 2400 MHz MCLK.
        let mclk = super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake);
        assert_eq!(mclk, Section::Value(2400.0), "DDR5-4800: 2400 MHz MCLK");
        let ch = decode_tier3_channel(0, mclk, &mcl, &ALDER_CHANNELS[0]);
        assert_eq!(ch.index, 0);

        // --- clocks (the shared mclk; the gear cells layer on later) ---
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0));
        assert_eq!(ch.clocks.uclk_mhz, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.fclk_mhz, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.div_mode, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.gear_mode, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.gdm, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.pdm, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.command_rate, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.rtl, Section::na(NaReason::NotApplicable));

        // --- the widened DDR5-4800 40-class timings --------------------
        assert_eq!(ch.timings.cl, Section::Value(40), "tCL 8-bit [23:16]");
        assert_eq!(ch.timings.cwl, Section::Value(40), "tCWL 7-bit [30:24]");
        assert_eq!(ch.timings.rcdrd, Section::Value(40), "tRCD 7-bit [14:8]");
        assert_eq!(
            ch.timings.rcwdwr,
            Section::Value(40),
            "the unified symmetric tRCD feeds both rcdrd and rcwdwr"
        );
        assert_eq!(ch.timings.rp, Section::Value(40), "tRP 7-bit [6:0]");
        assert_eq!(ch.timings.ras, Section::Value(77), "tRAS 8-bit [23:16]");
        assert_eq!(
            ch.timings.rc,
            Section::Value(117),
            "tRAS + tRP = 77 + 40 (synthesized — Intel exposes no tRC)"
        );
        assert_eq!(ch.timings.rrds, Section::Value(4), "tRRD_S 4-bit [11:8]");
        assert_eq!(ch.timings.rrld, Section::Value(8), "tRRD_L 4-bit [15:12]");
        assert_eq!(ch.timings.faw, Section::Value(16), "tFAW 8-bit [7:0]");
        assert_eq!(ch.timings.wtrs, Section::Value(12), "tWTR_S 7-bit [6:0]");
        assert_eq!(ch.timings.wtrl, Section::Value(14), "tWTR_L 7-bit [14:8]");
        assert_eq!(ch.timings.wr, Section::Value(40), "tWR 8-bit [23:16]");
        assert_eq!(ch.timings.rtp, Section::Value(8), "tRTP 7-bit [30:24]");
        assert_eq!(ch.timings.rfc1, Section::Value(1200), "tRFC1 12-bit [11:0]");
        assert_eq!(
            ch.timings.rfcsb,
            Section::Value(135),
            "tRFCsb 11-bit [10:0] → the DDR5 `rfcsb` slot"
        );

        // --- the unchanged TC_RDRD quartet + the two TC_WRWR entries ----
        assert_eq!(ch.timings.rdrd_scl, Section::Value(6), "sg");
        assert_eq!(ch.timings.rdrd_sc, Section::Value(10), "dg");
        assert_eq!(ch.timings.rdrd_sd, Section::Value(12), "dr");
        assert_eq!(ch.timings.rdrd_dd, Section::Value(18), "dd");
        assert_eq!(ch.timings.wrwr_scl, Section::Value(6), "sg (documented)");
        assert_eq!(ch.timings.wrwr_sc, Section::Value(10), "dg (documented)");

        // --- the slots with no Tier-3 source ----------------------------
        for s in [
            &ch.timings.rfc2,
            &ch.timings.rdwr,
            &ch.timings.wrrd,
            &ch.timings.wrwr_sd,
            &ch.timings.wrwr_dd,
        ] {
            assert_eq!(*s, Section::na(NaReason::NotApplicable), "{s:?}");
        }

        // --- CAD bus + voltages: never exposed by the IMC window --------
        for s in [
            &ch.cad_bus.proc_odt,
            &ch.cad_bus.clk_drv,
            &ch.cad_bus.addr_cmd_drv,
            &ch.cad_bus.cs_odt_drv,
            &ch.cad_bus.cke_drv,
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }
        for s in [&ch.cad_bus.rtt_nom, &ch.cad_bus.rtt_wr, &ch.cad_bus.rtt_park] {
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

    /// The all-absent Tier-3 arm: every `MclRegs` register absent (and
    /// an absent `MC_BIOS_REQ` → `Na` mclk) degrades the whole channel
    /// to honest `Na` — no panic, no garbage values (D5).
    #[test]
    fn tier3_all_absent_degrades_all_na() {
        let ch = decode_tier3_channel(
            2,
            super::super::decode_mclk(None, IntelGen::AlderLake),
            &MclRegs::default(),
            &ALDER_CHANNELS[2],
        );
        assert_eq!(ch.index, 2);
        assert!(ch.clocks.mclk_mhz.is_na(), "{:?}", ch.clocks.mclk_mhz);
        assert!(ch.clocks.uclk_mhz.is_na(), "{:?}", ch.clocks.uclk_mhz);
        assert!(ch.clocks.fclk_mhz.is_na(), "{:?}", ch.clocks.fclk_mhz);
        assert!(ch.clocks.gdm.is_na(), "{:?}", ch.clocks.gdm);
        assert!(ch.clocks.pdm.is_na(), "{:?}", ch.clocks.pdm);
        assert!(ch.clocks.div_mode.is_na(), "{:?}", ch.clocks.div_mode);
        assert!(ch.clocks.gear_mode.is_na(), "{:?}", ch.clocks.gear_mode);
        assert!(
            ch.clocks.command_rate.is_na(),
            "{:?}",
            ch.clocks.command_rate
        );
        assert_eq!(ch.rtl, Section::na(NaReason::NotApplicable));
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
    }

    /// Per-register containment (Tier 3): only `tc_pre` unreadable →
    /// only its sourced fields (tRCD / tRP / tRAS / tCWL — and the
    /// derived `rc`) degrade to `Na(ParseError)`; every other
    /// register's fields decode untouched.
    #[test]
    fn tier3_per_register_containment() {
        let mut mcl = tier3_ddr5_4800_mcl();
        mcl.tc_pre = None;
        let ch = decode_tier3_channel(
            1,
            super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake),
            &mcl,
            &ALDER_CHANNELS[1],
        );
        assert_eq!(ch.index, 1);
        for s in [
            &ch.timings.rcdrd,
            &ch.timings.rcwdwr,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.cwl,
        ] {
            assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
        }
        assert_eq!(ch.timings.rc, Section::na(NaReason::NotApplicable));
        // The other registers' fields decode untouched.
        assert_eq!(ch.timings.cl, Section::Value(40));
        assert_eq!(ch.timings.faw, Section::Value(16));
        assert_eq!(ch.timings.rrds, Section::Value(4));
        assert_eq!(ch.timings.rrld, Section::Value(8));
        assert_eq!(ch.timings.wtrs, Section::Value(12));
        assert_eq!(ch.timings.wtrl, Section::Value(14));
        assert_eq!(ch.timings.wr, Section::Value(40));
        assert_eq!(ch.timings.rtp, Section::Value(8));
        assert_eq!(ch.timings.rfc1, Section::Value(1200));
        assert_eq!(ch.timings.rfcsb, Section::Value(135));
        assert_eq!(ch.timings.rdrd_dd, Section::Value(18));
        assert_eq!(ch.timings.wrwr_sc, Section::Value(10));
    }

    /// Research line 107: the widened bit ranges apply to Controller 0
    /// / Subchannel 0 and are reused per subchannel — the same raws
    /// decode to identical timings under every `ALDER_CHANNELS`
    /// descriptor (the extraction is map-invariant; only the channel
    /// index differs).
    #[test]
    fn tier3_extraction_is_map_invariant_across_subchannels() {
        let mcl = tier3_ddr5_4800_mcl();
        let mclk = super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake);
        let mut decoded = Vec::new();
        for (i, map) in ALDER_CHANNELS.iter().enumerate() {
            let ch = decode_tier3_channel(i as u8, mclk.clone(), &mcl, map);
            assert_eq!(ch.index, i as u8);
            decoded.push(ch);
        }
        let ref_ch0 = decoded[0].clone();
        for ch in &decoded[1..] {
            assert_eq!(ch.timings, ref_ch0.timings, "map-invariant extraction");
            assert_eq!(ch.clocks, ref_ch0.clocks);
            assert_eq!(ch.cad_bus, ref_ch0.cad_bus);
            assert_eq!(ch.voltages, ref_ch0.voltages);
        }
    }

    /// The widened field widths, pinned at their maxima (the widening
    /// over the Tier-1 6-bit fields is the point): tCL 8-bit → 255
    /// (the 40–56 DDR5 class fits), tRCD / tRP / tWTR_S / tWTR_L
    /// 7-bit → 127, tRFC1 12-bit → 4095, tRFCsb 11-bit → 2047, tPPD
    /// 4-bit → 15 (decoded, no display slot) — and the masks never
    /// alias into neighboring fields.
    #[test]
    fn tier3_widened_field_widths_at_maximums() {
        // 8-bit tCL max (a 6-bit mask would truncate to 63).
        assert_eq!(tc_act_tcl(0x00FF_0000), 255);
        // 7-bit fields max.
        assert_eq!(tc_pre_trcd(0x0000_7F00), 127);
        assert_eq!(tc_pre_trp(0x7F), 127);
        assert_eq!(tc_wtr_twtr_s(0x7F), 127);
        assert_eq!(tc_wtr_twtr_l(0x0000_7F00), 127);
        // 12-bit tRFC1 / 11-bit tRFCsb max.
        assert_eq!(tc_rfp_trfc1(0x0000_0FFF), 4095);
        assert_eq!(tc_rfp2_trfcsb(0x0000_07FF), 2047);
        // 4-bit tPPD max (decoded, no display slot).
        assert_eq!(tc_act2_tppd(0x0F), 15);
        // The masks never alias into a neighboring field: the fixture
        // words decode their fields independently of the rest.
        assert_eq!(tc_act_tcl(0x0028_8410), 40);
        assert_eq!(tc_pre_tcwl(0x284D_2828), 40);
        assert_eq!(tc_wtr_twr(0x0828_0E0C), 40);
        assert_eq!(tc_pre_tras(0x284D_2828), 77);
    }

    /// The verbatim Research line 329 selector: a disabled-mirror word
    /// (`0x0` / `0xFFFFFFFF`) selects the MCL register; a populated
    /// mirror wins; a failed mirror read is not a disabled mirror (no
    /// fallback); an absent MCL read yields nothing (never a bogus
    /// zero).
    #[test]
    fn mirror_or_mcl_selects_per_research_line_329() {
        assert_eq!(
            mirror_or_mcl(Some(0xFFFF_FFFF), Some(0x0028_8410)),
            Some(0x0028_8410),
            "0x4004 = 0xFFFFFFFF + valid 0xD004 -> MCL wins"
        );
        assert_eq!(
            mirror_or_mcl(Some(0x0), Some(0x0028_8410)),
            Some(0x0028_8410),
            "0x4004 = 0x0 + valid 0xD004 -> MCL wins"
        );
        assert_eq!(
            mirror_or_mcl(Some(0x0028_8410), Some(0xDEAD_BEEF)),
            Some(0x0028_8410),
            "0x4004 valid -> mirror wins"
        );
        assert_eq!(
            mirror_or_mcl(None, Some(0x0028_8410)),
            None,
            "a failed mirror read is not a disabled mirror"
        );
        assert_eq!(
            mirror_or_mcl(Some(0xFFFF_FFFF), None),
            None,
            "disabled mirror + absent MCL -> nothing (no bogus zero)"
        );
        assert_eq!(mirror_or_mcl(None, None), None);
    }

    /// The brief's fallback arm, wired per controller: MC0 subchannel 0
    /// with a disabled mirror word at 0x4004 (`TC_ACT` @ the `ch0`
    /// block +0x04) and a valid native MCL word at 0xD004 → the MCL
    /// value wins end to end through the Tier-3 decode (tCL 40 from
    /// 0xD004, not the disabled mirror).
    #[test]
    fn mirror_fallback_disabled_mirror_selects_mcl() {
        let regs = IntelImcRegs {
            ch0: ChannelRegs {
                tc_rap: Some(0xFFFF_FFFF), // 0x4004: mirror disabled
                ..ChannelRegs::default()
            },
            mcl0: tier3_ddr5_4800_mcl(), // 0xD004 = 0x0028_8410 (valid)
            ..IntelImcRegs::default()
        };
        let mcl = tier3_mcl_for(0, &ALDER_CHANNELS[0], &regs);
        assert_eq!(
            mcl.tc_act,
            Some(0x0028_8410),
            "0xD004 wins over the disabled 0x4004"
        );
        // The MCL-only registers ride the MCL block as-is.
        assert_eq!(mcl.tc_wtr, Some(0x0828_0E0C));
        assert_eq!(mcl.tc_rfp, Some(0x2000_04B0));
        assert_eq!(mcl.tc_rfp2, Some(0x87));
        // The un-fallen mirror registers (absent here) stay absent.
        assert_eq!(mcl.tc_pre, None);
        // End to end: the decoded channel carries the MCL-sourced tCL.
        let ch = decode_tier3_channel(
            0,
            super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake),
            &mcl,
            &ALDER_CHANNELS[0],
        );
        assert_eq!(ch.timings.cl, Section::Value(40), "tCL from 0xD004");
    }

    /// The brief's mirror-wins arm: a populated mirror word beats the
    /// MCL word (the retail-desktop default — the mirror is enabled);
    /// pinned for both MCL-bearing controllers (MC0 subch0 and MC1
    /// subch2).
    #[test]
    fn mirror_wins_when_populated() {
        // MC0 subch0: a populated 0x4004 wins over the MCL 0xD004.
        let regs = IntelImcRegs {
            ch0: ChannelRegs {
                tc_rap: Some(0x0028_8410), // 0x4004 valid
                ..ChannelRegs::default()
            },
            mcl0: tier3_ddr5_4800_mcl(), // 0xD004
            ..IntelImcRegs::default()
        };
        let mcl = tier3_mcl_for(0, &ALDER_CHANNELS[0], &regs);
        assert_eq!(mcl.tc_act, Some(0x0028_8410), "0x4004 valid -> mirror wins");

        // MC1 subch2: a distinct valid 0x4804 beats a distinct 0xD804.
        let regs = IntelImcRegs {
            ch2: ChannelRegs {
                tc_rap: Some(0x0018_0410), // 0x4804 valid (tCL 24)
                ..ChannelRegs::default()
            },
            mcl1: MclRegs {
                tc_act: Some(0x0028_8410), // 0xD804 (tCL 40)
                ..MclRegs::default()
            },
            ..IntelImcRegs::default()
        };
        let mcl2 = tier3_mcl_for(2, &ALDER_CHANNELS[2], &regs);
        assert_eq!(mcl2.tc_act, Some(0x0018_0410), "0x4804 valid -> mirror wins");
        let ch = decode_tier3_channel(
            2,
            super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake),
            &mcl2,
            &ALDER_CHANNELS[2],
        );
        assert_eq!(ch.timings.cl, Section::Value(24), "tCL 24 from the mirror");
    }

    /// The brief's both-absent arm: no mirror read and no MCL read →
    /// the effective register set is all-`None` → the Tier-3 decode
    /// degrades every sourced slot to `Na(ParseError)` (never a value,
    /// never a panic); the mirror-only subchannels (OQ-4) degrade the
    /// same way with no MCL base to fall back to.
    #[test]
    fn mirror_and_mcl_both_absent_degrade_to_parse_error() {
        let mcl = tier3_mcl_for(0, &ALDER_CHANNELS[0], &IntelImcRegs::default());
        assert_eq!(mcl, MclRegs::default());
        let ch = decode_tier3_channel(
            0,
            super::super::decode_mclk(Some(0x0000_0024), IntelGen::AlderLake),
            &mcl,
            &ALDER_CHANNELS[0],
        );
        for s in [
            &ch.timings.cl,
            &ch.timings.rcdrd,
            &ch.timings.rfc1,
            &ch.timings.rdrd_scl,
            &ch.timings.wrwr_sc,
        ] {
            assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
        }
        // OQ-4: the mirror-only subchannels — no MCL base → the same
        // all-absent effective set.
        assert_eq!(
            tier3_mcl_for(1, &ALDER_CHANNELS[1], &IntelImcRegs::default()),
            MclRegs::default()
        );
        assert_eq!(
            tier3_mcl_for(3, &ALDER_CHANNELS[3], &IntelImcRegs::default()),
            MclRegs::default()
        );
    }

    /// Mirror-only subchannels (OQ-4): a populated mirror block is
    /// taken as-is under the Alder layout naming (the mirror is a
    /// legacy alias of the MCL block); the MCL-only registers have no
    /// source and stay absent.
    #[test]
    fn mirror_only_subchannels_take_the_mirror_as_is() {
        let regs = IntelImcRegs {
            ch1: ChannelRegs {
                tc_dbp: Some(0x284D_2828), // +0x00
                tc_rap: Some(0x0028_8410), // +0x04
                tc_rfp: Some(0x4),         // +0x08
                tc_rdrd: Some(0x0048_C286), // +0x20
                tc_wrrd: Some(0x286),      // +0x28
                ..ChannelRegs::default()
            },
            ..IntelImcRegs::default()
        };
        let mcl = tier3_mcl_for(1, &ALDER_CHANNELS[1], &regs);
        assert_eq!(mcl.tc_pre, Some(0x284D_2828));
        assert_eq!(mcl.tc_act, Some(0x0028_8410));
        assert_eq!(mcl.tc_act2, Some(0x4));
        assert_eq!(mcl.tc_rdrd, Some(0x0048_C286));
        assert_eq!(mcl.tc_wrwr, Some(0x286));
        assert_eq!(mcl.tc_wtr, None);
        assert_eq!(mcl.tc_rfp, None);
        assert_eq!(mcl.tc_rfp2, None);
    }

    /// H-3: `mirror_or_mcl` screens the MCL fallback for degenerates
    /// too — a bus-faulted MCL word is not a valid fallback source.
    #[test]
    fn mirror_or_mcl_screens_mcl_for_degenerates() {
        // Degenerate mirror + degenerate MCL -> None (no bogus value).
        assert_eq!(
            mirror_or_mcl(Some(0xFFFF_FFFF), Some(0xFFFF_FFFF)),
            None,
            "bus-fault mirror + bus-fault MCL -> None"
        );
        assert_eq!(
            mirror_or_mcl(Some(0), Some(0)),
            None,
            "zero mirror + zero MCL -> None"
        );
        // Degenerate mirror + valid MCL -> MCL wins.
        assert_eq!(
            mirror_or_mcl(Some(0xFFFF_FFFF), Some(0x0028_8410)),
            Some(0x0028_8410),
        );
        // Valid mirror + degenerate MCL -> mirror wins (MCL not used).
        assert_eq!(
            mirror_or_mcl(Some(0x0028_8410), Some(0xFFFF_FFFF)),
            Some(0x0028_8410),
        );
    }

    /// H-4: the widened DDR5 tRFC1 field (12-bit, max 4095) decodes
    /// values above the shared TIMING_MAX_TICKS of 2048 without
    /// degrading to Na.
    #[test]
    fn h4_trfc1_12bit_exceeds_shared_max() {
        // tRFC1 = 3000 (above 2048, within the 12-bit 4095 max).
        let mcl = MclRegs {
            tc_rfp: Some(0x0000_0BB8), // tRFC1 [11:0] = 3000
            ..MclRegs::default()
        };
        let ch = decode_tier3_channel(
            0,
            Section::Value(2400.0),
            &mcl,
            &ALDER_CHANNELS[0],
        );
        assert_eq!(
            ch.timings.rfc1,
            Section::Value(3000),
            "tRFC1 3000 is in-band for the 12-bit field (H-4)"
        );

        // tRFC1 = 4095 (the 12-bit max) is in-band.
        let mcl = MclRegs {
            tc_rfp: Some(0x0000_0FFF), // tRFC1 [11:0] = 4095
            ..MclRegs::default()
        };
        let ch = decode_tier3_channel(
            0,
            Section::Value(2400.0),
            &mcl,
            &ALDER_CHANNELS[0],
        );
        assert_eq!(
            ch.timings.rfc1,
            Section::Value(4095),
            "tRFC1 4095 is the 12-bit max (H-4)"
        );
    }

    /// The probe fires on every fully degenerate form — all-`None`,
    /// all-zero, all-bus-fault, and mixed 0x0 / 0xFFFF_FFFF raws —
    /// and not when any single slot is valid (the populated Tier-3
    /// fixture, and a lone valid `MC_BIOS_REQ` among zeros).
    #[test]
    fn ig28_primary_window_degenerate_probe() {
        // The probe is generation-agnostic (the closures answer every
        // offset the same), so any gen keys the reader; use Alder Lake.
        let gen = IntelGen::AlderLake;
        let imc_base = super::super::imc_base_for(gen);
        assert!(IntelImcRegs::default().primary_window_degenerate(), "all-None");
        let all_zero = IntelImcRegs::from_wide_reader(imc_base, |_off: usize| Some(0u32));
        assert!(all_zero.primary_window_degenerate(), "all-zero");
        let all_fault = IntelImcRegs::from_wide_reader(imc_base, |_off: usize| Some(0xFFFF_FFFFu32));
        assert!(all_fault.primary_window_degenerate(), "all-bus-fault");
        let mixed = IntelImcRegs::from_wide_reader(imc_base, |off: usize| {
            if off & 1 == 0 {
                Some(0u32)
            } else {
                Some(0xFFFF_FFFFu32)
            }
        });
        assert!(mixed.primary_window_degenerate(), "mixed 0x0 / 0xFFFF_FFFF");
        assert!(
            !tier3_ddr5_condition_regs().primary_window_degenerate(),
            "populated Tier-3 fixture"
        );
        let one_valid = IntelImcRegs::from_wide_reader(imc_base, |off: usize| {
            if off == super::super::MC_BIOS_REQ_OFFSET {
                Some(0x0001_0012)
            } else {
                Some(0u32)
            }
        });
        assert!(!one_valid.primary_window_degenerate(), "one valid slot");
    }
}
