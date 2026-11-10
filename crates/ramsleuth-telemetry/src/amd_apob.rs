//! APOB (AGESA Pre-Output Buffer) acquisition and parsing.
//!
//! The APOB is a firmware data structure written to physical DRAM during
//! early boot (AGESA Phase 1 on the Platform Security Processor). It contains
//! the results of memory training, fabric topology, CPU core mapping, and
//! board configuration — the complete platform state as determined by firmware
//! before OS handoff.
//!
//! On sIOD platforms (Threadripper TRX40/WRX80, EPYC), the APOB Group 7 Type 3
//! entry contains the trained DRAM timings in plaintext — the only
//! OS-accessible source of live timing data where the SMN registers hold
//! static JEDEC defaults.
//!
//! # Acquisition
//!
//! The raw APOB blob is read from the `ryzen_smu` driver's sysfs interface:
//! `/sys/kernel/ryzen_smu_drv/apob` (canonical) or
//! `/sys/kernel/ryzen_smu/apob` (legacy fallback). The kernel module maps
//! the APOB table from physical RAM via `ioremap_cache()` and exposes it
//! as a read-only sysfs attribute.
//!
//! # Format (source: work/docs/research/APOB-full-data-landscape.md)
//!
//! - **Variable-size header** (size given by `HeaderSize` at offset 0x0C):
//!   - Signature (u32, 0x424F5041), Version (u32), TableSize (u32),
//!     HeaderSize (u32), 32 B reserved, Config Offset List (array of u32),
//!     32 B HMAC.
//! - **48-byte entry headers**: RawGroupId (u32), RawDataTypeId (u32),
//!   InstanceId (u32), Size (u32, total including 48-byte header),
//!   32 B HMAC, then payload (Size − 48 bytes).
//! - **Chaining**: next entry = current offset + Size. The main chain starts
//!   at HeaderSize; each u32 in the Config Offset List starts another
//!   independent chain.
//! - **Encryption**: an entry is encrypted when the upper 16 bits of
//!   RawGroupId or RawDataTypeId are 0xFFFF. The actual group/type is in
//!   the lower 16 bits.
//!
//! # Timing data
//!
//! Per-channel DDR4 timing blocks are in **Group 7 (GEN) Type 3**
//! (`APOB_GEN_CONFIGURATION_INFO_TYPE`). The layout is platform-specific
//! and uses a scan-based block locator (MEMCLK + gap + timing bytes), not
//! fixed-offset blocks. See [`extract_ddr4_timings`] for details.
//!
//! # Safety
//!
//! No `unsafe` code. All reads go through bounds-checked `get()` calls.
//! All values are read as little-endian. The blob is trusted to come from
//! the kernel module, which has already validated the signature and size.
//! All parsing is total: malformed input yields errors or `None`, never
//! panics.

use std::fs;

/// sysfs paths for the APOB raw blob, tried in order.
///
/// The upstream `ryzen_smu` module registers its kobject as `ryzen_smu_drv`
/// (amkillam/ryzen_smu `drv.c`: `kobject_create_and_add("ryzen_smu_drv",
/// kernel_kobj)`), so the canonical path is
/// `/sys/kernel/ryzen_smu_drv/apob`; the legacy `ryzen_smu` directory name
/// is kept as a fallback for older or renamed builds.
const SYSFS_APOB_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/apob",
    "/sys/kernel/ryzen_smu/apob",
];

/// sysfs paths for the APOB available flag, tried in order.
const SYSFS_APOB_AVAILABLE_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/apob_available",
    "/sys/kernel/ryzen_smu/apob_available",
];

/// sysfs paths for the APOB size attribute, tried in order.
const SYSFS_APOB_SIZE_CANDIDATES: [&str; 2] = [
    "/sys/kernel/ryzen_smu_drv/apob_size",
    "/sys/kernel/ryzen_smu/apob_size",
];

/// APOB magic signature: "APOB" in little-endian.
///
/// Source: ZenStates-Core `Hardware/Apob/ApobHeader.cs`;
/// research doc §1.1.
const APOB_SIGNATURE: u32 = 0x424F5041;

/// Maximum reasonable APOB table size (matches the kernel module's cap).
///
/// Source: research doc §1.2.
const APOB_MAX_SIZE: usize = 1024 * 1024;

/// Fixed portion of the APOB header before the Config Offset List:
/// signature (4) + version (4) + table_size (4) + header_size (4) +
/// reserved (32) = 48 bytes.
const APOB_HEADER_FIXED_SIZE: usize = 0x30;

/// Size of the per-entry HMAC trailer at the end of the header.
const APOB_HEADER_HMAC_SIZE: usize = 32;

/// APOB entry header size in bytes (RawGroupId u32 + RawDataTypeId u32 +
/// InstanceId u32 + Size u32 + 32-byte HMAC).
///
/// Source: research doc §1.3.
const APOB_ENTRY_HEADER_SIZE: usize = 48;

/// Group 7 (GEN) Type 3: the configuration info entry containing
/// per-channel timing blocks.
///
/// Source: research doc §2.8.
const APOB_GROUP_GEN: u16 = 7;
const APOB_TYPE_CONFIGURATION_INFO: u16 = 3;

/// Group 4 (GNB) Type 1: the board configuration entry that contains
/// the board identifier, PCB revision string, and AGESA release build
/// numbers.
///
/// Source: research doc §2.5.
const APOB_GROUP_GNB: u16 = 4;
const APOB_TYPE_BOARD_CONFIG: u16 = 1;

/// P-state block stride for Zen 1/2 DDR4 platforms (Summit Ridge,
/// Matisse, Castle Peak, Rome): 0x70 = 112 bytes.
///
/// Source: research doc §3.2.
const DDR4_ZEN2_STRIDE: usize = 0x70;

/// P-state block stride for Zen 3 DDR4 platforms (Vermeer, Chagall,
/// Milan): 0x108 = 264 bytes.
///
/// Source: research doc §3.2.
const DDR4_ZEN3_STRIDE: usize = 0x108;

/// Minimum plausible DDR4 memory clock (MHz) for scan validation.
const DDR4_MEMCLK_MIN: u16 = 400;

/// Maximum plausible DDR4 memory clock (MHz) for scan validation.
const DDR4_MEMCLK_MAX: u16 = 2400;

/// Minimum supported APOB format version.
///
/// Versions 5+ use the modern chained 48-byte entry format. Versions
/// 1-4 are legacy (pre-chained-entry format) and not supported by this
/// parser. Source: AMD openSIL xUSL/Apob.c.
pub const APOB_MIN_VERSION: u32 = 5;

/// A parsed APOB header (variable size).
///
/// The header size is given by `header_size` (AMD's `OffsetOfFirstEntry`
/// — the byte offset where the first entry starts). The region from
/// `APOB_HEADER_FIXED_SIZE` (0x30) to `header_size -
/// APOB_HEADER_HMAC_SIZE` (32) contains the Config Offset List — an
/// array of u32 offsets to additional entry chains. The last 32 bytes
/// of the header are an HMAC (ignored for parsing).
///
/// Source: research doc §1.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApobHeader {
    /// APOB format version (raw u32 from the header; must be >= 5).
    pub version: u32,
    /// Total table size in bytes (including header and all entries).
    pub table_size: u32,
    /// Offset of the first entry in the main chain (AMD's
    /// `OffsetOfFirstEntry`); equivalent to the header size.
    pub header_size: u32,
    /// Config Offset List: u32 offsets to additional entry chains that
    /// are not reachable by following the main chain.
    pub config_offsets: Vec<u32>,
}

/// A single APOB entry with its payload.
///
/// Source: research doc §1.3.
#[derive(Debug, Clone)]
pub struct ApobEntry {
    /// Subsystem group ID (lower 16 bits of RawGroupId).
    ///
    /// Groups: 1=MEM, 2=DF, 3=CCX, 4=GNB, 5=FCH, 6=PSP, 7=GEN,
    /// 8=SMBIOS, 9=FABRIC.
    pub group: u16,
    /// Data type ID within the group (lower 16 bits of RawDataTypeId).
    pub entry_type: u16,
    /// Die/socket/controller instance (zero-indexed).
    pub instance: u32,
    /// Whether this entry is encrypted (upper 16 bits of RawGroupId or
    /// RawDataTypeId are 0xFFFF). Encrypted payloads are not decoded.
    pub encrypted: bool,
    /// The raw payload bytes (Size − 48).
    pub data: Vec<u8>,
}

/// A parsed APOB table: header + all entries.
#[derive(Debug, Clone)]
pub struct ApobData {
    /// The APOB header.
    pub header: ApobHeader,
    /// The physical address where the APOB was discovered (from the
    /// kernel module, for diagnostics).
    pub source_address: Option<u64>,
    /// All parsed entries from the main chain and all config offset
    /// chains (encrypted entries are included but marked).
    pub entries: Vec<ApobEntry>,
}

/// Per-channel DRAM timings extracted from the APOB Group 7 Type 3
/// entry.
///
/// The field layout follows the scan-based DDR4 block locator
/// (research doc §3.2). The `memclk_mhz` field is the memory clock
/// frequency; the remaining fields are the decoded timing parameters.
///
/// Note: the exact mapping of the scan-located bytes to specific timing
/// parameters is tentative (worked out from Summit Ridge, Matisse,
/// Cezanne, and Vermeer dumps at JEDEC DDR4-2133 15-15-15). The order
/// CL, CWL, RCD, RP, RTP, RAS, RC, WR, RRDS, WTRS, FAW, RRDL was
/// determined empirically; some placements are tentative where values
/// are equal.
///
/// The `trfc_*`, `trdwr`, `twrtr`, and `cmd_rate` fields are set to 0
/// when not directly available from the scan; the facade's overlay
/// function has a sanity gate (1..=2048) that skips zero values, so
/// these do not pollute the readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApobChannelTimings {
    /// Channel index (assigned sequentially during the scan).
    pub channel: u8,
    /// Memory clock frequency in MHz (the u16 at the start of the block).
    pub memclk_mhz: u16,
    /// CAS latency (cycles).
    pub tcl: u8,
    /// RAS to CAS delay (write) (cycles).
    pub trcdw: u8,
    /// RAS to CAS delay (read) (cycles).
    pub trcdr: u8,
    /// Row precharge time (cycles).
    pub trp: u8,
    /// Row active time (cycles).
    pub tras: u16,
    /// Row cycle time (tRAS + tRP) (cycles).
    pub trc: u16,
    /// Row to row delay (short, same bank group) (cycles).
    pub trrd_s: u8,
    /// Row to row delay (long, different bank group) (cycles).
    pub trrd_l: u8,
    /// Four-activate window (cycles).
    pub tfaw: u16,
    /// Write to read delay (short) (cycles).
    pub twtr_s: u8,
    /// Write to read delay (long) (cycles).
    pub twtr_l: u8,
    /// Write recovery time (cycles).
    pub twr: u16,
    /// Refresh cycle time (standard) (cycles).
    pub trfc_s: u16,
    /// Refresh cycle time (fine) (cycles).
    pub trfc_f: u16,
    /// Refresh cycle time (page bank) (cycles).
    pub trfc_pb: u16,
    /// CAS write latency (cycles).
    pub tcwl: u8,
    /// Read to precharge time (cycles).
    pub trtp: u8,
    /// Read to write turnaround (cycles).
    pub trdwr: u8,
    /// Write to read turnaround (cycles).
    pub twrtr: u8,
    /// Command rate: 0 = 1T, 1 = 2T, 2 = Gear Down Mode.
    pub cmd_rate: u8,
}

/// The extracted APOB timing data (per-channel).
#[derive(Debug, Clone)]
pub struct ApobTimings {
    /// One entry per timing block found in the APOB Group 7 Type 3
    /// payload (each block is a channel's P-state configuration).
    pub channels: Vec<ApobChannelTimings>,
}

/// Errors specific to APOB acquisition and parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApobError {
    /// The APOB is not available (kernel module not loaded, or APOB not
    /// found at any candidate address).
    NotFound,
    /// The APOB signature was not found in the blob.
    BadSignature,
    /// The APOB blob is truncated (shorter than the declared table size
    /// or header size).
    Truncated { expected: usize, actual: usize },
    /// The APOB format version is not supported.
    UnsupportedVersion(u32),
    /// An entry's payload extends beyond the table boundary.
    EntryOverrun {
        entry_index: usize,
        group: u16,
        type_id: u16,
    },
    /// The timing entry (Group 7 Type 3) was not found.
    TimingsNotFound,
    /// The timing entry is encrypted (upper 16 bits of GroupId or
    /// DataTypeId are 0xFFFF).
    TimingsEncrypted,
    /// The timing payload is malformed (no valid timing blocks found).
    InvalidTimingData(String),
    /// I/O error reading from sysfs.
    Io(String),
}

impl std::fmt::Display for ApobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(
                f,
                "APOB not found (no valid signature at any candidate address)"
            ),
            Self::BadSignature => write!(f, "APOB signature mismatch"),
            Self::Truncated { expected, actual } => write!(
                f,
                "APOB blob truncated: expected {} bytes, got {}",
                expected, actual
            ),
            Self::UnsupportedVersion(v) => write!(f, "APOB unsupported version: 0x{:08x}", v),
            Self::EntryOverrun {
                entry_index,
                group,
                type_id,
            } => write!(
                f,
                "APOB entry {} (group {}, type {}) overruns table boundary",
                entry_index, group, type_id
            ),
            Self::TimingsNotFound => write!(f, "APOB timings entry (Group 7 Type 3) not found"),
            Self::TimingsEncrypted => write!(f, "APOB timings entry is encrypted"),
            Self::InvalidTimingData(msg) => write!(f, "APOB invalid timing data: {}", msg),
            Self::Io(msg) => write!(f, "APOB I/O error: {}", msg),
        }
    }
}

impl std::error::Error for ApobError {}

/// Check if the APOB is available from the `ryzen_smu` sysfs interface.
///
/// This is a lightweight probe (reads only the `apob_available` flag) that
/// lets the facade distinguish between three failure modes on a sIOD
/// platform:
///
/// - `Ok(true)` — the module is loaded, has APOB support, and the APOB
///   was found at a candidate address (`apob_available` = 1).
/// - `Ok(false)` — the module is loaded and has APOB support, but the APOB
///   was not found at any candidate address (`apob_available` = 0).
/// - `Err(ApobError::NotFound)` — the `apob_available` sysfs attribute
///   does not exist (the module is not loaded, or is an old version
///   without APOB support).
/// - `Err(ApobError::Io(..))` — an I/O error reading the attribute.
///
/// Source: the `ryzen_smu` kernel module's `apob_available` sysfs attribute.
pub fn apob_is_available() -> Result<bool, ApobError> {
    read_apob_available()
}

/// Read the raw APOB blob from the `ryzen_smu` sysfs interface.
///
/// Reads the `apob_available` flag first. If 0, returns
/// `Err(ApobError::NotFound)`. Then reads the `apob_size` attribute to
/// determine how many bytes to expect. Finally reads the raw `apob` blob.
///
/// # Errors
///
/// - `ApobError::NotFound` — the APOB is not available (kernel module not
///   loaded, or APOB not found at any candidate address).
/// - `ApobError::Io` — sysfs read failure.
/// - `ApobError::Truncated` — the blob is shorter than the declared size.
pub fn read_apob_from_sysfs() -> Result<Vec<u8>, ApobError> {
    // 1. Check availability.
    let available = read_apob_available()?;
    if !available {
        return Err(ApobError::NotFound);
    }

    // 2. Read the declared size.
    let declared_size = read_apob_size()?;
    if declared_size == 0 || declared_size as usize > APOB_MAX_SIZE {
        return Err(ApobError::Truncated {
            expected: declared_size as usize,
            actual: 0,
        });
    }

    // 3. Read the raw blob.
    let blob = read_apob_blob()?;

    // 4. Validate the blob length.
    if blob.len() < declared_size as usize {
        return Err(ApobError::Truncated {
            expected: declared_size as usize,
            actual: blob.len(),
        });
    }

    // Return exactly the declared size (ignore any trailing padding).
    Ok(blob[..declared_size as usize].to_vec())
}

/// Read the `apob_available` flag from sysfs.
fn read_apob_available() -> Result<bool, ApobError> {
    for path in SYSFS_APOB_AVAILABLE_CANDIDATES {
        match fs::read_to_string(path) {
            Ok(s) => {
                let trimmed = s.trim();
                return Ok(trimmed == "1");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(ApobError::Io(e.to_string())),
        }
    }
    // No candidate found — the driver is not loaded or doesn't expose APOB.
    Err(ApobError::NotFound)
}

/// Read the `apob_size` attribute from sysfs.
fn read_apob_size() -> Result<u32, ApobError> {
    for path in SYSFS_APOB_SIZE_CANDIDATES {
        match fs::read_to_string(path) {
            Ok(s) => {
                let trimmed = s.trim();
                return match trimmed.parse::<u32>() {
                    Ok(v) => Ok(v),
                    Err(_) => Err(ApobError::Io(format!(
                        "apob_size is not a valid number: '{}'",
                        trimmed
                    ))),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(ApobError::Io(e.to_string())),
        }
    }
    Err(ApobError::NotFound)
}

/// Read the raw APOB blob from sysfs.
fn read_apob_blob() -> Result<Vec<u8>, ApobError> {
    for path in SYSFS_APOB_CANDIDATES {
        match fs::read(path) {
            Ok(blob) => return Ok(blob),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(ApobError::Io(e.to_string())),
        }
    }
    Err(ApobError::NotFound)
}

/// Read a little-endian u32 from `blob` at `offset`.
///
/// Returns `None` if the read would exceed the blob bounds.
#[inline]
fn read_u32_le(blob: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    if end > blob.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        blob[offset],
        blob[offset + 1],
        blob[offset + 2],
        blob[offset + 3],
    ]))
}

/// Read a little-endian u16 from `blob` at `offset`.
///
/// Returns `None` if the read would exceed the blob bounds.
#[inline]
fn read_u16_le(blob: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    if end > blob.len() {
        return None;
    }
    Some(u16::from_le_bytes([blob[offset], blob[offset + 1]]))
}

/// Parse a raw APOB blob into a structured [`ApobData`].
///
/// # Validation
///
/// 1. The blob must be at least large enough for the fixed header portion
///    (48 bytes) plus the 32-byte HMAC (80 bytes total minimum).
/// 2. The signature (first 4 bytes, LE) must be [`APOB_SIGNATURE`].
/// 3. The version must be ≥ [`APOB_MIN_VERSION`] (5) — versions 1-4 are
///    legacy and not supported.
/// 4. The table size must be ≤ the blob length and ≤ [`APOB_MAX_SIZE`].
/// 5. The header size (OffsetOfFirstEntry) must be ≥ 80 (fixed + HMAC)
///    and ≤ the table size.
/// 6. Each entry's payload must fit within the table boundary.
/// 7. Encrypted entries (upper 16 bits of RawGroupId or RawDataTypeId
///    are 0xFFFF) are included but marked — their payloads are not
///    decoded.
///
/// # Entry discovery
///
/// Entries are discovered by walking chains:
/// - The main chain starts at offset `header_size`.
/// - Each u32 in the Config Offset List (from 0x30 to `header_size - 32`)
///   starts an additional independent chain.
/// - Within a chain, the next entry is at `current_offset + Size`.
/// - A chain terminates when the next offset would exceed the table size,
///   or when `Size` is 0 or < 48, or when the entry header would exceed
///   the blob bounds.
///
/// # Errors
///
/// See [`ApobError`] for the full set of error variants.
pub fn parse_apob(blob: &[u8]) -> Result<ApobData, ApobError> {
    // 1. Minimum length check: fixed header (48) + HMAC (32) = 80 bytes.
    let min_header = APOB_HEADER_FIXED_SIZE + APOB_HEADER_HMAC_SIZE;
    if blob.len() < min_header {
        return Err(ApobError::Truncated {
            expected: min_header,
            actual: blob.len(),
        });
    }

    // 2. Signature check (little-endian u32 at offset 0).
    let signature = read_u32_le(blob, 0).ok_or(ApobError::Truncated {
        expected: 4,
        actual: blob.len(),
    })?;
    if signature != APOB_SIGNATURE {
        return Err(ApobError::BadSignature);
    }

    // 3. Version check (little-endian u32 at offset 4).
    // Versions 5+ use the modern chained 48-byte entry format; versions
    // 1-4 are legacy and not supported (AMD openSIL xUSL/Apob.c rule).
    let version = read_u32_le(blob, 4).ok_or(ApobError::Truncated {
        expected: 8,
        actual: blob.len(),
    })?;
    if version < APOB_MIN_VERSION {
        return Err(ApobError::UnsupportedVersion(version));
    }

    // 4. Table size check (little-endian u32 at offset 8).
    let table_size = read_u32_le(blob, 8).ok_or(ApobError::Truncated {
        expected: 12,
        actual: blob.len(),
    })? as usize;
    if table_size == 0 || table_size > APOB_MAX_SIZE {
        return Err(ApobError::Truncated {
            expected: table_size,
            actual: blob.len(),
        });
    }
    if blob.len() < table_size {
        return Err(ApobError::Truncated {
            expected: table_size,
            actual: blob.len(),
        });
    }

    // 5. Header size check (little-endian u32 at offset 12).
    let header_size = read_u32_le(blob, 12).ok_or(ApobError::Truncated {
        expected: 16,
        actual: blob.len(),
    })? as usize;
    if header_size < min_header || header_size > table_size {
        return Err(ApobError::Truncated {
            expected: min_header,
            actual: header_size,
        });
    }

    // 6. Config Offset List: u32 array from 0x30 to header_size - 32.
    //    The count is (header_size - 32 - 0x30) / 4.
    let config_list_start = APOB_HEADER_FIXED_SIZE;
    let config_list_end = header_size - APOB_HEADER_HMAC_SIZE;
    let mut config_offsets = Vec::new();
    if config_list_end > config_list_start {
        let count = (config_list_end - config_list_start) / 4;
        for i in 0..count {
            let off = config_list_start + i * 4;
            if let Some(v) = read_u32_le(blob, off) {
                config_offsets.push(v);
            }
        }
    }

    let header = ApobHeader {
        version,
        table_size: table_size as u32,
        header_size: header_size as u32,
        config_offsets,
    };

    // 7. Walk the main chain starting at header_size.
    let mut entries = Vec::new();
    walk_chain(blob, table_size, header_size, &mut entries)?;

    // 8. Walk each config offset chain.
    for &config_offset in &header.config_offsets {
        if config_offset != 0 {
            walk_chain(blob, table_size, config_offset as usize, &mut entries)?;
        }
    }

    Ok(ApobData {
        header,
        source_address: None, // populated by the caller if known
        entries,
    })
}

/// Walk a single entry chain, starting at `start_offset`.
///
/// Each entry has a 48-byte header: RawGroupId (u32), RawDataTypeId (u32),
/// InstanceId (u32), Size (u32, total including the 48-byte header),
/// 32-byte HMAC, then the payload (Size - 48 bytes).
///
/// The next entry in the chain is at `current_offset + Size`. The walk
/// terminates when:
/// - The next offset would exceed `table_size`.
/// - `Size` is 0 or < 48 (invalid).
/// - The entry header would exceed the blob bounds.
///
/// All bounds are checked; malformed entries terminate the chain without
/// an error (the caller may have other valid entries).
fn walk_chain(
    blob: &[u8],
    table_size: usize,
    start_offset: usize,
    entries: &mut Vec<ApobEntry>,
) -> Result<(), ApobError> {
    let mut offset = start_offset;
    let mut entry_index = entries.len();

    while offset + APOB_ENTRY_HEADER_SIZE <= table_size {
        // Read the 48-byte entry header.
        if offset + APOB_ENTRY_HEADER_SIZE > blob.len() {
            break;
        }

        let raw_group = read_u32_le(blob, offset).ok_or(ApobError::EntryOverrun {
            entry_index,
            group: 0,
            type_id: 0,
        })?;
        let raw_type = read_u32_le(blob, offset + 4).ok_or(ApobError::EntryOverrun {
            entry_index,
            group: (raw_group & 0xFFFF) as u16,
            type_id: 0,
        })?;
        let instance = read_u32_le(blob, offset + 8).ok_or(ApobError::EntryOverrun {
            entry_index,
            group: (raw_group & 0xFFFF) as u16,
            type_id: (raw_type & 0xFFFF) as u16,
        })?;
        let size = read_u32_le(blob, offset + 12).ok_or(ApobError::EntryOverrun {
            entry_index,
            group: (raw_group & 0xFFFF) as u16,
            type_id: (raw_type & 0xFFFF) as u16,
        })? as usize;

        // Validate the size: must be at least the 48-byte header and
        // must not extend past the table boundary.
        if size == 0 || size < APOB_ENTRY_HEADER_SIZE {
            // Size < 48 means this is not a valid entry header — the
            // chain is broken. Stop walking this chain.
            break;
        }
        let entry_end = offset + size;
        if entry_end > table_size {
            return Err(ApobError::EntryOverrun {
                entry_index,
                group: (raw_group & 0xFFFF) as u16,
                type_id: (raw_type & 0xFFFF) as u16,
            });
        }

        // Decode the group and type (lower 16 bits).
        let group = (raw_group & 0xFFFF) as u16;
        let entry_type = (raw_type & 0xFFFF) as u16;
        // Encryption: upper 16 bits of either field are 0xFFFF.
        let encrypted = (raw_group >> 16) == 0xFFFF || (raw_type >> 16) == 0xFFFF;

        // Extract the payload (Size - 48 bytes).
        let data_start = offset + APOB_ENTRY_HEADER_SIZE;
        let data_len = size - APOB_ENTRY_HEADER_SIZE;
        if data_start + data_len > blob.len() {
            // The entry end is within the table, but the blob is shorter
            // than the table (trailing padding was stripped). Use what
            // we have.
            let available = blob.len().saturating_sub(data_start);
            if available == 0 {
                break;
            }
            entries.push(ApobEntry {
                group,
                entry_type,
                instance,
                encrypted,
                data: blob[data_start..data_start + available].to_vec(),
            });
        } else {
            entries.push(ApobEntry {
                group,
                entry_type,
                instance,
                encrypted,
                data: blob[data_start..data_start + data_len].to_vec(),
            });
        }

        entry_index += 1;
        // Next entry in the chain.
        offset = entry_end;
    }

    Ok(())
}

/// Extract DDR4 timing blocks from the APOB Group 7 Type 3 entries.
///
/// This is the primary timing extraction function. It searches for
/// Group 7 (GEN) Type 3 (Configuration Info) entries and decodes the
/// per-channel timing blocks using a scan-based locator.
///
/// # Scan-based block locator
///
/// The timing blocks are not at fixed offsets. Instead, each block:
/// 1. Starts with a MEMCLK (u16, MHz) in the range 400–2400.
/// 2. Is followed by a gap of 2–10 bytes (program-dependent).
/// 3. Then 14 timing fields (see the struct docs for the tentative
///    mapping).
///
/// The algorithm scans the payload at every 2-byte boundary. When a
/// valid MEMCLK is found, it tries each possible gap (2–10 bytes) and
/// validates the resulting timing values. If a valid block is found,
/// the scan jumps forward by the P-state stride (platform-dependent)
/// to find the next P-state for the same channel. If no valid block is
/// found at the current position, the scan advances by 2 bytes.
///
/// # Platform stride
///
/// - Zen 1 / Zen 2: 0x70 (112 bytes)
/// - Zen 3: 0x108 (264 bytes)
/// - Zen 4 / Zen 5: not supported (DDR5 uses a different layout);
///   returns `None`.
///
/// # Returns
///
/// - `Some(vec)` — a non-empty list of decoded timing blocks (each block
///   is a channel's P-state configuration). The blocks are in scan order
///   (channel 0 P-state 0, channel 0 P-state 1, ..., channel 1 P-state 0,
///   ...).
/// - `None` — no Group 7 Type 3 entries found, all are encrypted, no
///   valid timing blocks could be located, or the platform uses DDR5
///   (Zen 4/5).
///
/// Never panics: all reads are bounds-checked.
pub fn extract_ddr4_timings(
    entries: &[ApobEntry],
    platform: crate::cpuid::AmdZen,
) -> Option<Vec<ApobChannelTimings>> {
    // Select the P-state stride based on the platform.
    let stride = match platform {
        crate::cpuid::AmdZen::Zen1 | crate::cpuid::AmdZen::Zen2 => DDR4_ZEN2_STRIDE,
        crate::cpuid::AmdZen::Zen3 => DDR4_ZEN3_STRIDE,
        // Zen 4/5 use DDR5 with a different layout — not supported yet.
        _ => return None,
    };

    // Find all Group 7 Type 3 entries (not encrypted).
    let timing_entries: Vec<&ApobEntry> = entries
        .iter()
        .filter(|e| e.group == APOB_GROUP_GEN && e.entry_type == APOB_TYPE_CONFIGURATION_INFO)
        .filter(|e| !e.encrypted)
        .collect();

    if timing_entries.is_empty() {
        return None;
    }

    let mut all_blocks = Vec::new();

    for entry in &timing_entries {
        let blocks = scan_ddr4_blocks(&entry.data, stride);
        all_blocks.extend(blocks);
    }

    if all_blocks.is_empty() {
        None
    } else {
        Some(all_blocks)
    }
}

/// Scan a Group 7 Type 3 payload for DDR4 timing blocks.
///
/// This implements the scan-based block locator described in the
/// research doc (§3.1, §3.2). The algorithm:
///
/// 1. Iterate over every 2-byte boundary in the payload.
/// 2. At each boundary, read a u16 as a candidate MEMCLK.
/// 3. If MEMCLK is in the valid range (400–2400 MHz), try each possible
///    gap (2–10 bytes) between MEMCLK and the timing fields.
/// 4. For each gap, read the timing fields and validate that they are
///    plausible (CL/CWL/RCD/RP in 8–64, RAS/RC in 8–255, etc.).
/// 5. If a valid block is found, record it and jump forward by `stride`
///    to find the next P-state for the same channel.
/// 6. If no valid block is found at the current position, advance by 2
///    bytes to try the next position.
///
/// # Tentative field mapping
///
/// The 14 timing fields after the gap are mapped (tentatively) as:
/// CL, CWL, RCD, RP, RTP, RAS (u16), RC (u16), WR, RRDS, WTRS, FAW, RRDL.
/// The exact order was determined empirically from Summit Ridge, Matisse,
/// Cezanne, and Vermeer dumps at JEDEC DDR4-2133 15-15-15; some
/// placements are tentative where values are equal.
///
/// The tRFC fields are read from the 3 u16 values that follow the 14
/// timing fields, when the payload is large enough.
///
/// Never panics: all reads are bounds-checked.
fn scan_ddr4_blocks(data: &[u8], stride: usize) -> Vec<ApobChannelTimings> {
    let mut blocks = Vec::new();
    let mut channel = 0u8;
    let mut pos = 0usize;

    while pos + 2 <= data.len() {
        // Read candidate MEMCLK.
        let memclk = match read_u16_le(data, pos) {
            Some(v) => v,
            None => break,
        };

        // Valid DDR4 memory clock range.
        if !(DDR4_MEMCLK_MIN..=DDR4_MEMCLK_MAX).contains(&memclk) {
            pos += 2;
            continue;
        }

        // Try each possible gap between MEMCLK and the timing fields.
        let mut found = false;
        for gap in 2..=10 {
            let timings_start = pos + 2 + gap;
            // We need at least 14 bytes of timing fields (CL, CWL, RCD,
            // RP, RTP are u8; RAS, RC are u16; WR, RRDS, WTRS, FAW,
            // RRDL are u8) = 5 + 2 + 2 + 5 = 14 bytes.
            if timings_start + 14 > data.len() {
                break;
            }

            let cl = data[timings_start];
            let cwl = data[timings_start + 1];
            let rcw = data[timings_start + 2];
            let rp = data[timings_start + 3];

            // Validate the primary timing parameters.
            if !((8..=64).contains(&cl)
                && (8..=64).contains(&cwl)
                && (8..=64).contains(&rcw)
                && (8..=64).contains(&rp))
            {
                continue;
            }

            let rtp = data[timings_start + 4];
            let ras = u16::from_le_bytes([data[timings_start + 5], data[timings_start + 6]]);
            let rc = u16::from_le_bytes([data[timings_start + 7], data[timings_start + 8]]);
            let wr = data[timings_start + 9];
            let rrds = data[timings_start + 10];
            let wtrs = data[timings_start + 11];
            let faw = data[timings_start + 12];
            let rrld = data[timings_start + 13];

            // Validate RAS and RC (u16 fields, should be in a plausible
            // range for DDR4).
            if !((8..=255).contains(&ras) && (8..=255).contains(&rc)) {
                continue;
            }
            // RC should be >= RAS + RP (row cycle = active time +
            // precharge). This is a strong validation check.
            if rc < ras + rp as u16 {
                continue;
            }

            // Try to read the tRFC fields (3 u16 values after the 14
            // timing fields) if the payload is large enough.
            let trfc_start = timings_start + 14;
            let (trfc_s, trfc_f, trfc_pb) = if trfc_start + 6 <= data.len() {
                let v1 = u16::from_le_bytes([data[trfc_start], data[trfc_start + 1]]);
                let v2 = u16::from_le_bytes([data[trfc_start + 2], data[trfc_start + 3]]);
                let v3 = u16::from_le_bytes([data[trfc_start + 4], data[trfc_start + 5]]);
                // Sanity: tRFC values should be plausible (not zero,
                // not absurdly large).
                if (1..=4096).contains(&v1) && (1..=4096).contains(&v2) && (1..=4096).contains(&v3)
                {
                    (v1, v2, v3)
                } else {
                    (0, 0, 0)
                }
            } else {
                (0, 0, 0)
            };

            // Valid block found.
            blocks.push(ApobChannelTimings {
                channel,
                memclk_mhz: memclk,
                tcl: cl,
                trcdw: rcw,
                trcdr: rcw, // tRCD is the same for read and write in DDR4
                trp: rp,
                tras: ras,
                trc: rc,
                trrd_s: rrds,
                trrd_l: rrld,
                tfaw: faw as u16,
                twtr_s: wtrs,
                twtr_l: 0, // not directly available from the scan
                twr: wr as u16,
                trfc_s,
                trfc_f,
                trfc_pb,
                tcwl: cwl,
                trtp: rtp,
                trdwr: 0,    // not directly available from the scan
                twrtr: 0,    // not directly available from the scan
                cmd_rate: 0, // not directly available from the scan
            });

            channel = channel.saturating_add(1);
            found = true;

            // Jump to the next P-state for this channel.
            pos += stride;
            break;
        }

        if !found {
            // No valid block at this position — advance by 2 bytes.
            pos += 2;
        }
    }

    blocks
}

/// Extract the sIOD DDR4 timings from an [`ApobData`].
///
/// This is a convenience wrapper around the scan-based extractor that
/// tries both Zen 2 and Zen 3 strides (since the platform is not
/// specified). It is kept for backward compatibility with callers that
/// do not have the platform information available.
///
/// # Errors
///
/// - `ApobError::TimingsNotFound` — no Group 7 Type 3 entry.
/// - `ApobError::TimingsEncrypted` — all Group 7 Type 3 entries are
///   encrypted.
/// - `ApobError::InvalidTimingData` — no valid timing blocks could be
///   located in the payload.
pub fn extract_siod_ddr4_timings(apob: &ApobData) -> Result<ApobTimings, ApobError> {
    // Find all Group 7 Type 3 entries.
    let timing_entries: Vec<&ApobEntry> = apob
        .entries
        .iter()
        .filter(|e| e.group == APOB_GROUP_GEN && e.entry_type == APOB_TYPE_CONFIGURATION_INFO)
        .collect();

    if timing_entries.is_empty() {
        return Err(ApobError::TimingsNotFound);
    }

    // Check if all are encrypted.
    if timing_entries.iter().all(|e| e.encrypted) {
        return Err(ApobError::TimingsEncrypted);
    }

    // Try both strides: Zen 2 (0x70) and Zen 3 (0x108). Use whichever
    // finds more valid blocks.
    let blocks_zen2: Vec<ApobChannelTimings> = timing_entries
        .iter()
        .filter(|e| !e.encrypted)
        .flat_map(|e| scan_ddr4_blocks(&e.data, DDR4_ZEN2_STRIDE))
        .collect();
    let blocks_zen3: Vec<ApobChannelTimings> = timing_entries
        .iter()
        .filter(|e| !e.encrypted)
        .flat_map(|e| scan_ddr4_blocks(&e.data, DDR4_ZEN3_STRIDE))
        .collect();

    // Use the stride that found more blocks (or Zen 2 if tied).
    let channels = if blocks_zen3.len() > blocks_zen2.len() {
        blocks_zen3
    } else {
        blocks_zen2
    };

    if channels.is_empty() {
        return Err(ApobError::InvalidTimingData(
            "no valid timing blocks found in the Group 7 Type 3 payload".into(),
        ));
    }

    Ok(ApobTimings { channels })
}

// ============================================================================
// AGESA version extraction (Group 4 Type 1 — board configuration)
// ============================================================================

/// Extract the AGESA firmware version string from an [`ApobData`].
///
/// Searches for the Group 4 (GNB) Type 1 entry (`APOB_SYS_BOARD_CONFIG`),
/// which contains the board identifier, PCB revision string, and AGESA
/// release build numbers. The entry's payload is scanned for version-shaped
/// tokens (e.g. `1.2.0.4`, `P3.20.0017`) using the same conservative
/// shape check as the DMI-based [`crate::platform::parse_agesa`].
///
/// # Behavior
///
/// - Returns `None` when the Group 4 Type 1 entry is absent, encrypted
///   (upper 16 bits of GroupId or DataTypeId are 0xFFFF), or carries no
///   valid AGESA token.
/// - The scan is over the raw payload bytes: both ASCII and UTF-16LE
///   string encodings are considered. A valid token is a whitespace-
///   delimited substring that matches the dotted-numeric AGESA shape.
/// - The match is deliberately conservative (reusing
///   [`crate::platform::parse_agesa`]): a token that does not fully
///   validate is skipped, and a payload with no valid token yields
///   `None` → honest N/A, never a fabricated value.
///
/// # Encrypted entries
///
/// On platforms where the board configuration entry is encrypted or
/// security-suppressed (upper 16 bits of GroupId or DataTypeId are
/// 0xFFFF), the function returns `None` without attempting to decode
/// the payload.
///
/// Pure (no I/O); never panics.
pub fn extract_agesa_from_apob(apob: &ApobData) -> Option<String> {
    // Find the Group 4 (GNB) Type 1 entry.
    let entry = apob.entries.iter().find(|e| {
        e.group == APOB_GROUP_GNB && e.entry_type == APOB_TYPE_BOARD_CONFIG && !e.encrypted
    });
    let entry = match entry {
        Some(e) => e,
        None => return None,
    };

    // Scan the payload for candidate strings and validate each against
    // the conservative AGESA shape check.
    for candidate in extract_strings(&entry.data) {
        if let Some(version) = crate::platform::parse_agesa(&candidate) {
            return Some(version);
        }
    }
    None
}

/// Read the AGESA version from the APOB (if available).
///
/// Reads the raw APOB blob from the `ryzen_smu` sysfs interface, parses it,
/// and extracts the AGESA version from the Group 4 Type 1 board
/// configuration entry. This is the high-level entry point used by the
/// facade to overlay the APOB-sourced AGESA version onto the platform
/// snapshot.
///
/// # Returns
///
/// - `Some(version)` — the APOB was available and carried a valid AGESA
///   token in the Group 4 Type 1 entry.
/// - `None` — the APOB is not available (module not loaded, APOB not
///   found), the Group 4 Type 1 entry is absent or encrypted, or the
///   payload carries no valid AGESA token.
///
/// All errors are contained: a failure here never propagates — the caller
/// falls back to the DMI-based AGESA source. Never panics.
pub fn read_agesa_from_apob() -> Option<String> {
    let blob = read_apob_from_sysfs().ok()?;
    let apob = parse_apob(&blob).ok()?;
    extract_agesa_from_apob(&apob)
}

/// Extract candidate strings from a raw byte payload.
///
/// Scans the payload for two encodings:
///
/// - **ASCII**: runs of printable ASCII bytes (0x20–0x7E) of at least
///   4 characters (shorter runs are unlikely to carry a version token
///   and are skipped to reduce noise).
/// - **UTF-16LE**: runs of (low byte 0x20–0x7E, high byte 0x00) pairs of
///   at least 4 characters (the ASCII range encoded as UTF-16LE — the
///   APOB is known to store strings in UTF-16LE on some platforms).
///
/// The results are returned as a `Vec<String>` of decoded candidates,
/// in the order they appear in the payload (ASCII candidates first, then
/// UTF-16LE). This is a pure scan — no interpretation is performed here;
/// the caller validates each candidate against the AGESA shape check.
///
/// Never panics: all reads are bounds-checked.
fn extract_strings(payload: &[u8]) -> Vec<String> {
    let mut out = Vec::new();

    // ASCII scan.
    let mut i = 0;
    while i < payload.len() {
        if (0x20..=0x7E).contains(&payload[i]) {
            let start = i;
            while i < payload.len() && (0x20..=0x7E).contains(&payload[i]) {
                i += 1;
            }
            let len = i - start;
            if len >= 4 {
                // Safe: the bytes in this run are all valid ASCII (0x20–0x7E).
                if let Ok(s) = std::str::from_utf8(&payload[start..i]) {
                    out.push(s.to_owned());
                }
            }
        } else {
            i += 1;
        }
    }

    // UTF-16LE scan (ASCII range: low byte 0x20–0x7E, high byte 0x00).
    let mut i = 0;
    while i + 1 < payload.len() {
        let low = payload[i];
        let high = payload[i + 1];
        if (0x20..=0x7E).contains(&low) && high == 0 {
            let start = i;
            while i + 1 < payload.len()
                && (0x20..=0x7E).contains(&payload[i])
                && payload[i + 1] == 0
            {
                i += 2;
            }
            let char_count = (i - start) / 2;
            if char_count >= 4 {
                // All code points are in the ASCII range, so the low bytes
                // form a valid UTF-8 string directly.
                let bytes: Vec<u8> = (start..i).step_by(2).map(|j| payload[j]).collect();
                out.push(String::from_utf8_lossy(&bytes).to_string());
            }
        } else {
            i += 1;
        }
    }

    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a synthetic APOB blob with the correct 48-byte chained
    /// entry format.
    ///
    /// The blob has:
    /// - A header with signature, version 2, table_size, header_size,
    ///   and an empty config offset list.
    /// - One or more entries with 48-byte headers, chained by Size.
    ///
    /// This is the correct format per the research doc, not the old
    /// 12-byte header format.
    fn build_apob_blob(entries: &[(u32, u32, u32, Vec<u8>)]) -> Vec<u8> {
        // entries: (raw_group, raw_type, instance, payload)
        let mut blob = Vec::new();

        // Header: signature + version + table_size (placeholder) +
        // header_size (placeholder) + reserved (32) + config offsets
        // (none) + HMAC (32).
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes()); // 0x00
        blob.extend_from_slice(&0x0018u32.to_le_bytes()); // 0x04 version (24, modern)
        blob.extend_from_slice(&0u32.to_le_bytes()); // 0x08 table_size (placeholder)
        let header_size = APOB_HEADER_FIXED_SIZE + APOB_HEADER_HMAC_SIZE; // 0x50 = 80
        blob.extend_from_slice(&(header_size as u32).to_le_bytes()); // 0x0C header_size
        blob.extend_from_slice(&[0u8; 32]); // 0x10 reserved
        blob.extend_from_slice(&[0u8; 32]); // 0x30 HMAC (no config offsets)

        // Write entries starting at header_size.
        let mut offset = header_size;
        for (raw_group, raw_type, instance, payload) in entries {
            let size = (APOB_ENTRY_HEADER_SIZE + payload.len()) as u32;
            blob.extend_from_slice(&raw_group.to_le_bytes()); // RawGroupId
            blob.extend_from_slice(&raw_type.to_le_bytes()); // RawDataTypeId
            blob.extend_from_slice(&instance.to_le_bytes()); // InstanceId
            blob.extend_from_slice(&size.to_le_bytes()); // Size
            blob.extend_from_slice(&[0u8; 32]); // HMAC
            blob.extend_from_slice(payload); // Payload
            offset += size as usize;
        }

        // Patch in the table_size.
        let table_size = offset as u32;
        blob[8..12].copy_from_slice(&table_size.to_le_bytes());

        blob
    }

    /// Construct a synthetic Group 7 Type 3 entry with DDR4 timing
    /// blocks in the scan-based format.
    ///
    /// Each block has:
    /// - MEMCLK (u16) at offset 0
    /// - Gap (2 bytes, the minimum)
    /// - 14 timing bytes: CL, CWL, RCD, RP, RTP, RAS(lo), RAS(hi),
    ///   RC(lo), RC(hi), WR, RRDS, WTRS, FAW, RRDL
    /// - 3 u16 tRFC values
    ///
    /// The stride between blocks is `stride`.
    type Ddr4BlockFields = (u16, u8, u8, u8, u8, u16, u16, u8, u8, u8, u8, u8, u16, u16);
    fn build_ddr4_timing_payload(blocks: &[Ddr4BlockFields], stride: usize) -> Vec<u8> {
        // blocks: (memclk, cl, cwl, rcw, rp, ras, rc, wr, rrds, wtrs, faw, rrld, trfc_s, trfc_f)
        let mut data = Vec::new();
        for (memclk, cl, cwl, rcw, rp, ras, rc, wr, rrds, wtrs, faw, rrld, trfc_s, trfc_f) in blocks
        {
            let block_len = stride;
            let mut block = vec![0u8; block_len];

            // MEMCLK at offset 0.
            block[0..2].copy_from_slice(&memclk.to_le_bytes());
            // Gap of 2 bytes (offsets 2-3 are zero).
            // Timing fields start at offset 4 (MEMCLK + gap of 2).
            let t = 4usize;
            block[t] = *cl; // CL
            block[t + 1] = *cwl; // CWL
            block[t + 2] = *rcw; // RCD
            block[t + 3] = *rp; // RP
            block[t + 4] = 14; // RTP (fixed plausible value)
            block[t + 5..t + 7].copy_from_slice(&ras.to_le_bytes()); // RAS (u16)
            block[t + 7..t + 9].copy_from_slice(&rc.to_le_bytes()); // RC (u16)
            block[t + 9] = *wr; // WR
            block[t + 10] = *rrds; // RRDS
            block[t + 11] = *wtrs; // WTRS
            block[t + 12] = *faw; // FAW
            block[t + 13] = *rrld; // RRDL
                                   // tRFC values after the 14 timing fields.
            block[t + 14..t + 16].copy_from_slice(&trfc_s.to_le_bytes());
            block[t + 16..t + 18].copy_from_slice(&trfc_f.to_le_bytes());
            block[t + 18..t + 20].copy_from_slice(&(*trfc_f / 2).to_le_bytes()); // trfc_pb

            data.extend_from_slice(&block);
        }
        data
    }

    /// A synthetic APOB with one Group 7 Type 3 entry containing two
    /// DDR4 timing blocks (two P-states for one channel) with known
    /// values.
    fn synthetic_apob_with_ddr4_timings() -> Vec<u8> {
        // Two blocks: P-state 0 and P-state 1 for channel 0.
        // Both use stride 0x70 (Zen 2).
        // Block 0: MEMCLK=1066 (DDR4-2133), CL=15, CWL=15, RCD=15, RP=15,
        //          RAS=36, RC=51, WR=15, RRDS=4, WTRS=4, FAW=30, RRDL=8,
        //          tRFC_S=298, tRFC_F=165
        // Block 1: MEMCLK=800 (DDR4-1600), CL=15, CWL=15, RCD=15, RP=15,
        //          RAS=36, RC=51, WR=15, RRDS=4, WTRS=4, FAW=30, RRLD=8,
        //          tRFC_S=298, tRFC_F=165
        let payload = build_ddr4_timing_payload(
            &[
                (1066, 15, 15, 15, 15, 36, 51, 15, 4, 4, 30, 8, 298, 165),
                (800, 15, 15, 15, 15, 36, 51, 15, 4, 4, 30, 8, 298, 165),
            ],
            DDR4_ZEN2_STRIDE,
        );

        // Group 7 (GEN), Type 3 (Configuration Info), instance 0.
        // raw_group = 7, raw_type = 3 (not encrypted).
        build_apob_blob(&[(7u32, 3u32, 0u32, payload)])
    }

    /// Header parsing: valid signature, version, and table size.
    #[test]
    fn test_parse_synthetic_apob() {
        let blob = synthetic_apob_with_ddr4_timings();
        let apob = parse_apob(&blob).expect("synthetic APOB must parse");
        assert_eq!(apob.header.version, 0x0018u32);
        assert_eq!(apob.entries.len(), 1);
        assert_eq!(apob.entries[0].group, 7);
        assert_eq!(apob.entries[0].entry_type, 3);
        assert_eq!(apob.entries[0].instance, 0);
        assert!(!apob.entries[0].encrypted);
    }

    /// Header parsing: bad signature.
    #[test]
    fn test_parse_bad_signature() {
        let mut blob = synthetic_apob_with_ddr4_timings();
        blob[0] = 0x00; // corrupt signature
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::BadSignature));
    }

    /// Header parsing: truncated blob (shorter than minimum header).
    #[test]
    fn test_parse_truncated_header() {
        let blob = vec![0u8; 40]; // too small (minimum is 80)
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// Header parsing: unsupported version (< 5).
    #[test]
    fn test_parse_unsupported_version() {
        let mut blob = synthetic_apob_with_ddr4_timings();
        blob[4..8].copy_from_slice(&0x0002u32.to_le_bytes()); // legacy v2
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::UnsupportedVersion(0x0002)));
    }

    /// Header parsing: table size larger than blob.
    #[test]
    fn test_parse_table_size_overrun() {
        let mut blob = synthetic_apob_with_ddr4_timings();
        let too_big = u32::try_from(blob.len() + 1000).unwrap_or(u32::MAX);
        blob[8..12].copy_from_slice(&too_big.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// Entry parsing: entry Size overruns table boundary.
    #[test]
    fn test_parse_entry_overrun() {
        // Build a blob with one entry whose Size extends past the table.
        let mut blob = Vec::new();
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob.extend_from_slice(&0x0018u32.to_le_bytes());
        let table_size = 200u32; // small table
        blob.extend_from_slice(&table_size.to_le_bytes());
        let header_size = 80u32;
        blob.extend_from_slice(&header_size.to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // reserved
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        // Entry at offset 80: Size = 500 (overruns table_size 200).
        blob.extend_from_slice(&7u32.to_le_bytes()); // raw_group
        blob.extend_from_slice(&3u32.to_le_bytes()); // raw_type
        blob.extend_from_slice(&0u32.to_le_bytes()); // instance
        blob.extend_from_slice(&500u32.to_le_bytes()); // size (too big)
        blob.extend_from_slice(&[0u8; 32]); // HMAC
                                            // Pad to table_size.
        while blob.len() < table_size as usize {
            blob.push(0);
        }

        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::EntryOverrun { .. }));
    }

    /// Entry parsing: encrypted entry (upper 16 bits of RawGroupId =
    /// 0xFFFF) is included but marked encrypted.
    #[test]
    fn test_parse_encrypted_entry() {
        // Group 7, Type 3, but encrypted (upper 16 bits = 0xFFFF).
        let blob = build_apob_blob(&[(0xFFFF0007u32, 3u32, 0u32, vec![0u8; 64])]);
        let apob = parse_apob(&blob).expect("encrypted entry should still parse");
        assert_eq!(apob.entries.len(), 1);
        assert_eq!(apob.entries[0].group, 7);
        assert!(apob.entries[0].encrypted);
    }

    /// Entry parsing: encrypted entry (upper 16 bits of RawDataTypeId =
    /// 0xFFFF) is included but marked encrypted.
    #[test]
    fn test_parse_encrypted_type() {
        // Group 7, Type 3, but type is encrypted.
        let blob = build_apob_blob(&[(7u32, 0xFFFF0003u32, 0u32, vec![0u8; 64])]);
        let apob = parse_apob(&blob).expect("encrypted entry should still parse");
        assert_eq!(apob.entries.len(), 1);
        assert_eq!(apob.entries[0].group, 7);
        assert_eq!(apob.entries[0].entry_type, 3);
        assert!(apob.entries[0].encrypted);
    }

    /// Timing extraction: no Group 7 Type 3 entry.
    #[test]
    fn test_extract_timings_not_found() {
        // An APOB with a Group 1 Type 3 entry (old format) — should not
        // be found by the new Group 7 Type 3 search.
        let blob = build_apob_blob(&[(1u32, 3u32, 0u32, vec![0u8; 108])]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let err = extract_siod_ddr4_timings(&apob).unwrap_err();
        assert!(matches!(err, ApobError::TimingsNotFound));
    }

    /// Timing extraction: encrypted Group 7 Type 3 entry yields
    /// TimingsEncrypted.
    #[test]
    fn test_extract_timings_encrypted() {
        // Group 7, Type 3, encrypted.
        let blob = build_apob_blob(&[(0xFFFF0007u32, 3u32, 0u32, vec![0u8; 200])]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let err = extract_siod_ddr4_timings(&apob).unwrap_err();
        assert!(matches!(err, ApobError::TimingsEncrypted));
    }

    /// Timing extraction: valid DDR4 blocks are found with the correct
    /// values.
    #[test]
    fn test_extract_ddr4_timings_known_values() {
        let blob = synthetic_apob_with_ddr4_timings();
        let apob = parse_apob(&blob).expect("APOB must parse");
        let timings = extract_siod_ddr4_timings(&apob).expect("timings must extract");

        // We expect 2 blocks (2 P-states for 1 channel).
        assert_eq!(timings.channels.len(), 2);

        // Block 0: P-state 0.
        let ch0 = &timings.channels[0];
        assert_eq!(ch0.memclk_mhz, 1066);
        assert_eq!(ch0.tcl, 15);
        assert_eq!(ch0.tcwl, 15);
        assert_eq!(ch0.trcdw, 15);
        assert_eq!(ch0.trcdr, 15);
        assert_eq!(ch0.trp, 15);
        assert_eq!(ch0.tras, 36);
        assert_eq!(ch0.trc, 51);
        assert_eq!(ch0.trrd_s, 4);
        assert_eq!(ch0.trrd_l, 8);
        assert_eq!(ch0.tfaw, 30);
        assert_eq!(ch0.twtr_s, 4);
        assert_eq!(ch0.twr, 15);
        assert_eq!(ch0.trfc_s, 298);
        assert_eq!(ch0.trfc_f, 165);

        // Block 1: P-state 1.
        let ch1 = &timings.channels[1];
        assert_eq!(ch1.memclk_mhz, 800);
        assert_eq!(ch1.tcl, 15);
    }

    /// Timing extraction: `extract_ddr4_timings` with the correct
    /// platform returns the blocks.
    #[test]
    fn test_extract_ddr4_timings_with_platform() {
        let blob = synthetic_apob_with_ddr4_timings();
        let apob = parse_apob(&blob).expect("APOB must parse");
        let blocks =
            extract_ddr4_timings(&apob.entries, crate::cpuid::AmdZen::Zen2).expect("blocks found");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].memclk_mhz, 1066);
        assert_eq!(blocks[0].tcl, 15);
    }

    /// Timing extraction: Zen 4 returns None (DDR5, not supported).
    #[test]
    fn test_extract_ddr4_timings_zen4_returns_none() {
        let blob = synthetic_apob_with_ddr4_timings();
        let apob = parse_apob(&blob).expect("APOB must parse");
        let blocks = extract_ddr4_timings(&apob.entries, crate::cpuid::AmdZen::Zen4);
        assert!(blocks.is_none());
    }

    /// Timing extraction: multiple Group 7 Type 3 entries (multiple
    /// instances) are all scanned.
    #[test]
    fn test_extract_ddr4_timings_multiple_entries() {
        // Two Group 7 Type 3 entries, each with one timing block.
        let payload1 = build_ddr4_timing_payload(
            &[(1066, 15, 15, 15, 15, 36, 51, 15, 4, 4, 30, 8, 298, 165)],
            DDR4_ZEN2_STRIDE,
        );
        let payload2 = build_ddr4_timing_payload(
            &[(1200, 16, 16, 16, 16, 36, 52, 16, 4, 4, 30, 8, 312, 176)],
            DDR4_ZEN2_STRIDE,
        );
        let blob = build_apob_blob(&[(7u32, 3u32, 0u32, payload1), (7u32, 3u32, 1u32, payload2)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        assert_eq!(apob.entries.len(), 2);
        let timings = extract_siod_ddr4_timings(&apob).expect("timings must extract");
        assert_eq!(timings.channels.len(), 2);
        assert_eq!(timings.channels[0].memclk_mhz, 1066);
        assert_eq!(timings.channels[1].memclk_mhz, 1200);
    }

    /// Config offset list: entries in a config chain are also found.
    #[test]
    fn test_parse_config_offset_chain() {
        // Build a blob with:
        // - Main chain: one Group 7 Type 3 entry at header_size, followed
        //   by a zero-filled gap (Size=0 terminates the main chain).
        // - Config chain: one Group 4 Type 1 entry at a non-contiguous
        //   offset, reachable only via the config offset list.
        let main_payload = build_ddr4_timing_payload(
            &[(1066, 15, 15, 15, 15, 36, 51, 15, 4, 4, 30, 8, 298, 165)],
            DDR4_ZEN2_STRIDE,
        );
        let config_payload = b"BOARD\0AGESA 1.2.0.4\0".to_vec();

        // We need to manually construct this blob because the config
        // offset list is in the header.
        let mut blob = Vec::new();

        // Header: signature + version + table_size (placeholder) +
        // header_size + reserved (32) + config offsets (1 u32) + HMAC (32).
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob.extend_from_slice(&0x0018u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // table_size placeholder
        let header_size = APOB_HEADER_FIXED_SIZE + 4 + APOB_HEADER_HMAC_SIZE; // 84
        blob.extend_from_slice(&(header_size as u32).to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // reserved
        blob.extend_from_slice(&0u32.to_le_bytes()); // config offset (placeholder)
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        // Main chain entry at header_size: Group 7 Type 3.
        let main_size = (APOB_ENTRY_HEADER_SIZE + main_payload.len()) as u32;
        let main_offset = header_size;
        blob.extend_from_slice(&7u32.to_le_bytes());
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // instance
        blob.extend_from_slice(&main_size.to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // HMAC
        blob.extend_from_slice(&main_payload);

        // Gap: 48 bytes of zeros. The main chain walker will read this
        // as the next entry, find Size=0, and terminate.
        let gap_start = main_offset + main_size as usize;
        blob.extend_from_slice(&[0u8; APOB_ENTRY_HEADER_SIZE]);

        // Config chain entry: Group 4 Type 1, placed after the gap.
        let config_offset = gap_start + APOB_ENTRY_HEADER_SIZE;
        let config_size = (APOB_ENTRY_HEADER_SIZE + config_payload.len()) as u32;
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // instance
        blob.extend_from_slice(&config_size.to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // HMAC
        blob.extend_from_slice(&config_payload);

        // Patch in the table_size and config offset.
        let table_size = blob.len();
        blob[8..12].copy_from_slice(&(table_size as u32).to_le_bytes());
        blob[APOB_HEADER_FIXED_SIZE..APOB_HEADER_FIXED_SIZE + 4]
            .copy_from_slice(&(config_offset as u32).to_le_bytes());

        let apob = parse_apob(&blob).expect("APOB with config chain must parse");
        assert_eq!(apob.entries.len(), 2);
        // The main chain entry (Group 7) comes first, then the config
        // chain entry (Group 4).
        assert_eq!(apob.entries[0].group, 7);
        assert_eq!(apob.entries[1].group, 4);

        // The AGESA version is extractable from the config chain entry.
        let agesa = extract_agesa_from_apob(&apob);
        assert_eq!(agesa, Some("1.2.0.4".to_owned()));
    }

    /// AGESA extraction: an ASCII AGESA version in the Group 4 Type 1
    /// payload is extracted correctly.
    #[test]
    fn test_agesa_extraction_ascii() {
        let payload =
            b"ASUS PROART X570-CREATOR\0Rev X.0\x20\x31\0AGESA 1.2.0.4\0more data\0".to_vec();
        let blob = build_apob_blob(&[(4u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let agesa = extract_agesa_from_apob(&apob);
        assert_eq!(agesa, Some("1.2.0.4".to_owned()));
    }

    /// AGESA extraction: a bare dotted-numeric AGESA version.
    #[test]
    fn test_agesa_extraction_bare_version() {
        let payload = b"BOARD ID STRING\0PCB REV 2.1\x005.06.01.00\0".to_vec();
        let blob = build_apob_blob(&[(4u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let agesa = extract_agesa_from_apob(&apob);
        assert_eq!(agesa, Some("5.06.01.00".to_owned()));
    }

    /// AGESA extraction: a project-code-prefixed AGESA version.
    #[test]
    fn test_agesa_extraction_project_code() {
        let payload = b"BOARD\0P3.20.0017\0".to_vec();
        let blob = build_apob_blob(&[(4u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let agesa = extract_agesa_from_apob(&apob);
        assert_eq!(agesa, Some("P3.20.0017".to_owned()));
    }

    /// AGESA extraction: a UTF-16LE encoded AGESA version.
    #[test]
    fn test_agesa_extraction_utf16le() {
        let version_utf16: Vec<u8> = "AGESA 1.2.0.4"
            .chars()
            .flat_map(|c| (c as u16).to_le_bytes())
            .collect();
        let mut payload = Vec::new();
        payload.extend_from_slice(b"BOARD\0\0\0");
        payload.extend_from_slice(&version_utf16);
        let blob = build_apob_blob(&[(4u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let agesa = extract_agesa_from_apob(&apob);
        assert_eq!(agesa, Some("1.2.0.4".to_owned()));
    }

    /// AGESA extraction: an encrypted Group 4 Type 1 entry yields None.
    #[test]
    fn test_agesa_extraction_encrypted() {
        let payload = b"AGESA 1.2.0.4\0".to_vec();
        // Encrypted: upper 16 bits of raw_group = 0xFFFF.
        let blob = build_apob_blob(&[(0xFFFF0004u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        assert!(apob.entries[0].encrypted);
        assert_eq!(extract_agesa_from_apob(&apob), None);
    }

    /// AGESA extraction: no Group 4 Type 1 entry yields None.
    #[test]
    fn test_agesa_extraction_no_board_config() {
        let blob = synthetic_apob_with_ddr4_timings();
        let apob = parse_apob(&blob).expect("APOB must parse");
        assert_eq!(extract_agesa_from_apob(&apob), None);
    }

    /// AGESA extraction: no valid version in payload yields None.
    #[test]
    fn test_agesa_extraction_no_version() {
        let payload = b"ASUS PROART X570-CREATOR\0Rev X.0\0BIOS F60\0".to_vec();
        let blob = build_apob_blob(&[(4u32, 1u32, 0u32, payload)]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        assert_eq!(extract_agesa_from_apob(&apob), None);
    }

    /// `extract_strings` finds ASCII runs of 4+ printable bytes.
    #[test]
    fn test_extract_strings_ascii() {
        let payload = b"AB\x00CDEFGH\x00IJ\x00KLMNOP";
        let strings = extract_strings(payload);
        assert!(strings.contains(&"CDEFGH".to_owned()));
        assert!(strings.contains(&"KLMNOP".to_owned()));
        assert!(!strings.contains(&"AB".to_owned()));
        assert!(!strings.contains(&"IJ".to_owned()));
    }

    /// `extract_strings` finds UTF-16LE runs.
    #[test]
    fn test_extract_strings_utf16le() {
        let utf16: Vec<u8> = "WORLD"
            .chars()
            .flat_map(|c| (c as u16).to_le_bytes())
            .collect();
        let mut payload = vec![0xFF, 0xFF];
        payload.extend_from_slice(&utf16);
        payload.extend_from_slice(&[0x00, 0x00]);
        let strings = extract_strings(&payload);
        assert!(strings.contains(&"WORLD".to_owned()));
    }

    /// `extract_strings` on an empty payload returns no candidates.
    #[test]
    fn test_extract_strings_empty() {
        assert!(extract_strings(&[]).is_empty());
    }

    /// The full pipeline: a synthetic APOB with both a Group 7 Type 3
    /// (timings) entry and a Group 4 Type 1 (board config) entry —
    /// the AGESA version is extracted and the timings are still
    /// decodable.
    #[test]
    fn test_agesa_and_timings_coexist() {
        let timing_payload = build_ddr4_timing_payload(
            &[(1066, 15, 15, 15, 15, 36, 51, 15, 4, 4, 30, 8, 298, 165)],
            DDR4_ZEN2_STRIDE,
        );
        let agesa_payload = b"BOARD\0AGESA 3.1.0.0\0".to_vec();
        let blob = build_apob_blob(&[
            (7u32, 3u32, 0u32, timing_payload),
            (4u32, 1u32, 0u32, agesa_payload),
        ]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        assert_eq!(apob.entries.len(), 2);

        // The AGESA version is extracted.
        assert_eq!(extract_agesa_from_apob(&apob), Some("3.1.0.0".to_owned()));

        // The timings are still decodable.
        let timings = extract_siod_ddr4_timings(&apob).expect("timings must extract");
        assert_eq!(timings.channels.len(), 1);
        assert_eq!(timings.channels[0].tcl, 15);
    }

    /// The ApobError Display implementations produce non-empty text.
    #[test]
    fn test_error_display_non_empty() {
        let errors = vec![
            ApobError::NotFound,
            ApobError::BadSignature,
            ApobError::Truncated {
                expected: 100,
                actual: 50,
            },
            ApobError::UnsupportedVersion(0x99),
            ApobError::EntryOverrun {
                entry_index: 0,
                group: 7,
                type_id: 3,
            },
            ApobError::TimingsNotFound,
            ApobError::TimingsEncrypted,
            ApobError::InvalidTimingData("test".to_string()),
            ApobError::Io("test".to_string()),
        ];
        for e in errors {
            let text = e.to_string();
            assert!(!text.is_empty(), "Display must be non-empty: {:?}", e);
        }
    }

    // ====================================================================
    // QA edge-case battery: boundaries and failure modes.
    // ====================================================================

    /// (QA) An empty blob is rejected as truncated.
    #[test]
    fn test_apob_empty_blob() {
        let blob = vec![];
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) A blob shorter than the minimum header is rejected.
    #[test]
    fn test_apob_short_blob() {
        let blob = vec![0u8; 50]; // minimum is 80
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) A blob with a valid length but a wrong signature is rejected.
    #[test]
    fn test_apob_bad_signature() {
        let mut blob = vec![0u8; 128];
        blob[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::BadSignature));
    }

    /// (QA) A zero table_size is rejected.
    #[test]
    fn test_apob_zero_table_size() {
        let mut blob = vec![0u8; 128];
        blob[0..4].copy_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob[4..8].copy_from_slice(&0x0018u32.to_le_bytes());
        blob[8..12].copy_from_slice(&0u32.to_le_bytes()); // table_size = 0
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) A huge table_size is rejected.
    #[test]
    fn test_apob_huge_table_size() {
        let mut blob = vec![0u8; 128];
        blob[0..4].copy_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob[4..8].copy_from_slice(&0x0018u32.to_le_bytes());
        blob[8..12].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) A header_size smaller than the minimum is rejected.
    #[test]
    fn test_apob_header_size_too_small() {
        let mut blob = vec![0u8; 200];
        blob[0..4].copy_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob[4..8].copy_from_slice(&0x0018u32.to_le_bytes());
        blob[8..12].copy_from_slice(&200u32.to_le_bytes()); // table_size
        blob[12..16].copy_from_slice(&40u32.to_le_bytes()); // header_size = 40 < 80
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) A header_size larger than table_size is rejected.
    #[test]
    fn test_apob_header_size_over_table() {
        let mut blob = vec![0u8; 200];
        blob[0..4].copy_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob[4..8].copy_from_slice(&0x0018u32.to_le_bytes());
        blob[8..12].copy_from_slice(&200u32.to_le_bytes()); // table_size
        blob[12..16].copy_from_slice(&250u32.to_le_bytes()); // header_size > table_size
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// (QA) An entry with Size = 0 terminates the chain cleanly.
    #[test]
    fn test_apob_entry_size_zero() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob.extend_from_slice(&0x0018u32.to_le_bytes());
        let header_size = 80u32;
        blob.extend_from_slice(&0u32.to_le_bytes()); // table_size placeholder
        blob.extend_from_slice(&header_size.to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // reserved
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        // Entry with Size = 0 (invalid, terminates chain).
        blob.extend_from_slice(&7u32.to_le_bytes());
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // size = 0
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        let table_size = blob.len() as u32;
        blob[8..12].copy_from_slice(&table_size.to_le_bytes());

        let apob = parse_apob(&blob).expect("APOB must parse");
        // The entry with Size=0 is not added (the chain terminates).
        assert_eq!(apob.entries.len(), 0);
    }

    /// (QA) An entry with Size < 48 terminates the chain cleanly.
    #[test]
    fn test_apob_entry_size_too_small() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob.extend_from_slice(&0x0018u32.to_le_bytes());
        let header_size = 80u32;
        blob.extend_from_slice(&0u32.to_le_bytes()); // table_size placeholder
        blob.extend_from_slice(&header_size.to_le_bytes());
        blob.extend_from_slice(&[0u8; 32]); // reserved
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        // Entry with Size = 20 (< 48, invalid).
        blob.extend_from_slice(&7u32.to_le_bytes());
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&20u32.to_le_bytes()); // size = 20
        blob.extend_from_slice(&[0u8; 32]); // HMAC

        let table_size = blob.len() as u32;
        blob[8..12].copy_from_slice(&table_size.to_le_bytes());

        let apob = parse_apob(&blob).expect("APOB must parse");
        assert_eq!(apob.entries.len(), 0);
    }

    /// (QA) The scan-based locator rejects a payload with no valid
    /// blocks (all zeros).
    #[test]
    fn test_scan_no_valid_blocks() {
        let data = vec![0u8; 512];
        let blocks = scan_ddr4_blocks(&data, DDR4_ZEN2_STRIDE);
        assert!(blocks.is_empty());
    }

    /// (QA) The scan-based locator rejects a payload with an out-of-
    /// range MEMCLK.
    #[test]
    fn test_scan_invalid_memclk() {
        // MEMCLK = 50 (too low) at offset 0.
        let mut data = vec![0u8; 512];
        data[0..2].copy_from_slice(&50u16.to_le_bytes());
        // Even if the timing bytes are valid, the MEMCLK is out of range.
        let t = 4usize;
        data[t] = 15;
        data[t + 1] = 15;
        data[t + 2] = 15;
        data[t + 3] = 15;
        data[t + 4] = 14;
        data[t + 5..t + 7].copy_from_slice(&36u16.to_le_bytes());
        data[t + 7..t + 9].copy_from_slice(&51u16.to_le_bytes());
        let blocks = scan_ddr4_blocks(&data, DDR4_ZEN2_STRIDE);
        assert!(blocks.is_empty());
    }

    /// (QA) The scan-based locator handles a payload that is too short
    /// for a complete block.
    #[test]
    fn test_scan_short_payload() {
        // Only 10 bytes — not enough for MEMCLK + gap + 14 timing bytes.
        let data = vec![0u8; 10];
        let blocks = scan_ddr4_blocks(&data, DDR4_ZEN2_STRIDE);
        assert!(blocks.is_empty());
    }

    /// (QA) The scan-based locator does not panic on all-0xFF data.
    #[test]
    fn test_scan_all_ff_no_panic() {
        let data = vec![0xFFu8; 1024];
        let blocks = scan_ddr4_blocks(&data, DDR4_ZEN2_STRIDE);
        // 0xFFFF is not in the valid MEMCLK range (400–2400), so no
        // blocks should be found.
        assert!(blocks.is_empty());
    }

    /// (QA) A timing payload with RC < RAS + RP is rejected (the RC
    /// validation check).
    #[test]
    fn test_scan_rc_less_than_ras_plus_rp() {
        // MEMCLK = 1066, CL = 15, but RC = 30 < RAS (36) + RP (15) = 51.
        let mut data = vec![0u8; 256];
        data[0..2].copy_from_slice(&1066u16.to_le_bytes());
        let t = 4usize; // gap = 2
        data[t] = 15; // CL
        data[t + 1] = 15; // CWL
        data[t + 2] = 15; // RCD
        data[t + 3] = 15; // RP
        data[t + 4] = 14; // RTP
        data[t + 5..t + 7].copy_from_slice(&36u16.to_le_bytes()); // RAS
        data[t + 7..t + 9].copy_from_slice(&30u16.to_le_bytes()); // RC < RAS + RP
        let blocks = scan_ddr4_blocks(&data, DDR4_ZEN2_STRIDE);
        assert!(blocks.is_empty());
    }

    /// (QA) The `extract_ddr4_timings` function returns None when there
    /// are no Group 7 Type 3 entries.
    #[test]
    fn test_extract_ddr4_no_entries() {
        let blob = build_apob_blob(&[(1u32, 3u32, 0u32, vec![0u8; 108])]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let blocks = extract_ddr4_timings(&apob.entries, crate::cpuid::AmdZen::Zen2);
        assert!(blocks.is_none());
    }

    /// (QA) The `extract_ddr4_timings` function returns None when all
    /// Group 7 Type 3 entries are encrypted.
    #[test]
    fn test_extract_ddr4_all_encrypted() {
        let blob = build_apob_blob(&[(0xFFFF0007u32, 3u32, 0u32, vec![0u8; 200])]);
        let apob = parse_apob(&blob).expect("APOB must parse");
        let blocks = extract_ddr4_timings(&apob.entries, crate::cpuid::AmdZen::Zen2);
        assert!(blocks.is_none());
    }
}
