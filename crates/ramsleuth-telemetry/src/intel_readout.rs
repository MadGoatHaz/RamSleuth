//! Intel IMC register decode into the frozen display types — the
//! hardware-authoritative Tier-1 register map (INTEL-02, Bug 2 fix).
//!
//! This module is the readout half of the Intel telemetry branch: it
//! decodes the raw memory-controller (IMC) register values — exposed RAW
//! by the `ramsleuth_intel` kernel module (the sysfs path) or read from
//! the `/dev/mem` MCHBAR fallback ([`IntelImcRegs::from_bar`]) — into the
//! frozen vendor-neutral display types shared with the AMD readout
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
//! Skylake / Kaby Lake / Coffee Lake / Comet Lake: a single memory
//! controller, 2 channels, DDR4, a 64 KiB mapped region):
//!
//! | Offset | Register | Fields decoded |
//! |---|---|---|
//! | `0x5E00` | `MC_BIOS_REQ` | `[7:0]` CLK_RATIO · `[8]` REF_CLK (0 = 133.3333 MHz, 1 = 100 MHz) · `[17:16]` GEAR_RATIO (Rocket+ only — not decoded in v1) · `[31]` RUN_BUSY (status — not decoded) |
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
//! kernel EDAC driver.
//!
//! # Mapping into the frozen display types (v1 Tier 1)
//!
//! - `mclk_mhz` ← `MC_BIOS_REQ`: `ratio × refclk` (the analog DRAM
//!   clock MCLK — e.g. ratio 18 × 133.3333 = **2400 MHz**; for DDR,
//!   MT/s = MCLK × 2), sanity-gated to [1, 4096] MHz; a ratio of 0 =
//!   unconfigured → `Na(ParseError)`.
//! - `uclk_mhz`, `fclk_mhz`, `gdm`, `pdm` ← `Na(NotApplicable)`
//!   (AMD-fabric concepts with no IMC analog).
//! - `div_mode`, `gear_mode` ← `Na(NotApplicable)` in v1 (the gear ratio
//!   exists only on Rocket+; Tier 2 will decode it from
//!   `MC_BIOS_REQ[17:16]`).
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
//! # Generation gate (v1 Tier 1)
//!
//! [`decode`] runs only for `{Skylake, KabyLake, CoffeeLake,
//! CometLake}`. Any other detected Intel generation (Alder/Raptor =
//! Tier 2, Meteor/Arrow = Tier 3, `Unrecognized`) degrades the whole
//! readout to `Na(UnsupportedHardware)` — never garbage data from a
//! mismatched register map. [`tier1_gate`] carries the rich detail (the
//! generation and its tier) for the facade's branch-level error.
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
//! # Legacy compatibility surface
//!
//! [`decode_channel`], [`channel_offsets`], and the legacy `decode_*`
//! bit-field helpers at the bottom of this module are the **pre-v2.2.1
//! skeleton decode, retained verbatim as a frozen compatibility
//! surface**: the GUI / TUI / client fixture code (which this chunk must
//! not touch) builds its synthetic [`IntelReadout`] fixtures through
//! [`decode_channel`], and the wire-safe [`IntelChannel`] /
//! [`IntelReadout`] shapes it produces stay stable (no protocol change).
//! The **live** decode path is exclusively [`decode`] over
//! [`IntelImcRegs`] (fed by the sysfs reader and
//! [`IntelImcRegs::from_bar`]); the skeleton path is not used by the
//! live path.
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

use crate::amd_readout::{CadBus, ClockReadout, DivMode, GearMode, TimingSet, VoltageSet};
use crate::cpuid::{CpuInfo, CpuVendor, IntelGen};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_mchbar::MchBar;

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

/// Global register: the BIOS request word carrying the DRAM clock ratio
/// (bits 7:0), the reference-clock select (bit 8), the gear ratio (bits
/// 17:16 — Rocket+ only, not decoded in v1), and RUN_BUSY (bit 31).
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

// ---------------------------------------------------------------------------
// Legacy (pre-v2.2.1 skeleton) register table — the frozen compatibility
// surface for the downstream fixture code; see the module docs.
// ---------------------------------------------------------------------------

/// Legacy skeleton global register — `0x5058` is a free-running
/// performance counter, not a frequency ratio (the Bug 2 mis-location).
pub const LEGACY_FREQ_RATIO_OFFSET: usize = 0x5058;

/// Legacy per-channel MCS command-block base; channel `n` sits at
/// `LEGACY_MCS_CHANNEL_BASE + n * LEGACY_MCS_CHANNEL_STRIDE`.
pub const LEGACY_MCS_CHANNEL_BASE: usize = 0x5400;
pub const LEGACY_MCS_CHANNEL_STRIDE: usize = 0x100;

/// Legacy offsets within a channel's MCS command block (in decode order).
const LEGACY_MCS_COMMAND_0: usize = 0x000; // tCL / tRCD / tRP / tRAS
const LEGACY_MCS_COMMAND_1: usize = 0x004; // command rate / gear / RTL
const LEGACY_MCS_COMMAND_2: usize = 0x008; // tCCD_S / tCCD_L
const LEGACY_MCS_COMMAND_3: usize = 0x00C; // tRDRD / tRDWR / tWRWR / tWRRD

/// Highest legacy per-channel offset (channel 3's last command register).
pub const LEGACY_MCS_MAX_OFFSET: usize =
    LEGACY_MCS_CHANNEL_BASE + 3 * LEGACY_MCS_CHANNEL_STRIDE + LEGACY_MCS_COMMAND_3;

// Compile-time proof that every register read in this module (both the
// hardware-authoritative Tier-1 table and the legacy skeleton table)
// fits the 64 KiB Tier-1 MCHBAR window (each read is a 4-byte access).
const _ASSERT_IMC_READS_IN_WINDOW: () = assert!(
    MAX_IMC_OFFSET <= MCHBAR_WINDOW
        && MAX_CHANNEL_OFFSET <= MCHBAR_WINDOW
        && LEGACY_FREQ_RATIO_OFFSET + 4 <= MCHBAR_WINDOW
        && LEGACY_MCS_MAX_OFFSET <= MCHBAR_WINDOW
);

// ---------------------------------------------------------------------------
// Sanity ranges (mirroring the AMD display gates: the same plausible
// bounds apply to the vendor-neutral display types, plan D3).
// ---------------------------------------------------------------------------

/// Plausible DRAM subtiming range (integer DRAM clock cycles).
const TIMING_MIN_TICKS: u16 = 1;
const TIMING_MAX_TICKS: u16 = 2048;

/// Plausible memory/fabric clock range in MHz.
const CLOCK_MIN_MHZ: f64 = 1.0;
const CLOCK_MAX_MHZ: f64 = 4096.0;

// ---------------------------------------------------------------------------
// Raw register types (the module exposes raw values; THIS file decodes).
// ---------------------------------------------------------------------------

/// One channel's raw IMC registers (8 registers; `None` when the
/// underlying read failed — per-register containment).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelRegs {
    /// `TC_DBP` (tCL / tCWL / tRCD / tRP).
    pub tc_dbp: Option<u32>,
    /// `TC_RAP` (tRRD_S / tRTP / tCKE / tFAW / tRAS).
    pub tc_rap: Option<u32>,
    /// `TC_RFP` (tRFC / tREFI).
    pub tc_rfp: Option<u32>,
    /// `TC_RAP2` (tRRD_L / tWR).
    pub tc_rap2: Option<u32>,
    /// `TC_RDRD` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_rdrd: Option<u32>,
    /// `TC_RDWR` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_rdwr: Option<u32>,
    /// `TC_WRRD` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_wrrd: Option<u32>,
    /// `TC_WRWR` (4×6-bit sg / dg / dr / dd turnaround).
    pub tc_wrwr: Option<u32>,
}

/// The full raw IMC register set the decode consumes: one global
/// register plus the two Tier-1 channels (17 slots; `mchbar_base` /
/// `mchbar_enabled` are diagnostics carried by the producer, not decode
/// inputs).
///
/// Producers: `intel_sysfs::acquire()` (the `ramsleuth_intel` kobject)
/// and [`IntelImcRegs::from_bar`] (the `/dev/mem` fallback). Both feed
/// the same pure decode core ([`decode`] / [`read_regs`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IntelImcRegs {
    /// `MC_BIOS_REQ` @ `0x5E00` (the global DRAM clock word).
    pub mcbios_req: Option<u32>,
    /// Channel 0 (`0x4000` block).
    pub ch0: ChannelRegs,
    /// Channel 1 (`0x4400` block).
    pub ch1: ChannelRegs,
}

impl IntelImcRegs {
    /// Read the 9-register IMC set from a live, read-only [`MchBar`]
    /// map: the global `MC_BIOS_REQ` plus the two per-channel blocks.
    ///
    /// Per-register containment (D5): each read degrades independently
    /// to `None` on failure (bounds, unmap, …) — one bad read never
    /// poisons the rest and never panics. The only `MchBar` touch point
    /// in this module.
    pub fn from_bar(bar: &MchBar) -> Self {
        let mut ch0 = ChannelRegs::default();
        let mut ch1 = ChannelRegs::default();
        read_channel_regs(bar, CHANNEL0_BASE, &mut ch0);
        read_channel_regs(bar, CHANNEL1_BASE, &mut ch1);
        Self {
            mcbios_req: bar.read_u32(MC_BIOS_REQ_OFFSET).ok(),
            ch0,
            ch1,
        }
    }
}

/// Read one channel's eight raw registers from the live map into
/// `regs` (per-register containment via `read_u32().ok()`).
fn read_channel_regs(bar: &MchBar, base: usize, regs: &mut ChannelRegs) {
    regs.tc_dbp = bar.read_u32(base + TC_DBP_OFFSET).ok();
    regs.tc_rap = bar.read_u32(base + TC_RAP_OFFSET).ok();
    regs.tc_rfp = bar.read_u32(base + TC_RFP_OFFSET).ok();
    regs.tc_rap2 = bar.read_u32(base + TC_RAP2_OFFSET).ok();
    regs.tc_rdrd = bar.read_u32(base + TC_RDRD_OFFSET).ok();
    regs.tc_rdwr = bar.read_u32(base + TC_RDWR_OFFSET).ok();
    regs.tc_wrrd = bar.read_u32(base + TC_WRRD_OFFSET).ok();
    regs.tc_wrwr = bar.read_u32(base + TC_WRWR_OFFSET).ok();
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

/// v1 Tier 1: the generations the hardware-authoritative register map
/// above is verified for.
pub fn tier1_supported(gen: IntelGen) -> bool {
    matches!(
        gen,
        IntelGen::Skylake | IntelGen::KabyLake | IntelGen::CoffeeLake | IntelGen::CometLake
    )
}

/// The rich v1 Tier-1 generation gate: `Ok(())` for Tier 1, else
/// [`TelemetryError::UnsupportedHardware`] whose `vendor` detail names
/// the detected generation and its tier — the facade surfaces this as
/// the branch-level `Na(UnsupportedHardware)` (plan §3.4: never garbage
/// data from a mismatched register map).
pub fn tier1_gate(gen: IntelGen) -> TelemetryResult<()> {
    if tier1_supported(gen) {
        return Ok(());
    }
    let tier = match gen {
        IntelGen::AlderLake | IntelGen::RaptorLake => {
            "Tier 2 (dual-MC DDR4/DDR5, designed-for, not implemented in v1)"
        }
        IntelGen::MeteorLake | IntelGen::ArrowLake => "Tier 3 (DDR5, out of v1 scope)",
        _ => "beyond Tier 1",
    };
    Err(TelemetryError::UnsupportedHardware {
        vendor: format!(
            "Intel {gen:?} is {tier}; v1 decodes Tier 1 (Skylake/Kaby Lake/Coffee Lake/Comet Lake) only"
        ),
    })
}

/// The fixed channel-count model for an [`IntelGen`] (plan: per-channel
/// 0–3). DDR4-class generations expose 2 channels; DDR5-class client
/// generations expose 4; an unrecognized family-6 Intel falls back to
/// the common 2-channel desktop layout. Tier 1 is always 2.
pub fn channel_count(gen: IntelGen) -> u8 {
    match gen {
        IntelGen::Skylake
        | IntelGen::KabyLake
        | IntelGen::CoffeeLake
        | IntelGen::CometLake
        | IntelGen::IceLake
        | IntelGen::TigerLake
        | IntelGen::AlderLake
        | IntelGen::RaptorLake => 2,
        IntelGen::MeteorLake | IntelGen::ArrowLake => 4,
        IntelGen::Unrecognized => 2,
    }
}

// ---------------------------------------------------------------------------
// Pure bit-field decoders (the frozen Tier-1 decode contract; unit-tested
// with synthetic register values — no I/O, no unsafe).
// ---------------------------------------------------------------------------

/// Extract the bit field `[hi:lo]` of `raw` as a `u16`.
///
/// Every frozen field is ≤ 12 bits wide, so the masked value converts
/// losslessly; the `unwrap_or` fallback is an unreachable no-panic guard.
fn field16(raw: u32, hi: u32, lo: u32) -> u16 {
    let mask = (1u32 << (hi - lo + 1)) - 1;
    u16::try_from((raw >> lo) & mask).unwrap_or(u16::MAX)
}

/// tCL — `TC_DBP` bits 5:0 (6-bit, integer DRAM clock cycles).
pub fn tc_dbp_cl(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tCWL — `TC_DBP` bits 13:8 (6-bit).
pub fn tc_dbp_cwl(reg: u32) -> u16 {
    field16(reg, 13, 8)
}

/// tRCD — `TC_DBP` bits 21:16 (6-bit). Intel enforces a *unified,
/// symmetric* tRCD: the same value feeds both the `rcdrd` and `rcwdwr`
/// display slots.
pub fn tc_dbp_trcd(reg: u32) -> u16 {
    field16(reg, 21, 16)
}

/// tRP — `TC_DBP` bits 29:24 (6-bit).
pub fn tc_dbp_trp(reg: u32) -> u16 {
    field16(reg, 29, 24)
}

/// tRRD_S — `TC_RAP` bits 5:0 (6-bit).
pub fn tc_rap_rrds(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tRTP — `TC_RAP` bits 11:6 (6-bit).
pub fn tc_rap_rtp(reg: u32) -> u16 {
    field16(reg, 11, 6)
}

/// tCKE — `TC_RAP` bits 15:12. **4-bit** (range 1–15; do not assume
/// 6-bit). Decoded by the core but has no frozen display slot (not
/// shown; the raw value stays available via the sysfs attributes).
pub fn tc_rap_tcke(reg: u32) -> u16 {
    field16(reg, 15, 12)
}

/// tFAW — `TC_RAP` bits 23:16 (8-bit).
pub fn tc_rap_faw(reg: u32) -> u16 {
    field16(reg, 23, 16)
}

/// tRAS — `TC_RAP` bits 31:24 (8-bit).
pub fn tc_rap_tras(reg: u32) -> u16 {
    field16(reg, 31, 24)
}

/// tRFC — `TC_RFP` bits 10:0 (11-bit, 1–2047).
pub fn tc_rfp_rfc1(reg: u32) -> u16 {
    field16(reg, 10, 0)
}

/// tREFI — `TC_RFP` bits 27:16 (12-bit). Decoded by the core but has no
/// frozen display slot (not shown; the raw value stays available via
/// the sysfs attributes).
pub fn tc_rfp_refi(reg: u32) -> u16 {
    field16(reg, 27, 16)
}

/// tRRD_L — `TC_RAP2` bits 5:0 (6-bit).
pub fn tc_rap2_rrld(reg: u32) -> u16 {
    field16(reg, 5, 0)
}

/// tWR — `TC_RAP2` bits 13:8 (6-bit).
pub fn tc_rap2_wr(reg: u32) -> u16 {
    field16(reg, 13, 8)
}

/// The 4×6-bit turnaround quartet of one `TC_*` register:
/// `(sg, dg, dr, dd)` — same channel / same bank group / different rank
/// / different channel, per the frozen `TC_RDRD` / `TC_RDWR` / `TC_WRRD`
/// / `TC_WRWR` layout (bits 5:0 / 11:6 / 17:12 / 23:18).
pub fn turnaround_quartet(reg: u32) -> [u16; 4] {
    [
        field16(reg, 5, 0),
        field16(reg, 11, 6),
        field16(reg, 17, 12),
        field16(reg, 23, 18),
    ]
}

/// The `MC_BIOS_REQ` analog DRAM clock (MCLK) in MHz: `ratio × refclk`
/// (the DRAM clock is the ratio multiple of the reference clock —
/// e.g. ratio 18 × 133.3333 = 2400 MHz; for DDR, MT/s = MCLK × 2 = 4800),
/// sanity-gated to [1, 4096] MHz.
///
/// Containment: an absent register → `Na(ParseError)`; a ratio of 0 =
/// unconfigured → `Na(ParseError)`; a reserved GEAR_RATIO encoding
/// (bits 17:16, Rocket+ only) does not affect the v1 decode.
fn decode_mclk(reg: Option<u32>) -> Section<f64> {
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
            clock_section(f64::from(ratio) * refclk)
        }
    }
}

/// The `MC_BIOS_REQ` reference-clock select (bit 8): `true` = 100 MHz,
/// `false` = 133.3333 MHz.
fn mcbios_refclk_100(raw: u32) -> bool {
    raw & 0x0100 != 0
}

/// Decode one displayed tick field from an optionally-failed register
/// read (the per-register containment rule): a failed read →
/// `Na(ParseError)`; a decoded value of `0` (untrained) or outside
/// [1, 2048] → `Na(ParseError)`; otherwise the tick value.
fn timing_cell(reg: Option<u32>, name: &str, extract: fn(u32) -> u16) -> Section<u16> {
    match reg {
        None => Section::na(NaReason::ParseError(format!("{name}: register read failed"))),
        Some(raw) => ticks_section(extract(raw), name),
    }
}

/// Sanity-gate a decoded tick count to a [`Section<u16>`].
fn ticks_section(ticks: u16, name: &str) -> Section<u16> {
    if (TIMING_MIN_TICKS..=TIMING_MAX_TICKS).contains(&ticks) {
        Section::Value(ticks)
    } else {
        Section::na(NaReason::ParseError(format!(
            "{name} {ticks} ticks outside plausible range [{TIMING_MIN_TICKS}, {TIMING_MAX_TICKS}]"
        )))
    }
}

/// Sanity-gate a decoded memory clock (MHz) to a [`Section<f64>`].
fn clock_section(mhz: f64) -> Section<f64> {
    if (CLOCK_MIN_MHZ..=CLOCK_MAX_MHZ).contains(&mhz) {
        Section::Value(mhz)
    } else {
        Section::na(NaReason::ParseError(format!(
            "memory clock {mhz} MHz outside plausible range [{CLOCK_MIN_MHZ:.0}, {CLOCK_MAX_MHZ:.0}]"
        )))
    }
}

// ---------------------------------------------------------------------------
// The pure decode core (hardware-authoritative Tier-1 map).
// ---------------------------------------------------------------------------

/// Decode one Tier-1 channel from its raw registers + the shared DRAM
/// core clock (the per-channel heart of [`decode`]). Never panics (D5):
/// every cell is a `Value`, an honest `Na(ParseError)` (a sourced
/// register failed / a field is untrained), or a structural
/// `Na(NotApplicable)`.
fn decode_tier1_channel(index: u8, mclk: Section<f64>, regs: &ChannelRegs) -> IntelChannel {
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

    // --- clocks: v1 decodes mclk only (the gear ratio is Rocket+,
    // decoded by Tier 2 from MC_BIOS_REQ[17:16]; the AMD-fabric slots
    // have no IMC analog) ----------------------------------------------
    let clocks = ClockReadout {
        mclk_mhz: mclk,
        uclk_mhz: Section::na(NaReason::NotApplicable),
        fclk_mhz: Section::na(NaReason::NotApplicable),
        div_mode: Section::na(NaReason::NotApplicable),
        gear_mode: Section::na(NaReason::NotApplicable),
        gdm: Section::na(NaReason::NotApplicable),
        pdm: Section::na(NaReason::NotApplicable),
        command_rate: Section::na(NaReason::NotApplicable),
    };

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
    // no RTL.
    let rtl = Section::na(NaReason::NotApplicable);

    // CAD bus + voltages: not exposed by the Intel IMC window (unchanged
    // from the v2.2.1 model).
    let cad_bus = CadBus {
        proc_odt: Section::na(NaReason::NotApplicable),
        rtt_nom: Section::na(NaReason::NotApplicable),
        rtt_wr: Section::na(NaReason::NotApplicable),
        rtt_park: Section::na(NaReason::NotApplicable),
        clk_drv: Section::na(NaReason::NotApplicable),
        addr_cmd_drv: Section::na(NaReason::NotApplicable),
        cs_odt_drv: Section::na(NaReason::NotApplicable),
        cke_drv: Section::na(NaReason::NotApplicable),
    };
    let voltages = VoltageSet {
        vddcr_soc_mv: Section::na(NaReason::NotApplicable),
        vddio_mem_mv: Section::na(NaReason::NotApplicable),
        vdd_misc_mv: Section::na(NaReason::NotApplicable),
        vpp_mv: Section::na(NaReason::NotApplicable),
        vcore_mv: Section::na(NaReason::NotApplicable),
    };

    IntelChannel {
        index,
        clocks,
        timings,
        cad_bus,
        voltages,
        rtl,
    }
}

/// Decode one displayed slot from a per-register turnaround quartet
/// (per-register containment: a failed register read →
/// `Na(ParseError)` for every slot sourced from it).
fn quartet_cell(quartet: Option<[u16; 4]>, name: &str, slot: usize) -> Section<u16> {
    match quartet {
        None => Section::na(NaReason::ParseError(format!("{name}: register read failed"))),
        Some(q) => ticks_section(q[slot], name),
    }
}

/// One all-`Na(UnsupportedHardware)` channel: the degraded shape for a
/// generation the v1 register map does not cover (plan §3.4 — never
/// garbage data from a mismatched register map).
fn unsupported_channel(index: u8) -> IntelChannel {
    let na = NaReason::UnsupportedHardware;
    let na_timing = || Section::na(NaReason::UnsupportedHardware);
    IntelChannel {
        index,
        clocks: ClockReadout {
            mclk_mhz: Section::na(na.clone()),
            uclk_mhz: Section::na(na.clone()),
            fclk_mhz: Section::na(na.clone()),
            div_mode: Section::na(na.clone()),
            gear_mode: Section::na(na.clone()),
            gdm: Section::na(na.clone()),
            pdm: Section::na(na.clone()),
            command_rate: Section::na(na.clone()),
        },
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
        cad_bus: CadBus {
            proc_odt: Section::na(na.clone()),
            rtt_nom: Section::na(na.clone()),
            rtt_wr: Section::na(na.clone()),
            rtt_park: Section::na(na.clone()),
            clk_drv: Section::na(na.clone()),
            addr_cmd_drv: Section::na(na.clone()),
            cs_odt_drv: Section::na(na.clone()),
            cke_drv: Section::na(na.clone()),
        },
        voltages: VoltageSet {
            vddcr_soc_mv: Section::na(na.clone()),
            vddio_mem_mv: Section::na(na.clone()),
            vdd_misc_mv: Section::na(na.clone()),
            vpp_mv: Section::na(na.clone()),
            vcore_mv: Section::na(na.clone()),
        },
        rtl: Section::na(na),
    }
}

/// The pure decode core over the raw register set (the single decode
/// implementation for both producers — the sysfs path and the `/dev/mem`
/// fallback; plan §3.5).
///
/// - **v1 Tier-1 generations** (`{Skylake, KabyLake, CoffeeLake,
///   CometLake}`): both channels decode from the verified register map —
///   the shared `MC_BIOS_REQ` core clock plus the per-channel `TC_*`
///   blocks (per-register containment; the frozen [1, 2048] tick /
///   [1, 4096] MHz sanity gates).
/// - **any other generation** (Tier 2 / Tier 3 / `Unrecognized`): the
///   whole readout degrades to `Na(UnsupportedHardware)` channels — the
///   registers are *not* decoded (never garbage from a mismatched map;
///   [`tier1_gate`] carries the rich detail for the branch-level error).
///
/// The `channel_mode` slot decodes `MAD_INTER_CHANNEL[1:0]` from
/// `mad_inter_channel` (`None` → `None`; reserved `11` → `None`); it is
/// independent of the generation gate and populated whenever the raw
/// value is present.
///
/// Never panics (D5): a `None` register degrades only its sourced fields.
pub fn decode(regs: &IntelImcRegs, gen: IntelGen, mad_inter_channel: Option<u32>) -> IntelReadout {
    let count = channel_count(gen);
    if !tier1_supported(gen) {
        let mut channels = Vec::with_capacity(usize::from(count));
        for ch in 0..count {
            channels.push(unsupported_channel(ch));
        }
        return IntelReadout {
            channels,
            channel_mode: None,
        };
    }
    let mclk = decode_mclk(regs.mcbios_req);
    let ch0 = decode_tier1_channel(0, mclk.clone(), &regs.ch0);
    let ch1 = decode_tier1_channel(1, mclk, &regs.ch1);
    IntelReadout {
        channels: vec![ch0, ch1],
        channel_mode: mad_inter_channel.and_then(ChannelMode::from_raw),
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
/// 2. [`IntelImcRegs::from_bar`] reads the 9-register set with
///    per-register containment (a failed read → `None`).
/// 3. [`decode`] runs the v1 Tier-1 generation gate + the register map:
///    Tier 1 decodes both channels; any other Intel generation returns
///    the degraded all-`Na(UnsupportedHardware)` readout (honest N/A,
///    never garbage — the facade's branch-level gate in a later phase
///    surfaces [`tier1_gate`]'s rich detail).
///
/// No I/O happens beyond the bounds-checked [`MchBar::read_u32`].
pub fn read_intel(bar: &MchBar) -> TelemetryResult<IntelReadout> {
    let info = CpuInfo::detect();
    let gen = intel_gen_gate(&info)?;
    let regs = IntelImcRegs::from_bar(bar);
    Ok(decode(&regs, gen, None))
}

/// Decode a raw register set produced by *any* source (the sysfs path
/// and [`IntelImcRegs::from_bar`] feed this identical pure core).
///
/// Total and panic-free: the vendor gate runs first — a non-Intel host
/// (this AMD reference host) yields the degraded all-`Na` readout
/// (2 channels) with zero register decoding; an Intel host decodes
/// through [`decode`] (the Tier-1 gate applies inside). The facade's
/// branch-level gates (vendor → Tier 1 → source) run before this call
/// in the live topology; this function re-verifies so it can never
/// decode off-vendor hardware even if misused directly.
pub fn read_regs(regs: &IntelImcRegs) -> IntelReadout {
    match intel_gen_gate(&CpuInfo::detect()) {
        Ok(gen) => decode(regs, gen, None),
        Err(_) => {
            let mut channels = Vec::with_capacity(2);
            for ch in 0..2u8 {
                channels.push(unsupported_channel(ch));
            }
            IntelReadout {
                channels,
                channel_mode: None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Frozen display types (shape unchanged — wire-safe, bincode round-trips
// stay valid; only which cells carry `Value` vs `Na` changes).
// ---------------------------------------------------------------------------

/// One decoded memory channel of an Intel platform.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelChannel {
    /// 0-based channel index (`index < channel_count(gen)`).
    pub index: u8,
    /// Clocks and ratios: in the live Tier-1 decode `mclk` comes from
    /// `MC_BIOS_REQ` and every other clock cell is `Na(NotApplicable)`
    /// (v1 — the gear ratio is Rocket+ only; AMD-fabric slots have no
    /// IMC analog). The legacy compat path may populate the gear /
    /// uclk cells.
    pub clocks: ClockReadout,
    /// DRAM subtimings in integer DRAM clock cycles: 24 populated /
    /// derived slots + 3 structural `Na(NotApplicable)` in the live
    /// Tier-1 decode (the full map in the module docs).
    pub timings: TimingSet,
    /// CAD bus: every field `Na(NotApplicable)` on Intel.
    pub cad_bus: CadBus,
    /// Memory rails: every field `Na(NotApplicable)` on Intel.
    pub voltages: VoltageSet,
    /// RTL (ticks): `Na(NotApplicable)` in the live Tier-1 decode — the
    /// legacy skeleton's "RTL" register was fiction; the legacy compat
    /// path may carry a `Value`.
    pub rtl: Section<u16>,
}

/// The hardware channel / interleave mode, decoded from the global
/// `MAD_INTER_CHANNEL` register (bits `[1:0]` CHAN_MODE).
///
/// Intel exposes the DRAM interleave configuration as a 2-bit field in the
/// global MAD (memory array descriptor) register set — it is *not*
/// derivable from the SPD-visible DIMM count. A Flex-Mode box (asymmetric
/// DIMMs, e.g. 16 + 8 GB) may bind only a single SPD yet run in Dual-Flex,
/// so the SPD-count label is authoritative only as a frontend fallback.
///
/// Wire-safe (serde + bincode); the reserved encoding `11` decodes to
/// `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelMode {
    /// `00`: dual-channel, symmetric interleaving.
    DualSymmetric,
    /// `01`: dual-channel, flex mode (asymmetric DIMMs).
    DualFlex,
    /// `10`: single-channel.
    Single,
}

impl ChannelMode {
    /// Decode bits `[1:0]` of `MAD_INTER_CHANNEL`: `00` = Dual Symmetric,
    /// `01` = Dual Flex, `10` = Single, `11` = reserved → `None`.
    pub fn from_raw(raw: u32) -> Option<ChannelMode> {
        match raw & 0x3 {
            0 => Some(ChannelMode::DualSymmetric),
            1 => Some(ChannelMode::DualFlex),
            2 => Some(ChannelMode::Single),
            _ => None,
        }
    }

    /// The full human-readable channel label (the header channel slot).
    pub fn label(&self) -> &'static str {
        match self {
            ChannelMode::DualSymmetric => "Dual-Channel (Symmetric)",
            ChannelMode::DualFlex => "Dual-Channel (Flex)",
            ChannelMode::Single => "Single-Channel",
        }
    }

    /// Short string for the header "Mode:" slot.
    pub fn mode_label(&self) -> &'static str {
        match self {
            ChannelMode::DualSymmetric => "Interleaved",
            ChannelMode::DualFlex => "Flex",
            ChannelMode::Single => "N/A",
        }
    }
}

/// The full Intel readout: one [`IntelChannel`] per detected channel.
///
/// Wire-safe (serde + bincode); the struct shape is frozen — only which
/// cells carry `Value` vs `Na` changes between decode generations.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelReadout {
    /// Decoded channels (0-based indices, `channels.len() == channel_count`).
    pub channels: Vec<IntelChannel>,
    /// Hardware channel mode from `MAD_INTER_CHANNEL[1:0]`; `None` on the
    /// `/dev/mem` fallback or a pre-24-attr module.
    pub channel_mode: Option<ChannelMode>,
}

// ---------------------------------------------------------------------------
// Legacy compatibility surface — the pre-v2.2.1 skeleton decode (Bug 2).
//
// Retained verbatim (old register map: `IMC_FREQ_RATIO @ 0x5058` + the
// packed `MCS_COMMAND_0..3` dwords @ `0x5400`) because the GUI / TUI /
// client fixture code builds its synthetic `IntelReadout`s through
// [`decode_channel`] and this chunk must not modify those crates. The
// live decode path is exclusively [`decode`] / [`read_regs`] /
// [`IntelImcRegs::from_bar`]; nothing live uses this surface.
// ---------------------------------------------------------------------------

/// Step between the legacy `IMC_FREQ_RATIO` codes (ratio × 10 MHz).
const FREQ_RATIO_STEP_MHZ: f64 = 10.0;

/// The four per-channel legacy MCS command-register offsets for channel
/// `ch`, in decode order (`COMMAND_0..3`). All values are 4-byte
/// aligned and within the MCHBAR window (compile-asserted; re-checked
/// by `MchBar::read_u32`).
pub fn channel_offsets(ch: u8) -> [usize; 4] {
    let base = LEGACY_MCS_CHANNEL_BASE + usize::from(ch) * LEGACY_MCS_CHANNEL_STRIDE;
    [
        base + LEGACY_MCS_COMMAND_0,
        base + LEGACY_MCS_COMMAND_1,
        base + LEGACY_MCS_COMMAND_2,
        base + LEGACY_MCS_COMMAND_3,
    ]
}

/// Extract a register field that is already masked to ≤ 255 as a `u16`
/// (legacy). The caller's mask makes the conversion lossless, so
/// `try_from` always succeeds; the `unwrap_or` fallback is an
/// unreachable no-panic guard.
fn field_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// Legacy tCL — `MCS_COMMAND_0` bits 7:0 (ticks).
pub fn decode_tcl(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// Legacy tRCD — `MCS_COMMAND_0` bits 15:8 (ticks).
pub fn decode_trcd(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// Legacy tRP — `MCS_COMMAND_0` bits 23:16 (ticks).
pub fn decode_trp(reg: u32) -> u16 {
    field_u16((reg >> 16) & 0xFF)
}

/// Legacy tRAS — `MCS_COMMAND_0` bits 31:24 (ticks).
pub fn decode_tras(reg: u32) -> u16 {
    field_u16((reg >> 24) & 0xFF)
}

/// Legacy command rate — `MCS_COMMAND_1` bits 1:0: `0` = 1N, `1` = 2N;
/// `2`/`3` are reserved encodings → `None`.
pub fn decode_cmd_rate(reg: u32) -> Option<DivMode> {
    match reg & 0b11 {
        0 => Some(DivMode::OneToOne),
        1 => Some(DivMode::OneToTwo),
        _ => None,
    }
}

/// Legacy gear mode — `MCS_COMMAND_1` bits 3:2: `0` = 1×, `1` = 2×,
/// `2` = 4×; `3` is reserved → `None`.
pub fn decode_gear(reg: u32) -> Option<GearMode> {
    match (reg >> 2) & 0b11 {
        0 => Some(GearMode::One),
        1 => Some(GearMode::Two),
        2 => Some(GearMode::Four),
        _ => None,
    }
}

/// Legacy RTL — `MCS_COMMAND_1` bits 6:4 (ticks).
pub fn decode_rtl(reg: u32) -> u16 {
    field_u16((reg >> 4) & 0b111)
}

/// Legacy tCCD_S (short) — `MCS_COMMAND_2` bits 7:0 (ticks).
pub fn decode_tccd_s(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// Legacy tCCD_L (long) — `MCS_COMMAND_2` bits 15:8 (ticks).
pub fn decode_tccd_l(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// Legacy tRDRD — `MCS_COMMAND_3` bits 7:0 (ticks).
pub fn decode_trdrd(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// Legacy tRDWR — `MCS_COMMAND_3` bits 15:8 (ticks).
pub fn decode_trdwr(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// Legacy tWRWR — `MCS_COMMAND_3` bits 23:16 (ticks).
pub fn decode_twrwr(reg: u32) -> u16 {
    field_u16((reg >> 16) & 0xFF)
}

/// Legacy tWRRD — `MCS_COMMAND_3` bits 31:24 (ticks).
pub fn decode_twrrd(reg: u32) -> u16 {
    field_u16((reg >> 24) & 0xFF)
}

/// Legacy DRAM frequency ratio — `IMC_FREQ_RATIO` bits 7:0 (in 10 MHz
/// units); a code of `0` means unconfigured → `None`.
pub fn decode_freq_ratio(reg: u32) -> Option<u16> {
    let ratio = field_u16(reg & 0xFF);
    (ratio != 0).then_some(ratio)
}

/// The legacy SA:MEM divisor for a decoded [`GearMode`]
/// (`uclk = mclk / gear`).
fn gear_divisor(gear: GearMode) -> f64 {
    match gear {
        GearMode::One => 1.0,
        GearMode::Two => 2.0,
        GearMode::Four => 4.0,
    }
}

/// Decode one legacy tick field from an optionally-failed register read:
/// a failed read → `Na(ParseError)`; a decoded value outside [1, 2048]
/// (including `0`) → `Na(ParseError)`; otherwise the tick value.
fn timing_section(reg: Option<u32>, name: &str, f: fn(u32) -> u16) -> Section<u16> {
    match reg {
        None => Section::na(NaReason::ParseError(format!("{name}: register read failed"))),
        Some(raw) => ticks_section(f(raw), name),
    }
}

/// Decode one channel from raw register values (the legacy skeleton
/// compat core — see the module docs; not used by the live path).
///
/// `mclk_reg` is the global `IMC_FREQ_RATIO` read; `regs` holds the
/// channel's four MCS command registers in [`channel_offsets`] order.
/// Each is `None` when the underlying read failed; the fields sourced
/// from a failed register degrade to `Na(ParseError)` while the rest of
/// the channel decodes normally. Never panics (D5).
pub fn decode_channel(index: u8, mclk_reg: Option<u32>, regs: [Option<u32>; 4]) -> IntelChannel {
    // Decode order: (COMMAND_0, COMMAND_1, COMMAND_2, COMMAND_3).
    let [cmd0, cmd1, cmd2, cmd3] = regs;

    // --- clocks -----------------------------------------------------------
    let mclk = match mclk_reg.and_then(decode_freq_ratio) {
        Some(ratio) => clock_section(f64::from(ratio) * FREQ_RATIO_STEP_MHZ),
        None => Section::na(NaReason::ParseError(
            "DRAM frequency ratio absent (register read failed or unconfigured)".to_owned(),
        )),
    };
    let gear = cmd1.and_then(decode_gear);
    let uclk = match (mclk.value().copied(), gear) {
        (Some(m), Some(g)) => clock_section(m / gear_divisor(g)),
        (None, _) => Section::na(NaReason::ParseError(
            "uclk = mclk / gear: mclk unavailable".to_owned(),
        )),
        (_, None) => Section::na(NaReason::ParseError(
            "uclk = mclk / gear: gear unavailable or reserved encoding".to_owned(),
        )),
    };
    let div_mode = match cmd1.and_then(decode_cmd_rate) {
        Some(d) => Section::Value(d),
        None => Section::na(NaReason::ParseError(
            "command rate absent (register read failed) or reserved encoding".to_owned(),
        )),
    };
    let gear_mode = match gear {
        Some(g) => Section::Value(g),
        None => Section::na(NaReason::ParseError(
            "gear mode absent (register read failed) or reserved encoding".to_owned(),
        )),
    };
    let clocks = ClockReadout {
        mclk_mhz: mclk,
        uclk_mhz: uclk,
        fclk_mhz: Section::na(NaReason::NotApplicable),
        div_mode,
        gear_mode,
        gdm: Section::na(NaReason::NotApplicable),
        pdm: Section::na(NaReason::NotApplicable),
        // D-C11: the IMC command rate (1N/2N) is already surfaced as
        // `div_mode`; a separate slot would redundantly re-expose the
        // same bits → honest not-applicable.
        command_rate: Section::na(NaReason::NotApplicable),
    };

    // --- timings (ticks) ----------------------------------------------------
    let cl = timing_section(cmd0, "tCL", decode_tcl);
    let rcdrd = timing_section(cmd0, "tRCD", decode_trcd);
    let rp = timing_section(cmd0, "tRP", decode_trp);
    let ras = timing_section(cmd0, "tRAS", decode_tras);
    let rc = match (ras.value().copied(), rp.value().copied()) {
        (Some(a), Some(b)) => ticks_section(a.saturating_add(b), "tRC (tRAS + tRP)"),
        _ => Section::na(NaReason::NotApplicable),
    };
    let rrds = timing_section(cmd2, "tCCD_S", decode_tccd_s);
    let rrld = timing_section(cmd2, "tCCD_L", decode_tccd_l);
    let rdrd_sd = timing_section(cmd3, "tRDRD", decode_trdrd);
    let rdwr = timing_section(cmd3, "tRDWR", decode_trdwr);
    let wrwr_sd = timing_section(cmd3, "tWRWR", decode_twrwr);
    let wrrd = timing_section(cmd3, "tWRRD", decode_twrrd);
    let timings = TimingSet {
        cl,
        rcwdwr: Section::na(NaReason::NotApplicable),
        rcdrd,
        rp,
        ras,
        rc,
        rrds,
        rrld,
        faw: Section::na(NaReason::NotApplicable),
        wtrs: Section::na(NaReason::NotApplicable),
        wtrl: Section::na(NaReason::NotApplicable),
        wr: Section::na(NaReason::NotApplicable),
        rfc1: Section::na(NaReason::NotApplicable),
        rfc2: Section::na(NaReason::NotApplicable),
        rfcsb: Section::na(NaReason::NotApplicable),
        cwl: Section::na(NaReason::NotApplicable),
        rtp: Section::na(NaReason::NotApplicable),
        rdwr,
        wrrd,
        rdrd_sd,
        rdrd_dd: Section::na(NaReason::NotApplicable),
        rdrd_scl: Section::na(NaReason::NotApplicable),
        rdrd_sc: Section::na(NaReason::NotApplicable),
        wrwr_sd,
        wrwr_dd: Section::na(NaReason::NotApplicable),
        wrwr_scl: Section::na(NaReason::NotApplicable),
        wrwr_sc: Section::na(NaReason::NotApplicable),
    };

    let rtl = timing_section(cmd1, "RTL", decode_rtl);

    // --- CAD bus + voltages: not exposed by the Intel MCHBAR window --------
    let cad_bus = CadBus {
        proc_odt: Section::na(NaReason::NotApplicable),
        rtt_nom: Section::na(NaReason::NotApplicable),
        rtt_wr: Section::na(NaReason::NotApplicable),
        rtt_park: Section::na(NaReason::NotApplicable),
        clk_drv: Section::na(NaReason::NotApplicable),
        addr_cmd_drv: Section::na(NaReason::NotApplicable),
        cs_odt_drv: Section::na(NaReason::NotApplicable),
        cke_drv: Section::na(NaReason::NotApplicable),
    };
    let voltages = VoltageSet {
        vddcr_soc_mv: Section::na(NaReason::NotApplicable),
        vddio_mem_mv: Section::na(NaReason::NotApplicable),
        vdd_misc_mv: Section::na(NaReason::NotApplicable),
        vpp_mv: Section::na(NaReason::NotApplicable),
        vcore_mv: Section::na(NaReason::NotApplicable),
    };

    IntelChannel {
        index,
        clocks,
        timings,
        cad_bus,
        voltages,
        rtl,
    }
}

#[cfg(test)]
mod tests {
    //! Fixture tests — the CI stand-in for the hardware acceptance
    //! (plan §6): the Skylake acceptance raws (`mcbios_req = 0x12` →
    //! ratio 18 @ 133.3333 MHz = 2400 MHz MCLK / 4800 MT/s) pin the
    //! decoded numbers exactly, plus degradation / containment / gate /
    //! wire-safety coverage, all without hardware, root, or I/O. The
    //! legacy-compat surface keeps its original fixture tests (they
    //! exercise the frozen compatibility core the downstream GUI / TUI /
    //! client fixtures rely on).

    use super::*;
    use crate::cpuid::AmdZen;

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
        }
    }

    /// §6.1 step 4: `mcbios_req = 0x00000012` → ratio 18 × 133.3333 MHz
    /// = **2400 MHz** MCLK, **4800 MT/s** (for DDR, MT/s = MCLK × 2) —
    /// pinned exactly.
    #[test]
    fn acceptance_mcbios_req_decodes_2400_mhz_4800_mts() {
        let mclk = decode_mclk(Some(0x0000_0012));
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
        let mclk = decode_mclk(Some(0x0000_0008));
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
        let ro = decode(&acceptance_regs(), IntelGen::Skylake, Some(0));
        assert_eq!(ro.channels.len(), 2, "Tier 1: two channels");
        assert_eq!(
            ro.channel_mode,
            Some(ChannelMode::DualSymmetric),
            "MAD_INTER_CHANNEL[1:0] = 00 = dual symmetric"
        );
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
    // [1, 4096] MHz sanity gate.
    // -----------------------------------------------------------------

    /// REF_CLK = 1 (100 MHz): ratio 18 × 100 = 1800 MHz, exact.
    #[test]
    fn decode_mclk_refclk_100_mhz() {
        assert_eq!(decode_mclk(Some(0x0112)), Section::Value(1800.0));
        // ratio 31 with the 100 MHz ref: 3100.0 MHz (in-band, exact).
        assert_eq!(decode_mclk(Some(0x011F)), Section::Value(3100.0));
    }

    /// A ratio of 0 is unconfigured → `Na(ParseError)`, never a 0 MHz
    /// value; an absent register degrades the same way.
    #[test]
    fn decode_mclk_zero_ratio_and_absent_register() {
        assert!(matches!(
            decode_mclk(Some(0)),
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            decode_mclk(None),
            Section::Na(NaReason::ParseError(_))
        ));
    }

    /// The reserved GEAR_RATIO bits (17:16 — Rocket+ only, Tier 2) do
    /// not affect the v1 mclk decode.
    #[test]
    fn decode_mclk_ignores_reserved_gear_bits_in_v1() {
        assert_eq!(decode_mclk(Some(0x0003_0012)), Section::Value(2400.0));
    }

    /// The [1, 4096] MHz sanity gate bounds the displayed mclk: ratio 31
    /// at the 133.3333 MHz ref (≈ 4133 MHz) / ratio 41 at the 100 MHz
    /// ref (4100 MHz) / ratio 255 (≈ 34000 MHz at the 133 ref; 25500
    /// MHz at the 100 ref) degrade to `Na(ParseError)`; ratio 30 at the
    /// 133 ref (≈ 4000 MHz) and ratio 40 at the 100 ref (4000 MHz)
    /// stay in-band.
    #[test]
    fn decode_mclk_sanity_gate() {
        assert!(matches!(
            decode_mclk(Some(31)),
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            decode_mclk(Some(0x0129)),
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            decode_mclk(Some(0x00FF)),
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(
            decode_mclk(Some(0x01FF)),
            Section::Na(NaReason::ParseError(_))
        ));
        assert!(matches!(decode_mclk(Some(30)), Section::Value(_)));
        assert!(matches!(decode_mclk(Some(0x0128)), Section::Value(_)));
    }

    // -----------------------------------------------------------------
    // The 4-bit tCKE field (decode-only, no display slot) + the 11-bit
    // tRFC bound.
    // -----------------------------------------------------------------

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

    // -----------------------------------------------------------------
    // The turnaround quartet layout (4×6-bit sg / dg / dr / dd).
    // -----------------------------------------------------------------

    #[test]
    fn turnaround_quartet_layout() {
        assert_eq!(
            turnaround_quartet(0x0048_C286),
            [6, 10, 12, 18],
            "sg / dg / dr / dd at bits 5:0 / 11:6 / 17:12 / 23:18"
        );
    }

    // -----------------------------------------------------------------
    // Degradation + containment (the no-panic contract, D5).
    // -----------------------------------------------------------------

    /// Every register absent (all-`None` raws, Tier 1) degrades the
    /// whole channel to honest `Na` — no panic, no garbage values.
    #[test]
    fn decode_all_absent_degrades_without_panic() {
        let ro = decode(&IntelImcRegs::default(), IntelGen::Skylake, None);
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

    /// Per-register containment: only `ch0.tc_rap` unreadable → only
    /// its sourced fields (tRAS / tRRD_S / tRTP / tFAW) degrade to
    /// `Na(ParseError)`; the derived `rc` follows to
    /// `Na(NotApplicable)`; every other register's fields decode
    /// untouched; ch1 (all registers intact) is fully decoded.
    #[test]
    fn decode_per_register_containment() {
        let mut regs = acceptance_regs();
        regs.ch0.tc_rap = None;
        let ro = decode(&regs, IntelGen::Skylake, None);
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

    /// A decoded field of `0` is untrained → `Na(ParseError)` per
    /// field: all-`0` `TC_DBP` / `TC_RAP` / `TC_RFP` registers degrade
    /// every sourced field while the intact registers keep their
    /// values.
    #[test]
    fn decode_zero_fields_degrade_to_parse_error() {
        let mut regs = acceptance_regs();
        regs.ch0.tc_dbp = Some(0);
        regs.ch0.tc_rfp = Some(0);
        regs.ch1.tc_rap = Some(0);
        let ro = decode(&regs, IntelGen::Skylake, None);

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
        // ras (TC_RAP intact on ch0) decodes; rc needs both sources.
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

    /// Any non-Tier-1 generation degrades the whole readout to
    /// `Na(UnsupportedHardware)` channels — the registers are not
    /// decoded at all (never garbage from a mismatched map); the
    /// channel count follows the frozen model.
    #[test]
    fn decode_non_tier1_gen_degrades_all_na() {
        let regs = acceptance_regs();
        for (gen, count) in [
            (IntelGen::IceLake, 2u8),
            (IntelGen::TigerLake, 2),
            (IntelGen::AlderLake, 2),
            (IntelGen::RaptorLake, 2),
            (IntelGen::MeteorLake, 4),
            (IntelGen::ArrowLake, 4),
            (IntelGen::Unrecognized, 2),
        ] {
            let ro = decode(&regs, gen, None);
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
    /// `UnsupportedHardware` with a detail that names the generation
    /// and its tier.
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
        assert!(ald_msg.contains("Tier 2"), "{ald_msg}");
        let met = tier1_gate(IntelGen::MeteorLake).unwrap_err();
        assert!(met.to_string().contains("Tier 3"), "{met}");
        let ice = tier1_gate(IntelGen::IceLake).unwrap_err();
        assert!(ice.to_string().contains("IceLake"), "{ice}");
        assert!(!tier1_supported(IntelGen::Unrecognized));
    }

    // -----------------------------------------------------------------
    // Entry points: `read_regs` total + panic-free on this host.
    // -----------------------------------------------------------------

    /// `read_regs` over all-absent raws is structural and panic-free on
    /// any host (the AMD reference host, virtualized Intel CI runners, a
    /// physical Intel box): every cell is an honest `Na` — never a
    /// bogus `Value`, never a panic (vendor-conditional, per the
    /// TELEMETRY-CI-FIX precedent).
    #[test]
    fn read_regs_on_this_host_is_structural_and_panic_free() {
        let ro = read_regs(&IntelImcRegs::default());
        match CpuInfo::detect().vendor {
            CpuVendor::Intel(gen) => {
                assert_eq!(
                    ro.channels.len(),
                    usize::from(channel_count(gen)),
                    "Intel host: the channel count follows the detected generation"
                );
            }
            _ => {
                assert_eq!(
                    ro.channels.len(),
                    2,
                    "non-Intel host: the degraded readout carries the common two-channel shape"
                );
            }
        }
        for (i, ch) in ro.channels.iter().enumerate() {
            assert_eq!(ch.index, i as u8);
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

    /// The decoded readout (every field type of `IntelChannel`) is
    /// wire-safe: populated, fully degraded, and gen-gated shapes all
    /// round-trip through bincode (the wire contract is unchanged).
    #[test]
    fn decode_readout_bincode_round_trip() {
        for ro in [
            decode(&acceptance_regs(), IntelGen::Skylake, Some(1)),
            decode(&IntelImcRegs::default(), IntelGen::Skylake, None),
            decode(&acceptance_regs(), IntelGen::AlderLake, None),
        ] {
            let bytes = bincode::serialize(&ro)
                .expect("IntelReadout must serialize (no-panic contract)");
            let back: IntelReadout =
                bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
            assert_eq!(ro, back);
        }
    }

    // -----------------------------------------------------------------
    // Legacy compatibility surface — the original fixture tests
    // (they exercise the frozen skeleton core the downstream GUI / TUI /
    // client fixtures build their `IntelReadout`s through).
    // -----------------------------------------------------------------

    /// Synthesize the legacy `MCS_COMMAND_0` from its four 8-bit fields.
    fn cmd0(cl: u32, rcdd: u32, rp: u32, ras: u32) -> u32 {
        (cl & 0xFF) | ((rcdd & 0xFF) << 8) | ((rp & 0xFF) << 16) | ((ras & 0xFF) << 24)
    }

    /// Synthesize the legacy `MCS_COMMAND_1` from its command-rate /
    /// gear / RTL fields.
    fn cmd1(rate: u32, gear: u32, rtl: u32) -> u32 {
        (rate & 0b11) | ((gear & 0b11) << 2) | ((rtl & 0b111) << 4)
    }

    /// Synthesize the legacy `MCS_COMMAND_2` from tCCD_S / tCCD_L.
    fn cmd2(ccd_s: u32, ccd_l: u32) -> u32 {
        (ccd_s & 0xFF) | ((ccd_l & 0xFF) << 8)
    }

    /// Synthesize the legacy `MCS_COMMAND_3` from its four 8-bit
    /// turnaround fields.
    fn cmd3(rdrd: u32, rdwr: u32, wrwr: u32, wrrd: u32) -> u32 {
        (rdrd & 0xFF) | ((rdwr & 0xFF) << 8) | ((wrwr & 0xFF) << 16) | ((wrrd & 0xFF) << 24)
    }

    /// A fully-populated synthetic legacy channel (DDR4-3200 class
    /// values).
    fn full_regs() -> [Option<u32>; 4] {
        [
            Some(cmd0(16, 16, 16, 32)),
            Some(cmd1(0, 0, 6)), // 1N, gear 1, RTL 6
            Some(cmd2(4, 12)),
            Some(cmd3(10, 8, 12, 4)),
        ]
    }

    /// (a) tCL/tRCD/tRP/tRAS decode from their `MCS_COMMAND_0` bit
    /// fields, including the 8-bit maximums.
    #[test]
    fn decode_command0_fields() {
        let reg = cmd0(16, 16, 16, 32);
        assert_eq!(decode_tcl(reg), 16);
        assert_eq!(decode_trcd(reg), 16);
        assert_eq!(decode_trp(reg), 16);
        assert_eq!(decode_tras(reg), 32);

        let reg = cmd0(255, 255, 255, 255);
        assert_eq!(decode_tcl(reg), 255);
        assert_eq!(decode_trcd(reg), 255);
        assert_eq!(decode_trp(reg), 255);
        assert_eq!(decode_tras(reg), 255);
    }

    /// (a) Command rate / gear / RTL decode from their `MCS_COMMAND_1`
    /// bit fields.
    #[test]
    fn decode_command1_fields() {
        // 1N, gear 1, RTL 6.
        let reg = cmd1(0, 0, 6);
        assert_eq!(decode_cmd_rate(reg), Some(DivMode::OneToOne));
        assert_eq!(decode_gear(reg), Some(GearMode::One));
        assert_eq!(decode_rtl(reg), 6);

        // 2N, gear 2, RTL 1 (max 3-bit value below).
        let reg = cmd1(1, 1, 1);
        assert_eq!(decode_cmd_rate(reg), Some(DivMode::OneToTwo));
        assert_eq!(decode_gear(reg), Some(GearMode::Two));
        assert_eq!(decode_rtl(reg), 1);

        // RTL 3-bit maximum.
        assert_eq!(decode_rtl(cmd1(0, 0, 7)), 7);
    }

    /// (b) Reserved encodings decode to `None` (never a bogus mode).
    #[test]
    fn decode_reserved_encodings_are_none() {
        assert_eq!(decode_cmd_rate(cmd1(2, 0, 0)), None);
        assert_eq!(decode_cmd_rate(cmd1(3, 0, 0)), None);
        assert_eq!(decode_gear(cmd1(0, 3, 0)), None);
        assert_eq!(decode_freq_ratio(0), None); // unconfigured ratio
    }

    /// (a) tCCD_S / tCCD_L decode from their `MCS_COMMAND_2` bit fields.
    #[test]
    fn decode_command2_fields() {
        let reg = cmd2(4, 12);
        assert_eq!(decode_tccd_s(reg), 4);
        assert_eq!(decode_tccd_l(reg), 12);
    }

    /// (a) tRDRD/tRDWR/tWRWR/tWRRD decode from their `MCS_COMMAND_3`
    /// bit fields.
    #[test]
    fn decode_command3_fields() {
        let reg = cmd3(10, 8, 12, 4);
        assert_eq!(decode_trdrd(reg), 10);
        assert_eq!(decode_trdwr(reg), 8);
        assert_eq!(decode_twrwr(reg), 12);
        assert_eq!(decode_twrrd(reg), 4);
    }

    /// (a) The frequency ratio decodes to the 10 MHz units (code 0 is
    /// unconfigured → `None`).
    #[test]
    fn decode_freq_ratio_to_mhz() {
        assert_eq!(decode_freq_ratio(0), None);
        assert_eq!(decode_freq_ratio(133), Some(133));
        assert_eq!(decode_freq_ratio(160), Some(160));
        assert_eq!(decode_freq_ratio(0xFF), Some(255));

        // 160 × 10 MHz = 1600 MHz (DDR4-3200).
        let ratio = match decode_freq_ratio(160) {
            Some(r) => r,
            None => panic!("ratio 160 must decode"),
        };
        assert_eq!(f64::from(ratio) * FREQ_RATIO_STEP_MHZ, 1600.0);
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
            IntelGen::AlderLake,
            IntelGen::RaptorLake,
            IntelGen::Unrecognized,
        ];
        for g in two {
            assert_eq!(channel_count(g), 2, "{g:?}");
        }
        for g in [IntelGen::MeteorLake, IntelGen::ArrowLake] {
            assert_eq!(channel_count(g), 4, "{g:?}");
        }
    }

    /// The per-channel legacy offset table is 4-aligned, fits the MCHBAR
    /// window, and the per-channel blocks never overlap.
    #[test]
    fn channel_offsets_within_window() {
        for ch in 0u8..4 {
            let offs = channel_offsets(ch);
            for off in offs {
                assert_eq!(off % 4, 0, "offset {off:#x} must be 4-aligned");
                assert!(
                    off + 4 <= MCHBAR_WINDOW,
                    "offset {off:#x} must fit the MCHBAR window"
                );
            }
        }
        assert_eq!(channel_offsets(0)[0], 0x5400);
        assert_eq!(channel_offsets(3)[3], 0x570C);

        let o0 = channel_offsets(0).to_vec();
        let o1 = channel_offsets(1).to_vec();
        for a in &o0 {
            assert!(!o1.contains(a), "channel blocks must not overlap: {a:#x}");
        }
    }

    /// The `/dev/mem` fallback map size is the 64 KiB (0x10000) Tier-1
    /// datasheet window, and the highest Tier-1 register reads
    /// (`MC_BIOS_REQ` @ `0x5E00`, a 4-byte access ending at `0x5E04`,
    /// and ch1's `TC_WRWR`) both sit inside it.
    #[test]
    fn devmem_fallback_map_is_64_kib() {
        assert_eq!(MCHBAR_WINDOW, 0x10000, "Tier-1 MCHBAR window is 64 KiB");
        for off in [MAX_IMC_OFFSET, MAX_CHANNEL_OFFSET] {
            assert!(off <= MCHBAR_WINDOW, "offset {off:#x} must fit the 64 KiB window");
        }
    }

    /// (e) The pure gate: Intel passes with its generation; AMD /
    /// unknown yield `UnsupportedHardware` (the `read_intel`
    /// short-circuit on any non-Intel host, no register access).
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

    /// (e) The live gate behavior is vendor-consistent: on a non-Intel
    /// host (the AMD reference host) it rejects with
    /// `UnsupportedHardware` before any register could be read, and on
    /// an Intel host it passes with the detected generation
    /// (`intel_gen_gate(Intel(_)) -> Ok`). Vendor-neutral, so the test
    /// passes on both the AMD reference host and the Intel CI runners
    /// (CI portability).
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

    /// (a) + (d) A fully-populated synthetic legacy channel: every
    /// decoded field is a `Value` with the expected value; CAD bus,
    /// voltages, fclk/GDM/PDM, and the unexposed timing slots are all
    /// `Na(NotApplicable)`; tRC is the derived `tRAS + tRP`.
    #[test]
    fn decode_channel_full_registers() {
        let ch = decode_channel(0, Some(160), full_regs());
        assert_eq!(ch.index, 0);

        // clocks: mclk 1600 MHz, uclk = mclk / gear(1), 1N, gear 1.
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.clocks.div_mode, Section::Value(DivMode::OneToOne));
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::One));
        assert_eq!(ch.clocks.fclk_mhz, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.gdm, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.pdm, Section::na(NaReason::NotApplicable));
        assert_eq!(ch.clocks.command_rate, Section::na(NaReason::NotApplicable));

        // RTL (channel-level field).
        assert_eq!(ch.rtl, Section::Value(6));

        // decoded timing slots (ticks).
        assert_eq!(ch.timings.cl, Section::Value(16));
        assert_eq!(ch.timings.rcdrd, Section::Value(16));
        assert_eq!(ch.timings.rp, Section::Value(16));
        assert_eq!(ch.timings.ras, Section::Value(32));
        assert_eq!(ch.timings.rc, Section::Value(48)); // 32 + 16 (derived)
        assert_eq!(ch.timings.rrds, Section::Value(4)); // tCCD_S slot
        assert_eq!(ch.timings.rrld, Section::Value(12)); // tCCD_L slot
        assert_eq!(ch.timings.rdwr, Section::Value(8));
        assert_eq!(ch.timings.wrrd, Section::Value(4));
        assert_eq!(ch.timings.rdrd_sd, Section::Value(10));
        assert_eq!(ch.timings.wrwr_sd, Section::Value(12));

        // (d) unexposed timing slots → Na(NotApplicable).
        for s in [
            &ch.timings.rcwdwr,
            &ch.timings.faw,
            &ch.timings.wtrs,
            &ch.timings.wtrl,
            &ch.timings.wr,
            &ch.timings.rfc1,
            &ch.timings.rfc2,
            &ch.timings.rfcsb,
            &ch.timings.cwl,
            &ch.timings.rtp,
            &ch.timings.rdrd_dd,
            &ch.timings.rdrd_scl,
            &ch.timings.rdrd_sc,
            &ch.timings.wrwr_dd,
            &ch.timings.wrwr_scl,
            &ch.timings.wrwr_sc,
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }

        // (d) CAD bus (5 ohm fields + 3 RTT fields) → Na(NotApplicable).
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

        // (d) voltages (5 rails, incl. Vcore — C12) → Na(NotApplicable).
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

    /// (a) Gear divides mclk into uclk (gear 4 → 1600 MHz / 4 = 400
    /// MHz) and 2N maps to [`DivMode::OneToTwo`].
    #[test]
    fn decode_channel_gear_divides_uclk() {
        let regs = [
            Some(cmd0(16, 16, 16, 32)),
            Some(cmd1(1, 2, 0)), // 2N, gear 4
            Some(cmd2(4, 12)),
            Some(cmd3(10, 8, 12, 4)),
        ];
        let ch = decode_channel(1, Some(160), regs);
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(400.0));
        assert_eq!(ch.clocks.div_mode, Section::Value(DivMode::OneToTwo));
        assert_eq!(ch.clocks.gear_mode, Section::Value(GearMode::Four));
    }

    /// (b) A failed register read degrades only the fields sourced from
    /// that register; everything else still decodes to `Value`.
    #[test]
    fn decode_channel_read_failure_degrades_only_sourced_fields() {
        let ch = decode_channel(2, Some(160), [
            None, // COMMAND_0 read failed
            Some(cmd1(0, 0, 6)),
            None, // COMMAND_2 read failed
            Some(cmd3(10, 8, 12, 4)),
        ]);

        // sourced from the failed reads → Na(ParseError)
        for s in [
            &ch.timings.cl,
            &ch.timings.rcdrd,
            &ch.timings.rp,
            &ch.timings.ras,
            &ch.timings.rrds,
            &ch.timings.rrld,
        ] {
            assert!(matches!(s, Section::Na(NaReason::ParseError(_))), "{s:?}");
        }
        // derived from a failed source → Na(NotApplicable)
        assert!(matches!(ch.timings.rc, Section::Na(NaReason::NotApplicable)));
        // decoded registers stay Value
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.clocks.uclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.rtl, Section::Value(6));
        assert_eq!(ch.timings.rdwr, Section::Value(8));
        assert_eq!(ch.timings.wrrd, Section::Value(4));
        assert_eq!(ch.timings.rdrd_sd, Section::Value(10));
        assert_eq!(ch.timings.wrwr_sd, Section::Value(12));
    }

    /// (e) + plan "guard-disabled fixture": every register absent (and
    /// the global ratio absent) degrades the whole legacy channel to
    /// `Na` sections — no panic, no garbage values.
    #[test]
    fn decode_channel_all_absent_degrades_without_panic() {
        let ch = decode_channel(3, None, [None; 4]);

        fn is_na<T: std::fmt::Debug>(s: &Section<T>) {
            assert!(s.is_na(), "{s:?}");
        }
        is_na(&ch.clocks.mclk_mhz);
        is_na(&ch.clocks.uclk_mhz);
        is_na(&ch.clocks.fclk_mhz);
        is_na(&ch.clocks.div_mode);
        is_na(&ch.clocks.gear_mode);
        is_na(&ch.clocks.gdm);
        is_na(&ch.clocks.pdm);
        is_na(&ch.clocks.command_rate);
        let timing_secs: [&Section<u16>; 27] = [
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
        for s in &timing_secs {
            assert!(s.is_na(), "{s:?}");
        }
        assert!(ch.rtl.is_na());
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

    /// (b) A decoded tick of `0` (tCL, and RTL) is not a valid trained
    /// value → `Na(ParseError)`; the same register's other fields still
    /// decode.
    #[test]
    fn zero_tick_fields_degrade_to_na() {
        let ch = decode_channel(0, Some(160), [
            Some(cmd0(0, 16, 16, 32)), // tCL = 0
            Some(cmd1(0, 0, 0)), // RTL = 0
            Some(cmd2(4, 12)),
            Some(cmd3(10, 8, 12, 4)),
        ]);
        assert!(matches!(ch.timings.cl, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(ch.rtl, Section::Na(NaReason::ParseError(_))));
        assert_eq!(ch.timings.rcdrd, Section::Value(16));
    }

    /// (b) A reserved gear encoding degrades `gear_mode` and (because
    /// it is the divisor) `uclk` to `Na(ParseError)`, while the
    /// remaining decoded fields stay `Value`.
    #[test]
    fn reserved_gear_degrades_gear_and_uclk() {
        let ch = decode_channel(0, Some(160), [
            Some(cmd0(16, 16, 16, 32)),
            Some(cmd1(0, 3, 6)), // gear = 0b11 (reserved)
            Some(cmd2(4, 12)),
            Some(cmd3(10, 8, 12, 4)),
        ]);
        assert!(matches!(ch.clocks.gear_mode, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(ch.clocks.uclk_mhz, Section::Na(NaReason::ParseError(_))));
        assert_eq!(ch.clocks.mclk_mhz, Section::Value(1600.0));
        assert_eq!(ch.clocks.div_mode, Section::Value(DivMode::OneToOne));
        assert_eq!(ch.rtl, Section::Value(6));
    }

    /// The clock sanity gate bounds the displayed value range.
    #[test]
    fn clock_section_sanity_gate() {
        assert_eq!(clock_section(1600.0), Section::Value(1600.0));
        assert_eq!(clock_section(1.0), Section::Value(1.0));
        assert_eq!(clock_section(4096.0), Section::Value(4096.0));
        for mhz in [0.0, 0.5, 4096.1, 65535.0] {
            assert!(matches!(clock_section(mhz), Section::Na(NaReason::ParseError(_))), "{mhz}");
        }
    }

    /// The frozen readout types are `Clone` + `Debug` + `PartialEq`,
    /// the legacy decode is deterministic, and an `IntelReadout` holds
    /// the channel set.
    #[test]
    fn readout_and_channel_traits() {
        fn bound<T: Clone + std::fmt::Debug + PartialEq>() {}
        bound::<IntelChannel>();
        bound::<IntelReadout>();

        let a = decode_channel(0, Some(160), full_regs());
        let b = a.clone();
        assert_eq!(a, b);
        assert!(!format!("{a:?}").is_empty());
        // deterministic: decoding the same registers twice is equal
        assert_eq!(a, decode_channel(0, Some(160), full_regs()));

        let ro = IntelReadout {
            channels: vec![a, b],
            channel_mode: None,
        };
        assert_eq!(ro.channels.len(), 2);
        assert_eq!(ro.channels[0].index, 0);
        assert_eq!(ro.channels[1].index, 0);
    }

    /// (f) P3-04: a multi-channel `IntelReadout` — a fully populated
    /// legacy channel plus an all-`Na` degradation channel — round-
    /// trips through bincode, proving the Intel readout (every field
    /// type of `IntelChannel`) is wire-safe.
    #[test]
    fn intel_readout_bincode_round_trip() {
        let ro = IntelReadout {
            channels: vec![
                decode_channel(0, Some(160), full_regs()),
                decode_channel(1, None, [None; 4]), // all-Na channel
            ],
            channel_mode: Some(ChannelMode::DualFlex),
        };

        let bytes = bincode::serialize(&ro)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
        assert_eq!(ro, back);

        // the fully degraded readout (every channel all-Na) is wire-safe
        // too
        let all_na = IntelReadout {
            channels: vec![
                decode_channel(0, None, [None; 4]),
                decode_channel(1, None, [None; 4]),
            ],
            channel_mode: None,
        };
        let bytes = bincode::serialize(&all_na)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
        assert_eq!(all_na, back);
    }

    // -----------------------------------------------------------------
    // ChannelMode (MAD_INTER_CHANNEL[1:0]).
    // -----------------------------------------------------------------

    /// `from_raw` maps the 2-bit CHAN_MODE field to its variants; the
    /// reserved encoding `11` degrades to `None`; high bits are masked.
    #[test]
    fn channel_mode_from_raw_maps_all_encodings() {
        assert_eq!(ChannelMode::from_raw(0x0), Some(ChannelMode::DualSymmetric));
        assert_eq!(ChannelMode::from_raw(0x1), Some(ChannelMode::DualFlex));
        assert_eq!(ChannelMode::from_raw(0x2), Some(ChannelMode::Single));
        assert_eq!(ChannelMode::from_raw(0x3), None, "11 is reserved");
        // High bits must be masked away (only bits [1:0] are read).
        assert_eq!(
            ChannelMode::from_raw(0xFFFF_FFFE),
            Some(ChannelMode::Single),
            "bits [1:0] only"
        );
        assert_eq!(
            ChannelMode::from_raw(0xFFFF_FFFF),
            None,
            "reserved, even with high bits set"
        );
    }

    /// The full channel label + the short "Mode:" label.
    #[test]
    fn channel_mode_labels() {
        assert_eq!(ChannelMode::DualSymmetric.label(), "Dual-Channel (Symmetric)");
        assert_eq!(ChannelMode::DualFlex.label(), "Dual-Channel (Flex)");
        assert_eq!(ChannelMode::Single.label(), "Single-Channel");

        assert_eq!(ChannelMode::DualSymmetric.mode_label(), "Interleaved");
        assert_eq!(ChannelMode::DualFlex.mode_label(), "Flex");
        assert_eq!(ChannelMode::Single.mode_label(), "N/A");
    }
}
