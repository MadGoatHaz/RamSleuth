//! Vendor-neutral platform identity — DMI + `/proc` sourced (C6-01).
//!
//! A new vendor-independent branch of the telemetry facade (plan D-C1):
//! motherboard / BIOS / AGESA come from the kernel DMI sysfs surface
//! (`/sys/class/dmi/id/*`), the CPU clock from `/proc/cpuinfo`, and the
//! total memory from `/proc/meminfo`. [`CpuInfo`](crate::cpuid::CpuInfo)
//! stays CPUID-pure; this branch runs on every vendor, exactly like the
//! SPD branch.
//!
//! **No-panic contract:** every field degrades independently to
//! [`Section::Na`] with [`NaReason::NotApplicable`] on absence, unreadable
//! I/O, or an unparseable payload — never a panic, never an `unwrap` /
//! `expect` on I/O, and never invented data (a missing source is an
//! honest N/A, not a placeholder string). The frozen entry points are
//! [`collect_platform()`] (→ [`SystemPlatform`]) and [`mem_total_gib()`]
//! (the total-capacity fallback the facade uses when no SPD module is
//! present, D-C3).
//!
//! | field | source | fallback |
//! |---|---|---|
//! | `cpu_clock_mhz` | `/proc/cpuinfo` `cpu MHz` (first core) | `Na(NotApplicable)` |
//! | `motherboard` | DMI `board_name` → `board_vendor` → `product_name` | `Na(NotApplicable)` (VM/container) |
//! | `bios` | DMI `bios_version` (` + <bios_date>` when present) | `Na(NotApplicable)` |
//! | `agesa` | best-effort AGESA token from the BIOS string, else the `ryzen_smu` `version` attribute | `Na(NotApplicable)` (common — no clean unprivileged AGESA) |
//!
//! The `ryzen_smu` `version` attribute is a plain text file (not an SMN
//! register read), so the `0xFFFFFFFF` sentinel rule of the SMN overlay
//! does not apply here: an absent or unreadable attribute simply yields
//! no AGESA source (honest N/A). All sources are unprivileged `std::fs`
//! reads; on non-Linux targets every read fails and the fields degrade
//! to N/A (the module still compiles).

use crate::error::{NaReason, Section};

/// DMI sysfs directory: the kernel-exposed SMBIOS strings (unprivileged).
const DMI_ID_DIR: &str = "/sys/class/dmi/id";

/// The `ryzen_smu` driver `version` attribute, tried in order (the
/// verified kobject name first, then the legacy directory). Consulted as
/// a best-effort AGESA source only when the BIOS string carries no token.
const SMU_VERSION_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/version",
    "/sys/kernel/ryzen_smu/version",
];

/// Vendor-neutral platform identity (C6-01, frozen shape).
///
/// A vendor-independent snapshot branch (plan D-C1): each field degrades
/// independently to `Na(NotApplicable)`, so the struct is always
/// constructible and the frozen [`collect_platform()`] entry point never
/// fails the process.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemPlatform {
    /// Current CPU clock in MHz (`/proc/cpuinfo` `cpu MHz`, first core).
    pub cpu_clock_mhz: Section<f64>,
    /// Motherboard name (DMI `board_name`, falling back to `board_vendor`,
    /// then `product_name`).
    pub motherboard: Section<String>,
    /// BIOS version (DMI `bios_version`, with ` + <bios_date>` appended
    /// when the date attribute is present).
    pub bios: Section<String>,
    /// Best-effort AMD AGESA version token (from the BIOS string or the
    /// `ryzen_smu` `version` attribute); commonly N/A.
    pub agesa: Section<String>,
}

/// Collect the [`SystemPlatform`] snapshot (the frozen C6-01 entry point).
///
/// # No-panic contract
///
/// This function never returns `Err` and never panics: every field
/// degrades independently to `Na(NotApplicable)` on absence, unreadable
/// I/O, or an unparseable payload.
pub fn collect_platform() -> SystemPlatform {
    SystemPlatform {
        cpu_clock_mhz: read_cpu_clock_mhz(),
        motherboard: read_motherboard(),
        bios: read_bios(),
        agesa: read_agesa(),
    }
}

/// Total system memory in GiB, from `/proc/meminfo` `MemTotal` (kB → GiB,
/// 1024-based).
///
/// The total-capacity fallback the facade uses when no SPD module is
/// present (plan D-C3). No-panic contract: any absence / read / parse
/// failure degrades to `Na(NotApplicable)`; never a panic, never an
/// invented value.
pub fn mem_total_gib() -> Section<f64> {
    match read_file("/proc/meminfo").and_then(|content| parse_mem_total_gib(&content)) {
        Some(gib) => Section::Value(gib),
        None => Section::na(NaReason::NotApplicable),
    }
}

// ---------------------------------------------------------------------------
// I/O layer (thin; every failure degrades to `None` — never a panic).
// ---------------------------------------------------------------------------

/// Read a text file, whitespace-trimmed.
///
/// Returns `None` on any absence, read failure, or empty payload (the
/// no-panic contract: I/O errors are swallowed into `None`, never raised).
fn read_file(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        }
        Err(_) => None,
    }
}

/// Read one DMI sysfs attribute (`/sys/class/dmi/id/<name>`).
fn read_dmi(name: &str) -> Option<String> {
    let path = format!("{DMI_ID_DIR}/{name}");
    read_file(&path)
}

/// The `ryzen_smu` driver `version` attribute (the first candidate that
/// exists), or `None` when the driver publishes none (or is absent).
fn read_smu_version() -> Option<String> {
    SMU_VERSION_CANDIDATES.iter().find_map(|path| read_file(path))
}

/// `cpu_clock_mhz` source: the first `cpu MHz` line of `/proc/cpuinfo`.
fn read_cpu_clock_mhz() -> Section<f64> {
    match read_file("/proc/cpuinfo").and_then(|content| parse_cpu_mhz(&content)) {
        Some(mhz) => Section::Value(mhz),
        None => Section::na(NaReason::NotApplicable),
    }
}

/// `motherboard` source: the frozen DMI fallback chain (`board_name` →
/// `board_vendor` → `product_name`).
fn read_motherboard() -> Section<String> {
    let board_name = read_dmi("board_name");
    let board_vendor = read_dmi("board_vendor");
    let product_name = read_dmi("product_name");
    match board_from_sources(
        board_name.as_deref(),
        board_vendor.as_deref(),
        product_name.as_deref(),
    ) {
        Some(name) => Section::Value(name),
        None => Section::na(NaReason::NotApplicable),
    }
}

/// `bios` source: DMI `bios_version`, with ` + <bios_date>` appended when
/// the date attribute is present.
fn read_bios() -> Section<String> {
    let version = read_dmi("bios_version");
    let date = read_dmi("bios_date");
    match bios_from_sources(version.as_deref(), date.as_deref()) {
        Some(text) => Section::Value(text),
        None => Section::na(NaReason::NotApplicable),
    }
}

/// `agesa` source: a best-effort AGESA token from the BIOS version string,
/// else from the `ryzen_smu` `version` attribute when it carries one.
fn read_agesa() -> Section<String> {
    let bios = read_dmi("bios_version");
    let smu = read_smu_version();
    agesa_from_sources(bios.as_deref(), smu.as_deref())
}

// ---------------------------------------------------------------------------
// Pure parse helpers (no I/O; unit-tested with synthetic payloads).
// ---------------------------------------------------------------------------

/// Parse the first `cpu MHz` line of `/proc/cpuinfo` content into an MHz
/// `f64`.
///
/// Returns `None` when the line is absent or its value fails to parse, and
/// also when the value is not a finite positive clock (a kernel reporting
/// `0.000` carries no usable data → honest N/A downstream).
fn parse_cpu_mhz(content: &str) -> Option<f64> {
    for line in content.lines() {
        let line = line.trim();
        let rest = match line.strip_prefix("cpu MHz") {
            Some(rest) => rest,
            None => continue,
        };
        let value = match rest.trim_start().strip_prefix(':') {
            Some(value) => value,
            None => continue,
        };
        // The first `cpu MHz` line is authoritative; whatever it carries
        // (valid, malformed, non-positive) ends the search here.
        match value.trim().parse::<f64>() {
            Ok(mhz) => return (mhz.is_finite() && mhz > 0.0).then_some(mhz),
            Err(_) => return None,
        }
    }
    None
}

/// Parse the `MemTotal:` line of `/proc/meminfo` content into total
/// memory in GiB (kB → GiB, 1024-based). The first `MemTotal` line is
/// authoritative.
fn parse_mem_total_gib(content: &str) -> Option<f64> {
    for line in content.lines() {
        let line = line.trim();
        let rest = match line.strip_prefix("MemTotal:") {
            Some(rest) => rest,
            None => continue,
        };
        let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(kb as f64 / (1024.0 * 1024.0));
    }
    None
}

/// The frozen DMI fallback order for the motherboard name: `board_name`
/// first, then `board_vendor`, then `product_name`; the first present,
/// non-empty value wins.
fn board_from_sources(
    board_name: Option<&str>,
    board_vendor: Option<&str>,
    product_name: Option<&str>,
) -> Option<String> {
    [board_name, board_vendor, product_name]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty())
        .map(str::to_owned)
}

/// The BIOS display string: the version, with ` + <date>` appended when
/// the date attribute is present and non-empty. `None` when the version is
/// absent (a date without a version carries no displayable data).
fn bios_from_sources(version: Option<&str>, date: Option<&str>) -> Option<String> {
    let version = version?;
    Some(match date {
        Some(date) if !date.trim().is_empty() => format!("{version} + {date}"),
        _ => version.to_owned(),
    })
}

/// The AGESA source chain (frozen): a best-effort token from the BIOS
/// version string first, then from the `ryzen_smu` `version` attribute;
/// `Na(NotApplicable)` when neither carries one (common — there is no
/// clean unprivileged AGESA source on most hosts).
fn agesa_from_sources(bios_version: Option<&str>, smu_version: Option<&str>) -> Section<String> {
    if let Some(token) = bios_version.and_then(parse_agesa) {
        return Section::Value(token);
    }
    if let Some(token) = smu_version.and_then(parse_agesa) {
        return Section::Value(token);
    }
    Section::na(NaReason::NotApplicable)
}

/// Best-effort AGESA token extraction from a BIOS (or driver) version
/// string.
///
/// Recognized shapes (whitespace-delimited, whole-token match only — no
/// substring guessing):
///
/// - an `AGESA` keyword (case-insensitive) immediately followed by a
///   version-shaped token (e.g. `AGESA 12.0.6557.0`);
/// - a bare version-shaped token: `d{1,2}.d{1,2}.d{2,6}` with an optional
///   fourth group `d{1,6}` (e.g. `12.0.6557.0`), optionally prefixed by
///   one letter in `P`/`C` (the documented project-code prefixes, e.g.
///   `P3.20.0017`).
///
/// The match is deliberately conservative: a token that does not fully
/// validate is skipped, and a string with no valid token yields `None` →
/// honest N/A, never a fabricated value.
fn parse_agesa(s: &str) -> Option<String> {
    let mut iter = s.split_whitespace().peekable();
    while let Some(token) = iter.next() {
        if token.eq_ignore_ascii_case("AGESA") {
            if let Some(next) = iter.peek() {
                if is_agesa_version(next) {
                    return Some(next.to_string());
                }
            }
            continue;
        }
        if is_agesa_version(token) {
            return Some(token.to_owned());
        }
    }
    None
}

/// Validate one whitespace token as an AGESA version shape (see
/// [`parse_agesa`]).
fn is_agesa_version(token: &str) -> bool {
    let groups: Vec<&str> = token.split('.').collect();
    if !(3..=4).contains(&groups.len()) {
        return false;
    }
    // First group: 1–2 digits, optionally prefixed by one `P`/`C` letter
    // (the documented project-code prefixes).
    let g0 = match groups[0].strip_prefix('P').or_else(|| groups[0].strip_prefix('C')) {
        Some(digits) => digits,
        None => groups[0],
    };
    is_version_group(g0, 1, 2)
        && is_version_group(groups[1], 1, 2)
        && is_version_group(groups[2], 2, 6)
        && groups.get(3).map_or(true, |g| is_version_group(g, 1, 6))
}

/// One dotted group: `min`–`max` ASCII digits.
fn is_version_group(group: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&group.len()) && group.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // (a) Pure parse helpers over synthetic payloads.
    // ------------------------------------------------------------------

    /// A synthetic `/proc/cpuinfo` (two cores; the first is authoritative).
    const CPUINFO: &str = "processor\t: 0\n\
        vendor_id\t: AuthenticAMD\n\
        cpu MHz\t\t: 3800.000\n\
        cache size\t: 512 KB\n\
        processor\t: 1\n\
        cpu MHz\t\t: 4200.000\n";

    /// (a1) The first `cpu MHz` line wins; the value parses as MHz.
    #[test]
    fn parse_cpu_mhz_first_core_wins() {
        assert_eq!(parse_cpu_mhz(CPUINFO), Some(3800.0));
    }

    /// (a2) Absent / malformed / non-positive values all degrade to
    /// `None` (never a panic, never a bogus clock).
    #[test]
    fn parse_cpu_mhz_degrades_to_none() {
        // No `cpu MHz` line at all.
        assert_eq!(parse_cpu_mhz("processor\t: 0\nvendor_id\t: GenuineIntel\n"), None);
        // Malformed value.
        assert_eq!(parse_cpu_mhz("cpu MHz\t\t: abc\n"), None);
        // Non-positive (a kernel sleeping at 0.000 carries no usable data).
        assert_eq!(parse_cpu_mhz("cpu MHz\t\t: 0.000\n"), None);
        // Negative.
        assert_eq!(parse_cpu_mhz("cpu MHz\t\t: -1.5\n"), None);
        // NaN-ish text.
        assert_eq!(parse_cpu_mhz("cpu MHz\t\t: nan\n"), None);
        // Key present but no colon after it.
        assert_eq!(parse_cpu_mhz("cpu MHz something: 1.0\n"), None);
    }

    /// (a3) `MemTotal` kB → GiB (1024-based), the unit word skipped.
    #[test]
    fn parse_mem_total_gib_happy_path() {
        let meminfo = "MemTotal:       32768000 kB\nMemFree:         1024 kB\nMemAvailable:   30000000 kB\n";
        let gib = parse_mem_total_gib(meminfo).expect("MemTotal present");
        assert!((gib - 31.25).abs() < 1e-9, "expected 31.25 GiB, got {gib}");
        // Exactly 1 GiB.
        assert_eq!(parse_mem_total_gib("MemTotal:       1048576 kB\n"), Some(1.0));
    }

    /// (a4) Absent / malformed `MemTotal` degrades to `None`.
    #[test]
    fn parse_mem_total_gib_degrades_to_none() {
        assert_eq!(parse_mem_total_gib("MemFree:         1024 kB\n"), None);
        assert_eq!(parse_mem_total_gib("MemTotal:       notanumber kB\n"), None);
        assert_eq!(parse_mem_total_gib("MemTotal:\n"), None);
    }

    /// (a5) The DMI motherboard fallback chain: `board_name` wins, then
    /// `board_vendor`, then `product_name`; empty values are skipped.
    #[test]
    fn board_sources_fallback_chain() {
        assert_eq!(
            board_from_sources(Some("ProArt X570-CREATOR"), Some("ASUSTeK"), Some("X570")),
            Some("ProArt X570-CREATOR".to_owned())
        );
        assert_eq!(
            board_from_sources(None, Some("ASUSTeK COMPUTER INC."), Some("X570")),
            Some("ASUSTeK COMPUTER INC.".to_owned())
        );
        assert_eq!(
            board_from_sources(None, None, Some("System Product Name")),
            Some("System Product Name".to_owned())
        );
        // Empty strings are skipped, not returned.
        assert_eq!(
            board_from_sources(Some(""), Some("  "), Some("Board")),
            Some("Board".to_owned())
        );
        assert_eq!(board_from_sources(None, None, None), None);
    }

    /// (a6) The BIOS display string: version + ` + date`; a missing date
    /// leaves the version bare; a missing version is `None` even with a
    /// date.
    #[test]
    fn bios_sources_combine_version_and_date() {
        assert_eq!(
            bios_from_sources(Some("F60"), Some("09/15/2024")),
            Some("F60 + 09/15/2024".to_owned())
        );
        assert_eq!(bios_from_sources(Some("F60"), None), Some("F60".to_owned()));
        assert_eq!(bios_from_sources(None, Some("09/15/2024")), None);
        assert_eq!(bios_from_sources(None, None), None);
    }

    // ------------------------------------------------------------------
    // (b) AGESA best-effort parse (plan D-C1).
    // ------------------------------------------------------------------

    /// (b1) Recognized AGESA shapes: the `AGESA` keyword form (any case),
    /// the bare 4-group form, and the project-code-prefixed 3-group form.
    #[test]
    fn parse_agesa_recognized_shapes() {
        assert_eq!(parse_agesa("AGESA 12.0.6557.0"), Some("12.0.6557.0".to_owned()));
        // A token following ordinary prose is still picked up.
        assert_eq!(parse_agesa("BIOS ver 12.0.6557.0"), Some("12.0.6557.0".to_owned()));
        assert_eq!(parse_agesa("P3.20.0017"), Some("P3.20.0017".to_owned()));
        assert_eq!(parse_agesa("C2.61.1000"), Some("C2.61.1000".to_owned()));
        // Case-insensitive keyword.
        assert_eq!(parse_agesa("agesa 12.0.6557.0"), Some("12.0.6557.0".to_owned()));
    }

    /// (b2) Non-AGESA strings degrade to `None` (never a fabricated
    /// token): vendor BIOS codes, slash dates, and short version numbers.
    #[test]
    fn parse_agesa_rejects_non_agesa() {
        assert_eq!(parse_agesa("F60"), None);
        assert_eq!(parse_agesa("ASUS AM555B-B3909"), None);
        assert_eq!(parse_agesa("5.21"), None);
        assert_eq!(parse_agesa("09/15/2024"), None);
        assert_eq!(parse_agesa("1.2"), None);
        assert_eq!(parse_agesa(""), None);
        // `AGESA` keyword with no valid token after it.
        assert_eq!(parse_agesa("AGESA F60"), None);
    }

    /// (b3) Documented heuristic boundary: a bare 3-group dot-numeric
    /// (e.g. a dot-separated date) passes the conservative shape test —
    /// an accepted best-effort trade, pinned here so the boundary is
    /// visible (strings with no dotted numeric at all still yield N/A).
    #[test]
    fn parse_agesa_documented_boundary() {
        assert_eq!(parse_agesa("09.15.2024"), Some("09.15.2024".to_owned()));
    }

    /// (b4) The source chain prefers the BIOS string; the `ryzen_smu`
    /// `version` attribute is consulted only when the BIOS string carries
    /// no token; neither → `Na(NotApplicable)`.
    #[test]
    fn agesa_source_chain() {
        assert_eq!(
            agesa_from_sources(Some("AGESA 12.0.6557.0"), Some("ryzen_smu 1.0.0")),
            Section::Value("12.0.6557.0".to_owned())
        );
        // BIOS string has no token → the smu attribute's token wins.
        assert_eq!(
            agesa_from_sources(Some("F60"), Some("SMU 12.0.6557.0")),
            Section::Value("12.0.6557.0".to_owned())
        );
        // Neither carries one → honest N/A.
        assert_eq!(
            agesa_from_sources(Some("F60"), Some("ryzen_smu 1.0.0")),
            Section::na(NaReason::NotApplicable)
        );
        assert_eq!(
            agesa_from_sources(None, None),
            Section::na(NaReason::NotApplicable)
        );
    }

    // ------------------------------------------------------------------
    // (c) Graceful-host runs (no-panic contract; host-state tolerant).
    // ------------------------------------------------------------------

    /// `Value`, or `Na(NotApplicable)` — the only two states this branch
    /// can produce for any field (the frozen fallback column).
    fn is_value_or_na(section: &Section<impl std::fmt::Debug>) -> bool {
        matches!(section, Section::Value(_) | Section::Na(NaReason::NotApplicable))
    }

    /// (c1) `collect_platform()` on this host never panics and every field
    /// is `Value` or `Na(NotApplicable)`; a carried clock is in-band
    /// (finite, positive MHz).
    #[test]
    fn collect_platform_on_this_host_is_graceful() {
        let p = collect_platform();

        assert!(
            is_value_or_na(&p.cpu_clock_mhz),
            "cpu_clock_mhz must be Value or Na(NotApplicable): {:?}",
            p.cpu_clock_mhz
        );
        assert!(
            is_value_or_na(&p.motherboard),
            "motherboard must be Value or Na(NotApplicable): {:?}",
            p.motherboard
        );
        assert!(
            is_value_or_na(&p.bios),
            "bios must be Value or Na(NotApplicable): {:?}",
            p.bios
        );
        assert!(
            is_value_or_na(&p.agesa),
            "agesa must be Value or Na(NotApplicable): {:?}",
            p.agesa
        );

        // A carried clock is in-band (never garbage, never a sentinel).
        if let Section::Value(mhz) = &p.cpu_clock_mhz {
            assert!(mhz.is_finite() && *mhz > 0.0, "clock must be finite positive MHz: {mhz}");
        }
    }

    /// (c2) On this Linux host, `/proc/cpuinfo` and `/proc/meminfo` are
    /// present: the clock and the total capacity come back as in-band
    /// `Value`s, never a bogus N/A.
    #[test]
    fn proc_sources_populated_on_this_host() {
        let mhz = collect_platform().cpu_clock_mhz;
        assert!(
            matches!(&mhz, Section::Value(v) if v.is_finite() && *v > 0.0),
            "expected a Value clock, got {mhz:?}"
        );

        let gib = mem_total_gib();
        assert!(
            matches!(&gib, Section::Value(v) if v.is_finite() && *v > 0.0 && *v < 1_000_000.0),
            "expected a Value GiB in (0, 1e6), got {gib:?}"
        );
    }

    /// (c3) The frozen wire shape round-trips through bincode (the P3-01
    /// convention for every frozen wire type): a representative mixed
    /// `Value`/`Na` snapshot and a fully-degraded one.
    #[test]
    fn system_platform_bincode_round_trip() {
        let p = SystemPlatform {
            cpu_clock_mhz: Section::Value(3800.0),
            motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
            bios: Section::Value("F60 + 09/15/2024".to_owned()),
            agesa: Section::na(NaReason::NotApplicable),
        };
        let bytes = bincode::serialize(&p)
            .expect("SystemPlatform must serialize (no-panic contract)");
        let back: SystemPlatform =
            bincode::deserialize(&bytes).expect("SystemPlatform must deserialize");
        assert_eq!(p, back);

        let all_na = SystemPlatform {
            cpu_clock_mhz: Section::na(NaReason::NotApplicable),
            motherboard: Section::na(NaReason::NotApplicable),
            bios: Section::na(NaReason::NotApplicable),
            agesa: Section::na(NaReason::NotApplicable),
        };
        let bytes = bincode::serialize(&all_na)
            .expect("SystemPlatform must serialize (no-panic contract)");
        let back: SystemPlatform =
            bincode::deserialize(&bytes).expect("SystemPlatform must deserialize");
        assert_eq!(all_na, back);
    }

    /// (c4) Live snapshot: `collect_platform()` on this host round-trips
    /// through bincode and compares equal (wire-safe in every host state).
    #[test]
    fn collect_platform_on_this_host_bincode_round_trip() {
        let p = collect_platform();
        let bytes = bincode::serialize(&p)
            .expect("SystemPlatform must serialize (no-panic contract)");
        let back: SystemPlatform =
            bincode::deserialize(&bytes).expect("SystemPlatform must deserialize");
        assert_eq!(p, back);
    }
}
