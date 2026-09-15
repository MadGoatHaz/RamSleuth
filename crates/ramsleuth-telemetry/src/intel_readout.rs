//! Intel per-channel IMC register decode into the shared display types (P2-07).
//!
//! This module is the readout half of the Intel telemetry branch: given the
//! read-only [`MchBar`] mapping acquired by P2-06, it decodes the memory
//! controller (IMC) register block into the frozen vendor-neutral display
//! types shared with the AMD readout (`ClockReadout`, `TimingSet`, `CadBus`,
//! `VoltageSet` — plan D3).
//!
//! # Channel model
//!
//! The plan (P2-07 / §1.2) specifies per-channel decode over channels `0–3`.
//! The channel count is a fixed function of the detected [`IntelGen`]
//! ([`channel_count`]): DDR4-class generations (Skylake through Raptor Lake)
//! expose 2 channels; DDR5-class client generations (Meteor Lake, Arrow
//! Lake) expose the full 4-channel plan range; `Unrecognized` falls back to
//! the common 2-channel desktop layout.
//!
//! # IMC register table (const — every read within the 1 MiB MCHBAR window)
//!
//! One **global** register plus a **per-channel MCS block**:
//!
//! | Offset (MCHBAR-relative) | Register | Fields decoded |
//! |---|---|---|
//! | `0x5058` | `IMC_FREQ_RATIO` | bits 7:0 — DRAM clock ratio, in 10 MHz units |
//! | `0x5400 + ch·0x100 + 0x000` | `MCS_CH{ch}_COMMAND_0` | tCL[7:0], tRCD[15:8], tRP[23:16], tRAS[31:24] (ticks) |
//! | `0x5400 + ch·0x100 + 0x004` | `MCS_CH{ch}_COMMAND_1` | command rate[1:0] (0=1N, 1=2N), gear[3:2] (0=1×, 1=2×, 2=4×), RTL[6:4] (ticks) |
//! | `0x5400 + ch·0x100 + 0x008` | `MCS_CH{ch}_COMMAND_2` | tCCD_S[7:0], tCCD_L[15:8] (ticks) |
//! | `0x5400 + ch·0x100 + 0x00C` | `MCS_CH{ch}_COMMAND_3` | tRDRD[7:0], tRDWR[15:8], tWRWR[23:16], tWRRD[31:24] (ticks) |
//!
//! The highest per-channel offset is `0x570C`; every access is a 4-byte
//! read fully inside the 1 MiB window mapped by P2-06. This is proven at
//! compile time (`_ASSERT_IMC_READS_IN_WINDOW`) and re-checked at runtime by
//! [`MchBar::read_u32`] (out-of-bounds → `TelemetryError::Parse`).
//!
//! **Status of the table:** like the P2-04 AMD PM layout, the byte offsets
//! and bit positions are a *documented model* of the published client-IMC
//! register map, to be reconciled against real Intel silicon at P2-11/QA —
//! they cannot be live-verified on this AMD reference host.
//!
//! # Mapping into the frozen display types
//!
//! - `mclk_mhz` ← `IMC_FREQ_RATIO` (ratio × 10 MHz, sanity-gated to
//!   [1, 4096] MHz).
//! - `uclk_mhz` ← `mclk / gear` (gear is the SA:MEM multiplier).
//! - `fclk_mhz`, `gdm`, `pdm` ← `Na(NotApplicable)` — AMD-fabric concepts
//!   with no IMC analog.
//! - `div_mode` ← command rate (1N → `DivMode::OneToOne`, 2N →
//!   `DivMode::OneToTwo`); a reserved encoding or failed read →
//!   `Na(ParseError)`.
//! - `command_rate` → `Na(NotApplicable)` — the IMC command rate (1N/2N)
//!   is already surfaced as `div_mode` above (P2-07); a separate slot
//!   would redundantly re-expose the same bits (D-C11).
//! - `gear_mode` ← gear (1/2/4 → `GearMode::One`/`Two`/`Four`); a reserved
//!   encoding or failed read → `Na(ParseError)`.
//! - tCL/tRCD/tRP/tRAS/RTL/tCCD_S/tCCD_L/tRDRD/tRDWR/tWRWR/tWRRD map into
//!   [`TimingSet`] slots in ticks (sanity-gated to [1, 2048]; a zero
//!   reading or failed register read → `Na(ParseError)`):
//!   - tCL → `cl`, tRCD → `rcdrd` (the modeled table carries a single RCD;
//!     `rcwdwr` → `Na(NotApplicable)`), tRP → `rp`, tRAS → `ras`.
//!   - tCCD_S/tCCD_L → `rrds`/`rrld`: the nearest shared slots for the CCD
//!     pair (the dashboard's secondary row), mapped as documented.
//!   - tRDRD → `rdrd_sd`, tWRWR → `wrwr_sd` (same-DIMM variants; the
//!     per-bank-group / SCL / SC variant slots → `Na(NotApplicable)`),
//!     tRDWR → `rdwr`, tWRRD → `wrrd`.
//!   - tRC is derived: `tRAS + tRP` when both decoded, else
//!     `Na(NotApplicable)` (the JEDEC identity).
//!   - Fields the modeled IMC table does not expose (`faw`, `wtrs`, `wtrl`,
//!     `wr`, `rfc1`, `rfc2`, `rfcsb`, `cwl`, `rtp`) → `Na(NotApplicable)`.
//! - **RTL has no slot in the frozen `TimingSet`**, so it is carried as the
//!   channel-level [`IntelChannel::rtl`] field (ticks).
//! - **CAD bus and voltages** → every field `Na(NotApplicable)`: the Intel
//!   MCHBAR window does not expose drive strengths or rails the way the AMD
//!   SMU does (plan §1.2: out of Phase 2 live-readout scope).
//!
//! # No-panic contract (D5)
//!
//! [`read_intel`] performs no I/O beyond the bounds-checked
//! [`MchBar::read_u32`]. The vendor/generation gate runs first and returns a
//! structured `TelemetryError::UnsupportedHardware` on non-Intel hardware
//! (including this AMD reference host) before any register is touched. A
//! failed register read degrades only the fields sourced from it to
//! `Na(ParseError)`; the rest of the channel decodes normally. No `unsafe`
//! is added here — the MMIO read primitive lives in P2-06.
//!
//! The full `read_intel` path needs a real (Intel-mapped) `MchBar` and is
//! not unit-tested on this host; instead the gate ([`intel_gen_gate`]), the
//! channel-count model ([`channel_count`]), and the pure decode core
//! ([`decode_channel`]) are fixture-tested below.

use crate::amd_readout::{CadBus, ClockReadout, DivMode, GearMode, TimingSet, VoltageSet};
use crate::cpuid::{CpuInfo, CpuVendor, IntelGen};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_mchbar::MchBar;

// ---------------------------------------------------------------------------
// IMC register table (const; see the module docs for the layout rationale).
// ---------------------------------------------------------------------------

/// Size of the MCHBAR MMIO window mapped by P2-06 (1 MiB). Mirrors
/// `intel_mchbar::MCHBAR_WINDOW_SIZE` (private there) so the table below can
/// be compile-checked against the actual mapped region.
pub const MCHBAR_WINDOW: usize = 1 << 20;

/// Global register: DRAM frequency ratio (bits 7:0, in 10 MHz units).
const IMC_FREQ_RATIO_OFFSET: usize = 0x5058;

/// Per-channel MCS command-block base; channel `n` sits at
/// `MCS_CHANNEL_BASE + n * MCS_CHANNEL_STRIDE`.
const MCS_CHANNEL_BASE: usize = 0x5400;
const MCS_CHANNEL_STRIDE: usize = 0x100;

/// Offsets within a channel's MCS block (4-byte registers, in decode order).
const MCS_COMMAND_0: usize = 0x000; // tCL / tRCD / tRP / tRAS
const MCS_COMMAND_1: usize = 0x004; // command rate / gear / RTL
const MCS_COMMAND_2: usize = 0x008; // tCCD_S / tCCD_L
const MCS_COMMAND_3: usize = 0x00C; // tRDRD / tRDWR / tWRWR / tWRRD

/// Highest per-channel offset: channel 3's last command register.
pub const MAX_CHANNEL_OFFSET: usize =
    MCS_CHANNEL_BASE + 3 * MCS_CHANNEL_STRIDE + MCS_COMMAND_3;

// Compile-time proof that every IMC read in this module fits the 1 MiB
// MCHBAR window (each read is a 4-byte access at the offsets above).
const _ASSERT_IMC_READS_IN_WINDOW: () = assert!(
    IMC_FREQ_RATIO_OFFSET + 4 <= MCHBAR_WINDOW
        && MAX_CHANNEL_OFFSET + 4 <= MCHBAR_WINDOW
);

// ---------------------------------------------------------------------------
// Sanity ranges (mirroring the AMD display gates: the same plausible bounds
// apply to the vendor-neutral display types, plan D3).
// ---------------------------------------------------------------------------

/// Plausible DRAM subtiming range in ticks.
const TIMING_MIN_TICKS: u16 = 1;
const TIMING_MAX_TICKS: u16 = 2048;

/// Plausible memory/fabric clock range in MHz.
const CLOCK_MIN_MHZ: f64 = 1.0;
const CLOCK_MAX_MHZ: f64 = 4096.0;

/// Step between `IMC_FREQ_RATIO` codes (ratio × 10 MHz).
const FREQ_RATIO_STEP_MHZ: f64 = 10.0;

// ---------------------------------------------------------------------------
// Frozen public API (P2-07 interface freeze — the P2-10 facade consumes it).
// ---------------------------------------------------------------------------

/// One decoded memory channel of an Intel platform.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelChannel {
    /// 0-based channel index (`index < channel_count(gen)`).
    pub index: u8,
    /// Clocks and ratios: mclk from `IMC_FREQ_RATIO`, uclk = mclk / gear.
    pub clocks: ClockReadout,
    /// DRAM subtimings in ticks.
    pub timings: TimingSet,
    /// CAD bus: every field `Na(NotApplicable)` on Intel MCHBAR.
    pub cad_bus: CadBus,
    /// Memory rails: every field `Na(NotApplicable)` on Intel MCHBAR.
    pub voltages: VoltageSet,
    /// RTL (ticks). Carried at channel level: the frozen `TimingSet` has no
    /// slot for it (see the module docs).
    pub rtl: Section<u16>,
}

/// The full Intel readout: one [`IntelChannel`] per detected channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntelReadout {
    /// Decoded channels (0-based indices, `channels.len() == channel_count`).
    pub channels: Vec<IntelChannel>,
}

/// Read and decode every IMC channel from a live, read-only [`MchBar`] map.
///
/// Sequence (all no-panic; D5):
/// 1. [`CpuInfo::detect()`] → [`intel_gen_gate`]: non-Intel hardware
///    (including this AMD reference host) returns
///    [`TelemetryError::UnsupportedHardware`] before any register is read.
/// 2. [`channel_count`] fixes the per-channel range (plan: 0–3).
/// 3. The global `IMC_FREQ_RATIO` register is read once; a failed read
///    degrades the mclk-derived fields in every channel to `Na(ParseError)`.
/// 4. Each channel's four MCS command registers are read and decoded by the
///    pure [`decode_channel`]; a failed read degrades only the fields
///    sourced from that register.
///
/// No I/O happens beyond the bounds-checked [`MchBar::read_u32`].
pub fn read_intel(bar: &MchBar) -> TelemetryResult<IntelReadout> {
    let info = CpuInfo::detect();
    let gen = intel_gen_gate(&info)?;
    let count = channel_count(gen);

    let mclk_reg = bar.read_u32(IMC_FREQ_RATIO_OFFSET).ok();

    let mut channels = Vec::with_capacity(usize::from(count));
    for ch in 0..count {
        channels.push(read_channel(bar, ch, mclk_reg));
    }
    Ok(IntelReadout { channels })
}

/// The pure vendor/generation gate for the Intel branch: Intel passes with
/// its [`IntelGen`]; AMD and unknown vendors yield
/// [`TelemetryError::UnsupportedHardware`] (no register access is possible
/// or attempted once this returns `Err`).
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

/// The fixed channel-count model for an [`IntelGen`] (plan: per-channel
/// 0–3). DDR4-class generations expose 2 channels; DDR5-class client
/// generations expose 4; an unrecognized family-6 Intel falls back to the
/// common 2-channel desktop layout.
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

/// The four per-channel MCS command-register offsets for channel `ch`, in
/// decode order (`COMMAND_0..3`). All values are 4-byte aligned and within
/// the 1 MiB window (compile-asserted; re-checked by `MchBar::read_u32`).
pub fn channel_offsets(ch: u8) -> [usize; 4] {
    let base = MCS_CHANNEL_BASE + usize::from(ch) * MCS_CHANNEL_STRIDE;
    [
        base + MCS_COMMAND_0,
        base + MCS_COMMAND_1,
        base + MCS_COMMAND_2,
        base + MCS_COMMAND_3,
    ]
}

/// Decode one channel from raw register values (the pure core of
/// [`read_intel`]).
///
/// `mclk_reg` is the global `IMC_FREQ_RATIO` read; `regs` holds the
/// channel's four MCS command registers in [`channel_offsets`] order. Each
/// is `None` when the underlying `MchBar::read_u32` failed; the fields
/// sourced from a failed register degrade to `Na(ParseError)` while the rest
/// of the channel decodes normally. Never panics (D5).
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

/// Read one channel's four MCS command registers from the live map, then
/// decode them. The only `MchBar` touch point in this module.
fn read_channel(bar: &MchBar, ch: u8, mclk_reg: Option<u32>) -> IntelChannel {
    let offsets = channel_offsets(ch);
    let mut regs = [None; 4];
    for (slot, &off) in regs.iter_mut().zip(offsets.iter()) {
        *slot = bar.read_u32(off).ok();
    }
    decode_channel(ch, mclk_reg, regs)
}

// ---------------------------------------------------------------------------
// Pure bit-field decoders (unit-tested with synthetic register values; no
// I/O, no unsafe — the mapping lives in P2-06's `MchBar`).
// ---------------------------------------------------------------------------

/// Extract a register field that is already masked to ≤ 255 as a `u16`.
///
/// The caller's mask makes the conversion lossless, so `try_from` always
/// succeeds; the `unwrap_or` fallback is an unreachable no-panic guard in
/// the same style as P2-06's constant fallback.
fn field_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// tCL — `MCS_COMMAND_0` bits 7:0 (ticks).
pub fn decode_tcl(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// tRCD — `MCS_COMMAND_0` bits 15:8 (ticks).
pub fn decode_trcd(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// tRP — `MCS_COMMAND_0` bits 23:16 (ticks).
pub fn decode_trp(reg: u32) -> u16 {
    field_u16((reg >> 16) & 0xFF)
}

/// tRAS — `MCS_COMMAND_0` bits 31:24 (ticks).
pub fn decode_tras(reg: u32) -> u16 {
    field_u16((reg >> 24) & 0xFF)
}

/// Command rate — `MCS_COMMAND_1` bits 1:0: `0` = 1N, `1` = 2N; `2`/`3` are
/// reserved encodings → `None`.
pub fn decode_cmd_rate(reg: u32) -> Option<DivMode> {
    match reg & 0b11 {
        0 => Some(DivMode::OneToOne),
        1 => Some(DivMode::OneToTwo),
        _ => None,
    }
}

/// Gear mode — `MCS_COMMAND_1` bits 3:2: `0` = 1×, `1` = 2×, `2` = 4×;
/// `3` is reserved → `None`.
pub fn decode_gear(reg: u32) -> Option<GearMode> {
    match (reg >> 2) & 0b11 {
        0 => Some(GearMode::One),
        1 => Some(GearMode::Two),
        2 => Some(GearMode::Four),
        _ => None,
    }
}

/// RTL — `MCS_COMMAND_1` bits 6:4 (ticks).
pub fn decode_rtl(reg: u32) -> u16 {
    field_u16((reg >> 4) & 0b111)
}

/// tCCD_S (short) — `MCS_COMMAND_2` bits 7:0 (ticks).
pub fn decode_tccd_s(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// tCCD_L (long) — `MCS_COMMAND_2` bits 15:8 (ticks).
pub fn decode_tccd_l(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// tRDRD — `MCS_COMMAND_3` bits 7:0 (ticks).
pub fn decode_trdrd(reg: u32) -> u16 {
    field_u16(reg & 0xFF)
}

/// tRDWR — `MCS_COMMAND_3` bits 15:8 (ticks).
pub fn decode_trdwr(reg: u32) -> u16 {
    field_u16((reg >> 8) & 0xFF)
}

/// tWRWR — `MCS_COMMAND_3` bits 23:16 (ticks).
pub fn decode_twrwr(reg: u32) -> u16 {
    field_u16((reg >> 16) & 0xFF)
}

/// tWRRD — `MCS_COMMAND_3` bits 31:24 (ticks).
pub fn decode_twrrd(reg: u32) -> u16 {
    field_u16((reg >> 24) & 0xFF)
}

/// DRAM frequency ratio — `IMC_FREQ_RATIO` bits 7:0 (in 10 MHz units); a
/// code of `0` means unconfigured → `None`.
pub fn decode_freq_ratio(reg: u32) -> Option<u16> {
    let ratio = field_u16(reg & 0xFF);
    (ratio != 0).then_some(ratio)
}

/// The SA:MEM divisor for a decoded [`GearMode`] (`uclk = mclk / gear`).
fn gear_divisor(gear: GearMode) -> f64 {
    match gear {
        GearMode::One => 1.0,
        GearMode::Two => 2.0,
        GearMode::Four => 4.0,
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

/// Decode one tick field from an optionally-failed register read: a failed
/// read → `Na(ParseError)`; a decoded value outside [1, 2048] (including
/// `0`) → `Na(ParseError)`; otherwise the tick value.
fn timing_section(reg: Option<u32>, name: &str, f: fn(u32) -> u16) -> Section<u16> {
    let Some(reg) = reg else {
        return Section::na(NaReason::ParseError(format!("{name}: register read failed")));
    };
    ticks_section(f(reg), name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::AmdZen;

    /// Synthesize `MCS_COMMAND_0` from its four 8-bit fields.
    fn cmd0(cl: u32, rcdd: u32, rp: u32, ras: u32) -> u32 {
        (cl & 0xFF) | ((rcdd & 0xFF) << 8) | ((rp & 0xFF) << 16) | ((ras & 0xFF) << 24)
    }

    /// Synthesize `MCS_COMMAND_1` from its command-rate / gear / RTL fields.
    fn cmd1(rate: u32, gear: u32, rtl: u32) -> u32 {
        (rate & 0b11) | ((gear & 0b11) << 2) | ((rtl & 0b111) << 4)
    }

    /// Synthesize `MCS_COMMAND_2` from tCCD_S / tCCD_L.
    fn cmd2(ccd_s: u32, ccd_l: u32) -> u32 {
        (ccd_s & 0xFF) | ((ccd_l & 0xFF) << 8)
    }

    /// Synthesize `MCS_COMMAND_3` from its four 8-bit turnaround fields.
    fn cmd3(rdrd: u32, rdwr: u32, wrwr: u32, wrrd: u32) -> u32 {
        (rdrd & 0xFF) | ((rdwr & 0xFF) << 8) | ((wrwr & 0xFF) << 16) | ((wrrd & 0xFF) << 24)
    }

    /// A fully-populated synthetic channel (DDR4-3200 class values).
    fn full_regs() -> [Option<u32>; 4] {
        [
            Some(cmd0(16, 16, 16, 32)),
            Some(cmd1(0, 0, 6)), // 1N, gear 1, RTL 6
            Some(cmd2(4, 12)),
            Some(cmd3(10, 8, 12, 4)),
        ]
    }

    /// (a) tCL/tRCD/tRP/tRAS decode from their `MCS_COMMAND_0` bit fields,
    /// including the 8-bit maximums.
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

    /// (a) Command rate / gear / RTL decode from their `MCS_COMMAND_1` bit
    /// fields.
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

    /// (a) tRDRD/tRDWR/tWRWR/tWRRD decode from their `MCS_COMMAND_3` bit
    /// fields.
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
        let Some(ratio) = decode_freq_ratio(160) else {
            panic!("ratio 160 must decode");
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

    /// The per-channel offset table is 4-aligned, fits the 1 MiB window,
    /// and the four per-channel blocks never overlap.
    #[test]
    fn channel_offsets_within_window() {
        for ch in 0u8..4 {
            let offs = channel_offsets(ch);
            for off in offs {
                assert_eq!(off % 4, 0, "offset {off:#x} must be 4-aligned");
                assert!(
                    off + 4 <= MCHBAR_WINDOW,
                    "offset {off:#x} must fit the 1 MiB window"
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

    /// (e) The pure gate: Intel passes with its generation; AMD / unknown
    /// yield `UnsupportedHardware` (the `read_intel` short-circuit on any
    /// non-Intel host, no register access).
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

    /// (e) On this fixed AMD reference host (plan test-host reality) the
    /// gate rejects before any register could be read.
    #[test]
    fn intel_gen_gate_short_circuits_on_this_amd_host() {
        let detected = CpuInfo::detect();
        assert_eq!(detected.vendor, CpuVendor::Amd(AmdZen::Zen3));
        assert!(matches!(
            intel_gen_gate(&detected),
            Err(TelemetryError::UnsupportedHardware { .. })
        ));
    }

    /// (a) + (d) A fully-populated synthetic channel: every decoded field is
    /// a `Value` with the expected value; CAD bus, voltages, fclk/GDM/PDM,
    /// and the unexposed timing slots are all `Na(NotApplicable)`; tRC is the
    /// derived `tRAS + tRP`.
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

        // (d) voltages (4 rails) → Na(NotApplicable).
        for s in [
            &ch.voltages.vddcr_soc_mv,
            &ch.voltages.vddio_mem_mv,
            &ch.voltages.vdd_misc_mv,
            &ch.voltages.vpp_mv,
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }
    }

    /// (a) Gear divides mclk into uclk (gear 4 → 1600 MHz / 4 = 400 MHz)
    /// and 2N maps to [`DivMode::OneToTwo`].
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

    /// (e) + plan "guard-disabled fixture": every register absent (and the
    /// global ratio absent) degrades the whole channel to `Na` sections —
    /// no panic, no garbage values.
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
        ] {
            assert!(matches!(s, Section::Na(NaReason::NotApplicable)), "{s:?}");
        }
    }

    /// (b) A decoded tick of `0` (tCL, and RTL) is not a valid trained value
    /// → `Na(ParseError)`; the same register's other fields still decode.
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

    /// (b) A reserved gear encoding degrades `gear_mode` and (because it is
    /// the divisor) `uclk` to `Na(ParseError)`, while the remaining decoded
    /// fields stay `Value`.
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

    /// The frozen readout types are `Clone` + `Debug` + `PartialEq`, the
    /// decode is deterministic, and an `IntelReadout` holds the channel set.
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
        };
        assert_eq!(ro.channels.len(), 2);
        assert_eq!(ro.channels[0].index, 0);
        assert_eq!(ro.channels[1].index, 0);
    }

    /// (f) P3-04: a multi-channel `IntelReadout` — a fully populated
    /// channel plus an all-`Na` degradation channel — round-trips through
    /// bincode, proving the Intel readout (every field type of
    /// `IntelChannel`) is wire-safe.
    #[test]
    fn intel_readout_bincode_round_trip() {
        let ro = IntelReadout {
            channels: vec![
                decode_channel(0, Some(160), full_regs()),
                decode_channel(1, None, [None; 4]), // all-Na channel
            ],
        };

        let bytes = bincode::serialize(&ro)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
        assert_eq!(ro, back);

        // the fully degraded readout (every channel all-Na) is wire-safe too
        let all_na = IntelReadout {
            channels: vec![
                decode_channel(0, None, [None; 4]),
                decode_channel(1, None, [None; 4]),
            ],
        };
        let bytes = bincode::serialize(&all_na)
            .expect("IntelReadout must serialize (no-panic contract)");
        let back: IntelReadout =
            bincode::deserialize(&bytes).expect("IntelReadout must deserialize");
        assert_eq!(all_na, back);
    }
}
