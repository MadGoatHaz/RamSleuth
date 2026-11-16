//! Probe-report overlay flow (extracted from `main.rs`).
//!
//! The `[F]` key's I/O side effect: [`probe_report_fetch`] (the one-shot
//! `GetProbeReport` RPC + consent overlay open) and the modal overlay's
//! key handler ([`handle_probe_overlay_key`] — the consent keys, the
//! preview keys, the preview scroll). The no-panic contract (D5): every
//! failure class is recorded in `AppState`, never a panic.

use std::path::Path;
use std::sync::{Arc, RwLock};

use crossterm::event::{KeyCode, KeyEvent};
use ramsleuth_client::Client;
use ramsleuth_protocol::{Request, Response};
use ramsleuth_telemetry::probe::render_probe_report_md;
use ramsleuth_tui::probe::{preview_max_scroll, write_probe_report, NoticeTone, ProbeOverlayState};
use ramsleuth_tui::AppState;
use ratatui::layout::Rect;

use super::{state_read, state_write};

// ------------------------------------------------------------------
// chunk probe-4: the probe-report flow — the one-shot
// `GetProbeReport` fetch (the `[F]` key's I/O side effect) + the
// modal overlay key handler (the consent keys, the preview keys,
// the preview scroll).
// ------------------------------------------------------------------

/// The `[F]` probe-report fetch (chunk probe-4): a one-shot
/// `GetProbeReport` over a fresh connection (the poller's
/// one-connection-per-request precedent — the probe report is its
/// own request class, not a telemetry poll): a
/// `Response::ProbeReport` renders to markdown (chunk 2's
/// `render_probe_report_md`) and opens the consent overlay (the
/// submission is consent-gated — the `[y]` answer moves the
/// markdown into the preview, no second fetch). Every failure class
/// (daemon down, a structured daemon `Error`, a contract-violating
/// frame, a transport error) is recorded in `state.error` — the
/// no-panic contract (D5); the dashboard keeps running, the overlay
/// never opens on a failed fetch.
pub fn probe_report_fetch(socket: &Path, state: &Arc<RwLock<AppState>>) {
    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(error) => {
            state_write(state).error = Some(error.to_string());
            return;
        }
    };
    match client.request(&Request::GetProbeReport) {
        Ok(Response::ProbeReport(report)) => {
            let markdown = render_probe_report_md(&report);
            state_write(state).probe = ProbeOverlayState::Consent { markdown };
        }
        Ok(Response::Error(message)) => {
            // The daemon was reachable but rejected the request
            // (a structured wire error, not a transport failure).
            state_write(state).error = Some(message);
        }
        Ok(_) => {
            // Any other frame in reply to `GetProbeReport` violates
            // the wire contract (probe reports reply only to
            // `GetProbeReport`, chunk probe-1a): record it (D5).
            state.write().unwrap().error = Some("unexpected response to GetProbeReport".to_owned());
        }
        Err(error) => {
            state_write(state).error = Some(error.to_string());
        }
    }
}

/// The preview's markdown, when the overlay is in the preview phase
/// (the `[w]`/`[c]` actions' guard — the consent phase has no write
/// / copy): a one-time clone (an intentional action, never a
/// per-keystroke cost).
fn preview_markdown(state: &Arc<RwLock<AppState>>) -> Option<String> {
    let app = state_read(state);
    match &app.probe {
        ProbeOverlayState::Preview { markdown, .. } => Some(markdown.clone()),
        _ => None,
    }
}

/// Set the preview's transient notice line (the `[w]`/`[c]`
/// outcome): the tone colors it (the render maps `Ok` → green,
/// `Warn` → amber, `Error` → crimson); a closed overlay is a
/// no-op (the callers' phase guard makes that unreachable — the
/// fn stays total, D5).
fn set_preview_notice(state: &Arc<RwLock<AppState>>, tone: NoticeTone, text: String) {
    if let ProbeOverlayState::Preview { notice, .. } = &mut state_write(state).probe {
        *notice = Some((tone, text));
    }
}

/// Scroll the preview's markdown by `delta` lines (the
/// `Up`/`Down`/`PageUp`/`PageDown` keys): clamped to the probe
/// module's `preview_max_scroll` over the current terminal `area`
/// (the same layout the render draws) — a short markdown never
/// scrolls, the end is never overshot.
fn scroll_probe_preview(state: &Arc<RwLock<AppState>>, area: Rect, delta: i32) {
    let (current, max) = {
        let app = state_read(state);
        match &app.probe {
            ProbeOverlayState::Preview {
                markdown, scroll, ..
            } => (*scroll, preview_max_scroll(markdown, area)),
            _ => return,
        }
    };
    let next = (i64::from(current) + i64::from(delta)).clamp(0, i64::from(max)) as u16;
    if next != current {
        if let ProbeOverlayState::Preview { scroll, .. } = &mut state_write(state).probe {
            *scroll = next;
        }
    }
}

/// The modal overlay's key handler (chunk probe-4): the consent
/// keys (`[y]` → the preview, `[q]`/`[n]`/`Esc` → close), the
/// preview keys (`[w]` → write `write_path`, `[c]` → the
/// `clipboard` closure, `[q]`/`[n]`/`Esc` → close), and the preview
/// scroll
/// (`Up`/`Down`/`PageUp`/`PageDown` over `area`) — every other key
/// (including `[f]` itself) is inert while the overlay is open.
/// The caller (the main loop) routes a key here only while the
/// overlay is open and bypasses the global key table (a `[q]` in
/// the preview closes it — it does not quit the TUI).
pub fn handle_probe_overlay_key(
    key: KeyEvent,
    state: &Arc<RwLock<AppState>>,
    area: Rect,
    write_path: &Path,
    clipboard: &dyn Fn(&str) -> Result<String, String>,
) {
    match key.code {
        // `[y]` (consent only): move the carried markdown into the
        // preview (the one-time clone; a `[y]` in the preview is a
        // no-op — the answer keys are consumed).
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let mut app = state_write(state);
            if let ProbeOverlayState::Consent { markdown } =
                std::mem::replace(&mut app.probe, ProbeOverlayState::None)
            {
                app.probe = ProbeOverlayState::Preview {
                    markdown,
                    notice: None,
                    scroll: 0,
                };
            }
        }
        // `[q]` / `[n]` / `Esc` (both phases): close the overlay —
        // cancel the consent / quit the preview (the natural
        // `[q]`uit key must never be trapped by the modal; it
        // closes it, it does not quit the TUI).
        KeyCode::Char('q')
        | KeyCode::Char('Q')
        | KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Esc => {
            state_write(state).probe = ProbeOverlayState::None;
        }
        // `[w]` (preview only): write the report to `write_path`
        // (the live loop passes `~/.ramsleuth/probe-report.md`; the
        // tests pass a temp dir) — the outcome is the notice line.
        KeyCode::Char('w') | KeyCode::Char('W') => {
            let Some(markdown) = preview_markdown(state) else {
                return;
            };
            let (tone, text) = match write_probe_report(write_path, &markdown) {
                Ok(path) => (
                    NoticeTone::Ok,
                    format!("Report written to {}", path.display()),
                ),
                Err(error) => (NoticeTone::Error, format!("write failed: {error}")),
            };
            set_preview_notice(state, tone, text);
        }
        // `[c]` (preview only): copy the report to the clipboard —
        // the outcome (a success note naming the utility, or the
        // fixed no-utility warning) is the notice line.
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let Some(markdown) = preview_markdown(state) else {
                return;
            };
            let (tone, text) = match clipboard(&markdown) {
                Ok(note) => (NoticeTone::Ok, note),
                Err(_) => (
                    NoticeTone::Warn,
                    "No clipboard utility found — use [w] to write to file".to_owned(),
                ),
            };
            set_preview_notice(state, tone, text);
        }
        // The preview scroll (the `area` clamp — the render's
        // layout).
        KeyCode::Up => scroll_probe_preview(state, area, -1),
        KeyCode::Down => scroll_probe_preview(state, area, 1),
        KeyCode::PageUp => scroll_probe_preview(state, area, -10),
        KeyCode::PageDown => scroll_probe_preview(state, area, 10),
        // Every other key is inert while the overlay is open.
        _ => {}
    }
}
