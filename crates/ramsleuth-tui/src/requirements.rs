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
//! [`render_about_block`] companion (its fixed height,
//! [`ABOUT_BLOCK_HEIGHT`]): a compact `ABOUT — RamSleuth` block — what
//! RamSleuth is, how the daemon model works, the capabilities, the
//! Intel/AMD split, and the docs pointer. It is drawn while the `[d]`
//! toggle is open, independent of `diagnose`'s presence (informational,
//! not a warning — the cyan zone-border style).
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
use ratatui::style::{Color, Style};
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
/// Light grey: the About block's title (the zone-block style mirror).
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
// The `[d]` screen's About/Help section: what RamSleuth is, how the
// daemon model works, the capabilities, the Intel/AMD split, and the
// docs pointer — the always-present companion to the presence-driven
// requirements strip.
// ---------------------------------------------------------------------

/// The About block's fixed height: the two border rows + the eight
/// content rows of [`about_lines`].
pub const ABOUT_BLOCK_HEIGHT: u16 = 10;

/// Render the `ABOUT — RamSleuth` block into `area` (the `[d]`
/// screen's About/Help section, the requirements strip's companion):
/// a compact eight-line summary — what RamSleuth is, how the daemon
/// model works, the capabilities, the Intel/AMD split, and the docs
/// pointer. Pure (no state read) and no-panic (D5): a zero-size area
/// draws nothing; the caller's presence decision (draw at all — the
/// `[d]` toggle) is the ui.rs render chain's.
pub fn render_about_block(frame: &mut Frame, area: Rect) {
    if area.is_empty() {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CYAN))
        .title("ABOUT — RamSleuth")
        .title_style(Style::default().fg(LIGHT_GREY))
        .style(Style::default().bg(SLATE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(about_lines()), inner);
}

/// The About block's eight content lines (compact — the block must
/// fit at small terminal sizes; every line is pre-wrapped to ≤ 78
/// cells, the 80-column floor inside the border).
fn about_lines() -> Vec<Line<'static>> {
    const ABOUT: &[&str] = &[
        "Live RAM telemetry + an AIDA64-style memory benchmark for AMD/Intel desktops.",
        "A root daemon (CAP_SYS_RAWIO only) reads the AMD SMU PM tables, the Intel",
        "MCHBAR IMC registers, and SPD EEPROMs, serving this TUI over the Unix socket.",
        "Capabilities: live clocks, timings, voltages + CAD bus; SPD details with",
        "XMP/EXPO profiles; channel mode + ECC; benchmark [B]; burn-in [X]; probe [F].",
        "Intel: per-channel IMC subtimings (Tier 1–3, Skylake→Arrow Lake).",
        "AMD: SMU PM clocks + voltages + CAD bus.",
        "Full guide: Docs/User_Guide.md (or the README).",
    ];
    ABOUT
        .iter()
        .map(|line| Line::from(Span::styled(line.to_owned(), Style::default().fg(DIM))))
        .collect()
}

// ---------------------------------------------------------------------
// Tests (headless: the six `diagnose` cases from the GUI first_run
// mirror + the About block's no-panic render + the 80-column floor —
// the strip's no-panic render over ratatui's in-memory TestBackend,
// the ui.rs test idiom).
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

    /// (h) The About block's content stays compact: exactly eight
    /// lines, each within the 80-column floor (78 cells inside the
    /// border) — the `[d]` screen must not overflow at small terminal
    /// sizes (the no-panic contract, D5).
    #[test]
    fn about_lines_fit_the_80_column_floor() {
        let lines = about_lines();
        assert_eq!(lines.len(), 8, "the block's height pins the 8 content lines");
        for line in &lines {
            assert_eq!(line.spans.len(), 1, "each about line is one span");
            assert!(
                line.spans[0].content.chars().count() <= 78,
                "an about line must fit the 80-column floor: {:?}",
                line.spans[0].content
            );
        }
    }

    /// (i) The About block's no-panic render: a zero-area surface
    /// draws nothing, and an 80×12 surface paints the title + the
    /// docs pointer (all eight content lines fit the inner ten rows).
    #[test]
    fn render_about_block_no_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(0, 0);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        terminal
            .draw(|f| render_about_block(f, Rect::new(0, 0, 0, 0)))
            .expect("a zero-area render must not panic");

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).expect("test terminal must init");
        let completed = terminal
            .draw(|f| render_about_block(f, Rect::new(0, 0, 80, 12)))
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
        assert!(text.contains("ABOUT — RamSleuth"), "{text}");
        assert!(text.contains("Docs/User_Guide.md"), "{text}");
        assert!(text.contains("CAP_SYS_RAWIO"), "{text}");
    }
}
