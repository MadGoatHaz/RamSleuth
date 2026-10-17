//! The sanity gates and shared screening helpers for the Intel readout
//! decode paths (step 1 of the `intel_readout` directory-module split —
//! moved verbatim from `intel_readout.rs`; the decode paths stay in
//! [`super`]).
//!
//! Two families:
//!
//! - **Degenerate-word screening** (C-2, H-2, H-14, M-11; the N-05
//!   canonical contract): [`is_present_word`] / [`is_degenerate_word`] —
//!   crate-visible so sibling decode modules can share the same rules.
//! - **Per-cell sanity gates**: the plausibility bounds
//!   ([`TIMING_MIN_TICKS`]..=[`TIMING_MAX_TICKS`], [`TRFC1_MAX_TICKS`],
//!   [`CLOCK_MIN_MHZ`]..=[`CLOCK_MAX_MHZ`], [`TC_PRE_HI_WORD_SHIFT`]),
//!   the cell builders that apply them ([`timing_cell`] /
//!   [`ticks_section`] / [`clock_section`] / [`quartet_cell`]), and the
//!   all-`Na(reason)` builders for the frozen slots Intel never sources
//!   (N-06: [`na_clocks`] / [`na_cad_bus`] / [`na_voltages`] /
//!   [`na_rtl`]).

use crate::amd_readout::{CadBus, ClockReadout, VoltageSet};
use crate::error::{NaReason, Section};

// ---------------------------------------------------------------------------
// Sanity ranges (mirroring the AMD display gates: the same plausible
// bounds apply to the vendor-neutral display types, plan D3).
// ---------------------------------------------------------------------------

/// Plausible DRAM subtiming range (integer DRAM clock cycles).
pub(super) const TIMING_MIN_TICKS: u16 = 1;
pub(super) const TIMING_MAX_TICKS: u16 = 2048;

/// tRFC1 on DDR5 can reach 4095 (12-bit field max) for high-density dies.
pub(super) const TRFC1_MAX_TICKS: u16 = 4095;

/// Maximum plausible memory clock in MHz.
/// DDR5-8800 → 4400 MHz; DDR5-10666 → 5333 MHz.
/// Set to 5333 to cover all current and near-future DDR5 speeds.
pub(super) const CLOCK_MIN_MHZ: f64 = 1.0;
pub(super) const CLOCK_MAX_MHZ: f64 = 5333.0;

/// Bit position offset from the 64-bit `TC_PRE` field spec to the high
/// 32-bit word (N-08). The datasheet `TC_PRE` spec is a 64-bit `[63:0]`
/// register; the high word holds `[63:32]`, so a field at spec position
/// `P` sits at position `P - 32` in the hi word.
pub(super) const TC_PRE_HI_WORD_SHIFT: u32 = 32;

// ---------------------------------------------------------------------------
// Degenerate-word screening helpers (C-2, H-2, H-14, M-11).
// ---------------------------------------------------------------------------

/// A word is "present" only if it's non-zero AND not a bus-fault
/// (`0xFFFF_FFFF`). This prevents degenerate reads from being
/// misinterpreted as valid evidence (e.g. OQ-11's DDR5 condition).
///
/// The canonical degenerate-screening helpers for this module (N-05):
/// every decode path screens raw words through these two functions
/// (crate-visible so sibling decode modules can share the same
/// contract).
pub(crate) fn is_present_word(v: Option<u32>) -> bool {
    matches!(v, Some(w) if w != 0 && w != 0xFFFF_FFFF)
}

/// A word is "degenerate" if it is `0` (untrained) or `0xFFFF_FFFF`
/// (bus-fault readback) — the two degenerate encodings Research line 329
/// uses for the mirror window. `None` (a failed read) is *not* degenerate
/// in this sense; it is a separate containment case.
pub(crate) fn is_degenerate_word(w: u32) -> bool {
    w == 0 || w == 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// The shared per-cell sanity gates (every decode path routes a decoded
// value through these before it may become a `Value` cell).
// ---------------------------------------------------------------------------

/// Decode one displayed tick field from an optionally-failed register
/// read (the per-register containment rule): a failed read →
/// `Na(ParseError)`; a degenerate word (`0` = untrained,
/// `0xFFFF_FFFF` = bus-fault) → `Na(ParseError)`; a decoded value
/// outside [1, max_ticks] → `Na(ParseError)`; otherwise the tick value.
pub(super) fn timing_cell(reg: Option<u32>, name: &str, extract: impl Fn(u32) -> u16) -> Section<u16> {
    timing_cell_max(reg, name, extract, TIMING_MAX_TICKS)
}

/// [`timing_cell`] with an explicit per-field tick maximum (H-4):
/// used for the widened DDR5 tRFC1 (12-bit, max 4095) and the DDR4
/// tRFC (13-bit, max 8191) fields that exceed the shared
/// [`TIMING_MAX_TICKS`] of 2048.
pub(super) fn timing_cell_max(
    reg: Option<u32>,
    name: &str,
    extract: impl Fn(u32) -> u16,
    max_ticks: u16,
) -> Section<u16> {
    match reg {
        None => Section::na(NaReason::ParseError(format!("{name}: register read failed"))),
        // H-14: Screen degenerate words (bus-fault / untrained) before
        // any bit extraction.
        Some(w) if is_degenerate_word(w) => {
            let detail = if w == 0 { "untrained (0)" } else { "bus-fault (0xFFFFFFFF)" };
            Section::na(NaReason::ParseError(format!("{name}: {detail}")))
        }
        Some(raw) => ticks_section_max(extract(raw), name, max_ticks),
    }
}

/// Sanity-gate a decoded tick count to a [`Section<u16>`] using the
/// shared [`TIMING_MAX_TICKS`] bound.
pub(super) fn ticks_section(ticks: u16, name: &str) -> Section<u16> {
    ticks_section_max(ticks, name, TIMING_MAX_TICKS)
}

/// Sanity-gate a decoded tick count to a [`Section<u16>`] using an
/// explicit `max_ticks` bound (H-4: per-field overrides for the
/// widened DDR5 / DDR4 tRFC fields).
pub(super) fn ticks_section_max(ticks: u16, name: &str, max_ticks: u16) -> Section<u16> {
    if (TIMING_MIN_TICKS..=max_ticks).contains(&ticks) {
        Section::Value(ticks)
    } else {
        Section::na(NaReason::ParseError(format!(
            "{name} {ticks} ticks outside plausible range [{TIMING_MIN_TICKS}, {max_ticks}]"
        )))
    }
}

/// Sanity-gate a decoded memory clock (MHz) to a [`Section<f64>`].
pub(super) fn clock_section(mhz: f64) -> Section<f64> {
    if (CLOCK_MIN_MHZ..=CLOCK_MAX_MHZ).contains(&mhz) {
        Section::Value(mhz)
    } else {
        Section::na(NaReason::ParseError(format!(
            "memory clock {mhz} MHz outside plausible range [{CLOCK_MIN_MHZ:.0}, {CLOCK_MAX_MHZ:.0}]"
        )))
    }
}

/// Decode one displayed slot from a per-register turnaround quartet
/// (per-register containment: a failed register read →
/// `Na(ParseError)` for every slot sourced from it).
pub(super) fn quartet_cell(quartet: Option<[u16; 4]>, name: &str, slot: usize) -> Section<u16> {
    match quartet {
        None => Section::na(NaReason::ParseError(format!("{name}: register read failed"))),
        Some(q) => ticks_section(q[slot], name),
    }
}

// ---------------------------------------------------------------------------
// N-06: the shared all-`Na(reason)` builders. The frozen display slots that
// Intel never sources (the CAD bus, the voltages, the RTL, and the clock /
// gear / command-rate cells on the live decode paths) used to be built by
// five copy-pasted struct literals; these helpers are the single source
// (pure refactor — the built values are byte-identical).
// ---------------------------------------------------------------------------

/// An all-`Na(reason)` [`ClockReadout`] (the eight frozen clock / gear /
/// command-rate slots).
pub(super) fn na_clocks(reason: &NaReason) -> ClockReadout {
    ClockReadout {
        mclk_mhz: Section::na(reason.clone()),
        uclk_mhz: Section::na(reason.clone()),
        fclk_mhz: Section::na(reason.clone()),
        div_mode: Section::na(reason.clone()),
        gear_mode: Section::na(reason.clone()),
        gdm: Section::na(reason.clone()),
        pdm: Section::na(reason.clone()),
        command_rate: Section::na(reason.clone()),
    }
}

/// An all-`Na(reason)` [`CadBus`] (the eight frozen CAD slots).
pub(super) fn na_cad_bus(reason: &NaReason) -> CadBus {
    CadBus {
        proc_odt: Section::na(reason.clone()),
        rtt_nom: Section::na(reason.clone()),
        rtt_wr: Section::na(reason.clone()),
        rtt_park: Section::na(reason.clone()),
        clk_drv: Section::na(reason.clone()),
        addr_cmd_drv: Section::na(reason.clone()),
        cs_odt_drv: Section::na(reason.clone()),
        cke_drv: Section::na(reason.clone()),
    }
}

/// An all-`Na(reason)` [`VoltageSet`] (the five frozen rail slots).
pub(super) fn na_voltages(reason: &NaReason) -> VoltageSet {
    VoltageSet {
        vddcr_soc_mv: Section::na(reason.clone()),
        vddio_mem_mv: Section::na(reason.clone()),
        vdd_misc_mv: Section::na(reason.clone()),
        vpp_mv: Section::na(reason.clone()),
        vcore_mv: Section::na(reason.clone()),
    }
}

/// An all-`Na(reason)` RTL slot.
pub(super) fn na_rtl(reason: &NaReason) -> Section<u16> {
    Section::na(reason.clone())
}
