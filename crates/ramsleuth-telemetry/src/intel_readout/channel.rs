//! Channel resolution: the 4-tier trust procedure, the capacity-derived
//! MMIO channel mode, the C-3 MAD/MCL1 aliasing policy, and the
//! `ChannelMode` / `MemoryTech` display-label and decode helpers.
//!
//! Step 2 of the `intel_readout` directory-module split — moved verbatim
//! from `mod.rs`.

use super::gates::*;
use super::types::*;

use crate::amd_readout::EccStatus;
use crate::cpuid::IntelGen;
use crate::intel_smbios::SmbiosTopology;

// ---------------------------------------------------------------------------
// ChannelMode display labels.
// ---------------------------------------------------------------------------

impl ChannelMode {
    /// The full human-readable channel label (the header channel slot).
    pub fn label(&self) -> &'static str {
        match self {
            ChannelMode::DualSymmetric => "Dual-Channel (Symmetric)",
            ChannelMode::DualFlex => "Dual-Channel (Flex)",
            ChannelMode::Single => "Single-Channel",
        }
    }

    /// Short string for the header "Mode:" slot.
    pub fn mode_label(&self) -> &'static str {
        match self {
            ChannelMode::DualSymmetric => "Interleaved",
            ChannelMode::DualFlex => "Flex",
            ChannelMode::Single => "N/A",
        }
    }

    /// The display label for a channel mode given the active memory
    /// technology (the single source both frontends render): a DDR5
    /// dual-channel configuration is **"Dual-Channel (4x 32-bit
    /// Subchannels)"** — never "quad-channel" (research: a 128-bit
    /// aggregate bus is dual-channel; true quad-channel is a 256-bit server
    /// topology). A DDR4 (or unknown-tech) dual keeps its symmetric / flex
    /// label.
    pub fn display_label(&self, tech: Option<MemoryTech>) -> String {
        match (self, tech) {
            (ChannelMode::DualSymmetric, Some(MemoryTech::Ddr5))
            | (ChannelMode::DualFlex, Some(MemoryTech::Ddr5)) => {
                "Dual-Channel (4x 32-bit Subchannels)".to_owned()
            }
            _ => self.label().to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryTech decoders.
// ---------------------------------------------------------------------------

impl MemoryTech {
    /// Decode the `DDR_TYPE` field (`MAD_INTER_CHANNEL[2:0]`):
    /// `000`=DDR4, `001`=DDR3/3L, `010`=LPDDR4, `011`=LPDDR5, `100`=DDR5;
    /// any other (reserved) encoding → [`MemoryTech::Unknown`].
    pub fn from_mad_bits(bits: u32) -> MemoryTech {
        match bits & 0x7 {
            0 => MemoryTech::Ddr4,
            1 => MemoryTech::Ddr3,
            2 => MemoryTech::Lpddr4,
            3 => MemoryTech::Lpddr5,
            4 => MemoryTech::Ddr5,
            _ => MemoryTech::Unknown,
        }
    }

    /// Decode a SMBIOS Type 17 memory-type byte (offset `0x12`):
    /// `0x1A`=DDR4, `0x22`=DDR5; anything else → `None` (no recognized
    /// technology — the caller keeps `None`).
    pub fn from_smbios_type(t: u8) -> Option<MemoryTech> {
        match t {
            0x1A => Some(MemoryTech::Ddr4),
            0x22 => Some(MemoryTech::Ddr5),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DIMM capacity decode (the corrected 0.5 GiB layout).
// ---------------------------------------------------------------------------

/// A channel's reported DIMM capacity (in GiB) from its `MAD_DIMM_CHx`
/// raw, decoded with the **corrected** bit layout (Chunk 2):
/// `DIMM_L_SIZE` (slot 0) at bits `[6:0]` + `DIMM_S_SIZE` (slot 1) at bits
/// `[22:16]`, each a 7-bit index in **0.5 GiB** increments (research:
/// `Value << 29 bytes` = value × 0.5 GiB). The sum of both slot sizes is
/// returned; `None` when the raw is absent. A slot index of `0` means
/// unpopulated.
///
/// **Correction:** the pre-Chunk-2 decode used a non-existent 1 GiB layout
/// (`[5:0] + [21:16]`); the real register encodes 0.5 GiB granularity in
/// 7-bit fields, so a 16 GiB DIMM is index `32` (`32 × 0.5`), not `16`.
pub(super) fn mad_dimm_capacity_gib(mad_dimm_ch: Option<u32>) -> Option<f64> {
    let r = mad_dimm_ch?;
    // M-11: Screen bus-fault and zero words — a degenerate MAD_DIMM
    // word is not a valid capacity reading.
    if is_degenerate_word(r) {
        return None;
    }
    let dimm_l = (r & 0x7F) as f64; // [6:0]
    let dimm_s = ((r >> 16) & 0x7F) as f64; // [22:16]
    Some((dimm_l + dimm_s) * 0.5)
}

// ---------------------------------------------------------------------------
// ECC status decode.
// ---------------------------------------------------------------------------

/// Decode the ECC capability from the raw CAPID0_A register (host-bridge
/// config-space offset 0xE4, read in-kernel by the module): bit 17
/// (ECC_DIS) set = the platform cannot do ECC (NotCapable); clear = the
/// controller is ECC-capable but ECC is inactive (CapableButDisabled).
/// An absent raw (no capid0a attribute / a read failure) yields Unknown.
///
/// The decode keys off the register bit, never the CPU vendor string
/// (the i5-6600T Q170 box is ECC-capable despite being a client CPU).
pub fn decode_ecc_status(capid0a: Option<u32>) -> EccStatus {
    match capid0a {
        None => EccStatus::Unknown,
        // H-10 / N-05: a degenerate word — the kernel emits
        // `0xFFFF_FFFF` when the config read fails, and a zero word is
        // equally suspicious (untrained / cleared) — is NOT a valid
        // capability assertion (the canonical degenerate screen).
        Some(v) if is_degenerate_word(v) => EccStatus::Unknown,
        Some(v) if v & 0x0002_0000 != 0 => EccStatus::NotCapable,
        Some(_) => EccStatus::CapableButDisabled,
    }
}

// ---------------------------------------------------------------------------
// Capacity-derived channel mode.
// ---------------------------------------------------------------------------

/// Derive the channel mode from the two per-channel capacities (GiB)
/// (Chunk 2, the capacity-derived model that replaces the old 2-bit
/// `MAD_INTER_CHANNEL[1:0]` decode):
///
/// - `CH_S == 0` → [`ChannelMode::Single`] (all memory on channel L);
/// - `CH_S > 0 && CH_S == CH_L` → [`ChannelMode::DualSymmetric`];
/// - `0 < CH_S < CH_L` → [`ChannelMode::DualFlex`] (Intel Flex Memory).
///
/// `None` when the pair is ill-formed (a negative / non-finite value, or
/// `ch_s > ch_l` — the caller must pass `(smaller, larger)`).
pub fn channel_mode_from_capacity(ch_s_gib: f64, ch_l_gib: f64) -> Option<ChannelMode> {
    if !ch_s_gib.is_finite() || !ch_l_gib.is_finite() || ch_s_gib < 0.0 || ch_l_gib < 0.0 {
        return None;
    }
    if ch_s_gib == 0.0 {
        Some(ChannelMode::Single)
    } else if ch_s_gib == ch_l_gib {
        Some(ChannelMode::DualSymmetric)
    } else if ch_s_gib < ch_l_gib {
        Some(ChannelMode::DualFlex)
    } else {
        None
    }
}

/// Compute the MMIO-decoded channel mode from the two `MAD_DIMM_CH0/1`
/// raws (the capacity-derived model): both sums must be readable and at
/// least one channel must report capacity — otherwise the MMIO is
/// "blocked" (`None`), which routes [`resolve_channel`] to the SMBIOS
/// tiers. `CH_L` / `CH_S` are the larger / smaller of the two channel
/// sums (there is no `CH_L_SIZE` field in `MAD_INTER_CHANNEL`).
pub(super) fn mmio_channel_mode(
    mad_dimm_ch0: Option<u32>,
    mad_dimm_ch1: Option<u32>,
) -> Option<ChannelMode> {
    let c0 = mad_dimm_capacity_gib(mad_dimm_ch0)?;
    let c1 = mad_dimm_capacity_gib(mad_dimm_ch1)?;
    let (ch_l, ch_s) = if c0 >= c1 { (c0, c1) } else { (c1, c0) };
    if ch_l == 0.0 {
        return None; // no population evidence -> the MMIO is blocked
    }
    channel_mode_from_capacity(ch_s, ch_l)
}

// ---------------------------------------------------------------------------
// 12th-gen+ IMC base.
// ---------------------------------------------------------------------------

/// The 12th-gen+ (Alder Lake and later) generation set — the set whose
/// IMC MAD registers moved to the `0xd800` base (Chunk 1's gen set;
/// research: key on the CPU generation, NOT the overlapping PCI
/// Device-ID ranges). The kernel module and this userspace decode must
/// use the *same* set (plan cross-cutting §7.3 / risk note).
pub fn is_12th_gen_plus(gen: IntelGen) -> bool {
    matches!(
        gen,
        IntelGen::AlderLake
            | IntelGen::RaptorLake
            | IntelGen::MeteorLake
            | IntelGen::ArrowLake
            | IntelGen::LunarLake
            | IntelGen::CoreSeries1Mobile
    )
}

// ---------------------------------------------------------------------------
// The 4-tier trust procedure.
// ---------------------------------------------------------------------------

/// The 4-tier trust procedure (research "Recommended Multi-Tier Decision
/// Procedure"): reconcile the hardware MMIO decode against the SMBIOS Type
/// 17 topology.
///
/// - **L1** ([`ChannelTrust::VerifiedHardware`]): the MMIO decoded a valid
///   mode (a dual decode, or a single decode SMBIOS does not contradict) →
///   the MMIO mode, no annotation. The hardware read is the primary
///   authority.
/// - **L3** ([`ChannelTrust::Override`]) — the **safety override** (the
///   core fix): the MMIO decoded `Single` but SMBIOS confirms ≥ 2
///   populated DIMMs across channels → report **Dual** (the MMIO read is
///   marked untrusted — register offset drift / a vendor MRC
///   customization / a stale field), with an "untrusted-MMIO mismatch"
///   annotation. This fires even when the module is old/absent, because
///   the MMIO decode and the SMBIOS cross-check are independent inputs.
/// - **L2** ([`ChannelTrust::VerifiedFirmware`]): the MMIO is blocked
///   (`None`) and SMBIOS shows ≥ 2 DIMMs in matched channels (→ Dual) or on
///   one channel (→ Single), or exactly 1 DIMM (→ Single).
/// - **L4** ([`ChannelTrust::Heuristic`]): the MMIO is blocked and the
///   SMBIOS locators are ambiguous (→ a dual heuristic, "unverified").
/// - **terminal** ([`ChannelTrust::None`]): neither source identified the
///   topology.
///
/// The active memory technology is the MMIO `DDR_TYPE` when present, else
/// the SMBIOS type. Pure (no I/O) — the facade / `/dev/mem` path supply
/// the inputs.
pub fn resolve_channel(
    mmio: Option<ChannelMode>,
    mmio_tech: Option<MemoryTech>,
    smbios: &SmbiosTopology,
) -> ChannelResolution {
    let dimm_count = smbios.populated_count;
    // M-6: MemoryTech::Unknown from a bus-faulted MAD word should not
    // shadow a valid SMBIOS type. Normalize to None before the .or().
    let mmio_tech_filtered = mmio_tech.filter(|t| !matches!(t, MemoryTech::Unknown));
    let tech = mmio_tech_filtered.or(smbios.memory_type);

    // L1 / L3: the MMIO decode is readable (a valid mode).
    if let Some(mode) = mmio {
        return match mode {
            // L1: a dual decode is the hardware authority.
            ChannelMode::DualSymmetric | ChannelMode::DualFlex => ChannelResolution {
                mode: Some(mode),
                tech,
                trust: Some(ChannelTrust::VerifiedHardware),
                annotation: String::new(),
                dimm_count,
            },
            // L3 (override) / L1 (verified single).
            ChannelMode::Single => {
                if smbios.populated_count >= 2
                    && smbios.derived_mode == crate::intel_smbios::DerivedMode::Dual
                {
                    ChannelResolution {
                        mode: Some(ChannelMode::DualSymmetric),
                        tech,
                        trust: Some(ChannelTrust::Override),
                        annotation: format!(
                            "[Override: MCHBAR decoded Single-Channel, but SMBIOS Type 17 confirms {dimm_count} DIMMs across Channel A/B. Reporting Dual-Channel]"
                        ),
                        dimm_count,
                    }
                } else if smbios.populated_count >= 2
                    && smbios.derived_mode == crate::intel_smbios::DerivedMode::Ambiguous
                {
                    // M-15: soft annotation fallback — the MMIO decoded
                    // Single and SMBIOS confirms ≥ 2 DIMMs, but the locators
                    // carry no channel designation, so the channel count is
                    // unresolvable from firmware. Keep the MMIO Single
                    // verdict (soft) and carry the annotation.
                    ChannelResolution {
                        mode: Some(ChannelMode::Single),
                        tech,
                        trust: Some(ChannelTrust::VerifiedHardware),
                        annotation: format!(
                            "MMIO reports single-channel with {dimm_count} DIMMs present — channel designations unresolvable from firmware locators"
                        ),
                        dimm_count,
                    }
                } else {
                    ChannelResolution {
                        mode: Some(ChannelMode::Single),
                        tech,
                        trust: Some(ChannelTrust::VerifiedHardware),
                        annotation: String::new(),
                        dimm_count,
                    }
                }
            }
        };
    }

    // L2 / L4: the MMIO is blocked (no valid decode) — fall to SMBIOS.
    if smbios.populated_count >= 2 {
        return match smbios.derived_mode {
            // L2: matched channels, firmware-verified.
            crate::intel_smbios::DerivedMode::Dual => ChannelResolution {
                mode: Some(ChannelMode::DualSymmetric),
                tech,
                trust: Some(ChannelTrust::VerifiedFirmware),
                annotation: "[MMIO Inaccessible: topology verified via SMBIOS Type 17]".to_owned(),
                dimm_count,
            },
            // L4: ambiguous locators, dual heuristic.
            crate::intel_smbios::DerivedMode::Ambiguous => ChannelResolution {
                mode: Some(ChannelMode::DualSymmetric),
                tech,
                trust: Some(ChannelTrust::Heuristic),
                annotation:
                    "[Heuristic: 2 DIMMs detected on dual-channel processor; socket locators ambiguous]"
                        .to_owned(),
                dimm_count,
            },
            // L2 single: multiple DIMMs on the same channel.
            crate::intel_smbios::DerivedMode::Single => ChannelResolution {
                mode: Some(ChannelMode::Single),
                tech,
                trust: Some(ChannelTrust::VerifiedFirmware),
                annotation: "[Single-Channel: multiple DIMMs installed on identical memory channel]"
                    .to_owned(),
                dimm_count,
            },
        };
    }
    if smbios.populated_count == 1 {
        return ChannelResolution {
            mode: Some(ChannelMode::Single),
            tech,
            trust: Some(ChannelTrust::VerifiedFirmware),
            annotation: String::new(),
            dimm_count,
        };
    }

    // Terminal: neither MMIO nor SMBIOS identified the topology.
    ChannelResolution {
        mode: None,
        tech,
        trust: Some(ChannelTrust::None),
        annotation: "[Error: unable to identify memory topology via MMIO or SMBIOS]".to_owned(),
        dimm_count,
    }
}

// ============================================================================
// C-3: MAD/MCL1 aliasing policy on 12th-gen+ (the `0xD800` page)
// ============================================================================

/// The C-3 MAD/MCL1 aliasing policy for 12th-gen+ (the `0xD800` page is
/// read as both the MAD block and the MCL1 block). Keyed on the resolved
/// memory technology ([`ChannelResolution::tech`]):
///
/// - [`C3AliasPolicy::Mad`] (DDR4) — the `0xD800` page is the MAD block:
///   trust the MAD words for channel mode/geometry; the MCL1 timing words
///   (the same page) are **not** decoded.
/// - [`C3AliasPolicy::Mcl1`] (DDR5) — the `0xD800` page is the MCL1 block:
///   trust the MCL1 words for timing; the MAD geometry words (the same
///   page) do **not** drive the channel mode.
/// - [`C3AliasPolicy::MadAnnotated`] (unknown tech) — trust the MAD words
///   for geometry (the current behavior) and carry the aliasing
///   annotation.
///
/// This is a **preparation** (no hardware verification yet): it prevents
/// one interpretation from corrupting the other on 12th-gen+ while
/// leaving pre-12th-gen (the `0x5000` IMC base, no aliasing) untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum C3AliasPolicy {
    /// The `0xD800` page is the MAD block (DDR4): MAD geometry trusted,
    /// MCL1 timing words not decoded.
    Mad,
    /// The `0xD800` page is the MCL1 block (DDR5): MCL1 timing trusted,
    /// MAD geometry words do not drive the channel mode.
    Mcl1,
    /// The tech is unknown: MAD geometry trusted (the current behavior) and
    /// the aliasing annotation is carried.
    MadAnnotated,
}

/// Select the C-3 aliasing policy from the resolved memory technology.
pub(super) fn c3_alias_policy(tech: Option<MemoryTech>) -> C3AliasPolicy {
    match tech {
        Some(MemoryTech::Ddr4) => C3AliasPolicy::Mad,
        Some(MemoryTech::Ddr5) => C3AliasPolicy::Mcl1,
        _ => C3AliasPolicy::MadAnnotated,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::IntelGen;
    use crate::intel_smbios::DerivedMode;

    /// `MemoryTech::from_mad_bits` decodes the `DDR_TYPE` field
    /// (`MAD_INTER_CHANNEL[2:0]`): 000=DDR4, 001=DDR3, 010=LPDDR4,
    /// 011=LPDDR5, 100=DDR5; reserved (5-7) → Unknown; high bits masked.
    #[test]
    fn memory_tech_from_mad_bits() {
        assert_eq!(MemoryTech::from_mad_bits(0x0), MemoryTech::Ddr4);
        assert_eq!(MemoryTech::from_mad_bits(0x1), MemoryTech::Ddr3);
        assert_eq!(MemoryTech::from_mad_bits(0x2), MemoryTech::Lpddr4);
        assert_eq!(MemoryTech::from_mad_bits(0x3), MemoryTech::Lpddr5);
        assert_eq!(MemoryTech::from_mad_bits(0x4), MemoryTech::Ddr5);
        assert_eq!(MemoryTech::from_mad_bits(0x5), MemoryTech::Unknown);
        assert_eq!(MemoryTech::from_mad_bits(0x6), MemoryTech::Unknown);
        assert_eq!(MemoryTech::from_mad_bits(0x7), MemoryTech::Unknown);
        // Only bits [2:0] are read (the rest is masked away).
        assert_eq!(MemoryTech::from_mad_bits(0xFFFF_FFF4), MemoryTech::Ddr5);
    }

    /// `MemoryTech::from_smbios_type` decodes the SMBIOS Type 17
    /// memory-type byte (offset `0x12`): 0x1A=DDR4, 0x22=DDR5; anything
    /// else → None.
    #[test]
    fn memory_tech_from_smbios_type() {
        assert_eq!(MemoryTech::from_smbios_type(0x1A), Some(MemoryTech::Ddr4));
        assert_eq!(MemoryTech::from_smbios_type(0x22), Some(MemoryTech::Ddr5));
        assert_eq!(MemoryTech::from_smbios_type(0x00), None);
        assert_eq!(MemoryTech::from_smbios_type(0xFF), None);
    }

    /// The full channel label + the short "Mode:" label.
    #[test]
    fn channel_mode_labels() {
        assert_eq!(
            ChannelMode::DualSymmetric.label(),
            "Dual-Channel (Symmetric)"
        );
        assert_eq!(ChannelMode::DualFlex.label(), "Dual-Channel (Flex)");
        assert_eq!(ChannelMode::Single.label(), "Single-Channel");

        assert_eq!(ChannelMode::DualSymmetric.mode_label(), "Interleaved");
        assert_eq!(ChannelMode::DualFlex.mode_label(), "Flex");
        assert_eq!(ChannelMode::Single.mode_label(), "N/A");
    }

    /// The DDR5 display label: a dual-channel DDR5 config renders
    /// "Dual-Channel (4x 32-bit Subchannels)" — never "quad-channel"
    /// (research: a 128-bit aggregate bus is dual-channel). DDR4 / unknown
    /// / single keep their labels.
    #[test]
    fn channel_mode_display_label_ddr5_subchannels_not_quad() {
        assert_eq!(
            ChannelMode::DualSymmetric.display_label(Some(MemoryTech::Ddr5)),
            "Dual-Channel (4x 32-bit Subchannels)"
        );
        assert_eq!(
            ChannelMode::DualFlex.display_label(Some(MemoryTech::Ddr5)),
            "Dual-Channel (4x 32-bit Subchannels)"
        );
        assert_eq!(
            ChannelMode::DualSymmetric.display_label(Some(MemoryTech::Ddr4)),
            "Dual-Channel (Symmetric)"
        );
        assert_eq!(
            ChannelMode::DualSymmetric.display_label(None),
            "Dual-Channel (Symmetric)"
        );
        assert_eq!(
            ChannelMode::Single.display_label(Some(MemoryTech::Ddr5)),
            "Single-Channel"
        );
    }

    /// The corrected 0.5 GiB DIMM-size layout: `DIMM_L_SIZE [6:0]` +
    /// `DIMM_S_SIZE [22:16]`, each a 7-bit index × 0.5 GiB (a 16 GiB DIMM
    /// is index 32, not the pre-Chunk-2 1 GiB index 16).
    #[test]
    fn mad_dimm_capacity_gib_0_5gib_layout() {
        assert_eq!(mad_dimm_capacity_gib(None), None);
        // M-11: zero word is degenerate (screened out).
        assert_eq!(mad_dimm_capacity_gib(Some(0x0)), None);
        // M-11: bus-fault word is degenerate (screened out).
        assert_eq!(mad_dimm_capacity_gib(Some(0xFFFF_FFFF)), None);
        // 16 GiB in slot 0 (index 32: 32 * 0.5 = 16 GiB).
        assert_eq!(mad_dimm_capacity_gib(Some(0x20)), Some(16.0));
        // 8 GiB slot 0 (index 16) + 16 GiB slot 1 (index 32 << 16).
        assert_eq!(mad_dimm_capacity_gib(Some(0x20_0010)), Some(24.0));
        // 32 GiB slot 0 (index 64) alone.
        assert_eq!(mad_dimm_capacity_gib(Some(0x40)), Some(32.0));
    }

    /// The capacity-derived mode table (Chunk 2): CH_S==0 → Single;
    /// CH_S>0 && ==CH_L → DualSymmetric; 0<CH_S<CH_L → DualFlex;
    /// ill-formed (ch_s>ch_l, negative, non-finite) → None.
    #[test]
    fn channel_mode_from_capacity_table() {
        assert_eq!(
            channel_mode_from_capacity(0.0, 16.0),
            Some(ChannelMode::Single)
        );
        assert_eq!(
            channel_mode_from_capacity(0.0, 32.0),
            Some(ChannelMode::Single)
        );
        assert_eq!(
            channel_mode_from_capacity(16.0, 16.0),
            Some(ChannelMode::DualSymmetric)
        );
        assert_eq!(
            channel_mode_from_capacity(32.0, 32.0),
            Some(ChannelMode::DualSymmetric)
        );
        assert_eq!(
            channel_mode_from_capacity(8.0, 16.0),
            Some(ChannelMode::DualFlex)
        );
        assert_eq!(
            channel_mode_from_capacity(16.0, 32.0),
            Some(ChannelMode::DualFlex)
        );
        assert_eq!(channel_mode_from_capacity(16.0, 8.0), None);
        assert_eq!(channel_mode_from_capacity(-1.0, 16.0), None);
        assert_eq!(channel_mode_from_capacity(f64::NAN, 16.0), None);
        assert_eq!(channel_mode_from_capacity(16.0, f64::INFINITY), None);
    }

    /// `mmio_channel_mode`: both channel sums readable + at least one
    /// populated → the capacity-derived mode; both absent or both zero →
    /// `None` (the MMIO is "blocked" → `resolve_channel` falls to SMBIOS).
    #[test]
    fn mmio_channel_mode_blocked_and_populated() {
        // 2×16 GiB (index 32 each) -> DualSymmetric.
        assert_eq!(
            mmio_channel_mode(Some(0x20), Some(0x20)),
            Some(ChannelMode::DualSymmetric)
        );
        // 8 + 16 GiB (index 16 + 32) -> DualFlex.
        assert_eq!(
            mmio_channel_mode(Some(0x10), Some(0x20)),
            Some(ChannelMode::DualFlex)
        );
        // M-11: 16 GiB on ch0 + zero word on ch1 -> the zero is screened
        // as degenerate, so the MMIO is blocked (None) — the SMBIOS tier
        // handles the single-channel case.
        assert_eq!(mmio_channel_mode(Some(0x20), Some(0x0)), None);
        // Both absent -> blocked (None).
        assert_eq!(mmio_channel_mode(None, None), None);
        // Both zero (no population) -> blocked (None).
        assert_eq!(mmio_channel_mode(Some(0x0), Some(0x0)), None);
        // M-11: bus-fault word on one channel -> blocked (None).
        assert_eq!(mmio_channel_mode(Some(0xFFFF_FFFF), Some(0x20)), None);
    }

    /// The IMC base selection is keyed on the CPU generation (NOT the
    /// overlapping PCI Device-ID ranges): 12th-gen+ → 0xd800, 6th-11th
    /// gen → 0x5000 (the kernel module uses the same set).
    #[test]
    fn imc_base_for_selects_by_generation() {
        for gen in [
            IntelGen::AlderLake,
            IntelGen::RaptorLake,
            IntelGen::MeteorLake,
            IntelGen::ArrowLake,
            IntelGen::LunarLake,
            IntelGen::CoreSeries1Mobile,
        ] {
            assert_eq!(super::super::imc_base_for(gen), 0xD800, "{gen:?}");
            assert!(is_12th_gen_plus(gen), "{gen:?}");
        }
        for gen in [
            IntelGen::Skylake,
            IntelGen::KabyLake,
            IntelGen::CoffeeLake,
            IntelGen::CometLake,
            IntelGen::IceLake,
            IntelGen::TigerLake,
            IntelGen::RocketLake,
            IntelGen::Unrecognized,
        ] {
            assert_eq!(super::super::imc_base_for(gen), 0x5000, "{gen:?}");
            assert!(!is_12th_gen_plus(gen), "{gen:?}");
        }
    }

    /// A synthetic SMBIOS topology fixture (the 4-tier tests).
    fn smbios_topology(
        populated: u8,
        derived: DerivedMode,
        tech: Option<MemoryTech>,
    ) -> SmbiosTopology {
        SmbiosTopology {
            populated_count: populated,
            dimm_sizes: (0..populated).map(|_| 16384).collect(),
            total_capacity_mib: (populated as u32) * 16384,
            memory_type: tech,
            speed_mts: None,
            locators: (0..populated).map(|i| format!("Ch{i}")).collect(),
            derived_mode: derived,
        }
    }

    /// The 4-tier trust procedure, one arm per tier (L1 verified-hardware,
    /// L2 verified-firmware, L3 override, L4 heuristic, terminal):
    /// `resolve_channel` pins `(mode, trust, annotation)` for each.
    #[test]
    fn resolve_channel_four_tiers_l1_l4() {
        // L1: MMIO dual (the hardware authority), no annotation.
        let r = resolve_channel(
            Some(ChannelMode::DualSymmetric),
            Some(MemoryTech::Ddr4),
            &smbios_topology(2, DerivedMode::Dual, Some(MemoryTech::Ddr4)),
        );
        assert_eq!(r.mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(r.trust, Some(ChannelTrust::VerifiedHardware));
        assert_eq!(r.tech, Some(MemoryTech::Ddr4));
        assert_eq!(r.annotation, "");
        assert_eq!(r.dimm_count, 2);

        // L3 (the safety override): MMIO single + SMBIOS 2 DIMMs across
        // channels -> Dual + Override + the untrusted-MMIO annotation.
        let r = resolve_channel(
            Some(ChannelMode::Single),
            None,
            &smbios_topology(2, DerivedMode::Dual, None),
        );
        assert_eq!(r.mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(r.trust, Some(ChannelTrust::Override));
        assert!(r.annotation.contains("Override"), "{:?}", r.annotation);
        assert!(r.annotation.contains("2 DIMMs"), "{:?}", r.annotation);

        // L1 single: MMIO single + SMBIOS agrees (1 DIMM) -> Single, no
        // override.
        let r = resolve_channel(
            Some(ChannelMode::Single),
            None,
            &smbios_topology(1, DerivedMode::Single, None),
        );
        assert_eq!(r.mode, Some(ChannelMode::Single));
        assert_eq!(r.trust, Some(ChannelTrust::VerifiedHardware));
        assert_eq!(r.annotation, "");

        // L2: MMIO blocked (None) + SMBIOS 2 DIMMs across channels -> Dual,
        // VerifiedFirmware.
        let r = resolve_channel(
            None,
            Some(MemoryTech::Ddr5),
            &smbios_topology(2, DerivedMode::Dual, Some(MemoryTech::Ddr5)),
        );
        assert_eq!(r.mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(r.trust, Some(ChannelTrust::VerifiedFirmware));
        assert_eq!(r.tech, Some(MemoryTech::Ddr5));
        assert!(r.annotation.contains("MMIO Inaccessible"));

        // L2 single: MMIO blocked + SMBIOS 2 DIMMs on the same channel ->
        // Single, VerifiedFirmware.
        let r = resolve_channel(None, None, &smbios_topology(2, DerivedMode::Single, None));
        assert_eq!(r.mode, Some(ChannelMode::Single));
        assert_eq!(r.trust, Some(ChannelTrust::VerifiedFirmware));

        // L4: MMIO blocked + SMBIOS ambiguous locators -> Dual heuristic.
        let r = resolve_channel(None, None, &smbios_topology(2, DerivedMode::Ambiguous, None));
        assert_eq!(r.mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(r.trust, Some(ChannelTrust::Heuristic));
        assert!(r.annotation.contains("Heuristic"));

        // Terminal: MMIO blocked + no SMBIOS population -> None mode,
        // ChannelTrust::None.
        let r = resolve_channel(None, None, &SmbiosTopology::empty());
        assert_eq!(r.mode, None);
        assert_eq!(r.trust, Some(ChannelTrust::None));
        assert!(r.annotation.contains("Error"));
    }

    /// (M-15) The soft annotation fallback: the MMIO decoded Single and
    /// SMBIOS confirms ≥ 2 DIMMs, but the locators carry no channel
    /// designation (`Ambiguous`). The channel count is unresolvable from
    /// firmware, so the Single verdict is kept (soft) and the M-15
    /// annotation is carried.
    #[test]
    fn resolve_channel_m15_single_with_ambiguous_locators_annotates() {
        let r = resolve_channel(
            Some(ChannelMode::Single),
            None,
            &smbios_topology(2, DerivedMode::Ambiguous, None),
        );
        assert_eq!(r.mode, Some(ChannelMode::Single));
        assert_eq!(r.trust, Some(ChannelTrust::VerifiedHardware));
        assert!(
            r.annotation.contains("unresolvable from firmware locators"),
            "{:?}",
            r.annotation
        );
        assert!(r.annotation.contains("2 DIMMs"), "{:?}", r.annotation);
        assert_eq!(r.dimm_count, 2);
    }

    /// M-6: a `MemoryTech::Unknown` from a bus-faulted MAD word does not
    /// shadow a valid SMBIOS type — `resolve_channel` normalizes it to
    /// `None` before the `.or()`.
    #[test]
    fn resolve_channel_normalizes_unknown_tech() {
        let smbios = smbios_topology(2, DerivedMode::Dual, Some(MemoryTech::Ddr4));
        // MMIO tech is Unknown (bus-faulted), SMBIOS has Ddr4.
        let r = resolve_channel(
            Some(ChannelMode::DualSymmetric),
            Some(MemoryTech::Unknown),
            &smbios,
        );
        assert_eq!(r.tech, Some(MemoryTech::Ddr4), "SMBIOS Ddr4 wins over MMIO Unknown");

        // MMIO tech is valid (Ddr5), SMBIOS has Ddr4 -> MMIO wins.
        let r = resolve_channel(
            Some(ChannelMode::DualSymmetric),
            Some(MemoryTech::Ddr5),
            &smbios,
        );
        assert_eq!(r.tech, Some(MemoryTech::Ddr5), "valid MMIO Ddr5 wins");
    }

    /// The L3 firmware override on the real reference topology (BUG 2
    /// end-to-end, without hardware): the 12600KF / Z690M-ITX/ax SMBIOS
    /// Type 17 table — 2 × 16 GiB DDR4, both locators
    /// `Controller<N>-ChannelA-DIMM0` (the channel is the Controller
    /// index: 0→A, 1→B) → `DerivedMode::Dual` — against an MMIO decode
    /// of `Single` (the `0xd800` residual) must resolve to
    /// `DualSymmetric` with the `Override` trust tier, `dimm_count 2`,
    /// and the non-empty override annotation.
    #[test]
    fn resolve_channel_l3_override_on_real_topology() {
        let smbios = SmbiosTopology {
            populated_count: 2,
            dimm_sizes: vec![16384, 16384],
            total_capacity_mib: 32768,
            memory_type: Some(MemoryTech::Ddr4),
            speed_mts: None,
            locators: vec![
                "Controller0-ChannelA-DIMM0".to_owned(),
                "Controller1-ChannelA-DIMM0".to_owned(),
            ],
            derived_mode: DerivedMode::Dual,
        };
        let res = resolve_channel(Some(ChannelMode::Single), None, &smbios);
        assert_eq!(res.mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(res.trust, Some(ChannelTrust::Override));
        assert_eq!(res.tech, Some(MemoryTech::Ddr4));
        assert_eq!(res.dimm_count, 2);
        assert!(
            !res.annotation.is_empty(),
            "the L3 override carries the untrusted-MMIO annotation"
        );
    }

    /// decode_ecc_status maps the raw CAPID0_A bit 17 (ECC_DIS) to the
    /// EccStatus verdict (the i5-6600T live value 0x62012671 is
    /// CapableButDisabled; a bit-17-set value is NotCapable; absent is
    /// Unknown).
    #[test]
    fn decode_ecc_status_maps_capid0a_bit17() {
        // Live-verified i5-6600T value: bit 17 clear -> CapableButDisabled.
        assert_eq!(
            decode_ecc_status(Some(0x6201_2671)),
            EccStatus::CapableButDisabled
        );
        // Bit 17 set (ECC_DIS) -> NotCapable.
        assert_eq!(decode_ecc_status(Some(0x0002_0000)), EccStatus::NotCapable);
        // H-10: bus-fault word is not a valid capability assertion.
        assert_eq!(decode_ecc_status(Some(0xFFFF_FFFF)), EccStatus::Unknown);
        // H-10: zero word is equally suspicious.
        assert_eq!(decode_ecc_status(Some(0)), EccStatus::Unknown);
        // Absent raw -> Unknown.
        assert_eq!(decode_ecc_status(None), EccStatus::Unknown);
    }
}
