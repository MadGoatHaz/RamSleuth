//! Intel IMC raw reader — the `ramsleuth_intel` sysfs kobject (INTEL-03).
//!
//! This module is the **primary raw acquisition** path of the Intel
//! branch (plan §3.5): it reads the 19 raw IMC register attributes
//! published by the `ramsleuth_intel` kernel module (INTEL-05) under
//! `/sys/kernel/ramsleuth_intel/` and assembles them into the raw
//! register set [`IntelImcRegs`] that the pure decode core
//! ([`crate::intel_readout::decode`]) consumes. The `/dev/mem` fallback
//! ([`IntelImcRegs::from_bar`]) produces the same type from a live MCHBAR
//! map, so both producers feed one decode (plan §3.5: module-first,
//! devmem-fallback, single decode core).
//!
//! 24-attr module builds additionally expose 5 **global** MAD
//! channel/geometry attributes (`mad_inter_channel`, `mad_intra_ch0/1`,
//! `mad_dimm_ch0/1`) — outside the per-channel `ch0`/`ch1` stride, so
//! they are NOT decode inputs: they ride as raw `Option<u32>` siblings on
//! [`SysfsRegs`]. On a pre-24-attr build (19 attrs) those files are
//! absent and the 5 fields simply read `None` — graceful degradation,
//! not an error; the core 19 keep their existing semantics.
//!
//! # Design split (mirrors the AMD branch)
//!
//! **The module exposes raw values; the decode lives in Rust** (plan §2):
//! every attribute is a raw register word, one line, zero decoding in C.
//! This module performs **no** bitfield decoding either — it only acquires
//! the raw set (plus the MCHBAR diagnostics) and hands it to the decode
//! core in `intel_readout`.
//!
//! # Acquisition strategy (mirrors `amd_smu`)
//!
//! 1. **Vendor gate (pure, zero I/O):** [`CpuInfo::detect()`] must report
//!    an Intel vendor; anything else yields
//!    [`TelemetryError::UnsupportedHardware`] *before any file access*
//!    (plan §D4 gate discipline — the gate is unit-testable without
//!    root, a driver, or a kobject).
//! 2. **Kobject gate:** the `ramsleuth_intel` kobject directory must
//!    exist. Absent → [`TelemetryError::DriverMissing`] (module not
//!    loaded / probe failed — the facade takes the `/dev/mem` fallback
//!    *only on this* outcome, plan §3.5). Present but not a directory →
//!    [`TelemetryError::Parse`] (broken install). Unreadable →
//!    [`TelemetryError::InsufficientPrivilege`]; any other I/O →
//!    [`TelemetryError::Io`].
//! 3. **Per-attribute containment:** each attribute (the core 19, plus
//!    the 5 global MAD attributes on 24-attr builds) is read
//!    independently. An **absent** attribute and a **malformed** payload
//!    both degrade to `None` for that register only — the rest of the set
//!    assembles normally, nothing panics, and one bad register never
//!    fails the whole read (frozen parse rules, plan §3.2). A MAD
//!    attribute absent on a pre-24-attr build is just `None` (graceful —
//!    not an error). Only a *permission* or *other I/O* failure on a
//!    present attribute is reported as a structured [`TelemetryError`].
//!
//! # Frozen parse rules (plan §3.2)
//!
//! - trim the trailing newline (a stray `\r` is tolerated);
//! - `mchbar_base`: exactly 16 hex digits, **no prefix**
//!   (e.g. `00000000fed10000`);
//! - `mchbar_enabled`: a single `1` / `0`;
//! - every register: `0x` + exactly 8 hex digits (the module's
//!   `0x%08x` format).
//!
//! Any deviation is malformed → `None` for that attribute (containment),
//! never a panic.
//!
//! # No-panic contract (D5)
//!
//! All I/O goes through `std::fs`; every failure is classified into a
//! structured [`TelemetryError`], and every payload parse is an
//! infallible `Option` fold. [`acquire()`] never panics on any host
//! state (kobject present, absent, not-a-directory, malformed, unreadable,
//! non-Intel CPU).
//!
//! # Tests
//!
//! Fixture tests build a synthetic kobject in a temp directory (the
//! injectable root of [`read_from`]) — the plan §6.1 Skylake DDR4-2400
//! acceptance values, the absent / not-a-directory kobject arms, the
//! per-attribute malformed / absent containment arms (incl. the 5 global
//! MAD attributes on 24-attr vs 19-attr builds), the parse strictness
//! table, and the error-classification arms. No hardware, no root.

use std::path::Path;

use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{TelemetryError, TelemetryResult};
use crate::intel_readout::{ChannelRegs, IntelImcRegs};

/// The `ramsleuth_intel` sysfs kobject directory (frozen contract, plan
/// §3.1 — the INTEL-05 module creates it only on a fully successful
/// probe).
const KOBJECT_DIR: &str = "/sys/kernel/ramsleuth_intel";

/// Driver name carried by [`TelemetryError::DriverMissing`].
const DRIVER: &str = "ramsleuth_intel";

/// Privilege hint carried by [`TelemetryError::InsufficientPrivilege`].
const PRIV_HINT: &str =
    "the ramsleuth_intel sysfs attributes are world-readable (0444); an unreadable one indicates a broken module install";

/// MCHBAR diagnostics surfaced by the kobject (plan §3.2): the physical
/// base and the enable bit. These are **not decode inputs** — the decode
/// consumes [`IntelImcRegs`] only; they are carried as a sibling for
/// operator diagnostics (an unreadable `MCHBAR` is the classic "module
/// probed on the wrong hardware" symptom).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MchBarInfo {
    /// MCHBAR physical base (`mchbar_base`; 16-digit hex, no prefix),
    /// `None` when the attribute is absent or malformed.
    pub base: Option<u64>,
    /// MCHBAR enable bit (`mchbar_enabled`; single `1`/`0`), `None` when
    /// the attribute is absent or malformed.
    pub enabled: Option<bool>,
}

/// Raw acquisition result from the `ramsleuth_intel` kobject: the 17-slot
/// raw IMC register set for the decode core, the MCHBAR diagnostics as a
/// sibling, and — on 24-attr module builds — the 5 global MAD
/// channel/geometry registers as raw siblings (not decode inputs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SysfsRegs {
    /// The raw register set (17 `Option<u32>` slots; `None` per register
    /// on a contained absent/malformed read).
    pub regs: IntelImcRegs,
    /// MCHBAR diagnostics (`mchbar_base` / `mchbar_enabled`).
    pub mchbar: MchBarInfo,
    /// Channel mode / interleave config (raw `mad_inter_channel`).
    /// `None` on a pre-24-attr module build (absent) or a malformed read.
    pub mad_inter_channel: Option<u32>,
    /// Channel 0 rank/geometry (raw `mad_intra_ch0`). `None` on a
    /// pre-24-attr module build (absent) or a malformed read.
    pub mad_intra_ch0: Option<u32>,
    /// Channel 1 rank/geometry (raw `mad_intra_ch1`). `None` on a
    /// pre-24-attr module build (absent) or a malformed read.
    pub mad_intra_ch1: Option<u32>,
    /// Channel 0 DIMM capacity (raw `mad_dimm_ch0`). `None` on a
    /// pre-24-attr module build (absent) or a malformed read.
    pub mad_dimm_ch0: Option<u32>,
    /// Channel 1 DIMM capacity (raw `mad_dimm_ch1`). `None` on a
    /// pre-24-attr module build (absent) or a malformed read.
    pub mad_dimm_ch1: Option<u32>,
}

/// Acquire the raw IMC register set from the `ramsleuth_intel` sysfs
/// kobject (the primary Intel path, plan §3.5).
///
/// Sequence (all no-panic, D5):
/// 1. the pure vendor gate ([`CpuInfo::detect`] → [`vendor_gate`]) —
///    non-Intel hardware is rejected before any file access;
/// 2. the kobject gate ([`kobject_gate`]) — absent →
///    [`TelemetryError::DriverMissing`];
/// 3. the per-attribute reads with per-register containment
///    ([`read_from`]) — the core 19 plus the 5 global MAD attributes,
///    which read `None` (graceful) on a pre-24-attr module build.
///
/// # Errors
///
/// - [`TelemetryError::UnsupportedHardware`] — non-Intel CPU (pure gate,
///   zero I/O).
/// - [`TelemetryError::DriverMissing`] — the kobject is absent (module
///   not loaded / probe failed).
/// - [`TelemetryError::InsufficientPrivilege`] — a present attribute is
///   unreadable.
/// - [`TelemetryError::Parse`] — the kobject path exists but is not a
///   directory.
/// - [`TelemetryError::Io`] — any other raw I/O failure.
pub fn acquire() -> TelemetryResult<SysfsRegs> {
    // 1. Vendor gate: pure, so non-Intel hardware is rejected before any
    //    file access (plan §D4).
    let info = CpuInfo::detect();
    vendor_gate(&info)?;

    // 2. The kobject + the 19 attributes (per-register containment).
    read_from(Path::new(KOBJECT_DIR))
}

/// Pure vendor gate: Intel passes; AMD and unknown vendors yield
/// [`TelemetryError::UnsupportedHardware`] with a human-readable vendor
/// string (the CPUID brand when known, else the vendor kind).
///
/// Performs no I/O — the gate must be able to reject before any file
/// access, and is unit-testable without root or a loaded driver.
pub fn vendor_gate(info: &CpuInfo) -> TelemetryResult<()> {
    match info.vendor {
        CpuVendor::Intel(_) => Ok(()),
        CpuVendor::Amd(_) | CpuVendor::Unknown => Err(TelemetryError::UnsupportedHardware {
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

/// Read the raw attributes from a `ramsleuth_intel`-shaped kobject
/// directory and assemble the [`SysfsRegs`] result: the core 19 (MCHBAR
/// diagnostics + the 17-slot register set) plus the 5 global MAD
/// attributes, which are absent on a pre-24-attr module build and then
/// read `None` (graceful degradation, not an error).
///
/// The root is injectable (the public [`acquire`] passes
/// [`KOBJECT_DIR`]); the fixture tests pass a temp directory.
///
/// # Errors
///
/// As [`acquire`] (minus the vendor gate, which runs before this call).
fn read_from(root: &Path) -> TelemetryResult<SysfsRegs> {
    kobject_gate(root)?;

    let mchbar = MchBarInfo {
        base: read_u64_attr(root, "mchbar_base")?,
        enabled: read_bool_attr(root, "mchbar_enabled")?,
    };

    let ch0 = ChannelRegs {
        tc_dbp: read_u32_attr(root, "ch0_tc_dbp")?,
        tc_rap: read_u32_attr(root, "ch0_tc_rap")?,
        tc_rfp: read_u32_attr(root, "ch0_tc_rfp")?,
        tc_rap2: read_u32_attr(root, "ch0_tc_rap2")?,
        tc_rdrd: read_u32_attr(root, "ch0_tc_rdrd")?,
        tc_rdwr: read_u32_attr(root, "ch0_tc_rdwr")?,
        tc_wrrd: read_u32_attr(root, "ch0_tc_wrrd")?,
        tc_wrwr: read_u32_attr(root, "ch0_tc_wrwr")?,
    };
    let ch1 = ChannelRegs {
        tc_dbp: read_u32_attr(root, "ch1_tc_dbp")?,
        tc_rap: read_u32_attr(root, "ch1_tc_rap")?,
        tc_rfp: read_u32_attr(root, "ch1_tc_rfp")?,
        tc_rap2: read_u32_attr(root, "ch1_tc_rap2")?,
        tc_rdrd: read_u32_attr(root, "ch1_tc_rdrd")?,
        tc_rdwr: read_u32_attr(root, "ch1_tc_rdwr")?,
        tc_wrrd: read_u32_attr(root, "ch1_tc_wrrd")?,
        tc_wrwr: read_u32_attr(root, "ch1_tc_wrwr")?,
    };

    // The 5 global MAD channel/geometry attributes (24-attr module
    // builds only): absent on a pre-24-attr build -> `None` (graceful),
    // with the same per-attribute containment as the core 19.
    let mad_inter_channel = read_u32_attr(root, "mad_inter_channel")?;
    let mad_intra_ch0 = read_u32_attr(root, "mad_intra_ch0")?;
    let mad_intra_ch1 = read_u32_attr(root, "mad_intra_ch1")?;
    let mad_dimm_ch0 = read_u32_attr(root, "mad_dimm_ch0")?;
    let mad_dimm_ch1 = read_u32_attr(root, "mad_dimm_ch1")?;

    Ok(SysfsRegs {
        regs: IntelImcRegs {
            mcbios_req: read_u32_attr(root, "mcbios_req")?,
            ch0,
            ch1,
        },
        mchbar,
        mad_inter_channel,
        mad_intra_ch0,
        mad_intra_ch1,
        mad_dimm_ch0,
        mad_dimm_ch1,
    })
}

/// Kobject gate: the directory must exist and be a directory.
///
/// - absent → [`TelemetryError::DriverMissing`] (module not loaded);
/// - present but not a directory → [`TelemetryError::Parse`] (broken
///   install — the path is occupied by a file);
/// - unreadable → [`TelemetryError::InsufficientPrivilege`];
/// - any other I/O → [`TelemetryError::Io`].
fn kobject_gate(root: &Path) -> TelemetryResult<()> {
    match std::fs::metadata(root) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(TelemetryError::Parse {
            detail: format!(
                "kobject path {root:?} exists but is not a directory (broken {DRIVER} install)"
            ),
        }),
        Err(e) => Err(classify_io(&e)),
    }
}

/// Classify a raw I/O error into a structured [`TelemetryError`]
/// (mirrors `amd_smu::classify_io_error`).
///
/// - `NotFound` → [`TelemetryError::DriverMissing`] (the `ramsleuth_intel`
///   interface being reached for does not exist);
/// - `PermissionDenied` → [`TelemetryError::InsufficientPrivilege`] with
///   the resolution hint;
/// - anything else → [`TelemetryError::Io`] (reconstructed from the raw
///   OS code, or kind + text, since `std::io::Error` is not `Clone` — the
///   same reconstruction as [`TelemetryError`]'s manual `Clone` impl).
fn classify_io(e: &std::io::Error) -> TelemetryError {
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

/// Read one raw attribute by name from the kobject directory.
///
/// - absent → `Ok(None)` (per-register containment — a module build that
///   exposes a subset degrades only the missing registers, e.g. a
///   pre-24-attr build lacking the 5 MAD attributes);
/// - present but not valid UTF-8 → `Ok(None)` (malformed payload);
/// - unreadable → [`TelemetryError::InsufficientPrivilege`];
/// - any other I/O → [`TelemetryError::Io`].
fn read_attr(root: &Path, name: &str) -> TelemetryResult<Option<String>> {
    match std::fs::read(root.join(name)) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => Ok(None),
            std::io::ErrorKind::PermissionDenied => {
                Err(TelemetryError::InsufficientPrivilege { hint: PRIV_HINT })
            }
            _ => Err(classify_io(&e)),
        },
    }
}

/// Read one raw 32-bit register attribute (`0x%08x`); absent / malformed
/// → `None`.
fn read_u32_attr(root: &Path, name: &str) -> TelemetryResult<Option<u32>> {
    read_attr(root, name).map(|s| s.as_deref().and_then(parse_reg8))
}

/// Read one raw 64-bit attribute (`mchbar_base`: 16-digit hex, no
/// prefix); absent / malformed → `None`.
fn read_u64_attr(root: &Path, name: &str) -> TelemetryResult<Option<u64>> {
    read_attr(root, name).map(|s| s.as_deref().and_then(parse_base16))
}

/// Read one raw boolean attribute (`mchbar_enabled`: single `1`/`0`);
/// absent / malformed → `None`.
fn read_bool_attr(root: &Path, name: &str) -> TelemetryResult<Option<bool>> {
    read_attr(root, name).map(|s| s.as_deref().and_then(parse_enabled))
}

/// Trim the trailing newline (and any stray `\r`) of a sysfs payload
/// (the module emits one trailing `\n` per line; plan §3.2).
fn trim_payload(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r'])
}

/// Parse a raw register payload: `0x` + **exactly 8** hex digits (the
/// module's `0x%08x` format). Any deviation — a missing or wrong-case
/// prefix, a length other than 8, a non-hex digit, extra content — is
/// malformed → `None` (per-register containment; never a panic).
pub fn parse_reg8(payload: &str) -> Option<u32> {
    let s = trim_payload(payload);
    let digits = s.strip_prefix("0x")?;
    if digits.len() != 8 {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Parse the `mchbar_base` payload: **exactly 16 hex digits, no prefix**
/// (the module's `%016llx` format). Any deviation → `None`.
pub fn parse_base16(payload: &str) -> Option<u64> {
    let s = trim_payload(payload);
    if s.len() != 16 {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

/// Parse the `mchbar_enabled` payload: a single `1` / `0`. Any
/// deviation → `None`.
pub fn parse_enabled(payload: &str) -> Option<bool> {
    match trim_payload(payload) {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Fixture tests — the CI stand-in for the kobject: a synthetic
    //! `ramsleuth_intel` directory in a temp root (no hardware, no
    //! root), pinning the plan §6.1 Skylake DDR4-2400 acceptance values,
    //! the 24-attr vs 19-attr MAD arms, and every containment /
    //! classification arm.

    use super::*;
    use std::path::PathBuf;

    /// A synthetic `ramsleuth_intel` kobject in a unique temp directory
    /// (parallel-safe: name + pid; removed on drop).
    struct TempKobject {
        root: PathBuf,
    }

    impl TempKobject {
        fn new(test: &str) -> Self {
            let mut root = std::env::temp_dir();
            root.push(format!("ramsleuth-intel-sysfs-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("temp kobject dir");
            Self { root }
        }

        fn write(&self, name: &str, payload: &str) {
            std::fs::write(self.root.join(name), payload).expect("attribute write");
        }

        fn write_bytes(&self, name: &str, bytes: &[u8]) {
            std::fs::write(self.root.join(name), bytes).expect("attribute write");
        }

        fn remove(&self, name: &str) {
            let _ = std::fs::remove_file(self.root.join(name));
        }
    }

    impl Drop for TempKobject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The plan §6.1 Skylake DDR4-2400 acceptance raws (i5-6600T,
    /// 17-17-17-39, dual-channel symmetric) — the same values the
    /// `intel_readout` fixture pins, written as frozen sysfs payloads
    /// (`0x%08x\n` registers, `%016llx\n` base, `1\n` enabled).
    fn write_acceptance_kobject(k: &TempKobject) {
        k.write("mchbar_base", "00000000fed10000\n");
        k.write("mchbar_enabled", "1\n");
        k.write("mcbios_req", "0x00000012\n");
        for ch in ["ch0", "ch1"] {
            k.write(&format!("{ch}_tc_dbp"), "0x11110f11\n");
            k.write(&format!("{ch}_tc_rap"), "0x27180204\n");
            k.write(&format!("{ch}_tc_rfp"), "0x000001a4\n");
            k.write(&format!("{ch}_tc_rap2"), "0x00000c0a\n");
            k.write(&format!("{ch}_tc_rdrd"), "0x0048c286\n");
            k.write(&format!("{ch}_tc_rdwr"), "0x00000280\n");
            k.write(&format!("{ch}_tc_wrrd"), "0x00000308\n");
            k.write(&format!("{ch}_tc_wrwr"), "0x0040c204\n");
        }
    }

    /// The raw register set the acceptance kobject must assemble (the
    /// 17 decode slots; the MCHBAR diagnostics are asserted separately).
    fn acceptance_regs() -> IntelImcRegs {
        let ch0 = ChannelRegs {
            tc_dbp: Some(0x1111_0F11),
            tc_rap: Some(0x2718_0204),
            tc_rfp: Some(0x0000_01A4),
            tc_rap2: Some(0x0000_0C0A),
            tc_rdrd: Some(0x0048_C286),
            tc_rdwr: Some(0x0000_0280),
            tc_wrrd: Some(0x0000_0308),
            tc_wrwr: Some(0x0040_C204),
        };
        IntelImcRegs {
            mcbios_req: Some(0x0000_0012),
            ch1: ch0.clone(),
            ch0,
        }
    }

    /// The 5 global MAD attributes as frozen sysfs payloads (`0x%08x\n`)
    /// — arbitrary raw fixture values (no decode in this chunk; the
    /// decode lives in a later `intel_readout` chunk).
    fn write_mad_attrs(k: &TempKobject) {
        k.write("mad_inter_channel", "0x00000003\n");
        k.write("mad_intra_ch0", "0x00000005\n");
        k.write("mad_intra_ch1", "0x00000007\n");
        k.write("mad_dimm_ch0", "0x00000008\n");
        k.write("mad_dimm_ch1", "0x0000000C\n");
    }

    // -----------------------------------------------------------------
    // (a) A fully-populated kobject.
    // -----------------------------------------------------------------

    /// A fully-populated kobject assembles the exact 17-slot raw
    /// register set + the MCHBAR diagnostics (Skylake acceptance values).
    #[test]
    fn acceptance_kobject_assembles_intel_imc_regs() {
        let k = TempKobject::new("acceptance");
        write_acceptance_kobject(&k);
        let out = read_from(&k.root).expect("kobject present -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(
            out.mchbar,
            MchBarInfo {
                base: Some(0xFED1_0000),
                enabled: Some(true)
            }
        );
    }

    /// A populated kobject without trailing newlines assembles identically
    /// (the frozen trim rule is lenient on the module's line ending).
    #[test]
    fn acceptance_kobject_without_newlines_assembles_identically() {
        let k = TempKobject::new("no-newlines");
        k.write("mchbar_base", "00000000fed10000");
        k.write("mchbar_enabled", "1");
        k.write("mcbios_req", "0x00000012");
        for ch in ["ch0", "ch1"] {
            k.write(&format!("{ch}_tc_dbp"), "0x11110f11");
            k.write(&format!("{ch}_tc_rap"), "0x27180204");
            k.write(&format!("{ch}_tc_rfp"), "0x000001a4");
            k.write(&format!("{ch}_tc_rap2"), "0x00000c0a");
            k.write(&format!("{ch}_tc_rdrd"), "0x0048c286");
            k.write(&format!("{ch}_tc_rdwr"), "0x00000280");
            k.write(&format!("{ch}_tc_wrrd"), "0x00000308");
            k.write(&format!("{ch}_tc_wrwr"), "0x0040c204");
        }
        let out = read_from(&k.root).expect("kobject present -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(
            out.mchbar,
            MchBarInfo {
                base: Some(0xFED1_0000),
                enabled: Some(true)
            }
        );
    }

    // -----------------------------------------------------------------
    // (a2) The 5 global MAD attributes (24-attr module builds).
    // -----------------------------------------------------------------

    /// A 24-attr kobject (the core 19 + the 5 MAD attributes) assembles
    /// the 5 raw MAD fields and keeps the core 19 intact.
    #[test]
    fn mad_attrs_populate_on_24_attr_kobject() {
        let k = TempKobject::new("mad-24");
        write_acceptance_kobject(&k);
        write_mad_attrs(&k);
        let out = read_from(&k.root).expect("kobject present -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(
            out.mchbar,
            MchBarInfo {
                base: Some(0xFED1_0000),
                enabled: Some(true)
            }
        );
        assert_eq!(out.mad_inter_channel, Some(0x0000_0003));
        assert_eq!(out.mad_intra_ch0, Some(0x0000_0005));
        assert_eq!(out.mad_intra_ch1, Some(0x0000_0007));
        assert_eq!(out.mad_dimm_ch0, Some(0x0000_0008));
        assert_eq!(out.mad_dimm_ch1, Some(0x0000_000C));
    }

    /// A pre-24-attr kobject (the core 19 only, no MAD files) degrades
    /// gracefully: all 5 MAD fields are `None`, the core 19 populate as
    /// usual (no error, no `DriverMissing`).
    #[test]
    fn absent_mad_attrs_degrade_to_none_on_19_attr_kobject() {
        let k = TempKobject::new("mad-absent");
        write_acceptance_kobject(&k); // the 19 core attributes only
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(
            out.mchbar,
            MchBarInfo {
                base: Some(0xFED1_0000),
                enabled: Some(true)
            }
        );
        assert_eq!(out.mad_inter_channel, None);
        assert_eq!(out.mad_intra_ch0, None);
        assert_eq!(out.mad_intra_ch1, None);
        assert_eq!(out.mad_dimm_ch0, None);
        assert_eq!(out.mad_dimm_ch1, None);
    }

    /// One malformed MAD attribute → `None` for that field only; the
    /// other 4 MAD fields and the core 19 assemble (containment).
    #[test]
    fn malformed_mad_attr_degrades_to_none_only() {
        let k = TempKobject::new("mad-malformed");
        write_acceptance_kobject(&k);
        write_mad_attrs(&k);
        k.write("mad_intra_ch1", "not-a-register\n");
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(out.mad_inter_channel, Some(0x0000_0003));
        assert_eq!(out.mad_intra_ch0, Some(0x0000_0005));
        assert_eq!(out.mad_intra_ch1, None);
        assert_eq!(out.mad_dimm_ch0, Some(0x0000_0008));
        assert_eq!(out.mad_dimm_ch1, Some(0x0000_000C));
    }

    // -----------------------------------------------------------------
    // (b) Kobject-level arms.
    // -----------------------------------------------------------------

    /// An absent kobject → `DriverMissing { driver: "ramsleuth_intel" }`
    /// (the facade's `/dev/mem` fallback trigger, plan §3.5).
    #[test]
    fn absent_kobject_is_driver_missing() {
        let k = TempKobject::new("absent");
        let missing = k.root.join("does-not-exist");
        assert_eq!(
            read_from(&missing),
            Err(TelemetryError::DriverMissing {
                driver: "ramsleuth_intel"
            })
        );
    }

    /// A path present as a *file* (broken install) → `Parse`, never a
    /// panic and never a `DriverMissing` misclassification.
    #[test]
    fn kobject_present_but_not_a_directory_is_parse() {
        let k = TempKobject::new("notadir");
        k.write_bytes("not-a-dir", b"a file, not a kobject");
        let err = read_from(&k.root.join("not-a-dir")).unwrap_err();
        assert!(
            matches!(err, TelemetryError::Parse { .. }),
            "expected Parse, got {err:?}"
        );
        assert!(
            err.to_string().contains("not a directory"),
            "Parse detail must name the broken path: {err:?}"
        );
    }

    // -----------------------------------------------------------------
    // (c) Per-attribute containment.
    // -----------------------------------------------------------------

    /// One malformed register attribute → `None` for that register only;
    /// the rest of the 17-slot set assembles (containment, no Err).
    #[test]
    fn malformed_attribute_degrades_to_none_only() {
        let k = TempKobject::new("malformed");
        write_acceptance_kobject(&k);
        k.write("ch0_tc_rap", "not-a-register\n");
        let mut expected = acceptance_regs();
        expected.ch0.tc_rap = None;
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, expected);
    }

    /// One absent register attribute → `None` for that register only
    /// (a future module build exposing a subset degrades gracefully).
    #[test]
    fn absent_attribute_degrades_to_none_only() {
        let k = TempKobject::new("absent-attr");
        write_acceptance_kobject(&k);
        k.remove("ch1_tc_wrwr");
        let mut expected = acceptance_regs();
        expected.ch1.tc_wrwr = None;
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, expected);
    }

    /// Malformed MCHBAR diagnostics are contained the same way: `None`
    /// diagnostics, register set intact.
    #[test]
    fn malformed_mchbar_diagnostics_contained() {
        let k = TempKobject::new("bad-mchbar");
        write_acceptance_kobject(&k);
        k.write("mchbar_base", "0x00000000fed10000\n"); // prefixed -> malformed
        k.write("mchbar_enabled", "2\n"); // not 0/1 -> malformed
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, acceptance_regs());
        assert_eq!(out.mchbar, MchBarInfo::default());
    }

    /// A non-UTF-8 payload is malformed → `None` (no panic).
    #[test]
    fn non_utf8_attribute_degrades_to_none() {
        let k = TempKobject::new("non-utf8");
        write_acceptance_kobject(&k);
        k.write_bytes("mcbios_req", &[0xFF, 0xFE, 0x0A]);
        let mut expected = acceptance_regs();
        expected.mcbios_req = None;
        let out = read_from(&k.root).expect("containment -> Ok");
        assert_eq!(out.regs, expected);
    }

    // -----------------------------------------------------------------
    // (d) Parse strictness (the frozen §3.2 rules).
    // -----------------------------------------------------------------

    /// `parse_reg8`: exactly `0x` + 8 hex digits.
    #[test]
    fn parse_reg8_strict_shape() {
        assert_eq!(parse_reg8("0x00000012\n"), Some(0x0000_0012));
        assert_eq!(parse_reg8("0x11110F11"), Some(0x1111_0F11)); // uppercase digits
        assert_eq!(parse_reg8("0x27180204\r\n"), Some(0x2718_0204)); // stray \r tolerated
        assert_eq!(parse_reg8("0x0000012\n"), None); // 7 digits
        assert_eq!(parse_reg8("0x000000122\n"), None); // 9 digits
        assert_eq!(parse_reg8("0x0000001g\n"), None); // non-hex
        assert_eq!(parse_reg8("0X00000012\n"), None); // wrong-case prefix
        assert_eq!(parse_reg8("00000012\n"), None); // missing prefix
        assert_eq!(parse_reg8("0x00000012 \n"), None); // trailing space
        assert_eq!(parse_reg8("0x"), None);
        assert_eq!(parse_reg8(""), None);
    }

    /// `parse_base16`: exactly 16 hex digits, no prefix.
    #[test]
    fn parse_base16_strict_shape() {
        assert_eq!(parse_base16("00000000fed10000\n"), Some(0xFED1_0000));
        assert_eq!(parse_base16("00000000FED10000"), Some(0xFED1_0000));
        assert_eq!(parse_base16("0x00000000fed10000\n"), None); // prefix -> malformed
        assert_eq!(parse_base16("00000000fed1000\n"), None); // 15 digits
        assert_eq!(parse_base16("00000000fed100000\n"), None); // 17 digits
        assert_eq!(parse_base16(""), None);
    }

    /// `parse_enabled`: a single `1` / `0`.
    #[test]
    fn parse_enabled_strict_shape() {
        assert_eq!(parse_enabled("1\n"), Some(true));
        assert_eq!(parse_enabled("0"), Some(false));
        assert_eq!(parse_enabled("2\n"), None);
        assert_eq!(parse_enabled("01\n"), None);
        assert_eq!(parse_enabled(""), None);
    }

    // -----------------------------------------------------------------
    // (e) Pure vendor gate.
    // -----------------------------------------------------------------

    /// Intel passes; AMD and unknown vendors yield `UnsupportedHardware`
    /// (no I/O — unit-testable without a kobject).
    #[test]
    fn vendor_gate_intel_passes_others_rejected() {
        use crate::cpuid::{AmdZen, IntelGen};

        let intel = CpuInfo {
            vendor: CpuVendor::Intel(IntelGen::Skylake),
            brand: "Intel Core i5-6600T".to_owned(),
        };
        assert_eq!(vendor_gate(&intel), Ok(()));

        let amd = CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "AMD Ryzen 9 5950X".to_owned(),
        };
        assert_eq!(
            vendor_gate(&amd),
            Err(TelemetryError::UnsupportedHardware {
                vendor: "AMD Ryzen 9 5950X".to_owned()
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
    }

    // -----------------------------------------------------------------
    // (f) I/O classification arms.
    // -----------------------------------------------------------------

    /// `NotFound` → `DriverMissing`, `PermissionDenied` →
    /// `InsufficientPrivilege`, other → `Io`.
    #[test]
    fn classify_io_kinds() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
        assert_eq!(
            classify_io(&e),
            TelemetryError::DriverMissing {
                driver: "ramsleuth_intel"
            }
        );

        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        assert_eq!(
            classify_io(&e),
            TelemetryError::InsufficientPrivilege { hint: PRIV_HINT }
        );

        let e = std::io::Error::other("boom");
        assert!(matches!(classify_io(&e), TelemetryError::Io(_)));
    }

    /// Raw-OS-code construction classifies the same way (POSIX errno
    /// 2 = ENOENT, 13 = EACCES).
    #[test]
    fn classify_io_raw_os_codes() {
        assert_eq!(
            classify_io(&std::io::Error::from_raw_os_error(2)),
            TelemetryError::DriverMissing {
                driver: "ramsleuth_intel"
            }
        );
        assert_eq!(
            classify_io(&std::io::Error::from_raw_os_error(13)),
            TelemetryError::InsufficientPrivilege { hint: PRIV_HINT }
        );
    }

    /// A present-but-unreadable attribute → `InsufficientPrivilege`
    /// (skipped when running as root: root bypasses file permissions).
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_attribute_is_insufficient_privilege() {
        use std::os::unix::fs::PermissionsExt;

        if is_root() {
            return;
        }
        let k = TempKobject::new("unreadable");
        write_acceptance_kobject(&k);
        std::fs::set_permissions(k.root.join("mcbios_req"), std::fs::Permissions::from_mode(0o0))
            .expect("chmod");
        let err = read_from(&k.root).unwrap_err();
        assert_eq!(
            err,
            TelemetryError::InsufficientPrivilege { hint: PRIV_HINT }
        );
    }

    /// `true` when the process runs as root (real UID 0, from
    /// `/proc/self/status`: `Uid:\t0 0 0 0`); `false` when the file or
    /// field is missing (the file is Linux-only, as is this helper).
    #[cfg(target_os = "linux")]
    fn is_root() -> bool {
        std::fs::read_to_string("/proc/self/status")
            .map(|s| {
                s.lines().any(|l| {
                    l.strip_prefix("Uid:")
                        .is_some_and(|rest| rest.split_whitespace().next() == Some("0"))
                })
            })
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------
    // (g) Host + contract pins.
    // -----------------------------------------------------------------

    /// `acquire()` on this host is graceful: it returns a `Result`
    /// without panicking — `Ok` or one of the structured `Err` variants
    /// (which one is host-state dependent; shape only, plan §3.5).
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

    /// The frozen kobject path (plan §3.1).
    #[test]
    fn kobject_dir_is_frozen() {
        assert_eq!(KOBJECT_DIR, "/sys/kernel/ramsleuth_intel");
    }
}
