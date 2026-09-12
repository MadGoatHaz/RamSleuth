//! Telemetry facade — the crate's single public snapshot (P2-10).
//!
//! [`collect()`] aggregates every Phase 2 provider into one
//! [`SystemMemoryTelemetry`] with **per-branch error containment**: the
//! AMD branch (P2-03/04/05), the Intel branch (P2-06/07), and the
//! vendor-independent SPD branch (P2-08/09) each degrade independently
//! to a structured `Section::Na(reason)` (or an empty SPD list), so a
//! failure in one branch can never affect the others (plan D5).
//!
//! **No-panic contract:** [`collect()`] never returns `Err` and never
//! panics — unavailable data always degrades to `Section::Na(reason)`.
//! This is the no-panic contract for the whole crate.
//!
//! Branch topology (consumes the frozen provider APIs only; adds no new
//! I/O of its own):
//!
//! ```text
//! CpuInfo::detect() ──┬─ AMD:   amd_smu::acquire() → amd_pm::parse() → amd_readout::map_amd()
//!                     ├─ Intel: intel_mchbar::acquire() → intel_readout::read_intel()
//!                     └─ SPD:   spd_eeprom::acquire() → spd_decode::decode() (per image)
//! ```
//!
//! A vendor branch runs only on matching silicon: the AMD branch gates
//! on `CpuVendor::Amd(_)` and the Intel branch on `CpuVendor::Intel(_)`
//! *before* any provider call, so a non-matching vendor yields
//! `Section::Na(UnsupportedHardware)` with zero I/O in that branch. The
//! SPD branch runs on every vendor (unprivileged sysfs reads).

use crate::amd_pm;
use crate::amd_readout::{self, AmdReadout};
use crate::amd_smu;
use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_mchbar;
use crate::intel_readout::{self, IntelReadout};
use crate::spd_decode::{self, SpdModule};
use crate::spd_eeprom;

/// The full system memory telemetry snapshot — the Phase 2 exit-criteria
/// struct (v2 §2.2.1).
///
/// - `cpu`: the detected CPU whose vendor dispatched the branches.
/// - `amd`: the AMD SMU readout; `Na` when not on AMD silicon or when
///   the `ryzen_smu` driver is missing / unreadable / of unknown PM
///   table version.
/// - `intel`: the Intel MCHBAR readout; `Na` when not on Intel silicon
///   or when the MCHBAR map is unavailable.
/// - `spd`: the decoded SPD modules, one per bound `ee1004` device;
///   empty when the driver is absent or no device is bound.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemMemoryTelemetry {
    /// The detected CPU (vendor + brand).
    pub cpu: CpuInfo,
    /// AMD branch outcome: the mapped SMU readout, or a structured N/A
    /// reason.
    pub amd: Section<AmdReadout>,
    /// Intel branch outcome: the mapped MCHBAR readout, or a structured
    /// N/A reason.
    pub intel: Section<IntelReadout>,
    /// SPD branch outcome: one decoded module per bound device (empty
    /// when unavailable).
    pub spd: Vec<SpdModule>,
}

/// Collect the full [`SystemMemoryTelemetry`] snapshot.
///
/// # No-panic contract (D5)
///
/// This function **never returns `Err` and never panics**: every branch
/// is independently error-contained. Any [`TelemetryError`] in the AMD
/// or Intel chain degrades *only that branch* to `Section::Na(reason)`
/// (via [`reason_from`]), and any SPD failure degrades to an empty
/// module list. A failure in one branch can never take down the others.
///
/// # Behavior
///
/// 1. `CpuInfo::detect()` — the single CPUID dispatch key.
/// 2. AMD branch (AMD silicon only): `amd_smu::acquire()` →
///    `amd_pm::parse()` → `amd_readout::map_amd()`.
/// 3. Intel branch (Intel silicon only): `intel_mchbar::acquire()` →
///    `intel_readout::read_intel()`.
/// 4. SPD branch (all vendors, unprivileged): `spd_eeprom::acquire()` →
///    `spd_decode::decode()` per image.
pub fn collect() -> SystemMemoryTelemetry {
    // 1. Detect the CPU once; every vendor branch dispatches on it.
    let cpu = CpuInfo::detect();
    // 2. AMD branch (gated on vendor before any provider I/O).
    let amd = amd_branch(&cpu);
    // 3. Intel branch (gated on vendor before any provider I/O).
    let intel = intel_branch(&cpu);
    // 4. SPD branch (vendor-independent; per-device containment lives
    //    inside the provider).
    let spd = spd_branch();
    // 5. Assemble with per-branch containment (pure).
    assemble(cpu, amd, intel, spd)
}

/// Map a frozen [`TelemetryError`] to its display [`NaReason`] tag.
///
/// Pure and total: every variant maps to exactly one [`NaReason`]. The
/// `Io` / `Parse` family degrades to [`NaReason::ParseError`] carrying
/// the error's display text, so the P2-11 grid can render
/// `N/A (I/O error: …)` verbatim.
pub fn reason_from(e: &TelemetryError) -> NaReason {
    match e {
        TelemetryError::UnsupportedHardware { .. } => NaReason::UnsupportedHardware,
        TelemetryError::DriverMissing { .. } => NaReason::DriverMissing,
        TelemetryError::InsufficientPrivilege { .. } => NaReason::InsufficientPrivilege,
        TelemetryError::UnknownPmTableVersion { .. } => NaReason::UnknownPmTableVersion,
        TelemetryError::Io(err) => NaReason::ParseError(format!("I/O error: {err}")),
        TelemetryError::Parse { detail } => NaReason::ParseError(detail.clone()),
    }
}

/// AMD branch: `amd_smu::acquire()` → `amd_pm::parse()` →
/// `amd_readout::map_amd()`.
///
/// Gated on the already-detected vendor: non-AMD silicon returns
/// `Err(UnsupportedHardware)` before any provider call (zero I/O in
/// this branch). Failures are contained here — they never propagate
/// past the branch.
fn amd_branch(cpu: &CpuInfo) -> TelemetryResult<AmdReadout> {
    if !matches!(cpu.vendor, CpuVendor::Amd(_)) {
        return Err(TelemetryError::UnsupportedHardware {
            vendor: "AMD SMU telemetry requires AMD silicon".to_owned(),
        });
    }
    let ctx = amd_smu::acquire()?;
    let snap = amd_pm::parse(&ctx)?;
    Ok(amd_readout::map_amd(&snap))
}

/// Intel branch: `intel_mchbar::acquire()` → `intel_readout::read_intel()`.
///
/// Gated on the already-detected vendor: non-Intel silicon returns
/// `Err(UnsupportedHardware)` before any provider call (zero I/O in
/// this branch — the provider's own vendor gate is a second line of
/// defense, never the first).
fn intel_branch(cpu: &CpuInfo) -> TelemetryResult<IntelReadout> {
    if !matches!(cpu.vendor, CpuVendor::Intel(_)) {
        return Err(TelemetryError::UnsupportedHardware {
            vendor: "Intel MCHBAR telemetry requires Intel silicon".to_owned(),
        });
    }
    let bar = intel_mchbar::acquire()?;
    intel_readout::read_intel(&bar)
}

/// SPD branch: `spd_eeprom::acquire()` → `spd_decode::decode()` per
/// image.
///
/// Vendor-independent and unprivileged (pure sysfs reads). Any
/// acquisition error degrades to an empty module list — one bad source
/// never drops the others, and the branch never fails the process (the
/// provider already skips per-device failures).
fn spd_branch() -> Vec<SpdModule> {
    match spd_eeprom::acquire() {
        Ok(images) => images.iter().map(spd_decode::decode).collect(),
        Err(_) => Vec::new(),
    }
}

/// Assemble the snapshot from the branch results (pure — no I/O).
///
/// This is the unit of per-branch containment: each `Err` degrades only
/// its own `Section` (via [`reason_from`]) while the other branches'
/// data is carried through untouched.
fn assemble(
    cpu: CpuInfo,
    amd: TelemetryResult<AmdReadout>,
    intel: TelemetryResult<IntelReadout>,
    spd: Vec<SpdModule>,
) -> SystemMemoryTelemetry {
    SystemMemoryTelemetry {
        cpu,
        amd: section_from(amd),
        intel: section_from(intel),
        spd,
    }
}

/// Map one vendor-branch result to a `Section`: `Ok` → `Value`,
/// `Err` → `Na(reason_from(err))` — the single containment rule shared
/// by the AMD and Intel branches.
fn section_from<T>(result: TelemetryResult<T>) -> Section<T> {
    match result {
        Ok(value) => Section::Value(value),
        Err(err) => Section::Na(reason_from(&err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amd_readout::{CadBus, ClockReadout, DivMode, RttValue, TimingSet, VoltageSet};
    use crate::cpuid::{AmdZen, IntelGen};

    /// (a) `reason_from` maps each of the six frozen `TelemetryError`
    /// variants to the correct `NaReason`.
    #[test]
    fn reason_from_maps_all_six_variants() {
        assert_eq!(
            reason_from(&TelemetryError::UnsupportedHardware {
                vendor: "Unknown".to_owned()
            }),
            NaReason::UnsupportedHardware
        );
        assert_eq!(
            reason_from(&TelemetryError::DriverMissing { driver: "ryzen_smu" }),
            NaReason::DriverMissing
        );
        assert_eq!(
            reason_from(&TelemetryError::InsufficientPrivilege { hint: "run as root" }),
            NaReason::InsufficientPrivilege
        );
        assert_eq!(
            reason_from(&TelemetryError::UnknownPmTableVersion { version: 0x0711_0300 }),
            NaReason::UnknownPmTableVersion
        );
        // Io → ParseError carrying the inner error's display text.
        assert_eq!(
            reason_from(&TelemetryError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied"
            ))),
            NaReason::ParseError("I/O error: denied".to_owned())
        );
        // Parse → ParseError carrying the raw detail verbatim.
        assert_eq!(
            reason_from(&TelemetryError::Parse {
                detail: "truncated PM blob at offset 0x1A".to_owned()
            }),
            NaReason::ParseError("truncated PM blob at offset 0x1A".to_owned())
        );
    }

    /// (b) `collect()` on this host (AMD Ryzen 9 5950X) never panics
    /// and has the frozen structure: `cpu.vendor` is `Amd(..)`, the AMD
    /// branch is `Na(DriverMissing)` (the `ryzen_smu` module is absent
    /// on this host) or `Value` (if the module happens to be loaded),
    /// the Intel branch is `Na(UnsupportedHardware)` (this host is AMD,
    /// not Intel), and `spd` is a `Vec` (this host has two DDR4 DIMMs —
    /// the count is host-dependent and is not asserted).
    #[test]
    fn collect_on_this_host_is_structural_and_panic_free() {
        // Running this to completion is itself the no-panic check.
        let t = collect();

        // cpu: this host is AMD.
        assert!(
            matches!(t.cpu.vendor, CpuVendor::Amd(_)),
            "this host must detect as AMD, got {:?}",
            t.cpu.vendor
        );

        // amd: Na(DriverMissing) (module absent) or Value (module loaded).
        assert!(
            matches!(t.amd, Section::Value(_))
                || matches!(t.amd, Section::Na(NaReason::DriverMissing)),
            "AMD branch must be Value or Na(DriverMissing), got {:?}",
            t.amd
        );

        // intel: this host is not Intel → Na(UnsupportedHardware).
        assert_eq!(t.intel, Section::Na(NaReason::UnsupportedHardware));

        // spd: structurally a Vec<SpdModule> (possibly non-empty); each
        // carried module keeps a nonzero I2C index. Contents and count
        // are host-dependent — not asserted.
        for module in &t.spd {
            assert!(module.index > 0, "SPD module index must be nonzero");
        }
    }

    /// (c) Per-branch containment: an `Err` in the AMD branch degrades
    /// *only* the AMD `Section` — the Intel branch's `Na` and the SPD
    /// modules are carried through untouched.
    #[test]
    fn amd_branch_error_does_not_affect_other_branches() {
        let cpu = CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "Ryzen 9 5950X".to_owned(),
        };
        let amd: TelemetryResult<AmdReadout> =
            Err(TelemetryError::DriverMissing { driver: "ryzen_smu" });
        let intel: TelemetryResult<IntelReadout> = Err(TelemetryError::UnsupportedHardware {
            vendor: "AMD (Intel IMC decode requires an Intel CPU)".to_owned(),
        });
        let spd = vec![fixture_module(0x52), fixture_module(0x53)];

        let t = assemble(cpu, amd, intel, spd.clone());

        // AMD: its own Err → Na(DriverMissing).
        assert_eq!(t.amd, Section::Na(NaReason::DriverMissing));
        // Intel: independently Na(UnsupportedHardware).
        assert_eq!(t.intel, Section::Na(NaReason::UnsupportedHardware));
        // SPD: the modules survive the AMD failure verbatim.
        assert_eq!(t.spd, spd);
        assert_eq!(t.spd.len(), 2);
    }

    /// (c′) Symmetric containment: an Intel-branch `Err` degrades only
    /// the Intel `Section`; the AMD branch and the SPD modules are
    /// carried through untouched.
    #[test]
    fn intel_branch_error_does_not_affect_other_branches() {
        let cpu = CpuInfo {
            vendor: CpuVendor::Intel(IntelGen::Skylake),
            brand: "Intel Core i7-8700K".to_owned(),
        };
        let amd: TelemetryResult<AmdReadout> = Err(TelemetryError::UnsupportedHardware {
            vendor: "Intel (AMD SMU telemetry requires AMD silicon)".to_owned(),
        });
        let intel: TelemetryResult<IntelReadout> =
            Err(TelemetryError::InsufficientPrivilege { hint: "run as root" });
        let spd = vec![fixture_module(0x50)];

        let t = assemble(cpu, amd, intel, spd.clone());

        // AMD: independently Na(UnsupportedHardware).
        assert_eq!(t.amd, Section::Na(NaReason::UnsupportedHardware));
        // Intel: its own Err → Na(InsufficientPrivilege).
        assert_eq!(t.intel, Section::Na(NaReason::InsufficientPrivilege));
        // SPD: carried through verbatim.
        assert_eq!(t.spd, spd);
    }

    /// `section_from` is the single containment rule for both vendor
    /// branches: `Ok` → `Value`, `Err` → `Na(reason_from(err))`.
    #[test]
    fn section_from_maps_ok_to_value_and_err_to_na() {
        let ok: TelemetryResult<u32> = Ok(42);
        assert_eq!(section_from(ok), Section::Value(42));

        let err: TelemetryResult<u32> =
            Err(TelemetryError::InsufficientPrivilege { hint: "run as root" });
        assert_eq!(section_from(err), Section::Na(NaReason::InsufficientPrivilege));
    }

    /// A minimal `SpdModule` fixture for the containment tests (fields
    /// are host-independent).
    fn fixture_module(index: u8) -> SpdModule {
        SpdModule {
            index,
            is_ddr5: false,
            maker: Section::Value("0xC1".to_owned()),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(1),
            density_mbit: Section::na(NaReason::ParseError("fixture".to_owned())),
            speed_mts: Section::Value(3200),
            profiles: Vec::new(),
        }
    }
    // ------------------------------------------------------------------
    // (d) P3-06: serde wire contract - whole-snapshot round-trips.
    // ------------------------------------------------------------------

    /// A representative [`AmdReadout`] fixture (mixed `Value` + `Na`
    /// cells across all four display sets), host-independent.
    fn fixture_amd() -> AmdReadout {
        AmdReadout {
            clocks: ClockReadout {
                mclk_mhz: Section::Value(1600.0),
                uclk_mhz: Section::Value(1600.0),
                fclk_mhz: Section::Value(1800.0),
                div_mode: Section::Value(DivMode::OneToOne),
                gear_mode: Section::na(NaReason::NotApplicable),
                gdm: Section::Value(false),
                pdm: Section::Value(true),
            },
            timings: TimingSet {
                cl: Section::Value(16),
                rcwdwr: Section::Value(16),
                rcdrd: Section::Value(16),
                rp: Section::Value(16),
                ras: Section::Value(34),
                rc: Section::Value(50),
                rrds: Section::Value(4),
                rrld: Section::Value(8),
                faw: Section::Value(16),
                wtrs: Section::Value(4),
                wtrl: Section::Value(12),
                wr: Section::Value(20),
                rfc1: Section::Value(75),
                rfc2: Section::na(NaReason::ParseError("fixture".to_owned())),
                rfcsb: Section::Value(38),
                cwl: Section::Value(12),
                rtp: Section::Value(8),
                rdwr: Section::Value(8),
                wrrd: Section::Value(4),
                rdrd_sd: Section::Value(4),
                rdrd_dd: Section::Value(8),
                rdrd_scl: Section::Value(8),
                rdrd_sc: Section::Value(8),
                wrwr_sd: Section::Value(4),
                wrwr_dd: Section::Value(8),
                wrwr_scl: Section::Value(8),
                wrwr_sc: Section::Value(8),
            },
            cad_bus: CadBus {
                proc_odt: Section::Value(33.0),
                rtt_nom: Section::Value(RttValue::Rzq(10)),
                rtt_wr: Section::Value(RttValue::Ohms(45.0)),
                rtt_park: Section::na(NaReason::NotApplicable),
                clk_drv: Section::Value(48.0),
                addr_cmd_drv: Section::Value(48.0),
                cs_odt_drv: Section::Value(33.0),
                cke_drv: Section::Value(48.0),
            },
            voltages: VoltageSet {
                vddcr_soc_mv: Section::Value(1150),
                vddio_mem_mv: Section::Value(1350),
                vdd_misc_mv: Section::Value(1100),
                vpp_mv: Section::Value(1800),
            },
        }
    }

    /// The full snapshot root is wire-safe: a representative
    /// [`SystemMemoryTelemetry`] (a `Value` AMD readout with mixed
    /// `Value`/`Na` cells, a `Na` Intel branch, two SPD modules) and a
    /// fully-degraded all-`Na` snapshot each round-trip through bincode
    /// and compare equal — every field of the Phase 2 exit-criteria
    /// struct crosses the wire.
    #[test]
    fn system_memory_telemetry_bincode_round_trip() {
        // representative: Value + Na sections across CPU / AMD / Intel / SPD
        let t = SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(fixture_amd()),
            intel: Section::Na(NaReason::UnsupportedHardware),
            spd: vec![fixture_module(0x52), fixture_module(0x53)],
        };

        let bytes = bincode::serialize(&t)
            .expect("SystemMemoryTelemetry must serialize (no-panic contract)");
        let back: SystemMemoryTelemetry =
            bincode::deserialize(&bytes).expect("SystemMemoryTelemetry must deserialize");
        assert_eq!(t, back);

        // fully degraded: every branch `Na`, no SPD modules
        let all_na = SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Unknown".to_owned(),
            },
            amd: Section::Na(NaReason::DriverMissing),
            intel: Section::Na(NaReason::InsufficientPrivilege),
            spd: Vec::new(),
        };
        let bytes = bincode::serialize(&all_na)
            .expect("SystemMemoryTelemetry must serialize (no-panic contract)");
        let back: SystemMemoryTelemetry =
            bincode::deserialize(&bytes).expect("SystemMemoryTelemetry must deserialize");
        assert_eq!(all_na, back);
    }

    /// (d') Live snapshot: `collect()` on this host (root or not)
    /// bincode-serializes, round-trips, and compares equal — the whole
    /// Phase 2 snapshot is wire-safe in every privilege state.
    #[test]
    fn collect_on_this_host_bincode_round_trip() {
        let t = collect();
        let bytes = bincode::serialize(&t)
            .expect("SystemMemoryTelemetry must serialize (no-panic contract)");
        let back: SystemMemoryTelemetry =
            bincode::deserialize(&bytes).expect("SystemMemoryTelemetry must deserialize");
        assert_eq!(t, back);
    }
}
