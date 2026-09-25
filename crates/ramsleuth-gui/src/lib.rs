//! ramsleuth-gui — the unprivileged desktop GUI (Phase 3, egui + eframe).
//!
//! This is the **library** the `ramsleuth` binary (P3-30) renders
//! from: the semantic style + palette (Grand Design §3.2), the two
//! file exports — F2 [`snapshot_png`] / F3 [`export_json`] — and the
//! shared update state + background poller the app shell reads behind
//! an `Arc<RwLock<…>>` — all headless and no-panic (plan D5: every
//! failure is a structured [`style::GuiError`] or a recorded state
//! error, never a crash).
//!
//! The GUI is unprivileged (plan D2): it talks to the daemon over the
//! Unix socket through the shared `ramsleuth-client` library; the
//! telemetry/bench/protocol path deps ride along because the zones
//! (P3-26+) render `SystemMemoryTelemetry` / `BenchmarkGrid` / wire
//! vocabulary verbatim (no type duplication, D2).
//!
//! Modules land one per chunk (plan §3, group F):
//! - `style` — P3-25 — the semantic palette + dark-slate
//!   [`build_style`] + the `export_json` / `snapshot_png` file helpers.
//! - `update` — P3-26 — the shared [`TelemetryData`] + the background
//!   poller (`spawn_poller`) the app shell runs behind
//!   `Arc<RwLock<TelemetryData>>` (no render-thread I/O, D6).
//! - `telemetry_zone` — P3-27 — zone 1: the live memory controller &
//!   subtimings matrix — [`timing_cells`] (the pure, deterministic cell
//!   builder over the AMD / Intel readout) + [`render_telemetry_zone`]
//!   (the titled, bounded-height `egui::Grid` renderer).
//! - `bench_zone` — P3-28 — zone 2: the AIDA64-style 4×4 benchmark
//!   grid + run controls + live progress — [`grid_cells`] (the pure
//!   16-cell builder over the terminal result grid) +
//!   [`render_bench_zone`] (the titled-frame `egui_extras::TableBuilder`
//!   renderer + the `BenchCmd` / cancel-flag controls).
//! - `status_zone` — P3-29 — zone 3: the hardware/SPD module
//!   cards, the daemon status line, and the F2 / F3 / Q actions
//!   row — [`spd_cards`] (the pure per-slot card builder) and
//!   [`render_status_zone`] (the titled-frame renderer reporting
//!   the clicked action as a [`GuiAction`] for the app shell).
//! - `history` — C6-24 — the 10-minute trend window: the
//!   fixed-capacity [`history::RingBuffer`] (300 samples at the 2 s
//!   poll), the three in-lockstep [`history::HistoryState`] series
//!   (MCLK / VDDCR_SOC / bandwidth), and the hand-rolled
//!   immediate-mode [`history::render_history`] sparkline plot (no
//!   chart crate, D-C4). The poller wiring is C6-25.
//! - `graph` — C7-20 — the Graphs window state + basic render:
//!   the five-series [`graph::GraphState`] (CPU core frequency /
//!   VDDCR_CPU — no source / VDDCR_SOC / CPU temperature / memory
//!   bandwidth — one timestamped sample per successful poll in the
//!   1800-deep ring = 60 min at the 2 s poll), the poller-side
//!   Na-guarded [`graph::record_graph_sample`] + the unprivileged
//!   [`graph::read_cpu_temp_c`] thermal-zone scan (D-4), and the
//!   hand-rolled time-windowed [`graph::render_graphs_window`] (no
//!   chart crate, D-3). The child viewport lands in C7-21, the
//!   interactivity in C7-22.
//! - `first_run` — C18 — the first-run/SETUP requirements strip
//!   (D-18.5): [`first_run::diagnose`] (the pure, headless-testable
//!   daemon / group / pinned-`ryzen_smu` builder) +
//!   [`first_run::render_requirements_strip`] (copy + "Got it"; no-panic)
//!   + the C21-04 one-click setup wizard ([`first_run::setup_argv`],
//!     [`first_run::SetupOutcome`], [`first_run::setup_with_dkms`],
//!     [`first_run::DKMS_INSTALL_CMD`],
//!     [`first_run::render_requirements_strip_with_setup`]).
//!
//! (The eframe app shell in `main.rs` lands in P3-30.)

pub mod bench_zone;
pub mod first_run;
pub mod graph;
pub mod history;
pub mod settings;
pub mod status_zone;
pub mod style;
pub mod telemetry_zone;
pub mod update;

// P3-25: the semantic style contract, re-exported at the root
// (workspace re-export style) — P3-26…P3-30 consume the palette,
// `build_style`, and the F2/F3 export helpers from here. `GuiError`
// rides along (the `ClientError` precedent) for the app shell's toasts.
pub use style::{
    build_style, export_json, snapshot_png, GuiError, AMBER, CRIMSON, CYAN, NA_GRAY, SLATE,
};

// P3-26: the shared update state + the background poller, re-exported
// at the root (workspace re-export style) — the app shell (P3-30)
// hands `spawn_poller` its `Arc<RwLock<TelemetryData>>` + the `BenchCmd`
// channel and only ever reads the lock on the render thread (D6), the
// zones (P3-27…P3-29) render `TelemetryData` / `BenchState` verbatim,
// and `poll_telemetry` / `run_bench` are the testable units both share.
// C7-20: the Graphs window state + basic render (D-3 / D-4 / D-5),
// re-exported at the root (workspace re-export style) — the poller
// (update.rs) is the only writer of `TelemetryData.graph` (one
// Na-guarded `record_graph_sample` per successful poll, with the
// `read_cpu_temp_c` thermal-zone scan + the D-5 bandwidth source),
// and C7-21's child viewport renders `render_graphs_window` over the
// shared state (a pure reader, D6).
pub use graph::{
    read_cpu_temp_c, record_graph_sample, render_graphs_window, GraphSample, GraphState,
    GRAPH_CAPACITY,
};

pub use update::{
    poll_telemetry, request_probe_report, run_bench, spawn_poller, BenchCmd, BenchState,
    ProbeResult, TelemetryData,
};

// P3-27: zone 1 (the live memory controller & subtimings), re-exported
// at the root (workspace re-export style) — the app shell (P3-30)
// calls `render_telemetry_zone` with a read-only `&TelemetryData`
// snapshot (the zone draws its own titled frame + bounded grid), and
// `timing_cells` is the pure, deterministic cell builder the tests
// exercise without an egui context.
pub use telemetry_zone::{render_telemetry_zone, timing_cells};

// P3-28: zone 2 (the AIDA64-style benchmark grid + run controls +
// live progress), re-exported at the root (workspace re-export style)
// — the app shell (P3-30) calls `render_bench_zone` with a read-only
// `&TelemetryData` snapshot + the poller's `BenchCmd` sender + the
// shared cancel flag, and `grid_cells` is the pure 16-cell builder
// the tests exercise without an egui context.
pub use bench_zone::{grid_cells, render_bench_zone};

// P3-29: zone 3 (the hardware/SPD module cards + the daemon
// status line + the F2 / F3 / Q actions row), re-exported at the
// root (workspace re-export style) — the app shell (P3-30) calls
// `render_status_zone` with a read-only `&TelemetryData` snapshot
// and executes the returned `GuiAction` (F2 writes the PNG,
// F3 the JSON, Q closes the viewport — the render thread does no
// I/O itself, D6), and `spd_cards` is the pure per-slot card
// builder the tests exercise without an egui context.
pub use status_zone::{render_status_zone, spd_cards, GuiAction};

// C6-26: the in-memory settings state + the settings panel (item
// 7a, D-C5 — in-memory only, XDG persistence deferred), re-exported
// at the root (workspace re-export style) — the app shell (C6-30)
// seeds `GuiSettings.socket` from the CLI `--socket` and shows
// `render_settings_panel` in the settings area, and C6-27 moves the
// live `poll_interval_ms` / `refresh_enabled` knobs onto the poller
// and the `Units` formatters onto the zones.
pub use settings::{
    format_bw, format_capacity, format_clock, render_settings_panel, CapacityUnit, ClockUnit,
    GuiSettings, Theme, Units, DEFAULT_POLL_INTERVAL_MS,
};

// C18: the first-run/SETUP requirements strip (D-18.5), re-exported at
// the root (workspace re-export style) — the app shell (C18-02) runs
// `diagnose` over the shared `TelemetryData` snapshot and renders the
// strip between the header and the settings strip (auto-shown on first
// launch, presence-driven, no-panic). The C21-04 one-click setup
// wizard symbols ride along for the C21-06 app-shell worker (the
// detached `pkexec` spawn over `setup_argv` + the AppState-owned
// `SetupOutcome`).
pub use first_run::{
    diagnose, render_requirements_strip, render_requirements_strip_with_setup, setup_argv,
    setup_with_dkms, Requirement, SetupOutcome, DKMS_INSTALL_CMD, RYZEN_SMU_PIN_SHORT,
};
