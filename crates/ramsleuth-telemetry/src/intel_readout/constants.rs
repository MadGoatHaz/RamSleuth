//! Hardware-authoritative register offset constants for the Intel IMC
//! decode (MCHBAR-relative). Pure constants — no logic, no dependencies.
//!
//! Every Tier-1 and legacy offset is compile-asserted to fit the 64 KiB
//! Tier-1 MCHBAR window; every Tier-3 offset is compile-asserted to fit
//! the 256 KiB wide window.

/// Tier-1 MCHBAR window is 64 KiB (0x10000) per datasheet.
///
/// Compile-check boundary for this module's register table (every Tier-1
/// and legacy offset must fit it). The runtime `/dev/mem` mmap in
/// `intel_mchbar.rs` uses the same 64 KiB size. Tier-2 (Alder/Raptor)
/// will require 256 KiB (0x40000).
pub const MCHBAR_WINDOW: usize = 1 << 16;

/// Tier-3 (Alder/Raptor Lake and later) MCHBAR window is 256 KiB
/// (0x40000) — the profiled window of the Alder family (IG-03;
/// Research roadmap Tier 3: "Expand window to 256 KiB"). The wide
/// `/dev/mem` map (`IntelImcRegs::from_bar_wide`, IG-25) and
/// `intel_mchbar::acquire` (via `intel_mchbar::window_size_for`) use it.
pub const MCHBAR_WINDOW_TIER3: usize = 1 << 18;

/// Global register: the BIOS request word carrying the DRAM clock ratio
/// (bits 7:0), the reference-clock select (bit 8), the gear ratio (bits
/// 17:16 — Rocket Lake and later; decoded by the Rocket Lake profile),
/// and RUN_BUSY (bit 31).
pub const MC_BIOS_REQ_OFFSET: usize = 0x5E00;

/// Per-channel IMC block bases: channel 0 at `0x4000`, channel 1 at
/// `0x4400` (single memory controller, Tier 1).
pub const CHANNEL0_BASE: usize = 0x4000;
pub const CHANNEL1_BASE: usize = 0x4400;

/// Stride between the two per-channel blocks (Tier 1).
pub const CHANNEL_STRIDE: usize = 0x400;

/// Offsets within a channel's IMC block (4-byte registers, in decode
/// order).
pub const TC_DBP_OFFSET: usize = 0x00;
pub const TC_RAP_OFFSET: usize = 0x04;
pub const TC_RFP_OFFSET: usize = 0x08;
pub const TC_RAP2_OFFSET: usize = 0x0C;
pub const TC_RDRD_OFFSET: usize = 0x20;
pub const TC_RDWR_OFFSET: usize = 0x24;
pub const TC_WRRD_OFFSET: usize = 0x28;
pub const TC_WRWR_OFFSET: usize = 0x2C;

/// Reference clock for `MC_BIOS_REQ` REF_CLK = 0 (the 400 MHz bus clock
/// divided by 3, i.e. 133.3333… MHz).
pub(crate) const REF_CLK_133_MHZ: f64 = 400.0 / 3.0;

/// Reference clock for `MC_BIOS_REQ` REF_CLK = 1 (100 MHz).
pub(crate) const REF_CLK_100_MHZ: f64 = 100.0;

/// Highest MCHBAR-relative offset this module reads (a 4-byte access at
/// `MC_BIOS_REQ`).
pub const MAX_IMC_OFFSET: usize = MC_BIOS_REQ_OFFSET + 4;

/// Highest per-channel offset (channel 1's `TC_WRWR`, 4-byte access).
pub const MAX_CHANNEL_OFFSET: usize = CHANNEL1_BASE + TC_WRWR_OFFSET + 4;

// Compile-time proof that every register read in this module
// (the hardware-authoritative Tier-1 table) fits the 64 KiB Tier-1
// MCHBAR window (each read is a 4-byte access). A violated bound
// fails const evaluation (division by zero).
const _ASSERT_IMC_READS_IN_WINDOW: () = {
    let in_window = MAX_IMC_OFFSET <= MCHBAR_WINDOW && MAX_CHANNEL_OFFSET <= MCHBAR_WINDOW;
    let _proof: usize = 1 / if in_window { 1 } else { 0 };
};

// ---------------------------------------------------------------------------
// Tier-3 (Alder/Raptor Lake) offset set — the 256 KiB wide window (IG-25).
// ---------------------------------------------------------------------------

/// Native MCL block bases (per controller; Research lines 107, 329,
/// OQ-4 — subchannels 1 and 3 have no MCL base): MC0 @ `0xD000`,
/// MC1 @ `0xD800`.
pub const MCL0_BASE: usize = 0xD000;
pub const MCL1_BASE: usize = 0xD800;

/// Offsets within a native MCL block (4-byte registers; the Alder
/// layout per `ALDER_FIELDS`, in decode order).
pub const MCL_TC_PRE_OFFSET: usize = 0x00;
pub const MCL_TC_ACT_OFFSET: usize = 0x04;
pub const MCL_TC_ACT2_OFFSET: usize = 0x08;
pub const MCL_TC_WTR_OFFSET: usize = 0x10;
pub const MCL_TC_RFP_OFFSET: usize = 0x14;
pub const MCL_TC_RFP2_OFFSET: usize = 0x18;
pub const MCL_TC_RDRD_OFFSET: usize = 0x20;
pub const MCL_TC_WRWR_OFFSET: usize = 0x28;

/// `MAD_DIMM_CH2` / `MAD_DIMM_CH3` intra-IMC-base offsets. The full
/// address is [`super::imc_base_for`] + this: `0x5000` pre-12th-gen (→
/// the legacy `0x5060` / `0x5064`) or `0xD800` 12th-gen+ (→ `0xD860` /
/// `0xD864`). The `+0x60` / `+0x64` intra-base position is preserved from
/// the legacy map — it is **not** the `+0x0C` / `+0x10`
/// `MAD_INTER_CHANNEL` / `MAD_DIMM_CH0/1` stride [`super::read_intel`]
/// reads, and reusing that stride would collide with the native MCL1
/// block (also at `0xD800`, whose `TC_RFP` / `TC_RFP2` sit at
/// `+0x14` / `+0x18`).
pub const MAD_DIMM_CH2_INTRA: usize = 0x60;
pub const MAD_DIMM_CH3_INTRA: usize = 0x64;

/// The pre-12th-gen (`imc_base` = `0x5000`) `MAD_DIMM_CH2` /
/// `MAD_DIMM_CH3` absolute offsets — the legacy `0x5060` / `0x5064`, kept
/// for the window compile-assert and the reader test fixtures.
pub const MAD_DIMM_CH2_OFFSET: usize = 0x5000 + MAD_DIMM_CH2_INTRA;
pub const MAD_DIMM_CH3_OFFSET: usize = 0x5000 + MAD_DIMM_CH3_INTRA;

/// Highest Tier-3 MCL offset (MC1's `TC_WRWR`, 4-byte access).
pub const MAX_MCL_OFFSET: usize = MCL1_BASE + MCL_TC_WRWR_OFFSET + 4;

/// Highest Tier-3 global/MAD offset (`MAD_DIMM_CH3`, 4-byte access).
pub const MAX_MAD_DIMM_OFFSET: usize = MAD_DIMM_CH3_OFFSET + 4;

// ---------------------------------------------------------------------------
// Phase 1 (Intel MC fix): the Alder Lake **DDR4** TC offset set.
//
// On DDR4 Alder Lake the TC timing registers live at `MCHBAR + 0xE000`
// (MC0) / `MCHBAR + 0x1E000` (MC1) — **not** the `0x4000` legacy mirror
// window the Tier-1 map reads (which returns `0xFFFFFFFF` on DDR4). MC1 is
// MC0's block offset by the inter-controller stride `0x10000`.
// ---------------------------------------------------------------------------

/// Per-MC TC block bases (relative to MCHBAR): MC0 @ `0xE000`,
/// MC1 @ `0x1E000` (`0xE000 + 0x10000`).
pub const ALK_D4_TC0_BASE: usize = 0xE000;
pub const ALK_D4_TC1_BASE: usize = 0x1E000;

/// Offsets within an Alder Lake DDR4 TC block (4-byte registers; the
/// datasheet §3.2.66–3.2.87 layout). `TC_PRE` is a 64-bit register read as
/// two words: the low word @ `+0x00` and the high word @ `+0x04`.
pub const ALK_D4_TC_PRE: usize = 0x00;
pub const ALK_D4_TC_PRE_HI: usize = 0x04;
pub const ALK_D4_TC_ACT: usize = 0x08;
pub const ALK_D4_TC_RDRD: usize = 0x0C;
pub const ALK_D4_TC_RDWR: usize = 0x10;
pub const ALK_D4_TC_WRRD: usize = 0x14;
pub const ALK_D4_TC_WRWR: usize = 0x18;
pub const ALK_D4_TC_ODT: usize = 0x70;
pub const ALK_D4_TC_RFP: usize = 0x438;
pub const ALK_D4_TC_RFTP: usize = 0x43C;

/// Highest Alder Lake DDR4 TC offset (MC1's `TC_RFTP`, 4-byte access).
pub const MAX_ALK_D4_OFFSET: usize = ALK_D4_TC1_BASE + ALK_D4_TC_RFTP + 4;

// Compile-time proof that the Tier-3 offset set (IG-25) fits the 256 KiB
// wide window (every read is a 4-byte access). A violated bound fails
// const evaluation (division by zero).
const _ASSERT_TIER3_READS_IN_WIDE_WINDOW: () = {
    let in_window = MAX_MCL_OFFSET <= MCHBAR_WINDOW_TIER3
        && MAX_MAD_DIMM_OFFSET <= MCHBAR_WINDOW_TIER3
        && MAX_IMC_OFFSET <= MCHBAR_WINDOW_TIER3
        && MAX_CHANNEL_OFFSET <= MCHBAR_WINDOW_TIER3
        && MAX_ALK_D4_OFFSET <= MCHBAR_WINDOW_TIER3;
    let _proof: usize = 1 / if in_window { 1 } else { 0 };
};
