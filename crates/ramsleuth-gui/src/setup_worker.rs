//! The one-click setup worker (C21-06 — the GUI face of the frozen
//! C21-01 helper contract): the wizard button's `running` edge
//! detaches a `pkexec` run of `ramsleuth-setup` (no shell — the argv
//! is [`setup_argv`]'s fixed vector; the mandatory `--user` is the
//! GUI's current user, explicit — under `pkexec` `SUDO_USER` may
//! be unset) entirely off the render thread (D6); the outcome lands
//! in the shared [`SetupOutcome`] with one brief write-lock hand-off
//! (C14-03), and every fallible step degrades to the `failure` state
//! (no-panic, plan D5 — risk 1: polkit is the GUI's convenience,
//! `sudo` is the floor).

use std::process::Command;
use std::sync::{Arc, RwLock};

use ramsleuth_gui::first_run::SetupOutcome;
use ramsleuth_gui::{setup_argv, TelemetryData};

/// The current user's name for the helper's mandatory `--user`
/// (the C21-01 contract: under `pkexec` `SUDO_USER` may be unset,
/// so the GUI passes it explicitly): `$USER`, else `$LOGNAME`;
/// `None` (an env with neither set) degrades to the manual `sudo`
/// pointer (no-panic, plan risk 1).
pub fn current_user() -> Option<String> {
    std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty())
        .or_else(|| {
            std::env::var("LOGNAME")
                .ok()
                .filter(|user| !user.is_empty())
        })
}

/// The manual `sudo` equivalent of the one click (plan risk 1:
/// polkit is the GUI's convenience, `sudo` is the floor — the
/// wizard's `failure` state points at it when `pkexec` is absent or
/// the spawn fails): pure, zero I/O.
pub fn sudo_setup_command(user: &str, with_dkms: bool) -> String {
    if with_dkms {
        format!("sudo ramsleuth-setup --user {user} --with-dkms")
    } else {
        format!("sudo ramsleuth-setup --user {user}")
    }
}

/// The trailing diagnostic for the setup status line: the FIRST
/// stderr line containing `ERROR:` — any helper tag, not just the
/// setup helper's own `[ramsleuth-setup] ERROR:`: the `pkexec
/// ramsleuth-setup` run is `exec`-replaced by the delegated vendor
/// helper (the `--with-dkms` path), and that helper dies with
/// `[ryzen-smu-dkms] ERROR:` / `[intel-dkms] ERROR:` — its line is
/// the structured reason, and a later unstructured line (a `dmesg`
/// tail, a depmod note) must never mask it. Else the last non-empty
/// stderr line, else the generic manual pointer (plan risk 1). The
/// process exit code is appended — one line, since the GUI shows a
/// single line. Pure, zero I/O.
pub const GENERIC_FAILURE_POINTER: &str =
    "the setup helper failed — run `sudo ramsleuth-setup --user <you>` in a terminal";

/// The durable, world-readable setup log (the setup helper tees every
/// line of every run to it; the non-root user can `cat` it): the known
/// location every setup failure message points at, so a failure on a
/// test machine is diagnosable off-band.
pub const SETUP_LOG_PATH: &str = "/var/lib/ramsleuth/setup.log";

pub fn setup_stderr_tail(stderr: &str, exit_code: i32) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let chosen = lines
        .iter()
        .find(|line| line.contains("ERROR:"))
        .or_else(|| lines.last())
        .map(|line| line.trim().to_owned())
        .unwrap_or_else(|| GENERIC_FAILURE_POINTER.to_owned());
    format!("{chosen} (exit {exit_code})")
}

/// Append the durable-log pointer as a trailing line to a setup failure
/// message: `Full log: <SETUP_LOG_PATH>` (the GUI's status label renders
/// the `\n` as a line break, and it wraps): the existing diagnostic +
/// exit code stay the first line. Pure, zero I/O.
pub fn with_setup_log_pointer(message: &str) -> String {
    format!("{message}\nFull log: {SETUP_LOG_PATH}")
}

/// The Secure Boot one-time-step pending code (the vendor helper's
/// distinct NON-fatal exit): the driver is built + installed +
/// signed; only the one-time MOK enrollment (reboot) is left.
/// Rendered as the actionable amber `one step left:` state — NOT a
/// `failed:` state.
pub const SECURE_BOOT_PENDING_EXIT: i32 = 10;

/// The built-in one-time-step guidance, rendered when the helper
/// exits `SECURE_BOOT_PENDING_EXIT` without its own usable stderr
/// line.
pub const SECURE_BOOT_PENDING_GUIDANCE: &str = "Secure Boot is on — the driver is built + installed + signed. ONE-TIME: reboot, at the blue MOK screen choose 'Enroll MOK key(s)' → 'Continue' → 'Yes', then re-click Setup (or disable Secure Boot in UEFI and re-click).";

/// Map the helper's exit code into the [`SetupOutcome`] the status
/// line renders (the frozen C21-01 contract: 0 = success/no-op,
/// 1 = hard failure with the [`setup_stderr_tail`], 2 = usage —
/// should not happen via the GUI; 10 = the Secure Boot one-time
/// step pending — actionable amber, not a failure; any other value,
/// including the worker's `-1` for a signal-killed helper, is a hard
/// failure too): pure, zero I/O — the worker is its only caller.
pub fn map_setup_exit(code: i32, stderr: &str) -> SetupOutcome {
    // The Secure Boot one-time step (exit 10): build + install
    // succeeded; only the one-time MOK enrollment (reboot) is
    // pending. The helper's own single-line reason wins; without one
    // (empty stderr), the built-in guidance.
    if code == SECURE_BOOT_PENDING_EXIT {
        let tail = setup_stderr_tail(stderr, code);
        let message = if tail.starts_with(GENERIC_FAILURE_POINTER) {
            SECURE_BOOT_PENDING_GUIDANCE.to_owned()
        } else {
            tail
        };
        return SetupOutcome {
            running: false,
            done: false,
            failure: None,
            secure_boot_pending: Some(message),
        };
    }
    let failure = match code {
        0 => None,
        2 => Some(with_setup_log_pointer(
            "the setup helper rejected its arguments (usage, exit 2 — this should \
              not happen via the GUI) — run `sudo ramsleuth-setup --user <you>` in a \
              terminal",
        )),
        // Exit 1 (and any other non-zero code, incl. `-1`): the
        // structured stderr tail + the durable-log pointer.
        _ => Some(with_setup_log_pointer(&setup_stderr_tail(stderr, code))),
    };
    SetupOutcome {
        running: false,
        done: code == 0,
        failure,
        ..Default::default()
    }
}

/// The pkexec spawn-failure message (a missing `pkexec` / a fork
/// failure — plan risk 1: polkit is the GUI's convenience, `sudo` is
/// the floor): the manual `sudo` command + the durable-log pointer.
/// Pure, zero I/O (the worker is its only caller).
pub fn pkexec_spawn_failure_message(error: &std::io::Error, user: &str, with_dkms: bool) -> String {
    with_setup_log_pointer(&format!(
        "`pkexec` could not run the setup helper ({error}) — run `{}` \
         manually (sudo is the floor, plan risk 1)",
        sudo_setup_command(user, with_dkms)
    ))
}

/// The detached setup worker (C21-06; the [`spawn_poller`]'s
/// `thread::spawn` precedent): runs the frozen C21-01 helper under
/// `pkexec` entirely off the render thread, then maps the result
/// into the shared [`SetupOutcome`] with one brief write-lock
/// hand-off (C14-03 — no lock is held across the spawn: the
/// `pkexec` run is the whole job, and it may take minutes on the
/// AMD DKMS build). No-panic (plan D5): a spawn failure (a missing
/// `pkexec` — plan risk 1: polkit is the GUI's convenience,
/// `sudo` is the floor) degrades to the `failure` state pointing at
/// the manual `sudo ramsleuth-setup` command.
pub fn spawn_setup_worker(
    setup: Arc<RwLock<SetupOutcome>>,
    state: Arc<RwLock<TelemetryData>>,
    with_dkms: bool,
    user: String,
) {
    std::thread::spawn(move || {
        let argv = setup_argv(with_dkms, &user);
        let outcome = match Command::new("pkexec").args(&argv).output() {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                map_setup_exit(output.status.code().unwrap_or(-1), &stderr)
            }
            Err(error) => SetupOutcome {
                running: false,
                done: false,
                failure: Some(pkexec_spawn_failure_message(&error, &user, with_dkms)),
                secure_boot_pending: None,
            },
        };
        // The auto-reconnect edge (the C21-36 removal): a successful
        // run (re)started the daemon + applied the per-user ACL —
        // force one immediate poll in the background poller
        // (independent of the refresh gate) so the app picks up the
        // now-serving daemon without a manual restart; the strip
        // unmounts on its own the moment it connects.
        if outcome.done {
            crate::state_write(&state).reconnect_requested = true;
        }
        // The one brief write-lock hand-off (the sanctioned D6
        // idiom — held only for the assignment, never across the
        // spawn).
        *crate::state_write(&setup) = outcome;
    });
}
