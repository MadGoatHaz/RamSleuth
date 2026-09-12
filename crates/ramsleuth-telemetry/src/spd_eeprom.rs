//! Unprivileged raw SPD EEPROM image acquisition (P2-08, `ee1004` via sysfs).
//!
//! This module reads the full raw SPD image of every DIMM the kernel's
//! `ee1004` I2C EEPROM driver has bound, **without any privilege**: the
//! driver exposes each device's entire contents as a world-readable sysfs
//! attribute (`eeprom`), so plain `std::fs` is all this module needs — no
//! root, no `CAP_SYS_RAWIO`, no user-space `/dev/i2c-*` access, no
//! `unsafe`, no new dependencies.
//!
//! # Strategy (guarded, read-only, no-panic)
//!
//! 1. **Enumerate** the bound clients under
//!    `/sys/bus/i2c/drivers/ee1004/` — the kernel places one
//!    `<bus>-<addr>` symlink per bound device (e.g. `5-0052`). Driver-
//!    directory extras (`module`, `bind`, `unbind`, `uevent`) never parse
//!    as client names and are dropped by [`parse_device_name`].
//! 2. **Read** each client's `eeprom` attribute — the full raw image:
//!    512 bytes for DDR4 (4 Kbit part), 1024 bytes for DDR5 (8 Kbit
//!    part). Each image is wrapped as a frozen [`SpdImage`] carrying the
//!    device's I2C address as its module `index` (`"5-0052"` → `0x52`).
//! 3. **Never fail the process** (plan D5):
//!    - driver not loaded, or its directory not listable → `Ok(vec![])`
//!      plus a one-line stderr warning;
//!    - a device present but its `eeprom` unreadable → skipped with a
//!      one-line warning (one bad DIMM must not drop the others);
//!    - an image whose length is neither 512 nor 1024 → skipped with a
//!      warning (not a layout this chunk serves).
//!
//! The caller (the P2-10 facade) renders an empty result as `SPD: N/A`;
//! the P2-09 decoder consumes the raw images (JEP106 makers, ranks,
//! part/serial, XMP/EXPO profiles).
//!
//! # Safety
//!
//! No `unsafe`: pure `std::fs` reads of world-readable sysfs files.
//!
//! # Platform
//!
//! The sysfs walk is `#[cfg(target_os = "linux")]`. On non-Linux targets
//! [`acquire()`] returns `Ok(vec![])` (this module still compiles; the
//! pure helpers [`parse_device_name`] / [`validate_image`] are
//! platform-independent).

use crate::error::TelemetryResult;

/// The `ee1004` driver's sysfs directory: one `<bus>-<addr>` symlink per
/// bound device, plus driver-level extras (`module`, `bind`, `unbind`,
/// `uevent`) that [`parse_device_name`] rejects.
const EE1004_DRIVER_DIR: &str = "/sys/bus/i2c/drivers/ee1004";

/// Per-device sysfs attribute holding the full raw EEPROM contents (the
/// driver reads the whole part, not a window).
const EEPROM_ATTR: &str = "eeprom";

/// Full size of a DDR4 SPD image (4 Kbit EEPROM).
const SPD_DDR4_LEN: usize = 512;

/// Full size of a DDR5 SPD image (8 Kbit EEPROM).
const SPD_DDR5_LEN: usize = 1024;

// ---------------------------------------------------------------------------
// Frozen public API
// ---------------------------------------------------------------------------

/// A raw SPD EEPROM image read from one bound `ee1004` device.
///
/// Frozen (P2-08): the P2-09 decoder consumes these (JEP106 module/die
/// makers, ranks, part number/serial, XMP/EXPO profiles) and the P2-10
/// facade carries them into `SystemMemoryTelemetry.spd`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpdImage {
    /// Module index: the device's I2C address parsed from the client name
    /// (`"5-0052"` → `0x52`).
    pub index: u8,
    /// The full raw image: 512 bytes (DDR4) or 1024 bytes (DDR5) — see
    /// [`validate_image`].
    pub data: Vec<u8>,
}

/// Acquire the raw SPD image of every bound `ee1004` device.
///
/// The unprivileged entry point of the SPD telemetry branch: pure sysfs
/// reads (see the module docs for the enumeration strategy). Returns:
///
/// - `Ok(vec![])` when the driver is absent, no device is bound, or the
///   platform is non-Linux (a one-line warning goes to stderr in the
///   first two cases) — the caller renders this as "no SPD data", never
///   an error;
/// - `Ok([SpdImage; N])` with one image per successfully read device.
///
/// Per-device read failures are skipped with a warning and **not**
/// returned as `Err`: one bad DIMM must not drop the others (plan D5),
/// and this module can never fail the process.
pub fn acquire() -> TelemetryResult<Vec<SpdImage>> {
    #[cfg(target_os = "linux")]
    {
        acquire_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("warning: SPD EEPROM acquisition is Linux-only; no SPD data");
        finalize_images(&[])
    }
}

/// Extract the module index from an `ee1004` sysfs client name.
///
/// Bound clients are named `<bus>-<addr>` (e.g. `"5-0052"`: I2C bus 5,
/// 7-bit address `0x52`). The index is the address part parsed as a hex
/// `u8` — `"10-0050"` → `Some(0x50)`, `"5-0052"` → `Some(0x52)`.
///
/// Returns `None` for driver-directory extras (`"module"`, `"bind"`, …)
/// and malformed names (missing dash, non-numeric bus, non-hex address,
/// address overflow) — those are dropped, never errors, never panics.
pub fn parse_device_name(name: &str) -> Option<u8> {
    let (bus, addr) = name.split_once('-')?;
    // The bus number is decimal digits (kernel `%u`); this rejects the
    // driver-directory extras and any non-client name.
    if bus.is_empty() || !bus.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    u8::from_str_radix(addr, 16).ok()
}

/// `true` when `data` is exactly a full SPD image: 512 bytes (DDR4) or
/// 1024 bytes (DDR5).
///
/// Length is the whole test at this layer: DDR4-vs-DDR5 *signature*
/// classification (header bytes) belongs to the P2-09 decoder.
pub fn validate_image(data: &[u8]) -> bool {
    matches!(data.len(), SPD_DDR4_LEN | SPD_DDR5_LEN)
}

/// Pure finalization: wraps each successfully read device as a frozen
/// [`SpdImage`], keeping only images of a valid SPD length
/// ([`validate_image`]).
///
/// Given an empty device list this yields `Ok(vec![])` — the "no SPD
/// data" result the caller renders as N/A.
pub(crate) fn finalize_images(devices: &[(u8, Vec<u8>)]) -> TelemetryResult<Vec<SpdImage>> {
    Ok(devices
        .iter()
        .filter(|(_, data)| validate_image(data))
        .map(|(index, data)| SpdImage {
            index: *index,
            data: data.clone(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Linux acquisition (sysfs walk). Non-Linux targets compile `acquire()` to
// the empty result above and exclude this whole section.
// ---------------------------------------------------------------------------

/// The enumeration state of the `ee1004` driver directory.
#[cfg(target_os = "linux")]
enum DriverDir {
    /// The directory was listed; its entry names (client symlinks plus
    /// driver-directory extras, the latter filtered by
    /// [`parse_device_name`]).
    Entries(Vec<String>),
    /// The directory is absent: the driver is not loaded — the normal
    /// "no SPD data" case.
    NotLoaded,
    /// The directory is present but could not be listed.
    Unlistable(std::io::Error),
}

/// List the `ee1004` driver directory, classifying "absent" (driver not
/// loaded) from other I/O failures.
#[cfg(target_os = "linux")]
fn enumerate_driver_dir() -> DriverDir {
    match std::fs::read_dir(EE1004_DRIVER_DIR) {
        Ok(rd) => DriverDir::Entries(
            rd.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect(),
        ),
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => DriverDir::NotLoaded,
            _ => DriverDir::Unlistable(e),
        },
    }
}

/// Read the full raw `eeprom` image of one bound device by client name.
#[cfg(target_os = "linux")]
fn read_eeprom(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(format!("{EE1004_DRIVER_DIR}/{name}/{EEPROM_ATTR}"))
}

/// The Linux acquisition path: enumerate → read → finalize (see the
/// module docs for the strategy and failure policy).
#[cfg(target_os = "linux")]
fn acquire_linux() -> TelemetryResult<Vec<SpdImage>> {
    let entries = match enumerate_driver_dir() {
        DriverDir::Entries(entries) => entries,
        DriverDir::NotLoaded => {
            eprintln!("warning: SPD EEPROM driver 'ee1004' is not loaded; no SPD data");
            return Ok(Vec::new());
        }
        DriverDir::Unlistable(e) => {
            eprintln!("warning: cannot enumerate {EE1004_DRIVER_DIR}: {e}; no SPD data");
            return Ok(Vec::new());
        }
    };
    // One bad device must not drop the others (D5): an unreadable
    // `eeprom` file is skipped with a one-line warning, never an error,
    // and never a process failure.
    let mut devices: Vec<(u8, Vec<u8>)> = Vec::new();
    for name in &entries {
        let Some(index) = parse_device_name(name) else {
            continue; // driver-directory extra (`module`, `bind`, …)
        };
        match read_eeprom(name) {
            Ok(data) => devices.push((index, data)),
            Err(e) => eprintln!("warning: SPD EEPROM '{name}' unreadable: {e}; skipped"),
        }
    }
    if devices.is_empty() {
        eprintln!(
            "warning: no bound SPD EEPROM (ee1004) devices under {EE1004_DRIVER_DIR}; no SPD data"
        );
        return Ok(Vec::new());
    }
    let skipped = devices.iter().filter(|(_, data)| !validate_image(data)).count();
    if skipped > 0 {
        eprintln!(
            "warning: {skipped} SPD image(s) skipped (length neither {SPD_DDR4_LEN} nor {SPD_DDR5_LEN} bytes)"
        );
    }
    finalize_images(&devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TelemetryError;

    /// (a) The index is the hex address part of a `<bus>-<addr>` client
    /// name (the kernel names bound devices `%u-%04x`).
    #[test]
    fn parse_device_name_extracts_hex_address() {
        assert_eq!(parse_device_name("10-0050"), Some(0x50));
        assert_eq!(parse_device_name("5-0052"), Some(0x52));
        assert_eq!(parse_device_name("5-0053"), Some(0x53));
        assert_eq!(parse_device_name("7-0051"), Some(0x51));
        assert_eq!(parse_device_name("0-0050"), Some(0x50));
        // Padded and unpadded address forms parse identically.
        assert_eq!(parse_device_name("12-007f"), Some(0x7f));
        assert_eq!(parse_device_name("12-7f"), Some(0x7f));
        assert_eq!(parse_device_name("3-0000"), Some(0x00));
    }

    /// (a) Malformed names — driver-directory extras, missing dash,
    /// non-numeric bus, non-hex address, address overflow — all yield
    /// `None` (dropped), never an error, never a panic.
    #[test]
    fn parse_device_name_rejects_garbage() {
        // Driver-directory extras (present alongside the client symlinks).
        assert_eq!(parse_device_name("module"), None);
        assert_eq!(parse_device_name("bind"), None);
        assert_eq!(parse_device_name("unbind"), None);
        assert_eq!(parse_device_name("uevent"), None);
        // Malformed client names.
        assert_eq!(parse_device_name("-0050"), None); // empty bus part
        assert_eq!(parse_device_name("abc-0050"), None); // non-numeric bus
        assert_eq!(parse_device_name("10-00zz"), None); // non-hex address
        assert_eq!(parse_device_name("10-100"), None); // overflows u8
        assert_eq!(parse_device_name("10"), None); // no dash
        assert_eq!(parse_device_name(""), None); // empty name
    }

    /// (b) Accepts exactly the two full SPD image lengths and rejects
    /// every other length (swept across both boundaries).
    #[test]
    fn validate_image_only_512_and_1024() {
        assert!(validate_image(&vec![0u8; SPD_DDR4_LEN]));
        assert!(validate_image(&vec![0u8; SPD_DDR5_LEN]));
        assert!(!validate_image(&[]));
        assert!(!validate_image(&vec![0u8; SPD_DDR4_LEN - 1]));
        assert!(!validate_image(&vec![0u8; SPD_DDR4_LEN + 1]));
        assert!(!validate_image(&vec![0u8; SPD_DDR5_LEN - 1]));
        assert!(!validate_image(&vec![0u8; SPD_DDR5_LEN + 1]));
        // 16 Kbit / 32 Kbit parts (2048/4096 B) are not served layouts.
        assert!(!validate_image(&vec![0u8; 2048]));
        assert!(!validate_image(&vec![0u8; 4096]));
    }

    /// (d) The empty case: the pure finalizer given an empty device list
    /// yields `Ok(vec![])` — the "no SPD data" result (no I/O involved).
    #[test]
    fn finalize_images_empty_list_yields_ok_empty() {
        assert_eq!(finalize_images(&[]), Ok(Vec::new()));
    }

    /// (d) The finalizer keeps only valid-length images (preserving
    /// index order) and drops the rest.
    #[test]
    fn finalize_images_keeps_only_valid_lengths() {
        let devices = vec![
            (0x50u8, vec![0xAAu8; SPD_DDR4_LEN]),
            (0x51, vec![0xBBu8; 100]), // wrong length: dropped
            (0x52, vec![0xCCu8; SPD_DDR5_LEN]),
        ];
        let images = finalize_images(&devices).expect("the finalizer is infallible");
        assert_eq!(
            images,
            vec![
                SpdImage {
                    index: 0x50,
                    data: vec![0xAAu8; SPD_DDR4_LEN],
                },
                SpdImage {
                    index: 0x52,
                    data: vec![0xCCu8; SPD_DDR5_LEN],
                },
            ]
        );
    }

    /// The frozen `SpdImage` derives: `Clone` + `PartialEq` + `Debug`.
    #[test]
    fn spd_image_derives_clone_partial_eq_debug() {
        let a = SpdImage { index: 0x50, data: vec![1, 2, 3] };
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(a, SpdImage { index: 0x51, data: vec![1, 2, 3] });
        assert_ne!(a, SpdImage { index: 0x50, data: vec![1, 2, 4] });
        assert!(format!("{a:?}").contains("SpdImage"));
    }

    /// (c) `acquire()` on this host returns a `Result` without
    /// panicking. SPD presence is host-dependent, so the outcome is
    /// asserted to be either `Ok(_)` (possibly empty) or `Err(Io(..))`
    /// — never anything else.
    #[test]
    fn acquire_on_this_host_returns_a_result() {
        let res = acquire();
        assert!(
            matches!(res, Ok(_) | Err(TelemetryError::Io(_))),
            "acquire() must be Ok (possibly empty) or Err(Io(..)), got: {res:?}"
        );
    }

    /// (c) Host-specific exit criterion (plan P2-08): this DDR4 host runs
    /// `ee1004` with two bound devices (`5-0052` / `5-0053`), so
    /// `acquire()` returns 1–2 raw 512-byte images — never an error.
    #[cfg(target_os = "linux")]
    #[test]
    fn acquire_on_this_ddr4_host_returns_raw_images() {
        let res = acquire();
        assert!(
            res.is_ok(),
            "acquire() must be Ok on this unprivileged DDR4 host, got: {res:?}"
        );
        // `unwrap_or_default` is a no-panic fallback: the assertion above
        // already proved the result is `Ok`.
        let images = res.unwrap_or_default();
        assert!(
            (1..=2).contains(&images.len()),
            "this host has two bound SPD EEPROMs, got {} image(s)",
            images.len()
        );
        for img in &images {
            assert_eq!(
                img.data.len(),
                SPD_DDR4_LEN,
                "DDR4 image must be {SPD_DDR4_LEN} bytes (index {:#04x})",
                img.index
            );
            assert!(validate_image(&img.data));
        }
    }
}
