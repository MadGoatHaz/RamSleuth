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
//!    the PM table through the driver's `read(2)` interface. The
//!    `ryzen_smu` uAPI (53XU/ryzen_smu) exposes the table via `read()`; the
//!    device's only ioctl, `RSMU_IOC_GET_VERSION`
//!    (`_IOR('R', 0x01, u32)`), reports the PMFW version. Missing device
//!    node → [`TelemetryError::DriverMissing`]; permission error →
//!    [`TelemetryError::InsufficientPrivilege`].
//! 4. **Version extraction:** the SMU version word is the first four bytes
//!    of the blob, little-endian ([`extract_version`]); a shorter blob is
//!    [`TelemetryError::Parse`]. The `nix` `ioctl` feature is enabled per
//!    plan D6 for the companion version ioctl.
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

/// `ryzen_smu` character device (fallback acquisition path).
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
/// - `version`: the SMU/PMFW version word — the first four bytes of `pm`,
///   little-endian (see [`extract_version`]).
/// - `pm`: the full raw PM-table blob, preserved verbatim for P2-04's
///   version-guarded parse.
#[derive(Debug, Clone)]
pub struct SmuContext {
    /// SMU/PMFW version word (first 4 bytes of the blob, little-endian).
    pub version: u32,
    /// Raw PM-table blob from the `ryzen_smu` driver.
    pub pm: Vec<u8>,
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
/// - [`TelemetryError::Parse`] — the blob is empty or too short to carry
///   the version header.
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

/// Extract the SMU version word from a raw PM-table blob.
///
/// Frozen contract (P2-03): the version is the first four bytes of the
/// blob, little-endian. A blob shorter than four bytes is malformed.
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

#[cfg(target_os = "linux")]
fn acquire_linux() -> TelemetryResult<SmuContext> {
    // Primary path: the sysfs PM-table blob — try the canonical kobject
    // name first, then the legacy one.
    let mut found: Option<TelemetryResult<SmuContext>> = None;
    for &path in SYSFS_PM_TABLE_CANDIDATES.iter() {
        match std::fs::read(path) {
            Ok(blob) => {
                found = Some(blob_to_context(blob));
                break;
            }
            Err(e) => match classify_io_error(&e) {
                // Absent at this candidate -> try the next one; if it was
                // the last, fall through to the char device (the module
                // may expose only one of the two interfaces).
                TelemetryError::DriverMissing { .. } => continue,
                // Present but unreadable (-> InsufficientPrivilege) or some
                // other raw failure (-> Io): report as classified.
                other => return Err(other),
            },
        }
    }
    match found {
        Some(result) => result,
        None => chardev_acquire(),
    }
}

/// Fallback path: open the `ryzen_smu` char device read-only and fetch the
/// PM table through the driver's `read(2)` interface.
#[cfg(target_os = "linux")]
fn chardev_acquire() -> TelemetryResult<SmuContext> {
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;

    let fd = nix::fcntl::open(DEV_NODE, OFlag::O_RDONLY, Mode::empty()).map_err(nix_err)?;
    let _guard = FdGuard(fd);
    let blob = read_pm_table(fd)?;
    blob_to_context(blob)
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

/// Convert a fetched PM-table blob into a [`SmuContext`], extracting the
/// version word from the blob header.
#[cfg(target_os = "linux")]
fn blob_to_context(blob: Vec<u8>) -> TelemetryResult<SmuContext> {
    let version = extract_version(&blob)?;
    Ok(SmuContext { version, pm: blob })
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

    /// (a) The version word is the first four bytes, little-endian.
    #[test]
    fn extract_version_le_header() {
        let blob = [0x01, 0x0B, 0x07, 0x00, 0, 0, 0, 0];
        assert_eq!(extract_version(&blob), Ok(0x0007_0B01));
    }

    /// (a) A blob shorter than four bytes is a `Parse` error (never a
    /// panic, never an out-of-bounds read).
    #[test]
    fn extract_version_short_blob_is_parse_error() {
        for blob in [&[] as &[u8], &[0x11], &[0x11, 0x22], &[0x11, 0x22, 0x33]] {
            assert!(
                matches!(extract_version(blob), Err(TelemetryError::Parse { .. })),
                "expected Parse error for {blob:?}"
            );
        }
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

    /// (e) The sysfs candidate list pins the verified upstream kobject
    /// (`ryzen_smu_drv`, amkillam/ryzen_smu `drv.c`) before the legacy
    /// directory name, so the correct path is always tried first.
    #[test]
    fn sysfs_candidates_try_verified_path_first() {
        assert_eq!(
            SYSFS_PM_TABLE_CANDIDATES,
            [
                "/sys/kernel/ryzen_smu_drv/pm_table",
                "/sys/kernel/ryzen_smu/pm_table"
            ]
        );
    }
}
