//! ramsleuth-daemon — the privileged ramsleuth service (Phase 3).
//!
//! The daemon is the **only** process in the workspace that may hold
//! `CAP_SYS_RAWIO` (the SMU/MCHBAR hardware handles) and run the
//! Unix-socket RPC server the unprivileged clients (CLI, TUI, GUI)
//! talk to. Plan D5 makes its privilege handling **SOFT**: missing
//! root or capabilities never panic, never exit, and never refuse to
//! serve — the daemon degrades gracefully, privileged telemetry fields
//! report `N/A (<reason>)` exactly as the Phase 2 providers already
//! do, and warnings are emitted at the call site (main, P3-17).
//!
//! Modules land one per chunk (plan §3):
//!
//! - `caps` — P3-12 — the SOFT privilege probe (this chunk).
//! - `socket` — P3-13 — listener setup (dir, stale-socket probe,
//!   bind, mode `0660`, best-effort chown).
//! - `cache` — P3-14 — the TTL lazy telemetry cache over `collect()`.
//! - `bench_job` — P3-15 — single-flight benchmark runs over
//!   `run_streamed`.
//! - `rpc` — P3-16 — per-connection frame dispatch + progress
//!   forwarding.
//!
//! The binary entry (`src/main.rs`) is rewritten in P3-17.

pub mod caps;

// P3-12: the SOFT privilege probe, re-exported at the root (workspace
// re-export style) — the daemon's "warn, keep serving" contract
// (plan D5). P3-17 (main) calls `probe()` and emits its warnings to
// stderr.
pub use caps::{probe, PrivilegeReport};
