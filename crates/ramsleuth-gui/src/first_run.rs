//! First-run / SETUP requirements: what is degraded, why, and the exact
//! command to fix it (C18, D-18.5).
//!
//! [`diagnose`] reads the same degradation signals the zones already use
//! (the no-panic contract, plan D5) and turns them into a short list of
//! actionable [`Requirement`]s: (1) the daemon is down (the status is
//! not `connected*`) → `sudo systemctl enable --now ramsleuth` (the
//! detail carries the status diagnostic + the `systemctl status
//! ramsleuth` check); (2) the last poll recorded a permission error
//! (the daemon socket is group-gated — a groupless client) →
//! `sudo usermod -aG ramsleuth $USER` (a re-login is required after
//! joining); (3) AMD silicon with the daemon connected and the AMD
//! branch `Na(DriverMissing)` (the host's vendor comes from the
//! existing pure-CPUID detection — the snapshot's `cpu.vendor`, with
//! [`CpuInfo::detect()`] as the unprivileged fallback that needs no
//! daemon) → `sudo ramsleuth-install-ryzen-smu-dkms` (the detail names
//! the pinned upstream, [`RYZEN_SMU_PIN_SHORT`] — D-18.6 transparency
//! inside the app). Intel (the built-in MCHBAR decode) and healthy AMD
//! → no requirements at all.
//!
//! [`render_requirements_strip_with_setup`] paints that list as the
//! `SETUP` strip the app shell shows on launch (the C18-02
//! integration): a bold-CYAN title, the primary CYAN
//! **`Set up RamSleuth`** button (C21 — the one-click wizard; the AMD
//! `DriverMissing` case labels it `+ AMD driver`) + its dim live status
//! line (idle / `running…` / `done — restart RamSleuth to activate` /
//! `failed: <msg>`), one row per requirement (an AMBER `!`, the
//! summary, the dim detail, the command with a **Copy** button — the
//! secondary fallback), a `Got it — keep using RamSleuth` button, and
//! the dim no-panic footer.
//!
//! **One-click setup (C21) + the Copy fallback (D-18.5, risk (a)):**
//! the primary affordance is the `Set up RamSleuth` button — a thin
//! client over the pkexec-able `ramsleuth-setup` root helper (one
//! privileged pass: daemon enable+start, group join, the socket ACL —
//! and on AMD the offline DKMS driver build + `modprobe`; no
//! re-login, no reboot). The render thread only flips
//! [`SetupOutcome::running`] (D6: zero I/O on the render thread — the
//! actual `pkexec` spawn is the setup worker's job, C21-06). The
//! per-row **Copy** buttons are KEPT as the secondary polkit-less
//! fallback (the D-18.5 grace line for the `polkit`/`acl`-less edge;
//! the clipboard path — no terminal spawn, no `sudo` shell-out).
//!
//! **No-panic contract (plan D5):** [`diagnose`] is pure and total — a
//! daemon-less [`TelemetryData::default()`] yields the single daemon
//! requirement, every input degrades, and neither [`diagnose`] nor
//! [`render_requirements_strip`] panics (no `unwrap` / `expect` /
//! `panic!` in the production paths).
//!
//! **Wiring:** C18-11 declares this module + the root re-exports, and
//! C18-02 consumes it (the header's `Setup` toggle + the auto-shown
//! strip between the header and the settings area — presence-driven:
//! it disappears on its own once every requirement is resolved).
//! C21-04 adds the one-click setup wizard ([`setup_argv`] +
//! [`SetupOutcome`] + [`setup_with_dkms`] +
//! [`render_requirements_strip_with_setup`]); C21-05 re-exports the
//! wizard symbols, C21-06 wires the app shell's setup worker to the
//! 4-arg entry (the 3-arg [`render_requirements_strip`] stays as the
//! pre-worker call site, the wizard rendering idle).

use ramsleuth_telemetry::cpuid::{CpuInfo, CpuVendor};
use ramsleuth_telemetry::error::{NaReason, Section};
use ramsleuth_telemetry::SystemMemoryTelemetry;

use crate::update::TelemetryData;
use crate::{AMBER, CYAN, NA_GRAY, SLATE};

/// The pinned `ryzen_smu` upstream, short sha (D-18.6):
/// `amkillam/ryzen_smu @ d2983668300dd2a598e5a7dc40e71ce0678cc270`
/// (verified 2026-08-15 — the current `main` HEAD; the full sha lives
/// in the shared helper, C18-09). This short form is the in-app
/// transparency line (the AMD requirement's detail).
pub const RYZEN_SMU_PIN_SHORT: &str = "d298366";

/// The manual/CLI fallback command for the AMD driver (the DKMS
/// requirement's command — the secondary polkit-less path, D-18.5).
pub const DKMS_INSTALL_CMD: &str = "sudo ramsleuth-install-ryzen-smu-dkms";

/// One actionable first-run requirement: the one-line summary (the
/// bold row text), the dim detail, and the exact command to run
/// (rendered with a **Copy** button — the clipboard is the "run it"
/// path, D-18.5; `None` = informational only, no command to copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The one-line summary, e.g. "Start the ramsleuth daemon".
    pub summary: String,
    /// The dim detail line: the diagnostic + the resolution note.
    pub detail: String,
    /// The exact command to run, or `None` when informational only.
    pub command: Option<String>,
}

/// Case 1 — the daemon is down: start it (the detail carries the status
/// diagnostic + the `systemctl status ramsleuth` check).
fn daemon_down_requirement(status: &str) -> Requirement {
    Requirement {
        summary: "Start the ramsleuth daemon".to_owned(),
        detail: format!("daemon: {status} — check with `systemctl status ramsleuth`"),
        command: Some("sudo systemctl enable --now ramsleuth".to_owned()),
    }
}

/// Case 2 — a groupless client: join the `ramsleuth` group (a re-login
/// is required after joining).
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
            "the helper builds the pinned upstream `amkillam/ryzen_smu @ {RYZEN_SMU_PIN_SHORT}` — \
             shown + checksummed + confirmed before any build; RamSleuth runs without it"
        ),
        command: Some(DKMS_INSTALL_CMD.to_owned()),
    }
}

/// The host's CPU vendor for the AMD-branch check (D-18.5: pure CPUID,
/// no daemon needed): the snapshot's `cpu.vendor` is the daemon's own
/// CPUID detection (the same host) and is authoritative when it names
/// a vendor; the unprivileged GUI's [`CpuInfo::detect()`] is the
/// fallback when the snapshot carries `Unknown`.
fn host_vendor(telemetry: &SystemMemoryTelemetry) -> CpuVendor {
    match telemetry.cpu.vendor {
        CpuVendor::Amd(_) | CpuVendor::Intel(_) => telemetry.cpu.vendor,
        CpuVendor::Unknown => CpuInfo::detect().vendor,
    }
}

/// Diagnose the current snapshot into the actionable first-run
/// requirements (D-18.5, the three cases in the module doc). Pure +
/// total (the no-panic contract, D5): a daemon-less
/// [`TelemetryData::default()`] yields the single daemon requirement;
/// every input degrades, nothing panics.
pub fn diagnose(data: &TelemetryData) -> Vec<Requirement> {
    let mut requirements = Vec::new();

    // Case 1: the daemon is down (never polled, or the last poll failed
    // the transport) — the snapshot is absent or stale; start it.
    if !data.daemon_status.starts_with("connected") {
        let status = if data.daemon_status.is_empty() {
            "not connected"
        } else {
            data.daemon_status.as_str()
        };
        requirements.push(daemon_down_requirement(status));
    }

    // Case 2: the last poll / run recorded a permission error (the
    // daemon socket is group-gated and this user is not in the
    // `ramsleuth` group — a groupless client; std's `Permission
    // denied` matches case-insensitively).
    if data
        .error
        .as_deref()
        .is_some_and(|error| error.to_lowercase().contains("permission"))
    {
        requirements.push(group_requirement());
    }

    // Case 3: AMD silicon with the daemon connected and the `ryzen_smu`
    // driver missing (the AMD branch is `Na(DriverMissing)`).
    if data.daemon_status.starts_with("connected") {
        if let Some(telemetry) = &data.telemetry {
            if matches!(telemetry.amd, Section::Na(NaReason::DriverMissing))
                && matches!(host_vendor(telemetry), CpuVendor::Amd(_))
            {
                requirements.push(dkms_requirement());
            }
        }
    }

    requirements
}

/// The one-click setup helper's fixed argv (the frozen C21-01
/// contract: `ramsleuth-setup [--with-dkms] [--user <name>]` — any
/// flag order). Pure + headless-testable, zero I/O: the render thread
/// never spawns anything (D6) — the setup worker wired in C21-06 runs
/// this under `pkexec`. Under `pkexec` the caller must pass `--user`
/// with the current user's name (`SUDO_USER` may be unset).
pub fn setup_argv(with_dkms: bool, user: &str) -> Vec<String> {
    let mut argv = vec![
        "/usr/bin/ramsleuth-setup".to_owned(),
        "--user".to_owned(),
        user.to_owned(),
    ];
    if with_dkms {
        argv.push("--with-dkms".to_owned());
    }
    argv
}

/// The live setup status shared between the render thread and the
/// setup worker (C21-06). GUI-local: it NEVER crosses the wire (no
/// serde — the protocol crate stays byte-frozen, the zero-wire gate).
///
/// Ownership: the render thread's only permitted write is flipping
/// `running` on a button click (D6 — no I/O on the render thread);
/// the per-tick edge consumer (the C21-06 worker in main.rs) clears
/// `running` AT SPAWN of the detached `pkexec` [`setup_argv`] helper,
/// which then sets `done` (the helper exited 0 — all requested steps
/// succeeded or were no-ops) or `failure` (the helper's trailing
/// diagnostic, exit 1, or the spawn itself failed).
///
/// The strip's dim status line renders the four states: idle (the
/// default) / `running…` / `done — restart RamSleuth to activate` /
/// `failed: <msg>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupOutcome {
    /// The setup edge: a button click flips this, and the per-tick
    /// edge consumer (the C21-06 worker, main.rs) clears it AT SPAWN
    /// of the detached `pkexec` helper — so the button re-enables
    /// while the helper runs, and a re-click spawns a second
    /// (idempotent) helper run.
    pub running: bool,
    /// The helper exited 0 — all requested setup steps succeeded or
    /// were no-ops. The new session state (the group membership, the
    /// socket ACL) activates in the *next* app launch — the current
    /// process's session predates it (no re-login, no reboot); the
    /// C21-36 modal prompt offers the in-place relaunch.
    pub done: bool,
    /// The helper failed: the trailing diagnostic for the status line
    /// (`failed: <msg>`).
    pub failure: Option<String>,
}

/// Decide the `--with-dkms` flag from the diagnosed requirements:
/// true iff the AMD `DriverMissing` case (case 3 — the reused GUI AMD
/// detection: the `cpuid` vendor + the `Na(DriverMissing)` reason) is
/// present. On AMD the one click also builds + `modprobe`s the
/// offline driver; otherwise daemon/group/ACL only.
pub fn setup_with_dkms(requirements: &[Requirement]) -> bool {
    requirements
        .iter()
        .any(|r| r.command.as_deref() == Some(DKMS_INSTALL_CMD))
}

/// Render the `SETUP` requirements strip with the one-click setup
/// wizard into `ui` (the C18-02 panel body + the C21-04 wizard): the
/// bold-CYAN title, the primary CYAN **`Set up RamSleuth`** button
/// (the AMD `DriverMissing` case labels it `+ AMD driver` —
/// [`setup_with_dkms`]), its dim live status line (idle / `running…`
/// / `done — restart RamSleuth to activate` / `failed: <msg>`), one row
/// per requirement (an AMBER `!`, the summary, the dim detail, the
/// command with the **Copy** button — `ui.ctx().copy_text`, the
/// secondary polkit-less fallback, D-18.5), the `Got it — keep using
/// RamSleuth` button (it flips `*open` — the render thread's one
/// permitted write, no I/O, the D6 settings precedent), and the dim
/// no-panic footer.
///
/// The wizard's click handler only flips `setup.running` (the render
/// thread does zero I/O — D6; the `pkexec` spawn is the C21-06
/// worker's job). No-panic degradation: the button + status line hide
/// entirely when there is nothing to set up (`requirements` empty —
/// the strip is presence-driven); the button is disabled only while
/// the `running` edge is pending (consumed at the next tick's spawn)
/// and re-enables while the detached helper runs (a re-click spawns a
/// second, idempotent helper run).
pub fn render_requirements_strip_with_setup(
    ui: &mut egui::Ui,
    requirements: &[Requirement],
    open: &mut bool,
    setup: &mut SetupOutcome,
) {
    let frame = egui::Frame::default()
        .fill(SLATE)
        .stroke(egui::Stroke::new(1.0_f32, CYAN))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0));
    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new("SETUP — get the most out of RamSleuth")
                .strong()
                .color(CYAN),
        );
        // The one-click wizard (C21): the primary affordance —
        // hidden when there is nothing to set up (no-panic
        // degradation).
        if !requirements.is_empty() {
            let label = if setup_with_dkms(requirements) {
                "Set up RamSleuth + AMD driver"
            } else {
                "Set up RamSleuth"
            };
            let _ = ui.horizontal(|ui| {
                // The palette's primary accent: CYAN fill + SLATE
                // text; disabled while the helper runs (no
                // double-click).
                let resp = ui.add_enabled(
                    !setup.running,
                    egui::Button::new(egui::RichText::new(label).color(SLATE)).fill(CYAN),
                );
                if resp.clicked() {
                    // The render thread's one permitted write (no
                    // I/O, D6): the setup worker (C21-06) picks up
                    // `running` and spawns the `pkexec` helper.
                    setup.running = true;
                }
            });
            let (status, color) = if setup.running {
                ("running…".to_owned(), CYAN)
            } else if setup.done {
                ("done — restart RamSleuth to activate".to_owned(), CYAN)
            } else if let Some(failure) = &setup.failure {
                (format!("failed: {failure}"), AMBER)
            } else {
                (
                    "one click runs all the privileged setup — no re-login, no reboot".to_owned(),
                    NA_GRAY,
                )
            };
            ui.add(
                egui::Label::new(egui::RichText::new(status.as_str()).weak().color(color))
                    .wrap(true),
            );
        }
        for requirement in requirements.iter() {
            let _ = ui.horizontal(|ui| {
                ui.label(egui::RichText::new("!").color(AMBER));
                ui.label(egui::RichText::new(requirement.summary.as_str()).strong());
                if let Some(command) = &requirement.command {
                    ui.label(egui::RichText::new(command.as_str()).monospace());
                    if ui.add(egui::Button::new("Copy")).clicked() {
                        // Copy-only (D-18.5, risk (a)): no terminal
                        // spawn, no sudo shell-out — the clipboard is
                        // the "run it" path (D6).
                        ui.ctx().copy_text(command.clone());
                    }
                }
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(requirement.detail.as_str())
                        .weak()
                        .color(NA_GRAY),
                )
                .wrap(true),
            );
        }
        if ui
            .add(egui::Button::new("Got it — keep using RamSleuth"))
            .clicked()
        {
            // The render thread's one permitted write (no I/O, D6).
            *open = false;
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "RamSleuth keeps running without this — degraded sections show \
                     `N/A (DriverMissing)`, exit 0, no panic.",
                )
                .weak()
                .color(NA_GRAY),
            )
            .wrap(true),
        );
    });
}

/// Render the `SETUP` requirements strip into `ui` (the pre-C21-04
/// 3-arg entry — the app shell's current call site): the wizard
/// renders in its idle state over a throwaway local outcome (a click
/// there only flashes for a frame — no setup worker is wired yet).
/// The app shell moves to [`render_requirements_strip_with_setup`] in
/// C21-06 (its `AppState` owns the shared [`SetupOutcome`]).
pub fn render_requirements_strip(ui: &mut egui::Ui, requirements: &[Requirement], open: &mut bool) {
    let mut setup = SetupOutcome::default();
    render_requirements_strip_with_setup(ui, requirements, open, &mut setup);
}

// ---------------------------------------------------------------------
// Tests (headless: the five `diagnose` cases, the setup wizard (the
// frozen C21-01 argv + the `--with-dkms` decision + the button over
// the settings.rs two-frame `ctx.run` idiom) + the strip's `Got it`).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ramsleuth_telemetry::cpuid::{AmdZen, IntelGen};
    use ramsleuth_telemetry::SystemPlatform;

    use super::*;

    /// An all-`Na` snapshot with the CPU vendor forced
    /// (host-independent — the update.rs test `mock_snapshot`
    /// pattern).
    fn snapshot(vendor: CpuVendor, amd: NaReason, intel: NaReason) -> SystemMemoryTelemetry {
        SystemMemoryTelemetry {
            cpu: CpuInfo {
                vendor,
                brand: "first-run test CPU".to_owned(),
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

    /// A connected snapshot (the daemon is up, no error).
    fn connected(vendor: CpuVendor, amd: NaReason, intel: NaReason) -> TelemetryData {
        TelemetryData {
            telemetry: Some(snapshot(vendor, amd, intel)),
            daemon_status: "connected: /run/ramsleuth/ramsleuth.sock".to_owned(),
            ..Default::default()
        }
    }

    /// (a) The daemon is down (no snapshot, a `disconnected` status) →
    /// exactly the daemon requirement (command + the status diagnostic).
    #[test]
    fn diagnose_daemon_down() {
        let data = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            ..Default::default()
        };
        let requirements = diagnose(&data);
        assert_eq!(
            requirements.len(),
            1,
            "a plain daemon-down snapshot yields exactly the daemon requirement: {requirements:?}"
        );
        assert_eq!(requirements[0].summary, "Start the ramsleuth daemon");
        assert_eq!(
            requirements[0].command.as_deref(),
            Some("sudo systemctl enable --now ramsleuth")
        );
        assert!(
            requirements[0].detail.contains("disconnected")
                && requirements[0]
                    .detail
                    .contains("systemctl status ramsleuth"),
            "the detail carries the diagnostic: {}",
            requirements[0].detail
        );
    }

    /// (b) A groupless client: the last poll hit a permission error
    /// (std's `Permission denied` — case-insensitive match) → the
    /// daemon requirement + the join-the-group requirement (the
    /// re-login note in the detail).
    #[test]
    fn diagnose_permission_error() {
        let data = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            error: Some(
                "cannot connect to /run/ramsleuth/ramsleuth.sock: daemon not running? \
                 (last error: Permission denied (os error 13))"
                    .to_owned(),
            ),
            ..Default::default()
        };
        let requirements = diagnose(&data);
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

    /// (c) AMD silicon, the daemon connected, the AMD branch
    /// `Na(DriverMissing)` → exactly the pinned-DKMS requirement (the
    /// detail names the pinned upstream); non-`DriverMissing` AMD →
    /// none.
    #[test]
    fn diagnose_amd_driver_missing() {
        let data = connected(
            CpuVendor::Amd(AmdZen::Zen3),
            NaReason::DriverMissing,
            NaReason::NotApplicable,
        );
        let requirements = diagnose(&data);
        assert_eq!(
            requirements.len(),
            1,
            "a connected AMD driver-missing snapshot yields exactly the DKMS requirement: {requirements:?}"
        );
        assert_eq!(
            requirements[0].summary,
            "Install the `ryzen_smu` kernel module (live AMD subtimings)"
        );
        assert_eq!(
            requirements[0].command.as_deref(),
            Some("sudo ramsleuth-install-ryzen-smu-dkms")
        );
        assert!(
            requirements[0].detail.contains(RYZEN_SMU_PIN_SHORT),
            "the detail names the pinned upstream: {}",
            requirements[0].detail
        );

        // AMD silicon without the driver-missing reason → no DKMS
        // requirement.
        let not_missing = connected(
            CpuVendor::Amd(AmdZen::Zen3),
            NaReason::NotApplicable,
            NaReason::NotApplicable,
        );
        assert!(
            diagnose(&not_missing).is_empty(),
            "non-DriverMissing AMD yields no requirement"
        );
    }

    /// (d) Intel silicon (the built-in MCHBAR decode — no extra
    /// driver), the daemon connected, an all-`Na` snapshot → zero
    /// requirements.
    #[test]
    fn diagnose_intel_healthy() {
        let data = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable,
            NaReason::NotApplicable,
        );
        assert!(
            diagnose(&data).is_empty(),
            "Intel (and healthy AMD) yields no requirement"
        );
    }

    /// (e) A daemon-less `TelemetryData::default()` (never polled) →
    /// exactly the single daemon requirement, no panic (D5).
    #[test]
    fn diagnose_default_is_daemon_only() {
        let requirements = diagnose(&TelemetryData::default());
        assert_eq!(
            requirements.len(),
            1,
            "a daemon-less default yields exactly the daemon requirement: {requirements:?}"
        );
        assert_eq!(
            requirements[0].command.as_deref(),
            Some("sudo systemctl enable --now ramsleuth")
        );
        assert!(
            requirements[0].detail.contains("not connected"),
            "the empty status reads not connected: {}",
            requirements[0].detail
        );
    }

    /// (f) The strip's `Got it` button closes it: two frames over
    /// `ctx.run` (the settings.rs idiom) — frame 1 lays out the strip
    /// (the command text is painted), frame 2 delivers a click on the
    /// button's painted label (the graph.rs text-click idiom — a plain
    /// egui button has no explicit id in 0.27), and `open` flips
    /// `true` → `false`.
    #[test]
    fn first_run_gotit() {
        let requirement = Requirement {
            summary: "Install the `ryzen_smu` kernel module (live AMD subtimings)".to_owned(),
            detail: "a dim detail line".to_owned(),
            command: Some("sudo ramsleuth-install-ryzen-smu-dkms".to_owned()),
        };
        let ctx = egui::Context::default();
        let mut open = true;
        let frame_input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(968.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        fn show_strip(ctx: &egui::Context, requirement: &Requirement, open: &mut bool) {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_requirements_strip(ui, std::slice::from_ref(requirement), open);
            });
        }

        // Frame 1: the layout — no click must not close the strip.
        let first = ctx.run(frame_input(Vec::new()), |ctx| {
            show_strip(ctx, &requirement, &mut open)
        });
        assert!(open, "no click must not close the strip");

        // The command text is painted in the frame.
        let texts: Vec<&str> = first
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("sudo ramsleuth-install-ryzen-smu-dkms")),
            "the command text must be painted: {texts:?}"
        );

        // Frame 2: a click on the `Got it` button's painted label
        // closes the strip.
        let pos = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Got it — keep using RamSleuth" =>
                {
                    Some(egui::pos2(
                        text.pos.x + text.galley.size().x / 2.0,
                        text.pos.y + text.galley.size().y / 2.0,
                    ))
                }
                _ => None,
            })
            .expect("the Got-it button's label must be painted");
        let click = |pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = ctx.run(frame_input(vec![click(true), click(false)]), |ctx| {
            show_strip(ctx, &requirement, &mut open)
        });
        assert!(!open, "a click on the Got-it button must close the strip");
    }

    /// (g) The frozen C21-01 helper argv: `--user <name>` always
    /// (under `pkexec` the user must be explicit — `SUDO_USER` may be
    /// unset), `--with-dkms` only on the AMD variant.
    #[test]
    fn setup_argv_contract() {
        assert_eq!(
            setup_argv(false, "alice"),
            vec![
                "/usr/bin/ramsleuth-setup".to_owned(),
                "--user".to_owned(),
                "alice".to_owned()
            ]
        );
        assert_eq!(
            setup_argv(true, "alice"),
            vec![
                "/usr/bin/ramsleuth-setup".to_owned(),
                "--user".to_owned(),
                "alice".to_owned(),
                "--with-dkms".to_owned()
            ]
        );
    }

    /// (h) The `--with-dkms` decision reuses the GUI AMD detection
    /// (the diagnosed requirements): the DKMS case (3) present → the
    /// full setup; daemon/group-only → plain; empty → plain.
    #[test]
    fn setup_with_dkms_decision() {
        assert!(!setup_with_dkms(&[]));
        assert!(!setup_with_dkms(&[daemon_down_requirement("disconnected")]));
        assert!(!setup_with_dkms(&[group_requirement()]));
        assert!(setup_with_dkms(&[group_requirement(), dkms_requirement()]));
    }

    /// (i) The wizard over the two-frame `ctx.run` idiom: the primary
    /// button paints (the AMD label when the DKMS requirement is
    /// present), the click flips `SetupOutcome::running` (the render
    /// thread's one permitted write — the status line paints
    /// `running…` in the same frame), and `Got it` still closes the
    /// strip (the wizard doesn't steal the existing affordance).
    #[test]
    fn first_run_setup_wizard_frames() {
        let requirement = dkms_requirement(); // the AMD variant
        let ctx = egui::Context::default();
        let mut open = true;
        let mut setup = SetupOutcome::default();
        let frame_input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(968.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        fn show_strip(
            ctx: &egui::Context,
            requirement: &Requirement,
            open: &mut bool,
            setup: &mut SetupOutcome,
        ) {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_requirements_strip_with_setup(
                    ui,
                    std::slice::from_ref(requirement),
                    open,
                    setup,
                );
            });
        }

        // Frame 1: the layout — the AMD-labelled primary button + the
        // idle status line paint; no click leaves the outcome idle.
        let first = ctx.run(frame_input(Vec::new()), |ctx| {
            show_strip(ctx, &requirement, &mut open, &mut setup)
        });
        assert!(
            !setup.running && !setup.done && setup.failure.is_none(),
            "no click leaves the outcome idle"
        );
        let texts: Vec<&str> = first
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"Set up RamSleuth + AMD driver"),
            "the AMD-labelled primary button must paint: {texts:?}"
        );

        // Frame 2: a click on the button's painted label flips
        // `running` (the render thread does zero I/O) and the same
        // frame paints `running…`.
        let pos = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Set up RamSleuth + AMD driver" =>
                {
                    Some(egui::pos2(
                        text.pos.x + text.galley.size().x / 2.0,
                        text.pos.y + text.galley.size().y / 2.0,
                    ))
                }
                _ => None,
            })
            .expect("the setup button's label must be painted");
        let click_at = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let second = ctx.run(
            frame_input(vec![click_at(pos, true), click_at(pos, false)]),
            |ctx| show_strip(ctx, &requirement, &mut open, &mut setup),
        );
        assert!(setup.running, "the button click must flip running");
        let texts: Vec<&str> = second
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"running…"),
            "the status line must paint running…: {texts:?}"
        );

        // Frame 3: `Got it` still closes the strip.
        let pos = second
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Got it — keep using RamSleuth" =>
                {
                    Some(egui::pos2(
                        text.pos.x + text.galley.size().x / 2.0,
                        text.pos.y + text.galley.size().y / 2.0,
                    ))
                }
                _ => None,
            })
            .expect("the Got-it button's label must be painted");
        let _ = ctx.run(
            frame_input(vec![click_at(pos, true), click_at(pos, false)]),
            |ctx| show_strip(ctx, &requirement, &mut open, &mut setup),
        );
        assert!(!open, "a click on the Got-it button must close the strip");
    }
}
