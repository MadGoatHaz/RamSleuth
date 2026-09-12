//! Pure SPD decode (P2-09): JEP106 maker, rank, density, base speed,
//! XMP 2.0 / XMP 3.0-EXPO profiles.
//!
//! This module consumes the raw [`SpdImage`] produced by the P2-08
//! `ee1004` acquisition (512-byte DDR4 / 1024-byte DDR5 full images) and
//! decodes it into the frozen [`SpdModule`] display struct. Pure bit
//! decoding: **no I/O, no `unsafe`, no new dependencies** - every byte is
//! read through the bounds-checked [`get`] helper, so a truncated or
//! malformed image degrades to `Na` fields, never a panic (plan D5).
//!
//! # Layout (documented model)
//!
//! DDR4 (SPD5116 / JESD79-4, 512 B) and DDR5 (SPD5378 / JESD79-5, 1024 B)
//! share the module-description block used here:
//!
//! | Byte                                   | Meaning                                            |
//! |----------------------------------------|----------------------------------------------------|
//! | `0x00`                                 | Row location (bits 7:5) + memory type (bits 4:0): `0x0A` DDR4 / `0x0C` DDR5 |
//! | `0x01` / `0x02`                        | Module manufacturer JEP106 vendor / continuation nibble |
//! | `0x2E` / `0x2F` (DDR5), `0x100` / `0x101` (DDR4) | DRAM die manufacturer JEP106 |
//! | `0x13`                                 | SDRAM density (code -> Gb)                         |
//! | `0x20`                                 | Minimum data rate, in 100 MT/s units               |
//! | `0x80`                                 | Rank config: bits 7:4 total DRAM devices, bits 3:0 per rank |
//! | `0x81..0x91`                           | Module part number (16 ASCII chars)                |
//! | `0x91..0xA1`                           | Module serial number (16 ASCII chars)              |
//! | `0xD0` / `0xF0` (DDR4)                 | XMP 2.0 profiles 1/2 (32-byte blocks)              |
//! | `0x200..0x300` (DDR5)                  | XMP 3.0 / EXPO region (256 B; four 32-byte blocks at `+16+32n`) |
//!
//! DDR5 byte `0x13` / `0x20` / `0x80` encodings are a documented
//! P2-04-style model (the live test host is DDR4); live reconciliation
//! happens at P2-11/QA.
//!
//! # JEP106 decoding
//!
//! The JEP106-0001 manufacturer code is assembled from two nibbles: the
//! vendor nibble (lower 4 bits of byte `0x01` / `0x2E` / `0x100`) plus
//! the continuation nibble (the companion byte `0x02` / `0x2F` / `0x101`).
//! A vendor nibble of `0000b` means "no manufacturer ID present", and
//! `1111b` means the full 8-bit byte *is* the code. Known codes map to
//! names via the [`JEP106`] table; an unknown non-zero code degrades to
//! its raw hex form (still a `Value`), never a panic.
//!
//! # Profiles
//!
//! - **XMP 2.0** (DDR4): 32-byte blocks at `0xD0` (slot 1) and `0xF0`
//!   (slot 2). A slot is valid when non-blank, its revision byte is
//!   `0x0A`, and its checksum (the sum of all 32 bytes, including the
//!   checksum byte, is `0` mod 256); a blank / mis-revision /
//!   bad-checksum slot is skipped, but the module is still shown.
//! - **XMP 3.0 / EXPO** (DDR5): 256-byte region at `0x200`, gated on its
//!   `XMP` signature, revision `0x30`, and length `0x100`; four 32-byte
//!   profile blocks at `+16 + 32n` are valid when non-blank, correctly
//!   indexed, and carrying a non-zero data-validity mask (bit 0 timings,
//!   bit 1 voltage, bit 2 frequency).
//!
//! # Safety
//!
//! No `unsafe`: a pure bounds-checked decode over the frozen
//! [`SpdImage`]. [`decode`] never panics for any input length.

use crate::error::{NaReason, Section};
use crate::spd_eeprom::SpdImage;

// ---------------------------------------------------------------------------
// SPD layout (byte offsets; see the module doc table).
// ---------------------------------------------------------------------------

/// Byte `0x00`: row location (bits 7:5) + memory type (bits 4:0).
const BYTE_MEMORY_TYPE: usize = 0x00;
/// DDR4 SDRAM memory-type code (`1010b`).
const MEM_TYPE_DDR4: u8 = 0x0A;
/// DDR5 SDRAM memory-type code (`1100b`).
const MEM_TYPE_DDR5: u8 = 0x0C;

/// Byte `0x01`: module manufacturer JEP106 vendor nibble.
const BYTE_MODULE_MAKER: usize = 0x01;
/// Byte `0x02`: module manufacturer JEP106 continuation nibble.
const BYTE_MODULE_MAKER_CONT: usize = 0x02;

/// DDR5 byte `0x2E`: DRAM die manufacturer JEP106 vendor nibble.
const BYTE_DDR5_DIE_MAKER: usize = 0x2E;
/// DDR5 byte `0x2F`: DRAM die manufacturer JEP106 continuation nibble.
const BYTE_DDR5_DIE_MAKER_CONT: usize = 0x2F;
/// DDR4 byte `0x100`: DRAM die manufacturer JEP106 vendor nibble.
const BYTE_DDR4_DIE_MAKER: usize = 0x100;
/// DDR4 byte `0x101`: DRAM die manufacturer JEP106 continuation nibble.
const BYTE_DDR4_DIE_MAKER_CONT: usize = 0x101;

/// Byte `0x13`: SDRAM density code.
const BYTE_DENSITY: usize = 0x13;
/// Byte `0x20`: minimum data rate, in 100 MT/s units.
const BYTE_BASE_SPEED: usize = 0x20;
/// Byte `0x80`: rank config (bits 7:4 total devices / bits 3:0 per rank).
const BYTE_RANK_CONFIG: usize = 0x80;

/// `0x81..=0x90`: module part number (16 ASCII chars).
const PART_START: usize = 0x81;
/// Part-number field length in bytes.
const PART_LEN: usize = 16;
/// `0x91..=0xA0`: module serial number (16 ASCII chars).
const SERIAL_START: usize = 0x91;
/// Serial-number field length in bytes.
const SERIAL_LEN: usize = 16;

/// Full size of a DDR5 SPD image (8 Kbit EEPROM).
const SPD_IMAGE_LEN_DDR5: usize = 1024;

/// Recognized JEP106-0001 manufacturer codes (full 8-bit code -> name).
///
/// Frozen (P2-09): the named `const` table consumed by [`decode_maker`];
/// codes outside the table degrade to their raw hex form.
pub const JEP106: &[(u8, &str)] = &[
    (0x01, "Intel"),
    (0x02, "AMD"),
    (0x0B, "Hitachi"),
    (0x0C, "NEC"),
    (0x0D, "Toshiba"),
    (0x89, "SK hynix"),
    (0x92, "Samsung"),
    (0x93, "Elpida"),
    (0x94, "Qimonda"),
    (0x96, "Winbond"),
    (0x97, "Macronix"),
    (0x98, "Nanya"),
    (0xC2, "Micron"),
];

// ---------------------------------------------------------------------------
// Frozen public API (P2-09 interface freeze).
// ---------------------------------------------------------------------------

/// One factory-rated memory profile (XMP 2.0 on DDR4, XMP 3.0 / EXPO on
/// DDR5).
///
/// Frozen (P2-09). Every field is a [`Section`]: an absent / zeroed /
/// invalid field degrades to `Na`, never a panic. `speed_mts` is the
/// profile data rate (2x the profile clock); `voltage` is in
/// millivolts (consistent with the P2-05 mV display contract).
#[derive(Debug, Clone, PartialEq)]
pub struct SpdProfile {
    /// 1-based profile slot number.
    pub index: u8,
    /// Profile data rate in MT/s.
    pub speed_mts: Section<u16>,
    /// CAS latency (`tCL`) in ticks.
    pub cas: Section<u8>,
    /// `tRCD` in ticks.
    pub trcd: Section<u8>,
    /// `tRP` in ticks.
    pub trp: Section<u8>,
    /// `tRAS` in ticks.
    pub tras: Section<u8>,
    /// SDRAM core voltage in millivolts.
    pub voltage: Section<u16>,
}

/// A fully-decoded SPD module (one DIMM slot).
///
/// Frozen (P2-09). The P2-10 facade carries `Vec<SpdModule>` into
/// `SystemMemoryTelemetry.spd`. `index` is the I2C address from the
/// P2-08 [`SpdImage`]; every other field is a [`Section`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpdModule {
    /// Module index: the I2C address of the `ee1004` device
    /// ([`SpdImage::index`]).
    pub index: u8,
    /// `true` when the module classifies as DDR5 ([`classify_ddr5`]).
    pub is_ddr5: bool,
    /// Module manufacturer (JEP106, bytes `0x01`/`0x02`), falling back
    /// to the DRAM die manufacturer when no module ID is present.
    pub maker: Section<String>,
    /// Module part number (bytes `0x81..0x91`, 16 ASCII chars).
    pub part: Section<String>,
    /// Module serial number (bytes `0x91..0xA1`, 16 ASCII chars).
    pub serial: Section<String>,
    /// Number of ranks (derived from the byte `0x80` rank config).
    pub rank: Section<u8>,
    /// Per-DRAM density in Mbit.
    pub density_mbit: Section<u16>,
    /// Minimum guaranteed module data rate in MT/s.
    pub speed_mts: Section<u16>,
    /// Factory-rated profiles (XMP 2.0 on DDR4, XMP 3.0 / EXPO on
    /// DDR5); empty when the module carries none.
    pub profiles: Vec<SpdProfile>,
}

/// Decode a raw SPD image into a populated [`SpdModule`].
///
/// Pure, never panics: every byte read goes through the bounds-checked
/// [`get`]; an out-of-range or invalid byte degrades only the affected
/// field to `Na`. A truncated image (any length) yields a module whose
/// past-end fields are `Na`.
pub fn decode(image: &SpdImage) -> SpdModule {
    let data = &image.data;
    let is_ddr5 = classify_ddr5(data);
    SpdModule {
        index: image.index,
        is_ddr5,
        maker: decode_maker(data, is_ddr5),
        part: decode_ascii(data, PART_START, PART_LEN, "part number"),
        serial: decode_ascii(data, SERIAL_START, SERIAL_LEN, "serial number"),
        rank: decode_rank(data),
        density_mbit: decode_density(data, is_ddr5),
        speed_mts: decode_base_speed(data),
        profiles: decode_profiles(data, is_ddr5),
    }
}

// ---------------------------------------------------------------------------
// Bounds-checked reads + field decoders.
// ---------------------------------------------------------------------------

/// Bounds-checked byte read: `None` when `off` is outside the image
/// (never panics, never indexes).
fn get(data: &[u8], off: usize) -> Option<u8> {
    data.get(off).copied()
}

/// The `Na` reason for a byte outside the image bounds.
fn oob(off: usize) -> NaReason {
    NaReason::ParseError(format!("byte 0x{off:02X} outside image bounds"))
}

/// Classify the module by its header signature (byte `0x00`, bits 4:0):
/// `0x0C` -> DDR5, `0x0A` -> DDR4. An unrecognized signature falls back
/// to the image length (1024 B -> DDR5, otherwise DDR4).
fn classify_ddr5(data: &[u8]) -> bool {
    match get(data, BYTE_MEMORY_TYPE).map(|b| b & 0x1F) {
        Some(MEM_TYPE_DDR5) => true,
        Some(MEM_TYPE_DDR4) => false,
        _ => data.len() == SPD_IMAGE_LEN_DDR5,
    }
}

/// Assemble the full 8-bit JEP106 code from its vendor + continuation
/// nibbles: vendor `1111b` -> the full byte is the code; vendor
/// `0000b` -> "no manufacturer ID present" (code 0); otherwise
/// `(continuation << 4) | vendor`.
fn jep106_code(vendor: u8, continuation: u8) -> u8 {
    let v = vendor & 0x0F;
    if v == 0x0F {
        vendor
    } else if v == 0x00 {
        0x00
    } else {
        ((continuation & 0x0F) << 4) | v
    }
}

/// The name for a JEP106 code, or `None` when the code is not in the
/// [`JEP106`] table.
fn jep106_name(code: u8) -> Option<&'static str> {
    JEP106.iter().find(|(c, _)| *c == code).map(|(_, name)| *name)
}

/// Render a non-zero JEP106 code: a name from the [`JEP106`] table, or
/// its raw hex form when the code is unknown.
fn maker_string(code: u8) -> Section<String> {
    match jep106_name(code) {
        Some(name) => Section::Value(name.to_owned()),
        None => Section::Value(format!("0x{code:02X}")),
    }
}

/// Decode the module manufacturer: JEP106 vendor + continuation at
/// bytes `0x01`/`0x02`; when no module ID is present, falls back to the
/// DRAM die manufacturer (DDR5 `0x2E`/`0x2F`, DDR4 `0x100`/`0x101`).
/// All-absent -> `Na(NotApplicable)`.
fn decode_maker(data: &[u8], is_ddr5: bool) -> Section<String> {
    let vendor = get(data, BYTE_MODULE_MAKER).unwrap_or(0);
    let cont = get(data, BYTE_MODULE_MAKER_CONT).unwrap_or(0);
    let code = jep106_code(vendor, cont);
    if code != 0x00 {
        return maker_string(code);
    }
    let (dv, dc) = if is_ddr5 {
        (
            get(data, BYTE_DDR5_DIE_MAKER).unwrap_or(0),
            get(data, BYTE_DDR5_DIE_MAKER_CONT).unwrap_or(0),
        )
    } else {
        (
            get(data, BYTE_DDR4_DIE_MAKER).unwrap_or(0),
            get(data, BYTE_DDR4_DIE_MAKER_CONT).unwrap_or(0),
        )
    };
    let dcode = jep106_code(dv, dc);
    if dcode == 0x00 {
        Section::na(NaReason::NotApplicable)
    } else {
        maker_string(dcode)
    }
}

/// Decode a 16-char ASCII module field (part number / serial number):
/// read up to `len` bytes, stopping at the first null / space padding
/// byte or non-ASCII byte (fields are space-padded ASCII per
/// JESD79-4/5). Blank -> `Na(NotApplicable)`; an out-of-range byte ->
/// `Na(ParseError)` (the field is incomplete).
fn decode_ascii(data: &[u8], start: usize, len: usize, what: &str) -> Section<String> {
    let mut s = String::new();
    for i in 0..len {
        match get(data, start + i) {
            None => {
                return Section::na(NaReason::ParseError(format!(
                    "{what}: byte 0x{:02X} outside image bounds",
                    start + i
                )))
            }
            Some(b) if b == 0x00 || b == 0x20 || !(0x21..=0x7E).contains(&b) => break,
            Some(b) => s.push(b as char),
        }
    }
    if s.is_empty() {
        Section::na(NaReason::NotApplicable)
    } else {
        Section::Value(s)
    }
}

/// Decode the rank count from byte `0x80`: bits 7:4 = total DRAM
/// devices, bits 3:0 = devices per rank; ranks = total / per-rank.
/// Zero / non-divisible config -> `Na(ParseError)`.
fn decode_rank(data: &[u8]) -> Section<u8> {
    let Some(cfg) = get(data, BYTE_RANK_CONFIG) else {
        return Section::na(oob(BYTE_RANK_CONFIG));
    };
    let total = cfg >> 4;
    let per_rank = cfg & 0x0F;
    if total == 0 || per_rank == 0 || total % per_rank != 0 {
        return Section::na(NaReason::ParseError(format!(
            "rank config byte 0x80 = 0x{cfg:02X} (total {total} / per-rank {per_rank}) is invalid"
        )));
    }
    Section::Value(total / per_rank)
}

/// Decode the SDRAM density (byte `0x13`) into Mbit:
/// - DDR4: code `0x10..=0x17` -> 2^(code-0x10) Gb (1..128 Gb);
/// - DDR5: documented model, code `0x11..=0x18` -> 1..64 Gb.
///
/// An unrecognized code -> `Na(ParseError)`; a result >= 64 Gb
/// (= 65536 Mbit) overflows the u16 cell -> `Na(ParseError)`.
fn decode_density(data: &[u8], is_ddr5: bool) -> Section<u16> {
    let Some(code) = get(data, BYTE_DENSITY) else {
        return Section::na(oob(BYTE_DENSITY));
    };
    let gb: u32 = if is_ddr5 {
        match code {
            0x11 => 1,
            0x12 => 2,
            0x13 => 4,
            0x14 => 8,
            0x15 => 16,
            0x16 => 24,
            0x17 => 32,
            0x18 => 64,
            _ => {
                return Section::na(NaReason::ParseError(format!(
                    "DDR5 density code 0x{code:02X} not recognized"
                )))
            }
        }
    } else if (0x10..=0x17).contains(&code) {
        1u32 << (code - 0x10)
    } else {
        return Section::na(NaReason::ParseError(format!(
            "DDR4 density code 0x{code:02X} not recognized"
        )));
    };
    let mbit = gb * 1024;
    match u16::try_from(mbit) {
        Ok(v) => Section::Value(v),
        Err(_) => Section::na(NaReason::ParseError(format!(
            "{gb} Gb = {mbit} Mbit exceeds the u16 density cell"
        ))),
    }
}

/// Decode the minimum guaranteed data rate (byte `0x20`): the code is
/// in 100 MT/s units, so `mts = code * 100` (max 25500, fits a u16).
/// Zero -> `Na(ParseError)`.
fn decode_base_speed(data: &[u8]) -> Section<u16> {
    let Some(code) = get(data, BYTE_BASE_SPEED) else {
        return Section::na(oob(BYTE_BASE_SPEED));
    };
    if code == 0 {
        return Section::na(NaReason::ParseError(
            "base speed byte 0x20 is 0 (no minimum data rate recorded)".to_owned(),
        ));
    }
    Section::Value(u16::from(code) * 100)
}

// ---------------------------------------------------------------------------
// XMP 2.0 (DDR4) + XMP 3.0 / EXPO (DDR5) profile decode.
// ---------------------------------------------------------------------------

/// DDR4 XMP 2.0 profile block size in bytes (one 32-byte structure per
/// slot, at `0xD0` and `0xF0`).
const XMP2_BLOCK: usize = 32;
/// DDR4 XMP 2.0 slot-1 base (bytes `0xD0..0xF0`).
const XMP2_SLOT1: usize = 0xD0;
/// DDR4 XMP 2.0 slot-2 base (bytes `0xF0..0x100`).
const XMP2_SLOT2: usize = 0xF0;
/// XMP 2.0 structure revision byte value (`0x0A`).
const XMP2_REVISION: u8 = 0x0A;

/// DDR5 XMP 3.0 / EXPO region base (bytes `0x200..0x300`).
const XMP3_BASE: usize = 0x200;
/// XMP 3.0 / EXPO region size in bytes (`0x100` = 256).
const XMP3_REGION: usize = 0x100;
/// XMP 3.0 / EXPO revision byte value (`0x30`).
const XMP3_REVISION: u8 = 0x30;
/// XMP 3.0 / EXPO region signature bytes (`0x58 0x4D 0x50` = "XMP").
const XMP3_SIGNATURE: [u8; 3] = [0x58, 0x4D, 0x50];
/// Offset of the first profile block inside the XMP 3.0 / EXPO region
/// (after the 16-byte region header).
const XMP3_PROFILE_OFFSET: usize = 16;
/// XMP 3.0 / EXPO profile block size in bytes (one 32-byte structure
/// per profile).
const XMP3_BLOCK: usize = 32;
/// XMP 3.0 / EXPO maximum profile count (four 32-byte blocks fit in the
/// 256-byte region after the 16-byte header).
const XMP3_MAX_PROFILES: usize = 4;

/// Decode the factory-rated profiles of one SPD image: XMP 2.0 slots for
/// DDR4, the XMP 3.0 / EXPO region for DDR5. Never panics; a truncated
/// image or a missing / invalid region yields an empty profile list.
fn decode_profiles(data: &[u8], is_ddr5: bool) -> Vec<SpdProfile> {
    if is_ddr5 {
        decode_xmp3(data)
    } else {
        let mut out = Vec::new();
        // XMP 2.0: two fixed slots (profile 1 at `0xD0`, profile 2 at
        // `0xF0`); each is decoded independently and an invalid / blank
        // slot simply contributes nothing.
        for (index, base) in [1u8, 2].iter().zip([XMP2_SLOT1, XMP2_SLOT2].iter()) {
            if let Some(p) = decode_xmp2(data, *index, *base) {
                out.push(p);
            }
        }
        out
    }
}

/// Decode one DDR4 XMP 2.0 32-byte slot at `base`.
///
/// Returns `None` when the slot is blank (all zero), the revision byte is
/// not [`XMP2_REVISION`], or the structure checksum fails (the sum of all
/// 32 bytes, including the checksum byte, must be `0` mod 256) - a
/// bad-checksum slot is skipped, but the module is still shown.
fn decode_xmp2(data: &[u8], index: u8, base: usize) -> Option<SpdProfile> {
    let block = read_block(data, base, XMP2_BLOCK)?;
    if block.iter().all(|&b| b == 0) {
        return None; // blank slot
    }
    if block[0] != XMP2_REVISION {
        return None; // not an XMP 2.0 structure
    }
    if checksum32(&block) != 0 {
        return None; // checksum failure -> profile skipped
    }
    xmp_profile(&block, |b| {
        // XMP 2.0 field offsets within the 32-byte structure.
        let speed = u16::from(b[30]) * 2; // frequency byte @30 (MHz) -> MT/s
        SpdProfile {
            index,
            speed_mts: non_zero_u16(speed, "XMP 2.0 frequency"),
            cas: non_zero_u8(b[4], "XMP 2.0 CL"),
            trcd: non_zero_u8(b[5], "XMP 2.0 tRCD"),
            trp: non_zero_u8(b[6], "XMP 2.0 tRP"),
            tras: non_zero_u8(b[7], "XMP 2.0 tRAS"),
            voltage: non_zero_u16(u16::from_le_bytes([b[19], b[20]]), "XMP 2.0 voltage"),
        }
    })
}

/// Decode the DDR5 XMP 3.0 / EXPO region (256 bytes at `0x200`).
///
/// The region is gated on its revision ([`XMP3_REVISION`]) and "XMP"
/// signature. Each profile block is 32 bytes at `+16 + 32n`; a blank or
/// zero-validity block is skipped. Returns the (possibly empty) list of
/// decoded profiles.
fn decode_xmp3(data: &[u8]) -> Vec<SpdProfile> {
    let region = read_block(data, XMP3_BASE, XMP3_REGION);
    let Some(region) = region else {
        return Vec::new(); // region truncated / OOB -> no profiles
    };
    if region[0] != XMP3_REVISION {
        return Vec::new();
    }
    if region[2..5] != XMP3_SIGNATURE {
        return Vec::new();
    }
    let mut out = Vec::new();
    for n in 0..XMP3_MAX_PROFILES {
        let base = XMP3_PROFILE_OFFSET + n * XMP3_BLOCK;
        let Some(block) = read_block(data, XMP3_BASE + base, XMP3_BLOCK) else {
            break; // ran past the image end
        };
        if block.iter().all(|&b| b == 0) {
            continue; // blank slot
        }
        // XMP 3.0 / EXPO: profile index @0 (should equal n), validity
        // mask @1 (bit 0 = timings present); a zero mask means "no data".
        if block[0] != n as u8 || block[1] == 0 {
            continue;
        }
        if let Some(p) = xmp_profile(&block, |b| {
            // XMP 3.0 / EXPO field offsets within the 32-byte structure.
            let speed = u16::from(b[2]) * 2; // frequency byte @2 (MHz) -> MT/s
            SpdProfile {
                index: n as u8,
                speed_mts: non_zero_u16(speed, "XMP 3.0 frequency"),
                cas: non_zero_u8(b[4], "XMP 3.0 CL"),
                trcd: non_zero_u8(b[5], "XMP 3.0 tRCD"),
                trp: non_zero_u8(b[6], "XMP 3.0 tRP"),
                tras: non_zero_u8(b[7], "XMP 3.0 tRAS"),
                voltage: non_zero_u16(u16::from_le_bytes([b[19], b[20]]), "XMP 3.0 voltage"),
            }
        }) {
            out.push(p);
        }
    }
    out
}

/// Copy a `len`-byte block starting at `base` out of `data`, bounds-checked:
/// `None` when the block would run past the image end (never panics).
fn read_block(data: &[u8], base: usize, len: usize) -> Option<Vec<u8>> {
    let end = base.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some(data[base..end].to_vec())
}

/// XMP 2.0 structure checksum: the sum of all 32 bytes (including the
/// checksum byte) mod 256 - a valid structure sums to `0`.
fn checksum32(block: &[u8]) -> u8 {
    block.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Build an [`SpdProfile`] from a 32-byte XMP structure via `build`.
/// Returns `None` when every field of the built profile is `Na` (nothing
/// usable was decoded) - the caller then skips the slot.
fn xmp_profile<F>(block: &[u8], build: F) -> Option<SpdProfile>
where
    F: FnOnce(&[u8; XMP2_BLOCK]) -> SpdProfile,
{
    let mut arr = [0u8; XMP2_BLOCK];
    arr.copy_from_slice(block);
    let p = build(&arr);
    // A slot whose fields are all Na carries no usable data -> skip it.
    let any_value = p.speed_mts.value().is_some()
        || p.cas.value().is_some()
        || p.trcd.value().is_some()
        || p.trp.value().is_some()
        || p.tras.value().is_some()
        || p.voltage.value().is_some();
    if any_value {
        Some(p)
    } else {
        None
    }
}

/// Wrap a non-zero `u16` as a [`Section::Value`]; `0` -> `Na(ParseError)`.
fn non_zero_u16(v: u16, what: &str) -> Section<u16> {
    if v == 0 {
        Section::na(NaReason::ParseError(format!("{what} is 0 (blank)")))
    } else {
        Section::Value(v)
    }
}

/// Wrap a non-zero `u8` as a [`Section::Value`]; `0` -> `Na(ParseError)`.
fn non_zero_u8(v: u8, what: &str) -> Section<u8> {
    if v == 0 {
        Section::na(NaReason::ParseError(format!("{what} is 0 (blank)")))
    } else {
        Section::Value(v)
    }
}
// ---------------------------------------------------------------------------
// Tests (plan P2-09 quality gates + brief items a-e).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Synthetic fixtures.
    // ------------------------------------------------------------------

    /// A full synthetic DDR4 image (512 B): Micron module, 8 Gb, 2000 MT/s
    /// minimum, 2 ranks, one valid XMP 2.0 profile (slot 1), blank slot 2.
    fn ddr4_image() -> SpdImage {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0A; // DDR4 signature
        // Module manufacturer: Micron 0xC2 (vendor 0x02, continuation 0x0C).
        data[0x01] = 0x02;
        data[0x02] = 0x0C;
        // 8 Gb density (DDR4 code 0x13 = 2^3 Gb).
        data[0x13] = 0x13;
        // Minimum data rate 2000 MT/s (20 x 100).
        data[0x20] = 0x14;
        // Rank config: 8 total devices, 4 per rank -> 2 ranks.
        data[0x80] = 0x84;
        // 16-char ASCII part/serial (null-terminated fields).
        data[0x81..0x81 + 15].copy_from_slice(b"MPK24168BC320G6");
        data[0x91..0x91 + 15].copy_from_slice(b"0ABCD123456789X");
        // XMP 2.0 slot 1 @ 0xD0: rev 0x0A, 200 MT/s, CL/tRCD/tRP/tRAS =
        // 16/16/16/34, 1350 mV, valid structure checksum.
        let mut block = [0u8; XMP2_BLOCK];
        block[0] = XMP2_REVISION;
        block[4] = 16;
        block[5] = 16;
        block[6] = 16;
        block[7] = 34;
        block[19] = 0x46; // 1350 mV little-endian
        block[20] = 0x05;
        block[30] = 0x64; // 100 MHz -> 200 MT/s
        let mut partial = 0u8;
        for b in block.iter().take(XMP2_BLOCK - 1) {
            partial = partial.wrapping_add(*b);
        }
        block[XMP2_BLOCK - 1] = partial.wrapping_neg(); // checksum -> 0 sum
        data[0xD0..0xD0 + XMP2_BLOCK].copy_from_slice(&block);
        SpdImage {
            index: 0x52,
            data,
        }
    }

    /// A full synthetic DDR5 image (1024 B): Samsung module, 16 Gb, 6400
    /// MT/s minimum, 1 rank, one valid XMP 3.0 / EXPO block, blank rest.
    fn ddr5_image() -> SpdImage {
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C; // DDR5 signature
        // Module manufacturer: Samsung 0x92 (vendor 0x02, continuation 0x09).
        data[0x01] = 0x02;
        data[0x02] = 0x09;
        // 16 Gb density (DDR5 code 0x15).
        data[0x13] = 0x15;
        // Minimum data rate 6400 MT/s (64 x 100).
        data[0x20] = 0x40;
        // Rank config: 8 total devices, 8 per rank -> 1 rank.
        data[0x80] = 0x88;
        data[0x81..0x81 + 13].copy_from_slice(b"S5H1G8719011A");
        data[0x91..0x91 + 16].copy_from_slice(b"2208ABCDEF123456");
        // XMP 3.0 / EXPO region @ 0x200: rev 0x30 + "XMP" signature @ +2.
        data[0x200] = XMP3_REVISION;
        data[0x202..0x205].copy_from_slice(&XMP3_SIGNATURE);
        // Profile 0 @ 0x210: index 0, validity mask 1, 256 MT/s,
        // CL/tRCD/tRP/tRAS = 20/20/20/40, 1250 mV.
        data[0x210] = 0x00;
        data[0x211] = 0x01;
        data[0x212] = 0x80; // 128 MHz -> 256 MT/s
        data[0x214] = 20;
        data[0x215] = 20;
        data[0x216] = 20;
        data[0x217] = 40;
        data[0x223] = 0xE2; // 1250 mV little-endian (block 19/20)
        data[0x224] = 0x04;
        SpdImage {
            index: 0x53,
            data,
        }
    }

    // ------------------------------------------------------------------
    // (a) Synthetic DDR4 decode.
    // ------------------------------------------------------------------

    /// The synthetic DDR4 image decodes to full ground truth (maker, part,
    /// serial, rank, density, speed) with exactly one valid XMP 2.0
    /// profile (slot 1; slot 2 is blank).
    #[test]
    fn synthetic_ddr4_decodes_to_ground_truth() {
        let m = decode(&ddr4_image());
        assert_eq!(m.index, 0x52);
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.part, Section::Value("MPK24168BC320G6".to_owned()));
        assert_eq!(m.serial, Section::Value("0ABCD123456789X".to_owned()));
        assert_eq!(m.rank, Section::Value(2));
        assert_eq!(m.density_mbit, Section::Value(8192));
        assert_eq!(m.speed_mts, Section::Value(2000));
        assert_eq!(m.profiles.len(), 1, "slot 1 valid, slot 2 blank");
        let p = &m.profiles[0];
        assert_eq!(p.index, 1);
        assert_eq!(p.speed_mts, Section::Value(200));
        assert_eq!(p.cas, Section::Value(16));
        assert_eq!(p.trcd, Section::Value(16));
        assert_eq!(p.trp, Section::Value(16));
        assert_eq!(p.tras, Section::Value(34));
        assert_eq!(p.voltage, Section::Value(1350));
    }

    // ------------------------------------------------------------------
    // (b) Synthetic DDR5 decode.
    // ------------------------------------------------------------------

    /// The synthetic DDR5 image decodes to full ground truth, including the
    /// XMP 3.0 / EXPO profile block.
    #[test]
    fn synthetic_ddr5_decodes_to_ground_truth() {
        let m = decode(&ddr5_image());
        assert_eq!(m.index, 0x53);
        assert!(m.is_ddr5);
        assert_eq!(m.maker, Section::Value("Samsung".to_owned()));
        assert_eq!(m.part, Section::Value("S5H1G8719011A".to_owned()));
        assert_eq!(m.serial, Section::Value("2208ABCDEF123456".to_owned()));
        assert_eq!(m.rank, Section::Value(1));
        assert_eq!(m.density_mbit, Section::Value(16384));
        assert_eq!(m.speed_mts, Section::Value(6400));
        assert_eq!(m.profiles.len(), 1, "profile 0 valid, the rest blank");
        let p = &m.profiles[0];
        assert_eq!(p.index, 0);
        assert_eq!(p.speed_mts, Section::Value(256));
        assert_eq!(p.cas, Section::Value(20));
        assert_eq!(p.trcd, Section::Value(20));
        assert_eq!(p.trp, Section::Value(20));
        assert_eq!(p.tras, Section::Value(40));
        assert_eq!(p.voltage, Section::Value(1250));
    }

    // ------------------------------------------------------------------
    // (c) Corrupted checksum + truncated images -> graceful, no panic.
    // ------------------------------------------------------------------

    /// An XMP 2.0 slot with a corrupted structure checksum is skipped; the
    /// module itself still decodes (brief: checksum failure -> profile
    /// skipped, module still shown).
    #[test]
    fn corrupted_xmp2_checksum_is_skipped_module_still_shown() {
        let mut data = ddr4_image().data;
        data[0xD0 + XMP2_BLOCK - 1] ^= 0xFF; // break the structure checksum
        let m = decode(&SpdImage {
            index: 0x52,
            data,
        });
        assert!(m.profiles.is_empty(), "bad checksum must drop the profile");
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.rank, Section::Value(2));
        assert_eq!(m.speed_mts, Section::Value(2000));
    }

    /// An image truncated to just its header + maker bytes decodes without
    /// panicking: the maker survives, every past-end field degrades to
    /// `Na(ParseError)`, and no profiles are decoded.
    #[test]
    fn truncated_header_only_image_degrades_to_na() {
        let data = ddr4_image().data[..3].to_vec(); // [0x0A, 0x02, 0x0C]
        let m = decode(&SpdImage {
            index: 0x52,
            data,
        });
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert!(matches!(&m.part, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.serial, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.rank, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.density_mbit, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.speed_mts, Section::Na(NaReason::ParseError(_))));
        assert!(m.profiles.is_empty());
    }

    /// A completely empty image decodes without panicking: every field is
    /// `Na`, the maker is `Na(NotApplicable)` (no module or die ID present).
    #[test]
    fn empty_image_decodes_to_all_na() {
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![],
        });
        assert_eq!(m.index, 0x50);
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Na(NaReason::NotApplicable));
        assert!(m.part.is_na());
        assert!(m.serial.is_na());
        assert!(m.rank.is_na());
        assert!(m.density_mbit.is_na());
        assert!(m.speed_mts.is_na());
        assert!(m.profiles.is_empty());
    }

    /// Sweep every prefix length of both fixture families (0..=512 and
    /// 0..=1024): `decode` never panics on any truncation and always
    /// preserves the module index.
    #[test]
    fn every_prefix_length_never_panics() {
        for full in [ddr4_image(), ddr5_image()] {
            let len = full.data.len();
            for n in 0..=len {
                let m = decode(&SpdImage {
                    index: full.index,
                    data: full.data[..n].to_vec(),
                });
                assert_eq!(m.index, full.index);
            }
        }
    }

    // ------------------------------------------------------------------
    // (d) JEP106 unknown / absent manufacturers.
    // ------------------------------------------------------------------

    /// A non-zero JEP106 code absent from the table renders as its raw hex
    /// form (still a `Value`), never a panic or `Na`.
    #[test]
    fn unknown_jep106_code_renders_raw_hex() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0A;
        data[0x01] = 0x0B; // vendor nibble
        data[0x02] = 0x0A; // continuation nibble -> 0xAB (unknown)
        let m = decode(&SpdImage {
            index: 0x50,
            data,
        });
        assert_eq!(m.maker, Section::Value("0xAB".to_owned()));
    }

    /// Vendor nibble `1111b`: the full byte is the code (0xFF is unknown
    /// -> raw hex).
    #[test]
    fn full_byte_jep106_vendor_renders_raw_hex() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0A;
        data[0x01] = 0xFF;
        let m = decode(&SpdImage {
            index: 0x50,
            data,
        });
        assert_eq!(m.maker, Section::Value("0xFF".to_owned()));
    }

    /// No module ID and no die ID at all -> `Na(NotApplicable)`.
    #[test]
    fn no_manufacturer_id_at_all_is_na() {
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![0u8; 512],
        });
        assert_eq!(m.maker, Section::Na(NaReason::NotApplicable));
    }

    /// No module ID present: the decoder falls back to the DRAM die
    /// manufacturer (DDR4 bytes 0x100/0x101).
    #[test]
    fn die_maker_fallback_when_module_id_absent() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0A;
        data[0x100] = 0x02; // Micron 0xC2 (vendor 0x02, continuation 0x0C)
        data[0x101] = 0x0C;
        let m = decode(&SpdImage {
            index: 0x50,
            data,
        });
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
    }

    // ------------------------------------------------------------------
    // (e) LIVE: the real P2-08 acquisition decoded on this host.
    // ------------------------------------------------------------------

    /// Run the actual `ee1004` acquisition and decode every image it
    /// returns. SPD presence is host-dependent: when no device is bound
    /// (or acquisition fails) the test passes vacuously. Never panics.
    #[test]
    fn live_acquire_decode_never_panics() {
        let images = match crate::spd_eeprom::acquire() {
            Ok(imgs) => imgs,
            Err(e) => {
                eprintln!("note: live acquire() -> Err({e}); skipping live decode");
                return;
            }
        };
        if images.is_empty() {
            eprintln!(
                "note: no bound SPD EEPROM on this host; live decode passes vacuously"
            );
            return;
        }
        for image in &images {
            let m = decode(image);
            assert_eq!(
                m.index, image.index, "decode must preserve the module index"
            );
            eprintln!(
                "live SPD {:#04x}: ddr5={} maker={:?} part={:?} serial={:?} rank={:?} density_mbit={:?} speed_mts={:?} profiles={}",
                m.index,
                m.is_ddr5,
                m.maker,
                m.part,
                m.serial,
                m.rank,
                m.density_mbit,
                m.speed_mts,
                m.profiles.len()
            );
            for p in &m.profiles {
                eprintln!(
                    "  profile {}: speed={:?} cas={:?} trcd={:?} trp={:?} tras={:?} voltage={:?}",
                    p.index, p.speed_mts, p.cas, p.trcd, p.trp, p.tras, p.voltage
                );
            }
        }
    }
}

