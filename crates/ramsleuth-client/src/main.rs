//! ramsleuth-client — Shared IPC client for the ramsleuth daemon (Phase 3).
//!
//! Per the v2 roadmap this becomes a lightweight **library** consumed by both
//! the TUI and GUI frontends: connect to `/run/ramsleuth/ramsleuth.sock`,
//! handle timeouts/retries/deserialization, and emit friendly diagnostics
//! when the daemon is not running. A `[lib]` target is added then.

fn main() {
    // Scaffold stub: no behavior yet.
}
