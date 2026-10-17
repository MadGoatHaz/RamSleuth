//! Server-uncore decode (Haswell-E / Broadwell-E / Skylake-X / Cascade
//! Lake): no OS-accessible MC timing registers — the telemetry sources
//! SMBIOS Type 17 + the SPD EEPROM, never the MCHBAR MMIO window.
//!
//! Step 2 of the `intel_readout` directory-module split — moved verbatim
//! from `mod.rs`.

use super::gates::*;
use super::types::*;

use std::collections::HashMap;

use crate::amd_readout::{CadBus, ClockReadout, EccStatus, TimingSet, VoltageSet};
use crate::cpuid::IntelGen;
use crate::error::{NaReason, Section};
use crate::intel_gen::GenMap;
use crate::intel_smbios::SmbiosTopology;
use crate::spd_decode::SpdModule;

/// The true reason a structurally-absent MC-live register field carries on
/// server uncore (an honest N/A — never a placeholder zero, never a
/// fabricated value). This is NOT a decode failure: on these platforms the
/// memory-controller timing registers live in CPU-internal uncore (no PCI
/// BAR, unreachable via `/dev/mem`) and the SPD EEPROM sits on the MC's
/// internal I2C bus (not OS-visible), so there is **no OS-accessible source
/// for the live timing data** — the data simply does not exist anywhere the
/// OS can read it.
pub const SERVER_NA_REASON: &str =
    "server uncore: no OS-accessible timing source (no MC BAR, no SPD EEPROM)";

/// `true` for a generation on the `Server` register-map family (the
/// server-uncore platforms — Haswell-E / Broadwell-E / Skylake-X / Cascade
/// Lake). These have no OS-accessible MC timing registers, so the decode
/// sources SMBIOS + SPD (see [`decode_server`]) and never reads the MCHBAR
/// MMIO window.
pub fn is_server_gen(gen: IntelGen) -> bool {
    matches!(
        crate::intel_gen::profile_for(gen).map(|p| p.map),
        Some(GenMap::Server)
    )
}

/// The channel-group key parsed from an SMBIOS Type 17 locator string
/// (M-9): the designator that tells one memory channel apart from the next
/// on a multi-DIMM-per-channel server. Recognizes the common forms — an
/// explicit `Ch<N>` / `Channel <N>`, and a leading slot letter
/// (`DIMM_A1` -> `A`, `B2` -> `B`) — and yields `None` when the locator
/// carries no channel signal (the binding then keeps the legacy ordinal
/// order rather than re-ordering on a guess).
fn locator_channel_key(locator: &str) -> Option<String> {
    let s = locator.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    // Explicit numeric channel designator ("ch0", "channel 1", "ch 2").
    for prefix in ["channel ", "ch ", "ch"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let digits: String = rest
                .chars()
                .filter(|c| c.is_ascii_digit())
                .take(2)
                .collect();
            if !digits.is_empty() {
                return Some(format!("ch{digits}"));
            }
        }
    }
    // A leading slot letter: drop the common non-channel word prefixes
    // ("dimm", "memory", "module", "slot", "cpu", "channel", "ch") and take
    // the first alphabetic designator of what remains (e.g. "DIMM_A1" ->
    // "a1" -> "A").
    for tok in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if matches!(
            tok,
            "dimm" | "memory" | "module" | "slot" | "cpu" | "channel" | "ch"
        ) {
            continue;
        }
        if let Some(c) = tok.chars().next() {
            if c.is_ascii_alphabetic() {
                return Some(c.to_ascii_uppercase().to_string());
            }
        }
    }
    None
}

/// Bind the SPD modules to the decode's output channels by their SMBIOS
/// channel group (M-9) instead of ordinal position. On a
/// multi-DIMM-per-channel server the ordinal bind (`channel i <- SPD i`)
/// mis-assigns: with 2 DIMMs per channel, channel 1 would take the second
/// DIMM of channel 0. We group the SPD modules by the channel key parsed
/// from each SMBIOS locator and, for output channel `i`, return the first
/// module of that channel's group.
///
/// All-or-nothing: the grouping only activates when the SPD list aligns
/// 1:1 with the SMBIOS locators AND every locator yields a channel key;
/// otherwise `None` (the caller keeps the legacy ordinal bind — never a
/// partial re-order on an ambiguous signal).
fn spd_by_channel<'a>(
    smbios: &'a SmbiosTopology,
    spd: &'a [SpdModule],
) -> Option<Vec<Option<&'a SpdModule>>> {
    if spd.is_empty() || spd.len() != smbios.locators.len() {
        return None;
    }
    let mut groups: Vec<Vec<&SpdModule>> = Vec::new();
    let mut key_index: HashMap<String, usize> = HashMap::new();
    for (idx, loc) in smbios.locators.iter().enumerate() {
        let key = locator_channel_key(loc)?;
        let g = *key_index.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(&spd[idx]);
    }
    Some(groups.iter().map(|g| g.first().copied()).collect())
}

/// The server-uncore decode: one [`IntelChannel`] per the generation's
/// channel count, every cell sourced from real hardware — the SMBIOS Type 17
/// topology (capacity / technology / population) and the SPD EEPROM (rated
/// timings / speed) — with the MC-live register fields an honest N/A.
///
/// # Mapping (all real data, no fabrication)
///
/// - `clocks.mclk_mhz` ← the representative SPD module's rated data rate / 2
///   (the analog DRAM clock — a DDR4-2400 module → 1200 MHz), falling back to
///   the SMBIOS Type 17 Configured Memory Speed / 2 when no module carries a
///   speed (the server-uncore case — the SPD EEPROM is not OS-visible, so the
///   SMBIOS speed is the only source); `Na(NotApplicable)` when neither
///   source carries a speed. `uclk_mhz` = `mclk_mhz` (the MC runs at the
///   DRAM clock); every other clock / gear / command-rate cell is
///   `Na(NotApplicable)` (no OS-accessible source).
/// - `timings` — the four SPD-rated core timings (`cl` / `rcdrd` / `rcwdwr` /
///   `rp` / `ras` + the synthesized `rc = ras + rp`) are `Value`s from the
///   module's factory profile; every other MC-live timing is `Na` with
///   [`SERVER_NA_REASON`]. A module with no usable profile degrades all its
///   timing cells to the same honest reason.
/// - `cad_bus` / `voltages` / `rtl` → `Na` with [`SERVER_NA_REASON`] (no
///   OS-accessible source on server uncore — never the generic
///   "not applicable").
/// - `channel_mode` / `memory_tech` / `channel_trust` / `dimm_count` ← the
///   SMBIOS Type 17 topology resolved with **no** MMIO input (the 4-tier
///   [`super::channel::resolve_channel`] firmware-verified L2 / L4 states).
/// - `channel_annotation` ← the honest source note (timings SPD-rated,
///   topology firmware-verified, the populated/total channel count).
/// - `ecc_status` ← [`EccStatus::Unknown`] (no OS-accessible ECC source on
///   server uncore; never fabricated).
///
/// # No-panic contract (D5)
///
/// Pure: no I/O, no `unsafe`. An empty SPD list degrades the timing / clock
/// cells to N/A while the SMBIOS topology still carries the readout; a
/// missing / ambiguous SMBIOS topology degrades the channel mode to the
/// honest terminal state. Never a panic, never an invented value.
pub fn decode_server(
    smbios: &SmbiosTopology,
    spd: &[SpdModule],
    gen: IntelGen,
) -> IntelReadout {
    let count = super::channel_count(gen);

    // The representative SPD module: the first one carrying usable rated
    // data (a speed or a profile timing); `None` when no module is present.
    let rep = spd.iter().find(|m| {
        m.speed_mts.value().is_some()
            || m.profiles
                .iter()
                .any(|p| p.cas.value().is_some() || p.trcd.value().is_some())
    });

    // The analog DRAM clock: the representative SPD module's rated data
    // rate when present, else the SMBIOS Type 17 Configured Memory Speed
    // (the server-uncore fallback — the SPD EEPROM is not OS-visible, so the
    // SMBIOS speed is the only source). DDR: MT/s = MCLK × 2.
    let mclk = rep
        .and_then(|m| m.speed_mts.value().copied())
        .or(smbios.speed_mts)
        // M-10: gate the derived DRAM clock through the shared sanity
        // gate — a corrupt SMBIOS speed (e.g. a 0xFFFF 16-bit readback)
        // divides to an out-of-range MHz and is N/A'd with a proper
        // reason instead of displayed as real data.
        .map(|mts| clock_section(f64::from(mts) / 2.0))
        .unwrap_or(Section::na(NaReason::NotApplicable));
    let clocks = server_clocks(&mclk);

    // M-9: bind each output channel to the FIRST SPD module of that
    // channel's SMBIOS locator group (see [`spd_by_channel`]) instead of
    // the ordinal position — on a multi-DIMM-per-channel server the
    // ordinal bind would hand channel 1 the second DIMM of channel 0.
    let by_channel = spd_by_channel(smbios, spd);

    let mut channels = Vec::with_capacity(usize::from(count));
    for i in 0..count {
        // Per-channel: the module bound to this channel (by its SMBIOS
        // channel group, else the representative when the grouping is
        // unavailable). A matched kit runs every channel at the same
        // rated timing. No module at all → the all-Na timing set.
        let module = match &by_channel {
            Some(groups) => groups.get(i as usize).copied().flatten().or(rep),
            None => spd.get(i as usize).or(rep),
        };
        let timings = match module {
            Some(m) => server_timings(m),
            None => server_na_timings(),
        };
        channels.push(IntelChannel {
            index: i,
            clocks: clocks.clone(),
            timings,
            cad_bus: server_na_cad_bus(),
            voltages: server_na_voltages(),
            rtl: server_na_u16(),
        });
    }

    // The SMBIOS Type 17 topology, resolved with NO MMIO input (the 4-tier
    // procedure's firmware-verified L2 / L4 states).
    let res = super::channel::resolve_channel(None, smbios.memory_type, smbios);

    // M-16: report the populated CHANNEL count (distinct channels with
    // >=1 DIMM) separately from the raw DIMM count — the old text printed
    // the DIMM count in the channel slot ("8 of 4 channels populated"),
    // which is self-contradictory on a multi-DIMM-per-channel server.
    let populated_channels = match &by_channel {
        Some(groups) => groups.iter().filter(|m| m.is_some()).count() as u8,
        None => res.dimm_count.min(count),
    };
    let annotation = format!(
        "Server uncore: no OS-accessible timing source (no MC BAR, no SPD \
         EEPROM) — timings are SPD-rated (module EEPROM) where visible, \
         topology is SMBIOS Type 17 ({} of {} channels populated, {} DIMMs)",
        populated_channels, count, res.dimm_count
    );

    IntelReadout {
        channels,
        channel_mode: res.mode,
        ecc_status: EccStatus::Unknown,
        memory_tech: res.tech,
        channel_trust: res.trust,
        channel_annotation: annotation,
        dimm_count: res.dimm_count,
    }
}

/// The server-uncore clock cells: `mclk` from the SPD rated data rate (or
/// the SMBIOS Configured Memory Speed fallback) with `uclk` = `mclk` (the MC
/// runs at the DRAM clock); every other clock / gear / command-rate cell
/// `Na(NotApplicable)` (no OS-accessible source).
fn server_clocks(mclk: &Section<f64>) -> ClockReadout {
    // N-06: the structural NAs come from the shared builder; the server
    // decode layers the (data-driven) mclk / uclk on top.
    let mut clocks = na_clocks(&NaReason::NotApplicable);
    clocks.mclk_mhz = mclk.clone();
    clocks.uclk_mhz = mclk.clone();
    clocks
}

/// An MC-live timing cell's honest server-uncore N/A ([`SERVER_NA_REASON`]).
fn server_na_u16() -> Section<u16> {
    na_rtl(&NaReason::ParseError(SERVER_NA_REASON.to_owned()))
}

/// The SPD-rated core timings for one module: the four JEDEC core timings
/// (`cl` / `rcdrd` / `rcwdwr` / `rp` / `ras` + the synthesized `rc`) from the
/// module's factory profile (profile 1); every other MC-live timing is
/// [`server_na_u16`]. A module with no usable profile degrades all its
/// timing cells to the same honest reason.
fn server_timings(module: &SpdModule) -> TimingSet {
    let rating = |v: Option<u8>| match v {
        Some(t) if t > 0 => Section::Value(u16::from(t)),
        _ => server_na_u16(),
    };
    let (cl, trcd, trp, tras) = match module.profiles.first() {
        Some(p) => (
            p.cas.value().copied(),
            p.trcd.value().copied(),
            p.trp.value().copied(),
            p.tras.value().copied(),
        ),
        None => (None, None, None, None),
    };
    // tRC = tRAS + tRP (the JEDEC identity — Intel exposes no tRC register),
    // when both decode.
    let rc = match (tras, trp) {
        (Some(a), Some(r)) if a > 0 && r > 0 => Section::Value(u16::from(a) + u16::from(r)),
        _ => server_na_u16(),
    };
    TimingSet {
        cl: rating(cl),
        rcwdwr: rating(trcd),
        rcdrd: rating(trcd),
        rp: rating(trp),
        ras: rating(tras),
        rc,
        rrds: server_na_u16(),
        rrld: server_na_u16(),
        faw: server_na_u16(),
        wtrs: server_na_u16(),
        wtrl: server_na_u16(),
        wr: server_na_u16(),
        rfc1: server_na_u16(),
        rfc2: server_na_u16(),
        rfcsb: server_na_u16(),
        cwl: server_na_u16(),
        rtp: server_na_u16(),
        rdwr: server_na_u16(),
        wrrd: server_na_u16(),
        rdrd_sd: server_na_u16(),
        rdrd_dd: server_na_u16(),
        rdrd_scl: server_na_u16(),
        rdrd_sc: server_na_u16(),
        wrwr_sd: server_na_u16(),
        wrwr_dd: server_na_u16(),
        wrwr_scl: server_na_u16(),
        wrwr_sc: server_na_u16(),
    }
}

/// The server-uncore all-Na timing set (no SPD module present): every cell
/// the honest server-uncore reason ([`SERVER_NA_REASON`]).
fn server_na_timings() -> TimingSet {
    TimingSet {
        cl: server_na_u16(),
        rcwdwr: server_na_u16(),
        rcdrd: server_na_u16(),
        rp: server_na_u16(),
        ras: server_na_u16(),
        rc: server_na_u16(),
        rrds: server_na_u16(),
        rrld: server_na_u16(),
        faw: server_na_u16(),
        wtrs: server_na_u16(),
        wtrl: server_na_u16(),
        wr: server_na_u16(),
        rfc1: server_na_u16(),
        rfc2: server_na_u16(),
        rfcsb: server_na_u16(),
        cwl: server_na_u16(),
        rtp: server_na_u16(),
        rdwr: server_na_u16(),
        wrrd: server_na_u16(),
        rdrd_sd: server_na_u16(),
        rdrd_dd: server_na_u16(),
        rdrd_scl: server_na_u16(),
        rdrd_sc: server_na_u16(),
        wrwr_sd: server_na_u16(),
        wrwr_dd: server_na_u16(),
        wrwr_scl: server_na_u16(),
        wrwr_sc: server_na_u16(),
    }
}

/// The server-uncore CAD-bus cells: every field the honest server-uncore
/// reason ([`SERVER_NA_REASON`]) — the IMC window does not expose drive
/// strengths and there is no OS-accessible source on these platforms.
fn server_na_cad_bus() -> CadBus {
    na_cad_bus(&NaReason::ParseError(SERVER_NA_REASON.to_owned()))
}

/// The server-uncore voltage cells: every rail the honest server-uncore
/// reason ([`SERVER_NA_REASON`]) — the IMC window does not expose memory
/// rails and there is no OS-accessible source on these platforms.
fn server_na_voltages() -> VoltageSet {
    na_voltages(&NaReason::ParseError(SERVER_NA_REASON.to_owned()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpuid::IntelGen;
    use crate::intel_smbios::DerivedMode;
    use crate::spd_decode::SpdProfile;

    /// A populated SMBIOS Type 17 topology (2 × 8 GiB DDR4 across two
    /// channels — the x99 reference shape).
    fn server_smbios() -> SmbiosTopology {
        SmbiosTopology {
            populated_count: 2,
            dimm_sizes: vec![8192, 8192],
            total_capacity_mib: 16384,
            memory_type: Some(MemoryTech::Ddr4),
            speed_mts: Some(2400),
            locators: vec!["A1".to_owned(), "B1".to_owned()],
            derived_mode: DerivedMode::Dual,
        }
    }

    /// A DDR4 SPD module with a factory profile carrying the given rated
    /// timings.
    fn server_spd_module(
        index: u8,
        speed_mts: u16,
        cl: u8,
        trcd: u8,
        trp: u8,
        tras: u8,
    ) -> SpdModule {
        SpdModule {
            index,
            is_ddr5: false,
            maker: Section::Value("Samsung".to_owned()),
            die_maker: Section::Value("Samsung".to_owned()),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::Value(16),
            part: Section::Value("M393B2K40DB0-ITD".to_owned()),
            serial: Section::Value("0ABC1234".to_owned()),
            rank: Section::Value(1),
            density_mbit: Section::Value(16384),
            speed_mts: Section::Value(speed_mts),
            profiles: vec![SpdProfile {
                index: 1,
                speed_mts: Section::Value(speed_mts),
                cas: Section::Value(cl),
                trcd: Section::Value(trcd),
                trp: Section::Value(trp),
                tras: Section::Value(tras),
                voltage: Section::Value(1200),
            }],
        }
    }

    /// `is_server_gen` classifies the four server-uncore generations and
    /// rejects every client / unprofiled generation.
    #[test]
    fn is_server_gen_classifies_server_platforms() {
        for gen in [
            IntelGen::HaswellE,
            IntelGen::BroadwellE,
            IntelGen::SkylakeX,
            IntelGen::CascadeLake,
        ] {
            assert!(is_server_gen(gen), "{gen:?} is a server gen");
        }
        for gen in [
            IntelGen::Skylake,
            IntelGen::KabyLake,
            IntelGen::CoffeeLake,
            IntelGen::CometLake,
            IntelGen::RocketLake,
            IntelGen::AlderLake,
            IntelGen::RaptorLake,
            IntelGen::MeteorLake,
            IntelGen::ArrowLake,
            IntelGen::Unrecognized,
        ] {
            assert!(!is_server_gen(gen), "{gen:?} is not a server gen");
        }
    }

    /// The Broadwell-E (x99 reference) server decode: 4 channels, MCLK from
    /// the SPD rated speed (2400 MT/s → 1200 MHz), the four SPD-rated core
    /// timings (+ the synthesized tRC) as `Value`s, every other MC-live
    /// timing the honest server-uncore N/A, the SMBIOS topology carried
    /// (dual-channel, DDR4, 2 DIMMs), and the honest source annotation.
    #[test]
    fn decode_server_broadwell_e_sources_smbios_and_spd() {
        let smbios = server_smbios();
        let spd = vec![
            server_spd_module(0x50, 2400, 16, 16, 16, 32),
            server_spd_module(0x51, 2400, 16, 16, 16, 32),
        ];
        let ro = decode_server(&smbios, &spd, IntelGen::BroadwellE);

        // Quad-channel Broadwell-E.
        assert_eq!(ro.channels.len(), 4, "Broadwell-E is quad-channel");
        // MCLK = SPD rated speed / 2 (2400 MT/s → 1200 MHz); UCLK = MCLK
        // (the MC runs at the DRAM clock).
        assert_eq!(ro.channels[0].clocks.mclk_mhz, Section::Value(1200.0));
        assert_eq!(ro.channels[0].clocks.uclk_mhz, Section::Value(1200.0));
        // The four SPD-rated core timings + the synthesized tRC.
        let t = &ro.channels[0].timings;
        assert_eq!(t.cl, Section::Value(16));
        assert_eq!(t.rcdrd, Section::Value(16));
        assert_eq!(t.rcwdwr, Section::Value(16));
        assert_eq!(t.rp, Section::Value(16));
        assert_eq!(t.ras, Section::Value(32));
        assert_eq!(t.rc, Section::Value(48), "tRC = tRAS + tRP");
        // Every other MC-live timing: the honest server-uncore N/A.
        let na = NaReason::ParseError(SERVER_NA_REASON.to_owned());
        assert_eq!(t.rrds, Section::Na(na.clone()));
        assert_eq!(t.faw, Section::Na(na.clone()));
        assert_eq!(t.rfc1, Section::Na(na.clone()));
        assert_eq!(t.rdrd_dd, Section::Na(na.clone()));
        // CAD bus / voltages / RTL: the honest server-uncore reason (not a
        // generic "not applicable" — there is no OS-accessible source).
        assert_eq!(ro.channels[0].cad_bus.proc_odt, Section::Na(na.clone()));
        assert_eq!(ro.channels[0].voltages.vddio_mem_mv, Section::Na(na.clone()));
        assert_eq!(ro.channels[0].rtl, Section::Na(na));
        // The SMBIOS topology is carried (dual-channel, DDR4, 2 DIMMs).
        assert_eq!(ro.channel_mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr4));
        assert_eq!(ro.dimm_count, 2);
        assert_eq!(ro.ecc_status, EccStatus::Unknown);
        // The honest source annotation names the server-uncore limitation,
        // the SPD-rated provenance, and the populated/total channel count.
        assert!(ro.channel_annotation.contains("Server uncore"));
        assert!(ro.channel_annotation.contains("SPD-rated"));
        assert!(ro.channel_annotation.contains("2 of 4"));
    }

    /// An empty SPD list: the MCLK / UCLK now come from the SMBIOS
    /// Configured Memory Speed (the server-uncore fallback — the SPD EEPROM
    /// is not OS-visible on these platforms), while the SPD-rated timings
    /// degrade to the honest server-uncore N/A. The SMBIOS topology still
    /// carries the readout (no panic, no invention).
    #[test]
    fn decode_server_empty_spd_degrades_timings_keeps_smbios() {
        let smbios = server_smbios();
        let ro = decode_server(&smbios, &[], IntelGen::BroadwellE);
        assert_eq!(ro.channels.len(), 4);
        // The SMBIOS speed (2400 MT/s) fills the MCLK / UCLK.
        assert_eq!(ro.channels[0].clocks.mclk_mhz, Section::Value(1200.0));
        assert_eq!(ro.channels[0].clocks.uclk_mhz, Section::Value(1200.0));
        // The SPD-rated timings have no module → the honest server-uncore N/A.
        assert!(ro.channels[0].timings.cl.is_na());
        // The SMBIOS topology still resolves.
        assert_eq!(ro.channel_mode, Some(ChannelMode::DualSymmetric));
        assert_eq!(ro.dimm_count, 2);
        assert_eq!(ro.memory_tech, Some(MemoryTech::Ddr4));
    }

    /// No SPD module **and** no SMBIOS speed (the fully-degraded server
    /// case): the MCLK / UCLK degrade to `Na(NotApplicable)` — never a
    /// fabricated value — and the timings carry the honest server-uncore
    /// reason.
    #[test]
    fn decode_server_no_speed_anywhere_degrades_clocks() {
        let ro = decode_server(&SmbiosTopology::empty(), &[], IntelGen::BroadwellE);
        assert_eq!(ro.channels.len(), 4);
        assert_eq!(
            ro.channels[0].clocks.mclk_mhz,
            Section::na(NaReason::NotApplicable)
        );
        assert_eq!(
            ro.channels[0].clocks.uclk_mhz,
            Section::na(NaReason::NotApplicable)
        );
        let na = NaReason::ParseError(SERVER_NA_REASON.to_owned());
        assert_eq!(ro.channels[0].timings.cl, Section::Na(na.clone()));
        assert_eq!(ro.channels[0].rtl, Section::Na(na));
    }

    /// Cascade Lake is the hex-channel server profile (6 channels).
    #[test]
    fn decode_server_cascade_lake_is_hex_channel() {
        let ro = decode_server(&SmbiosTopology::empty(), &[], IntelGen::CascadeLake);
        assert_eq!(ro.channels.len(), 6, "Cascade Lake is hex-channel");
    }

    /// A module with a speed but no usable profile: the MCLK still decodes
    /// from the rated speed, but the SPD-rated core timings degrade to the
    /// honest server-uncore N/A (never a fabricated zero).
    #[test]
    fn decode_server_module_without_profile_degrades_to_na() {
        let smbios = server_smbios();
        let mut m = server_spd_module(0x50, 2400, 0, 0, 0, 0);
        m.profiles.clear();
        let ro = decode_server(&smbios, std::slice::from_ref(&m), IntelGen::HaswellE);
        assert_eq!(ro.channels[0].clocks.mclk_mhz, Section::Value(1200.0));
        assert!(ro.channels[0].timings.cl.is_na());
        assert!(ro.channels[0].timings.ras.is_na());
        // Haswell-E is quad-channel like Broadwell-E.
        assert_eq!(ro.channels.len(), 4);
    }

    /// M-10: a corrupt SMBIOS Configured Memory Speed (a `0xFFFF` 16-bit
    /// readback → 65535 MT/s → 32767.5 MHz) is outside the plausible clock
    /// range, so the derived MCLK is N/A'd with a proper reason instead of
    /// being displayed as real data.
    #[test]
    fn decode_server_out_of_range_smbios_speed_is_na() {
        let mut smbios = SmbiosTopology::empty();
        smbios.speed_mts = Some(0xFFFF);
        let ro = decode_server(&smbios, &[], IntelGen::BroadwellE);
        assert!(
            matches!(
                ro.channels[0].clocks.mclk_mhz,
                Section::Na(NaReason::ParseError(_))
            ),
            "out-of-range SMBIOS speed degrades the MCLK to Na(ParseError): {:?}",
            ro.channels[0].clocks.mclk_mhz
        );
    }

    /// M-16: the server-uncore annotation reports the populated CHANNEL
    /// count and the raw DIMM count separately — never the old
    /// self-contradictory "8 of 4 channels populated" (the DIMM count in
    /// the channel slot).
    #[test]
    fn decode_server_annotation_reports_channels_and_dimms() {
        let smbios = server_smbios(); // 2 DIMMs
        let spd = vec![
            server_spd_module(0x50, 2400, 16, 16, 16, 32),
            server_spd_module(0x51, 2400, 16, 16, 16, 32),
        ];
        let ro = decode_server(&smbios, &spd, IntelGen::BroadwellE);
        assert!(
            ro.channel_annotation.contains("2 of 4"),
            "the populated channel count is in the annotation: {}",
            ro.channel_annotation
        );
        assert!(
            ro.channel_annotation.contains("2 DIMMs"),
            "the raw DIMM count is reported separately: {}",
            ro.channel_annotation
        );
    }
}
