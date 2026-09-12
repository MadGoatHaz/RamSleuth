# RamSleuth v2 — Master Log

Durable per-cycle compaction of `DEV_LOG.md`. Newest cycle first.

## RamSleuth v2 — Cycle 2 (Phase 2: Live Memory Controller Telemetry) — 2026-09-12

### What was delivered
`ramsleuth-telemetry` complete (11 chunks, P2-01…P2-11):
- CPUID vendor / Zen-family detection (interface freeze) — P2-01.
- Error + `Section<T>` no-panic contract (interface freeze) — P2-02.
- Privilege-guarded AMD SMU access (sysfs→char-dev, sole `nix` dep) — P2-03.
- Version-guarded bounds-checked AMD PM parse (SMU 7.11.x / 12.x / 13.x) — P2-04.
- Shared display types + AMD mapping (sanity-gated) — P2-05.
- Intel MCHBAR acquire + read-only /dev/mem mmap guard (Intel-gated; STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege; volatile MMIO read) — P2-06.
- Intel per-channel IMC decode — P2-07.
- Unprivileged SPD EEPROM acquisition — P2-08.
- SPD decode (JEP106, rank, XMP/EXPO) — P2-09.
- `SystemMemoryTelemetry` facade + `collect()` (per-branch error containment) — P2-10.
- Verification CLI (`cargo run -p ramsleuth-telemetry [--json]`) — P2-11.

### Quality
- 159/159 tests green (debug + release, whole workspace); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live telemetry CLI exit 0 (text + `--json`).
- QA audit 2026-09-12 on `v2-development` @ 367bef9: all 8 runnable gates PASS.

### Live result (this host: Zen 3 / DDR4, `ryzen_smu` module absent)
- `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD: N/A (DriverMissing)`; `Intel: N/A (UnsupportedHardware)`; SPD 2× DDR4 modules (rank=1, speed=3200 MT/s, maker `0xC1` raw-hex, density `0x0D` → Na). No panic in any privilege/CPU state.

### Key decisions
- Direct SMU access (no `ryzen_smu` crate): sysfs-first + char-dev read; `nix` (features `fs`/`ioctl`/`mman`) is the only new dependency of Phase 2.
- AMD PM + Intel IMC byte offsets are plan-mandated SKELETONS pending live-silicon reconciliation.

### BLOCKED acceptance gates (hardware/driver, not code)
1. AMD tick-identical ground truth — needs `ryzen_smu` module loaded + root.
2. Intel live MCHBAR decode — needs Intel silicon.
3. Model reconciliation — AMD PM byte offsets, Intel IMC offsets, SPD maker `0xC1` + density `0x0D` codes.

### Per-chunk history (summarized from DEV_LOG.md)
- P2-01: CPUID vendor/Zen-family detection (interface freeze).
- P2-02: `TelemetryError` + `Section<T>` no-panic contract (interface freeze); 14/14 tests.
- P2-03: AMD SMU acquire (sysfs→char-dev; `nix` 0.29 sole new dep); 20/20; live → `DriverMissing{ryzen_smu}`.
- P2-04: version-guarded bounds-checked AMD PM parse; 29/29; offsets = plan skeleton.
- P2-05: shared display types + sanity-gated AMD mapping; 39/39.
- P2-06: Intel MCHBAR + read-only /dev/mem guard; 48/48. *Process note:* review FAILED (F1 STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege, F2 volatile read) → fix `85114a6` → re-review PASS.
- P2-07: Intel per-channel IMC decode; 66/66; offsets = documented model/skeleton.
- P2-08: unprivileged SPD EEPROM acquire (world-readable sysfs `eeprom`); 74/74; live → 2× 512B DDR4.
- P2-09: SPD decode (JEP106, rank, XMP/EXPO); 85/85. *Process note:* recovered prior subagent's uncommitted/empty payload (3 compile fixes + missing 11-test module).
- P2-10: `SystemMemoryTelemetry` facade + `collect()` (per-branch containment); 90/90.
- P2-11: verification CLI (hand-rolled JSON, no serde/clap); 96/96. *Process note:* recovered prior subagent's empty payload.
- phase2-qa: full audit 2026-09-12 — 8/8 runnable gates PASS; 159/159 tests.

Cycle 2 close-out (2026-09-12): all 11 `branch/chunk-P2-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.

## RamSleuth v2 — Cycle 1 (Phase 1: Native Benchmark Engine) — 2026-09-11

### Delivered
- 6-crate Cargo workspace scaffolded (P1-01); runtime AVX2/AVX-512F feature detection (P1-02, `detect() -> CpuTopology`).
- `ramsleuth-bench` benchmark engine complete:
  - /sys physical-core enumeration with SMT filter + total/per-CCD L3 (P1-02).
  - L1/L2/L3/DRAM buffer sizing via pure `plan(&CpuTopology) -> BufferPlan`, all 64-byte aligned: sysfs L1/L2 with safe fallbacks, per-CCD L3 slice, DRAM max(256 MiB, 3× total L3), 128 MiB latency ring (P1-03).
  - AVX2 streaming read kernel — unrolled 4-wide aligned loads, lane accumulation, word-sum checksum, feature-dispatched scalar fallback (P1-04); AVX2 non-temporal write kernel — NT stores + single trailing SFENCE (P1-05); AVX2 copy kernel — load/NT-store pairs, destination checksum via avx2_read (P1-06).
  - AVX-512F read/write/copy variants (`_mm512_*` aligned/NT/SFENCE shapes) with documented AVX2 fallback when AVX-512F is absent; clippy msrv 1.89 gating (P1-07).
  - Pinned barrier-synced multi-thread worker dispatch — one `sched_setaffinity`-pinned worker per physical core, Barrier lockstep, exact-once block-aligned partition, checksum aggregation, graceful unpinned fallback; libc 0.2 the only new dep (P1-08).
  - 64-byte-stride pointer-chase latency kernel — SplitMix64 Fisher-Yates single-cycle ring, strictly dependent loads, serialized `__rdtscp` with self-calibrating Instant conversion + non-x86 fallback (P1-09).
  - Orchestrator aggregating the 4×4 AIDA64-style grid (Memory/L3/L2/L1 × Read/Write/Copy + Latency); DRAM-tier latency chases the materialized full-DRAM-size buffer (beyond L3); safe 64B-aligned `AlignedBuf`; best-of-3 Instant timing owned by the orchestrator (P1-10).
  - Verification CLI: `cargo run -p ramsleuth-bench [--avx512] [--json]` — std-only option parsing (unknown flag → exit 2), serde-free JSON (`read_gbps/write_gbps/copy_gbps/latency_ns`, non-finite → null), fixed-width grid renderer, graceful exit 1 on detect/run errors, documented AVX-512→AVX2 fallback note (P1-11).

### Quality
- 63/63 tests green (debug + release); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live bench run exit 0 (default, `--json` output parsed, `--avx512` fallback path verified).
- QA audit 2026-09-12 on `v2-development` @ b5eea35: 7/7 runnable gates PASS; AIDA64 parity gate deferred to the DDR5-6000 AM5 host (this host is Zen 3 / DDR4).

### Measured grid (this host: Ryzen 9 5950X, Zen 3, 16 phys / 32 logical, 64 MiB L3, DDR4, AVX2-only — informational)
| Tier | Read (GB/s) | Write (GB/s) | Copy (GB/s) | Latency (ns) |
| --- | --- | --- | --- | --- |
| Memory (DRAM) | 49.13 | 43.83 | 15.75 | 81.5 |
| L3 | 59.03 | 31.28 | 16.29 | 56.5 |
| L2 | 1.33 | 1.41 | 1.40 | 5.6 |
| L1 | 0.08 | 0.09 | 0.09 | 1.2 |

Repeat runs (variance context): `--json` → DRAM 47.58 / 43.37 / 15.68 GB/s @ 81.4 ns; L3 63.04 / 32.02 / 16.88 GB/s @ 39.5 ns. `--avx512` → DRAM 48.89 / 43.48 / 15.67 GB/s @ 82.9 ns. L3 drifts ~±25% run-to-run (best-of-3 at these sizes is noisy). Sanity: latency ordering monotonic L1 < L2 < L3 < DRAM; no negative/zero/NaN cells; DRAM read ≈ 48% of DDR4-3200 4-channel theoretical peak.

### Known follow-ups
1. L1/L2 bandwidth cells are overhead-limited: the small tier working set (32 KiB L1d / 1 MiB L2) split across 16 pinned workers is dominated by thread/barrier/timing overhead per pass. Add inner-loop iterations to amortize that overhead for small tiers (Phase 2 design note; cells not comparable across tiers).
2. Confirm §1.3 AIDA64 parity (DRAM BW ±5%, latency 60–75 ns band) on the target DDR5-6000 AM5 machine — this host measured DRAM latency 81.4–83.5 ns (out of band as expected on DDR4; reported, not failed).
3. Workspace MSRV 1.75 vs AVX-512F intrinsics requiring Rust 1.89 — decide on an MSRV bump.
4. Push strategy for `v2-development` vs the divergent legacy upstream `master` is pending user decision (no push from this cycle).

### Per-chunk history (summarized from DEV_LOG.md)
- P1-01: runtime CPU feature detection module (merged f0cb26e).
- P1-02: `detect() -> Result<CpuTopology, TopologyError>` — /sys topology enumeration, SMT filter; 13/13 tests.
- P1-03: pure buffer planning/sizing, 64-byte aligned; 23/23 tests.
- P1-04: AVX2 streaming read + word-sum checksum; 27/27 tests.
- P1-05: AVX2 non-temporal write; 31/31 tests.
- P1-06: AVX2 copy (NT stores + SFENCE); 35/35 tests.
- P1-07: AVX-512F read/write/copy with AVX2 fallback; 40/40 tests; 512-bit bodies compiled but not directly exercised on this AVX2-only host.
- P1-08: pinned barrier-synced worker dispatch; 47/47 tests.
- P1-09: pointer-chase latency kernel; 53/53 tests; flagged that a true DRAM row requires the chased working set to exceed L3 (resolved in P1-10).
- P1-10: orchestrator + 4×4 grid + DRAM latency materialization; 60/60 tests.
- P1-11: verification CLI; 63/63 tests; contract review passed.
- phase1-qa: full audit 2026-09-12 — 7/7 runnable gates PASS, AIDA64 parity gate deferred to AM5.

Cycle 1 close-out (2026-09-12): all 11 `branch/chunk-P1-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.
