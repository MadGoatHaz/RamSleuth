//! Zone 3 renderer: the hardware/SPD module cards + the daemon status
//! line + the F2 / F3 / Q actions row (P3-29).
//!
//! Grand Design §3.1 bottom-right panel: one small framed "card" per
//! bound SPD slot (a `slot 0xNN (DDR4|DDR5)` header over the
//! [`spd_cards`] rows — a product line, a DRAM-die line, a human rank
//! label, the raw maker / part / rank / density / speed cells, each
//! its [`Section`] value in CYAN or a bare `N/A` in muted `NA_GRAY`, plus
//! one row per XMP / EXPO profile (or a `profiles: none` placeholder)),
//! the daemon status line (`daemon: <status>` + the last-update stamp
//! `· <N.N>s ago`, or `· never` when no poll has landed; CYAN when
//! connected and healthy, CRIMSON when disconnected or errored), the
//! structured error line when present (CRIMSON), and the `ACTIONS`
//! row — `F2 · Snapshot PNG`, `F3 · Export JSON`, `Q · Quit` —
//! reported back as a [`GuiAction`] for the app shell (P3-30) to
//! execute (the render thread itself does no I/O, D6).
//!
//! **No-panic contract (D5):** the zone reads only a `&TelemetryData`
//! snapshot: no telemetry renders one gray placeholder, an empty
//! SPD list renders a gray `driver missing` placeholder, an all-`Na`
//! module renders every row a bare `N/A` in muted `NA_GRAY` — never a panic.
//!
//! **Pure core:** [`spd_cards`] (+ the daemon-status line/color and
//! the card value-color pickers) is I/O-free and deterministic (the
//! unit tests exercise it without an egui context);
//! [`render_status_zone`] is the thin `egui` surface over it (the
//! live render is verified in the QA phase).

use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::spd_decode::{SpdModule, SpdProfile};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::update::TelemetryData;
use crate::{CRIMSON, CYAN, NA_GRAY, SLATE};

/// The zone title (Grand Design §3.1, bottom-right panel).
const ZONE_TITLE: &str = "3 · HARDWARE & SPD";

/// One user action triggered in the status zone, reported back to the
/// app shell (P3-30) which performs the side effect — F2 writes the
/// PNG snapshot, F3 the JSON export (both into the CWD), Q closes the
/// viewport. The render thread never does I/O itself (D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiAction {
    /// No action button was clicked this frame.
    None,
    /// `F2 · Snapshot PNG`: snapshot the current benchmark grid.
    SnapshotPng,
    /// `F3 · Export JSON`: export the current telemetry snapshot.
    ExportJson,
    /// `Q · Quit`: close the application.
    Quit,
}

// ---------------------------------------------------------------------
// The pure core (testable: no I/O, no egui context).
// ---------------------------------------------------------------------

/// The per-slot SPD module cards: one `Vec` of `(label, display)` pairs
/// per bound [`SpdModule`] (a product line, a DRAM-die line, a human
/// rank label, the raw maker / part / rank / density / speed cells,
/// then one row per XMP / EXPO profile — or a `profiles: none`
/// placeholder when the module carries none).
///
/// Pure and deterministic: the same snapshot always yields the same
/// `Vec`. Each display is the formatted [`Section::Value`] (the
/// `<maker> (<part>)` product line, the `<die_maker> (<die_type>,
/// <density>Gb)` die line with each absent part dropped, the
/// `Single-Rank` / `Dual-Rank` / `<n>-Rank` rank label, bare rank,
/// `… Mbit` density, `… MT/s` speed, the profile line
/// `<speed> <cl>-<trcd>-<trp>-<tras> @ <volts>`) or a bare `N/A`
/// for a [`Section::Na`] (the reason stays on the wire as [`NaReason`]
/// — the GUI drops the parenthetical, D-4); an all-`Na` module
/// renders every row a bare `N/A` in muted `NA_GRAY` and never
/// panics. Returns an empty `Vec` when the snapshot carries no SPD
/// modules (the renderer draws its own placeholder).
pub fn spd_cards(telemetry: &SystemMemoryTelemetry) -> Vec<Vec<(String, String)>> {
    telemetry.spd.iter().map(card_rows).collect()
}

/// The `(label, display)` rows of one SPD module card: the product
/// line, the DRAM-die line, the human rank label, the raw
/// maker / part / rank / density / speed cells, then the profile
/// rows.
fn card_rows(module: &SpdModule) -> Vec<(String, String)> {
    let mut rows = vec![
        ("product".to_owned(), product_line(&module.maker, &module.part)),
        ("dram die".to_owned(), dram_die_line(module)),
        ("rank label".to_owned(), rank_label(&module.rank)),
        ("maker".to_owned(), display(&module.maker, |value: &String| value.clone())),
        ("part".to_owned(), display(&module.part, |value: &String| value.clone())),
        ("rank".to_owned(), display(&module.rank, |value: &u8| value.to_string())),
        (
            "density".to_owned(),
            display(&module.density_mbit, |value: &u16| format!("{value} Mbit")),
        ),
        (
            "speed".to_owned(),
            display(&module.speed_mts, |value: &u16| format!("{value} MT/s")),
        ),
    ];
    if module.profiles.is_empty() {
        rows.push(("profiles".to_owned(), "none".to_owned()));
    } else {
        for profile in &module.profiles {
            rows.push(profile_row(module.is_ddr5, profile));
        }
    }
    rows
}

/// The product-line row: `<maker> (<part>)` (the spec's "G.Skill …
/// (F5-…)" form). One `Na` partner renders the other bare (no empty
/// parens); both `Na` degrades the whole row to the maker's reason
/// text. Never a panic.
fn product_line(maker: &Section<String>, part: &Section<String>) -> String {
    match (maker, part) {
        (Section::Value(maker), Section::Value(part)) => format!("{maker} ({part})"),
        (Section::Value(maker), Section::Na(_)) => maker.clone(),
        (Section::Na(_), Section::Value(part)) => part.clone(),
        (Section::Na(reason), _) => na_text(reason),
    }
}

/// The DRAM-die row: `<die_maker> (<die_type>, <density>Gb)` with
/// each parenthetical part dropped when absent — a `Na` die type
/// yields `<die_maker> (<density>Gb)`, a `Na` density omits the
/// density, and both absent shows the die maker bare. A `Na` die
/// maker degrades the whole row to its reason text. Never a panic.
fn dram_die_line(module: &SpdModule) -> String {
    match &module.die_maker {
        Section::Na(reason) => na_text(reason),
        Section::Value(die_maker) => {
            let mut parts = Vec::new();
            if let Section::Value(die_type) = &module.die_type {
                parts.push(die_type.clone());
            }
            if let Section::Value(mbit) = &module.density_mbit {
                parts.push(density_gib(*mbit));
            }
            if parts.is_empty() {
                die_maker.clone()
            } else {
                format!("{die_maker} ({})", parts.join(", "))
            }
        }
    }
}

/// The density cell in Gb: `Mbit ÷ 1024` (16384 → `16Gb`); a
/// non-integer conversion (not a real-world density) keeps the raw
/// `Mbit` form — deterministic, never a panic.
fn density_gib(mbit: u16) -> String {
    if mbit % 1024 == 0 {
        format!("{}Gb", mbit / 1024)
    } else {
        format!("{mbit} Mbit")
    }
}

/// The human rank label: `1` → `Single-Rank`, `2` →
/// `Dual-Rank`, other positive counts → `<n>-Rank` (e.g.
/// `4-Rank`); a `Na` rank (or a degenerate `0` value) degrades to the
/// `N/A` reason text. The raw rank number stays on its own row.
fn rank_label(rank: &Section<u8>) -> String {
    match rank {
        Section::Na(reason) => na_text(reason),
        Section::Value(value) => match value {
            0 => na_text(&NaReason::NotApplicable),
            1 => "Single-Rank".to_owned(),
            2 => "Dual-Rank".to_owned(),
            other => format!("{other}-Rank"),
        },
    }
}

/// One profile row: the label is the scheme + slot (`XMP <n>` on
/// DDR4, `EXPO <n>` on DDR5), the display the `<speed> <cl>-<trcd>-
/// <trp>-<tras> @ <volts>` summary — an `N/A` field degrades just
/// that field, never the whole row (and never panics).
fn profile_row(is_ddr5: bool, profile: &SpdProfile) -> (String, String) {
    let scheme = if is_ddr5 { "EXPO" } else { "XMP" };
    let speed = display(&profile.speed_mts, |value: &u16| format!("{value} MT/s"));
    let tick = |section: &Section<u8>| display(section, |value: &u8| value.to_string());
    let voltage =
        display(&profile.voltage, |value: &u16| format!("{:.3} V", f64::from(*value) / 1000.0));
    (
        format!("{scheme} {}", profile.index),
        format!(
            "{speed} {}-{}-{}-{} @ {voltage}",
            tick(&profile.cas),
            tick(&profile.trcd),
            tick(&profile.trp),
            tick(&profile.tras)
        ),
    )
}

/// One [`Section`] cell's display: the formatted value, or a bare
/// `N/A` for an absent one (D-4 — the reason stays on the wire).
fn display<T>(section: &Section<T>, fmt: impl Fn(&T) -> String) -> String {
    match section {
        Section::Value(value) => fmt(value),
        Section::Na(reason) => na_text(reason),
    }
}

/// The human text of an absent cell: bare `N/A` (D-4 — the reason
/// stays on the wire as [`NaReason`]; the GUI drops the parenthetical
/// as verbose — the zone 1 / TUI precedent).
fn na_text(_reason: &NaReason) -> String {
    "N/A".to_owned()
}

/// The daemon status line: `daemon: <status>` (an empty status reads
/// `not connected` — the TUI zone-3 precedent) + the last-update
/// stamp — ` · <N.N>s ago` when a poll has landed, ` · never`
/// otherwise.
fn daemon_status_line(data: &TelemetryData) -> String {
    let status = if data.daemon_status.is_empty() {
        "not connected".to_owned()
    } else {
        data.daemon_status.clone()
    };
    let stamp = match data.last_update {
        Some(last) => format!(" · {:.1}s ago", last.elapsed().as_secs_f64()),
        None => " · never".to_owned(),
    };
    format!("daemon: {status}{stamp}")
}

/// The semantic color of the daemon status line: CYAN when connected
/// and healthy, CRIMSON when disconnected (an empty or non-`connected`
/// status) or when the last attempt errored.
fn daemon_status_color(data: &TelemetryData) -> egui::Color32 {
    let healthy = !data.daemon_status.is_empty()
        && data.daemon_status.starts_with("connected")
        && data.error.is_none();
    if healthy {
        CYAN
    } else {
        CRIMSON
    }
}

/// The semantic color of one card row's display: CYAN for a decoded
/// value, muted `NA_GRAY` for an absent one (a bare `N/A`, including
/// a degraded profile field — D-5: unavailable, not a fault).
fn card_value_color(display: &str) -> egui::Color32 {
    if display.contains("N/A") {
        NA_GRAY
    } else {
        CYAN
    }
}

// ---------------------------------------------------------------------
// The egui surface (compile-checked here; the live render is verified
// in the QA phase).
// ---------------------------------------------------------------------

/// Zone 3: render the SPD module cards + the daemon status line + the
/// error line + the `ACTIONS` row from `data` — a titled SLATE frame
/// (the zone 1 / 2 precedent). Returns the [`GuiAction`] triggered
/// this frame (`None` when no button was clicked) for the app shell
/// (P3-30) to execute.
pub fn render_status_zone(ui: &mut egui::Ui, data: &TelemetryData) -> GuiAction {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0));
    let inner = frame.show(ui, |ui| {
        // D-4a/D-4b fill: force the content — and hence this
        // frame's border — to span the full allocated column width
        // and the C9-05 status slice's remaining height; a `Frame`
        // otherwise shrinks to its content's natural size (the
        // SPD-card grid's), leaving dead space to the right of and
        // below the border.
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui.label(egui::RichText::new(ZONE_TITLE).strong().color(CYAN));
        ui.add_space(4.0);
        render_spd_cards(ui, data);
        ui.add_space(4.0);
        let status = daemon_status_line(data);
        ui.label(egui::RichText::new(&status).color(daemon_status_color(data)));
        if let Some(error) = &data.error {
            ui.label(egui::RichText::new(format!("! {error}")).color(CRIMSON));
        }
        ui.add_space(4.0);
        render_actions(ui)
    });
    inner.inner
}

/// The per-slot SPD module cards as a 2-column flow grid (R1):
/// one framed card per bound module (the `slot 0xNN (DDR4|DDR5)`
/// header + the [`spd_cards`] rows), the DIMM cards side-by-side in
/// row pairs — 1 DIMM → `1×1` (left cell), 2 → `1×2`,
/// 3 → `2+1`, 4 → `2×2` balanced — each card allocated
/// half the available inner width, so the cards fill the column
/// with no interior void. An empty SPD list renders a gray
/// `SPD: N/A` placeholder, and no telemetry at all renders one gray
/// placeholder — never a panic (plan D5).
fn render_spd_cards(ui: &mut egui::Ui, data: &TelemetryData) {
    match &data.telemetry {
        Some(telemetry) => {
            if telemetry.spd.is_empty() {
                ui.label(egui::RichText::new("SPD: N/A").color(NA_GRAY));
                return;
            }
            // R1 (D-13.1): row-pair flow — `(n + 1) / 2` rows of up
            // to 2 cards (1 → 1×1 left cell, 2 → 1×2, 3 → 2+1,
            // 4 → 2×2 balanced), each allocated
            // `(avail − item_spacing.x) / 2`: the layout's item
            // spacing lands between the pair, so the two columns
            // sum to the full inner width. The zero-height
            // allocation draws each card at its natural height (the
            // bench-slice precedent, main.rs:949-954); the 4 pt row
            // spacing replaces the old per-card vertical gap.
            let cards = spd_cards(telemetry);
            let n = telemetry.spd.len();
            let col_w = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(0.0);
            let rows = n.div_ceil(2); // (n + 1) / 2, MSRV 1.75
            for r in 0..rows {
                // A `ui.horizontal` row would not do: it is
                // `left_to_right(Align::Center)`, cross-axis centered —
                // with unequal card heights (a populated 0x52 over an
                // all-`Na` 0x53) the second card centers against the
                // first's expanded height instead of top-aligning
                // (staggered, not a grid). An explicit top-aligned
                // row keeps both cards on the row's top edge.
                let _ = ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                    for c in 0..2 {
                        let idx = r * 2 + c;
                        if idx < n {
                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(col_w, 0.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| render_spd_card(ui, &telemetry.spd[idx], &cards[idx]),
                            );
                        }
                    }
                });
                if r + 1 < rows {
                    ui.add_space(4.0);
                }
            }
        }
        None => {
            ui.label(egui::RichText::new("N/A").color(NA_GRAY));
        }
    }
}

/// One SPD module card: a small framed block — the slot header in
/// bold CYAN over a two-column grid of the card's rows (the label in
/// default text, the display in its semantic color).
fn render_spd_card(ui: &mut egui::Ui, module: &SpdModule, card: &[(String, String)]) {
    let generation = if module.is_ddr5 { "DDR5" } else { "DDR4" };
    let header = format!("slot 0x{:02X} ({generation})", module.index);
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0));
    let _ = frame.show(ui, |ui| {
        // D-4a fill (D-13.1): the card's CYAN border spans its
        // allocated half-column — a `Frame` otherwise shrinks to
        // its content's natural width, leaving dead space inside
        // the allocation.
        ui.set_min_width(ui.available_width());
        ui.label(egui::RichText::new(&header).strong().color(CYAN));
        let _ = egui::Grid::new(format!("ramsleuth_spd_card_0x{:02X}", module.index))
            .spacing(egui::vec2(12.0, 1.0))
            .min_col_width(80.0)
            .show(ui, |ui| {
                for (label, display) in card {
                    ui.add(egui::Label::new(egui::RichText::new(label.as_str())));
                    // A long value (a full product line) exceeds
                    // the half-column's inner width in its natural
                    // single-line form — wrapping keeps it inside
                    // the card (no clip, no cross-card overlap).
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(display.as_str()).color(card_value_color(display)),
                        )
                        .wrap(true),
                    );
                    ui.end_row();
                }
            });
    });
}

/// The `ACTIONS` row: three buttons — `F2 · Snapshot PNG`, `F3 ·
/// Export JSON`, `Q · Quit` — reporting which was clicked this frame
/// as a [`GuiAction`] (`None` when nothing was clicked). The app
/// shell (P3-30) performs the side effect; the render thread does no
/// I/O itself (D6).
fn render_actions(ui: &mut egui::Ui) -> GuiAction {
    let inner = ui.horizontal(|ui| {
        if ui.button("F2 · Snapshot PNG").clicked() {
            return GuiAction::SnapshotPng;
        }
        if ui.button("F3 · Export JSON").clicked() {
            return GuiAction::ExportJson;
        }
        if ui.button("Q · Quit").clicked() {
            return GuiAction::Quit;
        }
        GuiAction::None
    });
    inner.inner
}

// ---------------------------------------------------------------------
// Tests (headless: the pure core — no egui context, no I/O; the
// render path is compile-checked and verified live in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor};
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// One populated DDR4 module with a single XMP 2.0 profile
    /// (mirrors the TUI zone-3 fixture).
    fn fixture_module() -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::Value("Samsung".to_owned()),
            die_maker: Section::Value("SK hynix".to_owned()),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::Value(8),
            part: Section::Value("M391A2K40DB".to_owned()),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::Value(2),
            density_mbit: Section::Value(16_384),
            speed_mts: Section::Value(3_200),
            profiles: vec![SpdProfile {
                index: 1,
                speed_mts: Section::Value(3_600),
                cas: Section::Value(18),
                trcd: Section::Value(18),
                trp: Section::Value(18),
                tras: Section::Value(36),
                voltage: Section::Value(1_350),
            }],
        }
    }

    /// A fully degraded DDR5 module: every field `Na`, one all-`Na`
    /// EXPO profile (the no-panic shape).
    fn all_na_module() -> SpdModule {
        SpdModule {
            index: 0x53,
            is_ddr5: true,
            maker: Section::na(NaReason::DriverMissing),
            die_maker: Section::na(NaReason::NotApplicable),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::na(NaReason::NotApplicable),
            part: Section::na(NaReason::ParseError(
                "part number: byte 0x149 outside image bounds".to_owned(),
            )),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::na(NaReason::InsufficientPrivilege),
            density_mbit: Section::na(NaReason::UnknownPmTableVersion),
            speed_mts: Section::na(NaReason::ParseError(
                "base speed byte 0x20 is 0 (no minimum data rate recorded)".to_owned(),
            )),
            profiles: vec![SpdProfile {
                index: 0,
                speed_mts: Section::na(NaReason::NotApplicable),
                cas: Section::na(NaReason::NotApplicable),
                trcd: Section::na(NaReason::NotApplicable),
                trp: Section::na(NaReason::NotApplicable),
                tras: Section::na(NaReason::NotApplicable),
                voltage: Section::na(NaReason::NotApplicable),
            }],
        }
    }

    /// A representative snapshot: both module shapes, no readout
    /// branches (an AMD host).
    fn representative() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
            spd: vec![fixture_module(), all_na_module()],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3500.0),
                motherboard: Section::Value("Test Board".to_owned()),
                bios: Section::Value("1.0".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            // Parallel to spd (C6-06): module 0 is 16384 Mbit x 8 devices = 16 GiB;
            // module 1 density is Na, so its entry carries the offending source reason.
            total_capacity: Section::Value(16.0),
            dimm_sizes: vec![Section::Value(16.0), Section::na(NaReason::UnknownPmTableVersion)],
        }
    }

    /// A snapshot with no SPD modules at all (driver absent).
    fn no_spd() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Unknown".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::DriverMissing),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// A fully degraded snapshot: the all-`Na` module only.
    fn all_na_snapshot() -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Unknown,
                brand: "Unknown".to_owned(),
            },
            amd: Section::na(NaReason::DriverMissing),
            intel: Section::na(NaReason::DriverMissing),
            spd: vec![all_na_module()],
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: vec![Section::na(NaReason::UnknownPmTableVersion)],
        }
    }

    /// (a) A representative snapshot: one card per module, with the
    /// populated module's formatted values (the product line, the
    /// DRAM-die line, the human rank label, maker / part / rank /
    /// density / speed + the XMP profile line) and at least one
    /// non-`N/A` value.
    #[test]
    fn spd_cards_renders_a_card_per_module_with_values() {
        let cards = spd_cards(&representative());
        assert_eq!(cards.len(), 2, "one card per SPD slot");

        // The populated DDR4 module: every field decoded. The
        // fixture's die type is Na(NotApplicable), so the die line
        // drops it (16384 Mbit -> 16Gb); rank 2 -> Dual-Rank.
        let card = &cards[0];
        assert_eq!(
            card[0],
            ("product".to_owned(), "Samsung (M391A2K40DB)".to_owned())
        );
        assert_eq!(card[1], ("dram die".to_owned(), "SK hynix (16Gb)".to_owned()));
        assert_eq!(card[2], ("rank label".to_owned(), "Dual-Rank".to_owned()));
        assert_eq!(card[3], ("maker".to_owned(), "Samsung".to_owned()));
        assert_eq!(card[4], ("part".to_owned(), "M391A2K40DB".to_owned()));
        assert_eq!(card[5], ("rank".to_owned(), "2".to_owned()));
        assert_eq!(card[6], ("density".to_owned(), "16384 Mbit".to_owned()));
        assert_eq!(card[7], ("speed".to_owned(), "3200 MT/s".to_owned()));
        assert_eq!(card[8].0, "XMP 1");
        assert_eq!(card[8].1, "3600 MT/s 18-18-18-36 @ 1.350 V");

        // At least one formatted (non-N/A) value overall.
        assert!(
            cards
                .iter()
                .any(|card| card.iter().any(|(_, display)| !display.contains("N/A"))),
            "a populated module must carry formatted values: {cards:?}"
        );
    }

    /// (b) No SPD modules → an empty `Vec` (no cards, no panic); the
    /// all-`Na` module → one card whose every row (the product line,
    /// the die line, the rank label, the fields, and the all-`Na`
    /// EXPO profile line) is a bare `N/A` in muted gray, no panic.
    #[test]
    fn spd_cards_handles_no_spd_and_all_na_without_panic() {
        assert!(spd_cards(&no_spd()).is_empty(), "no SPD modules -> no cards");

        let cards = spd_cards(&all_na_snapshot());
        assert_eq!(cards.len(), 1, "one card per module");
        assert!(
            cards[0]
                .iter()
                .all(|(_, display)| display.contains("N/A")),
            "every all-Na row must carry an N/A display: {cards:?}"
        );
        assert_eq!(
            cards[0][0],
            ("product".to_owned(), "N/A".to_owned())
        );
        assert_eq!(
            cards[0][1],
            ("dram die".to_owned(), "N/A".to_owned())
        );
        assert_eq!(
            cards[0][2],
            ("rank label".to_owned(), "N/A".to_owned())
        );
        assert_eq!(
            cards[0][3],
            ("maker".to_owned(), "N/A".to_owned())
        );
        assert_eq!(
            cards[0][4].1,
            "N/A"
        );
        // The all-Na EXPO profile line: every field degrades to bare N/A.
        assert_eq!(cards[0][8].0, "EXPO 0");
        assert_eq!(
            cards[0][8].1,
            "N/A N/A-N/A-N/A-N/A @ N/A"
        );
    }

    /// (b′) The new card rows in isolation: the rank label maps
    /// 1 / 2 / n to `Single-Rank` / `Dual-Rank` / `<n>-Rank` (a
    /// degenerate `0` and an absent rank degrade to a bare `N/A`),
    /// the product line renders `<maker> (<part>)` with one `Na`
    /// partner bare and both `Na` a bare `N/A`, and the die line
    /// drops each absent part (`(type, density)` → `(density)` →
    /// `(type)` → bare maker → a bare `N/A`).
    #[test]
    fn card_row_formatters_no_panic_on_na() {
        // The human rank label.
        assert_eq!(rank_label(&Section::Value(1)), "Single-Rank");
        assert_eq!(rank_label(&Section::Value(2)), "Dual-Rank");
        assert_eq!(rank_label(&Section::Value(4)), "4-Rank");
        assert_eq!(rank_label(&Section::Value(0)), "N/A");
        assert_eq!(rank_label(&Section::na(NaReason::DriverMissing)), "N/A");

        // The product line (the spec's "G.Skill … (F5-…)" form).
        assert_eq!(
            product_line(
                &Section::Value("G.Skill Trident Z5 RGB".to_owned()),
                &Section::Value("F5-6000J3038F16GX2".to_owned())
            ),
            "G.Skill Trident Z5 RGB (F5-6000J3038F16GX2)"
        );
        assert_eq!(
            product_line(
                &Section::Value("Samsung".to_owned()),
                &Section::na(NaReason::NotApplicable)
            ),
            "Samsung"
        );
        assert_eq!(
            product_line(
                &Section::na(NaReason::NotApplicable),
                &Section::Value("M391A2K40DB".to_owned())
            ),
            "M391A2K40DB"
        );
        assert_eq!(
            product_line(
                &Section::na(NaReason::DriverMissing),
                &Section::na(NaReason::NotApplicable)
            ),
            "N/A"
        );

        // The DRAM-die line: each absent part is dropped (the
        // fixture already carries a Na die type + 16384 Mbit).
        let full = SpdModule {
            die_type: Section::Value("A-Die".to_owned()),
            ..fixture_module()
        };
        assert_eq!(dram_die_line(&full), "SK hynix (A-Die, 16Gb)");
        assert_eq!(dram_die_line(&fixture_module()), "SK hynix (16Gb)");
        assert_eq!(
            dram_die_line(&SpdModule {
                die_type: Section::Value("A-Die".to_owned()),
                density_mbit: Section::na(NaReason::NotApplicable),
                ..fixture_module()
            }),
            "SK hynix (A-Die)"
        );
        assert_eq!(
            dram_die_line(&SpdModule {
                density_mbit: Section::na(NaReason::NotApplicable),
                ..fixture_module()
            }),
            "SK hynix"
        );
        assert_eq!(
            dram_die_line(&SpdModule {
                die_maker: Section::na(NaReason::DriverMissing),
                ..fixture_module()
            }),
            "N/A"
        );

        // The density conversion: Mbit -> Gb (a non-integer
        // conversion keeps the raw Mbit form).
        assert_eq!(density_gib(16_384), "16Gb");
        assert_eq!(density_gib(8_192), "8Gb");
        assert_eq!(density_gib(2_000), "2000 Mbit");
    }

    /// (c) Deterministic: two calls on the same snapshot are equal
    /// (all three fixture shapes).
    #[test]
    fn spd_cards_is_deterministic() {
        for snapshot in [representative(), no_spd(), all_na_snapshot()] {
            assert_eq!(spd_cards(&snapshot), spd_cards(&snapshot));
        }
    }

    /// (d) The daemon status line: `not connected` for an empty
    /// status, the stamp from the last-update instant (or `never`
    /// before the first poll), and the recorded status verbatim.
    #[test]
    fn daemon_status_line_shapes() {
        // Never polled: empty status + no last-update instant.
        assert_eq!(
            daemon_status_line(&TelemetryData::default()),
            "daemon: not connected · never"
        );

        // Connected with a fresh stamp.
        let connected = TelemetryData {
            daemon_status: "connected: /tmp/ramsleuth.sock".to_owned(),
            last_update: Some(Instant::now() - Duration::from_millis(400)),
            ..Default::default()
        };
        let line = daemon_status_line(&connected);
        assert!(
            line.starts_with("daemon: connected: /tmp/ramsleuth.sock · "),
            "the status must be printed verbatim: {line}"
        );
        assert!(line.ends_with("s ago"), "a landed stamp must end in 's ago': {line}");

        // A transport failure: the status is kept, the stamp never.
        let down = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            error: Some("daemon not running".to_owned()),
            ..Default::default()
        };
        assert_eq!(daemon_status_line(&down), "daemon: disconnected · never");
    }

    /// (e) The daemon status color: CYAN only when connected and
    /// healthy, CRIMSON when disconnected, never polled, or errored.
    #[test]
    fn daemon_status_color_semantics() {
        let healthy = TelemetryData {
            daemon_status: "connected: /tmp/ramsleuth.sock".to_owned(),
            ..Default::default()
        };
        assert_eq!(daemon_status_color(&healthy), CYAN);

        let down = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            ..Default::default()
        };
        assert_eq!(daemon_status_color(&down), CRIMSON);

        // Never polled (the idle default state).
        assert_eq!(daemon_status_color(&TelemetryData::default()), CRIMSON);

        // Connected but the last reply was a structured error.
        let errored = TelemetryData {
            daemon_status: "connected: /tmp/ramsleuth.sock".to_owned(),
            error: Some("unexpected response to GetTelemetry".to_owned()),
            ..Default::default()
        };
        assert_eq!(daemon_status_color(&errored), CRIMSON);
    }

    /// (f) The card value color: CYAN for a decoded display, muted
    /// `NA_GRAY` for a bare `N/A` one (a full cell or a degraded
    /// profile field — D-5: unavailable, not a fault).
    #[test]
    fn card_value_color_semantics() {
        assert_eq!(card_value_color("Samsung"), CYAN);
        assert_eq!(card_value_color("3200 MT/s"), CYAN);
        assert_eq!(card_value_color("16384 Mbit"), CYAN);
        assert_eq!(card_value_color("none"), CYAN);
        assert_eq!(card_value_color("N/A"), NA_GRAY);
        assert_eq!(card_value_color("N/A N/A-N/A-N/A-N/A @ N/A"), NA_GRAY);
    }

    /// (g) The C9-07 width + height fill (D-4a + D-4b): the zone
    /// frame spans the full available width AND the full available
    /// height of the parent ui (the C9-05 status slice).
    /// `set_min_width` / `set_min_height` as the first lines inside
    /// the frame closure force the content — and hence the `Frame`
    /// border — to the allocated slice; a `Frame` otherwise shrinks
    /// to its content's natural size (the SPD-card grid's), leaving
    /// dead space to the right of and below the border. Asserted by
    /// rendering the zone into a bounded box and measuring the
    /// outermost CYAN-stroked rect (the frame border itself — the
    /// per-card frames are the other CYAN rects, each strictly
    /// inside the outer one): its size must equal the box's
    /// available width and height, for a populated snapshot (two
    /// cards), a no-SPD snapshot, and the no-telemetry placeholder
    /// alike.
    #[test]
    fn render_status_zone_frame_fills_available_width_and_height() {
        // The C9-05 status slice's shape: bounded, taller than the
        // content's natural height (so the vertical fill has room to
        // act) and wider than the card grid's natural width.
        const W: f32 = 420.0;
        const H: f32 = 640.0;
        for telemetry in [Some(representative()), Some(no_spd()), None] {
            let data = TelemetryData { telemetry, ..Default::default() };
            let ctx = egui::Context::default();
            ctx.begin_frame(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(W + 40.0, H + 40.0),
                )),
                ..Default::default()
            });
            egui::CentralPanel::default().show(&ctx, |ui| {
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(W, H),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        assert!((ui.available_width() - W).abs() < 1.0);
                        assert!((ui.available_height() - H).abs() < 1.0);
                        render_status_zone(ui, &data);
                    },
                );
            });
            let out = ctx.end_frame();
            // The outer zone frame + one CYAN-stroked rect per SPD
            // card; the cards sit inside the outer frame, so the
            // outer is the largest-area rect.
            let frame_rects: Vec<egui::Rect> = out
                .shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r) if r.stroke.color == CYAN => Some(r.rect),
                    _ => None,
                })
                .collect();
            assert!(
                !frame_rects.is_empty(),
                "the zone must paint its CYAN-stroked frame"
            );
            let outer = frame_rects
                .iter()
                .max_by(|a, b| a.area().total_cmp(&b.area()))
                .unwrap();
            assert!(
                (outer.width() - W).abs() < 1.0,
                "the frame border must fill the available width: {} != {W}",
                outer.width()
            );
            assert!(
                (outer.height() - H).abs() < 1.0,
                "the frame border must fill the available height: {} != {H}",
                outer.height()
            );
        }
    }
}
