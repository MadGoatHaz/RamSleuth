//! SMBIOS Type 17 cross-check (Intel memory-channel detection, Chunk 2),
//! vendor-neutral SPD fallback (Chunk 4), and AGESA version from SMBIOS
//! Type 40 ("Additional Information").
//!
//! The hardware MMIO decode ([`crate::intel_readout::decode`]) can misreport
//! a populated dual-channel kit as single-channel when the IMC registers are
//! read at the wrong base (the `0x5000` → `0xd800` migration) or carry a
//! stale / vendor-customized value. This module is the **firmware**
//! cross-check the 4-tier trust procedure
//! ([`crate::intel_readout::resolve_channel`]) relies on: it reads the
//! kernel-exported SMBIOS Type 17 "Memory Device" structures and derives a
//! population count, the per-DIMM sizes, the active memory technology, and —
//! from the board-locator strings — whether the populated DIMMs sit in
//! matched channel slots (dual), a single channel, or carry ambiguous
//! (non-standard) labels.
//!
//! **Chunk 4 (vendor-neutral SPD fallback):** the same SMBIOS Type 17
//! parsing is also used as a fallback when the `ee1004` EEPROM driver has no
//! bound devices (common on TRX40 / older AMD platforms). The
//! [`collect_module_entries`] function returns a [`SmbiosModuleEntry`] per
//! populated DIMM with basic module identity (size, speed, part number,
//! manufacturer, serial, locator). SMBIOS does **not** carry XMP/EXPO
//! profiles or JEP106 die-maker codes — those are SPD-EEPROM-only — so the
//! fallback is marked "limited" by the consumer (the facade threads the
//! source annotation into the readout).
//!
//! # Source (unprivileged, `std::fs`-only)
//!
//! The kernel's `dmi` sysfs exports each raw Type 17 structure as a
//! **directory** `/sys/firmware/dmi/entries/17-<n>` (one directory per
//! physical memory device; the trailing `<n>` is the structure handle
//! ordinal) whose payload file **`17-<n>/raw`** (mode `0400 root:root` —
//! the daemon runs uid 0, so `std::fs` reads it with no extra capability)
//! is the DMTF Type 17 fixed part (`[0..len]`, `len` = byte `0x01`)
//! followed by the null-separated string table. The string table starts
//! at the **first** string (there is no leading empty segment): string
//! ref `N` = the `N`th string.
//!
//! The real-table layout is SMBIOS **3.5** (verified on the reference
//! 12600KF / Z690M-ITX/ax dumps: 92-byte fixed part, matching DMTF
//! DSP0134 3.5.0 Table 76): the classic layout **shifted +2** by the
//! two-byte `Memory Error Information Handle` field at `0x06-0x07`
//! (`0xFE 0xFF` = "Not Provided"; the 3.x table has the `Device Locator`
//! @ `0x10` + `Bank Locator` @ `0x11` pair and no separate
//! "Memory Device Locator" field). The parser is version-aware (M-12): it
//! reads the SMBIOS major version (`id/bios_major`) and selects these 3.x
//! offsets (major ≥ 3) or the 2.x classic table (major < 3, the same fields
//! shifted -2). The verified 3.x offsets:
//!
//! - **capacity** @ `0x0C` (16-bit word; per the spec the magnitude is
//!   set by bit 15 — **clear = MB unit, set = KB unit**: `0x4000` =
//!   16384 MB = 16 GiB; the sentinel words `0x7FFF` (≥ 32 GiB − 1 MiB)
//!   and `0xFFFF` (unknown) point to the extended size);
//! - **extended capacity** @ `0x1C` (32-bit dword, MiB in bits 30:0 —
//!   the SMBIOS 3.0+ extended size; zero on the reference board);
//! - **memory type** @ `0x12` (`0x1A` = DDR4, `0x22` = DDR5);
//! - **speed** @ `0x20` (16-bit word, MT/s **directly** — the "Configured
//!   Memory Speed"; `0` = unknown. Verified `0x0960` = 2400 on the x99
//!   Broadwell-E and `0x0E10` = 3600 on the 12600KF);
//! - **locators**: the "Device Locator" string ref @ `0x10`, with the
//!   "Bank Locator" string ref @ `0x11` as the fallback.
//!
//! # No-panic contract (D5)
//!
//! Every read degrades to the empty topology on any absence / unreadable /
//! malformed input — a missing `dmi` directory (a VM / container / non-Intel
//! host) yields [`SmbiosTopology::empty`] with
//! [`SmbiosSourceStatus::DirAbsent`], a corrupt structure is skipped and
//! reported as [`SmbiosSourceStatus::ParseError`] (M-17: the status tells
//! the *why* the facade renders into the annotation), and nothing ever
//! panics. The root is injectable ([`collect_from`]) so the fixture tests
//! build a synthetic `17-*` tree in a temp directory (no hardware, no root).
//!
//! # Why not EDAC
//!
//! The Linux EDAC subsystem is **absent** on consumer Intel client desktops
//! (research: `igen6_edac` requires in-band ECC, which is fused off on
//! client silicon, and the mainline client driver aborts with `-ENODEV`).
//! This module therefore has **no** EDAC dependency — SMBIOS Type 17 is the
//! universal, unprivileged population source.
//!
//! # Derived mode
//!
//! [`SmbiosTopology::derived_mode`] classifies the populated DIMMs from
//! their locator strings, resolved by the data-driven channel-locator
//! grammar ([`channel_of`] over [`CHANNEL_PATTERNS`] — M-15):
//!
//! - **Dual** — the populated DIMMs span ≥ 2 distinct channel slots (e.g.
//!   `A2` + `B2`, the standard mirrored daisy-chain pair);
//! - **Single** — all populated DIMMs sit on one channel (e.g. `A1` + `A2`);
//! - **Ambiguous** — a populated DIMM carries a locator with no channel
//!   designation (e.g. `Bank 1` / `Bank 2`), so the channel cannot be told
//!   apart (the L4 heuristic input).

use std::collections::HashSet;
use std::path::Path;

use crate::intel_readout::MemoryTech;

/// The real SMBIOS Type 17 entry directory (frozen; unprivileged).
const DMI_ENTRIES_DIR: &str = "/sys/firmware/dmi/entries";

/// The file-name prefix of a raw SMBIOS Type 17 structure (`17-<n>`).
const TYPE17_PREFIX: &str = "17-";

/// The structure-header length field offset (shared by both layouts).
const OFF_LENGTH: usize = 0x01;

/// The byte length of the 16-bit "Memory Device Capacity" word (shared).
const CAPACITY_LEN: usize = 2;
const OFF_SPEED: usize = 0x20;
const SPEED_LEN: usize = 2;

/// The Type 17 fixed-part field offsets for one SMBIOS major-version
/// family (M-12: the parser is now version-aware instead of hard-coding
/// the 3.5 layout). SMBIOS 3.0+ inserted the 2-byte `Memory Error
/// Information Handle` at `0x06-0x07`, shifting the fields that follow by
/// +2 relative to the 2.x classic table.
///
/// Chunk 4: the string-reference fields (manufacturer, serial number,
/// asset tag, part number) are now version-keyed too — the same +2 shift
/// applies (verified against the real 12600KF/Z690M-ITX/ax 3.5 fixture
/// whose strings decode to "Team Group Inc" / "0104D817" /
/// "9876543210" / "TEAMGROUP-UD4-3600").
#[derive(Debug, Clone, Copy)]
struct Type17Offsets {
    /// The 16-bit "Memory Device Capacity" word.
    capacity: usize,
    /// The 32-bit "Memory Device Extended Size" dword. `None` for the 2.x
    /// family (the extended size is a 3.0+ field); `Some` for 3.x.
    capacity_extended: Option<usize>,
    /// The 1-byte "Memory Technology" field.
    memory_type: usize,
    /// The "Device Locator" string reference.
    mem_device_locator: usize,
    /// The "Bank Locator" string reference.
    bank_locator: usize,
    /// The "Manufacturer" string reference (Chunk 4).
    manufacturer: usize,
    /// The "Serial Number" string reference (Chunk 4).
    serial_number: usize,
    /// The "Asset Tag" string reference (Chunk 4).
    asset_tag: usize,
    /// The "Part Number" string reference (Chunk 4).
    part_number: usize,
}

/// The SMBIOS 2.x (2.4 / 2.6) Type 17 offsets — the classic DMTF table
/// (no `Error Information Handle`).
///
/// M-12: each field is the verified 3.x value shifted **-2**, the clean
/// inversion of the 3.0 `Memory Error Information Handle` insertion at
/// `0x06-0x07` (the task's stated principle). The result is a
/// non-overlapping layout: the 16-bit capacity word sits at `0x0A-0x0B`,
/// the locators at `0x08`/`0x09`, the memory type at `0x10`, and the
/// speed at `0x1E` — so a 2.x DIMM decodes its capacity, type, and
/// channel locator exactly as a 3.x one does. (A naive field-by-field
/// mix, e.g. a type byte at `0x0B`, would overlap the capacity word and
/// mis-decode it; the uniform -2 shift avoids that.) The 2.x family has no
/// extended-size field (a 3.0+ addition), so `capacity_extended` is
/// `None`.
const SMBIOS2_OFFSETS: Type17Offsets = Type17Offsets {
    capacity: 0x0A,
    capacity_extended: None,
    memory_type: 0x10,
    mem_device_locator: 0x08,
    bank_locator: 0x09,
    // Chunk 4: the 2.x string refs are the 3.x values shifted -2 (the
    // same uniform shift as the other fields).
    manufacturer: 0x15,
    serial_number: 0x16,
    asset_tag: 0x17,
    part_number: 0x18,
};

/// The SMBIOS 3.x (3.0–3.8, incl. the verified 3.5 reference) Type 17
/// offsets — the classic table shifted +2 by the `Error Information
/// Handle` at `0x06-0x07` (verified on the reference 12600KF /
/// Z690M-ITX/ax dumps: capacity `0x0C`, extended `0x1C`, type `0x12`,
/// locators `0x10`/`0x11`). The 3.x `Configured Memory Speed` lives at
/// `0x20` (`0x0E10` = 3600 MT/s on the reference) but is not decoded here.
const SMBIOS3_OFFSETS: Type17Offsets = Type17Offsets {
    capacity: 0x0C,
    capacity_extended: Some(0x1C),
    memory_type: 0x12,
    mem_device_locator: 0x10,
    bank_locator: 0x11,
    // Chunk 4: verified against the real 12600KF/Z690M-ITX/ax 3.5 fixture
    // (strings 3-6 = "Team Group Inc" / "0104D817" / "9876543210" /
    // "TEAMGROUP-UD4-3600").
    manufacturer: 0x17,
    serial_number: 0x18,
    asset_tag: 0x19,
    part_number: 0x1A,
};

/// The source status of the SMBIOS Type 17 collection (M-17): *why* the
/// topology is empty (or not), so the facade can tell the user the
/// firmware cross-check was unavailable — not just that it was. Rendered
/// into the readout's annotation (no new wire field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmbiosSourceStatus {
    /// The DMI source was read and parsed (a populated or legitimately
    /// empty topology).
    Ok,
    /// The `/sys/firmware/dmi` directory is absent (VM / container /
    /// non-Intel host).
    DirAbsent,
    /// The DMI directory exists but could not be read (permission / I/O).
    Unreadable,
    /// A Type 17 structure was present but corrupt / too short to parse.
    ParseError,
}

impl std::fmt::Display for SmbiosSourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SmbiosSourceStatus::Ok => "ok",
            SmbiosSourceStatus::DirAbsent => "DMI source absent (VM/container/non-Intel)",
            SmbiosSourceStatus::Unreadable => "DMI source unreadable",
            SmbiosSourceStatus::ParseError => "DMI structure corrupt",
        };
        write!(f, "{s}")
    }
}

/// The data-driven channel-locator patterns (M-15): common firmware
/// grammars mapped to a channel index (`0`–`3` → `A`–`D`). First match
/// wins; matching is case-insensitive over the locator's alphanumeric
/// words (see [`channel_of`]).
///
/// The table covers:
/// - standalone channel letters (`a`–`d`);
/// - OEM `P1`–`P4` designators;
/// - `CH0`–`CH3` designators;
/// - `Slot A`–`D` word pairs;
/// - `DIMM A`–`D` word pairs;
/// - `DIMM 1`–`4` word pairs (the number is the channel);
/// - the per-firmware `Controller0`–`3` designators (the Z690M-ITX/ax
///   reference grammar — `Controller N` → channel `N` — now vendor-neutral
///   in this table, superseding the old board-keyed special case).
const CHANNEL_PATTERNS: &[(&str, u8)] = &[
    // Standalone channel letters.
    ("a", 0),
    ("b", 1),
    ("c", 2),
    ("d", 3),
    // OEM P1–P4 designators.
    ("p1", 0),
    ("p2", 1),
    ("p3", 2),
    ("p4", 3),
    // CH0–CH3 designators.
    ("ch0", 0),
    ("ch1", 1),
    ("ch2", 2),
    ("ch3", 3),
    // "Slot A–D" word pairs.
    ("slot a", 0),
    ("slot b", 1),
    ("slot c", 2),
    ("slot d", 3),
    // "DIMM A–D" word pairs.
    ("dimm a", 0),
    ("dimm b", 1),
    ("dimm c", 2),
    ("dimm d", 3),
    // "DIMM 1–4" word pairs (the number is the channel).
    ("dimm 1", 0),
    ("dimm 2", 1),
    ("dimm 3", 2),
    ("dimm 4", 3),
    // Per-firmware "Controller0–3" designators (the Z690M-ITX/ax reference
    // grammar; `Controller N` → channel `N`).
    ("controller0", 0),
    ("controller1", 1),
    ("controller2", 2),
    ("controller3", 3),
];

/// The channel classification derived from the populated DIMM locators
/// (research: "matched A/B pairs → Dual; both on one channel → Single;
/// a locator with no channel designation → Ambiguous").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedMode {
    /// The populated DIMMs span two or more distinct channels.
    Dual,
    /// Every populated DIMM sits on a single channel.
    Single,
    /// A populated DIMM carries an ambiguous (non-standard) locator, so the
    /// channel cannot be told apart (the L4 heuristic input).
    Ambiguous,
}

/// The safe default for a no-population / unknown topology is ambiguous
/// (the L4 heuristic input) — never a confident Dual or Single.
impl Default for DerivedMode {
    fn default() -> Self {
        DerivedMode::Ambiguous
    }
}

/// The SMBIOS Type 17 population snapshot the 4-tier trust procedure
/// cross-checks against the hardware MMIO decode.
///
/// Always constructible (every field has a safe empty value) so a missing
/// `dmi` directory degrades to [`SmbiosTopology::empty`] — never a panic.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmbiosTopology {
    /// Number of populated DIMMs (capacity > 0).
    pub populated_count: u8,
    /// Per-populated-DIMM capacity in MiB (parallel to [`locators`]).
    pub dimm_sizes: Vec<u64>,
    /// Total installed capacity in MiB (the sum of [`dimm_sizes`]).
    pub total_capacity_mib: u64,
    /// The active memory technology of the first populated DIMM
    /// (`0x1A` → DDR4, `0x22` → DDR5); `None` when no populated DIMM
    /// carries a recognized type.
    pub memory_type: Option<MemoryTech>,
    /// The configured memory speed in MT/s (the "Configured Memory Speed"
    /// of the first populated DIMM, MT/s directly); `None` when no
    /// populated DIMM reports a nonzero speed.
    pub speed_mts: Option<u16>,
    /// The locator strings of the populated DIMMs (board-slot designations
    /// such as `A2` / `B2` / `DIMM A` / `Bank 1`).
    pub locators: Vec<String>,
    /// The channel classification of the populated DIMMs (see
    /// [`DerivedMode`]).
    pub derived_mode: DerivedMode,
}

impl SmbiosTopology {
    /// The empty (no-population) topology: zero DIMMs, no technology, no
    /// locators, an ambiguous derived mode. The degradation target for any
    /// missing / unreadable `dmi` source.
    pub fn empty() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Chunk 4: SMBIOS SPD fallback (basic module identity for hosts without ee1004)
// ---------------------------------------------------------------------------

/// A basic SPD-like module entry derived from SMBIOS Type 17 — the
/// fallback when the `ee1004` EEPROM driver has no bound devices (common
/// on TRX40 / older AMD platforms).
///
/// Vendor-neutral: SMBIOS Type 17 is exported by the kernel for all
/// platforms (AMD, Intel, ARM), regardless of CPU vendor.
///
/// **Limitations (Chunk 4 contract):** SMBIOS does **not** carry XMP/EXPO
/// profiles, JEP106 die-maker codes, or detailed SPD timing parameters —
/// those are SPD-EEPROM-only. This entry provides basic module identity:
/// size, speed, part number, manufacturer, serial, and locator. The
/// consumer (the facade) marks the data source so the display layer can
/// render it with the "limited" annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct SmbiosModuleEntry {
    /// Module index: the DMI handle ordinal (0, 1, 2, …) in the order the
    /// entries were enumerated (sorted by file name).
    pub index: u8,
    /// Capacity in MiB.
    pub capacity_mib: u64,
    /// Active memory technology (DDR4/DDR5/etc.), if recognized.
    pub memory_type: Option<MemoryTech>,
    /// Configured memory speed in MT/s, if reported.
    pub speed_mts: Option<u16>,
    /// Device locator string (e.g. "A2", "DIMM A",
    /// "Controller0-ChannelA-DIMM0").
    pub locator: String,
    /// Manufacturer string (e.g. "Team Group Inc"), if present and not
    /// "Unknown".
    pub manufacturer: Option<String>,
    /// Serial number string, if present.
    pub serial_number: Option<String>,
    /// Asset tag string, if present.
    pub asset_tag: Option<String>,
    /// Part number string (e.g. "TEAMGROUP-UD4-3600"), if present.
    pub part_number: Option<String>,
}

/// Collect basic SPD-like module entries from SMBIOS Type 17 for all
/// populated DIMMs — the [`acquire`](crate::spd_eeprom::acquire) fallback
/// when the `ee1004` driver has no bound devices.
///
/// Returns the `(entries, status)` pair:
/// - the entries: `vec![]` when the DMI source is absent (VM/container),
///   no populated DIMM is found, or every structure is corrupt — the
///   caller renders this as "no SPD data";
/// - the [`SmbiosSourceStatus`]: *why* the list is empty (absent /
///   unreadable / corrupt), so the facade can annotate the readout.
///
/// # No-panic contract
///
/// Mirrors [`collect_from`]: a missing directory (VM / container) or any
/// read failure yields `vec![]` with the matching [`SmbiosSourceStatus`]
/// — never a panic, never an invented DIMM.
pub fn collect_module_entries() -> (Vec<SmbiosModuleEntry>, SmbiosSourceStatus) {
    collect_module_entries_from(Path::new(DMI_ENTRIES_DIR))
}

/// Single-pass collection of both the SMBIOS topology and the full
/// module-entry list from the same DMI directory read.
///
/// This avoids the double I/O (and the minor inconsistency risk of the
/// directory changing between reads) of calling [`collect_from`] and then
/// re-parsing the entries separately. Both [`collect_from`] and
/// [`collect_module_entries_from`] delegate here so they always agree on
/// the parsed data.
///
/// Returns `(topology, status, module_entries)`.
fn collect_topology_and_entries_from(
    root: &Path,
) -> (SmbiosTopology, SmbiosSourceStatus, Vec<SmbiosModuleEntry>) {
    let mut topology = SmbiosTopology::empty();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            // M-17: distinguish "absent" (VM / container / non-Intel) from
            // a permission / I/O failure.
            let status = if e.kind() == std::io::ErrorKind::NotFound {
                SmbiosSourceStatus::DirAbsent
            } else {
                SmbiosSourceStatus::Unreadable
            };
            return (topology, status, Vec::new());
        }
    };
    // M-12: the SMBIOS major version selects the Type 17 offset table
    // (3.x carries the +2 `Error Information Handle` shift; 2.x does not).
    let major = read_bios_major(root);
    let offsets = if major >= 3 {
        SMBIOS3_OFFSETS
    } else {
        SMBIOS2_OFFSETS
    };
    // Collect the Type 17 entries and sort by file name for a deterministic
    // processing order (the kernel numbers them `17-0`, `17-1`, ...;
    // `read_dir` order is filesystem-dependent, so sorting keeps the
    // locator / slot order stable across hosts and tests).
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(TYPE17_PREFIX))
        })
        .collect();
    // Sort numerically by the handle number (`17-<n>`) so that a 12-DIMM
    // system orders as 0,1,…,9,10,11 — not lexicographically as
    // 0,1,10,11,2,…,9 (which would misassign the per-slot indices).
    paths.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(TYPE17_PREFIX))
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    // M-17: a present-but-corrupt structure (or an unreadable `raw` file)
    // is a parse failure, distinct from a valid unpopulated DIMM.
    let mut saw_invalid = false;
    let mut module_entries = Vec::new();
    for (ordinal, path) in paths.into_iter().enumerate() {
        // The kernel exports each `17-<n>` as a *directory*; the
        // structure bytes live in the payload file `17-<n>/raw`.
        let bytes = match std::fs::read(path.join("raw")) {
            Ok(b) => b,
            Err(_) => {
                saw_invalid = true;
                continue; // unreadable -> skip (no panic)
            }
        };
        match parse_type17_full(&bytes, &offsets) {
            Type17Parse::Populated(parsed) => {
                // Topology data (shared with collect_from)
                topology.populated_count = topology.populated_count.saturating_add(1);
                topology.dimm_sizes.push(parsed.capacity_mib);
                if topology.memory_type.is_none() {
                    topology.memory_type = parsed.memory_type;
                }
                if topology.speed_mts.is_none() {
                    topology.speed_mts = parsed.speed_mts;
                }
                topology.locators.push(parsed.locator.clone());
                // Full module entry (the SPD fallback data)
                module_entries.push(SmbiosModuleEntry {
                    index: ordinal as u8,
                    capacity_mib: parsed.capacity_mib,
                    memory_type: parsed.memory_type,
                    speed_mts: parsed.speed_mts,
                    locator: parsed.locator,
                    // Trim whitespace and filter "Unknown" / empty strings:
                    // SMBIOS often carries "Unknown" as a placeholder for
                    // manufacturer, which we treat as absent.
                    manufacturer: parsed
                        .manufacturer
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty() && s.to_ascii_lowercase() != "unknown"),
                    serial_number: parsed
                        .serial_number
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                    asset_tag: parsed
                        .asset_tag
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                    part_number: parsed
                        .part_number
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                });
            }
            Type17Parse::Unpopulated => {} // valid structure, empty DIMM
            Type17Parse::Invalid => saw_invalid = true,
        }
    }
    // Total capacity = the sum of the per-DIMM sizes (MiB).
    topology.total_capacity_mib = topology.dimm_sizes.iter().sum::<u64>();
    // Channel grammar (M-15): the data-driven [`CHANNEL_PATTERNS`] table is
    // vendor-neutral, so no DMI board name is needed to resolve channels.
    topology.derived_mode = derive_mode(&topology.locators);
    let status = if saw_invalid {
        SmbiosSourceStatus::ParseError
    } else {
        SmbiosSourceStatus::Ok
    };
    (topology, status, module_entries)
}

/// The injectable-root [`collect_module_entries`] (the fixture tests pass
/// a temp directory holding synthetic `17-*` files).
///
/// Delegates to the single-pass [`collect_topology_and_entries_from`] so
/// the DMI directory is read exactly once (not twice as in the original
/// implementation that called [`collect_from`] then re-parsed).
pub fn collect_module_entries_from(root: &Path) -> (Vec<SmbiosModuleEntry>, SmbiosSourceStatus) {
    let (topology, status, entries) = collect_topology_and_entries_from(root);
    // If the topology is empty, there are no entries.
    if topology.populated_count == 0 {
        return (Vec::new(), status);
    }
    (entries, status)
}

/// Collect the SMBIOS Type 17 topology from the kernel's `dmi` export
/// (the frozen [`DMI_ENTRIES_DIR`]).
///
/// Returns the `(topology, status)` pair (M-17): the [`SmbiosSourceStatus`]
/// says *why* the topology is empty (absent / unreadable / corrupt) so the
/// facade can annotate the readout — not just *that* it is.
///
/// # No-panic contract
///
/// A missing directory (VM / container / non-Intel host) or any read
/// failure yields [`SmbiosTopology::empty`] with the matching
/// [`SmbiosSourceStatus`] — never a panic, never an invented DIMM.
pub fn collect() -> (SmbiosTopology, SmbiosSourceStatus) {
    collect_from(Path::new(DMI_ENTRIES_DIR))
}

/// The injectable-root [`collect`] (the fixture tests pass a temp directory
/// holding synthetic `17-*` files). Any missing / unreadable source
/// degrades to the empty topology (with the matching
/// [`SmbiosSourceStatus`], M-17).
///
/// Delegates to the single-pass [`collect_topology_and_entries_from`] so
/// the topology always agrees with the module entries (they are parsed
/// from the same directory read).
pub fn collect_from(root: &Path) -> (SmbiosTopology, SmbiosSourceStatus) {
    let (topology, status, _entries) = collect_topology_and_entries_from(root);
    (topology, status)
}

/// All fields parsed from one populated SMBIOS Type 17 structure
/// (Chunk 4: the full field set the topology and the module-entry list
/// share).
struct ParsedModule {
    /// Capacity in MiB.
    capacity_mib: u64,
    /// Memory technology, if recognized.
    memory_type: Option<MemoryTech>,
    /// Device locator string.
    locator: String,
    /// Configured memory speed in MT/s, if reported.
    speed_mts: Option<u16>,
    /// Manufacturer string, if present.
    manufacturer: Option<String>,
    /// Serial number string, if present.
    serial_number: Option<String>,
    /// Asset tag string, if present.
    asset_tag: Option<String>,
    /// Part number string, if present.
    part_number: Option<String>,
}

/// The outcome of parsing one raw Type 17 structure (M-17): a populated
/// DIMM, a valid-but-unpopulated DIMM (capacity `0` — **not** an error),
/// or a corrupt / too-short structure (a parse failure).
enum Type17Parse {
    /// A populated DIMM with all parsed fields (Chunk 4: the full
    /// [`ParsedModule`] instead of the old 4-tuple).
    Populated(ParsedModule),
    /// A valid structure whose DIMM is unpopulated (capacity `0`).
    Unpopulated,
    /// A corrupt / too-short structure (a parse failure).
    Invalid,
}

/// The full Type 17 parser (Chunk 4): extracts every field the
/// [`SmbiosModuleEntry`] needs in addition to the topology data.
///
/// # No-panic contract
///
/// Every field read is bounds-checked; a short / corrupt structure yields
/// [`Type17Parse::Invalid`] (skipped) rather than a panic.
fn parse_type17_full(bytes: &[u8], offsets: &Type17Offsets) -> Type17Parse {
    if bytes.len() < 2 {
        return Type17Parse::Invalid;
    }
    let len = (bytes[OFF_LENGTH] as usize).min(bytes.len());
    let fixed = &bytes[..len];
    let table = &bytes[len..];

    // The fixed part must carry the capacity word — a shorter structure is
    // corrupt (M-17: a parse failure, distinct from a valid unpopulated
    // DIMM).
    if fixed.len() < offsets.capacity + CAPACITY_LEN {
        return Type17Parse::Invalid;
    }

    // Capacity: the version-keyed word — per SMBIOS (DSP0134 Table 76) the
    // magnitude is set by bit 15: **clear = MB, set = KB** (`0x4000` =
    // 16384 MB = the reference 16 GiB DIMM). The sentinel words
    // `0x7FFF` (≥ 32 GiB − 1 MiB) and `0xFFFF` (unknown), like a `0`
    // word, fall through to the Extended Size dword (3.x only, MiB); both
    // zero = unpopulated DIMM.
    let word = u16::from_le_bytes([fixed[offsets.capacity], fixed[offsets.capacity + 1]]);
    let mut capacity: u64 = match word {
        // Sentinels (and "not installed") -> the extended dword.
        0x0000 | 0x7FFF | 0xFFFF => 0,
        _ => {
            let size = (word & 0x7FFF) as u64;
            if word & 0x8000 != 0 {
                size / 1024 // KB -> MiB (floor)
            } else {
                size // MB
            }
        }
    };
    if capacity == 0 {
        if let Some(ext_off) = offsets.capacity_extended {
            if fixed.len() >= ext_off + 4 {
                let mut buf = [0u8; 4];
                buf.copy_from_slice(&fixed[ext_off..ext_off + 4]);
                capacity = u32::from_le_bytes(buf) as u64; // MiB (bits 30:0)
            }
        }
    }
    if capacity == 0 {
        return Type17Parse::Unpopulated;
    }

    // Memory type (version-keyed; 0x1A = DDR4, 0x22 = DDR5).
    let mem_type = if fixed.len() > offsets.memory_type {
        MemoryTech::from_smbios_type(fixed[offsets.memory_type])
    } else {
        None
    };

    // Locator: the Memory Device Locator string (version-keyed), falling
    // back to the Bank Locator string when the former is empty / absent.
    let mem_ref = if fixed.len() > offsets.mem_device_locator {
        fixed[offsets.mem_device_locator]
    } else {
        0
    };
    let bank_ref = if fixed.len() > offsets.bank_locator {
        fixed[offsets.bank_locator]
    } else {
        0
    };
    let locator = string_at(table, mem_ref)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| string_at(table, bank_ref))
        .unwrap_or_default();

    // Configured Memory Speed @ `0x20` (16-bit LE, MT/s directly; `0` =
    // unknown per the SMBIOS spec).
    let speed = if fixed.len() >= OFF_SPEED + SPEED_LEN {
        let word = u16::from_le_bytes([fixed[OFF_SPEED], fixed[OFF_SPEED + 1]]);
        if word == 0 {
            None
        } else {
            Some(word)
        }
    } else {
        None
    };

    // Chunk 4: the new string-reference fields (manufacturer, serial
    // number, asset tag, part number). Each is bounds-checked; an absent
    // field yields None (never a panic, never an invented value).
    let str_at = |off: usize| -> Option<String> {
        if fixed.len() > off {
            string_at(table, fixed[off])
        } else {
            None
        }
    };
    let manufacturer = str_at(offsets.manufacturer);
    let serial_number = str_at(offsets.serial_number);
    let asset_tag = str_at(offsets.asset_tag);
    let part_number = str_at(offsets.part_number);

    Type17Parse::Populated(ParsedModule {
        capacity_mib: capacity,
        memory_type: mem_type,
        locator,
        speed_mts: speed,
        manufacturer,
        serial_number,
        asset_tag,
        part_number,
    })
}

/// Extract the `ref_idx`-th string from a raw SMBIOS string table
/// (null-separated). Real SMBIOS string tables start at the **first**
/// string — there is **no** leading empty segment — so string ref `N`
/// is the `N`th null-separated segment (`ref 1` = the first).
///
/// # No-panic contract
///
/// An out-of-range reference (or `0`) yields `None` — never a panic, never
/// an out-of-bounds read.
fn string_at(table: &[u8], ref_idx: u8) -> Option<String> {
    if ref_idx == 0 {
        return None;
    }
    let parts: Vec<&[u8]> = table.split(|&b| b == 0).collect();
    // String ref `N` = the `N`th segment (no leading empty segment in a
    // real table); the trailing null terminator yields an empty segment,
    // which is not a string.
    parts
        .get(ref_idx as usize - 1)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|s| !s.is_empty())
}

/// Map a channel index (`0`–`3`) to its `A`–`D` letter designation.
fn channel_char(index: u8) -> char {
    char::from(b'A' + index.min(3))
}

/// The locator's alphanumeric words, lowercased, with every
/// non-alphanumeric run collapsed (e.g. `CPU0_DIMM_A` →
/// `["cpu0", "dimm", "a"]`). This is the normalization the data-driven
/// [`CHANNEL_PATTERNS`] matching runs over.
fn locator_words(locator: &str) -> Vec<String> {
    locator
        .to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The first [`CHANNEL_PATTERNS`] entry that matches the locator's words,
/// or `None`. A single-word pattern matches any equal word; a multi-word
/// pattern (e.g. `dimm 1`) matches consecutive equal words. First match
/// wins (the table order is the precedence order).
fn match_channel_pattern(words: &[String]) -> Option<u8> {
    for (pattern, index) in CHANNEL_PATTERNS {
        let pwords: Vec<&str> = pattern.split_whitespace().collect();
        let matched = if pwords.len() == 1 {
            words.iter().any(|w| w.as_str() == pwords[0])
        } else {
            words
                .windows(pwords.len())
                .any(|win| win.iter().enumerate().all(|(j, w)| w.as_str() == pwords[j]))
        };
        if matched {
            return Some(*index);
        }
    }
    None
}

/// The channel designation (A–D) of a locator string, if one is present
/// (the **vendor-neutral** general grammar — M-15).
///
/// Two layers, in order:
/// 1. The data-driven [`CHANNEL_PATTERNS`] table — standalone letters
///    (`A`–`D`), OEM `P1`–`P4`, `CH0`–`CH3`, `Slot A`–`D`, `DIMM A`–`D`,
///    `DIMM 1`–`4`, and the per-firmware `Controller0`–`3` designators.
/// 2. The legacy **letter+digit** rule — an A–D letter immediately
///    followed by one or more digits (`A1`, `B2`, …), where the letter is
///    the channel and the digit is the slot.
///
/// A locator with no recognized designation (e.g. `Bank 1`) yields `None`
/// (→ [`DerivedMode::Ambiguous`]). Matching is case-insensitive and
/// token-based, so an A–D letter that merely *starts* a longer word (the
/// `D` in `DIMM`) is never mistaken for a channel.
fn channel_of(locator: &str) -> Option<char> {
    let words = locator_words(locator);
    if let Some(index) = match_channel_pattern(&words) {
        return Some(channel_char(index));
    }
    for w in &words {
        let b = w.as_bytes();
        if b.len() >= 2 && matches!(b[0], b'a'..=b'd') && b[1..].iter().all(|c| c.is_ascii_digit())
        {
            return Some(channel_char(b[0] - b'a'));
        }
    }
    None
}

/// Classify the populated DIMM locators into a [`DerivedMode`]:
/// any ambiguous locator (no channel designation) → `Ambiguous`; ≥ 2
/// distinct channels → `Dual`; a single channel → `Single`.
fn derive_mode(locators: &[String]) -> DerivedMode {
    if locators.is_empty() {
        return DerivedMode::Ambiguous;
    }
    let mut channels = HashSet::new();
    for loc in locators {
        match channel_of(loc) {
            Some(c) => {
                channels.insert(c);
            }
            None => return DerivedMode::Ambiguous,
        }
    }
    match channels.len() {
        0 => DerivedMode::Ambiguous,
        1 => DerivedMode::Single,
        _ => DerivedMode::Dual,
    }
}

/// The SMBIOS major version of the DMI source (M-12), read from the
/// `id/bios_major` file (a sibling of the `entries` directory that `root`
/// points to). The canonical path is `/sys/firmware/dmi/id/bios_major`.
/// Absent / unreadable / non-numeric → the modern default `3` (the SMBIOS
/// 3.x layout — the reference board is 3.5); a real 2.x BIOS reports `2`
/// and selects the 2.x offset table.
fn read_bios_major(root: &Path) -> u8 {
    let base = root.parent();
    let text = base.and_then(|b| std::fs::read_to_string(b.join("id").join("bios_major")).ok());
    text.and_then(|t| t.trim().parse().ok()).unwrap_or(3)
}

// ---------------------------------------------------------------------------
// AGESA version from SMBIOS Type 40 ("Additional Information")
// ---------------------------------------------------------------------------

/// The file-name prefix of a raw SMBIOS Type 40 structure (`40-<n>`).
const TYPE40_PREFIX: &str = "40-";

/// The SMBIOS Type 40 structure type byte (0x28 = 40).
const TYPE40_TYPE: u8 = 0x28;

/// Parsed AGESA version information from SMBIOS Type 40.
///
/// SMBIOS Type 40 ("Additional Information", DMTF DSP0134 v3.8.0 §7.41)
/// is used by AMD OEMs (e.g. Gigabyte) to annotate the BIOS version
/// field with the AGESA firmware version. The entry references Type 0
/// (BIOS Information) at offset 0x05 (Firmware Version field) and
/// carries the AGESA string in its string table.
///
/// The AGESA string format: `AGESA!<Line> <Project> <Version>` (e.g.
/// `AGESA!V9 ComboAm5PI 1.2.8.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgesaInfo {
    /// Full AGESA string as found in the SMBIOS Type 40 entry
    /// (e.g., "AGESA!V9 ComboAm5PI 1.2.8.0").
    pub raw: String,
    /// AGESA line (e.g., "V9").
    pub line: Option<String>,
    /// PI project name (e.g., "ComboAm5PI").
    pub project: Option<String>,
    /// Version number (e.g., "1.2.8.0").
    pub version: Option<String>,
}

impl AgesaInfo {
    /// Format the AGESA info for display (e.g., "V9 ComboAm5PI 1.2.8.0").
    ///
    /// Joins the available fields (line, project, version) with spaces.
    /// Returns `None` when no useful fields are present (e.g. a bare
    /// "AGESA!" marker with no version content).
    pub fn display_string(&self) -> Option<String> {
        let parts: Vec<String> = vec![
            self.line.clone(),
            self.project.clone(),
            self.version.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

/// Read AGESA version from SMBIOS Type 40 entries.
///
/// SMBIOS Type 40 ("Additional Information") is used by AMD/Gigabyte to
/// annotate the BIOS version field with the AGESA firmware version.
/// The entry references Type 0 (BIOS Information) at offset 0x05
/// (Firmware Version field) and carries the AGESA string.
///
/// Multiple Type 40 entries are legal; they are scanned in handle-number
/// order and the first one carrying a valid AGESA string is returned.
///
/// Source: DMTF DSP0134 v3.8.0 §7.41, verified on Gigabyte B850 AI TOP.
///
/// # No-panic contract
///
/// A missing `dmi` directory (VM / container), an unreadable `raw` file,
/// or a corrupt Type 40 structure all yield `None` — never a panic, never
/// an invented AGESA version.
pub fn read_agesa_from_smbios() -> Option<AgesaInfo> {
    read_agesa_from(Path::new(DMI_ENTRIES_DIR))
}

/// The injectable-root version of [`read_agesa_from_smbios`] (the fixture
/// tests pass a temp directory holding synthetic `40-*` files).
fn read_agesa_from(root: &Path) -> Option<AgesaInfo> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return None,
    };
    // Collect the Type 40 entries and sort numerically by handle number
    // (`40-0`, `40-1`, …; numeric so `40-2` precedes `40-10`).
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(TYPE40_PREFIX))
        })
        .collect();
    paths.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(TYPE40_PREFIX))
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    // Try each Type 40 entry in order; return the first that yields an
    // AGESA string.
    for path in paths {
        let bytes = match std::fs::read(path.join("raw")) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if let Some(agesa) = parse_type40_entry(&bytes) {
            return Some(agesa);
        }
    }
    None
}

/// Parse one SMBIOS Type 40 ("Additional Information") structure and
/// extract the AGESA version if present.
///
/// Type 40 layout (DMTF DSP0134 v3.8.0 §7.41):
/// - Offset 0: Type (0x28)
/// - Offset 1: Length (formatted area size in bytes)
/// - Offset 2-3: Handle (16-bit LE)
/// - Offset 4: NumEntries
/// - Offset 5+: Entries (variable length, each:
///   - Length (1 byte, includes this byte)
///   - RefHandle (2 bytes LE — the referenced structure's handle)
///   - RefOffset (1 byte — offset into the referenced structure)
///   - StringRef (1 byte — index into this Type 40's string table)
///   - Value (variable — Length - 5 bytes)
/// - After the formatted area (at offset `Length`): the string table
///   (null-separated, double-null terminated).
///
/// # No-panic contract
///
/// Every read is bounds-checked; a short / corrupt structure yields
/// `None` rather than a panic.
fn parse_type40_entry(raw: &[u8]) -> Option<AgesaInfo> {
    // Validate minimum size: type (1) + length (1) + handle (2) +
    // num_entries (1) = 5 bytes.
    if raw.len() < 5 {
        return None;
    }
    // Validate type byte (0x28 = Type 40).
    if raw[0] != TYPE40_TYPE {
        return None;
    }
    let length = raw[1] as usize;
    // The formatted area must be at least 5 bytes (the header fields)
    // and must not exceed the total raw size.
    if length < 5 || length > raw.len() {
        return None;
    }
    let num_entries = raw[4] as usize;
    // The string table starts immediately after the formatted area.
    let table = &raw[length..];

    // Walk the variable-length entries.
    let mut pos = 5;
    for _ in 0..num_entries {
        if pos >= length {
            break;
        }
        let entry_len = raw[pos] as usize;
        // Each entry needs at least 5 bytes: length (1) + ref_handle (2)
        // + ref_offset (1) + str_ref (1).
        if entry_len < 5 || pos + entry_len > length {
            break;
        }
        let str_ref = raw[pos + 4];

        // If str_ref > 0, look up the string in the table and try to
        // parse it as an AGESA string.
        if str_ref > 0 {
            if let Some(s) = string_at(table, str_ref) {
                if let Some(agesa) = parse_agesa_string(&s) {
                    return Some(agesa);
                }
            }
        }
        pos += entry_len;
    }
    None
}

/// Parse an AGESA string in the format `AGESA!<Line> <Project> <Version>`
/// or `AGESA <Line> <Project> <Version>`.
///
/// The "AGESA" prefix is case-insensitive. After the prefix, an optional
/// "!" separator is consumed, then the remaining whitespace-delimited
/// tokens are parsed as:
/// - First token: the AGESA line (e.g., "V9")
/// - Second token: the PI project name (e.g., "ComboAm5PI")
/// - Last token: the version number (e.g., "1.2.8.0")
///
/// The version is identified by containing a dot (dotted version
/// notation). A token without a dot is not treated as a version:
/// - "AGESA!V9" → line="V9", project=None, version=None
/// - "AGESA!V9 1.2.3.4" → line="V9", project=None, version="1.2.3.4"
/// - "AGESA!V9 ComboAm5PI" → line="V9", project="ComboAm5PI", version=None
/// - "AGESA!V9 ComboAm5PI 1.2.8.0" → all three fields populated
///
/// A string with just the "AGESA" prefix (and optional "!") and no
/// content tokens is accepted and yields `Some(AgesaInfo)` with all
/// sub-fields `None`.
///
/// Returns `None` for strings that do not start with "AGESA".
fn parse_agesa_string(s: &str) -> Option<AgesaInfo> {
    let upper = s.to_uppercase();
    if !upper.starts_with("AGESA") {
        return None;
    }

    // After "AGESA", strip an optional "!" and leading whitespace.
    let rest = s.get(5..)?.trim_start();
    let rest = rest.strip_prefix('!').unwrap_or(rest).trim_start();

    // Split into whitespace-delimited tokens.
    let tokens: Vec<&str> = rest.split_whitespace().collect();

    if tokens.is_empty() {
        // Just "AGESA!" or "AGESA" with no content.
        return Some(AgesaInfo {
            raw: s.to_string(),
            line: None,
            project: None,
            version: None,
        });
    }

    // A token is a version candidate only if it contains a dot
    // (dotted version notation like "1.2.8.0"; "V9" is not a version).
    let is_version = |t: &&str| t.contains('.');

    let (line, project, version) = match tokens.len() {
        1 => {
            let t = tokens[0];
            if is_version(&t) {
                (None, None, Some(t.to_string()))
            } else {
                (Some(t.to_string()), None, None)
            }
        }
        2 => {
            let (t0, t1) = (tokens[0], tokens[1]);
            if is_version(&t1) {
                // line + version (no project)
                (Some(t0.to_string()), None, Some(t1.to_string()))
            } else if is_version(&t0) {
                // version + other (unlikely, but handle it)
                (None, Some(t1.to_string()), Some(t0.to_string()))
            } else {
                // line + project (no version)
                (Some(t0.to_string()), Some(t1.to_string()), None)
            }
        }
        _ => {
            // 3+ tokens: line, project, ..., version (last dotted token)
            let line = Some(tokens[0].to_string());
            let project = Some(tokens[1].to_string());
            let version = tokens
                .last()
                .filter(|t| is_version(t))
                .map(|t| t.to_string());
            (line, project, version)
        }
    };

    Some(AgesaInfo {
        raw: s.to_string(),
        line,
        project,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a raw SMBIOS Type 17 structure: a `len`-byte fixed part (the
    /// caller sets the field bytes) + a null-separated string table
    /// (string ref `N` = the `N`th string — **no** leading empty segment,
    /// exactly like the kernel's real tables). Returns the full image the
    /// kernel exports at `17-<n>/raw`.
    fn type17_file(len: usize, fixed: &mut [u8], strings: &[&str]) -> Vec<u8> {
        assert!(fixed.len() >= len, "fixed must be at least `len` bytes");
        fixed[OFF_LENGTH] = len as u8;
        let mut out = fixed.to_vec();
        // String table: one string per reference + its null terminator
        // (real tables start at the first string — no leading empty
        // segment).
        for s in strings {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out
    }

    /// Write a synthetic `17-<n>` entry into `dir`, mirroring the
    /// kernel's dmi-sysfs layout: a **directory** `17-<n>` whose payload
    /// file is `17-<n>/raw`.
    fn write_entry(dir: &Path, n: u8, bytes: &[u8]) {
        let entry = dir.join(format!("17-{n}"));
        std::fs::create_dir_all(&entry).expect("entry dir create");
        let mut f = std::fs::File::create(entry.join("raw")).expect("entry raw create");
        f.write_all(bytes).expect("entry raw write");
    }

    /// A temp DMI tree (pid-scoped; removed on drop). `root` is the
    /// `entries` directory (holding the `17-*` structures).
    struct TempDmi {
        root: std::path::PathBuf,
    }

    impl TempDmi {
        fn new(test: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("ramsleuth-dmi-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("entries")).expect("dmi entries dir");
            Self {
                root: base.join("entries"),
            }
        }
        /// Write the DMI `id/bios_major` (a sibling of `root`) — the
        /// version-aware offset-table selector (M-12).
        fn write_bios_major(&self, major: u8) {
            let id = self.root.parent().expect("base dir").join("id");
            std::fs::create_dir_all(&id).expect("dmi id dir");
            std::fs::write(id.join("bios_major"), major.to_string()).expect("bios_major write");
        }
    }

    impl Drop for TempDmi {
        fn drop(&mut self) {
            if let Some(base) = self.root.parent() {
                let _ = std::fs::remove_dir_all(base);
            }
        }
    }

    /// A populated DDR4 DIMM, 16 GiB, in the given slot. Real 3.5 layout:
    /// the `0x0C` capacity word carries `0x4000` (bit 15 clear = MB unit,
    /// 16384 MB — the same word the reference table uses), the locator
    /// string refs are `0x10` (Device Locator) / `0x11` (Bank Locator),
    /// the memory type is `0x12` (`0x1A` = DDR4). String table: ref 1 =
    /// the device-locator value, ref 2 = the bank-locator value (the
    /// same locator in both, as on the reference board).
    fn populated_ddr4(locator: &str) -> Vec<u8> {
        let o = SMBIOS3_OFFSETS;
        let mut fixed = [0u8; 0x20];
        fixed[o.capacity..o.capacity + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        fixed[o.memory_type] = 0x1A; // DDR4
        type17_file(0x20, &mut fixed, &[locator, locator])
    }

    /// An unpopulated DIMM (the `0x0C` word and the `0x1C` extended dword
    /// are both zero) — must be skipped.
    fn unpopulated() -> Vec<u8> {
        let o = SMBIOS3_OFFSETS;
        let mut fixed = [0u8; 0x20];
        fixed[o.memory_type] = 0x1A;
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        // capacity (0x0C word + 0x1C dword) stays 0
        type17_file(0x20, &mut fixed, &["A1", "A1"])
    }

    /// A populated DDR4 DIMM with the Configured Memory Speed field
    /// (@ `0x20`, 16-bit LE, MT/s directly) populated — the fixed part is
    /// extended to `0x22` to carry it (the `populated_ddr4` helper stops at
    /// `0x20`, so the speed there is absent / `None`).
    fn populated_ddr4_speed(locator: &str, speed: u16) -> Vec<u8> {
        let o = SMBIOS3_OFFSETS;
        let mut fixed = [0u8; 0x22];
        fixed[o.capacity..o.capacity + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        fixed[o.memory_type] = 0x1A; // DDR4
        fixed[OFF_SPEED..OFF_SPEED + SPEED_LEN].copy_from_slice(&speed.to_le_bytes());
        type17_file(0x22, &mut fixed, &[locator, locator])
    }

    /// The verbatim 12600KF / Z690M-ITX/ax SMBIOS Type 17 table
    /// (`17-0/raw`, handle `0x000C`; 183 bytes = the 92-byte SMBIOS 3.5
    /// fixed part + the string region). Captured byte-for-byte from the
    /// reference host's `/sys/firmware/dmi/entries` (evidence:
    /// `work/scratch/DeepSeek Report.md:28-41`).
    const REAL_12600KF_17_0: &[u8] = &[
        0x11, 0x5C, 0x0C, 0x00, 0x0B, 0x00, 0xFE, 0xFF, 0x40, 0x00, 0x40, 0x00, 0x00, 0x40, 0x09,
        0x00, 0x01, 0x02, 0x1A, 0x80, 0x00, 0x60, 0x09, 0x03, 0x04, 0x05, 0x06, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x10, 0x0E, 0xB0, 0x04, 0x46, 0x05, 0xB0, 0x04, 0x03, 0x08, 0x00, 0x00, 0x04,
        0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x43, 0x6F, 0x6E, 0x74, 0x72, 0x6F, 0x6C, 0x6C, 0x65, 0x72, 0x30, 0x2D, 0x43,
        0x68, 0x61, 0x6E, 0x6E, 0x65, 0x6C, 0x41, 0x2D, 0x44, 0x49, 0x4D, 0x4D, 0x30, 0x00, 0x42,
        0x41, 0x4E, 0x4B, 0x20, 0x30, 0x00, 0x54, 0x65, 0x61, 0x6D, 0x20, 0x47, 0x72, 0x6F, 0x75,
        0x70, 0x20, 0x49, 0x6E, 0x63, 0x00, 0x30, 0x31, 0x30, 0x34, 0x44, 0x38, 0x31, 0x37, 0x00,
        0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30, 0x00, 0x54, 0x45, 0x41, 0x4D,
        0x47, 0x52, 0x4F, 0x55, 0x50, 0x2D, 0x55, 0x44, 0x34, 0x2D, 0x33, 0x36, 0x30, 0x30, 0x20,
        0x20, 0x00, 0x00,
    ];

    /// The same real table for handle `0x000D` (`17-1/raw`): the
    /// `Controller1` locator + serial `0104D816` (evidence: `work/scratch/
    /// DeepSeek Report.md:43-56`).
    const REAL_12600KF_17_1: &[u8] = &[
        0x11, 0x5C, 0x0D, 0x00, 0x0B, 0x00, 0xFE, 0xFF, 0x40, 0x00, 0x40, 0x00, 0x00, 0x40, 0x09,
        0x00, 0x01, 0x02, 0x1A, 0x80, 0x00, 0x60, 0x09, 0x03, 0x04, 0x05, 0x06, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x10, 0x0E, 0xB0, 0x04, 0x46, 0x05, 0xB0, 0x04, 0x03, 0x08, 0x00, 0x00, 0x04,
        0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x43, 0x6F, 0x6E, 0x74, 0x72, 0x6F, 0x6C, 0x6C, 0x65, 0x72, 0x31, 0x2D, 0x43,
        0x68, 0x61, 0x6E, 0x6E, 0x65, 0x6C, 0x41, 0x2D, 0x44, 0x49, 0x4D, 0x4D, 0x30, 0x00, 0x42,
        0x41, 0x4E, 0x4B, 0x20, 0x30, 0x00, 0x54, 0x65, 0x61, 0x6D, 0x20, 0x47, 0x72, 0x6F, 0x75,
        0x70, 0x20, 0x49, 0x6E, 0x63, 0x00, 0x30, 0x31, 0x30, 0x34, 0x44, 0x38, 0x31, 0x36, 0x00,
        0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30, 0x00, 0x54, 0x45, 0x41, 0x4D,
        0x47, 0x52, 0x4F, 0x55, 0x50, 0x2D, 0x55, 0x44, 0x34, 0x2D, 0x33, 0x36, 0x30, 0x30, 0x20,
        0x20, 0x00, 0x00,
    ];

    /// (a) Two populated DDR4 DIMMs in matched slots `A2` + `B2` → the
    /// topology reports 2 DIMMs, both 16 GiB, DDR4, and a `Dual` derived
    /// mode (the standard mirrored daisy-chain pair).
    #[test]
    fn two_populated_across_channels_is_dual() {
        let dmi = TempDmi::new("dual");
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        write_entry(&dmi.root, 1, &populated_ddr4("B2"));

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.dimm_sizes, vec![16384, 16384]);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr4));
        assert_eq!(t.locators, vec!["A2".to_owned(), "B2".to_owned()]);
        assert_eq!(t.derived_mode, DerivedMode::Dual);
    }

    /// (b) A single populated DIMM → `Single` derived mode, count 1.
    #[test]
    fn one_populated_is_single() {
        let dmi = TempDmi::new("single");
        write_entry(&dmi.root, 0, &populated_ddr4("A1"));

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 1);
        assert_eq!(t.dimm_sizes, vec![16384]);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr4));
        assert_eq!(t.locators, vec!["A1".to_owned()]);
        assert_eq!(t.derived_mode, DerivedMode::Single);
    }

    /// (c) Two populated DIMMs on the *same* channel (`A1` + `A2`) →
    /// `Single` derived mode (both on one channel — the "multiple DIMMs on
    /// identical channel" case).
    #[test]
    fn two_populated_same_channel_is_single() {
        let dmi = TempDmi::new("same-channel");
        write_entry(&dmi.root, 0, &populated_ddr4("A1"));
        write_entry(&dmi.root, 1, &populated_ddr4("A2"));

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.derived_mode, DerivedMode::Single);
    }

    /// (d) Two populated DIMMs with locators that carry **no** channel
    /// designation (`Bank 1` / `Bank 2` — a bank is not a channel in the
    /// [`CHANNEL_PATTERNS`] table) → `Ambiguous` derived mode (the L4
    /// heuristic input).
    #[test]
    fn ambiguous_locators_is_ambiguous() {
        let dmi = TempDmi::new("ambiguous");
        write_entry(&dmi.root, 0, &populated_ddr4("Bank 1"));
        write_entry(&dmi.root, 1, &populated_ddr4("Bank 2"));

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.locators, vec!["Bank 1".to_owned(), "Bank 2".to_owned()]);
        assert_eq!(t.derived_mode, DerivedMode::Ambiguous);
    }

    /// (e) An empty directory (no `17-*` files) → the empty topology (no
    /// panic, no invented DIMM).
    #[test]
    fn empty_dir_is_empty_topology() {
        let dmi = TempDmi::new("empty");
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (f) A missing directory (the VM / non-Intel-host case) → the empty
    /// topology (no panic).
    #[test]
    fn missing_dir_is_empty_topology() {
        let (t, _status) = collect_from(Path::new("/nonexistent/dmi/entries-xyz"));
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (g) Unpopulated DIMMs (capacity 0) are skipped: a mix of one
    /// populated + two unpopulated entries reports exactly 1 DIMM.
    #[test]
    fn unpopulated_entries_are_skipped() {
        let dmi = TempDmi::new("unpopulated");
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        write_entry(&dmi.root, 1, &unpopulated());
        write_entry(&dmi.root, 2, &unpopulated());

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 1);
        assert_eq!(t.dimm_sizes, vec![16384]);
        assert_eq!(t.locators, vec!["A2".to_owned()]);
    }

    /// (h) A DDR5 memory-type field (`0x22`) decodes to [`MemoryTech::Ddr5`].
    #[test]
    fn ddr5_memory_type_decodes() {
        let o = SMBIOS3_OFFSETS;
        let mut fixed = [0u8; 0x20];
        fixed[o.memory_type] = 0x22; // DDR5
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        fixed[o.capacity..o.capacity + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        let bytes = type17_file(0x20, &mut fixed, &["A2", "A2"]);

        let dmi = TempDmi::new("ddr5");
        write_entry(&dmi.root, 0, &bytes);
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr5));
    }

    /// The Configured Memory Speed field (@ `0x20`, 16-bit LE) is parsed as
    /// **MT/s directly** (the x99 Broadwell-E `0x0960` = 2400; a matched kit
    /// reports the same speed on every DIMM, so the first populated one
    /// wins). The total capacity is the sum of the per-DIMM sizes.
    #[test]
    fn speed_field_is_parsed_as_mts_directly() {
        let dmi = TempDmi::new("speed");
        write_entry(&dmi.root, 0, &populated_ddr4_speed("A2", 2400));
        write_entry(&dmi.root, 1, &populated_ddr4_speed("B2", 2400));

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(
            t.speed_mts,
            Some(2400),
            "the configured speed is MT/s directly"
        );
        assert_eq!(t.total_capacity_mib, 32768, "2 x 16384 MiB");
        assert_eq!(t.populated_count, 2);
    }

    /// A `0` speed word (SMBIOS "unknown") and a fixed part that stops
    /// before `0x20` both yield `speed_mts = None` — never an invented
    /// speed. The total capacity still sums the populated DIMMs.
    #[test]
    fn absent_speed_is_none() {
        // (a) a fixed part that ends before the speed field.
        let dmi = TempDmi::new("speed-absent");
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.speed_mts, None, "no speed field -> None");
        assert_eq!(t.total_capacity_mib, 16384);

        // (b) a present-but-zero speed word.
        let dmi = TempDmi::new("speed-zero");
        write_entry(&dmi.root, 0, &populated_ddr4_speed("A2", 0));
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.speed_mts, None, "a 0 speed word is 'unknown' -> None");
        assert_eq!(t.total_capacity_mib, 16384);
    }

    /// (i) A corrupt / too-short structure is skipped (no panic) — a
    /// one-byte file yields the empty topology.
    #[test]
    fn short_structure_is_skipped() {
        let dmi = TempDmi::new("short");
        write_entry(&dmi.root, 0, &[0x11]); // one byte (type only, no length)
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (j) The general `channel_of` grammar (M-15): a standalone A–D
    /// letter, an A–D letter followed by digits, and the data-driven
    /// [`CHANNEL_PATTERNS`] entries are channel designations; an A–D letter
    /// that merely *starts* a longer word (`DIMM`, `RAM`, `Bank`) is not.
    #[test]
    fn channel_of_recognizes_designations_only() {
        // Standalone letters and the legacy letter+digit rule.
        assert_eq!(channel_of("A"), Some('A'));
        assert_eq!(channel_of("A2"), Some('A'));
        assert_eq!(channel_of("B12"), Some('B'));
        assert_eq!(channel_of("DIMM A1"), Some('A'));
        assert_eq!(channel_of("CPU0_DIMM_B"), Some('B'));
        assert_eq!(channel_of("Channel A Slot 1"), Some('A'));
        // The data-driven table entries.
        assert_eq!(channel_of("P1"), Some('A'));
        assert_eq!(channel_of("P4"), Some('D'));
        assert_eq!(channel_of("CH0"), Some('A'));
        assert_eq!(channel_of("CH3"), Some('D'));
        assert_eq!(channel_of("Slot A"), Some('A'));
        assert_eq!(channel_of("Slot D"), Some('D'));
        assert_eq!(channel_of("DIMM A"), Some('A'));
        assert_eq!(channel_of("DIMM 1"), Some('A'));
        assert_eq!(channel_of("DIMM 4"), Some('D'));
        assert_eq!(channel_of("Controller0-ChannelA-DIMM0"), Some('A'));
        assert_eq!(channel_of("Controller1-ChannelA-DIMM0"), Some('B'));
        // No channel designation: an A–D letter that starts a word is not
        // one, and a bare bank / slot number is not a channel.
        assert_eq!(channel_of("Bank 1"), None);
        assert_eq!(channel_of("Slot 1"), None);
        assert_eq!(channel_of("RAM"), None);
        assert_eq!(channel_of(""), None);
    }

    /// (k) `string_at` extracts the right null-separated string — real
    /// tables start at the first string (ref 1 = the first segment, no
    /// leading empty segment) — and is safe on an out-of-range
    /// reference (no panic).
    #[test]
    fn string_at_extracts_and_bounds() {
        let table = [b'A', b'2', 0, b'B', b'2', 0, 0];
        assert_eq!(string_at(&table, 1), Some("A2".to_owned()));
        assert_eq!(string_at(&table, 2), Some("B2".to_owned()));
        assert_eq!(string_at(&table, 0), None);
        assert_eq!(string_at(&table, 3), None);
        assert_eq!(string_at(&table, 99), None);
    }

    /// The real 12600KF / Z690M-ITX/ax table (SMBIOS 3.5, 92-byte fixed
    /// part, `17-<n>/raw`): both entries parse to 2 × 16 GiB DDR4 DIMMs
    /// whose `ControllerN` locators derive `Dual` from the **general**
    /// [`CHANNEL_PATTERNS`] table (M-15 — the per-firmware
    /// `Controller<digit>` grammar is now vendor-neutral in the table, so no
    /// DMI board name is needed) — the exact topology the L3 firmware
    /// override consumes.
    #[test]
    fn real_12600kf_table_reports_dual() {
        let dmi = TempDmi::new("real-12600kf");
        write_entry(&dmi.root, 0, REAL_12600KF_17_0);
        write_entry(&dmi.root, 1, REAL_12600KF_17_1);

        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.dimm_sizes, vec![16384, 16384]);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr4));
        assert_eq!(
            t.locators,
            vec![
                "Controller0-ChannelA-DIMM0".to_owned(),
                "Controller1-ChannelA-DIMM0".to_owned()
            ]
        );
        assert_eq!(t.derived_mode, DerivedMode::Dual);
        // The real table's Configured Memory Speed @ `0x20` is `0x0E10`
        // = 3600 MT/s (the 12600KF reference kit); the total capacity is
        // the sum of the two 16 GiB DIMMs.
        assert_eq!(t.speed_mts, Some(3600));
        assert_eq!(t.total_capacity_mib, 32768);
    }

    /// (M-15) The per-firmware `Controller<digit>` grammar is now in the
    /// **general** [`CHANNEL_PATTERNS`] table (vendor-neutral), superseding
    /// the old board-keyed special case: a `…Controller0…` /
    /// `…Controller1…` locator pair resolves to `Dual` directly, with no
    /// DMI board name required.
    #[test]
    fn m15_controller_grammar_is_general() {
        // The `Controller<digit>` designators resolve to their channel in
        // the general path.
        assert_eq!(channel_of("Controller0-ChannelA-DIMM0"), Some('A'));
        assert_eq!(channel_of("Controller1-ChannelA-DIMM0"), Some('B'));
        assert_eq!(channel_of("Controller2-ChannelA-DIMM0"), Some('C'));
        assert_eq!(channel_of("Controller3-ChannelA-DIMM0"), Some('D'));

        // The matched `Controller0` / `Controller1` pair → `Dual`.
        assert_eq!(
            derive_mode(&[
                "Controller0-ChannelA-DIMM0".to_owned(),
                "Controller1-ChannelA-DIMM0".to_owned(),
            ]),
            DerivedMode::Dual
        );

        // End to end through `collect_from`: the real Z690M-ITX/ax table
        // derives `Dual` with no board name (M-15: the `Controller<digit>`
        // grammar is vendor-neutral in the general table).
        let dmi = TempDmi::new("m15-keyed");
        write_entry(&dmi.root, 0, REAL_12600KF_17_0);
        write_entry(&dmi.root, 1, REAL_12600KF_17_1);
        let (t, _status) = collect_from(&dmi.root);
        assert_eq!(t.derived_mode, DerivedMode::Dual);
    }

    /// A populated DDR4 DIMM in the classic **SMBIOS 2.x** layout (M-12):
    /// the capacity word is at `0x0A` (not `0x0C`), the memory type at
    /// `0x0B`, and the locators at `0x08`/`0x09` — the 3.x values shifted
    /// -2 (no `Error Information Handle`). `16384` MB = 16 GiB.
    fn populated_ddr4_2x(locator: &str) -> Vec<u8> {
        let o = SMBIOS2_OFFSETS;
        let mut fixed = [0u8; 0x20];
        fixed[o.capacity..o.capacity + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        fixed[o.memory_type] = 0x1A; // DDR4
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        type17_file(0x20, &mut fixed, &[locator, locator])
    }

    /// (M-12) A SMBIOS 2.6 layout (the classic table, no +2 `Error
    /// Information Handle` shift): two populated DDR4 DIMMs in matched
    /// slots `A2` + `B2` parse to 2 × 16 GiB DDR4 and a `Dual` derived
    /// mode — selected by the `id/bios_major = 2` file. This pins the
    /// version-aware offset table (the 3.x fixtures above pin the other).
    #[test]
    fn smbios_2_6_layout_parses_with_the_2x_table() {
        let dmi = TempDmi::new("smbios-2-6");
        // M-12: the version file selects the 2.x offset table.
        dmi.write_bios_major(2);
        write_entry(&dmi.root, 0, &populated_ddr4_2x("A2"));
        write_entry(&dmi.root, 1, &populated_ddr4_2x("B2"));

        let (t, status) = collect_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.dimm_sizes, vec![16384, 16384]);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr4));
        assert_eq!(t.locators, vec!["A2".to_owned(), "B2".to_owned()]);
        assert_eq!(t.derived_mode, DerivedMode::Dual);
    }

    /// (M-12) The same 2.x-layout DIMM, when no `bios_major` file is
    /// present, defaults to the 3.x table and the 2.x offsets do NOT line
    /// up — a guard that the version file is what selects the layout (the
    /// default is the modern 3.x, matching the reference board).
    #[test]
    fn missing_bios_major_defaults_to_the_3x_table() {
        let dmi = TempDmi::new("smbios-default-3x");
        // A 3.x-layout DIMM parses cleanly with the default (no version
        // file → 3.x).
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        let (t, status) = collect_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(t.populated_count, 1);
        assert_eq!(t.dimm_sizes, vec![16384]);
    }

    /// (M-17) The source status distinguishes *why* the topology is
    /// empty: a missing directory is `DirAbsent`, a corrupt (too-short)
    /// structure is `ParseError`, and a readable-but-empty directory is
    /// `Ok` (a legitimate no-DIMM source, not a failure).
    #[test]
    fn source_status_tracks_the_failure_mode() {
        // Missing directory → DirAbsent.
        let (_, status) = collect_from(Path::new("/nonexistent/dmi/entries-xyz"));
        assert_eq!(status, SmbiosSourceStatus::DirAbsent);

        // A corrupt (too-short) structure → ParseError.
        let dmi = TempDmi::new("m17-corrupt");
        write_entry(&dmi.root, 0, &[0x11]); // one byte (no length)
        let (t, status) = collect_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::ParseError);
        assert_eq!(t, SmbiosTopology::empty());

        // A readable-but-empty directory → Ok (legitimately no DIMMs).
        let dmi = TempDmi::new("m17-empty-ok");
        let (t, status) = collect_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (M-17) the [`SmbiosSourceStatus`] `Display` renders each variant as
    /// the human annotation the facade threads into the readout.
    #[test]
    fn smbios_source_status_display() {
        assert_eq!(SmbiosSourceStatus::Ok.to_string(), "ok");
        assert_eq!(
            SmbiosSourceStatus::DirAbsent.to_string(),
            "DMI source absent (VM/container/non-Intel)"
        );
        assert_eq!(
            SmbiosSourceStatus::Unreadable.to_string(),
            "DMI source unreadable"
        );
        assert_eq!(
            SmbiosSourceStatus::ParseError.to_string(),
            "DMI structure corrupt"
        );
    }

    /// (M-15) The data-driven table is case-insensitive and word-based:
    /// upper / mixed case and separators *between* words (`-`, `_`, `/`) do
    /// not affect the match, and a first-match-wins precedence keeps a
    /// standalone letter from being shadowed by a longer word.
    #[test]
    fn m15_patterns_are_case_and_separator_insensitive() {
        assert_eq!(channel_of("P1"), Some('A'));
        assert_eq!(channel_of("p2"), Some('B'));
        assert_eq!(channel_of("CH1"), Some('B'));
        assert_eq!(channel_of("Slot_C"), Some('C'));
        assert_eq!(channel_of("Dimm 3"), Some('C'));
        assert_eq!(channel_of("dimm-b"), Some('B'));
        // First match wins: a standalone `A` token precedes any longer word.
        assert_eq!(channel_of("A/B"), Some('A'));
        // A `D` that only starts a longer word is not a channel.
        assert_eq!(channel_of("DIMM"), None);
        assert_eq!(channel_of("Decade 1"), None);
    }

    // ------------------------------------------------------------------
    // Chunk 4: SMBIOS SPD fallback — collect_module_entries tests
    // ------------------------------------------------------------------

    /// (Chunk 4) The real 12600KF/Z690M-ITX/ax table (SMBIOS 3.5, 92-byte
    /// fixed part): `collect_module_entries_from` returns 2 populated
    /// entries with all the new string-reference fields decoded correctly
    /// from the fixture's string table:
    /// - String 3: "Team Group Inc" (Manufacturer)
    /// - String 4: "0104D817" / "0104D816" (Serial Number)
    /// - String 5: "9876543210" (Asset Tag)
    /// - String 6: "TEAMGROUP-UD4-3600  " (Part Number)
    #[test]
    fn chunk4_real_12600kf_entries_decode_string_fields() {
        let dmi = TempDmi::new("chunk4-real");
        write_entry(&dmi.root, 0, REAL_12600KF_17_0);
        write_entry(&dmi.root, 1, REAL_12600KF_17_1);

        let (entries, status) = collect_module_entries_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(entries.len(), 2, "both populated DIMMs must be entries");

        // Entry 0 (handle 0x000C): the first DIMM.
        let e0 = &entries[0];
        assert_eq!(e0.index, 0);
        assert_eq!(e0.capacity_mib, 16384, "0x4000 MB = 16 GiB");
        assert_eq!(e0.memory_type, Some(MemoryTech::Ddr4));
        assert_eq!(e0.speed_mts, Some(3600), "0x0E10 = 3600 MT/s");
        assert_eq!(e0.locator, "Controller0-ChannelA-DIMM0");
        assert_eq!(
            e0.manufacturer.as_deref(),
            Some("Team Group Inc"),
            "string ref 3 = Manufacturer"
        );
        assert_eq!(
            e0.serial_number.as_deref(),
            Some("0104D817"),
            "string ref 4 = Serial Number (DIMM 0)"
        );
        assert_eq!(
            e0.asset_tag.as_deref(),
            Some("9876543210"),
            "string ref 5 = Asset Tag"
        );
        assert_eq!(
            e0.part_number.as_deref(),
            Some("TEAMGROUP-UD4-3600"),
            "string ref 6 = Part Number (trimmed)"
        );

        // Entry 1 (handle 0x000D): the second DIMM (different serial).
        let e1 = &entries[1];
        assert_eq!(e1.index, 1);
        assert_eq!(e1.capacity_mib, 16384);
        assert_eq!(e1.locator, "Controller1-ChannelA-DIMM0");
        assert_eq!(e1.manufacturer.as_deref(), Some("Team Group Inc"));
        assert_eq!(
            e1.serial_number.as_deref(),
            Some("0104D816"),
            "string ref 4 = Serial Number (DIMM 1)"
        );
        assert_eq!(e1.asset_tag.as_deref(), Some("9876543210"));
        assert_eq!(e1.part_number.as_deref(), Some("TEAMGROUP-UD4-3600"));
    }

    /// (Chunk 4) "Unknown" / empty manufacturer strings are filtered out
    /// (the SMBIOS spec allows "Unknown" as a placeholder — we treat it as
    /// absent rather than displaying it). Part number is trimmed.
    #[test]
    fn chunk4_unknown_manufacturer_is_filtered() {
        // Build a synthetic 3.x-layout entry with "Unknown" manufacturer
        // (string 3) and a part number with trailing spaces (string 4).
        let o = SMBIOS3_OFFSETS;
        let mut fixed = [0u8; 0x22];
        fixed[OFF_LENGTH] = 0x22;
        fixed[o.capacity..o.capacity + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        fixed[o.mem_device_locator] = 1;
        fixed[o.bank_locator] = 2;
        fixed[o.memory_type] = 0x1A; // DDR4
        fixed[o.manufacturer] = 3; // string ref -> "Unknown"
        fixed[o.serial_number] = 0; // no serial
        fixed[o.asset_tag] = 0; // no asset tag
        fixed[o.part_number] = 4; // string ref -> "PART-001  "
        fixed[OFF_SPEED..OFF_SPEED + SPEED_LEN].copy_from_slice(&3200u16.to_le_bytes());
        let bytes = type17_file(0x22, &mut fixed, &["A2", "A2", "Unknown", "PART-001  "]);

        let dmi = TempDmi::new("chunk4-unknown-mfr");
        write_entry(&dmi.root, 0, &bytes);

        let (entries, status) = collect_module_entries_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.manufacturer, None, "\"Unknown\" is filtered out");
        assert_eq!(e.serial_number, None, "string ref 0 is absent");
        assert_eq!(e.part_number.as_deref(), Some("PART-001"), "trimmed");
        assert_eq!(e.speed_mts, Some(3200));
    }

    /// (Chunk 4) A missing DMI directory yields empty entries with
    /// `DirAbsent` status (the VM / container case) — never a panic.
    #[test]
    fn chunk4_missing_dir_is_empty_entries() {
        let (entries, status) =
            collect_module_entries_from(Path::new("/nonexistent/dmi/entries-chunk4"));
        assert!(entries.is_empty());
        assert_eq!(status, SmbiosSourceStatus::DirAbsent);
    }

    /// (Chunk 4) A corrupt structure yields empty entries with
    /// `ParseError` status.
    #[test]
    fn chunk4_corrupt_structure_is_parse_error() {
        let dmi = TempDmi::new("chunk4-corrupt");
        write_entry(&dmi.root, 0, &[0x11]); // one byte (no length)
        let (entries, status) = collect_module_entries_from(&dmi.root);
        assert!(entries.is_empty());
        assert_eq!(status, SmbiosSourceStatus::ParseError);
    }

    /// (Chunk 4) `SmbiosModuleEntry` derives the expected traits.
    #[test]
    fn chunk4_module_entry_derives() {
        let e = SmbiosModuleEntry {
            index: 0,
            capacity_mib: 16384,
            memory_type: Some(MemoryTech::Ddr4),
            speed_mts: Some(3200),
            locator: "A2".to_owned(),
            manufacturer: Some("Test".to_owned()),
            serial_number: Some("S1".to_owned()),
            asset_tag: None,
            part_number: Some("P1".to_owned()),
        };
        let e2 = e.clone();
        assert_eq!(e, e2);
        assert!(format!("{e:?}").contains("SmbiosModuleEntry"));
    }

    // ====================================================================
    // QA edge-case battery: SMBIOS string-table boundaries and failure
    // modes.
    // ====================================================================

    /// (QA) Zero Type 17 entries → the empty topology, not an error.
    #[test]
    fn test_smbios_zero_entries() {
        let dmi = TempDmi::new("zero-entries");
        let (t, status) = collect_from(&dmi.root);
        assert_eq!(t, SmbiosTopology::empty());
        assert_eq!(
            status,
            SmbiosSourceStatus::Ok,
            "an empty (but present) dir is Ok, not an error"
        );
    }

    /// (QA) A null string reference (index 0) yields None — no panic.
    #[test]
    fn test_smbios_null_string_ref() {
        let table = b"string1\0string2\0string3\0";
        assert_eq!(string_at(table, 0), None);
        // Sanity: ref 1 is the first string in the table.
        assert_eq!(string_at(table, 1), Some("string1".to_owned()));
    }

    /// (QA) An out-of-range string reference (beyond the number of
    /// strings in the table) yields None — no panic, no out-of-bounds
    /// read.
    #[test]
    fn test_smbios_out_of_range_string_ref() {
        let table = b"string1\0string2\0string3\0string4\0string5\0";
        // Only 5 strings exist; ref 6 and ref 255 are out of range.
        assert_eq!(string_at(table, 6), None);
        assert_eq!(string_at(table, 255), None);
    }

    /// (QA) The `17-<n>` entries are sorted **numerically** by handle, not
    /// lexicographically — on a 12-DIMM system the order must be
    /// 0,1,2,…,9,10,11 (so slot 2 keeps index 2), not 0,1,10,11,2,…
    #[test]
    fn test_smbios_numeric_sort() {
        let dmi = TempDmi::new("numeric-sort");
        // Write entries out of order with distinct locators.
        for n in [10u8, 2, 0, 11, 1] {
            write_entry(&dmi.root, n, &populated_ddr4(&format!("DIMM{n}")));
        }
        let (t, status) = collect_from(&dmi.root);
        assert_eq!(status, SmbiosSourceStatus::Ok);
        assert_eq!(t.populated_count, 5);
        // Numeric order: 0, 1, 2, 10, 11 — not lexicographic 0,1,10,11,2.
        assert_eq!(
            t.locators,
            vec!["DIMM0", "DIMM1", "DIMM2", "DIMM10", "DIMM11"],
            "17-2 must sort before 17-10 (numeric, not lexicographic)"
        );
    }

    // ====================================================================
    // AGESA from SMBIOS Type 40 tests
    // ====================================================================

    /// Write a synthetic `40-<n>` entry into `dir`, mirroring the kernel's
    /// dmi-sysfs layout: a directory `40-<n>` whose payload file is
    /// `40-<n>/raw`.
    fn write_type40_entry(dir: &Path, n: u8, bytes: &[u8]) {
        let entry = dir.join(format!("40-{n}"));
        std::fs::create_dir_all(&entry).expect("type40 entry dir create");
        let mut f = std::fs::File::create(entry.join("raw")).expect("type40 raw create");
        f.write_all(bytes).expect("type40 raw write");
    }

    /// The verbatim SMBIOS Type 40 structure from ZandoBob's research on
    /// the Gigabyte B850 AI TOP (AGESA!V9 ComboAm5PI 1.2.8.0).
    ///
    /// Bytes: 28 0e 1d 00 01 09 00 00 05 01 00 00 00 00
    ///        41 47 45 53 41 21 56 39 20 43 6f 6d 62 6f 41 6d 35 50 49
    ///        20 31 2e 32 2e 38 2e 30 00 00
    ///
    /// Layout:
    /// - 0x00: Type = 0x28 (40)
    /// - 0x01: Length = 0x0E (14, formatted area size)
    /// - 0x02-0x03: Handle = 0x001D
    /// - 0x04: NumEntries = 1
    /// - 0x05: Entry[0].Length = 9
    /// - 0x06-0x07: Entry[0].RefHandle = 0x0000
    /// - 0x08: Entry[0].RefOffset = 0x05
    /// - 0x09: Entry[0].StringRef = 1
    /// - 0x0A-0x0D: Entry[0].Value = 4 bytes of zeros
    /// - 0x0E+: String table: "AGESA!V9 ComboAm5PI 1.2.8.0\0\0"
    const REAL_TYPE40_AGESA: &[u8] = &[
        0x28, 0x0E, 0x1D, 0x00, 0x01, 0x09, 0x00, 0x00, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00, b'A',
        b'G', b'E', b'S', b'A', b'!', b'V', b'9', b' ', b'C', b'o', b'm', b'b', b'o', b'A', b'm',
        b'5', b'P', b'I', b' ', b'1', b'.', b'2', b'.', b'8', b'.', b'0', 0x00, 0x00,
    ];

    /// (Type 40) The verbatim Gigabyte B850 AI TOP structure parses to the
    /// expected AGESA fields.
    #[test]
    fn test_parse_type40_agesa() {
        let agesa = parse_type40_entry(REAL_TYPE40_AGESA).expect("must parse");
        assert_eq!(agesa.line, Some("V9".to_string()));
        assert_eq!(agesa.project, Some("ComboAm5PI".to_string()));
        assert_eq!(agesa.version, Some("1.2.8.0".to_string()));
        assert_eq!(agesa.raw, "AGESA!V9 ComboAm5PI 1.2.8.0");
    }

    /// (Type 40) `display_string` formats the fields for the platform
    /// readout.
    #[test]
    fn test_type40_display_string() {
        let agesa = parse_type40_entry(REAL_TYPE40_AGESA).expect("must parse");
        assert_eq!(
            agesa.display_string(),
            Some("V9 ComboAm5PI 1.2.8.0".to_string())
        );

        // Partial info: line + version only.
        let partial = AgesaInfo {
            raw: "AGESA!V9 1.2.3.4".to_string(),
            line: Some("V9".to_string()),
            project: None,
            version: Some("1.2.3.4".to_string()),
        };
        assert_eq!(partial.display_string(), Some("V9 1.2.3.4".to_string()));

        // Version only.
        let version_only = AgesaInfo {
            raw: "AGESA!1.2.3.4".to_string(),
            line: None,
            project: None,
            version: Some("1.2.3.4".to_string()),
        };
        assert_eq!(version_only.display_string(), Some("1.2.3.4".to_string()));

        // No useful fields (bare marker) → None.
        let bare = AgesaInfo {
            raw: "AGESA!".to_string(),
            line: None,
            project: None,
            version: None,
        };
        assert_eq!(bare.display_string(), None);
    }

    /// (Type 40) AGESA string parsing variants.
    #[test]
    fn test_parse_agesa_string_variants() {
        // With !
        let a = parse_agesa_string("AGESA!V9 ComboAm5PI 1.2.8.0").expect("with !");
        assert_eq!(a.line, Some("V9".to_string()));
        assert_eq!(a.project, Some("ComboAm5PI".to_string()));
        assert_eq!(a.version, Some("1.2.8.0".to_string()));

        // Without !
        let b = parse_agesa_string("AGESA V9 ComboAm5PI 1.2.8.0").expect("without !");
        assert_eq!(b.line, Some("V9".to_string()));
        assert_eq!(b.project, Some("ComboAm5PI".to_string()));
        assert_eq!(b.version, Some("1.2.8.0".to_string()));

        // Case insensitive
        let c = parse_agesa_string("agesa!v9 comboam5pi 1.2.8.0").expect("lowercase");
        assert_eq!(c.line, Some("v9".to_string()));
        assert_eq!(c.project, Some("comboam5pi".to_string()));
        assert_eq!(c.version, Some("1.2.8.0".to_string()));

        // Not AGESA
        assert!(parse_agesa_string("BIOS Version F10").is_none());
        assert!(parse_agesa_string("American Megatrends Inc.").is_none());

        // Just marker
        let d = parse_agesa_string("AGESA!").expect("bare marker");
        assert_eq!(d.line, None);
        assert_eq!(d.project, None);
        assert_eq!(d.version, None);
        assert_eq!(d.raw, "AGESA!");

        // Single token after marker (line only, no version — "V9" has no dot)
        let e = parse_agesa_string("AGESA!V9").expect("line only");
        assert_eq!(e.line, Some("V9".to_string()));
        assert_eq!(e.project, None);
        assert_eq!(
            e.version, None,
            "a non-dotted token is a line, not a version"
        );

        // Single dotted token (version only, no line)
        let e2 = parse_agesa_string("AGESA!1.2.3.4").expect("version only");
        assert_eq!(e2.line, None);
        assert_eq!(e2.project, None);
        assert_eq!(e2.version, Some("1.2.3.4".to_string()));

        // Two tokens (line + version, no project)
        let f = parse_agesa_string("AGESA!V9 1.2.3.4").expect("line + version");
        assert_eq!(f.line, Some("V9".to_string()));
        assert_eq!(f.project, None);
        assert_eq!(f.version, Some("1.2.3.4".to_string()));

        // Two tokens (line + project, no version — neither has a dot)
        let g = parse_agesa_string("AGESA!V9 ComboAm5PI").expect("line + project");
        assert_eq!(g.line, Some("V9".to_string()));
        assert_eq!(g.project, Some("ComboAm5PI".to_string()));
        assert_eq!(g.version, None);
    }

    /// (Type 40) A corrupt / too-short structure yields None — no panic.
    #[test]
    fn test_type40_corrupt_is_none() {
        // Too short (no length field).
        assert!(parse_type40_entry(&[0x28]).is_none());
        // Wrong type byte.
        assert!(parse_type40_entry(&[0x11, 0x0E, 0x00, 0x00, 0x00]).is_none());
        // Length smaller than the minimum header.
        assert!(parse_type40_entry(&[0x28, 0x03, 0x00, 0x00, 0x00]).is_none());
        // Length exceeds the raw buffer.
        assert!(parse_type40_entry(&[0x28, 0xFF, 0x00, 0x00, 0x00]).is_none());
    }

    /// (Type 40) A Type 40 structure with a non-AGESA string yields None.
    #[test]
    fn test_type40_no_agesa_string_is_none() {
        // Type 40 with one entry referencing string "BIOS Version F10".
        let mut raw = vec![
            0x28, // Type 40
            0x0E, // Length = 14 (formatted area)
            0x00, 0x00, // Handle
            0x01, // NumEntries = 1
            0x09, // Entry[0].Length = 9
            0x00, 0x00, // Entry[0].RefHandle = 0
            0x05, // Entry[0].RefOffset = 5
            0x01, // Entry[0].StringRef = 1
            0x00, 0x00, 0x00, 0x00, // Entry[0].Value (4 bytes)
        ];
        // String table: "BIOS Version F10\0\0"
        raw.extend_from_slice(b"BIOS Version F10\0\0");
        assert!(parse_type40_entry(&raw).is_none());
    }

    /// (Type 40) Multiple entries in one Type 40 structure: the first
    /// entry with an AGESA string wins.
    #[test]
    fn test_type40_multiple_entries_first_agesa_wins() {
        // Two entries: first is non-AGESA, second is AGESA.
        let mut raw = vec![
            0x28, // Type 40
            0x19, // Length = 25 (formatted area: 5 header + 9 + 11)
            0x00, 0x00, // Handle
            0x02, // NumEntries = 2
            // Entry 0: non-AGESA string
            0x09, // Length = 9
            0x00, 0x00, // RefHandle = 0
            0x05, // RefOffset = 5
            0x01, // StringRef = 1
            0x00, 0x00, 0x00, 0x00, // Value (4 bytes)
            // Entry 1: AGESA string
            0x0B, // Length = 11
            0x00, 0x00, // RefHandle = 0
            0x05, // RefOffset = 5
            0x02, // StringRef = 2
            0x00, 0x00, 0x00, 0x00, // Value (4 bytes)
            0x00, // Value extra byte (11 - 5 = 6 bytes of value)
            0x00,
        ];
        // String table: "SomeOther\0AGESA!V9 ComboAm5PI 1.2.8.0\0\0"
        raw.extend_from_slice(b"SomeOther\0");
        raw.extend_from_slice(b"AGESA!V9 ComboAm5PI 1.2.8.0\0\0");
        let agesa = parse_type40_entry(&raw).expect("second entry has AGESA");
        assert_eq!(agesa.line, Some("V9".to_string()));
        assert_eq!(agesa.version, Some("1.2.8.0".to_string()));
    }

    /// (Type 40) End-to-end: a temp DMI directory with a `40-0` entry
    /// yields the AGESA info via `read_agesa_from`.
    #[test]
    fn test_read_agesa_from_temp_dir() {
        let dmi = TempDmi::new("type40-e2e");
        write_type40_entry(&dmi.root, 0, REAL_TYPE40_AGESA);
        let agesa = read_agesa_from(&dmi.root).expect("must find AGESA");
        assert_eq!(agesa.line, Some("V9".to_string()));
        assert_eq!(agesa.project, Some("ComboAm5PI".to_string()));
        assert_eq!(agesa.version, Some("1.2.8.0".to_string()));
    }

    /// (Type 40) End-to-end: a temp DMI directory with only Type 17
    /// entries (no Type 40) yields None — no panic, no false positive.
    #[test]
    fn test_read_agesa_from_no_type40_is_none() {
        let dmi = TempDmi::new("type40-none");
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        assert!(read_agesa_from(&dmi.root).is_none());
    }

    /// (Type 40) End-to-end: a missing DMI directory yields None.
    #[test]
    fn test_read_agesa_from_missing_dir_is_none() {
        assert!(read_agesa_from(Path::new("/nonexistent/dmi/entries-type40")).is_none());
    }

    /// (Type 40) Numeric sort: `40-2` is scanned before `40-10` (the
    /// first AGESA found wins, so the ordering matters).
    #[test]
    fn test_type40_numeric_sort_order() {
        let dmi = TempDmi::new("type40-sort");
        // 40-10 has an AGESA string; 40-2 does not.
        write_type40_entry(&dmi.root, 10, REAL_TYPE40_AGESA);
        // 40-2: a non-AGESA Type 40.
        let mut raw2 = vec![
            0x28, 0x0E, 0x00, 0x00, 0x01, 0x09, 0x00, 0x00, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00,
        ];
        raw2.extend_from_slice(b"NotAGESA\0\0");
        write_type40_entry(&dmi.root, 2, &raw2);
        // Numeric order: 40-2 first (no AGESA), then 40-10 (AGESA found).
        let agesa = read_agesa_from(&dmi.root).expect("40-10 has AGESA");
        assert_eq!(agesa.version, Some("1.2.8.0".to_string()));
    }
}
