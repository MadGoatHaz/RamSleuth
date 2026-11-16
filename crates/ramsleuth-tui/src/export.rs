//! Snapshot + export actions (extracted from `main.rs`).
//!
//! The `[S]`napshot (a timestamped text dashboard to the CWD) and the
//! `[E]`xport (a telemetry + bench JSON to `$HOME`) side effects, plus
//! their pure helpers. The no-panic contract (D5): a write failure is
//! recorded in `AppState`, never a panic.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use ramsleuth_bench::BenchmarkGrid;
use ramsleuth_telemetry::SystemMemoryTelemetry;
use ramsleuth_tui::AppState;

use super::{state_read, state_write};

/// The snapshot file path: `ramsleuth-tui-<unix-ts>.txt` in the current
/// directory (plan P3-24: the timestamped text snapshot in the CWD; the
/// file name follows the `ramsleuth-tui-<unix-ts>` brief template). A
/// pathological pre-epoch clock or a missing CWD degrades to timestamp
/// `0` / `.` — never a panic.
fn snapshot_path() -> PathBuf {
    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    dir.join(format!("ramsleuth-tui-{unix_ts}.txt"))
}

/// The `[S]`napshot action: write the current dashboard as text to
/// [`snapshot_path`] and record the written path in `daemon_status`
/// (transient — the next poll overwrites it with the connection state;
/// `AppState` has no spare field and this chunk is scoped to `main.rs`).
///
/// The content is the client's pure `dump::render` (P3-19) of the current
/// telemetry — the same dashboard text the CLI `dump` prints — or a small
/// placeholder when no snapshot has arrived yet. A write failure is
/// recorded in `state.error`; the TUI never dies on a snapshot.
pub fn write_snapshot(state: &Arc<RwLock<AppState>>) {
    let text = {
        let snapshot = state_read(state);
        match &snapshot.telemetry {
            Some(telemetry) => ramsleuth_client::render(telemetry),
            None => {
                let status = if snapshot.daemon_status.is_empty() {
                    "not connected".to_owned()
                } else {
                    snapshot.daemon_status.clone()
                };
                format!(
                    "RamSleuth TUI snapshot\n\ndaemon: {status}\n\nno telemetry yet (daemon down?)\n"
                )
            }
        }
    };
    let path = snapshot_path();
    match std::fs::write(&path, text) {
        Ok(()) => {
            state_write(state).daemon_status = format!("snapshot: {}", path.display());
        }
        Err(e) => {
            state_write(state).error = Some(format!("snapshot failed: {e}"));
        }
    }
}

/// The F3 wire snapshot (TUI-21 — the GUI `style.rs` C6-29 mirror):
/// the frozen [`SystemMemoryTelemetry`] root flattened into the top
/// level (its seven wire keys, unchanged — no type duplication, D2)
/// plus the `bench` key.
#[derive(serde::Serialize)]
struct ExportSnapshot<'a> {
    /// The telemetry snapshot, flattened into the top level of the
    /// file (the seven wire keys: `cpu` / `amd` / `intel` / `spd` /
    /// `platform` / `total_capacity` / `dimm_sizes`).
    #[serde(flatten)]
    telemetry: &'a SystemMemoryTelemetry,
    /// The terminal 4×4 benchmark grid (the wire unmeasured `N/A`
    /// cells are the honest `0.0`); `null` before the first completed
    /// run.
    bench: Option<&'a BenchmarkGrid>,
}

/// The wall-clock unix timestamp (seconds) for the export file name
/// (the GUI F2/F3 `unix_timestamp` mirror). A pre-epoch clock
/// (impossible on Linux) degrades to 0 — never a panic.
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The `$HOME` → CWD-fallback rule in its pure form (the GUI rule,
/// testable without env access): a set `HOME` is the export
/// destination; an unset `HOME` degrades to the CWD (`.`) — never a
/// panic.
pub fn out_dir_from_home(home: Option<&OsStr>) -> PathBuf {
    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The export destination dir (the GUI F2/F3 rule): `$HOME`, falling
/// back to the CWD when `HOME` is unset — the one env read the
/// `[E]`xport handler makes (the GUI F3 side-effect precedent).
fn export_out_dir() -> PathBuf {
    out_dir_from_home(std::env::var_os("HOME").as_deref())
}

/// F3: write one telemetry + benchmark snapshot to `path` as pretty
/// JSON (the GUI `style.rs::export_json` mirror, TUI-local — there is
/// no `GuiError` in the TUI crate: a JSON failure and a file write
/// map to a `String` the caller records in `state.error`).
///
/// The wire root ([`SystemMemoryTelemetry`], serde-derived since
/// P3-06) serializes under its own seven wire keys, and the terminal
/// [`BenchmarkGrid`] rides alongside it under the `bench` key (C6-29,
/// item 8b): the grid object when a run has completed, the JSON
/// `null` before the first run. Both absent-grid shapes — no run yet
/// (`None`) and the all-`0.0`/`N/A` grid — export as-is, never a
/// panic (D5).
pub fn export_json(
    telemetry: &SystemMemoryTelemetry,
    bench: Option<&BenchmarkGrid>,
    path: &Path,
) -> Result<(), String> {
    let snapshot = ExportSnapshot { telemetry, bench };
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("export JSON serialization failed: {err}"))?;
    std::fs::write(path, json).map_err(|err| format!("export file write failed: {err}"))?;
    Ok(())
}

/// The `[E]`xport action (TUI-21 — the GUI `main.rs` `perform_export`
/// `ExportJson` arm, TUI-local): write the current telemetry
/// snapshot + the terminal bench grid (the `bench` key — the JSON
/// `null` before the first completed run) to
/// `ramsleuth-export-<unix-ts>.json` in `out_dir`, returning the
/// written path. No telemetry yet → `Ok(None)` (no file — the GUI
/// rule). The only I/O is the one file write (no socket, no daemon,
/// no window) — testable against a temp `out_dir`.
pub fn perform_export(state: &AppState, out_dir: &Path) -> Result<Option<PathBuf>, String> {
    match &state.telemetry {
        Some(telemetry) => {
            let ts = unix_timestamp();
            let path = out_dir.join(format!("ramsleuth-export-{ts}.json"));
            export_json(telemetry, state.bench.grid.as_ref(), &path)?;
            Ok(Some(path))
        }
        None => Ok(None),
    }
}

/// Record one `[E]`xport outcome in the state (the GUI transient
/// notice, the `[S]`napshot mechanism): the written path lands in
/// `daemon_status` (transient — the next poll overwrites it, as the
/// snapshot path does), the no-telemetry hint lands there too, and a
/// write failure (a missing dir, a read-only `$HOME`, …) lands in
/// `error` — the TUI never dies on an export (the no-panic contract,
/// D5).
pub fn record_export_result(state: &mut AppState, result: Result<Option<PathBuf>, String>) {
    match result {
        Ok(Some(path)) => {
            state.daemon_status = format!("export: {}", path.display());
        }
        Ok(None) => {
            state.daemon_status = "nothing to export yet (no telemetry)".to_owned();
        }
        Err(message) => {
            state.error = Some(format!("export failed: {message}"));
        }
    }
}

/// The `[E]`xport key handler (TUI-21 — the GUI F3 side-effect
/// precedent): the one file write of the key, on the main thread —
/// the brief read scope takes the snapshot data (the file write runs
/// with no lock held, C14-03), into the `$HOME`/CWD-fallback out dir
/// (the GUI rule), and the outcome is recorded via
/// [`record_export_result`].
pub fn write_export(state: &Arc<RwLock<AppState>>) {
    let result = {
        let snapshot = state_read(state);
        perform_export(&snapshot, &export_out_dir())
    };
    record_export_result(&mut state_write(state), result);
}
