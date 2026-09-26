//! SOFT privilege probe (P3-12, plan D5): the daemon is the only
//! privileged process, but missing privilege must **never** panic or
//! exit — it degrades gracefully. [`probe`] reports the current
//! process's root / `CAP_SYS_RAWIO` state plus human-readable warnings
//! naming what will report `N/A`; the call site (main, P3-17) emits
//! them to stderr and keeps serving.

use std::fs;

use libc::geteuid;

/// `CAP_SYS_RAWIO` is bit 17 of the kernel capability set — the bit the
/// SMU (`/dev/ryzen_smu`) and `/dev/mem` MCHBAR reads require.
const CAP_SYS_RAWIO_BIT: u64 = 1 << 17;

/// SOFT report of the daemon's privilege state (plan D5). Every field
/// is populated best-effort: a missing or unreadable source of truth
/// (non-Linux host, permissions) degrades the relevant flag to
/// `false` with a warning — never an error, never a panic, never an
/// exit.
#[derive(Debug, Clone, PartialEq)]
pub struct PrivilegeReport {
    /// `true` when the process's effective uid is 0.
    pub is_root: bool,
    /// `true` when `CAP_SYS_RAWIO` is set in the process's effective
    /// capability set (`CapEff` bit 17).
    pub has_cap_sys_rawio: bool,
    /// Human-readable notes on what will degrade (e.g. the non-root
    /// warning when `is_root == false`); empty when fully privileged.
    /// The caller (main, P3-17) emits these to stderr.
    pub warnings: Vec<String>,
}

/// Pure parse of a `CapEff` hex capability set: `true` iff bit 17
/// (`CAP_SYS_RAWIO`) is set. Any parse problem — empty input,
/// non-hex characters, an over-long value, surrounding whitespace,
/// a sign — degrades to `false`; this function never panics.
///
/// [`probe`] feeds it the `CapEff:` line value from `/proc/self/status`
/// (a 16-digit hex string, e.g. `0000000000020000`).
pub fn cap_sys_rawio_from_cappeff(hex: &str) -> bool {
    match u64::from_str_radix(hex.trim(), 16) {
        Ok(value) => value & CAP_SYS_RAWIO_BIT != 0,
        Err(_) => false,
    }
}

/// [`probe`]'s `CapEff` source: read `/proc/self/status` and extract
/// the `CapEff:` line. Returns `(has_cap_sys_rawio, warning)` —
/// `warning` is `None` on a successful read, or a human-readable note
/// when the file is missing/unreadable (non-Linux host, permissions)
/// or the line is absent, in which case the flag degrades to `false`.
/// Never panics.
fn read_cap_eff() -> (bool, Option<String>) {
    match fs::read_to_string("/proc/self/status") {
        Ok(status) => {
            for line in status.lines() {
                if let Some(value) = line.strip_prefix("CapEff:") {
                    return (cap_sys_rawio_from_cappeff(value), None);
                }
            }
            (
                false,
                Some(
                    "no `CapEff:` line found in /proc/self/status; assuming CAP_SYS_RAWIO is absent"
                        .to_string(),
                ),
            )
        }
        Err(_) => (
            false,
            Some(
                "could not read /proc/self/status (non-Linux host or permission denied); assuming CAP_SYS_RAWIO is absent"
                    .to_string(),
            ),
        ),
    }
}

/// Probe the current process's privilege state (plan D5 SOFT semantics).
///
/// - `is_root` via `geteuid() == 0` (libc).
/// - `has_cap_sys_rawio` via the `CapEff:` line of `/proc/self/status`
///   ([`cap_sys_rawio_from_cappeff`], bit 17); an unreadable file
///   degrades the flag to `false` plus a warning.
/// - `warnings` names the sections that will report `N/A` (the
///   `InsufficientPrivilege`/`DriverMissing` fallbacks the Phase 2
///   providers already use); a caller (main, P3-17) emits them to
///   stderr.
///
/// This function never fails, never panics, and never exits: the daemon
/// serves in every privilege state, degrading gracefully.
pub fn probe() -> PrivilegeReport {
    // SAFETY: `geteuid` only reads the current process's effective uid
    // from the kernel; it takes no arguments, writes no memory, and
    // cannot fail — the returned value is always a valid uid.
    let is_root = unsafe { geteuid() == 0 };

    let (has_cap_sys_rawio, cap_warning) = read_cap_eff();

    let mut warnings = Vec::new();
    if !is_root {
        warnings.push(
            "not running as root; privileged telemetry fields (SMU/MCHBAR) will report N/A"
                .to_string(),
        );
    }
    if let Some(warning) = cap_warning {
        warnings.push(warning);
    } else if is_root && !has_cap_sys_rawio {
        // Root without the ambient capability (e.g. a unit missing
        // AmbientCapabilities=CAP_SYS_RAWIO): the non-root warning does
        // not apply, so name the degradation directly.
        warnings.push(
            "running as root but CAP_SYS_RAWIO is not in the effective set (CapEff bit 17 clear); privileged telemetry fields (SMU/MCHBAR) will report N/A (InsufficientPrivilege)"
                .to_string(),
        );
    }

    PrivilegeReport {
        is_root,
        has_cap_sys_rawio,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cappeff_bit_17_set_returns_true() {
        // Exactly CAP_SYS_RAWIO (bit 17, 2^17 = 0x020000) and nothing else.
        assert!(cap_sys_rawio_from_cappeff("0000000000020000"));
        // The full 40-cap set: bit 17 falls inside 0x3fffffffff.
        assert!(cap_sys_rawio_from_cappeff("0000003fffffffff"));
        // The value as it appears after the "CapEff:" prefix — leading
        // whitespace (the tab separator) is trimmed by the parser.
        assert!(cap_sys_rawio_from_cappeff("\t0000000000020000"));
    }

    #[test]
    fn cappeff_bit_17_clear_returns_false() {
        // Empty capability set.
        assert!(!cap_sys_rawio_from_cappeff("0000000000000000"));
        // Bit 18 (0x40000) — the adjacent position; the parser must
        // not confuse it with CAP_SYS_RAWIO.
        assert!(!cap_sys_rawio_from_cappeff("000000000040000"));
        // The original buggy value: CAP_SYS_ADMIN (bit 21, 0x200000) is
        // NOT CAP_SYS_RAWIO — pins the fix against the false warning.
        assert!(!cap_sys_rawio_from_cappeff("0000000000200000"));
        // Bit 37 (0x2000000000) — a distinct position; the parser must
        // not confuse it with CAP_SYS_RAWIO.
        assert!(!cap_sys_rawio_from_cappeff("0000002000000000"));
    }

    #[test]
    fn cappeff_malformed_inputs_return_false_without_panic() {
        // Every parse failure degrades to `false`: empty,
        // whitespace-only, the prefix (non-hex 'C'), non-hex digits,
        // over-long (past u64), negative.
        for bad in [
            "",
            "   ",
            "CapEff:0000000000020000",
            "000000000002000g",
            "zzzzzzzzzzzzzzzz",
            "000000000020000000000000",
            "-1",
        ] {
            assert!(!cap_sys_rawio_from_cappeff(bad), "expected false for {bad:?}");
        }
    }

    #[test]
    fn probe_returns_a_report_consistent_with_its_flags() {
        // SOFT contract: `probe()` never panics or exits in any
        // privilege state, and the flags and the warnings must agree.
        // (No specific is_root/cap assertion — those depend on the
        // test environment; only flag/warning consistency is checked.)
        let report = probe();
        assert!(
            report.is_root || !report.warnings.is_empty(),
            "an unprivileged probe must warn; got {report:?}"
        );
        assert!(
            report.has_cap_sys_rawio || !report.warnings.is_empty(),
            "a missing CAP_SYS_RAWIO must be explained by a warning; got {report:?}"
        );
        if !report.is_root {
            assert!(
                report
                    .warnings
                    .iter()
                    .any(|w| w.contains("not running as root")),
                "a non-root probe must carry the non-root warning; got {report:?}"
            );
        }
    }
}
