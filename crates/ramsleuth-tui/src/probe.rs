//! Chunk probe-4 — the TUI probe-report flow: consent → preview →
//! file / clipboard.
//!
//! The TUI-local surface of the consent-gated "Submit Probe Report"
//! feature (the daemon builder is chunk probe-1b, the markdown
//! renderer chunk probe-2, the issue template chunk probe-5), split
//! the way [`crate::graphs`] is split: the presentation state + the
//! overlay paint live in the library, the I/O the overlay keys
//! perform (the one daemon fetch, the file write, the clipboard
//! spawn) runs in the bin's main loop (the D6 side-effect split).
//!
//! - [`ProbeOverlayState`] — the overlay phase: `None` (closed),
//!   `Consent` (the "Submit probe report? … [y]es / [n]o" prompt —
//!   the already-rendered markdown rides along, so the `[y]` answer
//!   moves it straight into the preview — no second fetch), and
//!   `Preview` (the scrollable markdown + a transient notice line —
//!   the `[w]` write / `[c]` copy outcome — + the key footer).
//! - [`render_probe_overlay`] — the overlay paint: drawn **last** by
//!   [`crate::ui::render`], topmost, over the whole frame (the
//!   graphs-overlay paint-order idiom — the dashboard renders
//!   underneath first, the overlay wins the paint). The consent
//!   prompt is a centered titled block; the preview is a full-frame
//!   titled block with the markdown as a scrolled [`Paragraph`], the
//!   notice line, and the key footer.
//! - [`preview_max_scroll`] — the preview's scroll clamp (the
//!   markdown's line count minus the visible text height — the same
//!   layout the render draws): the main loop clamps the
//!   `Up`/`Down`/`PageUp`/`PageDown` keys with it.
//! - [`write_probe_report`] — the `[w]` action's file write: the
//!   markdown to `path` (the parent dir created as needed) — pure
//!   std I/O over the given path, testable against a temp dir (the
//!   bin's `export_json` precedent; no env access here — the
//!   destination is the caller's).
//! - [`probe_report_default_path`] — the live `[w]` destination:
//!   `~/.ramsleuth/probe-report.md` (`$HOME`; the GUI F2/F3 rule's
//!   CWD fallback when `HOME` is unset — never a panic).
//! - [`copy_to_clipboard`] / [`find_clipboard_utility`] /
//!   [`ClipboardUtil`] — the `[c]` action: the first clipboard
//!   utility present on `PATH` (`wl-copy` — Wayland — first, then
//!   `xclip` / `xsel` — X11; the search order is fixed, independent
//!   of the `PATH` order) with the markdown piped to its stdin. The
//!   utility scan is a pure `PATH`-string function (testable with a
//!   fake `PATH` — no env access, no process spawn); the spawn lives
//!   in `copy_to_clipboard` only.
//!
//! **No-panic contract (D5):** the overlay renders at any terminal
//! size (a zero / too-small area draws nothing), every clipboard /
//! write failure degrades to a notice line, and nothing here reads
//! the terminal (the bin routes the keys).

use std::io::Write;
use std::path::{Path, PathBuf};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Semantic palette (Grand Design §3.2 — zone-local constants, the
// ui.rs / graphs.rs mirror; only the members this surface uses).
// ---------------------------------------------------------------------------

/// Cyan: the borders + the key hints.
const CYAN: Color = Color::Rgb(0x00, 0xD4, 0xFF);
/// Green: a success notice line (the ui.rs `STATUS_DONE` mirror — a
/// dark-slate-compatible green; zone-local, not a palette entry).
const GREEN: Color = Color::Rgb(0x2E, 0x9E, 0x5B);
/// Amber: a warning notice line (no clipboard utility found).
const AMBER: Color = Color::Rgb(0xFF, 0xB3, 0x00);
/// Crimson: a failure notice line (a write / spawn error).
const CRIMSON: Color = Color::Rgb(0xFF, 0x3B, 0x30);
/// Slate: the panel background.
const SLATE: Color = Color::Rgb(0x1E, 0x1E, 0x24);
/// Dim grey: the static body text + the footer.
const DIM: Color = Color::Rgb(0x8A, 0x8A, 0x96);
/// Light grey: the titles + the prompt text.
const LIGHT_GREY: Color = Color::Rgb(0xC0, 0xC0, 0xCC);

// ---------------------------------------------------------------------------
// The overlay state.
// ---------------------------------------------------------------------------

/// The notice line's tone (the `[w]`/`[c]` outcome color): success
/// (green), warning (amber — no clipboard utility), failure (crimson
/// — a write / spawn error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTone {
    /// A success (the green notice).
    Ok,
    /// A warning (the amber notice — no clipboard utility found).
    Warn,
    /// A failure (the crimson notice — a write / spawn error).
    Error,
}

/// The probe-report overlay phase — the modal state the bin's main
/// loop drives (the `[F]` key fetches the report and opens
/// `Consent`; `[y]` moves the carried markdown into `Preview`;
/// `[n]` / `[q]` / `Esc` close it) and [`crate::ui::render`] draws
/// (last, topmost, over the whole frame).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProbeOverlayState {
    /// The overlay is closed (the default — the dashboard renders
    /// normally).
    #[default]
    None,
    /// The consent prompt: "Submit probe report? … [y]es / [n]o".
    /// `markdown` is the already-rendered report (chunk 2's
    /// `render_probe_report_md` over the fetched `ProbeReport`),
    /// carried so the `[y]` answer moves it straight into the
    /// preview (no second fetch).
    Consent { markdown: String },
    /// The markdown preview (after `[y]`): the scrollable report, a
    /// transient notice line (the `[w]` write / `[c]` copy outcome),
    /// and the `[w]`/`[c]`/`[q]` footer.
    Preview {
        markdown: String,
        notice: Option<(NoticeTone, String)>,
        scroll: u16,
    },
}

impl ProbeOverlayState {
    /// The overlay is open (a phase other than [`None`]).
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The carried markdown (both open phases), when present.
    pub fn markdown(&self) -> Option<&str> {
        match self {
            Self::Consent { markdown } | Self::Preview { markdown, .. } => Some(markdown),
            Self::None => None,
        }
    }
}

/// The consent prompt's question (the chunk's frozen form).
pub const CONSENT_QUESTION: &str = "Submit probe report?";
/// The consent prompt's detail line (the no-PII note — the renderer
/// footer's mirror: no username, hostname, IP, MAC, or serial
/// numbers).
pub const CONSENT_DETAIL: &str =
    "This gathers CPU/system/telemetry data (no personal info).";

// ---------------------------------------------------------------------------
// The overlay paint (drawn last + topmost by `ui::render`).
// ---------------------------------------------------------------------------

/// Render the probe-report overlay into `frame` (the
/// [`crate::ui::render`] call — last, topmost, over the whole frame):
/// `None` draws nothing (the dashboard is untouched); `Consent`
/// draws the centered titled box; `Preview` draws the full-frame
/// titled block with the scrolled markdown + the notice line + the
/// key footer. No-panic at any `area` size (a zero / too-small rect
/// draws nothing — the graphs panel rule).
pub fn render_probe_overlay(frame: &mut Frame, overlay: &ProbeOverlayState, area: Rect) {
    let Some(box_area) = overlay_area(area, overlay) else {
        return;
    };
    match overlay {
        ProbeOverlayState::None => {}
        ProbeOverlayState::Consent { .. } => render_consent(frame, box_area),
        ProbeOverlayState::Preview { markdown, notice, scroll } => {
            render_preview(frame, markdown, notice, *scroll, box_area)
        }
    }
}

/// The overlay's rect: the whole `area` for the preview, a centered
/// 72×8 box for the consent prompt (clamped to `area` — a smaller
/// terminal shrinks the box, it never overflows), `None` when the
/// overlay is closed or `area` cannot host it (the no-panic rule).
fn overlay_area(area: Rect, overlay: &ProbeOverlayState) -> Option<Rect> {
    if area.is_empty() || area.width < 3 || area.height < 3 {
        return None;
    }
    match overlay {
        ProbeOverlayState::None => None,
        ProbeOverlayState::Preview { .. } => Some(area),
        ProbeOverlayState::Consent { .. } => {
            let width = area.width.min(72);
            let height = area.height.min(8);
            let x = area.x + area.width.saturating_sub(width) / 2;
            let y = area.y + area.height.saturating_sub(height) / 2;
            Some(Rect::new(x, y, width, height))
        }
    }
}

/// The consent prompt: the centered titled block — the question
/// (bold light-grey), the no-PII detail line (dim), and the
/// `[y]es / [n]o` answer line (the cyan key hints).
fn render_consent(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title(Span::styled(
            " PROBE REPORT — consent ",
            Style::default().fg(LIGHT_GREY),
        ))
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return; // the borders consumed the rect: nothing to draw.
    }
    let mut lines = vec![Line::from("")];
    lines.push(Line::from(Span::styled(
        CONSENT_QUESTION,
        Style::default().fg(LIGHT_GREY).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(CONSENT_DETAIL, Style::default().fg(DIM))));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("[y]", Style::default().fg(CYAN)),
        Span::styled("yes    ", Style::default().fg(LIGHT_GREY)),
        Span::styled("[n]", Style::default().fg(CYAN)),
        Span::styled("no", Style::default().fg(LIGHT_GREY)),
    ]));
    lines.push(Line::from(""));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The markdown preview: the full-frame titled block — the report
/// markdown as a scrolled [`Paragraph`] (the `scroll` offset), the
/// transient notice line (the `[w]`/`[c]` outcome, its tone's
/// color), and the key footer.
fn render_preview(
    frame: &mut Frame,
    markdown: &str,
    notice: &Option<(NoticeTone, String)>,
    scroll: u16,
    area: Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title(Span::styled(
            " PROBE REPORT — preview (↑/↓ scroll) ",
            Style::default().fg(LIGHT_GREY),
        ))
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return; // the borders consumed the rect: nothing to draw.
    }
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),   // the markdown (scrollable)
            Constraint::Length(1), // the notice line (its slot reserved — stable layout)
            Constraint::Length(1), // the key footer
        ])
        .split(inner);
    let text: Vec<Line> = markdown.lines().map(Line::raw).collect();
    frame.render_widget(Paragraph::new(text).scroll((scroll, 0)), parts[0]);
    if let Some((tone, text)) = notice {
        let color = match tone {
            NoticeTone::Ok => GREEN,
            NoticeTone::Warn => AMBER,
            NoticeTone::Error => CRIMSON,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {text}"),
                Style::default().fg(color),
            ))),
            parts[1],
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " [w] write to file · [c] copy to clipboard · [q] quit preview",
            Style::default().fg(DIM),
        ))),
        parts[2],
    );
}

/// The preview's maximum vertical scroll offset: the markdown's line
/// count minus the visible text height (clamped to ≥ 0 — a short
/// report never scrolls). The visible height = the inner height (the
/// borders subtracted) minus the two reserved footer lines (the
/// notice + the key footer) — the same layout [`render_preview`]
/// draws, so the clamp matches the render.
pub fn preview_max_scroll(markdown: &str, area: Rect) -> u16 {
    let inner = area.height.saturating_sub(2);
    let text_height = inner.saturating_sub(2);
    let lines = markdown.lines().count() as u64;
    (lines.saturating_sub(text_height as u64)) as u16
}

// ---------------------------------------------------------------------------
// The `[w]` file write + the `[c]` clipboard.
// ---------------------------------------------------------------------------

/// The `[w]` action's file write: write `markdown` to `path`
/// (creating the parent dir as needed), returning the written path.
/// Pure std I/O over the given path — testable against a temp dir
/// (the bin's `export_json` precedent; no env access here, the
/// destination is the caller's).
pub fn write_probe_report(path: &Path, markdown: &str) -> Result<PathBuf, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, markdown)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(path.to_path_buf())
}

/// The live `[w]` destination: `~/.ramsleuth/probe-report.md`
/// (`$HOME`; the GUI F2/F3 rule's CWD fallback when `HOME` is unset
/// — never a panic).
pub fn probe_report_default_path() -> PathBuf {
    let home =
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".ramsleuth").join("probe-report.md")
}

/// A clipboard utility on the user's `PATH` (the `[c]` action):
/// `wl-copy` (Wayland) first, then `xclip` / `xsel` (X11) — the
/// first present executable wins (the search order is fixed,
/// independent of the `PATH` order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardUtil {
    /// `wl-copy` (Wayland).
    WlCopy,
    /// `xclip` (X11).
    Xclip,
    /// `xsel` (X11).
    Xsel,
}

impl ClipboardUtil {
    /// The utility's binary name.
    fn bin(self) -> &'static str {
        match self {
            ClipboardUtil::WlCopy => "wl-copy",
            ClipboardUtil::Xclip => "xclip",
            ClipboardUtil::Xsel => "xsel",
        }
    }
}

/// Scan a `PATH` string for a clipboard utility (the pure core of
/// [`copy_to_clipboard`] — testable with a fake `PATH`: no env
/// access, no process spawn): the fixed search order `wl-copy` →
/// `xclip` → `xsel`, the first present **executable** file wins.
pub fn find_clipboard_utility(path_env: &str) -> Option<ClipboardUtil> {
    let dirs: Vec<&Path> = path_env
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(Path::new)
        .collect();
    for util in [ClipboardUtil::WlCopy, ClipboardUtil::Xclip, ClipboardUtil::Xsel] {
        for dir in &dirs {
            if is_executable(&dir.join(util.bin())) {
                return Some(util);
            }
        }
    }
    None
}

/// `path` is an existing executable file (the Unix `mode & 0o111`
/// rule); every failure class (missing, not a file, not executable)
/// → `false` (the no-panic contract).
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// The `[c]` action: copy `markdown` to the clipboard via the first
/// utility found on the live `PATH` (the [`find_clipboard_utility`]
/// search order): the text is piped to the utility's stdin (dropped
/// → the EOF that ends the read) and the utility is waited on.
/// Returns the success note (naming the utility); an `Err` (no
/// utility, a spawn failure, a non-zero exit) — the caller degrades
/// it to the preview's notice line (D5: never a panic).
pub fn copy_to_clipboard(markdown: &str) -> Result<String, String> {
    let Some(path_env) = std::env::var_os("PATH") else {
        return Err("no PATH set".to_owned());
    };
    // A non-UTF-8 `PATH` (never on a sane host) degrades to the
    // no-utility class (D5).
    let Some(path_env) = path_env.to_str() else {
        return Err("no clipboard utility found".to_owned());
    };
    let Some(util) = find_clipboard_utility(path_env) else {
        return Err("no clipboard utility found".to_owned());
    };
    let mut child = std::process::Command::new(util.bin())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", util.bin()))?;
    {
        let Some(stdin) = child.stdin.as_mut() else {
            return Err(format!("no stdin pipe for {}", util.bin()));
        };
        stdin
            .write_all(markdown.as_bytes())
            .map_err(|e| format!("failed to write to {}: {e}", util.bin()))?;
    }
    let status = child
        .wait()
        .map_err(|e| format!("failed to wait for {}: {e}", util.bin()))?;
    if !status.success() {
        return Err(format!("{} exited with {status}", util.bin()));
    }
    Ok(format!("Copied to clipboard ({})", util.bin()))
}

// ---------------------------------------------------------------------------
// Tests (headless: the render tests use ratatui's in-memory
// TestBackend — no TTY; the file write against a temp dir; the
// utility scan against a fake PATH; the spawn itself is not
// exercised — it is compile-checked, as `poll_event` is).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Draw the overlay into an in-memory terminal of the given size
    /// and return the buffer as text (one line per row, trailing
    /// spaces trimmed) — the graphs.rs `draw_panel` pattern.
    fn draw_overlay(overlay: &ProbeOverlayState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render_probe_overlay(f, overlay, Rect::new(0, 0, width, height)))
            .expect("render must not panic");
        let buffer = completed.buffer;
        let width = usize::from(buffer.area().width);
        buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A multi-line markdown fixture (the renderer output shape: a
    /// header + a summary + a table).
    fn fixture_markdown() -> String {
        "# RamSleuth Probe Report\n\n> **TUI Test CPU** — Unknown on Linux / Test (kernel 6.6.0-test), RamSleuth v2.4.6\n\n## System\n\n| Field | Value |\n|---|---|\n| CPU | TUI Test CPU |\n"
            .to_owned()
    }

    /// A unique temp dir (pid-qualified — parallel test runs never
    /// collide; removed best-effort on drop, the `TempSocket`
    /// precedent).
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("ramsleuth-tui-probe-{name}-{}", process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("test dir must create");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// (a) The default phase is `None` (closed, no markdown).
    #[test]
    fn default_is_none() {
        let overlay = ProbeOverlayState::default();
        assert_eq!(overlay, ProbeOverlayState::None);
        assert!(!overlay.is_open());
        assert_eq!(overlay.markdown(), None);
    }

    /// (b) Both open phases carry the markdown (`markdown()`
    /// exposes it; the consent phase's is what the `[y]` answer
    /// moves into the preview).
    #[test]
    fn open_phases_carry_markdown() {
        let md = fixture_markdown();
        let consent = ProbeOverlayState::Consent { markdown: md.clone() };
        assert!(consent.is_open());
        assert_eq!(consent.markdown(), Some(md.as_str()));
        let preview = ProbeOverlayState::Preview {
            markdown: md.clone(),
            notice: None,
            scroll: 0,
        };
        assert!(preview.is_open());
        assert_eq!(preview.markdown().unwrap(), md, "the preview carries the markdown");
    }

    /// (c) The consent prompt renders: the title, the question, the
    /// no-PII detail, and the `[y]es / [n]o` answer line.
    #[test]
    fn render_consent_prompt() {
        let overlay = ProbeOverlayState::Consent { markdown: fixture_markdown() };
        let text = draw_overlay(&overlay, 100, 24);
        assert!(text.contains("PROBE REPORT — consent"), "{text}");
        assert!(text.contains("Submit probe report?"), "{text}");
        assert!(text.contains("(no personal info)"), "{text}");
        assert!(text.contains("[y]yes"), "{text}");
        assert!(text.contains("[n]no"), "{text}");
    }

    /// (d) The preview renders the markdown + the footer keys + the
    /// notice line.
    #[test]
    fn render_preview_with_notice() {
        let overlay = ProbeOverlayState::Preview {
            markdown: fixture_markdown(),
            notice: Some((
                NoticeTone::Ok,
                "Report written to ~/.ramsleuth/probe-report.md".to_owned(),
            )),
            scroll: 0,
        };
        let text = draw_overlay(&overlay, 100, 24);
        assert!(text.contains("PROBE REPORT — preview"), "{text}");
        assert!(text.contains("# RamSleuth Probe Report"), "{text}");
        assert!(text.contains("## System"), "{text}");
        assert!(text.contains("Report written to"), "{text}");
        assert!(text.contains("[w] write to file"), "{text}");
        assert!(text.contains("[c] copy to clipboard"), "{text}");
        assert!(text.contains("[q] quit preview"), "{text}");
    }

    /// (e) The preview scrolls: of a 20-line markdown, a 14-row
    /// terminal (the borders + the two footer lines leaving 10
    /// visible) shows `line 5`…`line 14` at `scroll = 5`; `line 0`
    /// and `line 1` are out of view.
    #[test]
    fn render_preview_scrolls() {
        let markdown: String = (0..20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let open = ProbeOverlayState::Preview {
            markdown: markdown.clone(),
            notice: None,
            scroll: 0,
        };
        let text = draw_overlay(&open, 100, 14);
        // 10 lines visible (14 rows − 2 borders − 2 footer lines).
        assert!(text.contains("│line 0"), "scroll 0 shows the top: {text}");
        assert!(text.contains("│line 9"), "scroll 0 shows the 10th line: {text}");
        assert!(!text.contains("│line 10"), "the 11th line is clipped: {text}");
        let scrolled = ProbeOverlayState::Preview { markdown, notice: None, scroll: 5 };
        let text = draw_overlay(&scrolled, 100, 14);
        assert!(text.contains("│line 5"), "{text}");
        assert!(text.contains("│line 14"), "{text}");
        assert!(!text.contains("│line 0"), "line 0 must be scrolled out: {text}");
        assert!(!text.contains("│line 4"), "line 4 must be scrolled out: {text}");
    }

    /// (f) Every phase renders at a zero / tiny area without
    /// panicking (the no-panic contract).
    #[test]
    fn render_all_phases_zero_area_never_panics() {
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let consent = ProbeOverlayState::Consent { markdown: fixture_markdown() };
        let preview = ProbeOverlayState::Preview {
            markdown: fixture_markdown(),
            notice: None,
            scroll: 3,
        };
        terminal
            .draw(|f| {
                render_probe_overlay(f, &ProbeOverlayState::None, Rect::new(0, 0, 0, 0));
                render_probe_overlay(f, &consent, Rect::new(0, 0, 0, 0));
                render_probe_overlay(f, &consent, Rect::new(0, 0, 1, 1));
                render_probe_overlay(f, &consent, Rect::new(0, 0, 2, 2));
                render_probe_overlay(f, &preview, Rect::new(0, 0, 0, 0));
                render_probe_overlay(f, &preview, Rect::new(0, 0, 2, 2));
            })
            .expect("render must not panic");
    }

    /// (g) The scroll clamp: a short markdown never scrolls; a long
    /// one clamps to lines − visible height; a degenerate area
    /// yields the full line count (the render shows what fits).
    #[test]
    fn preview_max_scroll_clamps() {
        let short = "a\nb\nc";
        assert_eq!(preview_max_scroll(short, Rect::new(0, 0, 80, 24)), 0);
        let long: String = (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        // 24 rows: inner 22, text 20 → max 30.
        assert_eq!(preview_max_scroll(&long, Rect::new(0, 0, 80, 24)), 30);
        // A 1-row area: inner 0 → text 0 → max = all 50 lines.
        assert_eq!(preview_max_scroll(&long, Rect::new(0, 0, 80, 1)), 50);
        // An empty markdown → 0.
        assert_eq!(preview_max_scroll("", Rect::new(0, 0, 80, 24)), 0);
    }

    /// (h) The file write: the nested `~/.ramsleuth`-shaped parent is
    /// created as needed, the content lands verbatim, and the
    /// written path is returned.
    #[test]
    fn write_probe_report_creates_dirs_and_writes() {
        let dir = TempDir::new("write");
        let path = dir.path.join(".ramsleuth").join("probe-report.md");
        assert!(!path.exists(), "the nested dir must not exist yet");
        let md = fixture_markdown();
        let written = write_probe_report(&path, &md).expect("write must succeed");
        assert_eq!(written, path);
        let back = std::fs::read_to_string(&path).expect("the file must exist");
        assert_eq!(back, md, "the content must land verbatim");
    }

    /// (i) A write failure (the parent is an existing **file**, not
    /// a dir) is a structured `Err` naming the path, never a panic
    /// (D5), and nothing is written.
    #[test]
    fn write_probe_report_failure_is_structured() {
        let dir = TempDir::new("write-fail");
        let blocker = dir.path.join("blocker");
        std::fs::write(&blocker, "x").expect("blocker must write");
        let path = blocker.join("probe-report.md"); // the parent is a file
        let err = write_probe_report(&path, "md").expect_err("a file parent must fail");
        assert!(err.contains("blocker"), "the error must name the path: {err}");
        assert!(!path.exists(), "nothing may be written");
    }

    /// An executable file `name` in `dir` (mode 0o755).
    fn touch_exec(dir: &Path, name: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o755)
            .open(dir.join(name))
            .expect("file must create");
        file.write_all(b"#!/bin/sh\n").expect("file must write");
    }

    /// A present but non-executable file `name` in `dir` (mode
    /// 0o644).
    fn touch_plain(dir: &Path, name: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o644)
            .open(dir.join(name))
            .expect("file must create");
    }

    /// (j) The search order: `wl-copy` beats `xclip` even when
    /// `xclip` sits earlier on the fake `PATH` (the fixed utility
    /// order, not the `PATH` order).
    #[test]
    fn find_utility_prefers_wl_copy_over_x_clip() {
        let dir = TempDir::new("clip-order");
        let a = dir.path.join("a");
        let b = dir.path.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        touch_exec(&a, "xclip");
        touch_exec(&b, "wl-copy");
        let path_env = format!("{}:{}", a.display(), b.display());
        assert_eq!(
            find_clipboard_utility(&path_env),
            Some(ClipboardUtil::WlCopy),
            "wl-copy wins the fixed order even later on PATH"
        );
    }

    /// (k) `xclip` beats `xsel`; removing them one by one degrades
    /// to `None`; an empty `PATH` finds nothing.
    #[test]
    fn find_utility_falls_back_and_degrades() {
        let dir = TempDir::new("clip-fb");
        let a = dir.path.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let path_env = a.display().to_string();
        touch_exec(&a, "xsel");
        touch_exec(&a, "xclip");
        assert_eq!(find_clipboard_utility(&path_env), Some(ClipboardUtil::Xclip));
        std::fs::remove_file(a.join("xclip")).unwrap();
        assert_eq!(find_clipboard_utility(&path_env), Some(ClipboardUtil::Xsel));
        std::fs::remove_file(a.join("xsel")).unwrap();
        assert_eq!(find_clipboard_utility(&path_env), None);
        assert_eq!(find_clipboard_utility(""), None, "an empty PATH: no utility");
    }

    /// (l) A present but **non-executable** utility is not found
    /// (the `0o111` rule).
    #[test]
    fn find_utility_ignores_non_executable() {
        let dir = TempDir::new("clip-noexec");
        let a = dir.path.join("a");
        std::fs::create_dir_all(&a).unwrap();
        touch_plain(&a, "wl-copy");
        assert_eq!(
            find_clipboard_utility(&a.display().to_string()),
            None,
            "a non-executable file is not a utility"
        );
    }
}
