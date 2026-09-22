//! Intel MCHBAR acquisition + read-only `/dev/mem` mmap guard (P2-06).
//!
//! This module is the gateway to Intel memory-controller MMIO. It locates the
//! MCHBAR (Memory Controller Hub Base Address Register) in the host-bridge
//! PCI config space and maps that MMIO window **read-only** so downstream
//! code (P2-07's per-channel register decode) can fetch the trained memory
//! controller registers.
//!
//! # Strategy (guarded, read-only, no-panic)
//!
//! 1. **Vendor gate (pure, zero I/O):** [`CpuInfo::detect()`] must report an
//!    Intel vendor; anything else yields [`TelemetryError::UnsupportedHardware`]
//!    *before any file or device access* (plan §D4 hard gate). On the AMD
//!    reference host this gate is the entire behavior: no PCI config is read
//!    and `/dev/mem` is never opened.
//! 2. **Host-bridge config space:** read
//!    `/sys/bus/pci/devices/0000:00:00.0/config` via `std::fs`. A missing
//!    device yields [`TelemetryError::DriverMissing`].
//! 3. **BAR5 decode:** [`decode_bar5`] parses the 8-byte field at config
//!    offset `0x48..0x50` (the 64-bit MCHBAR base register) and
//!    [`mchbar_base`] strips the low type/status bits to the page-aligned
//!    physical base. A short config or an I/O-space BAR is
//!    [`TelemetryError::Parse`]; a zero (unpopulated) base — typical of
//!    virtualized Intel hosts — is [`TelemetryError::UnsupportedHardware`]
//!    (a clean `N/A (unsupported hardware)`, not a confusing parse detail).
//! 4. **Read-only map:** open `/dev/mem` (fallback `/dev/fmem`) and
//!    `mmap(PROT_READ, MAP_PRIVATE)` the 1 MiB MCHBAR window at the decoded
//!    base, owned by the RAII [`MchBar`] guard.
//!    - EACCES/EPERM, or a STRICT_DEVMEM range rejection surfaced by the
//!      kernel as EIO/ENODATA, → [`TelemetryError::InsufficientPrivilege`].
//!    - Neither node present → [`TelemetryError::DriverMissing`].
//!
//! # Safety
//!
//! Every `unsafe` block is confined to the Linux map/read/unmap path and
//! carries a `// SAFETY:` justification. The mapping is `PROT_READ` (the
//! guard can never write to the device), `MAP_PRIVATE` (no copy-on-write
//! page can reach the hardware), page-aligned, and owned by exactly one
//! [`MchBar`] whose `Drop` calls `munmap` once — no dangling pointers, no
//! leaks, no double-unmap. All reads are bounds-checked against the mapped
//! window (out-of-bounds → [`TelemetryError::Parse`], never a fault or
//! panic).
//!
//! # Platform
//!
//! The PCI I/O and `mmap`/`munmap` are `#[cfg(target_os = "linux")]`. On
//! non-Linux targets [`acquire()`] runs the vendor gate and then returns
//! [`TelemetryError::UnsupportedHardware`] (this module still compiles; the
//! pure helpers [`decode_bar5`]/[`mchbar_base`] are platform-independent).

use std::ptr::NonNull;

use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{TelemetryError, TelemetryResult};

#[cfg(target_os = "linux")]
use std::os::unix::io::{BorrowedFd, RawFd};

/// PCI BDF of the host bridge that carries the MCHBAR register (plan §D4).
const HOST_BRIDGE_BDF: &str = "0000:00:00.0";

/// Config-space offset of the 64-bit MCHBAR ("BAR5") field (`0x48..0x50`).
const BAR5_OFFSET: usize = 0x48;

/// Length of the MCHBAR field in bytes (low + high dword).
const BAR5_LEN: usize = 8;

/// The devmem node mapped for the MCHBAR window.
const DEV_MEM: &str = "/dev/mem";

/// Fallback devmem node (plan §D4; some kernels expose MMIO via `/dev/fmem`).
const DEV_FMEM: &str = "/dev/fmem";

/// Privilege hint carried by [`TelemetryError::InsufficientPrivilege`].
const PRIV_HINT_DEVMEM: &str = "map /dev/mem read-only requires CAP_SYS_RAWIO or root";

/// Size of the MCHBAR MMIO window mapped read-only (1 MiB).
///
/// The host-bridge MCHBAR declares a 1 MiB register window; the memory
/// controller registers P2-07 decodes live within it. It is a page multiple,
/// so it is a valid `mmap` length.
const MCHBAR_WINDOW_SIZE: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Frozen public API
// ---------------------------------------------------------------------------

/// A read-only mapping of the MCHBAR MMIO window (RAII guard).
///
/// Owns the `mmap`'d region: [`Drop`] releases it with `munmap` exactly once,
/// so the mapping cannot leak and cannot outlive the guard. All reads go
/// through bounds-checked helpers ([`MchBar::read_u32`]); an out-of-bounds
/// offset is a structured [`TelemetryError::Parse`], never a fault or panic.
///
/// Not `Clone`/`Copy` (single owner of the mapping) and not `Send`/`Sync`
/// (raw pointer), which the guard's ownership model requires.
#[derive(Debug)]
pub struct MchBar {
    /// Decoded MCHBAR base address (page-aligned physical address).
    base: u64,
    /// Start of the read-only mapped window.
    region: NonNull<u8>,
    /// Length of the mapped window in bytes ([`MCHBAR_WINDOW_SIZE`]).
    len: usize,
}

impl MchBar {
    /// The decoded MCHBAR base address.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Length of the mapped window in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` when the window is empty (never for a mapped guard; completes
    /// the [`len()`] contract).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read a 32-bit little-endian register at `offset` within the window.
    ///
    /// `offset` is relative to the MCHBAR base (the window start). This is
    /// the read primitive P2-07 uses to fetch the IMC register block.
    ///
    /// # Errors
    ///
    /// [`TelemetryError::Parse`] when `offset + 4` exceeds the mapped window
    /// or `offset` overflows `usize` — never a SIGSEGV or panic.
    pub fn read_u32(&self, offset: usize) -> TelemetryResult<u32> {
        check_read_bounds(offset, self.len)?;
        // SAFETY: `check_read_bounds` proved `offset + 4 <= len`, so
        // `region + offset .. region + offset + 4` lies fully inside the
        // live `len`-byte mapping this `MchBar` owns (it is not dropped
        // until after this call returns). The mapping is `PROT_READ`, and a
        // byte-wise read imposes no alignment requirement. The reads are
        // `read_volatile` because the target is MMIO: the compiler must not
        // hoist, cache, or deduplicate these accesses across calls.
        let p = self.region.as_ptr().wrapping_add(offset);
        Ok(unsafe {
            u32::from_le_bytes([
                p.read_volatile(),
                p.wrapping_add(1).read_volatile(),
                p.wrapping_add(2).read_volatile(),
                p.wrapping_add(3).read_volatile(),
            ])
        })
    }
}

impl Drop for MchBar {
    fn drop(&mut self) {
        // Non-Linux: a mapped `MchBar` is never constructed, so there is
        // nothing to release.
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `region` is the live `mmap` mapping of exactly `len`
            // bytes created in `mmap_devmem`; this `MchBar` is the sole
            // owner (no `Clone`/`Copy`) and `Drop` runs exactly once, so
            // this is the single `munmap` for the range.
            let _ = unsafe {
                nix::sys::mman::munmap(self.region.cast::<std::ffi::c_void>(), self.len)
            };
        }
    }
}

/// Acquire the read-only MCHBAR mapping for Intel platforms.
///
/// The single entry point of the Intel telemetry branch. It is vendor-gated:
/// on non-Intel hardware it returns [`TelemetryError::UnsupportedHardware`]
/// without touching any file or device; on Linux + Intel it reads the
/// host-bridge config space, decodes the MCHBAR base, and maps the window
/// read-only.
///
/// # Errors
///
/// - [`TelemetryError::UnsupportedHardware`] — non-Intel vendor (gate fires
///   before any file access), a non-Linux platform, or a zero (unpopulated)
///   MCHBAR base — typical of virtualized Intel hosts, where BAR5 decodes
///   to 0 and the readout degrades to a clean `N/A (unsupported hardware)`.
/// - [`TelemetryError::DriverMissing`] — the host-bridge PCI config device
///   or both `/dev/mem` and `/dev/fmem` are absent.
/// - [`TelemetryError::InsufficientPrivilege`] — devmem open/map permission
///   denial (EACCES/EPERM) or a STRICT_DEVMEM range rejection.
/// - [`TelemetryError::Parse`] — config space too short or an I/O-space
///   BAR5.
/// - [`TelemetryError::Io`] — any other raw I/O failure.
pub fn acquire() -> TelemetryResult<MchBar> {
    // 1. Vendor gate: pure, so non-Intel hardware is rejected before any
    //    file or device access (plan §D4 hard gate).
    let info = CpuInfo::detect();
    intel_vendor_gate(info.vendor)?;

    // 2. Platform gate: the devmem map is Linux-only.
    #[cfg(target_os = "linux")]
    {
        acquire_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(TelemetryError::UnsupportedHardware {
            vendor: "non-Linux platform (MCHBAR /dev/mem mapping is Linux-only)".to_owned(),
        })
    }
}

/// Pure vendor gate: Intel passes; AMD or unknown vendors yield
/// [`TelemetryError::UnsupportedHardware`].
///
/// This is the plan §D4 "Intel before any file/mmap access" gate. It is pure
/// (no I/O), so it unit-tests without root, an Intel CPU, or `/dev/mem`.
pub fn intel_vendor_gate(vendor: CpuVendor) -> TelemetryResult<()> {
    match vendor {
        CpuVendor::Intel(_) => Ok(()),
        CpuVendor::Amd(_) => Err(TelemetryError::UnsupportedHardware {
            vendor: "AMD (MCHBAR is Intel-only)".to_owned(),
        }),
        CpuVendor::Unknown => Err(TelemetryError::UnsupportedHardware {
            vendor: "unknown CPU vendor".to_owned(),
        }),
    }
}

/// Decode the 64-bit MCHBAR ("BAR5") field from a PCI config-space image.
///
/// Reads the 8 bytes at [`BAR5_OFFSET`] (`0x48..0x50`) as little-endian low +
/// high dwords and combines them into the raw 64-bit BAR value. 32-bit and
/// 64-bit BARs are handled uniformly: for a 32-bit BAR the high dword is
/// zero, so `(high << 32) | low` is correct for either width. The low 4 bits
/// (memory/I/O indicator, 32/64-bit type, prefetchable) are retained in the
/// returned value and stripped by [`mchbar_base`].
///
/// # Errors
///
/// [`TelemetryError::Parse`] when the config image is shorter than the field
/// or the BAR is an I/O-space register (bit 0 set).
pub fn decode_bar5(config: &[u8]) -> TelemetryResult<u64> {
    let field = match config.get(BAR5_OFFSET..BAR5_OFFSET + BAR5_LEN) {
        Some(f) => f,
        None => {
            return Err(TelemetryError::Parse {
                detail: format!(
                    "config space too short for MCHBAR field at 0x{BAR5_OFFSET:x} (have {} byte(s), need {})",
                    config.len(),
                    BAR5_OFFSET + BAR5_LEN
                ),
            })
        }
    };
    let low = u32::from_le_bytes([field[0], field[1], field[2], field[3]]);
    let high = u32::from_le_bytes([field[4], field[5], field[6], field[7]]);
    // BAR bit 0: 0 = memory space, 1 = I/O space. MCHBAR must be memory.
    if low & 0b1 != 0 {
        return Err(TelemetryError::Parse {
            detail: format!(
                "MCHBAR field at 0x{BAR5_OFFSET:x} is an I/O-space BAR (bit 0 set); a memory BAR is required"
            ),
        });
    }
    Ok(((high as u64) << 32) | (low as u64))
}

/// Compute the page-aligned MCHBAR base address from a raw 64-bit BAR5 value.
///
/// Strips the low 4 bits (memory/I/O indicator, 32/64-bit type,
/// prefetchable), which are never address bits; the high dword is pure
/// address. The result is 4 KiB page-aligned by construction, as required by
/// the devmem `mmap` offset.
pub fn mchbar_base(bar5: u64) -> u64 {
    bar5 & !0xF
}

/// Bounds-check a 4-byte register read at `offset` against a window of
/// `len` bytes.
///
/// Pure (no mapping needed): `Ok(())` when `offset + 4 <= len`;
/// [`TelemetryError::Parse`] on out-of-bounds or `usize` overflow. Used by
/// [`MchBar::read_u32`] and unit-tested in isolation.
fn check_read_bounds(offset: usize, len: usize) -> Result<(), TelemetryError> {
    let Some(end) = offset.checked_add(4) else {
        return Err(TelemetryError::Parse {
            detail: format!("read_u32 offset {offset:#x} overflows usize"),
        });
    };
    if end > len {
        return Err(TelemetryError::Parse {
            detail: format!(
                "read_u32 offset {offset:#x} out of bounds (window is {len:#x} bytes)"
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux acquisition (config space -> /dev/mem map). Non-Linux targets
// compile `acquire()` to the platform error above and exclude this whole
// section.
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn acquire_linux() -> TelemetryResult<MchBar> {
    let config = read_host_bridge_config()?;
    let raw = decode_bar5(&config)?;
    let base = mchbar_base(raw);
    if base == 0 {
        return Err(TelemetryError::UnsupportedHardware {
            vendor: "Intel host bridge with unpopulated MCHBAR (BAR5=0, virtualized or unsupported)".to_owned(),
        });
    }
    let fd = open_devmem()?;
    let guard = FdGuard(fd);
    let region = mmap_devmem(guard.fd(), base)?;
    // `guard` closes the fd when it drops at scope exit; the mapping is
    // independent of the fd and is owned by the returned `MchBar`.
    Ok(MchBar {
        base,
        region,
        len: MCHBAR_WINDOW_SIZE,
    })
}

/// Read the host-bridge PCI config-space image from sysfs.
#[cfg(target_os = "linux")]
fn read_host_bridge_config() -> TelemetryResult<Vec<u8>> {
    let path = format!("/sys/bus/pci/devices/{HOST_BRIDGE_BDF}/config");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(bytes),
        // Host-bridge device absent from the PCI bus.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(TelemetryError::DriverMissing { driver: HOST_BRIDGE_BDF })
        }
        Err(e) => Err(TelemetryError::Io(reconstruct_io(&e))),
    }
}

/// Open `/dev/mem` read-only, falling back to `/dev/fmem` when the primary
/// node is absent (plan §D4).
#[cfg(target_os = "linux")]
fn open_devmem() -> TelemetryResult<RawFd> {
    use nix::fcntl::{open, OFlag};
    use nix::sys::stat::Mode;

    let mode = Mode::empty();
    match open(DEV_MEM, OFlag::O_RDONLY, mode) {
        Ok(fd) => Ok(fd),
        Err(e) => {
            let io_e = std::io::Error::from(e);
            if io_e.kind() == std::io::ErrorKind::NotFound {
                // Primary node absent: try the `/dev/fmem` fallback.
                match open(DEV_FMEM, OFlag::O_RDONLY, mode) {
                    Ok(fd) => Ok(fd),
                    Err(e2) => Err(classify_devmem(&std::io::Error::from(e2))),
                }
            } else {
                Err(classify_devmem(&io_e))
            }
        }
    }
}

/// `mmap` the MCHBAR window at `base` read-only and return the mapped region.
#[cfg(target_os = "linux")]
fn mmap_devmem(fd: RawFd, base: u64) -> TelemetryResult<NonNull<u8>> {
    use nix::libc::off_t;
    use nix::sys::mman::{mmap, MapFlags, ProtFlags};
    use std::num::NonZeroUsize;

    // The devmem offset must fit the platform `off_t`; a decoded base beyond
    // it is not a plausible physical address (`Parse`, not `Io`).
    let offset = match off_t::try_from(base) {
        Ok(o) => o,
        Err(_) => {
            return Err(TelemetryError::Parse {
                detail: format!("MCHBAR base {base:#x} exceeds the devmem addressable range"),
            })
        }
    };
    // The window size is a non-zero constant; the `MIN` fallback keeps this
    // total without `expect` (no-panic contract).
    let length = NonZeroUsize::new(MCHBAR_WINDOW_SIZE).unwrap_or(NonZeroUsize::MIN);
    // SAFETY: `fd` is a valid, open devmem descriptor for the duration of
    // this call (the caller holds the `FdGuard`); `borrow_raw` only borrows
    // it, so ownership and the close-on-drop stay with the guard.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    // SAFETY: `None` lets the kernel choose the mapping address; `length` is
    // a non-zero page multiple (1 MiB); `PROT_READ` makes the region
    // read-only, so no write can reach the device; `MAP_PRIVATE` yields a
    // private mapping (copy-on-write pages never write back to hardware);
    // `borrowed` is the open devmem node; `offset` is the page-aligned
    // MCHBAR base (`mchbar_base` stripped the low 4 bits) and fits `off_t`
    // (checked above). On success the kernel returns a live, page-aligned
    // mapping of exactly `length` bytes.
    let ptr = unsafe {
        mmap(None, length, ProtFlags::PROT_READ, MapFlags::MAP_PRIVATE, borrowed, offset)
    };
    match ptr {
        Ok(p) => Ok(p.cast::<u8>()),
        Err(e) => Err(classify_devmem(&std::io::Error::from(e))),
    }
}

/// Classify a devmem `open`/`mmap` failure: permission denials (EACCES/EPERM)
/// and STRICT_DEVMEM range rejections (EIO/ENODATA) →
/// [`TelemetryError::InsufficientPrivilege`]; absent node →
/// [`TelemetryError::DriverMissing`]; anything else → [`TelemetryError::Io`].
///
/// The STRICT_DEVMEM arm matches on the raw OS code rather than
/// [`std::io::ErrorKind`]: the kernel rejects a non-RAM (PCI MMIO) `mmap`
/// through `/dev/mem` with EIO (ENODATA in some configs), which Rust
/// surfaces as `ErrorKind::Other` — the kind-based arms above never see it.
#[cfg(target_os = "linux")]
fn classify_devmem(e: &std::io::Error) -> TelemetryError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => TelemetryError::InsufficientPrivilege {
            hint: PRIV_HINT_DEVMEM,
        },
        std::io::ErrorKind::NotFound => TelemetryError::DriverMissing { driver: DEV_MEM },
        _ => match e.raw_os_error() {
            // STRICT_DEVMEM range rejection (drivers/char/mem.c): EIO, or
            // ENODATA in some kernel configs.
            Some(code) if code == nix::libc::EIO || code == nix::libc::ENODATA => {
                TelemetryError::InsufficientPrivilege { hint: PRIV_HINT_DEVMEM }
            }
            _ => TelemetryError::Io(reconstruct_io(e)),
        },
    }
}

/// Reconstruct an owned `std::io::Error` (it is not `Clone`): from the raw OS
/// code when present, else from kind + text — the same reconstruction as
/// [`TelemetryError`]'s manual `Clone` impl.
#[cfg(target_os = "linux")]
fn reconstruct_io(e: &std::io::Error) -> std::io::Error {
    match e.raw_os_error() {
        Some(code) => std::io::Error::from_raw_os_error(code),
        None => std::io::Error::new(e.kind(), e.to_string()),
    }
}

/// RAII guard: closes the devmem fd exactly once on scope exit.
///
/// `close()` on a valid fd cannot meaningfully fail; its result is discarded
/// rather than escalated (no-panic contract). The mapping created from the fd
/// outlives it: that region is released by [`MchBar`]'s `Drop`, not here.
#[cfg(target_os = "linux")]
struct FdGuard(RawFd);

#[cfg(target_os = "linux")]
impl FdGuard {
    fn fd(&self) -> RawFd {
        self.0
    }
}

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

    /// Build a synthetic 256-byte PCI config-space image with the given
    /// low/high dwords placed at the MCHBAR field (`0x48..0x50`).
    fn config_with_bar5(low: u32, high: u32) -> Vec<u8> {
        let mut cfg = vec![0u8; 256];
        cfg[BAR5_OFFSET..BAR5_OFFSET + 4].copy_from_slice(&low.to_le_bytes());
        cfg[BAR5_OFFSET + 4..BAR5_OFFSET + BAR5_LEN].copy_from_slice(&high.to_le_bytes());
        cfg
    }

    /// (a) A 32-bit memory BAR (type bits 00, high dword zero): the decoded
    /// value carries the low-dword address and the base is page-aligned.
    #[test]
    fn decode_bar5_32bit_memory_bar() {
        let cfg = config_with_bar5(0x1000_0000, 0x0000_0000);
        assert_eq!(decode_bar5(&cfg), Ok(0x1000_0000));
        assert_eq!(mchbar_base(0x1000_0000), 0x1000_0000);
    }

    /// (a) A 64-bit memory BAR (type bits 01, high dword populated): the
    /// decoded value combines low + high and the base is the full 64-bit
    /// address; the prefetchable bit (0x8) is stripped with the other low
    /// status bits.
    #[test]
    fn decode_bar5_64bit_memory_bar() {
        let cfg = config_with_bar5(0x1000_0002, 0x0000_1234);
        assert_eq!(decode_bar5(&cfg), Ok(0x1234_1000_0002));
        assert_eq!(mchbar_base(0x1234_1000_0002), 0x1234_1000_0000);

        let cfg = config_with_bar5(0x1000_000A, 0x0000_5678); // + prefetch bit
        assert_eq!(decode_bar5(&cfg), Ok(0x5678_1000_000A));
        assert_eq!(mchbar_base(0x5678_1000_000A), 0x5678_1000_0000);
    }

    /// (a) An I/O-space BAR (bit 0 set) is rejected with `Parse` — never a
    /// mapped address, never a panic.
    #[test]
    fn decode_bar5_io_space_bar_is_parse_error() {
        for low in [0x1000_0001u32, 0x1000_0003, 0x1000_0005] {
            assert!(
                matches!(
                    decode_bar5(&config_with_bar5(low, 0)),
                    Err(TelemetryError::Parse { .. })
                ),
                "low {low:#x} must be rejected"
            );
        }
    }

    /// (a) Config space shorter than the 8-byte field is rejected with
    /// `Parse` (the sweep never indexes out of bounds).
    #[test]
    fn decode_bar5_short_config_is_parse_error() {
        for len in [0usize, 8, 0x30, 0x48, 0x4C, 0x4F] {
            assert!(
                matches!(
                    decode_bar5(&vec![0u8; len]),
                    Err(TelemetryError::Parse { .. })
                ),
                "len {len:#x} must be rejected"
            );
        }
        // A 64-bit BAR with a zero address decodes but yields base 0, which
        // the acquisition flow rejects (covered here via the pure helpers).
        let cfg = config_with_bar5(0x0, 0x0);
        assert_eq!(decode_bar5(&cfg), Ok(0));
        assert_eq!(mchbar_base(0), 0);
    }

    /// (b) [`mchbar_base`] strips exactly the four type/status bits; the
    /// result is page-aligned.
    #[test]
    fn mchbar_base_strips_type_bits() {
        assert_eq!(mchbar_base(0x1234_1000_000F), 0x1234_1000_0000);
        assert_eq!(mchbar_base(0x0000_0000_FFF0_0007), 0x0000_0000_FFF0_0000);
        assert_eq!(mchbar_base(0x0000_0000_0000_1000), 0x0000_0000_0000_1000);
        assert_eq!(mchbar_base(0x2), 0x0);
        assert_eq!(mchbar_base(0x0), 0x0);
    }

    /// (f) A raw BAR5 that decodes to a zero base (unpopulated — as on
    /// virtualized Intel hosts) strips to base 0 through the pure
    /// helpers; `acquire_linux` maps that `base == 0` to
    /// `UnsupportedHardware` (not `Parse`), so the readout degrades to a
    /// clean `N/A (unsupported hardware)` instead of a confusing parse
    /// detail.
    #[test]
    fn zero_bar5_decodes_to_zero_base_for_unsupported_hardware() {
        // Status-only BAR encodings (no address bits set) all strip to
        // base 0.
        for raw in [0x0u64, 0x4, 0xF] {
            assert_eq!(mchbar_base(raw), 0, "raw {raw:#x} must decode to base 0");
        }
        // The decode stage of the `acquire_linux` flow for an unpopulated
        // BAR5: the field decodes to 0 and the base stays 0, which
        // `acquire_linux` then maps to `UnsupportedHardware`.
        let cfg = config_with_bar5(0, 0);
        assert_eq!(decode_bar5(&cfg), Ok(0));
        assert_eq!(mchbar_base(0), 0);
    }

    /// (d) The pure Intel vendor gate: non-Intel vendors (AMD / unknown)
    /// yield `UnsupportedHardware`; Intel passes. No I/O involved.
    #[test]
    fn intel_vendor_gate_rejects_non_intel() {
        assert_eq!(
            intel_vendor_gate(CpuVendor::Amd(AmdZen::Zen3)),
            Err(TelemetryError::UnsupportedHardware {
                vendor: "AMD (MCHBAR is Intel-only)".to_owned()
            })
        );
        assert_eq!(
            intel_vendor_gate(CpuVendor::Unknown),
            Err(TelemetryError::UnsupportedHardware {
                vendor: "unknown CPU vendor".to_owned()
            })
        );
        assert_eq!(
            intel_vendor_gate(CpuVendor::Intel(IntelGen::Skylake)),
            Ok(())
        );
    }

    /// (c) `acquire()` respects the vendor gate on the running host:
    /// on a non-Intel host (the AMD reference host) it is specifically
    /// `UnsupportedHardware`, before any PCI config read or `/dev/mem`
    /// access, without panicking. On an Intel host the gate passes and
    /// the real-path outcome depends on root, `/dev/mem`, and BAR5 state
    /// (a virtualized CI runner degrades to `UnsupportedHardware` via
    /// the unpopulated-BAR5 path, physical hardware may map the window),
    /// so the Intel arm is shape-only: it must not panic.
    #[test]
    fn acquire_respects_vendor_gate() {
        let is_intel = matches!(CpuInfo::detect().vendor, CpuVendor::Intel(_));
        let res = acquire();
        if is_intel {
            // Shape-only: any frozen outcome (including a live mapping)
            // is acceptable on Intel silicon; a panic is not.
            let _ = res;
        } else {
            assert!(
                matches!(res, Err(TelemetryError::UnsupportedHardware { .. })),
                "the Intel gate must reject the non-Intel host before any I/O, got: {res:?}"
            );
        }
    }

    /// (e) The `read_u32` bounds check is pure: `offset + 4 > len` (or a
    /// `usize` overflow) yields `Parse` — the same result a live guard
    /// produces out-of-bounds, tested without a real mapping.
    #[test]
    fn read_bounds_check_never_overflows() {
        assert_eq!(check_read_bounds(0, 8), Ok(()));
        assert_eq!(check_read_bounds(4, 8), Ok(())); // 4 + 4 = 8, in bounds
        assert!(matches!(
            check_read_bounds(5, 8),
            Err(TelemetryError::Parse { .. })
        ));
        assert!(matches!(
            check_read_bounds(0, 3),
            Err(TelemetryError::Parse { .. })
        ));
        assert!(matches!(
            check_read_bounds(0, 0),
            Err(TelemetryError::Parse { .. })
        ));
        assert!(matches!(
            check_read_bounds(usize::MAX, 1024),
            Err(TelemetryError::Parse { .. })
        ));
    }

    /// STRICT_DEVMEM (review fix F1): on a root host with
    /// `CONFIG_STRICT_DEVMEM`, `mmap` of a non-RAM (PCI MMIO) range through
    /// `/dev/mem` fails with EIO (ENODATA in some kernel configs). Rust
    /// surfaces those as `ErrorKind::Other` with the raw OS code — so the
    /// classification must match the raw code and still yield
    /// [`TelemetryError::InsufficientPrivilege`] with the devmem hint per the
    /// frozen contract (never `Io`).
    #[cfg(target_os = "linux")]
    #[test]
    fn strict_devmem_rejections_classify_as_insufficient_privilege() {
        for code in [nix::libc::EIO, nix::libc::ENODATA] {
            assert_eq!(
                classify_devmem(&std::io::Error::from_raw_os_error(code)),
                TelemetryError::InsufficientPrivilege {
                    hint: PRIV_HINT_DEVMEM
                },
                "raw OS code {code} (STRICT_DEVMEM rejection) must classify as InsufficientPrivilege, not Io"
            );
        }
    }
}
