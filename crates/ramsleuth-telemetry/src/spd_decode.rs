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
//! DDR4 (SPD5118 / JESD79-4, 512 B) and DDR5 (SPD5378 / JESD79-5, 1024 B)
//! share the basic-info header; the field locations below are
//! generation-scoped (the DDR5 arms are a documented P2-04-style model,
//! untouched by the DDR4 re-baseline):
//!
//! | Byte                                   | Meaning                                            |
//! |----------------------------------------|----------------------------------------------------|
//! | `0x00`                                 | SPD bytes used; bits 4:0 also carry the legacy memory-type check (`0x0A` DDR4 / `0x0C` DDR5) - the documented fallback when `0x02` is unrecognized (C8-02) |
//! | `0x02`                                 | Basic-info memory type (JESD79-4/5): `0x0C` = DDR4 (512 B image) / DDR5 (1024 B image - the generations share the code), `0x0B` = DDR3 - the primary classification (C8-02) |
//! | `0x04` (DDR4)                          | SDRAM density: bits 3:0 -> a 10-entry Mbit table (`0` = 256 ... `6` = 16384, `8` = 12288, `9` = 24576); codes `0xA..=0xF` -> `Na` |
//! | `0x0B` (DDR4)                          | Voltage: bits 3:0 (`0x03` = 1.2 V); check-only (no struct field) |
//! | `0x0C` (DDR4)                          | Module organization (JESD79-4): bits 5:3 = number of ranks - 1 (`0` = 1 ... `7` = 8), bits 2:0 = SDRAM device width (`0 = x4 / 1 = x8 / 2 = x16 / 3 = x32`) - the DDR4 rank + width primary |
//! | `0x0D` (DDR4)                          | Bus width: bits 2:0 (`3` = x64 desktop bus); non-`3` -> `devices` `Na` naming `0x0D` |
//! | `0x11` (DDR4)                          | Timebase: DDR4 fixed MTB = 125 ps, FTB = 1 ps; the byte is a reserved-code check only (it never gates the constants) |
//! | `0x12` (DDR4)                          | `tCKAVGmin` (MTB units) + signed FTB byte `0x7D` (ps) - the JEDEC base-speed fallback; `0x7D` in {`0x6E`, `0xF8`} = sentinel -> base speed `Na` |
//! | `0x13` (DDR4)                          | `tCKAVGmax` (MTB units) + signed FTB byte `0x7C` (ps); check-only |
//! | `0x13` (DDR5)                          | SDRAM density code (`0x11..=0x18` -> 1..64 Gb; the DDR5 documented model) |
//! | `0x14..0x17` (DDR4)                    | CL supported bitmap; check-only (the profile CL comes from the XMP bitmap) |
//! | `0x20` (DDR5)                          | Minimum data rate, in 100 MT/s units (the DDR5 documented model) |
//! | `0x2E` / `0x2F` (DDR5)                 | DRAM die manufacturer JEP106 (8-bit vendor/continuation nibbles) |
//! | `0x80` (DDR5)                          | Rank config (hub model): per-rank device count (bits 3:0); bits 7:4 also carry the rank |
//! | `0x81` (DDR5)                          | SDRAM device width (bits 2:0: `0 = x4 / 1 = x8 / 2 = x16`) - the DDR5 width primary (the byte also opens the DDR2/DDR3 part region) |
//! | `0x81..0x91` (DDR2/DDR3)               | Module part number (16 ASCII chars)                |
//! | `0x140` / `0x141` (DDR4)               | Module manufacturer JEP106: 16-bit (bank, code), little-endian pair: `bank = (0x140 & 0x7F) + 1`, `code = 0x141` |
//! | `0x145..0x148` (DDR4)                  | Module serial number: 4 binary bytes, hex-rendered (`0ABCD123` style) |
//! | `0x149..0x15C` (DDR4)                  | Module part number (20 ASCII chars; falls back to `0x81..0x91` when present-but-blank) |
//! | `0x15E` / `0x15F` (DDR4)               | DRAM die manufacturer JEP106: the same (bank, code) encoding as the module maker |
//! | `0x200..0x220` (DDR5)                  | Module part number (32 ASCII chars)                |
//! | `0x91..0xA1` (DDR5 / legacy)           | Module serial number (16 ASCII chars)              |
//! | `0x180..0x1E6` (DDR4)                  | XMP 2.0: 9-byte header at `0x180` + two 47-byte profiles at `0x189` / `0x1B8` - see [Profiles](#profiles) |
//! | `0x300..0x400` (DDR5)                  | XMP 3.0 / EXPO region (256 B; four 32-byte blocks at `+16+32n`, JESD79-5) - coexists with the DDR5 part number at `0x200..0x220` (C7-03) |
//!
//! # JEP106 decoding
//!
//! DDR4 (JESD79-4) records the module / die manufacturer as a 16-bit
//! little-endian (bank, code) pair (`0x140`/`0x141` for the module,
//! `0x15E`/`0x15F` for the die): `bank = (low & 0x7F) + 1` (the top bit
//! of the low byte is reserved) and `code = high`. A non-zero code hits
//! the [`JEP106_BCD`] table for a name; an unknown non-zero code degrades
//! to `0x{code} (bank {bank})` (still a `Value`); a zero code means "no
//! manufacturer ID present" (`Na`). The DDR5 path keeps the historical
//! 8-bit vendor/continuation nibble model ([`JEP106`]).
//!
//! # Profiles
//!
//! - **XMP 2.0** (DDR4): a 9-byte header at `0x180` (gated on `0x180 ==
//!   0x0C`, `0x181 == 0x4A`, `0x183 == 0x20`; `0x182` is a profile-enable
//!   mask - check-only) followed by two 47-byte profiles at `0x189`
//!   (profile 1) and `0x1B8` (profile 2). A blank (all-zero) profile is
//!   skipped, and an all-`Na` decoded profile is dropped, but the module
//!   is still shown.
//! - **XMP 3.0 / EXPO** (DDR5): 256-byte region at `0x300` (JESD79-5;
//!   coexists with the DDR5 part number at `0x200`, C7-03), gated on its
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

/// Byte `0x00`: SPD bytes used; the legacy memory-type check reads bits
/// 4:0 (`0x0A` DDR4 / `0x0C` DDR5) - the documented fallback when byte
/// `0x02` carries no recognized type (C8-02).
const BYTE_MEMORY_TYPE: usize = 0x00;
/// Legacy DDR4 memory-type code (byte `0x00` bits 4:0, `1010b`).
const MEM_TYPE_DDR4: u8 = 0x0A;
/// Legacy DDR5 memory-type code (byte `0x00` bits 4:0, `1100b`).
const MEM_TYPE_DDR5: u8 = 0x0C;

/// Byte `0x02`: the basic-info memory type (JESD79-4/5) - the primary
/// classification (C8-02): `0x0C` = DDR4 (512 B image) / DDR5 (1024 B
/// image; the two generations share the code, disambiguated by the
/// image length), `0x0B` = DDR3 (legacy).
const BYTE_SPD_TYPE: usize = 0x02;
/// DDR4 / DDR5 memory-type key at byte `0x02` (C8-02).
const SPD_TYPE_DDR4: u8 = 0x0C;
/// DDR3 memory-type code at byte `0x02` (the legacy `0x81` part
/// location, C8-02).
const SPD_TYPE_DDR3: u8 = 0x0B;

// ---- DDR4 (JESD79-4, SPD5118) ---------------------------------------------

/// Byte `0x04`: DDR4 SDRAM density (bits 3:0 -> [`DENSITY4_TABLE`]).
const BYTE_DENSITY4: usize = 0x04;
/// Byte `0x0B`: DDR4 voltage (bits 3:0; `0x03` = 1.2 V); check-only.
#[allow(dead_code)]
const BYTE_VOLT4: usize = 0x0B;
/// Byte `0x0C`: DDR4 module organization (JESD79-4): bits 5:3 =
/// number of ranks - 1 (`0` = 1 ... `7` = 8), bits 2:0 = SDRAM device
/// width (`0 = x4 / 1 = x8 / 2 = x16 / 3 = x32`).
const BYTE_ORG4: usize = 0x0C;
/// Byte `0x0D`: DDR4 SDRAM bus width (bits 2:0; `3` = x64 desktop bus).
const BYTE_BUS4: usize = 0x0D;
/// Byte `0x11`: DDR4 timebase (MTB = 125 ps, FTB = 1 ps, fixed); the
/// byte is a reserved-code check only - it never gates the constants.
#[allow(dead_code)]
const BYTE_TIMEBASE: usize = 0x11;
/// Byte `0x12`: `tCKAVGmin` (MTB units).
const BYTE_TCKAVGMIN: usize = 0x12;
/// Byte `0x13`: `tCKAVGmax` (MTB units); check-only.
#[allow(dead_code)]
const BYTE_TCKAVGMAX: usize = 0x13;
/// Byte `0x7D`: signed fine-timebase (FTB) companion of `tCKAVGmin` (ps).
const BYTE_FTBL_TCKAVGMIN: usize = 0x7D;
/// Byte `0x7C`: signed fine-timebase (FTB) companion of `tCKAVGmax` (ps).
#[allow(dead_code)]
const BYTE_FTBL_TCKAVGMAX: usize = 0x7C;
/// Byte `0x140`: DDR4 module manufacturer JEP106 low byte (bank, LE).
const BYTE_MAKER4_LO: usize = 0x140;
/// Byte `0x141`: DDR4 module manufacturer JEP106 high byte (code).
const BYTE_MAKER4_HI: usize = 0x141;
/// `0x145..=0x148`: DDR4 module serial number (4 binary bytes).
const SERIAL4_START: usize = 0x145;
/// DDR4 serial-number field length in bytes (4).
const SERIAL4_LEN: usize = 4;
/// Byte `0x15E`: DDR4 DRAM die manufacturer JEP106 low byte (bank, LE).
const BYTE_DIE4_LO: usize = 0x15E;
/// Byte `0x15F`: DDR4 DRAM die manufacturer JEP106 high byte (code).
const BYTE_DIE4_HI: usize = 0x15F;
/// DDR4 XMP 2.0 region base (bytes `0x180..0x1E6`): the 9-byte header.
const XMP2_BASE: usize = 0x180;
/// DDR4 XMP 2.0 profile-1 base (47 bytes at `0x189..0x1B8`).
const XMP2_P1: usize = 0x189;
/// DDR4 XMP 2.0 profile-2 base (47 bytes at `0x1B8..0x1E7`).
const XMP2_P2: usize = 0x1B8;
/// DDR4 XMP 2.0 profile block size in bytes (47).
const XMP2_LEN: usize = 47;
/// XMP 2.0 header byte `0x180` value.
const XMP2_HEADER_0: u8 = 0x0C;
/// XMP 2.0 header byte `0x181` value.
const XMP2_HEADER_1: u8 = 0x4A;
/// XMP 2.0 header byte `0x183` (version) value.
const XMP2_VERSION: u8 = 0x20;

/// DDR4 density table: byte `0x04` bits 3:0 -> per-device density in Mbit.
const DENSITY4_TABLE: [u16; 10] = [
    256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 12288, 24576,
];

/// The standard JEDEC DDR4 data-rate bins in MT/s: the computed JEDEC
/// base speed (from `tCKAVGmin`) is floored to the largest bin that is
/// <= the computed value.
const BASE_SPEED_BINS: [u16; 14] = [
    1066, 1333, 1600, 1866, 2133, 2400, 2666, 2933, 3200, 3466, 3733, 4000,
    4266, 4400,
];

/// DDR4 (JESD79-4) module / die manufacturer names by 16-bit (bank, code)
/// pair, little-endian (`bank = (low & 0x7F) + 1`, `code = high`).
///
/// From coreboot `spdtool.py` + JEP106BE. Codes outside the table render
/// as `0x{code} (bank {bank})` (still a `Value`), never a panic.
pub const JEP106_BCD: &[(u8, u8, &str)] = &[
    (2, 0x98, "Kingston"),
    (5, 0xCD, "G.Skill"),
    (1, 0x2C, "Micron"),
    (1, 0xCE, "Samsung"),
    (1, 0xAD, "SK hynix"),
    (3, 0x9E, "Corsair"),
    (3, 0xFE, "Elpida"),
    (5, 0xB0, "OCZ"),
    (2, 0x4F, "Transcend"),
    (5, 0x43, "Ramaxel"),
    (3, 0xB5, "SuperTalent"),
];

// ---- DDR5 (JESD79-5, SPD5378) + legacy -------------------------------------

/// Byte `0x01`: module manufacturer JEP106 vendor nibble (DDR5).
const BYTE_MODULE_MAKER: usize = 0x01;
/// Byte `0x02`: module manufacturer JEP106 continuation nibble (DDR5;
/// the byte also carries the basic-info memory type, [`BYTE_SPD_TYPE`]).
const BYTE_MODULE_MAKER_CONT: usize = 0x02;

/// DDR5 byte `0x2E`: DRAM die manufacturer JEP106 vendor nibble.
const BYTE_DDR5_DIE_MAKER: usize = 0x2E;
/// DDR5 byte `0x2F`: DRAM die manufacturer JEP106 continuation nibble.
const BYTE_DDR5_DIE_MAKER_CONT: usize = 0x2F;

/// Byte `0x13`: DDR5 SDRAM density code (the DDR5 documented model).
const BYTE_DENSITY: usize = 0x13;
/// Byte `0x20`: minimum data rate, in 100 MT/s units (DDR5).
const BYTE_BASE_SPEED: usize = 0x20;
/// Byte `0x80`: rank config (hub model, DDR5): per-rank device count
/// (bits 3:0); bits 7:4 carry the rank.
const BYTE_RANK_CONFIG: usize = 0x80;
/// Byte `0x81`: SDRAM device width (bits 2:0, DDR5): the width primary
/// (the byte also opens the DDR2/DDR3 part-number region).
const BYTE_DEVICE_WIDTH: usize = 0x81;

/// `0x81..=0x90`: module part number (16 ASCII chars) - the DDR2/DDR3
/// location, and the DDR4 fallback when the DDR4 primary (`0x149`) is
/// present-but-blank.
const PART_START: usize = 0x81;
/// Part-number field length in bytes at `PART_START` (16).
const PART_LEN: usize = 16;
/// `0x149..=0x15C`: DDR4 module part number (20 ASCII chars, JESD79-4).
const DDR4_PART_START: usize = 0x149;
/// DDR4 part-number field length in bytes (20).
const DDR4_PART_LEN: usize = 20;
/// `0x200..=0x21F`: DDR5 module part number (32 ASCII chars, JESD79-5).
const DDR5_PART_START: usize = 0x200;
/// DDR5 part-number field length in bytes (32).
const DDR5_PART_LEN: usize = 32;
/// `0x91..=0xA0`: module serial number (16 ASCII chars; DDR5 / legacy).
const SERIAL_START: usize = 0x91;
/// Serial-number field length in bytes (16).
const SERIAL_LEN: usize = 16;

/// Full size of a DDR5 SPD image (8 Kbit EEPROM).
const SPD_IMAGE_LEN_DDR5: usize = 1024;

/// Recognized JEP106-0001 manufacturer codes (full 8-bit code -> name).
///
/// DDR5 path only (the 8-bit vendor/continuation nibble model): the named
/// `const` table consumed by [`decode_maker`] / [`decode_die_maker`];
/// codes outside the table degrade to their raw hex form. The DDR4 path
/// uses the 16-bit (bank, code) [`JEP106_BCD`] table instead.
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
    (0xC1, "G.Skill"),
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
/// P2-08 [`SpdImage`]; every other field is a [`Section`]. C6-02 adds
/// the separately carried die maker / die type / per-rank device count
/// (`die_maker` / `die_type` / `devices`); all degrade to `Na`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpdModule {
    /// Module index: the I2C address of the `ee1004` device
    /// ([`SpdImage::index`]).
    pub index: u8,
    /// `true` when the module classifies as DDR5 ([`classify_ddr5`]).
    pub is_ddr5: bool,
    /// Module manufacturer (JEP106: DDR4 (bank, code) `0x140`/`0x141`,
    /// DDR5 vendor/continuation nibbles `0x01`/`0x02`), falling back to
    /// the DRAM die manufacturer when no module ID is present.
    pub maker: Section<String>,
    /// DRAM die manufacturer (JEP106 from the die-ID bytes: DDR5
    /// `0x2E`/`0x2F` nibbles, DDR4 (bank, code) `0x15E`/`0x15F`) - the
    /// die ID [`decode_maker`] reads as its fallback, now carried
    /// separately (C6-02); `Na` when no die ID is present.
    pub die_maker: Section<String>,
    /// Human die-type label; a die variant (e.g. "A-Die") is not a
    /// standard SPD field, so this defaults to `Na(NotApplicable)` - a
    /// documented `die_maker` + density -> label mapping may fill it in
    /// later (C6-02).
    pub die_type: Section<String>,
    /// Total DRAM devices (rank x per-rank, JESD79-4/5, C8-03): DDR4 =
    /// rank x (bus / width) from `0x0C`/`0x0D` (no hub arithmetic); DDR5
    /// = rank x per-rank from the `0x80` hub nibble (the non-compliant
    /// vendor hub is worked around, never trusted, D-2). The per-DIMM
    /// capacity is `density_mbit x devices / 8192` GiB (the facade's
    /// D-C3 arithmetic consumes the total). `Na` when the module
    /// organization is absent or invalid (the same gating as [`rank`]).
    pub devices: Section<u8>,
    /// Module part number, generation-scoped (C7-02): DDR2/DDR3
    /// `0x81..0x91` (16 ASCII chars); DDR4 `0x149..0x15C` (20 chars,
    /// falling back to `0x81..0x91` when present-but-blank); DDR5
    /// `0x200..0x220` (32 chars).
    pub part: Section<String>,
    /// Module serial number (DDR4: 4 binary bytes at `0x145..0x148`,
    /// hex-rendered; DDR5 / legacy: bytes `0x91..0xA1`, 16 ASCII chars).
    pub serial: Section<String>,
    /// Number of ranks (C8-03, generation-scoped): DDR4 byte `0x0C`
    /// bits 5:3 + 1 (the JESD79-4 number-of-ranks code `0` = 1 ... `7` =
    /// 8; a blank / absent `0x0C` -> `Na`), DDR5 byte `0x80` bits 7:4
    /// (the documented hub model).
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
        die_maker: decode_die_maker(data, is_ddr5),
        // A die variant is not a standard SPD field: the label stays
        // Na until a documented die_maker + density mapping exists
        // (C6-02).
        die_type: Section::na(NaReason::NotApplicable),
        devices: decode_devices(data, is_ddr5),
        part: decode_part(data, is_ddr5),
        serial: decode_serial(data, is_ddr5),
        rank: decode_rank(data, is_ddr5),
        density_mbit: decode_density(data, is_ddr5),
        speed_mts: decode_base_speed(data, is_ddr5),
        profiles: decode_profiles(data, is_ddr5),
    }
}

// ---------------------------------------------------------------------------
// Bounds-checked reads + shared helpers.
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

/// Classify the module: the primary check is the basic-info memory type
/// (byte `0x02`, C8-02) - `0x0C` -> DDR5 iff the image is 1024 B (the
/// DDR4 / DDR5 generations share the code, disambiguated by the image
/// length), `0x0B` -> DDR3. An unrecognized / absent `0x02` falls back
/// to the legacy byte-`0x00` check (bits 4:0: `0x0C` -> DDR5, `0x0A`
/// -> DDR4), then to the image length (1024 B -> DDR5, otherwise
/// DDR4) - the current last resort.
fn classify_ddr5(data: &[u8]) -> bool {
    match get(data, BYTE_SPD_TYPE) {
        Some(SPD_TYPE_DDR4) => data.len() == SPD_IMAGE_LEN_DDR5,
        Some(SPD_TYPE_DDR3) => false,
        _ => match get(data, BYTE_MEMORY_TYPE).map(|b| b & 0x1F) {
            Some(MEM_TYPE_DDR5) => true,
            Some(MEM_TYPE_DDR4) => false,
            _ => data.len() == SPD_IMAGE_LEN_DDR5,
        },
    }
}

/// Assemble the full 8-bit JEP106 code from its vendor + continuation
/// nibbles (the DDR5 model): vendor `1111b` -> the full byte is the
/// code; vendor `0000b` -> "no manufacturer ID present" (code 0);
/// otherwise `(continuation << 4) | vendor`.
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

/// The name for a JEP106 8-bit code, or `None` when the code is not in
/// the [`JEP106`] table.
fn jep106_name(code: u8) -> Option<&'static str> {
    JEP106.iter().find(|(c, _)| *c == code).map(|(_, name)| *name)
}

/// Render a non-zero JEP106 8-bit code: a name from the [`JEP106`]
/// table, or its raw hex form when the code is unknown.
fn maker_string(code: u8) -> Section<String> {
    match jep106_name(code) {
        Some(name) => Section::Value(name.to_owned()),
        None => Section::Value(format!("0x{code:02X}")),
    }
}

/// Decode a JEP106 (bank, code) manufacturer ID from two SPD bytes
/// (the DDR4 16-bit little-endian model): `bank = (low & 0x7F) + 1`
/// (the top bit of the low byte is reserved - the bank is computed,
/// never trusted), `code = high`. Returns the name from
/// [`JEP106_BCD`], or `None` when the pair is not in the table.
fn jep106_name_bank_code(low: u8, high: u8) -> Option<&'static str> {
    let bank = (low & 0x7F) as u32 + 1;
    let code = high;
    JEP106_BCD
        .iter()
        .find(|(b, c, _)| u32::from(*b) == bank && *c == code)
        .map(|(_, _, name)| *name)
}

/// Build the maker string from a DDR4 (bank, code) pair at `off_low` /
/// `off_high`. A present pair (non-zero code) is a name from
/// [`JEP106_BCD`] or its raw `0x{code} (bank {bank})` form; an absent
/// pair (both bytes zero) -> `Na(NotApplicable)`; an out-of-range byte
/// -> `Na(ParseError)` naming the byte. Never panics.
fn maker_from_bank_code(data: &[u8], off_low: usize, off_high: usize) -> Section<String> {
    // A completely empty image carries no manufacturer data at all ->
    // "absent" (NotApplicable), distinct from a truncated-but-present
    // image (ParseError naming the past-end byte).
    if data.is_empty() {
        return Section::na(NaReason::NotApplicable);
    }
    let Some(low) = get(data, off_low) else {
        return Section::na(oob(off_low));
    };
    let Some(high) = get(data, off_high) else {
        return Section::na(oob(off_high));
    };
    if low == 0 && high == 0 {
        return Section::na(NaReason::NotApplicable);
    }
    match jep106_name_bank_code(low, high) {
        Some(name) => Section::Value(name.to_owned()),
        None => Section::Value(format!(
            "0x{high:02X} (bank {})",
            (low & 0x7F) + 1
        )),
    }
}

/// Decode a 16-char ASCII module field (part number / DDR5 serial
/// number): read up to `len` bytes, stopping at the first null / space
/// padding byte or non-ASCII byte (fields are space-padded ASCII per
/// JESD79-4/5). Blank -> `Na(NotApplicable)`; an out-of-range byte ->
/// `Na(ParseError)` (the field is incomplete).
fn decode_ascii(data: &[u8], start: usize, len: usize, what: &str) -> Section<String> {
    // Read up to `len` bytes, stopping at the first NUL terminator (an
    // out-of-range byte -> `Na(ParseError)`). The field is space-padded
    // ASCII: trailing padding is trimmed after the fact while internal
    // spaces are preserved (part numbers may contain them).
    let mut s = String::new();
    for i in 0..len {
        match get(data, start + i) {
            None => {
                return Section::na(NaReason::ParseError(format!(
                    "{what}: byte 0x{:02X} outside image bounds",
                    start + i
                )))
            }
            Some(0x00) => break,
            Some(b) => s.push(b as char),
        }
    }
    let trimmed = s.trim_end().to_string();
    if trimmed.is_empty() {
        Section::na(NaReason::NotApplicable)
    } else {
        Section::Value(trimmed)
    }
}

/// The legacy memory-type code (byte `0x00`, bits 4:0), or `None` when
/// the byte is outside the image bounds (the C8-02 fallback source -
/// the primary type lives at byte `0x02`).
fn memory_type(data: &[u8]) -> Option<u8> {
    get(data, BYTE_MEMORY_TYPE).map(|b| b & 0x1F)
}

/// `true` when the basic-info memory type (byte `0x02`, C8-02) is the
/// DDR4 / DDR5 key `0x0C` on a non-DDR5 image; the legacy byte-`0x00`
/// code `0x0A` applies when `0x02` is unrecognized. `false` for DDR3
/// (`0x0B`), other unrecognized / out-of-bounds headers (those
/// classify to the legacy `0x81` part location on the not-DDR5 path).
fn is_ddr4(data: &[u8]) -> bool {
    match get(data, BYTE_SPD_TYPE) {
        Some(SPD_TYPE_DDR4) => data.len() != SPD_IMAGE_LEN_DDR5,
        _ => memory_type(data) == Some(MEM_TYPE_DDR4),
    }
}

/// Decode the module part number, generation-scoped (C7-02; the
/// generation comes from the C8-02 byte-`0x02` classification):
/// - DDR5 (JESD79-5): the 32-char field at `0x200..0x220`;
/// - DDR4 (JESD79-4): the 20-char field at `0x149..0x15C`, falling back
///   to the 16-char field at `0x81..0x91` when the `0x149` region is
///   present-but-blank; a truncated image (the `0x149` region out of
///   bounds) keeps its own `Na(ParseError)` rather than the fallback's;
/// - DDR2/DDR3 (the `0x0B` key and everything else not called
///   DDR4 / DDR5): the 16-char field at `0x81..0x91`, unchanged.
///
/// Reuses [`decode_ascii`]; never panics.
fn decode_part(data: &[u8], is_ddr5: bool) -> Section<String> {
    if is_ddr5 {
        decode_ascii(data, DDR5_PART_START, DDR5_PART_LEN, "part number")
    } else if is_ddr4(data) {
        let primary = decode_ascii(data, DDR4_PART_START, DDR4_PART_LEN, "part number");
        if matches!(&primary, Section::Na(NaReason::NotApplicable)) {
            // The 0x149 region is present but blank: the part is recorded
            // at the legacy 0x81 location instead.
            decode_ascii(data, PART_START, PART_LEN, "part number")
        } else {
            primary
        }
    } else {
        decode_ascii(data, PART_START, PART_LEN, "part number")
    }
}

/// Decode the module serial number, generation-scoped:
/// - DDR4 (JESD79-4): the 4 binary bytes at `0x145..0x148`,
///   hex-rendered (8 chars, no separators, `0ABCD123` style); an
///   all-zero field -> `Na(NotApplicable)`; an out-of-range byte ->
///   `Na(ParseError)` naming the byte;
/// - DDR5 / legacy: the 16-char ASCII field at `0x91..0xA1`
///   ([`decode_ascii`]), unchanged.
///
/// Never panics.
fn decode_serial(data: &[u8], is_ddr5: bool) -> Section<String> {
    if is_ddr5 {
        return decode_ascii(data, SERIAL_START, SERIAL_LEN, "serial number");
    }
    let mut bytes = [0u8; SERIAL4_LEN];
    for (i, slot) in bytes.iter_mut().enumerate() {
        match get(data, SERIAL4_START + i) {
            Some(b) => *slot = b,
            None => return Section::na(oob(SERIAL4_START + i)),
        }
    }
    if bytes == [0u8; SERIAL4_LEN] {
        return Section::na(NaReason::NotApplicable);
    }
    Section::Value(format!(
        "{:02X}{:02X}{:02X}{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    ))
}

/// Decode the module manufacturer, generation-scoped:
/// - DDR4: the 16-bit (bank, code) pair at `0x140`/`0x141`
///   ([`maker_from_bank_code`]); an absent pair falls back to the DRAM
///   die manufacturer ([`decode_die_maker`]);
/// - DDR5: JEP106 vendor + continuation nibbles at `0x01`/`0x02`; when
///   no module ID is present, falls back to the DRAM die manufacturer.
/// All-absent -> `Na(NotApplicable)`.
fn decode_maker(data: &[u8], is_ddr5: bool) -> Section<String> {
    if is_ddr5 {
        let vendor = get(data, BYTE_MODULE_MAKER).unwrap_or(0);
        let cont = get(data, BYTE_MODULE_MAKER_CONT).unwrap_or(0);
        let code = jep106_code(vendor, cont);
        if code != 0x00 {
            return maker_string(code);
        }
        // No module ID: the module "maker" degrades to the DRAM die
        // manufacturer - the same source now carried separately in
        // `SpdModule.die_maker` (C6-02).
        return decode_die_maker(data, is_ddr5);
    }
    // DDR4: the (bank, code) pair; absent -> the die-maker fallback.
    let maker = maker_from_bank_code(data, BYTE_MAKER4_LO, BYTE_MAKER4_HI);
    if matches!(maker, Section::Value(_)) {
        return maker;
    }
    decode_die_maker(data, is_ddr5)
}

/// Decode the DRAM die manufacturer, generation-scoped:
/// - DDR4: the 16-bit (bank, code) pair at `0x15E`/`0x15F`
///   ([`maker_from_bank_code`]); an absent pair (both zero) ->
///   `Na(NotApplicable)`;
/// - DDR5: JEP106 vendor + continuation nibbles at `0x2E`/`0x2F` - the
///   source [`decode_maker`] reads as its fallback, now carried
///   separately in [`SpdModule.die_maker`] (C6-02).
/// A present-but-unknown code renders as its raw form (still a `Value`),
/// never a panic.
fn decode_die_maker(data: &[u8], is_ddr5: bool) -> Section<String> {
    if is_ddr5 {
        let (dv, dc) = (
            get(data, BYTE_DDR5_DIE_MAKER).unwrap_or(0),
            get(data, BYTE_DDR5_DIE_MAKER_CONT).unwrap_or(0),
        );
        let dcode = jep106_code(dv, dc);
        if dcode == 0x00 {
            Section::na(NaReason::NotApplicable)
        } else {
            maker_string(dcode)
        }
    } else {
        maker_from_bank_code(data, BYTE_DIE4_LO, BYTE_DIE4_HI)
    }
}

/// The SDRAM device width in bits from a width-code byte (the DDR5 hub
/// model, C8-03): `0 = x4 / 1 = x8 / 2 = x16` (the JESD79-4/5
/// device-width code, read at bits 2:0); any other code is
/// unrecognized in this documented model -> `None`.
fn width_from_code(code: u8) -> Option<u8> {
    match code & 0x07 {
        0 => Some(4),
        1 => Some(8),
        2 => Some(16),
        _ => None,
    }
}

/// The module's SDRAM device width in bits (C8-03, D-2),
/// generation-scoped:
/// - DDR4: byte `0x0C` bits 2:0 - `0 = x4 / 1 = x8 / 2 = x16 / 3 = x32`
///   (the full JESD79-4 code set; **no `0x81` fallback**); an
///   unrecognized code (4-7) or an absent `0x0C` -> `NaReason` naming
///   `0x0C`;
/// - DDR5: byte `0x81` bits 2:0 (the documented hub model; codes 0-2).
/// Missing / unrecognized width -> `NaReason`; never a panic.
fn device_width(data: &[u8], is_ddr5: bool) -> Result<u8, NaReason> {
    if is_ddr5 {
        let w = get(data, BYTE_DEVICE_WIDTH).ok_or_else(|| oob(BYTE_DEVICE_WIDTH))?;
        return width_from_code(w).ok_or_else(|| {
            NaReason::ParseError(format!(
                "device width byte 0x81 = 0x{w:02X} (code {}) is not recognized",
                w & 0x07
            ))
        });
    }
    let org = get(data, BYTE_ORG4).ok_or_else(|| oob(BYTE_ORG4))?;
    match org & 0x07 {
        0 => Ok(4),
        1 => Ok(8),
        2 => Ok(16),
        3 => Ok(32),
        c => Err(NaReason::ParseError(format!(
            "device width byte 0x0C bits 2:0 = {c} is not recognized"
        ))),
    }
}

/// The module's rank count (C8-03, D-2), generation-scoped:
/// - DDR4 = byte `0x0C` bits 5:3 + 1 (the JESD79-4 number-of-ranks code
///   `0` = 1 ... `7` = 8); a blank (`0x00`) / absent `0x0C` ->
///   `NaReason` naming `0x0C` (the byte `0x80` hub nibble is the
///   DDR5-only fallback now);
/// - DDR5 = byte `0x80` bits 7:4 (the documented hub model).
/// Never a panic.
fn rank_count(data: &[u8], is_ddr5: bool) -> Result<u8, NaReason> {
    if is_ddr5 {
        let hub = get(data, BYTE_RANK_CONFIG).ok_or_else(|| oob(BYTE_RANK_CONFIG))?;
        return rank_from_nibble(hub, BYTE_RANK_CONFIG);
    }
    let org = get(data, BYTE_ORG4).ok_or_else(|| oob(BYTE_ORG4))?;
    if org == 0x00 {
        return Err(NaReason::ParseError(
            "rank byte 0x0C is blank (no module organization)".to_owned(),
        ));
    }
    Ok(((org >> 3) & 0x07) + 1)
}

/// The rank from a hub nibble (byte `0x80` bits 7:4, DDR5): zero ->
/// `NaReason` naming the byte.
fn rank_from_nibble(hub: u8, byte: usize) -> Result<u8, NaReason> {
    let rank = hub >> 4;
    if rank == 0 {
        return Err(NaReason::ParseError(format!(
            "rank byte 0x{byte:02X} = 0x{hub:02X} carries rank 0 (invalid)"
        )));
    }
    Ok(rank)
}

/// The per-rank device count (C8-03, D-2, DDR5 hub model): the byte
/// `0x80` bits 3:0 hub nibble, trusted only when it is consistent with
/// the bus (`per_rank x width == bus`); otherwise it is derived as
/// `bus / width` (the non-compliant vendor hub - e.g. the live `0x80 =
/// 0x11` per-rank 1 on the 64-bit bus with x8 devices - is worked
/// around, never trusted). Per-rank `0` or a non-dividing bus ->
/// `NaReason` naming the byte; never a panic.
fn per_rank_count(data: &[u8], width: u8, bus: u8) -> Result<u8, NaReason> {
    let hub = get(data, BYTE_RANK_CONFIG).ok_or_else(|| oob(BYTE_RANK_CONFIG))?;
    let per_rank = hub & 0x0F;
    if per_rank == 0 {
        return Err(NaReason::ParseError(format!(
            "rank config byte 0x80 = 0x{hub:02X} carries per-rank 0 (invalid)"
        )));
    }
    if (per_rank as u16) * (width as u16) == bus as u16 {
        return Ok(per_rank);
    }
    if bus % width != 0 {
        return Err(NaReason::ParseError(format!(
            "bus width {bus} bits is not divisible by the {width}-bit device width (bytes 0x0C/0x81)"
        )));
    }
    Ok(bus / width)
}

/// The bus width in bits, generation-scoped:
/// - DDR4: byte `0x0D` bits 2:0 - `3` = the 64-bit desktop bus; any
///   other code -> `NaReason` naming `0x0D`;
/// - DDR5: the 64-bit desktop bus, extended to 128 when the byte `0x0D`
///   bit 4 is set (the documented hub model; a missing `0x0D` keeps the
///   base 64).
fn bus_width(data: &[u8], is_ddr5: bool) -> Result<u8, NaReason> {
    if is_ddr5 {
        return Ok(match get(data, BYTE_BUS4) {
            Some(b) if b & 0x10 != 0 => 128,
            _ => 64,
        });
    }
    let b = get(data, BYTE_BUS4).ok_or_else(|| oob(BYTE_BUS4))?;
    match b & 0x07 {
        3 => Ok(64),
        c => Err(NaReason::ParseError(format!(
            "bus width byte 0x0D bits 2:0 = {c} is not the x64 desktop bus"
        ))),
    }
}

/// The total device count (C8-03, D-2) = rank x per-rank, where the
/// rank is generation-scoped ([`rank_count`]) and the per-rank count is:
/// - DDR4: `bus / width` (the 64-bit bus from `0x0D` divided by the
///   `0x0C` width - no hub arithmetic);
/// - DDR5: the byte `0x80` hub nibble, derived when it fails the
///   consistency rule ([`per_rank_count`]).
/// Any organization failure -> `NaReason` naming the byte; never a
/// panic.
fn total_devices(data: &[u8], is_ddr5: bool) -> Result<u8, NaReason> {
    let rank = rank_count(data, is_ddr5)?;
    let width = device_width(data, is_ddr5)?;
    let bus = bus_width(data, is_ddr5)?;
    let per_rank = if is_ddr5 {
        per_rank_count(data, width, bus)?
    } else {
        if bus % width != 0 {
            return Err(NaReason::ParseError(format!(
                "bus width {bus} bits is not divisible by the {width}-bit device width (bytes 0x0C/0x0D)"
            )));
        }
        bus / width
    };
    rank.checked_mul(per_rank).ok_or_else(|| {
        NaReason::ParseError(format!(
            "device total (rank {rank} x per-rank {per_rank}) exceeds the u8 cell"
        ))
    })
}

/// Decode the rank count (C8-03, D-2): the generation-scoped
/// [`rank_count`]; every failure degrades to `Na(ParseError)` naming
/// the offending byte (the same gating as [`decode_devices`]).
fn decode_rank(data: &[u8], is_ddr5: bool) -> Section<u8> {
    match rank_count(data, is_ddr5) {
        Ok(rank) => Section::Value(rank),
        Err(reason) => Section::na(reason),
    }
}

/// Decode the total DRAM device count (C8-03, D-2): rank x per-rank
/// (DDR4: `bus / width` from `0x0C`/`0x0D`; DDR5: the `0x80` hub
/// model, derived when inconsistent). Any failure degrades to
/// `Na(ParseError)` naming the offending byte (the same gating as
/// [`decode_rank`]).
fn decode_devices(data: &[u8], is_ddr5: bool) -> Section<u8> {
    match total_devices(data, is_ddr5) {
        Ok(total) => Section::Value(total),
        Err(reason) => Section::na(reason),
    }
}

/// Decode the SDRAM density into Mbit, generation-scoped:
/// - DDR4 (JESD79-4): byte `0x04` bits 3:0 -> the 10-entry
///   [`DENSITY4_TABLE`] (`0` = 256 ... `6` = 16384, `8` = 12288, `9` =
///   24576 Mbit); codes `0xA..=0xF` -> `Na(ParseError)` naming the code;
/// - DDR5: documented model at byte `0x13`, code `0x11..=0x18` -> 1..64
///   Gb (a result >= 64 Gb = 65536 Mbit overflows the u16 cell ->
///   `Na(ParseError)`).
/// An absent byte -> `Na(ParseError)`; never a panic.
fn decode_density(data: &[u8], is_ddr5: bool) -> Section<u16> {
    if is_ddr5 {
        let Some(code) = get(data, BYTE_DENSITY) else {
            return Section::na(oob(BYTE_DENSITY));
        };
        let gb: u32 = match code {
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
        };
        let mbit = gb * 1024;
        return match u16::try_from(mbit) {
            Ok(v) => Section::Value(v),
            Err(_) => Section::na(NaReason::ParseError(format!(
                "{gb} Gb = {mbit} Mbit exceeds the u16 density cell"
            ))),
        };
    }
    let Some(b) = get(data, BYTE_DENSITY4) else {
        return Section::na(oob(BYTE_DENSITY4));
    };
    let code = (b & 0x0F) as usize;
    match DENSITY4_TABLE.get(code) {
        Some(&mbit) => Section::Value(mbit),
        None => Section::na(NaReason::ParseError(format!(
            "DDR4 density code 0x{code:02X} (byte 0x04 bits 3:0) not recognized"
        ))),
    }
}

/// The JEDEC base speed from `tCKAVGmin`: `tCKAVGmin_ps = 0x12 x 125 +
/// i8(0x7D)`; a sentinel `0x7D` (`0x6E` / `0xF8`) -> `Na`; a zero /
/// non-positive result -> `Na`; otherwise `2000 / tCK_ns` MT/s floored
/// to the largest standard bin <= the computed value
/// ([`BASE_SPEED_BINS`]).
fn tckavgmin_base_speed(data: &[u8]) -> Section<u16> {
    let Some(mtb) = get(data, BYTE_TCKAVGMIN) else {
        return Section::na(oob(BYTE_TCKAVGMIN));
    };
    let Some(ftb) = get(data, BYTE_FTBL_TCKAVGMIN) else {
        return Section::na(oob(BYTE_FTBL_TCKAVGMIN));
    };
    if ftb == 0x6E || ftb == 0xF8 {
        return Section::na(NaReason::ParseError(format!(
            "tCKAVGmin FTB byte 0x7D = 0x{ftb:02X} is a sentinel (no base speed recorded)"
        )));
    }
    let ps = (mtb as i32) * 125 + (ftb as i8) as i32;
    if mtb == 0 || ps <= 0 {
        return Section::na(NaReason::ParseError(format!(
            "tCKAVGmin = 0x12 ({mtb}) + 0x7D ({} ps) is not a positive timing",
            ftb as i8
        )));
    }
    let speed = 2000.0 / (ps as f64 / 1000.0);
    floor_to_bin(speed)
}

/// Floor a computed MT/s rate to the largest standard bin <= the value
/// ([`BASE_SPEED_BINS`]); a value below the lowest bin -> `Na` naming
/// the computed rate.
fn floor_to_bin(speed: f64) -> Section<u16> {
    match BASE_SPEED_BINS
        .iter()
        .filter(|&&b| f64::from(b) <= speed)
        .max()
    {
        Some(&bin) => Section::Value(bin),
        None => Section::na(NaReason::ParseError(format!(
            "computed base speed {speed:.0} MT/s is below the lowest standard bin (1066)"
        ))),
    }
}

/// Decode the minimum guaranteed data rate in MT/s, generation-scoped:
/// - DDR4: the XMP 2.0 profile-1 `tCK` data rate (`16000 / tCK`) when
///   the module carries a valid XMP header + a non-blank profile 1 with
///   a non-zero `tCK`; otherwise the JEDEC base speed from `tCKAVGmin`
///   ([`tckavgmin_base_speed`]);
/// - DDR5: byte `0x20` x 100 MT/s (the documented model), with the
///   out-of-band guard (a computed value above the DDR5 max of 8800
///   MT/s degrades to `Na`).
fn decode_base_speed(data: &[u8], is_ddr5: bool) -> Section<u16> {
    if is_ddr5 {
        let Some(code) = get(data, BYTE_BASE_SPEED) else {
            return Section::na(oob(BYTE_BASE_SPEED));
        };
        if code == 0 {
            return Section::na(NaReason::ParseError(
                "base speed byte 0x20 is 0 (no minimum data rate recorded)".to_owned(),
            ));
        }
        let speed = u16::from(code) * 100;
        if speed > 8800 {
            return Section::na(NaReason::ParseError(format!(
                "base speed {speed} MT/s implausible (byte 0x20 = {code})"
            )));
        }
        return Section::Value(speed);
    }
    // DDR4: XMP profile-1 primary, JEDEC base fallback.
    if let Some(speed) = xmp1_speed(data) {
        return speed;
    }
    tckavgmin_base_speed(data)
}

// ---------------------------------------------------------------------------
// XMP 2.0 (DDR4) + XMP 3.0 / EXPO (DDR5) profile decode.
// ---------------------------------------------------------------------------

/// DDR5 XMP 3.0 / EXPO region base (bytes `0x300..0x400`, JESD79-5):
/// the region's spec home, coexisting with the DDR5 part number at
/// `0x200..0x220`, which the former `0x200` base collided with (C7-03
/// moved the base here).
const XMP3_BASE: usize = 0x300;
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

/// The DDR4 XMP 2.0 header gate: `0x180 == 0x0C` && `0x181 == 0x4A` &&
/// `0x183 == 0x20` (the version). `0x182` is a profile-enable mask -
/// check-only, never gated on (some vendors leave it `0x00` on
/// populated slots). An out-of-range byte fails the gate (no profiles),
/// never panics.
fn xmp2_header_valid(data: &[u8]) -> bool {
    get(data, XMP2_BASE) == Some(XMP2_HEADER_0)
        && get(data, XMP2_BASE + 1) == Some(XMP2_HEADER_1)
        && get(data, XMP2_BASE + 3) == Some(XMP2_VERSION)
}

/// The XMP 2.0 profile-1 data rate in MT/s (`16000 / tCK`, from the
/// profile-1 `tCK` byte) when the module carries a valid XMP header and
/// a non-blank profile 1 with a non-zero `tCK`; `None` otherwise. This
/// is the DDR4 base-speed primary source.
fn xmp1_speed(data: &[u8]) -> Option<Section<u16>> {
    if !xmp2_header_valid(data) {
        return None;
    }
    let block = read_block(data, XMP2_P1, XMP2_LEN)?;
    if block.iter().all(|&b| b == 0) {
        return None; // blank profile 1
    }
    let tck = block[3];
    if tck == 0 {
        return None;
    }
    Some(Section::Value(16000 / u16::from(tck)))
}

/// Decode the factory-rated profiles of one SPD image: XMP 2.0 (the
/// 47-byte model) for DDR4, the XMP 3.0 / EXPO region for DDR5. Never
/// panics; a truncated image or a missing / invalid region yields an
/// empty profile list.
fn decode_profiles(data: &[u8], is_ddr5: bool) -> Vec<SpdProfile> {
    if is_ddr5 {
        return decode_xmp3(data);
    }
    let mut out = Vec::new();
    // XMP 2.0: a 9-byte header gate at 0x180, then two 47-byte profiles
    // (profile 1 at 0x189, profile 2 at 0x1B8); an invalid header yields
    // an empty list, and each blank / all-Na profile is skipped
    // independently.
    if xmp2_header_valid(data) {
        if let Some(p) = decode_xmp2_profile(data, 1, XMP2_P1) {
            out.push(p);
        }
        if let Some(p) = decode_xmp2_profile(data, 2, XMP2_P2) {
            out.push(p);
        }
    }
    out
}

/// Decode one DDR4 XMP 2.0 47-byte profile at `base`.
///
/// Returns `None` when the block is blank (all zero) or when every
/// decoded field is `Na` (nothing usable) - the caller then skips it,
/// but the module is still shown. Field layout (offsets relative to
/// `base`):
///
/// | +off | Field | Decode |
/// |------|-------|--------|
/// | `0x00` | VDD | bit 7 = +1 V, bits 6:0 = 1/100 V -> `mV = (b & 0x80 != 0) x 1000 + (b & 0x7F) x 10` (`0xA3` -> 1350 mV) |
/// | `0x03` | tCK | MTB units -> `speed_mts = 16000 / tCK` (`tCK = 5` -> 3200 MT/s); `0` -> `Na` |
/// | `0x04..0x06` | CL supported bitmap | 24-bit LE, bit N = CL 7+N -> `cas = first set bit + 7` |
/// | `0x09` / `0x24` | tRCD | `0x24 x 125 + i8(0x09)` ps -> ticks |
/// | `0x0A` / `0x23` | tRP | `0x23 x 125 + i8(0x0A)` ps -> ticks |
/// | `0x0B` / `0x0C` | tRAS | `(0x0B << 8 \| 0x0C)` ps -> ticks |
///
/// ps -> ticks: `round(ps / (tCK x 125))` (the frozen `trcd`/`trp`/
/// `tras` cells are tick counts); `tCK = 0` -> the timing cells are
/// `Na`.
fn decode_xmp2_profile(data: &[u8], index: u8, base: usize) -> Option<SpdProfile> {
    let block = read_block(data, base, XMP2_LEN)?;
    if block.iter().all(|&b| b == 0) {
        return None; // blank profile
    }
    // Voltage: bit 7 = 10s-of-volt, bits 6:0 = 1/100 V.
    let volt_byte = block[0];
    let voltage = (volt_byte & 0x80 != 0) as u16 * 1000 + (volt_byte & 0x7F) as u16 * 10;
    // Speed: tCK in MTB units.
    let tck = block[3];
    let speed = if tck > 0 { 16000 / u16::from(tck) } else { 0 };
    // CL: 24-bit LE bitmap, bit N = CL 7+N; the lowest set bit wins.
    let cl_bitmap = (block[4] as u32) | ((block[5] as u32) << 8) | ((block[6] as u32) << 16);
    let cas = (0..24)
        .find(|&n| cl_bitmap & (1 << n) != 0)
        .map(|n| (7 + n) as u8);
    // Timings: ps values -> ticks (round(ps / (tCK x 125))).
    let tck_ps = tck as u32 * 125;
    let to_ticks = |ps: i32| {
        if tck_ps > 0 && ps > 0 {
            (ps as f64 / f64::from(tck_ps)).round() as u8
        } else {
            0
        }
    };
    let trcd = to_ticks(block[0x24] as i32 * 125 + (block[0x09] as i8) as i32);
    let trp = to_ticks(block[0x23] as i32 * 125 + (block[0x0A] as i8) as i32);
    let tras = to_ticks(((block[0x0B] as i32) << 8) | (block[0x0C] as i32));
    let p = SpdProfile {
        index,
        speed_mts: if speed > 0 {
            Section::Value(speed)
        } else {
            Section::na(NaReason::ParseError(
                "XMP 2.0 tCK is 0 (no profile data rate)".to_owned(),
            ))
        },
        cas: cas
            .map(Section::Value)
            .unwrap_or(Section::na(NaReason::NotApplicable)),
        trcd: if trcd > 0 {
            Section::Value(trcd)
        } else {
            Section::na(NaReason::NotApplicable)
        },
        trp: if trp > 0 {
            Section::Value(trp)
        } else {
            Section::na(NaReason::NotApplicable)
        },
        tras: if tras > 0 {
            Section::Value(tras)
        } else {
            Section::na(NaReason::NotApplicable)
        },
        voltage: if voltage > 0 {
            Section::Value(voltage)
        } else {
            Section::na(NaReason::NotApplicable)
        },
    };
    // A profile whose fields are all Na carries no usable data -> skip it.
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

/// Decode the DDR5 XMP 3.0 / EXPO region (256 bytes at `0x300`,
/// JESD79-5 - the DDR5 part number's `0x200` home is untouched; C7-03
/// moved the base out of the part-number region).
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

/// Build an [`SpdProfile`] from a 32-byte XMP 3.0 / EXPO structure via
/// `build`. Returns `None` when every field of the built profile is `Na`
/// (nothing usable was decoded) - the caller then skips the slot.
fn xmp_profile<F>(block: &[u8], build: F) -> Option<SpdProfile>
where
    F: FnOnce(&[u8; XMP3_BLOCK]) -> SpdProfile,
{
    let mut arr = [0u8; XMP3_BLOCK];
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
// Tests (plan P2-09 quality gates + brief items a-e + the SPD re-baseline).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Real captured images (re-anchored from Work/SPD_Images/).
    // ------------------------------------------------------------------

    /// Image A: the real captured 512-byte G.Skill `F4-3600C18-32GVK`
    /// SPD5118 image (5950X host, slot `0x52`; the raw capture under
    /// `Work/SPD_Images/amd_5950x_0x52.bin`).
    const IMAGE_A_GSKILL: &[u8] = &[
        0x23, 0x11, 0x0C, 0x02, 0x86, 0x29, 0x00, 0x08, 0x00, 0x60, 0x00, 0x03,
        0x09, 0x03, 0x00, 0x00, 0x00, 0x00, 0x06, 0x0D, 0xF8, 0x3F, 0x00, 0x00,
        0x6E, 0x6E, 0x6E, 0x11, 0x00, 0x6E, 0xF0, 0x0A, 0x20, 0x08, 0x00, 0x05,
        0x00, 0xA8, 0x18, 0x28, 0x28, 0x00, 0x78, 0x00, 0x14, 0x3C, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x15, 0x2B, 0x16, 0x36, 0x0B, 0x2B, 0x0C, 0x36, 0x00, 0x00, 0x36, 0x15,
        0x2B, 0x0C, 0x2C, 0x16, 0x2B, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9C, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xE7, 0x00, 0xF2, 0x20, 0x11, 0x11, 0x41, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xAB, 0xC6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xCD, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x46, 0x34, 0x2D, 0x33, 0x36, 0x30, 0x30,
        0x43, 0x31, 0x38, 0x2D, 0x33, 0x32, 0x47, 0x56, 0x4B, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x80, 0xAD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0C, 0x4A, 0x05, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0xA3, 0x00, 0x00,
        0x05, 0x00, 0x08, 0x00, 0x00, 0x4F, 0x61, 0x61, 0x10, 0xBA, 0x1C, 0xF0,
        0x0A, 0x20, 0x08, 0x00, 0x05, 0x00, 0xC0, 0x11, 0x27, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0xE6, 0xA0, 0xEB, 0xAA, 0xAA, 0x8C, 0xBA,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Image B: the real captured 512-byte 16 GB SPD5118 image (i5-6600T
    /// host, slot `0x50`; the raw capture under
    /// `Work/SPD_Images/intel_6600t_0x50.bin`).
    const IMAGE_B_PRAEON: &[u8] = &[
        0x23, 0x11, 0x0C, 0x03, 0x86, 0x29, 0x00, 0x08, 0x00, 0x60, 0x00, 0x03,
        0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x05, 0x0D, 0xF8, 0xFF, 0x02, 0x00,
        0x6E, 0x6E, 0x6E, 0x11, 0x00, 0x6E, 0x30, 0x11, 0xF0, 0x0A, 0x20, 0x08,
        0x00, 0xA8, 0x14, 0x28, 0x28, 0x00, 0x78, 0x00, 0x14, 0x3C, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0C, 0x2B, 0x2D, 0x04, 0x16, 0x35, 0x23, 0x0D, 0x00, 0x00, 0x2C, 0x0B,
        0x03, 0x24, 0x35, 0x0C, 0x03, 0x2D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9C, 0xCE,
        0x00, 0x00, 0x00, 0x00, 0xE7, 0x00, 0xB3, 0x10, 0x0F, 0x11, 0x20, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xEF, 0x55, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8C, 0xC7, 0x01, 0x23,
        0x34, 0x6A, 0x03, 0x00, 0x00, 0x44, 0x44, 0x52, 0x34, 0x20, 0x4E, 0x42,
        0x20, 0x31, 0x36, 0x47, 0x20, 0x33, 0x32, 0x30, 0x30, 0x20, 0x20, 0x20,
        0x20, 0x41, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Image A as a decodable [`SpdImage`] (the 5950X slot `0x52`).
    fn image_a_gskill() -> SpdImage {
        SpdImage {
            index: 0x52,
            data: IMAGE_A_GSKILL.to_vec(),
        }
    }

    /// Image B as a decodable [`SpdImage`] (the i5-6600T slot `0x50`).
    fn image_b_pradeon() -> SpdImage {
        SpdImage {
            index: 0x50,
            data: IMAGE_B_PRAEON.to_vec(),
        }
    }

    // ------------------------------------------------------------------
    // Synthetic fixtures.
    // ------------------------------------------------------------------

    /// A full synthetic DDR4 image (512 B): Micron module (JEP106 (bank,
    /// code) (1, 0x2C) at `0x140`/`0x141`), 8 Gb (`0x04` code 5), 2
    /// ranks x 8 devices/rank (x8 width, x64 bus -> 16 total -> 16 GiB),
    /// one valid XMP 2.0 profile (slot 1: 2000 MT/s / CL16 / 1350 mV /
    /// tRCD 11 / tRP 11 / tRAS 34), blank slot 2.
    fn ddr4_image() -> SpdImage {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23; // SPD bytes used (legacy check sees 0x03 - junk; 0x02 wins, C8-02)
        data[0x01] = 0x11; // SPD revision 1.1 (check-only)
        data[0x02] = 0x0C; // DDR4 memory type (C8-02 primary)
        // 8 Gb density (0x04 bits 3:0 code 5).
        data[0x04] = 0x05;
        // Module organization (0x0C): bits 5:3 = 1 (rank 2), bits 2:0 = 1 (x8).
        data[0x0C] = 0x09;
        // Bus width (0x0D bits 2:0 code 3 = x64).
        data[0x0D] = 0x03;
        // Module manufacturer JEP106 (bank, code): (1, 0x2C) = Micron.
        data[BYTE_MAKER4_LO] = 0x00; // bank = (0 & 0x7F) + 1 = 1
        data[BYTE_MAKER4_HI] = 0x2C;
        // Serial (4 binary bytes at 0x145..0x148) -> "0ABCD123".
        data[0x145] = 0x0A;
        data[0x146] = 0xBC;
        data[0x147] = 0xD1;
        data[0x148] = 0x23;
        // DDR4 part at 0x149 (20-char field, JESD79-4).
        data[DDR4_PART_START..DDR4_PART_START + 15].copy_from_slice(b"MPK24168BC320G6");
        // XMP 2.0 header at 0x180 (0x0C / 0x4A / enable mask 0x01 / version 0x20).
        data[XMP2_BASE] = XMP2_HEADER_0;
        data[XMP2_BASE + 1] = XMP2_HEADER_1;
        data[XMP2_BASE + 2] = 0x01;
        data[XMP2_BASE + 3] = XMP2_VERSION;
        // XMP 2.0 profile 1 @ 0x189 (47 bytes): VDD 0xA3 (1350 mV), tCK 8
        // (-> 2000 MT/s), CL bitmap bit 9 (-> CL 16), tRCD 90 MTB (11250
        // ps -> 11 ticks), tRP 90 MTB (-> 11 ticks), tRAS 0x84A8 (33960
        // ps -> 34 ticks).
        data[XMP2_P1] = 0xA3; // +0x00 VDD
        data[XMP2_P1 + 3] = 8; // +0x03 tCK
        data[XMP2_P1 + 5] = 0x02; // +0x05 CL bitmap bit 9 (CL 16)
        data[XMP2_P1 + 0x24] = 90; // tRCD MTB companion
        data[XMP2_P1 + 0x23] = 90; // tRP MTB companion
        data[XMP2_P1 + 0x0B] = 0x84; // tRAS hi
        data[XMP2_P1 + 0x0C] = 0xA8; // tRAS lo
        // Profile 2 @ 0x1B8: blank.
        SpdImage {
            index: 0x52,
            data,
        }
    }

    /// A full synthetic DDR5 image (1024 B): Micron module (the C8-02
    /// re-anchor - byte `0x02` = `0x0C` is both the DDR5 type key and
    /// the JEP106 continuation, so the maker is `0xC2`, not the former
    /// Samsung `0x92`), 16 Gb, 6400 MT/s minimum, 1 rank x 8 devices
    /// (8 total -> 16 GiB), one valid XMP 3.0 / EXPO block, blank rest.
    fn ddr5_image() -> SpdImage {
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C; // legacy memory-type check: DDR5 (agrees with 0x02, C8-02)
        // Module manufacturer: Micron 0xC2 (vendor 0x02, continuation 0x0C).
        data[0x01] = 0x02;
        data[0x02] = 0x0C; // DDR5 type key (1024 B image, C8-02) + JEP106 continuation
        // 16 Gb density (DDR5 code 0x15).
        data[0x13] = 0x15;
        // Minimum data rate 6400 MT/s (64 x 100).
        data[0x20] = 0x40;
        // Module organization (C8-03 hub model): 1 rank (0x80 bits
        // 7:4), 8 devices per rank (0x80 bits 3:0), x8 width (0x81
        // bits 2:0, code 1). 8 x 8 = the 64-bit bus -> consistent ->
        // 1 x 8 = 8 total.
        data[0x81] = 0x01;
        data[0x80] = 0x18;
        // DDR5 part at 0x200 (32-char field, JESD79-5) + serial at 0x91.
        data[0x200..0x200 + 13].copy_from_slice(b"S5H1G8719011A");
        data[0x91..0x91 + 16].copy_from_slice(b"2208ABCDEF123456");
        // XMP 3.0 / EXPO region at 0x300 (its JESD79-5 home; C7-03 moved
        // XMP3_BASE here), coexisting with the part number at 0x200
        // (C7-02) - this fixture is the part/profile coexistence proof.
        data[0x300] = XMP3_REVISION; // rev 0x30 + "XMP" signature @ +2
        data[0x302..0x305].copy_from_slice(&XMP3_SIGNATURE);
        // Profile 0 @ 0x310: index 0, validity mask 1, 256 MT/s,
        // CL/tRCD/tRP/tRAS = 20/20/20/40, 1250 mV.
        data[0x310] = 0x00;
        data[0x311] = 0x01;
        data[0x312] = 0x80; // 128 MHz -> 256 MT/s
        data[0x314] = 20;
        data[0x315] = 20;
        data[0x316] = 20;
        data[0x317] = 40;
        data[0x323] = 0xE2; // 1250 mV little-endian (block 19/20)
        data[0x324] = 0x04;
        SpdImage {
            index: 0x53,
            data,
        }
    }

    // ------------------------------------------------------------------
    // (a) Synthetic DDR4 decode.
    // ------------------------------------------------------------------

    /// The synthetic DDR4 image decodes to full ground truth (maker,
    /// part, serial, rank, density, speed) with exactly one valid XMP 2.0
    /// profile (slot 1; slot 2 is blank).
    #[test]
    fn synthetic_ddr4_decodes_to_ground_truth() {
        let m = decode(&ddr4_image());
        assert_eq!(m.index, 0x52);
        assert!(!m.is_ddr5);
        // Maker: (bank, code) (1, 0x2C) = Micron.
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.part, Section::Value("MPK24168BC320G6".to_owned()));
        // Serial: 4 binary bytes at 0x145..0x148, hex-rendered.
        assert_eq!(m.serial, Section::Value("0ABCD123".to_owned()));
        // Rank: 0x0C bits 5:3 = 1 + 1 = 2; width x8; bus x64.
        assert_eq!(m.rank, Section::Value(2));
        // Density: 0x04 bits 3:0 code 5 -> 8192 Mbit (8 Gb).
        assert_eq!(m.density_mbit, Section::Value(8192));
        // Speed: XMP profile 1 tCK 8 -> 2000 MT/s (the primary source).
        assert_eq!(m.speed_mts, Section::Value(2000));
        // No die-ID bytes in this fixture -> absent die maker; the
        // die-type label defaults to Na.
        assert_eq!(m.die_maker, Section::na(NaReason::NotApplicable));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
        // Devices: rank 2 x (64 / 8) = 16 (8 Gb x 16 / 8192 = 16 GiB).
        assert_eq!(m.devices, Section::Value(16));
        // One valid XMP 2.0 profile (slot 1; slot 2 blank).
        assert_eq!(m.profiles.len(), 1, "slot 1 valid, slot 2 blank");
        let p = &m.profiles[0];
        assert_eq!(p.index, 1);
        assert_eq!(p.speed_mts, Section::Value(2000));
        assert_eq!(p.cas, Section::Value(16));
        assert_eq!(p.trcd, Section::Value(11));
        assert_eq!(p.trp, Section::Value(11));
        assert_eq!(p.tras, Section::Value(34));
        assert_eq!(p.voltage, Section::Value(1350));
    }

    // ------------------------------------------------------------------
    // (b) Synthetic DDR5 decode.
    // ------------------------------------------------------------------

    /// The synthetic DDR5 image decodes to full ground truth: the part at
    /// `0x200` (C7-02) and the XMP 3.0 / EXPO profile at `0x300` (C7-03
    /// moved `XMP3_BASE` to the region's JESD79-5 home) coexist and both
    /// decode.
    #[test]
    fn synthetic_ddr5_decodes_to_ground_truth() {
        let m = decode(&ddr5_image());
        assert_eq!(m.index, 0x53);
        assert!(m.is_ddr5);
        // C8-02: byte 0x02 doubles as the JEP106 continuation, so the
        // maker is Micron 0xC2 (the former Samsung 0x92).
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.part, Section::Value("S5H1G8719011A".to_owned()));
        assert_eq!(m.serial, Section::Value("2208ABCDEF123456".to_owned()));
        assert_eq!(m.rank, Section::Value(1));
        assert_eq!(m.density_mbit, Section::Value(16384));
        assert_eq!(m.speed_mts, Section::Value(6400));
        // C8-03: no die-ID bytes in this fixture -> absent die maker;
        // the die-type label defaults to Na; the hub model (0x80 =
        // 0x18: 1 rank / 8 per rank; 0x81 = 0x01: x8, consistent with
        // the 64-bit bus) -> 1 x 8 = 8 total devices.
        assert_eq!(m.die_maker, Section::na(NaReason::NotApplicable));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
        assert_eq!(m.devices, Section::Value(8));
        // C7-03: XMP3_BASE is 0x300, so the fixture's XMP 3.0 region
        // decodes alongside the part at 0x200 (the restored
        // ground-truth assertions).
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
    // (e) LIVE: the real captured images decoded.
    // ------------------------------------------------------------------

    /// Image A (the real G.Skill `F4-3600C18-32GVK` capture) decodes to
    /// the live truth: maker **G.Skill** ((bank, code) (5, 0xCD)),
    /// density **16384 Mbit** (16 Gb, `0x04` code 6), rank **2**
    /// (`0x0C` bits 5:3 = 1 + 1), x8 width, **16 devices** (2 x 8 ->
    /// 32 GiB per slot), **3200 MT/s** (XMP profile 1 tCK 5), part
    /// `F4-3600C18-32GVK`, a blank serial (all-zero `0x145..0x148`),
    /// the die maker **SK hynix** (1, 0xAD), and profile 1 =
    /// 3200 / CL18 / 1350 mV.
    #[test]
    fn image_a_decodes_to_live_truth() {
        let m = decode(&image_a_gskill());
        assert_eq!(m.index, 0x52);
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Value("G.Skill".to_owned()));
        assert_eq!(m.die_maker, Section::Value("SK hynix".to_owned()));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
        assert_eq!(m.density_mbit, Section::Value(16384));
        assert_eq!(m.rank, Section::Value(2));
        assert_eq!(m.devices, Section::Value(16));
        // XMP profile-1 tCK 5 -> 3200 MT/s (the primary base speed).
        assert_eq!(m.speed_mts, Section::Value(3200));
        assert_eq!(m.part, Section::Value("F4-3600C18-32GVK".to_owned()));
        // The live serial is all zero -> Na(NotApplicable).
        assert_eq!(m.serial, Section::Na(NaReason::NotApplicable));
        // One profile (slot 1 populated, slot 2 blank).
        assert_eq!(m.profiles.len(), 1, "profile 1 valid, profile 2 blank");
        let p = &m.profiles[0];
        assert_eq!(p.index, 1);
        assert_eq!(p.speed_mts, Section::Value(3200));
        assert_eq!(p.cas, Section::Value(18));
        assert_eq!(p.voltage, Section::Value(1350));
        // The tRCD/tRP/tRAS cells decode to tick counts (present). The
        // exact values follow the plan's documented FTB/MTB reading
        // (plan §9 open item - interpretation-dependent); the firm
        // anchors above (speed/CL/voltage) are the gate.
        assert!(p.trcd.value().is_some(), "tRCD decodes to a tick count");
        assert!(p.trp.value().is_some(), "tRP decodes to a tick count");
        assert!(p.tras.value().is_some(), "tRAS decodes to a tick count");
    }

    /// Image B (the real 16 GB capture on the i5-6600T) decodes: maker
    /// **`0xC7 (bank 13)`** (an unknown (bank, code) pair renders its
    /// raw form), density **16384 Mbit**, rank **1**, x8 width, **8
    /// devices**, **3200 MT/s** (no XMP -> the tCKAVGmin bin: 0x12 =
    /// 0x05 + 0x7D = 0x00 -> 625 ps -> 3200), part `DDR4 NB 16G 3200`,
    /// serial `6A030000`, no die maker (absent), and no profiles.
    #[test]
    fn image_b_decodes_to_live_truth() {
        let m = decode(&image_b_pradeon());
        assert_eq!(m.index, 0x50);
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Value("0xC7 (bank 13)".to_owned()));
        assert_eq!(m.die_maker, Section::na(NaReason::NotApplicable));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
        assert_eq!(m.density_mbit, Section::Value(16384));
        assert_eq!(m.rank, Section::Value(1));
        assert_eq!(m.devices, Section::Value(8));
        // No XMP header -> the tCKAVGmin base speed (625 ps -> 3200 bin).
        assert_eq!(m.speed_mts, Section::Value(3200));
        assert_eq!(m.part, Section::Value("DDR4 NB 16G 3200".to_owned()));
        assert_eq!(m.serial, Section::Value("6A030000".to_owned()));
        assert!(m.profiles.is_empty(), "image B carries no XMP");
    }

    /// The operator's live shape re-anchored onto image A (the reported
    /// `2 GiB` defect pinned as a regression): 16 Gb dies, 2 ranks, x8
    /// width -> **16 total devices** -> the per-slot capacity `16384 x
    /// 16 / 8192 = 32 GiB`.
    #[test]
    fn live_shape_total_devices() {
        let m = decode(&image_a_gskill());
        assert!(!m.is_ddr5);
        assert_eq!(m.rank, Section::Value(2), "0x0C bits 5:3 -> rank 2");
        assert_eq!(m.devices, Section::Value(16), "2 ranks x (64 / 8) = 16");
        assert_eq!(m.density_mbit, Section::Value(16384), "16 Gb per die (0x04 code 6)");
        // The facade's per-slot arithmetic (density_mbit x devices /
        // 8192) fed by the corrected decode: 32 GiB per slot.
        let (Some(density), Some(devices)) = (m.density_mbit.value(), m.devices.value()) else {
            panic!("the live shape must decode density + devices");
        };
        assert_eq!(
            *density as f64 * *devices as f64 / 8192.0,
            32.0,
            "16384 x 16 / 8192 = 32 GiB per slot"
        );
    }

    // ------------------------------------------------------------------
    // (c) Corrupted / truncated images -> graceful, no panic.
    // ------------------------------------------------------------------

    /// An XMP 2.0 profile that is non-blank but carries no decodable
    /// field (every decoded cell `Na`) is skipped; the module itself
    /// still decodes (profile skipped, module shown).
    #[test]
    fn corrupted_xmp2_profile_is_skipped_module_still_shown() {
        let mut data = ddr4_image().data;
        // Wipe profile 1 and set only an undecoded byte (+0x07) so the
        // block is non-blank but every decoded field is Na.
        data[XMP2_P1..XMP2_P1 + XMP2_LEN].fill(0);
        data[XMP2_P1 + 7] = 0x01;
        let m = decode(&SpdImage {
            index: 0x52,
            data,
        });
        assert!(m.profiles.is_empty(), "all-Na profile must be dropped");
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.rank, Section::Value(2));
        // No usable XMP speed -> the tCKAVGmin fallback (both zero here)
        // -> Na, not a fabricated value.
        assert!(m.speed_mts.is_na());
    }

    /// An image truncated to just its header bytes decodes without
    /// panicking: maker / part / serial / rank / density / speed all
    /// degrade to `Na(ParseError)` naming the past-end byte, and no
    /// profiles are decoded.
    #[test]
    fn truncated_header_only_image_degrades_to_na() {
        let data = vec![0x23, 0x11, 0x0C]; // 0x00 / 0x01 / 0x02
        let m = decode(&SpdImage {
            index: 0x52,
            data,
        });
        assert!(!m.is_ddr5);
        // Maker (0x140) and die maker (0x15E) are past the truncation.
        assert!(matches!(&m.maker, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.part, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.serial, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.rank, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.density_mbit, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.devices, Section::Na(NaReason::ParseError(_))));
        assert!(matches!(&m.speed_mts, Section::Na(NaReason::ParseError(_))));
        assert!(m.profiles.is_empty());
    }

    /// A completely empty image decodes without panicking: every field
    /// is `Na`, the maker is `Na(NotApplicable)` (no module or die ID
    /// present).
    #[test]
    fn empty_image_decodes_to_all_na() {
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![],
        });
        assert_eq!(m.index, 0x50);
        assert!(!m.is_ddr5);
        assert_eq!(m.maker, Section::Na(NaReason::NotApplicable));
        assert_eq!(m.die_maker, Section::na(NaReason::NotApplicable));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
        assert!(m.devices.is_na());
        assert!(m.part.is_na());
        assert!(m.serial.is_na());
        assert!(m.rank.is_na());
        assert!(m.density_mbit.is_na());
        assert!(m.speed_mts.is_na());
        assert!(m.profiles.is_empty());
    }

    /// Sweep every prefix length of both fixture families (0..=512 and
    /// 0..=1024) AND both real captures: `decode` never panics on any
    /// truncation and always preserves the module index.
    #[test]
    fn every_prefix_length_never_panics() {
        for full in [ddr4_image(), ddr5_image(), image_a_gskill(), image_b_pradeon()] {
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

    // ------------------------------------------------------------------
    // (f) P3-05: serde wire contract - bincode round-trips.
    // ------------------------------------------------------------------

    /// A representative decoded DDR4 + DDR5 module and an all-`Na`
    /// module each round-trip through bincode, proving the SPD field
    /// tree (`Section<String/u8/u16>`, `Vec<SpdProfile>`) is wire-safe.
    #[test]
    fn spd_module_bincode_round_trip() {
        let ddr4 = decode(&ddr4_image());
        let ddr5 = decode(&ddr5_image());
        // all-Na degradation: blank image -> every field `Na`, no profiles
        let all_na = decode(&SpdImage {
            index: 0x53,
            data: vec![0u8; 1024],
        });

        for m in [&ddr4, &ddr5, &all_na] {
            let bytes = bincode::serialize(m)
                .expect("SpdModule must serialize (no-panic contract)");
            let back: SpdModule =
                bincode::deserialize(&bytes).expect("SpdModule must deserialize");
            assert_eq!(*m, back);
        }

        // the 2-module fixture (the `SystemMemoryTelemetry.spd` shape) too
        let modules = vec![ddr4, ddr5];
        let bytes = bincode::serialize(&modules)
            .expect("Vec<SpdModule> must serialize (no-panic contract)");
        let back: Vec<SpdModule> =
            bincode::deserialize(&bytes).expect("Vec<SpdModule> must deserialize");
        assert_eq!(modules, back);
    }

    // ------------------------------------------------------------------
    // (d) JEP106 manufacturer decode.
    // ------------------------------------------------------------------

    /// Regression: every entry of the DDR4 [`JEP106_BCD`] (bank, code)
    /// table decodes to its recorded name from the `0x140`/`0x141` pair
    /// (the `low` byte carries `bank - 1`).
    #[test]
    fn jep106_bcd_table_regression_all_entries_decode() {
        for (bank, code, name) in JEP106_BCD {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C; // DDR4
            data[BYTE_MAKER4_LO] = bank - 1;
            data[BYTE_MAKER4_HI] = *code;
            let m = decode(&SpdImage { index: 0x50, data });
            assert_eq!(
                m.maker,
                Section::Value(name.to_string()),
                "JEP106 (bank, code) ({bank}, 0x{code:02X}) must decode to {name}"
            );
        }
    }

    /// An unknown (bank, code) pair renders as its raw `0x{code} (bank
    /// {bank})` form (still a `Value`), never a panic; a low byte with
    /// the reserved top bit set computes the same bank.
    #[test]
    fn jep106_bcd_unknown_renders_code_with_bank() {
        // bank 1 (low 0x00), code 0xFF (unknown).
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[BYTE_MAKER4_LO] = 0x00;
        data[BYTE_MAKER4_HI] = 0xFF;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.maker, Section::Value("0xFF (bank 1)".to_owned()));
        // bank 5 (low 0x04), code 0x00: present pair (low non-zero),
        // unknown code -> raw form.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[BYTE_MAKER4_LO] = 0x04;
        data[BYTE_MAKER4_HI] = 0x00;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.maker, Section::Value("0x00 (bank 5)".to_owned()));
        // The reserved top bit of the low byte never changes the bank:
        // low 0x84 -> bank (0x04 & 0x7F) + 1 = 5.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[BYTE_MAKER4_LO] = 0x84;
        data[BYTE_MAKER4_HI] = 0xFF;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.maker, Section::Value("0xFF (bank 5)".to_owned()));
    }

    /// Regression: the DDR5 8-bit vendor/continuation nibble [`JEP106`]
    /// table is unchanged - every entry decodes to its recorded name on
    /// the DDR5 path (the classification rides the legacy byte `0x00` so
    /// `0x02` is free to carry the continuation nibble).
    #[test]
    fn jep106_table_regression_all_entries_decode() {
        for (code, name) in JEP106 {
            let mut data = vec![0u8; 1024];
            data[0x00] = 0x0C; // legacy DDR5 (frees 0x02 for the nibble)
            data[0x01] = *code & 0x0F; // vendor nibble
            data[0x02] = *code >> 4; // continuation nibble
            let m = decode(&SpdImage { index: 0x51, data });
            assert!(m.is_ddr5, "1024 B + legacy 0x0C classifies DDR5");
            assert_eq!(
                m.maker,
                Section::Value(name.to_string()),
                "JEP106 entry 0x{code:02X} must decode to {name}"
            );
        }
    }

    /// A DDR4 module with no module ID and no die ID at all ->
    /// `Na(NotApplicable)`.
    #[test]
    fn no_manufacturer_id_at_all_is_na() {
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![0u8; 512],
        });
        assert_eq!(m.maker, Section::Na(NaReason::NotApplicable));
    }

    /// No module ID present (DDR4): the decoder falls back to the DRAM
    /// die manufacturer (the (bank, code) pair at `0x15E`/`0x15F`).
    #[test]
    fn die_maker_fallback_when_module_id_absent() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C; // DDR4
        data[BYTE_DIE4_LO] = 0x00; // Micron (bank, code) (1, 0x2C)
        data[BYTE_DIE4_HI] = 0x2C;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        // C6-02: the die maker is carried separately and agrees with
        // the fallback source.
        assert_eq!(m.die_maker, Section::Value("Micron".to_owned()));
    }

    // ------------------------------------------------------------------
    // Density regression (the 0x04 table + the DDR5 family).
    // ------------------------------------------------------------------

    /// Regression: the DDR4 density table (byte `0x04` bits 3:0) decodes
    /// all 10 published codes to their Mbit values; codes `0xA..=0xF`
    /// degrade to `Na(ParseError)` naming the code (no panic).
    #[test]
    fn ddr4_density_regression_0x04_table() {
        for (code, expected) in DENSITY4_TABLE.iter().enumerate() {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C;
            data[0x04] = code as u8; // bits 3:0 = the code
            let m = decode(&SpdImage { index: 0x50, data });
            assert_eq!(
                m.density_mbit,
                Section::Value(*expected),
                "DDR4 density code {code} must decode to {expected} Mbit"
            );
        }
        // The high bits of 0x04 do not affect the code (bits 3:0 only).
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x04] = 0x86; // the live capture's value: bits 3:0 = 6 -> 16384
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.density_mbit, Section::Value(16384));
        // Unknown codes 0xA..=0xF -> Na naming the code.
        for code in 0xA..=0x0Fu8 {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C;
            data[0x04] = code;
            let m = decode(&SpdImage { index: 0x50, data });
            let Section::Na(NaReason::ParseError(detail)) = &m.density_mbit else {
                panic!("density code 0x{code:02X} must be Na(ParseError)");
            };
            assert!(detail.contains(&format!("0x{code:02X}")), "{detail}");
        }
    }

    /// Regression: the DDR5 density documented model `0x11..=0x18` is
    /// unchanged: 1..32 Gb decode to their Mbit values and `0x18`
    /// (64 Gb) overflows the u16 Mbit cell -> `Na(ParseError)`.
    #[test]
    fn ddr5_density_regression_family() {
        for (code, mbit) in [
            (0x11u8, 1024u16),
            (0x12, 2048),
            (0x13, 4096),
            (0x14, 8192),
            (0x15, 16384),
            (0x16, 24576),
            (0x17, 32768),
        ] {
            let mut data = vec![0u8; 1024];
            data[0x00] = 0x0C; // DDR5 signature
            data[0x13] = code;
            let m = decode(&SpdImage { index: 0x51, data });
            assert_eq!(m.density_mbit, Section::Value(mbit), "DDR5 0x{code:02X}");
        }
        // 0x18 = 64 Gb = 65536 Mbit overflows the u16 cell.
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C;
        data[0x13] = 0x18;
        let m = decode(&SpdImage { index: 0x51, data });
        assert!(matches!(
            m.density_mbit,
            Section::Na(NaReason::ParseError(_))
        ));
    }

    // ------------------------------------------------------------------
    // Rank + devices regression.
    // ------------------------------------------------------------------

    /// Regression: the DDR4 rank is `0x0C` bits 5:3 + 1 (codes 0-7 ->
    /// 1-8 ranks; x8 width kept in bits 2:0); a blank (`0x00`) `0x0C`
    /// degrades to `Na(ParseError)` naming the byte; an absent `0x0C`
    /// (truncation) does too.
    #[test]
    fn rank_from_0x0c_bits_5_3_plus_1() {
        for rank_code in 0..8u8 {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C;
            data[0x0C] = (rank_code << 3) | 0x01; // rank code + x8 width
            data[0x0D] = 0x03; // x64 bus
            let m = decode(&SpdImage { index: 0x50, data });
            assert_eq!(
                m.rank,
                Section::Value(rank_code + 1),
                "0x0C bits 5:3 = {rank_code} -> rank {}",
                rank_code + 1
            );
        }
        // Blank 0x0C -> Na naming the byte.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(
            matches!(&m.rank, Section::Na(NaReason::ParseError(d)) if d.contains("0x0C")),
            "blank 0x0C must name the byte: {:?}",
            m.rank
        );
        // Absent 0x0C (truncated) -> Na naming the byte.
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![0x23, 0x11, 0x0C],
        });
        assert!(
            matches!(&m.rank, Section::Na(NaReason::ParseError(d)) if d.contains("0x0C")),
            "OOB 0x0C must name the byte: {:?}",
            m.rank
        );
    }

    /// Regression: the DDR5 rank stays the `0x80` bits 7:4 hub nibble
    /// (the documented model, unchanged); a zero rank nibble ->
    /// `Na(ParseError)` naming `0x80`.
    #[test]
    fn ddr5_rank_from_0x80_hub_nibble() {
        // 0x80 = 0x38 -> rank 3.
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C;
        data[0x80] = 0x38;
        data[0x81] = 0x01;
        let m = decode(&SpdImage { index: 0x51, data });
        assert_eq!(m.rank, Section::Value(3), "DDR5 0x80 = 0x38");
        // A zero rank nibble -> Na naming 0x80.
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C;
        data[0x80] = 0x08;
        let m = decode(&SpdImage { index: 0x51, data });
        assert!(
            matches!(&m.rank, Section::Na(NaReason::ParseError(d)) if d.contains("0x80")),
            "rank 0 must name byte 0x80: {:?}",
            m.rank
        );
    }

    /// Regression: the total device count is generation-scoped: the DDR4
    /// arm is rank x (bus / width) from `0x0C`/`0x0D` (no hub
    /// arithmetic); the DDR5 arm keeps the `0x80` hub model.
    #[test]
    fn devices_total_generation_scoped() {
        // DDR4: 0x0C = 0x09 (rank 2, x8) + 0x0D = 0x03 (x64) ->
        // 2 x (64 / 8) = 16 total.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x0C] = 0x09;
        data[0x0D] = 0x03;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.devices, Section::Value(16), "0x09 + 0x03 -> 2 x 8");
        assert_eq!(m.rank, Section::Value(2), "0x0C bits 5:3 + 1");
        // DDR4: the width code set divides the bus (rank 1, non-blank
        // 0x0C): x8 -> 8 per rank, x16 -> 4, x32 -> 2.
        for (org, width, per_rank) in [(0x01u8, 8u8, 8u8), (0x02, 16, 4), (0x03, 32, 2)] {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C;
            data[0x0C] = org; // rank 1 (bits 5:3 = 0) + the width code
            data[0x0D] = 0x03;
            let m = decode(&SpdImage { index: 0x50, data });
            assert_eq!(m.devices, Section::Value(per_rank), "width {width}");
            assert_eq!(m.rank, Section::Value(1), "width {width}");
        }
        // DDR5 hub model: 0x80 = 0x18 (rank 1 / per-rank 8) +
        // 0x81 = 0x01 (x8; 8 x 8 = 64, consistent) -> 1 x 8 = 8 total.
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C;
        data[0x80] = 0x18;
        data[0x81] = 0x01;
        let m = decode(&SpdImage { index: 0x51, data });
        assert_eq!(m.devices, Section::Value(8), "0x18 + 0x01");
        assert_eq!(m.rank, Section::Value(1), "0x18 + 0x01");
    }

    /// Invalid DDR4 module-organization configs gate `devices` to
    /// `Na(ParseError)` naming the offending byte: an unrecognized width
    /// code (4-7 at `0x0C` bits 2:0 - code 3 is now the valid x32) and a
    /// non-x64 bus code (`0x0D` bits 2:0 != 3); the rank degrades only
    /// when its own source fails. A truncated image names the OOB byte.
    #[test]
    fn devices_invalid_org_gates_naming_the_byte() {
        // Unrecognized width: 0x0C = 0x0C (rank 2; width code 4) ->
        // devices Na naming 0x0C; the rank (bits 5:3 + 1) is 2.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x0C] = 0x0C;
        data[0x0D] = 0x03;
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(
            matches!(&m.devices, Section::Na(NaReason::ParseError(d)) if d.contains("0x0C")),
            "unknown width must name byte 0x0C: {:?}",
            m.devices
        );
        assert_eq!(m.rank, Section::Value(2), "the 0x0C rank source is well-formed");
        // Non-x64 bus: 0x0C = 0x09 (rank 2, x8) + 0x0D = 0x00 (bus
        // code 0) -> devices Na naming 0x0D; the rank is 2.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x0C] = 0x09;
        data[0x0D] = 0x00;
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(
            matches!(&m.devices, Section::Na(NaReason::ParseError(d)) if d.contains("0x0D")),
            "non-x64 bus must name byte 0x0D: {:?}",
            m.devices
        );
        assert_eq!(m.rank, Section::Value(2), "the 0x0C rank source is well-formed");
        // Byte 0x0C out of bounds (truncated image): both cells Na,
        // naming the byte.
        let m = decode(&SpdImage {
            index: 0x50,
            data: vec![0x23, 0x11, 0x0C],
        });
        assert!(
            matches!(&m.devices, Section::Na(NaReason::ParseError(d)) if d.contains("0x0C")),
            "OOB 0x0C must name the byte: {:?}",
            m.devices
        );
        assert!(
            matches!(&m.rank, Section::Na(NaReason::ParseError(d)) if d.contains("0x0C")),
            "OOB 0x0C must name the byte: {:?}",
            m.rank
        );
    }

    // ------------------------------------------------------------------
    // (h) C6-02: the separately carried die maker / die type.
    // ------------------------------------------------------------------

    /// The DRAM die maker is carried separately from the module maker:
    /// with both present, `maker` stays the module ID while `die_maker`
    /// decodes the die-ID bytes (DDR4 `0x15E`/`0x15F`), and the
    /// die-type label stays its `Na(NotApplicable)` default.
    #[test]
    fn die_maker_carried_separately_from_module_maker() {
        let mut data = ddr4_image().data; // module maker: Micron
        data[BYTE_DIE4_LO] = 0x00; // SK hynix (bank, code) (1, 0xAD)
        data[BYTE_DIE4_HI] = 0xAD;
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.die_maker, Section::Value("SK hynix".to_owned()));
        assert_eq!(m.die_type, Section::na(NaReason::NotApplicable));
    }

    /// The DDR5 die-ID bytes (`0x2E`/`0x2F`) decode the separately
    /// carried die maker on a DDR5 module.
    #[test]
    fn die_maker_decodes_ddr5_die_id_bytes() {
        let mut data = ddr5_image().data; // module maker: Micron (C8-02)
        data[0x2E] = 0x02; // Micron 0xC2 (vendor 0x02, continuation 0x0C)
        data[0x2F] = 0x0C;
        let m = decode(&SpdImage { index: 0x53, data });
        assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        assert_eq!(m.die_maker, Section::Value("Micron".to_owned()));
    }

    /// A DDR4 die-ID (bank, code) pair absent from [`JEP106_BCD`]
    /// renders as its raw form in the separately carried die maker
    /// (still a `Value`); with no module ID the module maker falls back
    /// to the same die source, so the two agree.
    #[test]
    fn die_maker_unknown_code_renders_raw_hex_and_matches_fallback() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[BYTE_DIE4_LO] = 0x00; // bank 1, code 0xAB (unknown)
        data[BYTE_DIE4_HI] = 0xAB;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.maker, Section::Value("0xAB (bank 1)".to_owned())); // fallback = die
        assert_eq!(m.die_maker, Section::Value("0xAB (bank 1)".to_owned()));
    }

    // ------------------------------------------------------------------
    // (g2) C8-02: the byte-0x02 memory-type classification.
    // ------------------------------------------------------------------

    /// The basic-info memory type (byte `0x02`) classifies the module
    /// (C8-02, D-2a): `0x0C` + 512 B -> DDR4 (density from `0x04`, the
    /// part from the `0x149` primary), `0x0C` + 1024 B -> DDR5 (the
    /// generations share the code; the image length disambiguates),
    /// `0x0B` -> DDR3 (the legacy `0x81` part location).
    #[test]
    fn memory_type_byte_0x02_classifies() {
        // The live host shape: 0x00 = 0x23 (junk), 0x02 = 0x0C (the
        // DDR4 key), 0x04 = 0x86 (16 Gb), 512 B -> DDR4, part from
        // 0x149.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x04] = 0x86;
        data[0x149..0x149 + 16].copy_from_slice(b"F4-3600C18-32GVK");
        let m = decode(&SpdImage { index: 0x52, data });
        assert!(!m.is_ddr5, "512 B + 0x02 = 0x0C must classify DDR4");
        assert_eq!(m.density_mbit, Section::Value(16384), "16 Gb per 0x04 code 6");
        assert_eq!(m.part, Section::Value("F4-3600C18-32GVK".to_owned()));

        // The same key on a 1024 B image -> DDR5.
        let mut data = vec![0u8; 1024];
        data[0x02] = 0x0C;
        let m = decode(&SpdImage { index: 0x53, data });
        assert!(m.is_ddr5, "1024 B + 0x02 = 0x0C must classify DDR5");

        // 0x0B -> DDR3: the part stays at the legacy 0x81 location.
        let mut data = vec![0u8; 512];
        data[0x02] = 0x0B;
        data[0x81..0x81 + 14].copy_from_slice(b"DDR3PART-12345");
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(!m.is_ddr5, "0x02 = 0x0B is not DDR5");
        assert_eq!(m.part, Section::Value("DDR3PART-12345".to_owned()));
    }

    /// When byte `0x02` carries no recognized type, the legacy
    /// byte-`0x00` check (bits 4:0) classifies (D-2a): `0x0A` ->
    /// DDR4, `0x0C` -> DDR5; with both unrecognized the image length
    /// is the last resort (1024 B -> DDR5). The pre-existing fixtures
    /// (byte `0x00` + `0x02` agreeing) classify unchanged.
    #[test]
    fn legacy_byte_0x00_fallback_when_0x02_unrecognized() {
        // 0x02 blank + legacy 0x0A -> DDR4.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0A;
        let m = decode(&SpdImage { index: 0x52, data });
        assert!(!m.is_ddr5, "legacy 0x0A must classify DDR4");

        // 0x02 blank + legacy 0x0C -> DDR5 (the legacy code wins over
        // the 512 B length).
        let mut data = vec![0u8; 512];
        data[0x00] = 0x0C;
        let m = decode(&SpdImage { index: 0x53, data });
        assert!(m.is_ddr5, "legacy 0x0C must classify DDR5");

        // Both unrecognized: the image length is the last resort.
        for (len, expect_ddr5) in [(512usize, false), (1024, true)] {
            let mut data = vec![0u8; len];
            data[0x00] = 0x23; // junk (the live host shape)
            let m = decode(&SpdImage { index: 0x50, data });
            assert_eq!(
                m.is_ddr5, expect_ddr5,
                "unrecognized types: length {len} must classify {expect_ddr5}"
            );
        }
    }

    // ------------------------------------------------------------------
    // (i) C7-02: per-generation part-number decode.
    // ------------------------------------------------------------------

    /// DDR4: the 20-char part at `0x149` (JESD79-4) decodes in full.
    #[test]
    fn ddr4_part_20_chars_at_0x149_decodes() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x149..0x149 + 20].copy_from_slice(b"ABCDEFGHIJKLMNOPQRST");
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(!m.is_ddr5);
        assert_eq!(m.part, Section::Value("ABCDEFGHIJKLMNOPQRST".to_owned()));
    }

    /// DDR4 fallback: when the `0x149` region is present-but-blank, the
    /// decoder falls back to the legacy `0x81` location.
    #[test]
    fn ddr4_part_falls_back_to_0x81_when_0x149_blank() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x81..0x81 + 12].copy_from_slice(b"FALLBACK0123");
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.part, Section::Value("FALLBACK0123".to_owned()));
    }

    /// DDR5: the 32-char part at `0x200` (JESD79-5) decodes in full.
    #[test]
    fn ddr5_part_32_chars_at_0x200_decodes() {
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C; // DDR5 signature
        data[0x200..0x200 + 32].copy_from_slice(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345");
        let m = decode(&SpdImage { index: 0x51, data });
        assert!(m.is_ddr5);
        assert_eq!(
            m.part,
            Section::Value("ABCDEFGHIJKLMNOPQRSTUVWXYZ012345".to_owned())
        );
    }

    /// DDR3 (memory type `0x02`): the part stays at the unchanged
    /// 16-char `0x81` location.
    #[test]
    fn ddr3_part_16_chars_at_0x81_unchanged() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x02; // unrecognized legacy type; the 0x02 key byte is blank
        data[0x81..0x81 + 16].copy_from_slice(b"DDR3PART12345678");
        let m = decode(&SpdImage { index: 0x50, data });
        assert!(!m.is_ddr5);
        assert_eq!(m.part, Section::Value("DDR3PART12345678".to_owned()));
    }

    /// A DDR4 module whose `0x149` primary is present-but-blank and
    /// whose `0x81` fallback is also blank degrades to
    /// `Na(NotApplicable)` (never a panic).
    #[test]
    fn ddr4_part_blank_primary_and_fallback_is_not_applicable() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(m.part, Section::Na(NaReason::NotApplicable));
    }

    // ------------------------------------------------------------------
    // (j) C7-03: XMP3 base move - DDR5 part/profile coexistence.
    // ------------------------------------------------------------------

    /// C7-03 regression: a DDR5 image carrying the 32-char part number
    /// at `0x200` (JESD79-5) and a valid XMP 3.0 / EXPO profile at
    /// `0x300` decodes both - the part is the `0x200` string and the
    /// profile list is non-empty. Under the former `0x200` XMP3 base the
    /// region read would have sat inside the part-number ASCII and the
    /// profile would have been dropped.
    #[test]
    fn ddr5_part_at_0x200_and_xmp3_at_0x300_coexist() {
        let mut data = vec![0u8; 1024];
        data[0x00] = 0x0C; // DDR5 signature
        // Full 32-char part number at 0x200 (JESD79-5 home).
        data[0x200..0x200 + 32].copy_from_slice(b"CTA20256D8G0111A2345678901234567");
        // XMP 3.0 / EXPO region at 0x300 (JESD79-5 home).
        data[0x300] = XMP3_REVISION; // rev 0x30 + "XMP" signature @ +2
        data[0x302..0x305].copy_from_slice(&XMP3_SIGNATURE);
        // Profile 0 @ 0x310: index 0, validity mask 1, 256 MT/s,
        // CL/tRCD/tRP/tRAS = 20/20/20/40, 1250 mV.
        data[0x310] = 0x00;
        data[0x311] = 0x01;
        data[0x312] = 0x80; // 128 MHz -> 256 MT/s
        data[0x314] = 20;
        data[0x315] = 20;
        data[0x316] = 20;
        data[0x317] = 40;
        data[0x323] = 0xE2; // 1250 mV little-endian (block 19/20)
        data[0x324] = 0x04;
        let m = decode(&SpdImage {
            index: 0x53,
            data,
        });
        assert!(m.is_ddr5);
        assert_eq!(
            m.part,
            Section::Value("CTA20256D8G0111A2345678901234567".to_owned())
        );
        assert!(!m.profiles.is_empty(), "the XMP3 profile at 0x300 must decode");
        let p = &m.profiles[0];
        assert_eq!(p.speed_mts, Section::Value(256));
        assert_eq!(p.voltage, Section::Value(1250));
    }

    // ------------------------------------------------------------------
    // Base speed (DDR4 tCKAVGmin model + XMP primary).
    // ------------------------------------------------------------------

    /// The JEDEC base speed (no XMP present) is `tCKAVGmin` (0x12 x 125
    /// + i8(0x7D) ps) -> `2000 / tCK_ns` MT/s, floored to the largest
    /// standard bin <= the computed value: 0x12 = 0x0D + 0x7D = 0x11 ->
    /// 1642 ps -> 1218 -> 1066; a mid-bin case (0x12 = 0x06 + 0x7D =
    /// 0x64 -> 850 ps -> 2353 -> 2133).
    #[test]
    fn base_speed_tckavgmin_floors_to_bin() {
        // 1642 ps -> 1218 MT/s -> bin 1066.
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x12] = 0x0D;
        data[0x7D] = 0x11;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(
            m.speed_mts,
            Section::Value(1066),
            "1642 ps -> 1218 MT/s -> bin 1066"
        );
        // 850 ps -> 2353 MT/s -> bin 2133 (2400 > computed).
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        data[0x12] = 0x06;
        data[0x7D] = 0x64;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(
            m.speed_mts,
            Section::Value(2133),
            "850 ps -> 2353 MT/s -> bin 2133"
        );
    }

    /// The tCKAVGmin FTB sentinel bytes (0x7D in {0x6E, 0xF8}) mark "no
    /// base speed recorded" -> `Na(ParseError)` naming the byte, even
    /// with a non-zero MTB.
    #[test]
    fn base_speed_sentinel_degrades_to_na() {
        for sentinel in [0x6E, 0xF8] {
            let mut data = vec![0u8; 512];
            data[0x00] = 0x23;
            data[0x02] = 0x0C;
            data[0x12] = 0x05;
            data[0x7D] = sentinel;
            let m = decode(&SpdImage { index: 0x50, data });
            let Section::Na(NaReason::ParseError(detail)) = m.speed_mts else {
                panic!("sentinel 0x{sentinel:02X} must be Na(ParseError)");
            };
            assert!(detail.contains("sentinel"), "{detail}");
        }
    }

    /// With no XMP and no usable tCKAVGmin (zero MTB), the base speed
    /// degrades to `Na(ParseError)` - never a fabricated value.
    #[test]
    fn base_speed_no_source_degrades_to_na() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        // 0x12 = 0, 0x7D = 0 -> no tCKAVGmin; no XMP header.
        let m = decode(&SpdImage { index: 0x50, data });
        let Section::Na(NaReason::ParseError(_)) = m.speed_mts else {
            panic!("no speed source must be Na(ParseError)");
        };
    }

    /// The DDR4 base speed prefers XMP profile 1 over the JEDEC base:
    /// a valid XMP header + profile 1 (tCK 5) yields 3200 MT/s even
    /// when the tCKAVGmin bytes would floor to a lower bin.
    #[test]
    fn base_speed_xmp_profile1_wins_over_jedec_base() {
        let mut data = vec![0u8; 512];
        data[0x00] = 0x23;
        data[0x02] = 0x0C;
        // tCKAVGmin would floor to 1066 (1642 ps).
        data[0x12] = 0x0D;
        data[0x7D] = 0x11;
        // Valid XMP header + profile 1 with tCK 5 -> 3200 MT/s.
        data[XMP2_BASE] = XMP2_HEADER_0;
        data[XMP2_BASE + 1] = XMP2_HEADER_1;
        data[XMP2_BASE + 3] = XMP2_VERSION;
        data[XMP2_P1 + 3] = 5;
        let m = decode(&SpdImage { index: 0x50, data });
        assert_eq!(
            m.speed_mts,
            Section::Value(3200),
            "XMP profile 1 (tCK 5) wins over the JEDEC base"
        );
    }

    // ------------------------------------------------------------------
    // XMP 2.0 (the new 47-byte model).
    // ------------------------------------------------------------------

    /// The XMP 2.0 header gate: any of `0x180` / `0x181` / `0x183`
    /// invalid -> no profiles, even when a profile block is populated;
    /// the module is still shown.
    #[test]
    fn xmp2_header_gate() {
        for (off, bad) in [(XMP2_BASE, 0x00u8), (XMP2_BASE + 1, 0x00u8), (XMP2_BASE + 3, 0x00u8)] {
            let mut data = ddr4_image().data;
            // Install a full valid header, then corrupt one byte.
            data[XMP2_BASE] = XMP2_HEADER_0;
            data[XMP2_BASE + 1] = XMP2_HEADER_1;
            data[XMP2_BASE + 3] = XMP2_VERSION;
            data[off] = bad;
            let m = decode(&SpdImage { index: 0x52, data });
            assert!(
                m.profiles.is_empty(),
                "corrupt header byte 0x{off:02X} must drop all profiles"
            );
            assert_eq!(m.maker, Section::Value("Micron".to_owned()));
        }
        // A valid header with an all-zero (blank) profile 1 and a
        // populated profile 2 decodes only profile 2.
        let mut data = ddr4_image().data;
        data[XMP2_P1..XMP2_P1 + XMP2_LEN].fill(0);
        data[XMP2_P2 + 3] = 5; // tCK 5 -> 3200
        data[XMP2_P2] = 0xA3; // 1350 mV
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.profiles.len(), 1, "blank p1 skipped, p2 decodes");
        assert_eq!(m.profiles[0].index, 2);
        assert_eq!(m.profiles[0].speed_mts, Section::Value(3200));
    }

    /// A valid XMP 2.0 profile 2 at `0x1B8` decodes with `index == 2`
    /// (profile 1 blank).
    #[test]
    fn xmp2_profile2_at_0x1b8() {
        let mut data = ddr4_image().data;
        data[XMP2_P1..XMP2_P1 + XMP2_LEN].fill(0);
        data[XMP2_P2] = 0xA3; // VDD 1350 mV
        data[XMP2_P2 + 3] = 4; // tCK 4 -> 4000 MT/s
        data[XMP2_P2 + 5] = 0x01; // CL bitmap bit 8 -> CL 15
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.profiles.len(), 1);
        let p = &m.profiles[0];
        assert_eq!(p.index, 2);
        assert_eq!(p.speed_mts, Section::Value(4000));
        assert_eq!(p.cas, Section::Value(15));
        assert_eq!(p.voltage, Section::Value(1350));
    }

    /// The XMP 2.0 CL supported bitmap (24-bit LE at +0x04..+0x06): with
    /// several CL bits set, the lowest (smallest CL) wins.
    #[test]
    fn xmp2_cl_bitmap_first_set_bit() {
        // CL 16 (bit 9) and CL 18 (bit 11) both set -> CL 16 wins.
        let mut data = ddr4_image().data;
        data[XMP2_P1 + 3] = 5; // tCK 5
        data[XMP2_P1 + 5] = 0x02 | 0x08;
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.profiles[0].cas, Section::Value(16), "lowest set CL bit wins");
        // Only CL 18 set.
        let mut data = ddr4_image().data;
        data[XMP2_P1 + 3] = 5;
        data[XMP2_P1 + 5] = 0x08;
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.profiles[0].cas, Section::Value(18));
        // The top bit of the high byte (bit 23 of the 24-bit bitmap)
        // -> CL 30.
        let mut data = ddr4_image().data;
        data[XMP2_P1 + 3] = 5;
        data[XMP2_P1 + 5] = 0x00;
        data[XMP2_P1 + 6] = 0x80;
        let m = decode(&SpdImage { index: 0x52, data });
        assert_eq!(m.profiles[0].cas, Section::Value(30));
    }
}
