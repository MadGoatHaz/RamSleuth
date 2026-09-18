//! AMD SMU PM-table parse — version-guarded, bounds-checked (P2-04).
//!
//! Turns the raw PM-table blob acquired by [`crate::amd_smu`] (P2-03) into
//! a structured [`AmdPmSnapshot`]:
//!
//! - **Clocks** — MCLK / UCLK / FCLK (MHz) and the UCLK:MCLK divide mode
//!   (derived: `UCLK == MCLK` → 1:1 coupled, otherwise 1:2);
//! - **Voltage** — VDDCR_SOC and Vcore / VDDCR_VDD (mV, converted from
//!   the table's volt unit).
//!
//! The remaining fields of the frozen snapshot shape (GDM / PDM /
//! command-rate mode flags, the 27 DRAM subtimings, the 8 CAD-bus codes,
//! VDDIO_MEM / VDD_MISC / VPP) are **not present in this table** on the
//! verified driver model (P5-15): [`parse`] sets them to `0`, which
//! degrades to an honest `Disabled` / `N/A` under P2-05's sanity gates
//! (code `0` → `Disabled`/`NotApplicable`, `0` ticks/mV → out-of-band
//! Na). Display mapping (code→Ω, mV→V, ratio computation, sanity
//! ranges) belongs to P2-05 (`amd_readout.rs`).
//!
//! # Version guard (P5-15 reconciliation)
//!
//! [`SmuContext::version`] is the PM table's **`TableVersionId`** — the
//! little-endian `u32` the driver publishes in the sibling
//! `pm_table_version` sysfs attribute (P2-03). The blob itself is a
//! **headerless** array of little-endian `f32` values (byte offset =
//! index × 4) and carries no version word of its own.
//! [`PmLayout::from_version`] guards on the exact accepted
//! `TableVersionId` sets:
//!
//! | `TableVersionId` set                 | layout              | family          |
//! |--------------------------------------|---------------------|-----------------|
//! | [`VERMEER_TABLE_VERSIONS`] (10 ids)  | [`PmLayout::Vermeer`] | Zen 3 (5950X) |
//! | [`MATISSE_TABLE_VERSIONS`] (8 ids)   | [`PmLayout::Matisse`] | Zen 2         |
//!
//! Any other version word yields [`TelemetryError::UnknownPmTableVersion`].
//!
//! # Field layout (f32 offsets, verified on 5950X silicon via monitor_cpu)
//!
//! ```text
//! 0x0A0   VDDCR_VDD (Vcore)  f32 LE   (volts; ×1000 → mV)
//! 0x0B0   VDDCR_SOC          f32 LE   (volts; ×1000 → mV)
//! 0x0C0   FCLK               f32 LE   (MHz)
//! 0x0C8   UCLK               f32 LE   (MHz)
//! 0x0CC   MCLK               f32 LE   (MHz)
//! ```
//!
//! Minimum blob length [`MIN_LEN`] = 0x518 (326 × f32). Every field read
//! goes through the bounds-checked [`read_f32le`]; a non-finite float
//! (NaN / ±inf) or a negative value degrades to the field's zero value
//! instead of poisoning the snapshot — no code path can panic or read out
//! of bounds.
//!
//! # Safety
//!
//! No `unsafe`: every field read goes through [`read_f32le`], which
//! returns [`TelemetryError::Parse`] when `off + 4 > blob.len()` (it
//! never indexes and never panics). [`parse`] gates on the minimum blob
//! length before reading fields.

use crate::amd_smu::SmuContext;
use crate::error::{TelemetryError, TelemetryResult};

/// Byte offset of the Vcore / VDDCR_VDD voltage (volts) — little-endian
/// f32. The measured CPU core rail (C12 — 0x0A0 is the telemetry slot;
/// 0x09C is the setpoint and is not read).
const VDDCR_VDD_OFF: usize = 0x0A0;

/// Byte offset of the VDDCR_SOC voltage (volts) — little-endian f32.
const VDDCR_SOC_OFF: usize = 0x0B0;

/// Byte offset of the FCLK frequency (MHz) — little-endian f32.
const FCLK_OFF: usize = 0x0C0;

/// Byte offset of the UCLK frequency (MHz) — little-endian f32.
const UCLK_OFF: usize = 0x0C8;

/// Byte offset of the MCLK frequency (MHz) — little-endian f32.
const MCLK_OFF: usize = 0x0CC;

/// Minimum PM-table blob length in bytes: 326 × f32 (0x518).
const MIN_LEN: usize = 0x518;

/// Vermeer / Zen 3 accepted `TableVersionId`s (exact set; verified on
/// 5950X silicon against the amkillam/ryzen_smu driver model — the live
/// driver reports 0x380805).
pub const VERMEER_TABLE_VERSIONS: [u32; 10] = [
    0x2D_08_03, 0x2D_09_03, 0x38_00_05, 0x38_05_05, 0x38_06_05, 0x38_07_05,
    0x38_08_04, 0x38_08_05, 0x38_09_04, 0x38_09_05,
];

/// Matisse / Zen 2 accepted `TableVersionId`s (shares Vermeer's f32
/// layout).
pub const MATISSE_TABLE_VERSIONS: [u32; 8] = [
    0x24_00_03, 0x24_05_03, 0x24_06_03, 0x24_07_03,
    0x24_08_02, 0x24_08_03, 0x24_09_02, 0x24_09_03,
];

/// One supported PM-table family (P5-15: exact `TableVersionId` sets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmLayout {
    /// Vermeer / Zen 3 (e.g. Ryzen 9 5950X, the test host) — the 10
    /// accepted `TableVersionId`s in [`VERMEER_TABLE_VERSIONS`].
    Vermeer,
    /// Matisse / Zen 2 — the 8 accepted `TableVersionId`s in
    /// [`MATISSE_TABLE_VERSIONS`]; same f32 layout as Vermeer.
    Matisse,
}

impl PmLayout {
    /// Maps a raw `TableVersionId` to its PM-table family.
    ///
    /// The version is the driver's sibling `pm_table_version` value
    /// (P2-03/P5-15), not a word inside the blob. Each accepted id maps
    /// to its family; anything else (including the legacy 7.11.x / 12.x /
    /// 13.x major.minor encodings the P2-04 skeleton once matched) yields
    /// [`TelemetryError::UnknownPmTableVersion`] with the id preserved.
    pub fn from_version(version: u32) -> TelemetryResult<Self> {
        if VERMEER_TABLE_VERSIONS.contains(&version) {
            Ok(Self::Vermeer)
        } else if MATISSE_TABLE_VERSIONS.contains(&version) {
            Ok(Self::Matisse)
        } else {
            Err(TelemetryError::UnknownPmTableVersion { version })
        }
    }

    /// Short layout name for diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vermeer => "Vermeer (Zen 3)",
            Self::Matisse => "Matisse (Zen 2)",
        }
    }
}

/// The DRAM subtimings in ticks, packed in plan order: the 19 primary
/// (tCL … tWRRD) followed by the 8 tertiary/turnaround timings
/// (tRDRD / tWRWR across same-DIMM, same-CCD, SCL, SC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdPmTimings {
    /// tCL — CAS latency.
    pub cl: u16,
    /// tRCDWR — RAS-to-CAS delay (write).
    pub rcwdwr: u16,
    /// tRCDRD — RAS-to-CAS delay (read).
    pub rcdrd: u16,
    /// tRP — row precharge.
    pub rp: u16,
    /// tRAS — active time.
    pub ras: u16,
    /// tRC — row cycle (tRAS + tRP).
    pub rc: u16,
    /// tRRDS — row-to-row (same bank group).
    pub rrds: u16,
    /// tRRDL — row-to-row (different bank group).
    pub rrld: u16,
    /// tFAW — four-activate window.
    pub faw: u16,
    /// tWTRS — write-to-read (same rank).
    pub wtrs: u16,
    /// tWTRL — write-to-read (different rank).
    pub wtrl: u16,
    /// tWR — write recovery.
    pub wr: u16,
    /// tRFC1 — refresh cycle 1.
    pub rfc1: u16,
    /// tRFC2 — refresh cycle 2.
    pub rfc2: u16,
    /// tRFCsb — refresh (same bank).
    pub rfcsb: u16,
    /// tCWL — CAS write latency.
    pub cwl: u16,
    /// tRTP — read-to-precharge.
    pub rtp: u16,
    /// tRDWR — read-to-write.
    pub rdwr: u16,
    /// tWRRD — write-to-read turnaround.
    pub wrrd: u16,
    /// tRDRD — read-to-read, same DIMM.
    pub rdrd_sd: u16,
    /// tRDRD — read-to-read, same CCD.
    pub rdrd_dd: u16,
    /// tRDRD — read-to-read, via SCL.
    pub rdrd_scl: u16,
    /// tRDRD — read-to-read, via SC.
    pub rdrd_sc: u16,
    /// tWRWR — write-to-write, same DIMM.
    pub wrwr_sd: u16,
    /// tWRWR — write-to-write, same CCD.
    pub wrwr_dd: u16,
    /// tWRWR — write-to-write, via SCL.
    pub wrwr_scl: u16,
    /// tWRWR — write-to-write, via SC.
    pub wrwr_sc: u16,
}

/// CAD (command/address/data) bus drive strengths and resistances as raw
/// RZQ/driver codes, packed in plan order. P2-05 maps these to ohms via the
/// RZQ = 240 Ω base code table (this snapshot holds the raw codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdPmCadBus {
    /// Processor ODT code (disabled / RZQ/… code).
    pub proc_odt: u16,
    /// RttNom code (0 = disabled).
    pub rtt_nom: u16,
    /// RttWr code (0 = disabled).
    pub rtt_wr: u16,
    /// RttPark code (0 = disabled).
    pub rtt_park: u16,
    /// Clock driver strength code.
    pub clk_drv: u16,
    /// Address/command driver strength code.
    pub addr_cmd_drv: u16,
    /// CS/ODT driver strength code.
    pub cs_odt_drv: u16,
    /// CKE driver strength code.
    pub cke_drv: u16,
}

/// Memory/SOC rail voltages in raw millivolts, packed in plan order. P2-05
/// converts mV → V for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdPmVoltages {
    /// VDDCR_SOC in mV.
    pub vddcr_soc_mv: u16,
    /// VDDIO_MEM in mV.
    pub vddio_mem_mv: u16,
    /// VDD_MISC in mV.
    pub vdd_misc_mv: u16,
    /// VPP in mV.
    pub vpp_mv: u16,
    /// Vcore / VDDCR_VDD in mV (PM table 0x0A0, f32 volts × 1000 — C12;
    /// board-agnostic on any AM4 PM layout we accept).
    pub vcore_mv: u16,
}

/// A structured, version-guarded parse of the AMD SMU PM table (frozen
/// public API, P2-04).
///
/// In-table values are raw PM-table units (MHz, mV, derived div mode);
/// fields the table does not carry are zeroed and degrade to honest Na /
/// Disabled under P2-05's sanity gates. Built by [`parse`] from a
/// [`SmuContext`]: every field is populated, or the call returns `Err` —
/// never a partially populated snapshot, never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdPmSnapshot {
    /// The PM-table `TableVersionId` this snapshot was parsed from
    /// ([`SmuContext::version`]).
    pub version: u32,
    /// Memory clock (MCLK) in MHz.
    pub mclk_mhz: u16,
    /// Memory controller clock (UCLK) in MHz.
    pub uclk_mhz: u16,
    /// Infinity Fabric clock (FCLK) in MHz.
    pub fclk_mhz: u16,
    /// UCLK:MCLK divide mode: `0` = 1:1, `1` = 1:2.
    pub div_mode: u8,
    /// Gear Down Mode: `0` = off, `1` = on.
    pub gdm: u8,
    /// Power Down Mode: `0` = off, `1` = on.
    pub pdm: u8,
    /// DRAM command rate: `0` = 1T, `1` = 2T.
    pub command_rate: u8,
    /// DRAM subtimings in ticks (19 primary + 8 tertiary).
    pub timings: AmdPmTimings,
    /// CAD-bus raw RZQ/driver codes.
    pub cad_bus: AmdPmCadBus,
    /// Rail voltages in mV.
    pub voltages: AmdPmVoltages,
}

/// Shared truncation diagnostic for the bounds-checked readers.
fn truncated(off: usize, width: usize, len: usize) -> TelemetryError {
    TelemetryError::Parse {
        detail: format!(
            "PM blob truncated: read {width} byte(s) at offset {off:#06x} but the blob is only {len} byte(s)"
        ),
    }
}

/// Reads one little-endian IEEE-754 `f32` from the blob at `off`.
///
/// Bounds-checked: returns [`TelemetryError::Parse`] when `off + 4 > len` —
/// never indexes, never panics.
fn read_f32le(blob: &[u8], off: usize) -> TelemetryResult<f32> {
    match blob.get(off..off.saturating_add(4)) {
        Some(s) => {
            let b = [s[0], s[1], s[2], s[3]];
            Ok(f32::from_bits(u32::from_le_bytes(b)))
        }
        None => Err(truncated(off, 4, blob.len())),
    }
}

/// Converts a finite, non-negative `f32` to a saturating `u16` field
/// value (MHz or mV).
///
/// Non-finite (NaN / ±inf) and negative inputs degrade to `0` — under
/// P2-05's gates `0` renders as an honest `Disabled` / out-of-band Na.
/// Oversized values saturate at `u16::MAX`. The `f32 -> u32` cast
/// saturates by construction, so no path can trap or panic.
fn to_u16(v: f32) -> u16 {
    if !v.is_finite() || v < 0.0 {
        return 0;
    }
    let rounded = v.round() as u32;
    u16::try_from(rounded).unwrap_or(u16::MAX)
}

/// Parses the raw PM-table blob in `ctx` into a structured
/// [`AmdPmSnapshot`] (frozen public API, P2-04).
///
/// Version-guarded flow (P5-15):
///
/// 1. [`PmLayout::from_version`] maps the `TableVersionId` (the sibling
///    `pm_table_version` value, not a word inside the blob) to a family;
///    an unrecognized id → [`TelemetryError::UnknownPmTableVersion`].
/// 2. The blob (a headerless f32 array) must be at least [`MIN_LEN`]
///    bytes (shorter → [`TelemetryError::Parse`]); every field read is
///    additionally bounds-checked via [`read_f32le`], so the parse is
///    safe even if an offset constant is edited later.
///
/// Fields: FCLK / UCLK / MCLK (MHz, f32 at [`FCLK_OFF`] / [`UCLK_OFF`] /
/// [`MCLK_OFF`], rounded to integer MHz) and VDDCR_SOC / Vcore (volts,
/// f32 at [`VDDCR_SOC_OFF`] / [`VDDCR_VDD_OFF`], ×1000 → mV). The
/// divide mode is derived (`UCLK == MCLK` → 1:1, else 1:2). GDM / PDM /
/// command rate, the 27 timings, the 8 CAD codes, and the other three
/// voltages are not present in this table → `0` (honest Na / Disabled
/// under the P2-05 gates).
///
/// Pure: no I/O, no `unsafe`, no panic on any input (non-finite floats
/// and negative values degrade to `0`).
pub fn parse(ctx: &SmuContext) -> TelemetryResult<AmdPmSnapshot> {
    let layout = PmLayout::from_version(ctx.version)?;
    let pm = &ctx.pm;

    // Minimum-length gate: the blob is a headerless f32 array — there is
    // no version word inside it to cross-check (P5-15).
    if pm.len() < MIN_LEN {
        return Err(TelemetryError::Parse {
            detail: format!(
                "PM blob too short for {} layout: {} byte(s), need at least {MIN_LEN} (0x{MIN_LEN:x})",
                layout.name(),
                pm.len()
            ),
        });
    }

    let vddcr_vdd_v = read_f32le(pm, VDDCR_VDD_OFF)?;
    let vddcr_soc_v = read_f32le(pm, VDDCR_SOC_OFF)?;
    let fclk = read_f32le(pm, FCLK_OFF)?;
    let uclk = read_f32le(pm, UCLK_OFF)?;
    let mclk = read_f32le(pm, MCLK_OFF)?;

    Ok(AmdPmSnapshot {
        version: ctx.version,
        mclk_mhz: to_u16(mclk),
        uclk_mhz: to_u16(uclk),
        fclk_mhz: to_u16(fclk),
        // DivMode is derived, not stored: coupled (1:1) when UCLK ==
        // MCLK, 1:2 otherwise.
        div_mode: u8::from(uclk != mclk),
        // GDM / PDM / command rate are not present in this table (SMN
        // overlay slots, like `gdm`): `0` degrades to an honest
        // "Disabled" / 1T under the P2-05 gates.
        gdm: 0,
        pdm: 0,
        command_rate: 0,
        // The 27 DRAM subtimings are not present in this table: `0`
        // ticks degrade to an honest Na under the P2-05 timing gate.
        timings: AmdPmTimings {
            cl: 0,
            rcwdwr: 0,
            rcdrd: 0,
            rp: 0,
            ras: 0,
            rc: 0,
            rrds: 0,
            rrld: 0,
            faw: 0,
            wtrs: 0,
            wtrl: 0,
            wr: 0,
            rfc1: 0,
            rfc2: 0,
            rfcsb: 0,
            cwl: 0,
            rtp: 0,
            rdwr: 0,
            wrrd: 0,
            rdrd_sd: 0,
            rdrd_dd: 0,
            rdrd_scl: 0,
            rdrd_sc: 0,
            wrwr_sd: 0,
            wrwr_dd: 0,
            wrwr_scl: 0,
            wrwr_sc: 0,
        },
        // The 8 CAD-bus codes are not present in this table: `0`
        // degrades to Disabled / NotApplicable under the P2-05 CAD gates.
        cad_bus: AmdPmCadBus {
            proc_odt: 0,
            rtt_nom: 0,
            rtt_wr: 0,
            rtt_park: 0,
            clk_drv: 0,
            addr_cmd_drv: 0,
            cs_odt_drv: 0,
            cke_drv: 0,
        },
        voltages: AmdPmVoltages {
            vddcr_soc_mv: to_u16(vddcr_soc_v * 1000.0),
            // VDDIO_MEM / VDD_MISC / VPP are not present in this table:
            // `0` mV degrades to an honest Na under the P2-05 voltage gate.
            vddio_mem_mv: 0,
            vdd_misc_mv: 0,
            vpp_mv: 0,
            // Vcore (VDDCR_VDD, 0x0A0) is in the table: non-finite /
            // negative readings degrade to `0` mV → honest Na under the
            // P2-05 voltage gate (no new failure mode).
            vcore_mv: to_u16(vddcr_vdd_v * 1000.0),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative Vermeer / Zen 3 `TableVersionId` (the live driver
    /// on the test host reports 0x380805).
    const V_VERMEER: u32 = 0x38_08_05;
    /// A representative Matisse / Zen 2 `TableVersionId`.
    const V_MATISSE: u32 = 0x24_09_03;

    fn write_f32le(buf: &mut [u8], off: usize, v: f32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Builds a blob of exactly [`MIN_LEN`] with the given f32 values
    /// written at the field offsets `parse` reads.
    fn synthetic_blob(fclk: f32, uclk: f32, mclk: f32, vddcr_soc_v: f32, vcore_v: f32) -> Vec<u8> {
        let mut blob = vec![0u8; MIN_LEN];
        write_f32le(&mut blob, VDDCR_VDD_OFF, vcore_v);
        write_f32le(&mut blob, VDDCR_SOC_OFF, vddcr_soc_v);
        write_f32le(&mut blob, FCLK_OFF, fclk);
        write_f32le(&mut blob, UCLK_OFF, uclk);
        write_f32le(&mut blob, MCLK_OFF, mclk);
        blob
    }

    /// The snapshot a synthetic blob must parse to: in-table fields
    /// populated, every not-in-table field zeroed.
    fn known_snapshot(
        version: u32,
        mclk_mhz: u16,
        uclk_mhz: u16,
        fclk_mhz: u16,
        div_mode: u8,
        vddcr_soc_mv: u16,
        vcore_mv: u16,
    ) -> AmdPmSnapshot {
        AmdPmSnapshot {
            version,
            mclk_mhz,
            uclk_mhz,
            fclk_mhz,
            div_mode,
            gdm: 0,
            pdm: 0,
            command_rate: 0,
            timings: AmdPmTimings {
                cl: 0,
                rcwdwr: 0,
                rcdrd: 0,
                rp: 0,
                ras: 0,
                rc: 0,
                rrds: 0,
                rrld: 0,
                faw: 0,
                wtrs: 0,
                wtrl: 0,
                wr: 0,
                rfc1: 0,
                rfc2: 0,
                rfcsb: 0,
                cwl: 0,
                rtp: 0,
                rdwr: 0,
                wrrd: 0,
                rdrd_sd: 0,
                rdrd_dd: 0,
                rdrd_scl: 0,
                rdrd_sc: 0,
                wrwr_sd: 0,
                wrwr_dd: 0,
                wrwr_scl: 0,
                wrwr_sc: 0,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 0,
                rtt_nom: 0,
                rtt_wr: 0,
                rtt_park: 0,
                clk_drv: 0,
                addr_cmd_drv: 0,
                cs_odt_drv: 0,
                cke_drv: 0,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv,
                vddio_mem_mv: 0,
                vdd_misc_mv: 0,
                vpp_mv: 0,
                vcore_mv,
            },
        }
    }

    /// (a) Every accepted Vermeer `TableVersionId` maps to
    /// [`PmLayout::Vermeer`].
    #[test]
    fn from_version_accepts_every_vermeer_id() {
        for v in VERMEER_TABLE_VERSIONS {
            assert_eq!(
                PmLayout::from_version(v),
                Ok(PmLayout::Vermeer),
                "{v:#010x}"
            );
        }
    }

    /// (a) Every accepted Matisse `TableVersionId` maps to
    /// [`PmLayout::Matisse`].
    #[test]
    fn from_version_accepts_every_matisse_id() {
        for v in MATISSE_TABLE_VERSIONS {
            assert_eq!(
                PmLayout::from_version(v),
                Ok(PmLayout::Matisse),
                "{v:#010x}"
            );
        }
    }

    /// (c) Out-of-set version words — including the legacy 7.11.x / 12.x /
    /// 13.x major.minor encodings the P2-04 skeleton once matched — yield
    /// `Err(UnknownPmTableVersion { version })` with the id preserved.
    #[test]
    fn from_version_rejects_out_of_set_ids() {
        for version in [
            0x0007_0B02u32, // legacy 7.11.2 major.minor encoding
            0x000C_0001,    // legacy 12.0.1
            0x000D_0001,    // legacy 13.0.1
            0x38_00_06,      // near-miss (not in the Vermeer set)
            0x24_08_04,      // near-miss (not in the Matisse set)
            0x0000_0000,
            0xFFFF_FFFF,
        ] {
            assert_eq!(
                PmLayout::from_version(version),
                Err(TelemetryError::UnknownPmTableVersion { version }),
                "{version:#010x}"
            );
        }
    }

    /// (d) A known Vermeer blob parses to exactly the in-table fields:
    /// rounded MHz clocks, derived 1:1 div mode, VDDCR_SOC volts → mV, and
    /// every not-in-table field zeroed.
    #[test]
    fn parse_known_vermeer_blob() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(1800.0, 1800.0, 1800.0, 1.05, 1.35),
        };
        assert_eq!(
            parse(&ctx),
            Ok(known_snapshot(V_VERMEER, 1800, 1800, 1800, 0, 1050, 1350))
        );
    }

    /// (d) UCLK ≠ MCLK derives the 1:2 div mode (fractional MHz values
    /// round to the nearest integer; volts ×1000 round to mV).
    #[test]
    fn parse_div_mode_derived_1to2() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(1792.8, 800.4, 1600.6, 0.95, 1.2),
        };
        // fclk 1792.8 → 1793, uclk 800.4 → 800, mclk 1600.6 → 1601,
        // 0.95 V → 950 mV, 1.2 V → 1200 mV (Vcore), div mode 1:2.
        assert_eq!(
            parse(&ctx),
            Ok(known_snapshot(V_VERMEER, 1601, 800, 1793, 1, 950, 1200))
        );
    }

    /// (d) A known Matisse blob parses with the same f32 layout.
    #[test]
    fn parse_known_matisse_blob() {
        let ctx = SmuContext {
            version: V_MATISSE,
            pm: synthetic_blob(1200.0, 1200.0, 1200.0, 0.9, 1.1),
        };
        assert_eq!(
            parse(&ctx),
            Ok(known_snapshot(V_MATISSE, 1200, 1200, 1200, 0, 900, 1100))
        );
    }

    /// (d) The `command_rate` raw slot (mirroring the `gdm` / `pdm`
    /// pattern): `parse` leaves it `0` — it is an SMN field (`0x50200`
    /// bit 10), not a PM-table field — and the snapshot carries it.
    #[test]
    fn parse_zeroes_command_rate_like_gdm_and_pdm() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(1800.0, 1800.0, 1800.0, 1.05, 1.05),
        };
        let snap = parse(&ctx).expect("synthetic blob must parse");
        assert_eq!(snap.command_rate, 0);
        assert_eq!(snap.gdm, 0);
        assert_eq!(snap.pdm, 0);
    }

    /// (e) Non-finite floats (NaN / ±inf) at any field offset degrade to
    /// the zero value — no panic, no poisoning of the snapshot.
    #[test]
    fn non_finite_fields_degrade_to_zero() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(f32::NAN, f32::INFINITY, f32::INFINITY, f32::NAN, f32::NAN),
        };
        // uclk == mclk (both +inf) -> derived 1:1; everything else 0.
        assert_eq!(parse(&ctx), Ok(known_snapshot(V_VERMEER, 0, 0, 0, 0, 0, 0)));
    }

    /// (e) Negative (but finite) floats degrade to the zero value.
    #[test]
    fn negative_fields_degrade_to_zero() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(-1.0, -2.0, -4.0, -0.5, -1.5),
        };
        // -2.0 != -4.0 -> derived 1:2.
        assert_eq!(parse(&ctx), Ok(known_snapshot(V_VERMEER, 0, 0, 0, 1, 0, 0)));
    }

    /// (b) Truncated blobs — every length from 0 up to one byte short of
    /// [`MIN_LEN`] — yield `Err(Parse)`; no panic, no out-of-bounds read.
    #[test]
    fn truncated_blobs_yield_parse_error_not_panic() {
        let full = synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, 1.2);
        assert_eq!(full.len(), MIN_LEN);
        for len in 0..MIN_LEN {
            let ctx = SmuContext {
                version: V_VERMEER,
                pm: full[..len].to_vec(),
            };
            let res = parse(&ctx);
            assert!(
                matches!(res, Err(TelemetryError::Parse { .. })),
                "len {len} must be Err(Parse), got {res:?}"
            );
        }
    }

    /// (b) A blob longer than [`MIN_LEN`] (trailing data present) still
    /// parses — the gate is a minimum length, not an exact match.
    #[test]
    fn oversized_blob_still_parses() {
        let mut blob = synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, 1.2);
        blob.extend_from_slice(&[0u8; 16]);
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: blob,
        };
        assert_eq!(
            parse(&ctx),
            Ok(known_snapshot(V_VERMEER, 1600, 1600, 1600, 0, 1000, 1200))
        );
    }

    /// (d) Vcore (PM 0x0A0): f32 volts × 1000 → mV, the same conversion
    /// path as VDDCR_SOC; a 0.0 V reading → 0 mV (an honest Na under
    /// the P2-05 voltage gate downstream).
    #[test]
    fn parse_vcore_volts_to_mv() {
        let cases: [(f32, u16); 2] = [(0.75, 750), (1.35, 1350)];
        for (vcore_v, expected_mv) in cases {
            let ctx = SmuContext {
                version: V_VERMEER,
                pm: synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, vcore_v),
            };
            let snap = parse(&ctx).expect("synthetic blob must parse");
            assert_eq!(snap.voltages.vcore_mv, expected_mv);
        }
        // 0.0 V → 0 mV → out of band → Na downstream.
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, 0.0),
        };
        assert_eq!(
            parse(&ctx).expect("synthetic blob must parse").voltages.vcore_mv,
            0
        );
    }

    /// (e) `read_f32le`: little-endian IEEE-754 decode in bounds;
    /// `Err(Parse)` when off + 4 > len (including off == len).
    #[test]
    fn read_f32le_bounds() {
        // 1.5f32 = 0x3FC00000 -> LE bytes 00 00 C0 3F
        assert_eq!(read_f32le(&[0x00, 0x00, 0xC0, 0x3F], 0), Ok(1.5));
        // 2.0f32 = 0x40000000 at offset 1
        let mut buf = [0u8; 6];
        buf[1..5].copy_from_slice(&2.0f32.to_le_bytes());
        assert_eq!(read_f32le(&buf, 1), Ok(2.0));
        for (blob, off) in [
            (&[0x00u8, 0x00u8, 0xC0u8][..], 0usize), // 3 < 4
            (
                &[0x00u8, 0x00u8, 0xC0u8, 0x3Fu8][..],
                1, // off + 4 = 5 > 4
            ),
            (&[0x00u8, 0x00u8, 0xC0u8, 0x3Fu8][..], 4), // off == len
            (&[][..], 0),
        ] {
            assert!(
                matches!(read_f32le(blob, off), Err(TelemetryError::Parse { .. })),
                "blob {blob:?} off {off}"
            );
        }
    }

    /// (e) `to_u16`: rounds finite non-negative values, saturates
    /// oversized values at `u16::MAX`, and degrades non-finite / negative
    /// values to `0` (never a panic).
    #[test]
    fn to_u16_bounds_and_graceful_degradation() {
        assert_eq!(to_u16(1600.0), 1600);
        assert_eq!(to_u16(949.4), 949);
        assert_eq!(to_u16(949.6), 950);
        assert_eq!(to_u16(0.0), 0);
        assert_eq!(to_u16(65535.0), 65535);
        assert_eq!(to_u16(70000.0), 65535); // saturate
        assert_eq!(to_u16(1.0e30), 65535); // saturate, no panic
        assert_eq!(to_u16(f32::NAN), 0);
        assert_eq!(to_u16(f32::INFINITY), 0);
        assert_eq!(to_u16(f32::NEG_INFINITY), 0);
        assert_eq!(to_u16(-5.0), 0);
        assert_eq!(to_u16(-0.0), 0);
    }
}
