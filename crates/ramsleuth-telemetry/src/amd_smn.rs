//! AMD SMN register readout — `ryzen_smu` `smn` sysfs accessor + verified
//! bitfield table (P6-02; interface freeze for P6-03).
//!
//! # The sanctioned channel (plan D1)
//!
//! The driver's `smn` attribute — canonical kobject
//! `/sys/kernel/ryzen_smu_drv/smn` first, legacy `/sys/kernel/ryzen_smu/smn`
//! second — is a **write-address → read-value** protocol (verified against
//! the installed amkillam/ryzen_smu v0.1.7 source at `/opt/ryzen-smu-src`):
//! writing one 4-byte little-endian `u32` address makes the driver read that
//! SMN register into a shared `smn_result`; reading the same attribute then
//! returns the raw 4-byte result (`drv.c` `smn_store` / `smn_show`;
//! `libsmu.c` `smu_read_smn_addr`: `lseek(0)` → `write(addr, 4)` →
//! `lseek(0)` → `read(4)` on a single `O_RDWR` fd). **No register value is
//! ever written** — only the 4-byte address word; the 8-byte write form of
//! `smn_store` is never used (read-only use of a read/write attribute).
//! Never raw MMIO, never the `ryzen_smu` Rust crate (P2-03 D1 stands).
//!
//! # Verified register table (plan §1 — `monitor_cpu` `print_memory_timings`)
//!
//! | SMN reg | fields (bits) | snapshot slot(s) |
//! |---|---|---|
//! | `0x50200` | MCLK set-point `(v & 0x7F) / 3 × 100` MHz (6:0); **GDM** (11); command rate 1T/2T (10) | `gdm` (set-point/CR: decoded, not stored) |
//! | `0x50204` | tCL (5:0); tRAS (14:8); tRCDRD (20:16); tRCDWR (28:24) | `cl` / `ras` / `rcdrd` / `rcwdwr` |
//! | `0x50208` | tRC (7:0); tRP (21:16) | `rc` / `rp` |
//! | `0x5020C` | tRRDS (4:0); tRRDL (12:8); tRTP (28:24) | `rrds` / `rrld` / `rtp` |
//! | `0x50210` | tFAW (7:0) | `faw` |
//! | `0x50214` | tCWL (5:0); tWTRS (12:8); tWTRL (20:16) | `cwl` / `wtrs` / `wtrl` |
//! | `0x50218` | tWR (7:0) | `wr` |
//! | `0x50220` | tRDRD dd/sd/sc/scl (3:0 / 11:8 / 19:16 / 29:24) | `rdrd_dd` / `rdrd_sd` / `rdrd_sc` / `rdrd_scl` |
//! | `0x50224` | tWRWR dd/sd/sc/scl (3:0 / 11:8 / 19:16 / 29:24) | `wrwr_dd` / `wrwr_sd` / `wrwr_sc` / `wrwr_scl` |
//! | `0x50228` | tWRRD (3:0); tRDWR (12:8) | `wrrd` / `rdwr` |
//! | `0x50254` | tCKE (28:24) | decoded, documented, not stored (no frozen slot) |
//! | `0x50260` | tRFC (9:0); tRFC2 (20:11); tRFC4 (31:22) | `rfc1` / `rfc2` / `rfcsb` |
//! | `0x50264` | tRFC mirror — sentinel rule below | mirror-check per the reference |
//!
//! The bitfields follow the reference code **exactly** (it is the
//! cross-check target): the one transcription difference vs the plan's §1
//! table is tRP, which the reference masks as 6 bits at bit 16
//! (`(reg >> 16) & 0x3F`, bits 21:16) — the reference wins.
//!
//! Two reference rules are implemented:
//!
//! - **Offset rule** (reference line 220): when the first read of
//!   `0x50200` returns exactly `0x300`, the UMC register block is
//!   relocated by `+0x100000` — every register, including the re-read
//!   `0x50200` set-point word, is read at `address + 0x100000`.
//! - **tRFC mirror rule** (reference lines 275–277): when `0x50260` reads
//!   the sentinel `0x21060138` *and* differs from the `0x50264` mirror, the
//!   tRFC fields decode from the mirror word.
//!
//! # CAD + PDM: confirm-or-Na (plan D3)
//!
//! Verified against the installed driver source and its userspace: no CAD
//! drive-strength / termination bitfields and no PDM bitfield are published
//! in the `monitor_cpu` one-shot or anywhere in the driver. Per the
//! confirm-or-Na rule they are **not decoded**: the snapshot's `pdm` and the
//! eight `cad_bus` codes stay exactly as P2-04's `parse` zeroed them and
//! render as honest `Disabled` / `Na` under the P2-05 gates — no field is
//! ever displayed with an unverified mapping. [`apply_smn`] never writes
//! them; re-confirm (with the AMD-published UMC maps) before storing.
//!
//! # Known limitation (documented, plan D3)
//!
//! The driver's `smn_result` is a single shared global: a concurrent
//! `monitor_cpu` run can race this daemon's 2 s collection, and on a failed
//! SMU read the driver leaves `smn_result` stale. Stale/corrupt words
//! degrade per field through the P2-05 sanity gates, and the ground-truth
//! cross-check samples the two tools **sequentially, never concurrently**.
//!
//! # No-panic contract
//!
//! Every hardware path degrades to a structured [`TelemetryError`] or an
//! honest zero: vendor gate before any I/O; missing `smn` attribute
//! (`DriverMissing`) → the whole overlay is a no-op (status quo); any other
//! per-register failure zeroes only that register's fields; short/empty
//! reads are `Parse`. No `unwrap` / `expect` on any hardware-derived value;
//! no `unsafe` (the `nix` wrappers are safe).
//!
//! # Platform
//!
//! Linux-only I/O (the driver interface). On non-Linux targets
//! [`read_smn_register`] runs the vendor gate and then returns
//! [`TelemetryError::UnsupportedHardware`]; [`apply_smn`] degrades through
//! per-register containment.

use crate::amd_pm::{AmdPmSnapshot, AmdPmTimings};
use crate::amd_smu::{classify_io_error, vendor_gate};
use crate::cpuid::CpuInfo;
use crate::error::{TelemetryError, TelemetryResult};

// ---------------------------------------------------------------------------
// `smn` attribute + verified register table (const; see the module docs).
// ---------------------------------------------------------------------------

/// `ryzen_smu` sysfs `smn` attribute paths, tried in order.
///
/// The upstream module registers its kobject as `ryzen_smu_drv`
/// (amkillam/ryzen_smu `drv.c`: `kobject_create_and_add("ryzen_smu_drv",
/// kernel_kobj)`), so the canonical path is
/// `/sys/kernel/ryzen_smu_drv/smn`; the legacy `ryzen_smu` directory name
/// is kept as a fallback for older or renamed builds.
const SYSFS_SMN_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/smn",
    "/sys/kernel/ryzen_smu/smn",
];

/// Driver name carried by [`TelemetryError::DriverMissing`].
const DRIVER: &str = "ryzen_smu";

/// Size of one `smn` word (4-byte LE `u32` address or `smn_result`).
const SMN_WORD: usize = 4;

/// `0x50200` — MCLK set-point (bits 6:0), GDM (bit 11), command rate (bit 10).
const SMN_MCLK: u32 = 0x50200;
/// `0x50204` — tCL / tRAS / tRCDRD / tRCDWR.
const SMN_CMD0: u32 = 0x50204;
/// `0x50208` — tRC / tRP.
const SMN_CMD1: u32 = 0x50208;
/// `0x5020C` — tRRDS / tRRDL / tRTP.
const SMN_CMD2: u32 = 0x5020C;
/// `0x50210` — tFAW.
const SMN_CMD3: u32 = 0x50210;
/// `0x50214` — tCWL / tWTRS / tWTRL.
const SMN_CMD4: u32 = 0x50214;
/// `0x50218` — tWR.
const SMN_CMD5: u32 = 0x50218;
/// `0x50220` — tRDRD dd / sd / sc / scl.
const SMN_RDRD: u32 = 0x50220;
/// `0x50224` — tWRWR dd / sd / sc / scl.
const SMN_WRWR: u32 = 0x50224;
/// `0x50228` — tWRRD / tRDWR.
const SMN_TURN: u32 = 0x50228;
/// `0x50254` — tCKE (decoded, documented, not stored).
const SMN_CKE: u32 = 0x50254;
/// `0x50260` — tRFC / tRFC2 / tRFC4.
const SMN_RFC: u32 = 0x50260;
/// `0x50264` — tRFC mirror (sentinel rule).
const SMN_RFC_MIRROR: u32 = 0x50264;

/// The verified 13-register read set (plan §1) in `monitor_cpu` read order.
const SMN_REGISTER_SET: [u32; 13] = [
    SMN_MCLK, SMN_CMD0, SMN_CMD1, SMN_CMD2, SMN_CMD3, SMN_CMD4, SMN_CMD5,
    SMN_RDRD, SMN_WRWR, SMN_TURN, SMN_CKE, SMN_RFC, SMN_RFC_MIRROR,
];

/// Offset-rule marker (reference line 220): a raw `0x50200` read of exactly
/// `0x300` relocates the UMC register block.
const SMN_HIGH_BASE_MARKER: u32 = 0x300;
/// The relocation applied when the marker is seen: `+0x100000`.
const SMN_HIGH_BASE: u32 = 0x100000;
/// tRFC sentinel (reference lines 275–277): `0x50260` reading this value —
/// while differing from the `0x50264` mirror — switches the tRFC fields to
/// the mirror word.
const SMN_RFC_SENTINEL: u32 = 0x2106_0138;

// ---------------------------------------------------------------------------
// Pure decoders (fixture-testable without I/O; the `intel_readout::decode_*`
// pattern). Every extractor masks the reference's exact bit range, so the
// result is bounded by construction (max `0x3FF`) — no overflow, no panic.
// ---------------------------------------------------------------------------

/// MCLK set-point in MHz: `(reg & 0x7F) / 3 × 100` (reference line 232).
///
/// Cross-check context only (the PM table owns the snapshot's MCLK); max
/// value is 4200 MHz — saturating to `u16` by construction.
///
/// Decoded, documented, not stored (plan §1): the frozen snapshot has no
/// set-point slot, so the value is consumed only by the in-file tests and
/// the ground-truth cross-check record — hence the targeted
/// `#[allow(dead_code)]` (it is not otherwise reachable from production
/// code by design).
#[allow(dead_code)]
pub(crate) fn mclk_setpoint_mhz(reg: u32) -> u16 {
    let steps = (reg & 0x7F) / 3;
    u16::try_from(steps * 100).unwrap_or(u16::MAX)
}

/// Gear Down Mode: bit 11 of `0x50200` (`0` = off, `1` = on).
pub(crate) fn gdm_flag(reg: u32) -> u8 {
    ((reg >> 11) & 1) as u8
}

/// Command rate: bit 10 of `0x50200` (`0` = 1T, `1` = 2T) — decoded,
/// documented, not stored (no frozen slot; see [`mclk_setpoint_mhz`] for
/// the `#[allow(dead_code)]` rationale).
#[allow(dead_code)]
pub(crate) fn command_rate(reg: u32) -> u8 {
    ((reg >> 10) & 1) as u8
}

/// tCL — `0x50204` bits 5:0.
pub(crate) fn tcl(reg: u32) -> u16 {
    (reg & 0x3F) as u16
}
/// tRAS — `0x50204` bits 14:8.
pub(crate) fn tras(reg: u32) -> u16 {
    ((reg >> 8) & 0x7F) as u16
}
/// tRCDRD — `0x50204` bits 20:16.
pub(crate) fn trcdrd(reg: u32) -> u16 {
    ((reg >> 16) & 0x3F) as u16
}
/// tRCDWR — `0x50204` bits 28:24.
pub(crate) fn trcdwr(reg: u32) -> u16 {
    ((reg >> 24) & 0x3F) as u16
}
/// tRC — `0x50208` bits 7:0.
pub(crate) fn trc(reg: u32) -> u16 {
    (reg & 0xFF) as u16
}
/// tRP — `0x50208` bits 21:16 (the reference masks 6 bits at bit 16; the
/// plan's §1 table lists "22:16" — the reference wins).
pub(crate) fn trp(reg: u32) -> u16 {
    ((reg >> 16) & 0x3F) as u16
}
/// tRRDS — `0x5020C` bits 4:0.
pub(crate) fn trrds(reg: u32) -> u16 {
    (reg & 0x1F) as u16
}
/// tRRDL — `0x5020C` bits 12:8.
pub(crate) fn trrdl(reg: u32) -> u16 {
    ((reg >> 8) & 0x1F) as u16
}
/// tRTP — `0x5020C` bits 28:24.
pub(crate) fn trtp(reg: u32) -> u16 {
    ((reg >> 24) & 0x1F) as u16
}
/// tFAW — `0x50210` bits 7:0.
pub(crate) fn tfaw(reg: u32) -> u16 {
    (reg & 0xFF) as u16
}
/// tCWL — `0x50214` bits 5:0.
pub(crate) fn tcwl(reg: u32) -> u16 {
    (reg & 0x3F) as u16
}
/// tWTRS — `0x50214` bits 12:8.
pub(crate) fn twtrs(reg: u32) -> u16 {
    ((reg >> 8) & 0x1F) as u16
}
/// tWTRL — `0x50214` bits 20:16.
pub(crate) fn twtrl(reg: u32) -> u16 {
    ((reg >> 16) & 0x3F) as u16
}
/// tWR — `0x50218` bits 7:0.
pub(crate) fn twr(reg: u32) -> u16 {
    (reg & 0xFF) as u16
}
/// tRDRD same DIMM — `0x50220` bits 3:0.
pub(crate) fn trdrd_dd(reg: u32) -> u16 {
    (reg & 0xF) as u16
}
/// tRDRD same CCD — `0x50220` bits 11:8.
pub(crate) fn trdrd_sd(reg: u32) -> u16 {
    ((reg >> 8) & 0xF) as u16
}
/// tRDRD via SC — `0x50220` bits 19:16.
pub(crate) fn trdrd_sc(reg: u32) -> u16 {
    ((reg >> 16) & 0xF) as u16
}
/// tRDRD via SCL — `0x50220` bits 29:24.
pub(crate) fn trdrd_scl(reg: u32) -> u16 {
    ((reg >> 24) & 0x3F) as u16
}
/// tWRWR same DIMM — `0x50224` bits 3:0.
pub(crate) fn twrwr_dd(reg: u32) -> u16 {
    (reg & 0xF) as u16
}
/// tWRWR same CCD — `0x50224` bits 11:8.
pub(crate) fn twrwr_sd(reg: u32) -> u16 {
    ((reg >> 8) & 0xF) as u16
}
/// tWRWR via SC — `0x50224` bits 19:16.
pub(crate) fn twrwr_sc(reg: u32) -> u16 {
    ((reg >> 16) & 0xF) as u16
}
/// tWRWR via SCL — `0x50224` bits 29:24.
pub(crate) fn twrwr_scl(reg: u32) -> u16 {
    ((reg >> 24) & 0x3F) as u16
}
/// tWRRD — `0x50228` bits 3:0.
pub(crate) fn twrrd(reg: u32) -> u16 {
    (reg & 0xF) as u16
}
/// tRDWR — `0x50228` bits 12:8.
pub(crate) fn trdwr(reg: u32) -> u16 {
    ((reg >> 8) & 0x1F) as u16
}
/// tCKE — `0x50254` bits 28:24. Decoded, documented, not stored (no
/// frozen slot in the snapshot; see [`mclk_setpoint_mhz`] for the
/// `#[allow(dead_code)]` rationale).
#[allow(dead_code)]
pub(crate) fn tcke(reg: u32) -> u16 {
    ((reg >> 24) & 0x1F) as u16
}
/// tRFC — `0x50260` bits 9:0.
pub(crate) fn trfc(reg: u32) -> u16 {
    (reg & 0x3FF) as u16
}
/// tRFC2 — `0x50260` bits 20:11.
pub(crate) fn trfc2(reg: u32) -> u16 {
    ((reg >> 11) & 0x3FF) as u16
}
/// tRFC4 — `0x50260` bits 31:22.
pub(crate) fn trfc4(reg: u32) -> u16 {
    ((reg >> 22) & 0x3FF) as u16
}

// ---------------------------------------------------------------------------
// Pure decode core (frozen public API, P6-02 — P6-03 consumes `apply_smn`;
// this core is what makes the overlay testable without I/O).
// ---------------------------------------------------------------------------

/// The confirmed SMN fields decoded from the register set.
///
/// Carries GDM + the 27 DRAM subtimings. PDM and the eight CAD-bus codes
/// are deliberately **absent**: their bitfields are unconfirmed (plan D3,
/// confirm-or-Na), so the snapshot's `pdm` / `cad_bus` stay zeroed by
/// P2-04 and render as honest `Disabled` / `Na` under the P2-05 gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmnFields {
    /// Gear Down Mode (`0x50200` bit 11): `0` = off, `1` = on.
    pub gdm: u8,
    /// The 27 DRAM subtimings in ticks (the verified reference table).
    pub timings: AmdPmTimings,
}

/// The raw word for `addr` in a register list: the **first** occurrence's
/// value, or `0` when the address is absent or its word is `None` (no
/// panic, no garbage). Unknown addresses are ignored.
fn word_at(regs: &[(u32, Option<u32>)], addr: u32) -> u32 {
    regs.iter()
        .find(|(a, _)| *a == addr)
        .map(|(_, w)| w.unwrap_or(0))
        .unwrap_or(0)
}

/// Maps raw `smn` words onto the confirmed fields (pure: no I/O, no
/// panic on any input).
///
/// `regs` is the register set as `(address, word)` pairs; a failed read is
/// `None` (its fields decode to `0` → honest `Na` / `Disabled` downstream).
/// The `0x50264` mirror/sentinel rule applies to the tRFC fields (module
/// docs); every field is bounded by the reference's mask width.
pub fn decode_smn(regs: &[(u32, Option<u32>)]) -> SmnFields {
    // tRFC mirror rule (reference lines 275–277): sentinel word in
    // `0x50260` that differs from the `0x50264` mirror -> use the mirror.
    let (w_rfc, w_mirror) = (word_at(regs, SMN_RFC), word_at(regs, SMN_RFC_MIRROR));
    let rfc_word = if w_rfc == SMN_RFC_SENTINEL && w_rfc != w_mirror {
        w_mirror
    } else {
        w_rfc
    };

    SmnFields {
        gdm: gdm_flag(word_at(regs, SMN_MCLK)),
        timings: AmdPmTimings {
            cl: tcl(word_at(regs, SMN_CMD0)),
            rcwdwr: trcdwr(word_at(regs, SMN_CMD0)),
            rcdrd: trcdrd(word_at(regs, SMN_CMD0)),
            rp: trp(word_at(regs, SMN_CMD1)),
            ras: tras(word_at(regs, SMN_CMD0)),
            rc: trc(word_at(regs, SMN_CMD1)),
            rrds: trrds(word_at(regs, SMN_CMD2)),
            rrld: trrdl(word_at(regs, SMN_CMD2)),
            faw: tfaw(word_at(regs, SMN_CMD3)),
            wtrs: twtrs(word_at(regs, SMN_CMD4)),
            wtrl: twtrl(word_at(regs, SMN_CMD4)),
            wr: twr(word_at(regs, SMN_CMD5)),
            rfc1: trfc(rfc_word),
            rfc2: trfc2(rfc_word),
            rfcsb: trfc4(rfc_word),
            cwl: tcwl(word_at(regs, SMN_CMD4)),
            rtp: trtp(word_at(regs, SMN_CMD2)),
            rdwr: trdwr(word_at(regs, SMN_TURN)),
            wrrd: twrrd(word_at(regs, SMN_TURN)),
            rdrd_sd: trdrd_sd(word_at(regs, SMN_RDRD)),
            rdrd_dd: trdrd_dd(word_at(regs, SMN_RDRD)),
            rdrd_scl: trdrd_scl(word_at(regs, SMN_RDRD)),
            rdrd_sc: trdrd_sc(word_at(regs, SMN_RDRD)),
            wrwr_sd: twrwr_sd(word_at(regs, SMN_WRWR)),
            wrwr_dd: twrwr_dd(word_at(regs, SMN_WRWR)),
            wrwr_scl: twrwr_scl(word_at(regs, SMN_WRWR)),
            wrwr_sc: twrwr_sc(word_at(regs, SMN_WRWR)),
        },
    }
}

// ---------------------------------------------------------------------------
// Accessor (plan D1 — the `amd_smu` idiom: vendor gate, candidate list,
// RAII fd, classified errors).
// ---------------------------------------------------------------------------

/// Read one SMN register through the driver's `smn` sysfs attribute.
///
/// Protocol: open the first existing candidate `O_RDWR` → `lseek(0)` →
/// write the 4-byte LE address → `lseek(0)` → read the 4-byte LE
/// `smn_result` (exactly `libsmu` `smu_read_smn_addr`). Only the address
/// word is ever written — never a register value.
///
/// # Errors
///
/// - [`TelemetryError::UnsupportedHardware`] — non-AMD vendor (checked
///   before any I/O) or a non-Linux platform.
/// - [`TelemetryError::DriverMissing`] — no `smn` candidate exists
///   (module not loaded).
/// - [`TelemetryError::InsufficientPrivilege`] — the attribute exists but
///   `open(2)` is denied (not root; the attribute is `O_RDWR`).
/// - [`TelemetryError::Parse`] — short address write, or short/empty
///   `smn_result` read (truncated payload; a failed driver read leaves the
///   shared `smn_result` stale — the per-field sanity gates absorb it).
/// - [`TelemetryError::Io`] — any other raw I/O failure.
pub fn read_smn_register(address: u32) -> TelemetryResult<u32> {
    // Vendor gate: pure, so non-AMD hardware is rejected before any file
    // access (plan D1 gate discipline).
    vendor_gate(&CpuInfo::detect())?;

    #[cfg(target_os = "linux")]
    {
        read_smn_register_linux(address)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(TelemetryError::UnsupportedHardware {
            vendor: "non-linux platform (the ryzen_smu smn attribute requires Linux)".to_owned(),
        })
    }
}

/// Open the first existing `smn` attribute `O_RDWR` (canonical kobject
/// first, then the legacy name).
///
/// NotFound on a candidate skips to the next; a PermissionDenied on an
/// existing file is reported as [`TelemetryError::InsufficientPrivilege`]
/// (no fall-through — the attribute is there, the caller just lacks
/// rights); any other I/O error is reported as [`TelemetryError::Io`].
/// All candidates absent → [`TelemetryError::DriverMissing`].
fn open_smn_attr(candidates: &[&str]) -> TelemetryResult<std::os::unix::io::RawFd> {
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    for &path in candidates {
        match nix::fcntl::open(path, OFlag::O_RDWR, Mode::empty()) {
            Ok(fd) => return Ok(fd),
            Err(e) => match classify_io_error(&std::io::Error::from(e)) {
                TelemetryError::DriverMissing { .. } => continue,
                other => return Err(other),
            },
        }
    }
    Err(TelemetryError::DriverMissing { driver: DRIVER })
}

/// A short address write is a malformed payload ([`TelemetryError::Parse`]).
fn check_smn_write(written: usize) -> TelemetryResult<()> {
    if written < SMN_WORD {
        return Err(TelemetryError::Parse {
            detail: format!(
                "truncated smn address write: {written} of {SMN_WORD} byte(s)"
            ),
        });
    }
    Ok(())
}

/// A short or empty `smn_result` read is a malformed payload
/// ([`TelemetryError::Parse`]).
fn check_smn_read(read: usize) -> TelemetryResult<()> {
    if read < SMN_WORD {
        return Err(TelemetryError::Parse {
            detail: format!("truncated smn_result read: {read} of {SMN_WORD} byte(s)"),
        });
    }
    Ok(())
}

/// The `libsmu` `smn` protocol on an open fd: `lseek(0)` → write the
/// 4-byte LE address → `lseek(0)` → read the 4-byte LE `smn_result`.
///
/// The `lseek` results are **tolerated** (a failure never aborts the read):
/// `libsmu` discards them, and sysfs attribute files may not support
/// seeking at all — the driver's show/store never use the file offset, so
/// each `read(2)` returns the current `smn_result` regardless.
#[cfg(target_os = "linux")]
fn read_smn_register_linux(address: u32) -> TelemetryResult<u32> {
    let fd = open_smn_attr(&SYSFS_SMN_CANDIDATES)?;
    let _guard = FdGuard(fd);
    smn_protocol_read(fd, address)
}

/// Run the write-address → read-value protocol on one open fd.
#[cfg(target_os = "linux")]
fn smn_protocol_read(
    fd: std::os::unix::io::RawFd,
    address: u32,
) -> TelemetryResult<u32> {
    use nix::unistd::Whence;
    use std::os::unix::io::BorrowedFd;

    // Tolerated (see the docs): libsmu discards the lseek result, and sysfs
    // attribute files may not support seeking at all.
    let _ = nix::unistd::lseek(fd, 0, Whence::SeekSet);

    let addr = address.to_le_bytes();
    // SAFETY: `fd` was returned by `open(2)` in this call path and is owned
    // by the caller's `FdGuard` (which closes it exactly once on scope
    // exit); it is valid for the whole duration of this call and not
    // aliased by any other `BorrowedFd` while in use.
    let fd_ref = unsafe { BorrowedFd::borrow_raw(fd) };
    match nix::unistd::write(fd_ref, &addr) {
        Ok(n) => check_smn_write(n)?,
        Err(e) => return Err(classify_io_error(&std::io::Error::from(e))),
    }

    let _ = nix::unistd::lseek(fd, 0, Whence::SeekSet);

    let mut buf = [0u8; SMN_WORD];
    match nix::unistd::read(fd, &mut buf) {
        Ok(n) => {
            check_smn_read(n)?;
            Ok(u32::from_le_bytes(buf))
        }
        Err(e) => Err(classify_io_error(&std::io::Error::from(e))),
    }
}

/// RAII guard: closes the `smn` fd exactly once on scope exit (a local
/// copy of the `amd_smu` idiom — that one is private).
///
/// `close()` on a valid fd cannot meaningfully fail; its result is
/// discarded rather than escalated (no-panic contract).
#[cfg(target_os = "linux")]
struct FdGuard(std::os::unix::io::RawFd);

#[cfg(target_os = "linux")]
impl Drop for FdGuard {
    fn drop(&mut self) {
        let _ = nix::unistd::close(self.0);
    }
}

// ---------------------------------------------------------------------------
// No-panic overlay (frozen public API — the single consumer-facing entry
// point P6-03 wires into `amd_branch`).
// ---------------------------------------------------------------------------

/// Overlay the confirmed SMN fields onto a parsed [`AmdPmSnapshot`]
/// (plan D2: no-op, never fails the branch, never panics).
///
/// Behavior:
/// - **Non-AMD vendor** → no-op before any I/O (gate discipline).
/// - **Missing `smn` attribute** (`DriverMissing`) → no-op: the snapshot is
///   left exactly as P2-04 parsed it (the status quo on older module
///   builds that expose only the PM table).
/// - **Any other per-register failure** (privilege, I/O, short read) →
///   that register's fields stay zeroed (honest `Disabled` / `Na` under
///   the P2-05 gates); the other registers proceed independently.
/// - **Written fields**: `gdm` (`0x50200` bit 11) + the 27 `timings` — and
///   only those. PM-table fields (version, clocks, div mode, voltages) are
///   never touched; `pdm` / `cad_bus` stay zeroed (unconfirmed bitfields,
///   plan D3).
pub fn apply_smn(snap: &mut AmdPmSnapshot) {
    if vendor_gate(&CpuInfo::detect()).is_err() {
        return;
    }
    apply_smn_with(read_smn_register, snap);
}

/// The overlay core with an **injectable register reader** (the hermetic
/// test seam: tests feed synthetic words or synthetic errors; production
/// injects [`read_smn_register`]). No vendor gate here — that belongs to
/// the public [`apply_smn`] / [`read_smn_register`] contract, keeping this
/// core host-independent.
///
/// Register flow (module docs): probe `0x50200` (offset rule) → read the
/// remaining 12 at the resolved base → [`decode_smn`] → write `gdm` +
/// `timings` only. `DriverMissing` on the probe is a whole-overlay no-op;
/// any other probe failure degrades to per-register containment (a failed
/// read contributes `None` → its fields decode to `0`).
fn apply_smn_with<R: FnMut(u32) -> TelemetryResult<u32>>(
    mut reader: R,
    snap: &mut AmdPmSnapshot,
) {
    let (setpoint, base) = match reader(SMN_MCLK) {
        // Offset rule: a raw `0x50200` word of exactly the marker relocates
        // the UMC block; the set-point word is re-read at the relocated
        // address and the rest follow the same base.
        Ok(word) if word == SMN_HIGH_BASE_MARKER => {
            (reader(SMN_MCLK + SMN_HIGH_BASE).ok(), SMN_HIGH_BASE)
        }
        Ok(word) => (Some(word), 0),
        // No `smn` attribute at all (older module builds): no-op — the
        // snapshot keeps exactly what the PM table provided.
        Err(TelemetryError::DriverMissing { .. }) => return,
        // Privilege / I/O / short read: per-register containment — the
        // set-point stays zero and every other read is attempted at the
        // base offset (each failure zeroes its own fields).
        Err(_) => (None, 0),
    };

    let mut regs: Vec<(u32, Option<u32>)> = Vec::with_capacity(SMN_REGISTER_SET.len());
    regs.push((SMN_MCLK, setpoint));
    for &addr in SMN_REGISTER_SET.iter().skip(1) {
        regs.push((addr, reader(addr + base).ok()));
    }

    let fields = decode_smn(&regs);
    // Only the zeroed SMN fields are written. `pdm` and the 8 `cad_bus`
    // codes are deliberately untouched: their bitfields are unconfirmed
    // (plan D3 confirm-or-Na) and they render as honest `Disabled` / `Na`
    // under the P2-05 gates.
    snap.gdm = fields.gdm;
    snap.timings = fields.timings;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amd_pm::{AmdPmCadBus, AmdPmTimings, AmdPmVoltages};

    // ---- fixtures ---------------------------------------------------------

    /// A parsed-snapshot-shaped fixture: PM fields populated (live 5950X
    /// class), SMN fields zeroed — exactly P2-04 `parse` output.
    fn base_snapshot() -> AmdPmSnapshot {
        AmdPmSnapshot {
            version: 0x38_08_05,
            mclk_mhz: 1800,
            uclk_mhz: 1800,
            fclk_mhz: 1792,
            div_mode: 1,
            gdm: 0,
            pdm: 0,
            timings: AmdPmTimings {
                cl: 0, rcwdwr: 0, rcdrd: 0, rp: 0, ras: 0, rc: 0, rrds: 0, rrld: 0,
                faw: 0, wtrs: 0, wtrl: 0, wr: 0, rfc1: 0, rfc2: 0, rfcsb: 0,
                cwl: 0, rtp: 0, rdwr: 0, wrrd: 0, rdrd_sd: 0, rdrd_dd: 0,
                rdrd_scl: 0, rdrd_sc: 0, wrwr_sd: 0, wrwr_dd: 0, wrwr_scl: 0,
                wrwr_sc: 0,
            },
            cad_bus: AmdPmCadBus {
                proc_odt: 0, rtt_nom: 0, rtt_wr: 0, rtt_park: 0, clk_drv: 0,
                addr_cmd_drv: 0, cs_odt_drv: 0, cke_drv: 0,
            },
            voltages: AmdPmVoltages {
                vddcr_soc_mv: 1128,
                vddio_mem_mv: 0,
                vdd_misc_mv: 0,
                vpp_mv: 0,
            },
        }
    }

    /// The verified-table fixture words: one per register, distinct bits
    /// per field (the decode expectations are pinned below).
    fn fixture_regs() -> Vec<(u32, Option<u32>)> {
        vec![
            (SMN_MCLK, Some(0x0000_1539)),     // README live example: 1900 MHz, GDM off, 2T
            (SMN_CMD0, Some(0x1010_2410)),     // tCL=16 tRAS=36 tRCDRD=16 tRCDWR=16
            (SMN_CMD1, Some(0x0010_0030)),     // tRC=48 tRP=16
            (SMN_CMD2, Some(0x0400_0404)),     // tRRDS=4 tRRDL=4 tRTP=4
            (SMN_CMD3, Some(0x0000_0010)),     // tFAW=16
            (SMN_CMD4, Some(0x0008_0410)),     // tCWL=16 tWTRS=4 tWTRL=8
            (SMN_CMD5, Some(0x0000_0010)),     // tWR=16
            (SMN_RDRD, Some(0x0504_0302)),     // dd=2 sd=3 sc=4 scl=5
            (SMN_WRWR, Some(0x0908_0706)),     // dd=6 sd=7 sc=8 scl=9
            (SMN_TURN, Some(0x0000_0602)),     // tWRRD=2 tRDWR=6
            (SMN_CKE, Some(0x0400_0000)),      // tCKE=4 (decoded, not stored)
            (SMN_RFC, Some(0x7E08_20A0)),      // tRFC=160 tRFC2=260 tRFC4=504
            (SMN_RFC_MIRROR, Some(0x1111_2222)), // mirror (ignored: no sentinel)
        ]
    }

    // ---- table + protocol pins ---------------------------------------------

    /// (a) The `smn` attribute candidates pin the verified upstream kobject
    /// (`ryzen_smu_drv`) before the legacy directory name (the verified-
    /// kobject-first rule, pinned like the PM family).
    #[test]
    fn smn_candidates_try_verified_kobject_first() {
        assert_eq!(
            SYSFS_SMN_CANDIDATES,
            ["/sys/kernel/ryzen_smu_drv/smn", "/sys/kernel/ryzen_smu/smn"]
        );
    }

    /// (a) The 13-register read set pins the verified `monitor_cpu`
    /// addresses in read order.
    #[test]
    fn register_set_pins_verified_addresses() {
        assert_eq!(
            SMN_REGISTER_SET,
            [
                0x50200, 0x50204, 0x50208, 0x5020C, 0x50210, 0x50214, 0x50218,
                0x50220, 0x50224, 0x50228, 0x50254, 0x50260, 0x50264,
            ]
        );
    }

    /// (b) Every documented register's bitfield extraction is pinned
    /// against the reference (`monitor_cpu.c` `print_memory_timings`) — one
    /// fixture word per register, distinct bits per field.
    #[test]
    fn extractors_pin_reference_bitfields() {
        // 0x50200: set-point / GDM / command rate
        assert_eq!(mclk_setpoint_mhz(0x0000_1539), 1900); // README live example
        assert_eq!(mclk_setpoint_mhz(0x0000_0300), 0); // offset-marker word
        assert_eq!(gdm_flag(0x0000_1539), 0);
        assert_eq!(gdm_flag(0x0000_1539 | 0x0000_0800), 1); // bit 11 set
        assert_eq!(command_rate(0x0000_1539), 1); // 2T (bit 10 set in the example)
        assert_eq!(command_rate(0x0000_1539 & !0x0000_0400), 0); // 1T

        // 0x50204 / 0x50208 / 0x5020C / 0x50210 / 0x50214 / 0x50218
        assert_eq!(tcl(0x1010_2410), 16);
        assert_eq!(tras(0x1010_2410), 36);
        assert_eq!(trcdrd(0x1010_2410), 16);
        assert_eq!(trcdwr(0x1010_2410), 16);
        assert_eq!(trc(0x0010_0030), 48);
        assert_eq!(trp(0x0010_0030), 16);
        assert_eq!(trrds(0x0400_0404), 4);
        assert_eq!(trrdl(0x0400_0404), 4);
        assert_eq!(trtp(0x0400_0404), 4);
        assert_eq!(tfaw(0x0000_0010), 16);
        assert_eq!(tcwl(0x0008_0410), 16);
        assert_eq!(twtrs(0x0008_0410), 4);
        assert_eq!(twtrl(0x0008_0410), 8);
        assert_eq!(twr(0x0000_0010), 16);

        // 0x50220 / 0x50224 / 0x50228 / 0x50254 / 0x50260
        assert_eq!(trdrd_dd(0x0504_0302), 2);
        assert_eq!(trdrd_sd(0x0504_0302), 3);
        assert_eq!(trdrd_sc(0x0504_0302), 4);
        assert_eq!(trdrd_scl(0x0504_0302), 5);
        assert_eq!(twrwr_dd(0x0908_0706), 6);
        assert_eq!(twrwr_sd(0x0908_0706), 7);
        assert_eq!(twrwr_sc(0x0908_0706), 8);
        assert_eq!(twrwr_scl(0x0908_0706), 9);
        assert_eq!(twrrd(0x0000_0602), 2);
        assert_eq!(trdwr(0x0000_0602), 6);
        assert_eq!(tcke(0x0400_0000), 4);
        assert_eq!(trfc(0x7E08_20A0), 160);
        assert_eq!(trfc2(0x7E08_20A0), 260);
        assert_eq!(trfc4(0x7E08_20A0), 504);
    }

    /// (b) All-ones words pin every mask width: no field can decode above
    /// the reference's max (and the not-stored extractors are pinned too).
    #[test]
    fn all_ones_words_pin_every_mask_width() {
        let regs: Vec<(u32, Option<u32>)> =
            SMN_REGISTER_SET.iter().map(|a| (*a, Some(0xFFFF_FFFF))).collect();
        let f = decode_smn(&regs);
        assert_eq!(f.gdm, 1);
        let t = f.timings;
        assert_eq!(t.cl, 63);
        assert_eq!(t.ras, 127);
        assert_eq!(t.rcdrd, 63);
        assert_eq!(t.rcwdwr, 63);
        assert_eq!(t.rc, 255);
        assert_eq!(t.rp, 63);
        assert_eq!(t.rrds, 31);
        assert_eq!(t.rrld, 31);
        assert_eq!(t.rtp, 31);
        assert_eq!(t.faw, 255);
        assert_eq!(t.cwl, 63);
        assert_eq!(t.wtrs, 31);
        assert_eq!(t.wtrl, 63);
        assert_eq!(t.wr, 255);
        assert_eq!(t.rdrd_dd, 15);
        assert_eq!(t.rdrd_sd, 15);
        assert_eq!(t.rdrd_sc, 15);
        assert_eq!(t.rdrd_scl, 63);
        assert_eq!(t.wrwr_dd, 15);
        assert_eq!(t.wrwr_sd, 15);
        assert_eq!(t.wrwr_sc, 15);
        assert_eq!(t.wrwr_scl, 63);
        assert_eq!(t.wrrd, 15);
        assert_eq!(t.rdwr, 31);
        assert_eq!(t.rfc1, 1023);
        assert_eq!(t.rfc2, 1023);
        assert_eq!(t.rfcsb, 1023);
        assert_eq!(mclk_setpoint_mhz(0xFFFF_FFFF), 4200); // 127/3 × 100
        assert_eq!(command_rate(0xFFFF_FFFF), 1);
        assert_eq!(tcke(0xFFFF_FFFF), 31);
    }

    /// (b) `decode_smn` maps the fixture words onto all 27 timings + GDM
    /// exactly (the `0x50264` mirror is ignored without the sentinel).
    #[test]
    fn decode_smn_fixture_words_all_27_timings_and_gdm() {
        let f = decode_smn(&fixture_regs());
        assert_eq!(f.gdm, 0);
        assert_eq!(
            f.timings,
            AmdPmTimings {
                cl: 16, rcwdwr: 16, rcdrd: 16, rp: 16, ras: 36, rc: 48, rrds: 4,
                rrld: 4, faw: 16, wtrs: 4, wtrl: 8, wr: 16, rfc1: 160, rfc2: 260,
                rfcsb: 504, cwl: 16, rtp: 4, rdwr: 6, wrrd: 2, rdrd_sd: 3,
                rdrd_dd: 2, rdrd_scl: 5, rdrd_sc: 4, wrwr_sd: 7, wrwr_dd: 6,
                wrwr_scl: 9, wrwr_sc: 8,
            }
        );
    }

    /// (b) GDM is exactly bit 11 of `0x50200` (all lower / all upper bits
    /// irrelevant).
    #[test]
    fn gdm_is_bit_11_of_50200() {
        assert_eq!(decode_smn(&[(SMN_MCLK, Some(0x0000_0800u32))]).gdm, 1);
        assert_eq!(decode_smn(&[(SMN_MCLK, Some(0x0000_07FFu32))]).gdm, 0);
        assert_eq!(decode_smn(&[(SMN_MCLK, Some(0xFFFF_FFFFu32))]).gdm, 1);
    }

    /// (c) The `0x50264` mirror/sentinel rule (reference lines 275–277):
    /// sentinel + differing mirror → the mirror word; sentinel + equal →
    /// no swap; no sentinel → `0x50260`; absent mirror → zeros (never a
    /// panic).
    #[test]
    fn trfc_mirror_sentinel_rule() {
        let t = decode_smn(&[
            (SMN_RFC, Some(0x2106_0138)),
            (SMN_RFC_MIRROR, Some(0x1111_2222)),
        ])
        .timings;
        assert_eq!(t.rfc1, 546);
        assert_eq!(t.rfc2, 548);
        assert_eq!(t.rfcsb, 68);

        let t = decode_smn(&[
            (SMN_RFC, Some(0x2106_0138)),
            (SMN_RFC_MIRROR, Some(0x2106_0138)),
        ])
        .timings;
        assert_eq!(t.rfc1, 312);
        assert_eq!(t.rfc2, 192);
        assert_eq!(t.rfcsb, 132);

        let t = decode_smn(&[
            (SMN_RFC, Some(0x7E08_20A0)),
            (SMN_RFC_MIRROR, Some(0x1111_2222)),
        ])
        .timings;
        assert_eq!(t.rfc1, 160);
        assert_eq!(t.rfc2, 260);
        assert_eq!(t.rfcsb, 504);

        let t = decode_smn(&[
            (SMN_RFC, Some(0x2106_0138)),
            (SMN_RFC_MIRROR, None),
        ])
        .timings;
        assert_eq!(t.rfc1, 0);
        assert_eq!(t.rfc2, 0);
        assert_eq!(t.rfcsb, 0);
    }

    /// (c) Robust inputs: empty list → all zeros; duplicate addresses →
    /// the first occurrence wins; unknown addresses are ignored; `None`
    /// words decode to `0` (no panic, no garbage).
    #[test]
    fn decode_smn_robust_inputs() {
        let zero = SmnFields {
            gdm: 0,
            timings: AmdPmTimings {
                cl: 0, rcwdwr: 0, rcdrd: 0, rp: 0, ras: 0, rc: 0, rrds: 0, rrld: 0,
                faw: 0, wtrs: 0, wtrl: 0, wr: 0, rfc1: 0, rfc2: 0, rfcsb: 0,
                cwl: 0, rtp: 0, rdwr: 0, wrrd: 0, rdrd_sd: 0, rdrd_dd: 0,
                rdrd_scl: 0, rdrd_sc: 0, wrwr_sd: 0, wrwr_dd: 0, wrwr_scl: 0,
                wrwr_sc: 0,
            },
        };
        assert_eq!(decode_smn(&[]), zero);

        let dup = vec![
            (SMN_CMD0, Some(0x1010_2410)),
            (SMN_CMD0, Some(0xFFFF_FFFF)),
        ];
        assert_eq!(decode_smn(&dup).timings.cl, 16); // first occurrence wins

        let unknown = vec![
            (SMN_CMD0, Some(0x1010_2410)),
            (0x99_9999, Some(0xFFFF_FFFF)), // ignored
            (SMN_MCLK, None),               // absent word -> 0
        ];
        let f = decode_smn(&unknown);
        assert_eq!(f.timings.cl, 16);
        assert_eq!(f.gdm, 0);
    }

    // ---- accessor (hermetic) ----------------------------------------------

    /// (b) The open-path classification: an absent attribute is
    /// `DriverMissing` (the loop falls through to the structured error).
    #[test]
    fn open_smn_attr_missing_is_driver_missing() {
        let res = open_smn_attr(&["/nonexistent/ramsleuth_smn_test/smn"]);
        assert!(
            matches!(res, Err(TelemetryError::DriverMissing { .. })),
            "unexpected: {res:?}"
        );
    }

    /// (b) A present-but-unreadable attribute classifies as
    /// `InsufficientPrivilege` (non-root; running as root the mode bits are
    /// bypassed and the open succeeds — nothing to assert in that case).
    #[test]
    fn open_smn_attr_permission_denied_is_insufficient_privilege() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!("ramsleuth_smn_test_{}", std::process::id()));
        std::fs::File::create(&path).expect("temp file must be creatable in the test sandbox");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");

        let res = open_smn_attr(&[path.to_str().expect("temp path is valid UTF-8")]);
        let _ = std::fs::remove_file(&path);

        match res {
            Err(TelemetryError::InsufficientPrivilege { .. }) => {} // non-root: EACCES classified
            Ok(fd) => {
                // Root: the mode bits are bypassed — close the fd and accept
                // that EACCES cannot be forced here.
                let _ = nix::unistd::close(fd);
            }
            other => panic!("unexpected open result: {other:?}"),
        }
    }

    /// (b) The LE write/read protocol on a synthetic transport (unix socket
    /// pair — the `smn` attr's store/show split mirrored): the address word
    /// goes out, the pre-fed `smn_result` comes back; the request bytes are
    /// exactly the 4-byte LE address.
    #[cfg(target_os = "linux")]
    #[test]
    fn smn_protocol_le_write_read_pinned() {
        use std::io::{Read, Write};
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let (a, mut b) = UnixStream::pair().expect("unix socket pair");
        // Pre-feed the response the transport will deliver.
        b.write_all(&0xDEAD_BEEFu32.to_le_bytes())
            .expect("pre-feed response");

        assert_eq!(smn_protocol_read(a.as_raw_fd(), 0x50200), Ok(0xDEAD_BEEF));

        // The protocol's write must have been the 4-byte LE address.
        b.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("read timeout");
        let mut got = [0u8; SMN_WORD];
        let n = b.read(&mut got).unwrap_or(0);
        assert_eq!(n, SMN_WORD, "the address word must arrive (4 bytes)");
        assert_eq!(got, 0x50200u32.to_le_bytes());
    }

    /// (b) A short `smn_result` read is `Parse` (never a panic): two
    /// pre-fed bytes + EOF.
    #[cfg(target_os = "linux")]
    #[test]
    fn smn_protocol_short_read_is_parse() {
        use std::io::Write;
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let (a, mut b) = UnixStream::pair().expect("unix socket pair");
        b.write_all(&[0xAB, 0xCD]).expect("pre-feed 2 bytes");
        b.shutdown(std::net::Shutdown::Write)
            .expect("close the send direction (read side stays open for the address write)");

        let res = smn_protocol_read(a.as_raw_fd(), 0x50200);
        assert!(
            matches!(res, Err(TelemetryError::Parse { .. })),
            "unexpected: {res:?}"
        );
        // The short-read classification itself (no I/O): any count below 4
        // is Parse, 4 is Ok.
        for n in 0..SMN_WORD {
            assert!(matches!(check_smn_read(n), Err(TelemetryError::Parse { .. })), "{n}");
        }
        assert_eq!(check_smn_read(SMN_WORD), Ok(()));
        // The short-write classification mirrors it.
        for n in 0..SMN_WORD {
            assert!(matches!(check_smn_write(n), Err(TelemetryError::Parse { .. })), "{n}");
        }
        assert_eq!(check_smn_write(SMN_WORD), Ok(()));
        // (The write-failure branch is the same `classify_io_error` mapping
        // pinned in `amd_smu::tests` — a live EPIPE is not exercised: SIGPIPE
        // would terminate the test process.)
        let _ = a;
        let _ = b;
    }

    /// (b) An empty read (EOF with no queued bytes) is `Parse`.
    #[cfg(target_os = "linux")]
    #[test]
    fn smn_protocol_empty_read_is_parse() {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let (a, b) = UnixStream::pair().expect("unix socket pair");
        b.shutdown(std::net::Shutdown::Write).expect("close the send direction");

        let res = smn_protocol_read(a.as_raw_fd(), 0x50200);
        assert!(
            matches!(res, Err(TelemetryError::Parse { .. })),
            "unexpected: {res:?}"
        );
    }

    /// (c) On this host the accessor is graceful: `Ok` or one of the listed
    /// structured `Err` variants — never a panic (host-state dependent:
    /// driver loaded? privilege? — only the shape is asserted).
    #[test]
    fn read_smn_register_on_this_host_is_graceful() {
        let res = read_smn_register(0x50200);
        assert!(
            matches!(
                res,
                Ok(_)
                    | Err(TelemetryError::DriverMissing { .. })
                    | Err(TelemetryError::InsufficientPrivilege { .. })
                    | Err(TelemetryError::UnsupportedHardware { .. })
                    | Err(TelemetryError::Io(_))
                    | Err(TelemetryError::Parse { .. })
            ),
            "unexpected variant: {res:?}"
        );
    }

    // ---- overlay (hermetic: injected reader) --------------------------------

    /// (d) `DriverMissing` on the probe is a whole-overlay no-op: the
    /// snapshot stays byte-identical to P2-04's parse output (the status
    /// quo on older module builds).
    #[test]
    fn apply_smn_with_driver_missing_is_a_noop() {
        let mut snap = base_snapshot();
        apply_smn_with(
            |_| Err(TelemetryError::DriverMissing { driver: DRIVER }),
            &mut snap,
        );
        assert_eq!(snap, base_snapshot());
    }

    /// (d) Per-register containment: a privilege failure on every read
    /// zeroes the SMN fields (honest `Disabled` / `Na` downstream) while
    /// the PM-table fields survive untouched.
    #[test]
    fn apply_smn_with_privilege_failure_contains_per_register() {
        let mut snap = base_snapshot();
        apply_smn_with(
            |_| Err(TelemetryError::InsufficientPrivilege { hint: "run as root" }),
            &mut snap,
        );
        assert_eq!(snap.gdm, 0);
        assert_eq!(snap.timings, base_snapshot().timings); // all zero
        assert_eq!(snap.cad_bus, base_snapshot().cad_bus); // unconfirmed: untouched
        assert_eq!(snap.pdm, 0); // unconfirmed: untouched
        assert_eq!(snap.version, 0x38_08_05);
        assert_eq!(snap.mclk_mhz, 1800);
        assert_eq!(snap.uclk_mhz, 1800);
        assert_eq!(snap.fclk_mhz, 1792);
        assert_eq!(snap.div_mode, 1);
        assert_eq!(snap.voltages.vddcr_soc_mv, 1128);
    }

    /// (d) A synthetic register feed populates exactly the SMN fields (GDM +
    /// the 27 timings) and leaves clocks / voltages / `pdm` / `cad_bus`
    /// untouched — the overlay's write surface is precisely D2's.
    #[test]
    fn apply_smn_with_synthetic_feed_writes_smn_fields_only() {
        let mut snap = base_snapshot();
        apply_smn_with(
            |addr| {
                fixture_regs()
                    .into_iter()
                    .find(|(a, _)| *a == addr)
                    .map(|(_, w)| Ok(w.expect("fixture words are all Some")))
                    .unwrap_or(Ok(0))
            },
            &mut snap,
        );
        assert_eq!(snap.gdm, 0); // the fixture's 0x50200 = 0x1539
        let t = snap.timings;
        assert_eq!(t.cl, 16);
        assert_eq!(t.ras, 36);
        assert_eq!(t.rc, 48);
        assert_eq!(t.faw, 16);
        assert_eq!(t.rfc1, 160);
        assert_eq!(t.rfc2, 260);
        assert_eq!(t.rfcsb, 504);
        assert_eq!(t.wrwr_scl, 9);
        // The write surface is exactly D2: nothing else moved.
        assert_eq!(snap.version, 0x38_08_05);
        assert_eq!(snap.mclk_mhz, 1800);
        assert_eq!(snap.uclk_mhz, 1800);
        assert_eq!(snap.fclk_mhz, 1792);
        assert_eq!(snap.div_mode, 1);
        assert_eq!(snap.pdm, 0);
        assert_eq!(snap.cad_bus, base_snapshot().cad_bus);
        assert_eq!(snap.voltages, base_snapshot().voltages);
    }

    /// (e) The offset rule (reference line 220): a probe word of exactly
    /// `0x300` relocates every register by `+0x100000` — including the
    /// re-read `0x50200` set-point word — and the reads land at the
    /// relocated addresses.
    #[test]
    fn apply_smn_with_applies_the_offset_rule() {
        let mut seen: Vec<u32> = Vec::new();
        let mut snap = base_snapshot();
        apply_smn_with(
            |addr| {
                seen.push(addr);
                if addr == SMN_MCLK {
                    Ok(0x300) // the marker
                } else if addr == SMN_MCLK + SMN_HIGH_BASE {
                    Ok(0x1539) // relocated set-point
                } else if addr == SMN_CMD0 + SMN_HIGH_BASE {
                    Ok(0x1010_2410) // relocated tCL/tRAS/...
                } else {
                    assert!(
                        (SMN_CMD1 + SMN_HIGH_BASE..=SMN_RFC_MIRROR + SMN_HIGH_BASE)
                            .contains(&addr),
                        "unexpected relocated address: {addr:#x}"
                    );
                    Ok(0)
                }
            },
            &mut snap,
        );
        // 1 probe + 1 relocated re-probe + 12 relocated reads.
        assert_eq!(seen.len(), 14);
        assert_eq!(seen[0], SMN_MCLK);
        assert_eq!(seen[1], SMN_MCLK + SMN_HIGH_BASE);
        for a in &seen[2..] {
            assert!(*a >= SMN_CMD0 + SMN_HIGH_BASE, "low address after relocation: {a:#x}");
        }
        assert_eq!(snap.timings.cl, 16); // from the relocated 0x50204 word
        assert_eq!(snap.gdm, 0);
    }

    /// (f) On this host the overlay is graceful and preserves the PM fields
    /// (non-root → per-register containment; root → live words in-band;
    /// non-AMD host → early no-op — all three shapes are asserted, never a
    /// panic).
    #[test]
    fn apply_smn_on_this_host_is_graceful_and_preserves_pm_fields() {
        let mut snap = base_snapshot();
        let pm_before = (
            snap.version,
            snap.mclk_mhz,
            snap.uclk_mhz,
            snap.fclk_mhz,
            snap.div_mode,
            snap.voltages,
        );
        apply_smn(&mut snap);
        let pm_after = (
            snap.version,
            snap.mclk_mhz,
            snap.uclk_mhz,
            snap.fclk_mhz,
            snap.div_mode,
            snap.voltages,
        );
        assert_eq!(pm_before, pm_after, "PM-table fields must never be touched");
        assert!(snap.gdm <= 1, "gdm decodes from a single bit");
        assert_eq!(snap.pdm, 0); // unconfirmed: never written
        assert_eq!(snap.cad_bus, base_snapshot().cad_bus); // unconfirmed: never written
        // Every timing is within its reference mask width (0 or decoded).
        let t = &snap.timings;
        assert!(t.cl <= 63 && t.ras <= 127 && t.rcdrd <= 63 && t.rcwdwr <= 63);
        assert!(t.rc <= 255 && t.rp <= 63 && t.rrds <= 31 && t.rrld <= 31 && t.rtp <= 31);
        assert!(t.faw <= 255 && t.cwl <= 63 && t.wtrs <= 31 && t.wtrl <= 63 && t.wr <= 255);
        assert!(t.rdrd_dd <= 15 && t.rdrd_sd <= 15 && t.rdrd_sc <= 15 && t.rdrd_scl <= 63);
        assert!(t.wrwr_dd <= 15 && t.wrwr_sd <= 15 && t.wrwr_sc <= 15 && t.wrwr_scl <= 63);
        assert!(t.wrrd <= 15 && t.rdwr <= 31);
        assert!(t.rfc1 <= 1023 && t.rfc2 <= 1023 && t.rfcsb <= 1023);
    }
}
