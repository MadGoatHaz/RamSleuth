//! Guarded SPD EEPROM auto-bind fallback (daemon side, root-only).
//!
//! On an Intel platform where the kernel's `ee1004` driver bound fewer
//! SPD EEPROMs than the platform has active memory channels (e.g. a
//! Flex-Mode Skylake with 2 channels but only 1 bound EEPROM), the
//! daemon — the only process allowed to hold privileges — attempts to
//! bind the missing EEPROM(s) via the sysfs `new_device` mechanism, so
//! the next telemetry snapshot sees every DIMM.
//!
//! The frozen unprivileged telemetry crate (`spd_eeprom.rs`) stays
//! read-only: **this** module owns every sysfs write.
//!
//! # Policy (non-fatal, root-gated, opt-out)
//!
//! [`ensure_spd_eeproms_bound`] is the single entry point. The daemon's
//! collector closure calls it as its **first line** (before the spike +
//! settle + `collect()`), so a freshly bound EEPROM lands in the same
//! snapshot. It never panics, never exits, never returns an error, and
//! never affects the snapshot:
//!
//! - `enabled == false` (the `--no-spd-autobind` flag) → no-op;
//! - not root (`geteuid() != 0`) → silent no-op (the startup probe
//!   already warned once; the collector runs on every TTL re-collect,
//!   so this gate must not repeat the note);
//! - the `ee1004` driver dir is absent (driver not loaded) → no-op;
//! - the CPU is not Intel → no-op (this is an Intel-specific fallback);
//! - bound clients `>= channel_count(gen)` → no-op (steady state: every
//!   active channel has its EEPROM);
//! - every `new_device` write the kernel **accepted** is recorded in a
//!   process-lifetime set, so a NAKing/empty address is not re-written
//!   on every TTL re-collect.
//!
//! Mechanics: writing `"ee1004 <hex-addr>\n"` to
//! `/sys/bus/i2c/devices/i2c-<bus>/new_device` makes the kernel
//! instantiate the client (it appears under the driver dir immediately —
//! the caller's `collect()` re-enumerates it; there is no re-scan here).
//! A part that NAKs is auto-removed by the kernel (no residue). This
//! module **never** auto-unbinds: a bound EEPROM is a real DIMM the
//! DSDT failed to advertise — a persistent desired state.
//!
//! No panics, no new dependencies, no protocol/wire change; the sole
//! FFI is the same single-intrinsic `geteuid` the frozen `caps` module
//! uses.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use libc::geteuid;

use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::intel_readout::channel_count;

/// The `ee1004` driver's sysfs directory: one `<bus>-<addr>` symlink per
/// bound client, plus driver-level extras (`module`, `bind`, `unbind`,
/// `new_device`, `uevent`) that [`parse_bus_addr`] rejects.
const EE1004_DRIVER_DIR: &str = "/sys/bus/i2c/drivers/ee1004";

/// The i2c bus-device directory: one `i2c-<bus>` entry per I2C adapter.
const I2C_DEVICES_DIR: &str = "/sys/bus/i2c/devices";

/// The standard SPD-hub addresses, probed in order (the `ee1004`
/// `new_device` candidate set).
const SPD_ADDRS: [u8; 4] = [0x50, 0x51, 0x52, 0x53];

/// Process-lifetime record of `(bus, addr)` pairs whose `new_device`
/// write the kernel accepted (it bound a part, or the part NAKed and
/// the kernel auto-removed it — both look identical to the writer).
/// A `BTreeSet` behind a `Mutex`; `OnceLock` (Rust 1.70) keeps the
/// static MSRV-safe (1.75 — `LazyLock` is 1.80+).
static ATTEMPTED: OnceLock<Mutex<BTreeSet<(u8, u8)>>> = OnceLock::new();

/// The public entry point of the guarded SPD EEPROM auto-bind fallback
/// (see the module docs for the full policy).
///
/// `enabled == false` (the daemon's `--no-spd-autobind` flag) makes this
/// a no-op. Non-fatal by construction: every step is a
/// `match`/`if let` + `eprintln`, so no error can escape and the
/// caller's `collect()` — which runs immediately after and re-enumerates
/// the driver dir itself — is never affected.
pub fn ensure_spd_eeproms_bound(enabled: bool) {
    if !enabled {
        return;
    }
    // Root gate: `new_device` is a privileged write; the daemon is the
    // only process that may hold it. Silent on non-root (the startup
    // probe already warned once — the collector runs on every TTL
    // re-collect, so a per-collect note would spam stderr).
    // SAFETY: `geteuid` only reads the current process's effective uid
    // from the kernel; it takes no arguments, writes no memory, and
    // cannot fail — the returned value is always a valid uid (the same
    // single-intrinsic FFI the frozen `caps` module uses).
    if unsafe { geteuid() != 0 } {
        return;
    }
    // Driver gate: the `ee1004` dir must exist (driver loaded).
    if !std::path::Path::new(EE1004_DRIVER_DIR).is_dir() {
        return;
    }
    // Vendor gate: this is an Intel-specific fallback.
    let CpuVendor::Intel(gen) = CpuInfo::detect().vendor else {
        return;
    };
    let active = channel_count(gen);

    let bound = bound_clients();
    if bound.len() >= active as usize {
        return; // steady state: every active channel has its EEPROM
    }

    // Bus selection: if any client is bound, use its bus (the
    // 1-of-2 Skylake case); else try every `i2c-*` adapter.
    let buses = match bound.first() {
        Some((bus, _)) => vec![*bus],
        None => i2c_buses(),
    };
    if buses.is_empty() {
        return;
    }

    // The process-lifetime attempt set: a pair whose `new_device` write
    // the kernel already accepted (bound, or NAKed + auto-removed) is
    // never re-written on a later TTL re-collect.
    let mut attempted = ATTEMPTED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for bus in buses {
        for (target_bus, addr) in bind_targets(&bound, active, bus) {
            if !attempted.insert((target_bus, addr)) {
                continue; // already attempted this process's lifetime
            }
            match write_new_device(target_bus, addr) {
                Ok(()) => eprintln!(
                    "info: SPD auto-bind: ee1004 {target_bus}-{addr:04x} `new_device` accepted (i2c-{target_bus}, addr {addr:04x})"
                ),
                Err(e) => {
                    // A *failed* write (e.g. the adapter directory
                    // absent) is not "attempted": drop the record so a
                    // later collect may retry it.
                    attempted.remove(&(target_bus, addr));
                    eprintln!(
                        "warning: SPD auto-bind: `new_device` write for ee1004 {target_bus}-{addr:04x} failed: {e}; will retry on a later collect"
                    );
                }
            }
        }
    }
}

/// Enumerate the `ee1004` driver directory into `(bus, addr)` pairs for
/// every bound client (driver-dir extras and malformed names dropped).
fn bound_clients() -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(EE1004_DRIVER_DIR) {
        Ok(rd) => rd,
        Err(_) => return out,
    };
    for entry in rd.filter_map(|e| e.ok()) {
        if let Some(pair) = parse_bus_addr(&entry.file_name().to_string_lossy()) {
            out.push(pair);
        }
    }
    out
}

/// Enumerate `/sys/bus/i2c/devices/` for `i2c-<bus>` adapter names,
/// returning the decimal bus numbers (malformed names dropped).
fn i2c_buses() -> Vec<u8> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(I2C_DEVICES_DIR) {
        Ok(rd) => rd,
        Err(_) => return out,
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(num) = name.strip_prefix("i2c-") {
            if let Ok(bus) = num.parse::<u8>() {
                out.push(bus);
            }
        }
    }
    out
}

/// Re-split an `ee1004` client name `<bus>-<addr>` into `(bus, addr)`.
///
/// Unlike the frozen telemetry parser (`spd_eeprom::parse_device_name`,
/// which keeps only the address), this keeps **both** halves: the bus is
/// decimal digits (kernel `%u`, narrowed to `u8` here), the address is
/// hex. Returns `None` for driver-dir extras (`"module"`, `"bind"`,
/// `"new_device"`, …) and malformed names (missing dash, non-numeric
/// bus, non-hex address, overflow) — dropped, never an error, never a
/// panic.
pub(crate) fn parse_bus_addr(name: &str) -> Option<(u8, u8)> {
    let (bus, addr) = name.split_once('-')?;
    // The bus is decimal digits; this rejects the driver-dir extras and
    // any non-client name.
    if bus.is_empty() || !bus.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bus = u8::try_from(bus.parse::<u32>().ok()?).ok()?;
    let addr = u8::from_str_radix(addr, 16).ok()?;
    Some((bus, addr))
}

/// Write `"ee1004 <hex-addr>\n"` to the adapter's `new_device` trigger.
/// On success the kernel instantiates the client; a NAKing part is
/// auto-removed by the kernel (no residue, no unbind needed).
fn write_new_device(bus: u8, addr: u8) -> std::io::Result<()> {
    std::fs::write(
        format!("{I2C_DEVICES_DIR}/i2c-{bus}/new_device"),
        format!("ee1004 {addr:04x}\n"),
    )
}

/// Pure bind-target selection (the testable core of the fallback):
/// given the `(bus, addr)` pairs the `ee1004` dir already holds, the
/// platform's active channel count, and the bus to try, return the
/// `(bus, addr)` pairs to attempt via `new_device` — the standard SPD
/// addresses ([`SPD_ADDRS`]) not already bound **on that bus**, in
/// probe order.
///
/// `bound.len() >= active` → empty (steady state).
pub(crate) fn bind_targets(bound: &[(u8, u8)], active: u8, bus: u8) -> Vec<(u8, u8)> {
    if bound.len() >= active as usize {
        return Vec::new();
    }
    SPD_ADDRS
        .iter()
        .copied()
        .filter(|addr| !bound.iter().any(|(b, a)| *b == bus && *a == *addr))
        .map(|addr| (bus, addr))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address half is hex, the bus half decimal (the kernel names
    /// bound clients `%u-%04x`).
    #[test]
    fn parse_bus_addr_extracts_both_halves() {
        assert_eq!(parse_bus_addr("1-0050"), Some((1, 0x50)));
        assert_eq!(parse_bus_addr("10-0052"), Some((10, 0x52)));
        assert_eq!(parse_bus_addr("0-0053"), Some((0, 0x53)));
        // Unpadded address forms parse identically.
        assert_eq!(parse_bus_addr("3-51"), Some((3, 0x51)));
        assert_eq!(parse_bus_addr("3-0000"), Some((3, 0x00)));
    }

    /// Driver-dir extras and malformed names yield `None` (dropped),
    /// never an error, never a panic.
    #[test]
    fn parse_bus_addr_rejects_garbage() {
        for bad in [
            "module",
            "bind",
            "unbind",
            "uevent",
            "new_device", // driver-dir extras
            "-0050", // empty bus part
            "abc-0050", // non-numeric bus
            "10-00zz", // non-hex address
            "10-100", // address overflows u8
            "300-0050", // bus overflows u8
            "10", // no dash
            "", // empty name
        ] {
            assert_eq!(parse_bus_addr(bad), None, "{bad:?} must be dropped");
        }
    }

    /// Steady state: `bound >= active` → nothing to attempt (incl. a
    /// spare EEPROM past the channel count).
    #[test]
    fn bind_targets_empty_when_bound_reaches_active() {
        assert_eq!(
            bind_targets(&[(1, 0x50), (1, 0x51)], 2, 1),
            Vec::<(u8, u8)>::new()
        );
        assert_eq!(
            bind_targets(&[(5, 0x50), (5, 0x51), (5, 0x52), (5, 0x53)], 4, 5),
            Vec::<(u8, u8)>::new()
        );
        // A spare 3rd EEPROM on a 2-channel platform: still steady.
        assert_eq!(
            bind_targets(&[(1, 0x50), (1, 0x51), (1, 0x52)], 2, 1),
            Vec::<(u8, u8)>::new()
        );
    }

    /// The Flex-Mode 1-of-2 case: one bound EEPROM on a 2-channel
    /// platform → the unbound standard addresses on that same bus, in
    /// probe order.
    #[test]
    fn bind_targets_flex_mode_deficit() {
        assert_eq!(
            bind_targets(&[(1, 0x50)], 2, 1),
            vec![(1, 0x51), (1, 0x52), (1, 0x53)]
        );
    }

    /// No bound clients → all four standard addresses on the chosen bus.
    #[test]
    fn bind_targets_none_bound() {
        assert_eq!(
            bind_targets(&[], 2, 1),
            vec![(1, 0x50), (1, 0x51), (1, 0x52), (1, 0x53)]
        );
    }

    /// A 4-channel platform with 2 bound → the 2 unbound addresses.
    #[test]
    fn bind_targets_4ch_half_bound() {
        assert_eq!(
            bind_targets(&[(5, 0x50), (5, 0x51)], 4, 5),
            vec![(5, 0x52), (5, 0x53)]
        );
    }

    /// Only pairs bound **on the target bus** are skipped: a bound
    /// EEPROM on another bus does not suppress the same address here.
    #[test]
    fn bind_targets_skips_same_bus_only() {
        assert_eq!(
            bind_targets(&[(2, 0x50)], 2, 1),
            vec![(1, 0x50), (1, 0x51), (1, 0x52), (1, 0x53)]
        );
    }

    /// The opt-out path is a no-op in every environment (no panics, no
    /// I/O — `enabled == false` returns before any gate).
    #[test]
    fn ensure_disabled_is_a_noop() {
        ensure_spd_eeproms_bound(false);
    }
}
