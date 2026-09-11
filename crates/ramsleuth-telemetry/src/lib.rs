//! ramsleuth-telemetry — Hardware telemetry & register extraction.
//!
//! Phase 2 scaffold (library target). Will contain three providers:
//!
//! - **AMD Zen 1–5**: `ryzen_smu` PM-table parsing (clocks, subtimings,
//!   drive strengths, voltages).
//! - **Intel 6th–15th gen**: MCHBAR MMIO register decoding via `/dev/mem`.
//! - **SPD EEPROM**: `ee1004` raw-block parsing (JEP106 makers, XMP/EXPO).
//!
//! See `plans/PLAN.md` for the Phase 2 chunk decomposition.

// Intentionally minimal at scaffold stage — providers land in Phase 2.
