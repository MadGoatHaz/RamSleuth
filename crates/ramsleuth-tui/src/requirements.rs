//! TUI-08 — the SETUP requirements strip: what is degraded, why, and the
//! exact command to fix it (plan §2.3; the GUI `first_run::diagnose`
//! mirror, display-only).
//!
//! [`diagnose`] reads the same degradation signals the three-zone
//! dashboard already uses (the no-panic contract, plan D5) and turns
//! them into a short list of actionable [`Requirement`]s — the GUI's
//! four cases, TUI-`AppState` flavored:
//!
//! 1. **daemon down** — `daemon_status` is not `connected*` (never
//!    polled, or the last poll failed the transport) →
//!    `sudo systemctl enable --now ramsleuth` (the detail carries the
//!    status diagnostic + the `systemctl status ramsleuth` check);
//! 2. **permission error** — the last poll / run recorded a
//!    permission error (the daemon socket is group-gated — a
//!    groupless client; std's `Permission denied` matches
//!    case-insensitively) → `sudo usermod -aG ramsleuth $USER` (a
//!    re-login is required after joining);
//! 3. **AMD driver missing** — the daemon is connected, the AMD branch
//!    is `Na(DriverMissing)`, and the host's vendor is AMD (the
//!    snapshot's `cpu.vendor`, with [`CpuInfo::detect()`] as the
//!    unprivileged fallback that needs no daemon) →
//!    [`DKMS_INSTALL_CMD`] (the detail names the pinned upstream,
//!    [`RYZEN_SMU_PIN_SHORT`] — the D-18.6 transparency line);
//! 4. **Intel driver missing** — the daemon is connected, the Intel
//!    branch is `Na(DriverMissing)` (the `ramsleuth_intel` module is
//!    absent and the `/dev/mem` fallback is blocked), and the host's
//!    vendor is Intel (the same pure-CPUID check — the mirror of case
//!    3) → [`DKMS_INSTALL_CMD_INTEL`] (the helper builds the bundled
//!    `ramsleuth_intel` DKMS module).
//!
//! [`render_requirements_strip`] paints that list as a titled
//! `SETUP — requirements` `Block` on the amber warning border: one
//! amber summary line, a dim indented detail, and `$ <command>` per
//! requirement, plus the dim no-panic footer. Presence is driven by
//! the caller (TUI-16: shown while `diagnose` is non-empty and the
//! `[d]` toggle is open — the strip vanishes on its own once every
//! requirement is resolved).
//!
//! The same `[d]` screen carries the always-present
//! [`render_about_block`] companion: the `CONTROLS — RamSleuth` block —
//! the full 17-key contract as a responsive `key — action` grid (the
//! DATA / BENCH / VIEWS groups; the width-dependent
//! [`about_block_height`]). The app description is the docs' job
//! (`Docs/User_Guide.md`), so the screen shows only the controls. It is
//! drawn while the `[d]` toggle is open, independent of `diagnose`'s
//! presence (informational, not a warning — the cyan zone-border
//! style).
//!
//! **Display-only (plan §5.2):** no pkexec, no wizard, no in-app
//! execution — the TUI user is already in a terminal, so the strip
//! just shows the exact command(s) to run. Group membership activates
//! on re-login / re-exec; the strip's own detail line says so, and the
//! user simply re-runs the TUI once it is resolved (no modal —
//! plan §5.3).
//!
//! **No-panic contract (plan D5):** [`diagnose`] is pure and total —
//! a daemon-less [`AppState::default()`] yields the single daemon
//! requirement, every input degrades, and neither `diagnose` nor
//! [`render_requirements_strip`] (nor [`render_about_block`]) panics
//! (no `unwrap` / `expect` / `panic!` in the production paths).
//!
//! **Standalone:** this file is declared in `lib.rs` by TUI-23 (with
//! the root re-exports); until then it type-checks against the
//! current `ui::AppState` shape (the `telemetry` / `daemon_status` /
//! `error` fields all exist at the TUI-parity base) and re-declares
//! the Grand Design §3.2 palette locally (ui.rs's consts are
//! module-private).

use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::SystemMemoryTelemetry;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::AppState;

// The Grand Design §3.2 palette (ui.rs's private consts re-declared —
// this file stays standalone until TUI-23's lib wiring).
/// Amber: warnings — the strip's border + summary lines.
const AMBER: Color = Color::Rgb(0xFF, 0xB3, 0x00);
/// Cyan: the commands to run (the "do this" affordance) + the About
/// block's border.
const CYAN: Color = Color::Rgb(0x00, 0xD4, 0xFF);
/// Slate: the strip background.
const SLATE: Color = Color::Rgb(0x1E, 0x1E, 0x24);
/// Dim grey: the detail lines + the footer.
const DIM: Color = Color::Rgb(0x8A, 0x8A, 0x96);
/// Light grey: the controls block's title + group headings (the
/// zone-block style mirror).
const LIGHT_GREY: Color = Color::Rgb(0xC0, 0xC0, 0xCC);

/// The pinned `ryzen_smu` upstream, short sha (D-18.6):
/// `amkillam/ryzen_smu @ d2983668300dd2a598e5a7dc40e71ce0678cc270`
/// (verified 2026-08-15 — the current `main` HEAD). This short form
/// is the in-app transparency line (the AMD requirement's detail),
/// the GUI `first_run::RYZEN_SMU_PIN_SHORT` mirror.
pub const RYZEN_SMU_PIN_SHORT: &str = "d298366";

/// The manual fallback command for the AMD driver (the DKMS
/// requirement's command — the GUI `first_run::DKMS_INSTALL_CMD`
/// mirror; the TUI user runs it in their own terminal, no pkexec).
pub const DKMS_INSTALL_CMD: &str = "sudo ramsleuth-install-ryzen-smu-dkms";

/// The manual fallback command for the Intel driver (the command for
/// the Intel DKMS requirement — the GUI
/// `first_run::DKMS_INSTALL_CMD_INTEL` mirror; the TUI user runs it in
/// their own terminal, no pkexec).
pub const DKMS_INSTALL_CMD_INTEL: &str = "sudo ramsleuth-install-intel-dkms";

/// One actionable requirement: the one-line summary (the amber row
/// text), the dim detail, and the exact command to run (rendered as
/// `$ <command>` — the "run it" path is the user's own terminal,
/// plan §5.2; `None` = informational only, no command).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The one-line summary, e.g. "Start the ramsleuth daemon".
    pub summary: String,
    /// The dim detail line: the diagnostic + the resolution note.
    pub detail: String,
    /// The exact command to run, or `None` when informational only.
    pub command: Option<String>,
}

/// Case 1 — the daemon is down: start it (the detail carries the
/// status diagnostic + the `systemctl status ramsleuth` check).
fn daemon_down_requirement(status: &str) -> Requirement {
    Requirement {
        summary: "Start the ramsleuth daemon".to_owned(),
        detail: format!("daemon: {status} — check with `systemctl status ramsleuth`"),
        command: Some("sudo systemctl enable --now ramsleuth".to_owned()),
    }
}

/// Case 2 — a groupless client: join the `ramsleuth` group (a
/// re-login is required after joining; plan §5.3 — the strip says so,
/// the user re-runs the TUI once it is resolved, no modal).
fn group_requirement() -> Requirement {
    Requirement {
        summary: "Join the `ramsleuth` group".to_owned(),
        detail: "the daemon socket is group-gated — a re-login is required after joining"
            .to_owned(),
        command: Some("sudo usermod -aG ramsleuth $USER".to_owned()),
    }
}

/// Case 3 — the AMD `ryzen_smu` driver is missing: install the pinned
/// DKMS module (the detail names the pinned upstream, D-18.6).
fn dkms_requirement() -> Requirement {
    Requirement {
        summary: "Install the `ryzen_smu` kernel module (live AMD subtimings)".to_owned(),
        detail: format!(
            "the helper builds the pinned upstream `amkillam/ryzen_smu @ {RYZEN_SMU_PIN_SHORT}` \
              — shown + checksummed + confirmed before any build; RamSleuth runs without it"
        ),
        command: Some(DKMS_INSTALL_CMD.to_owned()),
    }
}

/// Case 4 — the Intel `ramsleuth_intel` driver is missing: install the
/// DKMS module (mirror of the AMD [`dkms_requirement`]; the helper
/// builds the bundled `ramsleuth_intel` source).
fn intel_dkms_requirement() -> Requirement {
    Requirement {
        summary: "Install the `ramsleuth_intel` kernel module (live Intel subtimings)"
            .to_owned(),
        detail: "the helper builds the bundled `ramsleuth_intel` DKMS module — shown + \
                 confirmed before any build; RamSleuth runs without it"
            .to_owned(),
        command: Some(DKMS_INSTALL_CMD_INTEL.to_owned()),
    }
}

/// The host's CPU vendor for the AMD/Intel-branch check (the GUI
/// `first_run::host_vendor` mirror): the snapshot's `cpu.vendor` is
/// the daemon's own CPUID detection (the same host) and is
/// authoritative when it names a vendor; the unprivileged
/// [`CpuInfo::detect()`] is the fallback when the snapshot carries
/// `Unknown` (no daemon needed).
fn host_vendor(telemetry: &SystemMemoryTelemetry) -> CpuVendor {
    match telemetry.cpu.vendor {
        CpuVendor::Amd(_) | CpuVendor::Intel(_) => telemetry.cpu.vendor,
        CpuVendor::Unknown => CpuInfo::detect().vendor,
    }
}

/// Diagnose the current TUI state into the actionable requirements
/// (the GUI `first_run::diagnose` mirror, the four cases in the
/// module doc). Pure + total (the no-panic contract, D5): a
/// daemon-less [`AppState::default()`] yields the single daemon
/// requirement; every input degrades, nothing panics.
pub fn diagnose(state: &AppState) -> Vec<Requirement> {
    let mut requirements = Vec::new();

    // Case 1: the daemon is down (never polled, or the last poll
    // failed the transport) — the status is not `connected*`; start
    // it.
    if !state.daemon_status.starts_with("connected") {
        let status = if state.daemon_status.is_empty() {
            "not connected"
        } else {
            state.daemon_status.as_str()
        };
        requirements.push(daemon_down_requirement(status));
    }

    // Case 2: the last poll / run recorded a permission error (the
    // daemon socket is group-gated and this user is not in the
    // `ramsleuth` group — a groupless client; std's `Permission
    // denied` matches case-insensitively).
    if state
        .error
        .as_deref()
        .is_some_and(|error| error.to_lowercase().contains("permission"))
    {
        requirements.push(group_requirement());
    }

    // Case 3: AMD silicon with the daemon connected and the
    // `ryzen_smu` driver missing (the AMD branch is
    // `Na(DriverMissing)`).
    // Case 4: Intel silicon with the daemon connected and the
    // `ramsleuth_intel` driver missing (the Intel branch is
    // `Na(DriverMissing)` — the module is absent and the `/dev/mem`
    // fallback is blocked); the mirror of the AMD case.
    if state.daemon_status.starts_with("connected") {
        if let Some(telemetry) = &state.telemetry {
            if matches!(telemetry.amd, Section::Na(NaReason::DriverMissing))
                && matches!(host_vendor(telemetry), CpuVendor::Amd(_))
            {
                requirements.push(dkms_requirement());
            }
            if matches!(telemetry.intel, Section::Na(NaReason::DriverMissing))
                && matches!(host_vendor(telemetry), CpuVendor::Intel(_))
            {
                requirements.push(intel_dkms_requirement());
            }
        }
    }

    requirements
}

/// Render the `SETUP — requirements` strip into `area`: a titled
/// `Block` on the amber warning border with one amber summary line, a
/// dim indented detail, and `$ <command>` per requirement (plan
/// §2.3). Pure over `&[Requirement]` — the presence decision (draw at
/// all) is the caller's (TUI-16); an empty slice draws the empty
/// titled block, and a zero-size area draws nothing (never a panic,
/// D5).
pub fn render_requirements_strip(frame: &mut Frame, requirements: &[Requirement], area: Rect) {
    if area.is_empty() {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(AMBER))
        .title("SETUP — requirements")
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(requirements_lines(requirements)), inner);
}

/// The strip's content: per requirement, `! <summary>` (amber), the
/// indented dim detail, and `$ <command>` (cyan, when the requirement
/// carries one), then the dim no-panic footer (the GUI `first_run`
/// strip's lines, terminal-flavored).
fn requirements_lines(requirements: &[Requirement]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for requirement in requirements {
        lines.push(Line::from(vec![
            Span::styled("! ", Style::default().fg(AMBER)),
            Span::styled(requirement.summary.to_owned(), Style::default().fg(AMBER)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {}", requirement.detail),
            Style::default().fg(DIM),
        )));
        if let Some(command) = &requirement.command {
            lines.push(Line::from(vec![
                Span::styled("$ ", Style::default().fg(CYAN)),
                Span::styled(command.to_owned(), Style::default().fg(CYAN)),
            ]));
        }
    }
    if !requirements.is_empty() {
        lines.push(Line::from(Span::styled(
            "RamSleuth keeps running without this — degraded sections show \
              `N/A (DriverMissing)`, exit 0, no panic.",
            Style::default().fg(DIM),
        )));
    }
    lines
}

// ---------------------------------------------------------------------
// The `[d]` screen's controls block: the full 17-key contract laid
// out as a responsive `key — action` grid (the app description is the
// docs' job — `Docs/User_Guide.md` — so the screen shows only the
// controls). The always-present companion to the presence-driven
// requirements strip.
// ---------------------------------------------------------------------

/// One key row of the controls block: the single-char key (lowercase —
/// the contract is case-insensitive, modifiers ignored) and its
/// one-line action text (the `User_Guide §5.1` mirror, compact).
#[derive(Debug, Clone, Copy)]
struct KeyRow {
    /// The key, e.g. `r`.
    key: &'static str,
    /// The action text, e.g. `Refresh — force poll`.
    text: &'static str,
}

/// A logical group of the 17-key contract: the small heading + its
/// rows (the groups aid scanning; the rows keep the frozen `events`
/// table's order within each class).
#[derive(Debug, Clone, Copy)]
struct KeyGroup {
    /// The group heading, e.g. `DATA`.
    heading: &'static str,
    /// The group's rows.
    rows: &'static [KeyRow],
}

/// The three groups of the 17-key contract: **DATA** (the fetch /
/// snapshot / export surface — `r` `s` `e` `p` `a` `f`), **BENCH**
/// (the run class — `b` `m` `x` `c`), **VIEWS** (the display toggles +
/// the session exit — `g` `t` `d` `u` `k` `w` `q`).
const KEY_GROUPS: [KeyGroup; 3] = [
    KeyGroup {
        heading: "DATA",
        rows: &[
            KeyRow { key: "r", text: "Refresh — force poll" },
            KeyRow { key: "s", text: "Snapshot — .txt → CWD" },
            KeyRow { key: "e", text: "Export — JSON → $HOME" },
            KeyRow { key: "p", text: "Poll — 100 ms → 60 s" },
            KeyRow { key: "a", text: "Auto refresh — on ↔ off" },
            KeyRow { key: "f", text: "Probe report — consent" },
        ],
    },
    KeyGroup {
        heading: "BENCH",
        rows: &[
            KeyRow { key: "b", text: "Bench — full run" },
            KeyRow { key: "m", text: "Bench — memory only" },
            KeyRow { key: "x", text: "Burn-in — 5 min soak" },
            KeyRow { key: "c", text: "Cancel — in-flight run" },
        ],
    },
    KeyGroup {
        heading: "VIEWS",
        rows: &[
            KeyRow { key: "g", text: "Graphs — 5-series panel" },
            KeyRow { key: "t", text: "Settings — the strip" },
            KeyRow { key: "d", text: "Info — this screen" },
            KeyRow { key: "u", text: "Capacity — GiB ↔ GB" },
            KeyRow { key: "k", text: "Clock — MHz ↔ GHz" },
            KeyRow { key: "w", text: "Window — 1 → 60 min" },
            KeyRow { key: "q", text: "Quit — exit 0" },
        ],
    },
];

/// The inter-column gap (the blank cells between the grid's columns).
const COLUMN_GAP: u16 = 2;

/// One cell of the grid's row sequence: a group heading or a key row.
#[derive(Debug, Clone, Copy)]
enum Cell {
    /// A group heading, e.g. `DATA` (the bold light-grey row).
    Heading(&'static str),
    /// A key row (the cyan `[k]` token + the light-grey text).
    Row(KeyRow),
}

/// The widest display row (`[k] text`, chars = cells — the codebase's
/// width convention): the column-fit thresholds derive from it, so
/// the grid never wraps at the widths it uses.
fn max_row_width() -> u16 {
    KEY_GROUPS
        .iter()
        .flat_map(|group| group.rows)
        .map(|row| 4 + row.text.chars().count() as u16)
        .max()
        .unwrap_or(0)
}

/// The grid's column count for the available inner width: three
/// columns (one group each) when they fit, two (DATA+BENCH | VIEWS)
/// next, one below (the very-narrow fallback — the rows clip
/// gracefully, never a panic, D5).
fn about_columns(inner: u16) -> usize {
    let widest = max_row_width();
    if inner >= 3 * widest + 2 * COLUMN_GAP {
        3
    } else if inner >= 2 * widest + COLUMN_GAP {
        2
    } else {
        1
    }
}

/// The column group indices for a column count (3 → one group per
/// column, 2 → DATA+BENCH | VIEWS, 1 → all three stacked).
fn column_group_indices(columns: usize) -> Vec<Vec<usize>> {
    match columns {
        3 => vec![vec![0], vec![1], vec![2]],
        2 => vec![vec![0, 1], vec![2]],
        _ => vec![vec![0, 1, 2]],
    }
}

/// One column's row sequence: its groups' headings + rows, in order.
fn column_sequence(columns: usize, column: usize) -> Vec<Cell> {
    column_group_indices(columns)[column]
        .iter()
        .flat_map(|&group_index| {
            let group = &KEY_GROUPS[group_index];
            std::iter::once(Cell::Heading(group.heading)).chain(
                group
                    .rows
                    .iter()
                    .map(|row| Cell::Row(*row)),
            )
        })
        .collect()
}

/// The controls block's height for the given full width: the two
/// border rows + the tallest column (its groups' headings + rows).
/// Replaces the old fixed ten rows: ten at ≥ 87 cols (three columns),
/// fourteen at 58–86 (two), twenty-two below (one).
pub fn about_block_height(width: u16) -> u16 {
    let inner = width.saturating_sub(2);
    if inner == 0 {
        return 2;
    }
    let columns = about_columns(inner);
    let tallest = column_group_indices(columns)
        .iter()
        .map(|groups| {
            groups
                .iter()
                .map(|&group_index| 1 + KEY_GROUPS[group_index].rows.len() as u16)
                .sum::<u16>()
        })
        .max()
        .unwrap_or(0);
    2 + tallest
}

/// Render the `CONTROLS — RamSleuth` block into `area` (the `[d]`
/// screen's controls section, the requirements strip's companion): the
/// full 17-key contract as a responsive `key — action` grid (the
/// three DATA / BENCH / VIEWS groups in one, two, or three columns by
/// width). Pure (no state read) and no-panic (D5): a zero-size area
/// draws nothing; the caller's presence decision (draw at all — the
/// `[d]` toggle) is the ui.rs render chain's.
pub fn render_about_block(frame: &mut Frame, area: Rect) {
    if area.is_empty() {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title("CONTROLS — RamSleuth")
        .title_style(Style::default().fg(LIGHT_GREY))
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(about_lines(area.width)), inner);
}

/// The controls block's content lines for the given full width: each
/// row joins the per-column cells (a group heading, or a `[k] text`
/// row, left-aligned, padded to the column width, gap-separated),
/// clipped to the column width — a very-narrow surface degrades to
/// one column of cleanly clipped rows (never a panic, D5).
fn about_lines(width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    if inner == 0 {
        return Vec::new();
    }
    let columns = about_columns(inner);
    let column_width = (inner - COLUMN_GAP * (columns - 1) as u16) / columns as u16;
    let sequences: Vec<Vec<Cell>> = (0..columns)
        .map(|column| column_sequence(columns, column))
        .collect();
    let rows = sequences.iter().map(Vec::len).max().unwrap_or(0);
    (0..rows)
        .map(|row| {
            let mut spans = Vec::new();
            for (column, sequence) in sequences.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::raw(" ".repeat(COLUMN_GAP as usize)));
                }
                match sequence.get(row) {
                    Some(Cell::Heading(heading)) => {
                        let display = heading.to_owned();
                        let cells = display.chars().count();
                        if cells > column_width as usize {
                            spans.push(Span::raw(
                                display
                                    .chars()
                                    .take(column_width as usize)
                                    .collect::<String>(),
                            ));
                        } else {
                            spans.push(Span::styled(
                                display,
                                Style::default().fg(LIGHT_GREY).add_modifier(Modifier::BOLD),
                            ));
                            spans.push(Span::raw(" ".repeat(
                                column_width as usize - cells,
                            )));
                        }
                    }
                    Some(Cell::Row(KeyRow { key, text })) => {
                        let display = format!("[{key}] {text}");
                        let cells = display.chars().count();
                        if cells > column_width as usize {
                            spans.push(Span::raw(
                                display
                                    .chars()
                                    .take(column_width as usize)
                                    .collect::<String>(),
                            ));
                        } else {
                            spans.push(Span::styled(
                                format!("[{key}]"),
                                Style::default().fg(CYAN),
                            ));
                            spans.push(Span::styled(
                                format!(" {text}"),
                                Style::default().fg(LIGHT_GREY),
                            ));
                            spans.push(Span::raw(
                                " ".repeat(column_width as usize - cells),
                            ));
                        }
                    }
                    // A shorter column: the rest of the row is blank.
                    None => spans.push(Span::raw(" ".repeat(column_width as usize))),
                }
            }
            Line::from(spans)
        })
        .collect()
}

// ---------------------------------------------------------------------
// Tests (headless: the six `diagnose` cases from the GUI first_run
// mirror + the controls block's 17-key content / width fit / no-panic
// render over ratatui's in-memory TestBackend, the ui.rs test idiom).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_telemetry::cpuid::{AmdZen, IntelGen};
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// An all-`Na` snapshot with the CPU vendor + the two
    /// vendor-branch reasons forced (host-independent — the GUI
    /// first_run test pattern).
    fn snapshot(vendor: CpuVendor, amd: NaReason, intel: NaReason) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor,
                brand: "requirements test CPU".to_owned(),
            },
            amd: Section::na(amd),
            intel: Section::na(intel),
            spd: Vec::new(),
            platform: SystemPlatform {
                cpu_clock_mhz: Section::na(NaReason::NotApplicable),
                motherboard: Section::na(NaReason::NotApplicable),
                bios: Section::na(NaReason::NotApplicable),
                agesa: Section::na(NaReason::NotApplicable),
                smu_version: Section::na(NaReason::NotApplicable),
            },
            total_capacity: Section::na(NaReason::NotApplicable),
            dimm_sizes: Vec::new(),
        }
    }

    /// A connected state (the daemon is up, no error).
    fn connected(vendor: CpuVendor, amd: NaReason, intel: NaReason) -> AppState {
        AppState {
            telemetry: Some(snapshot(vendor, amd, intel)),
            daemon_status: "connected: /run/ramsleuth/ramsleuth.sock".to_owned(),
            ..Default::default()
        }
    }

    /// (a) A daemon-less default state (never polled) → exactly the
    /// single daemon requirement, no panic (the no-panic contract, D5).
    #[test]
    fn diagnose_default_is_daemon_only() {
        let requirements = diagnose(&AppState::default());
        assert_eq!(
            requirements.len(),
            1,
            "a daemon-less default yields exactly the daemon requirement: {requirements:?}"
        );
        assert_eq!(requirements[0].summary, "Start the ramsleuth daemon");
        assert_eq!(
            requirements[0].command.as_deref(),
            Some("sudo systemctl enable --now ramsleuth")
        );
        assert!(
            requirements[0].detail.contains("not connected")
                && requirements[0]
                    .detail
                    .contains("systemctl status ramsleuth"),
            "the detail carries the diagnostic: {}",
            requirements[0].detail
        );
    }

    /// (b) A connected, clean state (Intel silicon — the built-in
    /// MCHBAR decode, no extra driver; both vendor branches
    /// non-`DriverMissing`) → zero requirements.
    #[test]
    fn diagnose_connected_clean() {
        let state = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable,
            NaReason::NotApplicable,
        );
        assert!(
            diagnose(&state).is_empty(),
            "a connected clean state yields no requirement: {:?}",
            diagnose(&state)
        );
    }

    /// (c) A groupless client: the last poll hit a permission error
    /// (std's `Permission denied` — the case-insensitive match) → the
    /// daemon requirement + the join-the-group requirement (the
    /// re-login note in the detail).
    #[test]
    fn diagnose_permission_error() {
        let state = AppState {
            daemon_status: "disconnected".to_owned(),
            error: Some(
                "cannot connect to /run/ramsleuth/ramsleuth.sock: daemon not running? \
                 (last error: Permission denied (os error 13))"
                    .to_owned(),
            ),
            ..Default::default()
        };
        let requirements = diagnose(&state);
        assert_eq!(
            requirements.len(),
            2,
            "a permission failure yields the daemon + group requirements: {requirements:?}"
        );
        assert_eq!(requirements[0].summary, "Start the ramsleuth daemon");
        let group = requirements
            .iter()
            .find(|r| r.command.as_deref() == Some("sudo usermod -aG ramsleuth $USER"))
            .expect("the permission error must yield the join-group requirement");
        assert_eq!(group.summary, "Join the `ramsleuth` group");
        assert!(
            group.detail.contains("re-login"),
            "a re-login note is required: {}",
            group.detail
        );
    }

    /// (d) AMD silicon, the daemon connected, the AMD branch
    /// `Na(DriverMissing)` → exactly the pinned-DKMS requirement (the
    /// detail names the pinned upstream).
    #[test]
    fn diagnose_amd_driver_missing() {
        let state = connected(
            CpuVendor::Amd(AmdZen::Zen3),
            NaReason::DriverMissing,
            NaReason::NotApplicable,
        );
        let requirements = diagnose(&state);
        assert_eq!(
            requirements.len(),
            1,
            "a connected AMD driver-missing state yields exactly the DKMS requirement: {requirements:?}"
        );
        assert_eq!(
            requirements[0].summary,
            "Install the `ryzen_smu` kernel module (live AMD subtimings)"
        );
        assert_eq!(requirements[0].command.as_deref(), Some(DKMS_INSTALL_CMD));
        assert!(
            requirements[0].detail.contains(RYZEN_SMU_PIN_SHORT),
            "the detail names the pinned upstream: {}",
            requirements[0].detail
        );
    }

    /// (e) Non-AMD silicon (Intel) with the AMD branch
    /// `Na(DriverMissing)` and the daemon connected → no AMD DKMS
    /// requirement (the vendor gate, the GUI first_run (d) mirror).
    #[test]
    fn diagnose_non_amd_driver_missing() {
        let state = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::DriverMissing,
            NaReason::NotApplicable,
        );
        assert!(
            diagnose(&state).is_empty(),
            "a non-AMD driver-missing state yields no requirement: {:?}",
            diagnose(&state)
        );
    }

    /// (f) Intel silicon, the daemon connected, the Intel branch
    /// `Na(DriverMissing)` (the `ramsleuth_intel` module is absent and
    /// the `/dev/mem` fallback is blocked) → exactly the Intel-DKMS
    /// requirement (the GUI first_run (c') mirror); non-`DriverMissing`
    /// Intel → none (the healthy case is covered by
    /// `diagnose_connected_clean`).
    #[test]
    fn diagnose_intel_driver_missing() {
        let state = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable, // amd: N/A on Intel silicon
            NaReason::DriverMissing, // intel: module absent
        );
        let requirements = diagnose(&state);
        assert_eq!(
            requirements.len(),
            1,
            "a connected Intel driver-missing state yields exactly the Intel DKMS requirement: {requirements:?}"
        );
        assert_eq!(
            requirements[0].summary,
            "Install the `ramsleuth_intel` kernel module (live Intel subtimings)"
        );
        assert_eq!(requirements[0].command.as_deref(), Some(DKMS_INSTALL_CMD_INTEL));
        assert!(
            requirements[0].detail.contains("bundled `ramsleuth_intel`"),
            "the detail names the bundled module: {}",
            requirements[0].detail
        );

        // Intel silicon without the driver-missing reason → no DKMS
        // requirement.
        let not_missing = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable,
            NaReason::NotApplicable,
        );
        assert!(
            diagnose(&not_missing).is_empty(),
            "non-DriverMissing Intel yields no requirement"
        );
    }

    /// (g) The strip's no-panic render (the ui.rs TestBackend idiom):
    /// a zero-area surface draws nothing, and one requirement paints
    /// the title, the amber summary, and the `$` command.
    #[test]
    fn render_strip_no_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(0, 0);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|f| render_requirements_strip(f, &[group_requirement()], Rect::new(0, 0, 0, 0)))
            .expect("a zero-area render must not panic");

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render_requirements_strip(f, &[group_requirement()], Rect::new(0, 0, 80, 10)))
            .expect("render must not panic");
        let buffer = completed.buffer;
        let width = usize::from(buffer.area().width);
        let text = buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("SETUP — requirements"), "{text}");
        assert!(text.contains("Join the `ramsleuth` group"), "{text}");
        assert!(
            text.contains("$ sudo usermod -aG ramsleuth $USER"),
            "the exact command must paint: {text}"
        );
    }

    /// (h) The controls block lists exactly the frozen 17-key
    /// contract (the `events::key_to_action` set — the two surfaces
    /// can't drift): each contract key appears once, every listed key
    /// maps through the pure `key_to_action` to its action, and the
    /// three group headings are present in order.
    #[test]
    fn about_lines_are_the_17_key_contract() {
        use std::collections::BTreeSet;

        use crate::events::{key_to_action, Action};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // The frozen seventeen action keys (the `events` table).
        let contract: [(char, Action); 17] = [
            ('r', Action::Refresh),
            ('s', Action::Snapshot),
            ('q', Action::Quit),
            ('b', Action::BenchFull),
            ('m', Action::BenchMemory),
            ('x', Action::BurnIn),
            ('c', Action::Cancel),
            ('g', Action::ToggleGraphs),
            ('t', Action::ToggleSettings),
            ('d', Action::ToggleRequirements),
            ('e', Action::ExportJson),
            ('p', Action::CyclePoll),
            ('u', Action::ToggleCapacity),
            ('k', Action::ToggleClock),
            ('a', Action::ToggleRefresh),
            ('w', Action::CycleWindow),
            ('f', Action::ProbeReport),
        ];
        // The block's key set is exactly the contract's (each key
        // once — no duplicates, no omissions).
        let about_keys: BTreeSet<char> = KEY_GROUPS
            .iter()
            .flat_map(|group| group.rows)
            .map(|row| row.key.chars().next().expect("a key row has a key"))
            .collect();
        let contract_keys: BTreeSet<char> = contract
            .iter()
            .map(|&(key, _)| key)
            .collect();
        assert_eq!(
            about_keys, contract_keys,
            "the controls block must list exactly the frozen 17-key contract"
        );
        assert_eq!(
            KEY_GROUPS
                .iter()
                .map(|group| group.rows.len())
                .sum::<usize>(),
            17,
            "the full 17-key contract"
        );
        // Every listed key maps through the pure contract to its
        // action (the screen can't advertise a dead key).
        for (key, action) in contract {
            assert_eq!(
                key_to_action(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Some(action),
                "the listed key {key} must map to {action:?}"
            );
        }
        // The three group headings, in order (the scan aids).
        assert_eq!(
            KEY_GROUPS
                .iter()
                .map(|group| group.heading)
                .collect::<Vec<_>>(),
            vec!["DATA", "BENCH", "VIEWS"]
        );
    }

    /// (h') The responsive grid fits its width at every threshold
    /// crossing: three columns (one group each) from 87 cols, two
    /// (DATA+BENCH | VIEWS) from 58, one below — the rendered rows
    /// match the block height (no off-by-one vs the layout
    /// constraint), and no row overruns the inner width.
    #[test]
    fn about_grid_fits_its_width() {
        // The column thresholds (the widest row is 27 cells).
        assert_eq!(about_columns(55), 1, "55 < 2×27+2");
        assert_eq!(about_columns(56), 2, "56 = 2×27+2");
        assert_eq!(about_columns(84), 2, "84 < 3×27+4");
        assert_eq!(about_columns(85), 3, "85 = 3×27+4");
        // The heights: the two border rows + the tallest column.
        assert_eq!(about_block_height(0), 2);
        assert_eq!(about_block_height(1), 2);
        assert_eq!(about_block_height(57), 22, "one column: 20 content rows");
        assert_eq!(about_block_height(58), 14, "two columns: 12 content rows");
        assert_eq!(about_block_height(80), 14);
        assert_eq!(about_block_height(86), 14);
        assert_eq!(about_block_height(87), 10, "three columns: 8 content rows");
        assert_eq!(about_block_height(100), 10);
        // Every rendered row fits its width, and the row count matches
        // the height, at each sample width.
        for width in [2u16, 20, 28, 29, 57, 58, 80, 86, 87, 100, 200] {
            let inner = width.saturating_sub(2);
            let lines = about_lines(width);
            assert_eq!(
                lines.len() as u16,
                about_block_height(width) - 2,
                "at {width} cols the rendered rows must match the block height"
            );
            for line in &lines {
                let rendered: usize = line
                    .spans
                    .iter()
                    .map(|span| span.content.chars().count())
                    .sum();
                assert!(
                    rendered <= inner as usize,
                    "a {width}-col row ({rendered} cells) must not overflow the inner width"
                );
            }
        }
    }

    /// (i) The controls block's no-panic render: a zero-area surface
    /// draws nothing, and an 80×16 surface (the two-column width)
    /// paints the title + the group headings + the key rows.
    #[test]
    fn render_about_block_no_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(0, 0);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|f| render_about_block(f, Rect::new(0, 0, 0, 0)))
            .expect("a zero-area render must not panic");

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render_about_block(f, Rect::new(0, 0, 80, 16)))
            .expect("render must not panic");
        let buffer = completed.buffer;
        let width = usize::from(buffer.area().width);
        let text = buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("CONTROLS — RamSleuth"), "{text}");
        for heading in ["DATA", "BENCH", "VIEWS"] {
            assert!(text.contains(heading), "{text}");
        }
        assert!(text.contains("[r] Refresh — force poll"), "{text}");
        assert!(text.contains("[q] Quit — exit 0"), "{text}");
    }

    /// (i') The exact layout at the two preview widths — 100 cols
    /// (three columns, 32 cells each + the two 2-cell gaps) and 60
    /// cols (two columns, 28 cells each + the one gap): the heading
    /// row + the first key row, byte-for-byte (the operator preview
    /// pin).
    #[test]
    fn about_block_lines_at_preview_widths() {
        let join = |lines: &[Line]| {
            lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        // 100 cols: DATA | BENCH | VIEWS (31 + 2 + 31 + 2 + 31 = 97 of
        // the 98 inner cells).
        let text = join(&about_lines(100));
        assert_eq!(
            text[0],
            format!(
                "{}{}  {}{}  {}{}",
                "DATA",
                " ".repeat(27),
                "BENCH",
                " ".repeat(26),
                "VIEWS",
                " ".repeat(26)
            )
        );
        assert_eq!(
            text[1],
            format!(
                "{}{}  {}{}  {}{}",
                "[r] Refresh — force poll",
                " ".repeat(7),
                "[b] Bench — full run",
                " ".repeat(11),
                "[g] Graphs — 5-series panel",
                " ".repeat(4)
            )
        );
        // 60 cols: DATA+BENCH | VIEWS (28 + 2 + 28 = 58).
        let text = join(&about_lines(60));
        assert_eq!(
            text[0],
            format!("{}{}  {}{}", "DATA", " ".repeat(24), "VIEWS", " ".repeat(23))
        );
        assert_eq!(
            text[1],
            format!(
                "{}{}  {}{}",
                "[r] Refresh — force poll",
                " ".repeat(4),
                "[g] Graphs — 5-series panel",
                " ".repeat(1)
            )
        );
        // The `[c]`/`[q]` pair: the BENCH column's final row | the
        // VIEWS column's final row.
        assert_eq!(
            text[7],
            format!(
                "{}{}  {}{}",
                "BENCH",
                " ".repeat(23),
                "[q] Quit — exit 0",
                " ".repeat(11)
            )
        );
        assert_eq!(
            text[11],
            format!(
                "{}{}  {}",
                "[c] Cancel — in-flight run",
                " ".repeat(2),
                " ".repeat(28)
            )
        );
    }

    /// (j) The gmktec class: the daemon serves on unsupported Intel
    /// hardware (both vendor branches `Na(UnsupportedHardware)` — the
    /// N100's report) → zero requirements: a healthy state, not a
    /// setup prompt (the liveness trigger — the daemon serving is
    /// what suppresses the strip, no state check involved).
    #[test]
    fn diagnose_connected_unsupported_hardware() {
        let state = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::UnsupportedHardware,
            NaReason::UnsupportedHardware,
        );
        assert!(
            diagnose(&state).is_empty(),
            "a serving daemon on unsupported hardware yields no requirement: {:?}",
            diagnose(&state)
        );
    }
}
