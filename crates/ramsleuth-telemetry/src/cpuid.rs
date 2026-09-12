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
//! - Intel family/model from leaf 1 → [`IntelGen`].
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
fn detect_x86() -> CpuInfo {
    use std::arch::x86_64::__cpuid;

    let leaf0 = __cpuid(0);
    let kind = vendor_kind_from_words(leaf0.ebx, leaf0.ecx, leaf0.edx);
    let max_ext = __cpuid(0x80000000).eax;

    let vendor = match kind {
        VendorKind::Amd => {
            let zen = if max_ext >= 0x80000001 {
                let eax = __cpuid(0x80000001).eax;
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
            let eax = __cpuid(1).eax;
            let family = (eax >> 8) & 0xF;
            let full_family = if family < 0xF {
                family
            } else {
                family + ((eax >> 20) & 0xFF)
            };
            let model = ((eax >> 16) & 0xF) * 16 + ((eax >> 4) & 0xF);
            let gen = if full_family == 0x6 {
                intel_gen_from_model(model).unwrap_or(IntelGen::Unrecognized)
            } else {
                IntelGen::Unrecognized
            };
            CpuVendor::Intel(gen)
        }
        VendorKind::Unknown => CpuVendor::Unknown,
    };

    CpuInfo {
        vendor,
        brand: read_brand(max_ext),
    }
}

/// Read the 48-byte CPUID brand string (leaves `0x80000002`–`0x80000004`).
///
/// Returns an empty string when the extended leaves are absent; never panics.
#[cfg(target_arch = "x86_64")]
fn read_brand(max_ext: u32) -> String {
    use std::arch::x86_64::__cpuid;

    if max_ext < 0x80000004 {
        return String::new();
    }
    let mut buf = [0u8; 48];
    for (i, leaf) in [0x80000002u32, 0x80000003, 0x80000004].iter().enumerate() {
        let r = __cpuid(*leaf);
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
fn intel_gen_from_model(model: u32) -> Option<IntelGen> {
    match model {
        0x4F | 0x56 | 0x5E => Some(IntelGen::Skylake),
        0x52 | 0x8E | 0x9E => Some(IntelGen::KabyLake),
        0x8F | 0x9A | 0x9F | 0xCA | 0xCB | 0xCF | 0xD0 => Some(IntelGen::CoffeeLake),
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

    /// (a) `detect()` on the reference host (Ryzen 9 5950X, Zen 3) reports
    /// vendor `Amd` and generation [`AmdZen::Zen3`].
    #[test]
    fn detect_this_host_is_amd_zen3() {
        let info = CpuInfo::detect();
        assert_eq!(info.vendor, CpuVendor::Amd(AmdZen::Zen3));
        assert!(!info.brand.is_empty(), "brand should be populated");
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
}
