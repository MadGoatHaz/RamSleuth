//! The consent-gated "Submit Probe Report" flow (chunk probe-3): the
//! modal dialog bodies, the header's per-frame action, the state
//! machine, and the GitHub issue URL helpers.

use ramsleuth_telemetry::ProbeReport;

/** The consent dialog's body (chunk probe-3): the exact disclosure of
what the probe report collects + the explicit no-personal-information
note. */
pub const PROBE_CONSENT_BODY: &str = "RamSleuth can gather the following system information to help the developers improve the app:\n\n\u{2022} CPU brand, detected generation, PCI host-bridge ID\n\u{2022} Kernel version, OS, architecture\n\u{2022} RamSleuth version, telemetry source\n\u{2022} The decoded memory readout (MCLK, MT/s, timings, SPD)\n\u{2022} The raw IMC register values\n\u{2022} N/A reasons\n\n**Not collected:** username, hostname, IP address, MAC address, serial numbers, file paths.\n\nDo you allow RamSleuth to gather this information?";

/// The header's transient notice after [Open GitHub Issue] (chunk
/// probe-3): the full report is in the clipboard — the user pastes
/// it into the issue body (the body does not ride the URL — see
/// [`probe_issue_url`]).
pub const PROBE_ISSUE_COPY_NOTICE: &str =
    "Report copied to clipboard — paste it into the issue body.";

/// The header's per-frame action (the header buttons that need
/// app-level dispatch): `None` when nothing was clicked this frame,
/// `Probe` when the "Probe" button was clicked (chunk probe-3 — the
/// app shell opens the consent flow), `About` when the "About"
/// button was clicked (chunk gui-about — the app shell opens the
/// About/Help modal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    None,
    Probe,
    About,
}

/// The consent-gated "Submit Probe Report" flow state (chunk probe-3):
/// the lifecycle of the two modal dialogs. There is no terminal state
/// — every closing path ([Cancel] in either dialog, [Open GitHub
/// Issue], [Copy to Clipboard], Esc) returns to `Idle`, so the
/// header's "Probe" button is always re-openable.
///
/// - `Idle` — no probe flow in progress (the default).
/// - `Consent` — the consent dialog is open (the "Probe" button was
///   clicked; [Allow] hands the request to the poller, [Cancel] /
///   Esc return here).
/// - `Preview(md)` — the rendered report markdown is shown in the
///   preview dialog ([Open GitHub Issue] / [Copy to Clipboard] /
///   [Cancel] / Esc all return to `Idle`).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProbeState {
    #[default]
    Idle,
    Consent,
    Preview(String),
}

/// One user event on the probe-flow state machine (chunk probe-3):
/// the pure transitions the consent / preview dialog buttons, the
/// modal's Esc dismiss, and the header's "Probe" button apply. Every
/// closing event returns to `Idle` (the machine has no terminal
/// state — the flow is always re-openable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeEvent {
    /// The header's "Probe" button (Idle → Consent).
    Open,
    /// The consent dialog's [Cancel] / Esc (Consent → Idle).
    CancelConsent,
    /// The preview dialog's [Cancel] / Esc (Preview → Idle).
    CancelPreview,
    /// The preview dialog's [Copy to Clipboard] (Preview → Idle).
    Copy,
    /// The preview dialog's [Open GitHub Issue] (Preview → Idle).
    Submit,
}

impl ProbeState {
    /// Apply one user event (pure): `Open` (Idle → Consent),
    /// `CancelConsent` (Consent → Idle), and the preview dialog's
    /// closers `CancelPreview` / `Copy` / `Submit` (Preview → Idle).
    /// Every closing path resets the flow to `Idle` (the machine has
    /// no terminal state — the header's Probe button is always
    /// re-openable). An event outside its expected state is a no-op
    /// (the state machine is total — never a panic).
    pub fn apply(&mut self, event: ProbeEvent) {
        let this = &*self;
        match (this, event) {
            (ProbeState::Idle, ProbeEvent::Open) => *self = ProbeState::Consent,
            (ProbeState::Consent, ProbeEvent::CancelConsent) => *self = ProbeState::Idle,
            (ProbeState::Preview(_), ProbeEvent::CancelPreview) => *self = ProbeState::Idle,
            (ProbeState::Preview(_), ProbeEvent::Copy) => *self = ProbeState::Idle,
            (ProbeState::Preview(_), ProbeEvent::Submit) => *self = ProbeState::Idle,
            _ => {}
        }
    }
}

/// The pre-filled GitHub issue title for a probe report (chunk
/// probe-3): `[Probe] <cpu_brand> · <cpu_gen> · <os> · v<version>` —
/// the issue template's `[Probe] ` prefix + the report's CPU / OS
/// identity + the generating version.
pub fn probe_issue_title(report: &ProbeReport) -> String {
    let s = &report.system;
    format!(
        "[Probe] {} \u{00b7} {} \u{00b7} {} \u{00b7} v{}",
        s.cpu_brand, s.cpu_gen, s.os, s.ramsleuth_version
    )
}

/// The pre-filled GitHub issue URL (chunk probe-3): the repo's
/// `issues/new` form with the title only, percent-encoded (RFC 3986).
/// The markdown body deliberately does NOT ride the URL — a full
/// report (4–5 KB raw, 8–12 KB percent-encoded) exceeds GitHub's
/// issue-form length limit (the 404); the clipboard carries it
/// instead (the "Open GitHub Issue" click copies it and flashes the
/// paste-into-the-body notice in the header).
pub fn probe_issue_url(title: &str) -> String {
    format!(
        "https://github.com/MadGoatHaz/RamSleuth/issues/new?title={}",
        url_encode(title)
    )
}

/// Percent-encode `s` per RFC 3986: the unreserved set (`A-Z a-z 0-9
/// - _ . ~`) passes through; every other byte becomes `%XX` (upper-
/// case hex). A space encodes as `%20` (never `+`).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(to_hex(byte >> 4) as char);
                out.push(to_hex(byte & 0x0f) as char);
            }
        }
    }
    out
}

/// One nibble as an upper-case hex digit (the [`url_encode`] helper).
fn to_hex(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'A' + (n - 10),
    }
}
