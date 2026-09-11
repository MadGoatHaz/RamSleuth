RamSleuth v2: Engineering Roadmap & Execution Plan

Project Architecture: Multi-Crate Cargo Workspace

Language Mandate: 100% Rust (Edition 2021/2024)

GUI Framework: egui + eframe

TUI Framework: ratatui + crossterm

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
