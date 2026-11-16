//! ramsleuth-gui — the eframe app shell (P3-30).
//!
//! This is the binary entry point (the library — `style` P3-25,
//! `update` P3-26, the three zones P3-27…P3-29 — is consumed from the
//! `ramsleuth_gui` root). It wires the frozen pieces into the running
//! 60 FPS window:
//!
//! - **CLI** — the pure [`parse_args`]: `--socket <path>` (default
//!   [`DEFAULT_SOCKET_PATH`]); `--help`/`-h` and `--version`/`-V`
//!   short-circuit in `main` before parsing (exit 0); an unknown flag
//!   / positional / missing value is a `String` error (exit 2 — the
//!   ramsleuth-daemon P3-17 / ramsleuth-client P3-21 / ramsleuth-tui
//!   P3-24 precedent).
//! - **App** — a 968×600 eframe window carrying the dark-slate
//!   [`build_style`]: the spec's 3-line header (Grand Design §3.1,
//!   C6-20) — line 1 a left anchor (the `RamSleuth v<version>`
//!   title, the platform badge — the CPU vendor's short `AMD` /
//!   `Intel` / `Unknown` pill — and the compact daemon status: a
//!   colored dot, `Daemon OK` connected / `Daemon` disconnected,
//!   the live settings socket on the hover tooltip — C6-30) plus a
//!   right-anchored button cluster (the `About`, `Probe`,
//!   `Setup`, `Settings`, and `Graphs` toggles — the `About`
//!   modal chunk gui-about; the latter C7-21, D-3);
//!   line 2 the CPU and platform identity; line 3 the RAM summary,
//!   channel, and sync mode — plus a transient export notice (the
//!   `[F2] snapshot · [F3] export · [Q] quit` key legend lives in
//!   the status zone's actions row, not the header). The
//!   `Settings` toggle opens the C6-26 settings
//!   panel as its own top strip below the header (C6-30). Over the
//!   three zones: the telemetry matrix on the left, the benchmark
//!   grid stacked over the hardware / SPD status on the right — the
//!   zones take the central panel's full height (C7-19 removed the
//!   embedded 10-minute trend history strip; the trend data now
//!   lives in the Graphs window, C7-21). Each frame takes one brief
//!   read of the shared
//!   `Arc<RwLock<TelemetryData>>` and repaints on a 16 ms cadence
//!   (~60 FPS).
//! - **Graphs window (C7-21, D-3):** the header's `Graphs` toggle
//!   spawns a dedicated second OS window as an eframe 0.27 deferred
//!   child viewport (one shared context + event loop — no second
//!   eframe lifecycle, Wayland-safe): while open the root re-
//!   registers it every frame (the keep-alive — egui GCs a child
//!   the first frame the root stops registering it, which is the
//!   close); the child's body renders the C7-20
//!   `render_graphs_window` over the shared state on its own ~60
//!   FPS cadence — a pure reader of the data fields (the poller
//!   stays their only writer, D6), the one permitted write being
//!   the C9-03 `Poll` combo over the shared
//!   `settings.poll_interval_ms` knob (the settings-panel D-2 / D6
//!   write precedent); the child's WM close button clears the same
//!   flag the header button toggles (D-C7: the two close paths are
//!   behaviorally identical).
//! - **Keyboard (C6-30):** the spec's key legend is live — a fresh
//!   key-down of F2 / F3 / Q (egui marks OS key-repeats
//!   `repeat: true`, so a held key fires exactly once) routes through
//!   the same [`GuiAction`] dispatch as the status zone's buttons
//!   (D-C7: keys and buttons are behaviorally identical).
//! - **No render-thread I/O (plan D6):** the background
//!   [`spawn_poller`] thread (P3-26) owns the daemon socket — the
//!   telemetry cadence (the live settings knob, default 2 s — C6-27),
//!   the socket itself (the live settings knob, seeded from the CLI
//!   `--socket` — C6-30), and the benchmark stream all run there; the
//!   render loop only reads the state (the settings panel's knob
//!   write — C6-30 — is the one permitted render-thread mutation:
//!   no I/O). The status zone's [`GuiAction`] is the one side effect
//!   the render loop performs: F2 / F3 run [`perform_export`] (a
//!   one-shot file write) and Q sets the stop flag + closes the
//!   viewport. The one-click setup's `running` edge (the wizard
//!   button, C21-06) detaches the `pkexec` worker the same way —
//!   the spawn + the helper run are entirely off the render thread.
//! - **Window icon (C21-26):** the root + the Graphs child viewport
//!   carry the embedded hicolor master (the 256×256
//!   `assets/icons/ramsleuth-256.png` PNG — C21-25), decoded once at
//!   startup; a decode failure degrades to eframe's default icon
//!   (no-panic, D5).
//! - **Post-setup auto-reconnect (the C21-36 removal):** a
//!   successful one-click setup (the helper's exit 0, the `done`
//!   outcome) needs no manual restart — the helper's per-user ACL
//!   covers the current session (no re-login), and the worker sets
//!   the state's `reconnect_requested` edge, which the background
//!   poller consumes with one immediate poll (independent of the
//!   `refresh_enabled` gate): the moment the (re)started daemon
//!   serves, the SETUP strip unmounts on its own (the liveness
//!   predicate). A failed setup keeps the existing `failed:` strip
//!   line (no dialog, no modal).
//!
//! **No-panic contract (plan D5):** a missing daemon never crashes the
//! GUI — the poller records the friendly error in the state (the
//! header + the status zone show it) and a failed export is a
//! structured [`GuiError`] in the notice line. On window close (the
//! Q key or button, the OS close button, or an eframe failure) the
//! app's `Drop` stops the poller and joins it (bounded) so the
//! process always exits cleanly.
//!
//! Manual verification (a live display, the QA phase): with the dev
//! daemon running (`cargo run -p ramsleuth-daemon -- --socket
//! /tmp/ramsleuth.sock`), `cargo run -p ramsleuth-gui -- --socket
//! /tmp/ramsleuth.sock` opens the 968×600 window with all three zones
//! live (values update at the configured poll interval, default
//! ~2 s); the F2 / F3 keys and their status-zone buttons both write
//! `ramsleuth-snapshot-<unix-ts>.png` /
//! `ramsleuth-export-<unix-ts>.json` to `$HOME` (the transient notice
//! carries the path); Q (key or button, or the window close button)
//! exits cleanly. The header's `Settings` toggle opens the settings
//! panel: its `Socket` field shows the CLI-seeded socket (C6-30), and
//! editing it retargets the poller live (a fresh connection per
//! cycle); the poll interval / refresh knobs behave per C6-27. With
//! no daemon the dashboard stays responsive, showing
//! `disconnected` + the start hint — no crash.
//!
//! ```text
//! Usage: ramsleuth [OPTIONS]
//!
//! Options:
//!   --socket <path>   Daemon Unix socket
//!                     (default: /run/ramsleuth/ramsleuth.sock)
//!   -h, --help        Print this help and exit
//!   -V, --version     Print the version and exit
//!
//! Keys: [F2] snapshot  [F3] export  [Q] quit
//! Exit codes: 0 quit, 1 eframe/display failure, 2 usage error
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ramsleuth_gui::{
    build_style, export_json, snapshot_png, spawn_poller, BenchCmd, GuiAction, GuiError,
    GuiSettings, TelemetryData,
};
use ramsleuth_protocol::cli_common::{short_circuit, version_line, ShortCircuit, USAGE_COMMON};

// The imports below are only needed by the test module (via
// `use super::*;`); they were part of the original single-file
// main.rs and the tests reference them by bare name.
#[cfg(test)]
use ramsleuth_display::{age_fragment, channel_mode, mclk_derived_speed, slot_note};
#[cfg(test)]
use ramsleuth_gui::first_run::SetupOutcome;
#[cfg(test)]
use ramsleuth_gui::{render_settings_panel, ProbeResult, Units, AMBER, CRIMSON, CYAN};
#[cfg(test)]
use ramsleuth_protocol::DEFAULT_SOCKET_PATH;
#[cfg(test)]
use ramsleuth_telemetry::amd_readout::MemoryChannelMode;
#[cfg(test)]
use ramsleuth_telemetry::probe::render_probe_report_md;
#[cfg(test)]
use ramsleuth_telemetry::ProbeReport;

// ---------------------------------------------------------------------
// Binary-local modules (the split-gui refactor): each section of the
// former single-file main.rs now lives in its own module; the module
// declarations + the `use` re-exports below keep every item in the
// crate root's namespace so the test module's `use super::*;` and the
// cross-module references all resolve unchanged.
// ---------------------------------------------------------------------
mod app;
mod cli;
mod graphs_viewport;
mod icon;
mod probe_ui;
mod setup_worker;
mod text;

use app::*;
use cli::*;
use icon::*;
use probe_ui::*;

// The items below are only referenced by the test module (via
// `use super::*;`); the non-test code reaches them through the
// `crate::<module>::<item>` paths, so these re-exports are test-only.
#[cfg(test)]
use graphs_viewport::*;
#[cfg(test)]
use setup_worker::*;
#[cfg(test)]
use text::*;

// ---------------------------------------------------------------------------
// Poison-tolerant shared-state guard acquisition (FE-3).
//
// The shared state lives behind an `RwLock` held by both threads (the
// updater writes, the render / poll thread reads). A stray panic that drops
// a guard poisons the lock; a bare `.read().unwrap()` / `.write().unwrap()`
// on the next acquisition would re-panic, cascading one bad read across
// both threads and taking the whole UI down. These two helpers are the
// single acquisition idiom: `unwrap_or_else(|p| p.into_inner())` recovers
// the inner value from a poisoned guard, so a panic degrades (the last-good
// state is read) instead of cascading (the no-panic contract, D5).
// ---------------------------------------------------------------------------

/// Acquire a read guard on the shared state, recovering a poisoned lock
/// (FE-3): `into_inner` yields the last-good inner value instead of
/// re-panicking, so one stray panic degrades a single read, never the
/// whole UI.
fn state_read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|p| p.into_inner())
}

/// Acquire a write guard on the shared state, recovering a poisoned lock
/// (FE-3): the same idiom as [`state_read`], for the mutation path.
fn state_write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|p| p.into_inner())
}

/// How long a transient header notice (an F2 / F3 export result) stays
/// visible before it fades.
const NOTICE_TTL: Duration = Duration::from_secs(5);
/// Bounded join deadline for the poller on app drop: the poller checks
/// `stop` every 200 ms tick and a blocked benchmark frame returns
/// within the client's 5 s read timeout, so this is generous; on
/// timeout the thread is detached (the process is exiting — the TUI
/// P3-24 bounded-join precedent).
const POLLER_JOIN_DEADLINE: Duration = Duration::from_secs(10);
/// The join-poll granularity while waiting for the poller.
const JOIN_POLL: Duration = Duration::from_millis(50);
/// The main dashboard's default inner size (D-13.4 — tight,
/// content-derived 968×600: 8 (OUTER_MARGIN, left) + the two equal zone
/// columns (`ui.columns(2)`, FE-5) + 8 (COLUMN_GAP) + 8 (OUTER_MARGIN,
/// right); H = 70 (header) + 8 (row bottom gap) + 522 (row height, >=
/// 11 pt headroom over both columns' natural content).
const DEFAULT_WINDOW_SIZE: [f32; 2] = [968.0, 600.0];
/// The main dashboard's minimum inner size (D-13.4 — both columns at
/// their practical minimum, 8 (OUTER_MARGIN) × 2 + 8 (COLUMN_GAP) + the
/// zones' minimum widths = 892; H = 600 = default — below that the
/// status zone's bottom rows clip (it has no scroll of its own)).
const MIN_WINDOW_SIZE: [f32; 2] = [892.0, 600.0];
/// The Graphs window's initial inner size (C7-21, D-3 — the deferred
/// child viewport; the plan's default, a bit narrower + shorter
/// than the main dashboard).
const GRAPHS_WINDOW_SIZE: [f32; 2] = [900.0, 520.0];
/// The Graphs window's minimum inner size (the title + the five
/// series rows must stay legible).
const GRAPHS_WINDOW_MIN_SIZE: [f32; 2] = [640.0, 420.0];
/// The repaint cadence: ~60 FPS (16 ms — eframe's vsync drives the
/// actual present; this only asks for the next frame).
const REPAINT: Duration = Duration::from_millis(16);
/// The gap between the two zone columns (FE-5: the `ui.columns(2)`
/// `item_spacing.x`).
const COLUMN_GAP: f32 = 8.0;
/// The central panel's symmetric side margin (D-15.1): the row's
/// left + right outer margin — equal to the pre-existing left
/// margin (symmetric by construction) and ≥ the frame stroke's
/// 0.5 pt outer half with 15× clearance (the missing-right-border
/// fix).
const OUTER_MARGIN: f32 = 8.0;
/// The header strip's stroke (a dim line over the SLATE fill).
const HEADER_STROKE: egui::Color32 = egui::Color32::from_rgb(0x34, 0x34, 0x40);
/// The header's compact daemon-status dot colors: GREEN for a
/// connected + healthy daemon, RED for a disconnected / errored one
/// (the short `Daemon OK` / `Daemon` label rides the same state).
const DAEMON_OK_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 200, 120);
const DAEMON_DOWN_RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);
/// The header's platform badge pill fill (a muted step above the
/// SLATE header fill).
const BADGE_BG: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x2A, 0x33);

/// The wall-clock unix timestamp (seconds) for the export file names.
/// A pre-epoch clock (impossible on Linux) degrades to 0 — never a
/// panic.
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Perform one user action against the current state (P3-30).
///
/// The status zone's returned [`GuiAction`] becomes its side effect:
/// `SnapshotPng` writes the current benchmark grid — with the CPU /
/// RAM header lines over it (C6-28) — to
/// `ramsleuth-snapshot-<unix-ts>.png` (via [`snapshot_png`]) and
/// `ExportJson` writes the current telemetry snapshot + the terminal
/// benchmark grid (the `bench` key — the JSON `null` before the
/// first completed run, C6-29) to `ramsleuth-export-<unix-ts>.json`
/// (via [`export_json`]) — both into
/// `out_dir` (the app uses `$HOME`), returning the written path. An
/// action with nothing to export (no grid yet / no telemetry yet), or
/// `Quit` / `None`, returns `Ok(None)` (no file).
///
/// Pure-ish and testable: the only I/O is the one file write per
/// export (no socket, no daemon, no window) — the render loop calls
/// this for the F2 / F3 side effect and shows the result (the path or
/// the [`GuiError`] text) as a transient header notice. A failed write
/// is a structured [`GuiError`], never a panic (plan D5).
fn perform_export(
    action: GuiAction,
    data: &TelemetryData,
    out_dir: &Path,
) -> Result<Option<PathBuf>, GuiError> {
    let ts = unix_timestamp();
    match action {
        GuiAction::SnapshotPng => match &data.bench.grid {
            Some(grid) => {
                let path = out_dir.join(format!("ramsleuth-snapshot-{ts}.png"));
                snapshot_png(grid, data.telemetry.as_ref(), &path)?;
                Ok(Some(path))
            }
            None => Ok(None),
        },
        GuiAction::ExportJson => match &data.telemetry {
            Some(telemetry) => {
                let path = out_dir.join(format!("ramsleuth-export-{ts}.json"));
                export_json(telemetry, data.bench.grid.as_ref(), &path)?;
                Ok(Some(path))
            }
            None => Ok(None),
        },
        GuiAction::Quit | GuiAction::None => Ok(None),
    }
}

/// The key → [`GuiAction`] map of the spec's key legend (Grand
/// Design §3.1, `[F2] snapshot · [F3] export · [Q] quit`): F2
/// snapshots the PNG, F3 exports the JSON, Q quits; any other key
/// yields `None` (no action this frame). Pure + headless-testable —
/// the app shell feeds it the fresh (non-repeat) key-down events, and
/// the mapped action dispatches through the same [`handle_action`]
/// path as the status zone's buttons (D-C7: keys and buttons are
/// behaviorally identical).
fn key_action(key: egui::Key) -> Option<GuiAction> {
    match key {
        egui::Key::F2 => Some(GuiAction::SnapshotPng),
        egui::Key::F3 => Some(GuiAction::ExportJson),
        egui::Key::Q => Some(GuiAction::Quit),
        _ => None,
    }
}

/// The shared settings state seeded from the CLI (C6-30): the
/// `--socket` argument becomes `settings.socket` (the settings panel
/// shows the active socket, and the poller reads it live — a panel
/// edit retargets the next cycle), every other knob at its
/// [`GuiSettings::default`] value (2 s poll, refresh off by default
/// — one baseline fetch on connect; enable in Settings to poll; the
/// default units + theme).
fn seed_settings(args: &GuiArgs) -> GuiSettings {
    GuiSettings {
        socket: args.socket.to_string_lossy().into_owned(),
        ..Default::default()
    }
}

fn main() -> ExitCode {
    // 0. `--help`/`-h` and `--version`/`-V` short-circuit before
    //    parsing (standard, exit 0): help prints the usage text,
    //    version the [`version_line`].
    match short_circuit(std::env::args().skip(1)) {
        ShortCircuit::Help => {
            print!("{USAGE}{USAGE_COMMON}");
            return ExitCode::SUCCESS;
        }
        ShortCircuit::Version => {
            println!("{}", version_line("ramsleuth"));
            return ExitCode::SUCCESS;
        }
        ShortCircuit::None => {}
    }

    // 1. Parse the command line (usage error → exit 2).
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ramsleuth: {error}");
            eprint!("{USAGE}{USAGE_COMMON}");
            return ExitCode::from(2);
        }
    };

    // 2. The shared state + the flags (the poller is the state's
    //    data fields' only writer — the settings knobs' one
    //    render-thread write is the exception, C6-27 / C6-30; the
    //    bench zone + the poller share the bench flags; the Graphs
    //    window open flag is shared between the header button and
    //    the child's WM close button — C7-21, D-C7). The settings
    //    are seeded from the CLI: `--socket` becomes
    //    `settings.socket` (the panel shows the active socket, the
    //    poller reads it live — C6-30).
    let state = Arc::new(RwLock::new(TelemetryData {
        settings: seed_settings(&args),
        ..Default::default()
    }));
    let (bench_tx, bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    // The Graphs window open flag (C7-21, D-3): shared between the
    // header's `Graphs` button and the child viewport's WM close
    // button — both write it, the root's per-frame re-registration
    // reads it (the keep-alive).
    let graphs_open = Arc::new(AtomicBool::new(false));
    // The one-click setup outcome (C21-06): the wizard button's
    // `running` flip (the render thread's one permitted write, D6)
    // is consumed per tick by the app, which detaches the `pkexec`
    // worker (its `done` / `failure` land back here — brief locks
    // only, C14-03).
    let setup = Arc::new(RwLock::new(
        ramsleuth_gui::first_run::SetupOutcome::default(),
    ));
    // Exports land in $HOME (fall back to the CWD if it is unset).
    let out_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // 3. The background poller: the GUI's only daemon connection (D6)
    //    — the telemetry cadence (the live settings knob, default 2 s
    //    — C6-27) + the live settings socket (C6-30) + the
    //    benchmark stream.
    let poller = spawn_poller(state.clone(), bench_rx, stop.clone(), cancel.clone());

    // 4. The eframe window: 968×600 initial, the dark-slate style set
    //    once at creation (eframe 0.27 `AppCreator`: a plain
    //    `Box<dyn App>`, no `Result` wrapper). `main_stop` keeps a
    //    handle in main for the post-shutdown stop (the app takes the
    //    other clone).
    let main_stop = stop.clone();
    // The window-embed icon (C21-26): decoded once at startup — the
    // root viewport + the Graphs child (via the app) share this same
    // decode; `None` (a decode failure) stays legal — eframe's
    // default icon (no-panic, D5).
    let icon = window_icon();

    let options = eframe::NativeOptions {
        viewport: {
            let mut viewport = egui::ViewportBuilder::default()
                .with_inner_size(DEFAULT_WINDOW_SIZE)
                .with_min_inner_size(MIN_WINDOW_SIZE)
                .with_app_id("RamSleuth");
            viewport.icon = icon.clone();
            viewport
        },
        ..Default::default()
    };
    let result = eframe::run_native(
        "RamSleuth",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_style(build_style());
            Box::new(RamSleuthApp {
                state,
                bench_tx,
                stop,
                cancel,
                settings_open: false,
                requirements_open: true,
                setup,
                icon,
                graphs_open,
                saved_refresh: None,
                last_graphs_open: false,
                out_dir,
                poller: Some(poller),
                notice: None,
                probe_state: ProbeState::default(),
                probe_title: None,
                about_open: false,
            })
        }),
    );

    // 5. The window is closed. The app's `Drop` already stopped +
    //    joined the poller (bounded); set stop again for the
    //    early-failure path (no app was ever created) so the detached
    //    poller exits too.
    main_stop.store(true, Ordering::Relaxed);

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ramsleuth: {error}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------------
// Tests (headless: no display, no daemon — the eframe app itself is
// compile-checked; the live window run is verified in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
