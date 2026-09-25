//! Guarded SPD EEPROM auto-bind fallback (daemon side, root-only).
//!
//! On an Intel platform where the kernel's `ee1004` driver bound fewer
//! SPD EEPROMs than the platform has active memory channels (e.g. a
//! Flex-Mode Skylake with 2 channels but only 1 bound EEPROM), the
//! daemon — the only process allowed to hold privileges — attempts to
//! bind the missing EEPROM(s), so the next telemetry snapshot sees
//! every DIMM.
//!
//! Two mechanisms, in order:
//!
//! 1. **`new_device`** — writing `"ee1004 <hex-addr>\n"` to
//!    `/sys/bus/i2c/devices/i2c-<bus>/new_device` makes the kernel
//!    instantiate the client;
//! 2. **driver `bind` file** (the EBUSY fallback) — when that write is
//!    refused (typically `-EBUSY`: the address is already occupied by a
//!    pre-existing client node the firmware instantiated — an
//!    ACPI/DSDT device that `ee1004` never bound), the node already
//!    exists at the address and is bound via the driver's `bind` sysfs
//!    file instead: writing the node's **name** (`"<bus>-<addr>"`, e.g.
//!    `"0-0051"`) to `/sys/bus/i2c/drivers/ee1004/bind` (the kernel's
//!    driver-core `bind` store looks the node up by name on the bus and
//!    requires the driver's id table to match — a node named `ee1004`
//!    always does).
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
//! - every attempted `(bus, addr)` pair — accepted **or** failed — is
//!   recorded in a process-lifetime set, so a NAKing/empty address, a
//!   refused `new_device`, and a failed `bind` write are each made
//!   **at most once** per process lifetime (no per-collect retry loop;
//!   a daemon restart re-attempts).
//!
//! Mechanics: a `new_device` write the kernel accepts instantiates the
//! client (it appears under the driver dir immediately — the caller's
//! `collect()` re-enumerates it; there is no re-scan here); a part that
//! NAKs is auto-removed by the kernel (no residue). A refused
//! `new_device` (e.g. `-EBUSY`) falls back to the driver `bind` file
//! **only when a node exists at the address** — a node the `ee1004` id
//! table does not match (a foreign driver's client) fails the bind, and
//! a missing node is recorded as-is; both are terminal. This module
//! **never** auto-unbinds: a bound EEPROM is a real DIMM the DSDT
//! failed to advertise — a persistent desired state.
//!
//! No panics, no new dependencies, no protocol/wire change; the sole
//! FFI is the same single-intrinsic `geteuid` the frozen `caps` module
//! uses.

use std::collections::BTreeSet;
use std::path::Path;
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

/// Process-lifetime record of every `(bus, addr)` pair this process
/// already attempted to bind — the kernel accepted the `new_device`
/// write (it bound a part, or the part NAKed and the kernel auto-removed
/// it — both look identical to the writer), the write was refused and
/// the driver-`bind` fallback bound the pre-existing node, or every
/// path failed (a NAKing/empty address, a `-EBUSY` with no node to
/// bind, a `bind` refusal) — each made **at most once** per process
/// lifetime, never re-tried on a later TTL re-collect (a daemon restart
/// re-attempts). A `BTreeSet` behind a `Mutex`; `OnceLock` (Rust 1.70)
/// keeps the static MSRV-safe (1.75 — `LazyLock` is 1.80+).
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
    if !Path::new(EE1004_DRIVER_DIR).is_dir() {
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

    // The process-lifetime attempt set: a pair already attempted —
    // accepted, bind-fallback-rescued, or terminally failed — is never
    // re-written on a later TTL re-collect (a daemon restart re-attempts).
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
                    // A *refused* write (e.g. `-EBUSY`: the address is
                    // occupied by a pre-existing, unbound node) is
                    // terminal for this process's lifetime — the record
                    // stays, so no per-collect retry loop — and the
                    // driver-`bind` fallback is attempted once when a
                    // node exists at the address.
                    match plan_fallback(
                        client_node_exists(target_bus, addr),
                        client_node_driver(target_bus, addr).as_deref(),
                    ) {
                        FallbackPlan::NodeAbsent => eprintln!(
                            "warning: SPD auto-bind: ee1004 {target_bus}-{addr:04x}: `new_device` refused ({e}); no client node at i2c-{target_bus}-{addr:04x} — terminal for this process lifetime"
                        ),
                        FallbackPlan::AlreadyBound => eprintln!(
                            "info: SPD auto-bind: ee1004 {target_bus}-{addr:04x}: `new_device` refused ({e}) but the node is already bound to ee1004"
                        ),
                        FallbackPlan::BindFile => match try_bind_existing(target_bus, addr) {
                            Ok(()) => eprintln!(
                                "info: SPD auto-bind: ee1004 {target_bus}-{addr:04x} bound via the driver `bind` file (pre-existing node; `new_device` refused: {e})"
                            ),
                            Err(be) => eprintln!(
                                "warning: SPD auto-bind: ee1004 {target_bus}-{addr:04x}: `new_device` refused ({e}) and the driver `bind` failed ({be}) — terminal for this process lifetime"
                            ),
                        },
                    }
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

/// Write the client node's **name** (`"<bus>-<addr>"`, e.g. `"0-0051"`)
/// to the `ee1004` driver's `bind` sysfs file, binding a pre-existing
/// (e.g. ACPI/DSDT-instantiated) client node the `new_device` write
/// refused.
///
/// The kernel's driver-core `bind` store looks the node up **by name**
/// on the bus (`bus_find_device_by_name`) and requires the driver's id
/// table to match (`driver_match_device`) — a node named `ee1004`
/// always matches; a foreign-named node does not (the write then fails,
/// surfaced here as `Err`, and the attempt is terminal).
fn try_bind_existing(bus: u8, addr: u8) -> Result<(), String> {
    let bind_path = format!("{EE1004_DRIVER_DIR}/bind");
    std::fs::write(&bind_path, bind_payload(bus, addr))
        .map_err(|e| format!("bind write to {bind_path} failed: {e}"))
}

/// The exact bytes [`try_bind_existing`] writes: the client node's
/// sysfs name — bus decimal, address 4-digit hex (the kernel names i2c
/// clients `%u-%04x`), newline-terminated — e.g. `"0-0051\n"`.
pub(crate) fn bind_payload(bus: u8, addr: u8) -> String {
    format!("{bus}-{addr:04x}\n")
}

/// The per-target fallback decision (the testable core): given that the
/// `new_device` write was **refused**, what to do next, keyed on the
/// client node's presence at the address and its current driver:
///
/// - node **absent** → terminal (nothing to bind; a `-EBUSY` with no
///   visible node means the address is locked by a non-visible
///   occupant — do not loop);
/// - node present, **already bound to `ee1004`** → nothing to do (it
///   lands in [`bound_clients`] on the next collect);
/// - node present, unbound (or bound to a foreign driver) → attempt the
///   driver `bind` file (the kernel's id-table match gate rejects a
///   foreign node; that failure is terminal too).
pub(crate) enum FallbackPlan {
    /// No client node exists at the address: terminal, no bind attempt.
    NodeAbsent,
    /// The node already carries the `ee1004` driver: nothing to do.
    AlreadyBound,
    /// The node exists without `ee1004`: try the driver `bind` file.
    BindFile,
}

pub(crate) fn plan_fallback(node_exists: bool, node_driver: Option<&str>) -> FallbackPlan {
    match (node_exists, node_driver) {
        (false, _) => FallbackPlan::NodeAbsent,
        (true, Some("ee1004")) => FallbackPlan::AlreadyBound,
        (true, _) => FallbackPlan::BindFile,
    }
}

/// Does a client node exist at `(bus, addr)` — bound to any driver, or
/// not (the pre-existing ACPI/DSDT-node case)?
fn client_node_exists(bus: u8, addr: u8) -> bool {
    Path::new(I2C_DEVICES_DIR)
        .join(client_node_name(bus, addr))
        .is_dir()
}

/// The name of the driver bound to the client node at `(bus, addr)`:
/// `None` when the node is absent or unbound (no `driver` symlink),
/// `Some(driver-name)` when bound (the symlink's basename, e.g.
/// `"ee1004"`).
fn client_node_driver(bus: u8, addr: u8) -> Option<String> {
    let link = Path::new(I2C_DEVICES_DIR)
        .join(client_node_name(bus, addr))
        .join("driver");
    let target = std::fs::read_link(link).ok()?;
    target.file_name()?.to_str().map(str::to_owned)
}

/// The client node's sysfs name at `(bus, addr)` — the kernel's
/// `%u-%04x` form (bus decimal, address 4-digit hex), e.g. `"0-0051"`.
fn client_node_name(bus: u8, addr: u8) -> String {
    format!("{bus}-{addr:04x}")
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

    /// The bind-file payload is the client node's sysfs name — bus
    /// decimal, address 4-digit hex, newline-terminated (the kernel
    /// names i2c clients `%u-%04x`; the driver-core `bind` store looks
    /// the node up by exactly this name — **not** a `"<driver> <addr>"`
    /// or `"i2c-<bus> <addr>"` form, those belong to `new_device`).
    #[test]
    fn bind_payload_is_the_node_name() {
        assert_eq!(bind_payload(0, 0x51), "0-0051\n");
        assert_eq!(bind_payload(1, 0x50), "1-0050\n");
        assert_eq!(bind_payload(10, 0x52), "10-0052\n");
        assert_eq!(bind_payload(3, 0x0), "3-0000\n");
    }

    /// A refused `new_device` with **no node** at the address is
    /// terminal: nothing to bind, no bind-file attempt.
    #[test]
    fn plan_fallback_node_absent_is_terminal() {
        assert!(matches!(
            plan_fallback(false, None),
            FallbackPlan::NodeAbsent
        ));
    }

    /// A refused `new_device` with a pre-existing **unbound** node at
    /// the address (the ACPI/DSDT `-EBUSY` case) → the driver `bind`
    /// file is the fallback.
    #[test]
    fn plan_fallback_unbound_node_uses_bind_file() {
        assert!(matches!(
            plan_fallback(true, None),
            FallbackPlan::BindFile
        ));
    }

    /// A refused `new_device` with a node bound to a **foreign**
    /// driver → still attempt the `bind` file (the kernel's id-table
    /// match gate rejects it; that failure is terminal too).
    #[test]
    fn plan_fallback_foreign_driver_uses_bind_file() {
        assert!(matches!(
            plan_fallback(true, Some("dummy")),
            FallbackPlan::BindFile
        ));
    }

    /// A refused `new_device` with a node **already bound to ee1004**
    /// → nothing to do (it lands in `bound_clients()` on the next
    /// collect).
    #[test]
    fn plan_fallback_already_bound_is_noop() {
        assert!(matches!(
            plan_fallback(true, Some("ee1004")),
            FallbackPlan::AlreadyBound
        ));
    }

    /// The attempt-set contract that stops the infinite retry: a pair
    /// is recorded when the attempt starts and **kept on failure** (the
    /// pre-fix code removed failed writes so they re-fired — with a
    /// new warning — on every TTL re-collect). Once recorded, the pair
    /// is skipped.
    #[test]
    fn failed_attempts_stay_recorded_and_are_skipped() {
        let mut attempted = BTreeSet::new();
        // First collect: the pair is not recorded yet → attempt it.
        assert!(attempted.insert((1, 0x51)));
        // The `new_device` write was refused and the `bind` fallback
        // also failed: the record STAYS (terminal, no per-collect loop).
        assert!(attempted.contains(&(1, 0x51)));
        // Every later collect: the insert reports "already there" →
        // the pair is skipped, never re-written, never re-warned.
        assert!(!attempted.insert((1, 0x51)));
    }

    /// The opt-out path is a no-op in every environment (no panics, no
    /// I/O — `enabled == false` returns before any gate).
    #[test]
    fn ensure_disabled_is_a_noop() {
        ensure_spd_eeproms_bound(false);
    }
}
