# RamSleuth — Architecture

**Document:** `Docs/Architecture.md` (part of the RamSleuth v2 repository-facing documentation)
**Applies to:** workspace v2.4.2 (branch `v2-development`)
**Audience:** expert readers — kernel-aware systems programmers, packagers, and maintainers who need the full design rationale behind RamSleuth.

Companion documents: [`README.md`](../README.md) (entry point), [`Docs/User_Guide.md`](User_Guide.md) (operational guide), [`packaging/README.md`](../packaging/README.md) (packaging and operator guide).

---

## Table of contents

1. [Overview & design goals](#1-overview--design-goals)
2. [The two-layer model](#2-the-two-layer-model)
3. [Privilege separation & the no-panic contract](#3-privilege-separation--the-no-panic-contract)
4. [The workspace map](#4-the-workspace-map)
5. [The daemon](#5-the-daemon)
6. [The client](#6-the-client)
7. [The wire protocol](#7-the-wire-protocol)
8. [The TUI](#8-the-tui)
9. [The GUI](#9-the-gui)
10. [The telemetry core](#10-the-telemetry-core)
11. [The benchmark engine](#11-the-benchmark-engine)
12. [Install & deployment](#12-install--deployment)
13. [Testing & CI](#13-testing--ci)

---

## 1. Overview & design goals

RamSleuth is a **pure-Rust** memory telemetry and benchmarking suite for AMD and Intel
desktop systems. It has two product layers:

- **Layer 1 — live telemetry.** Continuous, daemon-side observation of the *memory
  controller's operating state*: on AMD Zen silicon, the live SMU clock set
  (MCLK / UCLK / FCLK), the 27 DRAM subtimings read from the SMN register space,
  the gear-down mode (GDM), and the DRAM command rate; on Intel silicon, a
  read-only decode of the memory controller (IMC) registers behind the host
  bridge's MCHBAR window; and — on both — the fully unprivileged decode of every
  DIMM's SPD EEPROM (JEP106 makers, rank, density, base speed, and XMP 2.0 /
  XMP 3.0-EXPO profiles).
- **Layer 2 — the benchmark engine.** An AIDA64-style 4×4 grid of memory
  bandwidth (Read / Write / Copy) and pointer-chase latency across the Memory /
  L1 / L2 / L3 tiers, measured with AVX2 / AVX-512F SIMD kernels, one pinned
  worker per physical core, plus a multi-pass "burn-in" soak mode.

Both layers are exposed through **two frontends** — a ratatui/crossterm TUI and
an egui/eframe GUI — and a CLI, all of which are *unprivileged* processes that
talk to **one small-capability privileged daemon** over a local Unix socket.
This daemon runs as root but holds a single Linux capability, `CAP_SYS_RAWIO`,
and nothing else.

### Design goals

The architecture is shaped by four standing goals:

1. **Privilege separation.** The only process in the whole system that touches
   privileged hardware surfaces (the `ryzen_smu` driver interfaces and the
   `/dev/mem` MCHBAR fallback window — the `ramsleuth_intel` module's sysfs
   attributes are world-readable and need no privilege) is the daemon. Every
   frontend, the CLI, the standalone tools, and even the benchmark workers
   run unprivileged. A
   compromised frontend cannot escalate: it can only speak RPC to a socket
   gated by group membership or a per-user ACL, and it can only ask for the
   daemon's three sanctioned operations (telemetry, benchmark, burn-in).
2. **The no-panic contract.** Missing driver, missing privilege, missing
   hardware, malformed payloads, a wedged daemon, a closed connection — none of
   these ever crashes a RamSleuth process. Structured degradation is the only
   failure mode: fields render `N/A (<reason>)`, the process exits `0`, and the
   user sees *why* the data is absent rather than a stack trace. The daemon
   keeps serving with degraded fields; the frontends keep rendering.
3. **Transparency & provenance.** Every privileged operation is auditable:
   the install flow prints the exact commit, artifacts, and destinations before
   touching the system; the only third-party code (the `ryzen_smu` kernel
   module) is byte-pinned and checksummed; the `ramsleuth_intel` module, by
   contrast, is the project's own original in-repo code (no upstream); every
   released file is byte-identical to a file in the repository; and telemetry
   fields carry their reasons, not silent zeros. A `0` is never drawn as data
   when it means "no source" — absence renders as `N/A`, because a flat zero
   line is a lie.
4. **MSRV 1.75.** The workspace builds, tests, and lints clean on Rust 1.75.
   Every external dependency is chosen (and the dependency tree is pinned) so
   that the `1.75 × stable` CI matrix stays green. This is what keeps the
   precompiled `ramsleuth-bin` AUR package — and the release tarball —
   reproducible and small.

The **v2 two-layer model** is the spine of the project: telemetry (observe what
the hardware is doing) and benchmark (measure what it can do) are independent
crates with frozen interfaces, joined at exactly one point — the wire protocol
— so that new frontends can be added without touching hardware code, and the
hardware code can evolve without breaking the wire.

---

## 2. The two-layer model

### 2.1 Layer 1 — live telemetry

Layer 1 answers the question *"what state is the memory subsystem in, right
now?"*. The daemon aggregates it from four independent providers into one
`SystemMemoryTelemetry` snapshot ([`facade::collect()`](#101-facade--per-branch-containment)),
and the frontends render that snapshot plus a stream of time series.

**AMD (Zen 2 / Zen 3 / …).** Through the `ryzen_smu` kernel driver — *no PECI,
no direct MMIO from userspace* — the daemon reads:

- the SMU PM table: **MCLK / UCLK / FCLK** in MHz, the VDDCR_VDD (Vcore) and
  VDDCR_SOC rails in mV, and the derived UCLK:MCLK divide mode (1:1 coupled or
  1:2);
- **27 DRAM subtimings** in ticks (tCL, tRAS, tRCDRD/WR, tRC, tRP, tRRDS/L,
  tRTP, tFAW, tCWL, tWTRS/L, tWR, the tRDRD and tWRWR DDR/SD/SC/SCL quadruples,
  tWRRD, tRDWR, tRFC1/2/SB) read from the SMN register space via the driver's
  `smn` sysfs accessor;
- **GDM** (global data-path mode) and the **DRAM command rate** (1T/2T), from
  the SMN `0x50200` word.

**Intel.** Two raw sources feed one decode (§10.3). The **primary** is the
`ramsleuth_intel` kernel module, which maps the host-bridge **MCHBAR** window
in kernel space and publishes the raw IMC registers as 24 world-readable sysfs
attributes under `/sys/kernel/ramsleuth_intel/` — no C-side decoding; all
bit-field semantics live in Rust. The **fallback** is a read-only `mmap` of
the same window via `/dev/mem` (the operation that forces `CAP_SYS_RAWIO`),
taken only when the module's kobject is absent. The decode (the
hardware-verified Tier-1 register map) yields the DRAM core clock from
`MC_BIOS_REQ` plus the per-channel timing set — tCL, tCWL, the unified tRCD,
tRP, tRAS, the synthesized tRC, tRRD_S/L, tRTP, tFAW, tWR, tRFC, and the four
turnaround quartets: 24 of the 27 AMD subtiming slots; the rest of the Intel
readout (uclk / fclk / gear / GDM / PDM, the CAD bus, the voltage rails) has
no IMC analog and renders `NotApplicable`.

**SPD (both vendors, fully unprivileged).** The kernel's `ee1004` I2C EEPROM
driver exposes each DIMM's full raw image as a world-readable sysfs attribute.
RamSleuth decodes it in pure userspace: module and die **JEP106 makers**, rank
count and device width, **density**, **base speed** (MT/s), part/serial numbers,
and the **XMP 2.0** (DDR4) / **XMP 3.0 / EXPO** (DDR5) profile blocks. No root,
no driver install, no `unsafe`.

**Platform & board VRM.** DMI sysfs + `/proc` give the vendor-neutral identity
(motherboard, BIOS/AGESA, SMU firmware version, CPU clock, total memory), and a
DMI-keyed `nct6798` board profile overlays the DRAM I/O rail (VDDIO_MEM) for
supported boards.

### 2.2 Layer 2 — the benchmark engine

Layer 2 answers *"how fast is this memory subsystem?"*. The
[`ramsleuth-bench`](#43-ramsleuth-bench) crate implements an AIDA64-style
**4×4 grid**:

| Tier   | Read | Write | Copy | Latency |
|--------|------|-------|------|---------|
| Memory | GB/s | GB/s  | GB/s | ns/hop  |
| L1     | GB/s | GB/s  | GB/s | ns/hop  |
| L2     | GB/s | GB/s  | GB/s | ns/hop  |
| L3     | GB/s | GB/s  | GB/s | ns/hop  |

- **Bandwidth kernels** — hand-written **AVX2** (256-bit, 32-byte blocks) and
  **AVX-512F** (512-bit, 64-byte blocks) read / write / copy routines; the
  512-bit kernels carry their own AVX2 fallback, and the engine picks the family
  at runtime from `is_x86_feature_detected!`.
- **Pinned workers** — the host CPU topology is read from sysfs; exactly one
  worker thread per *physical* core (SMT siblings filtered) pins itself via
  `sched_setaffinity`, and all workers start a pass in lockstep across a
  barrier, so the DRAM bus is driven the way a real multi-threaded workload
  drives it.
- **Pointer-chase latency** — a 128 MiB buffer is materialized as a single
  pseudo-random *ring*: slot `i` stores the index of the next hop one cache
  line (64 B) away, defeating hardware prefetchers so every hop is a true
  dependent load. The chase is timed with serialized `__rdtscp` self-calibrated
  against `Instant`, and the grid cell is the median of three runs over at
  least one million hops.
- **Burn-in mode** — a multi-pass duration run (5-minute soak by default in the
  frontends, or any duration including infinite) that repeatedly executes whole
  passes, streaming one tick per completed cell and per completed per-tier
  latency pass. It is a distinct run class from the single-pass benchmark: the
  two are mutually exclusive daemon-side.

The engine is also a standalone binary (`ramsleuth-bench --avx512 --json`) so a
power user can run the grid directly, without the daemon, on any machine.

---

## 3. Privilege separation & the no-panic contract

### 3.1 The one privileged process

Every privileged read in RamSleuth goes through **one daemon** —
`ramsleuth-daemon` — which runs as **root** with exactly **one** capability,
`CAP_SYS_RAWIO` (bit 21), clamped into its bounding set and granted as an
ambient capability by its systemd unit. All clients are **unprivileged**: they
need only membership in the `ramsleuth` group (or a per-user POSIX ACL on the
socket, granted by the one-click setup — see [§5.3](#53-socket-access-model)) and
connect to the daemon's Unix socket.

**Why `CAP_SYS_RAWIO` specifically?** It is the *only* capability two
hardware surfaces require:

1. **AMD SMU reads** — the `ryzen_smu` driver's sysfs attributes and (on fork
   builds) its character device are gated behind elevated access;
2. **Intel MCHBAR fallback reads** — when the `ramsleuth_intel` module is
   absent, `mmap(PROT_READ, MAP_PRIVATE)` of the MCHBAR window through
   `/dev/mem` is refused without `CAP_SYS_RAWIO` (or root with the
   `STRICT_DEVMEM` restriction lifted). The module's primary path needs no
   privilege at all: its sysfs attributes are world-readable.

Everything else the daemon does — SPD decode, DMI, `/proc`, the benchmark
itself — is unprivileged, which is precisely why the capability floor stays at
exactly one. The daemon performs a **soft privilege probe** at startup:
missing root or `CAP_SYS_RAWIO` produces stderr *warnings* naming which fields
will degrade — the daemon keeps serving, with those fields reporting
`N/A` — and the daemon **never exits** for lack of privilege.

### 3.2 The no-panic contract

The contract has two faces:

- **Telemetry face.** The unit of degradation is the *field*: each value is a
  `Section<T>` — `Value(T)` or `Na(NaReason)`. [`facade::collect()`](#101-facade--per-branch-containment)
  **never returns `Err` and never panics**; a branch failure degrades only that
  branch's fields. Inside a branch, per-register and per-rail containment keeps
  one bad word from poisoning the others.
- **Process face.** Every RamSleuth binary treats *every* failure class —
  driver missing, privilege missing, hardware unsupported, malformed payload,
  daemon down, daemon wedged, protocol violation, closed connection mid-frame —
  as a **structured value**, never a panic or an `unwrap` on hardware-derived
  data. The user-visible consequence: affected fields show `N/A (<reason>)`,
  frontends stay responsive (showing the diagnostic in their status zones),
  and the process exits `0` on a normal run even when everything is N/A. The
  only non-zero exit is a usage/CLI error (exit `2`) or, for the daemon, a
  fatal socket-setup / signal-install failure at startup (exit `1`).

### 3.3 The `NaReason` vocabulary

The reason a field is absent is itself data. The frozen `NaReason` set
([`crates/ramsleuth-telemetry/src/error.rs`](../crates/ramsleuth-telemetry/src/error.rs)):

| Reason                 | Meaning |
|------------------------|---------|
| `UnsupportedHardware`  | The detected vendor/hardware is not served by this branch (e.g. the AMD branch on an Intel CPU, BAR5=0 on a virtualized Intel host). |
| `DriverMissing`        | A required kernel driver is not loaded (e.g. `ryzen_smu` absent → live AMD subtimings). |
| `InsufficientPrivilege`| The read needs more privilege than the caller has (e.g. `/dev/mem` without `CAP_SYS_RAWIO`). |
| `UnknownPmTableVersion`| The SMU PM table's `TableVersionId` is outside the verified layout sets. |
| `NotApplicable`        | The field does not apply to this platform (e.g. gear mode on AMD, Intel voltages). |
| `ParseError(String)`   | A hardware payload was malformed; the string names the field/byte range. |

The renderer layer owns the compact `N/A (…)` display strings, so the same
reason renders identically in the CLI, TUI, and GUI.

---

## 4. The workspace map

The repository is a Cargo **workspace of 8 members** — 7 product crates plus one
development tool — all inheriting a single version:

| Workspace fact | Value |
|----------------|-------|
| Version | **2.4.2** (`[workspace.package].version`) |
| Edition | 2021 |
| MSRV | **1.75** (`rust-version`) |
| Resolver | 2 |
| License | MIT |
| Wire codec | `bincode` 1.3 |
| Async runtime | `tokio` — **the daemon is the workspace's only tokio consumer**; every client/frontend is `std`-only |

The six installed binaries are `ramsleuth-daemon`, `ramsleuth-client`,
`ramsleuth-tui`, `ramsleuth` (the GUI), `ramsleuth-bench`, and
`ramsleuth-telemetry`; `ramsleuth-protocol` is library-only and is never
installed, and `ramsleuth-gen-icon` is a dev tool that is never installed.

### 4.1 Dependency direction

```text
ramsleuth-telemetry  ramsleuth-bench
        \                  /
         v                v
      ramsleuth-protocol
         ^                ^
         |                |
 ramsleuth-client --------+
         ^
    /    |    \
   v     v     v
 TUI    GUI   CLI (the crate's own bin)
         ^
         |
     ramsleuth-daemon  (tokio, spawn_blocking the bench/telemetry calls)
```

Payload types cross the wire **verbatim** from their owning crates
([`SystemMemoryTelemetry`](#10-the-telemetry-core),
`StreamTarget` / `StreamProgress` / `BenchmarkGrid` / `BurnInTick`) — the
protocol crate owns only the four wire enums and the socket-path constant, so
there is no duplicated type surface to drift.

### 4.2 `ramsleuth-telemetry`

**Role:** the hardware core of Layer 1 — all four telemetry providers, the
vendor dispatch, and the no-panic contract types. Both a library (consumed by
the daemon, the protocol, and the frontends) and a standalone binary
(`ramsleuth-telemetry`, the Phase-2 verification CLI: `--json` for a
hand-rolled JSON snapshot, dashboard text otherwise, exit 0 even when every
section is N/A, exit 2 on a bad flag).

| `src/` file | Responsibility |
|-------------|----------------|
| `lib.rs` | Crate root: re-exports `collect`, `SystemMemoryTelemetry`, and the public provider types. |
| `cpuid.rs` | `CpuInfo::detect()` — vendor (`AuthenticAMD` / `GenuineIntel` / unknown), microarchitecture (`AmdZen` Zen1–Zen5 by CPUID family; `IntelGen` Skylake–Arrow Lake), brand string. Pure `__cpuid` intrinsic; the dispatch key of the whole crate. |
| `error.rs` | The contract types: `TelemetryError` (rich cause: vendor, driver, privilege hint, PM-table version, `io::Error`, parse detail), `NaReason` (the six display reasons, §3.3), `Section<T>` (`Value`/`Na`). |
| `amd_smu.rs` | The single privileged gateway to the `ryzen_smu` driver: sysfs-first PM-blob acquisition, version sourcing, the char-device fallback, and safe `nix` wrappers (no `unsafe`). |
| `amd_pm.rs` | Version-guarded parse of the raw PM blob into clocks (MCLK/UCLK/FCLK) and voltages (VDDCR_VDD/VDDCR_SOC). |
| `amd_smn.rs` | The `smn` accessor protocol plus the verified SMN register table (the 27 subtimings, GDM, command rate). |
| `amd_readout.rs` | The four vendor-neutral display types (`ClockReadout`, `TimingSet`, `CadBus`, `VoltageSet`) and the AMD mapping onto them, with sanity gating. |
| `intel_mchbar.rs` | MCHBAR location in PCI config space + the read-only `/dev/mem` RAII window — the fallback source (the one `unsafe` cluster in the crate). |
| `intel_readout.rs` | The hardware-verified Tier-1 IMC register map (MCHBAR-relative) + the single pure decode core fed by both raw sources, into the same display types. |
| `intel_sysfs.rs` | The primary raw reader: the `ramsleuth_intel` kobject's 24 attributes (the 19 IMC-register attributes — 2 MCHBAR diagnostics, `MC_BIOS_REQ`, 16 per-channel `TC_*` — plus the 5 MAD channel/geometry) under `/sys/kernel/ramsleuth_intel/` → the raw `IntelImcRegs` set + the MAD words (per-attribute containment: absent / malformed → `None`). |
| `spd_eeprom.rs` | Unprivileged enumeration + raw-image acquisition of every bound `ee1004` device. |
| `spd_decode.rs` | Pure decode of the raw image: JEP106 makers, rank/density/speed, part/serial, XMP 2.0 / XMP 3.0-EXPO profiles. |
| `platform.rs` | The vendor-neutral identity branch: DMI, `/proc/cpuinfo`, `/proc/meminfo`, the `ryzen_smu` version attribute. |
| `board_vrm.rs` | DMI-keyed `nct6798` board profiles (the VDDIO_MEM overlay and cross-check rails). |
| `facade.rs` | `collect()` — the aggregation + per-branch containment (§10.1). |
| `main.rs` | The standalone `ramsleuth-telemetry` CLI. |

### 4.3 `ramsleuth-bench`

**Role:** the Layer-2 engine, independent of the daemon — all kernels, topology,
worker dispatch, orchestration, and the wire-ready streaming contract. Both a
library (the daemon runs it on a blocking thread) and a standalone binary
(`ramsleuth-bench --avx512 --json`).

| `src/` file | Responsibility |
|-------------|----------------|
| `lib.rs` | Crate root: `CpuFeatures`, `Tier`/`BenchOp`/`Metric`, `BenchmarkGrid`, `run_all`, the `streamed` contract types. |
| `features.rs` | Runtime AVX2 / AVX-512F detection via `std::is_x86_feature_detected!`. |
| `kernel_read.rs` / `kernel_write.rs` / `kernel_copy.rs` | The AVX2 256-bit bandwidth kernels (32-byte blocks). |
| `kernel_512.rs` | The AVX-512F 512-bit kernels (64-byte blocks), each carrying its own AVX2 fallback. |
| `topology.rs` | CPU topology from sysfs: SMT-filtered physical cores, logical CPUs, per-CCD and total L3. |
| `buffers.rs` | Buffer sizing for every tier (L1/L2/L3/DRAM) plus the fixed 128 MiB latency ring; allocation is the caller's job. |
| `worker.rs` | Pinned multi-threaded pass: per-core `sched_setaffinity`, barrier lockstep, block-aligned partitioning, checksum aggregation. |
| `latency.rs` | The pointer-chase ring builder + the `__rdtscp` self-calibrated measurement kernel. |
| `orchestrator.rs` | `run_all` — runs every tier × op, best-of-3 bandwidth / median latency, and fills the 4×4 `BenchmarkGrid`. |
| `streamed.rs` | The wire-ready contract: `run_streamed` (targeted cells, progress events, clean cancel) and `run_burn_in` (multi-pass duration runs, `BurnInTick` events). |
| `main.rs` | The standalone CLI harness. |

### 4.4 `ramsleuth-protocol`

**Role:** the wire vocabulary — the frozen message contract. **Library-only**
(never installed).

| `src/` file | Responsibility |
|-------------|----------------|
| `lib.rs` | Crate root: re-exports the frame codec and message types. |
| `messages.rs` | `Message` (`Request` \| `Response`), the `Request`/`Response`/`BenchMode` enums, and the frozen `DEFAULT_SOCKET_PATH` constant. Payloads are the telemetry/bench types verbatim. |
| `frame.rs` | The length-prefixed bincode 1.3 codec: `encode_frame` / `decode_frame`, the incremental-reader contract, the 16 MiB `MAX_FRAME_SIZE` guard, and `FrameError`. |

### 4.5 `ramsleuth-daemon`

**Role:** the one privileged process — the socket service, the TTL telemetry
cache, the single-flight benchmark job manager, and the only `tokio` consumer
in the workspace. Both a library and the `ramsleuth-daemon` binary.

| `src/` file | Responsibility |
|-------------|----------------|
| `main.rs` | Entry point: CLI (`--socket`, `--max-age`), soft privilege probe, listener setup, the async accept loop, signal handling. |
| `socket.rs` | Synchronous listener setup: parent-dir creation, stale-socket probe, `0660` mode, best-effort group chown, per-user ACL re-application, `tokio::from_std` hand-off. |
| `caps.rs` | The soft privilege probe: root + `CAP_SYS_RAWIO` (bit 21 of `CapEff`) with human-readable degradation warnings. |
| `cache.rs` | The TTL telemetry cache over an injectable collector (default 2 s), with the cold-cache warm-up double read. |
| `spd_bind.rs` | The guarded SPD EEPROM auto-bind fallback: as root (the daemon is the only process that may write here), binds the `ee1004` client(s) the kernel missed when bound-SPDs < channel count — via the i2c `new_device` sysfs write, falling back to the driver `bind` file when the write is refused (the address is occupied by a pre-existing, unbound ACPI/DSDT node, `-EBUSY`) — Intel-only, non-fatal, never unbinds, every attempt (accepted or failed) made at most once per process lifetime; disabled by `--no-spd-autobind` (default on). |
| `dram_spike.rs` | The bounded ~250 ms / 256 MiB DRAM load that pulls the memory controller out of idle before each SMU re-read. |
| `bench_job.rs` | The single-flight job manager: monotonic `run_id`s, the `(target, mode)` folding, progress/burn-in event streams, clean cancel, self-releasing slot. |
| `rpc.rs` | The per-connection async RPC loop: incremental frame decode, request dispatch, owner-connection streaming, `spawn_blocking` offload of the blocking event pumps. |
| `lib.rs` | Crate root: re-exports the public daemon types. |

### 4.6 `ramsleuth-client`

**Role:** the shared unprivileged IPC client — the transport every frontend
consumes. Both a library and the CLI binary (`dump` / `bench` / `status`).

| `src/` file | Responsibility |
|-------------|----------------|
| `client.rs` | The synchronous `std::os::unix::net::UnixStream` transport: connect with retries + backoff, timeouts, incremental frame read/write, one-round-trip `request()`, and the structured `ClientError` (incl. `DaemonDown`). |
| `dump.rs` | The `dump` command: the pure dashboard-style `render` (every cell as value or `N/A (<reason>)`) + the one-RPC `GetTelemetry` variant. |
| `commands.rs` | The `bench` (streamed run + terminal 4×4 grid through the pure `render_grid`) and `status` (per-section health summary) commands. |
| `main.rs` | The CLI: subcommand dispatch, `--socket` / `--tier` / `--mode`, per-command read timeouts, exit codes 0/1/2. |
| `lib.rs` | Crate root: re-exports `Client`, `ClientError`, and the three commands. |

### 4.7 `ramsleuth-tui`

**Role:** the terminal frontend (ratatui + crossterm) — the 16-key dashboard.
Both a library (the pure pieces: key mapping, render, rings, graphs) and the
`ramsleuth-tui` binary.

| `src/` file | Responsibility |
|-------------|----------------|
| `events.rs` | The frozen 16-key contract: pure `key_to_action` mapping + the single crossterm `poll_event` site. |
| `ui.rs` | The three-zone dashboard renderer, `AppState` (wire types verbatim), the semantic palette, the parity strips. |
| `ring.rs` | The TUI-local bounded FIFO ring (default 300 samples = 10 min at the 2 s poll) — a `std`-only mirror of the GUI core, no egui dependency. |
| `graphs.rs` | The five-series graphs state (1800-deep ring = 60 min), the Na-guarded record hook, the window filter, the block-bar sparkline panel, and the CPU-temp source scan. |
| `requirements.rs` | The TUI `diagnose` — the setup-requirements list behind the `[d]` strip. |
| `main.rs` | The binary: terminal `Drop` guard, the background poller thread, the action dispatcher, bench/burn-in workers, snapshot/export, CLI. |
| `lib.rs` | Crate root. |

### 4.8 `ramsleuth-gui`

**Role:** the desktop frontend (egui / eframe 0.27 + `egui_extras` + `png`)
— binary name `ramsleuth`. Both a library (style, update/poller, zones, graph,
history, settings, first-run) and the app shell.

| `src/` file | Responsibility |
|-------------|----------------|
| `main.rs` | The eframe app shell: window (968×600), the 3-line header, F2/F3/Q, the Graphs child viewport, the setup worker, icon, exit codes. |
| `update.rs` | The shared `TelemetryData` state + the background poller thread (the only GUI code that talks to the daemon) and the bench/burn-in workers. |
| `telemetry_zone.rs` | Zone 1 — the memory-controller & subtimings matrix (both vendors, per-channel Intel blocks, semantic cell colors). |
| `bench_zone.rs` | Zone 2 — the 4×4 grid, Run / Cancel, the burn-in row, progress. |
| `status_zone.rs` | Zone 3 — hardware/SPD cards, daemon status, the F2/F3/quit buttons. |
| `graph.rs` | The Graphs window: five-series state + interactive hand-rolled render (hover crosshair, pan, 1/5/15/60-min window, Poll combo). |
| `history.rs` | The trend ring core (300-sample default) reused by the graphs state. |
| `settings.rs` | The in-memory `GuiSettings` knobs (socket, poll interval, refresh, units, theme) + the settings panel. |
| `first_run.rs` | The SETUP requirements strip: `diagnose`, the one-click `Set up RamSleuth` wizard over `pkexec`, the post-setup restart modal. |
| `style.rs` | The dark-slate visual style + the semantic colors (cyan/amber/crimson/slate). |
| `lib.rs` | Crate root: re-exports for tests and the shell. |

### 4.9 `tools/gen-icon`

**Role:** the development tool (binary `ramsleuth-gen-icon`, **never
installed**) that deterministically renders the hicolor icon set
(16/24/32/48/64/128/256/512 px) and the 256 px window-embed master
(`assets/icons/ramsleuth-256.png`) from signed-distance primitives at 4×4
supersampling — pure arithmetic, byte-identical on every run, no metadata in
the PNG output.

| `src/` file | Responsibility |
|-------------|----------------|
| `main.rs` | The parametric icon renderer + PNG writer + post-write decode self-check. |

---

## 5. The daemon

`ramsleuth-daemon` is the single privileged process. It is a **tokio
multi-thread runtime** (`rt-multi-thread` is one of the few features the
workspace enables), because serving many concurrent clients with non-blocking
I/O is its whole job — but all the *expensive* work (telemetry collection,
benchmark runs, and even the blocking event pumps of a running job) is pushed
to `tokio::task::spawn_blocking` so that runtime workers are never parked.

### 5.1 Startup

```text
Usage: ramsleuth-daemon [OPTIONS]
  --socket <path>    Unix socket to listen on (default: /run/ramsleuth/ramsleuth.sock)
  --max-age <secs>   Telemetry cache TTL in seconds, a non-negative
                     integer (default: 2)
  --no-spd-autobind  Disable the guarded SPD EEPROM auto-bind
                     fallback (default: enabled; Intel-only,
                     root-only, non-fatal)
  -h, --help         Print this help and exit
```

1. **CLI parse** — unknown flag / missing value / non-numeric or negative
   `--max-age` → usage text + **exit 2**.
2. **Soft privilege probe** (`caps.rs`) — reads `CapEff` from
   `/proc/self/status` and tests bit 21 (`CAP_SYS_RAWIO`); missing root or the
   capability emits stderr *warnings* naming the fields that will degrade, and
   **the daemon continues serving** (no-panic contract).
3. **Listener setup** (`socket.rs`, synchronous, called from inside the
   runtime so `from_std` can register the socket) — see §5.2.
4. **Shared context** — the TTL cache over the spike-wrapped `collect()`
   (§5.4, §5.5) plus the `BenchJobManager` (§5.6), shared by every connection
   task behind an `Arc`.
5. **Accept loop** — `tokio::select!` over `listener.accept()` and the
   installed SIGTERM / SIGINT handlers; one `handle_connection` task per
   accepted stream.

A fatal socket-setup failure, or a failure to install the signal handlers, is
the only hard startup error: log + **exit 1**.

### 5.2 The socket lifecycle

The socket lives at **`/run/ramsleuth/ramsleuth.sock`** (the protocol's frozen
`DEFAULT_SOCKET_PATH`; `--socket` exists so the daemon can run unprivileged in
`/tmp` for local development). `setup_listener` performs, in order:

1. **Parent directory** — `create_dir_all` (best-effort; failure → a hard
   error carrying a `--socket` hint — never a silent fallback to another path).
2. **Stale-socket probe** — if the path already exists, a
   `UnixStream::connect` probe decides: *live listener* → `AlreadyRunning`
   (a second daemon must never clobber a running one); *dead file* → stale →
   remove and rebind.
3. **Bind** the std `UnixListener`.
4. **`chmod 0660`** — group-readable/writable, world-off: the access contract
   the clients rely on; a failure here is a hard error.
5. **Best-effort group chown** — `ramsleuth` first, `wheel` as the documented
   fallback; a failure only *warns* (the mode still grants the group access).
6. **Per-user ACLs, re-applied on every bind** — the daemon re-applies POSIX
   ACLs (`setfacl -m u:<uid>:rw`) from the state file
   **`/etc/ramsleuth/authorized-users`** (one username per line; dir 0755,
   file 0644) to the freshly bound socket. This runs on *every* socket
   creation — every daemon (re)start and every boot — because `/run` is a
   **tmpfs**: the ACLs do not survive a reboot, so the state file is the
   durable source and the daemon re-applies it. A missing state file only
   notes (the not-yet-seeded fresh install); an unavailable `setfacl`, an
   unknown user, or a failed `setfacl` run only warns — the group path keeps
   working. UIDs are resolved via `getpwnam` so no user-controlled string
   ever reaches the spawned command.
7. **Non-blocking + `tokio::from_std`** hand-off to the async side.

### 5.3 Socket access model

Two complementary mechanisms keep access *current-session-immediate* and
*future-login-persistent* (§12.3 documents the operator side):

- **group** — `0660` + group `ramsleuth` (or `wheel`); effective from the
  next login onward;
- **per-user ACL** — `u:<uid>:rw` from the authorized-users state file,
  effective immediately, re-applied by the daemon on every bind.

### 5.4 The TTL telemetry cache

`collect()` is expensive (CPUID + sysfs + `/dev/mem` + SPD EEPROM), so repeated
`GetTelemetry` RPCs within the TTL (default **2 s**, `--max-age`) reuse the
cached snapshot instead of re-collecting:

- **cold cache** — the first `get()` runs the collector **twice**: the
  warm-up read (spike + settle, below) is *discarded* and the second, settled
  read is the one served and cached — so the first sample a client ever sees
  is an operating-frequency sample, not a cold idle-frequency transient;
- **within TTL** — a plain clone, no collector call;
- **stale** — one re-collect, fresh timestamp, served.

The collector is injectable (`Box<dyn Fn() -> SystemMemoryTelemetry>`), which
is what makes the whole cache unit-testable with a counting mock and no
hardware.

### 5.5 The DRAM spike + clock settle

While the DIMMs sit in a low-power idle state, the SMU PM table's `MCLK`
reads the *idle* frequency. So the production collector is spike-wrapped:

```text
spike()            ~256 MiB buffer, 250 ms of strided read+write traffic
  → sleep 150 ms   CLOCK_SETTLE — let the SMU settle at the operating point
  → collect()      the SMU re-read now samples MCLK at the operating frequency
```

The spike is pure userspace (no privilege, no hardware data, no `unsafe`),
single-flight behind a global atomic gate (a second caller no-ops), and
time-boxed; it runs on the blocking pool immediately before `collect()`.
Its 256 MiB buffer is reserved with a fallible `try_reserve` (the `vec!`
macro panics on allocation failure and would crash the daemon's
`spawn_blocking` collector closure): a transient OOM on a loaded host
skips the spike with a stderr warning and the collector proceeds — the
`MCLK` sample may read the idle frequency once, which beats a dead daemon.
In-TTL clone paths never spike (the collector is not called).

### 5.6 The single-flight benchmark job manager

A bandwidth run monopolizes every pinned core, so **at most one run of
*either* class — a single-pass benchmark or a multi-pass burn-in — is active
at a time**. The manager:

- assigns a **monotonic `run_id`** (first run is 1) that tags the
  `BenchStarted` / `BenchProgress` / `BenchResult` / `BenchCancelled` frames;
- runs the frozen `run_streamed` / `run_burn_in` contracts on a
  `spawn_blocking` thread;
- **streams events only to the owning connection** — a benchmark's progress
  goes to the connection that sent its `StartBenchmark`, a burn-in's ticks to
  the connection that sent its `StartBurnIn`; no other client ever sees a
  run's frames;
- folds the wire's `(target, mode)` pair onto the effective target: `Full`
  keeps the requested target; `MemoryOnly` clamps it to the memory tier (a
  `Cell(·, op)` keeps its op, the tier is clamped);
- treats a second `start` while a run is active as `Busy` → the wire's
  `Response::Error("benchmark already running")`;
- **cancel** — any connection may `CancelBenchmark { run_id }`; the manager
  sets the run's cancel flag, the in-flight pass (milliseconds) always
  completes, and the run stops at the next gate — a clean stop between
  passes/cells, never a mid-pass kill;
- **self-releasing** — the job releases its own slot on completion, so a new
  run may start immediately after the terminal event; and a dying owner
  tears down nothing shared — an unfinished run simply runs to its terminal
  and frees the slot.

### 5.7 Signals & exit codes

SIGTERM and SIGINT stop the daemon **gracefully**: stop accepting, best-effort
socket-file removal, exit **0**. Exit codes: **2** = CLI/parse error (usage
text); **1** = fatal socket-setup or signal-install failure at startup;
**0** = everything else, including a fully N/A degraded run.

### 5.8 The systemd unit

The unit lives at **repo-root [`systemd/ramsleuth.service`](../systemd/ramsleuth.service)**
(not under `packaging/`) and installs to `/usr/lib/systemd/system/` via the
AUR packages / `install.sh`. Key directives:

```ini
[Unit]
Description=RamSleuth privileged memory telemetry and benchmark daemon
Documentation=https://github.com/MadGoatHaz/RamSleuth

[Service]
Type=simple
User=root
Group=ramsleuth                                   # wheel fallback documented
ExecStart=/usr/bin/ramsleuth-daemon --socket /run/ramsleuth/ramsleuth.sock
RuntimeDirectory=ramsleuth                        # systemd creates /run/ramsleuth
ReadWritePaths=/run/ramsleuth                     # the only writable path
CapabilityBoundingSet=CAP_SYS_RAWIO               # the bounding set is clamped
AmbientCapabilities=CAP_SYS_RAWIO                 # to that one capability
NoNewPrivileges=true
ProtectSystem=strict                              # read-only filesystem otherwise
ProtectHome=true
PrivateTmp=true
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

The unit **never loads the `ryzen_smu` module** — live AMD subtimings require
`modprobe ryzen_smu` (or the DKMS extra, §12.4); without it the daemon degrades
per the no-panic contract and keeps serving everything else. A clean signal
shutdown exits 0, so it is not a `Restart=on-failure` trigger; only fatal
startup failures restart.

### 5.9 polkit & the one-click setup helper

The GUI's one-click setup is a **thin client over a pkexec-able root helper**:

- **the policy** — [`packaging/polkit/90-ramsleuth-setup.policy`](../packaging/polkit/90-ramsleuth-setup.policy)
  declares the action **`org.freedesktop.ramsleuth.setup`** (`auth_admin` for
  any/active/inactive/other) and maps `pkexec` to
  **`/usr/bin/ramsleuth-setup`** with any arguments. Polkit is in Arch `base`,
  so the prompt machinery is present on every stock system.
- **the helper** — [`scripts/ramsleuth-setup.sh`](../scripts/ramsleuth-setup.sh)
  (installed as `/usr/bin/ramsleuth-setup`) runs as root — via `pkexec`
  (the GUI passes `--user <you>`, because `$SUDO_USER` is unset under
  pkexec), via `sudo` from a terminal (default `$SUDO_USER`), or a bare
  invocation re-execs under `sudo` — and performs every privileged step of
  the fresh-install flow in one idempotent session:
  1. `systemctl daemon-reload` + `enable --now ramsleuth.service` (the one hard
     step; the group is created defensively if missing);
  2. `usermod -aG ramsleuth <user>` — persisted for *future* logins (skipped
     when already a member);
  3. append `<user>` to `/etc/ramsleuth/authorized-users` — the state file the
     daemon re-applies as socket ACLs on every bind;
  4. best-effort `setfacl -m u:<user>:rw` on the **live** socket — *current*
     session access, **no re-login, no reboot** (warn-only if `acl` is absent);
  5. with `--with-dkms`: **vendor-aware** DKMS routing — `exec` the
     installed vendor helper and return its exit code: AMD →
     `/usr/bin/ramsleuth-install-ryzen-smu-dkms` (the offline pinned
     `ryzen_smu` build), Intel → `/usr/bin/ramsleuth-install-intel-dkms`
     (the in-repo `ramsleuth_intel` build); other / unknown vendor → a
     clear error, no install. `--with-intel-dkms` forces the Intel arm
     (an additive fast path; a hard failure on non-Intel silicon).

  Usage: `ramsleuth-setup [--with-dkms] [--with-intel-dkms] [--user <name>]`;
  exit **0** success (or idempotent no-op), **1** hard failure, **2**
  usage error.

---

## 6. The client

`ramsleuth-client`'s library is the **synchronous IPC transport every
unprivileged frontend consumes** — the CLI binary, the TUI, and the GUI all
call the same `Client`. It is deliberately **`std`-only: no tokio** (the daemon
is the workspace's only tokio consumer), because a frontend's RPC is short and
blocking-friendly, and pulling a runtime into every client would be the wrong
direction.

**Transport facts:**

- connects to the daemon's Unix socket (default `DEFAULT_SOCKET_PATH`,
  overridable per frontend via `--socket`);
- a local `connect` cannot hang — it completes or fails at once — so a daemon
  that is still binding its socket is given **3 connect attempts with short
  linear backoff** (100 ms · (attempt + 1)) before the final failure maps onto
  the friendly error;
- **default 5 s read / write timeouts**, tunable per connection
  (`set_read_timeout` / `set_write_timeout`); the CLI and the frontends raise
  the *bench/burn-in stream* timeout to **120 s between frames** (a run
  streams over minutes, so a legitimate gap far exceeds the 5 s transport
  default — the deadline only bounds a silently wedged daemon);
- reads in 16 KiB chunks through the protocol's **incremental-reader
  contract**: leftover bytes survive across `recv` calls, which is what makes
  streamed benchmark progress work over one connection;
- `request()` performs exactly **one round trip** (send one request frame,
  read one response frame) — the telemetry/health path.

**`ClientError` — the structured diagnostics (no panics):**

| Variant | When |
|---------|------|
| `DaemonDown(String)` | The socket is absent / refused after every retry — carries the *friendly, actionable* text: the socket path + how to start the daemon. This is what frontends surface in their status zones. |
| `Connect(io::Error)` | A non-transient connect failure (not missing/refused). |
| `Timeout` | A read/write deadline expired — the daemon is alive but silent/wedged. |
| `Protocol(String)` | The stream violated the wire contract: an oversized frame, a bincode rejection, or a request frame where a response was due. |
| `Io(io::Error)` | Mid-session i/o failure; a connection closed before a complete frame arrives carries `ErrorKind::UnexpectedEof`. |

The CLI binary dispatches three subcommands over this transport:

```text
Usage: ramsleuth-client [SUBCOMMAND] [OPTIONS]
  dump     Print the dashboard-style telemetry listing (default)
  bench    Run a streamed benchmark (progress lines + the 4x4 grid)
  status   Print the per-section health summary

  --socket <path>                  Daemon Unix socket (default: /run/ramsleuth/ramsleuth.sock)
  --tier <memory|l1|l2|l3|full>    Benchmark tier scope (default: full)
  --mode <full|memory-only>        Benchmark scope (default: full)
```

`dump` is the one-RPC `GetTelemetry` → pure `render` → stdout (a 10 s read
timeout); `bench` is the streamed run — a `BenchStarted` ack, one progress
line per completed cell, then the terminal AIDA64-style 4×4 grid through the
pure `render_grid` (an unmeasured cell — `0.0` on the wire — prints `N/A`);
`status` is the one-RPC per-section summary. Exit codes: **0** success, **1**
daemon/client error (each with its structured diagnostic), **2** usage error.

---

## 7. The wire protocol

The protocol is **binary, length-prefixed, and synchronous-format**: one
`Message` per frame, per direction. It is owned by `ramsleuth-protocol`
(`frame.rs` for the codec, `messages.rs` for the vocabulary) and spoken by
exactly two kinds of endpoint — the tokio daemon and the std clients — which
never reimplement the layout.

### 7.1 Frame format

```text
+----------------+----------------------------------------------+
| 4-byte LE u32  |   bincode 1.3 serialization of the Message   |
| payload length |                                              |
+----------------+----------------------------------------------+
```

- the length prefix counts **payload bytes only** (never itself); a frame is
  at most `4 + MAX_FRAME_SIZE` on the wire;
- **`MAX_FRAME_SIZE` = 16 MiB** — any frame whose *declared* length exceeds the
  guard is rejected as `FrameError::Oversized` **before the payload is
  awaited**, so a hostile or corrupted length prefix can never make a peer
  allocate an absurd buffer;
- the codec is an **incremental reader**: callers append every received byte
  to a buffer and call `decode_frame` repeatedly. `FrameError::Incomplete`
  means "not enough bytes yet — read more" (a transient signal, not a broken
  stream); `Frame.consumed` is the exact number of bytes one frame used, so
  back-to-back frames in one buffer decode in order;
- **no panics**: a bincode rejection surfaces as `FrameError::Decode(msg)`, a
  truncated buffer as `Incomplete`, an oversized declaration as `Oversized`.

### 7.2 Message types

`Message` is one of `Request(Request)` (client → daemon) or
`Response(Response)` (daemon → client). A client-sent `Response` is a
protocol violation and closes the connection.

**`Request`** (frozen — the four arms):

| Arm | Payload | Semantics |
|-----|---------|-----------|
| `GetTelemetry` | — | Fetch the current snapshot (served from the daemon's TTL cache; stays servable *while* a benchmark runs). |
| `StartBenchmark` | `target: StreamTarget`, `mode: BenchMode` | Start a single-pass run. `StreamTarget` = `Full` / `Tier(t)` / `Cell(t, op)`; `BenchMode` = `Full` / `MemoryOnly`. Single-flight — a second start while one is active gets `Response::Error`. |
| `StartBurnIn` | `target: StreamTarget`, `duration_minutes: u32` | Start a multi-pass burn-in; **`0` means infinite** (stop only via cancel); `n > 0` stops once a pass has completed and the run elapsed is `n × 60` s. A distinct run class sharing the single-flight slot. |
| `CancelBenchmark` | `run_id: u64` | Stop the active run `run_id` at its next gate (any connection may send it). |

**`Response`** (frozen — the seven arms):

| Arm | Payload | Semantics |
|-----|---------|-----------|
| `Telemetry` | `SystemMemoryTelemetry` | The full snapshot (~1.2 KiB, carried inline). |
| `BenchStarted` | `run_id: u64` | The run was accepted; `run_id` tags its frames. |
| `BenchProgress` | `StreamProgress` | One streamed event per completed bandwidth cell of a *benchmark* run (owning connection only). |
| `BurnInProgress` | `BurnInTick` | One streamed tick per completed cell / per-tier latency pass of each *burn-in* iteration (owning connection only). |
| `BenchResult` | `run_id`, `grid: BenchmarkGrid` | The terminal result grid (for a burn-in, its last completed pass). |
| `BenchCancelled` | `run_id` | The terminal ack for a cancelled run. |
| `Error` | `String` | The structured error reply — the wire-safe arm of the no-panic contract. |

**Payloads are reused, never duplicated**: `Request` / `Response` embed the
telemetry crate's `SystemMemoryTelemetry` and the bench crate's
`StreamTarget` / `StreamProgress` / `BenchmarkGrid` / `BurnInTick` verbatim —
the single source of truth for each is its owning crate. The snapshot's
`intel` slot is a `Section<IntelReadout>`, which carries the hardware-derived
`channel_mode: Option<ChannelMode>` wire field (decoded from
`MAD_INTER_CHANNEL[1:0]`, §10.3) alongside the raw register set. Every arm is
bincode-serializable; failures cross the wire as structured payloads, never as
panics.

### 7.3 Frozen constants

`DEFAULT_SOCKET_PATH` = **`/run/ramsleuth/ramsleuth.sock`** is the single
socket-path source for the daemon and every client, and is **frozen by a unit
test** that pins the literal (`default_socket_path_is_frozen`), so no change
can slip into the constant unnoticed.

---

## 8. The TUI

`ramsleuth-tui` (ratatui + crossterm) is a **single non-scrolling, three-zone
dark dashboard** driven by a **frozen 16-key contract**. All state is shared
between the render thread and one background poller thread through
`Arc<RwLock<AppState>>`; the wire types ride verbatim.

### 8.1 The 16-key contract

Case-insensitive, modifiers ignored; every other key (`Esc`, `Enter`, arrows,
function keys, mouse, resize) maps to nothing — the terminal re-reads the
surface size each frame, so resize needs no action of its own. The mapping
lives in one pure function (`events::key_to_action`), unit-tested headlessly:

| Key | Action | Effect |
|-----|--------|--------|
| `r` | **Refresh** | Force a telemetry refresh now (one fast RPC on the main thread; works with auto-refresh off). |
| `s` | **Snapshot** | Write the dashboard as a timestamped `.txt` (`ramsleuth-tui-<unix-ts>.txt`) to the **CWD**. |
| `q` | **Quit** | Restore the terminal (raw mode off, alternate screen left, cursor shown) and exit **0**. |
| `b` | **Bench full** | Start a full benchmark run (`StartBenchmark { Full, Full }`). |
| `m` | **Bench memory-only** | Start a memory-only bench (`StartBenchmark { Full, MemoryOnly }`). |
| `x` | **Burn-in** | Start a **5-minute** burn-in soak (`StartBurnIn { Full, 5 }`). |
| `c` | **Cancel** | Cancel the in-flight run (shared flag; the run's own connection sends `CancelBenchmark`). |
| `g` | **Toggle graphs** | Show/hide the 5-series graphs overlay panel. |
| `t` | **Toggle settings** | Show/hide the settings strip (poll interval, units, refresh, socket). |
| `d` | **Toggle requirements** | Show/hide the setup-requirements strip (auto-opens while a requirement is present). |
| `e` | **Export JSON** | Write the current `{ telemetry, bench }` as `ramsleuth-export-<unix-ts>.json` to **`$HOME`** (CWD fallback when `HOME` is unset). |
| `p` | **Cycle poll** | Cycle the poll-interval presets — 100 / 500 / 1000 / 2000 / 5000 / 10000 / 30000 / 60000 ms (default 2 s; wrap). |
| `u` | **Toggle capacity** | Capacity units GiB ↔ GB (display conversion; the wire carries GiB). |
| `k` | **Toggle clock** | Clock units MHz ↔ GHz (display conversion; the wire carries MHz). |
| `a` | **Toggle refresh** | Auto-refresh on ↔ off (a refresh-off TUI still fetches its one-shot baselines and honors `r`). |
| `w` | **Cycle window** | Cycle the graphs time window — 1 → 5 → 15 → 60 min (default 5). |

### 8.2 The three zones

- **Zone 1 — live memory controller & subtimings.** Every cell of the AMD
  (and Intel, if present) readout as `key: value` or `key: N/A (<reason>)`:
  clocks/ratios (MCLK / UCLK / FCLK, UCLK:MCLK divide mode, GDM, command rate),
  the primary / secondary / tertiary + turnaround timing sets, CAD
  drive/termination, and the voltage rails (VDDCR_VDD first).
- **Zone 2 — the AIDA-style benchmark engine.** The 4×4 grid (tier rows ×
  Read/Write/Copy/Latency columns). While a run is in flight it is *live*: a
  normal bench accumulates the streamed progress events (newest value per cell
  wins; non-finite/non-positive readings never count) and a burn-in shows its
  newest per-cell values, with measured cells dimmed by a `…` suffix and
  unstarted cells `N/A`; at rest it shows the terminal grid of the last
  completed run. Below: the status line (`Idle` / `Running… <m:ss>` /
  `Running… (burn-in <m:ss>, iter <n>)` / `Done`), the controls line (`[B]` /
  `[M]` / `[X]` dimmed while any run is in flight, `[C]` shown only while one
  is), and the live burn-in row.
- **Zone 3 — hardware & SPD.** Per-slot module lines (maker / die / part /
  rank / density / speed + XMP/EXPO profiles), the daemon status line, and the
  error line when present.

Over the zones: a 3-line header, the settings strip, the requirements strip
(presence-driven auto-open), and the graphs overlay drawn topmost. The
semantic palette is exact: values cyan `#00D4FF`, warnings amber `#FFB300`,
N/A grey `#8A8A94`, alarms crimson `#FF3B30`, background slate `#1E1E24`.

### 8.3 The graphs overlay

Five series, in fixed order: **CPU FREQ (MHz)**, **VDDCR_CPU (mV)** (Vcore),
**VDDCR_SOC (mV)**, **CPU TEMP (°C)**, **MEM BW (GB/s)**. They ride a
**1800-sample ring = 60 minutes at the 2 s poll cadence** (the TUI-local ring
core defaults to 300 = 10 min; the graphs instantiate it at 1800). One
Na-guarded sample is appended per successful poll — only when at least one
field is finite (an all-NaN poll is a hole, never a point); the ring is
cleared on a disconnected→connected reconnect so a sparkline never straddles
an outage with a gap. The overlay renders one **block-bar sparkline row per
series** (a `▁…█` bar plot quantized to the row's finite min/max span, NaN = a
gap, a series with no finite sample draws label + the N/A note), a
`[w]`-window filter (samples with `t ≥ newest − window·60`), and a sample-count
line.

### 8.4 The background poller

A `std::thread` (the GUI `spawn_poller` precedent adapted to the TUI's fixed
socket):

- each tick it re-reads the live settings knobs (the render-side key writes are
  the one permitted main-thread mutation — no I/O), clamps
  `poll_interval_ms` into the sane **100 ms … 60 s** range, and sleeps the
  live value — a changed knob takes effect on the next tick, no restart;
- **one fresh daemon connection per tick** — that single connect is both the
  *connectivity probe* (a fixed-socket TUI can only observe a daemon restart
  by trying to connect) and, when this tick fetches, the *carrier* of the
  `GetTelemetry` request, so a fetch is exactly one connection;
- the fetch decision: a one-shot **baseline** on the first tick and on every
  disconnected→connected transition (always — a refresh-off TUI is not dead
  on arrival), plus the **cadence** (the continuous 2 s poll) only while the
  refresh gate is on and the interval has elapsed;
- it services **at most one bench command per tick** from the key handler's
  `mpsc` channel — a queued command takes priority over this tick's telemetry
  step, and **telemetry polling pauses for the run's duration**; the run
  streams on the poller thread with the **120 s between-frames read timeout**
  (the client's 5 s default would kill a long run's gap mid-stream);
- a terminal draw cadence of **250 ms** keeps the `…s ago` stamp and progress
  line advancing live; state is locked only briefly per mutation, so the
  render thread never parks across a stream drain.

### 8.5 The one direct hardware read

The TUI is unprivileged in every respect *except one* deliberately
vendor-neutral read: **`read_cpu_temp_c()`** — the runtime source of the CPU
TEMP series. It scans `/sys/class/hwmon/` for the **`k10temp` / `zenpower`
sensor's `temp1_input`** (selected by name, never a fixed `hwmonN` index),
falls back to a **`cpu_thermal` thermal-zone** scan, and degrades every
failure class to `NaN` (std `fs` only, run on the poller thread, never the
render thread). Everything else in the TUI is daemon-served.

### 8.6 Lifecycle & exit codes

Terminal init (raw mode + alternate screen) is wrapped in a `Drop` guard that
restores everything on *every* exit path — normal quit, early return, or
unwind — so the terminal is never left in raw mode. **Exit codes: 0** normal
quit; **1** the terminal could not be initialized; **2** a usage error
(unknown flag / positional / missing value, with usage text).

---

## 9. The GUI

The `ramsleuth` binary is an **egui / eframe 0.27** immediate-mode app (~60
FPS). The version is pinned at 0.27 for a specific reason: **eframe 0.28 is
the first release declaring `rust-version` 1.76**, so 0.27.x (declared 1.72)
is the newest line compatible with the workspace MSRV of 1.75; `egui_extras`
and `png` 0.17 are MSRV-safe as declared.

### 9.1 The shell

- **default window 968×600**, minimum **892×600** (8 margin + 440 minimum
  right column + 8 margin = 892);
- Wayland **app id `RamSleuth`** on both the root and the Graphs child
  viewport;
- an **embedded 256×256 window icon** (`assets/icons/ramsleuth-256.png`,
  decoded once at startup; a decode failure degrades to eframe's default icon
  — no-panic);
- the dark-slate visual style and the same semantic palette as the TUI.

### 9.2 The 3-line header

- **line 1** — the `RamSleuth v<workspace-version>` title (derived at compile
  time from `CARGO_PKG_VERSION`), the platform tag, the daemon status (naming
  the live settings socket), the `Settings` toggle, the `Graphs` window
  toggle, and the **`[F2] snapshot · [F3] export · [Q] quit`** legend;
- **line 2** — CPU brand + live clock, board / BIOS / AGESA;
- **line 3** — RAM total + per-DIMM sizes + max SPD speed, channel mode, and
  the **UCLK:MCLK sync indicator, color-coded**: `1:1` coupled renders in
  value-cyan, while a `1:2` divide (gear desync) renders in **amber** — the
  two warning conditions in the matrix (the desync and an out-of-spec
  VDDCR_SOC) are the only amber cells; critical alarms use crimson.

### 9.3 The SETUP requirements strip

Auto-shown on first launch **while any requirement is present** (it
disappears on its own once every requirement is resolved). `diagnose` turns
the same degradation signals the zones use into actionable rows, each with a
**Copy** button (the polkit-less fallback):

1. **daemon down** (status not `connected*`) → `sudo systemctl enable --now ramsleuth`;
2. **missing `ramsleuth` group membership** (a permission error on the last
   poll — the socket is group-gated) → `sudo usermod -aG ramsleuth $USER`;
3. **AMD silicon with the daemon connected and the AMD branch
   `Na(DriverMissing)`** → `sudo ramsleuth-install-ryzen-smu-dkms` (the detail
   names the pinned upstream). Intel (built-in MCHBAR decode) and healthy AMD
   → no requirements at all.

The **primary** affordance is the one-click **`Set up RamSleuth`** button
(labelled `… + AMD driver` when the AMD module is the missing piece): one
click → **one polkit password prompt** → a *detached*
`pkexec /usr/bin/ramsleuth-setup --user <you>` worker (the render thread only
flips a `running` flag — zero I/O; the spawn and the helper run entirely off
the render thread) → the strip's status line advances `running…` →
`done — restart RamSleuth to activate` (a failed setup keeps the `failed: …`
line). Because the *current* process only picks up the new group membership on
re-exec, a successful setup shows a modal **"Setup complete"** dialog:
**`Restart now`** spawns a detached new instance of the current executable and
exits this process; **`Later`** dismisses it.

### 9.4 The settings panel

The header's `Settings` toggle opens the panel as its own top strip below the
header. Knobs (in-memory this cycle; serde derives are in place for a
documented XDG-persistence follow-up): the **socket field — editing it
*live-retargets the poller*** (a fresh connection per cycle), the **poll
interval** (100 ms … 60 s, default 2 s), the **refresh gate**, the **capacity
units** (GiB ↔ GB) and **clock units** (MHz ↔ GHz), and the **theme**.

### 9.5 The three zones

Zone 1 (telemetry matrix, left), Zone 2 (the 4×4 bench grid with **Run /
Cancel** buttons stacked over the hardware / SPD status, right), and Zone 3
(hardware/SPD cards + daemon status + the F2/F3/quit buttons) take the
central panel's full height. Each frame takes one brief read of the shared
state and repaints on a 16 ms cadence.

### 9.6 The Graphs window

The header's `Graphs` toggle spawns a **dedicated second OS window** — an
eframe 0.27 **deferred child viewport** (one shared context + event loop, no
second eframe lifecycle, Wayland-safe), **900×520**, rendering on its own ~60
FPS cadence: the **five 60-min series** (same source map as the TUI), a
selectable **1/5/15/60-min window** (default 5 min), horizontal pan, a hover
crosshair with a tooltip naming every series at the hovered sample, and a
**Poll combo** (0.5/1/2/5/10 s) that writes the shared poll-interval knob —
the child's one permitted write. While open the root re-registers the viewport
every frame (the keep-alive; egui garbage-collects a child the first frame the
root stops registering it — that is the close); the window's WM close button
clears the same flag the header button toggles, so the two close paths are
behaviorally identical.

### 9.7 Poller / render separation

The **background `spawn_poller` `std` thread owns the daemon socket**: the
telemetry cadence (the live settings knob, default 2 s), the socket itself
(live knob, seeded from the CLI `--socket`), and the benchmark/burn-in stream
all run there. It ticks every 200 ms so an in-flight run's progress frames
stay responsive, performs exactly one baseline fetch per distinct socket value,
appends the Na-guarded history + graphs samples per successful poll, clears
the series on a daemon reconnect, and services bench requests from an
`mpsc` channel (the 120 s between-frames stream timeout). The **render thread
does no I/O** — its only permitted mutations are the settings-panel knob
writes (no I/O) and the status-zone `GuiAction` side effect of F2/F3/Q: F2 /
F3 run a one-shot file write, Q sets the stop flag + closes the viewport.

### 9.8 Export

- **F2 → `ramsleuth-snapshot-<unix-ts>.png`** — the rendered dashboard as a
  PNG (via the `png` crate), written to `$HOME`;
- **F3 → `ramsleuth-export-<unix-ts>.json`** — the current snapshot as
  `{ telemetry, bench }` (the bench grid is `null` before the first completed
  run), written to `$HOME`.

Keys and buttons are behaviorally identical (a fresh key-down fires once —
egui marks OS key-repeats `repeat: true`). **Exit codes: 0** clean quit (the
window close, Q key, or Q button — the app's `Drop` stops and joins the
poller), **1** eframe/display failure, **2** usage error.

---

## 10. The telemetry core

### 10.1 Facade & per-branch containment

[`facade::collect()`](../crates/ramsleuth-telemetry/src/facade.rs) aggregates
every provider into one `SystemMemoryTelemetry` with **per-branch containment**:
each branch degrades independently to a `Section::Na(reason)` (or an empty SPD
list), so a failure in one branch can never affect the others. `collect()`
**never returns `Err` and never panics** — it is the no-panic contract for the
whole crate.

```text
CpuInfo::detect() ──┬─ AMD:      amd_smu::acquire() → amd_pm::parse() → amd_smn::apply_smn (overlay) → amd_readout::map_amd()
                    ├─ Intel:    intel_sysfs::acquire() (primary: ramsleuth_intel kobject) → intel_readout::decode
                    │           └─ on DriverMissing only: intel_mchbar::acquire() → intel_readout::read_intel (/dev/mem fallback)
                    ├─ SPD:      spd_eeprom::acquire() → spd_decode::decode() (per image)
                    ├─ Platform: platform::collect_platform() → dimm_sizes + total_capacity
                    └─ Board VRM: board_vrm::read_board_vrm(&platform.motherboard) → fill-when-Na vddio_mem_mv overlay
```

A vendor branch runs **only on matching silicon**: the AMD branch gates on
`CpuVendor::Amd(_)` and the Intel branch on `CpuVendor::Intel(_)` *before any
provider call*, so a non-matching vendor yields `Na(UnsupportedHardware)`
with **zero I/O** in that branch. The Intel branch additionally gates on the
v1 Tier-1 generation set (`Skylake` / `KabyLake` / `CoffeeLake` /
`CometLake`) before either raw source is touched: any other Intel generation
(Tier 2 / Tier 3 / unrecognized) degrades the whole branch to
`Na(UnsupportedHardware)` — never garbage from a mismatched register map.
The SPD and platform branches run on every vendor (unprivileged sysfs / DMI
+ `/proc` reads).

The snapshot's shape: `cpu` (vendor + brand — the dispatch key), `amd`
(`Section<AmdReadout>`), `intel` (`Section<IntelReadout>`), `spd` (one decoded
module per bound device; empty when the driver is absent), `platform`
(`SystemPlatform` — four fields, each independently degrading to
`Na(NotApplicable)`), `total_capacity` (GiB), and `dimm_sizes` (GiB, parallel
to `spd`). **Total capacity prefers `/proc/meminfo` `MemTotal`** (the OS
ground truth) over the SPD sum, which survives as the fallback; each
`dimm_sizes` entry is `density_mbit × devices / 8192` for the parallel module
(`Na` when that module's density or devices is `Na`, carrying the offending
source's reason).

### 10.2 The AMD path — via the `ryzen_smu` driver

**No PECI, no direct MMIO from userspace.** Everything goes through the
`ryzen_smu` kernel module, and the whole AMD layer contains **no `unsafe`** —
every syscall runs through safe `nix` wrappers with RAII fd guards.

**PM-blob acquisition (sysfs-first).** The canonical kobject is
`ryzen_smu_drv`, so the primary path is
`/sys/kernel/ryzen_smu_drv/pm_table` (the legacy `/sys/kernel/ryzen_smu/pm_table`
is tried second for older/renamed builds). The blob is a **headerless
little-endian `f32` array** (byte offset = index × 4). The SMU version is the
`TableVersionId` — a little-endian `u32` the driver publishes in the *sibling*
`pm_table_version` attribute (the blob carries no version word of its own);
the sibling `pm_table_size` attribute (8-byte LE `u64`) cross-checks the blob
length (a disagreement is a `Parse` error). When the version attribute is
absent (older builds, or the char-device fallback), the version degrades to
the legacy first-word-of-blob extraction so the outcome stays structured —
never a hard error, never a panic. The **fallback** acquisition path opens
`/dev/ryzen_smu` read-only and uses the driver's `read(2)` interface; the
installed amkillam build registers *only* the sysfs kobject (no char node), so
this path is dead on it and is kept purely as 53XU-fork tolerance. Missing
driver → `DriverMissing`; permission error → `InsufficientPrivilege`.

**The parse (version-guarded).** Accepted `TableVersionId` sets: **Vermeer
(Zen 3)** — 10 exact ids — and **Matisse (Zen 2)** — 8 exact ids; any other
word is `UnknownPmTableVersion`. Minimum blob length is `0x518` (326 × f32).
Field offsets (f32 LE, verified on 5950X silicon via `monitor_cpu`):
`0x0A0` VDDCR_VDD (Vcore, volts), `0x0B0` VDDCR_SOC (volts), `0x0C0` FCLK
(MHz), `0x0C8` UCLK (MHz), `0x0CC` MCLK (MHz). Every read goes through a
bounds-checked `read_f32le`; a non-finite or negative float degrades the
field to its zero value instead of poisoning the snapshot. The clock set
yields MCLK / UCLK / FCLK and the derived **UCLK:MCLK divide mode** (`UCLK ==
MCLK` → 1:1 coupled, otherwise 1:2).

**Live subtimings (the SMN accessor).** The driver's `smn` sysfs attribute
(canonical `/sys/kernel/ryzen_smu_drv/smn`, legacy second) is a
**write-address → read-value** protocol: `lseek(0)` → write one 4-byte LE
`u32` address → `lseek(0)` → read 4 bytes, on a single `O_RDWR` fd. **No
register value is ever written** — only the 4-byte address word. The verified
register table:

| SMN reg | Fields (bits) | Snapshot slots |
|---------|---------------|----------------|
| `0x50200` | MCLK set-point `(v & 0x7F)/3 × 100` MHz (6:0); **GDM** (11); command rate 1T/2T (10) | `gdm` + `command_rate` |
| `0x50204` | tCL (5:0); tRAS (14:8); tRCDRD (20:16); tRCDWR (28:24) | `cl` / `ras` / `rcdrd` / `rcwdwr` |
| `0x50208` | tRC (7:0); tRP (21:16) | `rc` / `rp` |
| `0x5020C` | tRRDS (4:0); tRRDL (12:8); tRTP (28:24) | `rrds` / `rrld` / `rtp` |
| `0x50210` | tFAW (7:0) | `faw` |
| `0x50214` | tCWL (5:0); tWTRS (12:8); tWTRL (20:16) | `cwl` / `wtrs` / `wtrl` |
| `0x50218` | tWR (7:0) | `wr` |
| `0x50220` | tRDRD dd/sd/sc/scl (3:0 / 11:8 / 19:16 / 29:24) | `rdrd_dd` / `rdrd_sd` / `rdrd_sc` / `rdrd_scl` |
| `0x50224` | tWRWR dd/sd/sc/scl (same bit layout) | `wrwr_dd` / `wrwr_sd` / `wrwr_sc` / `wrwr_scl` |
| `0x50228` | tWRRD (3:0); tRDWR (12:8) | `wrrd` / `rdwr` |
| `0x50260` | tRFC (9:0); tRFC2 (20:11); tRFC4 (31:22) | `rfc1` / `rfc2` / `rfcsb` |
| `0x50264` | tRFC mirror | mirror-check sentinel |

That is the **27 DRAM subtimings**. Two reference rules are implemented: the
**UMC offset rule** — when the first read of `0x50200` returns exactly
`0x300`, the whole register block is relocated by `+0x100000` (every register,
including the re-read set-point word, is read at `address + 0x100000`) — and
the **tRFC mirror rule** — when `0x50260` reads the sentinel `0x21060138` and
differs from the `0x50264` mirror, the tRFC fields decode from the mirror
word. CAD drive-strength / termination and PDM are *not* published by the
driver; per the confirm-or-Na rule they are not decoded and render as honest
`Disabled` / `Na` — no field is ever displayed with an unverified mapping.
Known limitation, documented: the driver's `smn_result` is a single shared
global, so a concurrent `monitor_cpu` run can race the daemon's collection,
and a failed SMU read leaves the `0xFFFFFFFF` failed-read sentinel — the
overlay treats that word as a failed read (per-register containment: its
fields decode to `0`, never as data).

**The live-subtimings requirement:** the module must be loaded (`modprobe
ryzen_smu`, or the DKMS extra, §12.4); the frozen unit never loads it. Absent
→ the AMD branch's subtiming fields report `N/A (DriverMissing)` and the rest
of the daemon keeps serving.

### 10.3 The Intel path — the `ramsleuth_intel` module, with the `/dev/mem` MCHBAR fallback

**MCHBAR, raw-exposes / Rust-decodes, two sources.** The branch is
vendor-gated on `CpuInfo::detect()` — **on AMD hosts this gate is the entire
behavior: no PCI config is read, no kobject is probed, and `/dev/mem` is
never opened.** On Intel, the raw IMC registers come from two sources that
feed **one** pure decode core:

1. **primary — the `ramsleuth_intel` kernel module** (GPL-2.0, the project's
   own original work — no upstream project, no pin — in the in-repo
   `kernel/ramsleuth-intel/` tree; provisioned from the source bundled by
   the main AUR packages — or by the standalone `ramsleuth-intel-dkms`
   extra, §12.5). It probes the host bridge at PCI `0000:00:00.0`, decodes
   the MCHBAR from config space, `ioremap`s the 64 KiB window, and publishes
   the raw IMC registers as **24 world-readable (`0444`) sysfs attributes**
   under `/sys/kernel/ramsleuth_intel/` — the 19 IMC-register attributes
   (the 2 MCHBAR diagnostics, `MC_BIOS_REQ`, and the 16 per-channel `TC_*`)
   plus the 5 MAD channel/geometry registers (`0x5000`–`0x5010`) — each
   register attribute the raw word, one line, `0x%08x` (the MCHBAR
   diagnostics as `%016llx` and a constant `1`). **No C-side decoding**:
   the module exposes raw values and
   Rust owns the bit-field semantics, so one module spans the supported
   client generations. The kobject exists **only on a fully successful
   probe** (Intel vendor, MCHBAR_EN set, non-zero masked base, `ioremap`
   OK); any probe failure leaves no kobject behind, and on a non-Intel host
   the load fails `-ENODEV` by design — the module is Intel-only. The Rust
   reader (`intel_sysfs`) takes the 24 attributes with per-attribute
   containment: an **absent** attribute and a **malformed** payload both
   degrade to `None` for that register only; only a permission / other-I/O
   failure on a present attribute is reported as a structured
   `TelemetryError`. A missing kobject is the `DriverMissing` outcome that
   triggers the fallback.
2. **fallback — `/dev/mem` MCHBAR, taken ONLY when the kobject is absent.**
   Any other sysfs outcome (`Parse` / `InsufficientPrivilege` / `Io`)
   propagates as-is: the module being loaded means the hardware is
   reachable, and silently switching sources would mask a real fault. The
   fallback maps the window read-only:
   - read the host-bridge PCI config space at
     `/sys/bus/pci/devices/0000:00:00.0/config` (missing device →
     `DriverMissing`);
   - decode the **64-bit MCHBAR field at config offset `0x48..0x50`** —
     `0x48` is the MCHBAR low dword (`0x40` is the EPBAR, a known trap),
     `0x4C` the high dword: `raw = (high << 32) | low`. Bit 0 is
     **MCHBAR_EN** (the window-enable — *not* a PCI I/O-space flag); bits
     15:1 are hardwired 0; bits 38:16 carry the base address. The base is
     `raw & 0x0000007FFFFFF000` — 64 KiB-aligned. A disabled MCHBAR with no
     address bits decodes to base 0 (the unpopulated state typical of
     virtualized Intel hosts) → `UnsupportedHardware` (a clean
     `N/A (unsupported hardware)`); a disabled MCHBAR that carries
     address bits, or a short config image → `Parse`;
   - open **`/dev/mem`** (fallback `/dev/fmem`) and `mmap(PROT_READ, MAP_PRIVATE)`
     the **64 KiB** (`0x10000`) MCHBAR window at that base, owned by an RAII guard whose
     `Drop` calls `munmap` exactly once. This is the
     **`CAP_SYS_RAWIO` requirement**: EACCES/EPERM, or a `STRICT_DEVMEM`
     range rejection surfaced as EIO/ENODATA, → `InsufficientPrivilege`.

   **Why the module exists:** with `CONFIG_STRICT_DEVMEM` (the stock
   setting on major distros) the kernel rejects the `mmap` of the
   non-RAM, PCI-MMIO MCHBAR region (e.g. `0xFED10000`) through `/dev/mem`
   outright, and on UEFI Secure-Boot / integrity-lockdown hosts lockdown
   blocks `/dev/mem` too — and unsigned out-of-tree modules, so there the
   module must be MOK-enrolled first (`mokutil --import ramsleuth_intel.ko`).
   The kernel-space `ioremap` inside the module is the path that works in
   every state the daemon can reach.

The only `unsafe` in the telemetry crate is confined to the fallback's
map/read/unmap path (each block carries a `// SAFETY:` justification):
`PROT_READ` means the guard can never write to the device, `MAP_PRIVATE`
means no copy-on-write page can reach the hardware, the region is
page-aligned, and every register read is bounds-checked against the mapped
window (the module's attributes need none of this: they are plain sysfs
reads).

**The hardware-authoritative Tier-1 register map (MCHBAR-relative).** Tier 1
= Skylake / Kaby Lake / Coffee Lake / Comet Lake: one memory controller, two
channels, DDR4. One global register, two per-channel blocks (channel 0
at `0x4000`, channel 1 at `0x4400`, stride `0x400`), and the MAD
channel/geometry block at `0x5000`:

| Offset | Register | Fields decoded (bits) |
|--------|----------|-----------------------|
| `0x5E00` | `MC_BIOS_REQ` | `[7:0]` CLK_RATIO · `[8]` REF_CLK (0 = 133.3333 MHz, 1 = 100 MHz) · `[17:16]` GEAR_RATIO (Rocket+ only — Tier 2) · `[31]` RUN_BUSY (status) |
| `+0x00` | `TC_DBP` | `[5:0]` tCL · `[13:8]` tCWL · `[21:16]` tRCD · `[29:24]` tRP (6-bit each) |
| `+0x04` | `TC_RAP` | `[5:0]` tRRD_S · `[11:6]` tRTP · `[15:12]` tCKE (**4-bit**, 1–15) · `[23:16]` tFAW (8-bit) · `[31:24]` tRAS (8-bit) |
| `+0x08` | `TC_RFP` | `[10:0]` tRFC (11-bit, 1–2047) · `[27:16]` tREFI (12-bit) |
| `+0x0C` | `TC_RAP2` | `[5:0]` tRRD_L · `[13:8]` tWR (6-bit each) |
| `+0x20` / `+0x24` / `+0x28` / `+0x2C` | `TC_RDRD` / `TC_RDWR` / `TC_WRRD` / `TC_WRWR` | 4×6-bit `sg / dg / dr / dd` turnaround each |
| `0x5000` | `MAD_INTER_CHANNEL` | `[1:0]` channel mode — drives the user-visible channel-mode label (00b symmetric / 01b flex / 10b single / 11b reserved) |
| `0x5004` | `MAD_INTRA_CH0` | channel-0 intra-channel rank/geometry (raw word) |
| `0x5008` | `MAD_INTRA_CH1` | channel-1 intra-channel rank/geometry (raw word) |
| `0x500C` | `MAD_DIMM_CH0` | channel-0 DIMM presence/capacity (raw word) |
| `0x5010` | `MAD_DIMM_CH1` | channel-1 DIMM presence/capacity (raw word) |

The timing unit is **integer DRAM clock cycles** (1 cycle = 2 UI). This
replaces the pre-v2.2.1 skeleton table, which mis-located the frequency
word; the layout is hardware-verified and pinned in CI by the Skylake
DDR4-2400 acceptance fixture (raw `mcbios_req = 0x00000009` → 9 ×
133.3333 = **1200 MHz** MCLK → 2400 MT/s; alt. raw `0x0000010C` =
ratio 12 @ 100 MHz; `tc_dbp = 0x11110F11` →
17-15-17-17; `tc_rap = 0x27180204` → tRRD_S 4 / tRTP 8 / tFAW 24 / tRAS 39;
`tc_rfp = 0x000001A4` → tRFC 420; synthesized tRC = 39 + 17 = 56; channel 1
symmetric).

**The decode (one pure core for both sources).** The 17 raw slots
(`IntelImcRegs` — 1 global + 2×8 per-channel), plus the raw
`mad_inter_channel` word as the decode's third argument (the other four MAD
registers stay raw in the sysfs attributes), feed `intel_readout::decode`:

- `mclk_mhz` ← `MC_BIOS_REQ`: `ratio × refclk` (no ÷2; MT/s = 2 × MCLK),
  sanity-gated to [1, 4096] MHz; a ratio of 0 = unconfigured →
  `Na(ParseError)`;
- **24 of the 27 AMD subtiming slots populated** — `cl`, `cwl`, the
  *unified symmetric* `tRCD` feeding both `rcdrd` and `rcwdwr`, `rp`, `ras`,
  **`rc` synthesized as `tRAS + tRP`** (Intel exposes no tRC register — the
  JEDEC identity), `rrds`, `rrld`, `rtp`, `faw`, `wr`, `rfc1`, and the four
  `TC_RDRD` / `TC_WRWR` quartets; `rdwr` takes `TC_RDWR`'s `dg` field (the
  representative bank-group turnaround), and the coarse `wrrd` slot is
  superseded by the finer `wtrs` / `wtrl` pair from `TC_WRRD` (dg / sg);
  every decoded tick is sanity-gated to [1, 2048];
- `rfc2`, `rfcsb`, `wrrd` → structural `Na(NotApplicable)` (Intel DDR4
  client runs standard single-tRFC scheduling);
- `tCKE` / `tREFI` are decoded by the core but have no frozen display slot
  (the raw values stay available via the sysfs attributes);
- `uclk` / `fclk` / `div_mode` / `gear_mode` / `gdm` / `pdm` /
  `command_rate` → `Na(NotApplicable)` (AMD-fabric concepts with no IMC
  analog; the gear ratio is Rocket+ / Tier 2), and the CAD bus + voltage
  rails → all `Na(NotApplicable)` (the IMC window exposes neither);
- **per-register containment**: an absent / failed register degrades only
  the slots sourced from it to `Na(ParseError)` — never a silent zero,
  never a panic.

**Platform/firmware N/A conditions (not bugs).** Three Intel readout
fields can legitimately read `N/A` on otherwise healthy hardware:
**`tRAS` / `tWTRS` / `tWTRL`** — firmware leaves those TC fields
unprogrammed on some SKUs, so they fail the [1, 2048] sanity gate; the
**2nd DIMM's SPD** — unavailable where the board's DSDT advertises a
single slot (the §10.4 daemon-side auto-bind attempts to recover it);
and **UCLK:MCLK** — `Na` where the uncore ratio is not populated. Each
is a structured `N/A (<reason>)`, never a silent zero, and none
indicates a RamSleuth defect.

**The Tier-1 generation gate.** The decode runs only for
`{Skylake, KabyLake, CoffeeLake, CometLake}`. Any other detected Intel
generation (Alder / Raptor = Tier 2, dual-MC DDR4/DDR5; Meteor / Arrow =
Tier 3, DDR5;
unrecognized) degrades the **whole** readout to `Na(UnsupportedHardware)` —
never garbage data from a mismatched register map. The channel **count**
used for SPD enumeration is a function of the detected generation: 2 for
DDR4-class, 4 for DDR5-class client silicon. The channel-**mode label**
shown to the user is hardware-derived, not derived from the count: `decode`
maps `MAD_INTER_CHANNEL[1:0]` onto `IntelReadout.channel_mode` — `00b` =
Dual-Channel Symmetric (fully interleaved), `01b` = Dual-Channel Flex
(asymmetric), `10b` = Single-Channel, `11b` = reserved (no label) — and when
the module is absent the label falls back to the installed-DIMM count.

### 10.4 The SPD path — fully unprivileged

The kernel's **`ee1004`** I2C EEPROM driver exposes each bound device's entire
contents as a **world-readable sysfs attribute** under
`/sys/bus/i2c/drivers/ee1004/` (one `<bus>-<addr>` symlink per device, e.g.
`5-0052`). Acquisition is plain `std::fs` — **no root, no `CAP_SYS_RAWIO`, no
user-space `/dev/i2c-*`, no `unsafe`**: the raw image is **512 bytes for
DDR4** (4 Kbit) and **1024 bytes for DDR5** (8 Kbit), and the device's I2C
address becomes the module `index`. Any failure degrades gracefully: driver
absent → empty list + a warning; one unreadable device → that device skipped;
a length that is neither 512 nor 1024 → skipped.

The **pure decode** (no I/O at all) yields, per module: **JEP106 maker**
(module and die — two-nibble vendor/continuation codes, with the known-code
table), **rank** count and device width, **density** (Gb), **base speed**
(MT/s), part / serial numbers, and the **XMP 2.0** profiles (DDR4, 32-byte
blocks at `0xD0` / `0xF0`) and the **XMP 3.0 / EXPO** region (DDR5, 256 B at
`0x300..0x400` — four 32-byte profile blocks, coexisting with the DDR5 part
number at `0x200..0x220`). Truncated or malformed fields degrade to `Na`
cells — the decoder can never panic.

**Daemon-side auto-bind (root, non-fatal).** The unprivileged acquisition
above is the read-only half: on an Intel platform where the kernel's
`ee1004` driver bound fewer SPD EEPROMs than active channels (a slot the
board's DSDT failed to advertise), the daemon — the only process
permitted to write — attempts to bind the missing client(s) before each
collection, so a freshly bound EEPROM lands in the same snapshot. The
first mechanism echoes `ee1004 <addr>` to
`/sys/bus/i2c/devices/i2c-<bus>/new_device`; when the kernel refuses
that write (typically `-EBUSY` — the address is already occupied by a
pre-existing client node the firmware instantiated that `ee1004` never
bound), the daemon falls back to the driver's `bind` file, writing the
node's name (`<bus>-<addr>`, e.g. `0-0051`) to
`/sys/bus/i2c/drivers/ee1004/bind`. It is **on by default**
(`--no-spd-autobind` disables it), Intel-only, strictly non-fatal
(every attempt — accepted, bind-rescued, or failed — is recorded in a
process-lifetime set and made at most once; a daemon restart
re-attempts), and **never unbinds**: a bound EEPROM is a real DIMM the
DSDT missed, a persistent desired state.

### 10.5 The platform branch

Vendor-neutral identity, unprivileged: **`/sys/class/dmi/id/*`** for
motherboard (`board_name` → `board_vendor` → `product_name`), BIOS
(`bios_version`, plus date when present), and the AGESA token (extracted from
the DMI BIOS string only — the real AGESA string is root-gated, so commonly
absent); **`/proc/cpuinfo`** `cpu MHz` (first core) for the live CPU clock;
**`/proc/meminfo`** `MemTotal` for the total-capacity ground truth; and the
`ryzen_smu` driver's `version` attribute (shape-checked, never verbatim) for
the SMU firmware version. Each of the four `SystemPlatform` fields degrades
independently to `Na(NotApplicable)` — a missing source is an honest N/A,
never a placeholder.

### 10.6 Board VRM

A **static, data-driven profile registry keyed on the DMI board name**
(the Crosshair VIII Hero's `nct6798` Super-I/O exposes in0–in14 *without
labels*, so only a DMI-keyed index table is an honest mapping): **in13 =
VDDIO_MEM** (the DRAM rail, ~1.2 V), **in0 = Vcore** and **in6 = SoC**
(cross-check only — the SMU PM table is the trusted source for those). The
Linux hwmon driver already applies the board's resistor divider, so every
`in{N}_input` is read as **millivolts, as-is**. Unknown board, missing
`nct6798` device, or one bad channel degrades *that rail only* (per-rail
containment, `Na(ParseError(…))`; the VDDIO_MEM tripwire band is 100–2000 mV)
— and the facade applies the read as a **fill-when-Na overlay** onto the
existing `vddio_mem_mv` slot (a carried `Value` is never clobbered).

---

## 11. The benchmark engine

### 11.1 The kernels

`features.rs` detects **AVX2 / AVX-512F** at runtime via
`std::is_x86_feature_detected!` (no `unsafe`). The bandwidth kernels:

- **AVX2** (`kernel_read` / `kernel_write` / `kernel_copy`) — 256-bit
  loads/stores, 32-byte blocks;
- **AVX-512F** (`kernel_512`) — 512-bit, 64-byte blocks, each kernel carrying
  its **own AVX2 fallback** so a forced `--avx512` on a machine without
  AVX-512F degrades cleanly.

### 11.2 Topology & pinned workers

`topology.rs` enumerates `/sys/devices/system/cpu`: one **representative
(lowest-index) logical CPU per physical core, SMT siblings filtered out**, all
logical CPUs, total L3 (deduplicated by `shared_cpu_list`), and the largest
per-CCD L3 slice. `worker.rs` then runs one pass:

1. **pre-flight pin probe** — the calling thread briefly pins itself to the
   first representative CPU and restores its mask; on failure the whole pass
   runs on a single unpinned thread (`pinned = false`) — the measurement
   stays honest about what it did;
2. **fan-out** — one `std::thread` per physical core, each pinning itself
   *before* all workers cross a `Barrier`, so kernel loops start in lockstep;
3. **work** — the buffer is partitioned into contiguous **block-aligned**
   slices (32 B for AVX2, 64 B for AVX-512); every worker except the last
   gets an equal base chunk and the **last worker takes the remainder tail**,
   so every byte is covered exactly once; small tiers re-run the pass to
   amortize spin-up overhead, large tiers run exactly one pass;
4. **aggregation** — per-worker byte counters summed and per-slice word-sum
   checksums wrapping-added; the checksum is order-independent, so the
   aggregate is verifiable against a single-thread reference.

Caller contract (checked; violations are `WorkerError`, never UB): equal
lengths, block-aligned bases, length a multiple of the block, and non-aliasing
src/dst for Copy.

### 11.3 Buffers

Sizing is `plan()` (`buffers.rs`) — **sizing only, never allocation**; every
size rounds up to a 64-byte (cache-line) multiple and stays page-aligned:

| Tier | Rule |
|------|------|
| L1 | host L1d from sysfs (scanned by `level`/`type`, never a bare index); 32 KiB fallback |
| L2 | host L2 from sysfs; 1 MiB fallback |
| L3 | **one per-CCD slice** (keeps the pass inside a single CCD's L3) |
| DRAM (Memory) | `max(256 MiB, 3 × total system L3)` |
| latency ring | **fixed 128 MiB** |

### 11.4 Pointer-chase latency

`latency.rs` builds a **single-cycle ring** — slot `i` stores the index of
the *next* hop — so traversing from any start visits every slot exactly once.
The orchestrator **materializes** the ring into the 128 MiB buffer with each
slot as a little-endian `u64` at physical offset `64·i` — consecutive hops one
cache line apart in pseudo-random order, defeating hardware
stream/spatial prefetchers so every hop is a true dependent memory access.
`chase_latency_ns` walks it as a strictly dependent load chain
(`idx = ring[idx]` — the CPU cannot overlap or prefetch hops):

- **x86_64:** serialized **`__rdtscp`** at start and end — `RDTSCP` waits for
  the whole dependent chain to complete — with the cycle count
  **self-calibrated against `Instant` over the same run** (no hardcoded TSC
  frequency, robust to power/thermal scaling);
- **non-x86_64:** `Instant` only (reduced precision, documented).

Hop count: at least one full traversal of the cycle *and* at least **one
million hops** for a stable average.

### 11.5 The orchestrator & the 4×4 grid

`run_all` executes every tier × op and fills the grid:

- **bandwidth:** **3** wall-timed passes per tier; the cell is
  `total_bytes / best_elapsed` — **best-of-3**, which trims the first run's
  cold start (page faults, TLB warmup, thread spin-up);
- **latency:** 3 runs over the materialized full-tier ring (chasing the whole
  tier buffer — not the compact 8-byte-entry ring — puts the Memory-tier
  working set beyond L3: true DRAM row latency, not an L3 one); the cell is
  the **median** ns/hop.

The standalone binary prints the grid as a fixed-width table (an unmeasured
cell prints `N/A`) and, with `--json`, a serde-free JSON object.

### 11.6 The wire-ready streaming contract

`streamed.rs` is the contract the daemon and the protocol consume —
**`run_streamed`** and **`run_burn_in`**:

- a **cell** is one (tier, op) bandwidth pair — the grid's 12 bandwidth cells,
  tier-major; `StreamTarget` selects `Full` (all 12 cells + the four per-tier
  latency passes), one `Tier` (its 3 cells + its latency pass), or one `Cell`;
  unrequested cells stay `0.0` in the returned grid;
- `run_streamed` detects the topology itself, sizes every tier via `plan`,
  picks the kernel family at runtime, and bounds the pinned worker count via
  `StreamOptions::threads` (`0` = one per physical core; `n > 0` truncates to
  the first `n`); after each completed bandwidth cell one
  `StreamProgress` (cell index / total, tier, op, value, label) is emitted
  through the caller's channel (a dropped receiver is ignored — the `.ok()`
  contract); the latency pass is a per-tier measurement with no progress event
  of its own;
- **cancellation** — the cancel gate is a relaxed load checked *before the
  run starts* and *between cells* (before every bandwidth and latency pass):
  a hit completes the in-flight pass (milliseconds) and stops the run
  cleanly — `StreamError::Cancelled`, partial grid discarded, no panic, no
  leaked threads (every pass is a synchronous, self-joining scoped dispatch);
- **`run_burn_in`** loops whole passes (fresh zero grid each iteration) until
  cancel, or — for a finite `duration_minutes` — the first completed pass
  whose run-elapsed time reaches `duration_minutes × 60` seconds (that pass's
  grid is returned); **`duration_minutes == 0` runs until cancelled**
  (infinite). Each completed bandwidth cell and each completed per-tier
  latency pass emits one `BurnInTick` — the 1-based iteration, the
  run-elapsed seconds at the emit, and exactly one of a bandwidth
  `(op, GB/s)` value or a latency `ns` value.

The daemon runs these on a `spawn_blocking` thread and forwards the events to
the owning connection as `BenchProgress` / `BurnInProgress` frames (§5.6) —
which is why the frontends' bench streams use a 120 s between-frames timeout
rather than the client's 5 s transport default.

---

## 12. Install & deployment

### 12.1 `install.sh`

The self-contained GitHub installer (repo root; also installed to
`/usr/share/ramsleuth/install.sh` for post-install re-runs and auditing).
`git clone … && ./install.sh` lands the system in **exactly the state of the
AUR `ramsleuth` package**: the 6 binaries (built with
`cargo build --release --workspace --locked` — RamSleuth compiles only this
repo), the frozen unit, the preset, the `ramsleuth` group, the one-click setup
helper, and its polkit policy. Properties:

- **root** (invokes via `sudo`), **self-contained**, **Arch-targeted**;
- **transparent** — before anything touches the system it shows the host,
  the exact commit being installed, and every artifact + destination, then
  asks `Proceed?` (decline → exit 0, no change);
- **idempotent** — a re-run is always safe; a failure prints a clear message
  and exits non-zero (never a silent half-state);
- it seeds the **`ramsleuth` group + the `authorized-users` ACL** for the
  invoking user;
- on **AMD hosts it asks** about the optional `ryzen_smu` DKMS module and, on
  `y`, hands off to the shared pinned helper (below).

Exit codes: **0** installed *or* declined (no change), **1** hard failure,
**2** bad usage / preflight refusal (not a checkout, non-Arch, no Rust
toolchain).

### 12.2 The AUR packages

| Package | What it is | When to use it |
|---------|------------|----------------|
| **`ramsleuth`** | STABLE source — builds the workspace from the official **`v$pkgver` git tag** (reproducible, auditable snapshot) | the default recommendation for production installs |
| **`ramsleuth-bin`** | PRECOMPILED — downloads the release tarball `ramsleuth-$pkgver-x86_64.tar.zst` from the official GitHub Release, pinned by `sha256sums` (no build, no makedepends) | the fastest install path |

Both install the identical file set and **mutually conflict** — the user
picks exactly one. Both install the one-click artifacts (helper + polkit
policy — `ramsleuth-bin` takes them from the release tarball), and their
`post_install` hooks perform a **zero-touch grant** when run under `sudo`:
group membership (persistent, next login) + the `authorized-users` seed
(current-session socket access) + a daemon restart so the ACL applies
immediately.

The full install file set (see `packaging/README.md` for the operator
reference): 6 binaries → `/usr/bin/`; the unit → `/usr/lib/systemd/system/`
(+ preset); the `ramsleuth` group (idempotent `groupadd -r` in the `.install`
hooks); the DKMS helper → `/usr/bin/ramsleuth-install-ryzen-smu-dkms`; the
Intel DKMS helper → `/usr/bin/ramsleuth-install-intel-dkms` (guarded —
skipped with a note in a pre-Intel checkout); **the in-repo
`ramsleuth_intel` source tree → `/usr/share/ramsleuth-intel-dkms/src/`**
(guarded the same way — the exact path the Intel helper resolves, so the
one-click Intel DKMS install works from a bare AUR install with no manual
source step); the setup helper → `/usr/bin/ramsleuth-setup`; the polkit
policy → `/usr/share/polkit-1/actions/`; and `install.sh` →
`/usr/share/ramsleuth/`. `ramsleuth-protocol` is library-only and is never
installed. Two optional DKMS extras provision the vendor kernel drivers —
`ryzen-smu-dkms` (§12.4) and `ramsleuth-intel-dkms` (§12.5). The AMD extra
is neither a dependency nor a conflict of the two packages (co-install-safe);
the Intel extra is **mutually exclusive** with them (each declares the other
in `conflicts=` — they share the bundled source-tree path) and is the
standalone provisioning path, redundant for Intel once a main package is
installed.

### 12.3 The `ramsleuth` group + ACL model

The socket is `0660` group-owned; a client needs **group membership** *or* a
**per-user ACL** to read telemetry. There are two access mechanisms, and after
any setup path **neither a re-login nor a reboot is needed**:

1. **current session — immediate, no re-login:** a POSIX ACL on the socket
   (`u:<user>:rw`). The daemon re-applies these ACLs from
   `/etc/ramsleuth/authorized-users` (dir 0755, file 0644, one username per
   line) on **every** socket creation — every daemon (re)start and every
   boot, since the socket lives on tmpfs `/run`;
2. **future logins — persistence:** group membership via
   `usermod -aG ramsleuth <user>` (the hooks / `install.sh` / the helper do
   this automatically for the invoking user). PAM applies group membership
   only at login time, so this half covers the next and later sessions — it
   is kept so a plain re-login keeps working.

On systems that cannot provide the group, the unit documents a `Group=wheel`
fallback.

### 12.4 The `ryzen-smu-dkms` extra (live AMD subtimings)

**Not a hard dependency** — without the module, RamSleuth degrades gracefully
(`N/A (DriverMissing)`, exit 0, no panic). When present:

- the **source is pinned, never branch-HEAD**: `amkillam/ryzen_smu` @
  `d2983668300dd2a598e5a7dc40e71ce0678cc270`;
- resolution order: **offline-vendored, byte-frozen** copies of the six pinned
  files (installed at `/usr/share/ryzen-smu-dkms/vendor/ryzen-smu`, verified
  against `SUMS.sha256` — a mismatch dies, no silent fallback) first, then a
  **pinned git clone** with a hard `HEAD == pin` verify (the tarball
  deliberately excludes the vendor dir for size);
- built **only by DKMS** on the target (the `ryzen-smu-dkms` AUR extra is thin
  by design — no module build in the chroot), `modprobe` is immediate (no
  reboot), and the load persists via `/etc/modules-load.d/ryzen_smu.conf`;
  `AUTOINSTALL=yes` rebuilds on kernel updates;
- licensing: the driver is **GPL-2.0**, a **separate work** from the MIT
  RamSleuth code — byte-identical and unmodified, carrying its verbatim
  `LICENSE` + `NOTICE.md`, never compiled into, linked with, or bundled in any
  RamSleuth binary (the vendored source ships *only* via the extra);
- the same helper script installs under **two names** to stay co-install-safe:
  `/usr/bin/ramsleuth-install-ryzen-smu-dkms` (all ramsleuth packages +
  `install.sh`; the name `ramsleuth-setup --with-dkms` delegates to) and
  `/usr/bin/ryzen-smu-dkms-install` (the standalone extra only).

### 12.5 The `ramsleuth-intel-dkms` extra (live Intel subtimings)

**Not a hard dependency** — without the module, the Intel section degrades
(`N/A (DriverMissing)`, exit 0, no panic), and the `/dev/mem` MCHBAR
fallback remains available where unblocked. When present:

- the **source is in-repo, never cloned**: the `kernel/ramsleuth-intel/`
  tree (GPL-2.0 — a separate work from the MIT RamSleuth code, byte-verbatim,
  never compiled into any RamSleuth binary) — unlike the AMD extra's pinned
  `ryzen_smu` clone, there is no network and no upstream pin;
- **both main packages bundle that source tree** (to
  `/usr/share/ramsleuth-intel-dkms/src/` — guarded like the Intel helper, so
  a pre-2.4.2 tag / the published v2.2.1 tarball ships nothing and skips
  cleanly), which is the exact path the helper resolves as its installed
  copy: the **one-click Intel DKMS install works from a bare AUR install**
  (either `ramsleuth` or `ramsleuth-bin`) with **no manual source step**;
- the standalone AUR extra is a **thin provisioning package** (mirroring
  `ryzen-smu-dkms`): it ships the DKMS config, the operator helper, and the
  module source under `/usr/share/ramsleuth-intel-dkms/` — the module is
  **never built in the build chroot** (no matching kernel headers, and it
  would build against the chroot kernel, not the target's); the
  `dkms add` / `build` / `install` run on the **target** via the helper, and
  `AUTOINSTALL=yes` rebuilds the module on kernel updates;
- the **helper is installed under two names**:
  `/usr/bin/ramsleuth-install-intel-dkms` (both `ramsleuth` packages +
  `install.sh`; the name `ramsleuth-setup --with-dkms` /
  `--with-intel-dkms` delegates to, §5.9) and
  `/usr/bin/ramsleuth-intel-dkms-install` (the standalone
  `ramsleuth-intel-dkms` extra only); the distinct names kept the AMD extra
  co-install-safe, but the Intel extra is now **mutually exclusive** with
  the mains (shared `/usr/share/ramsleuth-intel-dkms/src/` path — installing
  it removes a main package first);
- the helper is **vendor-aware and non-Intel-safe**: on a non-Intel host
  (e.g. an AMD dev box) the module builds fine, `modprobe` leaves it idle
  (the module's vendor gate rejects with `-ENODEV` by design — no kobject),
  and the helper exits **0** with a clear note ("the `/dev/mem` fallback
  will be used"); the at-boot load entry
  (`/etc/modules-load.d/ramsleuth_intel.conf`) is written **only** on a
  successful Intel load (a stale one is removed on the non-Intel path);
- the **frozen kobject contract** is the 24 world-readable attributes under
  `/sys/kernel/ramsleuth_intel/` (§10.3): the module creates the kobject
  only on a fully successful probe, and any probe failure leaves no kobject
  behind;
- like the AMD unit contract, the **systemd unit never loads the module** —
  the operator does, via the helper / `modprobe`;
- **Secure Boot / integrity-lockdown limitation** (the same class the AMD
  extra carries): on UEFI Secure-Boot hosts the kernel blocks both
  `/dev/mem` and unsigned out-of-tree modules, so the module must be
  MOK-enrolled first (`mokutil --import ramsleuth_intel.ko`); a host that
  can neither load the module nor map the window degrades the Intel section
  to the corresponding structured N/A (`DriverMissing`, or
  `InsufficientPrivilege` when the fallback's map is the one blocked).

---

## 13. Testing & CI

### 13.1 The test suite

**The full workspace test suite green** in debug **and** release, with
`cargo clippy --workspace --all-targets -- -D warnings` reporting **zero
warnings**. All tests are **in-crate unit tests** (there are no `tests/`
integration dirs and no `benches/` dirs) — the crate interfaces are designed
so the interesting surfaces are pure and testable headlessly:

- **protocol freeze** — `default_socket_path_is_frozen` pins the
  `DEFAULT_SOCKET_PATH` literal; every `Request` / `Response` / `Message` arm
  round-trips through bincode (including the appended `StartBurnIn` arm over
  the full target range and the infinite/finite durations); the frame codec
  tests cover the exact `[4-byte LE length][payload]` layout, every
  truncated-buffer cut of a real frame (all `Incomplete`), a hostile
  length prefix (`Oversized` *before* the payload is awaited), a length
  exactly at the 16 MiB cap (`Incomplete`, not `Oversized`), a bincode-rejected
  payload (`Decode`, never a panic), and two back-to-back frames decoding in
  order with exact `consumed` accounting;
- **socket setup** — tokio-based tests against real temp-dir sockets: a fresh
  path binds at mode `0660` with a live listener; a stale file is removed and
  rebound; a live listener is reported `AlreadyRunning` without touching it;
  an uncreatable parent yields `DirCreate` with the `--socket` hint; the
  authorized-users parser trims/dedupes; the ACL step degrades without
  panicking;
- **daemon** — the `TelemetryCache` semantics over an injectable counting
  mock (cold double-read warm-up, in-TTL clone, stale re-collect), the
  `BenchJobManager` single-flight / cancel / slot-release rules, and the RPC
  loop exercised **over `UnixStream::pair()`** — real frame bytes through a
  real socket pair, no hardware;
- **TUI** — the whole 16-key contract as pure `key_to_action` mapping tests
  (every key, case pairs, modifier-ignored, and every non-key → `None`), plus
  **headless ratatui `TestBackend` render tests** of the three zones, strips,
  and graphs panel over synthetic snapshots;
- **telemetry** — vendor-conditional tests (the AMD/Intel branches gate on
  `CpuInfo::detect()`, so the same suite stays portable on Intel, AMD, and
  virtualized CI hosts — unsupported branches assert their clean `Na`
  outcome), the PM-blob / SMN / SPD decode tables against fixtures, and the
  `NaReason` / `Section` bincode round-trips;
- **client / GUI** — the CLI parsers, `render` / `render_grid` over fixtures,
  and the GUI's headless egui-context render tests (zones, graph window,
  modals) against synthetic state.

### 13.2 CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on push and PR
to `v2-development`:

- **test** — matrix **`1.75` (MSRV) × `stable`** on `ubuntu-latest`
  (`fail-fast: false`): `cargo test --workspace` (debug),
  `cargo test --workspace --release`, and
  `cargo clippy --workspace --all-targets -- -D warnings`. The committed
  `Cargo.lock` is used as-is; the MSRV leg proves the 1.75 claim on a clean
  runner;
- **build** — `cargo build --release --workspace` on stable, uploading the
  **6 release binaries** as a workflow artifact.

### 13.3 The release pipeline

[`.github/workflows/release.yml`](../.github/workflows/release.yml) fires on
a **`v[0-9]*` tag** push (or a manual `workflow_dispatch` with a version
input). It builds the workspace `--locked`, **verifies all 15 release
artifacts** (6 binaries + 9 auxiliary: `systemd/ramsleuth.service`,
`ramsleuth.preset`, `RamSleuth.desktop`,
`scripts/install-ryzen-smu-dkms.sh`, `install.sh`, `LICENSE`,
`scripts/ramsleuth-setup.sh`,
`packaging/polkit/90-ramsleuth-setup.policy`, and the `assets/icons` hicolor
tree — the tree counts as one artifact), then packages the
**deterministic tarball `ramsleuth-<ver>-x86_64.tar.zst`** (fixed file order
`tar --sort=name`, fixed ownership `--owner=0 --group=0 --numeric-owner`,
fixed mtime `--mtime=@0`) with its **`.sha256` companion**, and publishes
both as the GitHub Release. **The tarball is the `ramsleuth-bin` download
source** — the AUR package pins it by `sha256sums`.

### 13.4 Related documentation

- [`README.md`](../README.md) — what RamSleuth is, installation, quickstart;
- [`Docs/User_Guide.md`](User_Guide.md) — running the GUI / TUI / CLI /
  standalone tools, reading the data, N/A reasons, day-2 operations;
- [`packaging/README.md`](../packaging/README.md) — the two-package AUR model,
  the version-bump standing policy, the `ryzen-smu-dkms` extra, and the
  CI/release artifact contracts.
