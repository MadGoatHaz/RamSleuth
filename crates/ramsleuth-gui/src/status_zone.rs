//! Zone 3 renderer: the hardware/SPD module cards + the daemon status
//! line + the F2 / F3 / Q actions row (P3-29).
//!
//! Grand Design §3.1 bottom-right panel: one small framed "card" per
//! bound SPD slot (a `slot 0xNN (DDR4|DDR5)` header over the
//! [`spd_cards`] rows — maker / part / rank / density / speed, each
//! its [`Section`] value in CYAN or `N/A (<reason>)` in CRIMSON, plus
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
//! snapshot: no telemetry renders one crimson placeholder, an empty
//! SPD list renders a crimson `driver missing` placeholder, an all-`Na`
//! module renders every row `N/A (<reason>)` — never a panic.
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
use crate::{CRIMSON, CYAN, SLATE};

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
/// per bound [`SpdModule`] (maker / part / rank / density / speed, then
/// one row per XMP / EXPO profile — or a `profiles: none` placeholder
/// when the module carries none).
///
/// Pure and deterministic: the same snapshot always yields the same
/// `Vec`. Each display is the formatted [`Section::Value`] (bare rank,
/// `… Mbit` density, `… MT/s` speed, the profile line
/// `<speed> <cl>-<trcd>-<trp>-<tras> @ <volts>`) or `N/A (<reason>)`
/// for a [`Section::Na`] (the dump renderer's form — [`NaReason`]
/// carries no `Display`); an all-`Na` module renders every row `N/A`
/// and never panics. Returns an empty `Vec` when the snapshot carries
/// no SPD modules (the renderer draws its own placeholder).
pub fn spd_cards(telemetry: &SystemMemoryTelemetry) -> Vec<Vec<(String, String)>> {
    telemetry.spd.iter().map(card_rows).collect()
}

/// The `(label, display)` rows of one SPD module card: maker / part /
/// rank / density / speed, then the profile rows.
fn card_rows(module: &SpdModule) -> Vec<(String, String)> {
    let mut rows = vec![
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

/// One [`Section`] cell's display: the formatted value, or
/// `N/A (<reason>)` for an absent one.
fn display<T>(section: &Section<T>, fmt: impl Fn(&T) -> String) -> String {
    match section {
        Section::Value(value) => fmt(value),
        Section::Na(reason) => na_text(reason),
    }
}

/// The human text of an absent cell: `N/A (<reason>)` (the renderer
/// owns this form — [`NaReason`] carries no `Display`; the zone 1 /
/// TUI precedent).
fn na_text(reason: &NaReason) -> String {
    match reason {
        NaReason::UnsupportedHardware => "N/A (unsupported hardware)".to_owned(),
        NaReason::DriverMissing => "N/A (driver missing)".to_owned(),
        NaReason::InsufficientPrivilege => "N/A (insufficient privilege)".to_owned(),
        NaReason::UnknownPmTableVersion => "N/A (unknown PM table version)".to_owned(),
        NaReason::NotApplicable => "N/A (not applicable)".to_owned(),
        NaReason::ParseError(detail) => format!("N/A (parse error: {detail})"),
    }
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
/// value, CRIMSON for an absent one (an `N/A (…)`, including a
/// degraded profile field).
fn card_value_color(display: &str) -> egui::Color32 {
    if display.contains("N/A") {
        CRIMSON
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

/// The per-slot SPD module cards: one framed card per bound module
/// (the `slot 0xNN (DDR4|DDR5)` header + the [`spd_cards`] rows); an
/// empty SPD list renders a crimson `driver missing` placeholder, and
/// no telemetry at all renders one crimson placeholder — never a
/// panic (plan D5).
fn render_spd_cards(ui: &mut egui::Ui, data: &TelemetryData) {
    match &data.telemetry {
        Some(telemetry) => {
            if telemetry.spd.is_empty() {
                ui.label(egui::RichText::new("SPD: N/A (driver missing)").color(CRIMSON));
                return;
            }
            for (module, card) in telemetry.spd.iter().zip(spd_cards(telemetry)) {
                render_spd_card(ui, module, &card);
                ui.add_space(4.0);
            }
        }
        None => {
            ui.label(egui::RichText::new("N/A (no telemetry)").color(CRIMSON));
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
        ui.label(egui::RichText::new(&header).strong().color(CYAN));
        let _ = egui::Grid::new(format!("ramsleuth_spd_card_0x{:02X}", module.index))
            .spacing(egui::vec2(12.0, 1.0))
            .min_col_width(80.0)
            .show(ui, |ui| {
                for (label, display) in card {
                    ui.add(egui::Label::new(egui::RichText::new(label.as_str())));
                    ui.add(egui::Label::new(
                        egui::RichText::new(display.as_str()).color(card_value_color(display)),
                    ));
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

    use super::*;

    /// One populated DDR4 module with a single XMP 2.0 profile
    /// (mirrors the TUI zone-3 fixture).
    fn fixture_module() -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::Value("Samsung".to_owned()),
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
            part: Section::na(NaReason::ParseError(
                "part number: byte 0x81 outside image bounds".to_owned(),
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
        }
    }

    /// (a) A representative snapshot: one card per module, with the
    /// populated module's formatted values (maker / part / rank /
    /// density / speed + the XMP profile line) and at least one
    /// non-`N/A` value.
    #[test]
    fn spd_cards_renders_a_card_per_module_with_values() {
        let cards = spd_cards(&representative());
        assert_eq!(cards.len(), 2, "one card per SPD slot");

        // The populated DDR4 module: every field decoded.
        let card = &cards[0];
        assert_eq!(card[0], ("maker".to_owned(), "Samsung".to_owned()));
        assert_eq!(card[1], ("part".to_owned(), "M391A2K40DB".to_owned()));
        assert_eq!(card[2], ("rank".to_owned(), "2".to_owned()));
        assert_eq!(card[3], ("density".to_owned(), "16384 Mbit".to_owned()));
        assert_eq!(card[4], ("speed".to_owned(), "3200 MT/s".to_owned()));
        assert_eq!(card[5].0, "XMP 1");
        assert_eq!(card[5].1, "3600 MT/s 18-18-18-36 @ 1.350 V");

        // At least one formatted (non-N/A) value overall.
        assert!(
            cards
                .iter()
                .any(|card| card.iter().any(|(_, display)| !display.contains("N/A"))),
            "a populated module must carry formatted values: {cards:?}"
        );
    }

    /// (b) No SPD modules → an empty `Vec` (no cards, no panic); the
    /// all-`Na` module → one card whose every row (fields and the
    /// all-`Na` EXPO profile line) is `N/A (…)`, no panic.
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
            ("maker".to_owned(), "N/A (driver missing)".to_owned())
        );
        assert_eq!(
            cards[0][1].1,
            "N/A (parse error: part number: byte 0x81 outside image bounds)"
        );
        // The all-Na EXPO profile line: every field degrades.
        assert_eq!(cards[0][5].0, "EXPO 0");
        assert_eq!(
            cards[0][5].1,
            "N/A (not applicable) N/A (not applicable)-N/A (not applicable)-N/A (not applicable)-N/A (not applicable) @ N/A (not applicable)"
        );
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

    /// (f) The card value color: CYAN for a decoded display, CRIMSON
    /// for an `N/A` one (a full cell or a degraded profile field).
    #[test]
    fn card_value_color_semantics() {
        assert_eq!(card_value_color("Samsung"), CYAN);
        assert_eq!(card_value_color("3200 MT/s"), CYAN);
        assert_eq!(card_value_color("16384 Mbit"), CYAN);
        assert_eq!(card_value_color("none"), CYAN);
        assert_eq!(card_value_color("N/A (driver missing)"), CRIMSON);
        assert_eq!(
            card_value_color(
                "N/A (parse error: x) N/A (not applicable) @ N/A (not applicable)"
            ),
            CRIMSON
        );
    }
}
