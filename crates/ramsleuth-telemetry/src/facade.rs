//! Telemetry facade — the crate's single public snapshot (P2-10).
//!
//! [`collect()`] aggregates every Phase 2 provider into one
//! [`SystemMemoryTelemetry`] with **per-branch error containment**: the
//! AMD branch (P2-03/04/05), the Intel branch (P2-06/07), the
//! vendor-independent SPD branch (P2-08/09), and the vendor-independent
//! platform branch (C6-01) each degrade independently to a structured
//! `Section::Na(reason)` (or an empty SPD list), so a failure in one
//! branch can never affect the others (plan D5). The per-DIMM
//! capacities (`dimm_sizes`) derive from the SPD modules, and the total
//! capacity prefers the `/proc/meminfo` `MemTotal` (the OS ground
//! truth) over the SPD sum, which survives as the non-meminfo
//! fallback (D-C3/D-C9, D-3). The board-VRM read (C12-03) runs after
//! the platform branch and feeds the fill-when-Na `vddio_mem_mv`
//! overlay at assembly (D-5: a carried `Value` is never clobbered;
//! VPP / VDD_MISC unmapped, untouched).
//!
//! **No-panic contract:** [`collect()`] never returns `Err` and never
//! panics — unavailable data always degrades to `Section::Na(reason)`.
//! This is the no-panic contract for the whole crate.
//!
//! Branch topology (consumes the frozen provider APIs only; adds no new
//! I/O of its own):
//!
//! ```text
//! CpuInfo::detect() ──┬─ AMD:      amd_smu::acquire() → amd_pm::parse() → amd_smn::apply_smn (overlay) → amd_readout::map_amd()
//!                     ├─ Intel:    intel_sysfs::acquire() (primary, §3.5) → intel_readout::decode
//!                     │           └─ on DriverMissing only: intel_mchbar::acquire() → read_intel (/dev/mem fallback)
//!                     ├─ SPD:      spd_eeprom::acquire() → spd_decode::decode() (per image)
//!                     ├─ Platform: platform::collect_platform() (C6-01) → dimm_sizes + total_capacity (D-C3)
//!                     └─ Board VRM: board_vrm::read_board_vrm(&platform.motherboard) (C12-03) → fill-when-Na vddio_mem_mv overlay (D-5)
//! ```
//!
//! A vendor branch runs only on matching silicon: the AMD branch gates
//! on `CpuVendor::Amd(_)` and the Intel branch on `CpuVendor::Intel(_)`
//! *before* any provider call, so a non-matching vendor yields
//! `Section::Na(UnsupportedHardware)` with zero I/O in that branch. The
//! SPD and platform branches run on every vendor (unprivileged sysfs /
//! DMI + `/proc` reads); the per-DIMM capacities derive from the SPD
//! modules (`density_mbit × devices / 8192`, D-C3) and the total
//! capacity prefers the meminfo `MemTotal` (the OS ground truth) over
//! the SPD sum, which survives as the non-meminfo fallback (D-C9, D-3).

use crate::amd_pm;
use crate::amd_readout::{self, AmdReadout};
use crate::amd_smn;
use crate::amd_smu;
use crate::board_vrm::{self, BoardVrmReadout};
use crate::cpuid::{CpuInfo, CpuVendor};
use crate::error::{NaReason, Section, TelemetryError, TelemetryResult};
use crate::intel_mchbar;
use crate::intel_readout::{self, IntelReadout};
use crate::intel_sysfs;
use crate::platform::{self, SystemPlatform};
use crate::spd_decode::{self, SpdModule};
use crate::spd_eeprom;

/// The full system memory telemetry snapshot — the Phase 2 exit-criteria
/// struct (v2 §2.2.1), extended in Cycle 6 with the platform branch
/// (C6-01) and the derived capacities (D-C3/D-C9).
///
/// - `cpu`: the detected CPU whose vendor dispatched the branches.
/// - `amd`: the AMD SMU readout; `Na` when not on AMD silicon or when
///   the `ryzen_smu` driver is missing / unreadable / of unknown PM
///   table version.
/// - `intel`: the Intel IMC readout (the `ramsleuth_intel` sysfs
///   kobject primary, the `/dev/mem` MCHBAR fallback); `Na` when not
///   on Intel silicon, not a v1 profiled generation (Tier 1 + Rocket
///   Lake), or when both raw sources are unavailable.
/// - `spd`: the decoded SPD modules, one per bound `ee1004` device;
///   empty when the driver is absent or no device is bound.
/// - `platform`: the vendor-neutral platform identity (C6-01, D-C1);
///   each of its four fields degrades independently to
///   `Na(NotApplicable)`.
/// - `total_capacity`: total installed memory in GiB (D-3/D-C3): the
///   `/proc/meminfo` `MemTotal` (the OS ground truth) when it carries
///   a value, else the sum of the [`dimm_sizes`](Self::dimm_sizes)
///   entries that carry a value (the non-meminfo fallback), else `Na`.
/// - `dimm_sizes`: per-DIMM capacity in GiB (D-C3/D-C9), parallel to
///   [`spd`](Self::spd) — entry `i` is `density_mbit × devices / 8192`
///   for module `i` (`devices` = the module's total DRAM device
///   count, D-2); `Na` when that module's density or devices is `Na`
///   (carrying the offending source's reason).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemMemoryTelemetry {
    /// The detected CPU (vendor + brand).
    pub cpu: CpuInfo,
    /// AMD branch outcome: the mapped SMU readout, or a structured N/A
    /// reason.
    pub amd: Section<AmdReadout>,
    /// Intel branch outcome: the mapped IMC readout (sysfs primary /
    /// `/dev/mem` fallback), or a structured N/A reason.
    pub intel: Section<IntelReadout>,
    /// SPD branch outcome: one decoded module per bound device (empty
    /// when unavailable).
    pub spd: Vec<SpdModule>,
    /// Platform branch outcome (C6-01): the vendor-neutral identity
    /// snapshot; every field degrades to `Na(NotApplicable)`
    /// independently.
    pub platform: SystemPlatform,
    /// Total installed capacity in GiB (D-3/D-C3): the
    /// `/proc/meminfo` `MemTotal` (the OS ground truth) when it
    /// carries a value, else the sum of the `Value`
    /// [`dimm_sizes`](Self::dimm_sizes) entries (the non-meminfo
    /// fallback), else `Na`.
    pub total_capacity: Section<f64>,
    /// Per-DIMM capacity in GiB (D-C3/D-C9), parallel to
    /// [`spd`](Self::spd): entry `i` is `density_mbit × devices /
    /// 8192` for module `i` (`devices` = the module's total DRAM
    /// device count, D-2), `Na` when that module's density or
    /// devices is `Na`.
    pub dimm_sizes: Vec<Section<f64>>,
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
/// 3. Intel branch (Intel silicon only, v1 profiled generations —
///    Tier 1 + Rocket Lake):
///    `intel_sysfs::acquire()` → `intel_readout::decode` (primary);
///    on `DriverMissing` (kobject absent) only:
///    `intel_mchbar::acquire()` → `intel_readout::read_intel` (the
///    `/dev/mem` fallback, plan §3.5).
/// 4. SPD branch (all vendors, unprivileged): `spd_eeprom::acquire()` →
///    `spd_decode::decode()` per image.
/// 5. Platform branch (all vendors, C6-01): `platform::collect_platform()`
///    — every field degrades independently and it never fails the
///    process.
/// 6. Board VRM branch (all vendors, C12-03):
///    `board_vrm::read_board_vrm(&platform.motherboard)` — the
///    DMI-keyed profile + nct6798 in13 read; every rail degrades to
///    `Na(NotApplicable)` when the board is unknown / the device is
///    absent (§9). Its VDDIO_MEM reading feeds the fill-when-Na
///    `vddio_mem_mv` overlay at assembly (D-5); the I/O stays here,
///    never in `assemble`.
/// 7. Per-DIMM capacities (D-C3/D-C9): `density_mbit × devices / 8192`
///    per SPD module (parallel to the SPD list; `Na` when that module's
///    density or devices is `Na`).
/// 8. Total capacity (D-3/D-C3): the `/proc/meminfo` `MemTotal` (the
///    OS ground truth) when it carries a value, else the sum of the
///    DIMM capacities (the non-meminfo fallback).
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
    // 5. Platform branch (vendor-independent, C6-01; never fails the
    //    process — every field degrades independently).
    let platform = platform_branch();
    // 6. Board VRM branch (vendor-independent, C12-03; the DMI-keyed
    //    profile + nct6798 in13 read — every rail degrades to
    //    Na(NotApplicable) on an unknown board / absent device, §9).
    let board_vrm = board_vrm::read_board_vrm(&platform.motherboard);
    // 7. Per-DIMM capacities (D-C3/D-C9): parallel to the SPD modules.
    let dimm_sizes = dimm_sizes(&spd);
    // 8. Total capacity (D-3/D-C3): the meminfo `MemTotal` (the OS
    //    ground truth) when it carries a value, else the DIMM sum
    //    (the non-meminfo fallback).
    let total_capacity = total_capacity(&dimm_sizes);
    // 9. Assemble with per-branch containment + the fill-when-Na
    //    VDDIO_MEM overlay (pure).
    assemble(
        cpu,
        amd,
        intel,
        spd,
        platform,
        total_capacity,
        dimm_sizes,
        board_vrm,
    )
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

/// Intel branch: the two-source acquisition of plan §3.5 —
/// `intel_sysfs::acquire()` (primary, the `ramsleuth_intel` kobject) →
/// `intel_readout::decode`; on `DriverMissing` only (the kobject is
/// absent — the module is not loaded) fall through to
/// `intel_mchbar::acquire()` → `intel_readout::read_intel` (the `/dev/mem`
/// MCHBAR fallback).
///
/// Gated on the already-detected vendor: non-Intel silicon returns
/// `Err(UnsupportedHardware)` before any provider call (zero I/O in
/// this branch — the provider's own vendor gate is a second line of
/// defense, never the first). A non-profiled Intel generation (v1
/// decodes Tier 1 — Skylake / Kaby Lake / Coffee Lake / Comet Lake —
/// plus Rocket Lake) degrades the whole branch to
/// `Err(UnsupportedHardware)` before either raw source is touched —
/// never garbage data from a mismatched register map.
///
/// The fallback rule is frozen: only a sysfs `DriverMissing` takes the
/// `/dev/mem` path; any other sysfs outcome (`Parse` /
/// `InsufficientPrivilege` / `Io`) propagates as-is — the module being
/// loaded means the hardware is reachable, and silently switching
/// sources would mask a real fault. Both sources feed the same pure
/// decode core (`decode` over `IntelImcRegs`), and a branch failure
/// degrades only the Intel `Section` (via [`reason_from`]).
fn intel_branch(cpu: &CpuInfo) -> TelemetryResult<IntelReadout> {
    // 1. Pure vendor gate (zero I/O): non-Intel silicon is rejected
    //    before any provider call (the branch's first line of defense).
    intel_sysfs::vendor_gate(cpu)?;
    // 2. The detected generation (pure; the vendor gate passed, so this
    //    is `Ok` for Intel silicon — re-verified, zero I/O).
    let gen = intel_readout::intel_gen_gate(cpu)?;
    // 3. The profile-based generation gate (IG-16): Tier 1 + Rocket
    //    Lake (Tier 2) pass; any other Intel generation degrades the
    //    whole branch before any raw source is touched (never garbage
    //    from a mismatched register map).
    intel_readout::gen_gate(gen)?;

    // 4. Primary: the `ramsleuth_intel` sysfs kobject (plan §3.5).
    match intel_sysfs::acquire() {
        // The raw set feeds the same pure decode core as the fallback.
        Ok(sysfs) => Ok(intel_readout::decode(&sysfs.regs, gen, sysfs.mad_inter_channel, sysfs.mad_dimm_ch0, sysfs.mad_dimm_ch1)),
        // The frozen fallthrough rule: the `/dev/mem` fallback is taken
        // ONLY on `DriverMissing` (kobject absent — module not loaded).
        // Any other sysfs outcome is reported as-is.
        Err(TelemetryError::DriverMissing { .. }) => {
            let bar = intel_mchbar::acquire()?;
            intel_readout::read_intel(&bar)
        }
        Err(err) => Err(err),
    }
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

/// Platform branch (C6-01): `platform::collect_platform()`.
///
/// Vendor-independent, like the SPD branch: it runs on every vendor and
/// every field degrades independently to `Na(NotApplicable)` — a
/// failure in this branch never affects the others and never fails the
/// process (the entry point never returns `Err`).
fn platform_branch() -> SystemPlatform {
    platform::collect_platform()
}

/// Per-DIMM capacities in GiB (D-C3/D-C9): one entry per SPD module,
/// parallel to `spd`.
///
/// Pure: entry `i` is `density_mbit × devices / 8192` for module `i`
/// (16384 Mbit × 8 devices / 8192 = 16 GiB). An entry degrades to `Na`
/// — carrying the offending source's reason (density's when both are
/// `Na`) — when that module's `density_mbit` or `devices` is `Na`.
/// Never a panic, never an invented value.
fn dimm_sizes(spd: &[SpdModule]) -> Vec<Section<f64>> {
    spd
        .iter()
        .map(|module| match (&module.density_mbit, &module.devices) {
            (Section::Na(reason), _) => Section::na(reason.clone()),
            (Section::Value(_), Section::Na(reason)) => Section::na(reason.clone()),
            (Section::Value(density_mbit), Section::Value(devices)) => {
                Section::Value((*density_mbit as f64) * (*devices as f64) / 8192.0)
            }
        })
        .collect()
}

/// Sum the per-DIMM capacities that carry a value (an `Na` entry
/// contributes nothing) when at least one does, else the honest
/// `Na(NotApplicable)` — the non-meminfo fallback for
/// [`total_capacity`] (D-3). Pure: no I/O, never a panic, never an
/// invented value.
fn sum_dimm_sizes(dimm_sizes: &[Section<f64>]) -> Section<f64> {
    let mut total = 0.0;
    let mut any_value = false;
    for cell in dimm_sizes {
        if let Section::Value(gib) = cell {
            total += gib;
            any_value = true;
        }
    }
    if any_value {
        Section::Value(total)
    } else {
        Section::na(NaReason::NotApplicable)
    }
}

/// Total installed capacity in GiB (D-3): the `/proc/meminfo`
/// `MemTotal` (the OS ground truth for installed memory, C6-01) when
/// that source carries a value — it reads slightly under the marketing
/// figure because of reserved memory, which is the honest OS view —
/// else the SPD sum ([`sum_dimm_sizes`], the non-meminfo fallback),
/// the chain ending in an honest `Na`, never a panic and never an
/// invented value. The per-DIMM sizes stay SPD-derived (D-2) — the
/// total is never scaled to them, and they are never scaled to it.
fn total_capacity(dimm_sizes: &[Section<f64>]) -> Section<f64> {
    let meminfo = platform::mem_total_gib();
    if meminfo.is_na() {
        sum_dimm_sizes(dimm_sizes)
    } else {
        meminfo
    }
}

/// Assemble the snapshot from the branch results (pure — no I/O).
///
/// This is the unit of per-branch containment: each `Err` degrades only
/// its own `Section` (via [`reason_from`]) while the other branches'
/// data is carried through untouched. The platform branch (already a
/// fully degraded-or-populated [`SystemPlatform`]) and the derived
/// capacities (`dimm_sizes` / `total_capacity`, computed in
/// [`collect()`] from the SPD modules — D-C3/D-C9) are carried through
/// as-is.
///
/// The board-VRM readout (C12-03, read in [`collect()`] — the I/O never
/// happens here) feeds the fill-when-Na `vddio_mem_mv` overlay (D-5):
/// the PM table carries no VDDIO voltage, so the AMD slot is
/// structurally `Na` and the profile's in13 reading fills it; a
/// carried `Value` is never clobbered, the Intel branch is out of the
/// overlay's reach, and VPP / VDD_MISC (unmapped, D-3) are untouched.
#[allow(clippy::too_many_arguments)] // C12-04 frozen shape: the board-VRM readout is a trailing parameter (plan §3)
fn assemble(
    cpu: CpuInfo,
    amd: TelemetryResult<AmdReadout>,
    intel: TelemetryResult<IntelReadout>,
    spd: Vec<SpdModule>,
    platform: SystemPlatform,
    total_capacity: Section<f64>,
    dimm_sizes: Vec<Section<f64>>,
    board_vrm: BoardVrmReadout,
) -> SystemMemoryTelemetry {
    let mut telemetry = SystemMemoryTelemetry {
        cpu,
        amd: section_from(amd),
        intel: section_from(intel),
        spd,
        platform,
        total_capacity,
        dimm_sizes,
    };
    // The C12-04 overlay (D-5): fill-when-Na only — the AMD slot is
    // structurally `Na` from the PM table, so the board profile's in13
    // reading fills it; a carried `Value` always stands (a hypothetical
    // future PM-sourced value can never be clobbered).
    match (&mut telemetry.amd, &board_vrm.vddio_mem_mv) {
        (Section::Value(readout), Section::Value(mv))
            if readout.voltages.vddio_mem_mv.is_na() =>
        {
            readout.voltages.vddio_mem_mv = Section::Value(*mv);
        }
        _ => {}
    }
    telemetry
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

    /// (b) `collect()` on this host never panics and has the frozen
    /// structure: `cpu.vendor` is the detected vendor (AMD on the
    /// reference host, Intel on CI runners — asserted against a fresh
    /// `detect()`, vendor-neutral), the AMD branch is `Value` /
    /// `Na(DriverMissing)` on AMD silicon (module loaded or absent) and
    /// `Na(UnsupportedHardware)` on non-AMD silicon (vendor gate), the
    /// Intel branch is `Na(UnsupportedHardware)` on non-Intel silicon
    /// (vendor gate) and — vendor/hardware-conditional, plan §3.5 — a
    /// `Value` or a hardware-access `Na` from the frozen set (the
    /// virtualized BAR5=0 / non-profiled `UnsupportedHardware`, the
    /// absent-kobject `DriverMissing` the blocked `/dev/mem` fallback
    /// still cannot clear, an `InsufficientPrivilege` / `ParseError`
    /// access failure) on Intel silicon, and `spd` is a `Vec` (the count
    /// is host-dependent and is not asserted).
    #[test]
    fn collect_on_this_host_is_structural_and_panic_free() {
        // Running this to completion is itself the no-panic check.
        let t = collect();

        // cpu: the detected vendor matches the host silicon (AMD on the
        // reference host, Intel on CI runners) — vendor-neutral.
        assert_eq!(
            t.cpu.vendor,
            CpuInfo::detect().vendor,
            "the facade must carry the detected vendor"
        );

        // amd: on AMD silicon, Value (module loaded) or Na(DriverMissing)
        // (module absent); on non-AMD silicon the vendor gate degrades the
        // branch to Na(UnsupportedHardware) before any provider call.
        match t.cpu.vendor {
            CpuVendor::Amd(_) => assert!(
                matches!(t.amd, Section::Value(_))
                    || matches!(t.amd, Section::Na(NaReason::DriverMissing)),
                "AMD branch must be Value or Na(DriverMissing), got {:?}",
                t.amd
            ),
            _ => assert_eq!(
                t.amd,
                Section::Na(NaReason::UnsupportedHardware),
                "non-AMD host: the AMD vendor gate must degrade the branch"
            ),
        }

        // intel: vendor/hardware-conditional (plan §3.5). On a non-Intel
        // host the vendor gate degrades the branch to
        // Na(UnsupportedHardware) before any provider call. On Intel
        // silicon the branch is a `Value` (either raw source decoded) or
        // a hardware-access `Na` from the frozen set — the virtualized
        // BAR5=0 / non-profiled `UnsupportedHardware`, the absent-kobject
        // `DriverMissing` that the blocked `/dev/mem` fallback still
        // cannot clear, an `InsufficientPrivilege` / `ParseError` access
        // failure — shape-only, never a panic.
        match t.cpu.vendor {
            CpuVendor::Intel(_) => assert!(
                matches!(t.intel, Section::Value(_))
                    || matches!(t.intel, Section::Na(NaReason::DriverMissing))
                    || matches!(t.intel, Section::Na(NaReason::InsufficientPrivilege))
                    || matches!(t.intel, Section::Na(NaReason::ParseError(_)))
                    || matches!(t.intel, Section::Na(NaReason::UnsupportedHardware)),
                "Intel host: the branch must be Value or a hardware-access Na, got {:?}",
                t.intel
            ),
            _ => assert_eq!(
                t.intel,
                Section::Na(NaReason::UnsupportedHardware),
                "non-Intel host: the Intel vendor gate must degrade the branch"
            ),
        }

        // spd: structurally a Vec<SpdModule> (possibly non-empty); each
        // carried module keeps a nonzero I2C index. Contents and count
        // are host-dependent — not asserted.
        for module in &t.spd {
            assert!(module.index > 0, "SPD module index must be nonzero");
        }

        // platform (C6-01): runs on every vendor; each field is a
        // `Value` or the frozen `Na(NotApplicable)` — never any other
        // reason.
        assert_value_or_na("cpu_clock_mhz", &t.platform.cpu_clock_mhz);
        assert_value_or_na("motherboard", &t.platform.motherboard);
        assert_value_or_na("bios", &t.platform.bios);
        assert_value_or_na("agesa", &t.platform.agesa);
        assert_value_or_na("smu_version", &t.platform.smu_version);

        // dimm_sizes (D-C9): parallel to the SPD list, positionally.
        assert_eq!(
            t.dimm_sizes.len(),
            t.spd.len(),
            "dimm_sizes must be parallel to spd"
        );

        // total_capacity (D-C3): an in-band positive `Value` (the DIMM
        // sum or the meminfo fallback) or the fallback's own
        // `Na(NotApplicable)` — never any other reason.
        match &t.total_capacity {
            Section::Value(gib) => {
                assert!(gib.is_finite() && *gib > 0.0, "total_capacity must be positive, got {gib}")
            }
            Section::Na(reason) => assert!(
                matches!(reason, NaReason::NotApplicable),
                "total_capacity Na must be NotApplicable, got {reason:?}"
            ),
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
        // The C6-06 fields carry through independently of the vendor
        // branches: a synthetic all-Na platform, the pure per-DIMM
        // capacities (the fixture modules carry Na density/devices),
        // and the derived total.
        let platform = fixture_platform();
        let dimm_sizes = dimm_sizes(&spd);
        let total_capacity = total_capacity(&dimm_sizes);

        let t = assemble(
            cpu,
            amd,
            intel,
            spd.clone(),
            platform.clone(),
            total_capacity.clone(),
            dimm_sizes.clone(),
            board_vrm::BoardVrmReadout::all_na(),
        );

        // AMD: its own Err → Na(DriverMissing) — the overlay only runs
        // on a `Value` amd readout, so the `vddio_mem_mv` slot stays
        // the Na the branch already carries (all-Na board readout:
        // inert).
        assert_eq!(t.amd, Section::Na(NaReason::DriverMissing));
        // Intel: independently Na(UnsupportedHardware).
        assert_eq!(t.intel, Section::Na(NaReason::UnsupportedHardware));
        // SPD: the modules survive the AMD failure verbatim.
        assert_eq!(t.spd, spd);
        assert_eq!(t.spd.len(), 2);
        // Platform / capacities: carried through verbatim.
        assert_eq!(t.platform, platform);
        assert_eq!(t.dimm_sizes, dimm_sizes);
        assert_eq!(t.total_capacity, total_capacity);
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
        // The C6-06 fields carry through independently of the vendor
        // branches (same shape as the AMD-failure arm above).
        let platform = fixture_platform();
        let dimm_sizes = dimm_sizes(&spd);
        let total_capacity = total_capacity(&dimm_sizes);

        let t = assemble(
            cpu,
            amd,
            intel,
            spd.clone(),
            platform.clone(),
            total_capacity.clone(),
            dimm_sizes.clone(),
            board_vrm::BoardVrmReadout::all_na(),
        );

        // AMD: independently Na(UnsupportedHardware) — the overlay only
        // reaches the AMD branch, so this arm never touches
        // `vddio_mem_mv` (all-Na board readout: inert).
        assert_eq!(t.amd, Section::Na(NaReason::UnsupportedHardware));
        // Intel: its own Err → Na(InsufficientPrivilege).
        assert_eq!(t.intel, Section::Na(NaReason::InsufficientPrivilege));
        // SPD: carried through verbatim.
        assert_eq!(t.spd, spd);
        // Platform / capacities: carried through verbatim.
        assert_eq!(t.platform, platform);
        assert_eq!(t.dimm_sizes, dimm_sizes);
        assert_eq!(t.total_capacity, total_capacity);
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
            die_maker: Section::na(NaReason::NotApplicable),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::na(NaReason::ParseError("fixture".to_owned())),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(1),
            density_mbit: Section::na(NaReason::ParseError("fixture".to_owned())),
            speed_mts: Section::Value(3200),
            profiles: Vec::new(),
        }
    }

    /// A fully-degraded [`SystemPlatform`] fixture (host-independent;
    /// every field the frozen `Na(NotApplicable)`).
    fn fixture_platform() -> SystemPlatform {
        SystemPlatform {
            cpu_clock_mhz: Section::na(NaReason::NotApplicable),
            motherboard: Section::na(NaReason::NotApplicable),
            bios: Section::na(NaReason::NotApplicable),
            agesa: Section::na(NaReason::NotApplicable),
            smu_version: Section::na(NaReason::NotApplicable),
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
                vcore_mv: Section::Value(1150),
            },
        }
    }

    /// The full snapshot root is wire-safe: a representative
    /// [`SystemMemoryTelemetry`] (a `Value` AMD readout with mixed
    /// `Value`/`Na` cells, a `Na` Intel branch, two SPD modules, a
    /// mixed platform, and mixed per-DIMM capacities) and a
    /// fully-degraded all-`Na` snapshot each round-trip through
    /// bincode and compare equal — every field of the snapshot struct
    /// crosses the wire.
    #[test]
    fn system_memory_telemetry_bincode_round_trip() {
        // representative: Value + Na sections across CPU / AMD / Intel /
        // SPD / platform / capacities
        let t = SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::Value(fixture_amd()),
            intel: Section::Na(NaReason::UnsupportedHardware),
            spd: vec![fixture_module(0x52), fixture_module(0x53)],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3600.0),
                motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
                bios: Section::Value("F60 + 09/15/2024".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            // A mixed `dimm_sizes` (one `Value`, one `Na`) plus a
            // `Value` total exercises both arms of the new cells on the
            // wire.
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![
                Section::Value(16.0),
                Section::na(NaReason::NotApplicable),
            ],
        };

        let bytes = bincode::serialize(&t)
            .expect("SystemMemoryTelemetry must serialize (no-panic contract)");
        let back: SystemMemoryTelemetry =
            bincode::deserialize(&bytes).expect("SystemMemoryTelemetry must deserialize");
        assert_eq!(t, back);

        // fully degraded: every branch `Na`, no SPD modules, no
        // platform data, no capacities
        let all_na = SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Unknown".to_owned(),
            },
            amd: Section::Na(NaReason::DriverMissing),
            intel: Section::Na(NaReason::InsufficientPrivilege),
            spd: Vec::new(),
            platform: fixture_platform(),
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
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
                vcore_mv: 1050,
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

    /// (e4) The full `amd_branch` wiring runs end to end without a
    /// panic: a synthetic AMD `CpuInfo` passes the branch vendor
    /// gate, then `acquire -> parse -> apply_smn -> map` executes
    /// against the real host — on AMD silicon the `ryzen_smu`
    /// provider runs (a success carries PM clocks + the overlay
    /// gdm/timings through the frozen P2-05 gates, a failure is a
    /// frozen acquire / parse error), and on non-AMD silicon (the
    /// Intel CI runners) the vendor gate inside the SMU provider
    /// fails the branch with `UnsupportedHardware`.
    #[test]
    fn amd_branch_smn_wiring_is_structural_and_panic_free() {
        let cpu = CpuInfo {
            vendor: CpuVendor::Amd(AmdZen::Zen3),
            brand: "Ryzen 9 5950X".to_owned(),
        };

        // Running this to completion is itself the no-panic check.
        match amd_branch(&cpu) {
            // The overlay adds no failure mode: a branch failure is
            // still one of the frozen TelemetryError variants (on
            // non-AMD silicon the vendor gate inside the SMU provider
            // adds UnsupportedHardware).
            Err(e) => assert!(
                matches!(
                    e,
                    TelemetryError::DriverMissing { .. }
                        | TelemetryError::InsufficientPrivilege { .. }
                        | TelemetryError::UnsupportedHardware { .. }
                        | TelemetryError::UnknownPmTableVersion { .. }
                        | TelemetryError::Io(_)
                        | TelemetryError::Parse { .. }
                ),
                "AMD branch failure must be a frozen TelemetryError variant: {e:?}"
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

    // ------------------------------------------------------------------
    // (f) C6-06: per-DIMM capacities + total capacity (D-C3/D-C9).
    // ------------------------------------------------------------------

    /// A [`SpdModule`] with `Value` density + devices (the capacity
    /// arithmetic fixture; everything else as in [`fixture_module`]).
    fn capacity_module(index: u8, density_mbit: u16, devices: u8) -> SpdModule {
        let mut module = fixture_module(index);
        module.density_mbit = Section::Value(density_mbit);
        module.devices = Section::Value(devices);
        module
    }

    /// (f1) `dimm_sizes` is parallel to `spd` and computes
    /// `density_mbit × devices / 8192` GiB per module: the plan's
    /// verification case (16384 Mbit × 8 devices = 16 GiB) plus smaller
    /// and non-power-of-two products (exact in binary: every factor
    /// divides 8192 cleanly).
    #[test]
    fn dimm_sizes_is_parallel_and_computes_capacity() {
        let spd = vec![
            capacity_module(0x52, 16384, 8), // 16 GiB
            capacity_module(0x53, 8192, 4),  // 4 GiB
            capacity_module(0x54, 12288, 8), // 12 GiB (non-power-of-two)
        ];

        let sizes = dimm_sizes(&spd);

        assert_eq!(sizes.len(), 3, "one entry per module, in order");
        assert_eq!(sizes[0], Section::Value(16.0));
        assert_eq!(sizes[1], Section::Value(4.0));
        assert_eq!(sizes[2], Section::Value(12.0));

        // An empty SPD list yields an empty (still parallel) list.
        assert_eq!(dimm_sizes(&[]), Vec::new());
    }

    /// (f2) Na propagation is per-module: a capacity entry degrades to
    /// `Na` carrying the offending source's reason (density's when both
    /// are `Na`), while the other modules are unaffected.
    #[test]
    fn dimm_sizes_propagates_na_per_module() {
        // Both density and devices Na → the density's reason wins.
        let density_na = fixture_module(0x50);
        // A clean module computes its capacity verbatim.
        let ok = capacity_module(0x51, 16384, 8);
        // Devices Na while the density is a value → the devices'
        // reason.
        let mut devices_na = capacity_module(0x52, 16384, 8);
        devices_na.devices = Section::na(NaReason::ParseError("invalid rank config".to_owned()));

        let sizes = dimm_sizes(&[density_na, ok, devices_na]);

        assert_eq!(sizes[0], Section::na(NaReason::ParseError("fixture".to_owned())));
        assert_eq!(sizes[1], Section::Value(16.0));
        assert_eq!(sizes[2], Section::na(NaReason::ParseError("invalid rank config".to_owned())));
    }

    /// (f3) `sum_dimm_sizes` is the sum of the DIMM capacities that
    /// carry a value (an `Na` entry contributes nothing) when at least
    /// one does, else the honest `Na(NotApplicable)` — the
    /// non-meminfo fallback arithmetic for [`total_capacity`] (D-3).
    #[test]
    fn sum_dimm_sizes_sums_the_value_cells() {
        let sizes = vec![
            Section::Value(8.0),
            Section::na(NaReason::NotApplicable),
            Section::Value(16.0),
        ];
        assert_eq!(sum_dimm_sizes(&sizes), Section::Value(24.0));

        // A single value module sums to itself.
        assert_eq!(sum_dimm_sizes(&[Section::Value(16.0)]), Section::Value(16.0));

        // All-`Na` (no DIMM carries a value) → the honest
        // Na(NotApplicable); an empty list degrades the same.
        let all_na = vec![
            Section::na(NaReason::NotApplicable),
            Section::na(NaReason::ParseError("invalid rank config".to_owned())),
        ];
        assert_eq!(sum_dimm_sizes(&all_na), Section::na(NaReason::NotApplicable));
        assert_eq!(sum_dimm_sizes(&[]), Section::na(NaReason::NotApplicable));

        // The host shape (two 16 GiB DIMMs) through the full
        // spd → dimm_sizes → sum_dimm_sizes chain.
        let spd = vec![capacity_module(0x52, 16384, 8), capacity_module(0x53, 16384, 8)];
        assert_eq!(sum_dimm_sizes(&dimm_sizes(&spd)), Section::Value(32.0));
    }

    /// (f3′) MemTotal preferred (D-3): the host shape (two 16 GiB
    /// DIMMs, SPD sum 32.0) — when the meminfo source carries a value
    /// the total is the OS ground truth, never the SPD sum (on the
    /// live 5950X host the enumeration is structurally incomplete —
    /// 2 of 4 DIMMs bound to `ee1004` — so the sum can never equal the
    /// OS total); when the meminfo source is absent the total degrades
    /// to the SPD sum (the non-meminfo fallback).
    #[test]
    fn total_capacity_prefers_meminfo_over_the_spd_sum() {
        // Two 16 GiB DIMMs: the SPD sum would read 32.0.
        let sizes = vec![Section::Value(16.0), Section::Value(16.0)];

        match platform::mem_total_gib() {
            // meminfo present → the OS ground truth wins over the sum
            // (the D-3 flip, pinned).
            Section::Value(gib) => {
                assert!(
                    gib.is_finite() && gib > 0.0,
                    "meminfo total must be positive: {gib}"
                );
                // `gib: f64` is `Copy`: reconstructing the section from
                // the inner value is value-identical to binding the whole
                // section — and it keeps the MSRV 1.75 test profile
                // compiling (a by-value `@` binding of the non-`Copy`
                // section plus a later use is E0382 there).
                assert_eq!(total_capacity(&sizes), Section::Value(gib));
            }
            // meminfo absent → the total is exactly the SPD sum.
            Section::Na(_) => {
                assert_eq!(total_capacity(&sizes), sum_dimm_sizes(&sizes));
                assert_eq!(total_capacity(&sizes), Section::Value(32.0));
            }
        }
    }

    /// (f4) `total_capacity` consults the `/proc/meminfo` total first
    /// (the D-3 primary) when no DIMM carries a value (including the
    /// empty-SPD case). `MemTotal` is boot-constant, so the result is
    /// deterministic — when the meminfo source is absent the total
    /// degrades to the SPD sum's own `Na(NotApplicable)`, the chain's
    /// honest end.
    #[test]
    fn total_capacity_falls_back_to_meminfo() {
        // No DIMM values (the fixture modules carry Na density/devices)
        // → the meminfo fallback, whatever this host reports.
        let sizes = dimm_sizes(&[fixture_module(0x52), fixture_module(0x53)]);
        assert!(
            sizes.iter().all(Section::is_na),
            "fixture modules carry no capacity value"
        );
        assert_eq!(total_capacity(&sizes), platform::mem_total_gib());

        // An empty SPD list → the same fallback.
        assert_eq!(total_capacity(&[]), platform::mem_total_gib());

        // The fallback is in-band: a positive finite GiB or the
        // honest `Na(NotApplicable)`.
        match platform::mem_total_gib() {
            Section::Value(gib) => {
                assert!(gib.is_finite() && gib > 0.0, "meminfo total must be positive: {gib}")
            }
            Section::Na(reason) => assert!(
                matches!(reason, NaReason::NotApplicable),
                "meminfo Na must be NotApplicable: {reason:?}"
            ),
        }
    }

    /// (f5) `platform_branch` runs on every vendor and never fails the
    /// process: on this host every field is a `Value` or the frozen
    /// `Na(NotApplicable)` (the C6-01 fallback column).
    /// A [`SystemPlatform`] cell must be a `Value` or the frozen
    /// `Na(NotApplicable)` — the only two states the platform branch
    /// produces (the C6-01 fallback column).
    fn assert_value_or_na<T: std::fmt::Debug>(name: &str, cell: &Section<T>) {
        assert!(
            matches!(cell, Section::Value(_) | Section::Na(NaReason::NotApplicable)),
            "platform.{name} must be Value or Na(NotApplicable), got {cell:?}"
        );
    }

    #[test]
    fn platform_branch_is_vendor_independent_and_graceful() {
        let p = platform_branch();
        assert_value_or_na("cpu_clock_mhz", &p.cpu_clock_mhz);
        assert_value_or_na("motherboard", &p.motherboard);
        assert_value_or_na("bios", &p.bios);
        assert_value_or_na("agesa", &p.agesa);
        assert_value_or_na("smu_version", &p.smu_version);
    }

    // ------------------------------------------------------------------
    // (g) C12-04: the fill-when-Na VDDIO_MEM overlay (D-5).
    // ------------------------------------------------------------------

    /// (g1) The merge: a profile `Value` fills the AMD `vddio_mem_mv`
    /// slot **only when it is `Na`** (the PM table carries no VDDIO
    /// voltage, so the slot is structurally Na); a carried `Value` is
    /// never clobbered (a hypothetical future PM-sourced reading wins);
    /// an all-Na readout leaves the slot `Na` (the §9 graceful path).
    /// VPP / VDD_MISC are unmapped (D-3) and survive every arm
    /// untouched; the Intel branch is out of the overlay reach.
    #[test]
    fn merge_board_vrm_fills_na_vddio_only() {
        fn amd_cpu() -> CpuInfo {
            CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            }
        }
        fn intel_err() -> TelemetryResult<IntelReadout> {
            Err(TelemetryError::UnsupportedHardware {
                vendor: "Intel (AMD SMU telemetry requires AMD silicon)".to_owned(),
            })
        }

        // Arm 1 — fill-when-Na: the structural Na slot + a profile
        // Value → the in13 reading lands.
        let mut amd_readout = fixture_amd();
        amd_readout.voltages.vddio_mem_mv = Section::na(NaReason::NotApplicable);
        let mut board_vrm = BoardVrmReadout::all_na();
        board_vrm.vddio_mem_mv = Section::Value(1200);
        let t = assemble(
            amd_cpu(),
            Ok(amd_readout),
            intel_err(),
            Vec::new(),
            fixture_platform(),
            Section::na(NaReason::NotApplicable),
            Vec::new(),
            board_vrm,
        );
        match &t.amd {
            Section::Value(readout) => {
                assert_eq!(readout.voltages.vddio_mem_mv, Section::Value(1200));
                // VPP / VDD_MISC: unmapped (D-3) — untouched.
                assert_eq!(readout.voltages.vpp_mv, Section::Value(1800));
                assert_eq!(readout.voltages.vdd_misc_mv, Section::Value(1100));
            }
            other => panic!("AMD branch must stay Value: {other:?}"),
        }

        // Arm 2 — no-clobber: a carried Value (the fixture 1350 mV) +
        // a profile Value → the carried value stands (D-5).
        let amd_readout = fixture_amd();
        let mut board_vrm = BoardVrmReadout::all_na();
        board_vrm.vddio_mem_mv = Section::Value(1200);
        let t = assemble(
            amd_cpu(),
            Ok(amd_readout),
            intel_err(),
            Vec::new(),
            fixture_platform(),
            Section::na(NaReason::NotApplicable),
            Vec::new(),
            board_vrm,
        );
        match &t.amd {
            Section::Value(readout) => assert_eq!(
                readout.voltages.vddio_mem_mv,
                Section::Value(1350),
                "a carried Value is never clobbered (D-5)"
            ),
            other => panic!("AMD branch must stay Value: {other:?}"),
        }

        // Arm 3 — all-Na fallback: the structural Na slot + an all-Na
        // readout (unknown board / absent device, §9 item 2) → the slot
        // stays Na(NotApplicable).
        let mut amd_readout = fixture_amd();
        amd_readout.voltages.vddio_mem_mv = Section::na(NaReason::NotApplicable);
        let t = assemble(
            amd_cpu(),
            Ok(amd_readout),
            intel_err(),
            Vec::new(),
            fixture_platform(),
            Section::na(NaReason::NotApplicable),
            Vec::new(),
            BoardVrmReadout::all_na(),
        );
        match &t.amd {
            Section::Value(readout) => assert_eq!(
                readout.voltages.vddio_mem_mv,
                Section::na(NaReason::NotApplicable)
            ),
            other => panic!("AMD branch must stay Value: {other:?}"),
        }
    }
}
