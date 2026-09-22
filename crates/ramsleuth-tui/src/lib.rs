//! ramsleuth-tui — the unprivileged terminal dashboard (Phase 3).
//!
//! This is the **library** of the TUI crate (lib + bin, the daemon/client
//! precedent): the binary (`src/main.rs`, P3-24) drives it, and the pure,
//! testable pieces live here so the key contract is frozen before any
//! rendering code exists (the P3-22 exit criterion).
//!
//! The TUI is unprivileged (plan D2): it never reads hardware itself — it
//! talks to the daemon over the Unix socket through the shared
//! `ramsleuth-client` library (the same `Client` the CLI uses), renders
//! the three Grand Design §3 zones with `ratatui`, and services the
//! `[R]`efresh / `[S]`napshot / `[Q]`uit keybindings.
//!
//! Modules land one per chunk (plan §3, group E):
//!
//! - `events` — P3-22 — the frozen input contract: the pure `key_to_action`
//!   mapping (`crossterm::event::KeyEvent` -> `Action`, testable without a
//!   terminal) + `poll_event` (the single crossterm raw-mode poll/read
//!   wrapper — every other part of the crate stays I/O-free).
//! - `graphs` — TUI-05..07 — the graphs core: the five-series
//!   `GraphSample` + the 1800-deep `GraphState` ring (TUI-03
//!   `RingBuffer` backing) + the Na-guarded `record_graph_sample`
//!   poller hook (part 1 of the module; parts 2/3 land in
//!   TUI-06/07).
//! - `ring` — TUI-03 — the TUI-local bounded FIFO ring buffer (the
//!   GUI `history.rs` core re-implemented std-only — the graphs
//!   module backing store).
//! - `ui` — P3-23 — the three-zone dashboard renderer over `AppState`.
//! - `main` (bin) — P3-24 — raw mode + alternate screen (Drop-safe
//!   restore), the background 2 s telemetry updater, the draw/poll loop.

pub mod events;
pub mod graphs;
// TUI-16: declared for its first consumer — the ui.rs render chain
// (the pure `diagnose` + `render_requirements_strip`). The module-doc
// inventory update + the root re-exports stay TUI-23's scope.
pub mod requirements;
pub mod ring;
pub mod ui;

// P3-22: the frozen input contract, re-exported at the root (workspace
// re-export style, the client/lib.rs precedent) — P3-23 (`ui`) and P3-24
// (bin) dispatch on `Action` via `key_to_action` from the crate root.
pub use events::{key_to_action, Action};

// P3-23: the three-zone dashboard, re-exported at the root (workspace
// re-export style) — P3-24 (bin) holds one `AppState` (telemetry snapshot,
// bench state, daemon status, error) behind an `Arc<RwLock<_>>` and calls
// `render` each tick.
pub use ui::{render, AppState, BenchState};
