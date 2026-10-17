//! Shared test fixture constants and helper functions for the
//! `intel_readout` directory-module split. Only compiled in test mode.
//!
//! Step 2 of the `intel_readout` directory-module split.

use super::types::*;

/// A populated mirror block under the Alder layout (the mirror is a
/// legacy alias of the MCL block — the same words, Tier-1 names):
/// the DDR5-4800 40-class fixture, so every MCL-paired mirror word
/// decodes; the MCL-only registers (+0x10 `TC_WTR` / +0x14 `TC_RFP`
/// / +0x18 `TC_RFP2`) have no mirror word and ride the MCL block
/// (absent here → `None`).
pub(crate) fn ddr5_mirror_block() -> ChannelRegs {
    ChannelRegs {
        tc_dbp: Some(0x284D_2828), // +0x00 → tc_pre
        tc_rap: Some(0x0028_8410), // +0x04 → tc_act
        tc_rfp: Some(0x0000_0004), // +0x08 → tc_act2
        tc_rdrd: Some(0x0048_C286), // +0x20 → tc_rdrd
        tc_wrrd: Some(0x0000_0286), // +0x28 → tc_wrwr
        ..ChannelRegs::default()
    }
}

/// The Tier-3 dispatch fixture with OQ-11's DDR5 condition met:
/// all four mirror blocks populated, both `MAD_DIMM_CH2/CH3` raws
/// present and non-zero, `MC_BIOS_REQ = 0x0001_0024` (the 11th-
/// gen-and-later half-step encoding of DDR5-4800 on the 133.3333
/// MHz base: ratio 36 = 4800 / 133.3333 → 2400 MHz, bit 16 →
/// gear 2 — the Alder Lake MC_BIOS_REQ decode).
pub(crate) fn tier3_ddr5_condition_regs() -> IntelImcRegs {
    IntelImcRegs {
        mcbios_req: Some(0x0001_0024),
        ch0: ddr5_mirror_block(),
        ch1: ddr5_mirror_block(),
        ch2: ddr5_mirror_block(),
        ch3: ddr5_mirror_block(),
        mad_dimm_ch2: Some(1),
        mad_dimm_ch3: Some(1),
        ..IntelImcRegs::default()
    }
}

/// A buffer-backed twin of [`crate::intel_mchbar::MchBar::read_u32`]:
/// bounds-checked 4-byte little-endian reads over `buf` (out-of-bounds →
/// `None`, mirroring `check_read_bounds`), recording every offset touched.
pub(crate) fn buffered_reader(
    buf: Vec<u8>,
    touched: &std::cell::RefCell<Vec<usize>>,
) -> impl FnMut(usize) -> Option<u32> + '_ {
    move |off| {
        touched.borrow_mut().push(off);
        let end = off.checked_add(4)?;
        let w = buf.get(off..end)?;
        Some(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    }
}

/// Seed one 4-byte little-endian word at `off` in `buf` (test-only).
pub(crate) fn seed_word(buf: &mut [u8], off: usize, word: u32) {
    buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
}
