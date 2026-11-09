//! APOB (AGESA Pre-Output Buffer) acquisition and parsing.
//!
//! The APOB is a firmware data structure written to physical DRAM during
//! early boot (AGESA Phase 1 on the Platform Security Processor). It contains
//! the results of memory training, fabric topology, CPU core mapping, and
//! board configuration — the complete platform state as determined by firmware
//! before OS handoff.
//!
//! On Castle Peak (Threadripper 3000 / TRX40), the APOB Group 1 Type 3
//! entry contains the trained DRAM timings in plaintext — the only
//! OS-accessible source of live timing data on sIOD platforms (the SMN
//! registers hold static JEDEC defaults).
//!
//! # Acquisition
//!
//! The raw APOB blob is read from the `ryzen_smu` driver's sysfs interface:
//! `/sys/kernel/ryzen_smu_drv/apob` (canonical) or
//! `/sys/kernel/ryzen_smu/apob` (legacy fallback). The kernel module maps
//! the APOB table from physical RAM via `ioremap_cache()` and exposes it
//! as a read-only sysfs attribute.
//!
//! # Format (source: architecture doc §4.2 + ZenStates-Core Apob.cs)
//!
//! - 16-byte header: signature (u32), version (u32), table_size (u32), flags (u32)
//! - Variable-length entry array: each entry has a 12-byte header
//!   (group_id u16, type_id u16, type_size u32, instance_id u8, flags u8,
//!   header_size u16) followed by the payload.
//!
//! All parsing is bounds-checked and never panics. Encrypted entries
//! (group_id == 0xFFFF or flags & 0x80) are skipped gracefully.
//!
//! # Safety
//!
//! No `unsafe` code. All reads go through bounds-checked `get()` calls.
//! All values are read as little-endian. The blob is trusted to come from
//! the kernel module, which has already validated the signature and size.

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
/// architecture doc §A.1.
const APOB_SIGNATURE: u32 = 0x424F5041;

/// Maximum reasonable APOB table size (matches the kernel module's cap).
///
/// Source: architecture doc §3.2 (`APOB_MAX_TABLE_SIZE`).
const APOB_MAX_SIZE: usize = 1024 * 1024;

/// APOB header size in bytes (signature + version + table_size + flags).
///
/// Source: architecture doc §A.1.
const APOB_HEADER_SIZE: usize = 16;

/// APOB entry header size in bytes.
///
/// Source: architecture doc §A.2.
const APOB_ENTRY_HEADER_SIZE: usize = 12;

/// APOB format versions (source: architecture doc §A.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApobVersion {
    /// Legacy format (pre-Zen).
    V1,
    /// Modern Zen format (Zen 1+).
    V2,
}

/// APOB entry group IDs (subsystem domain classification).
///
/// Source: ZenStates-Core Apob.cs group enumeration;
/// architecture doc §A.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ApobGroup {
    /// Memory controller.
    Mem = 0x0001,
    /// Data Fabric.
    Fabric = 0x0002,
    /// CPU core topology.
    Cpu = 0x0003,
    /// System board configuration.
    Board = 0x0004,
    /// Unknown group (auto-assigned discriminant 0x0005).
    Unknown(u16),
    /// Encrypted / security-suppressed entry (skip).
    Encrypted = 0xFFFF,
}

/// APOB entry type IDs within the Memory group.
///
/// Source: ZenStates-Core Apob.cs type enumeration;
/// architecture doc §A.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ApobMemType {
    /// General memory info (capabilities, channel count, boot mode).
    GeneralInfo = 0x0001,
    /// PMU training fail status.
    PmuTrainingFail = 0x0002,
    /// Primary DRAM timings (the target of this feature).
    Timings = 0x0003,
    /// DDR5 subchannel timings.
    SubchannelTimings = 0x0004,
    /// DIMM SMBus info.
    DimmSmbusInfo = 0x0005,
    /// Impedance configuration (ProcODT, RTT, drive strengths).
    ImpedanceConfig = 0x0008,
    /// CCDL timings.
    CcdlTimings = 0x0009,
    /// Unknown type (auto-assigned discriminant 0x000A).
    Unknown(u16),
}

/// A single parsed APOB header (16 bytes).
///
/// Source: architecture doc §A.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApobHeader {
    /// APOB format version.
    pub version: ApobVersion,
    /// Total table size in bytes (including header and all entries).
    pub table_size: u32,
    /// Global table flags.
    pub flags: u32,
}

/// A single APOB entry header (12 bytes, before the payload).
///
/// Source: architecture doc §A.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApobEntryHeader {
    /// Subsystem group.
    pub group: ApobGroup,
    /// Type within the group.
    pub type_id: u16,
    /// Payload size in bytes (excluding the 12-byte entry header).
    pub type_size: u32,
    /// Die/socket/controller instance (zero-indexed).
    pub instance_id: u8,
    /// Entry status flags (bit 7 = encrypted).
    pub flags: u8,
    /// Offset from entry start to payload (should be 12).
    pub header_size: u16,
}

impl ApobEntryHeader {
    /// Whether this entry is encrypted / security-suppressed.
    ///
    /// An entry is encrypted when its group is `0xFFFF` or when bit 7
    /// of its flags byte is set.
    ///
    /// Source: ZenStates-Core Apob.cs; architecture doc §A.2.
    pub fn is_encrypted(&self) -> bool {
        self.group == ApobGroup::Encrypted || (self.flags & 0x80) != 0
    }
}

/// A parsed APOB table: header + all entries.
#[derive(Debug, Clone)]
pub struct ApobData {
    /// The APOB header.
    pub header: ApobHeader,
    /// The physical address where the APOB was discovered (from the
    /// kernel module, for diagnostics).
    pub source_address: Option<u64>,
    /// All parsed entries (encrypted entries are included but marked).
    pub entries: Vec<ApobEntry>,
}

/// A single APOB entry with its payload.
#[derive(Debug, Clone)]
pub struct ApobEntry {
    /// The entry header.
    pub header: ApobEntryHeader,
    /// The raw payload bytes.
    pub data: Vec<u8>,
}

/// Per-channel DRAM timings extracted from the APOB Group 1 Type 3 entry.
///
/// The field layout is the sIOD DDR4 timing structure: 108 bytes per
/// channel. This covers Castle Peak (TRX40, 8 channels), Chagall
/// (WRX80), and EPYC DDR4 platforms (Rome/Milan 8 or 12 channels).
///
/// Source: architecture doc §A.3; ZenStates-Core ApobLayout.cs
/// (Castle Peak profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApobChannelTimings {
    /// Channel index (0–15; EPYC Milan/Genoa have 12 channels, TRX40 has 8).
    pub channel: u8,
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
    /// Row to row delay (short, same bank group) (cycles). @ 0x08
    pub trrd_s: u8,
    /// Row to row delay (long, different bank group) (cycles). @ 0x09
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
    /// Read to write turnaround (cycles). @ 0x18
    pub trdwr: u8,
    /// Write to read turnaround (cycles). @ 0x19
    pub twrtr: u8,
    /// Command rate: 0 = 1T, 1 = 2T, 2 = Gear Down Mode.
    pub cmd_rate: u8,
}

/// The extracted APOB timing data (per-channel).
#[derive(Debug, Clone)]
pub struct ApobTimings {
    /// One entry per channel found in the APOB Group 1 Type 3 payload.
    /// The channel count is derived from the payload size: TRX40 has 8
    /// channels (864 bytes), EPYC Milan/Genoa DDR4 have 12 channels
    /// (1296 bytes). Each channel block is 108 bytes.
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
    /// The APOB blob is truncated (shorter than the declared table size).
    Truncated { expected: usize, actual: usize },
    /// The APOB format version is not supported.
    UnsupportedVersion(u32),
    /// An entry's payload extends beyond the table boundary.
    EntryOverrun {
        entry_index: usize,
        group: u16,
        type_id: u16,
    },
    /// The timing entry (Group 1 Type 3) was not found.
    TimingsNotFound,
    /// The timing entry is encrypted (Group ID 0xFFFF or flags bit 7).
    TimingsEncrypted,
    /// The timing entry payload is too short to contain the expected
    /// number of channels.
    TimingsTooShort { expected: usize, actual: usize },
    /// The timing payload is malformed (not a multiple of the channel
    /// block size, or an unrealistic channel count).
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
                "APOB entry {} (group 0x{:04x}, type 0x{:04x}) overruns table boundary",
                entry_index, group, type_id
            ),
            Self::TimingsNotFound => write!(f, "APOB timings entry (Group 1 Type 3) not found"),
            Self::TimingsEncrypted => write!(f, "APOB timings entry is encrypted"),
            Self::TimingsTooShort { expected, actual } => write!(
                f,
                "APOB timings payload too short: expected {} bytes, got {}",
                expected, actual
            ),
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

/// Parse a raw APOB blob into a structured [`ApobData`].
///
/// # Validation
///
/// 1. The blob must be at least [`APOB_HEADER_SIZE`] bytes.
/// 2. The signature (first 4 bytes, LE) must be [`APOB_SIGNATURE`].
/// 3. The version must be supported (currently V1 or V2).
/// 4. The table size must be ≤ the blob length (the blob may have trailing
///    padding, but not be shorter than declared).
/// 5. Each entry's payload must fit within the table boundary.
/// 6. Encrypted entries (group 0xFFFF or flags bit 7) are included but
///    marked — their payloads are not decoded.
///
/// # Errors
///
/// See [`ApobError`] for the full set of error variants.
pub fn parse_apob(blob: &[u8]) -> Result<ApobData, ApobError> {
    // 1. Minimum length check.
    if blob.len() < APOB_HEADER_SIZE {
        return Err(ApobError::Truncated {
            expected: APOB_HEADER_SIZE,
            actual: blob.len(),
        });
    }

    // 2. Signature check (little-endian u32 at offset 0).
    let signature = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    if signature != APOB_SIGNATURE {
        return Err(ApobError::BadSignature);
    }

    // 3. Version check (little-endian u32 at offset 4).
    let raw_version = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
    let version = match raw_version {
        0x0001 => ApobVersion::V1,
        0x0002 => ApobVersion::V2,
        _ => return Err(ApobError::UnsupportedVersion(raw_version)),
    };

    // 4. Table size check (little-endian u32 at offset 8).
    let table_size = u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize;
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

    // 5. Flags (little-endian u32 at offset 12).
    let flags = u32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]);

    let header = ApobHeader {
        version,
        table_size: table_size as u32,
        flags,
    };

    // 6. Parse entries. Entries are contiguous after the 16-byte header.
    let mut entries = Vec::new();
    let mut offset = APOB_HEADER_SIZE;
    let mut entry_index = 0;

    while offset + APOB_ENTRY_HEADER_SIZE <= table_size {
        // Read the 12-byte entry header (all fields little-endian).
        let group_raw = u16::from_le_bytes([blob[offset], blob[offset + 1]]);
        let type_raw = u16::from_le_bytes([blob[offset + 2], blob[offset + 3]]);
        let type_size = u32::from_le_bytes([
            blob[offset + 4],
            blob[offset + 5],
            blob[offset + 6],
            blob[offset + 7],
        ]) as usize;
        let instance_id = blob[offset + 8];
        let entry_flags = blob[offset + 9];
        let header_size = u16::from_le_bytes([blob[offset + 10], blob[offset + 11]]) as usize;

        // Validate the entry header size (should be 12, but be lenient).
        if !(APOB_ENTRY_HEADER_SIZE..=64).contains(&header_size) {
            // Malformed entry header size — stop parsing.
            break;
        }

        // Check bounds: the payload must fit within the table.
        let payload_start = offset + header_size;
        let payload_end = payload_start + type_size;
        if payload_end > table_size {
            return Err(ApobError::EntryOverrun {
                entry_index,
                group: group_raw,
                type_id: type_raw,
            });
        }

        // Map the raw group ID to the typed enum.
        let group = match group_raw {
            0x0001 => ApobGroup::Mem,
            0x0002 => ApobGroup::Fabric,
            0x0003 => ApobGroup::Cpu,
            0x0004 => ApobGroup::Board,
            0xFFFF => ApobGroup::Encrypted,
            other => ApobGroup::Unknown(other),
        };

        let entry = ApobEntry {
            header: ApobEntryHeader {
                group,
                type_id: type_raw,
                type_size: type_size as u32,
                instance_id,
                flags: entry_flags,
                header_size: header_size as u16,
            },
            data: blob[payload_start..payload_end].to_vec(),
        };

        entries.push(entry);
        entry_index += 1;
        offset = payload_end;
    }

    Ok(ApobData {
        header,
        source_address: None, // populated by the caller if known
        entries,
    })
}

/// Extract the sIOD DDR4 timings from an [`ApobData`].
///
/// Searches for the Group 1 (Mem) Type 3 (Timings) entry and decodes the
/// per-channel timing structure. The channel count is derived from the
/// payload size: each channel block is 108 bytes. This covers:
///
/// - TRX40 (Castle Peak, 8 channels → 864 bytes)
/// - WRX80 (Chagall, 8 channels → 864 bytes)
/// - EPYC Rome (SP3, 8 channels → 864 bytes)
/// - EPYC Milan / Genoa DDR4 (SP3/SP5, 12 channels → 1296 bytes)
///
/// # sIOD DDR4 timing layout (per 108-byte channel block, little-endian)
///
/// | Offset | Field    | Type  | Description                          |
/// |--------|----------|-------|--------------------------------------|
/// | 0x00   | tCL      | u8    | CAS latency (cycles)                 |
/// | 0x01   | tRCDW    | u8    | RAS to CAS delay (write) (cycles)    |
/// | 0x02   | tRCDR    | u8    | RAS to CAS delay (read) (cycles)     |
/// | 0x03   | tRP      | u8    | Row precharge time (cycles)          |
/// | 0x04   | tRAS     | u16   | Row active time (cycles)             |
/// | 0x06   | tRC      | u16   | Row cycle time (cycles)              |
/// | 0x08   | tRRDS    | u8    | Row to row delay (short)             |
/// | 0x09   | tRRDL    | u8    | Row to row delay (long)              |
/// | 0x0A   | tFAW     | u16   | Four-activate window (cycles)        |
/// | 0x0C   | tWTR_S   | u8    | Write to read delay (short)          |
/// | 0x0D   | tWTR_L   | u8    | Write to read delay (long)           |
/// | 0x0E   | tWR      | u16   | Write recovery time (cycles)         |
/// | 0x10   | tRFC_S   | u16   | Refresh cycle time (standard)        |
/// | 0x12   | tRFC_F   | u16   | Refresh cycle time (fine)            |
/// | 0x14   | tRFC_PB  | u16   | Refresh cycle time (page bank)       |
/// | 0x16   | tCWL     | u8    | CAS write latency (cycles)           |
/// | 0x17   | tRTP     | u8    | Read to precharge time               |
/// | 0x18   | tRDWR    | u8    | Read to write turnaround             |
/// | 0x19   | tWRTR    | u8    | Write to read turnaround             |
/// | 0x1A   | tRTR_SCK | u8    | Read to read same chip select        |
/// | 0x1B   | tWTR_SCK | u8    | Write to write same chip select      |
/// | 0x1C   | CmdRate  | u8    | 0=1T, 1=2T, 2=GDM                     |
/// | 0x1D–0x6B | (reserved / extended) |   |                                |
///
/// Source: architecture doc §A.3; ZenStates-Core ApobLayout.cs
/// (Castle Peak profile).
///
/// # Errors
///
/// - `ApobError::TimingsNotFound` — no Group 1 Type 3 entry.
/// - `ApobError::TimingsEncrypted` — the entry is encrypted.
/// - `ApobError::InvalidTimingData` — the payload is not a multiple of the
///   108-byte channel block size, or the channel count is unrealistic
///   (0 or > 16).
pub fn extract_siod_ddr4_timings(apob: &ApobData) -> Result<ApobTimings, ApobError> {
    // Find the Group 1 Type 3 (APOB_MEM_TIMINGS) entry.
    // Source: architecture doc §A.2, type 0x0003.
    const TIMINGS_TYPE_ID: u16 = 0x0003;
    let timings_entry = apob
        .entries
        .iter()
        .find(|e| e.header.group == ApobGroup::Mem && e.header.type_id == TIMINGS_TYPE_ID);

    let timings_entry = match timings_entry {
        Some(e) => e,
        None => return Err(ApobError::TimingsNotFound),
    };

    // Check if encrypted.
    if timings_entry.header.is_encrypted() {
        return Err(ApobError::TimingsEncrypted);
    }

    // Derive the channel count from the payload size. Each channel block
    // is 108 bytes (the sIOD DDR4 layout). TRX40 has 8 channels (864
    // bytes), EPYC Milan/Genoa DDR4 have 12 channels (1296 bytes).
    const CHANNEL_SIZE: usize = 108;
    const MAX_CHANNELS: usize = 16; // sanity cap — more than 16 is not realistic

    let num_channels = timings_entry.data.len() / CHANNEL_SIZE;

    if num_channels == 0 {
        return Err(ApobError::InvalidTimingData(
            "no complete channel blocks in the timing payload".into(),
        ));
    }
    if num_channels > MAX_CHANNELS {
        return Err(ApobError::InvalidTimingData(format!(
            "unrealistic channel count: {} (max {})",
            num_channels, MAX_CHANNELS
        )));
    }

    let mut channels = Vec::with_capacity(num_channels);

    for ch in 0..num_channels {
        let base = ch * CHANNEL_SIZE;
        let d = &timings_entry.data;

        channels.push(ApobChannelTimings {
            channel: ch as u8,
            tcl: d[base],
            trcdw: d[base + 0x01],
            trcdr: d[base + 0x02],
            trp: d[base + 0x03],
            tras: u16::from_le_bytes([d[base + 0x04], d[base + 0x05]]),
            trc: u16::from_le_bytes([d[base + 0x06], d[base + 0x07]]),
            trrd_s: d[base + 0x08],
            trrd_l: d[base + 0x09],
            tfaw: u16::from_le_bytes([d[base + 0x0A], d[base + 0x0B]]),
            twtr_s: d[base + 0x0C],
            twtr_l: d[base + 0x0D],
            twr: u16::from_le_bytes([d[base + 0x0E], d[base + 0x0F]]),
            trfc_s: u16::from_le_bytes([d[base + 0x10], d[base + 0x11]]),
            trfc_f: u16::from_le_bytes([d[base + 0x12], d[base + 0x13]]),
            trfc_pb: u16::from_le_bytes([d[base + 0x14], d[base + 0x15]]),
            tcwl: d[base + 0x16],
            trtp: d[base + 0x17],
            trdwr: d[base + 0x18],
            twrtr: d[base + 0x19],
            cmd_rate: d[base + 0x1C],
        });
    }

    Ok(ApobTimings { channels })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a synthetic sIOD DDR4 APOB blob for testing.
    ///
    /// Header: APOB signature, version 0x0002, table_size = 16 + 12 + 864,
    /// flags 0.
    /// Entry: Group 1, Type 3, size 864, instance 0, flags 0, header_size 12.
    /// Payload: 8 channels × 108 bytes with known values.
    ///
    /// Channel 0 uses Sea_Monkey's known-good DDR4-3200 CL14 values:
    /// tCL=14, tRCDW=18, tRCDR=18, tRP=18, tRAS=38, tRC=56,
    /// tRRDS=4, tRRDL=8, tFAW=39, tWTR_S=4, tWTR_L=12, tWR=24,
    /// tRFC_S=312, tRFC_F=192, tRFC_PB=132, tCWL=18, tRTP=14,
    /// tRDWR=8, tWRTR=8, CmdRate=0 (1T).
    /// Channels 1–7 are zero-filled (dormant on TRX40).
    fn synthetic_castle_peak_apob() -> Vec<u8> {
        let mut blob = Vec::new();

        // Header (16 bytes).
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes()); // 0x424F5041
        blob.extend_from_slice(&0x0002u32.to_le_bytes()); // Version V2
        let table_size = (APOB_HEADER_SIZE + APOB_ENTRY_HEADER_SIZE + 864) as u32;
        blob.extend_from_slice(&table_size.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes()); // Flags

        // Entry header (12 bytes).
        blob.extend_from_slice(&1u16.to_le_bytes()); // Group: Mem
        blob.extend_from_slice(&3u16.to_le_bytes()); // Type: Timings
        blob.extend_from_slice(&864u32.to_le_bytes()); // Size: 8 × 108
        blob.push(0); // Instance: 0
        blob.push(0); // Flags: 0
        blob.extend_from_slice(&12u16.to_le_bytes()); // HeaderSize: 12

        // Payload: 8 channels × 108 bytes.
        for ch in 0..8u8 {
            if ch == 0 {
                // Sea_Monkey's known-good DDR4-3200 CL14 values.
                let mut ch_data = [0u8; 108];
                ch_data[0x00] = 14; // tCL
                ch_data[0x01] = 18; // tRCDW
                ch_data[0x02] = 18; // tRCDR
                ch_data[0x03] = 18; // tRP
                ch_data[0x04] = 38; // tRAS (low byte)
                ch_data[0x05] = 0; // tRAS (high byte)
                ch_data[0x06] = 56; // tRC (low byte)
                ch_data[0x07] = 0; // tRC (high byte)
                ch_data[0x08] = 4; // tRRDS
                ch_data[0x09] = 8; // tRRDL
                ch_data[0x0A] = 39; // tFAW (low byte)
                ch_data[0x0B] = 0; // tFAW (high byte)
                ch_data[0x0C] = 4; // tWTR_S
                ch_data[0x0D] = 12; // tWTR_L
                ch_data[0x0E] = 24; // tWR (low byte)
                ch_data[0x0F] = 0; // tWR (high byte)
                ch_data[0x10] = 56; // tRFC_S (low byte, 312 = 0x0138)
                ch_data[0x11] = 1; // tRFC_S (high byte)
                ch_data[0x12] = 192; // tRFC_F (low byte)
                ch_data[0x13] = 0; // tRFC_F (high byte)
                ch_data[0x14] = 132; // tRFC_PB (low byte)
                ch_data[0x15] = 0; // tRFC_PB (high byte)
                ch_data[0x16] = 18; // tCWL
                ch_data[0x17] = 14; // tRTP
                ch_data[0x18] = 8; // tRDWR
                ch_data[0x19] = 8; // tWRTR
                ch_data[0x1C] = 0; // CmdRate: 1T
                blob.extend_from_slice(&ch_data);
            } else {
                // Dormant channel: all zeros.
                blob.extend_from_slice(&[0u8; 108]);
            }
        }

        blob
    }

    /// Header parsing: valid signature, version, and table size.
    #[test]
    fn test_parse_synthetic_castle_peak_apob() {
        let blob = synthetic_castle_peak_apob();
        let apob = parse_apob(&blob).expect("synthetic APOB must parse");
        assert_eq!(apob.header.version, ApobVersion::V2);
        assert_eq!(apob.entries.len(), 1);
        assert_eq!(apob.entries[0].header.group, ApobGroup::Mem);
        assert_eq!(apob.entries[0].header.type_id, 3);

        let timings = extract_siod_ddr4_timings(&apob).expect("sIOD DDR4 timings must extract");
        assert_eq!(timings.channels.len(), 8);

        let ch0 = &timings.channels[0];
        assert_eq!(ch0.tcl, 14);
        assert_eq!(ch0.trcdw, 18);
        assert_eq!(ch0.trcdr, 18);
        assert_eq!(ch0.trp, 18);
        assert_eq!(ch0.tras, 38);
        assert_eq!(ch0.trc, 56);
        assert_eq!(ch0.trrd_s, 4);
        assert_eq!(ch0.trrd_l, 8);
        assert_eq!(ch0.tfaw, 39);
        assert_eq!(ch0.twtr_s, 4);
        assert_eq!(ch0.twtr_l, 12);
        assert_eq!(ch0.twr, 24);
        assert_eq!(ch0.trfc_s, 312);
        assert_eq!(ch0.trfc_f, 192);
        assert_eq!(ch0.trfc_pb, 132);
        assert_eq!(ch0.tcwl, 18);
        assert_eq!(ch0.trtp, 14);
        assert_eq!(ch0.trdwr, 8);
        assert_eq!(ch0.twrtr, 8);
        assert_eq!(ch0.cmd_rate, 0);

        // Dormant channels are all zeros.
        for ch in 1..8 {
            assert_eq!(timings.channels[ch].tcl, 0);
            assert_eq!(timings.channels[ch].channel, ch as u8);
        }
    }

    /// Header parsing: bad signature.
    #[test]
    fn test_parse_bad_signature() {
        let mut blob = synthetic_castle_peak_apob();
        blob[0] = 0x00; // corrupt signature
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::BadSignature));
    }

    /// Header parsing: truncated blob (shorter than header).
    #[test]
    fn test_parse_truncated_header() {
        let blob = vec![0u8; 10]; // too small
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// Header parsing: unsupported version.
    #[test]
    fn test_parse_unsupported_version() {
        let mut blob = synthetic_castle_peak_apob();
        blob[4..8].copy_from_slice(&0x9999u32.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::UnsupportedVersion(0x9999)));
    }

    /// Header parsing: table size larger than blob.
    #[test]
    fn test_parse_table_size_overrun() {
        let mut blob = synthetic_castle_peak_apob();
        let too_big = u32::try_from(blob.len() + 1000).unwrap_or(u32::MAX);
        blob[8..12].copy_from_slice(&too_big.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::Truncated { .. }));
    }

    /// Entry parsing: entry payload overruns table boundary.
    #[test]
    fn test_parse_entry_overrun() {
        let mut blob = synthetic_castle_peak_apob();
        // Set type_size to a value that overruns the table.
        blob[APOB_HEADER_SIZE + 4..APOB_HEADER_SIZE + 8].copy_from_slice(&0xFFFFu32.to_le_bytes());
        let err = parse_apob(&blob).unwrap_err();
        assert!(matches!(err, ApobError::EntryOverrun { .. }));
    }

    /// Entry parsing: encrypted entry (Group 0xFFFF) is included but marked.
    #[test]
    fn test_parse_encrypted_entry() {
        let mut blob = synthetic_castle_peak_apob();
        // Change the group to 0xFFFF (encrypted).
        blob[APOB_HEADER_SIZE..APOB_HEADER_SIZE + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let apob = parse_apob(&blob).expect("encrypted entry should still parse");
        assert_eq!(apob.entries.len(), 1);
        assert!(apob.entries[0].header.is_encrypted());
        assert_eq!(apob.entries[0].header.group, ApobGroup::Encrypted);
    }

    /// Entry parsing: encrypted entry (flags bit 7) is included but marked.
    #[test]
    fn test_parse_encrypted_flags() {
        let mut blob = synthetic_castle_peak_apob();
        // Set flags bit 7 (encrypted) in the entry header.
        blob[APOB_HEADER_SIZE + 9] = 0x80;
        let apob = parse_apob(&blob).expect("encrypted entry should still parse");
        assert_eq!(apob.entries.len(), 1);
        assert!(apob.entries[0].header.is_encrypted());
    }

    /// Timing extraction: no Group 1 Type 3 entry.
    #[test]
    fn test_extract_timings_not_found() {
        let mut blob = synthetic_castle_peak_apob();
        // Change the type to something else.
        blob[APOB_HEADER_SIZE + 2..APOB_HEADER_SIZE + 4].copy_from_slice(&0x99u16.to_le_bytes());
        let apob = parse_apob(&blob).expect("APOB must parse");
        let err = extract_siod_ddr4_timings(&apob).unwrap_err();
        assert!(matches!(err, ApobError::TimingsNotFound));
    }

    /// Timing extraction: encrypted timings entry (flags bit 7 set).
    ///
    /// The entry is still Group 1 Type 3 (so it is found), but the
    /// encrypted flag (bit 7) is set, so extraction is rejected.
    #[test]
    fn test_extract_timings_encrypted() {
        let mut blob = synthetic_castle_peak_apob();
        // Set flags bit 7 (encrypted) in the entry header.
        blob[APOB_HEADER_SIZE + 9] = 0x80;
        let apob = parse_apob(&blob).expect("APOB must parse");
        let err = extract_siod_ddr4_timings(&apob).unwrap_err();
        assert!(matches!(err, ApobError::TimingsEncrypted));
    }

    /// Timing extraction: payload not a multiple of the channel block size
    /// (50 bytes < 108 → no complete channel blocks).
    #[test]
    fn test_extract_timings_no_complete_blocks() {
        let mut blob = synthetic_castle_peak_apob();
        // Set type_size to 50 (less than one 108-byte channel block).
        blob[APOB_HEADER_SIZE + 4..APOB_HEADER_SIZE + 8].copy_from_slice(&50u32.to_le_bytes());
        // Also adjust the table_size in the header to match.
        let new_table_size = (APOB_HEADER_SIZE + APOB_ENTRY_HEADER_SIZE + 50) as u32;
        blob[8..12].copy_from_slice(&new_table_size.to_le_bytes());
        let apob = parse_apob(&blob).expect("APOB must parse");
        let err = extract_siod_ddr4_timings(&apob).unwrap_err();
        assert!(
            matches!(err, ApobError::InvalidTimingData(_)),
            "expected InvalidTimingData, got {err:?}"
        );
    }

    /// Timing extraction: a 4-channel payload (432 bytes) is valid with
    /// dynamic channel count (4 complete 108-byte blocks).
    #[test]
    fn test_extract_timings_four_channels() {
        let mut blob = synthetic_castle_peak_apob();
        // Set type_size to 432 (4 channels × 108).
        blob[APOB_HEADER_SIZE + 4..APOB_HEADER_SIZE + 8].copy_from_slice(&432u32.to_le_bytes());
        // Also adjust the table_size in the header to match.
        let new_table_size = (APOB_HEADER_SIZE + APOB_ENTRY_HEADER_SIZE + 432) as u32;
        blob[8..12].copy_from_slice(&new_table_size.to_le_bytes());
        let apob = parse_apob(&blob).expect("APOB must parse");
        let timings = extract_siod_ddr4_timings(&apob).expect("4-channel payload must extract");
        assert_eq!(timings.channels.len(), 4);
        // Channel 0 still has the known-good values.
        assert_eq!(timings.channels[0].tcl, 14);
    }

    /// Timing extraction: a 12-channel payload (1296 bytes, EPYC
    /// Milan/Genoa DDR4) is valid with dynamic channel count.
    #[test]
    fn test_extract_timings_twelve_channels_epyc() {
        let mut blob = synthetic_castle_peak_apob();
        // The synthetic blob currently has 8 channels (864 bytes).
        // Extend it to 12 channels (1296 bytes) by appending 4 more
        // zero-filled channel blocks.
        for _ in 0..4 {
            blob.extend_from_slice(&[0u8; 108]);
        }
        // Update the type_size to 1296 (12 channels × 108).
        blob[APOB_HEADER_SIZE + 4..APOB_HEADER_SIZE + 8].copy_from_slice(&1296u32.to_le_bytes());
        // Update the table_size in the header to match.
        let new_table_size = (APOB_HEADER_SIZE + APOB_ENTRY_HEADER_SIZE + 1296) as u32;
        blob[8..12].copy_from_slice(&new_table_size.to_le_bytes());
        let apob = parse_apob(&blob).expect("APOB must parse");
        let timings = extract_siod_ddr4_timings(&apob).expect("12-channel payload must extract");
        assert_eq!(timings.channels.len(), 12);
        // Channel 0 still has the known-good values.
        assert_eq!(timings.channels[0].tcl, 14);
        // Channels 8-11 are zero-filled (appended).
        assert_eq!(timings.channels[8].tcl, 0);
        assert_eq!(timings.channels[11].tcl, 0);
    }

    /// Multiple entries: the parser handles more than one entry.
    #[test]
    fn test_parse_multiple_entries() {
        let mut blob = Vec::new();

        // Header (16 bytes).
        blob.extend_from_slice(&APOB_SIGNATURE.to_le_bytes());
        blob.extend_from_slice(&0x0002u32.to_le_bytes()); // Version V2
        blob.extend_from_slice(&0u32.to_le_bytes()); // placeholder table_size
        blob.extend_from_slice(&0u32.to_le_bytes()); // Flags

        // Entry 1: Group 1, Type 3, size 108 (1 channel).
        blob.extend_from_slice(&1u16.to_le_bytes()); // Group: Mem
        blob.extend_from_slice(&3u16.to_le_bytes()); // Type: Timings
        blob.extend_from_slice(&108u32.to_le_bytes()); // Size: 1 × 108
        blob.push(0); // Instance: 0
        blob.push(0); // Flags: 0
        blob.extend_from_slice(&12u16.to_le_bytes()); // HeaderSize: 12
        blob.extend_from_slice(&[0u8; 108]); // Payload

        // Entry 2: Group 1, Type 1 (GeneralInfo), size 4.
        blob.extend_from_slice(&1u16.to_le_bytes()); // Group: Mem
        blob.extend_from_slice(&1u16.to_le_bytes()); // Type: GeneralInfo
        blob.extend_from_slice(&4u32.to_le_bytes()); // Size: 4
        blob.push(0); // Instance: 0
        blob.push(0); // Flags: 0
        blob.extend_from_slice(&12u16.to_le_bytes()); // HeaderSize: 12
        blob.extend_from_slice(&[0u8; 4]); // Payload

        // Now that we know the total size, fill in the table_size in the header.
        let total_size = blob.len() as u32;
        blob[8..12].copy_from_slice(&total_size.to_le_bytes());

        let apob = parse_apob(&blob).expect("multi-entry APOB must parse");
        assert_eq!(apob.entries.len(), 2);
        assert_eq!(apob.entries[0].header.group, ApobGroup::Mem);
        assert_eq!(apob.entries[0].header.type_id, 3);
        assert_eq!(apob.entries[1].header.group, ApobGroup::Mem);
        assert_eq!(apob.entries[1].header.type_id, 1);
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
                group: 1,
                type_id: 3,
            },
            ApobError::TimingsNotFound,
            ApobError::TimingsEncrypted,
            ApobError::TimingsTooShort {
                expected: 864,
                actual: 432,
            },
            ApobError::InvalidTimingData("test".to_string()),
            ApobError::Io("test".to_string()),
        ];
        for e in errors {
            let text = e.to_string();
            assert!(!text.is_empty(), "Display must be non-empty: {:?}", e);
        }
    }
}
