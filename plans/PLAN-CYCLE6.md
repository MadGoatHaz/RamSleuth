# RamSleuth v2 — Cycle 6 Plan: GUI Workstream (Grand Design §3.1/§3.2 parity)

> **Plan file:** `plans/PLAN-CYCLE6.md` (the handover §13 names this `PLAN-CYCLE6.md` — or `PLAN-PHASE7.md`; this file uses the handover's primary name. It is the Cycle 6 GUI plan, distinct from the per-phase `PLAN-PHASE<N>.md` series — Phase 1 = `PLAN.md`, Phase 6/Cycle 5 = `PLAN-PHASE6.md`.)
> **Base branch:** `v2-development` — every `branch/chunk-c6-NN` forks from and merges back here (`--no-ff`), NOT `main`.
> **Baseline (2026-09-15):** tip `19f56a2`, tree clean; **371/371 tests green debug+release**; `cargo clippy --workspace --all-targets -- -D warnings` zero; MSRV `1.75` **held** (lockfile-pinned — no bump, no new deps that break the pin); `ryzen_smu` installed+loaded on the 5950X host.
> **Scope:** close the 8-item "GUI Gap" (`FULLSCOPEvsCOMPLETED.md` §"GUI Gap", the authoritative list) so `ramsleuth-gui` matches Grand Design §3.1/§3.2. Nothing is hardware-gated — the whole list is implementable on the 5950X host today (operator involvement = the QA live-run + any DMI/AGESA source that degrades to honest N/A).
> **Mandate:** 100% pure Rust, Cargo workspace, Edition 2021; GUI = `egui` + `eframe` + `egui_extras` (0.27.2 line, already pinned) — **no new dependencies**; the chart is hand-rolled immediate-mode, settings are in-memory. Every chunk honors the no-panic contract (structured `Na(<reason>)`, never panic/`unwrap`/`expect` on hardware data) and changes **no frozen interface except the FROZEN SHAPES below** (a shape change = this plan + the ripple + rebase).
> **Chunk discipline:** one target source file per chunk, ~50–100 lines of non-test change, plus **at most one** `mod`/wiring line in a second file (wiring not counted — the Phase 2/3/5/6 precedent). A frozen/pub interface change is a plan edit + rebase, never silent.
> **IDs:** C6-01 … C6-30; execution order is the list order (data-model + wire freeze first, then GUI-local). Dependency tags: `[CRITICAL-PATH]` (interface freeze), `[COUPLED-TO: C6-NN]`, `[ISOLATED]`.

---

## 1. Confirmed facts (from recon — use, do not re-derive)

- **Three frontends consume the payload.** `ramsleuth-client` (CLI `dump`), `ramsleuth-tui` (ratatui), and `ramsleuth-gui` (egui) all render `SystemMemoryTelemetry`. The `daemon` serves it from `TelemetryCache` (an injectable `collect` closure) — so **any field added to the telemetry shapes must be added at every hand-construction fixture site across all four crates**, or the workspace will not compile. (The daemon has no sysfs of its own; it runs `collect()` — so the daemon's *only* Cycle-6 change is its test fixtures.)
- **`CpuInfo` (cpuid.rs L83) is CPUID-pure**: `vendor: CpuVendor` + `brand: String`, produced by `detect()` which "reads CPUID only via `__cpuid` — no memory, SMU, or MMIO." It carries **no clock / motherboard / BIOS / AGESA**. It is therefore left **unchanged** (see D-C1).
- **`SystemMemoryTelemetry` (facade.rs L52)**: `cpu`, `amd: Section<AmdReadout>`, `intel: Section<IntelReadout>`, `spd: Vec<SpdModule>`. No total-capacity slot. The facade branches (`amd_branch`/`intel_branch`/`spd_branch`) each degrade independently; `collect()` assembles them.
- **`ClockReadout` (amd_readout.rs L155)**: `mclk_mhz`, `uclk_mhz`, `fclk_mhz`, `div_mode: Section<DivMode>`, `gear_mode`, `gdm: Section<bool>`, `pdm` — **no command-rate slot**.
- **Command rate is decoded but not stored (P6-02):** `amd_smn.rs` L197 `pub(crate) fn command_rate(reg) -> u8 = ((reg >> 10) & 1)` (0=1T,1=2T), currently `#[allow(dead_code)]`; `SmnFields` (L330) carries only `gdm` + `timings`; `apply_smn` (L586/L616) writes **only** `gdm` + `timings`; the 0x50200 mask test (L799) already includes bit 10 (`0x0000_0400`). `AmdPmSnapshot` (amd_pm.rs L236) has raw `gdm`/`pdm: u8` (parse leaves them 0; `apply_smn` fills) — the exact pattern `command_rate` will mirror.
- **`SpdModule` (spd_decode.rs L167)**: `index`, `is_ddr5`, `maker`, `part`, `serial`, `rank`, `density_mbit`, `speed_mts`, `profiles` — **no die-maker / die-type / devices slot**. `decode_maker` (L273) already reads the **die ID** (bytes `0x2E`/`0x2F` DDR5, `0x100`/`0x101` DDR4) as a *fallback* when no module maker is present — the die maker is decoded today but not separately carried. `decode_rank` (L328) reads byte `0x80`: `total = cfg>>4`, `per_rank = cfg&0x0F` (devices/rank) — the `per_rank` value is already computed and is the per-DIMM device count. `density_mbit` is **per-die** Mbit (the 5950X G.Skill 16 GiB rank-1 kit = 16 Gb/die × 8 devices = 16 GiB).
- **Round-trip tests (extend, don't touch the codec):** `cpuid.rs` L333 `cpu_info_bincode_round_trip`; `facade.rs` L437 `system_memory_telemetry_bincode_round_trip` + L476 `collect_on_this_host_bincode_round_trip`; `protocol/messages.rs` L148 `telemetry_response_bincode_round_trip` (its `fixture_snapshot` L112 hand-builds a `SystemMemoryTelemetry` + inline `SpdModule` L119); `protocol/frame.rs` L160 `encode_decode_round_trip` (its fixtures use `BenchmarkGrid`/`StreamProgress` only — **no snapshot construction → frame.rs needs no ripple**; it re-runs green because the grown snapshot still encodes under the 16 MiB guard). `bench_op`/`worker_result`/`grid` round-trips in the bench crate are unaffected.
- **GUI map (7 files, 3543 lines):** `main.rs` (774) `render_header` L337–376 (title + CPU brand + daemon status + key legend + transient notice — **the header gap**), `render_zones` L384–423, `perform_export` L195 (F2→`snapshot_png`, F3→`export_json`), the eframe `update()` loop L305 (no keyboard handling); `style.rs` (401) `snapshot_png` L173 (312×312 bench-grid mini-heatmap — the export-parity gap) + `export_json` L140 (telemetry-only); `update.rs` (689) `TelemetryData` L83 (no history buffer), `BenchState` L61, `spawn_poller` L294 (`TELEMETRY_INTERVAL`=2 s const L49, `POLLER_TICK`=200 ms), `poll_telemetry` L131, `run_bench` L195; `telemetry_zone.rs` (627) `timing_cells` L64 (one flat single-column grid, `push_readout` L110 — the grouped-layout gap) + `render_cell_grid` L318; `bench_zone.rs` (428) `render_grid_table` L192 (terminal grid only) + `render_progress` L241 (single bar from `progress.last()` — the per-cell-live gap); `status_zone.rs` (540) `spd_cards` L71 / `card_rows` L77 (maker/part/rank/density/speed + profile rows — the SPD-content gap) + `render_actions` L274 (buttons only — the keyboard gap).
- **No DMI reading exists anywhere** in the workspace (grep for `dmi`/`board_name`/`bios_version` in telemetry + daemon → zero hits). `ryzen_smu` sysfs (`/sys/kernel/ryzen_smu_drv/{pm_table,pm_table_version,smn,codename,drv_version,version}`) is the existing unprivileged sysfs surface; a new `platform.rs` reuses the same `std::fs` read idioms.
- **Grand Design §3.1 mockup (the target):** line 1 `RamSleuth v2.0.0  [AMD AM5 Platform]  Daemon: Connected (IPC: …)`; line 2 `CPU: <brand> @ <clock> GHz | Motherboard: <board> (BIOS: <ver>, AGESA <aga>)`; line 3 `RAM: <total> GB (<N>x<S>GB) <speed> MT/s | <Dual|Quad>-Channel | Mode: Synchronous 1:1 (UCLK = MCLK = <mclk> MHz)`. Left panel = 2 sub-columns × 3 section pairs: `[Clocks & Ratios] | [Tertiary & Turnarounds]`, `[Primary Timings] | [CAD Bus Drive & Termination]`, `[Secondary Timings] | [Active System Voltages]`; the Clocks section's `GDM / CR: Disabled / 1T` row. Right = the AIDA 4×4 grid (live during a run), the run controls + progress, and the `HARDWARE & SPD` cards (`<product line>`, `DRAM Die: <maker> (<type>, <density>) | <Single|Dual>-Rank`, the EXPO/XMP profile line).

---

## 2. The 8 GUI Gap items → workstream (gating)

| # | Item | Class | Chunk(s) | Gate |
|---|---|---|---|---|
| 1 | Header + data-model gaps (land first) | data-model + wire + GUI | **C6-01…C6-19 (shapes+ripple), C6-20 (header)** | shapes: round-trip + no-panic; header: live GUI run |
| 2 | Grouped 2×3 timing layout | GUI-local | **C6-21** | live GUI run |
| 3 | GDM / CR row (command-rate slot) | data-model + wire + GUI | **C6-03/04/05 (shape), C6-21 (cell)** | round-trip + no-panic; live |
| 4 | Per-cell live bench updates | GUI-local | **C6-22** | live GUI run (grid fills during a run) |
| 5 | SPD card content (product line, die maker+type, human rank) | data-model + wire + GUI | **C6-02 (shape), C6-23 (card)** | round-trip + no-panic; live |
| 6 | History / charting (ring buffer + plot) | GUI-local (state) | **C6-24/25** | live (60 FPS, no freeze) |
| 7 | Settings / configuration (interval/units/theme) | GUI-local (state) | **C6-26/27** | live |
| 8 | Export parity (F2 full card, F3 +bench) + keyboard F2/F3/Q | GUI-local | **C6-28/29/30** | live (PNG/JSON + keys) |

**Gating:** items 1/3/5 are data-model + wire (telemetry + protocol + all three frontends consume the payload — the **bincode round-trip + no-panic gates apply**); items 2/4/6/7/8 are GUI-local. **Nothing is hardware-gated.** No parked chunks: all 30 run sequentially on the 5950X host. The only operator involvement is the QA live-run (and any DMI/AGESA source that degrades to honest N/A — not a gate).

---

## 3. FROZEN SHAPES (exact field additions — frozen before implementation; a deviation = plan edit + rebase)

All new fields are `Section<T>` (the frozen `error.rs` `Section`/`NaReason`) or raw `u8` in the snapshot, matching the existing per-cell containment; every one is `serde`-derived and rides the bincode round-trip. **`CpuInfo` is NOT changed** (it stays CPUID-pure; its round-trip test is unchanged).

**NEW `SystemPlatform`** — `platform.rs` (a new vendor-neutral branch, DMI + `/proc` sourced, like the SPD branch; each field degrades independently, `collect_platform() -> SystemPlatform` never fails the process):
| field | type | source | fallback |
|---|---|---|---|
| `cpu_clock_mhz` | `Section<f64>` | `/proc/cpuinfo` `cpu MHz` (first core) | `Na(NotApplicable)` if the file/line is absent |
| `motherboard` | `Section<String>` | `/sys/class/dmi/id/board_name` (fallback `board_vendor`, then `product_name`) | `Na(NotApplicable)` (VM/container) |
| `bios` | `Section<String>` | `/sys/class/dmi/id/bios_version` (` + <bios_date>` when present) | `Na(NotApplicable)` |
| `agesa` | `Section<String>` | best-effort AMD AGESA token parsed from the BIOS string, else the `ryzen_smu` `version`/`smu_fw` attr when it carries one | `Na(NotApplicable)` (common — no clean unprivileged AGESA) |

Plus a free fn **`mem_total_gib() -> Section<f64>`** (`/proc/meminfo` `MemTotal` kB → GiB) — the total-capacity fallback (a memory property, so it is a free fn, not a `SystemPlatform` field).

**`SystemMemoryTelemetry`** — `facade.rs` (adds three fields; the existing four stay):
| field | type | computed |
|---|---|---|
| `platform` | `SystemPlatform` | `collect_platform()` (new branch, runs on every vendor) |
| `total_capacity` | `Section<f64>` (GiB) | `sum(dimm_sizes)` when ≥1 DIMM, else `mem_total_gib()`, else `Na` |
| `dimm_sizes` | `Vec<Section<f64>>` (GiB) | parallel to `spd`; per module = `density_mbit × devices / 8192`; `Na` when that module's density or devices is `Na` |

**`SpdModule`** — `spd_decode.rs` (adds three fields; `decode()` populates them):
| field | type | source |
|---|---|---|
| `die_maker` | `Section<String>` | DRAM-die JEP106 from bytes `0x2E`/`0x2F` (DDR5) / `0x100`/`0x101` (DDR4) — the die ID `decode_maker` already reads as a fallback, now carried separately; `Na` when absent |
| `die_type` | `Section<String>` | human die-type label; **default `Na(NotApplicable)`** (a die variant like "A-Die" is not a standard SPD field — a documented die_maker+density→label mapping may be filled in later) |
| `devices` | `Section<u8>` | DRAM devices/rank = byte `0x80` bits 3:0 (the `per_rank` `decode_rank` already computes); `Na` when the rank config is invalid |

**`ClockReadout`** — `amd_readout.rs` (adds one field):
| field | type | mapped from |
|---|---|---|
| `command_rate` | `Section<CommandRate>` | `AmdPmSnapshot.command_rate`: `0→OneT`, `1→TwoT`, else `Na(ParseError(…))` |

**NEW `CommandRate`** — `amd_readout.rs`: `#[derive(…serde…)] pub enum CommandRate { OneT, TwoT }` (display `1T`/`2T`).

**`AmdPmSnapshot`** — `amd_pm.rs` (adds one raw slot, mirroring `gdm`/`pdm`): `command_rate: u8` (`parse()` leaves `0`; `apply_smn` fills it).

**`SmnFields`** — `amd_smn.rs` (adds one field): `command_rate: u8` (decoded from `0x50200` bit 10 via the existing `command_rate()` extractor — its `#[allow(dead_code)]` is removed).

**The `apply_smn` write-surface change (the only P6-02 frozen-contract extension):** the overlay now writes `gdm` + `timings` + `command_rate` (was `gdm` + `timings` only). Documented in the module + this plan; the sentinel rule (0x50200 `0xFFFFFFFF` read-failure → the field decodes to 0 → honest `Na`) is unchanged and never decodes as data.

---

## 4. DESIGN DECISIONS (each with a one-line rationale)

- **D-C1 — Platform identity is a new `SystemPlatform` branch, not fields on `CpuInfo`.** `CpuInfo::detect()` is CPUID-pure (no sysfs/DMI); motherboard/BIOS/AGESA/clock are DMI + `/proc` sources, so they belong in an independent vendor-neutral branch that degrades per-field (exactly like the SPD branch), leaving `CpuInfo` and its round-trip test untouched.
- **D-C2 — The command rate is a new `CommandRate` display enum + a raw `AmdPmSnapshot.command_rate` slot, decoded from SMN `0x50200` bit 10 (reusing the existing extractor) and written through `apply_smn`.** The value is already decoded-but-not-stored (P6-02); the honest path is to add the frozen slot + extend the overlay, mirroring exactly how `gdm` flows (raw snapshot → `map_amd` → `Section` display enum) — never a signature change.
- **D-C3 — Per-DIMM capacity = `density_mbit × devices / 8192` GiB; total = Σ per-DIMM with a `/proc/meminfo` `MemTotal` fallback when no SPD modules are present; all degrade to honest `Na`.** The SPD density is per-die and the Rank-Config device count is already parsed, so their product is the module's true capacity; the meminfo fallback keeps the RAM line populated even when the SPD driver is absent (no-panic, no invented data).
- **D-C4 — History/charting: a hand-rolled immediate-mode line plot (no new dependency) over a fixed-capacity ring buffer of `N=300` samples (= 10 min at the 2 s poll), appended by `spawn_poller`.** egui has no built-in chart and the workspace's no-new-deps spirit (MSRV-pinned lockfile) favors a ~40-line `ui.painter()` plot over adding a chart crate; 300 samples is enough for a 10-min trend without unbounded memory.
- **D-C5 — Settings: in-memory only this cycle (a `Settings` struct on `TelemetryData` — poll interval, units, theme — surfaced by a settings panel); the poller reads the live interval each tick; persistence to `$XDG_CONFIG_HOME` is a documented follow-up, not implemented.** Keeps the no-panic / no-new-file-I/O contract and the CLI-only `--socket` surface unchanged; in-memory settings satisfy the "units / theme / refresh controls" gap without a new I/O path to harden.
- **D-C6 — Export parity: F2 becomes a full "validation card" PNG (the header + the 4 zones' key readouts rendered to a larger PNG via the existing `png` encoder — the bench mini-heatmap stays as one component), and F3's JSON gains the bench grid + run state alongside the telemetry (a small `ExportPayload` wrapper).** The spec's F2 is "a clean .png validation card of the dashboard," not the bench grid alone; reusing the existing encoder + the frozen wire payloads (no new deps) closes the parity gap honestly.
- **D-C7 — Keyboard: the eframe `update()` loop polls `ctx.input()` for `Key::F2`/`Key::F3`/`Key::Q` and maps them to the same `GuiAction` values the status-zone buttons emit (one `handle_action` path).** The spec's legend is keyboard-driven; routing keys through the existing `GuiAction`/`perform_export` keeps a single side-effect path (no render-thread I/O, D6) and makes the buttons and keys behaviorally identical.
- **D-C8 — The RAM-line sync-mode summary + channel mode are GUI-local derivations (no new wire field): sync = `ClockReadout.div_mode` + `mclk_mhz` ("Synchronous 1:1 (UCLK = MCLK = <mclk> MHz)"); channel = `spd.len()` (1→Single, 2→Dual, 4→Quad, 0→N/A); speed = the max `SpdModule.speed_mts`.** Both are already present in the frozen payload (div mode + mclk on the AMD readout; module count + speed on `spd`), so deriving them in the header keeps the wire minimal and no-panic (missing data → N/A text).
- **D-C9 — The `dimm_sizes` vector stays parallel to `spd` (not a per-module `capacity` field).** The gap list explicitly places "per-DIMM size" on `SystemMemoryTelemetry`; a parallel `Vec` (one entry per module) lets the header print "N×S GB" without re-deriving, and it degrades per-module (a `Na` entry when that DIMM's density/devices is `Na`) consistent with the containment model.

---

## 5. MICRO-CHUNKS (30 single-file micro-chunks, in execution order)

Each entry: **id · target file · scope · tag · branch · verification gate.** "Ripple" = the small fixture updates that make a crate compile+test-green against the frozen shapes (a shape change ripples to every hand-construction site — §1). The `16 MiB` frame guard and the no-panic contract are the two standing gates on every wire/telemetry chunk.

### Phase A — Data model + wire freeze (items 1/3/5 shapes) `[CRITICAL-PATH]`

**C6-01 · `crates/ramsleuth-telemetry/src/platform.rs` (NEW)** — the `SystemPlatform` struct (D-C1) + `collect_platform()` + `mem_total_gib()`; pure `std::fs` reads of `/proc/cpuinfo` (clock), `/sys/class/dmi/id/{board_name,board_vendor,product_name,bios_version,bios_date}` (motherboard/BIOS), a best-effort AGESA parse (D-C1); every field degrades to `Na` on any failure (no panic, no `unwrap` on I/O). In-file tests: a synthetic DMI/cpuinfo/meminfo parse, the AGESA-from-BIOS path, and a graceful-host run (`collect_platform()` never panics). → `[CRITICAL-PATH]` · `branch/chunk-c6-01-platform` · *gate: `cargo test -p ramsleuth-telemetry` (new) green + clippy `-D warnings`; no other file touched.*

**C6-02 · `crates/ramsleuth-telemetry/src/spd_decode.rs`** — `SpdModule += die_maker, die_type, devices` (D-C3/D-C9); `decode()` populates them — `die_maker` from the die-ID bytes (`0x2E`/`0x2F` DDR5, `0x100`/`0x101` DDR4, the source `decode_maker` already reads as a fallback), `devices` from the `per_rank` `decode_rank` already computes (byte `0x80` bits 3:0), `die_type` = `Na(NotApplicable)` default; update the in-file fixture + the `die_maker_fallback`/`jep106`/prefix-sweep tests + the `SpdModule` bincode round-trip (new fields cross the wire). → `[CRITICAL-PATH]` · `branch/chunk-c6-02-spddie` · *gate: telemetry round-trip + no-panic green; `SpdModule`/`SpdProfile` otherwise frozen.*

**C6-03 · `crates/ramsleuth-telemetry/src/amd_readout.rs`** — `CommandRate { OneT, TwoT }` enum + `ClockReadout.command_rate: Section<CommandRate>` (D-C2); `map_amd` maps the snapshot's `command_rate` (`0→OneT`, `1→TwoT`, else `Na(ParseError)`); update the nested `AmdReadout`/`ClockReadout` test fixtures (`good_snapshot`, `all_na_readout`) + add a `CommandRate` bincode test. → `[CRITICAL-PATH]` · `branch/chunk-c6-03-cmdrate` · *gate: telemetry round-trip + no-panic; the `ClockReadout` display enum is the only AMD-shape addition here.*

**C6-04 · `crates/ramsleuth-telemetry/src/amd_pm.rs`** — `AmdPmSnapshot += command_rate: u8` (raw slot, `parse()` leaves `0` — the `gdm`/`pdm` pattern); update `parse()` + the `parse` test fixture. → `[CRITICAL-PATH]` · `branch/chunk-c6-04-pmcr` · *gate: telemetry parse tests green; the snapshot's zeroed-SMN-field contract (P2-04) is preserved — `command_rate` is an SMN field, not a PM-table field.*

**C6-05 · `crates/ramsleuth-telemetry/src/amd_smn.rs`** — `SmnFields += command_rate: u8`; `decode_smn` populates it from `0x50200` bit 10 (reuse the existing `command_rate()` extractor — remove its `#[allow(dead_code)]`); **extend the `apply_smn`/`apply_smn_with` write surface to `gdm + timings + command_rate`** (the documented P6-02 overlay extension — D-C2); update `base_snapshot` + the zero-field/sentinel tests (the `0xFFFFFFFF` sentinel still decodes `command_rate` to 0 → honest `Na`, never data). → `[CRITICAL-PATH]` · `branch/chunk-c6-05-smncr` · *gate: the SMN overlay tests (sentinel + per-register containment) green; the module doc's write-surface line updated.*

**C6-06 · `crates/ramsleuth-telemetry/src/facade.rs`** — `SystemMemoryTelemetry += platform, total_capacity, dimm_sizes` (D-C3/D-C9); add `platform_branch()` (vendor-independent, like `spd_branch`); compute `dimm_sizes` (`density_mbit × devices / 8192` per module, `Na` when that module's density/devices is `Na`) + `total_capacity` (Σ `dimm_sizes`, else `mem_total_gib()`, else `Na`); update `assemble()`; update all fixtures (`fixture_module`, `fixture_amd` +`command_rate`, `pm_only_snapshot` +`command_rate`, the two `SystemMemoryTelemetry` bincode round-trips — the `collect_on_this_host` one proves the live shape is wire-safe). → `[CRITICAL-PATH, COUPLED-TO C6-01/02/03/04/05]` · `branch/chunk-c6-06-facade` · *gate: whole-snapshot bincode round-trip (both fixtures) green + no-panic; this is the wire-shape hub.*

**C6-07 · `crates/ramsleuth-telemetry/src/lib.rs`** — wire `pub mod platform;` + re-export `SystemPlatform` (+ `mem_total_gib`) at the root (one wiring line, not counted). → `[CRITICAL-PATH, COUPLED-TO C6-01]` · `branch/chunk-c6-07-lib` · *gate: `cargo build -p ramsleuth-telemetry` + the crate re-export compiles.*

**C6-08 · `crates/ramsleuth-telemetry/src/main.rs`** — the telemetry CLI's own `all_na()` + `fixture_module()` fixtures updated for the new fields (compile ripple; the CLI's *printed* rows stay frozen this cycle — the new fields ride the wire, the human-facing print is unchanged). → `[CRITICAL-PATH, COUPLED-TO C6-02/06]` · `branch/chunk-c6-08-cli` · *gate: `cargo test -p ramsleuth-telemetry` (incl. the CLI bin tests) green.*

### Phase B — Consumer ripple (make the whole workspace compile+test-green against the frozen shapes) `[CRITICAL-PATH]`

Each is a small fixture update (a few lines) in one file so its crate's tests compile + the bincode round-trips extend with the new fields. After C6-01…C6-19, **all 371 baseline tests (now extended) are green debug+release**.

**C6-09 · `crates/ramsleuth-protocol/src/messages.rs`** — `fixture_snapshot += platform/total_capacity/dimm_sizes` + the inline `SpdModule += die_maker/die_type/devices`; `telemetry_response_bincode_round_trip` + the other arm tests re-run green (the wire gate — the grown snapshot crosses the `Response::Telemetry` arm). → `[CRITICAL-PATH, COUPLED-TO C6-06/02]` · `branch/chunk-c6-09-wire` · *gate: `cargo test -p ramsleuth-protocol` green + the `DEFAULT_SOCKET_PATH` freeze test untouched.*

**C6-10 · `crates/ramsleuth-client/src/dump.rs`** — the three `SystemMemoryTelemetry` fixtures (`representative`/`intel_populated`/`all_na`) + the nested `AmdReadout`/`ClockReadout`(+`command_rate`)/`IntelReadout`/`SpdModule` fixtures updated (compile ripple; the dump's printed rows stay frozen). → `[CRITICAL-PATH, COUPLED-TO C6-06/02/03]` · `branch/chunk-c6-10-dump` · *gate: `cargo test -p ramsleuth-client` green.*

**C6-11 · `crates/ramsleuth-client/src/commands.rs`** — `mixed_snapshot` + `all_na_snapshot` fixtures updated (+ nested `IntelReadout`/`SpdModule`). → `[CRITICAL-PATH, COUPLED-TO C6-06/02]` · `branch/chunk-c6-11-commands` · *gate: `cargo test -p ramsleuth-client` green.*

**C6-12 · `crates/ramsleuth-client/src/client.rs`** — `mock_snapshot` fixture updated. → `[CRITICAL-PATH, COUPLED-TO C6-06]` · `branch/chunk-c6-12-client` · *gate: `cargo test -p ramsleuth-client` green (the `request_get_telemetry_round_trip` proves the full frame round-trip).*

**C6-13 · `crates/ramsleuth-daemon/src/rpc.rs`** — `mock_snapshot` fixture updated (the daemon serves the frozen `collect()` shape unchanged — only its test mock gains the fields). → `[CRITICAL-PATH, COUPLED-TO C6-06]` · `branch/chunk-c6-13-rpc` · *gate: `cargo test -p ramsleuth-daemon` green.*

**C6-14 · `crates/ramsleuth-daemon/src/cache.rs`** — `mock_snapshot` fixture updated. → `[CRITICAL-PATH, COUPLED-TO C6-06]` · `branch/chunk-c6-14-cache` · *gate: `cargo test -p ramsleuth-daemon` green.*

**C6-15 · `crates/ramsleuth-tui/src/main.rs`** — `mock_snapshot` fixture updated. → `[CRITICAL-PATH, COUPLED-TO C6-06]` · `branch/chunk-c6-15-tui` · *gate: `cargo test -p ramsleuth-tui` green.*

**C6-16 · `crates/ramsleuth-tui/src/ui.rs`** — the two `SystemMemoryTelemetry` fixtures + the nested `AmdReadout`/`ClockReadout`(+`command_rate`)/`IntelReadout`/`SpdModule` fixtures updated (compile ripple; the TUI's printed cells stay frozen). → `[CRITICAL-PATH, COUPLED-TO C6-06/02/03]` · `branch/chunk-c6-16-tui-ui` · *gate: `cargo test -p ramsleuth-tui` green.*

**C6-17 · `crates/ramsleuth-gui/src/update.rs`** — `mock_snapshot` fixture updated (compile ripple for the GUI's own tests; the GUI feature chunks C6-25/C6-27 follow on this same file). → `[CRITICAL-PATH, COUPLED-TO C6-06]` · `branch/chunk-c6-17-gui-mock` · *gate: `cargo test -p ramsleuth-gui` green (poller/stand-in tests).*

**C6-18 · `crates/ramsleuth-gui/src/telemetry_zone.rs`** — the three `SystemMemoryTelemetry` fixtures (`representative`/`intel_populated`/`all_na`) + the nested `AmdReadout`/`ClockReadout`(+`command_rate`)/`IntelReadout` fixtures updated (compile ripple only; the grouped-layout feature is C6-21). → `[COUPLED-TO C6-17]` · `branch/chunk-c6-18-zone1-ripple` · *gate: `cargo test -p ramsleuth-gui` green.*

**C6-19 · `crates/ramsleuth-gui/src/status_zone.rs`** — the three `SystemMemoryTelemetry` fixtures (`representative`/`no_spd`/`all_na_snapshot`) + the two `SpdModule` fixtures (`fixture_module`/`all_na_module`) updated (compile ripple only; the card-content feature is C6-23). → `[COUPLED-TO C6-17]` · `branch/chunk-c6-19-zone3-ripple` · *gate: `cargo test -p ramsleuth-gui` green.*

### Phase C — GUI features (the workstream; in Gap-item order)

**C6-20 · `crates/ramsleuth-gui/src/main.rs`** — *Item 1 — the 3-line spec header.* Rework `render_header` (L337): line 1 = `RamSleuth` + a platform tag `[<Vendor> <Platform> Platform]` (from `CpuInfo.vendor` + a vendor→platform map: AMD→`AM5`/`AM4` best-effort, Intel→`LGA` best-effort, else `Platform`; honest when absent) + the daemon status; line 2 = `CPU: <brand> @ <clock GHz> | Motherboard: <board> (BIOS: <ver>, AGESA <aga>)` (from `cpu.brand` + `platform.{cpu_clock_mhz,motherboard,bios,agesa}`, each `Na`-degrading to its text); line 3 = `RAM: <total GB> (<N>x<S>GB) <speed> MT/s | <Dual|Quad>-Channel | Mode: <Synchronous 1:1|Asynchronous 1:2> (UCLK = MCLK = <mclk> MHz)` (from `total_capacity`/`dimm_sizes`/`spd.len()`/`spd speed`/`amd.{div_mode,mclk_mhz}` per D-C8). A missing `platform`/`amd` degrades each segment to its N/A text — never a panic. → `[ISOLATED]` · `branch/chunk-c6-20-header` · *gate: `cargo test -p ramsleuth-gui` green (add a header-line builder unit test) + the live GUI run shows the populated 3-line header.*

**C6-21 · `crates/ramsleuth-gui/src/telemetry_zone.rs`** — *Item 2 + Item 3 — the grouped 2×3 layout + the GDM/CR row.* Change the cell model from a flat `Vec<(String,String)>` to a list of section blocks (`struct TimingSection { title, rows }`); regroup `push_readout` into the six spec sections — left column `[Clocks & Ratios]` (MCLK/UCLK/FCLK/UCLK:MCLK/gear/`GDM / CR`/PDM), `[Primary Timings]` (tCL/tRCDWR/tRCDRD/tRP), `[Secondary Timings]` (tRAS/tRC/tRRDS/tRRLD/tFAW); right column `[Tertiary & Turnarounds]`, `[CAD Bus Drive & Termination]`, `[Active System Voltages]` — and the **`GDM / CR` row shows `Disabled / 1T`** (from `gdm` + the new `command_rate`, item 3's cell). Rework `render_cell_grid` to lay the six sections out in 2 sub-columns (an `egui::Grid` of two label/value sub-columns per row-pair). Update the pure-core tests (section grouping + the CR row). → `[COUPLED-TO C6-18]` · `branch/chunk-c6-21-zone1-group` · *gate: `cargo test -p ramsleuth-gui` green (the `timing_cells`→sections tests) + the live run shows the 2×3 layout with the `GDM / CR: … / …T` row.*

**C6-22 · `crates/ramsleuth-gui/src/bench_zone.rs`** — *Item 4 — per-cell live bench updates.* A pure `live_grid(progress: &[StreamProgress]) -> BenchmarkGrid` that accumulates each streamed event's `(tier, op, value)` into the grid (latest value per cell wins; unmeasured cells stay 0.0→`N/A`); `render_grid_table` picks the live grid while `bench.running` and the terminal `grid` otherwise, so the 4×4 fills cell-by-cell during a run (the single progress bar stays). Update `render_progress`/the tests. → `[ISOLATED]` · `branch/chunk-c6-22-bench-live` · *gate: `cargo test -p ramsleuth-gui` green (a `live_grid` test over a synthetic `BenchProgress` stream) + the live run fills the grid progressively during a full bench.*

**C6-23 · `crates/ramsleuth-gui/src/status_zone.rs`** — *Item 5 — SPD card content.* Enrich `card_rows`: a product-line row (maker + part, e.g. `G.Skill … (F5-…)`), a `DRAM Die` row (`<die_maker> (<die_type>, <density>Gb)` — `die_type` degrading to its N/A text, so "SK Hynix (16Gb)" when no type is known), and a human rank label row (`Single-Rank`/`Dual-Rank` from `rank`; the raw rank stays available). Update the card tests. → `[COUPLED-TO C6-19]` · `branch/chunk-c6-23-zone3-card` · *gate: `cargo test -p ramsleuth-gui` green + the live run shows the product line + die + human rank.*

**C6-24 · `crates/ramsleuth-gui/src/history.rs` (NEW)** — *Item 6 — the ring buffer + hand-rolled plot.* A fixed-capacity `RingBuffer<T>` (capacity `300`; `push`/`iter`/`len`/`is_full`, oldest evicted) + a `HistoryState` (the last-N samples: `mclk_mhz` + `vddcr_soc_mv` + a bandwidth `f64`) + a pure `render_history(ui, &HistoryState)` immediate-mode line plot (`ui.painter().line_segment` over normalized points, no new dependency — D-C4). One `pub mod history;` wiring line in the GUI `lib.rs`. In-file tests (ring wrap-around, empty-state plot is a no-op, no panic). → `[ISOLATED]` · `branch/chunk-c6-24-history` · *gate: `cargo test -p ramsleuth-gui` green + clippy `-D warnings`.*

**C6-25 · `crates/ramsleuth-gui/src/update.rs`** — *Item 6 — history state.* `TelemetryData += history: HistoryState`; `spawn_poller` appends one sample per successful poll (the poller remains the only writer — D6 preserved); the `history` field is cleared/primed on reconnect. (The `mock_snapshot` ripple was C6-17.) → `[COUPLED-TO C6-24]` · `branch/chunk-c6-25-history-state` · *gate: `cargo test -p ramsleuth-gui` green (the poller test asserts one sample appended per poll) + the live run shows a growing 10-min sparkline.*

**C6-26 · `crates/ramsleuth-gui/src/settings.rs` (NEW)** — *Item 7 — settings.* A `Settings` struct (`poll_interval_ms: u64` default 2000, `units: Units` (GHz/MHz, GB/MB), `theme: Theme` (dark-slate/light)) with `Default` + a pure `render_settings(ui, &mut Settings)` panel (the toggles mutate the struct); a small `format_clock`/`format_bw` unit helper. One `pub mod settings;` wiring line in the GUI `lib.rs`. In-file tests (defaults, the unit formatting, no panic). → `[ISOLATED]` · `branch/chunk-c6-26-settings` · *gate: `cargo test -p ramsleuth-gui` green + clippy `-D warnings`.*

**C6-27 · `crates/ramsleuth-gui/src/update.rs`** — *Item 7 — settings state + dynamic interval.* `TelemetryData += settings: Settings`; `spawn_poller` reads the **live** `settings.poll_interval_ms` each tick (replacing the `const TELEMETRY_INTERVAL` for the poll cadence — the render thread still does no I/O, D6); a unit-formatting hook the zones use (the existing MHz/GB/s formatters read the `units`). (The `mock_snapshot` ripple was C6-17.) → `[COUPLED-TO C6-26]` · `branch/chunk-c6-27-settings-state` · *gate: `cargo test -p ramsleuth-gui` green (a test that a changed interval is honored by the poll cadence) + the live run honors the interval/units/theme controls.*

**C6-28 · `crates/ramsleuth-gui/src/style.rs`** — *Item 8 — F2 export parity.* Add `validation_card_png(data: &TelemetryData, path: &Path) -> Result<(), GuiError>` that renders the dashboard's key readouts (the header line-2/line-3 text + the 4 zones' values) to a larger PNG via the existing `png` encoder (the bench mini-heatmap stays as one embedded component); reuse the `GuiError` arms. In-file tests (valid PNG signature, a plausible dimension, no panic on a degraded `data`). → `[ISOLATED]` · `branch/chunk-c6-28-cardpng` · *gate: `cargo test -p ramsleuth-gui` green (the PNG is valid + non-trivial) + the live F2 writes the full validation card.*

**C6-29 · `crates/ramsleuth-gui/src/style.rs`** — *Item 8 — F3 export parity.* Change `export_json` to write a combined `ExportPayload { telemetry, bench, run_id, running }` (the frozen telemetry root + the `BenchState` grid/run state) so the JSON export carries the bench grid too (was telemetry-only). Keep the telemetry sub-object byte-compatible. In-file tests (the JSON round-trips into an equal `ExportPayload`; the telemetry sub-object is unchanged). → `[COUPLED-TO C6-28]` · `branch/chunk-c6-29-jsonbench` · *gate: `cargo test -p ramsleuth-gui` green + the live F3 writes telemetry + bench.*

**C6-30 · `crates/ramsleuth-gui/src/main.rs`** — *Item 8 — export dispatch + keyboard.* Update `perform_export` (F2 → `validation_card_png`, F3 → the combined `export_json`) and add keyboard handling: the eframe `update()` loop polls `ctx.input(|i| i.key_pressed(Key::F2/F3/Q))` → the same `GuiAction` → `handle_action` (one side-effect path — D6; the buttons and keys are behaviorally identical, D-C7); the header legend `[F2] snapshot · [F3] export · [Q] quit` is kept. In-file tests (the key→`GuiAction` mapping). → `[COUPLED-TO C6-20/28/29]` · `branch/chunk-c6-30-keys-export` · *gate: `cargo test -p ramsleuth-gui` green + the live run: F2/F3/Q keys fire the exports + quit; no render-thread I/O.*

---

### Traceability (8 Gap items → chunks)

| Item | Chunks |
|---|---|
| 1 — header + data model | C6-01…C6-08 (shapes: platform/total/dimm), C6-09…C6-19 (ripple), **C6-20** (header render) |
| 2 — grouped layout | **C6-21** |
| 3 — GDM/CR row | C6-03/04/05 (command-rate shape), **C6-21** (the CR cell) |
| 4 — per-cell live bench | **C6-22** |
| 5 — SPD card content | C6-02 (die_maker/die_type/devices shape), **C6-23** (card render) |
| 6 — history / charting | **C6-24/25** |
| 7 — settings | **C6-26/27** |
| 8 — export parity + keyboard | **C6-28/29/30** |

---

## 6. PIPELINE (sliding-window-2, gated — no parked chunks)

**Execution order:** C6-01 → C6-02 → … → C6-30, exactly the list order. The CRITICAL-PATH shape chunks (C6-01…C6-08) all land before the consumer ripple (C6-09…C6-19) starts, so the ripple branches fork from the **post-shape-freeze** `v2-development` tip; the GUI-feature chunks (C6-20…C6-30) fork after the ripple is green. Same-file chunks are strictly sequential (`COUPLED-TO`): `telemetry_zone.rs` C6-18→C6-21; `status_zone.rs` C6-19→C6-23; `update.rs` C6-17→C6-25→C6-27; `style.rs` C6-28→C6-29; `main.rs` C6-20→C6-30.

**Sliding window 2:** while the implementer works `branch/chunk-c6-NN`, the reviewer works the just-completed `branch/chunk-c6-(N-1)` — the reviewer runs that chunk's verification gate, lease-signs it in `DEV_LOG.md`, `git merge --no-ff`s it into `v2-development`, and prunes the branch. The implementer commits `feat(c6-NN): …` and pushes the branch to origin (local-only; the operator-gated push of `v2-development` itself stays O6).

**Rebase trigger:** after every merge to `v2-development`, the in-flight branch rebases onto the new tip **before** its review (the frozen-shape ripple means a branch that forked before a shape change cannot review against stale fixtures). Because the shape freeze (C6-01…C6-08) precedes the ripple, no ripple branch forks before the shapes; a same-file later chunk rebases onto its earlier sibling's merge. If a rebase reveals a conflict in a FROZEN SHAPE (not just a fixture), stop — it is a plan edit, not a merge.

**Circuit-breaker rule (hard stop):** the pipeline halts and the reviewer returns `STATUS: FAILED` on any of: (a) a compile failure in any crate after a merge; (b) `clippy --workspace --all-targets -- -D warnings` non-clean; (c) **any baseline test red** (371 + the cycle's additions, debug or release); (d) a **no-panic / no-segfault** violation (a panic, a `0xFFFFFFFF` sentinel decoded as data, or a crash in any privilege × CPU state); (e) a FROZEN-SHAPE field changed beyond §3 without a plan edit. On a trip: exact diagnostics are written to `DEV_LOG.md`, the offending branch is not merged, and the pipeline resumes only after the defect is fixed and its gate re-run. (No-panic is the workspace's core contract — it is the first thing checked, never waived.)

**No gates / no parking:** unlike Phase 6, nothing here is hardware- or operator-gated. The operator's only role is the final QA live-run (§7) and, if a DMI/AGESA source is absent on the host, accepting its honest N/A (not a gate). The O2 (Intel) / O5 (MSRV) / O6 (push) items stay parked and are **not** part of Cycle 6.

---

## 7. QA + COMPACTION

**Exit criteria (all must hold before the cycle is declared COMPLETE):**
1. **Full regression:** `cargo test --workspace` green in **both debug and release** (the 371 baseline tests — now extended with the new-shape round-trips + the platform/SMN/SPD/GUI tests — plus zero regressions).
2. **Clippy:** `cargo clippy --workspace --all-targets -- -D warnings` = zero.
3. **MSRV + deps:** `rust-version = "1.75"` unchanged; no new dependency added (the chart is hand-rolled, settings in-memory); `Cargo.lock` pins intact (no `Cargo.toml`/`Cargo.lock`/`systemd/` diff outside the intended files).
4. **Frozen-shape audit:** the only wire-shape changes are exactly §3 (`SystemPlatform` new, `SystemMemoryTelemetry += platform/total_capacity/dimm_sizes`, `SpdModule += die_maker/die_type/devices`, `ClockReadout += command_rate`, `CommandRate` new, `AmdPmSnapshot += command_rate`, `SmnFields += command_rate`); every serde derive + the `cpuid.rs`/`facade.rs`/`messages.rs`/`frame.rs` round-trips extend with them and stay green; all three frontends (client/TUI/GUI) consume the payload (the three `mock_snapshot`/fixture ripples prove it).
5. **No-panic / no-segfault:** zero panics and zero segfaults across every privilege × CPU state run — `collect()` as root and unprivileged, the `smn` sentinel (`0xFFFFFFFF`) never decodes as data, the GUI with the daemon up / down / flapping, and an active full-bench run.
6. **Live GUI run on the 5950X host (operator):** with the daemon up (`--socket <sock>`), `cargo run -p ramsleuth-gui` opens the 1400×900 window at ~60 FPS (16 ms repaint) with **no freeze during an active full-bench run**, and shows: the 3-line header (platform tag + CPU line with clock/motherboard/BIOS/AGESA or honest N/A + RAM line with total/per-DIMM/speed/channel/sync); the grouped 2×3 timing layout with the `GDM / CR` row; the bench grid filling **per-cell live** during a run; the SPD cards with product line + die + human rank; a growing 10-min history sparkline; the settings panel (interval/units/theme honored); F2 → the full validation-card PNG; F3 → the combined telemetry+bench JSON; and the `F2`/`F3`/`Q` **keys** firing the same actions as the buttons. A flapping/absent daemon degrades to the placeholder + status line — no crash.

**Compaction (per the operating protocol, handover §12.6 — after the QA exit criteria pass):**
1. Merge the cycle's history into `MASTER_LOG.md` (the 30-chunk `feat(c6-NN)` + review/merge record).
2. Prune the 30 fully-merged `branch/chunk-c6-*` branches (each verified merged into `v2-development`; the `--no-ff` history stays on the branch).
3. Reset `DEV_LOG.md`: `@@@ CURRENT_STATE @@@` → `Cycle 6 (GUI workstream) COMPLETE — <exit-criteria summary>; compacted to MASTER_LOG; ready for Cycle 7.` and `@@@ ACTIVE_WORKERS @@@` → `(no active leases)`.
4. Refresh `Docs/HANDOVER.md` + `FULLSCOPEvsCOMPLETED.md` (mark the GUI Gap items closed, note the O2/O5/O6 still parked) for the next cycle.

**Out of scope / follow-ups (documented, not this cycle):** settings persistence to `$XDG_CONFIG_HOME` (D-C5); a `die_type` mapping table (D-C3, default N/A); extending the CLI `dump` / TUI printed rows with the new fields (the wire carries them; the human-facing print is frozen this cycle); the O2 Intel live decode / O5 MSRV / O6 push (operator-gated, parked).
