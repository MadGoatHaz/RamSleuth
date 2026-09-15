//! The GUI's in-memory settings state + the settings panel (C6-26,
//! item 7a).
//!
//! Design decision D-C5 (plan §C6-26/27): settings are **in-memory
//! only** this cycle — the [`GuiSettings`] struct holds the
//! configurable knobs (the daemon socket, the telemetry poll interval,
//! the display units, the theme, and the refresh switch), and
//! [`render_settings_panel`] mutates it from plain egui widgets.
//! Persistence to `$XDG_CONFIG_HOME` is a documented follow-up, not
//! implemented: the serde derives are already in place so the struct
//! round-trips through JSON (a test below pins the shape) when that
//! I/O path lands, and the CLI keeps its `--socket`-only surface
//! (`Default` covers the no-flag case with the protocol's
//! [`DEFAULT_SOCKET_PATH`]).
//!
//! **Wired (C6-27 / C6-30):** `TelemetryData` carries this state
//! (`settings`), the poller re-reads the live `poll_interval_ms` /
//! `refresh_enabled` knobs per tick (C6-27), and the app shell
//! (C6-30) shows [`render_settings_panel`] in the header's settings
//! strip, seeds `socket` from the CLI `--socket`, and re-reads the
//! `socket` knob live per poll / bench cycle. The zones' `Units` /
//! `Theme` formatters remain a follow-up (the knobs are editable,
//! not yet consumed by the zone renderers).
//!
//! **No-panic contract (D5):** the pure helpers here (the unit
//! formatters) degrade non-finite inputs to the honest `N/A` text —
//! never a panic; the panel widgets are plain egui controls with no
//! I/O of their own, so the whole module is testable headless.

use ramsleuth_protocol::DEFAULT_SOCKET_PATH;

/// The default telemetry poll cadence in milliseconds — the
/// pre-C6-27 fixed `TELEMETRY_INTERVAL` (2 s, since removed) as the
/// [`GuiSettings::default`] knob value (the poller reads the live
/// knob per tick, C6-27).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 2000;

/// The binary → decimal capacity conversion factor (1 GiB =
/// 1.073741824 GB).
const GIB_TO_GB: f64 = 1.073741824;

// ---------------------------------------------------------------------
// The unit + theme knobs (pure data — serde-derived for the deferred
// XDG persistence follow-up, D-C5).
// ---------------------------------------------------------------------

/// The capacity display unit. The telemetry carries **GiB** values on
/// the wire (`total_capacity` / `dimm_sizes`) — the knob is a pure
/// display conversion, the wire is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CapacityUnit {
    /// Binary gibibytes (1024³) — the wire's native unit.
    #[default]
    GiB,
    /// Decimal gigabytes (1000³) — the carried GiB value × [`GIB_TO_GB`].
    GB,
}

/// The clock display unit. The telemetry carries **MHz** values on
/// the wire (`cpu_clock_mhz` / the AMD clock readout) — the knob is a
/// pure display conversion, the wire is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ClockUnit {
    /// Megahertz — the wire's native unit.
    #[default]
    MHz,
    /// Gigahertz — the carried MHz value ÷ 1000.
    GHz,
}

/// The display unit knobs: one field per dimension (capacity + clock)
/// so the panel renders two independent combos — a single four-value
/// enum would conflate two different physical dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Units {
    /// The capacity unit (the RAM summary, the per-DIMM sizes).
    pub capacity: CapacityUnit,
    /// The clock unit (the CPU / memory controller clocks).
    pub clock: ClockUnit,
}

/// The app theme. [`Theme::DarkSlate`] is the current — and only
/// implemented — theme (the `style`-crate dark-slate `build_style`);
/// further themes (a `Light` is the documented candidate) are added
/// when `build_style` learns them. D-C5 keeps the theme in-memory
/// this cycle (the knob exists; no theme switching is wired).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    /// The current dark-slate theme (Grand Design §3.2).
    #[default]
    DarkSlate,
}

// ---------------------------------------------------------------------
// The settings state (in-memory, D-C5).
// ---------------------------------------------------------------------

/// The GUI's in-memory settings (C6-26, item 7a): the configurable
/// knobs the settings panel mutates. In-memory only this cycle — no
/// file I/O, no XDG persistence (a documented follow-up; the serde
/// derives are in place for it). The app shell (C6-30) seeds
/// `socket` from the CLI `--socket`; [`Default`] covers the no-flag
/// case with the protocol's [`DEFAULT_SOCKET_PATH`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GuiSettings {
    /// The daemon Unix socket to connect to (seeded from the CLI
    /// `--socket`; the poller reads it live per poll / bench cycle —
    /// C6-30, the settings panel edits it).
    pub socket: String,
    /// The telemetry poll cadence in milliseconds (the pre-C6-27
    /// fixed `TELEMETRY_INTERVAL` knob-ized — the poller reads the
    /// live value per tick, C6-27).
    pub poll_interval_ms: u64,
    /// The display units (capacity GiB/GB, clock MHz/GHz).
    pub units: Units,
    /// The app theme (currently the single [`Theme::DarkSlate`]).
    pub theme: Theme,
    /// Whether the background poller refreshes telemetry at all
    /// (`false` = keep the last snapshot frozen — C6-27 honors it).
    pub refresh_enabled: bool,
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            socket: DEFAULT_SOCKET_PATH.to_owned(),
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            units: Units::default(),
            theme: Theme::default(),
            refresh_enabled: true,
        }
    }
}

// ---------------------------------------------------------------------
// The unit formatters (pure, headless-testable — the C6-27 hook the
// zones use to read the `units` knobs).
// ---------------------------------------------------------------------

/// A whole-number `f64` with no decimals (`1800.0` → `1800`), one
/// decimal otherwise (`4.5` → `4.5`) — the crate's number convention
/// (the header's `trim_number` precedent).
fn trim(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}", value)
    } else {
        format!("{:.1}", value)
    }
}

/// One clock readout (a carried MHz wire value) as display text in
/// the selected clock unit — `3600 MHz` or `3.6 GHz`. A non-finite
/// input degrades to the honest `N/A` (never a panic, D5).
pub fn format_clock(mhz: f64, units: &Units) -> String {
    if !mhz.is_finite() {
        return "N/A".to_owned();
    }
    match units.clock {
        ClockUnit::MHz => format!("{} MHz", trim(mhz)),
        ClockUnit::GHz => format!("{} GHz", trim(mhz / 1000.0)),
    }
}

/// One capacity readout (a carried GiB wire value) as display text in
/// the selected capacity unit — `64 GiB` or `68.7 GB`. A non-finite
/// input degrades to the honest `N/A` (never a panic, D5).
pub fn format_capacity(gib: f64, units: &Units) -> String {
    if !gib.is_finite() {
        return "N/A".to_owned();
    }
    match units.capacity {
        CapacityUnit::GiB => format!("{} GiB", trim(gib)),
        CapacityUnit::GB => format!("{} GB", trim(gib * GIB_TO_GB)),
    }
}

/// One bandwidth readout (a carried GiB/s wire value, the benchmark
/// grid's GB/s cells re-expressed in the wire's binary unit) as
/// display text in the selected capacity unit — `12 GiB/s` or
/// `12.9 GB/s`. A non-finite input degrades to the honest `N/A`
/// (never a panic, D5).
pub fn format_bw(gibs: f64, units: &Units) -> String {
    if !gibs.is_finite() {
        return "N/A".to_owned();
    }
    match units.capacity {
        CapacityUnit::GiB => format!("{} GiB/s", trim(gibs)),
        CapacityUnit::GB => format!("{} GB/s", trim(gibs * GIB_TO_GB)),
    }
}

// ---------------------------------------------------------------------
// The settings panel (plain egui widgets mutating `settings` — no
// I/O of its own; C6-30 shows it in the settings area).
// ---------------------------------------------------------------------

fn capacity_unit_label(unit: CapacityUnit) -> &'static str {
    match unit {
        CapacityUnit::GiB => "GiB (binary)",
        CapacityUnit::GB => "GB (decimal)",
    }
}

fn clock_unit_label(unit: ClockUnit) -> &'static str {
    match unit {
        ClockUnit::MHz => "MHz",
        ClockUnit::GHz => "GHz",
    }
}

fn theme_label(theme: Theme) -> &'static str {
    match theme {
        Theme::DarkSlate => "Dark Slate",
    }
}

/// Render the settings panel into `ui`: a compact two-column grid of
/// the [`GuiSettings`] knobs — the daemon socket (a singleline text
/// edit), the poll interval (a clamped drag value in ms,
/// 100 ms – 60 s), the capacity + clock unit combos, the theme combo,
/// and the refresh checkbox. The widgets mutate `settings` directly
/// (the render-thread-only write the app shell permits, D6: no
/// I/O, no socket); nothing here is wired to the poller or the layout
/// yet (C6-27 / C6-30).
pub fn render_settings_panel(ui: &mut egui::Ui, settings: &mut GuiSettings) {
    let _ = egui::Grid::new("ramsleuth_settings")
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Socket");
            ui.add(egui::TextEdit::singleline(&mut settings.socket).desired_width(280.0));
            ui.end_row();

            ui.label("Poll interval");
            ui.add(
                egui::DragValue::new(&mut settings.poll_interval_ms)
                    .clamp_range(100..=60_000)
                    .suffix(" ms"),
            );
            ui.end_row();

            ui.label("Capacity units");
            let _ = egui::ComboBox::from_label("")
                .selected_text(capacity_unit_label(settings.units.capacity))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut settings.units.capacity,
                        CapacityUnit::GiB,
                        "GiB (binary, 1024³)",
                    );
                    ui.selectable_value(
                        &mut settings.units.capacity,
                        CapacityUnit::GB,
                        "GB (decimal, 1000³)",
                    );
                });
            ui.end_row();

            ui.label("Clock units");
            let _ = egui::ComboBox::from_label("")
                .selected_text(clock_unit_label(settings.units.clock))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.units.clock, ClockUnit::MHz, "MHz");
                    ui.selectable_value(&mut settings.units.clock, ClockUnit::GHz, "GHz");
                });
            ui.end_row();

            ui.label("Theme");
            let _ = egui::ComboBox::from_label("")
                .selected_text(theme_label(settings.theme))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.theme, Theme::DarkSlate, "Dark Slate");
                });
            ui.end_row();

            ui.label("Refresh");
            ui.checkbox(&mut settings.refresh_enabled, "auto-refresh telemetry");
            ui.end_row();
        });
}

// ---------------------------------------------------------------------
// Tests (headless: no window, no display, no daemon).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) The defaults match the brief (D-C5): the protocol's
    /// default socket, the current 2 s poll cadence, the binary
    /// capacity + MHz clock units, the single dark-slate theme, and
    /// refresh on.
    #[test]
    fn default_settings_match_the_brief() {
        let settings = GuiSettings::default();
        assert_eq!(settings.socket, DEFAULT_SOCKET_PATH.to_owned());
        assert_eq!(settings.poll_interval_ms, 2000);
        assert_eq!(settings.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
        assert_eq!(
            settings.units,
            Units { capacity: CapacityUnit::GiB, clock: ClockUnit::MHz }
        );
        assert_eq!(settings.theme, Theme::DarkSlate);
        assert!(settings.refresh_enabled, "refresh must default on");
    }

    /// (b) The serde round-trip: a fully-mutated settings struct
    /// serializes (the JSON the XDG persistence follow-up will write)
    /// and deserializes back to an equal value — every knob preserved.
    #[test]
    fn serde_round_trip_preserves_every_knob() {
        // A fully-mutated (non-default) struct, built as one literal
        // (no `Default` + field reassignment — the clippy
        // `field_reassign_with_default` precedent).
        let settings = GuiSettings {
            socket: "/tmp/ramsleuth-dev.sock".to_owned(),
            poll_interval_ms: 5000,
            units: Units { capacity: CapacityUnit::GB, clock: ClockUnit::GHz },
            theme: Theme::DarkSlate,
            refresh_enabled: false,
        };

        let json = serde_json::to_string(&settings).expect("settings must serialize");
        let back: GuiSettings = serde_json::from_str(&json).expect("settings must deserialize");
        assert_eq!(back, settings, "the round-trip must preserve every knob");
    }

    /// (c) The serde round-trip over the defaults.
    #[test]
    fn serde_round_trip_defaults() {
        let settings = GuiSettings::default();
        let json = serde_json::to_string(&settings).expect("defaults must serialize");
        let back: GuiSettings = serde_json::from_str(&json).expect("defaults must deserialize");
        assert_eq!(back, settings);
    }

    /// (d) The unit / theme serde shapes: each enum serializes to its
    /// variant name (the wire shape the persistence follow-up uses),
    /// the `Units` struct to its two named fields.
    #[test]
    fn unit_and_theme_serde_shapes() {
        assert_eq!(
            serde_json::to_string(&Units::default()).expect("units must serialize"),
            r#"{"capacity":"GiB","clock":"MHz"}"#
        );
        assert_eq!(
            serde_json::to_string(&Theme::DarkSlate).expect("theme must serialize"),
            "\"DarkSlate\""
        );
        let units: Units = serde_json::from_str(r#"{"capacity":"GB","clock":"GHz"}"#)
            .expect("units must deserialize");
        assert_eq!(units, Units { capacity: CapacityUnit::GB, clock: ClockUnit::GHz });
    }

    /// (e) `format_clock`: the MHz arm keeps the carried value, the
    /// GHz arm divides by 1000 (one decimal when non-whole); non-
    /// finite inputs degrade to the honest `N/A` (never a panic).
    #[test]
    fn format_clock_arms() {
        let mhz = Units { capacity: CapacityUnit::GiB, clock: ClockUnit::MHz };
        let ghz = Units { capacity: CapacityUnit::GiB, clock: ClockUnit::GHz };
        assert_eq!(format_clock(3600.0, &mhz), "3600 MHz");
        assert_eq!(format_clock(1800.0, &mhz), "1800 MHz");
        assert_eq!(format_clock(3600.0, &ghz), "3.6 GHz");
        assert_eq!(format_clock(1800.0, &ghz), "1.8 GHz");
        assert_eq!(format_clock(f64::NAN, &mhz), "N/A");
        assert_eq!(format_clock(f64::INFINITY, &ghz), "N/A");
    }

    /// (f) `format_capacity`: the GiB arm keeps the wire's native
    /// gibibytes, the GB arm converts × [`GIB_TO_GB`] (one decimal
    /// when non-whole); non-finite → `N/A`.
    #[test]
    fn format_capacity_arms() {
        let gib = Units { capacity: CapacityUnit::GiB, clock: ClockUnit::MHz };
        let gb = Units { capacity: CapacityUnit::GB, clock: ClockUnit::MHz };
        assert_eq!(format_capacity(64.0, &gib), "64 GiB");
        assert_eq!(format_capacity(4.5, &gib), "4.5 GiB");
        assert_eq!(format_capacity(64.0, &gb), "68.7 GB");
        assert_eq!(format_capacity(f64::NAN, &gb), "N/A");
    }

    /// (g) `format_bw`: the GiB/s arm keeps the raw value, the GB/s
    /// arm converts (one decimal when non-whole); non-finite → `N/A`.
    #[test]
    fn format_bw_arms() {
        let gib = Units { capacity: CapacityUnit::GiB, clock: ClockUnit::MHz };
        let gb = Units { capacity: CapacityUnit::GB, clock: ClockUnit::MHz };
        assert_eq!(format_bw(12.0, &gib), "12 GiB/s");
        assert_eq!(format_bw(12.0, &gb), "12.9 GB/s");
        assert_eq!(format_bw(f64::NEG_INFINITY, &gb), "N/A");
    }
}
