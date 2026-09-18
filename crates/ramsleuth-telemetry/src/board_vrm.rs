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
//! **Zero-wire:** the readout is consumed in-crate by the facade
//! (C12-04, the fill-when-Na overlay into the existing
//! `VoltageSet.vddio_mem_mv` slot); the in0/in6 cross-check fields ride
//! the readout for the live checklist only (D-5).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{NaReason, Section};

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

/// The static board profile registry (D-4).
pub const PROFILES: &[BoardVrmProfile] = &[CROSSHAIR_VIII_HERO];

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
}

impl BoardVrmReadout {
    /// The fully-degraded readout: every rail `Na(NotApplicable)`
    /// (unknown board, no `nct6798` device, or unreadable). Never panics.
    pub fn all_na() -> Self {
        let na = Section::na(NaReason::NotApplicable);
        Self {
            vddio_mem_mv: na.clone(),
            vcore_mv: na.clone(),
            soc_mv: na,
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

/// The binder entry point (the facade's `collect()` calls it after the
/// platform branch — the I/O stays in `collect`; the caller's
/// `motherboard` is the DMI name from [`crate::platform`]):
/// profile lookup → device walk → per-rail reads.
///
/// **No-panic:** no profile / no `nct6798` device →
/// [`BoardVrmReadout::all_na`] (all `Na(NotApplicable)`); an unreadable /
/// non-numeric / out-of-band channel degrades that rail only (§9).
pub fn read_board_vrm(motherboard: &Section<String>) -> BoardVrmReadout {
    let profile = match profile_for(motherboard) {
        Some(profile) => profile,
        None => return BoardVrmReadout::all_na(),
    };
    let dir = match find_nct6798_dir() {
        Some(dir) => dir,
        None => return BoardVrmReadout::all_na(),
    };
    read_rails(&dir, profile)
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

    /// (4) The all-Na constructor: every rail `Na(NotApplicable)`.
    #[test]
    fn all_na_constructor_marks_every_rail() {
        let readout = BoardVrmReadout::all_na();
        for section in [&readout.vddio_mem_mv, &readout.vcore_mv, &readout.soc_mv] {
            assert_eq!(*section, Section::Na(NaReason::NotApplicable));
        }
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

    /// (6) Host-tolerant no-panic: the live DMI board name →
    /// `read_board_vrm` never panics; on the Hero host `vddio_mem_mv` is
    /// a `Value` in the 100–2000 mV band, on any other host an `Na`
    /// (the §9 graceful path); every `Value` rail is in-band.
    #[test]
    fn read_board_vrm_on_this_host_never_panics() {
        let platform = collect_platform();
        let readout = read_board_vrm(&platform.motherboard);
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
    }
}
