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
//! # Version guard (P5-15 reconciliation, generation-first — TEL-2)
//!
//! [`SmuContext::version`] is the PM table's **`TableVersionId`** — the
//! little-endian `u32` the driver publishes in the sibling
//! `pm_table_version` sysfs attribute (P2-03). The blob itself is a
//! **headerless** array of little-endian `f32` values (byte offset =
//! index × 4) and carries no version word of its own.
//!
//! The decode is **generation-first** (mirroring
//! [`crate::intel_gen::profile_for`]): [`PmLayout::from_zen`] keys on the
//! CPUID-detected [`crate::cpuid::AmdZen`] family as the primary dispatch,
//! and treats the `TableVersionId` as a **per-generation plausibility
//! filter** — not a global closed allowlist. The verified generations
//! (Matisse / Zen 2, Vermeer / Zen 3) carry a major-prefix whitelist
//! ([`MATISSE_TABLE_VERSIONS`] / [`VERMEER_TABLE_VERSIONS`]); the
//! unverified generations (Zen 1 / 4 / 5) accept any non-degenerate
//! version word, so a Zen 4 (`0x1A`) / Zen 5 (`0x1C`) desktop reaches a
//! **live** decode path instead of degrading to whole-section N/A. Only a
//! genuinely unrecognized word — a zero sentinel (the legacy all-zero-blob
//! fallback), or a verified generation whose major prefix mismatches its
//! CPUID family — yields [`TelemetryError::UnknownPmTableVersion`].
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
use crate::cpuid::AmdZen;
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

/// One PM-table layout, keyed by AMD Zen generation (generation-first —
/// TEL-2; mirrors [`crate::intel_gen::profile_for`]).
///
/// The CPUID-detected [`crate::cpuid::AmdZen`] family is the primary
/// dispatch; the `TableVersionId` is a per-generation plausibility filter,
/// not a global closed allowlist. All five generations share one f32 field
/// layout: the verified generations (Matisse / Zen 2, Vermeer / Zen 3)
/// were confirmed on silicon, and the unverified generations (Zen 1 / 4 /
/// 5) reuse that layout as their live decode path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmLayout {
    /// Zen 1 (family `0x15`) — no verified AM4 PM layout; the conservative
    /// live path (unverified).
    Zen1,
    /// Matisse / Zen 2 (family `0x17`) — the 8 verified `TableVersionId`s in
    /// [`MATISSE_TABLE_VERSIONS`]; same f32 layout as Vermeer.
    Matisse,
    /// Vermeer / Zen 3 (family `0x19`; e.g. the Ryzen 9 5950X reference host)
    /// — the 10 verified `TableVersionId`s in [`VERMEER_TABLE_VERSIONS`].
    Vermeer,
    /// Zen 4 (family `0x1A`) — the live decode path (unverified; the
    /// CPUID family is the dispatch key, not a closed version set).
    Zen4,
    /// Zen 5 (family `0x1C`) — the live decode path (unverified; the
    /// CPUID family is the dispatch key, not a closed version set).
    Zen5,
}

impl PmLayout {
    /// Generation-first dispatch (TEL-2, mirrors
    /// [`crate::intel_gen::profile_for`]): the CPUID-detected
    /// [`crate::cpuid::AmdZen`] family is the primary key; the
    /// `TableVersionId` is a **per-generation plausibility filter**, not a
    /// global closed allowlist.
    ///
    /// A zero version word (the "no version published" sentinel from the
    /// legacy all-zero-blob fallback) is rejected for every generation.
    /// Verified generations (Matisse / Vermeer) additionally require the
    /// version's major byte to match the generation's known prefix — a
    /// mismatch (e.g. a Zen 3 family reporting a Zen 2 major) is genuinely
    /// unrecognized firmware → [`TelemetryError::UnknownPmTableVersion`].
    /// Unverified generations (Zen 1 / 4 / 5) accept any non-zero version
    /// word, so Zen 4 / Zen 5 desktops reach a live decode path.
    pub fn from_zen(zen: AmdZen, version: u32) -> TelemetryResult<Self> {
        if version == 0 {
            return Err(TelemetryError::UnknownPmTableVersion { version });
        }
        let layout = match zen {
            AmdZen::Zen1 => Self::Zen1,
            AmdZen::Zen2 => Self::Matisse,
            AmdZen::Zen3 => Self::Vermeer,
            AmdZen::Zen4 => Self::Zen4,
            AmdZen::Zen5 => Self::Zen5,
        };
        if !layout.version_is_plausible(version) {
            return Err(TelemetryError::UnknownPmTableVersion { version });
        }
        Ok(layout)
    }

    /// The per-generation `TableVersionId` plausibility filter (TEL-2).
    ///
    /// Verified generations key on the version's major byte (bits 23:16 —
    /// the `0xXX_YY_ZZ` encodings are 24-bit, i.e. `0x00XX_YYZZ`, so the
    /// family prefix is the second byte from the top): Matisse (Zen 2) =
    /// `0x24`; Vermeer (Zen 3) = `0x2D` / `0x38` (the prefixes of
    /// [`MATISSE_TABLE_VERSIONS`] / [`VERMEER_TABLE_VERSIONS`]).
    /// Unverified generations (Zen 1 / 4 / 5) accept any non-zero version
    /// word — the CPUID family is the authoritative dispatch key, and no
    /// closed version set is verified for them.
    fn version_is_plausible(self, version: u32) -> bool {
        if version == 0 {
            return false;
        }
        let major = (version >> 16) & 0xFF;
        match self {
            Self::Matisse => major == 0x24,
            Self::Vermeer => matches!(major, 0x2D | 0x38),
            Self::Zen1 | Self::Zen4 | Self::Zen5 => true,
        }
    }

    /// Short layout name for diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Zen1 => "Zen 1",
            Self::Matisse => "Matisse (Zen 2)",
            Self::Vermeer => "Vermeer (Zen 3)",
            Self::Zen4 => "Zen 4",
            Self::Zen5 => "Zen 5",
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
/// **PM-table-unavailable short-circuit:** when `ctx.pm` is `None` (the
/// driver is loaded but the `pm_table` attribute is not exposed — e.g.
/// Castle Peak / Threadripper 2990X), the function returns an all-zero
/// [`AmdPmSnapshot`] (every field degrades to an honest N/A / Disabled
/// under the P2-05 gates) **before** the version gate runs — with no table
/// the version is a `0` sentinel that [`PmLayout::from_zen`] would reject,
/// so the gate must not run in this case. The SMN overlay (timings / GDM /
/// channel registers) still populates its own slots downstream, so the
/// resulting readout is not all-N/A.
///
/// Generation-first flow (P5-15, TEL-2) for a present blob:
///
/// 1. [`PmLayout::from_zen`] dispatches on the CPUID [`AmdZen`] family
///    (the primary key, mirroring
///    [`crate::intel_gen::profile_for`]) and validates the `TableVersionId`
///    against the generation's plausibility filter; a genuinely
///    unrecognized word → [`TelemetryError::UnknownPmTableVersion`].
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
pub fn parse(ctx: &SmuContext, zen: AmdZen) -> TelemetryResult<AmdPmSnapshot> {
    // PM-table-unavailable short-circuit (see the function docs): an all-zero
    // snapshot returned before the version gate, so a `0` version sentinel
    // is never rejected here.
    let Some(pm) = &ctx.pm else {
        return Ok(all_zero_snapshot(ctx.version));
    };

    let layout = PmLayout::from_zen(zen, ctx.version)?;

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

/// An all-zero [`AmdPmSnapshot`]: every field degrades to an honest
/// `N/A` / `Disabled` under the P2-05 sanity gates. Used by [`parse`] when
/// the driver is loaded but the PM table is unavailable
/// (`SmuContext::pm == None`, e.g. Castle Peak / Threadripper 2990X) — the
/// PM-derived fields are N/A while the SMN overlay (timings / GDM / channel
/// registers) still populates its own slots downstream, so the readout is
/// not all-N/A.
fn all_zero_snapshot(version: u32) -> AmdPmSnapshot {
    AmdPmSnapshot {
        version,
        mclk_mhz: 0,
        uclk_mhz: 0,
        fclk_mhz: 0,
        div_mode: 0,
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
            vddcr_soc_mv: 0,
            vddio_mem_mv: 0,
            vdd_misc_mv: 0,
            vpp_mv: 0,
            vcore_mv: 0,
        },
    }
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

    /// (a) Every verified Vermeer `TableVersionId` (major prefix `0x2D` /
    /// `0x38`) resolves to [`PmLayout::Vermeer`] under its Zen 3 family —
    /// generation-first (TEL-2).
    #[test]
    fn from_zen_vermeer_major_accepted() {
        for v in VERMEER_TABLE_VERSIONS {
            assert_eq!(
                PmLayout::from_zen(AmdZen::Zen3, v),
                Ok(PmLayout::Vermeer),
                "{v:#010x}"
            );
        }
    }

    /// (a) Every verified Matisse `TableVersionId` (major prefix `0x24`)
    /// resolves to [`PmLayout::Matisse`] under its Zen 2 family.
    #[test]
    fn from_zen_matisse_major_accepted() {
        for v in MATISSE_TABLE_VERSIONS {
            assert_eq!(
                PmLayout::from_zen(AmdZen::Zen2, v),
                Ok(PmLayout::Matisse),
                "{v:#010x}"
            );
        }
    }

    /// (TC-TEL2) The unverified generations reach a **live** decode path —
    /// the closed-allowlist bug they used to hit is gone. A Zen 4 (`0x1A`) /
    /// Zen 5 (`0x1C`) / Zen 1 (`0x15`) family with a representative
    /// `TableVersionId` (a major prefix that is in NO verified set, so the
    /// old `from_version` returned `UnknownPmTableVersion` → whole-section
    /// N/A) now resolves to its live [`PmLayout`], and the full [`parse`]
    /// succeeds end to end.
    #[test]
    fn tc_tel2_new_generations_reach_live_path() {
        // `from_zen`: each unverified generation resolves to its live layout.
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen4, 0x40_00_01),
            Ok(PmLayout::Zen4),
        );
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen5, 0x42_00_01),
            Ok(PmLayout::Zen5),
        );
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen1, 0x18_00_01),
            Ok(PmLayout::Zen1),
        );
        // A fresh major prefix (in no old closed set) is accepted for an
        // unverified generation — the pre-fix `Na(UnknownPmTableVersion)`
        // case now decodes live.
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen5, 0x99_00_00),
            Ok(PmLayout::Zen5),
        );

        // End to end: a Zen 4 / Zen 5 `SmuContext` parses to a live
        // snapshot (not `UnknownPmTableVersion`).
        for (zen, version) in [(AmdZen::Zen4, 0x40_00_01), (AmdZen::Zen5, 0x42_00_01)] {
            let ctx = SmuContext {
                version,
                pm: Some(synthetic_blob(1800.0, 1800.0, 1800.0, 1.05, 1.35)),
            };
            let snap = parse(&ctx, zen).expect("Zen 4/5 must reach a live decode (TEL-2)");
            assert_eq!(snap.version, version);
            assert_eq!(snap.mclk_mhz, 1800);
            assert_eq!(snap.voltages.vcore_mv, 1350);
        }
    }

    /// (c) A zero `TableVersionId` — the "no version published" sentinel
    /// from the legacy all-zero-blob fallback — is rejected for **every**
    /// generation (genuinely unrecognized firmware → `UnknownPmTableVersion`).
    #[test]
    fn from_zen_rejects_zero_for_all_generations() {
        for zen in [
            AmdZen::Zen1,
            AmdZen::Zen2,
            AmdZen::Zen3,
            AmdZen::Zen4,
            AmdZen::Zen5,
        ] {
            assert_eq!(
                PmLayout::from_zen(zen, 0x0000_0000),
                Err(TelemetryError::UnknownPmTableVersion { version: 0 }),
                "{zen:?}"
            );
        }
    }

    /// (c) A **verified** generation whose version major prefix belongs to a
    /// *different* generation is genuinely unrecognized firmware →
    /// `UnknownPmTableVersion` (the per-generation plausibility filter
    /// catches the cross-family mismatch the closed allowlist could not).
    #[test]
    fn from_zen_rejects_verified_major_mismatch() {
        // Zen 3 family, but a Zen 2 (`0x24`) major → mismatch.
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen3, 0x24_09_03),
            Err(TelemetryError::UnknownPmTableVersion {
                version: 0x24_09_03
            }),
        );
        // Zen 2 family, but a Zen 3 (`0x38`) major → mismatch.
        assert_eq!(
            PmLayout::from_zen(AmdZen::Zen2, 0x38_08_05),
            Err(TelemetryError::UnknownPmTableVersion {
                version: 0x38_08_05
            }),
        );
        // Legacy majors (`0x00`-encoded 7.11.x / 12.x / 13.x) on a verified
        // family → mismatch (the encodings the P2-04 skeleton once matched).
        for version in [0x0007_0B02u32, 0x000C_0001, 0x000D_0001] {
            assert_eq!(
                PmLayout::from_zen(AmdZen::Zen3, version),
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
            pm: Some(synthetic_blob(1800.0, 1800.0, 1800.0, 1.05, 1.35)),
        };
        assert_eq!(
            parse(&ctx, AmdZen::Zen3),
            Ok(known_snapshot(V_VERMEER, 1800, 1800, 1800, 0, 1050, 1350))
        );
    }

    /// (d) UCLK ≠ MCLK derives the 1:2 div mode (fractional MHz values
    /// round to the nearest integer; volts ×1000 round to mV).
    #[test]
    fn parse_div_mode_derived_1to2() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: Some(synthetic_blob(1792.8, 800.4, 1600.6, 0.95, 1.2)),
        };
        // fclk 1792.8 → 1793, uclk 800.4 → 800, mclk 1600.6 → 1601,
        // 0.95 V → 950 mV, 1.2 V → 1200 mV (Vcore), div mode 1:2.
        assert_eq!(
            parse(&ctx, AmdZen::Zen3),
            Ok(known_snapshot(V_VERMEER, 1601, 800, 1793, 1, 950, 1200))
        );
    }

    /// (d) A known Matisse blob parses with the same f32 layout.
    #[test]
    fn parse_known_matisse_blob() {
        let ctx = SmuContext {
            version: V_MATISSE,
            pm: Some(synthetic_blob(1200.0, 1200.0, 1200.0, 0.9, 1.1)),
        };
        assert_eq!(
            parse(&ctx, AmdZen::Zen2),
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
            pm: Some(synthetic_blob(1800.0, 1800.0, 1800.0, 1.05, 1.05)),
        };
        let snap = parse(&ctx, AmdZen::Zen3).expect("synthetic blob must parse");
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
            pm: Some(synthetic_blob(f32::NAN, f32::INFINITY, f32::INFINITY, f32::NAN, f32::NAN)),
        };
        // uclk == mclk (both +inf) -> derived 1:1; everything else 0.
        assert_eq!(parse(&ctx, AmdZen::Zen3), Ok(known_snapshot(V_VERMEER, 0, 0, 0, 0, 0, 0)));
    }

    /// (e) Negative (but finite) floats degrade to the zero value.
    #[test]
    fn negative_fields_degrade_to_zero() {
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: Some(synthetic_blob(-1.0, -2.0, -4.0, -0.5, -1.5)),
        };
        // -2.0 != -4.0 -> derived 1:2.
        assert_eq!(parse(&ctx, AmdZen::Zen3), Ok(known_snapshot(V_VERMEER, 0, 0, 0, 1, 0, 0)));
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
                pm: Some(full[..len].to_vec()),
            };
            let res = parse(&ctx, AmdZen::Zen3);
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
            pm: Some(blob),
        };
        assert_eq!(
            parse(&ctx, AmdZen::Zen3),
            Ok(known_snapshot(V_VERMEER, 1600, 1600, 1600, 0, 1000, 1200))
        );
    }

    /// (TC-3990X) A `SmuContext` with `pm: None` (the driver is loaded but
    /// the `pm_table` attribute is not exposed — e.g. Castle Peak /
    /// Threadripper 2990X) parses to an all-zero snapshot (every field an
    /// honest N/A under the P2-05 gates) WITHOUT hitting the version gate —
    /// a `0` version sentinel is carried, not rejected. This is the fix for
    /// the 3990X "driver missing" misclassification: the AMD branch stays
    /// alive (the SMN overlay still populates its slots downstream) instead
    /// of the whole section degrading to `DriverMissing`.
    #[test]
    fn parse_pm_none_yields_all_zero_snapshot() {
        // A `0` version sentinel (no version attribute published) must NOT
        // be rejected — the short-circuit returns before the version gate.
        let ctx = SmuContext { version: 0, pm: None };
        assert_eq!(parse(&ctx, AmdZen::Zen2), Ok(all_zero_snapshot(0)));

        // A non-zero version (a readable `pm_table_version` attribute) is
        // carried into the all-zero snapshot.
        let ctx = SmuContext { version: 0x38_08_05, pm: None };
        assert_eq!(parse(&ctx, AmdZen::Zen3), Ok(all_zero_snapshot(0x38_08_05)));

        // Every field is zero (the honest-Na basis under the P2-05 gates).
        let snap = all_zero_snapshot(0);
        assert_eq!(snap.mclk_mhz, 0);
        assert_eq!(snap.uclk_mhz, 0);
        assert_eq!(snap.fclk_mhz, 0);
        assert_eq!(snap.div_mode, 0);
        assert_eq!(snap.gdm, 0);
        assert_eq!(snap.pdm, 0);
        assert_eq!(snap.command_rate, 0);
        assert_eq!(snap.voltages.vcore_mv, 0);
        assert_eq!(snap.voltages.vddcr_soc_mv, 0);
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
                pm: Some(synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, vcore_v)),
            };
            let snap = parse(&ctx, AmdZen::Zen3).expect("synthetic blob must parse");
            assert_eq!(snap.voltages.vcore_mv, expected_mv);
        }
        // 0.0 V → 0 mV → out of band → Na downstream.
        let ctx = SmuContext {
            version: V_VERMEER,
            pm: Some(synthetic_blob(1600.0, 1600.0, 1600.0, 1.0, 0.0)),
        };
        assert_eq!(
            parse(&ctx, AmdZen::Zen3).expect("synthetic blob must parse").voltages.vcore_mv,
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
