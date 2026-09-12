//! AMD SMU PM-table parse — version-guarded, bounds-checked (P2-04).
//!
//! Turns the raw PM-table blob acquired by [`crate::amd_smu`] (P2-03) into a
//! structured [`AmdPmSnapshot`]:
//!
//! - **Clocks** — MCLK / UCLK / FCLK (MHz), the UCLK:MCLK divide mode, Gear
//!   Down Mode (GDM) and Power Down Mode (PDM);
//! - **Timings** — the 19 primary DRAM subtimings plus the 8
//!   tertiary/turnaround timings (ticks);
//! - **CAD bus** — ProcODT / RttNom / RttWr / RttPark / ClkDrv / AddrCmdDrv /
//!   CsOdtDrv / CkeDrv as raw RZQ/driver codes;
//! - **Voltages** — VDDCR_SOC / VDDIO_MEM / VDD_MISC / VPP (mV).
//!
//! The snapshot holds *raw PM units* only; display mapping (code→Ω, mV→V,
//! ratio computation, sanity ranges) belongs to P2-05 (`amd_readout.rs`).
//!
//! # Version guard
//!
//! [`SmuContext::version`] (frozen in P2-03 as the little-endian `u32` at
//! blob offset [`VERSION_OFF`]) encodes the SMU firmware version as
//! `(major << 16) | (minor << 8) | patch` — the same scheme the `ryzen_smu`
//! driver prints via `"%d.%d.%d"`. [`PmLayout::from_version`] maps it to one
//! of the three supported table layouts:
//!
//! | version word          | layout               | family |
//! |-----------------------|----------------------|--------|
//! | `0x0007_0Bxx` (7.11.x)| [`PmLayout::Smu711`] | Zen 3  |
//! | `0x000C_xxxx` (12.x)  | [`PmLayout::Smu12`]  | Zen 4  |
//! | `0x000D_xxxx` (13.x)  | [`PmLayout::Smu13`]  | Zen 5  |
//!
//! Any other version word yields [`TelemetryError::UnknownPmTableVersion`].
//!
//! # Layout tables (P2-04 skeleton)
//!
//! Each layout is a [`PmTableLayout`] — named region anchors (byte offsets
//! from the start of the blob) holding packed little-endian fields in the
//! plan's field order:
//!
//! ```text
//! 0x00   version        u32 LE   (P2-03 frozen header)
//! anchor clocks         3 × u16  (mclk, uclk, fclk — MHz)
//! +3     modes          3 × u8   (div_mode, gdm, pdm; 1 reserved byte)
//! +4     voltages       4 × u16  (VDDCR_SOC, VDDIO_MEM, VDD_MISC, VPP — mV)
//! +8     cad            8 × u16  (RZQ/driver codes)
//! +54    timings        27 × u16 (19 primary + 8 tertiary ticks)
//! ```
//!
//! The three families place the region block at different depths
//! ([`SMU711`] shallowest → [`SMU12`] → [`SMU13`] deepest), mirroring the
//! *relative* placement of the published `ryzen_smu` metrics-table indices
//! (Zen 3 clock floats at 48/50/51, Zen 4 at 70/74/78, Zen 5 at 71/75/79).
//! The absolute anchors are the P2-04 **skeleton** table (plan D2): concrete,
//! named, auditable constants to be re-verified byte-for-byte against a live
//! blob at P2-11 (needs the `ryzen_smu` module loaded + root; per the
//! DEV_LOG the module is not currently loaded on the test host). Until then,
//! live data degrades gracefully: a non-mapping version word returns
//! [`TelemetryError::UnknownPmTableVersion`], a blob truncated for its layout
//! returns [`TelemetryError::Parse`], and no code path can panic or read out
//! of bounds.
//!
//! # Safety
//!
//! No `unsafe`: every field read goes through the bounds-checked helpers
//! `read_u8` / `read_u16le` / `read_u32le`, which return
//! [`TelemetryError::Parse`] when `off + width > blob.len()` (they never
//! index and never panic). [`parse`] additionally cross-checks the blob's
//! version header against [`SmuContext::version`] and gates on the layout's
//! minimum length before reading fields.

use crate::amd_smu::SmuContext;
use crate::error::{TelemetryError, TelemetryResult};

/// Byte offset of the SMU version word: the little-endian `u32` at the start
/// of the blob (P2-03 frozen header contract).
const VERSION_OFF: usize = 0x00;

/// Fixed packed-region sizes in bytes: 3×u16 clocks, 3×u8 modes + 1 reserved
/// byte, 4×u16 voltages, 8×u16 CAD codes, 27×u16 timings.
const CLOCKS_LEN: usize = 6;
const MODES_LEN: usize = 4;
const VOLTAGES_LEN: usize = 8;
const CAD_LEN: usize = 16;
const TIMINGS_LEN: usize = 54;

/// One supported PM-table family layout (plan D2: 7.11.x / 12.x / 13.x).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmLayout {
    /// SMU 7.11.x — Zen 3 (e.g. Ryzen 9 5950X, the test host).
    Smu711,
    /// SMU 12.x — Zen 4.
    Smu12,
    /// SMU 13.x — Zen 5.
    Smu13,
}

impl PmLayout {
    /// Maps a raw SMU version word to its PM-table layout.
    ///
    /// The word encodes `major.minor.patch` as `(major << 16) | (minor << 8) |
    /// patch` (P2-03 `extract_version`; the `ryzen_smu` driver prints the same
    /// encoding). `7.11.x` → [`Self::Smu711`], `12.x` → [`Self::Smu12`],
    /// `13.x` → [`Self::Smu13`]; anything else (including 7.10.x / 7.12.x and
    /// major ≥ 14) → [`TelemetryError::UnknownPmTableVersion`].
    pub fn from_version(version: u32) -> TelemetryResult<Self> {
        let major = (version >> 16) & 0xFF;
        let minor = (version >> 8) & 0xFF;
        match major {
            7 if minor == 11 => Ok(Self::Smu711),
            12 => Ok(Self::Smu12),
            13 => Ok(Self::Smu13),
            _ => Err(TelemetryError::UnknownPmTableVersion { version }),
        }
    }

    /// The region anchors for this layout.
    pub fn offsets(self) -> &'static PmTableLayout {
        match self {
            Self::Smu711 => &SMU711,
            Self::Smu12 => &SMU12,
            Self::Smu13 => &SMU13,
        }
    }

    /// Short layout name for diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Self::Smu711 => "Smu711",
            Self::Smu12 => "Smu12",
            Self::Smu13 => "Smu13",
        }
    }
}

/// The byte layout of one PM-table family.
///
/// Region anchors are offsets from the start of the blob; the regions pack
/// back-to-back in the order `clocks → modes → voltages → cad → timings`
/// (fixed sizes [`CLOCKS_LEN`] … [`TIMINGS_LEN`]). `end` is one past the last
/// required byte — the minimum blob length for the layout. All fields are
/// little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmTableLayout {
    /// Clock region start: `mclk_mhz, uclk_mhz, fclk_mhz` (3×u16, MHz).
    pub clocks: usize,
    /// Mode region start: `div_mode, gdm, pdm` (3×u8; 4th byte reserved).
    pub modes: usize,
    /// Voltage region start: `vddcr_soc, vddio_mem, vdd_misc, vpp` (4×u16, mV).
    pub voltages: usize,
    /// CAD region start: 8×u16 RZQ/driver codes ([`AmdPmCadBus`]).
    pub cad: usize,
    /// Timing region start: 27×u16 tick values ([`AmdPmTimings`]).
    pub timings: usize,
    /// One past the last required byte (minimum blob length).
    pub end: usize,
}

/// Derives a full layout from its clock-region anchor: the remaining regions
/// pack back-to-back at fixed sizes, so non-overlap and the `end` boundary
/// hold by construction.
const fn derived_layout(clocks: usize) -> PmTableLayout {
    let modes = clocks + CLOCKS_LEN;
    let voltages = modes + MODES_LEN;
    let cad = voltages + VOLTAGES_LEN;
    let timings = cad + CAD_LEN;
    PmTableLayout {
        clocks,
        modes,
        voltages,
        cad,
        timings,
        end: timings + TIMINGS_LEN,
    }
}

/// SMU 7.11.x (Zen 3) skeleton layout — shallowest region block (0x04).
pub const SMU711: PmTableLayout = derived_layout(0x04);

/// SMU 12.x (Zen 4) skeleton layout — region block at 0x10.
pub const SMU12: PmTableLayout = derived_layout(0x10);

/// SMU 13.x (Zen 5) skeleton layout — deepest region block (0x14).
pub const SMU13: PmTableLayout = derived_layout(0x14);

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
}

/// A structured, version-guarded parse of the AMD SMU PM table (frozen
/// public API, P2-04).
///
/// All values are raw PM-table units (MHz, ticks, RZQ/driver codes, mV); the
/// display mapping (code→Ω, mV→V, UCLK:MCLK ratio, sanity ranges) is P2-05's
/// job. Built by [`parse`] from a [`SmuContext`]: every field is populated,
/// or the call returns `Err` — never a partially populated snapshot, never a
/// panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmdPmSnapshot {
    /// The SMU version word this snapshot was parsed from
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

/// Reads one byte from the blob at `off`.
///
/// Bounds-checked: returns [`TelemetryError::Parse`] when `off >= len` —
/// never indexes, never panics.
fn read_u8(blob: &[u8], off: usize) -> TelemetryResult<u8> {
    match blob.get(off) {
        Some(b) => Ok(*b),
        None => Err(truncated(off, 1, blob.len())),
    }
}

/// Reads one little-endian 16-bit value from the blob at `off`.
///
/// Bounds-checked: returns [`TelemetryError::Parse`] when `off + 2 > len` —
/// never indexes, never panics.
fn read_u16le(blob: &[u8], off: usize) -> TelemetryResult<u16> {
    match blob.get(off..off.saturating_add(2)) {
        Some(s) => Ok(u16::from_le_bytes([s[0], s[1]])),
        None => Err(truncated(off, 2, blob.len())),
    }
}

/// Reads one little-endian 32-bit value from the blob at `off`.
///
/// Bounds-checked: returns [`TelemetryError::Parse`] when `off + 4 > len` —
/// never indexes, never panics.
fn read_u32le(blob: &[u8], off: usize) -> TelemetryResult<u32> {
    match blob.get(off..off.saturating_add(4)) {
        Some(s) => Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        None => Err(truncated(off, 4, blob.len())),
    }
}

/// Parses the raw PM-table blob in `ctx` into a structured
/// [`AmdPmSnapshot`] (frozen public API, P2-04).
///
/// Version-guarded flow:
///
/// 1. [`PmLayout::from_version`] maps `ctx.version` to a layout; an
///    unrecognized version word → [`TelemetryError::UnknownPmTableVersion`].
/// 2. The blob's own version header (the LE `u32` at [`VERSION_OFF`]) must
///    match `ctx.version` (defends against a context/blob drift) → a
///    mismatch is [`TelemetryError::Parse`].
/// 3. The blob must be at least the layout's [`PmTableLayout::end`] bytes
///    (shorter → [`TelemetryError::Parse`]); every field read is
///    additionally bounds-checked individually, so the parse is safe even if
///    a layout table is edited out of sync later.
///
/// Pure: no I/O, no `unsafe`, no panic on any input.
pub fn parse(ctx: &SmuContext) -> TelemetryResult<AmdPmSnapshot> {
    let layout = PmLayout::from_version(ctx.version)?;
    let pm = &ctx.pm;
    let off = layout.offsets();

    // Header integrity: the blob's version word must be the one P2-03
    // extracted into the context.
    let header = read_u32le(pm, VERSION_OFF)?;
    if header != ctx.version {
        return Err(TelemetryError::Parse {
            detail: format!(
                "PM blob header version {header:#010x} does not match SmuContext version {:#010x}",
                ctx.version
            ),
        });
    }

    // Minimum-length gate for the selected layout.
    if pm.len() < off.end {
        return Err(TelemetryError::Parse {
            detail: format!(
                "PM blob too short for {} layout: {} byte(s), need at least {} (0x{:x})",
                layout.name(),
                pm.len(),
                off.end,
                off.end
            ),
        });
    }

    let mclk_mhz = read_u16le(pm, off.clocks)?;
    let uclk_mhz = read_u16le(pm, off.clocks + 2)?;
    let fclk_mhz = read_u16le(pm, off.clocks + 4)?;

    let div_mode = read_u8(pm, off.modes)?;
    let gdm = read_u8(pm, off.modes + 1)?;
    let pdm = read_u8(pm, off.modes + 2)?;

    let voltages = AmdPmVoltages {
        vddcr_soc_mv: read_u16le(pm, off.voltages)?,
        vddio_mem_mv: read_u16le(pm, off.voltages + 2)?,
        vdd_misc_mv: read_u16le(pm, off.voltages + 4)?,
        vpp_mv: read_u16le(pm, off.voltages + 6)?,
    };

    let cad_bus = AmdPmCadBus {
        proc_odt: read_u16le(pm, off.cad)?,
        rtt_nom: read_u16le(pm, off.cad + 2)?,
        rtt_wr: read_u16le(pm, off.cad + 4)?,
        rtt_park: read_u16le(pm, off.cad + 6)?,
        clk_drv: read_u16le(pm, off.cad + 8)?,
        addr_cmd_drv: read_u16le(pm, off.cad + 10)?,
        cs_odt_drv: read_u16le(pm, off.cad + 12)?,
        cke_drv: read_u16le(pm, off.cad + 14)?,
    };

    let t = off.timings;
    let timings = AmdPmTimings {
        cl: read_u16le(pm, t)?,
        rcwdwr: read_u16le(pm, t + 2)?,
        rcdrd: read_u16le(pm, t + 4)?,
        rp: read_u16le(pm, t + 6)?,
        ras: read_u16le(pm, t + 8)?,
        rc: read_u16le(pm, t + 10)?,
        rrds: read_u16le(pm, t + 12)?,
        rrld: read_u16le(pm, t + 14)?,
        faw: read_u16le(pm, t + 16)?,
        wtrs: read_u16le(pm, t + 18)?,
        wtrl: read_u16le(pm, t + 20)?,
        wr: read_u16le(pm, t + 22)?,
        rfc1: read_u16le(pm, t + 24)?,
        rfc2: read_u16le(pm, t + 26)?,
        rfcsb: read_u16le(pm, t + 28)?,
        cwl: read_u16le(pm, t + 30)?,
        rtp: read_u16le(pm, t + 32)?,
        rdwr: read_u16le(pm, t + 34)?,
        wrrd: read_u16le(pm, t + 36)?,
        rdrd_sd: read_u16le(pm, t + 38)?,
        rdrd_dd: read_u16le(pm, t + 40)?,
        rdrd_scl: read_u16le(pm, t + 42)?,
        rdrd_sc: read_u16le(pm, t + 44)?,
        wrwr_sd: read_u16le(pm, t + 46)?,
        wrwr_dd: read_u16le(pm, t + 48)?,
        wrwr_scl: read_u16le(pm, t + 50)?,
        wrwr_sc: read_u16le(pm, t + 52)?,
    };

    Ok(AmdPmSnapshot {
        version: ctx.version,
        mclk_mhz,
        uclk_mhz,
        fclk_mhz,
        div_mode,
        gdm,
        pdm,
        timings,
        cad_bus,
        voltages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Representative version words for the three supported layouts.
    const V711: u32 = 0x0007_0B02; // 7.11.2 — the test-host family
    const V12: u32 = 0x000C_0001; // 12.0.1
    const V13: u32 = 0x000D_0001; // 13.0.1

    fn write_u16le(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn write_u32le(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn layouts() -> [(PmLayout, u32); 3] {
        [(PmLayout::Smu711, V711), (PmLayout::Smu12, V12), (PmLayout::Smu13, V13)]
    }

    /// Builds a blob of exactly the layout's minimum length with known
    /// values written at the same derived offsets `parse` reads.
    fn synthetic_blob(layout: PmLayout, version: u32) -> Vec<u8> {
        let l = *layout.offsets();
        let mut blob = vec![0u8; l.end];
        write_u32le(&mut blob, VERSION_OFF, version);
        // clocks (MHz)
        write_u16le(&mut blob, l.clocks, 1600); // mclk
        write_u16le(&mut blob, l.clocks + 2, 1600); // uclk
        write_u16le(&mut blob, l.clocks + 4, 1600); // fclk
        // modes
        blob[l.modes] = 0; // div_mode 1:1
        blob[l.modes + 1] = 1; // gdm on
        blob[l.modes + 2] = 0; // pdm off
        // voltages (mV)
        write_u16le(&mut blob, l.voltages, 1050); // VDDCR_SOC
        write_u16le(&mut blob, l.voltages + 2, 1350); // VDDIO_MEM
        write_u16le(&mut blob, l.voltages + 4, 1000); // VDD_MISC
        write_u16le(&mut blob, l.voltages + 6, 1800); // VPP
        // CAD codes
        for (i, code) in [1u16, 2, 3, 4, 5, 6, 7, 8].into_iter().enumerate() {
            write_u16le(&mut blob, l.cad + i * 2, code);
        }
        // timings (ticks): 19 primary + 8 tertiary
        let primary: [u16; 19] = [
            16, 16, 16, 16, 32, 48, 4, 4, 16, 8, 8, 8, 160, 160, 160, 16, 8, 8, 4,
        ];
        let tertiary: [u16; 8] = [100, 101, 102, 103, 104, 105, 106, 107];
        for (i, v) in primary.iter().chain(tertiary.iter()).enumerate() {
            write_u16le(&mut blob, l.timings + i * 2, *v);
        }
        blob
    }

    /// The snapshot every synthetic blob above must parse to (distinct
    /// per-field values so any offset swap is caught).
    fn known_snapshot(version: u32) -> AmdPmSnapshot {
        AmdPmSnapshot {
            version,
            mclk_mhz: 1600,
            uclk_mhz: 1600,
            fclk_mhz: 1600,
            div_mode: 0,
            gdm: 1,
            pdm: 0,
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
                proc_odt: 1,
                rtt_nom: 2,
                rtt_wr: 3,
                rtt_park: 4,
                clk_drv: 5,
                addr_cmd_drv: 6,
                cs_odt_drv: 7,
                cke_drv: 8,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1050,
                vddio_mem_mv: 1350,
                vdd_misc_mv: 1000,
                vpp_mv: 1800,
            },
        }
    }

    /// (a) For EACH supported layout: a synthetic blob of the correct length
    /// with known values at the plan offsets parses to exactly those values
    /// in every field.
    #[test]
    fn parse_each_layout_extracts_every_field() {
        for (layout, version) in layouts() {
            let ctx = SmuContext {
                version,
                pm: synthetic_blob(layout, version),
            };
            assert_eq!(
                parse(&ctx),
                Ok(known_snapshot(version)),
                "layout {layout:?} (version {version:#010x})"
            );
        }
    }

    /// (a) A blob longer than the layout minimum (trailing data present)
    /// still parses — the gate is a minimum length, not an exact match.
    #[test]
    fn oversized_blob_still_parses() {
        let (layout, version) = (PmLayout::Smu711, V711);
        let mut blob = synthetic_blob(layout, version);
        blob.extend_from_slice(&[0u8; 16]);
        let ctx = SmuContext { version, pm: blob };
        assert_eq!(parse(&ctx), Ok(known_snapshot(version)));
    }

    /// (b) Truncated blobs — every length from 0 up to one byte short of the
    /// layout minimum — yield `Err(Parse)`; no panic, no out-of-bounds read.
    #[test]
    fn truncated_blobs_yield_parse_error_not_panic() {
        for (layout, version) in layouts() {
            let full = synthetic_blob(layout, version);
            let end = layout.offsets().end;
            assert_eq!(full.len(), end);
            for len in 0..end {
                let ctx = SmuContext {
                    version,
                    pm: full[..len].to_vec(),
                };
                let res = parse(&ctx);
                assert!(
                    matches!(res, Err(TelemetryError::Parse { .. })),
                    "layout {layout:?}: len {len} must be Err(Parse), got {res:?}"
                );
            }
        }
    }

    /// (c) An unrecognized version word — including the 9.0.1 example and
    /// near-miss 7.10.x / 7.12.x — yields `Err(UnknownPmTableVersion {
    /// version })` with the exact word preserved.
    #[test]
    fn unknown_versions_yield_unknown_pm_table_version() {
        for version in [
            0x0009_0001u32, // 9.0.1 (brief example)
            0x0007_0A00, // 7.10.0
            0x0007_0C00, // 7.12.0
            0x0000_0000,
            0x000E_0000, // 14.0.0
            0xFFFF_FFFF,
        ] {
            let ctx = SmuContext {
                version,
                pm: vec![0u8; 256],
            };
            assert_eq!(
                parse(&ctx),
                Err(TelemetryError::UnknownPmTableVersion { version }),
                "version {version:#010x}"
            );
        }
    }

    /// (d) The version→layout mapping at its boundary versions: every
    /// 7.11.x → Smu711, every 12.x → Smu12, every 13.x → Smu13, everything
    /// else → `UnknownPmTableVersion`.
    #[test]
    fn version_to_layout_boundaries() {
        for version in [0x0007_0B00u32, 0x0007_0B01, 0x0007_0B02, 0x0007_0B03, 0x0007_0BFF] {
            assert_eq!(
                PmLayout::from_version(version),
                Ok(PmLayout::Smu711),
                "{version:#010x}"
            );
        }
        for version in [0x000C_0000u32, 0x000C_0102, 0x000C_FFFF] {
            assert_eq!(
                PmLayout::from_version(version),
                Ok(PmLayout::Smu12),
                "{version:#010x}"
            );
        }
        for version in [0x000D_0000u32, 0x000D_0005, 0x000D_FFFF] {
            assert_eq!(
                PmLayout::from_version(version),
                Ok(PmLayout::Smu13),
                "{version:#010x}"
            );
        }
        for version in [
            0x0000_0000u32,
            0x0001_0000, // 1.0.0
            0x0007_0A00, // 7.10.0
            0x0007_0C00, // 7.12.0
            0x0009_0001, // 9.0.1
            0x0011_0000, // 17.0.0
            0xFFFF_FFFF,
        ] {
            assert_eq!(
                PmLayout::from_version(version),
                Err(TelemetryError::UnknownPmTableVersion { version }),
                "{version:#010x}"
            );
        }
    }

    /// (e) `read_u8`: correct in-bounds values; `Err(Parse)` out of bounds
    /// (off == len, off > len, empty blob).
    #[test]
    fn read_u8_bounds() {
        assert_eq!(read_u8(&[0xAB], 0), Ok(0xAB));
        assert_eq!(read_u8(&[0x01, 0x02, 0x03], 2), Ok(0x03));
        for (blob, off) in [(&[0xABu8][..], 1usize), (&[][..], 0), (&[0xABu8, 0xCDu8][..], 5)] {
            assert!(
                matches!(read_u8(blob, off), Err(TelemetryError::Parse { .. })),
                "blob {blob:?} off {off}"
            );
        }
    }

    /// (e) `read_u16le`: little-endian decode in bounds; `Err(Parse)` when
    /// off + 2 > len (including off == len).
    #[test]
    fn read_u16le_bounds() {
        assert_eq!(read_u16le(&[0x34, 0x12], 0), Ok(0x1234));
        assert_eq!(read_u16le(&[0x00, 0x34, 0x12, 0x00], 1), Ok(0x1234));
        for (blob, off) in [
            (&[0x34u8][..], 0usize), // 1 < 2
            (&[0x34u8, 0x12u8][..], 1), // off == len - 1
            (&[0x34u8, 0x12u8][..], 2), // off == len
            (&[][..], 0),
        ] {
            assert!(
                matches!(read_u16le(blob, off), Err(TelemetryError::Parse { .. })),
                "blob {blob:?} off {off}"
            );
        }
    }

    /// (e) `read_u32le`: little-endian decode in bounds; `Err(Parse)` when
    /// off + 4 > len (including off == len).
    #[test]
    fn read_u32le_bounds() {
        assert_eq!(read_u32le(&[0x01, 0x02, 0x03, 0x04], 0), Ok(0x0403_0201));
        assert_eq!(read_u32le(&[0x00, 0x01, 0x02, 0x03, 0x04, 0x00], 1), Ok(0x0403_0201));
        for (blob, off) in [
            (&[0x01u8, 0x02u8, 0x03u8][..], 0usize), // 3 < 4
            (&[0x01u8, 0x02u8, 0x03u8, 0x04u8][..], 1), // off + 4 = 5 > 4
            (&[0x01u8, 0x02u8, 0x03u8, 0x04u8][..], 4), // off == len
            (&[][..], 0),
        ] {
            assert!(
                matches!(read_u32le(blob, off), Err(TelemetryError::Parse { .. })),
                "blob {blob:?} off {off}"
            );
        }
    }

    /// The blob's version header must match `SmuContext::version`; a
    /// drifted header is `Err(Parse)` (the consistent case parses).
    #[test]
    fn header_version_mismatch_is_parse_error() {
        let version = V711;
        let ctx = SmuContext {
            version,
            pm: synthetic_blob(PmLayout::Smu711, version),
        };
        assert_eq!(parse(&ctx), Ok(known_snapshot(version)));

        let mut drifted = ctx.clone();
        drifted.pm[..4].copy_from_slice(&0x0007_0B03u32.to_le_bytes());
        assert!(matches!(parse(&drifted), Err(TelemetryError::Parse { .. })));
    }
}
