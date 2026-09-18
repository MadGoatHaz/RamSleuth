//! ramsleuth-gui — the eframe app shell (P3-30).
//!
//! This is the binary entry point (the library — `style` P3-25,
//! `update` P3-26, the three zones P3-27…P3-29 — is consumed from the
//! `ramsleuth_gui` root). It wires the frozen pieces into the running
//! 60 FPS window:
//!
//! - **CLI** — the pure [`parse_args`]: `--socket <path>` (default
//!   [`DEFAULT_SOCKET_PATH`]); an unknown flag / positional / missing
//!   value is a `String` error (exit 2 — the ramsleuth-daemon P3-17 /
//!   ramsleuth-client P3-21 / ramsleuth-tui P3-24 precedent).
//! - **App** — a 968×600 eframe window carrying the dark-slate
//!   [`build_style`]: the spec's 3-line header (Grand Design §3.1,
//!   C6-20) — line 1 the `RamSleuth v2.0.0` title, the platform tag,
//!   the daemon status (naming the live settings socket — C6-30),
//!   the `Settings` toggle, the `Graphs` window toggle (C7-21,
//!   D-3), and the `[F2] snapshot · [F3] export · [Q] quit`
//!   legend; line 2 the CPU and platform identity; line 3
//!   the RAM summary, channel, and sync mode — plus a transient
//!   export notice. The `Settings` toggle opens the C6-26 settings
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
//!   viewport.
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
//! Usage: ramsleuth-gui [OPTIONS]
//!
//! Options:
//!   --socket <path>   Daemon Unix socket
//!                     (default: /run/ramsleuth/ramsleuth.sock)
//!
//! Keys: [F2] snapshot  [F3] export  [Q] quit
//! Exit codes: 0 quit, 1 eframe/display failure, 2 usage error
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ramsleuth_gui::{
    build_style, export_json, format_capacity, format_clock, render_bench_zone,
    render_graphs_window, render_settings_panel, render_status_zone,
    render_telemetry_zone, snapshot_png, spawn_poller, BenchCmd, GuiAction, GuiError,
    GuiSettings, TelemetryData, Units, AMBER, CRIMSON, CYAN, SLATE,
};
use ramsleuth_protocol::DEFAULT_SOCKET_PATH;
use ramsleuth_telemetry::amd_readout::{ClockReadout, DivMode};
use ramsleuth_telemetry::cpuid::{AmdZen, CpuVendor};
use ramsleuth_telemetry::error::Section;
use ramsleuth_telemetry::spd_decode::SpdModule;
use ramsleuth_telemetry::{SystemMemoryTelemetry, SystemPlatform};

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
/// content-derived 968×600: 8 (OUTER_MARGIN, left) + 504 (left
/// column = min(952·0.55, 952−440−8) over inner = 968 − 16) +
/// 8 (COLUMN_GAP) + 440 (MIN_RIGHT_W) + 8 (OUTER_MARGIN, right);
/// H = 70 (header) + 8 (row bottom gap) + 522 (row_h, >= 11 pt
/// headroom over both columns' natural content).
const DEFAULT_WINDOW_SIZE: [f32; 2] = [968.0, 600.0];
/// The main dashboard's minimum inner size (D-13.4 — both
/// columns at their minima: 8 (OUTER_MARGIN) + 420 (MIN_LEFT_W) +
/// 8 (COLUMN_GAP) + 440 (MIN_RIGHT_W) + 8 (OUTER_MARGIN) = 892,
/// exact sum; H = 600 = default — below that the status zone's
/// bottom rows clip (it has no scroll of its own)).
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
/// The min width of the left (telemetry) column — the zone's grid
/// needs room for its label + value columns.
const MIN_LEFT_W: f32 = 420.0;
/// The min width of the right (bench / status) column.
const MIN_RIGHT_W: f32 = 440.0;
/// The gap between the two columns.
const COLUMN_GAP: f32 = 8.0;
/// The central panel's symmetric side margin (D-15.1): the row's
/// left + right outer margin — equal to the pre-existing left
/// margin (symmetric by construction) and ≥ the frame stroke's
/// 0.5 pt outer half with 15× clearance (the missing-right-border
/// fix).
const OUTER_MARGIN: f32 = 8.0;
/// The header strip's stroke (a dim line over the SLATE fill).
const HEADER_STROKE: egui::Color32 = egui::Color32::from_rgb(0x34, 0x34, 0x40);

/// Usage text printed on parse errors (exit 2) — the ramsleuth-daemon
/// P3-17 / ramsleuth-client P3-21 / ramsleuth-tui P3-24 precedent. The
/// default socket path is the protocol's frozen `DEFAULT_SOCKET_PATH`
/// value (P3-10) as a literal: `concat!` only accepts literals, and
/// the protocol freeze test pins the string.
const USAGE: &str = concat!(
    "ramsleuth-gui — the live desktop dashboard (F2/F3/Q)\n",
    "\n",
    "Usage: ramsleuth-gui [OPTIONS]\n",
    "\n",
    "Options:\n",
    "  --socket <path>   Daemon Unix socket\n",
    "                    (default: /run/ramsleuth/ramsleuth.sock)\n",
    "\n",
    "Keys: [F2] snapshot  [F3] export  [Q] quit\n",
    "Exit codes: 0 quit, 1 eframe/display failure, 2 usage error\n",
);

/// The GUI's parsed command line (P3-30).
///
/// Pure data — produced by [`parse_args`] (no env access, no I/O,
/// unit-tested) and consumed once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiArgs {
    /// The daemon Unix socket to connect to (`--socket`); defaults to
    /// [`DEFAULT_SOCKET_PATH`] (the single socket-path source, P3-10).
    pub socket: PathBuf,
}

impl Default for GuiArgs {
    fn default() -> Self {
        Self { socket: PathBuf::from(DEFAULT_SOCKET_PATH) }
    }
}

/// Parse the GUI command line (the program name already removed).
///
/// Pure and testable: no env access, no I/O, no panic. The one flag
/// `--socket <path>` sets the daemon Unix socket (default
/// [`DEFAULT_SOCKET_PATH`]); it may repeat (the last value wins). Any
/// other flag, any positional argument, or a missing flag value is a
/// `String` error naming the problem (exit `2` at startup) — the
/// ramsleuth-client P3-21 / ramsleuth-tui P3-24 precedent.
pub fn parse_args<I, S>(args: I) -> Result<GuiArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = GuiArgs::default();
    let mut iter = args.into_iter();
    while let Some(raw) = iter.next() {
        let arg = raw.as_ref();
        if arg == "--socket" {
            let value = iter
                .next()
                .map(|v| v.as_ref().to_owned())
                .ok_or_else(|| {
                    "--socket requires a value (the daemon's Unix socket path)".to_owned()
                })?;
            parsed.socket = PathBuf::from(value);
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag `{arg}` (expected `--socket <path>`)"));
        } else {
            return Err(format!(
                "unexpected argument `{arg}` (no positional arguments are accepted)"
            ));
        }
    }
    Ok(parsed)
}

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
pub fn perform_export(
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

/// The header's daemon-status color: CYAN when connected and healthy
/// (no recorded error), CRIMSON otherwise — the status zone's
/// `daemon_status_color` rule (that helper is private to the zone, so
/// the header keeps this copy for the top strip).
fn header_status_color(data: &TelemetryData) -> egui::Color32 {
    let healthy = !data.daemon_status.is_empty()
        && data.daemon_status.starts_with("connected")
        && data.error.is_none();
    if healthy {
        CYAN
    } else {
        CRIMSON
    }
}

/// The header's transient-notice color: CYAN for a written file
/// ("wrote …"), CRIMSON for a failed export ("! …"), AMBER for the
/// informational "nothing to export yet" hint.
fn notice_color(text: &str) -> egui::Color32 {
    if text.starts_with('!') {
        CRIMSON
    } else if text.starts_with("wrote ") {
        CYAN
    } else {
        AMBER
    }
}

/// The eframe app (P3-30): the shared state, the poller's command
/// channel, the stop / cancel flags, the settings-panel visibility,
/// the export dir, the poller thread, and the transient header
/// notice.
struct RamSleuthApp {
    /// The shared presentation state — includes the in-memory
    /// `GuiSettings` knobs (C6-26 / C6-27 / C6-30: the poll cadence +
    /// refresh gate + the daemon socket live here; the poller
    /// re-reads them every tick, and the settings panel is the render
    /// thread's one permitted write: no I/O, D6). The background
    /// poller is the only writer of the data fields; the render loop
    /// only ever takes a brief read.
    state: Arc<RwLock<TelemetryData>>,
    /// The poller's benchmark channel (the bench zone's run buttons
    /// send into it; the poller owns the socket).
    bench_tx: Sender<BenchCmd>,
    /// The shared stop flag (the poller exits when it is set).
    stop: Arc<AtomicBool>,
    /// The shared benchmark cancel flag (the bench zone's Cancel button
    /// sets it; the poller's run checks it between frames).
    cancel: Arc<AtomicBool>,
    /// Whether the settings panel (the header's `Settings` toggle,
    /// C6-30) is open: while open it renders as its own top strip
    /// below the header and mutates `state.settings` (the render
    /// thread's one permitted write — no I/O, D6).
    settings_open: bool,
    /// Whether the Graphs window (C7-21, D-3 — the eframe deferred
    /// child viewport) is open: while set, `update` re-registers
    /// the viewport every frame (the keep-alive — egui GCs a child
    /// the first frame the root stops registering it, which is the
    /// close). The header's `Graphs` button (a render-thread click
    /// — no I/O, D6) and the child's own WM close button both
    /// write this shared flag (D-C7: the two close paths are
    /// behaviorally identical).
    graphs_open: Arc<AtomicBool>,
    /// The pre-open `settings.refresh_enabled` saved by the Graphs
    /// telemetry lifecycle detector (C9-02, D-2): `Some` while the
    /// window is open (the opening edge force-enabled the gate),
    /// restored + cleared on the closing edge — either close path
    /// clears the same flag, D-C7.
    saved_refresh: Option<bool>,
    /// The previous frame's `graphs_open` value: the C9-02 edge
    /// detector's memory (rising = the window opened, falling = it
    /// closed).
    last_graphs_open: bool,
    /// The export destination dir (F2 / F3 write here — `$HOME`).
    out_dir: PathBuf,
    /// The background poller thread (joined — bounded — on drop so the
    /// process exits cleanly).
    poller: Option<JoinHandle<()>>,
    /// The transient header notice: the last F2 / F3 result text +
    /// when it was set (it fades after [`NOTICE_TTL`]).
    notice: Option<(String, Instant)>,
}

impl Drop for RamSleuthApp {
    fn drop(&mut self) {
        // The window is gone (Q, the OS close button, or an eframe
        // failure): stop the poller and join it (bounded) — the render
        // thread never leaves a dangling daemon connection.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.poller.take() {
            let deadline = Instant::now() + POLLER_JOIN_DEADLINE;
            while !handle.is_finished() {
                if Instant::now() >= deadline {
                    // The poller is still draining a benchmark frame;
                    // the process is exiting — detach it (the TUI
                    // P3-24 bounded-join precedent).
                    eprintln!(
                        "ramsleuth-gui: the poller did not finish within {POLLER_JOIN_DEADLINE:?}; detaching"
                    );
                    break;
                }
                std::thread::sleep(JOIN_POLL);
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl eframe::App for RamSleuthApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // No render-thread I/O (plan D6): one brief read of the shared
        // state (the poller is its only writer) drives the header +
        // the three zones; the F2 / F3 side effect is one file write.
        // Fade the transient notice once it has lived its TTL.
        let expired = self
            .notice
            .as_ref()
            .is_some_and(|(_, set_at)| set_at.elapsed() > NOTICE_TTL);
        if expired {
            self.notice = None;
        }
        // The keyboard (C6-30, the spec's key legend): a fresh
        // key-down — egui marks OS key-repeats `repeat: true`, so a
        // held key fires exactly once, the button's click semantics —
        // routes through the same [`GuiAction`] dispatch as the
        // status zone's buttons (D-C7: keys and buttons are
        // behaviorally identical).
        let keyed_action = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key { key, pressed: true, repeat: false, .. } => {
                        key_action(*key)
                    }
                    _ => None,
                })
                .next()
        });
        // The top strips allocate in a fixed per-frame order (header,
        // settings while open, then the central panel): each takes
        // its own brief lock (the render thread does no I/O, D6 —
        // the settings strip is its one permitted write).
        {
            let data = self.state.read().unwrap();
            Self::render_header(
                ctx,
                &data,
                &mut self.settings_open,
                &self.graphs_open,
                &self.notice,
            );
        }
        if self.settings_open {
            self.render_settings_area(ctx);
        }
        let button_action = {
            let data = self.state.read().unwrap();
            self.render_zones(ctx, &data)
        };
        // One dispatch path for both sources: the button (the
        // explicit affordance) runs first, then a distinct key
        // action; a duplicate (button + key, same action) fires once.
        self.handle_action(ctx, button_action);
        if let Some(keyed_action) = keyed_action {
            if keyed_action != button_action {
                self.handle_action(ctx, keyed_action);
            }
        }

        // The Graphs window's telemetry lifecycle (C9-02, D-2): the
        // per-frame transition detector — force-on on the opening
        // edge, restore on the closing edge (both close paths clear
        // the same flag, D-C7) — before the keep-alive re-
        // registration below.
        self.apply_graphs_lifecycle();

        // The Graphs window (C7-21, D-3): while open, re-register
        // the deferred child viewport every frame — the keep-alive
        // (egui GCs the child the first frame the root stops
        // registering it, which is how both close paths destroy the
        // OS window).
        if self.graphs_open.load(Ordering::Relaxed) {
            show_graphs_viewport(ctx, &self.state, &self.graphs_open);
        }

        // ~60 FPS: eframe's vsync drives the present; this only asks
        // for the next frame on the 16 ms cadence.
        ctx.request_repaint_after(REPAINT);
    }
}

// ---------------------------------------------------------------------
// The spec's 3-line header (Grand Design §3.1, C6-20): the pure line
// builders (unit-tested without an egui context — no display needed)
// that `render_header` composes into the top panel; the capacity +
// clock segments honor the live `units` knob (C7-11).
// ---------------------------------------------------------------------

/// Line 1's platform tag (the mockup's `[AMD AM5 Platform]`): the
/// CPU vendor + a best-effort socket family from [`CpuVendor`] — the
/// Zen generations map to their socket family (Zen 1–3 → `AM4`,
/// Zen 4/5 → `AM5`), Intel maps to its generic `LGA` family (a
/// generation does not identify a socket number unambiguously —
/// mobile and desktop share generations), and an unrecognized vendor
/// carries no family at all (an honest bare `Platform`).
fn platform_tag(vendor: &CpuVendor) -> String {
    match vendor {
        CpuVendor::Amd(AmdZen::Zen1 | AmdZen::Zen2 | AmdZen::Zen3) => "AMD AM4 Platform".to_owned(),
        CpuVendor::Amd(AmdZen::Zen4 | AmdZen::Zen5) => "AMD AM5 Platform".to_owned(),
        CpuVendor::Intel(_) => "Intel LGA Platform".to_owned(),
        CpuVendor::Unknown => "Platform".to_owned(),
    }
}

/// Line 1's daemon status (the mockup's `Daemon: Connected
/// (IPC: /run/ramsleuth)`): a successful last poll names the
/// configured socket, the never-polled (empty-status) state shows
/// `Disconnected` + the socket we are trying, and any other recorded
/// status degrades to a bare `Disconnected`.
fn daemon_status_text(data: &TelemetryData, socket: &Path) -> String {
    if data.daemon_status.starts_with("connected") {
        format!("Daemon: Connected (IPC: {})", socket.display())
    } else if data.daemon_status.is_empty() {
        format!("Daemon: Disconnected (IPC: {})", socket.display())
    } else {
        "Daemon: Disconnected".to_owned()
    }
}

/// One [`Section`] cell as display text: the contained value, or
/// `N/A` (the header's honest-degradation rule — a missing cell
/// renders `N/A`, never a panic).
fn cell_text<T: std::fmt::Display>(cell: &Section<T>) -> String {
    match cell {
        Section::Value(v) => v.to_string(),
        Section::Na(_) => "N/A".to_owned(),
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
fn cpu_line_text(t: &SystemMemoryTelemetry, units: &Units) -> String {
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

/// Line 2's AGESA/SMU provenance fragment (C8-11, D-1): the header
/// labels the value by where it came from — a true AGESA token (the
/// DMI BIOS string scan, e.g. `ComboAm4v2 PI 1.2.0.12`) renders
/// `AGESA <v>`; when no true AGESA is available (common — the real
/// AGESA string is root-gated in the raw DMI table) but the
/// `ryzen_smu` driver publishes a shape-checked firmware version, it
/// renders `SMU <v>` under its own label (never the reported
/// `AGESA <smu value>` mislabel); when neither is available the
/// honest `AGESA N/A`. One value, one true label: no hybrid, no
/// fabricated string.
fn age_fragment(platform: &SystemPlatform) -> String {
    match (&platform.agesa, &platform.smu_version) {
        (Section::Value(v), _) => format!("AGESA {v}"),
        (Section::Na(_), Section::Value(v)) => format!("SMU {v}"),
        (Section::Na(_), Section::Na(_)) => "AGESA N/A".to_owned(),
    }
}

/// The rank word for the header breakdown (C9-01, D-1): mirrors the
/// SPD cards' rank label — `1` → `Single-Rank`, `2` → `Dual-Rank`,
/// `n > 0` → `<n>-Rank` — but returns `None` for a `Na`/`0` rank:
/// the compact header omits the word entirely (never prints `N/A`).
/// The rank is the C8-03 wire field (`SpdModule.rank`), read, never
/// written — no wire change this cycle.
fn rank_word(rank: &Section<u8>) -> Option<String> {
    match rank {
        Section::Value(0) | Section::Na(_) => None,
        Section::Value(1) => Some("Single-Rank".to_owned()),
        Section::Value(2) => Some("Dual-Rank".to_owned()),
        Section::Value(other) => Some(format!("{other}-Rank")),
    }
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
fn dimm_summary(sizes: &[Section<f64>], spd: &[SpdModule], units: &Units) -> String {
    let mut groups: Vec<(f64, Option<String>, usize)> = Vec::new();
    for (slot, cell) in sizes.iter().enumerate() {
        if let Some(gib) = cell.value() {
            // A slot beyond the SPD list (the degradation path — the
            // facade keeps the two equal-length) or a `Na`/`0` rank
            // carries no word.
            let rank = spd.get(slot).and_then(|module| rank_word(&module.rank));
            match groups
                .iter_mut()
                .find(|(value, word, _)| (value - gib).abs() < 0.05 && *word == rank)
            {
                Some(group) => group.2 += 1,
                None => groups.push((*gib, rank, 1)),
            }
        }
    }
    if groups.is_empty() {
        "N/A".to_owned()
    } else {
        groups
            .iter()
            .map(|(gib, rank, count)| match rank {
                Some(word) => format!("{count}x{} {word}", format_capacity(*gib, units)),
                None => format!("{count}x{}", format_capacity(*gib, units)),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// The channel mode from the DIMM count (D-C8): 1 / 2 / 4 →
/// Single- / Dual- / Quad-Channel; any other count (0, odd) degrades
/// to `N/A`.
fn channel_mode(dimm_count: usize) -> String {
    match dimm_count {
        1 => "Single-Channel".to_owned(),
        2 => "Dual-Channel".to_owned(),
        4 => "Quad-Channel".to_owned(),
        _ => "N/A".to_owned(),
    }
}

/// The total-vs-breakdown slot note (C9-01, D-1): the OS total
/// (`total_capacity`, MemTotal-preferred — C8-04) can strictly
/// exceed the SPD-visible sum when the host installs more DIMMs
/// than the SPD bus binds (the live host: 4×16 GiB installed, 2
/// bound). A uniform per-module size `u` whose quotient
/// `total / u` lands within a tenth of a module of an integer
/// `n` above the visible count → `<visible> of <n> slots
/// SPD-visible` (the OS reserves a fraction of the installed
/// capacity, so the quotient sits just below the integer); any
/// other excess → the generic `SPD sees <visible> of the
/// installed capacity`. A `Na` / non-finite total, no visible
/// modules, or `total ≤ sum` → `None` (no false alarm).
/// Unit-agnostic: counts, not GiB.
fn slot_note(total: &Section<f64>, sizes: &[Section<f64>]) -> Option<String> {
    let total = total.value().copied().filter(|value| value.is_finite())?;
    let visible: Vec<f64> = sizes.iter().filter_map(|cell| cell.value().copied()).collect();
    if visible.is_empty() {
        return None;
    }
    let sum = visible.iter().sum::<f64>();
    if total <= sum {
        return None;
    }
    let uniform = visible.iter().all(|value| (value - visible[0]).abs() < 0.05);
    if uniform {
        let unit = visible[0];
        if unit > 0.0 {
            let quotient = total / unit;
            let slots = quotient.round();
            if (quotient - slots).abs() <= 0.1 && slots > visible.len() as f64 {
                return Some(format!("{} of {slots:.0} slots SPD-visible", visible.len()));
            }
        }
    }
    Some(format!("SPD sees {} of the installed capacity", visible.len()))
}

/// Line 3's non-mode part (the mockup's `RAM: 64.0 GB (2x32GB)
/// DDR5-6000 MT/s | Dual-Channel | Mode: `): the total capacity in
/// the selected capacity unit (C7-11: `format_capacity` — the
/// default GiB keeps the carried wire value, the GB arm converts
/// × 1.073741824), the per-DIMM breakdown (C9-01, D-1: with the
/// rank word from the parallel SPD list — `2x16 GiB Single-Rank`),
/// the max SPD speed (omitted entirely when no module carries one),
/// the channel mode, the total-vs-breakdown slot note when the OS
/// total strictly exceeds the SPD sum (C9-01, D-1: `2 of 4 slots
/// SPD-visible`), and the `Mode: ` lead-in the mode segment
/// completes.
fn ram_line_prefix(t: &SystemMemoryTelemetry, units: &Units) -> String {
    let total = match t.total_capacity.value() {
        Some(gib) => format_capacity(*gib, units),
        None => "N/A".to_owned(),
    };
    let mut line = format!("RAM: {total} ({})", dimm_summary(&t.dimm_sizes, &t.spd, units));
    if let Some(mts) = t.spd.iter().filter_map(|m| m.speed_mts.value().copied()).max() {
        line.push_str(&format!(" {mts} MT/s"));
    }
    line.push_str(&format!(" | {}", channel_mode(t.dimm_sizes.len())));
    if let Some(note) = slot_note(&t.total_capacity, &t.dimm_sizes) {
        line.push_str(&format!(" | {note}"));
    }
    line.push_str(" | Mode: ");
    line
}

/// Line 3's mode segment (D-C8): the UCLK:MCLK ratio from the AMD
/// clock readout + its semantic color — AMBER for `Synchronous 1:1`
/// (with the MCLK in the selected clock unit when it carries one —
/// C7-11: `format_clock`), CRIMSON for `Asynchronous 1:2`, and
/// `None` (the default text color) for the honest `N/A`.
fn sync_mode_from_clocks(clocks: &ClockReadout, units: &Units) -> (String, Option<egui::Color32>) {
    match clocks.div_mode.value() {
        Some(DivMode::OneToOne) => {
            let text = match clocks.mclk_mhz.value() {
                Some(mhz) => {
                    format!("Synchronous 1:1 (UCLK = MCLK = {})", format_clock(*mhz, units))
                }
                None => "Synchronous 1:1".to_owned(),
            };
            (text, Some(AMBER))
        }
        Some(DivMode::OneToTwo) => ("Asynchronous 1:2".to_owned(), Some(CRIMSON)),
        None => ("N/A".to_owned(), None),
    }
}

/// Line 3's mode segment over a whole snapshot: the AMD branch must
/// carry a value whose `div_mode` is usable, else the honest `N/A` —
/// the Intel / driver-missing / degraded states all degrade here
/// (never a panic).
fn sync_mode(t: &SystemMemoryTelemetry, units: &Units) -> (String, Option<egui::Color32>) {
    match t.amd.value() {
        Some(readout) => sync_mode_from_clocks(&readout.clocks, units),
        None => ("N/A".to_owned(), None),
    }
}

impl RamSleuthApp {
    /// The header strip (Grand Design §3.1): the spec's 3-line
    /// header — line 1 the `RamSleuth v2.0.0` title, the platform
    /// tag, the daemon status (naming the live settings socket —
    /// C6-30), the `Settings` toggle, the `Graphs` window toggle
    /// (C7-21, D-3), and the `[F2] snapshot · [F3] export · [Q]
    /// quit` legend; line 2 the CPU + platform identity; line 3
    /// the RAM summary, channel, and sync mode (the capacity +
    /// clock segments render in the live `units` knob's units —
    /// C7-11) — plus the transient notice line (the last F2 / F3
    /// result) while one is showing. No
    /// telemetry yet (never polled) → the placeholder lines (a
    /// missing daemon never crashes the GUI, plan D5).
    ///
    /// An associated function (no `self`): it needs only the
    /// snapshot + the `settings_open` toggle (the `Settings` button
    /// flips it, C6-30) + the shared `graphs_open` flag (the
    /// `Graphs` button toggles it, C7-21) + the transient notice
    /// (the last F2 / F3 result line) — so the call site can hold
    /// the state's read guard and the `settings_open` /
    /// `graphs_open` / `notice` field borrows at once (the field
    /// split the borrow checker enforces).
    fn render_header(
        ctx: &egui::Context,
        data: &TelemetryData,
        settings_open: &mut bool,
        graphs_open: &Arc<AtomicBool>,
        notice: &Option<(String, Instant)>,
    ) {
        // The line builders over the current snapshot (the capacity +
        // clock segments honor the live `units` knob — C7-11) — or
        // the placeholders when no poll has landed yet (never a panic).
        let (cpu_line, ram_prefix, ram_mode, ram_mode_color) = match &data.telemetry {
            Some(t) => {
                let units = &data.settings.units;
                let (mode, color) = sync_mode(t, units);
                (cpu_line_text(t, units), ram_line_prefix(t, units), mode, color)
            }
            None => ("CPU: —".to_owned(), "RAM: —".to_owned(), String::new(), None),
        };
        let tag = data
            .telemetry
            .as_ref()
            .map(|t| platform_tag(&t.cpu.vendor))
            .unwrap_or_else(|| "Platform".to_owned());
        // The live settings socket (C6-30): the panel's `Socket`
        // field is the single source (seeded from the CLI `--socket`
        // at startup; the poller reads it live).
        let status = daemon_status_text(data, Path::new(&data.settings.socket));
        egui::TopBottomPanel::top("ramsleuth_header")
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .stroke(egui::Stroke::new(1.0_f32, HEADER_STROKE)),
            )
            .show(ctx, |ui| {
                // Line 1: title + platform tag + daemon status + legend.
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("RamSleuth v2.0.0").strong().color(CYAN).size(20.0),
                    );
                    ui.separator();
                    ui.label(egui::RichText::new(format!("[{tag}]")));
                    ui.separator();
                    ui.label(egui::RichText::new(&status).color(header_status_color(data)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        // The Graphs window (C7-21, D-3): the
                        // dedicated second OS window — rightmost,
                        // next to Settings; selected while open, a
                        // click toggles the shared open flag (spawn
                        // or close, D-C7).
                        graphs_button(ui, graphs_open);
                        ui.add_space(8.0);
                        // The settings toggle (C6-30): opens the
                        // settings panel strip below the header.
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Settings"))
                                    .selected(*settings_open),
                            )
                            .clicked()
                        {
                            *settings_open = !*settings_open;
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("[F2] snapshot · [F3] export · [Q] quit").weak(),
                        );
                    });
                });
                // Line 2: the CPU + platform identity.
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&cpu_line));
                // Line 3: the RAM summary + channel + the mode segment
                // (AMBER 1:1, CRIMSON 1:2, the default color for N/A).
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&ram_prefix));
                    if !ram_mode.is_empty() {
                        match ram_mode_color {
                            Some(color) => ui.label(egui::RichText::new(&ram_mode).color(color)),
                            None => ui.label(egui::RichText::new(&ram_mode)),
                        };
                    }
                });
                if let Some((text, _)) = notice {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(text).color(notice_color(text)));
                }
            });
    }

    /// The three zones: the telemetry matrix (zone 1) on the left; the
    /// benchmark grid + controls (zone 2) stacked over the hardware /
    /// SPD status (zone 3) on the right as two slices (C9-05, D-4b:
    /// the bench at its natural height, the status at the remaining
    /// column height, the 8 pt gap preserved) — the columns take the
    /// central panel's full height (C7-19 removed the embedded
    /// 10-minute trend history strip; the trend data now lives in
    /// the Graphs window, C7-21) — all visible, non-scrolling at the
    /// 968×600 size (in a small window the right column's slices
    /// degrade to a scroll area — the overflow fallback). Returns the
    /// [`GuiAction`] the status zone reported this frame (`None` when
    /// no button was clicked).
    fn render_zones(&self, ctx: &egui::Context, data: &TelemetryData) -> GuiAction {
        let mut action = GuiAction::None;
        let bench_tx = &self.bench_tx;
        let cancel = &self.cancel;
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(SLATE))
            .show(ctx, |ui| {
                // The columns take the central panel's full height
                // (C7-19 removed the bottom history strip's fixed
                // budget — no window-level scroll, the single
                // non-scrolling dashboard). A pathological window
                // collapses the row to zero (never a negative
                // allocation, never a panic).
                let row_h = (ui.available_size().y - COLUMN_GAP).max(0.0);
                // The style's item_spacing, captured before the row
                // zeroes its main axis below (the row manages its
                // horizontal spacing explicitly, D-15.1); each
                // column's content restores it — the zones' grids
                // read item_spacing.x for their inner gaps.
                let item_spacing = ui.spacing().item_spacing;
                ui.horizontal(|ui| {
                    // The row is spaced entirely by its explicit
                    // `add_space` calls (the two OUTER_MARGINs + the
                    // COLUMN_GAP). Zero the layout's automatic
                    // main-axis item_spacing so it does not double-
                    // count against the split budget (D-15.1) — the
                    // cross-axis (y) spacing is kept and inherited by
                    // each column's vertical content.
                    ui.style_mut().spacing.item_spacing =
                        egui::vec2(0.0, item_spacing.y);
                    ui.add_space(OUTER_MARGIN);
                    let avail = ui.available_size();
                    // The split is computed against `inner` (avail.x
                    // minus the trailing OUTER_MARGIN the row ends
                    // with after the right column): left_w +
                    // COLUMN_GAP + right_w + OUTER_MARGIN == avail.x
                    // exactly, the right column always keeps
                    // MIN_RIGHT_W, and the right allocation ends
                    // OUTER_MARGIN (8 pt) inside the panel edge —
                    // the frame stroke's 0.5 pt outer half clears
                    // the clip rect (D-15.1). A pathological avail.x
                    // below both minima degenerates left-first
                    // (never a negative allocation, never a panic).
                    let inner = (avail.x - OUTER_MARGIN).max(0.0);
                    let max_left = (inner - MIN_RIGHT_W - COLUMN_GAP).max(0.0);
                    let min_left = MIN_LEFT_W.min(max_left);
                    let left_w = (inner * 0.55).clamp(min_left, max_left);
                    let right_w = (inner - left_w - COLUMN_GAP).max(0.0);
                    // Left: zone 1 (the live timing matrix, its own
                    // bounded scroll area).
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(left_w, row_h),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.style_mut().spacing.item_spacing =
                                item_spacing;
                            render_telemetry_zone(ui, data)
                        },
                    );
                    ui.add_space(COLUMN_GAP);
                    // Right: zone 2 over zone 3 as two stacked
                    // slices (C9-05, D-4b): the bench slice at its
                    // natural height, the preserved 8 pt gap, then
                    // the status slice at the remaining column
                    // height — two `allocate_ui_with_layout` children
                    // of the column's single allocation (the border /
                    // balance against zone 1 is unchanged). The
                    // `ScrollArea` stays as the overflow fallback:
                    // in a small window the bench's natural height
                    // alone exceeds the column, the status slice
                    // takes its zero budget, and the content scrolls
                    // below — never a clipped / overflowing paint.
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(right_w, row_h),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.style_mut().spacing.item_spacing =
                                item_spacing;
                            let _ = egui::ScrollArea::vertical().show(ui, |ui| {
                                // The slice width is the scroll
                                // content's available width:
                                // `right_w` with no scrollbar,
                                // `right_w` minus the gutter while
                                // the fallback scrolls (today's
                                // exact width behavior — no
                                // regression).
                                let slice_w = ui.available_size().x.max(0.0);
                                // Slice 1: the bench at its natural
                                // height — the zero-budget
                                // allocation: top-down content
                                // self-sizes past the zero rect (egui
                                // never clips a child to it) and
                                // advances the layout by the drawn
                                // `min_rect`, whose height is the
                                // natural size.
                                let bench = ui.allocate_ui_with_layout(
                                    egui::Vec2::new(slice_w, 0.0),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| render_bench_zone(ui, data, bench_tx, cancel),
                                );
                                let bench_h = bench.response.rect.height().max(0.0);
                                ui.add_space(8.0);
                                // Slice 2: the status at the
                                // remaining column height
                                // (`row_h - bench_h - 8`, minus the
                                // item gap egui leaves after the
                                // bench allocation — reserving it
                                // keeps the filled status slice
                                // (C9-07) exactly at the column's
                                // bottom: no permanent scrollbar at
                                // 968×600, C7-19). Zero when the
                                // bench alone overflows (the scroll
                                // fallback above).
                                let gap = ui.spacing().item_spacing.y;
                                let status_h = (row_h - bench_h - 8.0 - gap).max(0.0);
                                let _ = ui.allocate_ui_with_layout(
                                    egui::Vec2::new(slice_w, status_h),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        action = render_status_zone(ui, data);
                                    },
                                );
                            });
                        },
                    );
                    ui.add_space(OUTER_MARGIN);
                });
            });
        action
    }

    /// The settings strip (C6-30): shown below the header while
    /// `settings_open` — its own top panel (the per-frame allocation
    /// order: header, settings, central). The grid mutates
    /// `TelemetryData.settings` under the write lock — the render
    /// thread's one permitted write (no I/O, D6); the poller re-reads
    /// the knobs live every tick (C6-27), including the socket
    /// (C6-30), so a panel change takes effect without a restart.
    fn render_settings_area(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("ramsleuth_settings")
            .frame(
                egui::Frame::default()
                    .fill(SLATE)
                    .stroke(egui::Stroke::new(1.0_f32, HEADER_STROKE)),
            )
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Settings").strong().color(CYAN));
                ui.add_space(2.0);
                // The one permitted render-thread write (D6): no I/O.
                let mut guard = self.state.write().unwrap();
                render_settings_panel(ui, &mut guard.settings);
            });
    }

    /// Execute the status zone's [`GuiAction`] — the one side effect
    /// the render loop performs: F2 / F3 run [`perform_export`] (a
    /// one-shot file write into `out_dir`) and flash the result as the
    /// transient notice; Q sets the shared stop flag and closes the
    /// viewport (the poller is joined on app drop).
    fn handle_action(&mut self, ctx: &egui::Context, action: GuiAction) {
        match action {
            GuiAction::SnapshotPng | GuiAction::ExportJson => {
                let data = self.state.read().unwrap();
                let text = match perform_export(action, &data, &self.out_dir) {
                    Ok(Some(path)) => format!("wrote {}", path.display()),
                    Ok(None) => "nothing to export yet (no data)".to_owned(),
                    Err(error) => format!("! {error}"),
                };
                self.notice = Some((text, Instant::now()));
            }
            GuiAction::Quit => {
                self.stop.store(true, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            GuiAction::None => {}
        }
    }

    /// The Graphs window's telemetry lifecycle (C9-02, D-2): run
    /// once per frame from `update` — the single `graphs_open`
    /// transition detector. The opening edge saves the current
    /// refresh gate and forces it on (the window's live streaming
    /// runs regardless of the auto-refresh knob — the poller
    /// re-reads it every tick, C6-27, so no poller change is
    /// needed); the closing edge restores the saved pre-open value
    /// and clears it. Both close paths (the header button toggle +
    /// the child's WM `close_requested`) clear the same shared
    /// flag, so one falling edge covers both (D-C7) — no per-site
    /// hooks. The settings write is the render thread's one
    /// permitted mutation (no I/O, D6 — the settings-panel
    /// precedent); the brief write lock is taken only on an edge,
    /// never per frame.
    fn apply_graphs_lifecycle(&mut self) {
        let now_open = self.graphs_open.load(Ordering::Relaxed);
        if now_open != self.last_graphs_open {
            let mut guard = self.state.write().unwrap();
            if let Some(refresh) = apply_refresh_transition(
                &mut self.saved_refresh,
                &mut self.last_graphs_open,
                now_open,
                guard.settings.refresh_enabled,
            ) {
                guard.settings.refresh_enabled = refresh;
            }
        }
    }
}

// ---------------------------------------------------------------------
// The Graphs window (C7-21, D-3): the eframe 0.27.2 native
// multi-viewport spawn — one deferred child OS window sharing the
// app's egui context + event loop (no second eframe lifecycle,
// Wayland-safe), re-registered by the root every frame while open
// (the keep-alive; egui GCs a child the first frame the root stops
// registering it, which is the close).
// ---------------------------------------------------------------------

/// The Graphs child viewport's stable id (C7-21, D-3): the same id
/// on every re-registration, so `show_viewport_deferred` patches
/// the existing registration in place (no duplicate window, no
/// title-bar reshuffle) and the keep-alive / GC mechanics key off
/// one identity. A function, not a `const` — `egui::Id::new` is
/// not const in the pinned egui 0.27.2.
fn graphs_viewport_id() -> egui::ViewportId {
    egui::ViewportId(egui::Id::new("ramsleuth_graphs_window"))
}

/// The header's `Graphs` button (C7-21, D-3): `selected` while the
/// window is open (the shared flag read live each frame), and a
/// click toggles the flag — `false` → the window spawns on the next
/// frame, `true` → the root stops re-registering the viewport, so
/// egui GCs the child and the OS window closes (D-C7: the button's
/// close and the WM's close button are behaviorally identical).
fn graphs_button(ui: &mut egui::Ui, graphs_open: &Arc<AtomicBool>) -> egui::Response {
    let open = graphs_open.load(Ordering::Relaxed);
    let response = ui.add(egui::Button::new(egui::RichText::new("Graphs")).selected(open));
    if response.clicked() {
        graphs_open.store(!open, Ordering::Relaxed);
    }
    response
}

/// Register (or re-register) the Graphs deferred child viewport
/// (C7-21, D-3) — the keep-alive: the app calls this every frame
/// while `graphs_open` is set; the same builder + id each time
/// patches the existing registration in place (idempotent), and a
/// frame without a call lets egui GC the child (the close). The
/// child's body ([`run_graphs_child_frame`]) runs on its own native
/// window's frame with its own viewport input over this one shared
/// context — no second eframe lifecycle, no second event loop (the
/// Wayland-safe design, D-3).
fn show_graphs_viewport(
    ctx: &egui::Context,
    state: &Arc<RwLock<TelemetryData>>,
    graphs_open: &Arc<AtomicBool>,
) {
    let shared = Arc::clone(state);
    let flag = Arc::clone(graphs_open);
    ctx.show_viewport_deferred(
        graphs_viewport_id(),
        egui::ViewportBuilder::default()
            .with_title("RamSleuth — Graphs")
            .with_inner_size(GRAPHS_WINDOW_SIZE)
            .with_min_inner_size(GRAPHS_WINDOW_MIN_SIZE),
        move |child_ctx, _class| {
            run_graphs_child_frame(child_ctx, &shared, &flag);
        },
    );
}

/// The Graphs child viewport's frame body (C7-21, D-3): one
/// idempotent style set over the shared context (the main window's
/// `AppCreator` already set it — this keeps a fresh context correct
/// too), one brief write of the shared state for the render — the
/// C9-03 `Poll` combo's write of the shared
/// `settings.poll_interval_ms` knob (the settings-panel D-2 / D6
/// write precedent: the render thread's one permitted write, no
/// I/O; the poller stays the only writer of the data fields) — the
/// window's render, the child's own ~60 FPS cadence, and the WM
/// close path — a `close_requested()` clears the shared flag so
/// the root stops re-registering and egui GCs the window (the
/// button's close is the same path, D-C7).
fn run_graphs_child_frame(
    ctx: &egui::Context,
    state: &Arc<RwLock<TelemetryData>>,
    graphs_open: &Arc<AtomicBool>,
) {
    ctx.set_style(build_style());
    {
        // The C9-03 co-land: a write lock (the settings-panel D6
        // write precedent — the `Poll` combo writes the shared
        // `settings.poll_interval_ms` knob the poller re-reads
        // live; the data fields stay poller-written). The knob is
        // copied out (a `u64`) and written back: a shared borrow of
        // `data.graph` and a mutable borrow of the knob through the
        // guard's deref cannot coexist in one expression.
        let mut data = state.write().unwrap();
        let mut poll_interval_ms = data.settings.poll_interval_ms;
        render_graphs_window(ctx, &data.graph, &mut poll_interval_ms);
        data.settings.poll_interval_ms = poll_interval_ms;
    }
    ctx.request_repaint_after(REPAINT);
    if ctx.input(|i| i.viewport().close_requested()) {
        graphs_open.store(false, Ordering::Relaxed);
    }
}

/// The Graphs window's telemetry lifecycle transition (C9-02, D-2):
/// the pure edge logic over the detector's memory — the app's
/// [`RamSleuthApp::apply_graphs_lifecycle`] runs it on each edge
/// under a brief write lock. The opening edge (closed → open) saves
/// the current refresh gate and forces it on (the window's live
/// streaming runs regardless of the auto-refresh knob); the closing
/// edge (open → closed) restores the saved pre-open value and
/// clears it. Both close paths (the header button toggle + the
/// child's WM `close_requested`) clear the same shared flag, so one
/// falling edge covers both (D-C7). A no-edge frame writes nothing.
/// Returns the `refresh_enabled` to write, or `None` for no change.
fn apply_refresh_transition(
    saved: &mut Option<bool>,
    last: &mut bool,
    now_open: bool,
    current: bool,
) -> Option<bool> {
    let change = if now_open && !*last {
        // Opening edge: save the original, then force on.
        *saved = Some(current);
        Some(true)
    } else if !now_open && *last {
        // Closing edge: restore the saved original (`None` — the
        // defensive no-op when nothing was saved).
        saved.take()
    } else {
        // No edge: nothing changes.
        None
    };
    *last = now_open;
    change
}

fn main() -> ExitCode {
    // 1. Parse the command line (usage error → exit 2).
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ramsleuth-gui: {error}");
            eprintln!("{USAGE}");
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
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
                graphs_open,
                saved_refresh: None,
                last_graphs_open: false,
                out_dir,
                poller: Some(poller),
                notice: None,
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
            eprintln!("ramsleuth-gui: {error}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------------
// Tests (headless: no display, no daemon — the eframe app itself is
// compile-checked; the live window run is verified in the QA phase).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use ramsleuth_bench::BenchmarkGrid;
    use ramsleuth_gui::{BenchState, CapacityUnit, ClockUnit, DEFAULT_POLL_INTERVAL_MS};
    use ramsleuth_telemetry::amd_readout::{ClockReadout, DivMode};
    use ramsleuth_telemetry::cpuid::{AmdZen, CpuInfo, CpuVendor, IntelGen};
    use ramsleuth_telemetry::error::{NaReason, Section};
    use ramsleuth_telemetry::spd_decode::SpdModule;
    use ramsleuth_telemetry::{SystemMemoryTelemetry, SystemPlatform};

    use super::*;

    /// A unique temp out dir per test (pid-scoped — the style / client
    /// temp-path precedent); created now, removed by the test.
    fn temp_out_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ramsleuth-gui-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("the test out dir must be created");
        dir
    }

    /// A representative benchmark grid (the Phase 1 reference
    /// magnitudes — the style test fixture).
    fn fixture_grid() -> BenchmarkGrid {
        BenchmarkGrid {
            read_gbps: [26.35, 35.10, 30.40, 12.11],
            write_gbps: [43.63, 38.20, 33.05, 14.02],
            copy_gbps: [12.11, 36.40, 31.80, 11.05],
            latency_ns: [86.84, 1.12, 4.20, 13.90],
        }
    }

    /// (a) No flags → the default socket (the protocol's frozen value).
    #[test]
    fn parse_args_no_flags_defaults() {
        let args = parse_args(Vec::<String>::new()).expect("no flags must parse");
        assert_eq!(args, GuiArgs::default());
        assert_eq!(args.socket, PathBuf::from(DEFAULT_SOCKET_PATH));
    }

    /// (a) `--socket /tmp/x` → that socket (a repeat wins — last
    /// value).
    #[test]
    fn parse_args_socket_flag() {
        let args = parse_args(["--socket", "/tmp/x"]).expect("--socket /tmp/x must parse");
        assert_eq!(args.socket, PathBuf::from("/tmp/x"));

        let args =
            parse_args(["--socket", "/tmp/a", "--socket", "/tmp/b"]).expect("repeats must parse");
        assert_eq!(args.socket, PathBuf::from("/tmp/b"));
    }

    /// (a) An unknown flag → an error naming it.
    #[test]
    fn parse_args_unknown_flag_errors() {
        let err = parse_args(["--bogus"]).expect_err("an unknown flag must error");
        assert!(err.contains("--bogus"), "the error must name the flag, got: {err}");
    }

    /// (a) A positional argument / a missing `--socket` value → an
    /// error.
    #[test]
    fn parse_args_positional_and_missing_value_error() {
        assert!(parse_args(["/tmp/x"]).is_err(), "a positional must error");
        assert!(
            parse_args(["--socket"]).is_err(),
            "a missing --socket value must error"
        );
    }

    /// (b) `ExportJson` with telemetry set → a JSON file is written to
    /// the out dir, `Ok(Some(path))`, the file exists + parses as a
    /// JSON object carrying the terminal grid under the `bench` key
    /// (C6-29 — the grid is exported alongside the telemetry).
    #[test]
    fn perform_export_json_writes_and_parses() {
        let dir = temp_out_dir("export");
        let data = TelemetryData {
            telemetry: Some(ramsleuth_telemetry::collect()),
            bench: BenchState { grid: Some(fixture_grid()), ..Default::default() },
            ..Default::default()
        };

        let path = perform_export(GuiAction::ExportJson, &data, &dir)
            .expect("the export must not fail")
            .expect("telemetry set must produce a file");
        assert!(path.starts_with(&dir), "the file must land in the out dir: {path:?}");
        let name = path.file_name().and_then(|s| s.to_str()).expect("the file must be named");
        assert!(name.starts_with("ramsleuth-export-"), "got: {name}");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("json"));

        let text = fs::read_to_string(&path).expect("the exported file must be readable");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("the export must be valid JSON");
        assert!(value.is_object(), "a snapshot must serialize to a JSON object");
        assert!(
            value["bench"].is_object(),
            "a completed run must export the bench grid object, got: {}",
            value["bench"]
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// (c) `SnapshotPng` with a grid set → a PNG file is written to
    /// the out dir, `Ok(Some(path))`, and its first 8 bytes are the
    /// PNG signature.
    #[test]
    fn perform_export_png_writes_valid_png() {
        let dir = temp_out_dir("snapshot");
        let data = TelemetryData {
            bench: BenchState { grid: Some(fixture_grid()), ..Default::default() },
            ..Default::default()
        };

        let path = perform_export(GuiAction::SnapshotPng, &data, &dir)
            .expect("the snapshot must not fail")
            .expect("a grid set must produce a file");
        assert!(path.starts_with(&dir), "the file must land in the out dir: {path:?}");
        let name = path.file_name().and_then(|s| s.to_str()).expect("the file must be named");
        assert!(name.starts_with("ramsleuth-snapshot-"), "got: {name}");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));

        const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let bytes = fs::read(&path).expect("the snapshot file must be readable");
        assert_eq!(&bytes[..8], &PNG_SIG, "the file must start with the PNG signature");

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// (d) `SnapshotPng` with no grid → `Ok(None)` and no file is
    /// written.
    #[test]
    fn perform_export_png_without_grid_is_none() {
        let dir = temp_out_dir("snapshot-none");
        let data = TelemetryData::default(); // no grid yet

        let result =
            perform_export(GuiAction::SnapshotPng, &data, &dir).expect("no grid must not fail");
        assert_eq!(result, None, "no grid must yield Ok(None)");
        assert!(
            dir.read_dir().expect("the out dir must exist").count() == 0,
            "no file must be written"
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// (d) mirror: `ExportJson` with no telemetry → `Ok(None)` and no
    /// file is written.
    #[test]
    fn perform_export_json_without_telemetry_is_none() {
        let dir = temp_out_dir("export-none");
        let data = TelemetryData::default(); // no snapshot yet

        let result =
            perform_export(GuiAction::ExportJson, &data, &dir).expect("no telemetry must not fail");
        assert_eq!(result, None, "no telemetry must yield Ok(None)");
        assert!(
            dir.read_dir().expect("the out dir must exist").count() == 0,
            "no file must be written"
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// (e) `Quit` / `None` → `Ok(None)` and no file is written — even
    /// with data present.
    #[test]
    fn perform_export_quit_and_none_are_noops() {
        let dir = temp_out_dir("noop");
        let data = TelemetryData {
            telemetry: Some(ramsleuth_telemetry::collect()),
            bench: BenchState { grid: Some(fixture_grid()), ..Default::default() },
            ..Default::default()
        };

        for action in [GuiAction::Quit, GuiAction::None] {
            let result =
                perform_export(action, &data, &dir).expect("a noop action must not fail");
            assert_eq!(result, None, "{action:?} must yield Ok(None)");
        }
        assert!(
            dir.read_dir().expect("the out dir must exist").count() == 0,
            "a noop action must write no file"
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// A failed export (a missing out dir) is a structured
    /// `GuiError::Io`, never a panic (the no-panic contract, plan D5 —
    /// the style test precedent).
    #[test]
    fn perform_export_missing_dir_is_io_error() {
        let data = TelemetryData {
            telemetry: Some(ramsleuth_telemetry::collect()),
            ..Default::default()
        };
        let missing = std::env::temp_dir()
            .join(format!("ramsleuth-gui-nodir-{}", std::process::id()))
            .join("no-such-subdir");
        let err = perform_export(GuiAction::ExportJson, &data, &missing)
            .expect_err("a missing dir must fail");
        assert!(
            matches!(err, GuiError::Io(_)),
            "a missing out dir must map to GuiError::Io, got: {err:?}"
        );
    }

    /// `header_status_color`: CYAN only when connected + healthy;
    /// CRIMSON for an error, a disconnected status, or a never-polled
    /// (empty) status.
    #[test]
    fn header_status_color_semantics() {
        let healthy = TelemetryData {
            daemon_status: "connected: /tmp/x".to_owned(),
            error: None,
            ..Default::default()
        };
        assert_eq!(header_status_color(&healthy), CYAN);

        let errored = TelemetryData {
            daemon_status: "connected: /tmp/x".to_owned(),
            error: Some("boom".to_owned()),
            ..Default::default()
        };
        assert_eq!(header_status_color(&errored), CRIMSON);

        let disconnected =
            TelemetryData { daemon_status: "disconnected".to_owned(), ..Default::default() };
        assert_eq!(header_status_color(&disconnected), CRIMSON);

        assert_eq!(header_status_color(&TelemetryData::default()), CRIMSON);
    }

    /// `notice_color`: a written file reads CYAN, a failed export
    /// CRIMSON, the informational hint AMBER.
    #[test]
    fn notice_color_semantics() {
        assert_eq!(notice_color("wrote /home/x/ramsleuth-export-1.json"), CYAN);
        assert_eq!(notice_color("! export I/O error: gone"), CRIMSON);
        assert_eq!(notice_color("nothing to export yet (no data)"), AMBER);
    }

    /// `unix_timestamp`: a plausible wall-clock second count (2020 –
    /// 2100), never a panic.
    #[test]
    fn unix_timestamp_is_plausible() {
        let ts = unix_timestamp();
        assert!(ts > 1_577_836_800, "expected a post-2020 timestamp, got: {ts}");
        assert!(ts < 4_102_444_800, "expected a pre-2100 timestamp, got: {ts}");
    }

    // ------------------------------------------------------------------
    // C6-20: the header's pure line builders (no egui context, no
    // display — the live window run is the QA gate).
    // ------------------------------------------------------------------

    /// A header-test snapshot: a known AMD Zen 3 CPU + a populated
    /// platform (one Na cell — AGESA, the common case), a
    /// configurable capacity tail, an Na AMD / Intel branch by
    /// default, and a configurable SPD list.
    fn fixture_telemetry(
        total_capacity: Section<f64>,
        dimm_sizes: Vec<Section<f64>>,
        spd: Vec<SpdModule>,
    ) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor: CpuVendor::Amd(AmdZen::Zen3),
                brand: "Ryzen 9 5950X".to_owned(),
            },
            amd: Section::na(NaReason::NotApplicable),
            intel: Section::na(NaReason::NotApplicable),
            spd,
            platform: SystemPlatform {
                cpu_clock_mhz: Section::Value(3600.0),
                motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
                bios: Section::Value("F60 + 09/15/2024".to_owned()),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity,
            dimm_sizes,
        }
    }

    /// A minimal SPD module fixture (every cell Na except the speed).
    fn fixture_spd_module(speed_mts: Option<u16>) -> SpdModule {
        SpdModule {
            index: 0x52,
            is_ddr5: false,
            maker: Section::na(NaReason::NotApplicable),
            die_maker: Section::na(NaReason::NotApplicable),
            die_type: Section::na(NaReason::NotApplicable),
            devices: Section::na(NaReason::NotApplicable),
            part: Section::na(NaReason::NotApplicable),
            serial: Section::na(NaReason::NotApplicable),
            rank: Section::na(NaReason::NotApplicable),
            density_mbit: Section::na(NaReason::NotApplicable),
            speed_mts: match speed_mts {
                Some(value) => Section::Value(value),
                None => Section::na(NaReason::NotApplicable),
            },
            profiles: Vec::new(),
        }
    }

    /// A clock-readout fixture (every cell Na except the two the mode
    /// segment consumes).
    fn fixture_clocks(div_mode: Option<DivMode>, mclk_mhz: Option<f64>) -> ClockReadout {
        ClockReadout {
            mclk_mhz: mclk_mhz
                .map(Section::Value)
                .unwrap_or_else(|| Section::na(NaReason::NotApplicable)),
            uclk_mhz: Section::na(NaReason::NotApplicable),
            fclk_mhz: Section::na(NaReason::NotApplicable),
            div_mode: div_mode
                .map(Section::Value)
                .unwrap_or_else(|| Section::na(NaReason::NotApplicable)),
            gear_mode: Section::na(NaReason::NotApplicable),
            gdm: Section::na(NaReason::NotApplicable),
            pdm: Section::na(NaReason::NotApplicable),
            command_rate: Section::na(NaReason::NotApplicable),
        }
    }

    /// (h1) `platform_tag`: the vendor → socket-family map — Zen 1–3
    /// → AM4, Zen 4/5 → AM5, Intel → LGA, unknown → the honest bare
    /// `Platform`.
    #[test]
    fn platform_tag_maps_every_vendor() {
        assert_eq!(platform_tag(&CpuVendor::Amd(AmdZen::Zen1)), "AMD AM4 Platform");
        assert_eq!(platform_tag(&CpuVendor::Amd(AmdZen::Zen2)), "AMD AM4 Platform");
        assert_eq!(platform_tag(&CpuVendor::Amd(AmdZen::Zen3)), "AMD AM4 Platform");
        assert_eq!(platform_tag(&CpuVendor::Amd(AmdZen::Zen4)), "AMD AM5 Platform");
        assert_eq!(platform_tag(&CpuVendor::Amd(AmdZen::Zen5)), "AMD AM5 Platform");
        assert_eq!(
            platform_tag(&CpuVendor::Intel(IntelGen::AlderLake)),
            "Intel LGA Platform"
        );
        assert_eq!(platform_tag(&CpuVendor::Unknown), "Platform");
    }

    /// (h2) `daemon_status_text`: a successful poll names the
    /// configured socket, the never-polled (empty) state shows
    /// `Disconnected` + the socket we are trying, and a recorded
    /// failure is a bare `Disconnected`.
    #[test]
    fn daemon_status_text_arms() {
        let socket = Path::new("/run/ramsleuth/ramsleuth.sock");
        let connected = TelemetryData {
            daemon_status: "connected: /run/ramsleuth/ramsleuth.sock".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            daemon_status_text(&connected, socket),
            "Daemon: Connected (IPC: /run/ramsleuth/ramsleuth.sock)"
        );

        assert_eq!(
            daemon_status_text(&TelemetryData::default(), socket),
            "Daemon: Disconnected (IPC: /run/ramsleuth/ramsleuth.sock)"
        );

        let down =
            TelemetryData { daemon_status: "disconnected".to_owned(), ..Default::default() };
        assert_eq!(daemon_status_text(&down, socket), "Daemon: Disconnected");
    }

    /// (h3) `cpu_line_text`: the spec's line 2 — brand + the clock in
    /// the selected clock unit (C7-11: `format_clock` — the default
    /// MHz keeps the carried wire value, the GHz arm ÷1000) +
    /// motherboard / BIOS + the provenance fragment (C8-11, D-1) —
    /// with both cells Na it degrades to `AGESA N/A`; a fully Na
    /// platform degrades every segment.
    #[test]
    fn cpu_line_text_populated_and_na_degraded() {
        let t = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            Vec::new(),
        );
        assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA N/A)"
        );

        // The GHz knob (C7-11): the same carried clock renders ÷1000
        // (3600 MHz → 3.6 GHz).
        let ghz = Units { clock: ClockUnit::GHz, ..Units::default() };
        assert_eq!(
            cpu_line_text(&t, &ghz),
            "CPU: Ryzen 9 5950X @ 3.6 GHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA N/A)"
        );

        let all_na = SystemMemoryTelemetry {
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            ..t
        };
        assert_eq!(
            cpu_line_text(&all_na, &Units::default()),
            "CPU: Ryzen 9 5950X @ N/A | Motherboard: N/A (BIOS: N/A, AGESA N/A)"
        );
    }

    /// (h3′) `age_fragment` (C8-11, D-1): the provenance arms — a
    /// true AGESA (the BIOS-string scan) wins and suppresses the SMU
    /// version (D-1's precedence); else the shape-checked `ryzen_smu`
    /// firmware version renders under its own `SMU` label; else the
    /// honest `AGESA N/A`. No `AGESA <smu value>` hybrid, never
    /// fabricated.
    #[test]
    fn age_fragment_by_provenance() {
        let base = SystemPlatform {
            cpu_clock_mhz: Section::Value(3600.0),
            motherboard: Section::Value("ProArt X570-CREATOR".to_owned()),
            bios: Section::Value("5601".to_owned()),
            agesa: Section::na(NaReason::NotApplicable),
            smu_version: Section::na(NaReason::NotApplicable),
        };
        assert_eq!(age_fragment(&base), "AGESA N/A");

        let smu = SystemPlatform {
            smu_version: Section::Value("56.78.0".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&smu), "SMU 56.78.0");

        let agesa = SystemPlatform {
            agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&agesa), "AGESA ComboAm4v2 PI 1.2.0.12");

        // The precedence (D-1): a true AGESA suppresses the SMU value.
        let both = SystemPlatform {
            agesa: Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned()),
            smu_version: Section::Value("56.78.0".to_owned()),
            ..base.clone()
        };
        assert_eq!(age_fragment(&both), "AGESA ComboAm4v2 PI 1.2.0.12");
    }

    /// (h3″) `cpu_line_text`'s provenance fragment in the header
    /// (C8-11, D-1): the live host shape — `bios = 5601` (a bare OEM
    /// build code, no AGESA token), no true AGESA (the real string is
    /// root-gated), the `ryzen_smu` firmware `56.78.0` → `… (BIOS:
    /// 5601, SMU 56.78.0)` — the reported `AGESA 56.78.0` mislabel
    /// pinned as a regression; a true AGESA renders `AGESA <v>` with
    /// the SMU value suppressed (D-1's precedence).
    #[test]
    fn cpu_line_text_provenance_fragment() {
        // The live host shape: the SMU firmware under its true label.
        let mut t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
        t.platform.bios = Section::Value("5601".to_owned());
        t.platform.smu_version = Section::Value("56.78.0".to_owned());
        assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: 5601, SMU 56.78.0)"
        );

        // A true AGESA (the BIOS-string scan) wins over the SMU value.
        let mut t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
        t.platform.agesa = Section::Value("ComboAm4v2 PI 1.2.0.12".to_owned());
        t.platform.smu_version = Section::Value("56.78.0".to_owned());
        assert_eq!(
            cpu_line_text(&t, &Units::default()),
            "CPU: Ryzen 9 5950X @ 3600 MHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA ComboAm4v2 PI 1.2.0.12)"
        );
    }

    /// (h4) `dimm_summary`: distinct-size grouping in the selected
    /// capacity unit (C7-11: `format_capacity` — default GiB, the GB
    /// knob converts × 1.073741824) — 2×16 → `2x16 GiB`, a mixed kit
    /// → `1x16 GiB + 1x32 GiB` (the Na entry contributes nothing),
    /// all-Na / empty → `N/A`, a non-whole size keeps one decimal;
    /// C9-01 (D-1): the parallel SPD list's rank word groups with
    /// the size — same-size same-rank stays one `<count>x` group
    /// with the word (`2x16 GiB Single-Rank`), same-size
    /// different-rank splits into two groups, and a `Na`/`0` rank
    /// omits the word (the bare form; the mixed-kit case keeps
    /// `1x16 GiB + 1x32 GiB` when the ranks are absent).
    #[test]
    fn dimm_summary_groups_and_degrades() {
        let default_units = Units::default();
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[], &default_units),
            "2x16 GiB"
        );
        assert_eq!(
            dimm_summary(
                &[
                    Section::Value(16.0),
                    Section::na(NaReason::NotApplicable),
                    Section::Value(32.0),
                ],
                &[],
                &default_units,
            ),
            "1x16 GiB + 1x32 GiB"
        );
        assert_eq!(
            dimm_summary(&[Section::na(NaReason::NotApplicable)], &[], &default_units),
            "N/A"
        );
        assert_eq!(dimm_summary(&[], &[], &default_units), "N/A");
        assert_eq!(
            dimm_summary(&[Section::Value(4.5)], &[], &default_units),
            "1x4.5 GiB"
        );

        // The GB knob (C7-11): 16 GiB → 17.2 GB per group.
        let gb = Units { capacity: CapacityUnit::GB, ..Units::default() };
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)], &[], &gb),
            "2x17.2 GB"
        );

        // C9-01 (D-1): the rank word from the parallel SPD list
        // groups with the size (single-rank fixture modules).
        let single_rank = || {
            let mut module = fixture_spd_module(Some(3200));
            module.rank = Section::Value(1);
            module
        };
        let dual_rank = || {
            let mut module = fixture_spd_module(Some(3200));
            module.rank = Section::Value(2);
            module
        };
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(16.0)],
                &[single_rank(), single_rank()],
                &default_units,
            ),
            "2x16 GiB Single-Rank"
        );
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(16.0)],
                &[single_rank(), dual_rank()],
                &default_units,
            ),
            "1x16 GiB Single-Rank + 1x16 GiB Dual-Rank"
        );
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(32.0)],
                &[single_rank(), dual_rank()],
                &default_units,
            ),
            "1x16 GiB Single-Rank + 1x32 GiB Dual-Rank"
        );
        // A `Na` rank omits the word: the ranked + unranked pair
        // splits into two groups (first-seen order).
        assert_eq!(
            dimm_summary(
                &[Section::Value(16.0), Section::Value(16.0)],
                &[single_rank(), fixture_spd_module(None)],
                &default_units,
            ),
            "1x16 GiB Single-Rank + 1x16 GiB"
        );
    }

    /// (h4b) `rank_word` (C9-01, D-1): the SPD cards' rank label
    /// for the compact header — `1` → `Single-Rank`, `2` →
    /// `Dual-Rank`, `n > 0` → `<n>-Rank` — with a `Na`/`0` rank
    /// yielding `None` (the word is omitted, never `N/A`).
    #[test]
    fn rank_word_arms() {
        assert_eq!(rank_word(&Section::Value(1)), Some("Single-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(2)), Some("Dual-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(4)), Some("4-Rank".to_owned()));
        assert_eq!(rank_word(&Section::Value(0)), None);
        assert_eq!(rank_word(&Section::na(NaReason::NotApplicable)), None);
    }

    /// (h4c) `slot_note` (C9-01, D-1): the total-vs-breakdown
    /// note — total > SPD sum with a uniform module size
    /// near-dividing the total (the live host: 62.68 GiB over
    /// 2×16 GiB, the OS reserves a fraction) → `2 of 4 slots
    /// SPD-visible`; a non-integer quotient → the generic note;
    /// total ≤ sum, a `Na` total, or no visible modules → `None`
    /// (no false alarm).
    #[test]
    fn slot_note_arms() {
        // The live host shape: 4×16 GiB installed (62.68 GiB OS
        // total), 2 bound to the SPD bus (32 GiB sum).
        assert_eq!(
            slot_note(&Section::Value(62.68), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("2 of 4 slots SPD-visible".to_owned())
        );
        // An exact multiple (no reserved fraction): 3 slots, 2
        // visible.
        assert_eq!(
            slot_note(&Section::Value(48.0), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("2 of 3 slots SPD-visible".to_owned())
        );
        // A non-integer quotient (50/16 = 3.125, more than a tenth
        // of a module off) → the generic note.
        assert_eq!(
            slot_note(&Section::Value(50.0), &[Section::Value(16.0), Section::Value(16.0)]),
            Some("SPD sees 2 of the installed capacity".to_owned())
        );
        // total ≤ sum → no note (the breakdown accounts for the
        // whole total).
        assert_eq!(
            slot_note(&Section::Value(32.0), &[Section::Value(16.0), Section::Value(16.0)]),
            None
        );
        assert_eq!(
            slot_note(&Section::Value(16.0), &[Section::Value(16.0), Section::Value(16.0)]),
            None
        );
        // A `Na` total → no note.
        assert_eq!(
            slot_note(&Section::na(NaReason::NotApplicable), &[Section::Value(16.0)]),
            None
        );
        // No visible modules (all-Na sizes) → no note, even with a
        // `Value` total.
        assert_eq!(
            slot_note(&Section::Value(62.68), &[Section::na(NaReason::NotApplicable)]),
            None
        );
    }

    /// (h5) `channel_mode`: 1 / 2 / 4 → Single / Dual / Quad, every
    /// other count (0, odd) → `N/A` (D-C8).
    #[test]
    fn channel_mode_from_dimm_count() {
        assert_eq!(channel_mode(1), "Single-Channel");
        assert_eq!(channel_mode(2), "Dual-Channel");
        assert_eq!(channel_mode(4), "Quad-Channel");
        assert_eq!(channel_mode(0), "N/A");
        assert_eq!(channel_mode(3), "N/A");
    }

    /// (h6) `ram_line_prefix`: the spec's line 3 minus the mode —
    /// the total in the selected capacity unit (C7-11:
    /// `format_capacity` — default GiB, the GB knob converts
    /// × 1.073741824), the per-DIMM breakdown (C9-01, D-1: with
    /// the rank word — `2x16 GiB Single-Rank`), the max SPD speed
    /// (omitted when no module carries one), the channel mode, the
    /// total-vs-breakdown slot note when the OS total strictly
    /// exceeds the SPD sum (C9-01, D-1: `2 of 4 slots
    /// SPD-visible`), and the `Mode: ` lead-in; the degraded tail
    /// renders the honest N/A segments (no note — total ≤ sum / Na).
    #[test]
    fn ram_line_prefix_populated_and_degraded() {
        let t = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![fixture_spd_module(Some(3200))],
        );
        assert_eq!(
            ram_line_prefix(&t, &Units::default()),
            "RAM: 32 GiB (2x16 GiB) 3200 MT/s | Dual-Channel | Mode: "
        );

        let no_speed = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![fixture_spd_module(None)],
        );
        assert_eq!(
            ram_line_prefix(&no_speed, &Units::default()),
            "RAM: 32 GiB (2x16 GiB) | Dual-Channel | Mode: "
        );

        // A single Na DIMM still counts as one bound module (the
        // channel is the count, not the sizes): Single-Channel.
        let degraded = fixture_telemetry(
            Section::na(NaReason::NotApplicable),
            vec![Section::na(NaReason::NotApplicable)],
            Vec::new(),
        );
        assert_eq!(
            ram_line_prefix(&degraded, &Units::default()),
            "RAM: N/A (N/A) | Single-Channel | Mode: "
        );

        // No bound modules at all: every segment degrades to N/A.
        let empty = fixture_telemetry(
            Section::na(NaReason::NotApplicable),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            ram_line_prefix(&empty, &Units::default()),
            "RAM: N/A (N/A) | N/A | Mode: "
        );

        // The GB knob (C7-11): 32 GiB → 34.4 GB total, 16 GiB →
        // 17.2 GB per group.
        let gb = Units { capacity: CapacityUnit::GB, ..Units::default() };
        assert_eq!(
            ram_line_prefix(&t, &gb),
            "RAM: 34.4 GB (2x17.2 GB) 3200 MT/s | Dual-Channel | Mode: "
        );

        // C9-01 (D-1): the live host shape — single-rank modules
        // + the OS total (62.68 GiB) above the SPD sum (32 GiB):
        // the rank word in the breakdown + the slot note segment.
        let single_rank = || {
            let mut module = fixture_spd_module(Some(3200));
            module.rank = Section::Value(1);
            module
        };
        let ranked = fixture_telemetry(
            Section::Value(62.68),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![single_rank(), single_rank()],
        );
        assert_eq!(
            ram_line_prefix(&ranked, &Units::default()),
            "RAM: 62.7 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | 2 of 4 slots SPD-visible | Mode: "
        );

        // The rank word with total ≤ the SPD sum: no note (no
        // false alarm).
        let balanced = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![single_rank(), single_rank()],
        );
        assert_eq!(
            ram_line_prefix(&balanced, &Units::default()),
            "RAM: 32 GiB (2x16 GiB Single-Rank) 3200 MT/s | Dual-Channel | Mode: "
        );
    }

    /// (h7) `sync_mode_from_clocks`: D-C8's ratio segment — 1:1 with
    /// an MCLK (AMBER; C7-11: the MCLK in the selected clock unit —
    /// default MHz, the GHz knob ÷1000), 1:1 without (AMBER, the bare
    /// text), 1:2 (CRIMSON), and a Na ratio (the honest N/A, default
    /// color).
    #[test]
    fn sync_mode_from_clocks_arms() {
        let default_units = Units::default();
        assert_eq!(
            sync_mode_from_clocks(
                &fixture_clocks(Some(DivMode::OneToOne), Some(1800.0)),
                &default_units,
            ),
            ("Synchronous 1:1 (UCLK = MCLK = 1800 MHz)".to_owned(), Some(AMBER))
        );
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToOne), None), &default_units),
            ("Synchronous 1:1".to_owned(), Some(AMBER))
        );
        assert_eq!(
            sync_mode_from_clocks(
                &fixture_clocks(Some(DivMode::OneToTwo), Some(1800.0)),
                &default_units,
            ),
            ("Asynchronous 1:2".to_owned(), Some(CRIMSON))
        );
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(None, Some(1800.0)), &default_units),
            ("N/A".to_owned(), None)
        );

        // The GHz knob (C7-11): 1800 MHz → 1.8 GHz.
        let ghz = Units { clock: ClockUnit::GHz, ..Units::default() };
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToOne), Some(1800.0)), &ghz),
            ("Synchronous 1:1 (UCLK = MCLK = 1.8 GHz)".to_owned(), Some(AMBER))
        );
    }

    /// (h8) `sync_mode`: an Na AMD branch (Intel silicon / the
    /// driver missing) degrades the whole segment to an honest `N/A`
    /// — never a panic (independent of the clock unit — the Na
    /// branch renders no clock at all).
    #[test]
    fn sync_mode_degrades_on_na_amd_branch() {
        let t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
        assert_eq!(sync_mode(&t, &Units::default()), ("N/A".to_owned(), None));
        let ghz = Units { clock: ClockUnit::GHz, ..Units::default() };
        assert_eq!(sync_mode(&t, &ghz), ("N/A".to_owned(), None));
    }

    // ------------------------------------------------------------------
    // C6-30: the keyboard key map, the CLI socket seed, the settings
    // panel render.
    // ------------------------------------------------------------------

    /// (k1) `key_action`: the legend's three keys map to their
    /// actions; every other key (other function keys, letters,
    /// modifiers, navigation) yields `None` (no action this frame).
    #[test]
    fn key_action_maps_only_the_legend_keys() {
        assert_eq!(key_action(egui::Key::F2), Some(GuiAction::SnapshotPng));
        assert_eq!(key_action(egui::Key::F3), Some(GuiAction::ExportJson));
        assert_eq!(key_action(egui::Key::Q), Some(GuiAction::Quit));

        for key in [
            egui::Key::F1,
            egui::Key::F4,
            egui::Key::F12,
            egui::Key::A,
            egui::Key::Escape,
            egui::Key::Enter,
            egui::Key::Space,
            egui::Key::Num0,
            egui::Key::ArrowUp,
        ] {
            assert_eq!(key_action(key), None, "{key:?} must not fire an action");
        }
    }

    /// (k2) `seed_settings`: the CLI `--socket` becomes
    /// `settings.socket` (the panel shows the active socket, the
    /// poller reads it live), every other knob stays at its default
    /// (2 s poll, refresh off by default — one baseline fetch on
    /// connect; enable in Settings to poll); the no-flag default
    /// seeds the protocol's default socket.
    #[test]
    fn seed_settings_from_the_cli_socket() {
        let args = GuiArgs { socket: PathBuf::from("/tmp/ramsleuth-dev.sock") };
        let settings = seed_settings(&args);
        assert_eq!(settings.socket, "/tmp/ramsleuth-dev.sock");
        assert_eq!(settings.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
        assert!(!settings.refresh_enabled, "refresh must default off");
        assert_eq!(
            settings,
            GuiSettings {
                socket: "/tmp/ramsleuth-dev.sock".to_owned(),
                ..Default::default()
            }
        );

        let settings = seed_settings(&GuiArgs::default());
        assert_eq!(settings.socket, DEFAULT_SOCKET_PATH);
    }

    /// (k3) The settings panel renders headless without panicking
    /// (the `begin_frame` idiom, the history test precedent): the
    /// default settings + a mutated one (every widget — the socket
    /// edit, the drag, the combos, the checkbox — executes).
    #[test]
    fn settings_panel_renders_headless_without_panicking() {
        for settings in [
            GuiSettings::default(),
            GuiSettings {
                socket: "/tmp/x".to_owned(),
                poll_interval_ms: 5_000,
                ..Default::default()
            },
        ] {
            let mut settings = settings;
            let ctx = egui::Context::default();
            ctx.begin_frame(egui::RawInput::default());
            egui::CentralPanel::default().show(&ctx, |ui| {
                render_settings_panel(ui, &mut settings);
            });
        }
    }

    // ------------------------------------------------------------------
    // C7-21 (D-3): the Graphs button + the deferred child viewport
    // (headless: no display, no daemon — the eframe spawn is
    // compile-checked; the live Wayland window is this chunk's gate).
    // ------------------------------------------------------------------

    /// A synthesized primary-button click at `pos` (press + release
    /// in one frame — egui's interaction resolution confirms a
    /// `Released { click: Some(..) }` over the widget's previous-
    /// frame rect).
    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// (g1) The button's click-toggle semantics over the shared
    /// `Arc<AtomicBool>`: no click leaves the flag closed, a click
    /// on the button opens the window, and a click while open
    /// closes it (both toggle directions, headless — the eframe
    /// spawn itself is the live gate).
    #[test]
    fn graphs_button_click_toggles_the_shared_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1200.0, 300.0));

        let frame = |events: Vec<egui::Event>| -> egui::Rect {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            ctx.begin_frame(input);
            let mut rect = egui::Rect::NOTHING;
            egui::CentralPanel::default().show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    rect = graphs_button(ui, &flag).rect;
                });
            });
            let _ = ctx.end_frame();
            rect
        };

        // No click: stays closed; the button is laid out.
        let rect = frame(Vec::new());
        assert!(!flag.load(Ordering::Relaxed), "no click must not open the window");
        assert!(rect.area() > 0.0, "the button must be laid out, got {rect:?}");

        // A click at the button's center: opens.
        frame(click_events(rect.center()));
        assert!(flag.load(Ordering::Relaxed), "a click on the button must open the window");

        // A click while open: closes.
        let rect = frame(Vec::new());
        frame(click_events(rect.center()));
        assert!(
            !flag.load(Ordering::Relaxed),
            "a click while open must close the window"
        );
    }

    /// (g2) The full header renders the Graphs button headless
    /// without panicking in both open states (the k3 headless
    /// idiom — the button's `selected`-while-open rendering is
    /// compile-checked + live-gated), and a render never toggles
    /// the flag (only a click does).
    #[test]
    fn render_header_renders_the_graphs_button_headless_without_panicking() {
        for open in [false, true] {
            let flag = Arc::new(AtomicBool::new(open));
            let ctx = egui::Context::default();
            ctx.begin_frame(egui::RawInput::default());
            let mut settings_open = false;
            RamSleuthApp::render_header(
                &ctx,
                &TelemetryData::default(),
                &mut settings_open,
                &flag,
                &None,
            );
            let _ = ctx.end_frame();
            assert_eq!(flag.load(Ordering::Relaxed), open, "a render must not toggle the flag");
        }
    }

    /// (g3) The spawn / keep-alive / GC path over the real egui
    /// context (the D-3 mechanism, headless): a root frame with the
    /// flag open registers the deferred child (`ViewportClass::
    /// Deferred` in the frame's viewport output), a child frame
    /// renders the graph window over the shared state without
    /// closing it, the next root frame's re-registration keeps the
    /// child alive, and the first root frame without a re-
    /// registration GCs it (that IS the close).
    #[test]
    fn graphs_viewport_spawn_keep_alive_and_gc_headless() {
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let flag = Arc::new(AtomicBool::new(true));
        // The eframe desktop runtime disables embed mode (a bare
        // `Context::default()` leaves it on — the no-integration
        // fallback) so the deferred viewport is registered, not run
        // inline.
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let child = graphs_viewport_id();

        // Root frame 1: the spawn — the deferred child is registered.
        ctx.begin_frame(egui::RawInput::default());
        show_graphs_viewport(&ctx, &state, &flag);
        let out = ctx.end_frame();
        let registered = out.viewport_output.get(&child).cloned();
        assert!(registered.is_some(), "the deferred child must be registered on spawn");
        assert!(
            registered.expect("registered").class == egui::ViewportClass::Deferred,
            "the child must be a deferred (independent-window) viewport"
        );

        // The child's own frame: renders the window over the shared
        // state (a pure reader, D6) on its own cadence, without
        // closing it.
        let mut child_input = egui::RawInput { viewport_id: child, ..Default::default() };
        child_input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        ctx.begin_frame(child_input);
        run_graphs_child_frame(&ctx, &state, &flag);
        let child_out = ctx.end_frame();
        assert!(!child_out.shapes.is_empty(), "the child frame must paint the graph window");
        assert!(flag.load(Ordering::Relaxed), "a plain child frame must not close the window");

        // Root frame 2 (flag still open): the re-registration keeps
        // the child alive (the keep-alive).
        ctx.begin_frame(egui::RawInput::default());
        show_graphs_viewport(&ctx, &state, &flag);
        let out = ctx.end_frame();
        assert!(
            out.viewport_output.contains_key(&child),
            "the keep-alive re-registration must retain the child"
        );

        // Root frame 3 (flag closed): no re-registration — the
        // child is GC'd (the D-3 close).
        flag.store(false, Ordering::Relaxed);
        ctx.begin_frame(egui::RawInput::default());
        let out = ctx.end_frame();
        assert!(
            !out.viewport_output.contains_key(&child),
            "a child the root stops registering must be GC'd (the close)"
        );
    }

    /// (g4) The WM close path (the D-3 close via the window's own
    /// close button): a child frame carrying `ViewportEvent::Close`
    /// sees `close_requested()` and clears the shared open flag —
    /// the next root frame stops re-registering and the window is
    /// GC'd (behaviorally identical to the header button's close,
    /// D-C7).
    #[test]
    fn graphs_viewport_close_requested_clears_the_flag_headless() {
        let state = Arc::new(RwLock::new(TelemetryData::default()));
        let flag = Arc::new(AtomicBool::new(true));
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let child = graphs_viewport_id();

        // Register the child once (the context learns its parent).
        ctx.begin_frame(egui::RawInput::default());
        show_graphs_viewport(&ctx, &state, &flag);
        let _ = ctx.end_frame();

        // The WM close: the child's frame carries `ViewportEvent::
        // Close` (eframe's glow runtime pushes it on the window's
        // CloseRequested event).
        let mut info =
            egui::ViewportInfo { parent: Some(egui::ViewportId::ROOT), ..Default::default() };
        info.events.push(egui::ViewportEvent::Close);
        let mut child_input = egui::RawInput { viewport_id: child, ..Default::default() };
        child_input.viewports.insert(child, info);
        ctx.begin_frame(child_input);
        run_graphs_child_frame(&ctx, &state, &flag);
        let _ = ctx.end_frame();

        assert!(
            !flag.load(Ordering::Relaxed),
            "close_requested must clear the open flag (the D-3 close)"
        );
    }

    /// (g5) The C9-02 transition detector's pure edge logic: the
    /// opening edge saves the current gate and forces it on (even
    /// with auto-refresh off), the closing edge restores the saved
    /// original (a pre-enabled original stays enabled) and consumes
    /// the save, a no-edge frame writes nothing, and a close edge
    /// with no saved value (defensive) writes nothing.
    #[test]
    fn refresh_transition_open_forces_on_and_close_restores() {
        // Opening with auto-refresh OFF (the default): the detector
        // saves the `false` original and forces the gate on.
        let mut saved = None;
        let mut last = false;
        assert_eq!(
            apply_refresh_transition(&mut saved, &mut last, true, false),
            Some(true),
            "the opening edge must force the gate on"
        );
        assert_eq!(saved, Some(false), "the pre-open original must be saved");
        assert!(last, "the edge memory must now read open");

        // A held-open frame is a no-edge: no write.
        assert_eq!(apply_refresh_transition(&mut saved, &mut last, true, true), None);
        assert_eq!(saved, Some(false));

        // Closing (either path — both clear the same flag, D-C7):
        // restore the saved `false` + consume the save.
        assert_eq!(
            apply_refresh_transition(&mut saved, &mut last, false, true),
            Some(false),
            "the closing edge must restore the saved original"
        );
        assert_eq!(saved, None, "the save must be consumed on the close edge");
        assert!(!last, "the edge memory must now read closed");

        // A held-closed frame is a no-edge: no write.
        assert_eq!(apply_refresh_transition(&mut saved, &mut last, false, false), None);
        assert_eq!(saved, None);

        // Opening with the gate already ON: the detector saves the
        // `true` original and re-asserts on.
        assert_eq!(apply_refresh_transition(&mut saved, &mut last, true, true), Some(true));
        assert_eq!(saved, Some(true));

        // Closing: a pre-enabled original stays enabled.
        assert_eq!(
            apply_refresh_transition(&mut saved, &mut last, false, true),
            Some(true),
            "a pre-enabled original must stay enabled after the close"
        );
        assert_eq!(saved, None);

        // Defensive: a close edge with no saved value writes
        // nothing (the flag cannot be open without the app having
        // seen the opening edge, but the detector must stay no-op
        // safe).
        let mut last = true;
        assert_eq!(apply_refresh_transition(&mut saved, &mut last, false, false), None);
        assert!(!last);
    }

    /// (g6) The C9-02 lifecycle through the app itself (the
    /// per-frame detector's wiring, headless — `update` calls
    /// `apply_graphs_lifecycle`): opening the window force-enables
    /// the shared refresh gate even with auto-refresh off, and
    /// closing it (either path — both store the same flag `false`,
    /// D-C7: the g1 button toggle and the g4 WM close) reverts the
    /// gate to the saved original; a pre-enabled original stays
    /// enabled after the close.
    #[test]
    fn graphs_lifecycle_force_enables_and_reverts_the_refresh_gate() {
        let out_dir = temp_out_dir("graphs-lifecycle");
        for original in [false, true] {
            let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
            let mut app = RamSleuthApp {
                state: Arc::new(RwLock::new(TelemetryData {
                    settings: GuiSettings {
                        refresh_enabled: original,
                        ..Default::default()
                    },
                    ..Default::default()
                })),
                bench_tx,
                stop: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                settings_open: false,
                graphs_open: Arc::new(AtomicBool::new(false)),
                saved_refresh: None,
                last_graphs_open: false,
                out_dir: out_dir.clone(),
                poller: None,
                notice: None,
            };
            let gate = |app: &RamSleuthApp| app.state.read().unwrap().settings.refresh_enabled;

            // The opening edge (the header button's click stores
            // `true`, g1): the detector force-enables the gate.
            app.graphs_open.store(true, Ordering::Relaxed);
            app.apply_graphs_lifecycle();
            assert!(gate(&app), "open must force the gate on (original: {original})");
            assert_eq!(
                app.saved_refresh,
                Some(original),
                "the pre-open original must be saved"
            );

            // A held-open frame: no edge, the gate stays on.
            app.apply_graphs_lifecycle();
            assert!(gate(&app), "a held-open frame must not touch the gate");

            // The closing edge (either path stores `false` — the
            // g1 button toggle / the g4 WM close): the detector
            // reverts to the saved original.
            app.graphs_open.store(false, Ordering::Relaxed);
            app.apply_graphs_lifecycle();
            assert_eq!(gate(&app), original, "close must restore the saved original (original: {original})");
            assert_eq!(app.saved_refresh, None, "the save must be consumed on the close edge");

            // A held-closed frame: no edge, the original stands.
            app.apply_graphs_lifecycle();
            assert_eq!(gate(&app), original, "a held-closed frame must not touch the gate");
        }
        fs::remove_dir_all(&out_dir).expect("cleanup");
    }
    /// (u1) The C9-05 right-column split (D-4b): the right column
    /// renders the bench and the status as two stacked slices — the
    /// bench at its natural height over the status, the 8 pt gap
    /// preserved — and degrades to the `ScrollArea` overflow
    /// fallback in a small window (the bench's natural height alone
    /// exceeds the column, the status slice takes its zero budget,
    /// and the content scrolls below). No panic at either size, no
    /// action without a click, one CYAN frame per zone.
    #[test]
    fn render_zones_right_column_split_two_stacked_slices() {
        let out_dir = temp_out_dir("right-split");
        // (w, h): the default 968×600 (the non-scrolling dashboard,
        // C7-19 — both slices fit the column) and a small window
        // (the scroll fallback path — the bench alone overflows).
        for (case, (w, h)) in [
            (DEFAULT_WINDOW_SIZE[0], DEFAULT_WINDOW_SIZE[1]),
            (900.0, 220.0),
        ]
        .into_iter()
        .enumerate()
        {
            let (bench_tx, _bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
            let app = RamSleuthApp {
                state: Arc::new(RwLock::new(TelemetryData::default())),
                bench_tx,
                stop: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                settings_open: false,
                graphs_open: Arc::new(AtomicBool::new(false)),
                saved_refresh: None,
                last_graphs_open: false,
                out_dir: out_dir.clone(),
                poller: None,
                notice: None,
            };
            let data = TelemetryData::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(w, h));
            let ctx = egui::Context::default();
            ctx.begin_frame(egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            });
            let action = app.render_zones(&ctx, &data);
            let out = ctx.end_frame();

            assert_eq!(action, GuiAction::None, "a no-click frame must report no action");

            // The zone frames: the CYAN-stroked rects of frame size
            // (the header / settings strips use `HEADER_STROKE`, not
            // CYAN; glyphs are `Shape::Text`, not `Shape::Rect`).
            let frames: Vec<egui::Rect> = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect)
                        if rect.stroke.color == CYAN && rect.rect.width() >= 100.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            // The right-column frames: those sharing the rightmost
            // column's left edge (the telemetry frame is the lone
            // left one).
            let total = frames.len();
            // The outer margins (D-15.1): the leftmost frame edge's
            // margin from the screen's left edge, and the rightmost
            // frame edge's margin from the screen's right edge.
            let left_x = frames.iter().fold(f32::INFINITY, |mn, r| mn.min(r.min.x));
            let right_x = frames.iter().fold(0.0_f32, |mx, r| mx.max(r.min.x));
            let rightmost_x = frames.iter().fold(0.0_f32, |mx, r| mx.max(r.max.x));
            let mut right: Vec<egui::Rect> =
                frames.into_iter().filter(|r| (r.min.x - right_x).abs() < 2.0).collect();
            if case == 0 {
                // Default window: all three frames are visible —
                // one per zone (the placeholder status draws no SPD
                // cards).
                assert_eq!(
                    right.len(),
                    2,
                    "the right column must hold the bench + status slices (frames: {total})"
                );
                right.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
                let (bench, status) = (right[0], right[1]);
                assert!(bench.max.y < status.min.y, "the bench slice must sit above the status slice");
                let gap = status.min.y - bench.max.y;
                assert!(gap >= 7.9, "the preserved 8 pt gap must separate the slices (got {gap})");
                assert!(
                    gap <= 25.0,
                    "the gap is 8 pt + one item spacing, not a layout blow-up (got {gap})"
                );
                // The bench slice keeps its natural height — far
                // from stretched to the column (filling the
                // remainder is the status slice's job, C9-07).
                let bench_h = bench.height();
                assert!(
                    bench_h < 0.6 * (h - 100.0),
                    "the bench slice must stay at its natural height (got {bench_h} of {h})"
                );
                // Both slices fit the column — the fallback stays
                // dormant (the non-scrolling dashboard, C7-19).
                assert!(
                    status.max.y <= h - 3.9,
                    "the status frame must fit the column (bottom {} of {})",
                    status.max.y,
                    h
                );
                // The symmetric outer margins (D-15.1): the left
                // margin == the right margin (both = OUTER_MARGIN),
                // and the right margin clears the frame stroke's
                // 0.5 pt outer half — the missing right border is
                // gone and stays gone.
                let left_margin = left_x;
                let right_margin = w - rightmost_x;
                assert!(
                    (left_margin - right_margin).abs() <= 1.0,
                    "the left and right outer margins must be symmetric (left {left_margin}, right {right_margin})"
                );
                assert!(
                    (left_margin - OUTER_MARGIN).abs() <= 1.0,
                    "the left margin must be OUTER_MARGIN (got {left_margin})"
                );
                assert!(
                    (right_margin - OUTER_MARGIN).abs() <= 1.0,
                    "the right margin must be OUTER_MARGIN (got {right_margin})"
                );
                assert!(
                    right_margin >= 0.5,
                    "the right margin must clear the frame stroke's 0.5 pt outer half (got {right_margin})"
                );
            } else {
                // Small window: the bench's natural height alone
                // exceeds the column — the scroll fallback is
                // active: the bench overflows the window bottom and
                // the status (stacked below it) is clipped off-
                // screen until scrolled into view.
                assert_eq!(
                    right.len(),
                    1,
                    "the visible right-column slice is the bench (frames: {total})"
                );
                assert!(
                    right[0].max.y > h - 3.9,
                    "the bench must overflow the small column (the scroll fallback), bottom {} of {}",
                    right[0].max.y,
                    h
                );
            }
        }
        fs::remove_dir_all(&out_dir).expect("cleanup");
    }
}
