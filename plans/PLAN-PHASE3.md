# RamSleuth v2 — Phase 3 Plan: Privilege-Separated Daemon, Unix-Socket RPC & Clients

> **Base branch:** `v2-development` — every `branch/chunk-P3-xx` forks from and merges back here (NOT `main`).
> **Sources of truth:** `Docs/Grand Design & Architecture Specification.md` §4 (privilege model), §3 (dashboard/UX), §2 (stack); `Docs/RamSleuth-v2.md` Phase 3 (§3.1–3.2); `Docs/HANDOVER.md` §8; frozen interfaces from `plans/PLAN.md` (P1-01/02/10) and `plans/PLAN-PHASE2.md` (P2-01/02/05/10).
> **Mandate:** 100% pure Rust, Cargo workspace, Edition 2021, `rust-version = 1.75` (kept — see OPEN ITEM below).
> **Chunk discipline:** one target source file per chunk, ≤ ~50–100 lines of source changed. Crate-birth chunks pair a manifest with the first module file (P2-03 precedent); each other chunk adds exactly one `mod`/re-export line to its crate's `lib.rs` (wiring only, not counted against the budget).
>
> **OPEN ITEM (deferred, non-blocking): MSRV 1.75→1.89 for AVX-512F intrinsics — resolve in a dedicated chunk during Phase 5 packaging.** Rationale: Phase 3 contains no AVX-512 intrinsics of its own; the workspace compiles clean at 1.75 today (159/159 tests, clippy `-D warnings` clean); the handover's open item (e) stays a plan-level decision parked for the packaging cycle. Every Phase 3 third-party dep added below must resolve to a release whose MSRV ≤ 1.75 (verify at build; pin the last MSRV-compatible release if a newer one breaks it, and record the pin in §7).

---

## 1. Phase 3 Scope (extracted from the specs)

**Objective (v2 §3):** Securely isolate privileged hardware operations inside a daemon, exposing a clean IPC channel for unprivileged user frontends.

### 1.1 Daemon service (`ramsleuth-daemon`) — the ONLY privileged process

- Runs as a systemd service (`systemd/ramsleuth.service`): `User=root` (root where the driver requires it) with `AmbientCapabilities=CAP_SYS_RAWIO`, `NoNewPrivileges=true`, `ProtectHome=true`, `ProtectSystem=strict`, `ReadWritePaths=/run/ramsleuth`.
- Owns every hardware handle by calling the Phase 2 providers: `ryzen_smu` (sysfs `pm_table` / char-dev ioctl), MCHBAR (`/dev/mem` read-only map), `ee1004` SPD — all via `ramsleuth_telemetry::collect()`; benchmarks via `ramsleuth_bench`.
- Listens on Unix domain socket `/run/ramsleuth/ramsleuth.sock` (mode `0660`); **must accept `--socket <path>`** (default `/run/ramsleuth/ramsleuth.sock`) so it can run unprivileged for local dev (e.g. `--socket /tmp/ramsleuth.sock`).
- Async RPC (tokio, **daemon-only dependency**): `GetTelemetry` (cached snapshot), `StartBenchmark { target, mode }` (streams progress events; single-flight), `CancelBenchmark` (clean worker termination — no mid-pass kill; the in-flight pass finishes, the run stops between passes/cells).
- **Graceful degradation (inherited no-panic contract):** missing driver / missing privilege / invalid CPUID / unknown PM version → the affected telemetry sections are structured `Na(<reason>)` in the payload. The process **never panics or segfaults**; capability check is **SOFT** (log a warning, keep serving).

### 1.2 Client (`ramsleuth-client`) — unprivileged CLI + shared IPC library

- Gains a `[lib]` target (its stub doc comment mandates this; TUI/GUI consume the library) alongside the existing `[[bin]]`.
- Synchronous `std::os::unix::net::UnixStream` transport (no tokio): connect with timeouts + bounded retries, friendly diagnostics when the daemon is down.
- Subcommands: `dump` (full hardware timings — **THE exit criterion**), `bench [--tier …] [--mode …]`, `status`.

### 1.3 TUI (`ramsleuth-tui`) — ratatui + crossterm

- Live dashboard in the 3 zones of Grand Design §3.1: (1) timing matrix, (2) benchmark grid + progress, (3) hardware/SPD + daemon status. Keys: **[R]efresh, [S]napshot, [Q]uit**. Terminal renders cleanly across sizes; unprivileged (talks to the daemon socket).

### 1.4 GUI (`ramsleuth-gui`) — egui + eframe

- `egui::Grid` timing matrix, `egui_extras::TableBuilder` benchmark grid, semantic palette (cyan `#00D4FF` / amber `#FFB300` / slate `#1E1E24` / crimson `#FF3B30`), 60 FPS target with no UI-thread blocking (telemetry via `Arc<RwLock<TelemetryData>>` updated by a background thread), F2 snapshot PNG / F3 export JSON / Q quit.

### 1.5 Shared wire protocol (NEW crate `ramsleuth-protocol`)

- One length-prefixed **Bincode** frame per message: `Message { Request(Request), Response(Response) }` (codec + message enums in §3).
- **Single source of truth:** payload types are REUSED from `ramsleuth-telemetry` (`SystemMemoryTelemetry`, `Section<T>`, `NaReason`, display types, `SpdModule`) and `ramsleuth-bench` (`BenchmarkGrid`, `Tier`, `Metric`, `WorkerResult`, new `StreamProgress`) — the protocol crate depends on both; **no telemetry/bench structs are duplicated anywhere**. Where a public payload type lacks serde (today: all of them), the serde derive is added in the owning file (P3-01…P3-09).
- `TelemetryError` is **not** on the wire (it holds a non-serializable `std::io::Error`); only the frozen display-level `NaReason`/`Section<T>` cross the socket — exactly the no-panic contract.

### 1.6 Exit criteria (v2 §3.2)

1. **An unprivileged** `cargo run -p ramsleuth-client -- dump` prints full hardware timings (every cell rendered as a value or `N/A (<reason>)`) **without sudo** against a running daemon.
2. `bench`/`status` subcommands work over the same socket; `StartBenchmark` streams progress and `CancelBenchmark` terminates the run cleanly.
3. TUI + GUI run unprivileged against the daemon, render the 3 zones, and degrade to "daemon not connected" without crashing.

---

## 2. Key Architectural Decisions

### D1 — MSRV stays 1.75 (no bump chunk in Phase 3)

Workspace `rust-version = "1.75"` is unchanged for every Phase 3 chunk. The AVX-512F intrinsic MSRV conflict (handover open item (e)) is deferred to a dedicated Phase 5 packaging chunk (see the OPEN ITEM header). Phase 3 adds no intrinsics; the constraint is that every new third-party dep resolves to an MSRV-≤1.75 release (§7 ledger).

### D2 — Single source of truth: the wire protocol reuses telemetry/bench public types

- `ramsleuth-protocol` depends on `ramsleuth-telemetry` + `ramsleuth-bench` and defines only: the `Request`/`Response`/`BenchMode`/`Message` wire enums (small, protocol-owned), the frame codec, and the socket-path constant.
- Every public payload type that lacks serde gets `#[derive(serde::Serialize, serde::Deserialize)]` **in its owning file** — P3-01…P3-06 (telemetry) and P3-07…P3-09 (bench). These are the only Phase 3 modifications to the two completed crates (additive derives + a small `pub(crate)` visibility widening in P3-08); no frozen signature changes.
- Verified inventory: **no** type in either crate currently derives serde. `Section<T>` derives cleanly (its `Na` arm carries `NaReason`, a plain enum); `WorkerResult`/`BenchmarkGrid` are plain POD.
- New deps: `serde` (derive) on telemetry/bench/protocol/daemon/client; `bincode` on protocol only; `tokio` on daemon only. Clients/TUI/GUI use synchronous `std::os::unix::net::UnixStream` — **no tokio outside the daemon**.

### D3 — Bincode 1.3, not 2.x

Frame codec = **u32 little-endian length prefix + bincode 1.3 payload** (`bincode::serialize` / `bincode::deserialize`), `MAX_FRAME_BYTES = 16 MiB` guard. Bincode 2's config-based API is rejected: no benefit for a fixed local protocol, larger API surface, and 1.3's MSRV (≤1.75) is safely known. Version-mismatch risk is nil: both ends are built from this workspace.

### D4 — Core-first gating: the CORE GATE

The CORE (protocol + daemon + CLI client) is fully implemented and proven **before** any TUI or GUI chunk begins.

> **CORE GATE — do not begin any TUI/GUI chunk until unprivileged `cargo run -p ramsleuth-client -- dump` prints full hardware timings against a running daemon.**

Verification procedure (performed by the reviewer at the gate): daemon started as the dev user with `--socket /tmp/ramsleuth.sock` (no root); client run as the same user; expected: full dashboard-style listing (CPU line; AMD/Intel sections as values **or** structured `N/A (DriverMissing)`/`N/A (UnsupportedHardware)`; SPD module lines when `ee1004` is present), exit 0, no panic on either process. The gate is a milestone, not a code chunk — nothing in groups E/F merges before it passes.

### D5 — Socket path override + soft capability check; never hard-exit

- The daemon accepts `--socket <path>` (default `/run/ramsleuth/ramsleuth.sock`, the constant lives in `ramsleuth-protocol` so daemon + clients share one source). Local dev: `--socket /tmp/ramsleuth.sock`.
- Capability handling is **SOFT** (P3-12): parse `/proc/self/status` `CapEff` bit 21 (`CAP_SYS_RAWIO`) + `geteuid()`; if unprivileged, emit a stderr warning listing which sections will degrade and **continue serving** — privileged fields fall back to `Na(InsufficientPrivilege)`/`Na(DriverMissing)` exactly as the Phase 2 providers already do. **Never hard-exit on missing privilege.**
- Stale socket handling (P3-13): if the path exists, probe with a `UnixStream::connect`; a live daemon → clean error "already running"; a dead file → remove + rebind. Parent directory is created best-effort; failure → friendly error suggesting `--socket /tmp/…` (never a silent fallback).

### D6 — Benchmark dispatch: single-flight, spawned on the blocking pool, progress-streamed

- One active benchmark at a time (a bandwidth run monopolizes every core; concurrent runs are meaningless). A second `StartBenchmark` → `Response::Error("benchmark already running")`.
- The run executes `tokio::task::spawn_blocking` over the **new** `ramsleuth_bench::streamed` API (P3-09): `run_streamed(topo, sizes, &StreamOptions, &AtomicBool cancel, &mut on_progress) -> Result<BenchmarkGrid, StreamError>` — reusing the orchestrator's existing passes (best-of-3 bandwidth, median-of-3 latency) and inserting cancel checks + progress events between passes/cells. Cancellation is **clean**: the in-flight pass (one `run_pinned` call, milliseconds) completes; the run then stops.
- Progress = `StreamProgress { tier, metric, phase: StreamPhase::{Preparing, Pass{done,total}, Finished} }` (frozen P3-09; the protocol wraps it with a `run_id`). The starting connection is the **owner** and receives the event stream plus the terminal frame (`BenchComplete` | `BenchCancelled` | `Error`) exactly once; any connection may send `CancelBenchmark { run_id }` (acknowledged, then the owner receives the terminal). `GetTelemetry` stays servable during a run.
- Telemetry is served from a TTL lazy cache (P3-14): `collect()` runs on demand when the snapshot is cold/stale (default max-age 5 s, `--max-age`), so repeated `GetTelemetry` RPCs are cheap and no background refresher task is needed.

### D7 — Dependency policy (full ledger in §7)

New external crates, each introduced in the phase chunk that consumes it: `serde` (workspace entry; telemetry/bench/protocol/daemon/client), `bincode 1.3` (protocol), `tokio` (daemon only: `rt-multi-thread`, `net`, `io-util`, `sync`, `time`, `signal`, `macros`), `libc` (daemon, for best-effort socket `chown`; bench precedent), `ratatui` + `crossterm` (TUI), `egui` + `eframe` + `egui_extras` + `png` + `serde_json` (GUI). **Rejected:** clap (std `env::args` precedent), `log`/`env_logger` (stderr `eprintln!` warnings), JSON-RPC (Bincode per the stack doc), tokio anywhere except the daemon.

---

## 3. Module layout (target file trees)

```
crates/ramsleuth-protocol/            (NEW — the only new crate of Phase 3)
├── Cargo.toml            P3-10  serde + bincode 1.3 + path deps on telemetry/bench
└── src/
    ├── lib.rs            P3-10  wiring + `DEFAULT_SOCKET_PATH` const + re-exports
    ├── messages.rs       P3-10  Request / Response / BenchMode / Message   [CRITICAL-PATH freeze]
    └── frame.rs          P3-11  encode_frame / decode_frame / FrameError / MAX_FRAME_BYTES   [CRITICAL-PATH freeze]

crates/ramsleuth-bench/               (Phase 1 crate — P3-07…P3-09 are additive)
└── src/
    ├── worker.rs         P3-07  + serde derives (BenchOp, WorkerResult)
    ├── orchestrator.rs   P3-08  + serde derives (Tier, Metric, BenchmarkGrid) + pub(crate) visibility widening
    └── streamed.rs       P3-09  NEW StreamOptions / StreamPhase / StreamProgress / StreamError / run_streamed

crates/ramsleuth-telemetry/           (Phase 2 crate — P3-01…P3-06 are additive)
└── src/
    ├── error.rs          P3-01  + serde derives (NaReason, Section<T>)
    ├── cpuid.rs          P3-02  + serde derives (AmdZen, IntelGen, CpuVendor, CpuInfo)
    ├── amd_readout.rs    P3-03  + serde derives (DivMode, GearMode, RttValue, ClockReadout, TimingSet, CadBus, VoltageSet, AmdReadout)
    ├── intel_readout.rs  P3-04  + serde derives (IntelChannel, IntelReadout)
    ├── spd_decode.rs     P3-05  + serde derives (SpdProfile, SpdModule)
    └── facade.rs         P3-06  + serde derive (SystemMemoryTelemetry) + whole-snapshot round-trip test

crates/ramsleuth-daemon/              (stub today — becomes lib + bin)
├── Cargo.toml            P3-12  tokio + libc + serde + protocol/telemetry/bench
├── src/
│   ├── lib.rs            P3-12  wiring + re-exports
│   ├── caps.rs           P3-12  soft capability probe (CapEff bit 21, euid, warnings)
│   ├── socket.rs         P3-13  listener setup: dir, stale-socket probe, bind, 0660, chown
│   ├── cache.rs          P3-14  TTL lazy TelemetryCache over collect() (injectable collector)
│   ├── bench_job.rs      P3-15  single-flight JobManager + spawn_blocking run_streamed + events
│   ├── rpc.rs            P3-16  per-connection async loop: frame reader, dispatch, owner forwarding
│   └── main.rs           P3-17  bin: --socket/--max-age, runtime, accept loop, signals, graceful stop
└── systemd/ramsleuth.service         P3-17  sandboxed unit (see D5/D7)

crates/ramsleuth-client/              (stub today — becomes lib + bin)
├── Cargo.toml            P3-18  serde + protocol/telemetry/bench
└── src/
    ├── lib.rs            P3-18  wiring + re-exports (the shared IPC library)
    ├── client.rs         P3-18  DaemonClient: sync UnixStream, connect timeouts/retries, request / request_streaming, ClientError diagnostics
    ├── dump.rs           P3-19  render(&SystemMemoryTelemetry) -> String (dashboard listing) + dump()
    ├── commands.rs       P3-20  bench() (progress + grid text) + status()
    └── main.rs           P3-21  bin: subcommands dump/bench/status, --socket/--tier/--mode

crates/ramsleuth-tui/                 (stub today — bin)
├── Cargo.toml            P3-22  ratatui + crossterm + client/telemetry/bench
└── src/
    ├── events.rs         P3-22  key -> Action (pure) + crossterm poll wrapper
    ├── ui.rs             P3-23  3 zones: timing matrix / bench grid + progress / SPD + status
    └── main.rs           P3-24  terminal init, background updater thread, tick loop, R/S/Q

crates/ramsleuth-gui/                 (stub today — bin)
├── Cargo.toml            P3-25  egui + eframe + egui_extras + png + serde_json + client/telemetry/bench
└── src/
    ├── style.rs          P3-25  semantic palette + dark-slate Style + snapshot_png + export_json
    ├── update.rs         P3-26  TelemetryData + background poller thread (Arc<RwLock<TelemetryData>>)
    ├── telemetry_zone.rs P3-27  egui::Grid timing matrix (3.1 left panel)
    ├── bench_zone.rs     P3-28  egui_extras::TableBuilder grid + run controls + progress
    ├── status_zone.rs    P3-29  hardware/SPD module cards + daemon status + F2/F3/Q actions
    └── main.rs           P3-30  eframe app: 60 FPS, header bar, zones, F2/F3/Q

Root: `Cargo.toml` gains `crates/ramsleuth-protocol` in `members` and a `[workspace.dependencies]` block (serde/bincode/tokio) — P3-10.
```

**Interface freezes (merge first, no silent changes — any change is a plan edit + rebase):** P3-01 (`NaReason`/`Section<T>` serde), P3-09 (`StreamProgress`/`StreamOptions`/`StreamError`/`run_streamed` — the bench streaming contract), P3-10 (wire `Request`/`Response`/`BenchMode`/`Message` + `DEFAULT_SOCKET_PATH`), P3-11 (frame codec: length-prefix layout, `MAX_FRAME_BYTES`, error semantics), P3-12 (daemon `CapabilityReport` soft-probe contract).

---

## 4. Micro-chunks (ordered — dependencies first)

### Group A — serde on the wire payload types (the two completed crates)

### Chunk P3-01 — serde on `NaReason` + `Section<T>`  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-telemetry/src/error.rs` (+1 serde line in the crate `Cargo.toml` — owned here; no `lib.rs` change)
- **Scope Boundary:** add `#[derive(serde::Serialize, serde::Deserialize)]` to `NaReason` and `Section<T>` (generic derive: `T: Serialize`/`Deserialize` bounds, the default serde behavior — every payload `T` satisfies them); no other change. Unit tests: `Section::<u16>` and `NaReason` (incl. `ParseError(String)`) bincode round-trips.
- **Dependency:** `[CRITICAL-PATH]` — first wire primitive; every other telemetry type derives against `Section<T>`/`NaReason`.
- **Quality Gates:** idiomatic Rust; zero clippy `-D warnings`; new tests 100% green; additive only — the frozen `TelemetryError`/`TelemetryResult` signatures are untouched (and `TelemetryError` gains **no** serde: its `Io(std::io::Error)` arm is non-serializable and it never crosses the wire — D2).
- **Exit Criteria:** telemetry compiles with serde 1.x (workspace entry, D7); round-trip tests pass; no behavior change to any existing test.

### Chunk P3-02 — serde on CPUID types
- **Target File:** `crates/ramsleuth-telemetry/src/cpuid.rs`
- **Scope Boundary:** add the serde derives to `AmdZen`, `IntelGen`, `CpuVendor` (nested `Amd(AmdZen)`/`Intel(IntelGen)`/`Unknown`), `CpuInfo { vendor, brand }`. Unit tests: `CpuInfo::detect()` snapshot round-trips through bincode and compares equal (host is self-consistent: `Amd(Zen3)` on the 5950X reference).
- **Dependency:** `[COUPLED-TO: P3-01]`
- **Quality Gates:** idiomatic Rust; zero clippy; round-trip test green; `detect()` signature untouched.
- **Exit Criteria:** `SystemMemoryTelemetry.cpu` is serializable end-to-end.

### Chunk P3-03 — serde on the AMD readout types
- **Target File:** `crates/ramsleuth-telemetry/src/amd_readout.rs`
- **Scope Boundary:** add the serde derives to `DivMode`, `GearMode`, `RttValue`, `ClockReadout`, `TimingSet`, `CadBus`, `VoltageSet`, `AmdReadout` (all fields are `Section<POD>`/plain scalars — derivable once P3-01 lands). Unit tests: a fixture `AmdReadout` (values + `Na` cells) round-trips; `RttValue::Rzq/Ohms/Disabled` arms all survive.
- **Dependency:** `[COUPLED-TO: P3-01]`
- **Quality Gates:** idiomatic Rust; zero clippy; round-trip green; mapping fns (`map_amd` etc.) untouched.
- **Exit Criteria:** `Section<AmdReadout>` is serializable end-to-end.

### Chunk P3-04 — serde on the Intel readout types
- **Target File:** `crates/ramsleuth-telemetry/src/intel_readout.rs`
- **Scope Boundary:** add the serde derives to `IntelChannel` and `IntelReadout` (fields reuse P3-03's display types + `Section<u16>`). Unit tests: a multi-channel `IntelReadout` fixture (incl. an all-`Na` channel) round-trips.
- **Dependency:** `[COUPLED-TO: P3-03]`
- **Quality Gates:** idiomatic Rust; zero clippy; round-trip green; decode fns untouched.
- **Exit Criteria:** `Section<IntelReadout>` is serializable end-to-end.

### Chunk P3-05 — serde on the SPD types
- **Target File:** `crates/ramsleuth-telemetry/src/spd_decode.rs`
- **Scope Boundary:** add the serde derives to `SpdProfile` and `SpdModule` (scalars, `Section<String/u8/u16>`, `Vec<SpdProfile>`). Unit tests: a 2-module fixture (DDR4 + DDR5, incl. empty `profiles`) round-trips.
- **Dependency:** `[COUPLED-TO: P3-01]`
- **Quality Gates:** idiomatic Rust; zero clippy; round-trip green; decode fns untouched.
- **Exit Criteria:** `SystemMemoryTelemetry.spd` is serializable end-to-end.

### Chunk P3-06 — serde on `SystemMemoryTelemetry` (the payload root)
- **Target File:** `crates/ramsleuth-telemetry/src/facade.rs`
- **Scope Boundary:** add the serde derive to `SystemMemoryTelemetry`; unit test: `collect()` on the host (root or not) bincode-serializes, round-trips, and compares equal — proving the whole Phase 2 snapshot is wire-safe, `Na` sections included.
- **Dependency:** `[COUPLED-TO: P3-02, P3-03, P3-04, P3-05]`
- **Quality Gates:** idiomatic Rust; zero clippy; `collect()`/`reason_from` untouched; round-trip green in every privilege state (run the test unprivileged in CI-of-review: all-`Na` still round-trips).
- **Exit Criteria:** the Phase 2 exit-criteria struct is fully wire-serializable — the protocol's `Response::Telemetry` payload exists.

### Chunk P3-07 — serde on bench worker types
- **Target File:** `crates/ramsleuth-bench/src/worker.rs` (+1 serde line in the crate `Cargo.toml` — owned here)
- **Scope Boundary:** add the serde derives to `BenchOp` and `WorkerResult { op, total_bytes, checksum, pinned }` (plain POD). Unit tests: all three `BenchOp` arms + a representative `WorkerResult` round-trip.
- **Dependency:** `[ISOLATED]` (only needs the bench crate to compile; no telemetry coupling)
- **Quality Gates:** idiomatic Rust; zero clippy; round-trip green; `run_pinned` untouched.
- **Exit Criteria:** `WorkerResult` is wire-serializable.

### Chunk P3-08 — serde on the grid types + `pub(crate)` widening
- **Target File:** `crates/ramsleuth-bench/src/orchestrator.rs`
- **Scope Boundary:** (a) add the serde derives to `Tier`, `Metric`, `BenchmarkGrid` (four `[f64; 4]` arrays) with a round-trip test; (b) widen `AlignedBuf`, `normalize_size`, `fill_pattern`, `bench_bandwidth`, `bench_latency` from private to `pub(crate)` — the ONLY change, consumed by P3-09 (no signature edits; `materialize_chase`/`chase_materialized`/`run_all_sized` stay as-is, `run_all` untouched).
- **Dependency:** `[ISOLATED]`
- **Quality Gates:** idiomatic Rust; zero clippy; grid round-trip green; all 7 existing orchestrator tests still green (the widening breaks nothing).
- **Exit Criteria:** the AIDA64 grid + tier/metric enums are wire-serializable; P3-09's target file can reuse the passes.

### Chunk P3-09 — targeted streaming benchmark runs (new bench module)  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-bench/src/streamed.rs` (new; +1 `mod streamed;` + re-exports in `src/lib.rs`)
- **Scope Boundary:** freeze the streaming contract: `StreamPhase { Preparing, Pass { done: u8, total: u8 }, Finished }`, `StreamProgress { tier: Tier, metric: Metric, phase: StreamPhase }`, `StreamOptions { tiers: Vec<Tier>, metrics: Vec<Metric>, use_avx512: bool }` (+ `StreamOptions::full(use_avx512)`), `StreamError { Cancelled, Topology(String), Worker(String) }`, and `run_streamed(topo: &CpuTopology, sizes: [usize; 4], opts: &StreamOptions, cancel: &AtomicBool, on_progress: &mut dyn FnMut(StreamProgress)) -> Result<BenchmarkGrid, StreamError>`: iterate the requested (tier, metric) cells — bandwidth cells via the widened `fill_pattern`/`AlignedBuf`/`bench_bandwidth` (best-of-3, `cancel` checked + `Pass{done,total}` emitted between iterations), latency via `bench_latency` (median-of-3) — unrequested cells stay `0.0`; any `cancel` hit between cells/passes → `Ok`-halt mapping to `Err(StreamError::Cancelled)` with the partial grid discarded; topology/worker errors map to their `StreamError` arms. ~110 lines code + ~30 test lines (new-file exception, one cohesive module).
- **Dependency:** `[COUPLED-TO: P3-08]` (widened passes); also `[CRITICAL-PATH]` — daemon + protocol branch on this contract.
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: tiny 4 KiB sizes, (a) full grid runs all 16 cells with finite values, (b) targeted single (tier, metric) fills exactly one non-zero row/column cell, (c) pre-set `cancel` → `StreamError::Cancelled` with zero progress `Pass` events, (d) mid-run `cancel` (thread set after first event) → `Cancelled`, (e) progress event ordering `Preparing → Pass… → Finished` per cell.
- **Exit Criteria:** `run_streamed` supports the daemon's single-flight model with clean cancellation and deterministic progress; `StreamProgress` is serde-ready (it derives — protocol wraps it verbatim).

### Group B — the wire protocol crate

### Chunk P3-10 — protocol crate birth + message contract  `[CRITICAL-PATH]`
- **Target File:** root `Cargo.toml` (member `crates/ramsleuth-protocol` + new `[workspace.dependencies]`: `serde = { version = "1", features = ["derive"] }`, `bincode = "1.3"`, `tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "sync", "time", "signal", "macros"] }`) + `crates/ramsleuth-protocol/Cargo.toml` (new) + `src/lib.rs` (new: doc, `pub const DEFAULT_SOCKET_PATH: &str = "/run/ramsleuth/ramsleuth.sock"`, `mod messages;` + re-exports) + `src/messages.rs` (new) — crate-birth multi-file exception (P2-03 precedent; ≈110 lines total)
- **Scope Boundary:** freeze the wire enums (all serde-derived, payloads REUSED — D2): `BenchMode { All, Read, Write, Copy, Latency }` (+ `to_metrics() -> Vec<Metric>`: `All` = the four metrics), `Request { GetTelemetry, StartBenchmark { target: Tier, mode: BenchMode }, CancelBenchmark { run_id: u64 } }`, `Response { Telemetry(SystemMemoryTelemetry), BenchAccepted { run_id: u64 }, BenchProgress { run_id: u64, progress: StreamProgress }, BenchComplete { run_id: u64, grid: BenchmarkGrid }, BenchCancelled { run_id: u64 }, Error(String) }`, `Message { Request(Request), Response(Response) }`. Unit tests: bincode round-trips of every `Request`/`Response` arm with real `collect()`/`BenchmarkGrid` payloads.
- **Dependency:** `[COUPLED-TO: P3-06, P3-09]` (payload roots must be serde-ready first); `[CRITICAL-PATH]` — the RPC contract, frozen at merge.
- **Quality Gates:** idiomatic Rust; zero clippy; round-trips green; **no** telemetry/bench struct redefinition anywhere (audit: `grep` the crate for `struct` — only the four enums above).
- **Exit Criteria:** daemon + clients can compile against one shared message vocabulary; `DEFAULT_SOCKET_PATH` is the single socket-path source.

### Chunk P3-11 — length-prefixed Bincode frame codec  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-protocol/src/frame.rs` (new; +`mod frame;` + re-exports in `lib.rs`)
- **Scope Boundary:** `pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024`; `pub enum FrameError { Incomplete, Oversized(u32), Decode(String) }` (`Display` + `Error`); `pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError>` = u32 LE length (of the bincode payload, pre-checked ≤ `MAX_FRAME_BYTES`) + `bincode::serialize`; `pub struct Frame<T> { pub message: T, pub consumed: usize }`; `pub fn decode_frame<T: DeserializeOwned>(buf: &[u8]) -> Result<Frame<T>, FrameError>` = `Incomplete` when <4 bytes or fewer than the declared payload bytes are buffered (incremental-reader contract: caller appends and retries), `Oversized` for the guard, `Decode` for bincode failure.
- **Dependency:** `[COUPLED-TO: P3-10]`; `[CRITICAL-PATH]` — wire layout frozen (length-prefix + bincode, both ends one workspace build).
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: full-frame round-trip of `Message`; `Incomplete` on 3-byte and partial-payload buffers; exact `consumed` accounting with two concatenated frames in one buffer; `Oversized` with a forged 0xFFFFFFFF length; `Decode` on garbage payload bytes; zero `unsafe`.
- **Exit Criteria:** one frame = one `Message` per direction, stream-safe under partial reads and hostile lengths.

### Group C — the daemon (only privileged process)

### Chunk P3-12 — daemon crate birth + soft capability probe  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-daemon/Cargo.toml` (new deps: tokio [workspace], libc `0.2`, serde [workspace], path deps protocol/telemetry/bench) + `src/lib.rs` (new: doc + wiring) + `src/caps.rs` (new) — crate-birth exception (≈100 lines)
- **Scope Boundary:** `pub struct CapabilityReport { root: bool, cap_sys_rawio: bool, warnings: Vec<String> }` + `pub fn probe() -> CapabilityReport`: `geteuid() == 0` (libc); parse `/proc/self/status` `CapEff` hex (pure `parse_cap_eff(&str) -> bool` helper, bit 21) — file missing (non-Linux) → `false` + warning, never an error; **soft** semantics (D5): unprivileged → warning strings naming the sections that will degrade (`InsufficientPrivilege`/`DriverMissing`), `probe()` itself never fails and never exits; emit the warnings to stderr at the call site (main).
- **Dependency:** `[COUPLED-TO: P3-10]` (crate compiles against the protocol); `[CRITICAL-PATH]` — the soft-probe contract frozen.
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: `parse_cap_eff` fixtures (bit-21 set/clear, short/empty/malformed hex → `false` no panic), `probe()` on the host returns without error in both privilege states.
- **Exit Criteria:** the daemon can report its own privilege state and warn — and keep serving.

### Chunk P3-13 — Unix socket listener setup
- **Target File:** `crates/ramsleuth-daemon/src/socket.rs` (new; +`mod socket;` in `lib.rs`)
- **Scope Boundary:** `pub enum SocketError { AlreadyRunning, Dir(String), Bind(String), Chmod(String) }` (`Display` with operator-friendly text); `pub async fn setup_listener(path: &Path) -> Result<tokio::net::UnixListener, SocketError>`: (a) `create_dir_all(parent)` best-effort — failure → `Dir` ("use `--socket /tmp/ramsleuth.sock` as non-root"); (b) if the file exists → probe a live daemon with a std `UnixStream::connect` (1 s timeout): connect OK → `AlreadyRunning`, connect refused → stale → `remove_file` + rebind; (c) `UnixListener::bind`; (d) `chmod 0660` (PermissionsExt) + best-effort `chown(path, uid, gid)` to the `ramsleuth` group when `getgrnam` finds it (libc; absent group → warning only, mode stays `0660`); (e) any `io` error → wrapped, never a panic.
- **Dependency:** `[COUPLED-TO: P3-12]`
- **Quality Gates:** idiomatic Rust; zero clippy (needs tokio `macros` for `#[tokio::test]` — feature present, D7); unit tests in a temp dir: fresh bind succeeds + `0660` mode observed; pre-existing **dead** file is removed and rebound; pre-existing **live** listener → `AlreadyRunning`; parent-dir failure → `Dir` with the `--socket` hint.
- **Exit Criteria:** `/run/ramsleuth/ramsleuth.sock` (or any `--socket` path) is created safely, mode `0660`, idempotent across restarts.

### Chunk P3-14 — TTL telemetry cache
- **Target File:** `crates/ramsleuth-daemon/src/cache.rs` (new; +`mod cache;` in `lib.rs`)
- **Scope Boundary:** `pub struct TelemetryCache { max_age: Duration, collector: Box<dyn Fn() -> SystemMemoryTelemetry + Send + Sync>, data: std::sync::RwLock<Option<Cached>> }` (`Cached { at: Instant, snap: SystemMemoryTelemetry }`); `pub fn new(max_age)` (collector = `ramsleuth_telemetry::collect`), `pub fn with_collector(max_age, c)` (tests), `pub fn get(&self) -> SystemMemoryTelemetry` (fresh → clone under the read lock; stale/absent → collect **outside** the lock, then store — `collect()` never returns `Err`, so no error path), `pub fn force_refresh(&self)`.
- **Dependency:** `[COUPLED-TO: P3-12]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: fake counting collector — first `get` = 1 call, immediate second = still 1, after TTL expiry = 2; `force_refresh` always re-calls; clone equality.
- **Exit Criteria:** `GetTelemetry` RPCs are cheap and deterministic; the cache is fully testable without hardware.

### Chunk P3-15 — single-flight benchmark job manager
- **Target File:** `crates/ramsleuth-daemon/src/bench_job.rs` (new; +`mod bench_job;` in `lib.rs`)
- **Scope Boundary:** `pub enum JobEvent { Progress(StreamProgress), Complete(BenchmarkGrid), Cancelled, Failed(String) }`; `pub struct JobHandle { pub run_id: u64, pub events: tokio::sync::mpsc::UnboundedReceiver<JobEvent> }`; `pub struct JobManager { active: Mutex<Option<u64>>, next_id: AtomicU64, flags: Mutex<HashMap<u64, Arc<AtomicBool>>> }` with `new()`, `is_active()`, `start(opts: &StreamOptions) -> Option<JobHandle>` (busy → `None`; else assign `run_id`, store the cancel flag, `tokio::task::spawn_blocking` a closure that: `detect()` (fail → `Failed`) → `plan(&topo)` sizes → `run_streamed(topo, sizes, opts, &flag, &mut |p| send(Progress(p)))` → terminal event `Complete`/`Cancelled`/`Failed`, then `release`), `cancel(run_id) -> bool` (sets the flag; run_streamed halts between passes → `Cancelled`), `release(run_id)`.
- **Dependency:** `[COUPLED-TO: P3-09, P3-12]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests (injected tiny `StreamOptions` + temp-dir-free): `start` while busy → `None` and the second `run_id` untouched; `cancel` on an active run → `Cancelled` event received on the handle (use a cooperative run via a pre-set flag or a fast option set); terminal `Complete` on a small full run; `release` idempotent.
- **Exit Criteria:** one benchmark at a time, clean cancellation, every run produces exactly one terminal event.

### Chunk P3-16 — RPC dispatch + progress forwarding
- **Target File:** `crates/ramsleuth-daemon/src/rpc.rs` (new; +`mod rpc;` in `lib.rs`)
- **Scope Boundary:** `pub async fn handle_conn(stream: tokio::net::UnixStream, cache: &Arc<TelemetryCache>, jobs: &Arc<JobManager>)`: incremental frame reader (16 KiB `read_buf` → `decode_frame::<Message>`, `Incomplete` → read more; `Oversized`/`Decode` → reply `Response::Error` + close) over `Message::Request`: `GetTelemetry` → `spawn_blocking(|| cache.get())` → `Response::Telemetry`; `StartBenchmark { target, mode }` → map to `StreamOptions { tiers: [target], metrics: mode.to_metrics(), use_avx512: host_has_avx512() }` → `jobs.start`: busy → `Response::Error("benchmark already running")`, else reply `BenchAccepted { run_id }` and enter the **owner loop**: `select!` on `handle.events` (forward `Progress` → `BenchProgress`; terminal → `BenchComplete`/`BenchCancelled`/`Error`, `jobs.release`, stop) and on the next inbound frame (`CancelBenchmark { run_id }` → `jobs.cancel` + reply `BenchCancelled { run_id }` ack; `StartBenchmark` → `Error` busy; `GetTelemetry` → served as usual); `CancelBenchmark` from a non-owner connection → ack if the `run_id` is active, else `Error("no such run")`. Connection close tears down nothing shared (single-flight state is in `JobManager`).
- **Dependency:** `[COUPLED-TO: P3-13, P3-14, P3-15]`
- **Quality Gates:** idiomatic Rust; zero clippy; integration-style unit test: loopback `UnixListener` in a temp dir, spawn `handle_conn` with a fake `cache` (P3-14 `with_collector`) + `JobManager`, a raw client writes `GetTelemetry` frame → reads `Telemetry` frame; then `StartBenchmark` with a tiny `StreamOptions` → `BenchAccepted` + ≥1 `BenchProgress` + terminal `BenchComplete`; `CancelBenchmark` on a fresh run → `BenchCancelled` terminal.
- **Exit Criteria:** the full RPC surface of Grand Design §4.1 is served correctly, including streamed progress and clean cancel.

### Chunk P3-17 — daemon binary entry + systemd unit
- **Target File:** `crates/ramsleuth-daemon/src/main.rs` (rewrites the stub) + `systemd/ramsleuth.service` (new, repo root) — bin+unit ship together (multi-file exception)
- **Scope Boundary:** `fn main() -> ExitCode`: std `env::args` parsing — `--socket <path>` (default `ramsleuth_protocol::DEFAULT_SOCKET_PATH`), `--max-age <secs>` (default 5); `caps::probe()` → `eprintln!` the warnings (soft, D5); `socket::setup_listener` → on error: friendly `eprintln!`, `ExitCode::FAILURE`; build `Arc<TelemetryCache>` + `Arc<JobManager>`; `#[tokio::main(flavor = "multi_thread")]`: accept loop `select!` with `tokio::signal::unix` SIGTERM + ctrl-c; on signal → cancel the active run, ≤2 s grace for the terminal event, `remove_file` the socket, exit `0`. The unit: `Type=simple`, `User=root`, `AmbientCapabilities=CAP_SYS_RAWIO`, `NoNewPrivileges=true`, `ProtectHome=true`, `ProtectSystem=strict`, `PrivateTmp=true`, `ReadWritePaths=/run/ramsleuth`, `Restart=on-failure`, `ExecStart=/usr/bin/ramsleuth-daemon --socket /run/ramsleuth/ramsleuth.sock`.
- **Dependency:** `[COUPLED-TO: P3-12, P3-13, P3-14, P3-15, P3-16]`
- **Quality Gates:** idiomatic Rust; zero clippy; manual-verification notes in the file doc (dev: `--socket /tmp/ramsleuth.sock` unprivileged; production: the unit); unknown flag → exit 2 with usage text (bench CLI precedent); no `panic!` on any signal/state.
- **Exit Criteria:** the daemon runs standalone in both privilege states, degrades per D5, and shuts down cleanly (socket removed, no orphans).

### Group D — the unprivileged client (CLI + shared IPC library)

### Chunk P3-18 — client crate birth + IPC transport
- **Target File:** `crates/ramsleuth-client/Cargo.toml` (new deps: serde [workspace], path deps protocol/telemetry/bench) + `src/lib.rs` (new: doc + re-exports — the shared library the TUI/GUI consume) + `src/client.rs` (new) — crate-birth exception (≈110 lines)
- **Scope Boundary:** `pub enum ClientError { NotRunning { path: String, hint: String }, Timeout { op: &'static str }, Io(String), Protocol(String) }` (`Display` — `hint` text: "start it with `sudo systemctl start ramsleuth` (or locally: `ramsleuth-daemon --socket /tmp/ramsleuth.sock`)"); `pub struct DaemonClient { stream: std::os::unix::net::UnixStream }` (synchronous — D2): `connect(path) -> Result<Self, ClientError>` (3 attempts, 250 ms timeout, 100 ms·(n+1) backoff; `NotFound`/`ConnectionRefused` → retries then `NotRunning`; sets 5 s read/write timeouts), `request(&mut self, req: &Request) -> Result<Response, ClientError>` (frame out → frame in; inbound `Message::Request` = `Protocol` error), `request_streaming(&mut self, req, on_event: impl FnMut(&Response) -> bool) -> Result<Response, ClientError>` (reply + subsequent frames: `on_event(&resp)` for non-terminal, stop + return at the first terminal `BenchComplete`/`BenchCancelled`/`Error` or `on_event == false`).
- **Dependency:** `[COUPLED-TO: P3-11]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: loopback std `UnixListener` thread answering a pre-encoded `Telemetry` frame → `connect` + `request` round-trip; `NotRunning` on a dead path (3 retries observed via timing, no hang); `Timeout` on a silent listener.
- **Exit Criteria:** one small synchronous transport serves the CLI and (via `lib`) the TUI/GUI.

### Chunk P3-19 — `dump` (the exit-criterion command)
- **Target File:** `crates/ramsleuth-client/src/dump.rs` (new; +`mod dump;` in `lib.rs`)
- **Scope Boundary:** `pub fn render(snap: &SystemMemoryTelemetry) -> String` — the dashboard-style listing (Grand Design §3.1): header lines (CPU vendor/brand; daemon socket), Clocks & Ratios, Primary, Secondary, Tertiary & Turnarounds, CAD bus (Ω / RZQ codes via `RttValue`), Voltages (mV→V display), per-slot SPD modules — **every** `Section` cell renders its value or `N/A (<reason>)` from `NaReason` (pure formatting; no socket logic), and `pub fn dump(path: &Path) -> Result<String, ClientError>` = `connect` + `GetTelemetry` + `render`.
- **Dependency:** `[COUPLED-TO: P3-18, P3-06]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: `render` on a fixture snapshot — (a) fully-populated AMD values formatted (MHz/Ω/V), (b) all-`Na` host state renders `N/A (DriverMissing)`/`N/A (UnsupportedHardware)` lines with empty no-panic, (c) SPD module lines present for a 2-module fixture.
- **Exit Criteria:** `render` is presentation-safe in every privilege state; `dump` is one RPC away from the exit criterion.

### Chunk P3-20 — `bench` + `status` commands
- **Target File:** `crates/ramsleuth-client/src/commands.rs` (new; +`mod commands;` in `lib.rs`)
- **Scope Boundary:** `pub fn bench(path: &Path, target: Tier, mode: BenchMode) -> Result<String, ClientError>` — `StartBenchmark` via `request_streaming`: print a progress line per `BenchProgress` (`[tier/metric] Preparing|Pass n/3|Finished`), return the terminal result as the AIDA64-style grid text (cells of the run, MB/s for Memory / GB/s per the Phase 1 convention, ns for latency; `Error` text on failure); `pub fn status(path: &Path) -> Result<String, ClientError>` — `GetTelemetry` → one-line-per-section summary (`AMD: <populated|N/A (reason)>`, `Intel: …`, `SPD: n modules`, `daemon: <socket>`).
- **Dependency:** `[COUPLED-TO: P3-18, P3-09, P3-10]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: grid-text renderer on a fixture `BenchmarkGrid` (all 16 cells, units correct); a scripted `request_streaming` fake (loopback server emitting 2 `BenchProgress` + terminal) → progress lines + terminal text; `status` on a fixture snapshot.
- **Exit Criteria:** both subcommands work over the shared transport, including live progress rendering.

### Chunk P3-21 — client binary entry
- **Target File:** `crates/ramsleuth-client/src/main.rs` (rewrites the stub)
- **Scope Boundary:** `fn main() -> ExitCode`: subcommand dispatch `dump` | `bench` | `status` (std `env::args`, no clap) with `--socket <path>` (default `DEFAULT_SOCKET_PATH`), `--tier memory|l1|l2|l3` (default `memory`), `--mode all|read|write|copy|latency` (default `all`); unknown subcommand/flag → usage text, exit 2; `ClientError` → friendly `eprintln!` (incl. the daemon-down hint), exit 1; success → stdout, exit 0.
- **Dependency:** `[COUPLED-TO: P3-18, P3-19, P3-20]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: arg parsing (tier/mode string maps, unknown values); manual-verification note for the CORE GATE command.
- **Exit Criteria:** `cargo run -p ramsleuth-client -- dump` is the Phase 3 exit-criterion command (proven at the gate below).

### ⛩ CORE GATE (milestone — after P3-21, before any TUI/GUI chunk)

> **CORE GATE — do not begin any TUI/GUI chunk until unprivileged `cargo run -p ramsleuth-client -- dump` prints full hardware timings against a running daemon.**

Reviewer procedure: (1) as the dev user: `cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock` (no root — expect the soft capability warning, still serving); (2) same user: `cargo run -p ramsleuth-client -- --socket /tmp/ramsleuth.sock dump` → full dashboard-style listing (CPU line; AMD/Intel sections as live values **or** structured `N/A (DriverMissing)`/`N/A (UnsupportedHardware)`; SPD lines when `ee1004` is present), exit 0, no panic; (3) `… bench --tier memory --mode all` → progress lines + 4-metric grid; (4) `… status` → per-section summary; (5) daemon killed mid-bench → client reports a clean protocol/IO error, no hang > timeout. **Only then may P3-22…P3-30 merge.**

### Group E — TUI (ratatui + crossterm), unprivileged

### Chunk P3-22 — TUI crate birth + key events
- **Target File:** `crates/ramsleuth-tui/Cargo.toml` (new deps: ratatui, crossterm, path deps client/telemetry/bench) + `src/events.rs` (new) — crate-birth exception
- **Scope Boundary:** `pub enum Action { Refresh, Snapshot, Quit }`; `pub fn key_to_action(code: crossterm::event::KeyCode) -> Option<Action>` (pure: `R`/`S`/`Q`, case-insensitive) + `pub fn poll(timeout_ms: u64) -> Vec<Action>` (crossterm event poll wrapper; mouse/resize ignored, resize re-read by the terminal each frame).
- **Dependency:** `[COUPLED-TO: P3-18]` (gate cleared); MSRV check: ratatui/crossterm releases ≤1.75 (D1, §7)
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: `key_to_action` for all three keys + a non-action key → `None`.
- **Exit Criteria:** the TUI's input contract is frozen and testable without a terminal.

### Chunk P3-23 — TUI dashboard (3 zones)
- **Target File:** `crates/ramsleuth-tui/src/ui.rs` (new; the TUI is bin-only — no `lib.rs`: its `mod ui;` wiring line lands in `main.rs` when P3-24 merges)
- **Scope Boundary:** `pub struct AppState { pub connected: bool, pub last_error: Option<String>, pub telemetry: Option<SystemMemoryTelemetry>, pub bench: Option<BenchmarkGrid>, pub progress: Option<(String, u8, u8)> }` + `pub fn draw(f: &mut ratatui::Frame, state: &AppState)`: `Layout` vertical [header 2 rows: title + CPU/daemon status] + horizontal [left 40%: zone 1 timing matrix (clocks, primary, secondary, tertiary, CAD, voltages — every cell value or `N/A (<reason>)`, from the `Section` fields; zone 2 in the left-bottom when bench idle); middle: zone 2 AIDA64 4×4 grid (ratatui `Table`, MB/s/GB/s/ns formatting) + progress bar; right: zone 3 SPD module cards (maker/part/rank/speed/profiles) + daemon status + `[R]efresh [S]napshot [Q]uit` legend].
- **Dependency:** `[COUPLED-TO: P3-22]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests on the pure cell-formatters (value vs `N/A (<reason>)` strings, MB/s↔GB/s unit pick per tier) — drawing itself verified manually at exit (terminal screenshot review per the repo convention); renders at 80×24 minimum without panic.
- **Exit Criteria:** all 3 Grand Design §3 zones render from `AppState` in one frame.

### Chunk P3-24 — TUI main loop + background updates
- **Target File:** `crates/ramsleuth-tui/src/main.rs` (rewrites the stub; declares `mod events; mod ui;`)
- **Scope Boundary:** `fn main() -> ExitCode`: crossterm raw mode + alternate screen (restore on exit, `Drop`-safe guard); background `std::thread`: every 2 s `DaemonClient::connect(--socket)` + `GetTelemetry` → `Arc<RwLock<AppState>>` (down → `connected=false`, `last_error` hint); loop: `events::poll(250)` → `[R]` force a refresh now, `[S]` write a timestamped `ramsleuth-snapshot-<unixts>.txt` (the rendered dashboard text via `client::dump::render`) to the CWD, `[Q]` break → exit 0; draw each tick (ratatui backend on crossterm).
- **Dependency:** `[COUPLED-TO: P3-22, P3-23]`
- **Quality Gates:** idiomatic Rust; zero clippy; no blocking call on the UI thread (all IPC in the background thread); manual exit: daemon-down → status zone shows the hint, TUI stays responsive; live daemon → values update; renders at 80×24 and 200×60.
- **Exit Criteria:** the TUI is a stable unprivileged live dashboard with R/S/Q.

### Group F — GUI (egui + eframe), unprivileged

### Chunk P3-25 — GUI crate birth + semantic style
- **Target File:** `crates/ramsleuth-gui/Cargo.toml` (new deps: egui, eframe, egui_extras, png, serde_json, path deps client/telemetry/bench) + `src/style.rs` (new) — crate-birth exception (≈105 lines)
- **Scope Boundary:** palette constants (Grand Design §3.2, exact): `CYAN = Color32::from_rgb(0x00, 0xD4, 0xFF)` (primary timings/bandwidth), `AMBER = from_rgb(0xFF, 0xB3, 0x00)` (sync clocks 1:1, voltages, low latency), `SLATE = from_rgb(0x1E, 0x1E, 0x24)` (backgrounds/separators), `CRIMSON = from_rgb(0xFF, 0x3B, 0x30)` (1:2 desync, out-of-spec voltages); `pub fn dark_style() -> egui::Style` (slate window/panel fills, cyan-strong widgets); `pub fn export_json(snap: &SystemMemoryTelemetry, grid: Option<&BenchmarkGrid>) -> Result<String, String>` (`serde_json::to_string_pretty` of a small pretty-struct built from the reused types — no type duplication); `pub fn snapshot_png(frame: &eframe::Frame) -> Result<PathBuf, String>` (read the framebuffer via eframe's glow context — verify the exact accessor name at build; `png` crate encode; `ramsleuth-snapshot-<unixts>.png` in the CWD; unavailable context → `Err` shown as a status toast, never a crash).
- **Dependency:** `[COUPLED-TO: P3-18]` (gate cleared); MSRV check: egui/eframe/egui_extras/png releases ≤1.75 (D1, §7)
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: the four hex constants decode to the exact RGB; `export_json` on fixture snapshot+grid produces parseable JSON with the same values; `export_json` with `grid=None` omits the grid block.
- **Exit Criteria:** the visual contract (palette + exports) is frozen and testable headlessly.

### Chunk P3-26 — GUI background update loop
- **Target File:** `crates/ramsleuth-gui/src/update.rs` (new; bin module — `main.rs` declares `mod update; …`)
- **Scope Boundary:** `pub struct TelemetryData { pub connected: bool, pub error: Option<String>, pub telemetry: Option<SystemMemoryTelemetry>, pub grid: Option<BenchmarkGrid>, pub progress: Option<String>, pub running: bool }` (GUI presentation state — wraps the reused wire types, duplicates none) + `pub fn start_updater(socket: String, rx: std::sync::mpsc::Receiver<UpdateCmd>) -> Arc<RwLock<TelemetryData>>`: background `std::thread` — every 2 s `GetTelemetry` (fresh connect each cycle: survives daemon restarts; down → `connected=false` + hint) and service `UpdateCmd { RunBenchmark(Tier, BenchMode), Cancel }` over the client (streaming progress → `progress` text; terminal → `grid`/`running=false`).
- **Dependency:** `[COUPLED-TO: P3-25]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: `TelemetryData` default/degradation states; a scripted command channel (in-test sender) flips `running`/`progress`/`grid` as expected — no socket needed.
- **Exit Criteria:** the UI thread only ever reads `Arc<RwLock<TelemetryData>>` — 60 FPS feasibility (D6: no IPC on the render thread).

### Chunk P3-27 — GUI timing matrix zone
- **Target File:** `crates/ramsleuth-gui/src/telemetry_zone.rs` (new)
- **Scope Boundary:** `pub fn show(ui: &mut egui::Ui, data: &TelemetryData)`: `egui::Grid` (uniform columns) for Grand Design §3.1 left panel — Clocks & Ratios (MCLK/UCLK/FCLK MHz, UCLK:MCLK, Gear, GDM/PDM — sync `1:1` in AMBER, `1:2` in CRIMSON), Primary (tCL/tRCDWR/tRCDRD/tRP/tRAS, CYAN), Secondary, Tertiary & Turnarounds, CAD bus (Ω / RttValue codes), Voltages (mV→V; SOC > 1.30 V on AM5 → CRIMSON) — every `Section` cell prints its value or a slate `N/A (<reason>)`; disconnect → one slate hint line (no panic).
- **Dependency:** `[COUPLED-TO: P3-26]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests on the pure value-color pickers (sync ratio, voltage band, `N/A` text) — drawing verified manually (screenshot review).
- **Exit Criteria:** the full timing matrix renders from live data with the semantic palette.

### Chunk P3-28 — GUI benchmark zone
- **Target File:** `crates/ramsleuth-gui/src/bench_zone.rs` (new)
- **Scope Boundary:** `pub fn show(ui, data, tx: &UpdateCmdSender)`: `egui_extras::TableBuilder` 4×4 (Memory/L1/L2/L3 × Read/Write/Copy/Latency; MB/s for Memory, GB/s for cache tiers, ns for latency; low-latency band in AMBER, out-of-band in CRIMSON); controls: `Run Full`, `Memory Only`, `Cancel` (→ `UpdateCmd`); progress bar while `running` (from `data.progress`).
- **Dependency:** `[COUPLED-TO: P3-26]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests on the unit-formatter per (tier, metric); manual: 60 FPS steady during an active run (progress updates without frame stalls — the render thread never blocks, D6).
- **Exit Criteria:** the AIDA64-style grid + interactive controls + live progress in the right panel.

### Chunk P3-29 — GUI hardware/SPD + status zone
- **Target File:** `crates/ramsleuth-gui/src/status_zone.rs` (new)
- **Scope Boundary:** `pub fn show(ui, data)`: bottom-right panel — daemon status line (connected + socket path, or the red hint), per-slot SPD cards (maker/part/rank/speed + EXPO/XMP profile summaries from `SpdModule`/`SpdProfile`), and the `ACTIONS` row: `[F2] Snapshot PNG  [F3] Export JSON  [Q] Quit`.
- **Dependency:** `[COUPLED-TO: P3-26]`
- **Quality Gates:** idiomatic Rust; zero clippy; unit tests: the SPD card text builder on a fixture module (profile present/absent); manual: renders with an empty SPD list (`N/A (DriverMissing)` line, no panic).
- **Exit Criteria:** the third Grand Design §3 zone is complete.

### Chunk P3-30 — GUI main + 60 FPS app shell
- **Target File:** `crates/ramsleuth-gui/src/main.rs` (rewrites the stub; declares `mod style; mod update; mod telemetry_zone; mod bench_zone; mod status_zone;`)
- **Scope Boundary:** `fn main() -> ExitCode`: `eframe::run_native("RamSleuth", NativeOptions { viewport: `1400×900` initial, `..Default::default() }, …)` with `cc.egui_ctx.set_style(style::dark_style())`; app struct holds `Arc<RwLock<TelemetryData>>` (from `update::start_updater(--socket)`) + the command sender; each frame: brief `RwLock` read (never blocking — the updater owns writes) → header bar (title, `[AMD <platform>]`, daemon status — Grand Design §3.1 top strip) → `CentralPanel`: [left: telemetry_zone | right: bench_zone / status_zone stacked]; keys: F2 → `style::snapshot_png`, F3 → `style::export_json` (both to the CWD, result as a toast line), Q → `ctx.send_viewport_cmd(Close)`; **60 FPS** = eframe's vsync default + zero render-thread I/O (D6) — the 2 s telemetry cadence and the benchmark run happen entirely in the background thread.
- **Dependency:** `[COUPLED-TO: P3-25, P3-26, P3-27, P3-28, P3-29]`
- **Quality Gates:** idiomatic Rust; zero clippy; manual exit (headless build `cargo build -p ramsleuth-gui --release` OK; on the desktop host: steady 60 FPS, no freeze during a full run, F2 PNG + F3 JSON files land in the CWD, daemon-down → hint only, no crash).
- **Exit Criteria:** the v2 §4.2 GUI exit criterion: steady 60 FPS, no UI freezes during active benchmark runs.

---

## 5. Phase 3 Acceptance & Exit Criteria (consolidated)

1. **The exit criterion (v2 §3.2):** unprivileged `cargo run -p ramsleuth-client -- dump` prints full hardware timings (values or structured `N/A (<reason>)` for every cell) without sudo, exit 0 — proven at the **CORE GATE** (§4) with `--socket /tmp/ramsleuth.sock`.
2. **RPC surface (Grand Design §4.1):** `GetTelemetry` (cached), `StartBenchmark { target, mode }` (progress streamed, single-flight), `CancelBenchmark` (clean worker termination) — all demonstrated client-side; a second concurrent `StartBenchmark` is refused with `Error`; the socket is mode `0660` at the default path under the systemd unit.
3. **Graceful degradation (Grand Design §4.2):** daemon run unprivileged → soft warning + all-`Na`-where-privileged sections, still serving; missing driver / unknown PM version / no `/dev/mem` → `Na(<reason>)` in the payload; **no panic, no segfault** in daemon or any client in any privilege/CPU state (re-verify the Phase 2 no-panic contract across the new boundary: client → frame → daemon → collect → frame → client).
4. **TUI (v2 §4):** renders the 3 zones cleanly at 80×24 and larger; `[R]`efresh / `[S]`napshot (`.txt` written) / `[Q]`uit; daemon-down shows the hint, stays responsive.
5. **GUI (v2 §4):** steady 60 FPS without freezes during active benchmark runs; `egui::Grid` timing matrix + `egui_extras::TableBuilder` benchmark grid; semantic palette exact (`#00D4FF`/`#FFB300`/`#1E1E24`/`#FF3B30`); F2 snapshot PNG + F3 export JSON written to disk; Q quits.
6. **Quality (Global):** `cargo clippy --workspace --all-targets -- -D warnings` clean; whole-workspace `cargo test` green (debug + release); `cargo build --workspace --release` OK; no tokio outside `ramsleuth-daemon`; no duplicated telemetry/bench payload structs anywhere (protocol reuses types by reference — audited at the CORE GATE and final review).

## 6. Global quality gates (apply to every chunk)

- Idiomatic Rust, Edition 2021; `rust-version = 1.75` **kept** (D1); all `unsafe` (there is none in new code) would carry `// SAFETY:` — the Phase 3 surface is entirely safe Rust over the already-audited Phase 1/2 internals.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo test --workspace` → green, debug **and** release; unit tests co-located (`#[cfg(test)]`) for every chunk that adds logic; no `panic!`/`unwrap`/`expect` on socket/hardware-derived data in daemon or clients (frame/IO errors are structured values).
- Merge model: `branch/chunk-P3-xx` → `v2-development` via `git merge --no-ff` after per-chunk review + tests; interface-freeze chunks (`[CRITICAL-PATH]`: P3-01, P3-09, P3-10, P3-11, P3-12) merge first within their group; **no TUI/GUI chunk merges before the CORE GATE passes**; 100% local — no pushes (handover §6).

## 7. Dependency ledger (DO NOT add now — owned by the named chunk)

| Dependency | Version/features | Added in | Owner file(s) |
|---|---|---|---|
| `serde` | `1` + `derive` (workspace entry) | **P3-01** (telemetry Cargo) / **P3-07** (bench Cargo) / **P3-10** (workspace block + protocol/daemon/client Cargos) | every payload type file, protocol, daemon, client |
| `bincode` | **`1.3`** (workspace entry; 2.x rejected — D3) | **P3-10** | `crates/ramsleuth-protocol/src/frame.rs` |
| `tokio` | `1` + `rt-multi-thread, net, io-util, sync, time, signal, macros` (workspace entry; **daemon only** — D2/D7) | **P3-12** | `crates/ramsleuth-daemon/src/{socket,bench_job,rpc,main}.rs` |
| `libc` | `0.2` | **P3-12** | `caps.rs` (`geteuid`), `socket.rs` (`chown`/`getgrnam`) |
| `ratatui` | latest MSRV ≤1.75 (verify; pin if needed) | **P3-22** | `tui/{ui,main}.rs` |
| `crossterm` | latest MSRV ≤1.75 (verify; pin if needed) | **P3-22** | `tui/{events,main}.rs` |
| `egui` / `eframe` | matching pair, latest MSRV ≤1.75 (verify; pin if needed) | **P3-25** | `gui/*.rs` |
| `egui_extras` | matching `egui` version | **P3-25** | `gui/bench_zone.rs` |
| `png` | latest MSRV ≤1.75 (verify; pin if needed) | **P3-25** | `gui/style.rs` (`snapshot_png`) |
| `serde_json` | latest MSRV ≤1.75 (verify; pin if needed) | **P3-25** | `gui/style.rs` (`export_json`) |

Rejected: clap (std `env::args` precedent), `log`/`env_logger` (stderr `eprintln!`), JSON-RPC (Bincode per the stack doc), tokio in clients/TUI/GUI (synchronous `UnixStream` — D2), any duplication of telemetry/bench payload structs (D2).

## 8. Topological execution order

```
Group A (serde, the two completed crates)
  P3-01 (error.rs) ─┬─> P3-02 (cpuid) ───────────────┬─> P3-06 (facade: SystemMemoryTelemetry) ─┐
  P3-01 ──> P3-03 (amd_readout) ──> P3-04 (intel_readout) ─┤                                     │
  P3-01 ──> P3-05 (spd_decode) ──────────────────────────┘                                     │
  P3-07 (bench worker) ───────────────────────────────────────────────────────────────────────┤
  P3-08 (bench orchestrator: serde + pub(crate)) ─> P3-09 (bench streamed: run_streamed) ─────┘
Group B (protocol — needs the serde roots)
  P3-10 (crate + messages: Request/Response/BenchMode/Message) ─> P3-11 (frame codec)
Group C (daemon — needs the protocol)
  P3-12 (crate + caps) ─> P3-13 (socket) ─┐
  P3-12 ─> P3-14 (cache) ─────────────────┤─> P3-16 (rpc) ─> P3-17 (main + systemd unit)
  P3-12 + P3-09 ─> P3-15 (bench_job) ─────┘
Group D (client — needs the protocol only)          [pipeline-able alongside C]
  P3-18 (crate + client.rs) ─> P3-19 (dump) ─┬─> P3-21 (main)
  P3-18 ─> P3-20 (commands) ─────────────────┘
  ⛩ CORE GATE — proven before any E/F merge
Group E (TUI)                                    Group F (GUI)
  P3-22 (crate + events) ─> P3-23 (ui) ─> P3-24 (main)     P3-25 (crate + style) ─> P3-26 (update) ─┬─> P3-27 (telemetry_zone)
                                                                                                     ├─> P3-28 (bench_zone)
                                                                                                     └─> P3-29 (status_zone) ─> P3-30 (main)
```

Critical-path (interface) freezes merge first: **P3-01, then P3-09, then P3-10 + P3-11, then P3-12** — in that order; everything else in a group is parallelizable by lease once its in-group deps merge.

---

*Plan authored for Cycle 3 (Phase 3). Chunk count: **30** (P3-01…P3-30). The rough 18–24 estimate in the brief assumed pre-merged file pairs; the strict 1-file-per-chunk law sets the floor at 30 for the mandated surface (6 telemetry serde files + 3 bench + 2 protocol + 6 daemon + 4 client + 3 TUI + 6 GUI), so this plan holds the law and lands at 30 small, single-file (or crate-birth paired) chunks.*
