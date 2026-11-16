//! Register reader functions for the Intel IMC: the `IntelImcRegs` impl
//! block (the `/dev/mem` read sets) and the per-block raw readers
//! (per-register containment — one bad read never poisons the rest).
//!
//! Dependencies: [`super::constants`] (the offset table),
//! [`super::types`] (the raw register shapes),
//! [`crate::intel_mchbar::MchBar`] (the MMIO read primitive), and
//! [`crate::intel_gen::ALDER_CHANNELS`] (the Tier-3 mirror mapping).

use crate::intel_gen::ALDER_CHANNELS;
use crate::intel_mchbar::MchBar;

use super::constants::*;
use super::types::{AlderDdr4TcRegs, ChannelRegs, IntelImcRegs, MclRegs};

impl IntelImcRegs {
    /// Read the 9-register IMC set from a live, read-only [`MchBar`]
    /// map: the global `MC_BIOS_REQ` plus the two per-channel blocks.
    ///
    /// Per-register containment (D5): each read degrades independently
    /// to `None` on failure (bounds, unmap, …) — one bad read never
    /// poisons the rest and never panics. The thin `MchBar` shell of
    /// the pure reader core [`from_reader`] (the 64 KiB Tier 1 /
    /// Rocket Lake `/dev/mem` read set).
    pub fn from_bar(bar: &MchBar) -> Self {
        Self::from_reader(|off| bar.read_u32(off).ok())
    }

    /// The [`from_bar`] read set over an abstract register reader:
    /// `read(offset) -> Option<u32>` (a failed / out-of-bounds read is
    /// `None` — the same per-register containment (D5) as
    /// [`MchBar::read_u32`]'s `.ok()`).
    ///
    /// The pure, I/O-free core of the 64 KiB Tier 1 / Rocket Lake
    /// `/dev/mem` path — the unit-testable half of the reader contract
    /// (the 9-register set: the global `MC_BIOS_REQ` plus the two
    /// `0x4000` / `0x4400` channel blocks; every Tier-3 extension
    /// field stays `None`).
    pub(crate) fn from_reader<R>(mut read: R) -> Self
    where
        R: FnMut(usize) -> Option<u32>,
    {
        let mut ch0 = ChannelRegs::default();
        let mut ch1 = ChannelRegs::default();
        read_channel_regs(&mut read, CHANNEL0_BASE, &mut ch0);
        read_channel_regs(&mut read, CHANNEL1_BASE, &mut ch1);
        Self {
            mcbios_req: read(MC_BIOS_REQ_OFFSET),
            ch0,
            ch1,
            // The Tier-3 extension fields (ch2 / ch3, mcl0 / mcl1,
            // mad_dimm_ch2 / mad_dimm_ch3) stay default (all `None`):
            // `from_bar` reads only the 64 KiB Tier-1 window (offsets
            // <= 0x5E04); the Tier-3 `/dev/mem` reads route through
            // [`IntelImcRegs::from_bar_wide`] (the 256 KiB wide map,
            // IG-25).
            ..Self::default()
        }
    }

    /// Read the full Tier-3 (Alder/Raptor Lake) offset set from a live,
    /// read-only [`MchBar`] map: the global `MC_BIOS_REQ`, the four
    /// legacy mirror blocks (the `ALDER_CHANNELS` subchannel mirrors at
    /// `0x4000` / `0x4400` / `0x4800` / `0x4C00`), the two native MCL
    /// blocks (`mcl0` @ `0xD000` / `mcl1` @ `0xD800`, OQ-4), and the
    /// two Alder/DDR5 `MAD_DIMM_CH2` / `MAD_DIMM_CH3` raws — the full
    /// 74-slot [`IntelImcRegs`] set (the wide `/dev/mem` half of plan
    /// §3.5; IG-25).
    ///
    /// The map is profile-sized (`intel_mchbar::window_size_for`:
    /// 256 KiB for the Alder family, 64 KiB for Tier 1/2); every
    /// offset in this set fits both sizes (highest `0xD82C` ≤ the
    /// 64 KiB Tier-1 window — compile-proven by
    /// `_ASSERT_TIER3_READS_IN_WIDE_WINDOW`), so a profile-sized map
    /// never bounds-fails; the 64 KiB 9-register [`from_bar`] is
    /// untouched (Tier 1/2 reads route through it, never this set).
    ///
    /// Per-register containment (D5) mirrors [`from_bar`]: each read
    /// degrades independently to `None` on failure (bounds, unmap, …) —
    /// one bad read never poisons the rest and never panics. The thin
    /// `MchBar` shell of the pure reader core [`from_wide_reader`].
    ///
    /// `imc_base` is the gen-keyed IMC MAD base the caller derives
    /// ([`super::imc_base_for`]: `0xD800` for 12th-gen+, `0x5000` for
    /// pre-12th-gen) — the `MAD_DIMM_CH2/CH3` raws key off it (N-10:
    /// the base is caller-supplied, mirroring [`from_wide_reader`],
    /// never hard-coded to one generation's value).
    pub fn from_bar_wide(bar: &MchBar, imc_base: usize) -> Self {
        Self::from_wide_reader(imc_base, |off| bar.read_u32(off).ok())
    }

    /// The [`from_bar_wide`] read set over an abstract register reader:
    /// `read(offset) -> Option<u32>` (a failed / out-of-bounds read is
    /// `None` — the same per-register containment (D5) as
    /// [`MchBar::read_u32`]'s `.ok()`).
    ///
    /// The pure, I/O-free core of the 256 KiB wide `/dev/mem` path
    /// (IG-25 + Phase 1): the full set — the global `MC_BIOS_REQ`, the
    /// four `ALDER_CHANNELS` mirror blocks, the two native MCL blocks
    /// (`0xD000`/`0xD800` — the uncore fallbacks the 9-register
    /// `from_bar` never reads), the two `MAD_DIMM_CH2` /
    /// `MAD_DIMM_CH3` raws (gen-keyed: the `imc_base` argument + the
    /// `0x60` / `0x64` intra-base offset), and the two Alder Lake DDR4
    /// TC blocks (`0xE000`/`0x1E000`, Phase 1) — the unit-testable half
    /// of the reader contract.
    pub(crate) fn from_wide_reader<R>(imc_base: usize, mut read: R) -> Self
    where
        R: FnMut(usize) -> Option<u32>,
    {
        let mut ch0 = ChannelRegs::default();
        let mut ch1 = ChannelRegs::default();
        let mut ch2 = ChannelRegs::default();
        let mut ch3 = ChannelRegs::default();
        for (i, map) in ALDER_CHANNELS.iter().enumerate() {
            let block = match i {
                0 => &mut ch0,
                1 => &mut ch1,
                2 => &mut ch2,
                _ => &mut ch3,
            };
            read_channel_regs(&mut read, map.mirror_base as usize, block);
        }
        let mut mcl0 = MclRegs::default();
        let mut mcl1 = MclRegs::default();
        read_mcl_regs(&mut read, MCL0_BASE, &mut mcl0);
        read_mcl_regs(&mut read, MCL1_BASE, &mut mcl1);
        // Phase 1: the two Alder Lake DDR4 TC blocks (MC0 @ 0xE000,
        // MC1 @ 0x1E000) — the 64-bit TC_PRE split into two words each.
        let mut alk_d4_tc0 = AlderDdr4TcRegs::default();
        let mut alk_d4_tc1 = AlderDdr4TcRegs::default();
        read_alder_ddr4_tc_regs(&mut read, ALK_D4_TC0_BASE, &mut alk_d4_tc0);
        read_alder_ddr4_tc_regs(&mut read, ALK_D4_TC1_BASE, &mut alk_d4_tc1);
        // H-12: the `MAD_DIMM_CH2` / `CH3` raws sit at the gen-keyed IMC
        // base (`0xD800` 12th-gen+, `0x5000` pre-12th-gen) + the `0x60` /
        // `0x64` intra-base offset — the legacy `0x5060` / `0x5064`
        // preserved relative to the base (NOT the `+0x0C` / `+0x10`
        // `MAD_INTER_CHANNEL` / `MAD_DIMM_CH0/1` stride
        // [`super::read_intel`] reads, which would collide with the
        // native MCL1 block).
        Self {
            mcbios_req: read(MC_BIOS_REQ_OFFSET),
            ch0,
            ch1,
            ch2,
            ch3,
            mcl0,
            mcl1,
            // L-10: the gen-keyed MAD base (0x5000 / 0xD800) is set by
            // `read_intel_with` after this pure reader (which reads fixed
            // offsets only) — `None` here.
            mad_inter_channel: None,
            mad_dimm_ch0: None,
            mad_dimm_ch1: None,
            mad_dimm_ch2: read(imc_base + MAD_DIMM_CH2_INTRA),
            mad_dimm_ch3: read(imc_base + MAD_DIMM_CH3_INTRA),
            alk_d4_tc0,
            alk_d4_tc1,
        }
    }
}

/// Read one channel's eight raw registers through `read` into `regs`
/// (per-register containment: a failed / out-of-bounds read → `None`).
fn read_channel_regs<R: FnMut(usize) -> Option<u32>>(
    read: &mut R,
    base: usize,
    regs: &mut ChannelRegs,
) {
    regs.tc_dbp = read(base + TC_DBP_OFFSET);
    regs.tc_rap = read(base + TC_RAP_OFFSET);
    regs.tc_rfp = read(base + TC_RFP_OFFSET);
    regs.tc_rap2 = read(base + TC_RAP2_OFFSET);
    regs.tc_rdrd = read(base + TC_RDRD_OFFSET);
    regs.tc_rdwr = read(base + TC_RDWR_OFFSET);
    regs.tc_wrrd = read(base + TC_WRRD_OFFSET);
    regs.tc_wrwr = read(base + TC_WRWR_OFFSET);
}

/// Read one native MCL block's eight raw registers through `read` into
/// `regs` (per-register containment: a failed / out-of-bounds read →
/// `None`).
fn read_mcl_regs<R: FnMut(usize) -> Option<u32>>(read: &mut R, base: usize, regs: &mut MclRegs) {
    regs.tc_pre = read(base + MCL_TC_PRE_OFFSET);
    regs.tc_act = read(base + MCL_TC_ACT_OFFSET);
    regs.tc_act2 = read(base + MCL_TC_ACT2_OFFSET);
    regs.tc_wtr = read(base + MCL_TC_WTR_OFFSET);
    regs.tc_rfp = read(base + MCL_TC_RFP_OFFSET);
    regs.tc_rfp2 = read(base + MCL_TC_RFP2_OFFSET);
    regs.tc_rdrd = read(base + MCL_TC_RDRD_OFFSET);
    regs.tc_wrwr = read(base + MCL_TC_WRWR_OFFSET);
}

/// Read one Alder Lake DDR4 memory controller's TC block through `read`
/// into `regs` (per-register containment: a failed / out-of-bounds read
/// → `None`). The 64-bit `TC_PRE` is read as two 32-bit words
/// (`+0x00` low / `+0x04` high); the remaining eight registers are single
/// 32-bit words at their datasheet offsets (Phase 1).
fn read_alder_ddr4_tc_regs<R: FnMut(usize) -> Option<u32>>(
    read: &mut R,
    base: usize,
    regs: &mut AlderDdr4TcRegs,
) {
    regs.tc_pre_lo = read(base + ALK_D4_TC_PRE);
    regs.tc_pre_hi = read(base + ALK_D4_TC_PRE_HI);
    regs.tc_act = read(base + ALK_D4_TC_ACT);
    regs.tc_rdrd = read(base + ALK_D4_TC_RDRD);
    regs.tc_rdwr = read(base + ALK_D4_TC_RDWR);
    regs.tc_wrrd = read(base + ALK_D4_TC_WRRD);
    regs.tc_wrwr = read(base + ALK_D4_TC_WRWR);
    regs.tc_odt = read(base + ALK_D4_TC_ODT);
    regs.tc_rfp = read(base + ALK_D4_TC_RFP);
    regs.tc_rftp = read(base + ALK_D4_TC_RFTP);
}
