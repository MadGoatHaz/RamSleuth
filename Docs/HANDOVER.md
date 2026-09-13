# RamSleuth v2 — Development Handover

> **Audience:** the next development cycle (**Cycle 4 / Phase 5 — Packaging & Distribution**). Self-contained: read this first, then `plans/` and the other `Docs/` files.
> **Date:** 2026-09-13. **Branch:** `v2-development` — **LOCAL ONLY, not pushed.**
> **Sources of truth:** this file + `Docs/RamSleuth-v2.md` (roadmap) + `Docs/Grand Design & Architecture Specification.md` (spec) + `plans/PLAN.md` (Phase 1 — done) + `plans/PLAN-PHASE2.md` (Phase 2 — done) + `plans/PLAN-PHASE3.md` (Phase 3 — done, incl. the Phase 4 TUI/GUI) + `FULLSCOPEvsCOMPLETED.md` (full scope vs. completed) + `MASTER_LOG.md` (durable cycle history, Cycles 1–3) + `DEV_LOG.md` (lease board — currently empty).
>
> **Full scope reference:** for the full project scope vs. completed breakdown (with mermaid map), see `FULLSCOPEvsCOMPLETED.md` at the workspace root.

---

## 1. Project Overview

RamSleuth v2 is a dual-layer hardware introspection + benchmarking utility for Linux x86_64:

1. **Live memory-controller telemetry** — active trained subtimings, clocks (MCLK / UCLK / FCLK, gear/div modes, GDM/PDM), CAD-bus drive strengths & terminations, and voltages (AMD via the `ryzen_smu` SMU PM table; Intel via MCHBAR MMIO), plus SPD EEPROM decode (JEP106 module/die makers, rank, XMP 2.0/3.0, EXPO).
2. **AIDA64-style benchmark engine** — multi-threaded AVX2/AVX-512 non-temporal SIMD bandwidth (Read / Write / Copy × Memory / L3 / L2 / L1) and pointer-chase latency.

End goal: unprivileged TUI (ratatui) and GUI (egui/eframe) frontends served by a privileged daemon — **delivered in Cycle 3 (Phases 3 + 4)** — i.e. ZenTimings + ASRock Timing Configurator + AIDA64 in one native Linux tool. What remains is Phase 5: packaging & distribution.

**Mandate:** 100% pure Rust, Cargo workspace (Edition 2021, resolver 2), Linux x86_64. No C++/Python/JS components.

## 2. Current State (2026-09-13 — Cycle 3 close-out)

- **Phases 1–4: COMPLETE. Phase 5 (Packaging & Distribution) = NEXT.** Phase 3 (privilege-separated daemon + socket + clients) and Phase 4 (TUI/GUI) were **delivered together in Cycle 3**.
- **Phase 1 — Native Benchmark Engine (`ramsleuth-bench`): COMPLETE.** 11 chunks (P1-01…P1-11) merged into `v2-development`; QA audit 2026-09-12: 7/7 runnable gates PASS. AIDA64 parity gate deferred to a DDR5-6000 AM5 host (the reference host is DDR4).
- **Phase 2 — Live Memory Controller Telemetry (`ramsleuth-telemetry`): COMPLETE.** 11 chunks (P2-01…P2-11) merged; QA audit 2026-09-12 @ `367bef9`: 8/8 runnable gates PASS.
- **Phase 3 — Privilege-Separated Daemon + Unix Socket + Clients: COMPLETE (Cycle 3).** `ramsleuth-protocol` (wire contract + Bincode frame codec), `ramsleuth-daemon` (caps / socket / cache / bench_job / rpc / bin + `systemd/ramsleuth.service`), `ramsleuth-client` (unprivileged CLI + shared std-only transport). **CORE GATE PASSED**: unprivileged `ramsleuth-client -- dump` prints full hardware timings against a running daemon (the Phase 3 exit criterion).
- **Phase 4 — TUI + GUI Dashboards: COMPLETE (Cycle 3).** `ramsleuth-tui` (ratatui, 3 zones) and `ramsleuth-gui` (egui/eframe, 3 zones) — see the Cycle 3 Summary (§3) and `FULLSCOPEvsCOMPLETED.md`.
- **State at Cycle 3 close-out (QA, 2026-09-13):** **7 crates** (bench, telemetry, protocol, daemon, client, tui, gui); **327/327 whole-workspace tests green in debug + release**; `cargo clippy --workspace --all-targets -- -D warnings` → **zero warnings**; **MSRV 1.75** held across all **441 lockfile-pinned packages**; release build OK.
- **Live-verified end-to-end (this host, 5950X, unprivileged):** daemon up with a **0660 socket**; unprivileged `ramsleuth-client` `dump` / `status` / `bench` all exit 0 (bench: `BenchStarted` + per-cell live progress + full 4×4 grid); TUI rendered under a PTY (3 zones); GUI ran on Wayland (1400×900, all zones, F2/F3 export, clean Q); daemon `SIGTERM` → graceful stop, exit 0, socket removed; no-daemon path → **exit 1** + friendly `DaemonDown` start hint; **zero panics / segfaults**.
- **Verification CLIs (all work today, exit 0):**
  - `target/release/ramsleuth-daemon` — the single privileged process (root; or `--socket /tmp/ramsleuth.sock` for an unprivileged dev socket — the SOFT caps probe warns and keeps serving).
  - `target/release/ramsleuth-client -- dump | status | bench [--tier memory|l1|l2|l3|full] [--mode full|memory-only]` — **no sudo needed**; `dump` is the exit-criterion command; no daemon running → exit 1 + friendly `DaemonDown` hint (never a crash).
  - `target/release/ramsleuth-tui` — terminal dashboard (R = refresh, S = snapshot, Q = quit; background 2 s updater).
  - `target/release/ramsleuth-gui` — desktop dashboard (F2 = PNG snapshot, F3 = JSON export to `$HOME`, Q = quit).
  - Direct verification CLIs (no daemon; from the Cycle 1/2 lineage, still valid): `cargo run -p ramsleuth-bench` — full 4×4 grid (text + hand-rolled JSON); optional `--avx512` (falls back to AVX2 when the host lacks AVX-512F — as on the reference host) and `--json` flags; unknown flag → exit 2. And `cargo run -p ramsleuth-telemetry` — dashboard-style telemetry listing; every cell renders as a value or `N/A (<reason>)`; optional `--json`; **exit 0 even when all sections are `Na`** (a structured N/A is a valid outcome). Run as root with the `ryzen_smu` module loaded for live AMD data.
- **Live result on the reference host today (unprivileged):** `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD: N/A (DriverMissing)` (ryzen_smu not installed — DKMS setup in §7); `Intel: N/A (UnsupportedHardware)`; SPD: 2× DDR4 modules (rank=1, 3200 MT/s; density `0x0D` → `N/A`, maker `0xC1` shown raw-hex — see model reconciliation §9). No panic in any privilege/CPU state.

## 3. Cycle 3 Summary (2026-09-13)

What was built: the **privilege-separated architecture** — one privileged daemon (holding `CAP_SYS_RAWIO`) owns all hardware I/O; every client is fully unprivileged and reaches it over a Unix socket. Five new crates + a serde wire foundation:

- **`ramsleuth-protocol`** — the wire contract: `Request` / `Response` / `BenchMode` / `Message` serde enums (payload types reused verbatim from telemetry + bench), `DEFAULT_SOCKET_PATH` (`/run/ramsleuth/ramsleuth.sock`), length-prefixed Bincode frame codec with a 16 MiB `MAX_FRAME_SIZE` guard. Zero `unsafe`, tokio-free.
- **Serde wire foundation** — serde derives on **all** telemetry (CPUID / AMD / Intel / SPD / facade) + bench (worker / orchestrator / streamed) public wire types, each with bincode round-trip tests; frozen payload shapes reused verbatim (no duplication, no boxing).
- **`ramsleuth-daemon`** (lib + bin) — SOFT capability probe (never panics/exits), socket listener (mode **0660**, stale-file probe → rebind, best-effort `chown`), TTL `TelemetryCache` (injectable collector), single-flight `BenchJobManager` (clean cancel + per-cell progress), per-connection async RPC, bin (`--socket` / `--max-age`, SIGTERM/SIGINT graceful stop + socket removal) + `systemd/ramsleuth.service` (CAP_SYS_RAWIO-clamped, sandboxed unit).
- **`ramsleuth-client`** (lib + bin) — synchronous `UnixStream` transport (timeouts, 3-retry backoff, friendly `DaemonDown`), pure `dump` dashboard renderer, `bench` / `status` commands, CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes 0/1/2). std-only.
- **`ramsleuth-tui`** — ratatui 0.29 + crossterm 0.28; pure `key_to_action` (R / S / Q); 3-zone non-scrolling dashboard (timing matrix / bench grid + progress / SPD + daemon status); terminal loop with a background 2 s updater; degrades to "daemon not connected" without crashing.
- **`ramsleuth-gui`** — egui / eframe / egui_extras 0.27.2; semantic palette; F2 PNG / F3 JSON export to `$HOME`; `TelemetryData` + background poller with a bench command channel + cancel; 3 zones; 1400×900 @ ~60 FPS app.

Process: **30 micro-chunks (P3-01…P3-30) + 2 doc-drift fixes** (DocFix / DocFix2: residual `--max-age` doc drift `5 s` → `2 s`; doc-only), each implemented on `branch/chunk-P3-*`, per-chunk reviewed, merged `--no-ff` into `v2-development`, and pruned at close-out. QA passed (327/327 tests debug + release, zero clippy warnings, MSRV 1.75, live end-to-end incl. the CORE GATE) and the cycle is compacted into `MASTER_LOG.md` (Cycles 1–3).

## 4. Workspace Layout (7 crates)

| Crate | Status | Contents |
|---|---|---|
| `crates/ramsleuth-bench` | **DONE (Phase 1)** | `features` (AVX2/AVX-512F runtime detect), `topology` (/sys physical-core enumeration, SMT filter, total/per-CCD L3), `buffers` (pure 64B-aligned sizing plan), `kernel_read` / `kernel_write` / `kernel_copy` (AVX2 streaming / NT / copy), `kernel_512` (AVX-512F variants + AVX2 fallback), `worker` (pinned barrier-synced per-core dispatch), `latency` (pointer-chase ring, `__rdtscp`), `orchestrator` (4×4 grid, best-of-3 / median), `main` (verification CLI; hand-rolled JSON, no serde/clap). Deps: `libc` only. |
| `crates/ramsleuth-telemetry` | **DONE (Phase 2)** | `cpuid` (vendor + frozen generation map), `error` (`TelemetryError` / `Section<T>` no-panic contract), `amd_smu` (sysfs `pm_table` → `/dev/ryzen_smu` ioctl), `amd_pm` (version-guarded bounds-checked PM parse), `amd_readout` (shared display types + AMD mapping), `intel_mchbar` (PCI BAR5 + read-only `/dev/mem` mmap guard), `intel_readout` (per-channel IMC decode), `spd_eeprom` (ee1004 unprivileged acquire), `spd_decode` (JEP106 / rank / XMP / EXPO), `facade` (`SystemMemoryTelemetry` + `collect()`), `main` (verification CLI). Deps: `nix` 0.29 (`fs`/`ioctl`/`mman`) only. |
| `crates/ramsleuth-protocol` | **DONE (Phase 3)** | Wire contract: `Request` / `Response` / `BenchMode` / `Message` serde enums (payload types reused verbatim from telemetry + bench), `DEFAULT_SOCKET_PATH` (`/run/ramsleuth/ramsleuth.sock`), length-prefixed Bincode frame codec with a 16 MiB `MAX_FRAME_SIZE` guard (`encode_frame` / `decode_frame` / `Frame` / `FrameError`). Zero `unsafe`, tokio-free. |
| `crates/ramsleuth-daemon` | **DONE (Phase 3)** | `caps` SOFT privilege probe (geteuid + `CapEff` bit 21; never panics/exits), `socket` listener (mode **0660**, stale-file probe → live `AlreadyRunning` / dead rebind, best-effort `chown`), TTL `TelemetryCache` (injectable collector), single-flight `BenchJobManager` (clean cancel + per-cell progress), `rpc` (per-connection async over the frozen wire contract), `main` bin (`--socket` / `--max-age`, SIGTERM/SIGINT → graceful stop + socket removal), `systemd/ramsleuth.service` (CAP_SYS_RAWIO-clamped, sandboxed unit). Deps: tokio (daemon-only). |
| `crates/ramsleuth-client` | **DONE (Phase 3)** | Synchronous `UnixStream` transport (read/write timeouts, 3-retry backoff, friendly `DaemonDown` diagnostics); pure `dump` dashboard renderer (full hardware timings, every N/A cell with its reason — **the Phase 3 exit criterion**); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes 0/1/2). std-only, no tokio — the transport shared by the CLI and by `tui`/`gui`. |
| `crates/ramsleuth-tui` | **DONE (Phase 4)** | ratatui 0.29 + crossterm 0.28 (MSRV ≤ 1.75 via two lockfile pins); pure `key_to_action` (R / S / Q); 3-zone non-scrolling layout (timing matrix / bench grid + progress / SPD + daemon status); terminal loop with a background 2 s updater. |
| `crates/ramsleuth-gui` | **DONE (Phase 4)** | egui / eframe / egui_extras 0.27.2 (newest 1.75-compatible line); semantic palette (cyan / amber / slate / crimson); F2 PNG snapshot / F3 JSON export to `$HOME`; `TelemetryData` behind `Arc<RwLock>` + background poller (bench command channel + cancel); 3 zones; 1400×900 @ ~60 FPS eframe app with no UI-thread blocking. |

Root `Cargo.toml`: **7 members**, `version 0.1.0`, `edition 2021`, `rust-version = "1.75"`, license MIT, repo `https://github.com/MadGoatHaz/RamSleuth`.

## 5. Key Architectural Decisions & Frozen Interfaces

1. **No-panic `Section<T>` contract (P2-02, frozen):** displayable telemetry values are `Section<T> { Value(T), Na(TelemetryError) }`; fallible operations return `TelemetryResult<T>`. `TelemetryError` variants: `UnsupportedHardware`, `UnsupportedVendor`, `DriverMissing`, `InsufficientPrivilege`, `UnknownPmTableVersion`, `NoDevmem`, `NotApplicable`, `InvalidValue(String)`, `Io(String)`. **No `panic!`, no `unwrap()`/`expect()` on hardware-derived data, no unguarded deref** anywhere in the telemetry crate; every `unsafe` block carries `// SAFETY:` and is bounds-checked. The CLI renders every missing value as `N/A (<reason>)` and exits 0.
2. **Direct SMU access — no `ryzen_smu` Rust crate (P2-03, decision D1):** sysfs-first read of `/sys/kernel/ryzen_smu/pm_table` (`std::fs`), fallback open of `/dev/ryzen_smu` + driver ioctl via `nix`. We own the version-guarded PM parse and all mapping. `nix` (features `fs`/`ioctl`/`mman`) is the **only** Phase 2 dependency and the only telemetry dep. Rejected: `ryzen_smu` crate, `memoffset`, serde/serde_json (hand-rolled JSON), clap (std `env::args`).
3. **AMD PM + Intel IMC byte offsets are plan-mandated SKELETONS (P2-04 / P2-07):** the offset/layout tables are version-guarded and bounds-checked, but their numeric values are **not yet reconciled against live silicon** (open gate §9c). Until then, displayed values are best-effort model values.
4. **Intel MCHBAR gating (P2-06, decision D4):** `CpuInfo` must be Intel before **any** file/mmap access; `/dev/mem` is mapped **read-only** behind a guard struct that bounds-checks reads and `munmap`s in `Drop`; STRICT_DEVMEM EIO/ENODATA → `InsufficientPrivilege`; missing file → `NoDevmem`. On the AMD host the path returns `Na(UnsupportedVendor)`/`Na(UnsupportedHardware)` and verifiably never touches `/dev/mem`.
5. **SPD is unprivileged (P2-08 / P2-09):** raw `ee1004` sysfs images (512 B DDR4 / 1024 B DDR5); JEP106 module + die makers (continued-ID support), rank bits, part/serial, JEDEC speed, XMP 2.0/3.0 + EXPO profile summaries; a checksum failure skips the profile, never the module; no panic on truncated/garbage data.
6. **Phase 1 checksum conventions (frozen):** read/copy kernels return the **wrapping sum of the buffer's little-endian 64-bit words** (data-sensitive DCE sink; `0` for a zeroed buffer); the scalar fallback has identical semantics; write kernels' frozen return is the **byte counter** (so `checksum == total_bytes` for Write); per-slice word-sums wrapping-add to exactly the whole-buffer checksum (proves the partition has neither overlap nor gap); all tier buffers are 64-byte aligned (`AlignedBuf`); bandwidth cells are **best-of-3** `Instant` timings, latency cells the **median** of 3 runs of a ≥1 M-hop chase over the *materialized* full-tier ring (Memory tier exceeds L3 → true DRAM latency); non-x86_64 latency falls back to `Instant` only (documented precision loss).
7. **CPUID family mapping (P2-01, frozen):** the 5950X reference host reports **family `0x19` → `Amd(Zen3)`** (frozen map: `0x15`→Zen 1, `0x17`→Zen 2, `0x19`→Zen 3, `0x1A`→Zen 4, `0x1C`→Zen 5, else `Unknown`). **Note: desktop Zen 4/5 silicon also reports family `0x19` on some boards, so the AMD PM parse (P2-04) keys on the SMU version (7.11.x / 12.x / 13.x), not on `AmdZen`** — generation ambiguity cannot break the PM layout. Intel: family `0x6` model table covers Skylake (`0x4F`/`0x56`/`0x5E`) through Arrow Lake; unrecognized models → `IntelGen::Unrecognized` (still Intel-gated, not `Unknown`).
8. **Topology & buffers are pure (P1-02 / P1-03):** `detect() -> Result<CpuTopology, TopologyError>` over /sys (SMT siblings filtered; per-CCD L3 slices; total L3); `plan(&CpuTopology) -> BufferPlan` is a pure function (L1 16 KiB, L2 256 KiB, L3 = 50% of a CCD slice, DRAM = max(256 MiB, 3× total L3), latency ring 128 MiB; all 64-byte aligned; safe fallbacks when sysfs fields are missing).
9. **Phase 3/4 privilege + wire contract (Cycle 3, frozen):** one privileged daemon is the **sole privilege boundary** — all hardware I/O (ryzen_smu sysfs/char-dev, MCHBAR `/dev/mem`, ee1004) stays daemon-side; the client/TUI/GUI chain is tokio-free and never touches hardware. Wire: synchronous length-prefixed **Bincode frames** with a **16 MiB `MAX_FRAME_SIZE` guard** over a **0660 Unix socket** (`/run/ramsleuth/ramsleuth.sock`); tokio confined to the daemon (async accept loop + `spawn_blocking` pumps); serde derives on every wire-crossing public type (payloads reused verbatim — no duplication, no boxing of frozen arm shapes). The daemon **inherits the no-panic contract**: any absent driver/privilege/hardware surfaces as structured `Na` sections in the payload — never a crash of the service or the clients.

## 6. Verification Environment (confirmed 2026-09-12)

### (a) Primary AMD dev host — Ryzen 9 5950X (Zen 3)

- 16C/32T, 64 MiB L3, DDR4 (2× modules, 3200 MT/s, rank-1), AVX2 — **no AVX-512** (the `--avx512` path falls back to AVX2; the 512-bit kernels compile but the runtime gate selects AVX2).
- **OS: CachyOS, kernel `7.2.3-1-cachyos-custom`.**
- **`ryzen_smu` kernel module: NOT installed.** Evidence: `sudo modprobe ryzen_smu` → `FATAL: Module ryzen_smu not found in directory /lib/modules/7.2.3-1-cachyos-custom`; `/sys/kernel/ryzen_smu/` does not exist. So live AMD telemetry renders `N/A (DriverMissing)` today.
- **Integration path:** the **standard path is now the DKMS + pacman workflow — see §7** (pacman-managed, auto-rebuilds on kernel updates; this supersedes the older manual `make` + `cp` + `depmod` flow). Manual fallback, if DKMS is not an option: (1) install kernel headers matching the running kernel (`uname -r` = `7.2.3-1-cachyos-custom`); (2) obtain the ryzen_smu project source (kernel module + userspace tool) — **verify the correct upstream repo/URL at setup time — do not trust a cached URL**; (3) build the out-of-tree kernel module against the running kernel (`make` in the module source); (4) install it into `/lib/modules/$(uname -r)/` (e.g. `extra/`) and run `sudo depmod -a`; (5) load it: `sudo modprobe ryzen_smu`; (6) verify `ls /sys/kernel/ryzen_smu/` shows `pm_table`, then run the daemon as root (§7 step 7) and the AMD section (clocks/timings/CAD/voltages) populates instead of `N/A (DriverMissing)`. The codebase handles the module-absent case gracefully either way — `Na(DriverMissing)`, never panics.
  - Caution: building an out-of-tree module against a **custom CachyOS kernel** requires the exact matching headers and could need build adjustments (Kconfig/version guards) before it loads.

### (b) Intel test machine — i5-6600 (Skylake)

- LGA-1151, Intel Core i5-6600 (6th-gen Skylake), **dual-channel (2 DIMM channels)** — matches `channel_count(Skylake) = 2` in the Intel decode model.
- Purpose: **live Intel MCHBAR decode verification** (BAR5 → read-only `/dev/mem` mmap → per-channel IMC registers). The Phase 2 Intel path is Intel-gated and returns `N/A (UnsupportedHardware)` on the AMD host, so this machine is where the decode gets its live proof.
- Intel voltages/CAD-bus sections are out of Phase 2 scope by design → `Na(NotApplicable)` on Intel.

### (c) Deferred target

- The AIDA64 parity gate (±5% DRAM bandwidth; 60–75 ns DDR5-6000 AM5 latency band) is deferred to a **DDR5-6000 AM5 host** — the 5950X host is DDR4 (measured DRAM latency ~81.5 ns is out of that band as expected; reported, not failed).

## 7. ryzen_smu DKMS + pacman setup (for live AMD subtimings)

The **standard path** for live AMD telemetry on the 5950X host — DKMS + pacman, **not** the manual `make` + `cp` + `depmod` flow (manual fallback kept in §6a). DKMS keeps the module pinned to the running kernel and auto-rebuilds it on kernel updates (pacman-managed — no manual rebuild).

0. **Verify the kernel build tree exists:** `ls /lib/modules/$(uname -r)/build` — if absent, the matching headers package is missing (step 1).
1. **Install build tooling + matching headers:** `sudo pacman -S --needed dkms base-devel`, plus the headers package matching the running kernel — **verify its name first**: `pacman -Qs headers | grep -iE 'cachyos|custom'` (on this host: the `7.2.3-1-cachyos-custom` headers).
2. **Clone the `ryzen_smu` source.** **Verify the correct upstream at setup time** — the code's uAPI hint is `53XU/ryzen_smu`; **do not trust a cached URL**.
3. **Provide a `dkms.conf` if the source lacks one:**
   ```
   PACKAGE_NAME=ryzen_smu
   MAKE="make -C ${kernel_source_dir} M=${dkms_tree}/ryzen_smu/${PACKAGE_VERSION} modules"
   AUTOINSTALL=yes
   ```
4. **Build + install against the running kernel:** `sudo dkms add ryzen_smu` → `sudo dkms build ryzen_smu -k $(uname -r)` → `sudo dkms install ryzen_smu -k $(uname -r)` (a single `sudo dkms install ryzen_smu -k $(uname -r)` does add + build + install).
5. **Load now and at boot:** `sudo modprobe ryzen_smu` + `echo ryzen_smu | sudo tee /etc/modules-load.d/ryzen_smu.conf`.
6. **Verify:** `ls /sys/kernel/ryzen_smu/` should show `pm_table`.
7. **Run the daemon as root** for live AMD subtimings (`sudo target/release/ramsleuth-daemon`, or the systemd unit) — the unprivileged client/TUI/GUI then render them. Without the module the app degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic).

Notes:

- `AUTOINSTALL=yes` means DKMS **auto-rebuilds the module on every kernel update** (pacman-managed) — no manual rebuild step after an OS/kernel update.
- **Custom-kernel caution:** this host runs `7.2.3-1-cachyos-custom`; an out-of-tree module against a custom kernel may need **Kconfig / version-guard adjustments** in the module build before it compiles/loads (exact matching headers required, per §6a).
- The systemd unit (`systemd/ramsleuth.service`) deliberately does not load the module — it assumes the module is loaded (DKMS `AUTOINSTALL` handles that); without it the daemon degrades per the no-panic contract and keeps serving everything else.

## 8. Push Policy & Git State

- **Branch `v2-development` is the development branch — LOCAL ONLY. Nothing has been pushed to GitHub.**
- Upstream `https://github.com/MadGoatHaz/RamSleuth` has a **divergent legacy `master`** (origin/HEAD → origin/master). Do not merge into or force onto it; treat upstream as a read-only reference.
- **Push policy (confirmed 2026-09-12): stay 100% local until there is a CONFIRMED, TESTED, WORKING end-result app.** **That condition is now MET** (Cycle 3 QA: CORE GATE passed, 327/327 tests, live end-to-end verified, zero panics — §2). **Ready to push to `origin/v2-development` (fast-forward — NEVER force-push) on explicit go-ahead.** No force-pushes, ever, without explicit sign-off; the first push, if any, is a deliberate, reviewed event (likely a fresh `v2` branch/tag rather than anything onto the legacy `master` — decide at push time, with explicit sign-off).
- **Git state (2026-09-13): tree clean; single local branch; 175 commits ahead of divergent `origin/master` (~170+).** All 32 Cycle 3 chunk branches (30 × `branch/chunk-P3-*` + `branch/chunk-docfix` / `branch/chunk-docfix2`) and every earlier Phase 1/2 chunk branch were merged via `--no-ff` and pruned at close-out (`git branch -d` only; no unmerged branch touched).
- `DEV_LOG.md` is the active lease board (reset at Cycle 3 close-out — no active leases); `MASTER_LOG.md` holds the durable per-cycle history (Cycles 1–3 + the 2026-09-12 decisions record).

## 9. Open Items — Carried Cross-Cutting (from Phases 1–3)

- **(a) AMD tick-identical ground truth — BLOCKED on environment.** Needs the `ryzen_smu` module installed (DKMS workflow, §7) + root on the 5950X host, then: live values tick-identical to the `ryzen_smu` CLI's own readout; clocks ±1 MHz; voltages ±10 mV; CAD equal to the RZQ/code-table mapping.
- **(b) Intel live MCHBAR decode — pending.** On the i5-6600 machine: run the daemon as root (or the direct CLI `sudo cargo run -p ramsleuth-telemetry --release`); confirm per-channel tCL/tRCD/tRP/tRAS + command-rate/gear decode against known-good values; confirm `Na(NotApplicable)` for Intel voltages/CAD.
- **(c) Model reconciliation — pending live silicon.** AMD PM byte offsets (P2-04 skeleton), Intel IMC register offsets (P2-07 skeleton), and SPD decode of the codes observed on the 5950X host: **maker `0xC1`** (currently rendered raw-hex) and **density `0x0D`** (currently `Na`). Reconcile against the live PM table / SPD spec, then update the decode tables + fixtures.
- **(d) Phase 1 L1/L2 bandwidth overhead refinement.** The small-tier working sets (32 KiB L1d / 1 MiB L2) split across 16 pinned workers are dominated by thread/barrier/timing overhead per pass → cells are not comparable across tiers. Fix: **add inner-loop iterations** in the small-tier passes to amortize per-pass overhead. (Known follow-up from Cycle 1; cells remain informational until then.)
- **(e) MSRV conflict — decision deferred.** Workspace `rust-version = 1.75`, but the AVX-512F intrinsics (P1-07) require **Rust 1.89** (clippy is msrv-gated at 1.89 for that crate). The workspace is deliberately kept at 1.75 via lockfile pins for the ratatui/egui transitive deps; **bump the workspace MSRV to 1.89 or cfg-gate the 512-bit bodies** — plan as a dedicated chunk during Phase 5 packaging.

## 10. Next Phase — Phase 5: Packaging & Distribution

Goal (per `Docs/RamSleuth-v2.md` Phase 5): turn the verified 7-crate app into a distributable package. The app itself is **100% pure Rust — self-contained binaries** with no runtime dependencies beyond the OS; the packaging scope is:

- **(a) PKGBUILD / AUR package (`ramsleuth-git`)** — build all 7 crates, ship `systemd/ramsleuth.service`, install the daemon binary to `/usr/bin/ramsleuth-daemon`.
- **(b) systemd preset + create the `ramsleuth` group at install.** **This is the one real install-dependency gap for Phase 5:** the shipped unit uses `Group=ramsleuth`, and **the group must exist or the service fails to start** (the unit documents `Group=wheel` as the fallback on systems without a `ramsleuth` group). The `/run/ramsleuth` dir is handled by the unit's `RuntimeDirectory=ramsleuth` (and the daemon's `setup_listener` creates it as a fallback). CAP_SYS_RAWIO + sandboxing (`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `NoNewPrivileges`, bounding set clamped to just that capability) are already in the unit.
- **(c) Optionally package / provision the `ryzen_smu` DKMS kernel module** (live AMD subtimings) as a recommended extra — workflow in §7.
- **(d) GitHub Actions CI** — test + clippy + build matrix for `x86_64-unknown-linux-gnu`.
- **(e) Push to GitHub on go-ahead** — the push condition is met (§8); fast-forward to `origin/v2-development`, **NEVER force-push**; a deliberate, reviewed event with explicit sign-off.

The daemon **inherits the no-panic contract** (frozen, §5 item 9): any absent driver/privilege/hardware surfaces as structured `Na` sections in the payload — never a crash of the service or the clients.

## 11. Operating Protocol Summary (how work runs in this repo)

1. **Strict phased gating:** each cycle compiles + runs + passes its exit criteria before the next phase starts. Each phase is planned first (`plans/PLAN-PHASE<N>.md`) with micro-chunks, dependency tags (`[ISOLATED]` / `[COUPLED-TO: …]` / `[CRITICAL-PATH]`), frozen-interface markers, and per-chunk quality gates.
2. **Single-file micro-chunking:** one target source file per chunk, ≤ ~50–100 lines changed; each chunk adds exactly one `mod <name>;` wiring line to `src/lib.rs` (wiring not counted against the budget).
3. **Per-chunk flow:** implement on `branch/chunk-<ID>` (forked from `v2-development`) → in-file unit tests (`#[cfg(test)]`) → clippy `-D warnings` → **per-chunk code review** (lease-signed) → `git merge --no-ff` into `v2-development` → branch pruned. Interface-freeze chunks (`[CRITICAL-PATH]`) merge first; no silent signature changes — changes are a plan edit + rebase.
4. **Lease board:** `DEV_LOG.md` `@@@ ACTIVE_WORKERS @@@` — sign in with target files before work; sign out with `[DONE]`/`FAILED` + one-line decision + ahead-note; update `@@@ CURRENT_STATE @@@`. (Board is empty right now.)
5. **Handoffs:** 3-line **Semantic Pulse** — `STATUS` / `DECISION` / `AHEAD`, one short line each. No full-file dumps or code in chat; all detail goes to disk (plans, logs, docs).
6. **QA audit per cycle:** independent pass of every runnable gate (build, tests debug+release, clippy, live CLI runs in multiple privilege/CPU states) → then **compaction**: merge history into `MASTER_LOG.md`, prune chunk branches, reset `DEV_LOG.md`.
7. **Safety/fallbacks (hard rule):** unsupported CPU, missing driver, missing privilege, unknown PM-table version, absent `/dev/mem` → structured `Na(<reason>)`; **never panic, never segfault, never `unwrap`/`expect` on hardware data**; the Intel path never touches `/dev/mem` on non-Intel hardware; non-x86_64 compiles to a documented degraded path.
8. **Docs discipline:** `Docs/` holds the roadmap + spec + this handover + the full scope reference (`FULLSCOPEvsCOMPLETED.md`); `plans/` holds per-phase plans; `MASTER_LOG.md` is the durable record. Update them every cycle; this document is the standing handover — refresh it at each cycle close-out.
