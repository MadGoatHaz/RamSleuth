//! ramsleuth-display — the shared presentation layer (WS-A, C3 single-source).
//!
//! One implementation of every value formatter, N/A text map, and the
//! CPU-temperature scan that the three real frontends (the ratatui TUI,
//! the egui GUI, and the CLI client) render through. Before this crate
//! the same formatters were hand-duplicated in `tui/src/ui.rs`,
//! `gui/src/main.rs`, and `gui/src/telemetry_zone.rs`, and the same
//! `NaReason` rendered two different strings (TUI compact vs GUI
//! verbose) — the structural root of the FE-1/FE-2/FE-6/FE-7 findings.
//!
//! **What it is:** pure presentation logic over the wire payload types
//! (`ramsleuth-telemetry` + `ramsleuth-bench`, both leaves — no cycle).
//! It carries **no** `ratatui` / `egui` / `tokio` dependency: `std`-only,
//! like every other client. A frontend keeps only its
//! terminal-vs-egui **layout** (widget placement, color choice) local; the
//! **text** it renders comes from here, so the same value + the same
//! `NaReason` render identically in every frontend by construction.
//!
//! **What it is not:** it does not recompute a value the daemon provides.
//! The formatters take the wire payload (the `SystemMemoryTelemetry`
//! snapshot, a `BenchmarkGrid`, an `f64` clock) and shape it for display.
//!
//! # No-panic contract
//!
//! Every formatter degrades a non-finite / `Na` input to an honest
//! placeholder (`N/A`, `0:00`) — never a panic (D5). The CPU-temp scan
//! degrades every failure class to `f64::NAN`.

use std::fs;
use std::path::{Path, PathBuf};

use ramsleuth_bench::{BenchmarkGrid, Metric, Tier};
use ramsleuth_telemetry::amd_readout::{ClockReadout, DivMode, EccStatus};
use ramsleuth_telemetry::cpuid::CpuVendor;
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::intel_readout::ChannelTrust;
use ramsleuth_telemetry::spd_decode::SpdModule;
use ramsleuth_telemetry::{SystemMemoryTelemetry, SystemPlatform};

// ---------------------------------------------------------------------------
// Numeric formatters (the capacity / clock display rules, C7-11).
// ---------------------------------------------------------------------------

/// The binary → decimal capacity conversion factor (1 GiB =
/// 1.073741824 GB).
const GIB_TO_GB: f64 = 1.073741824;

/// Trim a display number: a whole number renders without decimals, one
/// decimal otherwise (the TUI/GUI shared `trim` rule).
fn trim(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// One capacity readout (the carried GiB wire value) as display text in
/// the selected capacity unit: the GiB arm keeps the value (`16 GiB`),
/// the GB arm converts × [`GIB_TO_GB`] (`17.2 GB`); a non-finite value
/// degrades to the honest `N/A`.
pub fn format_capacity(gib: f64, capacity_gib: bool) -> String {
    if !gib.is_finite() {
        return "N/A".to_owned();
    }
    if capacity_gib {
        format!("{} GiB", trim(gib))
    } else {
        format!("{} GB", trim(gib * GIB_TO_GB))
    }
}

/// One clock readout (the carried MHz wire value) as display text in the
/// selected clock unit: the MHz arm keeps the value (`3500 MHz`), the GHz
/// arm ÷1000 (`3.5 GHz`); a non-finite value degrades to the honest `N/A`.
pub fn format_clock(mhz: f64, clock_mhz: bool) -> String {
    if !mhz.is_finite() {
        return "N/A".to_owned();
    }
    if clock_mhz {
        format!("{} MHz", trim(mhz))
    } else {
        format!("{} GHz", trim(mhz / 1000.0))
    }
}

/// One run's elapsed in the status line's `m:ss` form (the plan's
/// `0:42`): the minutes un-padded, the seconds two-digit; a non-finite /
/// non-positive reading renders `0:00` (a bad elapsed never panics the
/// line, D5).
pub fn format_elapsed(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    format!("{}:{:02}", total / 60, total % 60)
}

// ---------------------------------------------------------------------------
// The RAM summary-line formatters (the header line 3 segments).
// ---------------------------------------------------------------------------

/// The rank word for the header breakdown: `1` → `Single-Rank`,
/// `2` → `Dual-Rank`, `n > 0` → `<n>-Rank`; a `Na`/`0` rank yields
/// `None` (the word is omitted from the group, never printed as `N/A`).
pub fn rank_word(rank: &Section<u8>) -> Option<String> {
    match rank {
        Section::Value(0) | Section::Na(_) => None,
        Section::Value(1) => Some("Single-Rank".to_owned()),
        Section::Value(2) => Some("Dual-Rank".to_owned()),
        Section::Value(other) => Some(format!("{other}-Rank")),
    }
}

/// The per-DIMM capacity summary: one `<count>x<size>` group per distinct
/// carried `(size, rank word)` (first-seen order, ` + `-joined, the size in
/// the selected capacity unit); the rank word from the parallel `spd`
/// slice (positionally aligned with `sizes`) is appended when present
/// (`2x16 GiB Single-Rank`); a `Na` size contributes nothing; an all-`Na`
/// / empty list degrades to `N/A`.
pub fn dimm_summary(sizes: &[Section<f64>], spd: &[SpdModule], capacity_gib: bool) -> String {
    let mut groups: Vec<(f64, Option<String>, usize)> = Vec::new();
    for (slot, cell) in sizes.iter().enumerate() {
        if let Some(gib) = cell.value() {
            let rank = spd.get(slot).and_then(|module| rank_word(&module.rank));
            match groups
                .iter_mut()
                .find(|(value, word, _)| (value - gib).abs() < 0.05 && *word == rank)
            {
                Some(group) => group.2 += 1,
                None => groups.push((*gib, rank, 1)),
            }
        }
    }
    if groups.is_empty() {
        "N/A".to_owned()
    } else {
        groups
            .iter()
            .map(|(gib, rank, count)| match rank {
                Some(word) => format!("{count}x{} {word}", format_capacity(*gib, capacity_gib)),
                None => format!("{count}x{}", format_capacity(*gib, capacity_gib)),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// The channel mode from the SPD-visible DIMM count: 1 / 2 / 4 →
/// Single- / Dual- / Quad-Channel; any other count (0, odd) degrades to
/// `N/A`. This is the **fallback** label: the caller prefers the
/// hardware channel mode and uses this count-derived label only when no
/// readout carries one.
pub fn channel_mode(dimm_count: usize) -> String {
    match dimm_count {
        1 => "Single-Channel".to_owned(),
        2 => "Dual-Channel".to_owned(),
        4 => "Quad-Channel".to_owned(),
        _ => "N/A".to_owned(),
    }
}

/// The total-vs-breakdown slot note: the OS total can strictly exceed the
/// SPD-visible sum when the host installs more DIMMs than the SPD bus
/// binds. A uniform per-module size whose quotient `total / u` lands
/// within a tenth of a module of an integer `n` above the visible count →
/// `<visible> of <n> slots SPD-visible`; any other excess → `SPD sees
/// <visible> of the installed capacity`. A `Na` / non-finite total, no
/// visible modules, or `total ≤ sum` → `None` (no false alarm).
/// Unit-agnostic: counts, not GiB.
pub fn slot_note(total: &Section<f64>, sizes: &[Section<f64>]) -> Option<String> {
    let total = total.value().copied().filter(|value| value.is_finite())?;
    let visible: Vec<f64> = sizes.iter().filter_map(|cell| cell.value().copied()).collect();
    if visible.is_empty() {
        return None;
    }
    let sum = visible.iter().sum::<f64>();
    if total <= sum {
        return None;
    }
    let uniform = visible.iter().all(|value| (value - visible[0]).abs() < 0.05);
    if uniform {
        let unit = visible[0];
        if unit > 0.0 {
            let quotient = total / unit;
            let slots = quotient.round();
            if (quotient - slots).abs() <= 0.1 && slots > visible.len() as f64 {
                return Some(format!("{} of {slots:.0} slots SPD-visible", visible.len()));
            }
        }
    }
    Some(format!("SPD sees {} of the installed capacity", visible.len()))
}

/// The summary-line speed fallback when no bound SPD module carries a
/// speed: the platform's derived MT/s = MCLK × 2 (the DDR double-pumping
/// rule), from the AMD readout's MCLK, else the max Intel channel MCLK
/// (`None` when no channel carries one). `None` overall when neither
/// branch yields an MCLK (the segment is omitted).
pub fn mclk_derived_speed(t: &SystemMemoryTelemetry) -> Option<String> {
    let mclk = match t.amd.value() {
        Some(readout) => readout.clocks.mclk_mhz.value().copied(),
        None => t
            .intel
            .value()
            .and_then(|readout| {
                readout
                    .channels
                    .iter()
                    .filter_map(|c| c.clocks.mclk_mhz.value().copied())
                    .reduce(|a, b| if b > a { b } else { a })
            }),
    }?;
    Some(format!("{:.0} MT/s", mclk * 2.0))
}

/// The RAM summary line's ECC segment: the platform ECC status (the
/// readouts are mutually exclusive per platform, Intel probed first) as
/// display text — the enum label, the honest `N/A` for the unreadable /
/// absent state.
pub fn ecc_label(t: &SystemMemoryTelemetry) -> String {
    match t
        .intel
        .value()
        .map(|ro| ro.ecc_status)
        .or_else(|| t.amd.value().map(|ro| ro.ecc_status))
        .unwrap_or(EccStatus::Unknown)
    {
        EccStatus::Unknown => "N/A".to_owned(),
        s => s.label().to_owned(),
    }
}

/// The RAM summary line's mode segment over one AMD clock readout: the
/// UCLK:MCLK ratio text — `Synchronous 1:1` (with the MCLK in the
/// selected clock unit when it carries one), `Asynchronous 1:2`, or the
/// honest `N/A`. The caller chooses the semantic color.
pub fn sync_mode_from_clocks(clocks: &ClockReadout, clock_mhz: bool) -> String {
    match clocks.div_mode.value() {
        Some(DivMode::OneToOne) => match clocks.mclk_mhz.value() {
            Some(mhz) => {
                format!("Synchronous 1:1 (UCLK = MCLK = {})", format_clock(*mhz, clock_mhz))
            }
            None => "Synchronous 1:1".to_owned(),
        },
        Some(DivMode::OneToTwo) => "Asynchronous 1:2".to_owned(),
        None => "N/A".to_owned(),
    }
}

/// The RAM summary line's mode segment over a whole snapshot (the AMD
/// branch): the AMD readout must carry a value whose `div_mode` is usable,
/// else the honest `N/A` — the Intel / driver-missing / degraded states all
/// degrade here (never a panic). The caller chooses the semantic color.
pub fn sync_mode_text(t: &SystemMemoryTelemetry, clock_mhz: bool) -> String {
    match t.amd.value() {
        Some(readout) => sync_mode_from_clocks(&readout.clocks, clock_mhz),
        None => "N/A".to_owned(),
    }
}

/// The RAM summary line's mode segment over an Intel readout (the
/// trust-resolved `channel_mode`): the mode label with the Chunk 2 trust
/// badge — the L3 override / L4 heuristic (unverified) tiers append
/// `(unverified)`, the L2 firmware-verified tier appends `(firmware)`.
/// A non-Intel / no-mode readout yields `None` and the caller falls back
/// to the AMD sync mode (or the honest `N/A`). The caller chooses the
/// semantic color.
pub fn intel_mode_text(t: &SystemMemoryTelemetry) -> Option<String> {
    let ro = t.intel.value()?;
    let mode = ro.channel_mode?;
    let mut label = mode.mode_label().to_owned();
    match ro.channel_trust {
        Some(ChannelTrust::Override) | Some(ChannelTrust::Heuristic) => {
            label.push_str(" (unverified)");
        }
        Some(ChannelTrust::VerifiedFirmware) => {
            label.push_str(" (firmware)");
        }
        _ => {}
    }
    Some(label)
}

/// Line 2's AGESA/SMU provenance fragment: the header labels the value by
/// where it came from — a true AGESA token (the DMI BIOS string scan)
/// renders `AGESA <v>` and suppresses the SMU value; else a shape-checked
/// `ryzen_smu` firmware version renders under its own `SMU` label (never
/// the reported `AGESA <smu value>` mislabel); else the honest `AGESA N/A`.
/// One value, one true label — no hybrid, never fabricated.
pub fn age_fragment(platform: &SystemPlatform) -> String {
    match (&platform.agesa, &platform.smu_version) {
        (Section::Value(v), _) => format!("AGESA {v}"),
        (Section::Na(_), Section::Value(v)) => format!("SMU {v}"),
        (Section::Na(_), Section::Na(_)) => "AGESA N/A".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// The cell + N/A text maps (the C3 single source for the N/A rendering).
// ---------------------------------------------------------------------------

/// One [`Section`] cell as display text: the contained value (its
/// `Display` form), or the bare `N/A` (the header's honest-degradation
/// rule — a missing cell renders `N/A`, never a panic).
pub fn cell_text<T: std::fmt::Display>(cell: &Section<T>) -> String {
    match cell {
        Section::Value(v) => v.to_string(),
        Section::Na(_) => "N/A".to_owned(),
    }
}

/// The human text of one **per-cell** absent [`NaReason`] (the compact
/// form): the terse, user-friendly reason tag — `N/A (unsupported CPU)`,
/// `N/A (driver missing)`, `N/A (insufficient privilege)`,
/// `N/A (new PM table)`, `N/A (decode failed)`; a structurally
/// not-applicable cell ([`NaReason::NotApplicable`]) keeps the one bare
/// `N/A` (explaining every structurally-absent field would be noise in a
/// dense grid). Built from the [`NaReason`] `Display` phrase
/// (DAEMON-2), so a variant rename is a test failure, not a silent
/// `Debug` drift.
pub fn na_cell_text(reason: &NaReason) -> String {
    match reason {
        NaReason::NotApplicable => "N/A".to_owned(),
        r => format!("N/A ({r})"),
    }
}

/// The human text of a **whole-branch** degraded readout (the verbose,
/// self-explanatory form — the single gray note under a vendor header when
/// the branch is `Na`): names the reason and states the daemon/driver
/// state when that is the reassuring fact. The per-cell form is
/// [`na_cell_text`]; this is the section-level form. Both frontends source
/// their section-level N/A from here, so the same [`NaReason`] renders the
/// same meaning everywhere (FE-6).
pub fn na_section_text(reason: &NaReason) -> String {
    match reason {
        NaReason::UnsupportedHardware => {
            "No data — this CPU doesn't expose the memory-controller registers RamSleuth reads. Expected on this part, not an error."
                .to_owned()
        }
        NaReason::UnknownPmTableVersion => {
            "No data — this CPU's memory-firmware version isn't in the supported set yet. The driver is loaded and working."
                .to_owned()
        }
        NaReason::ParseError(_) => {
            "No data — the register values couldn't be decoded on this platform.".to_owned()
        }
        NaReason::DriverMissing => {
            "No data — the required kernel driver isn't loaded. The daemon is up; use the Setup prompt to install it."
                .to_owned()
        }
        NaReason::InsufficientPrivilege => {
            "No data — this read needs more privilege than the daemon has.".to_owned()
        }
        NaReason::NotApplicable => {
            "No data — this section isn't available on this platform.".to_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// The benchmark grid cell formatter (the zone-2 4×4 table).
// ---------------------------------------------------------------------------

/// One benchmark grid cell's display text: the formatted value (two-decimal
/// GB/s for a bandwidth metric, ns for latency) or `N/A` for an unmeasured
/// cell (0.0 on the wire, or a non-finite reading). The single source the
/// TUI and GUI grid renderers share.
pub fn bench_cell_text(grid: &BenchmarkGrid, tier: Tier, metric: Metric) -> String {
    let value = grid.cell(tier, metric);
    if !value.is_finite() || value <= 0.0 {
        return "N/A".to_owned();
    }
    match metric {
        Metric::Latency => format!("{value:.2} ns"),
        _ => format!("{value:.2} GB/s"),
    }
}

// ---------------------------------------------------------------------------
// The CPU identity + the CPU-temperature scan.
// ---------------------------------------------------------------------------

/// The stable display name of a detected CPU (DAEMON-2): the vendor's
/// generation via its [`std::fmt::Display`] impl (the pinned stable name —
/// `Alder Lake`, `Zen 5`, …), `Unknown` for an unrecognized vendor. The
/// single source the daemon `cpu_gen` wire string, the TUI/GUI header, and
/// the CLI use.
pub fn cpu_gen_text(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Intel(gen) => gen.to_string(),
        CpuVendor::Amd(zen) => zen.to_string(),
        CpuVendor::Unknown => "Unknown".to_owned(),
    }
}

/// The CPU temperature in °C (FE-7, the shared scan): first the hwmon
/// sensor ([`read_hwmon_temp_c`] — the Intel `coretemp` or the AMD
/// `k10temp` / `zenpower` `temp1_input`), then the thermal-zone scan
/// (the Intel `x86_pkg_temp` or the AMD `cpu_thermal` — the pre-C9-04
/// fallback). The first finite reading wins; both sources are matched by
/// sensor name only, never by `hwmonN` / `thermal_zoneN` slot.
///
/// Every failure class degrades to `f64::NAN`: no hwmon class at all, no
/// matching sensor name, no matching CPU thermal zone, an unreadable
/// `name` / `type`, or an unreadable / non-numeric reading (the ENODATA
/// sensor case). Nothing panics — std `fs` reads only. Runs on the poller
/// thread (never the render thread).
pub fn read_cpu_temp_c() -> f64 {
    let hwmon = read_hwmon_temp_c();
    if hwmon.is_finite() {
        return hwmon; // the hwmon source (coretemp/k10temp/zenpower) wins.
    }
    // The fallback: the thermal-zone scan (the Intel `x86_pkg_temp` or the
    // AMD `cpu_thermal`), matched by the zone's `type` name only — never by
    // the `thermal_zoneN` slot (its number is unstable across boards; a wifi
    // / GPU zone can occupy a low slot).
    let Ok(zones) = fs::read_dir("/sys/class/thermal") else {
        return f64::NAN; // no thermal class at all.
    };
    let mut types: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in zones.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue; // a non-UTF-8 zone name: not a zone we read.
        };
        if !name.starts_with("thermal_zone") {
            continue; // not a thermal zone dir.
        }
        let dir = entry.path();
        let Ok(zone_type) = fs::read_to_string(dir.join("type")) else {
            continue; // the zone's type is unreadable: not it.
        };
        types.push(zone_type.trim().to_string());
        dirs.push(dir);
    }
    let type_refs: Vec<&str> = types.iter().map(String::as_str).collect();
    let Some(selected) = select_thermal_zone(&type_refs) else {
        return f64::NAN; // no `x86_pkg_temp` / `cpu_thermal` zone.
    };
    let Some(dir) = types
        .iter()
        .position(|t| t == selected)
        .and_then(|i| dirs.get(i))
    else {
        // `selected` came from `types`: not expected, still degrades.
        return f64::NAN;
    };
    read_zone_temp_c(dir)
}

/// The hwmon CPU-temperature scan: walk `/sys/class/hwmon/`, read each
/// `hwmon*` device's `name` (the kernel sensor name, trimmed +
/// case-insensitive), select the preferred one by name only
/// ([`select_hwmon_dir`] — `coretemp` first, else `k10temp`, else
/// `zenpower`, never a fixed `hwmonN` index — the index is unstable
/// across boots/CPUs), and return that device's `temp1_input`
/// (millidegrees ÷ 1000) in °C.
///
/// Every failure class degrades to `f64::NAN`. Nothing panics — std `fs`
/// reads only.
fn read_hwmon_temp_c() -> f64 {
    let Ok(devices) = fs::read_dir("/sys/class/hwmon") else {
        return f64::NAN; // no hwmon class at all.
    };
    let mut names: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in devices.flatten() {
        let Ok(raw_name) = entry.file_name().into_string() else {
            continue; // a non-UTF-8 device name: skip.
        };
        if !raw_name.starts_with("hwmon") {
            continue; // not an hwmon device dir.
        }
        let dir = entry.path();
        let Ok(name) = fs::read_to_string(dir.join("name")) else {
            continue; // the device's `name` is unreadable: not it.
        };
        names.push(name.trim().to_string());
        dirs.push(dir);
    }
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let Some(selected) = select_hwmon_dir(&name_refs) else {
        return f64::NAN; // no CPU-temp hwmon device.
    };
    let Some(dir) = names.iter().position(|name| name == selected).and_then(|i| dirs.get(i))
    else {
        // `selected` came from `names`: not expected, still degrades.
        return f64::NAN;
    };
    read_hwmon_temp1_c(dir)
}

/// The known CPU-temperature hwmon sensor names, in priority order: the
/// Intel `coretemp` (per-core + package — the most accurate) first, then
/// the AMD `k10temp` (classic) and `zenpower` (newer). On a real host
/// only one vendor's sensors exist, so the order is a tiebreaker, not a
/// cross-vendor preference.
const CPU_TEMP_HWMON_NAMES: &[&str] = &["coretemp", "k10temp", "zenpower"];

/// The known CPU-temperature thermal-zone types, in priority order: the
/// Intel `x86_pkg_temp` (package) first, then the AMD `cpu_thermal`.
/// Matched by the zone's `type` name only — never by the `thermal_zoneN`
/// slot (that number is unstable across boards; a wifi / GPU zone can
/// occupy a low slot).
const CPU_TEMP_TZ_NAMES: &[&str] = &["x86_pkg_temp", "cpu_thermal"];

/// The hwmon name selection (pure, I/O-free): walk
/// [`CPU_TEMP_HWMON_NAMES`] in priority order — `coretemp` (the Intel
/// per-core + package sensor) first, then `k10temp` (the AMD classic),
/// then `zenpower` (the newer AMD) — and return the first known name
/// present, else no match. Case-insensitive on the trimmed name; by name
/// only — a fixed `hwmonN` index is never matched (it is unstable across
/// boots/CPUs).
fn select_hwmon_dir<'a>(names: &'a [&'a str]) -> Option<&'a str> {
    for wanted in CPU_TEMP_HWMON_NAMES {
        for name in names {
            let trimmed = name.trim();
            if trimmed.eq_ignore_ascii_case(wanted) {
                return Some(trimmed); // the highest-priority present sensor wins.
            }
        }
    }
    None
}

/// The thermal-zone type selection (pure, I/O-free): walk
/// [`CPU_TEMP_TZ_NAMES`] in priority order — `x86_pkg_temp` (the Intel
/// package sensor) first, then `cpu_thermal` (the AMD zone) — and return
/// the first known type present, else no match. Case-insensitive on the
/// trimmed type; by type name only — a fixed `thermal_zoneN` slot is
/// never matched (it is unstable across boards; a wifi / GPU zone can
/// occupy a low slot).
fn select_thermal_zone<'a>(types: &'a [&'a str]) -> Option<&'a str> {
    for wanted in CPU_TEMP_TZ_NAMES {
        for t in types {
            let trimmed = t.trim();
            if trimmed.eq_ignore_ascii_case(wanted) {
                return Some(trimmed); // the highest-priority present zone wins.
            }
        }
    }
    None
}

/// One hwmon device's `temp1_input` (millidegrees) in °C; every failure
/// class (missing / unreadable / non-numeric / non-finite) degrades to NaN.
fn read_hwmon_temp1_c(dir: &Path) -> f64 {
    let Ok(raw) = fs::read_to_string(dir.join("temp1_input")) else {
        return f64::NAN; // missing (ENODATA sensor) or unreadable.
    };
    parse_millidegrees(&raw).unwrap_or(f64::NAN)
}

/// One thermal zone's `temp` (millidegrees) in °C; every failure class
/// (missing / unreadable / non-numeric / non-finite) degrades to NaN.
fn read_zone_temp_c(zone_dir: &Path) -> f64 {
    let Ok(raw) = fs::read_to_string(zone_dir.join("temp")) else {
        return f64::NAN; // missing (ENODATA sensor) or unreadable.
    };
    parse_millidegrees(&raw).unwrap_or(f64::NAN)
}

/// One millidegree reading (the kernel's `temp` / `temp1_input` encoding)
/// in °C; a non-numeric or non-finite reading yields `None` (the no-panic
/// contract).
fn parse_millidegrees(raw: &str) -> Option<f64> {
    match raw.trim().parse::<f64>() {
        Ok(millidegrees) if millidegrees.is_finite() => Some(millidegrees / 1000.0),
        _ => None, // a non-numeric reading: not data.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ramsleuth_telemetry::cpuid::{AmdZen, IntelGen};

    // --- formatter pins (TC-FE2): a canonical input → a pinned output, so
    // any future divergence is a test failure.

    fn section(v: f64) -> Section<f64> {
        Section::Value(v)
    }

    #[test]
    fn format_capacity_pins_gib_and_gb_arms() {
        assert_eq!(format_capacity(16.0, true), "16 GiB");
        assert_eq!(format_capacity(16.0, false), "17.2 GB");
        assert_eq!(format_capacity(32.5, true), "32.5 GiB");
        assert_eq!(format_capacity(f64::NAN, true), "N/A");
        assert_eq!(format_capacity(f64::INFINITY, false), "N/A");
    }

    #[test]
    fn format_clock_pins_mhz_and_ghz_arms() {
        assert_eq!(format_clock(3500.0, true), "3500 MHz");
        assert_eq!(format_clock(3500.0, false), "3.5 GHz");
        assert_eq!(format_clock(1234.5, false), "1.2 GHz");
        assert_eq!(format_clock(f64::NAN, true), "N/A");
    }

    #[test]
    fn format_elapsed_pins_the_mss_form() {
        assert_eq!(format_elapsed(0.0), "0:00");
        assert_eq!(format_elapsed(59.9), "0:59");
        assert_eq!(format_elapsed(60.0), "1:00");
        assert_eq!(format_elapsed(125.0), "2:05");
        assert_eq!(format_elapsed(3661.0), "61:01");
        assert_eq!(format_elapsed(f64::NAN), "0:00");
        assert_eq!(format_elapsed(-4.0), "0:00");
        assert_eq!(format_elapsed(f64::INFINITY), "0:00");
    }

    #[test]
    fn channel_mode_pins_the_count_arms() {
        assert_eq!(channel_mode(1), "Single-Channel");
        assert_eq!(channel_mode(2), "Dual-Channel");
        assert_eq!(channel_mode(4), "Quad-Channel");
        assert_eq!(channel_mode(0), "N/A");
        assert_eq!(channel_mode(3), "N/A");
    }

    fn spd(rank: u8) -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::Value("m".to_owned()),
            die_maker: Section::Value("d".to_owned()),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::Value(8),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(rank),
            density_mbit: Section::Value(16_384),
            speed_mts: Section::Value(3_200),
            profiles: Vec::new(),
        }
    }

    #[test]
    fn dimm_summary_pins_the_group_form() {
        let sizes = vec![section(16.0), section(16.0)];
        let modules = vec![spd(1), spd(1)];
        assert_eq!(
            dimm_summary(&sizes, &modules, true),
            "2x16 GiB Single-Rank"
        );
        // A Na size contributes nothing; an all-Na list degrades to N/A.
        let na_sizes = vec![Section::na(NaReason::NotApplicable)];
        assert_eq!(dimm_summary(&na_sizes, &[], true), "N/A");
    }

    #[test]
    fn slot_note_pins_the_excess_form() {
        // total == sum → no note (no excess to explain).
        assert_eq!(slot_note(&section(32.0), &[section(16.0), section(16.0)]), None);
        // total ≤ sum → no note (no false alarm).
        assert_eq!(slot_note(&section(16.0), &[section(16.0), section(16.0)]), None);
        // Non-uniform sizes that exceed the sum → the generic note.
        assert_eq!(
            slot_note(&section(48.0), &[section(16.0), section(8.0)]),
            Some("SPD sees 2 of the installed capacity".to_owned())
        );
        // Uniform size whose quotient lands on an integer above the
        // visible count → the "of N slots" note.
        assert_eq!(
            slot_note(&section(64.0), &[section(16.0), section(16.0)]),
            Some("2 of 4 slots SPD-visible".to_owned())
        );
    }

    #[test]
    fn rank_word_pins_the_arms() {
        assert_eq!(rank_word(&Section::Value(1)), Some("Single-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(2)), Some("Dual-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(3)), Some("3-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(0)), None);
        assert_eq!(rank_word(&Section::na(NaReason::NotApplicable)), None);
    }

    #[test]
    fn age_fragment_pins_the_provenance_arms() {
        let platform = |agesa: Section<String>, smu: Section<String>| SystemPlatform {
            cpu_clock_mhz: Section::Value(3500.0),
            motherboard: Section::Value("mb".to_owned()),
            bios: Section::Value("1.0".to_owned()),
            agesa,
            smu_version: smu,
        };
        assert_eq!(
            age_fragment(&platform(Section::Value("PI 1.2.0.12".to_owned()), Section::na(NaReason::NotApplicable))),
            "AGESA PI 1.2.0.12"
        );
        assert_eq!(
            age_fragment(&platform(Section::na(NaReason::NotApplicable), Section::Value("1.0.0".to_owned()))),
            "SMU 1.0.0"
        );
        assert_eq!(
            age_fragment(&platform(Section::na(NaReason::NotApplicable), Section::na(NaReason::NotApplicable))),
            "AGESA N/A"
        );
    }

    // --- N/A maps (TC-FE6): the compact and verbose forms for every reason.

    #[test]
    fn na_cell_text_pins_the_compact_form() {
        assert_eq!(na_cell_text(&NaReason::NotApplicable), "N/A");
        assert_eq!(na_cell_text(&NaReason::DriverMissing), "N/A (driver missing)");
        assert_eq!(
            na_cell_text(&NaReason::UnsupportedHardware),
            "N/A (unsupported CPU)"
        );
        assert_eq!(
            na_cell_text(&NaReason::InsufficientPrivilege),
            "N/A (insufficient privilege)"
        );
        assert_eq!(
            na_cell_text(&NaReason::UnknownPmTableVersion),
            "N/A (new PM table)"
        );
        assert_eq!(na_cell_text(&NaReason::ParseError("x".to_owned())), "N/A (decode failed)");
    }

    #[test]
    fn na_section_text_pins_the_verbose_form() {
        for reason in [
            NaReason::UnsupportedHardware,
            NaReason::DriverMissing,
            NaReason::InsufficientPrivilege,
            NaReason::UnknownPmTableVersion,
            NaReason::NotApplicable,
            NaReason::ParseError("x".to_owned()),
        ] {
            let text = na_section_text(&reason);
            assert!(text.starts_with("No data —"), "{reason:?}: {text}");
            assert!(text.contains("not an error") || text.contains("daemon") || text.contains("platform") || text.contains("driver") || text.contains("privilege") || text.contains("decoded"), "{reason:?}: {text}");
        }
        // The section form is the verbose one (distinct from the compact).
        assert_ne!(na_section_text(&NaReason::DriverMissing), na_cell_text(&NaReason::DriverMissing));
    }

    #[test]
    fn cell_text_pins_value_and_na() {
        assert_eq!(cell_text(&Section::Value("hi".to_owned())), "hi");
        assert_eq!(
            cell_text(&Section::<String>::na(NaReason::NotApplicable)),
            "N/A"
        );
    }

    // --- cpu_gen_text (TC-DAEMON2): the stable name, not the Debug form.

    #[test]
    fn cpu_gen_text_pins_the_stable_names() {
        assert_eq!(
            cpu_gen_text(&CpuVendor::Intel(IntelGen::AlderLake)),
            "Alder Lake"
        );
        assert_eq!(cpu_gen_text(&CpuVendor::Amd(AmdZen::Zen5)), "Zen 5");
        assert_eq!(cpu_gen_text(&CpuVendor::Unknown), "Unknown");
    }

    // --- bench_cell_text (TC-FE2): the grid cell form.

    #[test]
    fn bench_cell_text_pins_value_and_na() {
        let grid = BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 0.0],
            write_gbps: [43.63, 38.20, 0.0, 14.02],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        };
        assert_eq!(bench_cell_text(&grid, Tier::Memory, Metric::Read), "26.35 GB/s");
        assert_eq!(bench_cell_text(&grid, Tier::L1, Metric::Latency), "1.12 ns");
        assert_eq!(bench_cell_text(&grid, Tier::L2, Metric::Write), "N/A");
    }

    // --- read_cpu_temp_c (TC-FE7): the pure selection helpers.

    #[test]
    fn select_hwmon_dir_is_vendor_agnostic() {
        // Intel `coretemp` outranks the AMD sensors when both are present.
        assert_eq!(
            select_hwmon_dir(&["k10temp", "coretemp", "asus"]),
            Some("coretemp")
        );
        // AMD `k10temp` is preferred over `zenpower` (the pre-existing order).
        assert_eq!(
            select_hwmon_dir(&["zenpower", "k10temp", "asus"]),
            Some("k10temp")
        );
        // `zenpower` alone.
        assert_eq!(select_hwmon_dir(&["asus", "zenpower"]), Some("zenpower"));
        // Case-insensitive + trimmed match on the name.
        assert_eq!(select_hwmon_dir(&["  CoreTemp  "]), Some("CoreTemp"));
        // No CPU sensor (only non-CPU hwmon devices) → None.
        assert_eq!(select_hwmon_dir(&["asus", "iwlwifi", "nct6798"]), None);
        assert_eq!(select_hwmon_dir(&[]), None);
    }

    #[test]
    fn select_thermal_zone_is_vendor_agnostic() {
        // Intel `x86_pkg_temp` outranks the AMD zone when both are present.
        assert_eq!(
            select_thermal_zone(&["cpu_thermal", "x86_pkg_temp", "acpitz"]),
            Some("x86_pkg_temp")
        );
        // AMD `cpu_thermal` alone.
        assert_eq!(
            select_thermal_zone(&["acpitz", "cpu_thermal"]),
            Some("cpu_thermal")
        );
        // Case-insensitive + trimmed match on the type.
        assert_eq!(
            select_thermal_zone(&["  X86_PKG_TEMP  "]),
            Some("X86_PKG_TEMP")
        );
        // No CPU zone (only non-CPU zones — the wifi / acpi cases) → None.
        assert_eq!(select_thermal_zone(&["iwlwifi_1", "acpitz", "acpu"]), None);
        assert_eq!(select_thermal_zone(&[]), None);
    }

    #[test]
    fn cpu_temp_sensor_lists_pin_the_priority_order() {
        // The exact priority lists the scan matches against (name, not slot).
        assert_eq!(CPU_TEMP_HWMON_NAMES, &["coretemp", "k10temp", "zenpower"]);
        assert_eq!(CPU_TEMP_TZ_NAMES, &["x86_pkg_temp", "cpu_thermal"]);
    }

    // --- TC-FE6 (the N/A symmetry confirming test): every [`NaReason`]
    // has BOTH a compact per-cell form and a verbose section form, the
    // two frontends source both from here, so the same reason reads the
    // same everywhere. `NotApplicable` is the one reason whose cell form
    // is the bare `N/A` (the noise-free form), distinct from its section
    // form.
    #[test]
    fn tc_fe6_na_forms_are_symmetric_for_all_reasons() {
        let reasons = [
            NaReason::UnsupportedHardware,
            NaReason::DriverMissing,
            NaReason::InsufficientPrivilege,
            NaReason::UnknownPmTableVersion,
            NaReason::NotApplicable,
            NaReason::ParseError("x".to_owned()),
        ];
        for reason in &reasons {
            // Every reason has a non-empty cell + section form.
            assert!(!na_cell_text(reason).is_empty(), "{reason:?} cell");
            assert!(!na_section_text(reason).is_empty(), "{reason:?} section");
            // The section form is the verbose, self-explanatory one.
            assert!(
                na_section_text(reason).starts_with("No data —"),
                "{reason:?} section must be verbose: {}",
                na_section_text(reason)
            );
        }
        // `NotApplicable` is the one bare-`N/A` cell (distinct from its
        // verbose section form); every other cell carries the reason tag.
        assert_eq!(na_cell_text(&NaReason::NotApplicable), "N/A");
        for reason in &reasons {
            if *reason != NaReason::NotApplicable {
                assert!(
                    na_cell_text(reason).starts_with("N/A ("),
                    "a non-NotApplicable cell names its reason: {reason:?}"
                );
            }
        }
    }

    // --- TC-FE7 (the CPU-temp scan no-panic confirming test): the shared
    // `read_cpu_temp_c` degrades every failure class to `f64::NAN` (no
    // hwmon class, no matching sensor, no `cpu_thermal` zone, an
    // unreadable / non-numeric reading) — it never panics, so both the
    // TUI and the GUI poller thread can call it safely.
    #[test]
    fn tc_fe7_read_cpu_temp_c_never_panics() {
        let temp = read_cpu_temp_c();
        assert!(
            temp.is_nan() || (temp > -50.0 && temp < 150.0),
            "a finite reading must be a sane temperature, got {temp}"
        );
    }

    // --- TC-FE2 (the shared-formatter parity confirming test): one
    // representative `SystemMemoryTelemetry` rendered through the shared
    // formatters the TUI *and* the GUI both call — the composed output is
    // pinned, so the two frontends can never render the same snapshot
    // differently (the byte-identical guarantee is by construction: both
    // route through this one implementation).
    #[test]
    fn tc_fe2_one_snapshot_rendered_through_the_shared_formatters() {
        use ramsleuth_telemetry::amd_readout::{
            CadBus, ClockReadout, CommandRate, EccStatus, MemoryChannelMode, RttValue, TimingSet,
            VoltageSet,
        };
        use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};

        let na = || NaReason::NotApplicable;
        let t = SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen4),
                brand: "Ryzen 9 7950X".to_owned(),
            },
            amd: Section::Value(ramsleuth_telemetry::amd_readout::AmdReadout {
                clocks: ClockReadout {
                    mclk_mhz: Section::Value(2100.0),
                    uclk_mhz: Section::Value(2100.0),
                    fclk_mhz: Section::Value(2100.0),
                    div_mode: Section::Value(DivMode::OneToOne),
                    gear_mode: Section::na(na()),
                    gdm: Section::Value(false),
                    pdm: Section::Value(false),
                    command_rate: Section::Value(CommandRate::OneT),
                },
                timings: TimingSet {
                    cl: Section::Value(14),
                    rcwdwr: Section::Value(14),
                    rcdrd: Section::Value(14),
                    rp: Section::Value(14),
                    ras: Section::Value(32),
                    rc: Section::Value(46),
                    rrds: Section::Value(4),
                    rrld: Section::Value(4),
                    faw: Section::Value(14),
                    wtrs: Section::Value(4),
                    wtrl: Section::Value(4),
                    wr: Section::Value(14),
                    rfc1: Section::Value(56),
                    rfc2: Section::Value(76),
                    rfcsb: Section::Value(34),
                    cwl: Section::Value(14),
                    rtp: Section::Value(14),
                    rdwr: Section::Value(4),
                    wrrd: Section::Value(4),
                    rdrd_sd: Section::Value(4),
                    rdrd_dd: Section::Value(4),
                    rdrd_scl: Section::Value(4),
                    rdrd_sc: Section::Value(4),
                    wrwr_sd: Section::Value(4),
                    wrwr_dd: Section::Value(4),
                    wrwr_scl: Section::Value(4),
                    wrwr_sc: Section::Value(4),
                },
                cad_bus: CadBus {
                    proc_odt: Section::Value(48.0),
                    rtt_nom: Section::Value(RttValue::Rzq(8)),
                    rtt_wr: Section::Value(RttValue::Ohms(40.0)),
                    rtt_park: Section::na(na()),
                    clk_drv: Section::Value(48.0),
                    addr_cmd_drv: Section::Value(48.0),
                    cs_odt_drv: Section::Value(48.0),
                    cke_drv: Section::Value(48.0),
                },
                voltages: VoltageSet {
                    vddcr_soc_mv: Section::Value(1150),
                    vddio_mem_mv: Section::Value(1350),
                    vdd_misc_mv: Section::Value(1100),
                    vpp_mv: Section::Value(1800),
                    vcore_mv: Section::na(na()),
                },
                channel_mode: MemoryChannelMode::DualSymmetric,
                ecc_status: EccStatus::CapableButDisabled,
            }),
            intel: Section::na(na()),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(5700.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::Value("PI 1.2.0.12".to_owned()),
                smu_version: Section::na(na()),
            },
            total_capacity: Section::Value(64.0),
            dimm_sizes: vec![Section::Value(32.0), Section::Value(32.0)],
            vcore: Section::na(na()),
        };

        // The composed header line 3 (the same string the TUI and the GUI
        // render through these shared formatters).
        let summary = dimm_summary(&t.dimm_sizes, &t.spd, true);
        let ecc = ecc_label(&t);
        let mode = sync_mode_text(&t, true);
        let speed = mclk_derived_speed(&t);
        let age = age_fragment(&t.platform);
        assert_eq!(summary, "2x32 GiB");
        assert_eq!(ecc, "ECC: Capable (disabled)");
        assert_eq!(mode, "Synchronous 1:1 (UCLK = MCLK = 2100 MHz)");
        assert_eq!(speed, Some("4200 MT/s".to_owned()));
        assert_eq!(age, "AGESA PI 1.2.0.12");
        // The capacity + clock formatters pin the unit arms.
        assert_eq!(format_capacity(64.0, true), "64 GiB");
        assert_eq!(format_clock(5700.0, false), "5.7 GHz");
    }
}
