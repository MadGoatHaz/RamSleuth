//! SMBIOS Type 17 cross-check (Intel memory-channel detection, Chunk 2).
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
//! "Memory Device Locator" field). Verified offsets:
//!
//! - **capacity** @ `0x0C` (16-bit word; per the spec the magnitude is
//!   set by bit 15 — **clear = MB unit, set = KB unit**: `0x4000` =
//!   16384 MB = 16 GiB; the sentinel words `0x7FFF` (≥ 32 GiB − 1 MiB)
//!   and `0xFFFF` (unknown) point to the extended size);
//! - **extended capacity** @ `0x1C` (32-bit dword, MiB in bits 30:0 —
//!   the SMBIOS 3.0+ extended size; zero on the reference board);
//! - **memory type** @ `0x12` (`0x1A` = DDR4, `0x22` = DDR5);
//! - **locators**: the "Device Locator" string ref @ `0x10`, with the
//!   "Bank Locator" string ref @ `0x11` as the fallback.
//!
//! # No-panic contract (D5)
//!
//! Every read degrades to the empty topology on any absence / unreadable /
//! malformed input — a missing `dmi` directory (a VM / container / non-Intel
//! host) yields [`SmbiosTopology::empty`], a short or corrupt structure is
//! skipped, and nothing ever panics. The root is injectable
//! ([`collect_from`]) so the fixture tests build a synthetic `17-*` tree in
//! a temp directory (no hardware, no root).
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
//! their locator strings:
//!
//! - **Dual** — the populated DIMMs span ≥ 2 distinct channel slots (e.g.
//!   `A2` + `B2`, the standard mirrored daisy-chain pair);
//! - **Single** — all populated DIMMs sit on one channel (e.g. `A1` + `A2`);
//! - **Ambiguous** — a populated DIMM carries a non-standard locator with no
//!   channel designation (e.g. `DIMM 1` / `DIMM 2`), so the channel cannot be
//!   told apart (the L4 heuristic input).

use std::collections::HashSet;
use std::path::Path;

use crate::intel_readout::MemoryTech;

/// The real SMBIOS Type 17 entry directory (frozen; unprivileged).
const DMI_ENTRIES_DIR: &str = "/sys/firmware/dmi/entries";

/// The file-name prefix of a raw SMBIOS Type 17 structure (`17-<n>`).
const TYPE17_PREFIX: &str = "17-";

/// Type 17 fixed-part field offsets (the real SMBIOS 3.5 layout — the
/// classic DMTF table shifted +2 by the `Error Information Handle` at
/// `0x06-0x07`; verified on the reference 12600KF / Z690M-ITX/ax dumps).
const OFF_LENGTH: usize = 0x01;
const OFF_CAPACITY: usize = 0x0C;
const CAPACITY_LEN: usize = 2;
const OFF_MEM_DEVICE_LOCATOR: usize = 0x10;
const OFF_BANK_LOCATOR: usize = 0x11;
const OFF_MEMORY_TYPE: usize = 0x12;
const OFF_CAPACITY_EXTENDED: usize = 0x1C;

/// The channel classification derived from the populated DIMM locators
/// (research: "matched A/B pairs → Dual; both on one channel → Single;
/// non-standard `DIMM 1/2` labels → Ambiguous").
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
    /// The active memory technology of the first populated DIMM
    /// (`0x1A` → DDR4, `0x22` → DDR5); `None` when no populated DIMM
    /// carries a recognized type.
    pub memory_type: Option<MemoryTech>,
    /// The locator strings of the populated DIMMs (board-slot designations
    /// such as `A2` / `B2` / `DIMM 1`).
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

/// Collect the SMBIOS Type 17 topology from the kernel's `dmi` export
/// (the frozen [`DMI_ENTRIES_DIR`]).
///
/// # No-panic contract
///
/// A missing directory (VM / container / non-Intel host) or any read
/// failure yields [`SmbiosTopology::empty`] — never a panic, never an
/// invented DIMM.
pub fn collect() -> SmbiosTopology {
    collect_from(Path::new(DMI_ENTRIES_DIR))
}

/// The injectable-root [`collect`] (the fixture tests pass a temp directory
/// holding synthetic `17-*` files). Any missing / unreadable source
/// degrades to the empty topology.
pub fn collect_from(root: &Path) -> SmbiosTopology {
    let mut topology = SmbiosTopology::empty();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return topology, // no dmi dir -> empty (no panic)
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
    paths.sort();
    for path in paths {
        // The kernel exports each `17-<n>` as a *directory*; the
        // structure bytes live in the payload file `17-<n>/raw`.
        let bytes = match std::fs::read(path.join("raw")) {
            Ok(b) => b,
            Err(_) => continue, // unreadable -> skip (no panic)
        };
        if let Some((capacity, mem_type, locator)) = parse_type17(&bytes) {
            topology.populated_count = topology.populated_count.saturating_add(1);
            topology.dimm_sizes.push(capacity);
            if topology.memory_type.is_none() {
                topology.memory_type = mem_type;
            }
            topology.locators.push(locator);
        }
    }
    topology.derived_mode = derive_mode(&topology.locators);
    topology
}

/// Parse one raw SMBIOS Type 17 structure into its (capacity, memory type,
/// locator) triple, or `None` when the DIMM is unpopulated (capacity `0`)
/// or the structure is too short to carry the fields.
///
/// # No-panic contract
///
/// Every field read is bounds-checked; a short / corrupt structure yields
/// `None` (skipped) rather than a panic.
fn parse_type17(bytes: &[u8]) -> Option<(u64, Option<MemoryTech>, String)> {
    if bytes.len() < 2 {
        return None;
    }
    let len = (bytes[OFF_LENGTH] as usize).min(bytes.len());
    let fixed = &bytes[..len];
    let table = &bytes[len..];

    // Capacity: the `0x0C` word — per SMBIOS 3.5 (DSP0134 Table 76) the
    // magnitude is set by bit 15: **clear = MB, set = KB** (`0x4000` =
    // 16384 MB = the reference 16 GiB DIMM). The sentinel words
    // `0x7FFF` (≥ 32 GiB − 1 MiB) and `0xFFFF` (unknown), like a `0`
    // word, fall through to the `0x1C` Extended Size dword (MiB); both
    // zero = unpopulated DIMM.
    let mut capacity: u64 = 0;
    if fixed.len() >= OFF_CAPACITY + CAPACITY_LEN {
        let word = u16::from_le_bytes([fixed[OFF_CAPACITY], fixed[OFF_CAPACITY + 1]]);
        capacity = match word {
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
    }
    if capacity == 0 && fixed.len() >= OFF_CAPACITY_EXTENDED + 4 {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&fixed[OFF_CAPACITY_EXTENDED..OFF_CAPACITY_EXTENDED + 4]);
        capacity = u32::from_le_bytes(buf) as u64; // MiB (bits 30:0)
    }
    if capacity == 0 {
        return None; // unpopulated DIMM
    }

    // Memory type @ 0x12 (0x1A = DDR4, 0x22 = DDR5).
    let mem_type = if fixed.len() > OFF_MEMORY_TYPE {
        MemoryTech::from_smbios_type(fixed[OFF_MEMORY_TYPE])
    } else {
        None
    };

    // Locator: the Memory Device Locator string (0x10), falling back to
    // the Bank Locator string (0x11) when the former is empty / absent.
    let mem_ref = if fixed.len() > OFF_MEM_DEVICE_LOCATOR {
        fixed[OFF_MEM_DEVICE_LOCATOR]
    } else {
        0
    };
    let bank_ref = if fixed.len() > OFF_BANK_LOCATOR {
        fixed[OFF_BANK_LOCATOR]
    } else {
        0
    };
    let locator = string_at(table, mem_ref)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| string_at(table, bank_ref))
        .unwrap_or_default();

    Some((capacity, mem_type, locator))
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
    parts.get(ref_idx as usize - 1)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|s| !s.is_empty())
}

/// The channel designation (A–D) of a locator string, if one is present.
///
/// Two grammars, tried in order:
///
/// 1. **`Controller<digit>`** (case-insensitive) — this firmware names
///    its DIMMs `Controller0-ChannelA-DIMM0` / `Controller1-ChannelA-DIMM0`
///    — *both* say "ChannelA"; the true channel is the **Controller**
///    index: `0`→`A`, `1`→`B`, `2`→`C`, `3`→`D` (a digit above `3` is not
///    a channel designation);
/// 2. a standalone A–D letter token (e.g. `A` in `CPU0_DIMM_A`) or an A–D
///    letter immediately followed by a digit (e.g. `A1`, `B2`). This
///    distinguishes a channel designation from an A–D letter that is
///    merely the start of a word (the `D` in `DIMM` is never a channel).
fn channel_of(locator: &str) -> Option<char> {
    // Pass 1: the `Controller<digit>` grammar (0→A … 3→D).
    for token in locator.split(|c: char| !c.is_alphanumeric()) {
        if let Some(digit) = token
            .to_ascii_lowercase()
            .as_str()
            .strip_prefix("controller")
            .and_then(|rest| rest.parse::<u8>().ok())
            .filter(|&d| d <= 3)
        {
            return Some(char::from(b'A' + digit));
        }
    }
    // Pass 2: the standalone A–D letter rules.
    for token in locator.split(|c: char| !c.is_alphanumeric()) {
        let b = token.as_bytes();
        if b.is_empty() {
            continue;
        }
        let first = (b[0] as char).to_ascii_uppercase();
        if !matches!(first, 'A' | 'B' | 'C' | 'D') {
            continue;
        }
        if b.len() == 1 {
            return Some(first);
        }
        if b.len() >= 2 && b[1].is_ascii_digit() {
            return Some(first);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a raw SMBIOS Type 17 structure: a `len`-byte fixed part (the
    /// caller sets the field bytes) + a null-separated string table
    /// (string ref `N` = the `N`th string — **no** leading empty segment,
    /// exactly like the kernel's real tables). Returns the full image the
    /// kernel exports at `17-<n>/raw`.
    fn type17_file(
        len: usize,
        fixed: &mut [u8],
        strings: &[&str],
    ) -> Vec<u8> {
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

    /// A temp directory (pid-scoped; removed on drop).
    struct TempDmi {
        root: std::path::PathBuf,
    }

    impl TempDmi {
        fn new(test: &str) -> Self {
            let mut root = std::env::temp_dir();
            root.push(format!("ramsleuth-dmi-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("dmi dir");
            Self { root }
        }
    }

    impl Drop for TempDmi {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
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
        let mut fixed = [0u8; 0x20];
        fixed[OFF_CAPACITY..OFF_CAPACITY + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_BANK_LOCATOR] = 2;
        fixed[OFF_MEMORY_TYPE] = 0x1A; // DDR4
        type17_file(0x20, &mut fixed, &[locator, locator])
    }

    /// An unpopulated DIMM (the `0x0C` word and the `0x1C` extended dword
    /// are both zero) — must be skipped.
    fn unpopulated() -> Vec<u8> {
        let mut fixed = [0u8; 0x20];
        fixed[OFF_MEMORY_TYPE] = 0x1A;
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_BANK_LOCATOR] = 2;
        // capacity (0x0C word + 0x1C dword) stays 0
        type17_file(0x20, &mut fixed, &["A1", "A1"])
    }

    /// The verbatim 12600KF / Z690M-ITX/ax SMBIOS Type 17 table
    /// (`17-0/raw`, handle `0x000C`; 183 bytes = the 92-byte SMBIOS 3.5
    /// fixed part + the string region). Captured byte-for-byte from the
    /// reference host's `/sys/firmware/dmi/entries` (evidence:
    /// `work/scratch/DeepSeek Report.md:28-41`).
const REAL_12600KF_17_0: &[u8] = &[
        0x11, 0x5C, 0x0C, 0x00, 0x0B, 0x00, 0xFE, 0xFF, 0x40, 0x00, 0x40, 0x00, 0x00, 0x40, 0x09, 0x00,
        0x01, 0x02, 0x1A, 0x80, 0x00, 0x60, 0x09, 0x03, 0x04, 0x05, 0x06, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x10, 0x0E, 0xB0, 0x04, 0x46, 0x05, 0xB0, 0x04, 0x03, 0x08, 0x00, 0x00, 0x04, 0xEF, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x43, 0x6F, 0x6E, 0x74,
        0x72, 0x6F, 0x6C, 0x6C, 0x65, 0x72, 0x30, 0x2D, 0x43, 0x68, 0x61, 0x6E, 0x6E, 0x65, 0x6C, 0x41,
        0x2D, 0x44, 0x49, 0x4D, 0x4D, 0x30, 0x00, 0x42, 0x41, 0x4E, 0x4B, 0x20, 0x30, 0x00, 0x54, 0x65,
        0x61, 0x6D, 0x20, 0x47, 0x72, 0x6F, 0x75, 0x70, 0x20, 0x49, 0x6E, 0x63, 0x00, 0x30, 0x31, 0x30,
        0x34, 0x44, 0x38, 0x31, 0x37, 0x00, 0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30,
        0x00, 0x54, 0x45, 0x41, 0x4D, 0x47, 0x52, 0x4F, 0x55, 0x50, 0x2D, 0x55, 0x44, 0x34, 0x2D, 0x33,
        0x36, 0x30, 0x30, 0x20, 0x20, 0x00, 0x00,
    ];

    /// The same real table for handle `0x000D` (`17-1/raw`): the
    /// `Controller1` locator + serial `0104D816` (evidence: `work/scratch/
    /// DeepSeek Report.md:43-56`).
const REAL_12600KF_17_1: &[u8] = &[
        0x11, 0x5C, 0x0D, 0x00, 0x0B, 0x00, 0xFE, 0xFF, 0x40, 0x00, 0x40, 0x00, 0x00, 0x40, 0x09, 0x00,
        0x01, 0x02, 0x1A, 0x80, 0x00, 0x60, 0x09, 0x03, 0x04, 0x05, 0x06, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x10, 0x0E, 0xB0, 0x04, 0x46, 0x05, 0xB0, 0x04, 0x03, 0x08, 0x00, 0x00, 0x04, 0xEF, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x43, 0x6F, 0x6E, 0x74,
        0x72, 0x6F, 0x6C, 0x6C, 0x65, 0x72, 0x31, 0x2D, 0x43, 0x68, 0x61, 0x6E, 0x6E, 0x65, 0x6C, 0x41,
        0x2D, 0x44, 0x49, 0x4D, 0x4D, 0x30, 0x00, 0x42, 0x41, 0x4E, 0x4B, 0x20, 0x30, 0x00, 0x54, 0x65,
        0x61, 0x6D, 0x20, 0x47, 0x72, 0x6F, 0x75, 0x70, 0x20, 0x49, 0x6E, 0x63, 0x00, 0x30, 0x31, 0x30,
        0x34, 0x44, 0x38, 0x31, 0x36, 0x00, 0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30,
        0x00, 0x54, 0x45, 0x41, 0x4D, 0x47, 0x52, 0x4F, 0x55, 0x50, 0x2D, 0x55, 0x44, 0x34, 0x2D, 0x33,
        0x36, 0x30, 0x30, 0x20, 0x20, 0x00, 0x00,
    ];

    /// (a) Two populated DDR4 DIMMs in matched slots `A2` + `B2` → the
    /// topology reports 2 DIMMs, both 16 GiB, DDR4, and a `Dual` derived
    /// mode (the standard mirrored daisy-chain pair).
    #[test]
    fn two_populated_across_channels_is_dual() {
        let dmi = TempDmi::new("dual");
        write_entry(&dmi.root, 0, &populated_ddr4("A2"));
        write_entry(&dmi.root, 1, &populated_ddr4("B2"));

        let t = collect_from(&dmi.root);
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

        let t = collect_from(&dmi.root);
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

        let t = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.derived_mode, DerivedMode::Single);
    }

    /// (d) Two populated DIMMs with non-standard locators (`DIMM 1` /
    /// `DIMM 2` — no channel designation) → `Ambiguous` derived mode (the
    /// L4 heuristic input).
    #[test]
    fn ambiguous_locators_is_ambiguous() {
        let dmi = TempDmi::new("ambiguous");
        write_entry(&dmi.root, 0, &populated_ddr4("DIMM 1"));
        write_entry(&dmi.root, 1, &populated_ddr4("DIMM 2"));

        let t = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 2);
        assert_eq!(t.locators, vec!["DIMM 1".to_owned(), "DIMM 2".to_owned()]);
        assert_eq!(t.derived_mode, DerivedMode::Ambiguous);
    }

    /// (e) An empty directory (no `17-*` files) → the empty topology (no
    /// panic, no invented DIMM).
    #[test]
    fn empty_dir_is_empty_topology() {
        let dmi = TempDmi::new("empty");
        let t = collect_from(&dmi.root);
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (f) A missing directory (the VM / non-Intel-host case) → the empty
    /// topology (no panic).
    #[test]
    fn missing_dir_is_empty_topology() {
        let t = collect_from(Path::new("/nonexistent/dmi/entries-xyz"));
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

        let t = collect_from(&dmi.root);
        assert_eq!(t.populated_count, 1);
        assert_eq!(t.dimm_sizes, vec![16384]);
        assert_eq!(t.locators, vec!["A2".to_owned()]);
    }

    /// (h) A DDR5 memory-type field (`0x22`) decodes to [`MemoryTech::Ddr5`].
    #[test]
    fn ddr5_memory_type_decodes() {
        let mut fixed = [0u8; 0x20];
        fixed[OFF_MEMORY_TYPE] = 0x22; // DDR5
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_BANK_LOCATOR] = 2;
        fixed[OFF_CAPACITY..OFF_CAPACITY + CAPACITY_LEN].copy_from_slice(&0x4000u16.to_le_bytes());
        let bytes = type17_file(0x20, &mut fixed, &["A2", "A2"]);

        let dmi = TempDmi::new("ddr5");
        write_entry(&dmi.root, 0, &bytes);
        let t = collect_from(&dmi.root);
        assert_eq!(t.memory_type, Some(MemoryTech::Ddr5));
    }

    /// (i) A corrupt / too-short structure is skipped (no panic) — a
    /// one-byte file yields the empty topology.
    #[test]
    fn short_structure_is_skipped() {
        let dmi = TempDmi::new("short");
        write_entry(&dmi.root, 0, &[0x11]); // one byte (type only, no length)
        let t = collect_from(&dmi.root);
        assert_eq!(t, SmbiosTopology::empty());
    }

    /// (j) The `channel_of` heuristic: a channel designation is recognized
    /// (the `Controller<digit>` grammar, a single A–D letter,
    /// letter+digit) but not an A–D letter that starts a word (`DIMM`,
    /// `RAM`).
    #[test]
    fn channel_of_recognizes_designations_only() {
        // The `Controller<digit>` grammar (0→A … 3→D) — this firmware
        // names both DIMMs "ChannelA"; the channel is the Controller
        // index.
        assert_eq!(channel_of("Controller0-ChannelA-DIMM0"), Some('A'));
        assert_eq!(channel_of("Controller1-ChannelA-DIMM0"), Some('B'));
        assert_eq!(channel_of("Controller2-ChannelA-DIMM0"), Some('C'));
        assert_eq!(channel_of("Controller3-ChannelA-DIMM0"), Some('D'));
        assert_eq!(channel_of("A2"), Some('A'));
        assert_eq!(channel_of("DIMM A1"), Some('A'));
        assert_eq!(channel_of("CPU0_DIMM_B"), Some('B'));
        assert_eq!(channel_of("Channel A Slot 1"), Some('A'));
        // No channel designation (a digit above 3 is not a channel; an
        // A–D letter that starts a word is not one either).
        assert_eq!(channel_of("Controller4-ChannelA-DIMM0"), None);
        assert_eq!(channel_of("DIMM 1"), None);
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
    /// whose `ControllerN` locators derive `Dual` — the exact topology
    /// the L3 firmware override consumes.
    #[test]
    fn real_12600kf_table_reports_dual() {
        let dmi = TempDmi::new("real-12600kf");
        write_entry(&dmi.root, 0, REAL_12600KF_17_0);
        write_entry(&dmi.root, 1, REAL_12600KF_17_1);

        let t = collect_from(&dmi.root);
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
    }
}
