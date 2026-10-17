//! Phase 1 (Intel MC fix): the Alder Lake **DDR4** decode path — the
//! `ALDER_DDR4_FIELDS` bitfield decoders (Intel 12th Gen Datasheet Vol 2
//! §3.2.66–3.2.87), the per-channel decode core, and the full DDR4
//! readout.
//!
//! Step 2 of the `intel_readout` directory-module split — moved verbatim
//! from `mod.rs`.

use super::gates::*;
use super::types::*;

use crate::amd_readout::TimingSet;
use crate::cpuid::IntelGen;
use crate::error::{NaReason, Section};
use crate::intel_gen::{
    AlderDdr4Fields, ALDER_DDR4_CHANNELS, ALDER_DDR4_FIELDS,
};

// ---------------------------------------------------------------------------
// Phase 1 (Intel MC fix): the Alder Lake **DDR4** bitfield decoders — the
// `ALDER_DDR4_FIELDS` ranges (Intel 12th Gen Datasheet Vol 2
// §3.2.66–3.2.87). `TC_PRE` is a 64-bit register read as a low word
// (`+0x00`) and a high word (`+0x04`): a field's datasheet `[hi:lo]`
// position maps to the high word at `[hi-32 : lo-32]` when `lo >= 32`,
// else to the low word at `[hi:lo]`. Every other register is a plain
// 32-bit word. The 18-bit `tREFI` (`TC_RFTP [17:0]`) exceeds the `u16`
// `field16` contract and has no frozen display slot, so it is not decoded
// here (the raw word stays available via the sysfs attributes).
//
// N-09: every extractor here takes the field table as a parameter
// (`&AlderDdr4Fields`; the live caller passes `&ALDER_DDR4_FIELDS`), so a
// future DDR4 generation is a data change in `intel_gen.rs`, not a code
// change in this module.
// ---------------------------------------------------------------------------

/// tRCD — `TC_PRE` 64-bit [58:51] = high word [26:19].
pub(super) fn alk_d4_pre_trcd(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trcd.0 - TC_PRE_HI_WORD_SHIFT, f.trcd.1 - TC_PRE_HI_WORD_SHIFT)
}

/// tRAS — `TC_PRE` 64-bit [50:42] = high word [18:10].
pub(super) fn alk_d4_pre_tras(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.tras.0 - TC_PRE_HI_WORD_SHIFT, f.tras.1 - TC_PRE_HI_WORD_SHIFT)
}

/// tWRPRE — `TC_PRE` 64-bit [41:32] = high word [9:0] (decoded — no
/// frozen display slot).
pub fn alk_d4_pre_wrpre(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.twrpre.0 - TC_PRE_HI_WORD_SHIFT, f.twrpre.1 - TC_PRE_HI_WORD_SHIFT)
}

/// tPPD — `TC_PRE` 64-bit [23:20] (low word; decoded — no display slot).
pub fn alk_d4_pre_tppd(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.tppd.0, f.tppd.1)
}

/// tRDPRE — `TC_PRE` 64-bit [19:13] (low word; decoded — no display slot).
pub fn alk_d4_pre_rdpre(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trdpre.0, f.trdpre.1)
}

/// tRP — `TC_PRE` 64-bit [7:0] (low word).
pub(super) fn alk_d4_pre_trp(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trp.0, f.trp.1)
}

/// tRRD_DG — `TC_ACT` [21:15] (the DDR4 long row-to-row delay).
pub(super) fn alk_d4_act_trrd_dg(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trrd_dg.0, f.trrd_dg.1)
}

/// tRRD_SG — `TC_ACT` [14:9] (the DDR4 short row-to-row delay).
pub(super) fn alk_d4_act_trrd_sg(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trrd_sg.0, f.trrd_sg.1)
}

/// tFAW — `TC_ACT` [8:0] (9-bit).
pub(super) fn alk_d4_act_tfaw(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.tfaw.0, f.tfaw.1)
}

/// tCWL — `TC_ODT` [31:24].
pub(super) fn alk_d4_odt_tcwl(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.tcwl.0, f.tcwl.1)
}

/// tCL — `TC_ODT` [22:16].
pub(super) fn alk_d4_odt_tcl(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.tcl.0, f.tcl.1)
}

/// tRFC — `TC_RFTP` [30:18] (13-bit).
pub(super) fn alk_d4_rftp_trfc(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trfc.0, f.trfc.1)
}

/// tREFI×9 — `TC_RFP` [31:24] (decoded — no frozen display slot).
pub fn alk_d4_rfp_trefi9(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.trefi9.0, f.trefi9.1)
}

/// OREF_RI — `TC_RFP` [7:0] (rank-idle period; decoded — no display slot).
pub fn alk_d4_rfp_oref_ri(reg: u32, f: &AlderDdr4Fields) -> u16 {
    field16(reg, f.oref_ri.0, f.oref_ri.1)
}

/// The Alder Lake DDR4 turnaround quartet of one `TC_*` register:
/// `(sg, dg, dr, dd)`, each extracted from its own field-table range
/// (the DDR4 widths differ per register — 7/8/9-bit — so this is not the
/// Tier-1 4×6-bit [`super::legacy_ddr4::turnaround_quartet`]).
pub(super) fn alk_d4_turnaround(
    reg: u32,
    which: WhichAlkD4Turnaround,
    f: &AlderDdr4Fields,
) -> [u16; 4] {
    let (sg, dg, dr, dd) = match which {
        WhichAlkD4Turnaround::Rdrd => (f.rdrd_sg, f.rdrd_dg, f.rdrd_dr, f.rdrd_dd),
        WhichAlkD4Turnaround::Rdwr => (f.rdwr_sg, f.rdwr_dg, f.rdwr_dr, f.rdwr_dd),
        WhichAlkD4Turnaround::Wrrd => (f.wrrd_sg, f.wrrd_dg, f.wrrd_dr, f.wrrd_dd),
        WhichAlkD4Turnaround::Wrwr => (f.wrwr_sg, f.wrwr_dg, f.wrwr_dr, f.wrwr_dd),
    };
    [
        field16(reg, sg.0, sg.1),
        field16(reg, dg.0, dg.1),
        field16(reg, dr.0, dr.1),
        field16(reg, dd.0, dd.1),
    ]
}

// ---------------------------------------------------------------------------
// The Alder Lake DDR4 per-channel decode core.
// ---------------------------------------------------------------------------

/// Decode one Alder Lake DDR4 channel from its raw [`AlderDdr4TcRegs`]
/// block + the shared DRAM core clock + the DDR4 field table (N-09:
/// the caller passes `&ALDER_DDR4_FIELDS`, so a future DDR4 generation
/// is a data change, not a code change) — the per-channel heart of the
/// DDR4 decode path, Phase 1c. Never panics (D5): every cell is a
/// `Value`, an honest `Na(ParseError)` (a sourced register failed / a
/// field is untrained), or a structural `Na(NotApplicable)`.
///
/// The 64-bit `TC_PRE` is split: the high word (`+0x04`) sources
/// tRCD / tRAS / tWRPRE; the low word (`+0x00`) sources tPPD / tRDPRE /
/// tRP. `tCL` / `tCWL` come from `TC_ODT`; `tFAW` / `tRRD_SG` /
/// `tRRD_DG` from `TC_ACT`; `tRFC` from `TC_RFTP`. The DDR4 turnaround
/// quartets (`TC_RDRD` / `TC_RDWR` / `TC_WRRD` / `TC_WRWR`) use the
/// per-register field-table widths (not the Tier-1 4×6-bit layout).
/// `tWR` / `tRTP` / `rfc2` / `rfcsb` have no slot in the DDR4
/// `TC` layout → `Na(NotApplicable)`. The `rc = ras + rp` synthesis and
/// the [1, 2048] tick gate are preserved (Breakdown §6). The gear cells
/// layer on in the caller dispatch (as in Tier 1 / Tier 3).
///
/// N-03: like the Tier-1 / Tier-3 siblings (H-2), the raw words are
/// normalized through the canonical degenerate screen at the top — a
/// bus-fault / untrained word never fabricates a turnaround-quartet
/// value (the DDR4 quartet path has no per-word screen of its own).
pub(super) fn decode_alder_ddr4_channel(
    index: u8,
    mclk: Section<f64>,
    regs: &AlderDdr4TcRegs,
    fields: &AlderDdr4Fields,
) -> IntelChannel {
    // N-03: Screen degenerate words (bus-fault / untrained) — same as
    // the Tier-1 and Tier-3 siblings (H-2).
    let regs = AlderDdr4TcRegs {
        tc_pre_lo: regs.tc_pre_lo.filter(|&w| !is_degenerate_word(w)),
        tc_pre_hi: regs.tc_pre_hi.filter(|&w| !is_degenerate_word(w)),
        tc_act: regs.tc_act.filter(|&w| !is_degenerate_word(w)),
        tc_rdrd: regs.tc_rdrd.filter(|&w| !is_degenerate_word(w)),
        tc_rdwr: regs.tc_rdwr.filter(|&w| !is_degenerate_word(w)),
        tc_wrrd: regs.tc_wrrd.filter(|&w| !is_degenerate_word(w)),
        tc_wrwr: regs.tc_wrwr.filter(|&w| !is_degenerate_word(w)),
        tc_odt: regs.tc_odt.filter(|&w| !is_degenerate_word(w)),
        tc_rfp: regs.tc_rfp.filter(|&w| !is_degenerate_word(w)),
        tc_rftp: regs.tc_rftp.filter(|&w| !is_degenerate_word(w)),
    };

    // TC_PRE (64-bit): the unified symmetric tRCD feeds both the rcdrd and
    // the rcwdwr slot; tRAS / tRP (the low word carries tRP).
    let rcdrd = timing_cell(regs.tc_pre_hi, "tRCD", |w| alk_d4_pre_trcd(w, fields));
    let rcwdwr = rcdrd.clone();
    let ras = timing_cell(regs.tc_pre_hi, "tRAS", |w| alk_d4_pre_tras(w, fields));
    let rp = timing_cell(regs.tc_pre_lo, "tRP", |w| alk_d4_pre_trp(w, fields));

    // TC_ODT: tCL / tCWL.
    let cl = timing_cell(regs.tc_odt, "tCL", |w| alk_d4_odt_tcl(w, fields));
    let cwl = timing_cell(regs.tc_odt, "tCWL", |w| alk_d4_odt_tcwl(w, fields));

    // TC_ACT: tRRD_SG (short) / tRRD_DG (long) / tFAW.
    let rrds = timing_cell(regs.tc_act, "tRRD_SG", |w| alk_d4_act_trrd_sg(w, fields));
    let rrld = timing_cell(regs.tc_act, "tRRD_DG", |w| alk_d4_act_trrd_dg(w, fields));
    let faw = timing_cell(regs.tc_act, "tFAW", |w| alk_d4_act_tfaw(w, fields));

    // tRC is synthesized (the JEDEC identity — Intel exposes no tRC
    // register): `tRAS + tRP` when both decode.
    let rc = match (ras.value().copied(), rp.value().copied()) {
        (Some(a), Some(b)) => ticks_section(a.saturating_add(b), "tRC (tRAS + tRP)"),
        _ => Section::na(NaReason::NotApplicable),
    };

    // TC_RFTP: tRFC (13-bit).
    let rfc1 = timing_cell(regs.tc_rftp, "tRFC", |w| alk_d4_rftp_trfc(w, fields));

    // The DDR4 turnaround quartets (per-register widths).
    let rdrd = regs
        .tc_rdrd
        .map(|r| alk_d4_turnaround(r, WhichAlkD4Turnaround::Rdrd, fields));
    let rdwr = regs
        .tc_rdwr
        .map(|r| alk_d4_turnaround(r, WhichAlkD4Turnaround::Rdwr, fields));
    let wrrd = regs
        .tc_wrrd
        .map(|r| alk_d4_turnaround(r, WhichAlkD4Turnaround::Wrrd, fields));
    let wrwr = regs
        .tc_wrwr
        .map(|r| alk_d4_turnaround(r, WhichAlkD4Turnaround::Wrwr, fields));
    let rdrd_scl = quartet_cell(rdrd, "tRDRD(sg)", 0);
    let rdrd_sc = quartet_cell(rdrd, "tRDRD(dg)", 1);
    let rdrd_sd = quartet_cell(rdrd, "tRDRD(dr)", 2);
    let rdrd_dd = quartet_cell(rdrd, "tRDRD(dd)", 3);
    // `rdwr` ← TC_RDWR dg (the representative bank-group turnaround).
    let rdwr = quartet_cell(rdwr, "tRDWR(dg)", 1);
    // tWTR_S = dg, tWTR_L = sg (research-doc parity matrix).
    let wtrs = quartet_cell(wrrd, "tWTR_S(dg)", 1);
    let wtrl = quartet_cell(wrrd, "tWTR_L(sg)", 0);
    let wrwr_scl = quartet_cell(wrwr, "tWRWR(sg)", 0);
    let wrwr_sc = quartet_cell(wrwr, "tWRWR(dg)", 1);
    let wrwr_sd = quartet_cell(wrwr, "tWRWR(dr)", 2);
    let wrwr_dd = quartet_cell(wrwr, "tWRWR(dd)", 3);

    // --- clocks: the shared map decodes mclk only; the gear cells layer
    // on in the caller dispatch (GearCap::Gear4 — the Alder profile).
    // N-06: the structural NAs come from the shared builder. ---
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
        wr: Section::na(NaReason::NotApplicable),
        rfc1,
        rfc2: Section::na(NaReason::NotApplicable),
        rfcsb: Section::na(NaReason::NotApplicable),
        cwl,
        rtp: Section::na(NaReason::NotApplicable),
        rdwr,
        wrrd: Section::na(NaReason::NotApplicable),
        rdrd_sd,
        rdrd_dd,
        rdrd_scl,
        rdrd_sc,
        wrwr_sd,
        wrwr_dd,
        wrwr_scl,
        wrwr_sc,
    };

    // The legacy skeleton's "RTL" was fiction — the real IMC table has
    // no RTL. N-06: the structural NAs come from the shared builders.
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

/// Decode the full Alder Lake **DDR4** readout (Phase 1c): the two
/// per-controller channels (`ALDER_DDR4_CHANNELS`) decoded through
/// [`decode_alder_ddr4_channel`] from the `0xE000`/`0x1E000` TC blocks,
/// with the gear cells layered on from `MC_BIOS_REQ[17:16]`
/// (GearCap::Gear4). The channel-mode / tech / trust / annotation /
/// dimm-count slots carry the 4-tier [`ChannelResolution`] the caller
/// already computed (shared with the other decode arms).
pub(super) fn decode_alder_ddr4(
    regs: &IntelImcRegs,
    gen: IntelGen,
    res: &ChannelResolution,
) -> IntelReadout {
    let mclk = super::decode_mclk(regs.mcbios_req, gen);
    // The gear cells are shared across channels (the same MC_BIOS_REQ
    // word): decode once, layer on every channel.
    let (gear_mode, uclk) =
        super::gear_uclk_cells(crate::intel_gen::GearCap::Gear4, &mclk, regs.mcbios_req);
    let mut channels = Vec::with_capacity(ALDER_DDR4_CHANNELS.len());
    for i in 0..ALDER_DDR4_CHANNELS.len() {
        let tc = if i == 0 {
            &regs.alk_d4_tc0
        } else {
            &regs.alk_d4_tc1
        };
        let mut channel =
            decode_alder_ddr4_channel(i as u8, mclk.clone(), tc, &ALDER_DDR4_FIELDS);
        channel.clocks.gear_mode = gear_mode.clone();
        channel.clocks.uclk_mhz = uclk.clone();
        channels.push(channel);
    }
    super::decoded_readout(channels, res, crate::amd_readout::EccStatus::Unknown)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::IntelGen;
    use crate::intel_gen::ALDER_DDR4_FIELDS;
    use crate::intel_smbios::SmbiosTopology;
    use super::super::MemoryTech;

    /// N-03: an Alder Lake DDR4 channel whose every TC register is the
    /// bus-fault word `0xFFFF_FFFF` produces all-`Na` timing cells — the
    /// same degenerate-screening prologue as the Tier-1 / Tier-3
    /// siblings (H-2), so the DDR4 turnaround-quartet path (which has no
    /// per-word degenerate screen) never fabricates values from the
    /// all-ones bit pattern.
    #[test]
    fn alder_ddr4_all_bus_fault_produces_all_na() {
        let fault = 0xFFFF_FFFF;
        let regs = AlderDdr4TcRegs {
            tc_pre_lo: Some(fault),
            tc_pre_hi: Some(fault),
            tc_act: Some(fault),
            tc_rdrd: Some(fault),
            tc_rdwr: Some(fault),
            tc_wrrd: Some(fault),
            tc_wrwr: Some(fault),
            tc_odt: Some(fault),
            tc_rfp: Some(fault),
            tc_rftp: Some(fault),
        };
        let ch =
            decode_alder_ddr4_channel(0, Section::Value(1600.0), &regs, &ALDER_DDR4_FIELDS);
        // Every timing cell must be Na (no fabricated values).
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
            assert!(s.is_na(), "bus-fault DDR4 channel: {s:?} must be Na");
        }
        // The clocks stay valid (the mclk is shared, not per-channel).
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
    }

    /// Phase 1 confirming test (TC-ALK-DDR4): the Alder Lake **DDR4**
    /// decode path. A DDR4 Alder Lake system (`MAD_INTER_CHANNEL[2:0] ==
    /// 0`) routes to the `0xE000`/`0x1E000` TC blocks and decodes through
    /// `ALDER_DDR4_FIELDS` — 2 channels (2 MCs × 1 channel, no
    /// subchannels), the 64-bit `TC_PRE` split into low/high words, and
    /// the DDR4 turnaround quartets. The seeded words decode to exactly
    /// the timings placed in their documented bit positions
    /// (self-consistent by construction).
    #[test]
    fn tc_alk_ddr4_decodes_alder_lake_ddr4_from_the_e000_blocks() {
        let d4_tc = AlderDdr4TcRegs {
            tc_pre_lo: Some(0x0051_0011), // tRP 17 / tRDPRE 8 / tPPD 5
            tc_pre_hi: Some(0x0088_A019), // tRCD 17 / tRAS 40 / tWRPRE 25
            tc_act: Some(0x0005_0C10), // tFAW 16 / tRRD_SG 6 / tRRD_DG 10
            tc_rdrd: Some(0x140C_0806), // dd 20 / dr 12 / dg 8 / sg 6
            tc_rdwr: Some(0x140C_0807), // dd 20 / dr 12 / dg 8 / sg 7
            tc_wrrd: Some(0x2838_1404), // dd 20 / dr 14 / dg 10 / sg 4
            tc_wrwr: Some(0x140C_0805), // dd 20 / dr 12 / dg 8 / sg 5
            tc_odt: Some(0x1516_0000), // tCWL 21 / tCL 22
            tc_rfp: Some(0x0200_0003), // tREFI9 2 / OREF_RI 3
            tc_rftp: Some(0x02D0_1004), // tRFC 180 / tREFI 4100
        };
        let regs = IntelImcRegs {
            mcbios_req: Some(0x0000_0120), // ratio 32 @ 100 MHz / 2 = 1600 MHz
            mad_inter_channel: Some(0), // [2:0] == 0 -> DDR4 (the dispatch key)
            alk_d4_tc0: d4_tc.clone(),
            alk_d4_tc1: d4_tc,
            ..IntelImcRegs::default()
        };
        let smbios = SmbiosTopology {
            memory_type: Some(MemoryTech::Ddr4),
            ..SmbiosTopology::empty()
        };
        // MAD_INTER_CHANNEL[2:0] == 0 -> DDR4 (the dispatch key).
        let ro = super::super::decode(&regs, IntelGen::AlderLake, &smbios);

        // 2 channels (2 MCs × 1 channel) — not the 4-subchannel DDR5
        // geometry.
        assert_eq!(ro.channels.len(), 2, "DDR4 Alder Lake = 2 channels");
        assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr4));

        let ch = &ro.channels[0];
        // MCLK = 32 @ 100 MHz / 2 = 1600 MHz (half-step, 11th-gen+ decode).
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
        // The decoded DDR4 timings land exactly where the seed bits put
        // them (TC_PRE high word: tRCD/tRAS; low word: tRP; TC_ODT:
        // tCL/tCWL; TC_ACT: tFAW/tRRD_SG/tRRD_DG; TC_RFTP: tRFC).
        assert_eq!(ch.timings.cl, Section::Value(22));
        assert_eq!(ch.timings.cwl, Section::Value(21));
        assert_eq!(ch.timings.rcdrd, Section::Value(17));
        assert_eq!(ch.timings.rcwdwr, Section::Value(17));
        assert_eq!(ch.timings.rp, Section::Value(17));
        assert_eq!(ch.timings.ras, Section::Value(40));
        assert_eq!(ch.timings.rc, Section::Value(57)); // tRAS 40 + tRP 17
        assert_eq!(ch.timings.rrds, Section::Value(6));
        assert_eq!(ch.timings.rrld, Section::Value(10));
        assert_eq!(ch.timings.faw, Section::Value(16));
        assert_eq!(ch.timings.rfc1, Section::Value(180));
        // The DDR4 turnaround quartets (per-register widths).
        assert_eq!(ch.timings.rdrd_scl, Section::Value(6));
        assert_eq!(ch.timings.rdrd_sc, Section::Value(8));
        assert_eq!(ch.timings.rdrd_sd, Section::Value(12));
        assert_eq!(ch.timings.rdrd_dd, Section::Value(20));
        assert_eq!(ch.timings.rdwr, Section::Value(8)); // TC_RDWR dg
        assert_eq!(ch.timings.wtrs, Section::Value(10)); // TC_WRRD dg
        assert_eq!(ch.timings.wtrl, Section::Value(4)); // TC_WRRD sg
        assert_eq!(ch.timings.wrwr_scl, Section::Value(5));
        assert_eq!(ch.timings.wrwr_sc, Section::Value(8));
        assert_eq!(ch.timings.wrwr_sd, Section::Value(12));
        assert_eq!(ch.timings.wrwr_dd, Section::Value(20));
        // No tWR / tRTP / rfc2 / rfcsb slot in the DDR4 TC layout.
        assert!(ch.timings.wr.is_na());
        assert!(ch.timings.rtp.is_na());
        assert!(ch.timings.rfc2.is_na());
        assert!(ch.timings.rfcsb.is_na());
        // Channel 1 decodes identically (the same seeded MC1 block).
        assert_eq!(ro.channels[1].timings.cl, Section::Value(22));
        assert_eq!(ro.channels[1].timings.rfc1, Section::Value(180));
    }
}
