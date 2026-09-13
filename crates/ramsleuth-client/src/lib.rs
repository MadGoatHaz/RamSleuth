//! ramsleuth-client — the unprivileged shared IPC client (Phase 3).
//!
//! This is the **library** every unprivileged frontend consumes (CLI in
//! P3-21, TUI in P3-22+, GUI in P3-25+): it connects to the daemon's
//! Unix socket (default `ramsleuth_protocol::DEFAULT_SOCKET_PATH`),
//! speaks the frozen P3-10/P3-11 frame protocol over a synchronous
//! `std::os::unix::net::UnixStream` (plan D2: no tokio — the daemon is
//! the workspace's only tokio consumer), and degrades to **friendly,
//! actionable diagnostics** instead of panics when the daemon is down
//! (the client's no-panic contract, plan D5).
//!
//! Modules land one per chunk (plan §3, group D):
//!
//! - `client` — P3-18 — the synchronous RPC transport: connect with
//!   timeout + retries, send/recv frames, one round-trip `request`,
//!   structured `ClientError` diagnostics (daemon-down hint, timeouts,
//!   protocol violations).
//! - `dump` — P3-19 — the `dump` command: the pure dashboard-style
//!   `render` over a `SystemMemoryTelemetry` snapshot (every cell prints
//!   its value or `N/A (<reason>)`, never a panic) + the one-RPC `dump`
//!   (`GetTelemetry` → render → stdout).
//!
//! `commands` (P3-20) and the rewritten binary entry (`main.rs`, P3-21)
//! build on the transport this crate exposes.

pub mod client;
pub mod dump;

// P3-18: the synchronous RPC transport, re-exported at the root
// (workspace re-export style) — P3-19 (`dump`), P3-20 (`bench`/`status`)
// and P3-21 (bin) all call `Client::connect` + `request`; the TUI/GUI
// frontends (P3-22+/P3-25+) consume the same `Client` via this library.
pub use client::{Client, ClientError};

// P3-19: the dump command, re-exported at the root (workspace
// re-export style) — P3-21 (bin) calls `dump::dump` on its connected
// `Client`, and the TUI snapshot export (P3-24) + the GUI (P3-25) reuse
// the pure `render` (the module `dump` and the function `dump` coexist:
// different namespaces).
pub use dump::{dump, render};
