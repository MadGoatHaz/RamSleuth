//! The hardware-authoritative Tier-1 (Skylake–Comet Lake) DDR4 decode
//! path: the 64 KiB MCHBAR register map, the 4×6-bit turnaround quartet
//! extraction, and the per-channel decode core.
//!
//! Step 2 of the `intel_readout` directory-module split — moved verbatim
//! from `mod.rs`.

use super::gates::*;
use super::types::*;

use crate::amd_readout::TimingSet;
use crate::error::{NaReason, Section};

// ---------------------------------------------------------------------------
// Pure bit-field decoders (the frozen Tier-1 decode contract; unit-tested
// with synthetic register values — no I/O, no unsafe).
// ---------------------------------------------------------------------------

/// tCL — `TC_DBP` bits 5:0 (6-bit, integer DRAM clock cycles).
pub(super) fn tc_dbp_cl(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tCWL — `TC_DBP` bits 13:8 (6-bit).
pub(super) fn tc_dbp_cwl(reg: u32) -> u16 {
    field16(reg, 13, 8)
}

/// tRCD — `TC_DBP` bits 21:16 (6-bit). Intel enforces a *unified,
/// symmetric* tRCD: the same value feeds both the `rcdrd` and `rcwdwr`
/// display slots.
pub(super) fn tc_dbp_trcd(reg: u32) -> u16 {
    field16(reg, 21, 16)
}

/// tRP — `TC_DBP` bits 29:24 (6-bit).
pub(super) fn tc_dbp_trp(reg: u32) -> u16 {
    field16(reg, 29, 24)
}

/// tRRD_S — `TC_RAP` bits 5:0 (6-bit).
pub(super) fn tc_rap_rrds(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tRTP — `TC_RAP` bits 11:6 (6-bit).
pub(super) fn tc_rap_rtp(reg: u32) -> u16 {
    field16(reg, 11, 6)
}

/// tCKE — `TC_RAP` bits 15:12. **4-bit** (range 1–15; do not assume
/// 6-bit). Decoded by the core but has no frozen display slot (not
/// shown; the raw value stays available via the sysfs attributes).
pub fn tc_rap_tcke(reg: u32) -> u16 {
    field16(reg, 15, 12)
}

/// tFAW — `TC_RAP` bits 23:16 (8-bit).
pub(super) fn tc_rap_faw(reg: u32) -> u16 {
    field16(reg, 23, 16)
}

/// tRAS — `TC_RAP` bits 31:24 (8-bit).
pub(super) fn tc_rap_tras(reg: u32) -> u16 {
    field16(reg, 31, 24)
}

/// tRFC — `TC_RFP` bits 10:0 (11-bit, 1–2047).
pub(super) fn tc_rfp_rfc1(reg: u32) -> u16 {
    field16(reg, 10, 0)
}

/// tREFI — `TC_RFP` bits 27:16 (12-bit). Decoded by the core but has no
/// frozen display slot (not shown; the raw value stays available via
/// the sysfs attributes).
pub fn tc_rfp_refi(reg: u32) -> u16 {
    field16(reg, 27, 16)
}

/// tRRD_L — `TC_RAP2` bits 5:0 (6-bit).
pub(super) fn tc_rap2_rrld(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tWR — `TC_RAP2` bits 13:8 (6-bit).
pub(super) fn tc_rap2_wr(reg: u32) -> u16 {
    field16(reg, 13, 8)
}

/// The 4×6-bit turnaround quartet of one `TC_*` register:
/// `(sg, dg, dr, dd)` — same channel / same bank group / different rank
/// / different channel, per the frozen `TC_RDRD` / `TC_RDWR` / `TC_WRRD`
/// / `TC_WRWR` layout (bits 5:0 / 11:6 / 17:12 / 23:18).
pub(super) fn turnaround_quartet(reg: u32) -> [u16; 4] {
    [
        field16(reg, 5, 0),
        field16(reg, 11, 6),
        field16(reg, 17, 12),
        field16(reg, 23, 18),
    ]
}

// ---------------------------------------------------------------------------
// The Tier-1 per-channel decode core.
// ---------------------------------------------------------------------------

/// Decode one Tier-1 channel from its raw registers + the shared DRAM
/// core clock (the per-channel heart of [`super::decode`]). Never panics
/// (D5): every cell is a `Value`, an honest `Na(ParseError)` (a sourced
/// register failed / a field is untrained), or a structural
/// `Na(NotApplicable)`.
pub(super) fn decode_tier1_channel(
    index: u8,
    mclk: Section<f64>,
    regs: &ChannelRegs,
) -> IntelChannel {
    // H-2: Normalize degenerate words to None so the turnaround-quartet
    // path (which has no per-word degenerate screen) never fabricates
    // values from a bus-fault readback.
    let regs = ChannelRegs {
        tc_dbp: regs.tc_dbp.filter(|&w| !is_degenerate_word(w)),
        tc_rap: regs.tc_rap.filter(|&w| !is_degenerate_word(w)),
        tc_rfp: regs.tc_rfp.filter(|&w| !is_degenerate_word(w)),
        tc_rap2: regs.tc_rap2.filter(|&w| !is_degenerate_word(w)),
        tc_rdrd: regs.tc_rdrd.filter(|&w| !is_degenerate_word(w)),
        tc_rdwr: regs.tc_rdwr.filter(|&w| !is_degenerate_word(w)),
        tc_wrrd: regs.tc_wrrd.filter(|&w| !is_degenerate_word(w)),
        tc_wrwr: regs.tc_wrwr.filter(|&w| !is_degenerate_word(w)),
    };

    // --- timings (integer DRAM clock cycles) ---------------------------
    // TC_DBP: the unified symmetric tRCD feeds both the rcdrd and the
    // rcwdwr slot (Intel exposes one tRCD, parity matrix: "Symmetrical
    // decode").
    let cl = timing_cell(regs.tc_dbp, "tCL", tc_dbp_cl);
    let cwl = timing_cell(regs.tc_dbp, "tCWL", tc_dbp_cwl);
    let trcd = timing_cell(regs.tc_dbp, "tRCD", tc_dbp_trcd);
    let rcdrd = trcd.clone();
    let rcwdwr = trcd;
    let rp = timing_cell(regs.tc_dbp, "tRP", tc_dbp_trp);

    // TC_RAP: tRRD_S / tRTP / tFAW / tRAS (tCKE is decoded — no display
    // slot).
    let ras = timing_cell(regs.tc_rap, "tRAS", tc_rap_tras);
    let rrds = timing_cell(regs.tc_rap, "tRRD_S", tc_rap_rrds);
    let rtp = timing_cell(regs.tc_rap, "tRTP", tc_rap_rtp);
    let faw = timing_cell(regs.tc_rap, "tFAW", tc_rap_faw);

    // tRC is synthesized (the JEDEC identity — Intel exposes no tRC
    // register): `tRAS + tRP` when both decode.
    let rc = match (ras.value().copied(), rp.value().copied()) {
        (Some(a), Some(b)) => ticks_section(a.saturating_add(b), "tRC (tRAS + tRP)"),
        _ => Section::na(NaReason::NotApplicable),
    };

    // TC_RAP2: tRRD_L / tWR.
    let rrld = timing_cell(regs.tc_rap2, "tRRD_L", tc_rap2_rrld);
    let wr = timing_cell(regs.tc_rap2, "tWR", tc_rap2_wr);

    // TC_RFP: tRFC1 (tREFI is decoded — no display slot; the DDR4 client
    // runs single-tRFC scheduling, so rfc2/rfcsb are not applicable).
    let rfc1 = timing_cell(regs.tc_rfp, "tRFC", tc_rfp_rfc1);

    // Turnaround quartets (4×6-bit sg / dg / dr / dd per register).
    let rdrd = regs.tc_rdrd.map(turnaround_quartet);
    let rdwr = regs.tc_rdwr.map(turnaround_quartet);
    let wrrd = regs.tc_wrrd.map(turnaround_quartet);
    let wrwr = regs.tc_wrwr.map(turnaround_quartet);
    let rdrd_scl = quartet_cell(rdrd, "tRDRD(sg)", 0);
    let rdrd_sc = quartet_cell(rdrd, "tRDRD(dg)", 1);
    let rdrd_sd = quartet_cell(rdrd, "tRDRD(dr)", 2);
    let rdrd_dd = quartet_cell(rdrd, "tRDRD(dd)", 3);
    // `rdwr` ← TC_RDWR dg (the representative bank-group turnaround;
    // documented choice).
    let rdwr = quartet_cell(rdwr, "tRDWR(dg)", 1);
    // tWTR_S = dg, tWTR_L = sg (research-doc parity matrix); the coarse
    // `wrrd` slot is superseded by the finer pair.
    let wtrs = quartet_cell(wrrd, "tWTR_S(dg)", 1);
    let wtrl = quartet_cell(wrrd, "tWTR_L(sg)", 0);

    // --- clocks: the shared map decodes mclk only; the Rocket Lake
    // profile layers on gear_mode + uclk (MC_BIOS_REQ[17:16], Breakdown
    // §3D) in [`super::decode`] after this call. The AMD-fabric slots have no
    // IMC analog. N-06: the structural NAs come from the shared
    // builder.
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
        rfcsb: Section::na(NaReason::NotApplicable),
        cwl,
        rtp,
        rdwr,
        wrrd: Section::na(NaReason::NotApplicable),
        rdrd_sd,
        rdrd_dd,
        rdrd_scl,
        rdrd_sc,
        wrwr_sd: quartet_cell(wrwr, "tWRWR(dr)", 2),
        wrwr_dd: quartet_cell(wrwr, "tWRWR(dd)", 3),
        wrwr_scl: quartet_cell(wrwr, "tWRWR(sg)", 0),
        wrwr_sc: quartet_cell(wrwr, "tWRWR(dg)", 1),
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// tCKE is a **4-bit** field (1–15): the acceptance raw
    /// `0x27180204` carries 0 (untrained in this fixture — not
    /// displayed, no gate impact), and the 4-bit mask never aliases
    /// into neighboring fields.
    #[test]
    fn tc_rap_tcke_is_a_four_bit_field() {
        assert_eq!(tc_rap_tcke(0x2718_0204), 0);
        assert_eq!(tc_rap_tcke(0x0000_8000), 8);
        // the 8-bit tFAW above it is unaffected by the tCKE mask.
        assert_eq!(tc_rap_faw(0x2718_0204), 24);
    }

    /// tRFC is 11-bit: the maximum 2047 decodes (in-band for the [1,
    /// 2048] tick gate); tREFI is 12-bit (decoded, not displayed).
    #[test]
    fn tc_rfp_field_widths() {
        assert_eq!(tc_rfp_rfc1(0x0000_01A4), 420);
        assert_eq!(tc_rfp_rfc1(0x0000_07FF), 2047);
        assert_eq!(tc_rfp_refi(0x0FFF_0000), 4095);
    }

    /// The turnaround quartet layout (4×6-bit sg / dg / dr / dd).
    #[test]
    fn turnaround_quartet_layout() {
        assert_eq!(
            turnaround_quartet(0x0048_C286),
            [6, 10, 12, 18],
            "sg / dg / dr / dd at bits 5:0 / 11:6 / 17:12 / 23:18"
        );
    }

    /// H-2 + H-14: a Tier-1 channel whose every register is the
    /// bus-fault word `0xFFFF_FFFF` produces all-`Na` timing cells —
    /// never fabricated values from the all-ones bit pattern.
    #[test]
    fn tier1_all_bus_fault_produces_all_na() {
        let fault = 0xFFFF_FFFF;
        let regs = ChannelRegs {
            tc_dbp: Some(fault),
            tc_rap: Some(fault),
            tc_rfp: Some(fault),
            tc_rap2: Some(fault),
            tc_rdrd: Some(fault),
            tc_rdwr: Some(fault),
            tc_wrrd: Some(fault),
            tc_wrwr: Some(fault),
        };
        let ch = decode_tier1_channel(0, Section::Value(2400.0), &regs);
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
            assert!(s.is_na(), "bus-fault channel: {s:?} must be Na");
        }
        // The clocks stay valid (the mclk is shared, not per-channel).
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0));
    }

    /// H-2 + H-14: a Tier-1 channel whose every register is zero
    /// (untrained) produces all-`Na` timing cells.
    #[test]
    fn tier1_all_zero_produces_all_na() {
        let regs = ChannelRegs {
            tc_dbp: Some(0),
            tc_rap: Some(0),
            tc_rfp: Some(0),
            tc_rap2: Some(0),
            tc_rdrd: Some(0),
            tc_rdwr: Some(0),
            tc_wrrd: Some(0),
            tc_wrwr: Some(0),
        };
        let ch = decode_tier1_channel(0, Section::Value(2400.0), &regs);
        for s in [
            &ch.timings.cl,
            &ch.timings.cwl,
            &ch.timings.rcdrd,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.rrds,
            &ch.timings.rrld,
            &ch.timings.faw,
            &ch.timings.wr,
            &ch.timings.rfc1,
            &ch.timings.rdrd_scl,
            &ch.timings.wrwr_scl,
        ] {
            assert!(s.is_na(), "zero channel: {s:?} must be Na");
        }
    }
}
