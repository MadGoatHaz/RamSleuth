//! ramsleuth-telemetry — Hardware telemetry & register extraction.
//!
//! Phase 2 scaffold (library target). Will contain three providers:
//!
//! - **AMD Zen 1–5**: `ryzen_smu` PM-table parsing (clocks, subtimings,
//!   drive strengths, voltages).
//! - **Intel 6th–15th gen**: MCHBAR MMIO register decoding via `/dev/mem`.
//! - **SPD EEPROM**: `ee1004` raw-block parsing (JEP106 makers, XMP/EXPO).
//!
//! See `plans/PLAN-PHASE2.md` for the Phase 2 chunk decomposition.

// Phase 2 modules land one per chunk (wiring only, not counted against the
// per-chunk line budget). Each is `pub mod` so its frozen interface is
// reachable (`crate::cpuid::CpuInfo`, …); P2-10 adds root re-exports.
pub mod cpuid; // P2-01 — CPUID vendor + family/generation detection (interface freeze).
pub mod error; // P2-02 — Telemetry error + Section<T> no-panic contract (interface freeze).
