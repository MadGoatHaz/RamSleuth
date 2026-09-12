RamSleuth v2: Engineering Roadmap & Execution Plan

Project Architecture: Multi-Crate Cargo Workspace

Language Mandate: 100% Rust (Edition 2021/2024)

GUI Framework: egui + eframe

TUI Framework: ratatui + crossterm

Status (2026-09-12)

- Phase 1 — Native Benchmark Engine (`ramsleuth-bench`): **COMPLETE** (11 chunks, P1-01…P1-11; QA audit 2026-09-12, 7/7 runnable gates PASS; 63/63 tests green).
- Phase 2 — Live Memory Controller Telemetry (`ramsleuth-telemetry`): **COMPLETE** (11 chunks, P2-01…P2-11; QA audit 2026-09-12, 8/8 runnable gates PASS; 159/159 whole-workspace tests green, debug + release; clippy clean).
- Phase 3 — Privilege-Separated Daemon + Unix Socket + Clients (`ramsleuth-daemon`, `ramsleuth-client`): **NEXT** (planning not started; stub crates exist in the workspace).
- Phases 4–5 (TUI/GUI presentation, packaging & distribution) remain future cycles.

Development-Cycle Decisions & Constraints (confirmed 2026-09-12)

1. **Push policy — local only.** All development stays 100% local on branch `v2-development`. Nothing is pushed to the GitHub upstream (`https://github.com/MadGoatHaz/RamSleuth`, whose `master` carries divergent legacy history) until there is a **confirmed, tested, working end-result app** that works as intended. No force-pushes, ever, without explicit sign-off.
2. **Pure-Rust mandate.** 100% Rust, Cargo workspace, Edition 2021. Kernels and CLIs use `std` + `core::arch` intrinsics; third-party crates are added only in the phase that consumes them (Phase 1: `libc` only; Phase 2: `nix` only — features `fs`, `ioctl`, `mman`).
3. **Phased gating.** Each phase/cycle must compile, run, and pass its exit criteria before the next phase starts; each chunk is reviewed and merged (`branch/chunk-N` → `v2-development`, `--no-ff`) before the next; QA audit + compaction per cycle; no silent signature changes to frozen interfaces.

Verification Environment (confirmed 2026-09-12)

(a) **Primary AMD dev host — Ryzen 9 5950X (Zen 3), 16C/32T, 64 MiB L3, DDR4, AVX2 (no AVX-512); CachyOS, kernel `7.2.3-1-cachyos-custom`.**
- The `ryzen_smu` kernel module is **NOT installed** on this host: `sudo modprobe ryzen_smu` → `FATAL: Module ryzen_smu not found in directory /lib/modules/7.2.3-1-cachyos-custom`; `/sys/kernel/ryzen_smu/` does not exist.
- Consequence: live AMD telemetry degrades to `N/A (DriverMissing)` (verified; never panics). The tick-identical ground-truth gate is **BLOCKED** until the module is built, installed, and loaded.
- ryzen_smu integration steps (operator-performed environment setup; the codebase already handles the module-absent case gracefully):
  1. Install kernel headers matching the running kernel (`uname -r` = `7.2.3-1-cachyos-custom`) — e.g. the matching CachyOS headers package.
  2. Obtain the ryzen_smu project source (kernel module + userspace tool); verify the correct upstream repo/URL at setup time.
  3. Build the out-of-tree kernel module against the running kernel (`make` in the module source).
  4. Install it into `/lib/modules/$(uname -r)/` (e.g. `extra/`) and run `sudo depmod -a`.
  5. Load it: `sudo modprobe ryzen_smu` (or `sudo insmod ryzen_smu.ko`).
  6. Verify: `ls /sys/kernel/ryzen_smu/` should show `pm_table`; then `sudo cargo run -p ramsleuth-telemetry --release` should populate the AMD section (clocks/timings/CAD/voltages) instead of `N/A (DriverMissing)`.
  - Note: building an out-of-tree module against a custom CachyOS kernel may require the exact matching headers and could need adjustments.

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

Open /dev/ryzen_smu or read /sys/kernel/ryzen_smu/pm_table.

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
