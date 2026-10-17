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
use gates::*;

use crate::amd_readout::{EccStatus, GearMode, TimingSet};
use crate::cpuid::{CpuInfo, CpuVendor, IntelGen};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_gen::{ALDER_CHANNELS, GearCap, GenMap, profile_for};
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
        IntelGen::RocketLake => {
            "Tier 2 (Gear Mode + turnaround parity; outside this Tier-1 gate)"
        }
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
fn all_na_readout(
    channels: Vec<IntelChannel>,
    dimm_count: u8,
    annotation: &str,
) -> IntelReadout {
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
pub fn decode(
    regs: &IntelImcRegs,
    gen: IntelGen,
    smbios: &SmbiosTopology,
) -> IntelReadout {
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
            let gear_cells = (p.gear != GearCap::None)
                .then(|| gear_uclk_cells(p.gear, &mclk, regs.mcbios_req));
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
            let ddr5 = is_present_word(regs.mad_dimm_ch2)
                && is_present_word(regs.mad_dimm_ch3);
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
                    effective_res.annotation.push_str(&format!(
                        "[C-3: MAD-derived tech {mad:?} disagrees with SMBIOS Type 17 ({smb:?}) — trusting the firmware-written SMBIOS source for the 0xD800 page policy]"
                    ));
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
            let subch_indices: &[usize] = if ddr5 {
                &[0, 1, 2, 3]
            } else {
                &[0, 2]
            };
            let mclk = decode_mclk(regs.mcbios_req, gen);
            // The gear cells are shared across subchannels (the same
            // MC_BIOS_REQ word): decode once, layer on every channel
            // (GearCap::Gear4 — the Alder profile; Breakdown §3D).
            let (gear_mode, uclk) = gear_uclk_cells(p.gear, &mclk, regs.mcbios_req);
            let mut channels = Vec::with_capacity(subch_indices.len());
            for (pos, &i) in subch_indices.iter().enumerate() {
                let map = &ALDER_CHANNELS[i];
                let mcl = modern_ddr5::tier3_mcl_for(i, map, &effective_regs);
                let mut channel = modern_ddr5::decode_tier3_channel(i as u8, mclk.clone(), &mcl, map);
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
mod tests {
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
            tc_rfp: Some(0x0000_01A4), // tRFC 420, tREFI 0 (not displayed)
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
        assert!((mhz - 1066.67).abs() < 0.01, "MCLK ≈ 1066.67 MHz, got {mhz}");
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
        assert_eq!(ch0.clocks.command_rate, Section::na(NaReason::NotApplicable));
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
        for s in [&ch0.cad_bus.rtt_nom, &ch0.cad_bus.rtt_wr, &ch0.cad_bus.rtt_park] {
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
            assert_eq!(mcbios_gear(0x0000_0000, cap), Some(GearMode::One), "00b, {cap:?}");
            assert_eq!(
                mcbios_gear(0x0112, cap),
                Some(GearMode::One),
                "00b + junk bits, {cap:?}"
            );
        }
        assert_eq!(mcbios_gear(0x0001_0000, GearCap::None), None, "01b over-cap (Tier 1)");
        assert_eq!(mcbios_gear(0x0001_0000, GearCap::Gear2), Some(GearMode::Two));
        assert_eq!(mcbios_gear(0x0001_0000, GearCap::Gear4), Some(GearMode::Two));
        assert_eq!(mcbios_gear(0x0002_0000, GearCap::None), None, "10b over-cap (Tier 1)");
        assert_eq!(
            mcbios_gear(0x0002_0000, GearCap::Gear2),
            None,
            "10b over-cap (Rocket, documented)"
        );
        assert_eq!(mcbios_gear(0x0002_0000, GearCap::Gear4), Some(GearMode::Four));
        assert_eq!(mcbios_gear(0x0003_0000, GearCap::None), None, "11b over-cap (Tier 1)");
        assert_eq!(mcbios_gear(0x0003_0000, GearCap::Gear2), Some(GearMode::Two), "11b (Gear2)");
        assert_eq!(mcbios_gear(0x0003_0000, GearCap::Gear4), Some(GearMode::Two), "11b (Gear4)");
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
        assert_eq!(mcbios_gear(raw, GearCap::Gear4), Some(GearMode::Two), "11b → gear 2");
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
        assert_eq!(MCHBAR_WINDOW_TIER3, 0x40000, "the Alder profile window (IG-03)");
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
        for (i, base) in [CHANNEL0_BASE, CHANNEL1_BASE, 0x4800, 0x4C00].into_iter().enumerate() {
            for (j, off) in ch_offsets.iter().enumerate() {
                seed_word(&mut buf, base + off, 0xA000_0000 | (i as u32) << 12 | (j as u32) << 8);
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
                seed_word(&mut buf, base + off, 0xB000_0000 | (i as u32) << 12 | (j as u32) << 8);
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
                seed_word(&mut buf, base + off, 0xD000_0000 | (i as u32) << 12 | (j as u32) << 8);
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
        assert_eq!(set.len(), 71, "exactly the full 71-slot Alder read set (51 + 20 DDR4 TC)");
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
                seed_word(&mut buf, base + off, 0xC000_0000 | (i as u32) << 12 | (j as u32) << 8);
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
            assert_eq!(ro.channels.len(), 4, "{gen:?}: DDR5 condition met -> 4 subchannels");
            assert_eq!(
                ro.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
                vec![0, 1, 2, 3],
                "{gen:?}"
            );
            assert_eq!(ro.channel_mode, None, "{gen:?}: no population evidence");
            for ch in &ro.channels {
                assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?}");
                assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Two), "{gen:?}");
                assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "{gen:?}: uclk = mclk / 2");
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
                    ro.channel_mode,
                    None,
                    "{gen:?} ({label}): no population evidence"
                );
                for ch in &ro.channels {
                    assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?} ({label})");
                    assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Two), "{gen:?} ({label})");
                    assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "{gen:?} ({label})");
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
                assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Four), "{gen:?}");
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
        let bytes = bincode::serialize(&ro)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
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
        let ro = decode(&tier3_ddr5_condition_regs(), IntelGen::AlderLake, &SmbiosTopology::empty());
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
        assert_eq!(ch0.timings.faw, Section::Value(16), "tFAW 16 in the 8-bit field");
        assert_eq!(ch0.timings.rfc1, Section::Value(1200), "tRFC1 1200 in the 12-bit field");
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
        assert_all_sourced_na(&ro.channels[0], "subch0: zero MCL words win (mirrors disabled)");
        assert_all_sourced_na(&ro.channels[1], "subch1: mirror-only, reserved TC_RDRD word");
        assert_all_sourced_na(&ro.channels[2], "subch2: all registers absent");
        assert_all_sourced_na(&ro.channels[3], "subch3: mirror-only, reserved TC_RDRD word");
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
                assert_eq!(ro.channels.len(), 2, "{gen:?} ({label}) (C-4: dual-channel)");
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
            assert_eq!(ro.channels.len(), 2, "{gen:?}: DDR4 default (MAD raws zero)");
            for ch in &ro.channels {
                assert_eq!(ch.clocks.mclk_mhz, Section::Value(2400.0), "{gen:?}");
                assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Two), "{gen:?}");
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
        let ro = decode(&IntelImcRegs::default(), IntelGen::Skylake, &SmbiosTopology::empty());
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

        for s in [&ch0.timings.ras, &ch0.timings.rrds, &ch0.timings.rtp, &ch0.timings.faw] {
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
        for s in [&ch1.timings.ras, &ch1.timings.rrds, &ch1.timings.rtp, &ch1.timings.faw] {
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
                assert_eq!(ch.rtl, Section::na(NaReason::UnsupportedHardware), "{gen:?}");
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
        assert!(matches!(
            ald,
            TelemetryError::UnsupportedHardware { .. }
        ));
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
            assert!(msg.contains("Tier 3 (Alder/Raptor/Meteor/Arrow Lake)"), "{msg}");
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
        let tier1 = decode(&acceptance_regs(), IntelGen::Skylake, &SmbiosTopology::empty());
        let mut regs = acceptance_regs();
        regs.mcbios_req = Some(0x0001_0012);
        let rocket = decode(&regs, IntelGen::RocketLake, &SmbiosTopology::empty());
        assert_eq!(rocket.channels.len(), 2, "Rocket Lake: two channels");
        for (t, r) in tier1.channels.iter().zip(&rocket.channels) {
            assert_eq!(t.clocks.mclk_mhz, Section::Value(2400.0), "Tier 1: ratio 18 full-step");
            assert_eq!(r.clocks.mclk_mhz, Section::Value(1200.0), "Rocket: ratio 18 half-step");
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
        let sync = decode(&acceptance_regs(), IntelGen::RocketLake, &SmbiosTopology::empty());
        for ch in &sync.channels {
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(1200.0), "ratio 18 half-step");
            assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
            assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "uclk = mclk (gear 1)");
        }
        let absent = decode(&IntelImcRegs::default(), IntelGen::RocketLake, &SmbiosTopology::empty());
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
            let p = profile_for(gen)
                .unwrap_or_else(|| panic!("{gen:?} must be profiled"));
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
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0), "mclk = 16 × 100 MHz");
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
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(3000.0), "mclk = 60 × 100 / 2");
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
        assert_eq!(legacy_ddr4::turnaround_quartet(0x0048_C286), [6, 10, 12, 18], "TC_RDRD");
        assert_eq!(legacy_ddr4::turnaround_quartet(0x0000_0280), [0, 10, 0, 0], "TC_RDWR");
        assert_eq!(legacy_ddr4::turnaround_quartet(0x0000_0308), [8, 12, 0, 0], "TC_WRRD");
        assert_eq!(legacy_ddr4::turnaround_quartet(0x0040_C204), [4, 8, 12, 16], "TC_WRWR");

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
            assert_eq!(ch.clocks.mclk_mhz, Section::Value(1200.0), "ratio 18 half-step");
            assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
            assert_eq!(ch.clocks.uclk_mhz, Section::Value(1200.0), "gear 1: uclk = mclk");
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
            decode(&IntelImcRegs::default(), IntelGen::Skylake, &SmbiosTopology::empty()),
            decode(&acceptance_regs(), IntelGen::AlderLake, &SmbiosTopology::empty()),
        ] {
            let bytes = bincode::serialize(&ro)
                .expect("IntelReadout must serialize (no-panic contract)");
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
            assert!(off <= MCHBAR_WINDOW, "offset {off:#x} must fit the 64 KiB window");
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
        assert_eq!(clock_section(5333.0), Section::Value(5333.0), "H-6: DDR5-10666 upper bound");
        for mhz in [0.0, 0.5, 5333.1, 65535.0] {
            assert!(matches!(clock_section(mhz), Section::Na(NaReason::ParseError(_))), "{mhz}");
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

        let bytes = bincode::serialize(&ro)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
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
        let bytes = bincode::serialize(&all_na)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
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
        assert!(!is_present_word(Some(0xFFFF_FFFF)), "bus-fault is not present");
        assert!(is_present_word(Some(1)), "non-zero non-fault is present");
        assert!(is_present_word(Some(0x1234_5678)), "arbitrary valid word is present");

        assert!(is_degenerate_word(0), "zero is degenerate");
        assert!(is_degenerate_word(0xFFFF_FFFF), "bus-fault is degenerate");
        assert!(!is_degenerate_word(1), "non-zero non-fault is not degenerate");
        assert!(!is_degenerate_word(0x1234_5678), "arbitrary valid word is not degenerate");
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
}
