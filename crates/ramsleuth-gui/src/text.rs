//! The spec's 3-line header (Grand Design §3.1, C6-20): the pure line
//! builders (unit-tested without an egui context — no display needed)
//! that `render_header` composes into the top panel; the capacity +
//! clock segments honor the live `units` knob (C7-11). Also the
//! header's daemon-status indicator, the transient-notice color, and
//! the About modal body.

use std::fmt::Write as _;

use ramsleuth_display::{
    age_fragment, cell_text, channel_mode, dimm_summary as display_dimm_summary, ecc_label,
    intel_mode_text, mclk_derived_speed, slot_note, sync_mode_from_clocks as display_sync_mode,
};
use ramsleuth_gui::{
    format_capacity, format_clock, CapacityUnit, ClockUnit, TelemetryData, Units, AMBER, CRIMSON,
    CYAN,
};
use ramsleuth_telemetry::amd_readout::{ClockReadout, DivMode, MemoryChannelMode};
use ramsleuth_telemetry::cpuid::CpuVendor;
use ramsleuth_telemetry::error::Section;
use ramsleuth_telemetry::intel_readout::{ChannelMode, ChannelTrust};
use ramsleuth_telemetry::spd_decode::SpdModule;
use ramsleuth_telemetry::SystemMemoryTelemetry;

/// The header's compact daemon-status indicator: the dot color
/// (GREEN when connected and healthy — no recorded error; RED
/// otherwise — the status zone's `daemon_status_color` rule, copied
/// here for the top strip) + the short label (`Daemon OK` /
/// `Daemon`). The socket path is not rendered inline (the former
/// verbose `Daemon: Connected (IPC: …)` line drove the header's
/// text collision at the 968 px default width) — it rides the
/// label's hover tooltip.
pub fn daemon_status_indicator(data: &TelemetryData) -> (egui::Color32, &'static str) {
    let healthy = !data.daemon_status.is_empty()
        && data.daemon_status.starts_with("connected")
        && data.error.is_none();
    if healthy {
        (crate::DAEMON_OK_GREEN, "Daemon OK")
    } else {
        (crate::DAEMON_DOWN_RED, "Daemon")
    }
}

/// The header's transient-notice color: CYAN for a written file
/// ("wrote …"), CRIMSON for a failed export ("! …"), AMBER for the
/// informational "nothing to export yet" hint.
pub fn notice_color(text: &str) -> egui::Color32 {
    if text.starts_with('!') {
        CRIMSON
    } else if text.starts_with("wrote ") {
        CYAN
    } else {
        AMBER
    }
}

/// The About/Help modal's body (chunk gui-about): the user-facing
/// explanation of what RamSleuth is, how the daemon model works,
/// what the app can do, the Intel/AMD split, and where the full
/// guide lives. Pure (the only non-static piece is the package
/// version, tracked live via `env!`): the render maps each
/// `(heading, body)` pair to a styled heading + wrapped text, and
/// the (a1) test pins the content surface.
pub fn about_body() -> Vec<(&'static str, String)> {
    vec![
        (
            "What RamSleuth is",
            "Live RAM telemetry + a Memory & Latency Benchmark \
             for AMD and Intel desktops. Watch your memory in real \
             time — clocks, timings, voltages, channel mode, ECC — \
             and measure how fast your memory subsystem really is."
                .to_owned(),
        ),
        (
            "How it works",
            "A small daemon runs as root with only the CAP_SYS_RAWIO \
             capability. It reads the hardware sources — the AMD SMU \
             PM tables, the Intel MCHBAR IMC registers, and the SPD \
             EEPROMs on each DIMM — and serves them to this GUI over \
             a local Unix socket. This GUI is unprivileged: it only \
             talks to the daemon."
                .to_owned(),
        ),
        (
            "What it can do",
            "\u{2022} Live telemetry: clocks, timings, voltages, the \
             AMD UCLK / MCLK mode\n\
             \u{2022} SPD module details: part, serial, rank, speed, \
             XMP / EXPO profiles\n\
             \u{2022} Channel mode + ECC status (both platforms)\n\
             \u{2022} Memory & Latency Benchmark: read / write / \
             copy / latency\n\
             \u{2022} Benchmark burn-in for stress testing\n\
             \u{2022} Probe report: a consent-gated snapshot for bug \
             reports (the header's Probe button)"
                .to_owned(),
        ),
        (
            "Intel vs AMD",
            "Intel: per-channel IMC subtimings decoded from the \
             MCHBAR registers (Tier 1–3, Skylake → Arrow \
             Lake).\nAMD: SMU PM clocks, voltages, and the UCLK / MCLK \
             CAD bus mode via the ryzen-smu driver."
                .to_owned(),
        ),
        (
            "This build",
            format!(
                "RamSleuth v{} — full guide: Docs/User_Guide.md \
                 (or the README).",
                env!("CARGO_PKG_VERSION")
            ),
        ),
    ]
}

/// Line 1's platform badge (the small muted pill): the CPU vendor's
/// short word — `AMD` / `Intel` / `Unknown` — a far narrower take on
/// the former `[AMD AM5 Platform]` tag (whose socket-family detail
/// the CPU brand on line 2 already carries; the extra width drove
/// the header's text collision at the 968 px default).
pub fn platform_badge(vendor: &CpuVendor) -> &'static str {
    match vendor {
        CpuVendor::Amd(_) => "AMD",
        CpuVendor::Intel(_) => "Intel",
        CpuVendor::Unknown => "Unknown",
    }
}

/// Line 2 (the mockup's `CPU: AMD Ryzen 9 7950X 16-Core @ 5.70 GHz |
/// Motherboard: … (BIOS: …, AGESA …)`): the CPUID brand + the
/// platform clock in the selected clock unit (C7-11: `format_clock`
/// — the default MHz keeps the carried wire value, the GHz arm
/// ÷1000), then the DMI motherboard / BIOS cells + the AGESA/SMU
/// provenance fragment ([`age_fragment`] — `AGESA <v>` for a true
/// AGESA, `SMU <v>` for the ryzen_smu firmware version, `AGESA N/A`
/// otherwise — C8-11, D-1) — every `Na` cell degrades to its `N/A`
/// text (never a panic).
pub fn cpu_line_text(t: &SystemMemoryTelemetry, units: &Units) -> String {
    let platform = &t.platform;
    let clock = match platform.cpu_clock_mhz.value() {
        Some(mhz) => format_clock(*mhz, units),
        None => "N/A".to_owned(),
    };
    format!(
        "CPU: {} @ {} | Motherboard: {} (BIOS: {}, {})",
        t.cpu.brand,
        clock,
        cell_text(&platform.motherboard),
        cell_text(&platform.bios),
        age_fragment(platform),
    )
}

/// The per-DIMM capacity summary (the mockup's `2x32GB`): one
/// `<count>x<size>` group per distinct carried size (first-seen
/// order, ` + `-joined) with the size rendered in the selected
/// capacity unit (C7-11: `format_capacity` — the `x`-group prefix is
/// kept); C9-01 (D-1): the group key is `(size, rank word)` — the
/// rank word from the parallel `spd` slice (the C8-03 wire field,
/// positionally aligned with `sizes`, the facade contract) is
/// appended to the group when present (`2x16 GiB Single-Rank`; a
/// `Na`/`0` rank omits the word, degrading to the bare `2x16 GiB`);
/// the `Na` size entries contribute nothing, and an all-`Na` /
/// empty list degrades to `N/A`.
/// The grouping logic is the shared `ramsleuth-display` `dimm_summary`
/// (WS-A, C3) — this is a thin adapter over the GUI's `Units` knob.
pub fn dimm_summary(sizes: &[Section<f64>], spd: &[SpdModule], units: &Units) -> String {
    display_dimm_summary(sizes, spd, units.capacity == CapacityUnit::GiB)
}

/// Line 3's non-mode part (the mockup's `RAM: 64.0 GB (2x32GB)
/// DDR5-6000 MT/s | Dual-Channel | ECC: … | Mode: `): the total
/// capacity in the selected capacity unit (C7-11:
/// `format_capacity` — the default GiB keeps the carried wire value,
/// the GB arm converts × 1.073741824), the per-DIMM breakdown
/// (C9-01, D-1: with the rank word from the parallel SPD list —
/// `2x16 GiB Single-Rank`), the speed segment (the max SPD speed; the
/// platform MCLK×2 fallback when no module carries one —
/// [`mclk_derived_speed`]; omitted only when neither is available),
/// the channel mode (hardware-preferred — the Intel
/// `MAD_INTER_CHANNEL` mode, else the AMD UMC mode; the SPD-count
/// [`channel_mode`] when neither readout carries one), the
/// total-vs-breakdown slot note when the OS total strictly exceeds the
/// SPD sum (C9-01, D-1: `2 of 4 slots SPD-visible`), the ECC
/// status ([`ecc_label`] — the platform [`EccStatus`] label, the
/// honest `N/A` when unreadable / absent), and the `Mode: ` lead-in
/// the mode segment completes.
pub fn ram_line_prefix(t: &SystemMemoryTelemetry, units: &Units) -> String {
    let total = match t.total_capacity.value() {
        Some(gib) => format_capacity(*gib, units),
        None => "N/A".to_owned(),
    };
    let mut line = format!(
        "RAM: {total} ({})",
        dimm_summary(&t.dimm_sizes, &t.spd, units)
    );
    // The max SPD speed; the platform MCLK×2 fallback when no bound
    // module carries one (omitted only when neither is available).
    let speed = t
        .spd
        .iter()
        .filter_map(|m| m.speed_mts.value().copied())
        .max()
        .map(|mts| format!("{mts} MT/s"))
        .or_else(|| mclk_derived_speed(t));
    if let Some(speed) = speed {
        let _ = write!(line, " {speed}");
    }
    // Single source of truth (Chunk 2): the Intel readout's trust-resolved
    // channel label ([`IntelReadout::channel_display_label`] — the same
    // string the TUI renders, so the frontends agree), the AMD
    // UMC-synthesized mode on AMD hosts, the SPD-count label only when
    // neither readout carries a resolved mode.
    let channel = match (t.intel.value(), t.amd.value()) {
        (Some(ro), _) => ro
            .channel_display_label()
            .unwrap_or_else(|| channel_mode(t.dimm_sizes.len())),
        (None, Some(ro)) => match ro.channel_mode {
            MemoryChannelMode::Unknown => "N/A".to_owned(),
            m => m.label().to_owned(),
        },
        (None, None) => channel_mode(t.dimm_sizes.len()),
    };
    let _ = write!(line, " | {channel}");
    if let Some(note) = slot_note(&t.total_capacity, &t.dimm_sizes) {
        let _ = write!(line, " | {note}");
    }
    let _ = write!(line, " | {}", ecc_label(t));
    line.push_str(" | Mode: ");
    line
}

/// Line 3's mode segment (D-C8): the UCLK:MCLK ratio **text** from the
/// shared `ramsleuth-display` (the single source the TUI renders too) +
/// its semantic color (GUI-local) — AMBER for `Synchronous 1:1` (with
/// the MCLK in the selected clock unit when it carries one), CRIMSON for
/// `Asynchronous 1:2`, and `None` (the default text color) for the
/// honest `N/A`.
pub fn sync_mode_from_clocks(
    clocks: &ClockReadout,
    units: &Units,
) -> (String, Option<egui::Color32>) {
    let text = display_sync_mode(clocks, units.clock == ClockUnit::MHz);
    let color = match clocks.div_mode.value() {
        Some(DivMode::OneToOne) => Some(AMBER),
        Some(DivMode::OneToTwo) => Some(CRIMSON),
        None => None,
    };
    (text, color)
}

/// Line 3's mode segment over a whole snapshot: the AMD branch must
/// carry a value whose `div_mode` is usable, else the honest `N/A` —
/// the driver-missing / degraded AMD states degrade here (never a
/// panic). The Intel hardware channel mode takes the slot over this
/// ([`intel_mode`] runs first); on Intel this yields the `N/A`
/// fallback only when no mode is present.
pub fn sync_mode(t: &SystemMemoryTelemetry, units: &Units) -> (String, Option<egui::Color32>) {
    match t.amd.value() {
        Some(readout) => sync_mode_from_clocks(&readout.clocks, units),
        None => ("N/A".to_owned(), None),
    }
}

/// Line 3's mode segment over an Intel readout (the trust-resolved
/// `channel_mode`): `DualSymmetric` → `Interleaved` (CYAN, the default),
/// `DualFlex` → `Flex` (AMBER, the asymmetric configuration worth
/// flagging), `Single` → `N/A` (the default color — a single channel has
/// no interleave mode; the channel slot carries `Single-Channel`). The
/// Chunk 2 trust badge rides the label: the L3 override / L4 heuristic
/// (unverified) tiers append `(unverified)` and render AMBER; the L2
/// firmware-verified tier appends `(firmware)`. A non-Intel / no-mode
/// readout yields `None` and the caller falls back to the AMD sync mode
/// (or the honest `N/A`).
pub fn intel_mode(t: &SystemMemoryTelemetry) -> Option<(String, Option<egui::Color32>)> {
    let ro = t.intel.value()?;
    let mode = ro.channel_mode?;
    // The label (the mode + the Chunk 2 trust badge) is the shared
    // `ramsleuth-display` `intel_mode_text` (the single source the TUI
    // renders too); the semantic color is GUI-local.
    let label = intel_mode_text(t)?;
    let color = match (mode, ro.channel_trust) {
        (_, Some(ChannelTrust::Override) | Some(ChannelTrust::Heuristic)) => Some(AMBER),
        (ChannelMode::DualFlex, _) => Some(AMBER),
        (ChannelMode::Single, _) => None,
        (ChannelMode::DualSymmetric, _) => Some(CYAN),
    };
    Some((label, color))
}
