//! Intel IMC register decode into the frozen display types — the
//! hardware-authoritative Tier-1 register map (INTEL-02, Bug 2 fix).
//!
//! This module is the readout half of the Intel telemetry branch: it
//! decodes the raw memory-controller (IMC) register values — exposed RAW
//! by the `ramsleuth_intel` kernel module (the sysfs path) or read from
//! the `/dev/mem` MCHBAR fallback ([`IntelImcRegs::from_bar`] /
//! [`IntelImcRegs::from_bar_wide`]) — into the frozen vendor-neutral
//! display types shared with the AMD readout
//! ([`ClockReadout`], [`TimingSet`], [`CadBus`], [`VoltageSet`]).
//!
//! The design split mirrors AMD (plan §2.4): **the source exposes raw
//! register values, this module performs all decoding**. Every register
//! read is contained per register: an absent / unreadable register
//! degrades only the fields sourced from it to `Na(ParseError)` — never
//! a panic, never a silent zero (the no-panic contract, D5).
//!
//! # Register table (verified Tier-1 layout, MCHBAR-relative)
//!
//! One **global** register plus **two per-channel** blocks (Tier 1 =
//! Skylake / Kaby Lake / Coffee Lake / Comet Lake — shared with Rocket
//! Lake: a single memory controller, 2 channels, DDR4, a 64 KiB mapped
//! region):
//!
//! | Offset | Register | Fields decoded |
//! |---|---|---|
//! | `0x5E00` | `MC_BIOS_REQ` | `[7:0]` CLK_RATIO · `[8]` REF_CLK (0 = 133.3333 MHz, 1 = 100 MHz) · `[17:16]` GEAR_RATIO (Rocket Lake and later — the Rocket Lake profile decodes it) · `[31]` RUN_BUSY (status — not decoded) |
//! | `0x4000` (ch0) / `0x4400` (ch1), stride `0x400`, `+0x00` | `TC_DBP` | `[5:0]` tCL · `[13:8]` tCWL · `[21:16]` tRCD · `[29:24]` tRP (6-bit each) |
//! | `+0x04` | `TC_RAP` | `[5:0]` tRRD_S · `[11:6]` tRTP · `[15:12]` tCKE (**4-bit**, 1–15) · `[23:16]` tFAW (8-bit) · `[31:24]` tRAS (8-bit) |
//! | `+0x08` | `TC_RFP` | `[10:0]` tRFC (11-bit, 1–2047) · `[27:16]` tREFI (12-bit) |
//! | `+0x0C` | `TC_RAP2` | `[5:0]` tRRD_L · `[13:8]` tWR (6-bit each) |
//! | `+0x20` | `TC_RDRD` | `[5:0]` sg · `[11:6]` dg · `[17:12]` dr · `[23:18]` dd (4×6-bit) |
//! | `+0x24` | `TC_RDWR` | same 4×6-bit layout |
//! | `+0x28` | `TC_WRRD` | same 4×6-bit layout |
//! | `+0x2C` | `TC_WRWR` | same 4×6-bit layout |
//!
//! Every access is a 4-byte read well inside the 64 KiB Tier-1 MCHBAR
//! window (compile-asserted below and re-checked at runtime by
//! [`MchBar::read_u32`]). The timing unit is **integer DRAM clock cycles**
//! (1 cycle = 2 UI).
//!
//! **Status: hardware-authoritative.** This replaces the v2.2.1 table
//! (`IMC_FREQ_RATIO @ 0x5058`, `MCS_COMMAND_* @ 0x5400`), which was a
//! mis-located skeleton: `0x5058` is a free-running performance counter,
//! not a frequency ratio (Bug 2). The verified layout is pinned in CI by
//! the Skylake §6.1 acceptance fixture (ratio 18 @ 133.3333 MHz →
//! 2400 MHz MCLK / 4800 MT/s) in the tests below and cross-referenced
//! against coreboot NRI / Libre-FSP and the
//! kernel EDAC driver. The `CLK_RATIO` semantics are generation-
//! dependent (the Alder Lake MC_BIOS_REQ decode): Skylake–Comet Lake
//! store a physical DRAM-clock multiplier, while 11th gen (Rocket
//! Lake) and later store the effective DDR data rate in half-cycle
//! steps — the 12600KF DDR4-3600 word 0x79E00124 (36 @ 100 MHz) →
//! 1800 MHz MCLK / 3600 MT/s is pinned in the tests below.
//!
//! # Mapping into the frozen display types (v1 Tier 1)
//!
//! - `mclk_mhz` ← `MC_BIOS_REQ`: the analog DRAM clock MCLK,
//!   generation-dependent — Skylake–Comet Lake: `ratio × refclk`
//!   (e.g. ratio 18 × 133.3333 = **2400 MHz**); 11th gen and later:
//!   `ratio × refclk / 2` (the ratio is the effective DDR data-rate
//!   multiplier in half-cycle steps — e.g. the 12600KF DDR4-3600 word
//!   0x79E00124: 36 × 100 MHz / 2 = **1800 MHz**); for DDR, MT/s =
//!   MCLK × 2. Sanity-gated to [1, 5333] MHz; a ratio of 0 =
//!   unconfigured → `Na(ParseError)`.
//! - `uclk_mhz` ← `MC_BIOS_REQ`: `mclk / gear` (gears 1/2/4, Breakdown
//!   §3D) on the Rocket Lake profile; `Na(NotApplicable)` on Tier 1
//!   (no gear register).
//! - `fclk_mhz`, `gdm`, `pdm` ← `Na(NotApplicable)` (AMD-fabric
//!   concepts with no IMC analog).
//! - `div_mode` ← `Na(NotApplicable)` on both profiled maps (D-C11
//!   unchanged); `gear_mode` ← `Na(NotApplicable)` on Tier 1, decoded
//!   from `MC_BIOS_REQ[17:16]` on the Rocket Lake profile.
//! - `command_rate` ← `Na(NotApplicable)` (D-C11 unchanged).
//! - `rtl` ← `Na(NotApplicable)` (the old skeleton's "RTL" register was
//!   fiction; the real IMC table has no RTL).
//! - Timings ([`TimingSet`], DRAM clock cycles; a `0` field = untrained
//!   → `Na(ParseError)`; per-register containment; the same [1, 2048]
//!   sanity gate as the AMD readout):
//!   - `cl` ← tCL · `cwl` ← tCWL · `rcdrd` **and** `rcwdwr` ← the *same*
//!     tRCD field (Intel enforces a unified, symmetric tRCD — parity
//!     matrix: "Symmetrical decode") · `rp` ← tRP · `ras` ← tRAS · `rc`
//!     ← `ras + rp` when both decode (a synthesized JEDEC identity —
//!     Intel exposes no tRC register), else `Na(NotApplicable)`.
//!   - `rrds` ← tRRD_S · `rrld` ← tRRD_L · `faw` ← tFAW · `rtp` ← tRTP ·
//!     `wr` ← tWR.
//!   - `rfc1` ← tRFC · `rfc2`, `rfcsb` ← `Na(NotApplicable)` (Intel
//!     DDR4 client runs standard single-tRFC scheduling).
//!   - `rdwr` ← `TC_RDWR[11:6]` (dg — the representative bank-group
//!     turnaround; a documented choice).
//!   - `wtrs` ← `TC_WRRD[11:6]` (dg) · `wtrl` ← `TC_WRRD[5:0]` (sg) —
//!     per the research-doc parity matrix (tWTR_S = dg, tWTR_L = sg) ·
//!     `wrrd` ← `Na(NotApplicable)` (superseded by the finer wtrs/wtrl
//!     pair).
//!   - `rdrd_scl` ← `TC_RDRD[5:0]` (sg) · `rdrd_sc` ← `TC_RDRD[11:6]`
//!     (dg) · `rdrd_sd` ← `TC_RDRD[17:12]` (dr) · `rdrd_dd` ←
//!     `TC_RDRD[23:18]` (dd).
//!   - `wrwr_scl` ← `TC_WRWR[5:0]` · `wrwr_sc` ← `[11:6]` · `wrwr_sd` ←
//!     `[17:12]` · `wrwr_dd` ← `[23:18]`.
//!   - tCKE / tREFI are decoded by the core ([`tc_rap_tcke`] /
//!     [`tc_rfp_refi`]) but have **no frozen display slot** → not shown
//!     (the raw values remain available via the sysfs attributes).
//! - `cad_bus`, `voltages` → every field `Na(NotApplicable)` (unchanged:
//!   the IMC window does not expose drive strengths or rails).
//!
//! # Generation gate (profile dispatch)
//!
//! [`decode`] dispatches on [`intel_gen::profile_for`]: every profiled
//! generation has a live decode path — Tier 1 = `{Skylake, KabyLake,
//! CoffeeLake, CometLake}` and Rocket Lake decode from the verified
//! 64 KiB register map (identical offsets — Rocket additionally
//! decodes `gear_mode` + `uclk_mhz` from `MC_BIOS_REQ[17:16]`), and
//! the Tier-3 Alder family (Alder / Raptor / Meteor / Arrow Lake,
//! IG-26) decodes its subchannels from the widened 256 KiB offset set:
//! the decode-time subchannel set is picked by OQ-11's DDR5 condition
//! (both `MAD_DIMM_CH2` / `MAD_DIMM_CH3` raws present and non-zero → 4
//! subchannels, else the 2-channel DDR4 default), each subchannel maps
//! to its [`AlderChannelMap`] descriptor and decodes through the
//! Research-line-329 mirror → MCL fallback ([`tier3_mcl_for`]) into
//! [`decode_tier3_channel`], with the gear cells layered on from
//! `MC_BIOS_REQ[17:16]` on its `GearCap::Gear4` profile; for
//! `MeteorLake` / `ArrowLake` the OQ-5 tile-routing probe (IG-28)
//! runs first — a fully degenerate primary window (`None` / `0x0` /
//! `0xFFFF_FFFF`, Research line 328: the IMC behind the secondary
//! `MCHBAR + 0x180000` SoC-tile range) degrades the whole readout to
//! `Na(UnsupportedHardware)` (branch-level detail:
//! [`tile_routing_suspected`]) — never a decode of the empty window.
//! Every remaining (unprofiled) generation — Jasper Lake / Lunar Lake /
//! Core Series 1 Mobile / `Unrecognized` — degrades the whole readout
//! to `Na(UnsupportedHardware)` — never garbage data from a mismatched
//! register map. (Ice Lake / Tiger Lake now share the Tier-1 profile;
//! Sapphire Rapids has its own Server profile.) [`gen_gate`] (the
//! facade's branch-level gate: Tier 1, Rocket Lake, the Tier-3 Alder
//! family, and the Server profile) carries the rich detail for the
//! unprofiled generations; [`tier1_gate`] remains the Tier-1-only
//! compatibility alias.
//!
//! # No-panic contract (D5)
//!
//! The pure core ([`decode`]) performs no I/O at all; [`read_intel`] and
//! [`read_regs`] run the vendor gate first (non-Intel hardware → a
//! structured [`TelemetryError::UnsupportedHardware`] / a degraded
//! all-Na readout, before any register is touched) and degrade
//! per-register failures to `Na(ParseError)`. No `unsafe` is added here
//! — the MMIO read primitive lives in the `intel_mchbar` module
//! (bounds-checked; out-of-bounds → `Parse`, never a fault).
//!
//! # The live decode is the only decode
//!
//! The **live** decode path is exclusively [`decode`] over
//! [`IntelImcRegs`] (fed by the sysfs reader and the `/dev/mem`
//! readers [`IntelImcRegs::from_bar`] (Tier 1 / Rocket Lake, 64 KiB)
//! / [`IntelImcRegs::from_bar_wide`] (the Tier-3 Alder family,
//! 256 KiB — routed by [`read_intel`] on the profile's map family)).
//! The pre-v2.2.1 skeleton decode (the known-wrong
//! `IMC_FREQ_RATIO @ 0x5058` mis-location) is **gone** (TEL-4): the
//! downstream GUI / TUI / client / standalone-bin fixture code builds
//! its synthetic [`IntelReadout`]s through the [`fixture_intel_readout`]
//! helper (a purpose-built constructor over the live [`decode`]).
//!
//! # Fixture tests (the CI stand-in for the hardware acceptance)
//!
//! The `tests` module pins the plan §6.1 Skylake acceptance numbers
//! exactly (raw `mcbios_req = 0x00000012` = ratio 18 @ 133.3333 MHz →
//! 2400 MHz MCLK / 4800 MT/s; `ch0_tc_dbp = 0x11110F11` → 17-15-17-17;
//! `ch0_tc_rap = 0x27180204` → tRRD_S 4 / tRTP 8 / tFAW 24 / tRAS 39;
//! `ch0_tc_rfp = 0x000001A4` → tRFC 420; ch1 symmetric; rc = 56), plus
//! all-absent degradation, per-register containment, zero-field /
//! reserved / overflow gates, the gen gate, and bincode round-trips —
//! all without hardware, root, or I/O.
//!
//! # Module layout (step 2 of the directory split)
//!
//! The frozen type shapes live in [`types`] and the shared sanity
//! gates / degenerate-word screening helpers in [`gates`] (both are
//! re-exported at this module root, so every existing
//! `crate::intel_readout::X` path is unchanged). The decode paths are
//! split by generation: [`legacy_ddr4`] (Tier-1 Skylake–Comet Lake),
//! [`modern_ddr5`] (Tier-3 Alder/Meteor/Arrow DDR5), [`modern_ddr4`]
//! (Alder Lake DDR4, Phase 1), and [`server_uncore`] (Haswell-E /
//! Broadwell-E / Skylake-X / Cascade Lake). Channel resolution
//! (the 4-tier trust procedure, the C-3 aliasing policy) lives in
//! [`channel`]. The register readers, the shared MCLK / gear / uclk
//! decoders, and the profile-dispatch [`decode`] entry point stay in
//! this file.

pub mod channel;
pub mod gates;
pub mod legacy_ddr4;
pub mod modern_ddr4;
pub mod modern_ddr5;
pub mod server_uncore;
pub mod types;

#[cfg(test)]
pub(crate) mod fixtures;

// Re-export the public type shapes so external `use ...intel_readout::X`
// paths still work (step 1 of the directory-module split).
pub use types::*;

// Re-export the channel resolution helpers so external
// `use ...intel_readout::resolve_channel` paths still work.
pub use channel::*;

// Re-export the server-uncore decode so external
// `use ...intel_readout::decode_server` / `is_server_gen` paths still work.
pub use server_uncore::*;

// The gate helpers are module-local (`pub(super)` in `gates`); the
// degenerate-word screeners keep their crate-visible contract (N-05)
// at this module root.
pub(crate) use gates::is_present_word;

// Module-local imports for the decode paths and the tests below.
use std::fmt::Write as _;

use gates::*;

use crate::amd_readout::{EccStatus, GearMode, TimingSet};
use crate::cpuid::{CpuInfo, CpuVendor, IntelGen};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_gen::{profile_for, GearCap, GenMap, ALDER_CHANNELS};
use crate::intel_mchbar::MchBar;
use crate::intel_smbios::SmbiosTopology;

// ---------------------------------------------------------------------------
// Hardware-authoritative Tier-1 register table (MCHBAR-relative offsets).
// ---------------------------------------------------------------------------

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
/// `/dev/mem` map ([`IntelImcRegs::from_bar_wide`], IG-25) and
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
const REF_CLK_133_MHZ: f64 = 400.0 / 3.0;

/// Reference clock for `MC_BIOS_REQ` REF_CLK = 1 (100 MHz).
const REF_CLK_100_MHZ: f64 = 100.0;

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
/// address is [`imc_base_for`] + this: `0x5000` pre-12th-gen (→ the
/// legacy `0x5060` / `0x5064`) or `0xD800` 12th-gen+ (→ `0xD860` /
/// `0xD864`). The `+0x60` / `+0x64` intra-base position is preserved from
/// the legacy map — it is **not** the `+0x0C` / `+0x10`
/// `MAD_INTER_CHANNEL` / `MAD_DIMM_CH0/1` stride [`read_intel`] reads, and
/// reusing that stride would collide with the native MCL1 block (also at
/// `0xD800`, whose `TC_RFP` / `TC_RFP2` sit at `+0x14` / `+0x18`).
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
    /// The map is profile-sized ([`intel_mchbar::window_size_for`]:
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
    /// ([`imc_base_for`]: `0xD800` for 12th-gen+, `0x5000` for
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
        // `MAD_INTER_CHANNEL` / `MAD_DIMM_CH0/1` stride [`read_intel`]
        // reads, which would collide with the native MCL1 block).
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

// ---------------------------------------------------------------------------
// Gates (vendor + v1 Tier-1 generation).
// ---------------------------------------------------------------------------

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
/// map above is verified for. A compatibility alias kept from the
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
/// This is the gate that lets Rocket Lake reach [`decode`]'s profile
/// dispatch for the first time (IG-14/IG-15 make `detect()` emit it;
/// IG-12 wires the profile). [`tier1_gate`] is retained as the
/// Tier-1-only compatibility alias for other callers.
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
/// ([`IntelImcRegs::primary_window_degenerate`]). Per Research line
/// 328, on disaggregated SoC/compute tiles the physical IMC can sit
/// behind the secondary uncore range (MCHBAR + 0x180000) that the
/// module does not expose; the honest degradation is the all-Na
/// readout ([`decode`] returns the all-`Na(UnsupportedHardware)` shape
/// at the generation's channel count) with this message — never a
/// value fabricated from the secondary range.
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
/// flips [`decode`] to the 4-subchannel geometry; this function stays
/// the static model.)
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

// ---------------------------------------------------------------------------
// Shared MCLK / gear / uclk decoders (used by all three decode paths).
// ---------------------------------------------------------------------------

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
/// generations never reach [`decode_mclk`] from [`decode`] (they
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
/// wired into [`decode`] by IG-12 for the Rocket Lake profile.
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
/// [`decode`] by IG-12 for the Rocket Lake profile.
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

// ---------------------------------------------------------------------------
// Shared readout builders.
// ---------------------------------------------------------------------------

/// One all-`Na(UnsupportedHardware)` channel: the degraded shape for a
/// generation the v1 register map does not cover (plan §3.4 — never
/// garbage data from a mismatched register map).
fn unsupported_channel(index: u8) -> IntelChannel {
    let na = NaReason::UnsupportedHardware;
    let na_timing = || Section::na(NaReason::UnsupportedHardware);
    IntelChannel {
        index,
        // N-06: the structural NAs come from the shared builders.
        clocks: na_clocks(&na),
        timings: TimingSet {
            cl: na_timing(),
            rcwdwr: na_timing(),
            rcdrd: na_timing(),
            rp: na_timing(),
            ras: na_timing(),
            rc: na_timing(),
            rrds: na_timing(),
            rrld: na_timing(),
            faw: na_timing(),
            wtrs: na_timing(),
            wtrl: na_timing(),
            wr: na_timing(),
            rfc1: na_timing(),
            rfc2: na_timing(),
            rfcsb: na_timing(),
            cwl: na_timing(),
            rtp: na_timing(),
            rdwr: na_timing(),
            wrrd: na_timing(),
            rdrd_sd: na_timing(),
            rdrd_dd: na_timing(),
            rdrd_scl: na_timing(),
            rdrd_sc: na_timing(),
            wrwr_sd: na_timing(),
            wrwr_dd: na_timing(),
            wrwr_scl: na_timing(),
            wrwr_sc: na_timing(),
        },
        cad_bus: na_cad_bus(&na),
        voltages: na_voltages(&na),
        rtl: na_rtl(&na),
    }
}

/// An all-Na (terminal / unsupported) readout: the degraded shape for a
/// generation the decode does not cover, or a fully degenerate primary
/// window. Carries no channel-mode claim (mode / tech / trust all `None`)
/// but preserves the resolved SMBIOS population count and a human
/// annotation (L-2: the all-Na degradation no longer discards the
/// resolved `dimm_count`).
fn all_na_readout(channels: Vec<IntelChannel>, dimm_count: u8, annotation: &str) -> IntelReadout {
    IntelReadout {
        channels,
        channel_mode: None,
        ecc_status: EccStatus::Unknown,
        memory_tech: None,
        channel_trust: None,
        channel_annotation: annotation.to_owned(),
        dimm_count,
    }
}

/// L-7: the structured all-Na readout a server platform gets when the
/// MMIO [`decode`] / [`read_intel`] path is hit directly (bypassing the
/// facade's `is_server_gen` routing to [`decode_server`]). Server uncore
/// has no OS-accessible MC timing registers, so the honest degradation is
/// the all-`Na(UnsupportedHardware)` shape at the generation's channel
/// count, with a note pointing at the correct path and the resolved SMBIOS
/// population preserved (L-2).
fn server_all_na(gen: IntelGen, dimm_count: u8) -> IntelReadout {
    let count = channel_count(gen);
    let mut channels = Vec::with_capacity(usize::from(count));
    for ch in 0..count {
        channels.push(unsupported_channel(ch));
    }
    all_na_readout(
        channels,
        dimm_count,
        "server platform: use decode_server (SMBIOS+SPD), not MMIO decode",
    )
}

/// A decoded readout: the resolved channels + the 4-tier channel
/// resolution (mode / tech / trust / annotation / dimm count) + the ECC
/// state.
pub(crate) fn decoded_readout(
    channels: Vec<IntelChannel>,
    res: &ChannelResolution,
    ecc: EccStatus,
) -> IntelReadout {
    IntelReadout {
        channels,
        channel_mode: res.mode,
        ecc_status: ecc,
        memory_tech: res.tech,
        channel_trust: res.trust,
        channel_annotation: res.annotation.clone(),
        dimm_count: res.dimm_count,
    }
}

// ---------------------------------------------------------------------------
// The pure decode core (profile dispatch).
// ---------------------------------------------------------------------------

/// Decode the raw register set over the raw register set (the single decode
/// implementation for both producers — the sysfs path and the `/dev/mem`
/// fallback; plan §3.5).
///
/// Dispatches on [`intel_gen::profile_for`] — "Tier 1 is one case of
/// the dispatcher":
///
/// - **Tier 1 + Rocket Lake**: both channels decode from the verified
///   64 KiB register map (identical offsets — Tier 1 and Rocket share
///   it): the shared `MC_BIOS_REQ` core clock plus the per-channel
///   `TC_*` blocks (per-register containment; the frozen [1, 5333] MHz
///   sanity gate). Rocket Lake additionally
///   decodes `gear_mode` + `uclk_mhz` from `MC_BIOS_REQ[17:16]`
///   (Breakdown §3D, Gear2 cap). Breakdown §3B's Rocket Lake
///   CLK_RATIO widening is a no-op in code: [`decode_mclk`] already
///   masks `[7:0]`. The MCLK ratio semantics inside [`decode_mclk`]
///   are generation-keyed (the Alder Lake MC_BIOS_REQ decode): Tier 1
///   = `ratio × refclk`, Rocket = `ratio × refclk / 2`.
/// - **the Tier-3 Alder family** (`AlderLake` / `RaptorLake` /
///   `MeteorLake` / `ArrowLake` — IG-26): the decode-time subchannel
///   set is picked by OQ-11's DDR5 condition — **4 subchannels** when
///   both `MAD_DIMM_CH2` / `MAD_DIMM_CH3` raws are present and
///   non-zero, else the **2-channel DDR4 default** (MC0 subch0 + MC1
///   subch2). Each subchannel index maps to its [`AlderChannelMap`]
///   descriptor; its effective [`MclRegs`] is the Research-line-329
///   mirror → MCL fallback ([`tier3_mcl_for`]); the widened DDR5
///   fields decode through [`decode_tier3_channel`] (per-register
///   containment; the same [1, 5333] MHz gates); the MCLK is the
///   11th-gen-and-later half-step decode (`ratio × refclk / 2` — the
///   Alder Lake MC_BIOS_REQ decode); the
///   gear cells layer on from `MC_BIOS_REQ[17:16]` on the profile's
///   `GearCap::Gear4` (Breakdown §3D: gear 4 on Alder / Meteor /
///   Arrow for LPDDR5X and high-speed DDR5; Research line 145:
///   \>7200 MT/s). **OQ-11: confirm on hardware before shipping** —
///   the DDR5 condition is the working assumption.
/// - **the OQ-5 tile-routing probe (IG-28)** — `MeteorLake` /
///   `ArrowLake` only, before any subchannel decode: if every
///   primary-window register in [`IntelImcRegs`] is degenerate
///   (`None` / `0x0` / `0xFFFF_FFFF` — Research line 328's signature
///   for the IMC behind the secondary `MCHBAR + 0x180000` range the
///   module does not expose), the whole readout degrades to
///   `Na(UnsupportedHardware)` at the generation's channel count —
///   never a decode of the empty window, never a value fabricated
///   from the secondary range; the branch-level detail is
///   [`tile_routing_suspected`].
/// - **every remaining generation** — the unprofiled generations
///   (Jasper Lake / Lunar Lake / Core Series 1 Mobile /
///   `Unrecognized`): the whole readout degrades to
///   `Na(UnsupportedHardware)` channels — the registers are *not*
///   decoded (never garbage from a mismatched map; [`gen_gate`] — the
///   facade's branch gate — carries the rich detail for the branch-level
///   error; [`tier1_gate`] remains the Tier-1-only alias).
///
/// The channel-mode fields are resolved by the **4-tier trust procedure**
/// ([`resolve_channel`], Chunk 2): the capacity-derived MMIO mode
/// ([`mmio_channel_mode`] from the `MAD_DIMM_CH0/1` sums — the old
/// `MAD_INTER_CHANNEL[1:0]` 2-bit decode is gone; `[2:0]` is the
/// `DDR_TYPE` → [`MemoryTech`]) is cross-checked against the SMBIOS Type 17
/// topology (`smbios`). The `channel_mode` / `memory_tech` /
/// `channel_trust` / `channel_annotation` / `dimm_count` slots all carry
/// the resolution on the decoded path (the all-Na degradation keeps them
/// empty — behavior preserved). The L3 safety override is the core fix: an
/// MMIO `Single` decode contradicted by SMBIOS's 2-DIMMs-across-channels is
/// reported `Dual` (the misreported-single-channel bug).
///
/// Never panics (D5): a `None` register degrades only its sourced fields.
pub fn decode(regs: &IntelImcRegs, gen: IntelGen, smbios: &SmbiosTopology) -> IntelReadout {
    // L-7: server platforms use decode_server (SMBIOS + SPD), never the
    // MMIO decode. If called directly (bypassing the facade's
    // is_server_gen routing), return a structured all-Na readout — never a
    // decode of the (nonexistent) server MCHBAR window.
    if server_uncore::is_server_gen(gen) {
        return server_all_na(gen, smbios.populated_count);
    }
    // The 4-tier resolution (computed once; shared by the decoded arms).
    // L-10: all MAD data comes from `regs` (the decode inputs
    // `mad_inter_channel` / `mad_dimm_ch0/1` ride on the register set).
    let mmio_mode = channel::mmio_channel_mode(regs.mad_dimm_ch0, regs.mad_dimm_ch1);
    let mmio_tech = regs.mad_inter_channel.map(MemoryTech::from_mad_bits);
    let res = channel::resolve_channel(mmio_mode, mmio_tech, smbios);

    match profile_for(gen) {
        // Profiled with a live decode path: Tier 1 and Rocket Lake reuse
        // the identical 64 KiB register map; Rocket additionally decodes
        // the gear cells.
        Some(p) if matches!(p.map, GenMap::Tier1 | GenMap::Rocket) => {
            let mclk = decode_mclk(regs.mcbios_req, gen);
            // L-5: the gear cells are loop-invariant (the same MC_BIOS_REQ
            // word for every channel) — decode once, layer on every channel
            // (the Tier-3 pattern).
            let gear_cells =
                (p.gear != GearCap::None).then(|| gear_uclk_cells(p.gear, &mclk, regs.mcbios_req));
            let mut channels = Vec::with_capacity(usize::from(p.channel_count));
            for i in 0..p.channel_count {
                // L-4: explicit per-index handling — the Tier-1 register set
                // has exactly two channels; a higher index (unreachable for
                // the 2-channel profile) degrades to an all-Na channel
                // instead of mapping to ch1.
                let per = match i {
                    0 => &regs.ch0,
                    1 => &regs.ch1,
                    _ => {
                        channels.push(unsupported_channel(i));
                        continue;
                    }
                };
                let mut channel = legacy_ddr4::decode_tier1_channel(i, mclk.clone(), per);
                if let Some((gear_mode, uclk)) = &gear_cells {
                    channel.clocks.gear_mode = gear_mode.clone();
                    channel.clocks.uclk_mhz = uclk.clone();
                }
                channels.push(channel);
            }
            decoded_readout(channels, &res, EccStatus::Unknown)
        }
        // The Tier-3 Alder family (IG-26): OQ-11's DDR5 condition
        // (both MAD_DIMM_CH2 / MAD_DIMM_CH3 raws present and non-zero)
        // selects the 4-subchannel geometry; absent / zero raws keep
        // the 2-channel DDR4 default (MC0 subch0 + MC1 subch2).
        // OQ-11: confirm on hardware before shipping (working
        // assumption).
        Some(p) if matches!(p.map, GenMap::Alder) => {
            // ============================================================================
            // C-3: KNOWN REGISTER ALIASING ON 12th-gen+ (PENDING HARDWARE VERIFICATION)
            // ============================================================================
            // On Alder Lake and later, the MAD block base (0xD800) overlaps the MCL1
            // TC register block (also at 0xD800). The same physical words are exposed
            // as both:
            //   - mad_inter_channel / mad_intra_ch0 / mad_intra_ch1 / mad_dimm_ch0/1
            //   - mcl1_tc_pre / mcl1_tc_act / mcl1_tc_act2 / mcl1_tc_wtr / ...
            //
            // The current code consumes BOTH interpretations in a single decode call.
            // This is KNOWN to be incorrect — one of the two mappings is wrong.
            //
            // KNOWN CIRCULARITY: This policy keys on `res.tech`, which is resolved by
            // `resolve_channel` from `MAD_INTER_CHANNEL[2:0]` — a word read from the
            // very 0xD800 page whose ownership is in question. The policy is
            // self-consistent only if the initial MAD read is correct. To de-risk:
            // cross-check `res.tech` against the independent SMBIOS Type 17 memory
            // type before committing (applied in the policy selection below, N-04:
            // on disagreement the firmware-written SMBIOS source wins).
            //
            // Until hardware verification determines the true ownership of the 0xD800
            // page, the code uses the following heuristic:
            //   - If SMBIOS confirms DDR4: prefer MAD interpretation for geometry,
            //     ignore MCL1 timing words from the same page
            //   - If SMBIOS confirms DDR5: prefer MCL1 interpretation for timing,
            //     ignore MAD geometry words from the same page
            //   - If SMBIOS is unavailable: use MAD for geometry (safer default)
            //
            // TODO(C-3): Replace heuristic with verified register map once hardware
            // confirmation is available (requires Alder Lake DDR4 + DDR5 test systems).
            // ============================================================================
            // OQ-5 (IG-28): the tile-routing probe — on Meteor /
            // Arrow Lake, a fully degenerate primary window (every
            // register None / 0x0 / 0xFFFF_FFFF) is the runtime
            // signature of SoC-tile IMC routing (Research line 328):
            // the physical IMC sits behind the secondary range
            // (MCHBAR + 0x180000) the module does not expose.
            // All-Na(UnsupportedHardware) at the generation's channel
            // count (the branch-level detail is
            // [`tile_routing_suspected`]) — never a decode of the
            // empty window, never a secondary-range fabrication.
            if matches!(gen, IntelGen::MeteorLake | IntelGen::ArrowLake)
                && regs.primary_window_degenerate()
            {
                let count = channel_count(gen);
                let mut channels = Vec::with_capacity(usize::from(count));
                for ch in 0..count {
                    channels.push(unsupported_channel(ch));
                }
                // L-2: thread the resolved SMBIOS population + the
                // tile-routing annotation into the all-Na readout.
                return all_na_readout(
                    channels,
                    res.dimm_count,
                    "SoC-tile IMC routing suspected (MCHBAR + 0x180000 secondary range, OQ-5) — primary window degenerate",
                );
            }
            // C-2: A word is "present" only if it's non-zero AND not a
            // bus-fault (0xFFFF_FFFF). This prevents degenerate reads
            // from being misinterpreted as DDR5 evidence.
            let ddr5 = is_present_word(regs.mad_dimm_ch2) && is_present_word(regs.mad_dimm_ch3);
            // Phase 1 (Intel MC fix): the Alder / Raptor Lake **DDR4**
            // path. Taken only when the OQ-11 DDR5 subchannel condition is
            // NOT met (the 2-channel DDR4 default) AND the resolved memory
            // technology confirms DDR4 (`MAD_INTER_CHANNEL[2:0] == 0` via
            // the 4-tier [`resolve_channel`], else the SMBIOS Type 17
            // type). The DDR4 TC registers live at `0xE000`/`0x1E000`
            // (NOT the `0x4000` mirror window), decoded through
            // `ALDER_DDR4_FIELDS` over the 2-MC × 1-channel geometry
            // (`ALDER_DDR4_CHANNELS`). Meteor / Arrow Lake are DDR5-only
            // and never take this branch.
            if matches!(gen, IntelGen::AlderLake | IntelGen::RaptorLake)
                && !ddr5
                && matches!(res.tech, Some(MemoryTech::Ddr4))
            {
                return modern_ddr4::decode_alder_ddr4(regs, gen, &res);
            }

            // C-3: apply the aliasing policy so one interpretation of the
            // shared `0xD800` page does not corrupt the other.
            //
            // N-04: cross-check the MAD-derived tech against the
            // independent SMBIOS Type 17 memory type (the C-3 circularity
            // de-risk). `res.tech` comes from `resolve_channel`, which
            // prefers the MMIO `MAD_INTER_CHANNEL[2:0]` word — read from
            // the very `0xD800` page whose ownership is in question. On
            // disagreement, trust the firmware-written (not MMIO-read)
            // SMBIOS source for the policy selection and annotate the
            // discrepancy.
            let mut effective_res = res.clone();
            let policy_tech = match (res.tech, smbios.memory_type) {
                (Some(mad), Some(smb)) if mad != smb => {
                    if !effective_res.annotation.is_empty() {
                        effective_res.annotation.push(' ');
                    }
                    let _ = write!(effective_res.annotation,
                        "[C-3: MAD-derived tech {mad:?} disagrees with SMBIOS Type 17 ({smb:?}) — trusting the firmware-written SMBIOS source for the 0xD800 page policy]"
                    );
                    Some(smb)
                }
                _ => res.tech,
            };
            let policy = channel::c3_alias_policy(policy_tech);

            // The effective channel resolution: the Mcl1 (DDR5) policy does
            // not trust the MAD geometry words → recompute from SMBIOS
            // alone; the MadAnnotated (unknown) policy carries the aliasing
            // annotation; the Mad (DDR4) policy keeps `res`.
            match policy {
                channel::C3AliasPolicy::Mcl1 => {
                    effective_res = channel::resolve_channel(None, None, smbios);
                }
                channel::C3AliasPolicy::MadAnnotated => {
                    if !effective_res.annotation.is_empty() {
                        effective_res.annotation.push(' ');
                    }
                    effective_res.annotation.push_str(
                        "[0xD800 page: MAD/MCL1 aliasing — geometry from MAD, timing unavailable]",
                    );
                }
                channel::C3AliasPolicy::Mad => {}
            }

            // The effective register set: the Mad (DDR4) policy does not
            // trust the MCL1 timing words (the same `0xD800` page) → null
            // them (the non-aliased mirror @ `0x4800` is still used via
            // [`mirror_or_mcl`]).
            let mut effective_regs = regs.clone();
            if matches!(policy, channel::C3AliasPolicy::Mad) {
                effective_regs.mcl1 = MclRegs::default();
            }
            let subch_indices: &[usize] = if ddr5 { &[0, 1, 2, 3] } else { &[0, 2] };
            let mclk = decode_mclk(regs.mcbios_req, gen);
            // The gear cells are shared across subchannels (the same
            // MC_BIOS_REQ word): decode once, layer on every channel
            // (GearCap::Gear4 — the Alder profile; Breakdown §3D).
            let (gear_mode, uclk) = gear_uclk_cells(p.gear, &mclk, regs.mcbios_req);
            let mut channels = Vec::with_capacity(subch_indices.len());
            for (pos, &i) in subch_indices.iter().enumerate() {
                let map = &ALDER_CHANNELS[i];
                let mcl = modern_ddr5::tier3_mcl_for(i, map, &effective_regs);
                let mut channel =
                    modern_ddr5::decode_tier3_channel(i as u8, mclk.clone(), &mcl, map);
                channel.clocks.gear_mode = gear_mode.clone();
                channel.clocks.uclk_mhz = uclk.clone();
                // M-7: the display index is the channel's position in the
                // output vector, not the subchannel identity. The DDR4
                // 2-channel default uses subchannels `[0, 2]` (MC0-subch0 +
                // MC1-subch2); keeping `i` would render "ch 0 / ch 2" and
                // imply a gapped 4-channel platform. The DDR5 4-subchannel
                // set is already positional (`[0, 1, 2, 3]`), so this is a
                // no-op there.
                channel.index = pos as u8;
                channels.push(channel);
            }
            decoded_readout(channels, &effective_res, EccStatus::Unknown)
        }
        // Unprofiled generations, and any future profiled family whose
        // decode path has not landed (Sandy / Haswell): behavior
        // preserved — the all-Na(UnsupportedHardware) degradation at
        // the generation's channel count.
        _ => {
            let count = channel_count(gen);
            let mut channels = Vec::with_capacity(usize::from(count));
            for ch in 0..count {
                channels.push(unsupported_channel(ch));
            }
            // L-2: thread the resolved SMBIOS population + annotation.
            all_na_readout(channels, res.dimm_count, &res.annotation)
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points (both delegate to the same pure decode core).
// ---------------------------------------------------------------------------

/// Read and decode every IMC channel from a live, read-only [`MchBar`]
/// map (the `/dev/mem` fallback path).
///
/// Sequence (all no-panic; D5):
/// 1. [`CpuInfo::detect()`] → [`intel_gen_gate`]: non-Intel hardware
///    (including the AMD reference host) returns
///    [`TelemetryError::UnsupportedHardware`] before any register is
///    read.
/// 2. The read follows the generation's register-map family (the map
///    is already profile-sized by [`intel_mchbar::window_size_for`]
///    for the same profile): the Tier-3 Alder family reads the full
///    74-slot offset set through [`IntelImcRegs::from_bar_wide`]
///    (the 256 KiB profile map — the ch2/ch3 mirror blocks, the two
///    native MCL blocks @ `0xD000`/`0xD800`, and the
///    `MAD_DIMM_CH2/CH3` raws OQ-11's DDR5 condition needs, none of
///    which the 9-register set reads, which makes the 4-subchannel
///    decode reachable on live hardware, IG-26 fix); Tier 1 / Rocket
///    Lake keep the 9-register [`IntelImcRegs::from_bar`] (64 KiB).
///    Per-register containment: a failed read → `None`.
/// 3. [`decode`] dispatches on the generation's profile: every
///    profiled generation decodes its channels — Tier 1 + Rocket Lake
///    on the 64 KiB map, the Alder family on the Tier-3 subchannel
///    path (IG-26: the 2-channel DDR4 default, or the 4-subchannel
///    geometry when OQ-11's DDR5 condition holds); any non-profiled
///    Intel generation returns the degraded all-`Na(UnsupportedHardware)`
///    readout (honest N/A, never garbage — the facade's branch-level
///    gate surfaces [`gen_gate`]'s rich detail).
///
/// No I/O happens beyond the bounds-checked [`MchBar::read_u32`] (and the
/// SMBIOS read is supplied by the caller, `smbios` — the `/dev/mem` path
/// benefits from the same 4-tier cross-check as the sysfs path, plan
/// hard-constraint #5).
pub fn read_intel(bar: &MchBar, smbios: &SmbiosTopology) -> TelemetryResult<IntelReadout> {
    // L-9: the no-arg convenience wrapper (standalone use) detects the CPU
    // internally; the facade uses [`read_intel_with`] to thread its single
    // per-snapshot detect result through (no redundant re-detect).
    let gen = intel_gen_gate(&CpuInfo::detect())?;
    read_intel_with(bar, smbios, gen)
}

/// Read and decode every IMC channel from a live, read-only [`MchBar`]
/// map for an already-detected generation (L-9): the facade threads its
/// single per-snapshot detect result through, so the `/dev/mem` path does
/// not re-detect the CPU (the pre-L-9 path called `CpuInfo::detect()` a
/// third time per snapshot).
pub fn read_intel_with(
    bar: &MchBar,
    smbios: &SmbiosTopology,
    gen: IntelGen,
) -> TelemetryResult<IntelReadout> {
    // L-7: server platforms use decode_server (SMBIOS + SPD), never the
    // MMIO read — guard here (before any bar read) so a direct call
    // (bypassing the facade's is_server_gen routing) returns a structured
    // all-Na readout instead of reading the (nonexistent) server window.
    if server_uncore::is_server_gen(gen) {
        return Ok(server_all_na(gen, smbios.populated_count));
    }
    // Chunk 2 / L-10 / N-10: the gen-keyed MAD channel/geometry registers
    // (0xd800 for 12th-gen+, 0x5000 otherwise — [`imc_base_for`], the same
    // base the kernel module selects) ride on the register set (all MAD
    // data comes from `regs` in [`decode`]). The base is derived once and
    // threaded to the wide reader (whose `MAD_DIMM_CH2/CH3` raws key off
    // it) — N-10: the reader no longer hard-codes one generation's base.
    // Per-register containment: an out-of-bounds / failed read degrades
    // to `None` (never a panic).
    let base = imc_base_for(gen);
    // The /dev/mem read set follows the generation's register-map
    // family (IG-26 fix): the Alder family takes the 256 KiB wide set
    // (its map is the 256 KiB profile window); Tier 1 / Rocket Lake
    // (and any future 64 KiB family) keep the 9-register Tier-1 set.
    let mut regs = match profile_for(gen).map(|p| p.map) {
        Some(GenMap::Alder) => IntelImcRegs::from_bar_wide(bar, base),
        _ => IntelImcRegs::from_bar(bar),
    };
    regs.mad_inter_channel = bar.read_u32(base).ok();
    regs.mad_dimm_ch0 = bar.read_u32(base + 0x0C).ok();
    regs.mad_dimm_ch1 = bar.read_u32(base + 0x10).ok();
    Ok(decode(&regs, gen, smbios))
}

/// Decode a raw register set produced by *any* source (the sysfs path
/// and both `/dev/mem` readers ([`IntelImcRegs::from_bar`] /
/// [`IntelImcRegs::from_bar_wide`]) feed this identical pure core).
///
/// Total and panic-free: the vendor gate runs first — a non-Intel host
/// (this AMD reference host) yields the degraded all-`Na` readout
/// (2 channels) with zero register decoding; an Intel host decodes
/// through [`decode`] (the profile dispatch applies inside). The
/// facade's
/// branch-level gates (vendor → profiled generation → source) run
/// before this call in the live topology; this function re-verifies
/// so it can never decode off-vendor hardware even if misused
/// directly.
pub fn read_regs(regs: &IntelImcRegs) -> IntelReadout {
    match intel_gen_gate(&CpuInfo::detect()) {
        // The `read_regs` entry point carries no SMBIOS input (it is a
        // pure raw-set decode), so it resolves against the empty topology
        // (the 4-tier degrades to the terminal / MMIO-only states).
        Ok(gen) => decode(regs, gen, &SmbiosTopology::empty()),
        Err(_) => {
            let mut channels = Vec::with_capacity(2);
            for ch in 0..2u8 {
                channels.push(unsupported_channel(ch));
            }
            all_na_readout(channels, 0, "")
        }
    }
}

/// The MCHBAR-relative IMC base for a generation (Chunk 2): `0xd800` for
/// the 12th-gen+ set, `0x5000` for everything else (6th–11th gen). The
/// `/dev/mem` fallback ([`read_intel`]) and the kernel module both key
/// off this, so the MAD registers are read at the correct location.
pub fn imc_base_for(gen: IntelGen) -> usize {
    if channel::is_12th_gen_plus(gen) {
        0xD800
    } else {
        0x5000
    }
}

impl IntelReadout {
    /// The single-source-of-truth channel display label (Chunk 2): the
    /// trust-resolved [`ChannelMode`] + the DDR5 subchannel form
    /// ([`ChannelMode::display_label`]) + the "unverified" trust badge for
    /// the L3 override / L4 heuristic tiers. **Both the TUI and the GUI
    /// render this exact string**, so they can never disagree on the
    /// channel mode. `None` when no channel mode resolved.
    pub fn channel_display_label(&self) -> Option<String> {
        let mode = self.channel_mode?;
        let mut label = mode.display_label(self.memory_tech);
        if matches!(
            self.channel_trust,
            Some(ChannelTrust::Override) | Some(ChannelTrust::Heuristic)
        ) {
            label.push_str(" [unverified]");
        }
        Some(label)
    }
}

// ---------------------------------------------------------------------------
// Fixture helpers (TEL-4) — the purpose-built constructor the downstream
// GUI / TUI / client / standalone-bin fixture tests build their synthetic
// [`IntelReadout`]s through. The pre-v2.2.1 skeleton decode (the known-
// wrong `IMC_FREQ_RATIO @ 0x5058` map) is gone: the only decode in this
// module is the live [`decode`] over [`IntelImcRegs`].
// ---------------------------------------------------------------------------

/// The canonical "fully populated" channel raws (the §6.1 acceptance
/// raws — in-range values that decode cleanly through the live map):
/// `TC_DBP = 0x1111_0F11` (tCL 17, tCWL 15, tRCD 17, tRP 17),
/// `TC_RAP = 0x2718_0204` (tRRD_S 4, tRTP 8, tFAW 24, tRAS 39),
/// `TC_RFP = 0x0000_01A4` (tRFC 420), `TC_RAP2 = 0x0000_0C0A`
/// (tRRD_L 10, tWR 12), and the four turnaround quartets.
pub fn fixture_channel_regs() -> ChannelRegs {
    ChannelRegs {
        tc_dbp: Some(0x1111_0F11),
        tc_rap: Some(0x2718_0204),
        tc_rfp: Some(0x0000_01A4),
        tc_rap2: Some(0x0000_0C0A),
        tc_rdrd: Some(0x0048_C286),
        tc_rdwr: Some(0x0000_0280),
        tc_wrrd: Some(0x0000_0308),
        tc_wrwr: Some(0x0040_C204),
    }
}

/// A synthetic [`IntelReadout`] built through the **live** [`decode`]
/// (TEL-4): `gen` selects the profile (Tier 1 / Rocket Lake on the
/// 64 KiB map; the Alder family on the Tier-3 subchannel path);
/// `mcbios_req` is the global DRAM clock word (the MCLK source — e.g.
/// `0x0000_000C` = ratio 12 @ 133.3333 MHz = **1600 MHz** for a
/// Skylake–Comet Lake host; `0x0000_0120` = ratio 32 @ 100 MHz / 2 =
/// **1600 MHz**, gear bits `00b` = 1×, for an 11th-gen-and-later host);
/// `ch0` / `ch1` are the raw channel register sets ([`ChannelRegs::
/// default()`] = the all-`Na` degradation channel); `single` truncates
/// the readout to its first channel (the decode always emits the
/// profile's channel count — the single-channel fixture shape).
///
/// The fixture carries no MAD raws and no SMBIOS topology, so the
/// 4-tier channel resolution lands in the terminal branch; the
/// constructor normalizes those slots to the canonical empty fixture
/// state (`channel_mode` / `memory_tech` / `channel_trust` `None`,
/// empty annotation, `dimm_count` 0, `ecc_status`
/// [`EccStatus::Unknown`]). The caller may override `channel_mode`
/// (e.g. [`ChannelMode::DualFlex`]) for the mode-line rendering tests.
pub fn fixture_intel_readout(
    gen: IntelGen,
    mcbios_req: Option<u32>,
    ch0: &ChannelRegs,
    ch1: &ChannelRegs,
    single: bool,
) -> IntelReadout {
    let regs = IntelImcRegs {
        mcbios_req,
        ch0: ch0.clone(),
        ch1: ch1.clone(),
        ..IntelImcRegs::default()
    };
    let mut out = decode(&regs, gen, &SmbiosTopology::default());
    out.channel_mode = None;
    out.memory_tech = None;
    out.channel_trust = None;
    out.channel_annotation = String::new();
    out.dimm_count = 0;
    out.ecc_status = EccStatus::Unknown;
    if single {
        out.channels.truncate(1);
    }
    out
}

// ---------------------------------------------------------------------------
// Orchestration tests (the CI stand-in for the hardware acceptance).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
