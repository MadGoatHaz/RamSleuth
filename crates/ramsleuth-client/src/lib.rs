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
//! - `commands` — P3-20 — the `bench` + `status` + `probe` + `burn`
//!   commands: `bench` streams a benchmark run (a `BenchStarted` ack,
//!   one progress line per completed cell, the terminal 4×4 grid
//!   through the pure `render_grid`); `status` is the one-RPC
//!   per-section summary; `probe` fetches the consent-gated probe
//!   report (`GetProbeReport` → the chunk-2 markdown to
//!   `~/.ramsleuth/probe-report.md` / stdout / raw JSON) and `burn`
//!   starts the daemon's burn-in soak (`StartBurnIn`, ack + stop
//!   guidance, then exits). All print their output and return the same
//!   text.
//! - `dump` — P3-19 — the `dump` command: the pure dashboard-style
//!   `render` over a `SystemMemoryTelemetry` snapshot (every cell prints
//!   its value or `N/A (<reason>)`, never a panic) + the one-RPC `dump`
//!   (`GetTelemetry` → render → stdout).
//!
//! The rewritten binary entry (`main.rs`, P3-21) dispatches `dump` /
//! `bench` / `status` / `probe` / `burn` over the transport this crate
//! exposes.

pub mod client;
pub mod commands;
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

// P3-20: the bench + status commands, re-exported at the root (the
// workspace re-export style) — P3-21 (bin) calls `bench` / `status` on
// its connected `Client` and `render_grid` for any grid export; the
// returned `String` is exactly the text printed (the unit tests assert
// on it, the bin may ignore it).
pub use commands::{bench, render_grid, status};

// The TUI `[F]` / `[X]` + GUI parity commands: `probe` (the
// consent-gated probe report — `GetProbeReport` → the chunk-2 markdown
// to `~/.ramsleuth/probe-report.md`, `--stdout`, or the raw JSON;
// `probe_to` + `default_probe_report_path` carry the injectable
// destination the unit tests drive) and `burn` (the daemon's burn-in
// soak — `StartBurnIn { Full, minutes }` → the ack + stop guidance).
pub use commands::{burn, default_probe_report_path, probe, probe_to};
