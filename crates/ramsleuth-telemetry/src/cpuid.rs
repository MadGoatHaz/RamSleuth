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
//! - Intel family/model from leaf 1 → [`IntelGen`]; the brand string
//!   resolves every model-number collision (research §2: the
//!   `0x8E`/`0x9E`/`0xA5`/`0x97`/`0x9A`/`0xBA` marketing-generation
//!   splits, OQ-10).
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

/// The stable, human-readable name of an AMD Zen generation (DAEMON-2):
/// the single source for the `cpu_gen` wire string, the TUI/GUI/CLI
/// display, and the probe report. A variant rename now changes a
/// **pinned** string (the `TC-DAEMON2` byte-pin), not a silent `Debug`
/// drift. Display-only — the wire encoding (bincode discriminant) is
/// unchanged.
impl std::fmt::Display for AmdZen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Zen1 => "Zen 1",
            Self::Zen2 => "Zen 2",
            Self::Zen3 => "Zen 3",
            Self::Zen4 => "Zen 4",
            Self::Zen5 => "Zen 5",
        };
        f.write_str(name)
    }
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
    /// 4th gen Xeon Scalable (Sapphire Rapids, 2023) — server /
    /// workstation (models `0x8F`).
    ///
    /// Appended after [`Self::RocketLake`] (OQ-10 append-only, index 12).
    /// The coarse pre-fix table mis-bucketed `0x8F` to Coffee Lake; it
    /// has no [`crate::intel_gen::profile_for`] entry (honest `N/A`).
    SapphireRapids,
    /// Jasper Lake (Atom Tremont client, 2021, models `0x9C`).
    ///
    /// Appended after [`Self::SapphireRapids`] (OQ-10 append-only,
    /// index 13). The pre-fix table mis-bucketed `0x9C` to Ice Lake; it
    /// has no [`crate::intel_gen::profile_for`] entry (honest `N/A`).
    JasperLake,
    /// Core Ultra 2 (Lunar Lake, 2024) — Mobile on Package (`0xBD`).
    ///
    /// Appended after [`Self::JasperLake`] (OQ-10 append-only, index 14).
    /// Distinct from [`Self::ArrowLake`]: the brand string alone cannot
    /// split the two Series-2 lines, so the model field (`0xBD`) does.
    /// No [`crate::intel_gen::profile_for`] entry (honest `N/A`).
    LunarLake,
    /// Core Series 1 Mobile (non-Ultra, 2024) — the `Core N processor
    /// NNN` refresh line on the Raptor Lake-P / Alder Lake-P dies
    /// (`0xBA` / `0x9A`).
    ///
    /// Appended after [`Self::LunarLake`] (OQ-10 append-only, index 15).
    /// Distinguished from the `iN-` legacy line by the dropped `i`
    /// prefix + `Ultra` token; no [`crate::intel_gen::profile_for`]
    /// entry (honest `N/A`).
    CoreSeries1Mobile,
}

/// The stable, human-readable name of an Intel generation (DAEMON-2):
/// the single source for the `cpu_gen` wire string, the TUI/GUI/CLI
/// display, and the probe report. A variant rename now changes a
/// **pinned** string (the `TC-DAEMON2` byte-pin), not a silent `Debug`
/// drift. Display-only — the wire encoding (bincode discriminant) is
/// unchanged.
impl std::fmt::Display for IntelGen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Skylake => "Skylake",
            Self::KabyLake => "Kaby Lake",
            Self::CoffeeLake => "Coffee Lake",
            Self::CometLake => "Comet Lake",
            Self::IceLake => "Ice Lake",
            Self::TigerLake => "Tiger Lake",
            Self::AlderLake => "Alder Lake",
            Self::RaptorLake => "Raptor Lake",
            Self::MeteorLake => "Meteor Lake",
            Self::ArrowLake => "Arrow Lake",
            Self::Unrecognized => "Unrecognized",
            Self::RocketLake => "Rocket Lake",
            Self::SapphireRapids => "Sapphire Rapids",
            Self::JasperLake => "Jasper Lake",
            Self::LunarLake => "Lunar Lake",
            Self::CoreSeries1Mobile => "Core Series 1 Mobile",
        };
        f.write_str(name)
    }
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
/// The full 8th→14th-gen / Core Ultra / Xeon / Atom `DisplayModel` table
/// (research: "Intel CPUID Microarchitecture Mapping" §1) plus the
/// pre-8th-gen Tier-1 line. `None` for an unlisted model →
/// [`IntelGen::Unrecognized`] via [`intel_gen_from_brand`].
///
/// **This table is the *conservative* (brand-absent) assignment.** Where a
/// `DisplayModel` collides across marketing generations (the 7th/8th/10th
/// `0x8E`, the 7th/8th/9th `0x9E`, the 10th `0xA5`, the 12th/13th `0x97`
/// and `0x9A`, the 13th/14th `0xB7` / `0xBA` / `0xBF`), the brand
/// string's marketing generation breaks the tie in
/// [`intel_gen_from_brand`] — the model table alone cannot, and
/// deliberately does not, guess (OQ-10).
///
/// **ADL P/E CPUID incoherence (research §4):** pre-release Alder Lake
/// engineering samples reported *different* Leaf 0x1 models on P-cores
/// (`0x97`) vs. E-cores (`0xBE` / Atom `0x20`), which broke DRM.
/// Production microcode unifies Leaf 0x1 across every thread in the socket
/// (both core clusters report `0x97` for Alder Lake-S, `0x9A` for -P, …),
/// so the model field this function reads is coherent on all retail
/// silicon. The per-core native type is available on CPUID Leaf 0x1A
/// (hybrid sub-leaf: core type `0x20` = Atom / `0x40` = Core) if a future
/// chunk needs per-core classification — the base generation resolved here
/// does **not** depend on it.
fn intel_gen_from_model(model: u32) -> Option<IntelGen> {
    match model {
        // Pre-8th / 6th-gen Tier-1 line (Skylake core; `0x52` Kaby Lake
        // folds to Skylake — the 64 KiB IMC base is unchanged).
        0x3F | 0x4F | 0x52 | 0x56 | 0x5E => Some(IntelGen::Skylake),
        // 7th/8th mobile (Kaby Lake line) — brand refines (0x8E).
        0x8E => Some(IntelGen::KabyLake),
        // 8th/9th desktop (Coffee Lake line) + Cannon Lake 8th — brand
        // refines (0x9E).
        0x9E | 0x66 => Some(IntelGen::CoffeeLake),
        // 10th Ice Lake.
        0x7E => Some(IntelGen::IceLake),
        // 10th Comet Lake (`0xA6` was Alder Lake → Comet Lake).
        0xA5 | 0xAD | 0xAF | 0xA6 => Some(IntelGen::CometLake),
        // 11th Tiger Lake (`0x8C` TGL-U was Ice Lake → Tiger Lake;
        // `0x8D` TGL-H/B is new).
        0x8C | 0x8D => Some(IntelGen::TigerLake),
        // 11th Rocket Lake (was Raptor Lake).
        0xA7 => Some(IntelGen::RocketLake),
        // 12th Alder Lake (`0x97` ADL-S was Tiger Lake → Alder Lake [THE
        // FIX]; `0x9A` ADL-P was Coffee Lake; `0xBE` ADL-N is new).
        0x97 | 0x9A | 0xBE => Some(IntelGen::AlderLake),
        // 13th/14th Raptor Lake (`0xB7` was Comet Lake; `0xBA` new;
        // `0xBF` harvested ADL was Alder Lake).
        0xB7 | 0xBA | 0xBF => Some(IntelGen::RaptorLake),
        // Core Ultra 1 (Meteor Lake).
        0xAC | 0xAA => Some(IntelGen::MeteorLake),
        // Core Ultra 2 (Arrow Lake; removes the bogus 0x8A placeholder).
        0xC6 | 0xC5 | 0xB5 => Some(IntelGen::ArrowLake),
        // Core Ultra 2 Mobile-on-Package (Lunar Lake).
        0xBD => Some(IntelGen::LunarLake),
        // Xeon Scalable / W (Sapphire Rapids; was Coffee Lake).
        0x8F => Some(IntelGen::SapphireRapids),
        // Atom Tremont client (Jasper Lake; was Ice Lake).
        0x9C => Some(IntelGen::JasperLake),
        // ES / prototype silicon (were Ice Lake / Coffee Lake / Alder Lake).
        0x9B | 0x9F | 0xC0 => Some(IntelGen::Unrecognized),
        _ => None,
    }
}

/// The platform tier a brand-string model token belongs to (research §3).
///
/// The naming schemes a single CPUID model field cannot distinguish;
/// [`brand_parse`] routes a brand string to exactly one of these, in the
/// research's cascading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandTier {
    /// Legacy Core `iN-NNxxx` (7th–14th gen desktop + mobile).
    LegacyCore,
    /// Core Ultra `N NNN[SFX]` (Series 1 Meteor Lake / Series 2 Arrow &
    /// Lunar Lake) — the `i` prefix is dropped.
    CoreUltra,
    /// Non-Ultra Core Series 1 `N processor NNN[SFX]` — drops both the
    /// `i` and the `Ultra` token.
    CoreSeries1,
    /// Xeon W workstation `W-NNNN`.
    XeonW,
    /// Xeon Scalable server `Tier NNNN` (the generation is the 2nd
    /// digit of the 4-digit SKU).
    XeonScalable,
    /// Entry N-series `N100` / `N200` (no generation number).
    NSeries,
}

/// A parsed brand-string token: marketing generation + tier + suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandToken {
    /// The marketing generation: 7–14 for the legacy / Core lines, 1–2
    /// for the Core Ultra / Series-1 lines, `0` for the N-series (which
    /// carries no generation number).
    pub gen: u8,
    /// The platform tier the token belongs to.
    pub tier: BrandTier,
    /// The trailing alphabetic suffix (e.g. `V` for Lunar Lake,
    /// `H`/`HK` for mobile, `K`/`KF` for desktop). Empty when absent.
    pub suffix: String,
}

/// Index of the first token equal to `needle` (case-insensitive).
fn token_index(toks: &[&str], needle: &str) -> Option<usize> {
    toks.iter().position(|t| t.eq_ignore_ascii_case(needle))
}

/// 1. Core Ultra `N NNN[SFX]` (Series 1 / 2) — e.g. `Ultra 7 155H`.
fn parse_core_ultra(toks: &[&str]) -> Option<BrandToken> {
    let i = token_index(toks, "Ultra")?;
    if i + 2 >= toks.len() {
        return None;
    }
    let tier = toks[i + 1].as_bytes();
    if tier.len() != 1 || !matches!(tier[0], b'5' | b'7' | b'9') {
        return None;
    }
    let sku = toks[i + 2].as_bytes();
    if sku.len() < 3 {
        return None;
    }
    let gen = sku[0] - b'0';
    if !matches!(gen, 1 | 2) || !sku[1].is_ascii_digit() || !sku[2].is_ascii_digit() {
        return None;
    }
    let suffix: String = sku[3..]
        .iter()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|b| *b as char)
        .collect();
    Some(BrandToken { gen, tier: BrandTier::CoreUltra, suffix })
}

/// 2. Non-Ultra Core Series 1 `N processor NNN[SFX]` — e.g.
/// `Core 7 processor 150U`.
fn parse_core_series1(toks: &[&str]) -> Option<BrandToken> {
    let i = token_index(toks, "processor")?;
    if i == 0 || i + 1 >= toks.len() {
        return None;
    }
    let tier = toks[i - 1].as_bytes();
    if tier.len() != 1 || !matches!(tier[0], b'3' | b'5' | b'7') {
        return None;
    }
    let sku = toks[i + 1].as_bytes();
    if sku.len() < 3 {
        return None;
    }
    let gen = sku[0] - b'0';
    if !matches!(gen, 1 | 2) || !sku[1].is_ascii_digit() || !sku[2].is_ascii_digit() {
        return None;
    }
    let suffix: String = sku[3..]
        .iter()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|b| *b as char)
        .collect();
    Some(BrandToken { gen, tier: BrandTier::CoreSeries1, suffix })
}

/// 3. Legacy Core `iN-NNxxx` — e.g. `i7-11700K`, `i7-8700`, `i7-1165G7`.
fn parse_legacy_core(toks: &[&str]) -> Option<BrandToken> {
    for tok in toks {
        let tb = tok.as_bytes();
        if tb.len() < 3 || tb[0] != b'i' || !tb[1].is_ascii_digit() || tb[2] != b'-' {
            continue;
        }
        let rest = &tb[3..];
        let dlen = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if dlen < 4 {
            continue;
        }
        // 5-digit SKU → two-digit marketing gen (10th–14th), e.g.
        // `11700` → 11. A 4-digit SKU → one digit (6th–9th, e.g. `8700`
        // → 8) UNLESS it starts with `1`, in which case it is a two-digit
        // mobile gen (10th–14th, e.g. `1165G7` → 11, `1335U` → 13).
        let gen: u8 = if dlen == 5 || rest[0] == b'1' {
            (rest[0] - b'0') * 10 + (rest[1] - b'0')
        } else {
            rest[0] - b'0'
        };
        let suffix: String = rest[dlen..]
            .iter()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|b| *b as char)
            .collect();
        return Some(BrandToken { gen, tier: BrandTier::LegacyCore, suffix });
    }
    None
}

/// 4. Xeon W workstation `W-NNNN` — e.g. `W-1290P` (gen = first two
/// digits). Client-era `W-10xxx`/`W-11xxx` are 10th/11th gen; the
/// `W-24xx`/`W-34xx` platform-tier SKUs yield `>14` and are clamped away
/// by [`brand_gen_hint`].
fn parse_xeon_w(toks: &[&str]) -> Option<BrandToken> {
    for tok in toks {
        let tb = tok.as_bytes();
        if tb.len() < 3 || tb[0] != b'W' || tb[1] != b'-' {
            continue;
        }
        let rest = &tb[2..];
        let dlen = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if dlen < 4 {
            continue;
        }
        let gen = (rest[0] - b'0') * 10 + (rest[1] - b'0');
        let suffix: String = rest[dlen..]
            .iter()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|b| *b as char)
            .collect();
        return Some(BrandToken { gen, tier: BrandTier::XeonW, suffix });
    }
    None
}

/// 5. Xeon Scalable server `Tier NNNN` — e.g. `Platinum 8480+` (gen =
/// the 2nd digit of the 4-digit SKU).
fn parse_xeon_scalable(toks: &[&str]) -> Option<BrandToken> {
    let i = toks
        .iter()
        .position(|t| matches!(*t, "Platinum" | "Gold" | "Silver" | "Bronze"))?;
    if i + 1 >= toks.len() {
        return None;
    }
    let sku = toks[i + 1].as_bytes();
    let dlen = sku.iter().take_while(|b| b.is_ascii_digit()).count();
    if dlen != 4 {
        return None;
    }
    let gen = sku[1] - b'0';
    let suffix: String = sku[dlen..]
        .iter()
        .copied()
        .filter(|c| c.is_ascii_alphabetic() || *c == b'+')
        .map(char::from)
        .collect();
    Some(BrandToken { gen, tier: BrandTier::XeonScalable, suffix })
}

/// 6. Entry N-series `N100` / `N200` — no generation number.
fn parse_n_series(toks: &[&str]) -> Option<BrandToken> {
    for tok in toks {
        let tb = tok.as_bytes();
        if tb.len() == 4
            && tb[0] == b'N'
            && tb[1].is_ascii_digit()
            && tb[2].is_ascii_digit()
            && tb[3].is_ascii_digit()
        {
            return Some(BrandToken {
                gen: 0,
                tier: BrandTier::NSeries,
                suffix: String::new(),
            });
        }
    }
    None
}

/// Parse a CPUID brand string into a [`BrandToken`], routing through the
/// research §3 five-alternative cascade (Core Ultra → Core Series 1 →
/// legacy `iN-` → Xeon W → Xeon Scalable → N-series) and returning the
/// first match.
///
/// Returns `None` for engineering samples (`Intel(R) Genuine Processor`,
/// `Intel(R) 0000`), non-Intel brands, and empty strings — never panics.
/// Callers fall back to the model table's conservative assignment
/// ([`intel_gen_from_brand`]).
pub fn brand_parse(brand: &str) -> Option<BrandToken> {
    let toks: Vec<&str> = brand.split_whitespace().collect();
    parse_core_ultra(&toks)
        .or_else(|| parse_core_series1(&toks))
        .or_else(|| parse_legacy_core(&toks))
        .or_else(|| parse_xeon_w(&toks))
        .or_else(|| parse_xeon_scalable(&toks))
        .or_else(|| parse_n_series(&toks))
}

/// Extract the Intel marketing generation from a CPUID brand string.
///
/// Thin wrapper over [`brand_parse`] (the research §3 five-alternative
/// cascade): returns the parsed token's `gen`, clamped to `1..=14` (the
/// N-series token carries `gen = 0` and therefore yields `None`). This is
/// the general distinguishing input for model-number collisions (OQ-10),
/// wired through [`intel_gen_from_brand`]:
/// - 5-digit model numbers: the first two digits (10th–14th gen,
///   e.g. `11700` → 11, `10905` → 10);
/// - 4-digit model numbers: the first digit for desktop (6th–9th, e.g.
///   `8700` → 8) or two digits when the SKU starts with `1` (10th–14th
///   mobile, e.g. `1335U` → 13);
/// - other 4-digit part numbers (Pentium `G6400`, Xeon `E-2288G`) and
///   non-Intel brands carry no generation and yield no hint.
///
/// Returns `None` when the brand has no parseable model token or the
/// candidate is outside `1..=14`.
pub fn brand_gen_hint(brand: &str) -> Option<u8> {
    brand_parse(brand).and_then(|t| (1..=14).contains(&t.gen).then_some(t.gen))
}

/// Resolve an Intel generation from the CPUID model number, with the
/// brand string as the general distinguishing signal.
///
/// [`intel_gen_from_model`] is consulted first and its result returned
/// unchanged for every non-colliding model (the brand is ignored). The
/// colliding models — those whose 8-bit signature spans several marketing
/// generations (research §2) — are resolved by the brand string's
/// marketing generation via [`brand_parse`]:
///
/// | model | brand gen        | resolved [`IntelGen`]                 |
/// |-------|------------------|---------------------------------------|
/// | 0x8E  | 7 / 8 / 10 / —   | Kaby / Coffee / Comet / Kaby          |
/// | 0x9E  | 7 / —           | Kaby / Coffee                         |
/// | 0xA5  | 11 / —          | Rocket / Comet                        |
/// | 0x97  | 13 / —          | Raptor / **Alder** [the i5-12600KF fix]|
/// | 0x9A  | S1 / 13 / —     | CoreSeries1Mobile / Raptor / Alder    |
/// | 0xBA  | S1 / —          | CoreSeries1Mobile / Raptor            |
///
/// (`—` = absent / unparseable brand → the model table's conservative
/// default; `S1` = the non-Ultra Core Series-1 naming.) The remaining
/// models (0x8C/0x8D Tiger Lake, 0xBE Alder Lake-N, 0xB7/0xBF Raptor
/// Lake, 0x8F Sapphire Rapids, 0x9C Jasper Lake, 0x9B/0x9F/0xC0
/// prototype) are non-colliding at this coarse granularity and keep the
/// model table's assignment. This is the general mechanism: any future
/// model-number collision is disambiguated by adding an arm here.
pub fn intel_gen_from_brand(brand: &str, model: u32) -> Option<IntelGen> {
    let from_model = intel_gen_from_model(model);
    let tok = brand_parse(brand);
    match model {
        // --- Colliding models: the brand string resolves the tie ---
        0x8E => match tok.as_ref().map(|t| t.gen) {
            Some(7) => Some(IntelGen::KabyLake),
            Some(8) => Some(IntelGen::CoffeeLake),
            Some(10) => Some(IntelGen::CometLake),
            _ => from_model, // 7th/8th-mobile default: Kaby Lake
        },
        0x9E => match tok.as_ref().map(|t| t.gen) {
            Some(7) => Some(IntelGen::KabyLake),
            _ => from_model, // 8th/9th / Xeon E default: Coffee Lake
        },
        0xA5 => match tok.as_ref().map(|t| t.gen) {
            Some(11) => Some(IntelGen::RocketLake),
            _ => from_model, // 10th default: Comet Lake
        },
        0x97 => match tok.as_ref().map(|t| t.gen) {
            Some(13) => Some(IntelGen::RaptorLake),
            _ => from_model, // 12th default: Alder Lake [THE i5-12600KF FIX]
        },
        0x9A => {
            if matches!(tok, Some(BrandToken { tier: BrandTier::CoreSeries1, .. })) {
                Some(IntelGen::CoreSeries1Mobile)
            } else {
                match tok.as_ref().map(|t| t.gen) {
                    Some(13) => Some(IntelGen::RaptorLake),
                    _ => from_model, // 12th default: Alder Lake
                }
            }
        }
        0xBA => {
            if matches!(tok, Some(BrandToken { tier: BrandTier::CoreSeries1, .. })) {
                Some(IntelGen::CoreSeries1Mobile)
            } else {
                from_model // 13th default: Raptor Lake
            }
        }
        // --- Non-colliding models: the model table is authoritative ---
        _ => from_model,
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
        // Indices 0-11 are byte-pinned and must not shift (OQ-10).
        let pre_existing: [(IntelGen, u32); 12] = [
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
            (IntelGen::RocketLake, 11),
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
        // The chunk-1 appended variants occupy the next discriminants only
        // (12-15) — the append-only OQ-10 rule, byte-for-byte.
        let appended: [(IntelGen, u32); 4] = [
            (IntelGen::SapphireRapids, 12),
            (IntelGen::JasperLake, 13),
            (IntelGen::LunarLake, 14),
            (IntelGen::CoreSeries1Mobile, 15),
        ];
        for (gen, expected) in appended {
            let bytes =
                bincode::serialize(&gen).expect("IntelGen must serialize (OQ-10 pin)");
            assert_eq!(
                bytes,
                expected.to_le_bytes().to_vec(),
                "{gen:?} appended wire encoding (OQ-10)"
            );
        }
    }

    /// (f) The pre-existing model → generation table is pinned
    /// bucket-by-bucket: IG-10 moves no model out of any existing bucket
    /// (and widens no bucket — unlisted models stay `None`).
    #[test]
    fn intel_gen_from_model_full_table() {
        let table: &[(u32, IntelGen)] = &[
            // Pre-8th / 6th-gen Tier-1 line.
            (0x3F, IntelGen::Skylake),
            (0x4F, IntelGen::Skylake),
            (0x52, IntelGen::Skylake),
            (0x56, IntelGen::Skylake),
            (0x5E, IntelGen::Skylake),
            // 7th/8th mobile.
            (0x8E, IntelGen::KabyLake),
            // 8th/9th desktop + Cannon Lake 8th.
            (0x9E, IntelGen::CoffeeLake),
            (0x66, IntelGen::CoffeeLake),
            // 10th Ice Lake.
            (0x7E, IntelGen::IceLake),
            // 10th Comet Lake.
            (0xA5, IntelGen::CometLake),
            (0xAD, IntelGen::CometLake),
            (0xAF, IntelGen::CometLake),
            (0xA6, IntelGen::CometLake),
            // 11th Tiger Lake.
            (0x8C, IntelGen::TigerLake),
            (0x8D, IntelGen::TigerLake),
            // 11th Rocket Lake.
            (0xA7, IntelGen::RocketLake),
            // 12th Alder Lake (0x97 = THE i5-12600KF FIX).
            (0x97, IntelGen::AlderLake),
            (0x9A, IntelGen::AlderLake),
            (0xBE, IntelGen::AlderLake),
            // 13th/14th Raptor Lake.
            (0xB7, IntelGen::RaptorLake),
            (0xBA, IntelGen::RaptorLake),
            (0xBF, IntelGen::RaptorLake),
            // Core Ultra 1 (Meteor Lake).
            (0xAC, IntelGen::MeteorLake),
            (0xAA, IntelGen::MeteorLake),
            // Core Ultra 2 (Arrow Lake).
            (0xC6, IntelGen::ArrowLake),
            (0xC5, IntelGen::ArrowLake),
            (0xB5, IntelGen::ArrowLake),
            // Core Ultra 2 MoP (Lunar Lake).
            (0xBD, IntelGen::LunarLake),
            // Xeon SP / W (Sapphire Rapids).
            (0x8F, IntelGen::SapphireRapids),
            // Atom Tremont client (Jasper Lake).
            (0x9C, IntelGen::JasperLake),
            // ES / prototype silicon.
            (0x9B, IntelGen::Unrecognized),
            (0x9F, IntelGen::Unrecognized),
            (0xC0, IntelGen::Unrecognized),
        ];
        for &(model, gen) in table {
            assert_eq!(
                intel_gen_from_model(model),
                Some(gen),
                "model {model:#04x} bucket moved"
            );
        }
        // No widening: unlisted models stay None.
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
    fn rocket_lake_model_and_0xa5_split() {
        // 0xA7 is now the model-only Rocket Lake (11th-gen desktop).
        assert_eq!(intel_gen_from_model(0xA7), Some(IntelGen::RocketLake));
        // The shared 0xA5 keeps its conservative Comet Lake bucket at the
        // model level; the brand string resolves it in
        // `intel_gen_from_brand` (11th-gen → Rocket Lake).
        assert_eq!(intel_gen_from_model(0xA5), Some(IntelGen::CometLake));
        // Sweep: the ONLY model that maps to Rocket Lake is 0xA7.
        for m in 0..=0xFFu32 {
            if m == 0xA7 {
                assert_eq!(intel_gen_from_model(m), Some(IntelGen::RocketLake));
            } else {
                assert_ne!(
                    intel_gen_from_model(m),
                    Some(IntelGen::RocketLake),
                    "model {m:#04x} should not be Rocket Lake"
                );
            }
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

    /// (i) `intel_gen_from_brand`: the general distinguishing-signal
    /// resolver — the model number always wins for non-colliding models;
    /// the colliding models (research §2) are split by the brand
    /// string's marketing generation.
    #[test]
    fn intel_gen_from_brand_resolves_all_collisions() {
        // 0x97 [THE i5-12600KF FIX]: 12th-gen brand → Alder Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-12600KF CPU @ 3.70GHz", 0x97),
            Some(IntelGen::AlderLake)
        );
        // 0x97 + 13th-gen brand (rebadged ADL) → Raptor Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-13400 CPU @ 2.50GHz", 0x97),
            Some(IntelGen::RaptorLake)
        );
        // 0x97 + empty / non-Intel brand → conservative Alder Lake.
        assert_eq!(intel_gen_from_brand("", 0x97), Some(IntelGen::AlderLake));
        assert_eq!(
            intel_gen_from_brand("AMD Ryzen 9 5950X", 0x97),
            Some(IntelGen::AlderLake)
        );
        // Genuine Tiger Lake (0x8C) with an 11th-gen mobile brand.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-11800H CPU @ 1.90GHz", 0x8C),
            Some(IntelGen::TigerLake)
        );
        // 0x8D (Tiger Lake-H / -B, incl. the "KB" desktop BGA) → Tiger Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i9-11900KB", 0x8D),
            Some(IntelGen::TigerLake)
        );
        // 0xA5: 11th-gen → Rocket Lake; 10th-gen / absent → Comet Lake.
        assert_eq!(intel_gen_from_brand("i7-11700K", 0xA5), Some(IntelGen::RocketLake));
        assert_eq!(intel_gen_from_brand("i7-10700", 0xA5), Some(IntelGen::CometLake));
        assert_eq!(intel_gen_from_brand("", 0xA5), Some(IntelGen::CometLake));
        // 0xB7: 13th and 14th gen both → Raptor Lake (the coarse enum has
        // no distinct refresh variant).
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-13700K", 0xB7),
            Some(IntelGen::RaptorLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-14700K", 0xB7),
            Some(IntelGen::RaptorLake)
        );
        // 0xBF: harvested ADL → Raptor Lake (13th and 14th).
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-13400F", 0xBF),
            Some(IntelGen::RaptorLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-14400", 0xBF),
            Some(IntelGen::RaptorLake)
        );
        // 0x9A: Series-1 → CoreSeries1Mobile; 13th → Raptor Lake;
        // 12th → Alder Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) 3 processor 100U", 0x9A),
            Some(IntelGen::CoreSeries1Mobile)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-1335U", 0x9A),
            Some(IntelGen::RaptorLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-1240P", 0x9A),
            Some(IntelGen::AlderLake)
        );
        // 0xBA: Series-1 → CoreSeries1Mobile; else → Raptor Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) 7 processor 150U", 0xBA),
            Some(IntelGen::CoreSeries1Mobile)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-13700H", 0xBA),
            Some(IntelGen::RaptorLake)
        );
        // 0x8E: 7th → Kaby Lake; 8th → Coffee Lake; 10th → Comet Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-7200U", 0x8E),
            Some(IntelGen::KabyLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-8250U", 0x8E),
            Some(IntelGen::CoffeeLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i5-10210U", 0x8E),
            Some(IntelGen::CometLake)
        );
        // 0x9E: 7th → Kaby Lake; 8th/9th → Coffee Lake.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-7700", 0x9E),
            Some(IntelGen::KabyLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i7-8700K", 0x9E),
            Some(IntelGen::CoffeeLake)
        );
        assert_eq!(
            intel_gen_from_brand("Intel(R) Core(TM) i9-9900K", 0x9E),
            Some(IntelGen::CoffeeLake)
        );
        // Non-colliding models: the brand is ignored, the model wins.
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0x8F),
            Some(IntelGen::SapphireRapids)
        );
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0x9C),
            Some(IntelGen::JasperLake)
        );
        assert_eq!(
            intel_gen_from_brand("i7-11700K", 0xBD),
            Some(IntelGen::LunarLake)
        );
        assert_eq!(intel_gen_from_brand("i7-11700K", 0xBE), Some(IntelGen::AlderLake));
        // ES / prototype → Unrecognized.
        assert_eq!(
            intel_gen_from_brand("Intel(R) Genuine Processor", 0x9B),
            Some(IntelGen::Unrecognized)
        );
        // Unlisted model → None regardless of brand.
        assert_eq!(intel_gen_from_brand("i7-11700K", 0x00), None);
    }

    /// The reference device: an i5-12600KF (model `0x97`, Alder Lake
    /// brand) resolves to Alder Lake — which HAS a `profile_for` entry, so
    /// the Intel branch no longer degrades to `Na(UnsupportedHardware)`.
    #[test]
    fn i5_12600kf_resolves_to_alder_lake() {
        let gen = intel_gen_from_brand("Intel(R) Core(TM) i5-12600KF CPU @ 3.70GHz", 0x97);
        assert_eq!(gen, Some(IntelGen::AlderLake));
        // The branch-un-N/A proof: Alder Lake has a decode profile.
        assert!(crate::intel_gen::profile_for(IntelGen::AlderLake).is_some());
    }

    /// `brand_parse` (research §3): one case per naming scheme, plus the
    /// Series-2 Arrow/Lunar `V`-suffix split and the ES / non-Intel /
    /// empty no-match cases (never panics).
    #[test]
    fn brand_parse_five_alternatives() {
        // Core Ultra Series 1 (Meteor Lake): gen 1.
        let t = brand_parse("Intel(R) Core(TM) Ultra 7 155H").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::CoreUltra, 1, "H"));
        // Core Ultra Series 2 (Arrow Lake): gen 2, K suffix.
        let t = brand_parse("Intel(R) Core(TM) Ultra 9 285K").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::CoreUltra, 2, "K"));
        // Core Ultra Series 2 (Lunar Lake): gen 2, V suffix (MoP).
        let t = brand_parse("Intel(R) Core(TM) Ultra 7 258V").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::CoreUltra, 2, "V"));
        // Non-Ultra Core Series 1: gen 1.
        let t = brand_parse("Intel(R) Core(TM) 7 processor 150U").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::CoreSeries1, 1, "U"));
        // Legacy Core iN-NNxxx: 5-digit → two-digit gen.
        let t = brand_parse("Intel(R) Core(TM) i7-11700K CPU @ 3.60GHz").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::LegacyCore, 11, "K"));
        // Legacy 4-digit desktop → single-digit gen.
        let t = brand_parse("Intel(R) Core(TM) i7-8700").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::LegacyCore, 8, ""));
        // Legacy 4-digit mobile (starts with 1) → two-digit gen.
        let t = brand_parse("Intel(R) Core(TM) i5-1335U").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::LegacyCore, 13, "U"));
        // Xeon W workstation (gen = first two digits).
        let t = brand_parse("Intel(R) Xeon(R) W-1290P CPU @ 3.70GHz").unwrap();
        assert_eq!((t.tier, t.gen, t.suffix.as_str()), (BrandTier::XeonW, 12, "P"));
        // Xeon Scalable server (gen = 2nd digit).
        let t = brand_parse("Intel(R) Xeon(R) Platinum 8480+ CPU").unwrap();
        assert_eq!((t.tier, t.gen), (BrandTier::XeonScalable, 4));
        // Entry N-series (no generation number).
        let t = brand_parse("Intel(R) Processor N100").unwrap();
        assert_eq!((t.tier, t.gen), (BrandTier::NSeries, 0));
        // ES / non-Intel / empty → None (no panic).
        assert_eq!(brand_parse("Intel(R) Genuine Processor"), None);
        assert_eq!(brand_parse("Intel(R) 0000 @ 2.10GHz"), None);
        assert_eq!(brand_parse("AMD Ryzen 9 5950X"), None);
        assert_eq!(brand_parse(""), None);
    }

    /// Every collision model with an empty (unparseable) brand keeps its
    /// conservative model-table fallback — never `None` for a listed
    /// model, never a panic.
    #[test]
    fn collisions_with_empty_brand_keep_conservative_fallback() {
        assert_eq!(intel_gen_from_brand("", 0x8E), Some(IntelGen::KabyLake));
        assert_eq!(intel_gen_from_brand("", 0x9E), Some(IntelGen::CoffeeLake));
        assert_eq!(intel_gen_from_brand("", 0xA5), Some(IntelGen::CometLake));
        assert_eq!(intel_gen_from_brand("", 0x97), Some(IntelGen::AlderLake));
        assert_eq!(intel_gen_from_brand("", 0x9A), Some(IntelGen::AlderLake));
        assert_eq!(intel_gen_from_brand("", 0xB7), Some(IntelGen::RaptorLake));
        assert_eq!(intel_gen_from_brand("", 0xBA), Some(IntelGen::RaptorLake));
        assert_eq!(intel_gen_from_brand("", 0xBF), Some(IntelGen::RaptorLake));
        // Non-colliding models are unaffected by the (empty) brand.
        assert_eq!(intel_gen_from_brand("", 0x8C), Some(IntelGen::TigerLake));
        assert_eq!(intel_gen_from_brand("", 0x8D), Some(IntelGen::TigerLake));
        assert_eq!(intel_gen_from_brand("", 0xBE), Some(IntelGen::AlderLake));
        assert_eq!(intel_gen_from_brand("", 0x8F), Some(IntelGen::SapphireRapids));
        assert_eq!(intel_gen_from_brand("", 0x9C), Some(IntelGen::JasperLake));
        assert_eq!(intel_gen_from_brand("", 0xBD), Some(IntelGen::LunarLake));
        assert_eq!(intel_gen_from_brand("", 0x9B), Some(IntelGen::Unrecognized));
        assert_eq!(intel_gen_from_brand("", 0x9F), Some(IntelGen::Unrecognized));
        assert_eq!(intel_gen_from_brand("", 0xC0), Some(IntelGen::Unrecognized));
    }

    /// Exhaustive sweep: every 8-bit model value through both resolvers,
    /// across a representative brand set, never panics.
    #[test]
    fn full_model_sweep_no_panic() {
        let brands = [
            "",
            "Intel(R) Core(TM) i5-12600KF CPU @ 3.70GHz",
            "Intel(R) Core(TM) Ultra 7 155H",
            "Intel(R) Core(TM) 7 processor 150U",
            "Intel(R) Xeon(R) W-1290P CPU @ 3.70GHz",
            "Intel(R) Xeon(R) Platinum 8480+ CPU",
            "Intel(R) Processor N100",
            "Intel(R) Genuine Processor",
            "AMD Ryzen 9 5950X",
        ];
        for m in 0u32..=0xFF {
            let _ = intel_gen_from_model(m);
            for brand in brands {
                let _ = intel_gen_from_brand(brand, m);
            }
        }
    }
}
