RamSleuth: Grand Design & Architecture Specification

Project Codename: RamSleuth v2

Target Platform: Linux (x86_64)

Primary Language: 100% Pure Rust (Edition 2024 / 2021)

Classification: Low-Level Hardware Profiler, Memory Controller Diagnostic Tool & Cache/Memory Benchmark Suite

1. Executive Summary & Vision

RamSleuth is a specialized, open-source hardware profiling and memory benchmarking utility designed natively for Linux.

For years, PC enthusiasts, memory overclockers, and hardware engineers running Linux have faced an uneven tooling landscape. On Windows, tuning DDR4 and DDR5 memory relies on a familiar trio of utilities:

ZenTimings (for exhaustive AMD subtimings, Fabric clocks, and bus drive strengths),

ASRock Timing Configurator or MemTweakIt (for Intel IMC registers), and

AIDA64 Extreme (for the de facto standard Read, Write, Copy, and Latency cache/memory benchmark matrix).

Under Linux, users have typically been forced into a fragmented ecosystem: deciphering raw hex dumps via busybox devmem, scraping partial JEDEC tables with dmidecode, or rebooting into Windows or UEFI to verify subtiming stability.

RamSleuth bridges this gap. It operates as a dual-layer utility available in both an authentic terminal interface (TUI) and an immediate-mode graphical desktop presentation (GUI). It reads active, live-trained memory controller registers across AMD Zen and Intel Core architectures, parses raw SPD EEPROM blocks directly from the SMBus, and executes a multi-threaded, non-temporal SIMD memory and cache benchmark suite that produces results directly comparable to industry-standard benchmarks.

2. Core Technology Stack

To avoid garbage collection pauses, minimize IPC latency, guarantee safety when accessing hardware registers, and eliminate C++ bridge maintenance, RamSleuth is written exclusively in Rust:

Component

Technology

Rationale

Workspace & Tooling

Rust Cargo Workspaces

Unified builds, strict dependency boundaries, and simple cross-crate testing.

SIMD & Benchmarking

core::arch::x86_64 Intrinsics

Zero-cost AVX2/AVX-512 streaming loads, non-temporal stores (vmovntdq), and serialized cycle counting (__rdtscp).

Daemon IPC

Unix Domain Sockets + Bincode

Microsecond serialization, local POSIX file permissions (0660), zero network attack surface.

GUI Frontend

egui + eframe

Pure Rust immediate-mode GUI. No Webview/Electron bloat, no C++ Qt bridge instability. Trivial state synchronization with polling hardware feeds.

TUI Frontend

ratatui + crossterm

High-performance terminal UI ideal for headless servers, minimal window managers, and SSH workflows.

Privilege Model

Linux Capabilities (CAP_SYS_RAWIO)

Minimal privilege escalation; UI code runs completely unprivileged.

3. Visual & Interactive Design

RamSleuth enforces an information-dense, zero-fluff, dark-mode aesthetic. It presents all critical metrics on a single, non-scrolling dashboard, avoiding multi-tab navigation where essential context is hidden.

3.1 Unified Visual Dashboard (GUI & TUI)

┌────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│ RamSleuth v2.0.0                      [AMD AM5 Platform]                      Daemon: Connected (IPC: /run/ramsleuth)   │
│ CPU: AMD Ryzen 9 7950X 16-Core @ 5.70 GHz | Motherboard: ASUS ROG CROSSHAIR X670E HERO (BIOS: 2204, AGESA 1.2.0.0a)     │
│ RAM: 64.0 GB (2x32GB) DDR5-6000 MT/s | Dual-Channel (2x32-bit per DIMM) | Mode: Synchronous 1:1 (UCLK = MCLK = 3000 MHz)│
├────────────────────────────────────────────────────────────────────────┬───────────────────────────────────────────────┤
│ LIVE MEMORY CONTROLLER & SUBTIMINGS                                    │ AIDA-STYLE BENCHMARK ENGINE                   │
├────────────────────────────────┬───────────────────────────────────────┼───────────────────────────────────────────────┤
│ [Clocks & Ratios]              │ [Tertiary & Turnarounds]              │               Read        Write       Copy    Latency │
│  MCLK:         3000.0 MHz      │  tRDRDSD:     1       tWRWRSD:    1   │ Memory:    88,412 MB/s 91,204 MB/s 84,150 MB/s 61.2 ns│
│  UCLK:         3000.0 MHz      │  tRDRDDD:     1       tWRWRDD:    1   │ L1 Cache:   3,840 GB/s  1,980 GB/s  3,790 GB/s  0.7 ns│
│  FCLK:         2000.0 MHz      │  tRDRDSCL:    4       tWRWRSCL:   4   │ L2 Cache:   1,890 GB/s  1,720 GB/s  1,810 GB/s  2.5 ns│
│  UCLK:MCLK:    1:1             │  tRDRDSC:     1       tWRWRSC:    1   │ L3 Cache:   1,120 GB/s  1,060 GB/s  1,090 GB/s 10.2 ns│
│  Gear Mode:    Gear 1          │  tRDWR:       16      tWRRD:      4   ├───────────────────────────────────────────────┤
│  GDM / CR:     Disabled / 1T   │                                       │ [ RUN FULL BENCHMARK ]    [ MEMORY ONLY ]     │
├────────────────────────────────┼───────────────────────────────────────┤ Progress: [========================] 100% Idle│
│ [Primary Timings]              │ [CAD Bus Drive & Termination]         ├───────────────────────────────────────────────┤
│  tCL:          30              │  ProcODT:     48.0 Ω                  │ HARDWARE & SPD MODULE TELEMETRY               │
│  tRCDWR:       38              │  RttNom:      Disabled                ├───────────────────────────────────────────────┤
│  tRCDRD:       38              │  RttWr:       RZQ/2 (120 Ω)           │ Slot 1 (DIMM_A2):                             │
│  tRP:          38              │  RttPark:     RZQ/5 (48 Ω)            │  G.Skill Trident Z5 RGB (F5-6000J3038F16GX2)  │
│  tRAS:         96              │  ClkDrv:      40.0 Ω                  │  DRAM Die: SK Hynix (A-Die, 16Gb) | Single-Rank│
├────────────────────────────────┼  AddrCmdDrv:  40.0 Ω                  │  EXPO Profile 1: DDR5-6000 CL30-38-38-96 1.35V│
│ [Secondary Timings]            │  CsOdtDrv:    40.0 Ω                  │                                               │
│  tRC:          134             │  CkeDrv:      40.0 Ω                  │ Slot 2 (DIMM_B2):                             │
│  tRRDS / tRRDL:4 / 8           ├───────────────────────────────────────┤  G.Skill Trident Z5 RGB (F5-6000J3038F16GX2)  │
│  tFAW:         20              │ [Active System Voltages]              │  DRAM Die: SK Hynix (A-Die, 16Gb) | Single-Rank│
│  tWTRS / tWTRL:4 / 16          │  VDDCR_SOC:   1.200 V                 │  EXPO Profile 1: DDR5-6000 CL30-38-38-96 1.35V│
│  tWR:          48              │  VDDIO_MEM:   1.350 V                 ├───────────────────────────────────────────────┤
│  tRFC1 / tRFC2:480 / 360       │  VDD_MISC:    1.100 V                 │ ACTIONS:                                      │
│  tRFCsb:       300             │  VPP:         1.800 V                 │ [F2] Snapshot PNG   [F3] Export JSON  [Q] Quit│
└────────────────────────────────┴───────────────────────────────────────┴───────────────────────────────────────────────┘


3.2 GUI Implementation Strategy (egui / eframe)

Immediate-Mode Ergonomics: egui redraws the state directly on every frame. Telemetry updates received from the daemon background thread update an Arc<RwLock<TelemetryData>> struct that the UI immediately displays without complex state-binding trees.

Layout Structure:

Utilizes egui::Grid with uniform column sizing for the timing tables.

Utilizes egui_extras::TableBuilder for the benchmark results matrix.

Semantic Palette:

Cyan (#00D4FF): Primary timings and bandwidth scores.

Amber (#FFB300): Synchronous clocks (1:1 ratio), memory voltages, and low latency.

Slate (#1E1E24): Background panels and card separators.

Crimson (#FF3B30): Gear desync (1:2), out-of-spec voltages ($>1.30\text{V}$ SOC on AM5), and memory parity errors.

4. Privilege Separation & System Daemon Architecture

Direct access to hardware registers (/dev/mem, PCI configuration space) and kernel mailbox character devices (/dev/ryzen_smu) requires elevated permissions (CAP_SYS_RAWIO or root). Running a graphical desktop client with elevated privileges creates severe security vulnerabilities and desktop compositor instability.

 ┌────────────────────────────────────────────────────────┐
 │                   User Interface Client                │
 │       (ramsleuth-gui or ramsleuth-tui: Unprivileged)   │
 └───────────────────────────▲────────────────────────────┘
                             │
                             │ UNIX Domain Socket: /run/ramsleuth/ramsleuth.sock
                             │ Wire Format: Bincode / JSON-RPC
                             │
 ┌───────────────────────────▼────────────────────────────┐
 │                      ramsleuth-daemon                  │
 │   - Runs as dedicated systemd service                  │
 │   - Linux Capability Bounding: CAP_SYS_RAWIO           │
 │   - Owns hardware handles, PCI mmap, and SMU drivers   │
 └───────────────────────────┬────────────────────────────┘
                             │
         ┌───────────────────┴───────────────────┐
         ▼                                       ▼
┌─────────────────────────────────┐   ┌─────────────────────────────────┐
│       Telemetry Subsystem       │   │    Native Benchmark Engine      │
│  - ryzen_smu (AMD Zen 1-5)      │   │  - AVX2 / AVX-512 SIMD Loops    │
│  - MCHBAR MMIO (Intel 6th-15th) │   │  - Thread affinity & pinning    │
│  - ee1004 / SMBus EEPROM        │   │  - Unrolled pointer chasing     │
└─────────────────────────────────┘   └─────────────────────────────────┘


4.1 Daemon Specifications (ramsleuth-daemon)

Operates as a systemd service (ramsleuth.service).

Grants socket access to wheel or a dedicated ramsleuth user group with permissions 0660.

Enforces process sandboxing (ProtectHome=true, ProtectSystem=strict, NoNewPrivileges=true).

Exposes an asynchronous RPC interface supporting commands:

GetTelemetry: Returns the latest unified timing, clock, and SPD record.

StartBenchmark { target, mode }: Initiates bandwidth/latency passes and streams progress events back over the socket.

CancelBenchmark: Terminates active worker threads cleanly.

4.2 Graceful Degradation & Safety (verified 2026-09-12)

- The **daemon (Phase 3) holds `CAP_SYS_RAWIO`** (plus root where the driver requires it); the GUI/TUI/CLI frontends run completely unprivileged and consume data over the Unix socket.
- Every privileged hardware access — AMD SMU (ryzen_smu sysfs/char-dev), Intel MCHBAR (`/dev/mem` MMIO), SPD — **degrades to `N/A (<reason>)`** when privilege, driver, or hardware is absent: `InsufficientPrivilege`, `DriverMissing`, `UnsupportedHardware`, `NoDevmem`, `UnknownPmTableVersion`, … The telemetry crate has **no panic path on hardware-derived data** (no `unwrap`/`expect`, no unguarded deref; every read bounds-checked), verified live on the reference host with the `ryzen_smu` module *absent*: AMD → `N/A (DriverMissing)`, Intel → `N/A (UnsupportedHardware)`, SPD → live, exit 0.
- The daemon inherits this contract: a failing or unprivileged hardware section is a structured `N/A` in the payload — never a crash of the service or the client.

5. Hardware Telemetry & Register Extraction

5.1 AMD Architecture (Zen 1 through Zen 5)

AMD Zen architectures do not expose active trained timings through standard PCI config space alone; active memory training values are managed by the internal System Management Unit (SMU) and the Data Fabric/Unified Memory Controller (UMC).

The SMU Communication Channel: RamSleuth interfaces with the ryzen_smu kernel driver. It sends message requests through the SMU mailbox registers to read the cryptographic Power Management (PM) table:

Clock Extraction: Extracts actual running frequencies for Fabric Clock (FCLK), Memory Controller Clock (UCLK), and Memory Clock (MCLK). It calculates the synchronous ratio ($1:1$ vs $1:2$) and checks if Gear Down Mode (GDM) or Power Down Mode (PDM) is enabled.

Timing Extraction: Reads the trained timing register arrays populated by AGESA during memory training, including primary timings (tCL, tRCDWR, tRCDRD, tRP, tRAS), secondary timings (tRC, tRRDS, tRRDL, tFAW, tWTRS, tWTRL, tWR, tRFC1, tRFC2, tRFCsb, tCWL, tRTP), and turnaround parameters (tRDWR, tWRRD, tRDRD/tWRWR across same-chip-select and different-chip-select groups).

Impedance & Terminations: Decodes the physical drive strengths (ProcODT, RttNom, RttWr, RttPark) from the UMC PHY registers to display the exact ohm values ($\Omega$) chosen by the motherboard auto-rules or manual user settings.

5.2 Intel Architecture (Skylake through Core Ultra)

On modern Intel architectures, memory timing configuration registers are memory-mapped into physical address space via the Memory Controller Hub (MCH).

MCHBAR Mapping:

The daemon reads the Host Bridge at PCI Configuration Space Bus 0, Device 0, Function 0 (00:00.0).

It reads offset 0x48 to locate the 64-bit physical base address of MCHBAR.

It maps this memory range into the daemon process using mmap() against /dev/mem (or via the kernel's direct MMIO interfaces where restricted).

Channel Offset Decoding:

Iterates across the memory channels (Channels 0–3, depending on whether the system runs Dual-Channel DDR4 or Quad-Subchannel DDR5).

Parses specific register offsets for:

Primary Control: Extracting tCL, tRCD, tRP, tRAS, and Command Rate ($1\text{N}$ vs $2\text{N}$).

Gear Modes: Decoding the System Agent (SA) clock multiplier register to determine Gear 1, Gear 2, or Gear 4 operation.

Tertiary & Turnaround: Extracting Round Trip Latencies (RTL) and Inter-Rank/Intra-Rank turnarounds (tCCD_L, tCCD_S, tRDRD, tRDWR).

5.3 Direct SPD EEPROM Parsing (Module Verification)

Enumerates active I2C/SMBus adapters via /sys/bus/i2c/devices/i2c-*.

Reads the raw 512-byte (DDR4) or 1024-byte (DDR5) EEPROM image exposed by the kernel's ee1004 driver.

Parses JEDEC JEP106 manufacturer IDs to identify module and DRAM die makers (e.g., SK Hynix A-die vs M-die, Samsung B-die, Micron).

Decodes Intel XMP 2.0 / 3.0 and AMD EXPO profiles to show factory-rated profiles alongside live trained values.

5.4 Verification Environment & Driver Requirements (confirmed 2026-09-12)

- **AMD live data requires the `ryzen_smu` kernel module to be installed and loaded** on the dev host (**installed on the primary host since Cycle 4** — amkillam/ryzen_smu v0.1.7 via DKMS). The access channel is **sysfs-first**: read `/sys/kernel/ryzen_smu_drv/pm_table` (plain `std::fs`) + the sibling `pm_table_version` (`TableVersionId`) / `pm_table_size` attrs — the legacy `/sys/kernel/ryzen_smu/pm_table` path is a secondary candidate — falling back to the `/dev/ryzen_smu` character device (ioctl via `nix`, 53XU-fork tolerance only). The `ryzen_smu` Rust crate is deliberately *not* a dependency (direct driver access; self-owned version-guarded PM parse — live-verified for Vermeer in Cycle 4).
- Primary dev host: **AMD Ryzen 9 5950X (Zen 3, Vermeer), 16C/32T, 64 MiB L3, DDR4-3200, AVX2; CachyOS, kernel `7.2.3-1-cachyos-custom`** — `ryzen_smu` is **NOW INSTALLED + LOADED** there (amkillam/ryzen_smu v0.1.7, DKMS — `dkms status` = `ryzen_smu/1.d298366, 7.2.3-1-cachyos-custom, x86_64: installed`; sysfs at `/sys/kernel/ryzen_smu_drv/`, incl. the `smn` attr). Live AMD subtimings **work**: clocks ≈ 1792 MHz OneToOne + VDDCR_SOC ≈ 1.128 V (CAD/timings honest `Na` until the SMN-attr follow-up). Install/management = the idempotent `scripts/install-ryzen-smu-dkms.sh` helper (DKMS staging + build + install + `modprobe` + verify; `AUTOINSTALL=yes` rebuilds on kernel updates) — documented in `Docs/HANDOVER.md` §6 (and `Docs/RamSleuth-v2.md` §"Verification Environment"). Without the module the AMD sections render `N/A (DriverMissing)` (graceful, no panic).
- Intel MCHBAR/IMC path: to be **verified live on the i5-6600 (Skylake, LGA-1151, dual-channel = `channel_count(Skylake) = 2`) test machine**; on the AMD host the path is Intel-gated and returns `N/A (UnsupportedHardware)` with zero `/dev/mem` access.
- SPD (ee1004) needs no privilege and no extra module: live on both hosts (2× DDR4 modules on the 5950X host).

6. Native Benchmark Engine: Matching AIDA64

                           MEMORY BENCHMARK ARCHITECTURE
                           
 ┌─────────────────────────────────────────────────────────────────────────────┐
 │ Parallel Bandwidth Pass (AVX2 / AVX-512)                                    │
 │ Worker Thread 0 (Core 0) ───> [ 64 MB Aligned Buffer A ] ───> Stream to B   │
 │ Worker Thread 1 (Core 1) ───> [ 64 MB Aligned Buffer A ] ───> Stream to B   │
 │ Worker Thread N (Core N) ───> [ 64 MB Aligned Buffer A ] ───> Stream to B   │
 │                                                                             │
 │ Aggregate Throughput: Total Bytes Transferred / Elapsed Wall Time = GB/s    │
 └─────────────────────────────────────────────────────────────────────────────┘
 
 ┌─────────────────────────────────────────────────────────────────────────────┐
 │ Serial Latency Pass (Pointer Chasing Ring)                                   │
 │ Thread 0 (Pinned to Preferred Physical Core)                                │
 │ Traversal: [Node A] ──> [Node K] ──> [Node D] ──> [Node Z] ... (128MB Ring) │
 │                                                                             │
 │ Hardware Prefetchers Defeated via Pseudo-Random 64-byte Strides             │
 │ Cycle Count Measured via Serialized RDTSCP (__rdtscp)                       │
 └─────────────────────────────────────────────────────────────────────────────┘


6.1 Bandwidth Measurement (Read, Write, Copy)

Single-threaded memory benchmarks fail to saturate modern multi-channel memory architectures. A single Zen 4/5 core or Intel Raptor Lake core hits an internal load/store buffer ceiling around 35–45 GB/s. To hit the full $80\text{--}100+\text{ GB/s}$ dual-channel throughput typical of DDR5, RamSleuth uses an explicit multi-threaded worker model:

Topology Awareness & Thread Pinning:

Reads /sys/devices/system/cpu/cpu*/topology/ to build a map of physical cores versus SMT/Hyperthreaded logical siblings.

Spawns exactly one worker thread per physical core on the primary memory controller CCD/socket, pinning each thread via pthread_setaffinity_np(). SMT siblings are intentionally excluded during bandwidth tests to prevent resource contention over CPU vector execution ports.

SIMD Vector Kernels:

Read Benchmark: Threads execute unrolled vector loops loading 256-bit (_mm256_load_si256) or 512-bit (_mm512_load_si512) words from aligned memory into CPU vector registers.

Write Benchmark: Threads write streaming data directly to memory utilizing non-temporal store instructions (_mm256_stream_si256 or _mm512_stream_si512). Non-temporal stores bypass the CPU cache hierarchy entirely, preventing "Read-For-Ownership" cache line allocations that artificially lower observed write throughput.

Copy Benchmark: Threads interleave aligned vector loads from Source Buffer $A$ and write them via non-temporal vector stores to Destination Buffer $B$.

Buffer Sizing for Cache Eviction:

DRAM bandwidth is measured across an allocated buffer size calculated as:


$$\text{Total Buffer} = \max(256\text{ MB},\; 3 \times \text{Total System L3 Cache})$$

This guarantees that all reads and writes miss the CPU cache and hit the DRAM physical bus.

6.2 Latency Measurement (Pointer Chasing)

Bandwidth measures bus width and frequency; latency measures real access delay in nanoseconds. To match the precision of AIDA64's latency test:

The Pointer Ring: RamSleuth allocates a contiguous 128 MB buffer. It populates this buffer with an array of 64-bit pointers arranged as a circular linked list.

Prefetcher Defeat: A linear stride (e.g., traversing memory sequentially in 64-byte steps) triggers the CPU's hardware Stream Prefetcher and L2 Spatial Prefetcher, resulting in false readings that reflect cache latencies rather than DRAM. RamSleuth applies a pseudo-random permutation (Fisher-Yates shuffle across 64-byte cache line boundaries), ensuring every pointer hop forces an unpredicted lookup across different memory rows and banks.

Cycle Measurement: The traversal executes on a single thread pinned to the system's preferred/highest-clocking core. It uses the serialized cycle instruction __rdtscp():

// Conceptual pointer chasing loop in Rust
let mut ptr = buffer_base_ptr;
let mut aux: u32 = 0;
let start = core::arch::x86_64::__rdtscp(&mut aux);
for _ in 0..iterations {
    ptr = *(ptr as *const *const u8);
}
let cycles = core::arch::x86_64::__rdtscp(&mut aux) - start;
let latency_ns = (cycles as f64 / nominal_freq_hz) * 1e9 / iterations as f64;


6.3 Cache Hierarchy Partitioning (L1, L2, L3 Benchmarks)

To populate the complete AIDA64-style grid, RamSleuth executes the exact same bandwidth and latency loops constrained within precise memory boundaries:

L1 Data Cache: Buffer restricted to 16 KB (fits fully inside the 32 KB or 48 KB L1D cache). Reports throughput in the range of 2,000–4,000 GB/s with sub-nanosecond latencies ($0.6\text{--}0.8\text{ ns}$).

L2 Cache: Buffer restricted to 256 KB (fits fully inside 512 KB–2 MB L2 caches). Reports throughput in the range of 1,000–2,000 GB/s with latencies around $2\text{--}3\text{ ns}$.

L3 Cache: Buffer sized to 50% of the active CCD's L3 slice (e.g., 16 MB on standard 32 MB Zen CCDs). Reports throughput around 600–1,200 GB/s with latencies around $9\text{--}12\text{ ns}$.

7. Success & Acceptance Criteria

Feature Completeness: Live readout of active subtimings matching ZenTimings on AMD AM4/AM5 and ASRock Timing Configurator on Intel Core.

Benchmark Parity: Memory bandwidth scores within $\pm 5\%$ of AIDA64 on dual-channel DDR4 and DDR5 configurations; latency within $\pm 2\text{ ns}$.

Privilege & System Safety: No GUI crashes run with elevated privileges; daemon fails cleanly on unsupported architectures without panics or segmentation faults.

Developer Experience: Clean, idiomatic Rust builds with standard cargo build --workspace --release.
