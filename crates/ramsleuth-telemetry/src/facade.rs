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
//! CpuInfo::detect() ──┬─ AMD:   amd_smu::acquire() → amd_pm::parse() → amd_smn::apply_smn (overlay) → amd_readout::map_amd()
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
use crate::amd_smn;
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
///    `amd_pm::parse()` → `amd_smn::apply_smn` (no-panic overlay) →
///    `amd_readout::map_amd()`.
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
/// `amd_smn::apply_smn` (no-panic overlay) → `amd_readout::map_amd()`.
///
/// Gated on the already-detected vendor: non-AMD silicon returns
/// `Err(UnsupportedHardware)` before any provider call (zero I/O in
/// this branch). Failures are contained here — they never propagate
/// past the branch. The SMN overlay (P6-03, plan D2) is infallible by
/// construction — a missing `smn` attribute is a no-op and per-register
/// failures contain to zeros — so only `acquire` / `parse` can fail the
/// branch: the error semantics are unchanged.
fn amd_branch(cpu: &CpuInfo) -> TelemetryResult<AmdReadout> {
    if !matches!(cpu.vendor, CpuVendor::Amd(_)) {
        return Err(TelemetryError::UnsupportedHardware {
            vendor: "AMD SMU telemetry requires AMD silicon".to_owned(),
        });
    }
    let ctx = amd_smu::acquire()?;
    let mut snap = amd_pm::parse(&ctx)?;
    // The no-panic overlay (P6-02, frozen): populates `gdm` + the 27
    // `timings` from the driver `smn` attribute when readable; never
    // fails the branch and never touches the PM-table fields.
    amd_smn::apply_smn(&mut snap);
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
    use crate::amd_readout::{
        CadBus, ClockReadout, CommandRate, DivMode, RttValue, TimingSet, VoltageSet,
    };
    use crate::amd_pm::{AmdPmCadBus, AmdPmSnapshot, AmdPmTimings, AmdPmVoltages};
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
                command_rate: Section::Value(CommandRate::TwoT),
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

    // ------------------------------------------------------------------
    // (e) P6-03: smn overlay wiring into amd_branch (plan D2).
    // ------------------------------------------------------------------

    /// A parsed-snapshot-shaped fixture: PM fields populated (live 5950X
    /// class), SMN fields zeroed — exactly the P2-04 `parse` output shape
    /// (the overlay write surface is `gdm` + `timings` only).
    fn pm_only_snapshot() -> AmdPmSnapshot {
        AmdPmSnapshot {
            version: 0x38_08_05,
            mclk_mhz: 1800,
            uclk_mhz: 1800,
            fclk_mhz: 1792,
            div_mode: 1,
            gdm: 0,
            pdm: 0,
            command_rate: 0,
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

    /// The verified `monitor_cpu` register feed (the P6-02 fixture words),
    /// with `gdm_on` setting/clearing bit 11 of `0x50200`.
    fn fixture_smn_regs(gdm_on: bool) -> Vec<(u32, Option<u32>)> {
        let mclk: u32 = 0x0000_1539;
        let mclk = if gdm_on { mclk | 0x0000_0800 } else { mclk };
        vec![
            (0x50200, Some(mclk)),
            (0x50204, Some(0x1010_2410)),
            (0x50208, Some(0x0010_0030)),
            (0x5020C, Some(0x0400_0404)),
            (0x50210, Some(0x0000_0010)),
            (0x50214, Some(0x0008_0410)),
            (0x50218, Some(0x0000_0010)),
            (0x50220, Some(0x0504_0302)),
            (0x50224, Some(0x0908_0706)),
            (0x50228, Some(0x0000_0602)),
            (0x50254, Some(0x0400_0000)),
            (0x50260, Some(0x7E08_20A0)),
            (0x50264, Some(0x1111_2222)),
        ]
    }

    /// (e1) Overlay data present: the frozen decode core maps a GDM-on
    /// register feed and the branch tail (`map_amd`) reflects it — GDM +
    /// the 27 timings cross into the display readout while the PM-table
    /// fields survive untouched. The live `apply_smn` entry point runs
    /// this exact composition; on a non-root host the attribute degrades
    /// to zeros, so the data-present path is pinned through the frozen
    /// public decode core it calls, with its documented write surface.
    #[test]
    fn smn_overlay_data_reflected_in_mapped_output() {
        let mut snap = pm_only_snapshot();

        let fields = crate::amd_smn::decode_smn(&fixture_smn_regs(true));
        assert_eq!(fields.gdm, 1); // bit 11 of 0x50200 set

        // The overlay write surface (plan D2): `gdm` + `timings` only.
        snap.gdm = fields.gdm;
        snap.timings = fields.timings;

        let ro = amd_readout::map_amd(&snap);

        // GDM on crosses into the display readout.
        assert_eq!(ro.clocks.gdm, Section::Value(true));
        // A sample across the register set reflects verbatim (the full
        // 27-field decode is pinned in P6-02 against the same words).
        assert_eq!(ro.timings.cl, Section::Value(16));
        assert_eq!(ro.timings.ras, Section::Value(36));
        assert_eq!(ro.timings.rc, Section::Value(48));
        assert_eq!(ro.timings.faw, Section::Value(16));
        assert_eq!(ro.timings.rfc1, Section::Value(160));
        assert_eq!(ro.timings.rfc2, Section::Value(260));
        assert_eq!(ro.timings.rfcsb, Section::Value(504));
        assert_eq!(ro.timings.rdrd_sc, Section::Value(4));
        assert_eq!(ro.timings.wrwr_scl, Section::Value(9));
        // PM-table fields survive the overlay untouched.
        assert_eq!(ro.clocks.mclk_mhz, Section::Value(1800.0));
        assert_eq!(ro.clocks.uclk_mhz, Section::Value(1800.0));
        assert_eq!(ro.clocks.fclk_mhz, Section::Value(1792.0));
        assert_eq!(ro.clocks.div_mode, Section::Value(DivMode::OneToTwo));
        assert_eq!(ro.voltages.vddcr_soc_mv, Section::Value(1128));
        // Unconfirmed fields keep their frozen P2-04 zero state.
        assert_eq!(ro.clocks.pdm, Section::Value(false));
        assert_eq!(ro.cad_bus.rtt_nom, Section::Value(RttValue::Disabled));
    }

    /// (e2) Plan-mandated check: `apply_smn` — the overlay call the
    /// wiring makes — on a host without the `smn` attribute leaves the
    /// SMN fields zeroed while the PM clocks/voltages survive (the
    /// status quo on older module builds). A sentinel-populated SMN
    /// state makes each arm non-vacuous: `DriverMissing` must no-op the
    /// overlay byte-for-byte; a present-but-unreadable attribute
    /// (per-register containment) must zero the SMN fields; a readable
    /// attribute yields mask-bounded live values. Never a panic, never a
    /// PM-field write.
    #[test]
    fn apply_smn_without_smn_attr_preserves_pm_and_keeps_smn_zeroed() {
        // P2-04 shape with sentinel SMN fields at the mask maxima (so the
        // bound checks below hold under every arm).
        let mut snap = pm_only_snapshot();
        snap.gdm = 1;
        snap.timings = AmdPmTimings {
            cl: 63, rcwdwr: 63, rcdrd: 63, rp: 63, ras: 127, rc: 255, rrds: 31,
            rrld: 31, faw: 255, wtrs: 31, wtrl: 63, wr: 255, rfc1: 1023,
            rfc2: 1023, rfcsb: 1023, cwl: 63, rtp: 31, rdwr: 31, wrrd: 15,
            rdrd_sd: 15, rdrd_dd: 15, rdrd_scl: 63, rdrd_sc: 15, wrwr_sd: 15,
            wrwr_dd: 15, wrwr_scl: 63, wrwr_sc: 15,
        };

        // The frozen public entry point the wiring calls (the host-state
        // probe runs first, so each arm is asserted against the state it
        // saw).
        let probe = crate::amd_smn::read_smn_register(0x50200);
        crate::amd_smn::apply_smn(&mut snap);

        // PM clocks/voltages survive every arm — the overlay write
        // surface is `gdm` + `timings` only (frozen contract).
        assert_eq!(snap.version, 0x38_08_05);
        assert_eq!(snap.mclk_mhz, 1800);
        assert_eq!(snap.uclk_mhz, 1800);
        assert_eq!(snap.fclk_mhz, 1792);
        assert_eq!(snap.div_mode, 1);
        assert_eq!(snap.voltages.vddcr_soc_mv, 1128);
        assert_eq!(snap.pdm, 0); // never written
        assert_eq!(snap.cad_bus, pm_only_snapshot().cad_bus); // never written

        if matches!(probe, Err(TelemetryError::DriverMissing { .. })) {
            // Host without the `smn` attribute: whole-overlay no-op —
            // the snapshot is byte-identical (sentinels intact).
            assert_eq!(snap.gdm, 1);
            assert_eq!(snap.timings.cl, 63);
        } else {
            // Present-but-unreadable: containment zeros the SMN fields.
            // Readable: mask-bounded live values. The sentinels are the
            // mask maxima, so the bounds hold under either outcome.
            assert!(snap.gdm <= 1);
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

    /// (e3) Per-register containment (the overlay frozen decode core): a
    /// failed read contributes `None` — only that register fields decode
    /// to zero; every other register fields decode exactly as with the
    /// full feed. One bad register never poisons the rest, and the
    /// zeroed fields render as honest `Na` in the mapped output.
    #[test]
    fn smn_one_failed_register_does_not_poison_the_rest() {
        let full = fixture_smn_regs(false);
        // Fail one register: 0x50214 (tCWL / tWTRS / tWTRL).
        let broken: Vec<(u32, Option<u32>)> = full
            .iter()
            .map(|(a, w)| (*a, if *a == 0x50214 { None } else { *w }))
            .collect();

        let full_f = crate::amd_smn::decode_smn(&full);
        let broken_f = crate::amd_smn::decode_smn(&broken);

        // The failed register own fields decode to zero...
        assert_eq!(broken_f.timings.cwl, 0);
        assert_eq!(broken_f.timings.wtrs, 0);
        assert_eq!(broken_f.timings.wtrl, 0);
        // ...and every other register fields decode exactly as before.
        assert_eq!(broken_f.gdm, full_f.gdm);
        assert_eq!(broken_f.timings.cl, full_f.timings.cl);
        assert_eq!(broken_f.timings.ras, full_f.timings.ras);
        assert_eq!(broken_f.timings.rc, full_f.timings.rc);
        assert_eq!(broken_f.timings.faw, full_f.timings.faw);
        assert_eq!(broken_f.timings.wr, full_f.timings.wr);
        assert_eq!(broken_f.timings.rfc1, full_f.timings.rfc1);
        assert_eq!(broken_f.timings.rdrd_sc, full_f.timings.rdrd_sc);
        assert_eq!(broken_f.timings.wrwr_scl, full_f.timings.wrwr_scl);
        assert_eq!(broken_f.timings.rdwr, full_f.timings.rdwr);

        // Through the branch tail: the zeroed fields render as honest
        // `Na` (frozen P2-05 gate) while the intact ones keep values.
        let mut snap = pm_only_snapshot();
        snap.gdm = broken_f.gdm;
        snap.timings = broken_f.timings;
        let ro = amd_readout::map_amd(&snap);
        assert!(ro.timings.cwl.is_na());
        assert!(ro.timings.wtrs.is_na());
        assert!(ro.timings.wtrl.is_na());
        assert_eq!(ro.timings.cl, Section::Value(16));
        assert_eq!(ro.timings.rfc1, Section::Value(160));
    }

    /// (e4) The full `amd_branch` wiring on this host (AMD silicon):
    /// acquire -> parse -> apply_smn -> map runs end to end without a
    /// panic, and the branch error semantics are unchanged — a failure
    /// is still one of the frozen `TelemetryError` variants (only
    /// acquire / parse can fail the branch now that the overlay is
    /// infallible), and a success carries PM clocks + the overlay
    /// gdm/timings through the frozen P2-05 gates (in-band values or
    /// honest `Na`, never garbage).
    #[test]
    fn amd_branch_smn_wiring_is_structural_and_panic_free() {
        let cpu = CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "Ryzen 9 5950X".to_owned(),
        };

        // Running this to completion is itself the no-panic check.
        match amd_branch(&cpu) {
            // The overlay adds no failure mode: on AMD silicon a branch
            // failure is still a frozen acquire / parse error.
            Err(e) => assert!(
                matches!(
                    e,
                    TelemetryError::DriverMissing { .. }
                        | TelemetryError::InsufficientPrivilege { .. }
                        | TelemetryError::UnknownPmTableVersion { .. }
                        | TelemetryError::Io(_)
                        | TelemetryError::Parse { .. }
                ),
                "AMD branch failure must be a frozen acquire/parse error: {e:?}"
            ),
            Ok(ro) => {
                // GDM crosses the frozen mode-flag gate: a single bit
                // always decodes to 0/1, so it is Value(bool), never Na.
                assert!(!ro.clocks.gdm.is_na(), "gdm must be Value(bool)");
                // PM clocks survive the overlay through the frozen clock
                // gate: in-band Value or honest Na, never an out-of-band
                // Value.
                let check_clock = |cell: &Section<f64>| {
                    if let Section::Value(mhz) = cell {
                        assert!((1.0..=4096.0).contains(mhz), "out-of-band clock {mhz}")
                    }
                };
                check_clock(&ro.clocks.mclk_mhz);
                check_clock(&ro.clocks.uclk_mhz);
                check_clock(&ro.clocks.fclk_mhz);
                // Every one of the 27 timings crosses the frozen ticks
                // gate: in-band Value(1..=2048) or honest Na (zeroed),
                // never an out-of-band Value.
                let t = &ro.timings;
                let check_timing = |cell: &Section<u16>| match cell {
                    Section::Value(ticks) => {
                        assert!((1..=2048).contains(ticks), "out-of-band timing {ticks}")
                    }
                    Section::Na(NaReason::ParseError(_)) => {}
                    other => panic!("unexpected timing cell: {other:?}"),
                };
                let all = [
                    &t.cl, &t.rcwdwr, &t.rcdrd, &t.rp, &t.ras, &t.rc, &t.rrds,
                    &t.rrld, &t.faw, &t.wtrs, &t.wtrl, &t.wr, &t.rfc1, &t.rfc2,
                    &t.rfcsb, &t.cwl, &t.rtp, &t.rdwr, &t.wrrd, &t.rdrd_sd,
                    &t.rdrd_dd, &t.rdrd_scl, &t.rdrd_sc, &t.wrwr_sd,
                    &t.wrwr_dd, &t.wrwr_scl, &t.wrwr_sc,
                ];
                for cell in &all {
                    check_timing(cell);
                }
            }
        }
    }
}
