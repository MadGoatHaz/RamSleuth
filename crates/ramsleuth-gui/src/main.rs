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
//! - **App** — a 1400×900 eframe window carrying the dark-slate
//!   [`build_style`]: the spec's 3-line header (Grand Design §3.1,
//!   C6-20) — line 1 the `RamSleuth v2.0.0` title, the platform tag,
//!   the daemon status, and the `[F2] snapshot · [F3] export · [Q]
//!   quit` legend; line 2 the CPU and platform identity; line 3 the
//!   RAM summary, channel, and sync mode — plus a transient export
//!   notice. Over the three zones: the telemetry matrix on the left,
//!   the benchmark grid stacked over the hardware / SPD status on the
//!   right. Each frame takes one brief read of the shared
//!   `Arc<RwLock<TelemetryData>>` and repaints on a 16 ms cadence
//!   (~60 FPS).
//! - **No render-thread I/O (plan D6):** the background
//!   [`spawn_poller`] thread (P3-26) owns the daemon socket — the 2 s
//!   telemetry cadence and the benchmark stream run there; the render
//!   loop only reads the state. The status zone's [`GuiAction`] is the
//!   one side effect the render loop performs: F2 / F3 run
//!   [`perform_export`] (a one-shot file write) and Q sets the stop
//!   flag + closes the viewport.
//!
//! **No-panic contract (plan D5):** a missing daemon never crashes the
//! GUI — the poller records the friendly error in the state (the
//! header + the status zone show it) and a failed export is a
//! structured [`GuiError`] in the notice line. On window close (Q, the
//! OS close button, or an eframe failure) the app's `Drop` stops the
//! poller and joins it (bounded) so the process always exits cleanly.
//!
//! Manual verification (a live display, the QA phase): with the dev
//! daemon running (`cargo run -p ramsleuth-daemon -- --socket
//! /tmp/ramsleuth.sock`), `cargo run -p ramsleuth-gui -- --socket
//! /tmp/ramsleuth.sock` opens the 1400×900 window with all three zones
//! live (values update every ~2 s); F2 writes
//! `ramsleuth-snapshot-<unix-ts>.png`, F3 writes
//! `ramsleuth-export-<unix-ts>.json` to `$HOME` (the transient notice
//! carries the path); Q (or the window close button) exits cleanly.
//! With no daemon the dashboard stays responsive, showing
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
    build_style, export_json, render_bench_zone, render_status_zone, render_telemetry_zone,
    snapshot_png, spawn_poller, BenchCmd, GuiAction, GuiError, TelemetryData, AMBER, CRIMSON,
    CYAN, SLATE,
};
use ramsleuth_protocol::DEFAULT_SOCKET_PATH;
use ramsleuth_telemetry::amd_readout::{ClockReadout, DivMode};
use ramsleuth_telemetry::cpuid::{AmdZen, CpuVendor};
use ramsleuth_telemetry::error::Section;
use ramsleuth_telemetry::SystemMemoryTelemetry;

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
/// The window's initial inner size (the plan's 1400×900 dashboard).
const WINDOW_SIZE: [f32; 2] = [1400.0, 900.0];
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
/// `ExportJson` writes the current telemetry snapshot to
/// `ramsleuth-export-<unix-ts>.json` (via [`export_json`]) — both into
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
                export_json(telemetry, &path)?;
                Ok(Some(path))
            }
            None => Ok(None),
        },
        GuiAction::Quit | GuiAction::None => Ok(None),
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
/// channel, the stop / cancel flags, the export dir, the poller
/// thread, and the transient header notice.
struct RamSleuthApp {
    /// The shared presentation state — the background poller is its
    /// only writer; the render loop only ever takes a brief read (D6).
    state: Arc<RwLock<TelemetryData>>,
    /// The poller's benchmark channel (the bench zone's run buttons
    /// send into it; the poller owns the socket).
    bench_tx: Sender<BenchCmd>,
    /// The shared stop flag (the poller exits when it is set).
    stop: Arc<AtomicBool>,
    /// The shared benchmark cancel flag (the bench zone's Cancel button
    /// sets it; the poller's run checks it between frames).
    cancel: Arc<AtomicBool>,
    /// The daemon socket the poller connects to (kept for the app's
    /// context; the render loop never touches it — D6).
    socket: PathBuf,
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
        let action = {
            let data = self.state.read().unwrap();
            self.render_header(ctx, &data);
            self.render_zones(ctx, &data)
        };
        self.handle_action(ctx, action);

        // ~60 FPS: eframe's vsync drives the present; this only asks
        // for the next frame on the 16 ms cadence.
        ctx.request_repaint_after(REPAINT);
    }
}

// ---------------------------------------------------------------------
// The spec's 3-line header (Grand Design §3.1, C6-20): the pure line
// builders (unit-tested without an egui context — no display needed)
// that `render_header` composes into the top panel.
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

/// A whole-number `f64` with no decimals (`1800.0` → `1800`), one
/// decimal otherwise (`4.5` → `4.5`) — the unit-agnostic number
/// formatter the header's GB / MHz segments share.
fn trim_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0}", value)
    } else {
        format!("{:.1}", value)
    }
}

/// Line 2 (the mockup's `CPU: AMD Ryzen 9 7950X 16-Core @ 5.70 GHz |
/// Motherboard: … (BIOS: …, AGESA …)`): the CPUID brand + the
/// platform clock (MHz → GHz, two decimals), then the DMI
/// motherboard / BIOS / AGESA cells — every `Na` cell degrades to
/// its `N/A` text (never a panic).
fn cpu_line_text(t: &SystemMemoryTelemetry) -> String {
    let platform = &t.platform;
    let clock = match platform.cpu_clock_mhz.value() {
        Some(mhz) => format!("{:.2} GHz", mhz / 1000.0),
        None => "N/A".to_owned(),
    };
    format!(
        "CPU: {} @ {} | Motherboard: {} (BIOS: {}, AGESA {})",
        t.cpu.brand,
        clock,
        cell_text(&platform.motherboard),
        cell_text(&platform.bios),
        cell_text(&platform.agesa),
    )
}

/// The per-DIMM capacity summary (the mockup's `2x32GB`): one
/// `<count>x<size>GB` group per distinct carried size (first-seen
/// order, ` + `-joined); the `Na` entries contribute nothing, and an
/// all-`Na` / empty list degrades to `N/A`.
fn dimm_summary(sizes: &[Section<f64>]) -> String {
    let mut groups: Vec<(f64, usize)> = Vec::new();
    for cell in sizes {
        if let Some(gib) = cell.value() {
            match groups.iter_mut().find(|(value, _)| (value - gib).abs() < 0.05) {
                Some(group) => group.1 += 1,
                None => groups.push((*gib, 1)),
            }
        }
    }
    if groups.is_empty() {
        "N/A".to_owned()
    } else {
        groups
            .iter()
            .map(|(gib, count)| format!("{count}x{size}GB", size = trim_number(*gib)))
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

/// Line 3's non-mode part (the mockup's `RAM: 64.0 GB (2x32GB)
/// DDR5-6000 MT/s | Dual-Channel | Mode: `): the total capacity
/// (GiB → one-decimal GB display), the per-DIMM summary, the max SPD
/// speed (omitted entirely when no module carries one), the channel
/// mode, and the `Mode: ` lead-in the mode segment completes.
fn ram_line_prefix(t: &SystemMemoryTelemetry) -> String {
    let total = match t.total_capacity.value() {
        Some(gib) => format!("{gib:.1} GB"),
        None => "N/A".to_owned(),
    };
    let mut line = format!("RAM: {total} ({})", dimm_summary(&t.dimm_sizes));
    if let Some(mts) = t.spd.iter().filter_map(|m| m.speed_mts.value().copied()).max() {
        line.push_str(&format!(" {mts} MT/s"));
    }
    line.push_str(&format!(" | {} | Mode: ", channel_mode(t.dimm_sizes.len())));
    line
}

/// Line 3's mode segment (D-C8): the UCLK:MCLK ratio from the AMD
/// clock readout + its semantic color — AMBER for `Synchronous 1:1`
/// (with the MCLK when it carries one), CRIMSON for `Asynchronous
/// 1:2`, and `None` (the default text color) for the honest `N/A`.
fn sync_mode_from_clocks(clocks: &ClockReadout) -> (String, Option<egui::Color32>) {
    match clocks.div_mode.value() {
        Some(DivMode::OneToOne) => {
            let text = match clocks.mclk_mhz.value() {
                Some(mhz) => {
                    format!("Synchronous 1:1 (UCLK = MCLK = {} MHz)", trim_number(*mhz))
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
fn sync_mode(t: &SystemMemoryTelemetry) -> (String, Option<egui::Color32>) {
    match t.amd.value() {
        Some(readout) => sync_mode_from_clocks(&readout.clocks),
        None => ("N/A".to_owned(), None),
    }
}

impl RamSleuthApp {
    /// The header strip (Grand Design §3.1): the spec's 3-line header
    /// — line 1 the `RamSleuth v2.0.0` title + the platform tag + the
    /// daemon status + the `[F2] snapshot · [F3] export · [Q] quit`
    /// legend, line 2 the CPU + platform identity, line 3 the RAM
    /// summary + channel + sync mode — plus the transient notice line
    /// (the last F2 / F3 result) while one is showing. No telemetry
    /// yet (never polled) → the placeholder lines (a missing daemon
    /// never crashes the GUI, plan D5).
    fn render_header(&self, ctx: &egui::Context, data: &TelemetryData) {
        // The line builders over the current snapshot — or the
        // placeholders when no poll has landed yet (never a panic).
        let (cpu_line, ram_prefix, ram_mode, ram_mode_color) = match &data.telemetry {
            Some(t) => {
                let (mode, color) = sync_mode(t);
                (cpu_line_text(t), ram_line_prefix(t), mode, color)
            }
            None => ("CPU: —".to_owned(), "RAM: —".to_owned(), String::new(), None),
        };
        let tag = data
            .telemetry
            .as_ref()
            .map(|t| platform_tag(&t.cpu.vendor))
            .unwrap_or_else(|| "Platform".to_owned());
        let status = daemon_status_text(data, &self.socket);
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
                if let Some((text, _)) = &self.notice {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(text).color(notice_color(text)));
                }
            });
    }

    /// The three zones: the telemetry matrix (zone 1) on the left; the
    /// benchmark grid + controls (zone 2) stacked over the hardware /
    /// SPD status (zone 3) on the right — all visible, non-scrolling
    /// at the 1400×900 size (the right column degrades to a scroll
    /// area in a small window). Returns the [`GuiAction`] the status
    /// zone reported this frame (`None` when no button was clicked).
    fn render_zones(&self, ctx: &egui::Context, data: &TelemetryData) -> GuiAction {
        let mut action = GuiAction::None;
        let bench_tx = &self.bench_tx;
        let cancel = &self.cancel;
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(SLATE))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let avail = ui.available_size();
                    // A pathological window can be narrower than both
                    // minima — the right column collapses (never a
                    // negative allocation, never a panic).
                    let max_left = (avail.x - MIN_RIGHT_W - COLUMN_GAP).max(MIN_LEFT_W);
                    let left_w = (avail.x * 0.55).clamp(MIN_LEFT_W, max_left);
                    let right_w = (avail.x - left_w - COLUMN_GAP).max(0.0);
                    // Left: zone 1 (the live timing matrix, its own
                    // bounded scroll area).
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(left_w, avail.y),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| render_telemetry_zone(ui, data),
                    );
                    ui.add_space(COLUMN_GAP);
                    // Right: zone 2 stacked over zone 3.
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(right_w, avail.y),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            let _ = egui::ScrollArea::vertical().show(ui, |ui| {
                                render_bench_zone(ui, data, bench_tx, cancel);
                                ui.add_space(8.0);
                                action = render_status_zone(ui, data);
                            });
                        },
                    );
                });
            });
        action
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

    // 2. The shared state + the two flags (the poller is the state's
    //    only writer; the bench zone + the poller share the flags).
    let state = Arc::new(RwLock::new(TelemetryData::default()));
    let (bench_tx, bench_rx) = std::sync::mpsc::channel::<BenchCmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    // Exports land in $HOME (fall back to the CWD if it is unset).
    let out_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // 3. The background poller: the GUI's only daemon connection (D6)
    //    — the 2 s telemetry cadence + the benchmark stream.
    let poller = spawn_poller(
        args.socket.clone(),
        state.clone(),
        bench_rx,
        stop.clone(),
        cancel.clone(),
    );

    // 4. The eframe window: 1400×900 initial, the dark-slate style set
    //    once at creation (eframe 0.27 `AppCreator`: a plain
    //    `Box<dyn App>`, no `Result` wrapper). `main_stop` keeps a
    //    handle in main for the post-shutdown stop (the app takes the
    //    other clone).
    let main_stop = stop.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size(WINDOW_SIZE),
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
                socket: args.socket,
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
    use ramsleuth_gui::BenchState;
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
    /// JSON object.
    #[test]
    fn perform_export_json_writes_and_parses() {
        let dir = temp_out_dir("export");
        let data = TelemetryData {
            telemetry: Some(ramsleuth_telemetry::collect()),
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

    /// (h3) `cpu_line_text`: the spec's line 2 — brand + the
    /// MHz→GHz clock + motherboard / BIOS / AGESA, the Na AGESA cell
    /// degrading to its `N/A` text; a fully Na platform degrades
    /// every segment.
    #[test]
    fn cpu_line_text_populated_and_na_degraded() {
        let t = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            Vec::new(),
        );
        assert_eq!(
            cpu_line_text(&t),
            "CPU: Ryzen 9 5950X @ 3.60 GHz | Motherboard: ProArt X570-CREATOR (BIOS: F60 + 09/15/2024, AGESA N/A)"
        );

        let all_na = SystemMemoryTelemetry {
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
            },
            ..t
        };
        assert_eq!(
            cpu_line_text(&all_na),
            "CPU: Ryzen 9 5950X @ N/A | Motherboard: N/A (BIOS: N/A, AGESA N/A)"
        );
    }

    /// (h4) `dimm_summary`: distinct-size grouping — 2×16 →
    /// `2x16GB`, a mixed kit → `1x16GB + 1x32GB` (the Na entry
    /// contributes nothing), all-Na / empty → `N/A`, a non-whole
    /// size keeps one decimal.
    #[test]
    fn dimm_summary_groups_and_degrades() {
        assert_eq!(
            dimm_summary(&[Section::Value(16.0), Section::Value(16.0)]),
            "2x16GB"
        );
        assert_eq!(
            dimm_summary(&[
                Section::Value(16.0),
                Section::na(NaReason::NotApplicable),
                Section::Value(32.0),
            ]),
            "1x16GB + 1x32GB"
        );
        assert_eq!(dimm_summary(&[Section::na(NaReason::NotApplicable)]), "N/A");
        assert_eq!(dimm_summary(&[]), "N/A");
        assert_eq!(dimm_summary(&[Section::Value(4.5)]), "1x4.5GB");
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
    /// the total (one-decimal GB), the per-DIMM summary, the max SPD
    /// speed (omitted when no module carries one), and the channel
    /// mode; the degraded tail renders the honest N/A segments.
    #[test]
    fn ram_line_prefix_populated_and_degraded() {
        let t = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![fixture_spd_module(Some(3200))],
        );
        assert_eq!(
            ram_line_prefix(&t),
            "RAM: 32.0 GB (2x16GB) 3200 MT/s | Dual-Channel | Mode: "
        );

        let no_speed = fixture_telemetry(
            Section::Value(32.0),
            vec![Section::Value(16.0), Section::Value(16.0)],
            vec![fixture_spd_module(None)],
        );
        assert_eq!(
            ram_line_prefix(&no_speed),
            "RAM: 32.0 GB (2x16GB) | Dual-Channel | Mode: "
        );

        // A single Na DIMM still counts as one bound module (the
        // channel is the count, not the sizes): Single-Channel.
        let degraded = fixture_telemetry(
            Section::na(NaReason::NotApplicable),
            vec![Section::na(NaReason::NotApplicable)],
            Vec::new(),
        );
        assert_eq!(
            ram_line_prefix(&degraded),
            "RAM: N/A (N/A) | Single-Channel | Mode: "
        );

        // No bound modules at all: every segment degrades to N/A.
        let empty = fixture_telemetry(
            Section::na(NaReason::NotApplicable),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(ram_line_prefix(&empty), "RAM: N/A (N/A) | N/A | Mode: ");
    }

    /// (h7) `sync_mode_from_clocks`: D-C8's ratio segment — 1:1 with
    /// an MCLK (AMBER), 1:1 without (AMBER, the bare text), 1:2
    /// (CRIMSON), and a Na ratio (the honest N/A, default color).
    #[test]
    fn sync_mode_from_clocks_arms() {
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToOne), Some(1800.0))),
            ("Synchronous 1:1 (UCLK = MCLK = 1800 MHz)".to_owned(), Some(AMBER))
        );
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToOne), None)),
            ("Synchronous 1:1".to_owned(), Some(AMBER))
        );
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(Some(DivMode::OneToTwo), Some(1800.0))),
            ("Asynchronous 1:2".to_owned(), Some(CRIMSON))
        );
        assert_eq!(
            sync_mode_from_clocks(&fixture_clocks(None, Some(1800.0))),
            ("N/A".to_owned(), None)
        );
    }

    /// (h8) `sync_mode`: an Na AMD branch (Intel silicon / the
    /// driver missing) degrades the whole segment to an honest `N/A`
    /// — never a panic.
    #[test]
    fn sync_mode_degrades_on_na_amd_branch() {
        let t = fixture_telemetry(Section::Value(32.0), Vec::new(), Vec::new());
        assert_eq!(sync_mode(&t), ("N/A".to_owned(), None));
    }
}
