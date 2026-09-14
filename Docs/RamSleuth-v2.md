RamSleuth v2: Engineering Roadmap & Execution Plan

Project Architecture: Multi-Crate Cargo Workspace

Language Mandate: 100% Rust (Edition 2021/2024)

GUI Framework: egui + eframe

TUI Framework: ratatui + crossterm

Status (2026-09-13 — Cycle 4 close-out)

- Phase 1 — Native Benchmark Engine (`ramsleuth-bench`): **COMPLETE** (Cycle 1, 11 chunks, P1-01…P1-11; QA 2026-09-12, 7/7 runnable gates PASS).
- Phase 2 — Live Memory Controller Telemetry (`ramsleuth-telemetry`): **COMPLETE** (Cycle 2, 11 chunks, P2-01…P2-11; QA 2026-09-12, 8/8 runnable gates PASS). The AMD PM-table model was **reconciled for Vermeer in Phase 5 (P5-15)** — f32 layout + `TableVersionId` version sets; live decode on the 5950X (clocks + VDDCR_SOC; CAD/timings honest `Na` until the SMN-attr follow-up).
- Phase 3 — Privilege-Separated Daemon + Unix Socket + Clients (`ramsleuth-protocol`, `ramsleuth-daemon`, `ramsleuth-client`): **COMPLETE** (Cycle 3, 30 chunks + 2 doc-drift fixes; CORE GATE passed).
- Phase 4 — TUI / GUI presentation (`ramsleuth-tui`, `ramsleuth-gui`): **COMPLETE** (folded into Cycle 3).
- Phase 5 — Packaging & Distribution: **COMPLETE** (Cycle 4, 15 chunks P5-01…P5-15: AUR `ramsleuth-git` + optional `ryzen-smu-dkms` extra + install helper + GitHub Actions CI + README; ryzen_smu uAPI + install-path reconciliation; AMD PM-table model reconciliation).
- Workspace QA baseline (2026-09-13): **337/337 tests green (debug + release), clippy `-D warnings` clean, MSRV 1.75 (lockfile-pinned), release build OK.** `ryzen_smu` (amkillam v0.1.7) is installed + loaded on the AMD dev host — live AMD subtimings work. **Ready for Cycle 5** (live-hardware verification + remaining model reconciliation — see `Docs/HANDOVER.md` §5).

Development-Cycle Decisions & Constraints (confirmed 2026-09-12)

1. **Push policy — local only.** All development stays 100% local on branch `v2-development`. Nothing is pushed to the GitHub upstream (`https://github.com/MadGoatHaz/RamSleuth`, whose `master` carries divergent legacy history) until there is a **confirmed, tested, working end-result app** that works as intended. No force-pushes, ever, without explicit sign-off.
2. **Pure-Rust mandate.** 100% Rust, Cargo workspace, Edition 2021. Kernels and CLIs use `std` + `core::arch` intrinsics; third-party crates are added only in the phase that consumes them (Phase 1: `libc` only; Phase 2: `nix` only — features `fs`, `ioctl`, `mman`).
3. **Phased gating.** Each phase/cycle must compile, run, and pass its exit criteria before the next phase starts; each chunk is reviewed and merged (`branch/chunk-N` → `v2-development`, `--no-ff`) before the next; QA audit + compaction per cycle; no silent signature changes to frozen interfaces.

Verification Environment (confirmed 2026-09-12)

(a) **Primary AMD dev host — Ryzen 9 5950X (Zen 3, Vermeer), 16C/32T, 64 MiB L3, DDR4-3200, AVX2 (no AVX-512); CachyOS, kernel `7.2.3-1-cachyos-custom`.**
- The `ryzen_smu` kernel module is **NOW INSTALLED + LOADED** (amkillam/ryzen_smu v0.1.7 via DKMS — `dkms status` = `ryzen_smu/1.d298366, 7.2.3-1-cachyos-custom, x86_64: installed`). The canonical sysfs kobject is **`/sys/kernel/ryzen_smu_drv/`** — `pm_table` (2288 B blob), sibling `pm_table_version` (`TableVersionId` = 0x380805 on this host), `pm_table_size`, the `smn` attr (the CAD/timings follow-up channel), `codename`/`drv_version`/`version` + the SMU command attrs. The legacy `/sys/kernel/ryzen_smu/` path remains a secondary candidate in the code.
- Consequence: live AMD telemetry **works** — the daemon decodes the PM table (MCLK/UCLK/FCLK ≈ 1792 MHz OneToOne, VDDCR_SOC ≈ 1.128 V; CAD/timings/other rails honest `Na` until the SMN-attr path). The tick-identical ground-truth gate is **UNBLOCKED** (open item 1: cross-check vs `sudo monitor_cpu`, installed at `/usr/bin/monitor_cpu`, mode 700 root).
- Install / management: `scripts/install-ryzen-smu-dkms.sh` (idempotent operator helper; re-execs under sudo) — fast path when `pm_table` is present; `pacman -S --needed dkms base-devel`; kernel build-tree guard (never guesses a custom-kernel headers package — lists candidates and stops); shallow clone of the verified upstream `amkillam/ryzen_smu` (`RYZEN_SMU_URL` override — the former `53XU/ryzen_smu` default is DEAD, HTTP 404); stages the source into `/usr/src/ryzen_smu-$PKGVER` + the repo `dkms.conf`; `dkms add/build/install ${MODULE}/${PKGVER} -k ${KERNEL}`; `modprobe` + `/etc/modules-load.d/ryzen_smu.conf`; verifies `pm_table`. DKMS `AUTOINSTALL=yes` rebuilds the module on kernel updates. Without the module the app degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic). Full details: `Docs/HANDOVER.md` §6.

(b) **Intel test machine — LGA-1151 Intel i5-6600 (Skylake, 6th-gen), dual-channel (2 DIMM channels).**
- Available for **live Intel MCHBAR decode verification** (the Phase 2 Intel path is Intel-gated and returns `N/A (UnsupportedHardware)` on the AMD host).
- The i5-6600 is dual-channel, which matches the `channel_count(Skylake) = 2` model in the Intel decode path.

Workspace Structure

The project is structured as a modular Cargo workspace in the repository root:

RamSleuth/
├── Cargo.toml                      # Workspace definition
├── Docs/
│   ├── Grand Design & Architecture Specification.md
│   ├── RamSleuth-v2.md
│   └── KickOffPrompt.md
└── crates/
    ├── ramsleuth-bench/             # SIMD bandwidth & latency kernels
    ├── ramsleuth-telemetry/         # ryzen_smu, MCHBAR MMIO, ee1004 SPD parsers
    ├── ramsleuth-daemon/            # Privileged service & Unix socket server
    ├── ramsleuth-client/            # Shared IPC client library
    ├── ramsleuth-tui/               # Terminal UI (ratatui)
    └── ramsleuth-gui/               # Immediate-mode Desktop GUI (egui/eframe)


Phase 1: Native Benchmark Engine (ramsleuth-bench)

Objective: Build a standalone, high-performance benchmarking crate that saturates multi-channel memory bandwidth and measures nanosecond-accurate access latencies.

1.1 Implementation Tasks

Target Feature Detection & SIMD Kernels:

Implement vector bandwidth kernels utilizing AVX2 (_mm256_*) and runtime-detected AVX-512 (_mm512_*).

Read Kernel: Unrolled streaming loads across cache-line boundaries.

Write Kernel: Non-temporal stores (_mm256_stream_si256 / _mm512_stream_si512) to bypass cache hierarchy and avoid Read-For-Ownership overhead.

Copy Kernel: Interleaved vector loads and non-temporal streaming stores.

Multi-Threaded Saturation & Affinity:

Query /sys/devices/system/cpu/cpu*/topology/ to identify physical cores vs SMT threads.

Dispatch one worker thread per physical core, pinned with sched_setaffinity or core_affinity.

Synchronize thread launch using barriers (std::sync::Barrier) for concurrent memory access.

Pointer-Chasing Latency Kernel:

Construct a pseudo-random circular linked list across a 128 MB buffer.

Enforce a 64-byte stride between nodes to defeat hardware stream and spatial prefetchers.

Execute single-threaded on the preferred physical core using serialized core::arch::x86_64::__rdtscp.

Cache Hierarchy Buffer Isolation:

Implement buffer size profiles:

L1 Cache: 16 KB

L2 Cache: 256 KB

L3 Cache: 50% of single CCD capacity (e.g., 16 MB)

DRAM: $\ge 256\text{ MB}$ ($>2\times$ total CPU L3 cache)

1.2 Verification CLI

Expose a binary target cargo run -p ramsleuth-bench that executes all tests and outputs formatted text and JSON metrics.

1.3 Exit Criteria

DRAM read/write bandwidth matches AIDA64 on the same machine within $\pm 5\%$.

DRAM latency values align with platform baselines (e.g., 60–75 ns on DDR5-6000 AM5).

Phase 2: Hardware Telemetry & Register Extraction (ramsleuth-telemetry)

Objective: Extract real, active memory controller subtimings, clocks, voltages, and SPD information directly from hardware.

2.1 Implementation Tasks

AMD Zen Telemetry (ryzen_smu Provider):

Inspect CPUID family/model (Zen 1 through Zen 5).

Open /dev/ryzen_smu or read `/sys/kernel/ryzen_smu_drv/pm_table` (+ the sibling `pm_table_version` / `pm_table_size` attrs; the legacy `/sys/kernel/ryzen_smu/pm_table` path remains a secondary candidate).

Parse SMU PM tables to extract:

Clocks: MCLK, UCLK, FCLK, DivMode (1:1 vs 1:2), Gear Down Mode.

Timings: tCL, tRCDWR, tRCDRD, tRP, tRAS, tRC, tRRDS, tRRDL, tFAW, tWTRS, tWTRL, tWR, tRFC1, tRFC2, tRFCsb, tCWL, tRTP, tRDWR, tWRRD.

Drive Strengths & Resistances: ProcODT, RttNom, RttWr, RttPark, ClkDrv, AddrCmdDrv.

Voltages: VDDCR_SOC, VDDIO_MEM, VDD_MISC.

Intel Core Telemetry (MCHBAR Provider):

Read PCI configuration space 00:00.0 offset 0x48 for 64-bit MCHBAR base address.

Map physical address range using /dev/mem or /dev/fmem via mmap.

Extract primary bitfields: tCL, tRCD, tRP, tRAS, Command Rate (1N/2N), Gear Mode (1/2/4).

Extract turnaround timings and Round Trip Latencies (RTL).

SPD EEPROM Parser (ee1004 Provider):

Query /sys/bus/i2c/drivers/ee1004/*/eeprom.

Decode raw 512-byte (DDR4) / 1024-byte (DDR5) blocks:

Manufacturer identification via JEP106 IDs.

DRAM die manufacturer and stepping (Samsung B-die, SK Hynix A-die/M-die).

Part numbers, serial numbers, rank organization, and XMP 2.0/3.0 / EXPO profiles.

2.2 Exit Criteria

Telemetry library produces a populated SystemMemoryTelemetry struct containing verified live timings on test hardware.

Gracefully returns structured errors (UnsupportedHardware, DriverMissing) without panicking.

2.3 Status & Acceptance Notes (2026-09-12)

- Code status: all 11 chunks merged and QA-passed; crate builds, tests green (159/159 whole-workspace, debug + release), clippy clean.
- **AMD tick-identical ground truth: BLOCKED** pending `ryzen_smu` module install + root on the 5950X host (see Verification Environment (a); the module is currently not installed on `7.2.3-1-cachyos-custom`).
- **Intel live decode: to be verified on the i5-6600** (Skylake, LGA-1151, dual-channel) machine — not yet run.
- **Model reconciliation pending live silicon:** AMD PM byte offsets, Intel IMC register offsets (both currently plan-mandated SKELETONS), and SPD decode of maker `0xC1` + density `0x0D` codes observed on this host (raw-hex fallback today; no die-maker mapping for these codes yet).
- Verified live on the 5950X host: `CPU: Amd(Zen3)`; `AMD: N/A (DriverMissing)`; `Intel: N/A (UnsupportedHardware)`; SPD: 2× DDR4 modules (rank=1, 3200 MT/s) — exit 0, no panic in any privilege/CPU state.
- **Current state (2026-09-13, supersedes the above snapshot):** `ryzen_smu` is installed + loaded on this host (amkillam/ryzen_smu v0.1.7, DKMS) — the AMD section is **populated live** (clocks ≈ 1792 MHz OneToOne + VDDCR_SOC ≈ 1.128 V, decoded from the reconciled Vermeer f32 PM-table model — P5-15; CAD/timings honest `Na` until the SMN-attr follow-up); the tick-identical ground-truth cross-check is **UNBLOCKED** (open item 1, vs `sudo monitor_cpu`). Intel live decode still pending the i5-6600 (open item 2); model reconciliation partially done (open item 3). See `Docs/HANDOVER.md` §4–§5.

Phase 3: Daemon Architecture & Privileged IPC (ramsleuth-daemon, ramsleuth-client)

Objective: Securely isolate privileged hardware operations inside a daemon, exposing a clean IPC channel for unprivileged user frontends.

3.1 Implementation Tasks

Daemon Service (ramsleuth-daemon):

Initialize hardware drivers, verifying CAP_SYS_RAWIO.

Listen on Unix Domain Socket /run/ramsleuth/ramsleuth.sock with file mode 0660.

Handle IPC requests asynchronously using tokio:

GetTelemetry: Returns cached hardware subtiming struct.

RunBenchmark { target, mode }: Triggers benchmark passes and streams progress back.

Provide systemd/ramsleuth.service unit file with sandboxing directives.

Client Library (ramsleuth-client):

Lightweight crate to connect to /run/ramsleuth/ramsleuth.sock.

Handles timeouts, connection retries, and deserialization.

Emits user-friendly error diagnostics when the daemon is not running.

3.2 Exit Criteria

An unprivileged terminal command cargo run -p ramsleuth-client -- dump prints full hardware timings without requiring sudo.

Phase 4: Modern Presentation Layer (ramsleuth-gui, ramsleuth-tui)

Objective: Build high-density, intuitive interfaces matching ZenTimings and AIDA64 for desktop and terminal users.

4.1 Implementation Tasks

Desktop GUI (crates/ramsleuth-gui using egui / eframe):

Setup eframe::NativeOptions with hardware-accelerated rendering and a dark-slate theme.

Timing Matrix: Build a dense grid (egui::Grid) displaying Clocks, Primaries, Secondaries, Tertiaries, CAD Bus resistances, and Voltages.

Benchmark Grid: Build an interactive results table (egui_extras::TableBuilder) displaying Read, Write, Copy, and Latency for Memory, L1, L2, and L3.

Interactive Controls: "Run All", "Benchmark Memory", and progress indicators that update during active benchmark passes.

Export: Single-button screenshot generation saving a clean .png validation card to disk.

Terminal UI (crates/ramsleuth-tui using ratatui):

Terminal dashboard rendering the timing matrix and benchmark table.

Keybindings: [R] Run Benchmark, [S] Export Snapshot, [Q] Quit.

4.2 Exit Criteria

GUI runs at a steady 60 FPS without UI freezes during active benchmark runs.

TUI renders cleanly across varying standard terminal sizes.

Phase 5: Tooling, Packaging & Distribution

Objective: Package RamSleuth for seamless installation on Linux distributions.

5.1 Implementation Tasks

Kernel Driver Hooks:

Implement automated system checks notifying users if ryzen_smu or ee1004 kernel modules need installation or loading (modprobe).

Packaging:

Provide Arch Linux PKGBUILD for AUR (ramsleuth-git).

Include systemd presets enabling ramsleuth.service on installation.

CI/CD Pipeline:

GitHub Actions workflow verifying builds, unit tests, and clippy lints across x86_64-unknown-linux-gnu.

5.2 Exit Criteria

Single-command installation (cargo install or makepkg -si) results in a fully functioning daemon and UI binaries.
