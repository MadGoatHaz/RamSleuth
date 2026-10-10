//! C12-03 — DMI-keyed board VRM profiles + the NCT6798 hwmon binder.
//!
//! Board-specific Super I/O rails, driven by a **static, data-driven
//! profile registry** keyed on the DMI board name (plan §4 D-4): the
//! Crosshair VIII Hero's `nct6798` exposes in0–in14 *without labels*,
//! so only a DMI-keyed index table is an honest mapping (the
//! LibreHardwareMonitor table for this board, 1:1):
//!
//! | channel | rail                    |
//! | ------- | ----------------------- |
//! | in13    | VDDIO_MEM (DRAM, ~1.2 V)|
//! | in0     | Vcore (cross-check)     |
//! | in6     | SoC (cross-check)       |
//!
//! **Units (plan §1 D-2):** the Linux `nct6775`/`nct6798` hwmon driver
//! already applies the board's resistor divider, so every
//! `in{N}_input` is read as **millivolts, as-is** — LHM's "8 mV/count"
//! is its *raw-ADC* scaling and must never be re-applied (an 8×
//! re-scaling of ~1.2 V would read ~9.6 V and flag at the live check).
//!
//! **Graceful degradation (plan §9):** unknown board (no case-insensitive
//! `"CROSSHAIR VIII HERO"` contains-match) → no profile → all rails
//! [`Section::Na`](crate::error::Section::Na) `NotApplicable`; known board
//! but no `nct6798` hwmon device → the same all-Na; an unreadable /
//! non-numeric / out-of-band channel degrades **that rail only**
//! (per-rail containment) to `Na(ParseError(detail))` — the VDDIO_MEM
//! tripwire band is 100–2000 mV (> 2 V is implausible for DDR4 VDDIO).
//! Never a panic; no `unwrap`/`expect` on hardware-derived data;
//! `std::fs` only (no new dependencies).
//!
//! **Wire:** the readout is consumed in-crate by the facade. Its
//! `vddio_mem_mv` / `vcore_mv` / `soc_mv` cross-check fields ride the
//! readout for the AMD fill-when-Na overlay + the live checklist only
//! (D-5, no wire); the new `vcore: Option<VoltageTelemetry>` field (Chunk
//! 3) IS wire — the facade promotes it to the snapshot root's trailing
//! `SystemMemoryTelemetry.vcore` (the single vCore source of truth for
//! TUI/GUI/CLI: Intel = the board `in0` direct VRM sense; AMD = the SMU
//! PM-table vCore). The `VoltageTelemetry` / `RailConfidence` types are
//! append-only wire-safe (a NEW struct + a trailing field — never
//! inserted / reordered).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NaReason, Section};

/// Telemetry source verification status (Chunk 3, research
/// "Board-Profile Architecture"): how a resolved vCore reading was
/// established.
///
/// **Wire-safe:** a NEW enum that rides `SystemMemoryTelemetry.vcore`
/// (a trailing field on the snapshot root). Its bincode discriminants
/// are `VerifiedProfile = 0`, `HeuristicDefault = 1`, `Unknown = 2` —
/// appended after any existing data, never reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RailConfidence {
    /// Fully verified against a board-specific DMI profile (the curated
    /// seed: the `in0 → Vcore` mapping is known-correct for this board).
    VerifiedProfile,
    /// Inferred via the vendor Super-I/O routing conventions + the
    /// voltage envelope check (no exact DMI match — the vendor-aware
    /// fallback).
    HeuristicDefault,
    /// Unmapped, disconnected, or out-of-bounds rail (the N/A path).
    Unknown,
}

impl RailConfidence {
    /// The canonical vCore confidence badge — the single source every
    /// frontend (TUI / GUI / CLI) renders, so the badge is presented the
    /// same way everywhere: `[est]` for the heuristic vendor-fallback
    /// reading (the board mapping is estimated, not a verified profile),
    /// `None` for a verified profile (no badge) and the `Unknown` N/A
    /// state. (release-polish #4: unifies the per-frontend spellings
    /// `[est]` / `[Unverified]` / `[UNVERIFIED]`.)
    pub fn badge(&self) -> Option<&'static str> {
        match self {
            RailConfidence::HeuristicDefault => Some("[est]"),
            RailConfidence::VerifiedProfile | RailConfidence::Unknown => None,
        }
    }
}

/// One resolved voltage reading (Chunk 3, research "Voltage reading
/// payload"): the board-delivered value, the optional VID reference, the
/// signed load-line delta, and the source-verification confidence.
///
/// **Wire-safe (append-only):** this is a NEW struct that rides
/// `SystemMemoryTelemetry.vcore` (a trailing field on the snapshot root).
/// Its field order is the wire contract. `delivered_volts` is the
/// authoritative primary (the `in0` mV / 1000 — read as-is, no software
/// multiplier); `requested_vid` is `Option` (a no-VID Intel reading
/// carries `None` — omitted, never an error, since there is no public
/// Intel VID MSR); `delta_volts` is the signed `delivered - vid` (a
/// negative value is the normal Vdroop + LLC + undervolt gap) and is
/// `None` when no VID is present.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoltageTelemetry {
    /// The rail's display name (e.g. `"CPU Vcore"`).
    pub rail_name: String,
    /// The board-delivered voltage in volts (`in0` mV / 1000 — read
    /// as-is, no software multiplier; the nct6775/6798 driver already
    /// applies the board's divider).
    pub delivered_volts: f64,
    /// The processor's logical VID target in volts, when safely readable
    /// (`None` on Intel — there is no public Intel VID MSR).
    pub requested_vid: Option<f64>,
    /// The signed load-line delta in volts (`delivered - vid`; a negative
    /// value is the normal Vdroop / LLC / undervolt gap), `None` when no
    /// VID is present.
    pub delta_volts: Option<f64>,
    /// How the reading was established (the UI badge source).
    pub confidence: RailConfidence,
}

/// The NCT6798 in-channels a board profile owns (plan §3 C12-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rail {
    /// VDDIO_MEM — the DRAM I/O rail (~1.2 V on DDR4; Hero: in13).
    VddioMem,
    /// Vcore (VDDCR_VDD) — cross-check only; the SMU PM table (0x0A0)
    /// is the trusted source (D-5).
    Vcore,
    /// VDDCR_SOC — cross-check only; the SMU PM table (0x0B0) is the
    /// trusted source (D-5).
    Soc,
}

/// One row of a board's inN → rail mapping table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RailMap {
    /// The hwmon channel position: the device's `in{index}_input`.
    pub index: u8,
    /// The rail this channel carries on this board.
    pub rail: Rail,
}

/// A static, data-driven board profile (D-4): the DMI board key
/// (matched case-insensitive contains) + the NCT6798 inN → rail mapping.
///
/// Adding a future board = one new `BoardVrmProfile` const + one line in
/// [`PROFILES`] — no per-board code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardVrmProfile {
    /// The DMI `board_name` substring to match (case-insensitive
    /// contains, not equals: "ROG CROSSHAIR VIII HERO" and
    /// "CROSSHAIR VIII HERO WIFI" both contain the key).
    pub board_key: &'static str,
    /// The NCT6798 inN → rail mapping for this board.
    pub rails: &'static [RailMap],
}

/// The Crosshair VIII Hero profile (the LibreHardwareMonitor table, 1:1):
/// in13 → VDDIO_MEM, in0 → Vcore, in6 → SoC. The hwmon `inN_input` values
/// are driver-scaled mV (D-2) — used as-is, never re-scaled.
pub const CROSSHAIR_VIII_HERO: BoardVrmProfile = BoardVrmProfile {
    board_key: "CROSSHAIR VIII HERO",
    rails: &[
        RailMap {
            index: 13,
            rail: Rail::VddioMem,
        },
        RailMap {
            index: 0,
            rail: Rail::Vcore,
        },
        RailMap {
            index: 6,
            rail: Rail::Soc,
        },
    ],
};

/// The NCT6798 `in0 → Vcore` mapping the Intel seed profiles share
/// (Chunk 3, research §"in0 = CPU Vcore (direct VRM sense trace)"):
/// `in0_input` is the unattenuated 1:1 trace from the voltage-regulator
/// feedback loop to the Super-I/O input pin. The `in0_input` value is
/// driver-scaled mV, read as-is (D-2) — no software multiplier.
const INTEL_VCORE_RAILS: &[RailMap] = &[RailMap {
    index: 0,
    rail: Rail::Vcore,
}];

/// --- Intel curated seed profiles (Chunk 3) --------------------------
///
/// A small, growable seed of high-volume enthusiast / workstation boards
/// known to route the CPU vCore to the nct6798 `in0` (research
/// §"Initial Curated Seed Targets"; kept Z690/Z790-only by design). An
/// exact (case-insensitive contains) `board_key` match promotes the
/// `in0 → Vcore` reading to [`RailConfidence::VerifiedProfile`]. Adding a
/// future board = one new `BoardVrmProfile` const + one line in
/// [`PROFILES`] — no per-board code.

pub const ASUS_ROG_MAXIMUS_Z690: BoardVrmProfile = BoardVrmProfile {
    board_key: "ROG MAXIMUS Z690",
    rails: INTEL_VCORE_RAILS,
};
pub const ASUS_ROG_MAXIMUS_Z790: BoardVrmProfile = BoardVrmProfile {
    board_key: "ROG MAXIMUS Z790",
    rails: INTEL_VCORE_RAILS,
};
pub const ASUS_ROG_STRIX_Z690: BoardVrmProfile = BoardVrmProfile {
    board_key: "ROG STRIX Z690",
    rails: INTEL_VCORE_RAILS,
};
pub const ASUS_ROG_STRIX_Z790: BoardVrmProfile = BoardVrmProfile {
    board_key: "ROG STRIX Z790",
    rails: INTEL_VCORE_RAILS,
};
pub const ASUS_TUF_GAMING_Z690: BoardVrmProfile = BoardVrmProfile {
    board_key: "TUF GAMING Z690",
    rails: INTEL_VCORE_RAILS,
};
pub const ASUS_TUF_GAMING_Z790: BoardVrmProfile = BoardVrmProfile {
    board_key: "TUF GAMING Z790",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z690_EXTREME: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z690 EXTREME",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z790_EXTREME: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z790 EXTREME",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z690_STEEL_LEGEND: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z690 STEEL LEGEND",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z790_STEEL_LEGEND: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z790 STEEL LEGEND",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z690_TAICHI: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z690 TAICHI",
    rails: INTEL_VCORE_RAILS,
};
pub const ASROCK_Z690M_ITX_AX: BoardVrmProfile = BoardVrmProfile {
    board_key: "Z690M-ITX/AX",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MEG_Z690_ACE: BoardVrmProfile = BoardVrmProfile {
    board_key: "MEG Z690 ACE",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MEG_Z790_ACE: BoardVrmProfile = BoardVrmProfile {
    board_key: "MEG Z790 ACE",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MPG_Z690_EDGE: BoardVrmProfile = BoardVrmProfile {
    board_key: "MPG Z690 EDGE",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MPG_Z790_EDGE: BoardVrmProfile = BoardVrmProfile {
    board_key: "MPG Z790 EDGE",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MAG_Z690_TOMAHAWK: BoardVrmProfile = BoardVrmProfile {
    board_key: "MAG Z690 TOMAHAWK",
    rails: INTEL_VCORE_RAILS,
};
pub const MSI_MAG_Z790_TOMAHAWK: BoardVrmProfile = BoardVrmProfile {
    board_key: "MAG Z790 TOMAHAWK",
    rails: INTEL_VCORE_RAILS,
};

/// The static board profile registry (D-4): the AMD Crosshair VIII Hero
/// (the C12-03 seed) + the Intel curated seed (Chunk 3).
pub const PROFILES: &[BoardVrmProfile] = &[
    CROSSHAIR_VIII_HERO,
    ASUS_ROG_MAXIMUS_Z690,
    ASUS_ROG_MAXIMUS_Z790,
    ASUS_ROG_STRIX_Z690,
    ASUS_ROG_STRIX_Z790,
    ASUS_TUF_GAMING_Z690,
    ASUS_TUF_GAMING_Z790,
    ASROCK_Z690_EXTREME,
    ASROCK_Z790_EXTREME,
    ASROCK_Z690_STEEL_LEGEND,
    ASROCK_Z790_STEEL_LEGEND,
    ASROCK_Z690_TAICHI,
    ASROCK_Z690M_ITX_AX,
    MSI_MEG_Z690_ACE,
    MSI_MEG_Z790_ACE,
    MSI_MPG_Z690_EDGE,
    MSI_MPG_Z790_EDGE,
    MSI_MAG_Z690_TOMAHAWK,
    MSI_MAG_Z790_TOMAHAWK,
];

/// The binder's per-board readout: the three NCT6798 rails in mV.
///
/// All [`Section::Na`] `NotApplicable` by construction when the board is
/// unknown or the device is absent (see [`BoardVrmReadout::all_na`]);
/// a per-rail read failure degrades that rail only (per-rail containment,
/// §9).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardVrmReadout {
    /// VDDIO_MEM (DRAM), mV — the C12-04 fill-when-Na source.
    pub vddio_mem_mv: Section<u16>,
    /// Vcore cross-check, mV (no wire; live-checklist aid, D-5).
    pub vcore_mv: Section<u16>,
    /// SoC cross-check, mV (no wire; live-checklist aid, D-5).
    pub soc_mv: Section<u16>,
    /// The resolved vCore (Chunk 3): `Some` when a curated profile
    /// matched (`VerifiedProfile`) or the vendor-aware fallback bound
    /// `in0 → Vcore` (`HeuristicDefault`); `None` when the board is
    /// unknown / enterprise / out-of-bounds / no device (the N/A path).
    /// This is the wire-safe payload the facade promotes to
    /// `SystemMemoryTelemetry.vcore`.
    pub vcore: Option<VoltageTelemetry>,
}

impl BoardVrmReadout {
    /// The fully-degraded readout: every rail `Na(NotApplicable)` and no
    /// resolved vCore (unknown board, no `nct6798` device, or
    /// unreadable). Never panics.
    pub fn all_na() -> Self {
        let na = Section::na(NaReason::NotApplicable);
        Self {
            vddio_mem_mv: na.clone(),
            vcore_mv: na.clone(),
            soc_mv: na,
            vcore: None,
        }
    }
}

/// The profile lookup (D-4, pure): a `Value(name)` matches the first
/// profile whose `board_key` is a **case-insensitive substring** of
/// `name` (contains, not equals); a `Na` board name → `None` (unknown
/// board — the graceful all-Na path, §9 item 2).
pub fn profile_for(motherboard: &Section<String>) -> Option<&'static BoardVrmProfile> {
    let name = match motherboard {
        Section::Value(name) => name,
        Section::Na(_) => return None,
    };
    let lowered = name.to_ascii_lowercase();
    PROFILES
        .iter()
        .find(|profile| lowered.contains(profile.board_key.to_ascii_lowercase().as_str()))
}

/// The NCT6798 hwmon walk (the `graph.rs` name-only pattern, std::fs
/// only): scan `/sys/class/hwmon/` and return the first `hwmon*` device
/// whose trimmed `name` equals `nct6798` case-insensitively — never a
/// fixed `hwmonN` index (it is unstable across boots). No hwmon class,
/// no readable name, or no match → `None`.
fn find_nct6798_dir() -> Option<PathBuf> {
    let devices = fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in devices.flatten() {
        let raw_name = entry.file_name();
        let Ok(raw_name) = raw_name.into_string() else {
            continue; // a non-UTF-8 device name: skip.
        };
        if !raw_name.starts_with("hwmon") {
            continue; // not an hwmon device dir.
        }
        let Ok(device_name) = fs::read_to_string(entry.path().join("name")) else {
            continue; // the device's `name` is unreadable: not it.
        };
        if device_name.trim().eq_ignore_ascii_case("nct6798") {
            return Some(entry.path());
        }
    }
    None
}

/// The per-rail plausibility band (the P2-05 voltage-gate shapes, reused
/// as per-rail constants in this module — not a global re-typing):
/// VDDIO_MEM 100–2000 mV (DDR4 ~1.2 V; > 2 V is implausible — the §9
/// tripwire), Vcore / SoC 100–4000 mV.
fn band(rail: Rail) -> (u16, u16) {
    match rail {
        Rail::VddioMem => (100, 2000),
        Rail::Vcore | Rail::Soc => (100, 4000),
    }
}

/// The per-rail sanity gate (pure): in-band → `Value(mv)`; out-of-band →
/// `Na(ParseError(detail))` carrying the reading (a > 2 V DRAM value is
/// the mis-scaling tripwire — §9 item 4: it renders N/A with a detail
/// string, never plotted).
fn gate_mv(rail: Rail, mv: u16) -> Section<u16> {
    let (lo, hi) = band(rail);
    if mv >= lo && mv <= hi {
        Section::Value(mv)
    } else {
        Section::na(NaReason::ParseError(format!(
            "{rail:?} reads {mv} mV, outside the plausible {lo}–{hi} mV band"
        )))
    }
}

/// One channel of the device: `in{index}_input` is driver-scaled mV (D-2)
/// → `u16` → the rail's [`gate_mv`] band. A missing / unreadable /
/// non-numeric file degrades **that rail only** to `Na(ParseError)`
/// (per-rail containment, §9) — one bad channel never poisons the rest.
fn read_channel(dir: &Path, map: &RailMap) -> Section<u16> {
    let path = dir.join(format!("in{}_input", map.index));
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return Section::na(NaReason::ParseError(format!(
                "nct6798 in{}_input unreadable ({path:?}): {err}",
                map.index
            )));
        }
    };
    let mv: u16 = match text.trim().parse() {
        Ok(mv) => mv,
        Err(err) => {
            return Section::na(NaReason::ParseError(format!(
                "nct6798 in{}_input non-numeric ({path:?}): {err}",
                map.index
            )));
        }
    };
    gate_mv(map.rail, mv)
}

/// Reads the profile's rails from one resolved device dir (the
/// I/O-bearing core of [`read_board_vrm`], split out so the per-rail
/// read/gate path is testable against a synthetic dir). A rail absent
/// from the profile's mapping stays `Na(NotApplicable)` (the all-Na base).
fn read_rails(dir: &Path, profile: &BoardVrmProfile) -> BoardVrmReadout {
    let mut readout = BoardVrmReadout::all_na();
    for map in profile.rails {
        let section = read_channel(dir, map);
        match map.rail {
            Rail::VddioMem => readout.vddio_mem_mv = section,
            Rail::Vcore => readout.vcore_mv = section,
            Rail::Soc => readout.soc_mv = section,
        }
    }
    readout
}

/// The vCore electrical envelope (Chunk 3, research §"Electrical
/// Boundary Verification"): `in0` must read within 0.200–1.750 V.
/// Below = a grounded / unpopulated channel; above = an attenuated
/// auxiliary rail (e.g. VCCIO / VDD) — either way the reading is
/// rejected (N/A), never a garbage value.
const VCORE_MIN_VOLTS: f64 = 0.200;
const VCORE_MAX_VOLTS: f64 = 1.750;

/// The consumer-desktop board vendors (Chunk 3, research §"Fallback
/// Engine Rules"): the vendor-aware vCore fallback activates ONLY for
/// these. Matched as a case-insensitive substring of the DMI
/// `board_vendor` / `sys_vendor` string.
const CONSUMER_VENDORS: &[&str] = &[
    "asus",
    "asrock",
    "micro-star",
    "micro star",
    "msi",
    "gigabyte",
];
/// The enterprise / OEM board vendors: the fallback is aborted for these
/// (their BMCs / embedded controllers route primary power telemetry away
/// from the Super I/O, so a heuristic `in0 → Vcore` would be misleading).
const ENTERPRISE_VENDORS: &[&str] = &["dell", "hewlett", "hp", "lenovo", "supermicro"];

/// A DMI vendor's class (Chunk 3, research §"Fallback Engine Rules"):
/// the vendor-aware vCore fallback is permitted only for
/// [`VendorClass::Consumer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorClass {
    /// A consumer-desktop board vendor (the fallback may bind in0→Vcore).
    Consumer,
    /// An enterprise / OEM vendor (the fallback is aborted → N/A).
    Enterprise,
    /// Neither recognized (the fallback is aborted → N/A).
    Unknown,
}

/// Classify a DMI board / system vendor string (pure): a consumer
/// desktop vendor → [`VendorClass::Consumer`], an enterprise / OEM
/// vendor → [`VendorClass::Enterprise`], anything else →
/// [`VendorClass::Unknown`]. Case-insensitive substring match.
pub fn classify_vendor(vendor: &str) -> VendorClass {
    let lowered = vendor.to_ascii_lowercase();
    if CONSUMER_VENDORS.iter().any(|token| lowered.contains(token)) {
        VendorClass::Consumer
    } else if ENTERPRISE_VENDORS.iter().any(|token| lowered.contains(token)) {
        VendorClass::Enterprise
    } else {
        VendorClass::Unknown
    }
}

/// The vendor-aware vCore fallback (Chunk 3, research §"Fallback Engine
/// Rules"): no curated profile match + a **consumer** vendor → bind
/// `in0 → Vcore` as [`RailConfidence::HeuristicDefault`] (subject to the
/// 0.200–1.750 V bounds check); an enterprise / unknown vendor, a
/// missing vendor, a missing reading, or an out-of-bounds reading →
/// `None` (N/A). Never a fabricated value.
pub fn vcore_fallback(
    board_vendor: Option<&str>,
    in0_mv: Option<u16>,
) -> Option<VoltageTelemetry> {
    let vendor = board_vendor?;
    if classify_vendor(vendor) != VendorClass::Consumer {
        return None; // enterprise / unknown → no heuristic vCore.
    }
    let mv = in0_mv?;
    let volts = f64::from(mv) / 1000.0;
    if !(VCORE_MIN_VOLTS..=VCORE_MAX_VOLTS).contains(&volts) {
        return None; // grounded / unpopulated / attenuated → N/A.
    }
    Some(VoltageTelemetry {
        rail_name: "CPU Vcore".to_owned(),
        delivered_volts: volts,
        requested_vid: None, // no reliable Intel VID MSR.
        delta_volts: None,
        confidence: RailConfidence::HeuristicDefault,
    })
}

/// Read `in0_input` from one device dir as-is (driver-scaled mV, D-2):
/// the nct6775/6798 driver already applies the board's divider, so no
/// software multiplier is applied. A missing / unreadable / non-numeric
/// file → `None` (never a panic).
fn read_in0_mv(dir: &Path) -> Option<u16> {
    let text = fs::read_to_string(dir.join("in0_input")).ok()?;
    text.trim().parse().ok()
}

/// Resolve the vCore from one device dir (the pure core of the vCore
/// path, testable against a synthetic dir): a curated profile match →
/// the profile's `in0 → Vcore` as [`RailConfidence::VerifiedProfile`];
/// no match → the vendor-aware fallback (`HeuristicDefault` for an
/// in-bounds consumer board). **Both** paths apply the 0.200–1.750 V
/// electrical bounds check — an out-of-bounds `in0` (grounded /
/// unpopulated / attenuated) is rejected → `None` (N/A), never a
/// garbage value. `None` also when there is no `in0` reading.
fn resolve_vcore(
    profile: Option<&BoardVrmProfile>,
    board_vendor: Option<&str>,
    dir: &Path,
) -> Option<VoltageTelemetry> {
    let in0_mv = read_in0_mv(dir)?;
    let volts = f64::from(in0_mv) / 1000.0;
    // The electrical bounds check applies to every path: an
    // out-of-envelope reading is grounded / unpopulated / an attenuated
    // aux rail → reject (N/A).
    if !(VCORE_MIN_VOLTS..=VCORE_MAX_VOLTS).contains(&volts) {
        return None;
    }
    if profile.is_some() {
        // A curated profile match: the `in0 → Vcore` mapping is
        // known-correct for this board.
        return Some(VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: volts,
            requested_vid: None,
            delta_volts: None,
            confidence: RailConfidence::VerifiedProfile,
        });
    }
    // No curated profile: the vendor-aware fallback (consumer →
    // HeuristicDefault; enterprise / unknown → None).
    vcore_fallback(board_vendor, Some(in0_mv))
}

/// The binder entry point (the facade's `collect()` calls it after the
/// platform branch — the I/O stays in `collect`; the caller's
/// `motherboard` is the DMI name and `board_vendor` the DMI
/// `board_vendor` from [`crate::platform`]):
/// profile lookup → device walk → per-rail reads → vCore resolution
/// (curated `VerifiedProfile` or the vendor-aware fallback).
///
/// **No-panic:** no `nct6798` device → [`BoardVrmReadout::all_na`] (all
/// `Na(NotApplicable)`, no vCore); no profile → the all-Na rails + the
/// vendor-aware vCore fallback; an unreadable / non-numeric /
/// out-of-band channel degrades that rail only (§9); an out-of-bounds
/// vCore → `None` (N/A).
pub fn read_board_vrm(
    motherboard: &Section<String>,
    board_vendor: Option<&str>,
) -> BoardVrmReadout {
    let profile = profile_for(motherboard);
    let dir = match find_nct6798_dir() {
        Some(dir) => dir,
        None => return BoardVrmReadout::all_na(),
    };
    let mut readout = match profile {
        Some(profile) => read_rails(&dir, profile),
        None => BoardVrmReadout::all_na(),
    };
    readout.vcore = resolve_vcore(profile, board_vendor, &dir);
    readout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::collect_platform;

    /// The hero profile, via its own key (test helper; the lookup is
    /// pure, so a test-only `expect` on it is not hardware-derived data).
    fn hero() -> &'static BoardVrmProfile {
        profile_for(&Section::Value("CROSSHAIR VIII HERO".to_owned()))
            .expect("the hero profile must match its own key")
    }

    /// (1) D-4 lookup: the exact key, a lower-case input
    /// (case-insensitive), and a suffixed name ("… (WI-FI)" — contains,
    /// not equals); the frozen inN → rail mapping.
    #[test]
    fn profile_for_matches_hero_case_insensitively() {
        for name in [
            "CROSSHAIR VIII HERO",
            "rog crosshair viii hero",
            "ROG CROSSHAIR VIII HERO (WI-FI)",
        ] {
            let profile = profile_for(&Section::Value(name.to_owned()))
                .expect("profile lookup must match the hero name case-insensitively");
            assert_eq!(profile.board_key, "CROSSHAIR VIII HERO");
        }
        assert_eq!(
            hero().rails,
            &[
                RailMap {
                    index: 13,
                    rail: Rail::VddioMem,
                },
                RailMap {
                    index: 0,
                    rail: Rail::Vcore,
                },
                RailMap {
                    index: 6,
                    rail: Rail::Soc,
                },
            ]
        );
    }

    /// (2) Unknown board → `None`; a `Na` board name → `None` (the
    /// graceful all-Na path, §9 item 2).
    #[test]
    fn profile_for_returns_none_for_unknown_or_na_board() {
        assert_eq!(
            profile_for(&Section::Value("ASUS TUF GAMING X570-PLUS (WI-FI)".to_owned())),
            None
        );
        assert_eq!(
            profile_for(&Section::Value("ProArt X570-CREATOR".to_owned())),
            None
        );
        assert_eq!(profile_for(&Section::na(NaReason::NotApplicable)), None);
    }

    /// (3) The per-rail bands: VDDIO_MEM in-band at the 100 / 1200 / 2000
    /// mV edges, out-of-band at 0 and 2001 (the > 2 V tripwire carries a
    /// non-empty `ParseError` detail); Vcore / SoC on the 100–4000 mV
    /// shape.
    #[test]
    fn gate_mv_bands_per_rail() {
        assert_eq!(gate_mv(Rail::VddioMem, 100), Section::Value(100));
        assert_eq!(gate_mv(Rail::VddioMem, 1200), Section::Value(1200));
        assert_eq!(gate_mv(Rail::VddioMem, 2000), Section::Value(2000));
        for mv in [0u16, 2001] {
            assert!(
                matches!(&gate_mv(Rail::VddioMem, mv), Section::Na(NaReason::ParseError(detail)) if !detail.is_empty()),
                "out-of-band {mv} mV must degrade to a detailed Na"
            );
        }
        assert_eq!(gate_mv(Rail::Vcore, 1152), Section::Value(1152));
        assert_eq!(gate_mv(Rail::Soc, 1128), Section::Value(1128));
        assert!(
            matches!(gate_mv(Rail::Vcore, 4001), Section::Na(NaReason::ParseError(_)))
        );
    }

    /// (4) The all-Na constructor: every rail `Na(NotApplicable)` and no
    /// resolved vCore.
    #[test]
    fn all_na_constructor_marks_every_rail() {
        let readout = BoardVrmReadout::all_na();
        for section in [&readout.vddio_mem_mv, &readout.vcore_mv, &readout.soc_mv] {
            assert_eq!(*section, Section::Na(NaReason::NotApplicable));
        }
        assert!(readout.vcore.is_none(), "all_na carries no vCore");
    }

    /// (5) A synthetic nct6798 dir: in13=1200 / in0=1152 / in6=1128 → the
    /// full Value readout (D-2: driver-scaled mV, used as-is); then a
    /// non-numeric in13 → **that rail only** degrades (per-rail
    /// containment, §9) while the other two stand.
    #[test]
    fn read_rails_synthetic_device_carries_and_contains() {
        let dir =
            std::env::temp_dir().join(format!("ramsleuth-c12-03-{}", std::process::id()));
        fs::create_dir_all(&dir)
            .expect("temp dir creation must succeed in the test sandbox");
        for (name, value) in [
            ("in13_input", "1200\n"),
            ("in0_input", "1152\n"),
            ("in6_input", "1128\n"),
        ] {
            fs::write(dir.join(name), value).expect("synthetic channel write");
        }

        let readout = read_rails(&dir, hero());
        assert_eq!(readout.vddio_mem_mv, Section::Value(1200));
        assert_eq!(readout.vcore_mv, Section::Value(1152));
        assert_eq!(readout.soc_mv, Section::Value(1128));

        // Per-rail containment: a bad in13 poisons only VddioMem.
        fs::write(dir.join("in13_input"), "garbage\n").expect("synthetic channel write");
        let readout = read_rails(&dir, hero());
        assert!(matches!(
            readout.vddio_mem_mv,
            Section::Na(NaReason::ParseError(_))
        ));
        assert_eq!(readout.vcore_mv, Section::Value(1152));
        assert_eq!(readout.soc_mv, Section::Value(1128));

        let _ = fs::remove_dir_all(&dir);
    }

    /// (6) Host-tolerant no-panic: the live DMI board name + vendor →
    /// `read_board_vrm` never panics; on the Hero host `vddio_mem_mv` is
    /// a `Value` in the 100–2000 mV band, on any other host an `Na`
    /// (the §9 graceful path); every `Value` rail is in-band; a resolved
    /// vCore (if any) is in the 0.200–1.750 V envelope.
    #[test]
    fn read_board_vrm_on_this_host_never_panics() {
        let platform = collect_platform();
        let readout = read_board_vrm(
            &platform.motherboard,
            crate::platform::dmi_board_vendor().as_deref(),
        );
        // Reaching here is the no-panic check.
        for section in [&readout.vddio_mem_mv, &readout.vcore_mv, &readout.soc_mv] {
            if let Section::Value(mv) = section {
                assert!(
                    (100..=4000).contains(mv),
                    "every Value rail must be in-band: {mv} mV"
                );
            }
        }
        match &readout.vddio_mem_mv {
            Section::Value(mv) => {
                assert!((100..=2000).contains(mv), "vddio in-band: {mv} mV");
            }
            Section::Na(_) => {
                // Not the Hero / no nct6798 on this host: tolerated (§9).
            }
        }
        // A resolved vCore is always in the 0.200–1.750 V envelope
        // (the bounds check rejects anything outside it → None).
        if let Some(vcore) = &readout.vcore {
            assert!(
                (VCORE_MIN_VOLTS..=VCORE_MAX_VOLTS).contains(&vcore.delivered_volts),
                "a resolved vCore must be in-envelope: {} V",
                vcore.delivered_volts
            );
        }
    }

    // ------------------------------------------------------------------
    // (Chunk 3) Intel seed profiles + the vendor-aware fallback + the
    // vCore electrical bounds check + the new wire types.
    // ------------------------------------------------------------------

    /// A representative Intel seed board name per vendor (the exact
    /// `board_key` + a lower-case / suffixed variant each, to pin the
    /// case-insensitive contains-match).
    const INTEL_SEED_NAMES: [&str; 6] = [
        "ROG MAXIMUS Z690",
        "ROG STRIX Z790-E GAMING",
        "TUF GAMING Z690-PLUS",
        "Z690 EXTREME",
        "Z790 STEEL LEGEND WIFI",
        "MEG Z790 ACE",
    ];

    /// (C3-1) Each Intel seed `board_key` resolves to a profile whose
    /// `in0 → Vcore` mapping is the single shared `INTEL_VCORE_RAILS`
    /// (the direct VRM sense trace), and a lower-case / suffixed variant
    /// of each name still matches (the case-insensitive contains rule).
    #[test]
    fn intel_seed_profiles_match() {
        for name in INTEL_SEED_NAMES {
            let profile = profile_for(&Section::Value(name.to_owned()))
                .unwrap_or_else(|| panic!("the Intel seed {name:?} must match a profile"));
            assert_eq!(
                profile.rails,
                INTEL_VCORE_RAILS,
                "the {name:?} profile maps in0 → Vcore"
            );
            assert_eq!(
                profile.rails,
                &[RailMap { index: 0, rail: Rail::Vcore }]
            );
            // The lower-case form matches too (case-insensitive).
            let lowered = name.to_ascii_lowercase();
            let matched = profile_for(&Section::Value(lowered.clone())).is_some();
            assert!(matched, "the lower-case form {lowered:?} must match");
        }
        // Every seeded board key in PROFILES is an Intel Z690/Z790 (or
        // the AMD Hero) — the seed is small + growable by design.
        assert!(PROFILES.len() >= 18, "the seed + the Hero are all present");
    }

    /// (C3-5) The reference board (the 12600KF test host): the DMI name
    /// `Z690M-ITX/ax` resolves to its own curated profile via the
    /// case-insensitive contains-match (N1: the vCore badge flips from
    /// `[est]` to the verified-profile form on this board), and the
    /// ASRock Taichi key must not cross-match it.
    #[test]
    fn profile_for_z690m_itx_ax() {
        let profile = profile_for(&Section::Value("Z690M-ITX/ax".to_owned()))
            .unwrap_or_else(|| panic!("the reference board must match its own profile"));
        assert_eq!(profile.board_key, "Z690M-ITX/AX");
        assert_eq!(profile.rails, INTEL_VCORE_RAILS);
        // Negative control: the Taichi key does not cross-match the ITX
        // name (and vice versa).
        assert_ne!(
            profile_for(&Section::Value("Z690M-ITX/ax".to_owned())).map(|p| p.board_key),
            Some("Z690 TAICHI"),
            "Z690M-ITX/ax must not resolve to the Taichi profile"
        );
        assert_ne!(
            profile_for(&Section::Value("Z690 TAICHI".to_owned())).map(|p| p.board_key),
            Some("Z690M-ITX/AX"),
            "Z690 TAICHI must not resolve to the ITX profile"
        );
    }

    /// (C3-2) The vendor-aware fallback classification: a consumer
    /// desktop vendor (ASUS / ASRock / MSI / Gigabyte) → the fallback
    /// binds `in0 → Vcore` as `HeuristicDefault`; an enterprise / OEM
    /// vendor (Dell / HP / Lenovo / Supermicro) and an unknown vendor →
    /// `None` (no heuristic vCore, N/A).
    #[test]
    fn vendor_fallback_consumer_vs_enterprise() {
        // Consumer vendors → HeuristicDefault (in-bounds reading).
        for vendor in [
            "ASUSTeK COMPUTER INC.",
            "ASRock",
            "Micro-Star International Co., Ltd.",
            "Gigabyte Technology Co., Ltd.",
        ] {
            let vcore =
                vcore_fallback(Some(vendor), Some(1216)).expect("consumer vendor in-bounds");
            assert_eq!(
                vcore.confidence,
                RailConfidence::HeuristicDefault,
                "{vendor:?} must be a consumer (heuristic) fallback"
            );
            assert!((vcore.delivered_volts - 1.216).abs() < 1e-9);
            assert_eq!(vcore.rail_name, "CPU Vcore");
            assert!(vcore.requested_vid.is_none());
            assert!(vcore.delta_volts.is_none());
        }
        // Enterprise / OEM vendors → None (no heuristic vCore).
        for vendor in ["Dell Inc.", "Hewlett-Packard", "LENOVO", "Supermicro"] {
            assert_eq!(
                vcore_fallback(Some(vendor), Some(1216)),
                None,
                "{vendor:?} is enterprise: no heuristic vCore"
            );
        }
        // Unknown vendor + a missing vendor → None.
        assert_eq!(vcore_fallback(Some("Some Random OEM"), Some(1216)), None);
        assert_eq!(vcore_fallback(None, Some(1216)), None);
        // A consumer vendor with no reading → None.
        assert_eq!(vcore_fallback(Some("ASUSTeK COMPUTER INC."), None), None);
    }

    /// (C3-3) The vCore electrical bounds check: `in0` at 199 / 1751 mV
    /// is rejected (grounded / unpopulated / attenuated → `None`); 200 /
    /// 1200 / 1750 mV is accepted. The bounds check applies on the
    /// vendor-aware fallback path and the curated-profile path alike.
    #[test]
    fn vcore_electrical_bounds() {
        // Out-of-bounds → None (the fallback path).
        for mv in [0u16, 199, 1751, 3000] {
            assert_eq!(
                vcore_fallback(Some("ASUSTeK COMPUTER INC."), Some(mv)),
                None,
                "{mv} mV is out of the 200–1750 mV envelope"
            );
        }
        // In-bounds (the edges + a mid value) → HeuristicDefault.
        for mv in [200u16, 1200, 1750] {
            let vcore =
                vcore_fallback(Some("ASUSTeK COMPUTER INC."), Some(mv)).expect("in-bounds");
            assert_eq!(vcore.confidence, RailConfidence::HeuristicDefault);
            assert!((vcore.delivered_volts - f64::from(mv) / 1000.0).abs() < 1e-9);
        }
    }

    /// (C3-4) The new wire types: `RailConfidence` + `VoltageTelemetry`
    /// round-trip through bincode (append-only wire-safe), and a
    /// synthetic VID yields the signed (negative) delta.
    #[test]
    fn voltage_telemetry_shape_and_round_trip() {
        // The three confidence arms round-trip.
        let confs: Vec<RailConfidence> = vec![
            RailConfidence::VerifiedProfile,
            RailConfidence::HeuristicDefault,
            RailConfidence::Unknown,
        ];
        let bytes = bincode::serialize(&confs).expect("RailConfidence must serialize");
        let back: Vec<RailConfidence> =
            bincode::deserialize(&bytes).expect("RailConfidence must deserialize");
        assert_eq!(confs, back);

        // A no-VID Intel reading (the common case) + a synthetic-VID
        // reading (the delta path) both round-trip.
        let no_vid = VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: 1.216,
            requested_vid: None,
            delta_volts: None,
            confidence: RailConfidence::HeuristicDefault,
        };
        let with_vid = VoltageTelemetry {
            rail_name: "CPU Vcore".to_owned(),
            delivered_volts: 1.216,
            requested_vid: Some(1.280),
            delta_volts: Some(1.216 - 1.280), // -0.064 (the Vdroop gap)
            confidence: RailConfidence::VerifiedProfile,
        };
        for vcore in [no_vid, with_vid.clone()] {
            let bytes = bincode::serialize(&vcore).expect("VoltageTelemetry must serialize");
            let back: VoltageTelemetry =
                bincode::deserialize(&bytes).expect("VoltageTelemetry must deserialize");
            assert_eq!(vcore, back, "VoltageTelemetry round-trips");
        }
        // The signed delta: delivered (1.216) < vid (1.280) → negative.
        assert!(with_vid.delta_volts.expect("a vid carries a delta") < 0.0);
    }
}
