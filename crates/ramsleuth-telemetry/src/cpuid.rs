//! CPUID-based vendor + microarchitecture detection (P2-01, interface freeze).
//!
//! Reads CPUID only via the safe `__cpuid` intrinsic — no memory, SMU, or
//! MMIO access. Exposes the frozen public API that every downstream provider
//! dispatches on: [`AmdZen`], [`IntelGen`], [`CpuVendor`], [`CpuInfo`], and
//! [`CpuInfo::detect()`].
//!
//! Detection:
//! - Vendor string from leaf 0 (EBX ‖ EDX ‖ ECX) → "AuthenticAMD" /
//!   "GenuineIntel" / unknown.
//! - AMD extended family from leaf `0x80000001` → [`AmdZen`].
//! - Intel family/model from leaf 1 → [`IntelGen`] (the brand string
//!   disambiguates the shared `0xA5` Comet/Rocket model, OQ-10).
//! - Brand string from leaves `0x80000002`–`0x80000004`.
//!
//! Non-`x86_64` targets compile to [`CpuVendor::Unknown`] (brand
//! `"non-x86"`); they never fail to build and `detect()` never panics.

/// AMD Zen microarchitecture generation.
///
/// Keyed by the CPUID family observed on real Zen silicon (the reference test
/// host, a Ryzen 9 5950X / Zen 3, reports family `0x19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AmdZen {
    /// Zen 1 (family `0x15`).
    Zen1,
    /// Zen 2 (family `0x17`).
    Zen2,
    /// Zen 3 (family `0x19`) — e.g. the Ryzen 9 5950X reference host.
    Zen3,
    /// Zen 4 (family `0x1A`).
    Zen4,
    /// Zen 5 (family `0x1C`).
    Zen5,
}

/// Intel microarchitecture generation, for MCHBAR IMC decoding (P2-07).
///
/// Best-effort model table covering Skylake through Arrow Lake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IntelGen {
    /// 6th gen (Skylake, 2015) — DDR4.
    Skylake,
    /// 7th gen (Kaby Lake, 2016) — DDR4.
    KabyLake,
    /// 8th/9th gen (Coffee Lake, 2017/2018) — DDR4.
    CoffeeLake,
    /// 10th gen (Comet Lake, 2019) — DDR4.
    CometLake,
    /// 10th gen (Ice Lake, 2019, 10 nm) — DDR4.
    IceLake,
    /// 11th gen (Tiger Lake, 2020, 10 nm) — DDR4.
    TigerLake,
    /// 12th gen (Alder Lake, 2021) — DDR4/DDR5.
    AlderLake,
    /// 13th gen (Raptor Lake, 2022) — DDR4/DDR5.
    RaptorLake,
    /// 14th gen (Meteor Lake, 2023) — DDR5.
    MeteorLake,
    /// 15th gen (Arrow Lake, 2024) — DDR5.
    ArrowLake,
    /// Intel (family 6) with an unrecognized model.
    Unrecognized,
    /// 11th gen (Rocket Lake, 2021, 10 nm) — DDR4.
    ///
    /// Appended after [`Self::Unrecognized`] so the existing variant
    /// indices (and the frozen wire encoding) are preserved (OQ-10).
    /// Emitted by `detect()` (IG-15) when the CPUID model is the shared
    /// `0xA5` (Comet Lake / Rocket Lake — the model number alone cannot
    /// split them, OQ-10) and the brand string's marketing generation
    /// is 11th gen ([`intel_gen_from_brand`]); a `0xA5` CPU without an
    /// 11th-gen brand hint stays the conservative Comet Lake bucket.
    RocketLake,
}

/// CPU vendor + generation. The frozen dispatch key for every provider.
///
/// - [`CpuVendor::Amd`]: AMD Zen silicon (see [`AmdZen`]).
/// - [`CpuVendor::Intel`]: Intel silicon (see [`IntelGen`], possibly
///   [`IntelGen::Unrecognized`]).
/// - [`CpuVendor::Unknown`]: unrecognized vendor, or an AMD CPU whose family
///   is outside the frozen [`AmdZen`] set (e.g. a future Zen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CpuVendor {
    Amd(AmdZen),
    Intel(IntelGen),
    Unknown,
}

/// Detected CPU information, produced by [`CpuInfo::detect()`].
///
/// `vendor` is the frozen dispatch key; `brand` is the CPUID brand string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CpuInfo {
    pub vendor: CpuVendor,
    pub brand: String,
}

impl CpuInfo {
    /// Detect the CPU vendor and microarchitecture via CPUID.
    ///
    /// Safe and total: never panics and always returns a value. On
    /// non-`x86_64` targets returns
    /// `CpuInfo { vendor: Unknown, brand: "non-x86" }`.
    pub fn detect() -> CpuInfo {
        #[cfg(target_arch = "x86_64")]
        {
            detect_x86()
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "non-x86".to_owned(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// x86_64 CPUID reads (gated; total — never panics).
// ---------------------------------------------------------------------------

/// Run CPUID detection on `x86_64`.
#[cfg(target_arch = "x86_64")]
// `__cpuid` is an `unsafe fn` on MSRV 1.75 and pre-stabilization stables
// (the block is mandatory there — E0133) and a safe intrinsic on recent
// stables (where it would trip `unused_unsafe`); block + allow keeps
// every toolchain warning-free (C21-34).
#[allow(unused_unsafe)]
fn detect_x86() -> CpuInfo {
    use std::arch::x86_64::__cpuid;

    let leaf0 = unsafe { __cpuid(0) };
    let kind = vendor_kind_from_words(leaf0.ebx, leaf0.ecx, leaf0.edx);
    let max_ext = unsafe { __cpuid(0x80000000).eax };
    // Read the brand up front: the Intel generation resolution consults
    // it to disambiguate the shared `0xA5` Comet/Rocket model (OQ-10).
    let brand = read_brand(max_ext);

    let vendor = match kind {
        VendorKind::Amd => {
            let zen = if max_ext >= 0x80000001 {
                let eax = unsafe { __cpuid(0x80000001).eax };
                // Kernel-compatible family decode: 4-bit field + 8-bit
                // extension (bits 27:20) once the field saturates at 0xF.
                amd_zen_from_family((eax >> 8) & 0xF, (eax >> 20) & 0xFF)
            } else {
                None
            };
            match zen {
                Some(z) => CpuVendor::Amd(z),
                None => CpuVendor::Unknown,
            }
        }
        VendorKind::Intel => {
            let eax = unsafe { __cpuid(1).eax };
            let family = (eax >> 8) & 0xF;
            let full_family = if family < 0xF {
                family
            } else {
                family + ((eax >> 20) & 0xFF)
            };
            let model = ((eax >> 16) & 0xF) * 16 + ((eax >> 4) & 0xF);
            let gen = if full_family == 0x6 {
                // IG-15: resolve via the brand string, not the model
                // number alone. The CPUID model field cannot split the
                // shared `0xA5` (Comet Lake 10th gen vs Rocket Lake
                // 11th gen — OQ-10); [`intel_gen_from_brand`] returns
                // the model's bucket unchanged for every unambiguous
                // model and consults the brand's marketing generation
                // only for the shared `0xA5` (11th-gen → Rocket Lake;
                // absent or non-11th-gen hint → the conservative Comet
                // Lake default).
                intel_gen_from_brand(&brand, model).unwrap_or(IntelGen::Unrecognized)
            } else {
                IntelGen::Unrecognized
            };
            CpuVendor::Intel(gen)
        }
        VendorKind::Unknown => CpuVendor::Unknown,
    };

    CpuInfo {
        vendor,
        brand,
    }
}

/// Read the 48-byte CPUID brand string (leaves `0x80000002`–`0x80000004`).
///
/// Returns an empty string when the extended leaves are absent; never panics.
#[cfg(target_arch = "x86_64")]
// `__cpuid` safety crosses stables — see the `detect_x86` note (C21-34).
#[allow(unused_unsafe)]
fn read_brand(max_ext: u32) -> String {
    use std::arch::x86_64::__cpuid;

    if max_ext < 0x80000004 {
        return String::new();
    }
    let mut buf = [0u8; 48];
    for (i, leaf) in [0x80000002u32, 0x80000003, 0x80000004].iter().enumerate() {
        let r = unsafe { __cpuid(*leaf) };
        let words = [r.eax, r.ebx, r.ecx, r.edx];
        for (j, &w) in words.iter().enumerate() {
            let off = i * 16 + j * 4;
            buf[off..off + 4].copy_from_slice(&w.to_ne_bytes());
        }
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_owned()
}

// ---------------------------------------------------------------------------
// Pure helpers (no CPUID; unit-tested in isolation).
// ---------------------------------------------------------------------------

/// Vendor kind derived from the CPUID leaf 0 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VendorKind {
    Amd,
    Intel,
    Unknown,
}

/// Map the three leaf-0 vendor u32 words to a [`VendorKind`].
///
/// The 12-byte vendor string is laid out as `EBX ‖ EDX ‖ ECX` (the `bx`,
/// `dx`, `cx` arguments in that byte order) and equals `"AuthenticAMD"` or
/// `"GenuineIntel"` for the known vendors.
fn vendor_kind_from_words(bx: u32, cx: u32, dx: u32) -> VendorKind {
    let mut v = [0u8; 12];
    v[0..4].copy_from_slice(&bx.to_ne_bytes());
    v[4..8].copy_from_slice(&dx.to_ne_bytes());
    v[8..12].copy_from_slice(&cx.to_ne_bytes());
    if v == *b"AuthenticAMD" {
        VendorKind::Amd
    } else if v == *b"GenuineIntel" {
        VendorKind::Intel
    } else {
        VendorKind::Unknown
    }
}

/// Map an AMD CPUID family to an [`AmdZen`] generation.
///
/// `family` is the 4-bit family field (EAX bits 11:8) and `ext_family` the
/// 8-bit extension (EAX bits 27:20) from leaf `0x80000001`. The full family
/// is `family + ext_family` when `family == 0xF`, else `family`.
///
/// Family map: `0x15`→Zen 1, `0x17`→Zen 2, `0x19`→Zen 3 (5950X),
/// `0x1A`→Zen 4, `0x1C`→Zen 5; anything else is `None`.
fn amd_zen_from_family(family: u32, ext_family: u32) -> Option<AmdZen> {
    let full = if family < 0xF { family } else { family + (ext_family & 0xFF) };
    match full {
        0x15 => Some(AmdZen::Zen1),
        0x17 => Some(AmdZen::Zen2),
        0x19 => Some(AmdZen::Zen3),
        0x1A => Some(AmdZen::Zen4),
        0x1C => Some(AmdZen::Zen5),
        _ => None,
    }
}

/// Map an Intel (family 6) CPUID model number to an [`IntelGen`].
///
/// Best-effort table covering Skylake through Arrow Lake; P2-07 refines the
/// per-generation IMC register offset tables. `None` → [`IntelGen::Unrecognized`].
///
/// **OQ (IG-10): the `0xA5` Comet Lake / Rocket Lake split is unresolved
/// with this helper's model-only input.** Every Rocket Lake (11th-gen
/// desktop) SKU reports model `0xA5` — the same as Comet Lake — and the
/// hardware distinguishes the two by stepping/microcode, neither of which
/// reaches this function (the steppings overlap, e.g. `0x2`/`0x4`; the
/// microcode revision is an MSR, not a CPUID field). No model number is
/// unambiguously Rocket Lake, so per the Director decision (OQ-1, option
/// (a)) this is NOT guessed: `0xA5` keeps its pre-existing
/// [`IntelGen::CometLake`] bucket and [`IntelGen::RocketLake`] gains no
/// arm. Disambiguation would require plumbing stepping (and/or the CPUID
/// brand string) into this helper — a follow-up OQ, not an IG-10 change.
fn intel_gen_from_model(model: u32) -> Option<IntelGen> {
    match model {
        0x4F | 0x56 | 0x5E => Some(IntelGen::Skylake),
        0x52 | 0x8E | 0x9E => Some(IntelGen::KabyLake),
        0x8F | 0x9A | 0x9F | 0xCA | 0xCB | 0xCF | 0xD0 => Some(IntelGen::CoffeeLake),
        // OQ (IG-10): `0xA5` is shared by Comet Lake and Rocket Lake; the
        // split needs stepping/microcode, which this model-only helper
        // does not receive — it keeps the conservative Comet Lake bucket.
        0xA5 | 0xAD | 0xAF | 0xB7 | 0xC2 | 0xC5 => Some(IntelGen::CometLake),
        0x8C | 0x92 | 0x9B | 0x9C => Some(IntelGen::IceLake),
        0x88 | 0x97 | 0xA8 => Some(IntelGen::TigerLake),
        0xA6 | 0xBF | 0xC0 => Some(IntelGen::AlderLake),
        0xA7 | 0xD4 | 0xD5 => Some(IntelGen::RaptorLake),
        0xAC | 0xAA | 0xB6 => Some(IntelGen::MeteorLake),
        0x8A => Some(IntelGen::ArrowLake),
        _ => None,
    }
}

/// Extract the Intel marketing generation from a CPUID brand string.
///
/// The brand string carries the model number (e.g. `i7-11700K`,
/// `i5-10500`, `W-10905`); its leading digits encode the marketing
/// generation, a signal the CPUID model field alone does not carry.
/// This is the general distinguishing input for model-number collisions
/// (the shared `0xA5` Comet/Rocket case, OQ-10; wired for it by
/// [`intel_gen_from_brand`]):
/// - 5-digit model numbers: the first two digits (10th–14th gen,
///   e.g. `11700` → 11, `10905` → 10);
/// - 4-digit model numbers: the first digit, but only for Core
///   `i3`/`i5`/`i7`/`i9` tokens (6th–9th gen, e.g. `8700` → 8) — other
///   4-digit part numbers (Pentium `G6400`, Xeon `E-2288G`) are not
///   generation-numbered and yield no hint.
///
/// Returns `None` when the brand has no dash-digit model token (AMD
/// brands, empty strings, …) or the candidate is outside `1..=14`.
pub fn brand_gen_hint(brand: &str) -> Option<u8> {
    for token in brand.split_whitespace() {
        let Some(dash) = token.find('-') else {
            continue;
        };
        let rest = &token[dash + 1..];
        let dlen = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        if dlen == 0 {
            continue;
        }
        let digits = rest.as_bytes();
        let gen: u8 = if dlen == 5 {
            // Two-digit marketing generation (10th–14th).
            (digits[0] - b'0') * 10 + (digits[1] - b'0')
        } else if dlen == 4 {
            // Single-digit generation (6th–9th): Core i3/i5/i7/i9 only.
            let prefix = &token[..dash];
            let is_core_i = prefix.len() >= 2
                && prefix.as_bytes()[prefix.len() - 2] == b'i'
                && prefix.as_bytes()[prefix.len() - 1].is_ascii_digit();
            if !is_core_i {
                continue;
            }
            digits[0] - b'0'
        } else {
            continue;
        };
        return (1..=14).contains(&gen).then_some(gen);
    }
    None
}

/// Resolve an Intel generation from the CPUID model number, with the
/// brand string as the general distinguishing signal.
///
/// [`intel_gen_from_model`] is consulted first and its result returned
/// unchanged for every unambiguous model (the brand is ignored). The one
/// collision the model field cannot split — `0xA5`, shared by Comet Lake
/// (10th) and Rocket Lake (11th) (OQ-10) — is resolved by the brand
/// string's marketing generation: an 11th-gen brand (e.g. `i7-11700K`)
/// is Rocket Lake; any other (absent, non-Intel, 10th-gen, …) brand
/// stays the conservative Comet Lake.
///
/// This is the general mechanism: any future model-number collision is
/// disambiguated by adding a [`brand_gen_hint`] check here.
pub fn intel_gen_from_brand(brand: &str, model: u32) -> Option<IntelGen> {
    let from_model = intel_gen_from_model(model);
    if model == 0xA5 && from_model == Some(IntelGen::CometLake) {
        match brand_gen_hint(brand) {
            Some(11) => Some(IntelGen::RocketLake),
            _ => Some(IntelGen::CometLake),
        }
    } else {
        from_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the `(family_field, ext_family)` pair that decodes to `full`.
    fn fam_args(full: u32) -> (u32, u32) {
        if full < 0xF {
            (full, 0)
        } else {
            (0xF, (full - 0xF) & 0xFF)
        }
    }

    /// Split a 12-char vendor string into the `(bx, cx, dx)` u32 words.
    fn vendor_words(s: &str) -> (u32, u32, u32) {
        let b = s.as_bytes();
        let bx = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
        let dx = u32::from_ne_bytes([b[4], b[5], b[6], b[7]]);
        let cx = u32::from_ne_bytes([b[8], b[9], b[10], b[11]]);
        (bx, cx, dx)
    }

    /// (a) `detect()` on the running host returns a self-consistent
    /// `CpuInfo` without pinning a vendor: a known vendor (AMD on the
    /// reference host, Intel on CI runners) always carries a populated
    /// brand string. Vendor-neutral, so the test passes on both the AMD
    /// reference host and the Intel CI runners (CI portability).
    #[test]
    fn detect_returns_consistent_vendor() {
        let info = CpuInfo::detect();
        match info.vendor {
            CpuVendor::Amd(_) | CpuVendor::Intel(_) => {
                assert!(!info.brand.is_empty(), "brand should be populated: {info:?}");
            }
            CpuVendor::Unknown => {
                // An unrecognized vendor: no brand claim to verify.
            }
        }
    }

    /// (b) The pure AMD family → [`AmdZen`] map, exercised with synthetic
    /// families (keyed by the family the hardware actually reports).
    #[test]
    fn amd_zen_family_map() {
        let zen = |full: u32| {
            let (f, e) = fam_args(full);
            amd_zen_from_family(f, e)
        };
        assert_eq!(zen(0x15), Some(AmdZen::Zen1));
        assert_eq!(zen(0x17), Some(AmdZen::Zen2));
        assert_eq!(zen(0x19), Some(AmdZen::Zen3));
        assert_eq!(zen(0x1A), Some(AmdZen::Zen4));
        assert_eq!(zen(0x1C), Some(AmdZen::Zen5));
        // Unrecognized families (incl. the clean-numbering gaps) map to None.
        assert_eq!(zen(0x16), None);
        assert_eq!(zen(0x18), None);
        assert_eq!(zen(0x06), None); // legacy (non-Zen)
    }

    /// (c) The pure vendor-string helper maps the three leaf-0 u32 words.
    #[test]
    fn vendor_string_map() {
        let (bx, cx, dx) = vendor_words("AuthenticAMD");
        assert_eq!(vendor_kind_from_words(bx, cx, dx), VendorKind::Amd);
        let (bx, cx, dx) = vendor_words("GenuineIntel");
        assert_eq!(vendor_kind_from_words(bx, cx, dx), VendorKind::Intel);
        let (bx, cx, dx) = vendor_words("NotARealCPU!");
        assert_eq!(vendor_kind_from_words(bx, cx, dx), VendorKind::Unknown);
    }

    /// (d) `detect()` is total: it never panics and always returns a value.
    #[test]
    fn detect_never_panics() {
        let info = CpuInfo::detect();
        assert!(matches!(
            info.vendor,
            CpuVendor::Amd(_) | CpuVendor::Intel(_) | CpuVendor::Unknown
        ));
    }

    /// (P3-02) The CPUID types are wire-serializable: the live
    /// `detect()` snapshot (vendor + generation + brand) round-trips
    /// through bincode (the Phase 3 frame codec, plan D3) and
    /// compares equal. The host is self-consistent, so equality holds
    /// for any vendor/generation combination.
    #[test]
    fn cpu_info_bincode_round_trip() {
        let info = CpuInfo::detect();
        let bytes = bincode::serialize(&info)
            .expect("CpuInfo must serialize (no-panic contract)");
        let back: CpuInfo =
            bincode::deserialize(&bytes).expect("CpuInfo must deserialize");
        assert_eq!(info, back);
    }

    /// (e) OQ-10 wire pinning (IG-10): appending [`IntelGen::RocketLake`]
    /// after [`IntelGen::Unrecognized`] (IG-01) must not shift any
    /// pre-existing variant's bincode encoding. bincode 1.x encodes a
    /// unit-variant enum as its u32 discriminant, little-endian (4
    /// bytes); each pre-existing variant serializes to exactly its
    /// original discriminant (Skylake=0 … ArrowLake=9,
    /// Unrecognized=10) and the appended Rocket Lake lands at 11 — the
    /// append-only OQ-10 rule, byte-for-byte.
    #[test]
    fn intel_gen_bincode_byte_pinning_oq10() {
        let pre_existing: [(IntelGen, u32); 11] = [
            (IntelGen::Skylake, 0),
            (IntelGen::KabyLake, 1),
            (IntelGen::CoffeeLake, 2),
            (IntelGen::CometLake, 3),
            (IntelGen::IceLake, 4),
            (IntelGen::TigerLake, 5),
            (IntelGen::AlderLake, 6),
            (IntelGen::RaptorLake, 7),
            (IntelGen::MeteorLake, 8),
            (IntelGen::ArrowLake, 9),
            (IntelGen::Unrecognized, 10),
        ];
        for (gen, expected) in pre_existing {
            let bytes =
                bincode::serialize(&gen).expect("IntelGen must serialize (OQ-10 pin)");
            assert_eq!(
                bytes,
                expected.to_le_bytes().to_vec(),
                "{gen:?} wire encoding must be unchanged (OQ-10)"
            );
        }
        // The appended variant occupies the next discriminant only.
        let bytes = bincode::serialize(&IntelGen::RocketLake)
            .expect("IntelGen::RocketLake must serialize (OQ-10 pin)");
        assert_eq!(bytes, 11u32.to_le_bytes().to_vec());
    }

    /// (f) The pre-existing model → generation table is pinned
    /// bucket-by-bucket: IG-10 moves no model out of any existing bucket
    /// (and widens no bucket — unlisted models stay `None`).
    #[test]
    fn intel_gen_from_model_full_table() {
        let table: &[(u32, IntelGen)] = &[
            (0x4F, IntelGen::Skylake),
            (0x56, IntelGen::Skylake),
            (0x5E, IntelGen::Skylake),
            (0x52, IntelGen::KabyLake),
            (0x8E, IntelGen::KabyLake),
            (0x9E, IntelGen::KabyLake),
            (0x8F, IntelGen::CoffeeLake),
            (0x9A, IntelGen::CoffeeLake),
            (0x9F, IntelGen::CoffeeLake),
            (0xCA, IntelGen::CoffeeLake),
            (0xCB, IntelGen::CoffeeLake),
            (0xCF, IntelGen::CoffeeLake),
            (0xD0, IntelGen::CoffeeLake),
            (0xA5, IntelGen::CometLake),
            (0xAD, IntelGen::CometLake),
            (0xAF, IntelGen::CometLake),
            (0xB7, IntelGen::CometLake),
            (0xC2, IntelGen::CometLake),
            (0xC5, IntelGen::CometLake),
            (0x8C, IntelGen::IceLake),
            (0x92, IntelGen::IceLake),
            (0x9B, IntelGen::IceLake),
            (0x9C, IntelGen::IceLake),
            (0x88, IntelGen::TigerLake),
            (0x97, IntelGen::TigerLake),
            (0xA8, IntelGen::TigerLake),
            (0xA6, IntelGen::AlderLake),
            (0xBF, IntelGen::AlderLake),
            (0xC0, IntelGen::AlderLake),
            (0xA7, IntelGen::RaptorLake),
            (0xD4, IntelGen::RaptorLake),
            (0xD5, IntelGen::RaptorLake),
            (0xAC, IntelGen::MeteorLake),
            (0xAA, IntelGen::MeteorLake),
            (0xB6, IntelGen::MeteorLake),
            (0x8A, IntelGen::ArrowLake),
        ];
        for &(model, gen) in table {
            assert_eq!(
                intel_gen_from_model(model),
                Some(gen),
                "model {model:#04x} bucket moved"
            );
        }
        // No widening: unlisted models stay unrecognized.
        assert_eq!(intel_gen_from_model(0x00), None);
        assert_eq!(intel_gen_from_model(0xFF), None);
    }

    /// (g) OQ (IG-10): the `0xA5` Comet Lake / Rocket Lake split. `0xA5`
    /// is reported by BOTH generations; the hardware split is by
    /// stepping/microcode, which `intel_gen_from_model` does not receive
    /// (model-only input), so it is deliberately NOT guessed: `0xA5`
    /// keeps its pre-existing Comet Lake bucket, and `0x9A` (genuine
    /// Coffee Lake, 8th/9th-gen desktop) is not re-bucketed to Rocket
    /// Lake (Director, OQ-1 option (a)). Since IG-15 the split IS
    /// resolved — via `intel_gen_from_brand` (the brand string's
    /// 11th-gen marketing generation, now plumbed into `detect()`), not
    /// via this model-only function, which stays unchanged below.
    #[test]
    fn rocket_lake_oq_0xa5_split_unresolvable_from_model() {
        // Conservative: the shared model keeps its pre-existing bucket.
        assert_eq!(intel_gen_from_model(0xA5), Some(IntelGen::CometLake));
        // Coffee Lake is never mislabeled Rocket Lake.
        assert_eq!(intel_gen_from_model(0x9A), Some(IntelGen::CoffeeLake));
        // No model number is unambiguously Rocket Lake, so no 8-bit CPUID
        // model value maps to [`IntelGen::RocketLake`] (exhaustive over
        // the model field); the variant stays unreachable through this
        // model-only function — since IG-15 `detect()` resolves the
        // shared `0xA5` via `intel_gen_from_brand` (the brand string),
        // not via this one.
        for m in 0..=0xFFu32 {
            assert_ne!(intel_gen_from_model(m), Some(IntelGen::RocketLake));
        }
    }

    /// (h) `brand_gen_hint` (IG-14): the brand string's model-number
    /// token as the marketing-generation distinguishing signal.
    #[test]
    fn brand_gen_hint_parses_model_number_tokens() {
        assert_eq!(
            brand_gen_hint("Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz"),
            Some(11)
        );
        assert_eq!(
            brand_gen_hint("Intel(R) Core(TM) i5-10500 CPU @ 3.10GHz"),
            Some(10)
        );
        assert_eq!(
            brand_gen_hint("Intel(R) Xeon(R) W-10905 CPU @ 3.20GHz"),
            Some(10)
        );
        // Pentium 4-digit part number: not a Core generation number.
        assert_eq!(
            brand_gen_hint("Intel(R) Pentium(R) Gold G6400 CPU @ 4.00GHz"),
            None
        );
        // Non-Intel brand: no dash-digit model token.
        assert_eq!(brand_gen_hint("AMD Ryzen 9 5950X"), None);
        // Empty brand.
        assert_eq!(brand_gen_hint(""), None);
        // Compact forms (no "Intel(R)" prefix).
        assert_eq!(brand_gen_hint("i9-10900K"), Some(10));
        assert_eq!(brand_gen_hint("i7-12700"), Some(12));
        assert_eq!(brand_gen_hint("i5-13600K"), Some(13));
        // Xeon E 4-digit part number: no hint.
        assert_eq!(brand_gen_hint("E-2288G"), None);
        // Single-digit generation (8th) from a 4-digit Core model.
        assert_eq!(brand_gen_hint("i7-8700"), Some(8));
    }

    /// (i) `intel_gen_from_brand` (IG-14): the general
    /// distinguishing-signal resolver — the model number always wins for
    /// unambiguous models; only the shared `0xA5` consults the brand.
    #[test]
    fn intel_gen_from_brand_resolves_the_shared_0xa5() {
        // 0xA5 + 11th-gen brand → Rocket Lake.
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0xA5),
            Some(IntelGen::RocketLake)
        );
        // 0xA5 + 10th-gen brand → Comet Lake.
        assert_eq!(
            intel_gen_from_brand("i7-10700", 0xA5),
            Some(IntelGen::CometLake)
        );
        // 0xA5 + empty brand → conservative Comet Lake default.
        assert_eq!(
            intel_gen_from_brand("", 0xA5),
            Some(IntelGen::CometLake)
        );
        // 0xA5 + non-Intel brand → conservative Comet Lake default.
        assert_eq!(
            intel_gen_from_brand("AMD Ryzen 9 5950X", 0xA5),
            Some(IntelGen::CometLake)
        );
        // Unambiguous models: the brand is ignored, the model wins.
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0xA6),
            Some(IntelGen::AlderLake)
        );
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0x9A),
            Some(IntelGen::CoffeeLake)
        );
        // Unlisted model → None regardless of brand.
        assert_eq!(intel_gen_from_brand("i7-11700K", 0x00), None);
    }
}
