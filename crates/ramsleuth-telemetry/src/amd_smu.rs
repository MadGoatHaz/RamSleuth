//! AMD SMU access layer — raw PM-table blob acquisition (P2-03).
//!
//! This module is the single, privilege-guarded gateway to the `ryzen_smu`
//! kernel driver on AMD Zen silicon. Everything downstream (P2-04's
//! version-guarded parse, P2-05's readout mapping) consumes the
//! [`SmuContext`] this module produces; no other part of the crate touches
//! the driver.
//!
//! # Acquisition strategy (sysfs-first)
//!
//! 1. **Vendor gate (pure, zero I/O):** [`CpuInfo::detect()`] must report an
//!    AMD vendor; anything else yields
//!    [`TelemetryError::UnsupportedHardware`] *before any file access*
//!    (plan §D4 gate discipline).
//! 2. **Primary — sysfs:** read the PM table with `std::fs` from the
//!    driver kobject directory. The upstream module registers that
//!    kobject as `ryzen_smu_drv` (verified in amkillam/ryzen_smu
//!    `drv.c`), so the canonical path is
//!    `/sys/kernel/ryzen_smu_drv/pm_table`; the legacy
//!    `/sys/kernel/ryzen_smu/pm_table` is tried second for older or
//!    renamed builds. NotFound on every candidate → fall through to
//!    step 3. Permission error on an existing candidate →
//!    [`TelemetryError::InsufficientPrivilege`]. Any other I/O error →
//!    [`TelemetryError::Io`].
//! 3. **Fallback — char device:** open `/dev/ryzen_smu` read-only and fetch
//!    the PM table through the driver's `read(2)` interface (the 53XU/
//!    ryzen_smu uAPI). The installed amkillam/ryzen_smu build registers
//!    **only the sysfs kobject** — no `/dev/ryzen_smu` node — so this path
//!    is dead on that module and is kept purely as 53XU-fork tolerance:
//!    missing device node → [`TelemetryError::DriverMissing`], permission
//!    error → [`TelemetryError::InsufficientPrivilege`].
//! 4. **Version sourcing (P5-15 reconciliation):** the SMU version is the
//!    PM table's `TableVersionId` — the little-endian `u32` the driver
//!    publishes in the sibling sysfs attribute
//!    `/sys/kernel/ryzen_smu_drv/pm_table_version`
//!    ([`SYSFS_PM_VERSION_CANDIDATES`]). The `pm_table` blob itself is a
//!    **headerless** array of little-endian `f32` values (byte offset =
//!    index × 4) and carries **no** version word. The sibling
//!    `pm_table_size` attribute (8-byte LE `u64`,
//!    [`SYSFS_PM_SIZE_CANDIDATES`]) cross-checks the blob length when
//!    present; a disagreement is [`TelemetryError::Parse`]. When the
//!    version attribute is **absent** (older module builds, or the char
//!    device fallback), the version degrades to the legacy
//!    first-word-of-blob extraction ([`extract_version`]) so the outcome
//!    stays structured (`UnknownPmTableVersion` downstream) — never a hard
//!    error and never a panic.
//!
//! # Safety
//!
//! No `unsafe` code: every syscall goes through a `nix` safe wrapper
//! (`nix::fcntl::open`, `nix::unistd::read`/`close`), and the fd lifetime
//! is owned by [`FdGuard`] (closed exactly once on scope exit). Error
//! classification is pure ([`classify_io_error`]), so every failure degrades
//! to a structured [`TelemetryError`] — never a panic (no-panic contract,
//! plan §D5).
//!
//! # Platform
//!
//! The driver interfaces are Linux-only. On non-Linux targets
//! [`acquire()`] runs the vendor gate and then returns
//! [`TelemetryError::UnsupportedHardware`] (this module still compiles).

use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{TelemetryError, TelemetryResult};

/// `ryzen_smu` sysfs PM-table paths, tried in order (primary acquisition).
///
/// The upstream module registers its kobject as `ryzen_smu_drv`
/// (amkillam/ryzen_smu `drv.c`: `kobject_create_and_add("ryzen_smu_drv",
/// kernel_kobj)`), so the canonical path is
/// `/sys/kernel/ryzen_smu_drv/pm_table`; the legacy `ryzen_smu` directory
/// name is kept as a fallback for older or renamed builds.
const SYSFS_PM_TABLE_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/pm_table",
    "/sys/kernel/ryzen_smu/pm_table",
];

/// Sibling sysfs attributes carrying the PM table's `TableVersionId`
/// (little-endian `u32`), tried in order (P5-15).
///
/// The `pm_table` blob is a headerless f32 array with no version word of
/// its own; on module builds that publish it, the version lives in this
/// sibling attribute.
const SYSFS_PM_VERSION_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/pm_table_version",
    "/sys/kernel/ryzen_smu/pm_table_version",
];

/// Sibling sysfs attributes carrying the PM table size (little-endian
/// `u64`), tried in order (P5-15).
///
/// When present, the reported size must equal the blob length; a
/// disagreement is a malformed payload ([`TelemetryError::Parse`]).
const SYSFS_PM_SIZE_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/pm_table_size",
    "/sys/kernel/ryzen_smu/pm_table_size",
];

/// `ryzen_smu` character device (fallback acquisition path).
///
/// Dead on the installed amkillam/ryzen_smu build (which registers only
/// the sysfs kobject); kept as 53XU-fork tolerance.
const DEV_NODE: &str = "/dev/ryzen_smu";

/// Driver name carried by [`TelemetryError::DriverMissing`].
const DRIVER: &str = "ryzen_smu";

/// Privilege hint carried by [`TelemetryError::InsufficientPrivilege`].
const PRIV_HINT: &str =
    "run as root or grant CAP_SYS_RAWIO; the ryzen_smu pm_table requires elevated access";

/// Hard cap on the PM-table blob read from the char device.
///
/// Real PM tables are on the order of tens of KiB; a payload growing past
/// this cap is treated as malformed ([`TelemetryError::Parse`]) instead of
/// being read unboundedly.
const MAX_PM_TABLE_SIZE: usize = 256 * 1024;

/// Raw SMU acquisition result (frozen public API, P2-03).
///
/// - `version`: the PM table's `TableVersionId` — the little-endian `u32`
///   from the driver's sibling `pm_table_version` sysfs attribute
///   (P5-15). The `pm_table` blob itself is a headerless f32 array and
///   carries no version word; on module builds without that attribute
///   (or via the char device fallback) it degrades to the legacy first
///   four bytes of `pm` ([`extract_version`]). A `0` sentinel when `pm` is
///   `None` and no version attribute is published.
/// - `pm`: the full raw PM-table blob, preserved verbatim for P2-04's
///   version-guarded parse — or `None` when the driver is loaded (the
///   kobject directory exists) but the `pm_table` attribute is not exposed
///   on this platform (e.g. Castle Peak / Threadripper 2990X, where the
///   RSMU/PM-table is not supported). A `None` PM table degrades the
///   PM-derived fields to an honest N/A while the SMN path (timings / GDM
///   / channel registers) still works — it is NOT "driver missing".
#[derive(Debug, Clone)]
pub struct SmuContext {
    /// PM-table `TableVersionId` (sibling `pm_table_version` attribute, or
    /// the legacy first word of `pm` as fallback). A `0` sentinel when
    /// `pm` is `None` and no version attribute is published.
    pub version: u32,
    /// Raw PM-table blob from the `ryzen_smu` driver (headerless f32
    /// array), or `None` when the driver is loaded but the `pm_table`
    /// attribute is not exposed on this platform (see the struct docs).
    pub pm: Option<Vec<u8>>,
}

/// Acquire the raw SMU PM-table blob and its version.
///
/// See the module docs for the acquisition strategy; this is the only
/// function in the crate that opens the `ryzen_smu` driver interfaces.
///
/// # Errors
///
/// - [`TelemetryError::UnsupportedHardware`] — non-AMD vendor (checked
///   before any file access) or a non-Linux platform.
/// - [`TelemetryError::DriverMissing`] — neither the sysfs path nor the
///   char device exists (module not loaded).
/// - [`TelemetryError::InsufficientPrivilege`] — the interface exists but
///   is not readable without elevation.
/// - [`TelemetryError::Parse`] — the sibling `pm_table_version` attribute is
///   truncated, the blob is empty or too short for the legacy version
///   fallback, or the sibling `pm_table_size` disagrees with the blob
///   length.
/// - [`TelemetryError::Io`] — any other raw I/O failure.
pub fn acquire() -> TelemetryResult<SmuContext> {
    // 1. Vendor gate: pure, so non-AMD hardware is rejected before any
    //    file access (plan §D4).
    let info = CpuInfo::detect();
    vendor_gate(&info)?;

    // 2. Platform gate: the driver interfaces are Linux-only.
    #[cfg(target_os = "linux")]
    {
        acquire_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(TelemetryError::UnsupportedHardware {
            vendor: "non-linux platform (ryzen_smu access requires Linux)".to_owned(),
        })
    }
}

/// Pure vendor gate: AMD passes; Intel or unknown vendors yield
/// [`TelemetryError::UnsupportedHardware`] with a human-readable vendor
/// string (the CPUID brand when known, else the vendor kind).
///
/// Performs no I/O — the gate must be able to reject before any file
/// access, and is unit-testable without root or a loaded driver.
pub fn vendor_gate(info: &CpuInfo) -> TelemetryResult<()> {
    match info.vendor {
        CpuVendor::Amd(_) => Ok(()),
        CpuVendor::Intel(_) | CpuVendor::Unknown => Err(TelemetryError::UnsupportedHardware {
            vendor: vendor_string(info),
        }),
    }
}

/// Vendor string for [`TelemetryError::UnsupportedHardware`]: the CPUID
/// brand when non-empty, else a vendor-kind fallback.
fn vendor_string(info: &CpuInfo) -> String {
    if !info.brand.is_empty() {
        info.brand.clone()
    } else {
        match info.vendor {
            CpuVendor::Amd(_) => "amd".to_owned(),
            CpuVendor::Intel(_) => "intel".to_owned(),
            CpuVendor::Unknown => "unknown".to_owned(),
        }
    }
}

/// Extract the SMU version word from a raw PM-table blob (legacy fallback).
///
/// P5-15 reconciliation: the current `ryzen_smu` model carries the
/// `TableVersionId` in the sibling `pm_table_version` sysfs attribute, not
/// in the blob (a headerless f32 array). This extraction — the first four
/// bytes, little-endian — is retained as the fallback for older module
/// builds that publish no sibling attribute, and for blobs acquired via
/// the char device; an unrecognized word degrades downstream to
/// `UnknownPmTableVersion`.
///
/// # Errors
///
/// [`TelemetryError::Parse`] when `blob.len() < 4`.
pub fn extract_version(blob: &[u8]) -> TelemetryResult<u32> {
    match blob.get(..4) {
        Some(h) => Ok(u32::from_le_bytes([h[0], h[1], h[2], h[3]])),
        None => Err(TelemetryError::Parse {
            detail: format!(
                "PM blob too short for version header: {} byte(s), need 4",
                blob.len()
            ),
        }),
    }
}

/// Parse the PM table's `TableVersionId` from the sibling
/// `pm_table_version` attribute payload: the first four bytes,
/// little-endian.
///
/// Trailing bytes (e.g. a newline on some builds) are ignored.
///
/// # Errors
///
/// [`TelemetryError::Parse`] when fewer than four bytes are present.
pub fn parse_version_word(bytes: &[u8]) -> TelemetryResult<u32> {
    match bytes.get(..4) {
        Some(h) => Ok(u32::from_le_bytes([h[0], h[1], h[2], h[3]])),
        None => Err(TelemetryError::Parse {
            detail: format!(
                "pm_table_version attribute too short: {} byte(s), need 4",
                bytes.len()
            ),
        }),
    }
}

/// Parse the PM table size from the sibling `pm_table_size` attribute
/// payload: the first eight bytes, little-endian `u64`.
///
/// Trailing bytes are ignored.
///
/// # Errors
///
/// [`TelemetryError::Parse`] when fewer than eight bytes are present.
pub fn parse_table_size(bytes: &[u8]) -> TelemetryResult<u64> {
    match bytes.get(..8) {
        Some(h) => {
            let mut b = [0u8; 8];
            b.copy_from_slice(h);
            Ok(u64::from_le_bytes(b))
        }
        None => Err(TelemetryError::Parse {
            detail: format!(
                "pm_table_size attribute too short: {} byte(s), need 8",
                bytes.len()
            ),
        }),
    }
}

/// Cross-check the sibling `pm_table_size` value against the blob length.
///
/// # Errors
///
/// [`TelemetryError::Parse`] when the two disagree.
pub fn check_size_matches(expected: u64, actual: usize) -> TelemetryResult<()> {
    if expected != actual as u64 {
        return Err(TelemetryError::Parse {
            detail: format!(
                "pm_table_size attribute reports {expected} byte(s) but the blob is {actual} byte(s)"
            ),
        });
    }
    Ok(())
}

/// Source the PM table's version word: the sibling `pm_table_version`
/// attribute payload when present (P5-15), else the legacy first word of
/// the blob.
///
/// Pure: no I/O, no panic.
pub fn version_from_sources(blob: &[u8], version_attr: Option<&[u8]>) -> TelemetryResult<u32> {
    match version_attr {
        Some(bytes) => parse_version_word(bytes),
        None => extract_version(blob),
    }
}

/// The five key PM-table f32 values' byte offsets (little-endian, 4 bytes
/// each): VDDCR_VDD, VDDCR_SOC, FCLK, UCLK, MCLK (the verified field layout,
/// `amd_pm`).
const PM_KEY_VALUE_OFFSETS: [usize; 5] = [0x0A0, 0x0B0, 0x0C0, 0x0C8, 0x0CC];

/// Read the five key PM-table f32 values as their raw `u32` bit patterns.
///
/// The PM blob is a headerless little-endian f32 array (byte offset = index
/// × 4). Each key value is read as its 4 LE bytes reinterpreted as a `u32`
/// (the f32 bit pattern — no float math, no NaN / overflow handling). A blob
/// too short for an offset yields `None` for that value (per-value
/// containment, no panic). Returns the values in [`PM_KEY_VALUE_OFFSETS`]
/// order: (vddcr_vdd, vddcr_soc, fclk, uclk, mclk).
pub fn read_pm_key_values(pm_blob: &[u8]) -> [Option<u32>; 5] {
    let mut out = [None; 5];
    for (i, &off) in PM_KEY_VALUE_OFFSETS.iter().enumerate() {
        out[i] = pm_blob
            .get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    }
    out
}

/// Classify a raw I/O error into a structured [`TelemetryError`].
///
/// - `NotFound` → [`TelemetryError::DriverMissing`] (the `ryzen_smu`
///   interface being reached for does not exist).
/// - `PermissionDenied` → [`TelemetryError::InsufficientPrivilege`] with
///   the resolution hint.
/// - Anything else → [`TelemetryError::Io`] (reconstructed from the raw OS
///   code, or kind + text, since `std::io::Error` is not `Clone` — the
///   same reconstruction as [`TelemetryError`]'s manual `Clone` impl).
pub fn classify_io_error(e: &std::io::Error) -> TelemetryError {
    match e.kind() {
        std::io::ErrorKind::NotFound => TelemetryError::DriverMissing { driver: DRIVER },
        std::io::ErrorKind::PermissionDenied => TelemetryError::InsufficientPrivilege {
            hint: PRIV_HINT,
        },
        _ => TelemetryError::Io(match e.raw_os_error() {
            Some(code) => std::io::Error::from_raw_os_error(code),
            None => std::io::Error::new(e.kind(), e.to_string()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Linux acquisition (sysfs -> char device). Non-Linux targets compile
// `acquire()` to the platform error above and exclude this whole section.
// ---------------------------------------------------------------------------

/// The outcome of a PM-table acquisition attempt at one candidate path.
/// Derived by [`classify_pm_state`] from the observed filesystem state.
#[cfg(target_os = "linux")]
enum PmAcquireState {
    /// The `pm_table` blob was read successfully.
    Available(Vec<u8>),
    /// The kobject directory exists (the driver is loaded) but the
    /// `pm_table` attribute is not exposed — the PM table is unsupported
    /// on this platform (e.g. Castle Peak / Threadripper 2990X). The SMN
    /// path still works; the PM fields degrade to an honest N/A.
    DriverPresentNoPm,
    /// The kobject directory is absent — the driver is truly not loaded
    /// at this candidate; try the next.
    DriverMissing,
    /// A raw I/O failure (not `NotFound`) — permission denied, EIO, etc.
    IoError(std::io::Error),
}

/// Determine the acquisition outcome given the observed filesystem state.
/// Pure function — testable without I/O.
#[cfg(target_os = "linux")]
fn classify_pm_state(
    dir_exists: bool,
    pm_table_read: Result<Vec<u8>, std::io::Error>,
) -> PmAcquireState {
    match pm_table_read {
        Ok(data) => PmAcquireState::Available(data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if dir_exists {
                // Driver is loaded (kobject dir exists) but PM table attribute
                // is not exposed. This happens on platforms where RSMU/PM-table
                // is not supported (e.g., Castle Peak / Threadripper 2990X).
                PmAcquireState::DriverPresentNoPm
            } else {
                // Directory absent — driver truly not loaded
                PmAcquireState::DriverMissing
            }
        }
        Err(e) => PmAcquireState::IoError(e),
    }
}

#[cfg(target_os = "linux")]
fn acquire_linux() -> TelemetryResult<SmuContext> {
    // Primary path: the sysfs PM-table blob — try the canonical kobject
    // name first, then the legacy one.
    let mut found: Option<TelemetryResult<SmuContext>> = None;
    for &path in SYSFS_PM_TABLE_CANDIDATES.iter() {
        let dir = kobj_dir_for(path);
        let dir_exists = dir.exists();
        let read_result = std::fs::read(path);
        match classify_pm_state(dir_exists, read_result) {
            PmAcquireState::Available(blob) => {
                found = Some(context_from_blob(blob));
                break;
            }
            PmAcquireState::DriverPresentNoPm => {
                found = Some(Ok(SmuContext {
                    version: read_pm_version_attr(&dir),
                    pm: None,
                }));
                break;
            }
            PmAcquireState::DriverMissing => {
                continue;
            }
            PmAcquireState::IoError(e) => {
                return Err(classify_io_error(&e));
            }
        }
    }
    match found {
        Some(result) => result,
        None => chardev_acquire(),
    }
}

/// The kobject directory that owns a `pm_table` attribute path (its
/// parent). Used to tell "the driver is loaded" (the directory exists)
/// from "the driver is missing" (the directory is absent) when the
/// `pm_table` attribute itself is not exposed on the platform.
#[cfg(target_os = "linux")]
fn kobj_dir_for(pm_table_path: &str) -> std::path::PathBuf {
    std::path::Path::new(pm_table_path)
        .parent()
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

/// Read the sibling `pm_table_version` attribute out of a kobject
/// directory, returning the parsed `TableVersionId` word, or `0` (the
/// "no version published" sentinel) when the attribute is absent or
/// unreadable.
///
/// Used on the PM-table-unavailable path (the driver is loaded but the
/// `pm_table` attribute is not exposed): the version is carried for
/// display only — [`crate::amd_pm::parse`] short-circuits to an all-zero
/// snapshot before the version gate when `pm` is `None`, so a `0` here is
/// harmless.
#[cfg(target_os = "linux")]
fn read_pm_version_attr(dir: &std::path::Path) -> u32 {
    match std::fs::read(dir.join("pm_table_version")) {
        Ok(bytes) => parse_version_word(&bytes).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Fallback path: open the `ryzen_smu` char device read-only and fetch
/// the PM table through the driver's `read(2)` interface.
///
/// The installed amkillam/ryzen_smu build registers only the sysfs
/// kobject (no `/dev/ryzen_smu` node), so this path is **dead on that
/// module** and is kept purely as 53XU-fork tolerance: an absent node
/// degrades to [`TelemetryError::DriverMissing`].
#[cfg(target_os = "linux")]
fn chardev_acquire() -> TelemetryResult<SmuContext> {
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    let fd = nix::fcntl::open(DEV_NODE, OFlag::O_RDONLY, Mode::empty()).map_err(nix_err)?;
    let _guard = FdGuard(fd);
    let blob = read_pm_table(fd)?;
    context_from_blob(blob)
}

/// Read the full PM table from the char device, looping until EOF.
///
/// An empty payload or one growing past [`MAX_PM_TABLE_SIZE`] is a
/// malformed blob ([`TelemetryError::Parse`]); syscall failures are
/// classified by [`classify_io_error`].
#[cfg(target_os = "linux")]
fn read_pm_table(fd: std::os::unix::io::RawFd) -> TelemetryResult<Vec<u8>> {
    let mut blob: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        let n = nix::unistd::read(fd, &mut chunk).map_err(nix_err)?;
        if n == 0 {
            break; // EOF
        }
        blob.extend_from_slice(&chunk[..n]);
        if blob.len() > MAX_PM_TABLE_SIZE {
            return Err(TelemetryError::Parse {
                detail: format!(
                    "PM table exceeds {MAX_PM_TABLE_SIZE}-byte read cap (got {} bytes)",
                    blob.len()
                ),
            });
        }
    }
    if blob.is_empty() {
        return Err(TelemetryError::Parse {
            detail: "empty PM table blob from /dev/ryzen_smu".to_owned(),
        });
    }
    Ok(blob)
}

/// Build a [`SmuContext`] from a fetched PM-table blob (P5-15 model).
///
/// Version sourcing: the sibling `pm_table_version` attribute (first
/// candidate that exists) decodes the `TableVersionId`; when no candidate
/// exists (older module builds, or the char device) the version degrades
/// to the legacy first word of the blob. The sibling `pm_table_size`
/// attribute must equal the blob length when present; a disagreement is
/// [`TelemetryError::Parse`] (never a panic).
#[cfg(target_os = "linux")]
fn context_from_blob(blob: Vec<u8>) -> TelemetryResult<SmuContext> {
    let version_attr = read_sibling(&SYSFS_PM_VERSION_CANDIDATES)?;
    let version = version_from_sources(&blob, version_attr.as_deref())?;
    if let Some(size_attr) = read_sibling(&SYSFS_PM_SIZE_CANDIDATES)? {
        let expected = parse_table_size(&size_attr)?;
        check_size_matches(expected, blob.len())?;
    }
    Ok(SmuContext { version, pm: Some(blob) })
}

/// Read the first existing sibling sysfs attribute in `candidates`.
///
/// NotFound on a candidate skips to the next one (older module builds
/// omit the sibling attributes); PermissionDenied on an existing file is
/// reported as [`TelemetryError::InsufficientPrivilege`]; any other I/O
/// error as [`TelemetryError::Io`]. Returns `None` when no candidate
/// exists.
#[cfg(target_os = "linux")]
fn read_sibling(candidates: &[&str]) -> TelemetryResult<Option<Vec<u8>>> {
    for &path in candidates {
        match std::fs::read(path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(e) => match classify_io_error(&e) {
                TelemetryError::DriverMissing { .. } => continue,
                other => return Err(other),
            },
        }
    }
    Ok(None)
}

/// Turn a `nix` syscall error into a structured [`TelemetryError`] via
/// [`classify_io_error`] (nix `Errno` -> `std::io::Error` -> classification).
#[cfg(target_os = "linux")]
fn nix_err(e: nix::errno::Errno) -> TelemetryError {
    classify_io_error(&std::io::Error::from(e))
}

/// RAII guard: closes the `ryzen_smu` fd exactly once on scope exit.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::{AmdZen, IntelGen};

    /// (a, legacy fallback) The version word is the first four bytes,
    /// little-endian — the fallback for builds without the sibling
    /// `pm_table_version` attribute.
    #[test]
    fn extract_version_le_header() {
        let blob = [0x01, 0x0B, 0x07, 0x00, 0, 0, 0, 0];
        assert_eq!(extract_version(&blob), Ok(0x0007_0B01));
    }

    /// (a, legacy fallback) A blob shorter than four bytes is a `Parse`
    /// error (never a panic, never an out-of-bounds read).
    #[test]
    fn extract_version_short_blob_is_parse_error() {
        for blob in [&[] as &[u8], &[0x11], &[0x11, 0x22], &[0x11, 0x22, 0x33]] {
            assert!(
                matches!(extract_version(blob), Err(TelemetryError::Parse { .. })),
                "expected Parse error for {blob:?}"
            );
        }
    }

    /// (a, P5-15) The sibling `pm_table_version` attribute decodes to the
    /// `TableVersionId` from its first four bytes, little-endian; trailing
    /// bytes (e.g. a newline) are ignored.
    #[test]
    fn parse_version_word_le() {
        assert_eq!(parse_version_word(&[0x05, 0x08, 0x38, 0x00]), Ok(0x0038_0805));
        assert_eq!(
            parse_version_word(&[0x03, 0x08, 0x2D, 0x00, 0x0A]),
            Ok(0x002D_0803)
        );
    }

    /// (a, P5-15) A `pm_table_version` payload shorter than four bytes is
    /// a `Parse` error (never a panic).
    #[test]
    fn parse_version_word_short_is_parse_error() {
        for bytes in [&[] as &[u8], &[0x11], &[0x11, 0x22], &[0x11, 0x22, 0x33]] {
            assert!(
                matches!(parse_version_word(bytes), Err(TelemetryError::Parse { .. })),
                "expected Parse error for {bytes:?}"
            );
        }
    }

    /// (a, P5-15) The sibling `pm_table_size` attribute decodes to the
    /// table size from its first eight bytes, little-endian; trailing
    /// bytes are ignored.
    #[test]
    fn parse_table_size_le() {
        let mut b = [0u8; 8];
        b.copy_from_slice(&2288u64.to_le_bytes());
        assert_eq!(parse_table_size(&b), Ok(2288));

        let mut b2 = vec![0u8; 9];
        b2[..8].copy_from_slice(&2288u64.to_le_bytes());
        b2[8] = 0x0A;
        assert_eq!(parse_table_size(&b2), Ok(2288));
    }

    /// (a, P5-15) A `pm_table_size` payload shorter than eight bytes is a
    /// `Parse` error (never a panic).
    #[test]
    fn parse_table_size_short_is_parse_error() {
        for bytes in [&[] as &[u8], &[1u8, 2, 3, 4, 5, 6, 7]] {
            assert!(
                matches!(parse_table_size(bytes), Err(TelemetryError::Parse { .. })),
                "expected Parse error for {bytes:?}"
            );
        }
    }

    /// (a, P5-15) The size cross-check passes on agreement and yields
    /// `Parse` on disagreement (never a panic).
    #[test]
    fn size_cross_check() {
        assert_eq!(check_size_matches(2288, 2288), Ok(()));
        assert!(matches!(
            check_size_matches(2288, 2287),
            Err(TelemetryError::Parse { .. })
        ));
        assert!(matches!(
            check_size_matches(0, 1),
            Err(TelemetryError::Parse { .. })
        ));
    }

    /// (a, P5-15) Version sourcing prefers the sibling attribute when
    /// present (decoding it, ignoring the blob's first word) and falls
    /// back to the blob's first word when the attribute is absent; a
    /// truncated attribute payload is `Parse` (never a panic).
    #[test]
    fn version_sourcing_sibling_preferred_then_fallback() {
        let blob = [0x01, 0x0B, 0x07, 0x00, 0, 0, 0, 0];
        assert_eq!(
            version_from_sources(&blob, Some(&[0x05, 0x08, 0x38, 0x00])),
            Ok(0x0038_0805)
        );
        assert_eq!(version_from_sources(&blob, None), Ok(0x0007_0B01));
        assert!(matches!(
            version_from_sources(&blob, Some(&[0x05, 0x08])),
            Err(TelemetryError::Parse { .. })
        ));
    }

    /// (b) Error classification by kind: NotFound -> DriverMissing,
    /// PermissionDenied -> InsufficientPrivilege, other -> Io.
    #[test]
    fn classify_io_error_kinds() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
        assert_eq!(
            classify_io_error(&e),
            TelemetryError::DriverMissing { driver: "ryzen_smu" }
        );

        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        assert_eq!(
            classify_io_error(&e),
            TelemetryError::InsufficientPrivilege { hint: PRIV_HINT }
        );

        let e = std::io::Error::other("boom");
        assert!(matches!(classify_io_error(&e), TelemetryError::Io(_)));
    }

    /// (b) Raw-OS-code construction classifies the same way (POSIX errno
    /// 2 = ENOENT, 13 = EACCES).
    #[test]
    fn classify_io_error_raw_os_codes() {
        assert_eq!(
            classify_io_error(&std::io::Error::from_raw_os_error(2)),
            TelemetryError::DriverMissing { driver: "ryzen_smu" }
        );
        assert_eq!(
            classify_io_error(&std::io::Error::from_raw_os_error(13)),
            TelemetryError::InsufficientPrivilege { hint: PRIV_HINT }
        );
    }

    /// (d) The pure vendor gate: non-AMD vendors yield
    /// `UnsupportedHardware` (no I/O, no root, no driver required); AMD
    /// passes.
    #[test]
    fn vendor_gate_non_amd_yields_unsupported_hardware() {
        let intel = CpuInfo {
            vendor: CpuVendor::Intel(IntelGen::Skylake),
            brand: "GenuineIntel test cpu".to_owned(),
        };
        assert_eq!(
            vendor_gate(&intel),
            Err(TelemetryError::UnsupportedHardware {
                vendor: "GenuineIntel test cpu".to_owned()
            })
        );

        let unknown = CpuInfo {
            vendor: CpuVendor::Unknown,
            brand: String::new(),
        };
        assert_eq!(
            vendor_gate(&unknown),
            Err(TelemetryError::UnsupportedHardware {
                vendor: "unknown".to_owned()
            })
        );

        let amd = CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "AMD Ryzen 9 5950X".to_owned(),
        };
        assert_eq!(vendor_gate(&amd), Ok(()));
    }

    /// (c) `acquire()` on this host is graceful: it returns a `Result`
    /// without panicking — `Ok` or one of the structured `Err` variants
    /// (never an unlisted variant, never a panic). Which variant is
    /// host-state dependent (driver loaded? privilege?), so only the shape
    /// is asserted.
    #[test]
    fn acquire_on_this_host_is_graceful() {
        let res = acquire();
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
            "acquire() returned an unexpected variant: {res:?}"
        );
    }

    /// (e) The sysfs candidate lists pin the verified upstream kobject
    /// (`ryzen_smu_drv`, amkillam/ryzen_smu `drv.c`) before the legacy
    /// directory name — for the blob and both sibling attributes — so the
    /// correct paths are always tried first.
    #[test]
    fn sysfs_candidates_try_verified_path_first() {
        assert_eq!(
            SYSFS_PM_TABLE_CANDIDATES,
            [
                "/sys/kernel/ryzen_smu_drv/pm_table",
                "/sys/kernel/ryzen_smu/pm_table"
            ]
        );
        assert_eq!(
            SYSFS_PM_VERSION_CANDIDATES,
            [
                "/sys/kernel/ryzen_smu_drv/pm_table_version",
                "/sys/kernel/ryzen_smu/pm_table_version"
            ]
        );
        assert_eq!(
            SYSFS_PM_SIZE_CANDIDATES,
            [
                "/sys/kernel/ryzen_smu_drv/pm_table_size",
                "/sys/kernel/ryzen_smu/pm_table_size"
            ]
        );
    }

    /// (a) The five key PM-table f32 values are read as their raw LE u32 bit
    /// patterns from the verified offsets; a short blob degrades the
    /// out-of-bounds values to `None` (per-value containment, no panic).
    #[test]
    fn read_pm_key_values_reads_the_five_f32_bit_patterns() {
        // A blob long enough for every key offset (>= 0x0C8 + 4 = 208 bytes).
        let mut blob = vec![0u8; 0x0D0];
        let patterns = [
            0x3E4C_CCCDu32, // VDDCR_VDD @ 0x0A0
            0x3EF2_CCCCu32, // VDDCR_SOC @ 0x0B0
            0x40F0_0000u32, // FCLK @ 0x0C0
            0x40C8_0000u32, // UCLK @ 0x0C8
            0x40C8_0000u32, // MCLK @ 0x0CC
        ];
        for (pattern, off) in patterns.iter().zip(PM_KEY_VALUE_OFFSETS.iter()) {
            blob[*off..*off + 4].copy_from_slice(&pattern.to_le_bytes());
        }
        assert_eq!(read_pm_key_values(&blob), patterns.map(Some));

        // A blob long enough for only the first three offsets degrades the
        // last two to `None` (the earlier ones still read, as zeros here).
        let short = vec![0u8; 0x0C0 + 4];
        assert_eq!(
            read_pm_key_values(&short),
            [Some(0), Some(0), Some(0), None, None]
        );

        // An empty blob degrades everything to `None`.
        assert_eq!(read_pm_key_values(&[]), [None, None, None, None, None]);
    }

    /// (a) `kobj_dir_for` derives the kobject directory (the parent) from a
    /// `pm_table` attribute path — the canonical `ryzen_smu_drv` and the
    /// legacy `ryzen_smu` both resolve to their owning directory.
    #[cfg(target_os = "linux")]
    #[test]
    fn kobj_dir_for_derives_the_parent_directory() {
        assert_eq!(
            kobj_dir_for("/sys/kernel/ryzen_smu_drv/pm_table"),
            std::path::PathBuf::from("/sys/kernel/ryzen_smu_drv")
        );
        assert_eq!(
            kobj_dir_for("/sys/kernel/ryzen_smu/pm_table"),
            std::path::PathBuf::from("/sys/kernel/ryzen_smu")
        );
    }

    /// (a) `read_pm_version_attr` parses the sibling `pm_table_version`
    /// attribute out of a kobject directory, and degrades to the `0`
    /// sentinel when the attribute is absent (the PM-table-unavailable
    /// path). Uses a process-unique temp directory with cleanup.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_pm_version_attr_parses_sibling_or_zero_sentinel() {
        let dir = std::env::temp_dir()
            .join(format!("ramsleuth_amd_smu_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test: create temp dir");
        // Absent attribute -> the `0` sentinel.
        assert_eq!(read_pm_version_attr(&dir), 0);
        // Present attribute (4-byte LE `TableVersionId` 0x0038_0805) -> parsed.
        let mut ver = [0u8; 4];
        ver.copy_from_slice(&0x0038_0805u32.to_le_bytes());
        std::fs::write(dir.join("pm_table_version"), ver).expect("test: write version");
        assert_eq!(read_pm_version_attr(&dir), 0x0038_0805);
        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    // F-03: `classify_pm_state` is a pure function — the branch decision
    // is testable without I/O.

    #[cfg(target_os = "linux")]
    #[test]
    fn pm_state_dir_exists_no_pm() {
        assert!(matches!(
            classify_pm_state(true, Err(std::io::Error::from(std::io::ErrorKind::NotFound))),
            PmAcquireState::DriverPresentNoPm
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pm_state_dir_absent_no_pm() {
        assert!(matches!(
            classify_pm_state(false, Err(std::io::Error::from(std::io::ErrorKind::NotFound))),
            PmAcquireState::DriverMissing
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pm_state_dir_exists_pm_present() {
        assert!(matches!(
            classify_pm_state(true, Ok(vec![0x01, 0x02])),
            PmAcquireState::Available(_)
        ));
    }
}
