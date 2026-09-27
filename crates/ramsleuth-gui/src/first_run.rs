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
//! `sudo usermod -aG ramsleuth $USER` (the one-click setup also
//! applies a per-user ACL, so the current session connects
//! immediately — no re-login; the manual command alone activates at
//! re-login); (3) AMD silicon with the daemon connected and the AMD
//! branch `Na(DriverMissing)` (the host's vendor comes from the
//! existing pure-CPUID detection — the snapshot's `cpu.vendor`, with
//! [`CpuInfo::detect()`] as the unprivileged fallback that needs no
//! daemon) → `sudo ramsleuth-install-ryzen-smu-dkms` (the detail names
//! the pinned upstream, [`RYZEN_SMU_PIN_SHORT`] — D-18.6 transparency
//! inside the app); (4) Intel silicon with the daemon connected and
//! the Intel branch `Na(DriverMissing)` (the module is absent and the
//! `/dev/mem` fallback is blocked) → `sudo ramsleuth-install-intel-dkms`
//! (the same pure-CPUID vendor check — the mirror of the AMD case).
//! Healthy AMD / Intel (the built-in MCHBAR decode) → no requirements
//! at all.
//!
//! [`render_requirements_strip_with_setup`] paints that list as the
//! `SETUP` strip the app shell shows on launch (the C18-02
//! integration): a bold-CYAN title, the single primary CYAN
//! **`Set up RamSleuth`** button (C21 — the unified one-click action:
//! one button over every requirement mix — the `--with-dkms` flag is
//! decided by the host's CPU vendor, not by the requirement list) +
//! its dim live status
//! line (idle / `running…` / `done — reconnecting to the daemon…` /
//! `one step left: <msg>` — the Secure Boot one-time MOK step, actionable
//! amber, not a failure / `failed: <msg>`), one row per requirement (an AMBER `!`, the
//! summary, the dim detail, the command with a **Copy** button — the
//! secondary fallback), a `Got it — keep using RamSleuth` button, and
//! the dim no-panic footer.
//!
//! **One-click setup (C21) + the Copy fallback (D-18.5, risk (a)):**
//! the primary — and only — affordance is the `Set up RamSleuth`
//! button: a single unified action, a thin client over the pkexec-able
//! `ramsleuth-setup` root helper (one privileged pass: daemon
//! enable+start, group join, the socket ACL — and, on AMD/Intel
//! silicon, the offline DKMS driver build + `modprobe`; the helper
//! routes `--with-dkms` by the host's CPU vendor; no re-login, no
//! reboot, no app restart — the poller auto-reconnects to the
//! (re)started daemon and the strip unmounts on its own). The render
//! thread only flips [`SetupOutcome::running`] (D6: zero I/O on the
//! render thread — the actual `pkexec` spawn is the setup worker's
//! job, C21-06). The per-row **Copy** buttons are KEPT as the
//! secondary polkit-less fallback (the D-18.5 grace line for the
//! `polkit`/`acl`-less edge; the clipboard path — no terminal spawn,
//! no `sudo` shell-out).
//!
//! **No-panic contract (plan D5):** [`diagnose`] is pure and total — a
//! daemon-less [`TelemetryData::default()`] yields the single daemon
//! requirement, every input degrades, and neither [`diagnose`] nor
//! [`render_requirements_strip`] panics (no `unwrap` / `expect` /
//! `panic!` in the production paths).
//!
//! **Liveness trigger:** [`requirements_strip_visible`] is the SETUP
//! strip's visibility decision — the strip shows while the header's
//! `Setup` toggle is open AND [`diagnose`] reports a requirement.
//! Case 1 fires on liveness alone (`daemon_status` not `connected*` —
//! never polled, the daemon down, or a groupless client's refused
//! connect), so a daemon-less launch shows the one-click setup prompt
//! (a fresh install, a stopped daemon, or a leftover-partial install
//! all converge on it), and the strip disappears on its own the moment
//! the daemon serves — even when the served telemetry is all `N/A`
//! (unsupported hardware is a healthy state, not a setup prompt).
//!
//! **Wiring:** C18-11 declares this module + the root re-exports, and
//! C18-02 consumes it (the header's `Setup` toggle + the auto-shown
//! strip between the header and the settings area — presence-driven:
//! it disappears on its own once every requirement is resolved).
//! C21-04 adds the one-click setup wizard ([`setup_argv`] +
//! [`SetupOutcome`] + [`setup_with_dkms`] (the host-vendor decision
//! for the unified `--with-dkms` flag) +
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

/// The manual/CLI fallback command for the Intel driver (the command
/// for the Intel DKMS requirement — the secondary polkit-less path,
/// mirror of the AMD [`DKMS_INSTALL_CMD`]).
pub const DKMS_INSTALL_CMD_INTEL: &str = "sudo ramsleuth-install-intel-dkms";

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

/// Case 2 — a groupless client: join the `ramsleuth` group (the
/// one-click setup also applies a per-user ACL, so the current
/// session connects immediately — no re-login; the manual command
/// alone activates at re-login).
fn group_requirement() -> Requirement {
    Requirement {
        summary: "Join the `ramsleuth` group".to_owned(),
        detail: "the daemon socket is group-gated — the one-click setup also applies a per-user ACL, so the current session connects immediately (no re-login; the manual command alone activates at re-login)"
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

/// Case 4 — the Intel `ramsleuth_intel` driver is missing: install the
/// DKMS module (mirror of the AMD [`dkms_requirement`]; the helper
/// builds the bundled `ramsleuth_intel` source).
fn intel_dkms_requirement() -> Requirement {
    Requirement {
        summary: "Install the `ramsleuth_intel` kernel module (live Intel subtimings)".to_owned(),
        detail: "the helper builds the bundled `ramsleuth_intel` DKMS module — shown + confirmed before any build; RamSleuth runs without it".to_owned(),
        command: Some(DKMS_INSTALL_CMD_INTEL.to_owned()),
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
    // Case 4: Intel silicon with the daemon connected and the
    // `ramsleuth_intel` driver missing (the Intel branch is
    // `Na(DriverMissing)` — the module is absent and the `/dev/mem`
    // fallback is blocked); the mirror of the AMD case.
    if data.daemon_status.starts_with("connected") {
        if let Some(telemetry) = &data.telemetry {
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

/// The SETUP strip's visibility decision (the liveness-based trigger):
/// the strip shows while the header's `Setup` toggle is open
/// (`requirements_open`) AND [`diagnose`] reports at least one
/// requirement. Because case 1 fires on liveness alone
/// (`daemon_status` not `connected*` — never polled, the daemon down,
/// or a groupless client whose connect the socket refuses), this
/// predicate is what makes a daemon-less launch show the one-click
/// setup prompt: a fresh install (no group, no daemon), a stopped
/// daemon, and a leftover-partial install (the group / the
/// authorized-users file present from a prior run, the daemon down)
/// all converge on the daemon-down requirement. The moment the daemon
/// serves, case 1 clears and — with no other requirement — the
/// predicate is false, even when the served telemetry is all `N/A`
/// (an unsupported part reports `N/A (UnsupportedHardware)`, a
/// healthy state, not a setup case). Pure + total (D5); the app
/// shell's per-frame allocation decision is its only caller.
pub fn requirements_strip_visible(requirements_open: bool, data: &TelemetryData) -> bool {
    requirements_open && !diagnose(data).is_empty()
}

/// The one-click setup helper's fixed argv (the frozen C21-01
/// contract: `ramsleuth-setup [--with-dkms] [--user <name>]` — any
/// flag order). Pure + headless-testable, zero I/O: the render thread
/// never spawns anything (D6) — the setup worker wired in C21-06 runs
/// this under `pkexec`. Under `pkexec` the caller must pass `--user`
/// with the current user's name (`SUDO_USER` may be unset).
/// `--with-dkms` is the vendor-aware flag (the helper routes it by CPU
/// vendor: AMD → `ryzen_smu`, Intel → `ramsleuth_intel`) and is passed
/// whenever the host is AMD or Intel silicon (the unified-action
/// decision, [`setup_with_dkms`]); other/unknown silicon gets the
/// daemon + group + ACL pass only (the helper hard-fails on the flag
/// for a non-AMD/Intel vendor).
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
/// The strip's dim status line renders the five states: idle (the
/// default) / `running…` / `done — reconnecting to the daemon…` /
/// `one step left: <msg>` (the Secure Boot one-time MOK step — actionable
/// amber, not a failure) / `failed: <msg>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupOutcome {
    /// The setup edge: a button click flips this, and the per-tick
    /// edge consumer (the C21-06 worker, main.rs) clears it AT SPAWN
    /// of the detached `pkexec` helper — so the button re-enables
    /// while the helper runs, and a re-click spawns a second
    /// (idempotent) helper run.
    pub running: bool,
    /// The helper exited 0 — all requested setup steps succeeded or
    /// were no-ops. No app restart is needed: the helper's per-user
    /// ACL covers the current session (no re-login, no reboot), and
    /// the background poller gets a forced immediate re-poll (the
    /// state's `reconnect_requested` edge) — the strip unmounts on
    /// its own the moment the (re)started daemon serves.
    pub done: bool,
    /// The helper failed: the trailing diagnostic for the status line
    /// (`failed: <msg>`).
    pub failure: Option<String>,
    /// The helper exited with the Secure Boot one-time-step code (10):
    /// the driver is built + installed + signed; only the one-time MOK
    /// enrollment (reboot) is pending. Rendered as the actionable amber
    /// `one step left: <msg>` — NOT a failure (no `failed:`, no modal).
    pub secure_boot_pending: Option<String>,
}

/// Decide the `--with-dkms` flag of the unified one-click action from
/// the host's CPU vendor (NOT from the diagnosed requirement list —
/// that is what made the daemon-down first launch run the helper
/// without the driver and leave a second, driver-only stage behind):
/// true iff the host is AMD or Intel silicon — the snapshot's
/// `cpu.vendor` when it names one (the daemon's own CPUID detection),
/// else the unprivileged [`CpuInfo::detect()`] fallback (a daemon-down
/// first launch carries no snapshot). The single click then covers
/// daemon + group + ACL + the offline vendor driver in one privileged
/// pass (the helper routes `--with-dkms` by the CPU vendor: AMD →
/// `ryzen_smu`, Intel → `ramsleuth_intel`); other / unknown silicon
/// gets the daemon + group + ACL pass only (the helper hard-fails on
/// the flag for a non-AMD/Intel vendor).
pub fn setup_with_dkms(data: &TelemetryData) -> bool {
    let vendor = match &data.telemetry {
        Some(telemetry) => host_vendor(telemetry),
        None => CpuInfo::detect().vendor,
    };
    matches!(vendor, CpuVendor::Amd(_) | CpuVendor::Intel(_))
}

/// Render the `SETUP` requirements strip with the one-click setup
/// wizard into `ui` (the C18-02 panel body + the C21-04 wizard): the
/// bold-CYAN title, the single primary CYAN **`Set up RamSleuth`**
/// button (one label over every requirement mix — the unified
/// action; the `--with-dkms` decision is the host-vendor one,
/// [`setup_with_dkms`]), its dim live status line (idle /
/// `running…` / `done — reconnecting to the daemon…` /
/// `failed: <msg>`), one row per requirement (an AMBER `!`, the
/// summary, the dim detail, the command with the **Copy** button —
/// `ui.ctx().copy_text`, the secondary polkit-less fallback, D-18.5),
/// the `Got it — keep using RamSleuth` button (it flips `*open` — the
/// render thread's one permitted write, no I/O, the D6 settings
/// precedent), and the dim no-panic footer.
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
            // The unified action: one label over every requirement
            // mix (the `--with-dkms` flag is the worker's vendor
            // decision, [`setup_with_dkms`] — not a second button).
            let label = "Set up RamSleuth";
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
                ("done — reconnecting to the daemon…".to_owned(), CYAN)
            } else if let Some(pending) = &setup.secure_boot_pending {
                (format!("one step left: {pending}"), AMBER)
            } else if let Some(failure) = &setup.failure {
                (format!("failed: {failure}"), AMBER)
            } else {
                (
                    "One click installs the driver and starts the daemon — no reboot needed."
                        .to_owned(),
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
    /// no-re-login ACL note in the detail).
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
            group.detail.contains("per-user ACL") && group.detail.contains("no re-login"),
            "the detail must promise the immediate-ACL access (no re-login): {}",
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

    /// (c') Intel silicon, the daemon connected, the Intel branch
    /// `Na(DriverMissing)` (the module is absent and the `/dev/mem`
    /// fallback is blocked) → exactly the Intel-DKMS requirement;
    /// non-`DriverMissing` Intel → none (the healthy case is covered
    /// by `diagnose_intel_healthy`).
    #[test]
    fn diagnose_intel_driver_missing() {
        let data = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable, // amd: N/A on Intel silicon
            NaReason::DriverMissing, // intel: module absent
        );
        let requirements = diagnose(&data);
        assert_eq!(
            requirements.len(),
            1,
            "a connected Intel driver-missing snapshot yields exactly the Intel DKMS requirement: {requirements:?}"
        );
        assert_eq!(
            requirements[0].summary,
            "Install the `ramsleuth_intel` kernel module (live Intel subtimings)"
        );
        assert_eq!(
            requirements[0].command.as_deref(),
            Some("sudo ramsleuth-install-intel-dkms")
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
    /// unset), `--with-dkms` present when the `with_dkms` flag is set
    /// (the vendor-aware flag — AMD or Intel `DriverMissing`).
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

    /// (h) The `--with-dkms` decision is the HOST-VENDOR one (the
    /// unified action): AMD or Intel silicon → the one click carries
    /// the flag (daemon + group + ACL + driver in a single pass); a
    /// daemon-down first launch on AMD silicon (the lilgoat case —
    /// the diagnosed list carries no DKMS case when the daemon is
    /// down) → the SAME click carries the flag, so no second,
    /// driver-only stage ever appears. (The unknown-vendor /
    /// no-snapshot arm falls back to the unprivileged
    /// `CpuInfo::detect()` — host-dependent, so it is not asserted
    /// here.)
    #[test]
    fn setup_with_dkms_is_the_vendor_decision() {
        // AMD silicon (the snapshot's vendor is authoritative).
        assert!(setup_with_dkms(&connected(
            CpuVendor::Amd(AmdZen::Zen3),
            NaReason::NotApplicable,
            NaReason::NotApplicable,
        )));
        // Intel silicon (the mirror).
        assert!(setup_with_dkms(&connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::NotApplicable,
            NaReason::DriverMissing,
        )));
        // The daemon-down first launch on AMD silicon (the lilgoat
        // case): the snapshot names the vendor, the daemon status is
        // not connected — the unified action must still carry
        // `--with-dkms` (one click from the very first launch).
        let first_launch = TelemetryData {
            telemetry: Some(snapshot(
                CpuVendor::Amd(AmdZen::Zen3),
                NaReason::NotApplicable,
                NaReason::NotApplicable,
            )),
            daemon_status: "disconnected".to_owned(),
            ..Default::default()
        };
        assert!(
            setup_with_dkms(&first_launch),
            "a daemon-down launch on AMD silicon must carry --with-dkms"
        );
    }

    /// (h') The unified one-click: exactly ONE setup action exists —
    /// the single `Set up RamSleuth` button whose argv is the
    /// polkit-authorized `pkexec /usr/bin/ramsleuth-setup`; the
    /// `--with-dkms` flag (vendor-decided) is part of that same
    /// single argv. There is no separate driver-only stage: the AMD /
    /// Intel `DriverMissing` requirement mix yields the same single
    /// action (with the flag), never a second one.
    #[test]
    fn setup_is_a_single_unified_action() {
        // The AMD `DriverMissing` mix (the old second stage's input).
        let data = connected(
            CpuVendor::Amd(AmdZen::Zen3),
            NaReason::DriverMissing,
            NaReason::NotApplicable,
        );
        assert!(setup_with_dkms(&data));
        assert_eq!(
            setup_argv(setup_with_dkms(&data), "alice"),
            vec![
                "/usr/bin/ramsleuth-setup".to_owned(),
                "--user".to_owned(),
                "alice".to_owned(),
                "--with-dkms".to_owned()
            ],
            "the DriverMissing mix must yield the single polkit-authorized action with --with-dkms"
        );
        // The daemon-down mix (the old first stage's input): on AMD
        // silicon the same single action (with the flag) — the two
        // stages are one.
        let first_launch = TelemetryData {
            telemetry: Some(snapshot(
                CpuVendor::Amd(AmdZen::Zen3),
                NaReason::NotApplicable,
                NaReason::NotApplicable,
            )),
            daemon_status: "disconnected".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            setup_argv(setup_with_dkms(&first_launch), "alice"),
            vec![
                "/usr/bin/ramsleuth-setup".to_owned(),
                "--user".to_owned(),
                "alice".to_owned(),
                "--with-dkms".to_owned()
            ],
            "the daemon-down launch must carry the driver in the same single action"
        );
    }

    /// (i) The wizard over the two-frame `ctx.run` idiom: the single
    /// unified `Set up RamSleuth` button paints (the same label over
    /// the DKMS requirement mix — no `+ AMD driver` variant), the
    /// truthful idle status line paints (no reboot), the click flips
    /// `SetupOutcome::running` (the render thread's one permitted
    /// write — the status line paints `running…` in the same frame),
    /// a `done` outcome paints the auto-reconnect line (no manual
    /// restart), and `Got it` still closes the strip (the wizard
    /// doesn't steal the existing affordance).
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

        // Frame 1: the layout — the unified primary button + the
        // truthful idle status line paint; no click leaves the
        // outcome idle.
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
            texts.contains(&"Set up RamSleuth"),
            "the unified primary button must paint: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("+ AMD driver") || t.contains("+ Intel driver")),
            "no driver-stage label variant may paint: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("no reboot needed")),
            "the idle status line must make the truthful no-reboot promise: {texts:?}"
        );

        // Frame 2: a click on the button's painted label flips
        // `running` (the render thread does zero I/O) and the same
        // frame paints `running…`.
        let pos = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Set up RamSleuth" => {
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

        // Frame 2b: a `done` outcome (the worker clears `running`
        // and sets `done`) paints the auto-reconnect line — no
        // manual restart, no modal (the C21-36 removal).
        setup.running = false;
        setup.done = true;
        let done_frame = ctx.run(frame_input(Vec::new()), |ctx| {
            show_strip(ctx, &requirement, &mut open, &mut setup)
        });
        let texts: Vec<&str> = done_frame
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
                .any(|t| t.contains("done — reconnecting to the daemon…")),
            "the done line must promise the auto-reconnect: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("restart")),
            "no restart instruction may paint: {texts:?}"
        );
        setup.done = false;

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

    /// (j) The wizard's unified label over the Intel `DriverMissing`
    /// mix: the SAME single `Set up RamSleuth` button paints (no
    /// `+ Intel driver` variant — the unified action; the AMD mirror
    /// is (i)).
    #[test]
    fn first_run_setup_wizard_unified_label_intel() {
        let requirement = intel_dkms_requirement(); // the Intel variant
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
        let first = ctx.run(frame_input(Vec::new()), |ctx| {
            show_strip(ctx, &requirement, &mut open, &mut setup)
        });
        let texts: Vec<&str> = first
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"Set up RamSleuth"),
            "the unified primary button must paint: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("+ Intel driver") || t.contains("+ AMD driver")),
            "no driver-stage label variant may paint: {texts:?}"
        );
    }

    /// (k) The Secure Boot one-step-left state: the strip's status
    /// line paints `one step left: <msg>` (the actionable amber state
    /// — NOT the `failed:` failure state, and `done` is false so no
    /// auto-reconnect edge fires): headless over the two-frame
    /// `ctx.run` idiom.
    #[test]
    fn first_run_secure_boot_pending_renders() {
        let requirement = dkms_requirement();
        let ctx = egui::Context::default();
        let mut open = true;
        let mut setup = SetupOutcome {
            secure_boot_pending: Some(
                "reboot, enroll the MOK key, then re-click Setup".to_owned(),
            ),
            ..Default::default()
        };
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
        let first = ctx.run(frame_input(Vec::new()), |ctx| {
            show_strip(ctx, &requirement, &mut open, &mut setup)
        });
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
                .any(|t| t.contains("one step left: reboot, enroll the MOK key")),
            "the one-step-left status line must paint: {texts:?}",
        );
        assert!(
            !texts.iter().any(|t| t.starts_with("failed:")),
            "a pending state must not paint the failure state: {texts:?}",
        );
        assert!(
            !setup.done,
            "a pending outcome is not done — no auto-reconnect edge may fire",
        );
        assert!(
            open,
            "no click must not close the strip",
        );
    }

    /// (l) The liveness trigger — the SETUP strip's visibility
    /// decision: the daemon is down (the poller's `disconnected`
    /// status + the `DaemonDown` error) and the toggle is open → the
    /// prompt shows (the stopped-daemon case).
    #[test]
    fn strip_visible_daemon_down_prompts() {
        let data = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            error: Some(
                "cannot connect to /run/ramsleuth/ramsleuth.sock: daemon not running?"
                    .to_owned(),
            ),
            ..Default::default()
        };
        assert!(
            requirements_strip_visible(true, &data),
            "daemon down + toggle open must show the setup prompt"
        );
    }

    /// (l') The liveness trigger — the never-polled first frame (the
    /// empty status of `TelemetryData::default()`) shows the prompt
    /// defensively before the startup baseline fetch resolves.
    #[test]
    fn strip_visible_never_polled_prompts() {
        assert!(
            requirements_strip_visible(true, &TelemetryData::default()),
            "a never-polled default state must show the setup prompt"
        );
    }

    /// (l'') The liveness trigger — the gmktec case: the daemon
    /// serves on unsupported Intel hardware (both vendor branches
    /// `Na(UnsupportedHardware)` — the N100's report) → zero
    /// requirements → NO prompt (a healthy state, not a setup case).
    #[test]
    fn strip_visible_daemon_up_unsupported_hardware_no_prompt() {
        let data = connected(
            CpuVendor::Intel(IntelGen::Skylake),
            NaReason::UnsupportedHardware,
            NaReason::UnsupportedHardware,
        );
        assert!(
            !requirements_strip_visible(true, &data),
            "a serving daemon must not prompt, even with all-N/A telemetry: {:?}",
            diagnose(&data)
        );
    }

    /// (l''') The liveness trigger — the user's explicit dismissal
    /// (`Got it — keep using RamSleuth` closed the strip) is respected
    /// while the daemon stays down: no prompt until the header's
    /// `Setup` toggle re-opens it.
    #[test]
    fn strip_visible_dismissed_daemon_down_no_prompt() {
        let data = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            ..Default::default()
        };
        assert!(
            !requirements_strip_visible(false, &data),
            "a dismissed strip must stay closed (the header toggle re-opens it)"
        );
    }

    /// (l'''') The liveness trigger — the leftover-partial-install
    /// case: the state (the group, the authorized-users file) is
    /// present from a prior run and the daemon serves, but THIS user's
    /// connect is refused (the socket is group-gated — the recorded
    /// permission error) → the prompt shows (the one-click setup
    /// heals the group membership + the socket ACL in one pass).
    #[test]
    fn strip_visible_permission_denied_prompts() {
        let data = TelemetryData {
            daemon_status: "disconnected".to_owned(),
            error: Some(
                "cannot connect to /run/ramsleuth/ramsleuth.sock: daemon not running? \
                 (last error: Permission denied (os error 13))"
                    .to_owned(),
            ),
            ..Default::default()
        };
        assert!(
            requirements_strip_visible(true, &data),
            "a groupless client's refused connect must show the setup prompt"
        );
    }
}
