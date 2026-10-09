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
//! Each raw Type 17 structure is exported by the kernel at
//! `/sys/firmware/dmi/entries/17-<n>` (one file per physical memory device;
//! the trailing `<n>` is the structure handle ordinal). The file is the
//! DMTF Type 17 fixed part (`[0..len]`, `len` = byte `0x01`) followed by the
//! null-separated string table. Per the research-validated offsets:
//!
//! - **capacity** @ `0x28` (4 little-endian bytes, in MiB; `0` = absent /
//!   not installed);
//! - **memory type** @ `0x12` (`0x1A` = DDR4, `0x22` = DDR5);
//! - **locators**: the "Memory Device Locator" string ref @ `0x06`, with the
//!   "Device Locator" string ref @ `0x0C` as the fallback.
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

/// Type 17 fixed-part field offsets (research-validated).
const OFF_LENGTH: usize = 0x01;
const OFF_MEM_DEVICE_LOCATOR: usize = 0x06;
const OFF_DEVICE_LOCATOR: usize = 0x0C;
const OFF_MEMORY_TYPE: usize = 0x12;
const OFF_CAPACITY: usize = 0x28;
const CAPACITY_LEN: usize = 4;

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
        let bytes = match std::fs::read(&path) {
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

    // Capacity @ 0x28 (4 LE bytes, MiB); `0` = absent / not installed.
    let capacity = if fixed.len() >= OFF_CAPACITY + CAPACITY_LEN {
        let mut buf = [0u8; CAPACITY_LEN];
        buf.copy_from_slice(&fixed[OFF_CAPACITY..OFF_CAPACITY + CAPACITY_LEN]);
        u32::from_le_bytes(buf) as u64
    } else {
        0
    };
    if capacity == 0 {
        return None; // unpopulated DIMM
    }

    // Memory type @ 0x12 (0x1A = DDR4, 0x22 = DDR5).
    let mem_type = if fixed.len() > OFF_MEMORY_TYPE {
        MemoryTech::from_smbios_type(fixed[OFF_MEMORY_TYPE])
    } else {
        None
    };

    // Locator: the Memory Device Locator string (0x06), falling back to the
    // Device Locator string (0x0C) when the former is empty / absent.
    let mem_ref = if fixed.len() > OFF_MEM_DEVICE_LOCATOR {
        fixed[OFF_MEM_DEVICE_LOCATOR]
    } else {
        0
    };
    let dev_ref = if fixed.len() > OFF_DEVICE_LOCATOR {
        fixed[OFF_DEVICE_LOCATOR]
    } else {
        0
    };
    let locator = string_at(table, mem_ref)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| string_at(table, dev_ref))
        .unwrap_or_default();

    Some((capacity, mem_type, locator))
}

/// Extract the `ref_idx`-th string from a raw SMBIOS string table
/// (null-separated; string `0` is the leading empty terminator).
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
    // A real string reference points to a non-empty segment; the trailing
    // null terminator yields an empty segment, which is not a string.
    parts.get(ref_idx as usize)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|s| !s.is_empty())
}

/// The channel designation (A–D) of a locator string, if one is present.
///
/// A channel letter is recognized when a token (split on non-alphanumeric
/// runs) is exactly one A–D letter (e.g. `A` in `CPU0_DIMM_A`) or an A–D
/// letter immediately followed by a digit (e.g. `A1`, `B2`). This
/// distinguishes a channel designation from an A–D letter that is merely
/// the start of a word (the `D` in `DIMM` is never a channel).
fn channel_of(locator: &str) -> Option<char> {
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
    /// (`strings[0]` is the leading empty terminator, then one string per
    /// reference). Returns the full file image the kernel exports.
    fn type17_file(
        len: usize,
        fixed: &mut [u8],
        strings: &[&str],
    ) -> Vec<u8> {
        assert!(fixed.len() >= len, "fixed must be at least `len` bytes");
        fixed[OFF_LENGTH] = len as u8;
        let mut out = fixed.to_vec();
        // String table: leading empty terminator + each string + terminator.
        out.push(0);
        for s in strings {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out
    }

    /// Write a synthetic `17-<n>` file into `dir`.
    fn write_entry(dir: &Path, n: u8, bytes: &[u8]) {
        let mut f = std::fs::File::create(dir.join(format!("17-{n}"))).expect("entry create");
        f.write_all(bytes).expect("entry write");
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

    /// A populated DDR4 DIMM in slot `A2`, 16 GiB (capacity 16384 MiB).
    /// String table: [0]=terminator, [1]="Memory Device Locator" value
    /// (ref 1 = "A2"), [2]="Device Locator" value (ref 2 = "A2").
    fn populated_ddr4(locator: &str) -> Vec<u8> {
        let mut fixed = [0u8; 0x2C];
        fixed[OFF_MEMORY_TYPE] = 0x1A; // DDR4
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_DEVICE_LOCATOR] = 2;
        let cap: u32 = 16384; // 16 GiB in MiB
        fixed[OFF_CAPACITY..OFF_CAPACITY + 4].copy_from_slice(&cap.to_le_bytes());
        type17_file(0x2C, &mut fixed, &[locator, locator])
    }

    /// An unpopulated DIMM (capacity 0) — must be skipped.
    fn unpopulated() -> Vec<u8> {
        let mut fixed = [0u8; 0x2C];
        fixed[OFF_MEMORY_TYPE] = 0x1A;
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_DEVICE_LOCATOR] = 2;
        // capacity stays 0
        type17_file(0x2C, &mut fixed, &["A1", "A1"])
    }

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
        let mut fixed = [0u8; 0x2C];
        fixed[OFF_MEMORY_TYPE] = 0x22; // DDR5
        fixed[OFF_MEM_DEVICE_LOCATOR] = 1;
        fixed[OFF_DEVICE_LOCATOR] = 2;
        let cap: u32 = 16384;
        fixed[OFF_CAPACITY..OFF_CAPACITY + 4].copy_from_slice(&cap.to_le_bytes());
        let bytes = type17_file(0x2C, &mut fixed, &["A2", "A2"]);

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
    /// (single letter, letter+digit) but not an A–D letter that starts a
    /// word (`DIMM`, `RAM`).
    #[test]
    fn channel_of_recognizes_designations_only() {
        assert_eq!(channel_of("A2"), Some('A'));
        assert_eq!(channel_of("DIMM A1"), Some('A'));
        assert_eq!(channel_of("CPU0_DIMM_B"), Some('B'));
        assert_eq!(channel_of("Channel A Slot 1"), Some('A'));
        // No channel designation.
        assert_eq!(channel_of("DIMM 1"), None);
        assert_eq!(channel_of("Slot 1"), None);
        assert_eq!(channel_of("RAM"), None);
        assert_eq!(channel_of(""), None);
    }

    /// (k) `string_at` extracts the right null-separated string and is
    /// safe on an out-of-range reference (no panic).
    #[test]
    fn string_at_extracts_and_bounds() {
        let table = [0, b'A', b'2', 0, b'B', b'2', 0];
        assert_eq!(string_at(&table, 1), Some("A2".to_owned()));
        assert_eq!(string_at(&table, 2), Some("B2".to_owned()));
        assert_eq!(string_at(&table, 0), None);
        assert_eq!(string_at(&table, 3), None);
        assert_eq!(string_at(&table, 99), None);
    }
}
