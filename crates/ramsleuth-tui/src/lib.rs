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
//! the Grand Design §3 parity dashboard with `ratatui`, and services the
//! frozen key contract — the P3-22 trio (`[R]`efresh / `[S]`napshot /
//! `[Q]`uit, byte-for-byte) plus the TUI-parity additions (the bench /
//! burn-in / cancel class, the graphs + settings + requirements panels,
//! the units / refresh / window cycles, and the probe-report action
//! (chunk probe-4) — the full 17-key table in [`events`]).
//!
//! Modules land one per chunk (plan §3, group E):
//!
//! - `events` — P3-22 + TUI-01/02 + chunk probe-4 — the frozen input
//!   contract: the pure `key_to_action` mapping
//!   (`crossterm::event::KeyEvent` -> `Action`, the full 17-key parity
//!   table, testable without a terminal) +
//!   `poll_event` (the single crossterm raw-mode poll/read wrapper —
//!   every other part of the crate stays I/O-free).
//! - `graphs` — TUI-05..07 — the five-series graphs core (the GUI
//!   `graph.rs` mirror): the timestamped `GraphSample` + the 1800-deep
//!   `GraphState` ring (the `RingBuffer` backing) + the Na-guarded
//!   `record_graph_sample` poller hook (TUI-05) + the ordered CPU-temp
//!   source scan `read_cpu_temp_c` (TUI-06) + the `[w]` window filter /
//!   `[g]` block sparkline panel (`window_samples` /
//!   `render_graphs_panel`, TUI-07).
//! - `probe` — chunk probe-4 — the consent-gated probe-report
//!   overlay: the modal `ProbeOverlayState` (the consent prompt →
//!   the scrollable markdown preview) + its topmost paint
//!   (`render_probe_overlay`, drawn last by `ui::render` over the
//!   whole frame) + the preview's I/O surfaces — the `[w]` file
//!   write (`write_probe_report` → `~/.ramsleuth/probe-report.md`)
//!   and the `[c]` clipboard (`copy_to_clipboard` — `wl-copy` /
//!   `xclip` / `xsel`, the pure `PATH` scan `find_clipboard_utility`)
//!   — all testable headlessly (the key I/O itself runs in the
//!   bin's main loop, the D6 side-effect split).
//! - `ring` — TUI-03 — the TUI-local bounded FIFO ring buffer (the GUI
//!   `history.rs` core re-implemented std-only — the graphs module
//!   backing store).
//! - `requirements` — TUI-08 — the SETUP requirements strip (the GUI
//!   `first_run::diagnose` mirror, display-only): the pure no-panic
//!   `diagnose(&AppState)` (the three prerequisite cases) +
//!   `render_requirements_strip` (the amber `SETUP — requirements`
//!   block with the exact fix commands).
//! - `ui` — P3-23 + TUI-09..16 — the parity dashboard renderer over
//!   `AppState`: the 3-line header, zone 1 (memory controller + the
//!   VDDCR_VDD / GEAR_DOWN / CR rows), zone 2 (the live bench grid + the
//!   status / controls / burn-in lines), zone 3 (hardware + SPD with the
//!   die row), the settings + requirements strips, and the topmost
//!   graphs overlay (the C21-36 modal idiom).
//! - `main` (bin) — P3-24 + TUI-17..22 — raw mode + alternate screen
//!   (Drop-safe restore), the GUI-style background poller (the
//!   live-interval telemetry loop, the single bench / burn-in worker,
//!   the per-poll graph recording), the draw/poll loop, and the full
//!   17-key dispatch (the P3-22 trio byte-for-byte).

pub mod events;
pub mod graphs;
// chunk probe-4: the consent-gated probe-report overlay (declared
// for its first consumer — the ui.rs render chain).
pub mod probe;
// TUI-08/16: the SETUP requirements strip (declared for its first
// consumer — the ui.rs render chain); TUI-23 completes the wiring
// (the module-doc inventory above + the root re-exports below).
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

// TUI-03: the bounded FIFO ring buffer, re-exported at the root
// (workspace re-export style) — the graphs module's backing store.
pub use ring::RingBuffer;

// TUI-05..07: the five-series graphs core, re-exported at the root
// (workspace re-export style) — the bin's poller is the only writer
// (`record_graph_sample` + the poller-thread `read_cpu_temp_c` scan,
// plan D6); the render path is a pure reader (the `window_samples`
// filter + the `[g]` `render_graphs_panel` overlay).
pub use graphs::{
    GraphSample,
    GraphState,
    record_graph_sample,
    read_cpu_temp_c,
    window_samples,
    render_graphs_panel,
    GRAPH_CAPACITY,
};

// TUI-08: the SETUP requirements strip, re-exported at the root
// (workspace re-export style) — `diagnose` is the pure presence rule the
// render chain (TUI-16) + the `[d]` toggle drive.
pub use requirements::{Requirement, diagnose, render_requirements_strip};

// chunk probe-4: the consent-gated probe-report overlay,
// re-exported at the root (workspace re-export style) — the bin's
// main loop drives the phases (the `[F]` fetch, the consent keys,
// the preview keys) and performs the I/O (the file write, the
// clipboard spawn); `ui` draws the overlay topmost.
pub use probe::{
    ClipboardUtil,
    NoticeTone,
    ProbeOverlayState,
    copy_to_clipboard,
    find_clipboard_utility,
    probe_report_default_path,
    write_probe_report,
};
