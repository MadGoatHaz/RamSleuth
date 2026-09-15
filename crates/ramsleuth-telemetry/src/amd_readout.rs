//! AMD subtiming readout + the four vendor-neutral display types (P2-05).
//!
//! This module freezes the shared display types that BOTH the AMD readout
//! (here) and the Intel readout (P2-07) populate, plus the AMD mapping from
//! the frozen [`AmdPmSnapshot`] (P2-04) into those types. Plan D3: the
//! display types are vendor-neutral (MHz, ticks, ohms, mV) and are the single
//! mapping target for both vendors; the facade (P2-10) and the CLI (P2-11)
//! consume them.
//!
//! # Display types (frozen — P2-07 / P2-10 code against these)
//!
//! - [`ClockReadout`] — MCLK/UCLK/FCLK (MHz) + UCLK:MCLK divide mode + gear
//!   mode + GDM/PDM flags + DRAM command rate.
//! - [`TimingSet`] — the 27 DRAM subtimings, in ticks.
//! - [`CadBus`] — CAD-bus ODT / driver strengths in ohms + the three RTT
//!   fields as [`RttValue`].
//! - [`VoltageSet`] — the four memory rails, in millivolts.
//! - [`AmdReadout`] — the AMD aggregate of clocks + the three sets.
//!
//! Every field is a [`Section<T>`]: a value, or a structured [`NaReason`] when
//! the raw data is absent, not applicable to the platform, or outside a
//! plausible sanity range (the crate-wide no-panic contract, plan D5).
//!
//! # AMD mapping rules ([`map_amd`])
//!
//! - **Clocks** — raw MHz pass through as `f64`; a reading of `0` or above the
//!   plausible ceiling degrades to [`NaReason::ParseError`]. UCLK:MCLK divide
//!   mode maps `0`→[`DivMode::OneToOne`], `1`→[`DivMode::OneToTwo`], else
//!   [`NaReason::ParseError`]. **Gear mode is not reported by AMD PM tables**
//!   → [`NaReason::NotApplicable`]. GDM/PDM map `0`/`1` to `false`/`true`,
//!   else [`NaReason::ParseError`]. DRAM command rate maps `0`→
//!   [`CommandRate::OneT`], `1`→[`CommandRate::TwoT`], else
//!   [`NaReason::ParseError`].
//! - **Timings** — raw tick counts pass through as `u16` (the plan's display
//!   unit is ticks); `0` or above the plausible ceiling →
//!   [`NaReason::ParseError`].
//! - **CAD bus** — raw RZQ/driver codes map to ohms via the RZQ = 240 Ω base
//!   code table ([`RZQ_KNOWN_CODES`]); an unknown code (or code `0` for a
//!   pure-ohm field) → [`NaReason::NotApplicable`]. The three RTT fields use
//!   [`RttValue`]: code `0` → [`RttValue::Disabled`], a known code →
//!   [`RttValue::Rzq`], unknown → [`NaReason::NotApplicable`].
//! - **Voltages** — raw millivolts pass through as `u16`; below the plausible
//!   floor or above the ceiling → [`NaReason::ParseError`].
//!
//! # Design notes / deliberate deviations from the plan sketch
//!
//! - **Voltages stay in millivolts** (`Section<u16>`), not volts. The plan
//!   sketch showed `Option<f32>` volts, but the frozen brief specifies "each
//!   `Section<u16>` mV" and "pass through mV (sanity-gated)". mV→V display
//!   formatting is the P2-11 CLI's job; keeping integer mV in the frozen
//!   display contract avoids float rounding.
//! - **Timings stay in ticks** (`Section<u16>`), the plan's stated display
//!   unit (§1.4 lists every timing in ticks). No tick→ns conversion here.
//! - **Driver strengths use the RZQ code table** for the ohm mapping: the
//!   P2-04 snapshot labels all eight CAD fields "RZQ/driver codes" and the
//!   plan mandates a single "code→Ω table". A byte-exact driver encoding is a
//!   P2-11 live-verification concern (the P2-04 layout is a plan skeleton).
//!
//! # Safety
//!
//! No I/O and no `unsafe`: this is a pure mapping over the frozen
//! [`AmdPmSnapshot`]. Every code path returns a [`Section`]; nothing panics,
//! unwraps, or dereferences unguarded data.

use crate::amd_pm::AmdPmSnapshot;
use crate::error::{NaReason, Section};

// ---------------------------------------------------------------------------
// Sanity ranges (inclusive) for the AMD mapping. A reading outside its range
// degrades to `Na(ParseError(..))` rather than a panic (plan D5).
// ---------------------------------------------------------------------------

/// RZQ reference impedance (ohms) — the base of the CAD code table.
const RZQ_BASE_OHMS: f64 = 240.0;

/// The known RZQ divisor codes (the denominator `N` in "RZQ/N"). Any code not
/// in this set is not a recognized RZQ/ODT encoding → `Na(NotApplicable)`.
/// Each maps to `RZQ_BASE_OHMS / code` ohms.
const RZQ_KNOWN_CODES: [u16; 16] = [
    1, 2, 3, 4, 5, 6, 8, 10, 12, 15, 16, 20, 24, 30, 40, 60,
];

/// Plausible memory/fabric clock range in MHz.
const CLOCK_MIN_MHZ: f64 = 1.0;
const CLOCK_MAX_MHZ: f64 = 4096.0;

/// Plausible DRAM subtiming range in ticks.
const TIMING_MIN_TICKS: u16 = 1;
const TIMING_MAX_TICKS: u16 = 2048;

/// Plausible memory-rail range in millivolts.
const VOLTAGE_MIN_MV: u16 = 100;
const VOLTAGE_MAX_MV: u16 = 4000;

// ---------------------------------------------------------------------------
// Vendor-neutral display types (frozen here; P2-07 + P2-10 consume them).
// ---------------------------------------------------------------------------

/// UCLK:MCLK divide mode (the "1:1 / 1:2" ratio shown on the dashboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DivMode {
    /// UCLK runs at the same rate as MCLK (1:1).
    OneToOne,
    /// UCLK runs at half the MCLK rate (1:2).
    OneToTwo,
}

/// DRAM command rate (the "1T / 2T" bus encoding shown on the dashboard).
///
/// Sourced from the SMN `0x50200` command-rate bit via the raw
/// [`AmdPmSnapshot::command_rate`] slot (C6-04) and mapped by [`map_amd`];
/// a reserved encoding degrades to [`NaReason::ParseError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandRate {
    /// One transaction per clock (1T).
    OneT,
    /// Two transactions per clock (2T).
    TwoT,
}

/// Memory-controller gear mode (SA:MEM clock multiplier).
///
/// An Intel-side concept; AMD PM tables do not report it, so the AMD mapping
/// leaves [`ClockReadout::gear_mode`] as `Na(NotApplicable)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GearMode {
    /// Gear 1.
    One,
    /// Gear 2.
    Two,
    /// Gear 4.
    Four,
}

/// A raw-data ODT / RTT resistance value.
///
/// - [`RttValue::Disabled`] — the RTT path is off (code `0`).
/// - [`RttValue::Rzq`] — a RZQ divisor code (`N` in "RZQ/N"); the ohm value is
///   `RZQ_BASE_OHMS / N`.
/// - [`RttValue::Ohms`] — a value already expressed in ohms (kept for sources
///   that report ohms directly; the AMD code table produces
///   [`RttValue::Rzq`] / [`RttValue::Disabled`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RttValue {
    /// The RTT path is disabled.
    Disabled,
    /// A RZQ divisor code (`N` in "RZQ/N").
    Rzq(u32),
    /// A resistance already in ohms.
    Ohms(f64),
}

impl RttValue {
    /// The ohm value for this RTT, if determinable.
    ///
    /// [`RttValue::Disabled`] has no ohm value → `None`. [`RttValue::Rzq`]
    /// resolves against the RZQ = 240 Ω base. [`RttValue::Ohms`] is the value
    /// itself.
    pub fn ohms(self) -> Option<f64> {
        match self {
            Self::Disabled => None,
            Self::Rzq(n) => Some(RZQ_BASE_OHMS / f64::from(n)),
            Self::Ohms(v) => Some(v),
        }
    }
}

/// Clocks and clock ratios (MHz), the vendor-neutral readout.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClockReadout {
    /// Memory clock (MHz).
    pub mclk_mhz: Section<f64>,
    /// Memory-controller clock (MHz).
    pub uclk_mhz: Section<f64>,
    /// Infinity Fabric clock (MHz).
    pub fclk_mhz: Section<f64>,
    /// UCLK:MCLK divide mode (1:1 / 1:2).
    pub div_mode: Section<DivMode>,
    /// Memory-controller gear mode (SA:MEM multiplier); not reported by AMD.
    pub gear_mode: Section<GearMode>,
    /// Gear Down Mode flag.
    pub gdm: Section<bool>,
    /// Power Down Mode flag.
    pub pdm: Section<bool>,
    /// DRAM command rate (1T / 2T).
    pub command_rate: Section<CommandRate>,
}

/// The 27 DRAM subtimings, in ticks (the plan's display unit).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TimingSet {
    /// tCL — CAS latency.
    pub cl: Section<u16>,
    /// tRCDWR — RAS-to-CAS delay (write).
    pub rcwdwr: Section<u16>,
    /// tRCDRD — RAS-to-CAS delay (read).
    pub rcdrd: Section<u16>,
    /// tRP — row precharge.
    pub rp: Section<u16>,
    /// tRAS — active time.
    pub ras: Section<u16>,
    /// tRC — row cycle.
    pub rc: Section<u16>,
    /// tRRDS — row-to-row (same bank group).
    pub rrds: Section<u16>,
    /// tRRDL — row-to-row (different bank group).
    pub rrld: Section<u16>,
    /// tFAW — four-activate window.
    pub faw: Section<u16>,
    /// tWTRS — write-to-read (same rank).
    pub wtrs: Section<u16>,
    /// tWTRL — write-to-read (different rank).
    pub wtrl: Section<u16>,
    /// tWR — write recovery.
    pub wr: Section<u16>,
    /// tRFC1 — refresh cycle 1.
    pub rfc1: Section<u16>,
    /// tRFC2 — refresh cycle 2.
    pub rfc2: Section<u16>,
    /// tRFCsb — refresh (same bank).
    pub rfcsb: Section<u16>,
    /// tCWL — CAS write latency.
    pub cwl: Section<u16>,
    /// tRTP — read-to-precharge.
    pub rtp: Section<u16>,
    /// tRDWR — read-to-write.
    pub rdwr: Section<u16>,
    /// tWRRD — write-to-read turnaround.
    pub wrrd: Section<u16>,
    /// tRDRD — read-to-read, same DIMM.
    pub rdrd_sd: Section<u16>,
    /// tRDRD — read-to-read, same CCD.
    pub rdrd_dd: Section<u16>,
    /// tRDRD — read-to-read, via SCL.
    pub rdrd_scl: Section<u16>,
    /// tRDRD — read-to-read, via SC.
    pub rdrd_sc: Section<u16>,
    /// tWRWR — write-to-write, same DIMM.
    pub wrwr_sd: Section<u16>,
    /// tWRWR — write-to-write, same CCD.
    pub wrwr_dd: Section<u16>,
    /// tWRWR — write-to-write, via SCL.
    pub wrwr_scl: Section<u16>,
    /// tWRWR — write-to-write, via SC.
    pub wrwr_sc: Section<u16>,
}

/// CAD (command/address/data) bus ODT and driver strengths.
///
/// The five ODT/driver fields are ohms; the three RTT fields are
/// [`RttValue`] (disabled / RZQ divisor / ohms).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CadBus {
    /// Processor ODT (ohms).
    pub proc_odt: Section<f64>,
    /// RTT nominal ([`RttValue`]).
    pub rtt_nom: Section<RttValue>,
    /// RTT write ([`RttValue`]).
    pub rtt_wr: Section<RttValue>,
    /// RTT park ([`RttValue`]).
    pub rtt_park: Section<RttValue>,
    /// Clock driver strength (ohms).
    pub clk_drv: Section<f64>,
    /// Address/command driver strength (ohms).
    pub addr_cmd_drv: Section<f64>,
    /// CS/ODT driver strength (ohms).
    pub cs_odt_drv: Section<f64>,
    /// CKE driver strength (ohms).
    pub cke_drv: Section<f64>,
}

/// Memory/SOC rail voltages, in millivolts.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoltageSet {
    /// VDDCR_SOC (mV).
    pub vddcr_soc_mv: Section<u16>,
    /// VDDIO_MEM (mV).
    pub vddio_mem_mv: Section<u16>,
    /// VDD_MISC (mV).
    pub vdd_misc_mv: Section<u16>,
    /// VPP (mV).
    pub vpp_mv: Section<u16>,
}

/// The AMD-specific aggregate of the four vendor-neutral display sets.
///
/// Produced by [`map_amd`] from the frozen [`AmdPmSnapshot`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AmdReadout {
    /// Clocks + ratios.
    pub clocks: ClockReadout,
    /// DRAM subtimings (ticks).
    pub timings: TimingSet,
    /// CAD-bus ODT / driver strengths.
    pub cad_bus: CadBus,
    /// Memory/SOC rail voltages (mV).
    pub voltages: VoltageSet,
}

// ---------------------------------------------------------------------------
// Pure mapping helpers (no I/O, no unsafe).
// ---------------------------------------------------------------------------

/// Resolves a known RZQ divisor code to its ohm value (`RZQ / code`), or
/// `None` for a code the table does not recognize (including `0`).
fn rzq_ohms(code: u16) -> Option<f64> {
    let known = RZQ_KNOWN_CODES.contains(&code);
    known.then(|| RZQ_BASE_OHMS / f64::from(code))
}

/// Maps a raw MHz clock to a sanity-gated [`Section<f64>`].
fn map_clock(mhz: u16) -> Section<f64> {
    let v = f64::from(mhz);
    if (CLOCK_MIN_MHZ..=CLOCK_MAX_MHZ).contains(&v) {
        Section::Value(v)
    } else {
        Section::na(NaReason::ParseError(format!(
            "clock {mhz} MHz outside plausible range [{CLOCK_MIN_MHZ:.0}, {CLOCK_MAX_MHZ:.0}]"
        )))
    }
}

/// Maps a raw UCLK:MCLK divide-mode byte to a sanity-gated
/// [`Section<DivMode>`].
fn map_div_mode(raw: u8) -> Section<DivMode> {
    match raw {
        0 => Section::Value(DivMode::OneToOne),
        1 => Section::Value(DivMode::OneToTwo),
        _ => Section::na(NaReason::ParseError(format!(
            "UCLK:MCLK divide mode {raw} not 0 (1:1) or 1 (1:2)"
        ))),
    }
}

/// Maps a raw DRAM command-rate byte to a sanity-gated
/// [`Section<CommandRate>`].
fn map_command_rate(raw: u8) -> Section<CommandRate> {
    match raw {
        0 => Section::Value(CommandRate::OneT),
        1 => Section::Value(CommandRate::TwoT),
        _ => Section::na(NaReason::ParseError(format!(
            "DRAM command rate {raw} not 0 (1T) or 1 (2T)"
        ))),
    }
}

/// Maps a raw 0/1 mode byte (GDM / PDM) to a sanity-gated [`Section<bool>`].
fn map_mode_flag(raw: u8) -> Section<bool> {
    match raw {
        0 => Section::Value(false),
        1 => Section::Value(true),
        _ => Section::na(NaReason::ParseError(format!("mode byte {raw} not 0 or 1"))),
    }
}

/// Maps a raw tick count to a sanity-gated [`Section<u16>`] (ticks are the
/// plan's display unit).
fn map_timing(ticks: u16) -> Section<u16> {
    if (TIMING_MIN_TICKS..=TIMING_MAX_TICKS).contains(&ticks) {
        Section::Value(ticks)
    } else {
        Section::na(NaReason::ParseError(format!(
            "timing {ticks} ticks outside plausible range [{TIMING_MIN_TICKS}, {TIMING_MAX_TICKS}]"
        )))
    }
}

/// Maps a raw CAD ODT/driver code to a sanity-gated [`Section<f64>`] (ohms).
/// Code `0` or an unrecognized code → `Na(NotApplicable)`.
fn map_code_to_ohms(code: u16) -> Section<f64> {
    match rzq_ohms(code) {
        Some(ohms) => Section::Value(ohms),
        None => Section::na(NaReason::NotApplicable),
    }
}

/// Maps a raw RTT code to a sanity-gated [`Section<RttValue>`]. Code `0` →
/// [`RttValue::Disabled`]; a known RZQ divisor → [`RttValue::Rzq`]; unknown →
/// `Na(NotApplicable)`.
fn map_rtt(code: u16) -> Section<RttValue> {
    if code == 0 {
        return Section::Value(RttValue::Disabled);
    }
    match rzq_ohms(code) {
        Some(_) => Section::Value(RttValue::Rzq(u32::from(code))),
        None => Section::na(NaReason::NotApplicable),
    }
}

/// Maps a raw millivolt reading to a sanity-gated [`Section<u16>`] (mV are
/// passed through; the P2-11 CLI formats them as volts).
fn map_voltage(mv: u16) -> Section<u16> {
    if (VOLTAGE_MIN_MV..=VOLTAGE_MAX_MV).contains(&mv) {
        Section::Value(mv)
    } else {
        Section::na(NaReason::ParseError(format!(
            "voltage {mv} mV outside plausible range [{VOLTAGE_MIN_MV}, {VOLTAGE_MAX_MV}]"
        )))
    }
}

/// Maps the frozen [`AmdPmSnapshot`] (P2-04) to the vendor-neutral
/// [`AmdReadout`].
///
/// Pure: no I/O, no `unsafe`, no panic. Every field degrades to
/// [`Section::Na`] on absent / not-applicable / out-of-band data.
pub fn map_amd(snap: &AmdPmSnapshot) -> AmdReadout {
    let t = &snap.timings;
    let c = &snap.cad_bus;
    let v = &snap.voltages;

    AmdReadout {
        clocks: ClockReadout {
            mclk_mhz: map_clock(snap.mclk_mhz),
            uclk_mhz: map_clock(snap.uclk_mhz),
            fclk_mhz: map_clock(snap.fclk_mhz),
            div_mode: map_div_mode(snap.div_mode),
            // AMD PM tables report the UCLK:MCLK divide mode, not an
            // Intel-style SA:MEM gear multiplier → not applicable.
            gear_mode: Section::na(NaReason::NotApplicable),
            gdm: map_mode_flag(snap.gdm),
            pdm: map_mode_flag(snap.pdm),
            command_rate: map_command_rate(snap.command_rate),
        },
        timings: TimingSet {
            cl: map_timing(t.cl),
            rcwdwr: map_timing(t.rcwdwr),
            rcdrd: map_timing(t.rcdrd),
            rp: map_timing(t.rp),
            ras: map_timing(t.ras),
            rc: map_timing(t.rc),
            rrds: map_timing(t.rrds),
            rrld: map_timing(t.rrld),
            faw: map_timing(t.faw),
            wtrs: map_timing(t.wtrs),
            wtrl: map_timing(t.wtrl),
            wr: map_timing(t.wr),
            rfc1: map_timing(t.rfc1),
            rfc2: map_timing(t.rfc2),
            rfcsb: map_timing(t.rfcsb),
            cwl: map_timing(t.cwl),
            rtp: map_timing(t.rtp),
            rdwr: map_timing(t.rdwr),
            wrrd: map_timing(t.wrrd),
            rdrd_sd: map_timing(t.rdrd_sd),
            rdrd_dd: map_timing(t.rdrd_dd),
            rdrd_scl: map_timing(t.rdrd_scl),
            rdrd_sc: map_timing(t.rdrd_sc),
            wrwr_sd: map_timing(t.wrwr_sd),
            wrwr_dd: map_timing(t.wrwr_dd),
            wrwr_scl: map_timing(t.wrwr_scl),
            wrwr_sc: map_timing(t.wrwr_sc),
        },
        cad_bus: CadBus {
            proc_odt: map_code_to_ohms(c.proc_odt),
            rtt_nom: map_rtt(c.rtt_nom),
            rtt_wr: map_rtt(c.rtt_wr),
            rtt_park: map_rtt(c.rtt_park),
            clk_drv: map_code_to_ohms(c.clk_drv),
            addr_cmd_drv: map_code_to_ohms(c.addr_cmd_drv),
            cs_odt_drv: map_code_to_ohms(c.cs_odt_drv),
            cke_drv: map_code_to_ohms(c.cke_drv),
        },
        voltages: VoltageSet {
            vddcr_soc_mv: map_voltage(v.vddcr_soc_mv),
            vddio_mem_mv: map_voltage(v.vddio_mem_mv),
            vdd_misc_mv: map_voltage(v.vdd_misc_mv),
            vpp_mv: map_voltage(v.vpp_mv),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amd_pm::{AmdPmCadBus, AmdPmTimings, AmdPmVoltages};

    /// A fully in-range synthetic snapshot (DDR4-3200 class values).
    fn good_snapshot() -> AmdPmSnapshot {
        AmdPmSnapshot {
            version: 0x0007_0B02, // 7.11.2 — the test-host family
            mclk_mhz: 1600,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 16,
                rcwdwr: 16,
                rcdrd: 16,
                rp: 16,
                ras: 32,
                rc: 48,
                rrds: 4,
                rrld: 4,
                faw: 16,
                wtrs: 8,
                wtrl: 8,
                wr: 8,
                rfc1: 160,
                rfc2: 160,
                rfcsb: 160,
                cwl: 16,
                rtp: 8,
                rdwr: 8,
                wrrd: 4,
                rdrd_sd: 100,
                rdrd_dd: 101,
                rdrd_scl: 102,
                rdrd_sc: 103,
                wrwr_sd: 104,
                wrwr_dd: 105,
                wrwr_scl: 106,
                wrwr_sc: 107,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 5,
                rtt_nom: 2,
                rtt_wr: 0,
                rtt_park: 4,
                clk_drv: 6,
                addr_cmd_drv: 8,
                cs_odt_drv: 10,
                cke_drv: 12,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1050,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
            },
        }
    }

    /// (a) In-range values map to `Section::Value` with the correct converted
    /// values: MHz passthrough, ticks passthrough, RZQ code → Ω, mV
    /// passthrough, and the AMD-not-applicable gear mode → `Na`.
    #[test]
    fn valid_snapshot_maps_to_value_fields() {
        let ro = map_amd(&good_snapshot());

        // clocks — MHz passthrough, div mode, gear mode N/A, GDM/PDM flags
        assert_eq!(ro.clocks.mclk_mhz, Section::Value(1600.0));
        assert_eq!(ro.clocks.uclk_mhz, Section::Value(1600.0));
        assert_eq!(ro.clocks.fclk_mhz, Section::Value(1600.0));
        assert_eq!(ro.clocks.div_mode, Section::Value(DivMode::OneToOne));
        assert_eq!(ro.clocks.gear_mode, Section::Na(NaReason::NotApplicable));
        assert_eq!(ro.clocks.gdm, Section::Value(true));
        assert_eq!(ro.clocks.pdm, Section::Value(false));
        assert_eq!(ro.clocks.command_rate, Section::Value(CommandRate::OneT));

        // timings — ticks passthrough (a sample across primary + tertiary)
        assert_eq!(ro.timings.cl, Section::Value(16));
        assert_eq!(ro.timings.ras, Section::Value(32));
        assert_eq!(ro.timings.rc, Section::Value(48));
        assert_eq!(ro.timings.rfc1, Section::Value(160));
        assert_eq!(ro.timings.rdrd_sc, Section::Value(103));
        assert_eq!(ro.timings.wrwr_sc, Section::Value(107));

        // cad bus — RZQ code → Ω (240/N) and RTT codes
        assert_eq!(ro.cad_bus.proc_odt, Section::Value(48.0)); // RZQ/5
        assert_eq!(ro.cad_bus.rtt_nom, Section::Value(RttValue::Rzq(2)));
        assert_eq!(ro.cad_bus.rtt_wr, Section::Value(RttValue::Disabled));
        assert_eq!(ro.cad_bus.rtt_park, Section::Value(RttValue::Rzq(4)));
        assert_eq!(ro.cad_bus.clk_drv, Section::Value(40.0)); // RZQ/6
        assert_eq!(ro.cad_bus.addr_cmd_drv, Section::Value(30.0)); // RZQ/8
        assert_eq!(ro.cad_bus.cs_odt_drv, Section::Value(24.0)); // RZQ/10
        assert_eq!(ro.cad_bus.cke_drv, Section::Value(20.0)); // RZQ/12

        // voltages — mV passthrough
        assert_eq!(ro.voltages.vddcr_soc_mv, Section::Value(1050));
        assert_eq!(ro.voltages.vddio_mem_mv, Section::Value(1350));
        assert_eq!(ro.voltages.vdd_misc_mv, Section::Value(1000));
        assert_eq!(ro.voltages.vpp_mv, Section::Value(1800));
    }

    /// (b) A zero or absurd clock degrades that clock to `Na(ParseError)`
    /// while the other valid fields stay `Value`.
    #[test]
    fn out_of_band_clock_degrades_to_na_others_stay_value() {
        let mut s = good_snapshot();
        s.mclk_mhz = 0; // zero → Na
        s.fclk_mhz = 65535; // absurd → Na
        let ro = map_amd(&s);

        assert!(ro.clocks.mclk_mhz.is_na());
        assert_eq!(ro.clocks.mclk_mhz.value(), None);
        assert!(matches!(ro.clocks.mclk_mhz, Section::Na(NaReason::ParseError(_))));
        assert!(ro.clocks.fclk_mhz.is_na());
        assert!(matches!(ro.clocks.fclk_mhz, Section::Na(NaReason::ParseError(_))));

        // the valid clock and unrelated sections are unaffected
        assert_eq!(ro.clocks.uclk_mhz, Section::Value(1600.0));
        assert!(!ro.clocks.uclk_mhz.is_na());
        assert_eq!(ro.timings.cl, Section::Value(16));
        assert_eq!(ro.voltages.vddio_mem_mv, Section::Value(1350));
    }

    /// (c) An unknown RZQ code (not in the table) maps to
    /// `Na(NotApplicable)`; a known code still maps.
    #[test]
    fn unknown_rzq_code_maps_to_not_applicable() {
        let mut s = good_snapshot();
        s.cad_bus.rtt_nom = 7; // not a RZQ divisor code → Na
        s.cad_bus.proc_odt = 9; // not a RZQ divisor code → Na
        s.cad_bus.clk_drv = 65535; // absurd → Na
        let ro = map_amd(&s);

        assert_eq!(ro.cad_bus.rtt_nom, Section::Na(NaReason::NotApplicable));
        assert_eq!(ro.cad_bus.proc_odt, Section::Na(NaReason::NotApplicable));
        assert_eq!(ro.cad_bus.clk_drv, Section::Na(NaReason::NotApplicable));
        // a known code still maps
        assert_eq!(ro.cad_bus.rtt_park, Section::Value(RttValue::Rzq(4)));
        assert_eq!(ro.cad_bus.cke_drv, Section::Value(20.0));
    }

    /// (d) Every field of `AmdReadout` is a `Section`; `is_na()` / `value()`
    /// behave as expected across all four sets (all `Value` in the good
    /// snapshot except the AMD-not-applicable gear mode).
    #[test]
    fn every_field_is_a_section_value_and_na_exercised() {
        let ro = map_amd(&good_snapshot());

        // ClockReadout (mixed field types — each a Section)
        assert!(!ro.clocks.mclk_mhz.is_na());
        assert_eq!(ro.clocks.mclk_mhz.value(), Some(&1600.0));
        assert!(!ro.clocks.uclk_mhz.is_na());
        assert!(!ro.clocks.fclk_mhz.is_na());
        assert!(!ro.clocks.div_mode.is_na());
        assert_eq!(ro.clocks.div_mode.value(), Some(&DivMode::OneToOne));
        assert!(!ro.clocks.gdm.is_na());
        assert_eq!(ro.clocks.gdm.value(), Some(&true));
        assert!(!ro.clocks.pdm.is_na());
        assert_eq!(ro.clocks.pdm.value(), Some(&false));
        assert!(!ro.clocks.command_rate.is_na());
        assert_eq!(ro.clocks.command_rate.value(), Some(&CommandRate::OneT));
        // the one Na field (AMD reports no gear mode)
        assert!(ro.clocks.gear_mode.is_na());
        assert_eq!(ro.clocks.gear_mode.value(), None);

        // TimingSet — all 27 `Section<u16>`, all Value in the good snapshot
        let timing_secs: [&Section<u16>; 27] = [
            &ro.timings.cl,
            &ro.timings.rcwdwr,
            &ro.timings.rcdrd,
            &ro.timings.rp,
            &ro.timings.ras,
            &ro.timings.rc,
            &ro.timings.rrds,
            &ro.timings.rrld,
            &ro.timings.faw,
            &ro.timings.wtrs,
            &ro.timings.wtrl,
            &ro.timings.wr,
            &ro.timings.rfc1,
            &ro.timings.rfc2,
            &ro.timings.rfcsb,
            &ro.timings.cwl,
            &ro.timings.rtp,
            &ro.timings.rdwr,
            &ro.timings.wrrd,
            &ro.timings.rdrd_sd,
            &ro.timings.rdrd_dd,
            &ro.timings.rdrd_scl,
            &ro.timings.rdrd_sc,
            &ro.timings.wrwr_sd,
            &ro.timings.wrwr_dd,
            &ro.timings.wrwr_scl,
            &ro.timings.wrwr_sc,
        ];
        for s in &timing_secs {
            assert!(!s.is_na(), "good-snapshot timing must be Value");
            assert!(s.value().is_some());
        }

        // CadBus — five ohm `Section<f64>` + three `Section<RttValue>`, all Value
        let ohm_secs: [&Section<f64>; 5] = [
            &ro.cad_bus.proc_odt,
            &ro.cad_bus.clk_drv,
            &ro.cad_bus.addr_cmd_drv,
            &ro.cad_bus.cs_odt_drv,
            &ro.cad_bus.cke_drv,
        ];
        for s in &ohm_secs {
            assert!(!s.is_na(), "good-snapshot ohm field must be Value");
            assert!(s.value().is_some());
        }
        let rtt_secs: [&Section<RttValue>; 3] =
            [&ro.cad_bus.rtt_nom, &ro.cad_bus.rtt_wr, &ro.cad_bus.rtt_park];
        for s in &rtt_secs {
            assert!(!s.is_na(), "good-snapshot RTT field must be Value");
            assert!(s.value().is_some());
        }

        // VoltageSet — all four `Section<u16>` mV, all Value
        let volt_secs: [&Section<u16>; 4] = [
            &ro.voltages.vddcr_soc_mv,
            &ro.voltages.vddio_mem_mv,
            &ro.voltages.vdd_misc_mv,
            &ro.voltages.vpp_mv,
        ];
        for s in &volt_secs {
            assert!(!s.is_na(), "good-snapshot voltage must be Value");
            assert!(s.value().is_some());
        }
    }

    /// (e) The shared display types are `Clone` + `Debug` + `PartialEq` (so
    /// P2-07 / P2-10 can compare them), and `map_amd` is deterministic.
    #[test]
    fn shared_types_are_clone_debug_partial_eq() {
        fn assert_traits<T: Clone + std::fmt::Debug + PartialEq>() {}
        assert_traits::<ClockReadout>();
        assert_traits::<TimingSet>();
        assert_traits::<CadBus>();
        assert_traits::<VoltageSet>();
        assert_traits::<AmdReadout>();
        assert_traits::<DivMode>();
        assert_traits::<CommandRate>();
        assert_traits::<GearMode>();
        assert_traits::<RttValue>();

        let ro = map_amd(&good_snapshot());
        let cloned = ro.clone();
        assert_eq!(ro, cloned); // PartialEq
        let dbg = format!("{cloned:?}"); // Debug
        assert!(!dbg.is_empty());
        assert!(dbg.contains("mclk_mhz"), "Debug must name a field: {dbg}");
        // deterministic: mapping the same snapshot twice is equal
        assert_eq!(ro, map_amd(&good_snapshot()));
    }

    /// Mode-byte boundaries: 0/1 map, anything else → `Na(ParseError)`.
    #[test]
    fn mode_byte_boundaries() {
        for (raw, expected) in [(0u8, DivMode::OneToOne), (1, DivMode::OneToTwo)] {
            assert_eq!(map_div_mode(raw), Section::Value(expected));
        }
        for raw in [2u8, 7, 255] {
            assert!(matches!(map_div_mode(raw), Section::Na(NaReason::ParseError(_))));
        }

        assert_eq!(map_mode_flag(0), Section::Value(false));
        assert_eq!(map_mode_flag(1), Section::Value(true));
        for raw in [2u8, 9, 255] {
            assert!(matches!(map_mode_flag(raw), Section::Na(NaReason::ParseError(_))));
        }

        assert_eq!(map_command_rate(0), Section::Value(CommandRate::OneT));
        assert_eq!(map_command_rate(1), Section::Value(CommandRate::TwoT));
        for raw in [2u8, 9, 255] {
            assert!(matches!(map_command_rate(raw), Section::Na(NaReason::ParseError(_))));
        }
    }

    /// The DRAM command rate crosses [`map_amd`]: `0` →
    /// [`CommandRate::OneT`], `1` → [`CommandRate::TwoT`], any other raw
    /// byte → `Na(ParseError)` (the raw slot is a single SMN bit; a
    /// reserved value is never a mode).
    #[test]
    fn command_rate_maps_through_map_amd() {
        let mut s = good_snapshot();
        s.command_rate = 0;
        assert_eq!(map_amd(&s).clocks.command_rate, Section::Value(CommandRate::OneT));

        s.command_rate = 1;
        assert_eq!(map_amd(&s).clocks.command_rate, Section::Value(CommandRate::TwoT));

        for raw in [2u8, 7, 255] {
            s.command_rate = raw;
            let ro = map_amd(&s);
            assert!(
                matches!(ro.clocks.command_rate, Section::Na(NaReason::ParseError(_))),
                "{raw}"
            );
        }
    }

    /// Timing + voltage sanity gates: 0 and out-of-band → `Na(ParseError)`;
    /// in-band (incl. the inclusive upper bound) → `Value`.
    #[test]
    fn timing_and_voltage_sanity_gates() {
        // timings
        assert_eq!(map_timing(16), Section::Value(16));
        assert_eq!(map_timing(TIMING_MAX_TICKS), Section::Value(TIMING_MAX_TICKS));
        for t in [0u16, TIMING_MAX_TICKS + 1, 65535] {
            assert!(matches!(map_timing(t), Section::Na(NaReason::ParseError(_))), "{t}");
        }

        // voltages
        assert_eq!(map_voltage(1350), Section::Value(1350));
        assert_eq!(map_voltage(VOLTAGE_MIN_MV), Section::Value(VOLTAGE_MIN_MV));
        assert_eq!(map_voltage(VOLTAGE_MAX_MV), Section::Value(VOLTAGE_MAX_MV));
        for mv in [0u16, VOLTAGE_MIN_MV - 1, VOLTAGE_MAX_MV + 1, 65535] {
            assert!(matches!(map_voltage(mv), Section::Na(NaReason::ParseError(_))), "{mv}");
        }
    }

    /// Clock sanity gate boundaries: 0 and above the ceiling → `Na`,
    /// in-band (incl. the inclusive upper bound) → `Value`.
    #[test]
    fn clock_sanity_gate_boundaries() {
        assert_eq!(map_clock(1600), Section::Value(1600.0));
        assert_eq!(map_clock(4096), Section::Value(4096.0));
        for mhz in [0u16, 4097, 65535] {
            assert!(matches!(map_clock(mhz), Section::Na(NaReason::ParseError(_))), "{mhz}");
        }
    }

    /// The RZQ code table resolves known divisors to `240/N` ohms and rejects
    /// unknown codes (including 0).
    #[test]
    fn rzq_code_table() {
        assert_eq!(rzq_ohms(1), Some(240.0));
        assert_eq!(rzq_ohms(2), Some(120.0));
        assert_eq!(rzq_ohms(5), Some(48.0));
        assert_eq!(rzq_ohms(60), Some(4.0));
        for code in [0u16, 7, 9, 11, 13, 14, 100, 65535] {
            assert_eq!(rzq_ohms(code), None, "{code}");
        }
    }

    /// `RttValue::ohms()` resolves `Rzq` against the base, passes `Ohms`
    /// through, and yields `None` for `Disabled`.
    #[test]
    fn rtt_value_ohms() {
        assert_eq!(RttValue::Disabled.ohms(), None);
        assert_eq!(RttValue::Rzq(5).ohms(), Some(48.0));
        assert_eq!(RttValue::Ohms(33.5).ohms(), Some(33.5));
    }

    /// A `Section` in the fully degraded state: `Na(UnsupportedHardware)`.
    /// `T` is inferred from the struct field at each call site.
    fn na_cell<T>() -> Section<T> {
        Section::na(NaReason::UnsupportedHardware)
    }

    /// A fully degraded [`AmdReadout`]: every cell
    /// [`NaReason::UnsupportedHardware`] (the no-panic state for an
    /// unsupported platform, plan D5).
    fn all_na_readout() -> AmdReadout {
        AmdReadout {
            clocks: ClockReadout {
                mclk_mhz: na_cell(),
                uclk_mhz: na_cell(),
                fclk_mhz: na_cell(),
                div_mode: na_cell(),
                gear_mode: na_cell(),
                gdm: na_cell(),
                pdm: na_cell(),
                command_rate: na_cell(),
            },
            timings: TimingSet {
                cl: na_cell(),
                rcwdwr: na_cell(),
                rcdrd: na_cell(),
                rp: na_cell(),
                ras: na_cell(),
                rc: na_cell(),
                rrds: na_cell(),
                rrld: na_cell(),
                faw: na_cell(),
                wtrs: na_cell(),
                wtrl: na_cell(),
                wr: na_cell(),
                rfc1: na_cell(),
                rfc2: na_cell(),
                rfcsb: na_cell(),
                cwl: na_cell(),
                rtp: na_cell(),
                rdwr: na_cell(),
                wrrd: na_cell(),
                rdrd_sd: na_cell(),
                rdrd_dd: na_cell(),
                rdrd_scl: na_cell(),
                rdrd_sc: na_cell(),
                wrwr_sd: na_cell(),
                wrwr_dd: na_cell(),
                wrwr_scl: na_cell(),
                wrwr_sc: na_cell(),
            },
            cad_bus: CadBus {
                proc_odt: na_cell(),
                rtt_nom: na_cell(),
                rtt_wr: na_cell(),
                rtt_park: na_cell(),
                clk_drv: na_cell(),
                addr_cmd_drv: na_cell(),
                cs_odt_drv: na_cell(),
                cke_drv: na_cell(),
            },
            voltages: VoltageSet {
                vddcr_soc_mv: na_cell(),
                vddio_mem_mv: na_cell(),
                vdd_misc_mv: na_cell(),
                vpp_mv: na_cell(),
            },
        }
    }

    /// (f) P3-03: a representative `AmdReadout` — `Value` and `Na` cells
    /// across every field type — round-trips through bincode, as does the
    /// all-`Na` degradation state (the full AMD readout is wire-safe).
    #[test]
    fn amd_readout_bincode_round_trip() {
        // values + `Na`: the good snapshot (all `Value` except the
        // AMD-not-applicable gear mode) with a clock and a CAD code pushed
        // out of band → `Na(ParseError)` / `Na(NotApplicable)`.
        let mut s = good_snapshot();
        s.mclk_mhz = 0; // zero clock → Na(ParseError)
        s.cad_bus.proc_odt = 7; // unknown code → Na(NotApplicable)
        let ro = map_amd(&s);
        assert!(ro.clocks.mclk_mhz.is_na());
        assert!(ro.cad_bus.proc_odt.is_na());
        assert!(ro.clocks.gear_mode.is_na());

        let bytes = bincode::serialize(&ro)
            .expect("AmdReadout must serialize (no-panic contract)");
        let back: AmdReadout =
            bincode::deserialize(&bytes).expect("AmdReadout must deserialize");
        assert_eq!(ro, back);

        // the fully degraded state must be wire-safe too
        let all_na = all_na_readout();
        let bytes = bincode::serialize(&all_na)
            .expect("AmdReadout must serialize (no-panic contract)");
        let back: AmdReadout =
            bincode::deserialize(&bytes).expect("AmdReadout must deserialize");
        assert_eq!(all_na, back);
    }

    /// (g) P3-03: every [`RttValue`] arm survives a bincode round-trip —
    /// `Disabled`, `Rzq(code)`, `Ohms(value)` — both bare and as the
    /// `Section` cells the CAD bus fields use.
    #[test]
    fn rtt_value_arms_round_trip() {
        let values = vec![
            RttValue::Disabled,
            RttValue::Rzq(2),
            RttValue::Ohms(33.5),
        ];
        let bytes = bincode::serialize(&values)
            .expect("RttValue must serialize (no-panic contract)");
        let back: Vec<RttValue> =
            bincode::deserialize(&bytes).expect("RttValue must deserialize");
        assert_eq!(values, back);

        let sections: Vec<Section<RttValue>> = vec![
            Section::Value(RttValue::Disabled),
            Section::Value(RttValue::Rzq(10)),
            Section::Value(RttValue::Ohms(45.0)),
            Section::na(NaReason::NotApplicable),
        ];
        let bytes = bincode::serialize(&sections)
            .expect("Section<RttValue> must serialize (no-panic contract)");
        let back: Vec<Section<RttValue>> =
            bincode::deserialize(&bytes).expect("Section<RttValue> must deserialize");
        assert_eq!(sections, back);
    }

    /// (g′) C6-03: every [`CommandRate`] arm survives a bincode round-trip,
    /// both bare and as the `Section` cells the clock readout uses.
    #[test]
    fn command_rate_arms_round_trip() {
        let values = vec![CommandRate::OneT, CommandRate::TwoT];
        let bytes = bincode::serialize(&values)
            .expect("CommandRate must serialize (no-panic contract)");
        let back: Vec<CommandRate> =
            bincode::deserialize(&bytes).expect("CommandRate must deserialize");
        assert_eq!(values, back);

        let sections: Vec<Section<CommandRate>> = vec![
            Section::Value(CommandRate::OneT),
            Section::Value(CommandRate::TwoT),
            Section::na(NaReason::NotApplicable),
            Section::na(NaReason::ParseError("reserved encoding".to_owned())),
        ];
        let bytes = bincode::serialize(&sections)
            .expect("Section<CommandRate> must serialize (no-panic contract)");
        let back: Vec<Section<CommandRate>> =
            bincode::deserialize(&bytes).expect("Section<CommandRate> must deserialize");
        assert_eq!(sections, back);
    }
}
