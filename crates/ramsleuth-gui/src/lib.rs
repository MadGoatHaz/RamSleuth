//! ramsleuth-gui — the unprivileged desktop GUI (Phase 3, egui + eframe).
//!
//! This is the **library** the `ramsleuth-gui` binary (P3-30) renders
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
//!
//! (`telemetry_zone` / `bench_zone` / `status_zone` land in P3-27…
//! P3-29; the eframe app shell in `main.rs` lands in P3-30.)

pub mod style;
pub mod update;

// P3-25: the semantic style contract, re-exported at the root
// (workspace re-export style) — P3-26…P3-30 consume the palette,
// `build_style`, and the F2/F3 export helpers from here. `GuiError`
// rides along (the `ClientError` precedent) for the app shell's toasts.
pub use style::{build_style, export_json, snapshot_png, GuiError, AMBER, CRIMSON, CYAN, SLATE};

// P3-26: the shared update state + the background poller, re-exported
// at the root (workspace re-export style) — the app shell (P3-30)
// hands `spawn_poller` its `Arc<RwLock<TelemetryData>>` + the `BenchCmd`
// channel and only ever reads the lock on the render thread (D6), the
// zones (P3-27…P3-29) render `TelemetryData` / `BenchState` verbatim,
// and `poll_telemetry` / `run_bench` are the testable units both share.
pub use update::{poll_telemetry, run_bench, spawn_poller, BenchCmd, BenchState, TelemetryData};
