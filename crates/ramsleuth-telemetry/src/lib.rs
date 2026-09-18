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
pub mod amd_smu; // P2-03 — privilege-guarded AMD SMU access (sysfs pm_table -> char-dev read).
pub mod amd_pm; // P2-04 — version-guarded, bounds-checked AMD PM-table parse (AmdPmSnapshot).
pub mod amd_readout; // P2-05 — shared display types + AMD mapping (sanity-gated, no I/O).
pub mod amd_smn; // P6-02 — ryzen_smu `smn` sysfs accessor + verified SMN bitfield table + no-panic overlay.
pub mod intel_mchbar; // P2-06 — Intel MCHBAR PCI decode + read-only /dev/mem mmap guard.
pub mod intel_readout; // P2-07 — Intel per-channel IMC register decode -> shared display types.
pub mod spd_eeprom; // P2-08 — ee1004 raw SPD image acquisition (sysfs, unprivileged).
pub mod spd_decode; // P2-09 — pure SPD decode: JEP106 / rank / density / speed + XMP 2.0 / EXPO profiles.
pub mod facade; // P2-10 — SystemMemoryTelemetry facade + collect() (per-branch containment).
pub mod platform; // C6-01 — SystemPlatform: DMI + /proc sourced vendor-neutral branch (D-C1; per-field Na degradation).
pub mod board_vrm; // C12-03 — DMI-keyed board VRM profiles + the nct6798 hwmon binder (board-specific Super I/O rails; graceful all-Na on unknown board).

// P2-10: the crate's public snapshot API, re-exported at the root.
pub use facade::{collect, SystemMemoryTelemetry};
// C6-07: the C6-01 platform types, re-exported at the root.
pub use platform::{SystemPlatform, mem_total_gib};
