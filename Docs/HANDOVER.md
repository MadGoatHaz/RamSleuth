# RamSleuth v2 — Development Handover

> **Audience:** the next development cycle (Cycle 3 / Phase 3). Self-contained: read this first, then `plans/` and the other `Docs/` files.
> **Date:** 2026-09-12. **Branch:** `v2-development` — **LOCAL ONLY, not pushed.**
> **Sources of truth:** this file + `Docs/RamSleuth-v2.md` (roadmap) + `Docs/Grand Design & Architecture Specification.md` (spec) + `plans/PLAN.md` (Phase 1 — done) + `plans/PLAN-PHASE2.md` (Phase 2 — done) + `MASTER_LOG.md` (durable cycle history) + `DEV_LOG.md` (lease board — currently empty).

---

## 1. Project Overview

RamSleuth v2 is a dual-layer hardware introspection + benchmarking utility for Linux x86_64:

1. **Live memory-controller telemetry** — active trained subtimings, clocks (MCLK / UCLK / FCLK, gear/div modes, GDM/PDM), CAD-bus drive strengths & terminations, and voltages (AMD via the `ryzen_smu` SMU PM table; Intel via MCHBAR MMIO), plus SPD EEPROM decode (JEP106 module/die makers, rank, XMP 2.0/3.0, EXPO).
2. **AIDA64-style benchmark engine** — multi-threaded AVX2/AVX-512 non-temporal SIMD bandwidth (Read / Write / Copy × Memory / L3 / L2 / L1) and pointer-chase latency.

End goal: unprivileged TUI (ratatui) and GUI (egui/eframe) frontends served by a privileged daemon (Phase 3) — i.e. ZenTimings + ASRock Timing Configurator + AIDA64 in one native Linux tool.

**Mandate:** 100% pure Rust, Cargo workspace (Edition 2021, resolver 2), Linux x86_64. No C++/Python/JS components.

## 2. Current State (2026-09-12)

- **Phase 1 — Native Benchmark Engine (`ramsleuth-bench`): COMPLETE.** 11 chunks (P1-01…P1-11) merged into `v2-development`; QA audit 2026-09-12: 7/7 runnable gates PASS. AIDA64 parity gate deferred to a DDR5-6000 AM5 host (the reference host is DDR4).
- **Phase 2 — Live Memory Controller Telemetry (`ramsleuth-telemetry`): COMPLETE.** 11 chunks (P2-01…P2-11) merged; QA audit 2026-09-12 @ `367bef9`: 8/8 runnable gates PASS.
- **Phase 3 — Privilege-Separated Daemon + Unix Socket + Clients: NEXT.** `ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui` exist only as stub crates.
- **Quality bar (verified for both phases):** **159/159 whole-workspace tests green in debug + release**; `cargo clippy --workspace --all-targets -- -D warnings` → **zero warnings**; release build OK.
- **Verification CLIs (both work today, exit 0):**
  - `cargo run -p ramsleuth-bench` — full 4×4 grid (text + hand-rolled JSON); optional `--avx512` (falls back to AVX2 when the host lacks AVX-512F — as on the reference host) and `--json` flags; unknown flag → exit 2.
  - `cargo run -p ramsleuth-telemetry` — dashboard-style telemetry listing; every cell renders as a value or `N/A (<reason>)`; optional `--json`; **exit 0 even when all sections are `Na`** (a structured N/A is a valid outcome). Run as root with the `ryzen_smu` module loaded for live AMD data.
- **Live result on the reference host today:** `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD: N/A (DriverMissing)` (ryzen_smu not installed); `Intel: N/A (UnsupportedHardware)`; SPD: 2× DDR4 modules (rank=1, 3200 MT/s). No panic in any privilege/CPU state.

## 3. Workspace Layout (6 crates)

| Crate | Status | Contents |
|---|---|---|
| `crates/ramsleuth-bench` | **DONE (Phase 1)** | `features` (AVX2/AVX-512F runtime detect), `topology` (/sys physical-core enumeration, SMT filter, total/per-CCD L3), `buffers` (pure 64B-aligned sizing plan), `kernel_read` / `kernel_write` / `kernel_copy` (AVX2 streaming / NT / copy), `kernel_512` (AVX-512F variants + AVX2 fallback), `worker` (pinned barrier-synced per-core dispatch), `latency` (pointer-chase ring, `__rdtscp`), `orchestrator` (4×4 grid, best-of-3 / median), `main` (verification CLI; hand-rolled JSON, no serde/clap). Deps: `libc` only. |
| `crates/ramsleuth-telemetry` | **DONE (Phase 2)** | `cpuid` (vendor + frozen generation map), `error` (`TelemetryError` / `Section<T>` no-panic contract), `amd_smu` (sysfs `pm_table` → `/dev/ryzen_smu` ioctl), `amd_pm` (version-guarded bounds-checked PM parse), `amd_readout` (shared display types + AMD mapping), `intel_mchbar` (PCI BAR5 + read-only `/dev/mem` mmap guard), `intel_readout` (per-channel IMC decode), `spd_eeprom` (ee1004 unprivileged acquire), `spd_decode` (JEP106 / rank / XMP / EXPO), `facade` (`SystemMemoryTelemetry` + `collect()`), `main` (verification CLI). Deps: `nix` 0.29 (`fs`/`ioctl`/`mman`) only. |
| `crates/ramsleuth-daemon` | **stub (Phase 3)** | empty bin; will hold the CAP_SYS_RAWIO service + Unix-socket RPC server (tokio planned). |
| `crates/ramsleuth-client` | **stub (Phase 3)** | empty bin; will hold the shared IPC client library (socket connect, timeouts/retries, de/serialization). |
| `crates/ramsleuth-tui` | **stub (Phase 4)** | empty bin; ratatui + crossterm terminal dashboard. |
| `crates/ramsleuth-gui` | **stub (Phase 4)** | empty bin; egui/eframe desktop dashboard. |

Root `Cargo.toml`: 6 members, `version 0.1.0`, `edition 2021`, `rust-version = "1.75"`, license MIT, repo `https://github.com/MadGoatHaz/RamSleuth`.

## 4. Key Architectural Decisions & Frozen Interfaces

1. **No-panic `Section<T>` contract (P2-02, frozen):** displayable telemetry values are `Section<T> { Value(T), Na(TelemetryError) }`; fallible operations return `TelemetryResult<T>`. `TelemetryError` variants: `UnsupportedHardware`, `UnsupportedVendor`, `DriverMissing`, `InsufficientPrivilege`, `UnknownPmTableVersion`, `NoDevmem`, `NotApplicable`, `InvalidValue(String)`, `Io(String)`. **No `panic!`, no `unwrap()`/`expect()` on hardware-derived data, no unguarded deref** anywhere in the telemetry crate; every `unsafe` block carries `// SAFETY:` and is bounds-checked. The CLI renders every missing value as `N/A (<reason>)` and exits 0.
2. **Direct SMU access — no `ryzen_smu` Rust crate (P2-03, decision D1):** sysfs-first read of `/sys/kernel/ryzen_smu/pm_table` (`std::fs`), fallback open of `/dev/ryzen_smu` + driver ioctl via `nix`. We own the version-guarded PM parse and all mapping. `nix` (features `fs`/`ioctl`/`mman`) is the **only** Phase 2 dependency and the only telemetry dep. Rejected: `ryzen_smu` crate, `memoffset`, serde/serde_json (hand-rolled JSON), clap (std `env::args`).
3. **AMD PM + Intel IMC byte offsets are plan-mandated SKELETONS (P2-04 / P2-07):** the offset/layout tables are version-guarded and bounds-checked, but their numeric values are **not yet reconciled against live silicon** (open gate §7c). Until then, displayed values are best-effort model values.
4. **Intel MCHBAR gating (P2-06, decision D4):** `CpuInfo` must be Intel before **any** file/mmap access; `/dev/mem` is mapped **read-only** behind a guard struct that bounds-checks reads and `munmap`s in `Drop`; STRICT_DEVMEM EIO/ENODATA → `InsufficientPrivilege`; missing file → `NoDevmem`. On the AMD host the path returns `Na(UnsupportedVendor)`/`Na(UnsupportedHardware)` and verifiably never touches `/dev/mem`.
5. **SPD is unprivileged (P2-08 / P2-09):** raw `ee1004` sysfs images (512 B DDR4 / 1024 B DDR5); JEP106 module + die makers (continued-ID support), rank bits, part/serial, JEDEC speed, XMP 2.0/3.0 + EXPO profile summaries; a checksum failure skips the profile, never the module; no panic on truncated/garbage data.
6. **Phase 1 checksum conventions (frozen):** read/copy kernels return the **wrapping sum of the buffer's little-endian 64-bit words** (data-sensitive DCE sink; `0` for a zeroed buffer); the scalar fallback has identical semantics; write kernels' frozen return is the **byte counter** (so `checksum == total_bytes` for Write); per-slice word-sums wrapping-add to exactly the whole-buffer checksum (proves the partition has neither overlap nor gap); all tier buffers are 64-byte aligned (`AlignedBuf`); bandwidth cells are **best-of-3** `Instant` timings, latency cells the **median** of 3 runs of a ≥1 M-hop chase over the *materialized* full-tier ring (Memory tier exceeds L3 → true DRAM latency); non-x86_64 latency falls back to `Instant` only (documented precision loss).
7. **CPUID family mapping (P2-01, frozen):** the 5950X reference host reports **family `0x19` → `Amd(Zen3)`** (frozen map: `0x15`→Zen 1, `0x17`→Zen 2, `0x19`→Zen 3, `0x1A`→Zen 4, `0x1C`→Zen 5, else `Unknown`). **Note: desktop Zen 4/5 silicon also reports family `0x19` on some boards, so the AMD PM parse (P2-04) keys on the SMU version (7.11.x / 12.x / 13.x), not on `AmdZen`** — generation ambiguity cannot break the PM layout. Intel: family `0x6` model table covers Skylake (`0x4F`/`0x56`/`0x5E`) through Arrow Lake; unrecognized models → `IntelGen::Unrecognized` (still Intel-gated, not `Unknown`).
8. **Topology & buffers are pure (P1-02 / P1-03):** `detect() -> Result<CpuTopology, TopologyError>` over /sys (SMT siblings filtered; per-CCD L3 slices; total L3); `plan(&CpuTopology) -> BufferPlan` is a pure function (L1 16 KiB, L2 256 KiB, L3 = 50% of a CCD slice, DRAM = max(256 MiB, 3× total L3), latency ring 128 MiB; all 64-byte aligned; safe fallbacks when sysfs fields are missing).

## 5. Verification Environment (confirmed 2026-09-12)

### (a) Primary AMD dev host — Ryzen 9 5950X (Zen 3)

- 16C/32T, 64 MiB L3, DDR4 (2× modules, 3200 MT/s, rank-1), AVX2 — **no AVX-512** (the `--avx512` path falls back to AVX2; the 512-bit kernels compile but the runtime gate selects AVX2).
- **OS: CachyOS, kernel `7.2.3-1-cachyos-custom`.**
- **`ryzen_smu` kernel module: NOT installed.** Evidence: `sudo modprobe ryzen_smu` → `FATAL: Module ryzen_smu not found in directory /lib/modules/7.2.3-1-cachyos-custom`; `/sys/kernel/ryzen_smu/` does not exist. So live AMD telemetry renders `N/A (DriverMissing)` today.
- **Integration steps (operator-performed; the codebase already handles the module-absent case gracefully — `Na(DriverMissing)`, never panics):**
  1. Install kernel headers matching the running kernel (`uname -r` = `7.2.3-1-cachyos-custom`) — the matching CachyOS headers package.
  2. Obtain the ryzen_smu project source (kernel module + userspace tool). **Verify the correct upstream repo/URL at setup time** — do not trust a cached URL.
  3. Build the out-of-tree kernel module against the running kernel (`make` in the module source).
  4. Install it into `/lib/modules/$(uname -r)/` (e.g. `extra/`) and run `sudo depmod -a`.
  5. Load it: `sudo modprobe ryzen_smu` (or `sudo insmod ryzen_smu.ko`).
  6. Verify: `ls /sys/kernel/ryzen_smu/` should show `pm_table`; then `sudo cargo run -p ramsleuth-telemetry --release` should populate the AMD section (clocks/timings/CAD/voltages) instead of `N/A (DriverMissing)`.
  - Caution: building an out-of-tree module against a **custom CachyOS kernel** may require the exact matching headers and could need build adjustments (Kconfig/version guards) before it loads.

### (b) Intel test machine — i5-6600 (Skylake)

- LGA-1151, Intel Core i5-6600 (6th-gen Skylake), **dual-channel (2 DIMM channels)** — matches `channel_count(Skylake) = 2` in the Intel decode model.
- Purpose: **live Intel MCHBAR decode verification** (BAR5 → read-only `/dev/mem` mmap → per-channel IMC registers). The Phase 2 Intel path is Intel-gated and returns `N/A (UnsupportedHardware)` on the AMD host, so this machine is where the decode gets its live proof.
- Intel voltages/CAD-bus sections are out of Phase 2 scope by design → `Na(NotApplicable)` on Intel.

### (c) Deferred target

- The AIDA64 parity gate (±5% DRAM bandwidth; 60–75 ns DDR5-6000 AM5 latency band) is deferred to a **DDR5-6000 AM5 host** — the 5950X host is DDR4 (measured DRAM latency ~81.5 ns is out of that band as expected; reported, not failed).

## 6. Push Policy & Git State

- **Branch `v2-development` is the development branch — LOCAL ONLY. Nothing has been pushed to GitHub.**
- Upstream `https://github.com/MadGoatHaz/RamSleuth` has a **divergent legacy `master`** (origin/HEAD → origin/master). Do not merge into or force onto it; treat upstream as a read-only reference.
- **Push policy (confirmed 2026-09-12): stay 100% local until there is a CONFIRMED, TESTED, WORKING end-result app that works as intended. No force-pushes, ever, without explicit sign-off.**
- Merge model per chunk: `branch/chunk-<ID>` → `v2-development` via `git merge --no-ff` after per-chunk review + tests. All Phase 1 + Phase 2 chunk branches are merged and pruned (verified at each cycle close-out); the working tree is clean at HEAD `7f9d8ac` (Cycle 2 compaction).
- `DEV_LOG.md` is the active lease board (currently no active leases); `MASTER_LOG.md` holds the durable per-cycle history (Cycles 1–2 + the 2026-09-12 decisions record).

## 7. Open Items / Acceptance Gates

- **(a) AMD tick-identical ground truth — BLOCKED on environment.** Needs the `ryzen_smu` module installed + root on the 5950X host (§5a steps), then: live values tick-identical to the `ryzen_smu` CLI's own readout; clocks ±1 MHz; voltages ±10 mV; CAD equal to the RZQ/code-table mapping.
- **(b) Intel live MCHBAR decode — pending.** Run `sudo cargo run -p ramsleuth-telemetry --release` on the i5-6600 machine; confirm per-channel tCL/tRCD/tRP/tRAS + command-rate/gear decode against known-good values; confirm `Na(NotApplicable)` for Intel voltages/CAD.
- **(c) Model reconciliation — pending live silicon.** AMD PM byte offsets (P2-04 skeleton), Intel IMC register offsets (P2-07 skeleton), and SPD decode of the codes observed on the 5950X host: **maker `0xC1`** (currently rendered raw-hex) and **density `0x0D`** (currently `Na`). Reconcile against the live PM table / SPD spec, then update the decode tables + fixtures.
- **(d) Phase 1 L1/L2 bandwidth overhead refinement.** The small-tier working sets (32 KiB L1d / 1 MiB L2) split across 16 pinned workers are dominated by thread/barrier/timing overhead per pass → cells are not comparable across tiers. Fix: **add inner-loop iterations** in the small-tier passes to amortize per-pass overhead. (Known follow-up from Cycle 1; cells remain informational until then.)
- **(e) MSRV conflict.** Workspace `rust-version = 1.75`, but the AVX-512F intrinsics (P1-07) require **Rust 1.89** (clippy is msrv-gated at 1.89 for that crate). **Decide: bump the workspace MSRV to 1.89 (recommended) or cfg-gate the 512-bit bodies** — a plan-level decision for the next cycle.
- **(f) Push strategy.** Stay local until a confirmed working end-result app exists (§6); the first push should then be a deliberate, reviewed event (likely a fresh `v2` branch/tag rather than anything onto the legacy `master` — decide at push time, with explicit sign-off).

## 8. Next Phase — Phase 3: Privilege-Separated Daemon + Unix Socket + Clients

Goal (per `Docs/RamSleuth-v2.md` Phase 3): isolate all privileged hardware operations in a daemon and serve unprivileged frontends.

- **`ramsleuth-daemon` (Phase 3):** systemd service holding **`CAP_SYS_RAWIO`** (plus root where the driver requires it); initializes and owns the hardware handles (ryzen_smu sysfs/char-dev, MCHBAR `/dev/mem`, ee1004); listens on **Unix domain socket `/run/ramsleuth/ramsleuth.sock`** (mode 0660, `ramsleuth` group); async RPC (tokio planned) with commands `GetTelemetry` (cached struct), `RunBenchmark { target, mode }` (progress streamed), `CancelBenchmark`; sandboxing (`ProtectHome`, `ProtectSystem=strict`, `NoNewPrivileges`); ships `systemd/ramsleuth.service`.
- **`ramsleuth-client` (Phase 3):** lightweight shared IPC library — connect to the socket, timeouts + retries, de/serialization (Bincode per the stack doc, or JSON-RPC), user-friendly diagnostics when the daemon is down. Exit criterion: `cargo run -p ramsleuth-client -- dump` prints full hardware timings **without sudo**.
- Phase 4 (TUI/GUI — ratatui + egui/eframe) and Phase 5 (packaging — PKGBUILD/AUR, systemd preset, GitHub Actions CI) follow in later cycles, same per-chunk protocol.
- The daemon **inherits the no-panic contract**: any absent driver/privilege/hardware surfaces as structured `Na` sections in the payload — never a crash of the service or the client.

## 9. Operating Protocol Summary (how work runs in this repo)

1. **Strict phased gating:** each cycle compiles + runs + passes its exit criteria before the next phase starts. Each phase is planned first (`plans/PLAN-PHASE<N>.md`) with micro-chunks, dependency tags (`[ISOLATED]` / `[COUPLED-TO: …]` / `[CRITICAL-PATH]`), frozen-interface markers, and per-chunk quality gates.
2. **Single-file micro-chunking:** one target source file per chunk, ≤ ~50–100 lines changed; each chunk adds exactly one `mod <name>;` wiring line to `src/lib.rs` (wiring not counted against the budget).
3. **Per-chunk flow:** implement on `branch/chunk-<ID>` (forked from `v2-development`) → in-file unit tests (`#[cfg(test)]`) → clippy `-D warnings` → **per-chunk code review** (lease-signed) → `git merge --no-ff` into `v2-development` → branch pruned. Interface-freeze chunks (`[CRITICAL-PATH]`) merge first; no silent signature changes — changes are a plan edit + rebase.
4. **Lease board:** `DEV_LOG.md` `@@@ ACTIVE_WORKERS @@@` — sign in with target files before work; sign out with `[DONE]`/`FAILED` + one-line decision + ahead-note; update `@@@ CURRENT_STATE @@@`. (Board is empty right now.)
5. **Handoffs:** 3-line **Semantic Pulse** — `STATUS` / `DECISION` / `AHEAD`, one short line each. No full-file dumps or code in chat; all detail goes to disk (plans, logs, docs).
6. **QA audit per cycle:** independent pass of every runnable gate (build, tests debug+release, clippy, live CLI runs in multiple privilege/CPU states) → then **compaction**: merge history into `MASTER_LOG.md`, prune chunk branches, reset `DEV_LOG.md`.
7. **Safety/fallbacks (hard rule):** unsupported CPU, missing driver, missing privilege, unknown PM-table version, absent `/dev/mem` → structured `Na(<reason>)`; **never panic, never segfault, never `unwrap`/`expect` on hardware data**; the Intel path never touches `/dev/mem` on non-Intel hardware; non-x86_64 compiles to a documented degraded path.
8. **Docs discipline:** `Docs/` holds the roadmap + spec + this handover; `plans/` holds per-phase plans; `MASTER_LOG.md` is the durable record. Update them every cycle; this document is the standing handover — refresh it at each cycle close-out.
