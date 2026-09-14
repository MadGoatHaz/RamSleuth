# RamSleuth v2 — Phase 6 Plan: Live-Hardware Verification & Remaining Reconciliation (Cycle 5)

> **Base branch:** `v2-development` — every `branch/chunk-p6-NN` forks from and merges back here (`--no-ff`), NOT `main`.
> **Sources of truth:** `Docs/HANDOVER.md` §5 (open items O1–O6), §6 (ryzen_smu install state), §8 (push policy), §9 (frozen interfaces), §13 (Cycle 5 entry order); `FULLSCOPEvsCOMPLETED.md` §4 (cross-cutting open items); `plans/PLAN-PHASE5.md` (chunk discipline, §6 MSRV fork, §7 push policy); `Docs/Grand Design & Architecture Specification.md` §4.2 (graceful-degradation / no-panic safety contract).
> **Mandate:** 100% pure Rust, Cargo workspace, Edition 2021, `rust-version = "1.75"` KEPT this cycle (P6-07 records the decision matrix; no manifest change is implemented). Every chunk respects the no-panic contract (structured `Na(<reason>)`, never panic / never `unwrap`/`expect` on hardware data) and changes **no frozen interface** (§3). Baseline 337/337 (debug+release) + clippy `-D warnings`-clean must stay green; new tests are additive.
> **Chunk discipline:** one target source file per chunk, ~50–100 lines of non-test change, plus **at most one** `mod`/wiring line in a second file (wiring not counted against the budget — the Phase 2/3 precedent). The single documented exception is P6-02 (a NEW module whose payload IS a const decode table + pure decoders + tests, mirroring the P5-15 model-reconciliation precedent). Per-chunk flow: implement on `branch/chunk-p6-NN-<slug>` → in-file unit tests → clippy `-D warnings` → lease-signed review → `--no-ff` merge → branch pruned. A frozen/pub interface change is a plan edit + rebase, never silent.
> **IDs:** P6-01 … P6-08; the order below is the execution order (unblocked first, gated parked last — §6).

---

## 1. Confirmed facts (from recon — use, do not re-derive)

- **Git state (2026-09-14):** `v2-development` @ `b890ecf`, tree clean, 2 commits ahead of `origin/v2-development` @ `b908f7b` (local-only, unpushed — push is operator-gated, P6-08). 337/337 tests debug+release; `cargo clippy --workspace --all-targets -- -D warnings` clean; MSRV 1.75 held.
- **All hardware I/O lives in `ramsleuth-telemetry`** (the "daemon side"): `ramsleuth-daemon` holds **no sysfs code of its own** — it runs `ramsleuth_telemetry::collect` as the injectable TTL-cache collector (daemon `main.rs`) and `run_streamed` for benchmarks (`bench_job.rs`). The handover's "daemon-side extension" for the SMN path therefore lands in the telemetry crate's AMD chain: `amd_smu::acquire()` → `amd_pm::parse()` → `amd_readout::map_amd()` (assembled by `facade::amd_branch`).
- **`ryzen_smu` uAPI (verified against the installed amkillam v0.1.7 source at `/opt/ryzen-smu-src`):**
  - The `smn` attr (`/sys/kernel/ryzen_smu_drv/smn`, RW) is a **write-address→read-value** protocol: writing a 4-byte LE `u32` address makes the driver read that SMN register into a shared `smn_result`; reading the same attr returns the raw 4-byte LE result (`drv.c` `smn_show`/`smn_store`; `libsmu.c` `smu_read_smn_addr`: `lseek(0)` → `write(addr,4)` → `lseek(0)` → `read(4)`, on a single `O_RDWR` fd). On an SMU read failure `smn_result` is **not updated** (stale-value hazard) — the design answer is the per-field sanity gates (§3 D2/D3), not error plumbing. `SMU_Return_OK = 0x01`.
  - **Verified SMN bitfields — `monitor_cpu` `print_memory_timings()` (the sanctioned reference; `-m` flag = one-shot, then exit):**

    | SMN reg | field | bits | snapshot slot (`AmdPmTimings`/clocks) |
    |---|---|---|---|
    | `0x50200` | MCLK set-point = `(v & 0x7F) / 3 × 100` MHz | 6:0 | cross-check context (PM table owns MCLK) |
    | `0x50200` | **GDM** | 11 | `gdm` (handover anchor: GDM = 0x50200 bit 11) |
    | `0x50200` | command rate (1T/2T) | 10 | no frozen slot — decoded, documented, not stored |
    | `0x50204` | tCL | 5:0 | `cl` |
    | `0x50204` | tRAS | 14:8 | `ras` |
    | `0x50204` | tRCDRD | 20:16 | `rcdrd` |
    | `0x50204` | tRCDWR | 28:24 | `rcwdwr` |
    | `0x50208` | tRC | 7:0 | `rc` |
    | `0x50208` | tRP | 22:16 | `rp` |
    | `0x5020C` | tRRDS | 4:0 | `rrds` |
    | `0x5020C` | tRRDL | 12:8 | `rrld` |
    | `0x5020C` | tRTP | 28:24 | `rtp` |
    | `0x50210` | tFAW | 7:0 | `faw` |
    | `0x50214` | tCWL | 5:0 | `cwl` |
    | `0x50214` | tWTRS | 13:8 | `wtrs` |
    | `0x50214` | tWTRL | 20:16 | `wtrl` |
    | `0x50218` | tWR | 7:0 | `wr` |
    | `0x50220` | tRDRD dd / sd / sc / scl | 3:0 / 11:8 / 19:16 / 29:24 | `rdrd_dd` / `rdrd_sd` / `rdrd_sc` / `rdrd_scl` |
    | `0x50224` | tWRWR dd / sd / sc / scl | 3:0 / 11:8 / 19:16 / 29:24 | `wrwr_dd` / `wrwr_sd` / `wrwr_sc` / `wrwr_scl` |
    | `0x50228` | tWRRD / tRDWR | 3:0 / 12:8 | `wrrd` / `rdwr` |
    | `0x50254` | tCKE | 28:24 | no frozen slot — decoded, documented, not stored |
    | `0x50260` | tRFC / tRFC2 / tRFC4 | 9:0 / 20:11 / 31:22 | `rfc1` / `rfc2` / `rfcsb` |
    | `0x50264` | tRFC mirror (sentinel `0x21060138` → use this one) | — | mirror-check per the reference |

    CAD drive strengths/terminations and PDM are **not** in this verified table: they stay zeroed (→ honest `Disabled`/`Na` under the existing P2-05 gates) unless their bitfields are confirmed during P6-02's verification step (§3 D3).
  - Driver context attrs (read-only, for the cross-check record): `codename`, `drv_version`, `version`.
  - `monitor_cpu` flags: bare = PM-table monitor loop (1 s updates; SIGINT/TERM → clean exit); `-m` = one-shot SMN memory-timings block; `-v` = version; re-execs itself under `sudo` when not privileged.
- **Frozen interfaces (NO changes in Phase 6 — §3 of the handover, verified against code):**
  - `TelemetryError` (6 variants: `UnsupportedHardware{vendor}`, `DriverMissing{driver}`, `InsufficientPrivilege{hint}`, `UnknownPmTableVersion{version}`, `Io(io::Error)`, `Parse{detail}`), `NaReason` (6 arms), `Section<T>` — `error.rs` (P2-02).
  - `SmuContext { version: u32, pm: Vec<u8> }` (P2-03) and `AmdPmSnapshot` field **shape** (P2-04) — the snapshot's zeroed SMN fields are *populated in place* (P5-15 precedent: semantics changed, shape frozen); no field is added or renamed.
  - Display types: `ClockReadout`, `TimingSet`, `CadBus`, `VoltageSet`, `AmdReadout`, `DivMode`, `GearMode`, `RttValue` (P2-05); `IntelChannel`, `IntelReadout` (P2-07); `MchBar` (P2-06); `SpdModule`, `SpdProfile` (P2-09); `SystemMemoryTelemetry` + `collect` (P2-10).
  - Wire contract: `Request`/`Response`/`BenchMode`/`Message` + Bincode frame codec + every payload's bincode layout (P3); `WorkerResult`/`WorkerError`/`BenchOp` (P3-07); `StreamProgress`/`StreamError` (P3-09); client CLI: 3 subcommands (`dump`/`bench`/`status`), flags, exit codes 0/1/2 (P3-21); the Phase 1 checksum conventions (read/copy = wrapping LE-u64 word-sum **per pass**; write = byte counter; per-slice wrapping sums = whole-buffer per pass; best-of-3 bandwidth / median latency) and `BufferPlan` sizing (P1-03); CPUID family map (P2-01); `systemd/ramsleuth.service`.
  - MSRV 1.75 + committed lockfile pins (ratatui 0.29 / crossterm 0.28 via two lockfile pins; egui/eframe 0.27.2 line) until the P6-07 decision.
- **Current live state (5950X):** PM clocks ≈ 1792 MHz OneToOne + VDDCR_SOC ≈ 1.128 V (matches `monitor_cpu` ±10 mV); `gdm`/`pdm` = 0 → rendered `off` (the frozen `map_mode_flag(0)` semantics), 27 timings + 8 CAD codes = 0 → honest `Na`/`Disabled`; SPD: density `0x0D` → `Na(ParseError)`, maker `0xC1` → raw-hex `"0xC1"`; Intel `Na(UnsupportedHardware)`; L1 (32 KiB) / L2 (1 MiB) bandwidth cells are dominated by per-pass thread-spawn + barrier overhead at 16 pinned workers.
- **Small-tier geometry (16 workers on this host):** L1 32 KiB → 2 KiB slices; L2 1 MiB → 64 KiB; L3 (per-CCD slice; 16–32 MiB here) → ≥1 MiB; DRAM 256 MiB → 16 MiB. Only the two sub-4 MiB tiers are overhead-dominated.
- **No existing SMN code** anywhere in the workspace (`smn` appears only in docs); `nix 0.29` (`fs`/`ioctl`/`mman`) is the telemetry crate's only dependency and is already used for the char-dev path — the SMN accessor reuses the same `FdGuard` + `classify_io_error` idioms (plus `lseek`).

## 2. Phase 6 scope (open items → chunks)

| Open item (handover §5) | Work | Chunk(s) |
|---|---|---|
| O1 — AMD tick-identical ground truth (UNBLOCKED) | Matched-condition `monitor_cpu` vs `ramsleuth-client -- dump` (root daemon): clocks ±1 MHz, voltages ±10 mV; investigate the 1792-vs-1800 MHz delta (set-point register `0x50200[6:0]` vs measured PM-table MCLK); CAD gate deferred until the SMN path lands | **P6-01** (script) + operator live run after P6-03 |
| O3a — CAD / 27 DRAM timings / GDM / PDM via the driver's `smn` attr (sanctioned channel, NOT raw MMIO) | New `amd_smn` module (accessor + verified const bitfield table + pure decode + no-panic overlay) wired into `amd_branch` | **P6-02** + **P6-03** |
| O3b — SPD density `0x0D` + maker `0xC1` (outside the frozen tables) | Reconcile against JESD79-4 byte 19 / JEP106-0001 + the live module part number; update `JEP106` / `decode_density` + fixtures | **P6-04** |
| O4 — P1 L1/L2 bandwidth overhead refinement | Inner-loop iterations in the small-tier worker passes; large tiers unchanged; checksum conventions frozen | **P6-05** |
| O2 + O3c — Intel MCHBAR live decode (i5-6600, dual-channel) | Live validation of the skeleton IMC offset/bitfield table; reconcile offsets if they diverge; Intel voltages/CAD = `Na(NotApplicable)` | **P6-06** (HARDWARE-GATED) |
| O5 — MSRV 1.75 vs 1.89 (operator call) | DOCS-ONLY decision matrix (lockfile pins, AVX-512 cfg-gating, CI legs); no code change | **P6-07** (operator-gated) |
| O6 — Push to GitHub (operator go-ahead) | DOCS-ONLY runbook (ff-only push, prune the 17 `branch/chunk-p5-*`, optional `v2.0.0` tag); no remote operation from the pipeline | **P6-08** (operator-gated) |

## 3. Key decisions

**D1 — The SMN channel is a new telemetry module (`amd_smn.rs`), mirroring `amd_smu.rs`'s idioms.** Vendor gate first (pure, zero I/O on non-AMD); Linux-only cfg split; canonical `/sys/kernel/ryzen_smu_drv/smn` tried before legacy `/sys/kernel/ryzen_smu/smn` (the verified-kobject-first rule, pinned by test like the PM family); `O_RDWR` open + `lseek(0)`/write-4-LE-`lseek(0)`/read-4 protocol exactly as `libsmu`; `FdGuard` RAII + `classify_io_error` (NotFound → `DriverMissing`, EACCES → `InsufficientPrivilege`, short read/write or other → `Parse`/`Io`). **Never raw MMIO, never the `ryzen_smu` Rust crate** (decision D1 of P2-03 stands).

**D2 — SMN data flows into the frozen snapshot via a no-op overlay, never a signature change.** `pub fn apply_smn(snap: &mut AmdPmSnapshot)` (in `amd_smn.rs`) is the only new consumer-facing function. It (1) reads the register set of §1, (2) decodes each field via pure bit-field extractors (the `intel_readout::decode_*` pattern — fixture-testable without I/O), (3) writes **only** the zeroed fields (`gdm`, `pdm`, the 27 `timings`, the 8 `cad_bus` codes) — PM-table clocks/voltages are never touched. Error containment: a failed register read leaves **that register's** fields zeroed (→ honest `Na`/`Disabled` under the existing P2-05 gates); a missing `smn` attr (`DriverMissing`) is a **no-op, not an error** (older module builds, char-dev fallback path — the status quo); `apply_smn` returns `()` and can never fail the `amd_branch`. Consequence: `SmuContext`, `AmdPmSnapshot`, `parse()`, `map_amd()`, and the wire shape are all unchanged.

**D3 — The register table is the verified `monitor_cpu` map (§1); CAD + PDM are confirm-or-Na.** The 27 timings + GDM (+ documented-not-stored CR/tCKE) come from the table above. **CAD drive/termination codes and PDM are decoded only if their bitfields are confirmed** in P6-02's verification step (installed driver source + AMD-published UMC maps); if unconfirmed they stay zero → honest `Disabled`/`Na` and the cross-check's CAD gate stays deferred with that documented reason. No field is ever displayed with an unverified mapping (no-panic + no-garbage discipline). Known limitation (documented in the module): the driver's `smn_result` is a single shared global — the daemon's 2 s collection may race a concurrently-running `monitor_cpu`; a stale/corrupt reading degrades per-field via the sanity gates, and the cross-check script samples the two tools **sequentially, never concurrently**.

**D4 — The ground-truth cross-check is a `scripts/` helper, not a client subcommand.** `scripts/amd-ground-truth.sh` (bash, `set -euo pipefail`, operator-run — the `install-ryzen-smu-dkms.sh` precedent). A fourth client subcommand was considered and **rejected**: the P3-21 CLI contract (3 subcommands, flags, exit codes 0/1/2) is frozen and the TUI/GUI share that surface. The script: (a) precondition-checks root, `monitor_cpu`, a reachable daemon (default socket, `--socket` overridable), and the driver attrs; (b) matched conditions = idle (short settle), `monitor_cpu` PM frame first, then `ramsleuth-client -- dump` (root daemon), then one-shot `monitor_cpu -m` — **sequentially**; (c) compares: PM clocks ±1 MHz, VDDCR_SOC ±10 mV, and (when the dump's timings are populated) each timing ±1 tick vs `monitor_cpu -m`; (d) CAD: populated values → informational record, still-Na → "deferred (SMN CAD bitfields unconfirmed)" note, non-fatal; (e) records the 1792-vs-1800 investigation context: the `0x50200[6:0]` set-point (via one raw `smn` write/read in bash: `printf '\x00\x20\x50\x00' > …/smn && cat …/smn`) + `codename`/`drv_version`/`version`; (f) exits 0 = in-tolerance or documented deferrals, 1 = out of tolerance, 2 = preconditions unmet. Zero `.rs`/manifest changes.

**D5 — Small-tier inner-loop iterations live entirely in `worker.rs`; `run_pinned`'s signature and the wire shape are unchanged.** Each worker, after the single barrier crossing, re-runs its kernel `iters` times over its slice, where `iters = small_tier_iters(total)`: `total < 4 MiB → clamp(32 MiB / total, 1..=65536)`, else `1` (L1 32 KiB → 1024; L2 1 MiB → 32; L3 ≥ 4 MiB and DRAM → 1 — large tiers byte-identical to today). `WorkerResult.total_bytes` and `checksum` scale by `iters` (wrapping): per-pass checksum semantics are the frozen convention unchanged; the cross-worker partition invariant still holds per pass, and the aggregate over `iters` passes = `iters ×` the whole-buffer single-pass value (word-sum is order-independent; write stays `checksum == total_bytes`). The `run_pinned` root re-export and `bench_bandwidth`/`run_streamed` are untouched (GB/s = `total_bytes / best_elapsed` stays consistent — numerator and denominator both scale). The in-file `debug_assert` and the affected in-file tests are updated in the same file.

**D6 — The Intel chunk is validation-first, reconcile-if-divergent.** P6-06 runs the live MCHBAR decode on the i5-6600 (root daemon or `sudo cargo run -p ramsleuth-telemetry --release`), records the per-channel raw registers + decoded values, and compares against the installed-DIMM known-good (JEDEC min data rate from the live SPD + BIOS config). Verified → the module doc's "documented model" status flips to "live-verified on Skylake" + fixtures aligned to live values (comments/fixtures only). Divergent → the offset/bitfield `const`s + decoders in `intel_readout.rs` are corrected (those are `pub` items of that file — this plan is the plan-edit; the **wire** `IntelChannel`/`IntelReadout` shapes never change). Intel voltages/CAD = `Na(NotApplicable)` confirmed live (out-of-scope by design); AMD-host regression (gate short-circuit, zero `/dev/mem` touch) re-verified.

**D7 — The MSRV and push chunks are DOCS-ONLY decision/runbook records.** P6-07 authors `Docs/MSRV-DECISION.md` (the §6 fork of PLAN-PHASE5 restated with the full fact matrix: the 441-package lockfile pins ≤ 1.75, the ratatui/crossterm + egui/eframe pin stories, the `#[clippy::msrv = "1.89"]` annotations on `kernel_512.rs` vs the 1.75-green CI leg proving the 512 bodies compile on both legs today, and the consequences of each option — bump ⇒ `rust-version` edit + CI matrix `["1.89","stable"]` + pin re-verification; keep ⇒ status quo + the annotation stays a conservative hint). P6-08 authors `Docs/PUSH-RUNBOOK.md` (strict ff push, the 17-branch prune list, optional tag, pre-push checklist, rollback notes). Neither touches code, manifests, or the remote; both are parked for operator sign-off and never stall the pipeline.

## 4. Chunks (8 single-file micro-chunks)

### Chunk 1 — P6-01 · `scripts/amd-ground-truth.sh` (NEW) · [ISOLATED] · branch `branch/chunk-p6-01-groundtruth`
**Gate: `operator-sudo` + live 5950X host — for the live run; authoring/review is unblocked (bash gates only).**
**Scope (~110–140 lines bash; the P5-04 script precedent — budget exceeded only by comments/usage text):**
1. Preconditions (exit 2 on any miss, with an actionable message): root (`EUID == 0`), `/usr/bin/monitor_cpu` present, a reachable daemon (default `/run/ramsleuth/ramsleuth.sock`, `--socket <path>` overridable), `/sys/kernel/ryzen_smu_drv/{pm_table,smn}` present.
2. Record driver context: `codename` / `drv_version` / `version` attrs.
3. Matched-condition capture (sequential, short idle settle): (a) one `monitor_cpu` PM-table frame (`timeout 2`), (b) `ramsleuth-client -- dump`, (c) one-shot `monitor_cpu -m` (SMN timings).
4. Compare: PM clocks (MCLK/UCLK/FCLK) ±1 MHz; VDDCR_SOC ±10 mV; when the dump's AMD timings are populated, each of the 27 vs `monitor_cpu -m` ±1 tick (the reference's field list, §1); CAD: populated → informational, still-`Na` → deferred note (non-fatal, per D3/D4).
5. 1792-vs-1800 delta record: the `0x50200` set-point read in bash (write LE address, read 4-byte result, hex dump) + both MCLK sources + the PM f32 value — so the operator can classify the delta (set-point vs measured; scaling/rounding is ruled out if the two agree at 1800/1792 respectively).
6. Exit 0 = all active gates in-tolerance (deferrals noted), 1 = any active gate out of tolerance, 2 = preconditions.
**Acceptance criteria:** `bash -n` clean; shellcheck zero findings (info-only tolerance matches P5-04/13 precedent); zero `.rs`/`Cargo.*` changes; the live run (operator, post-P6-03 for the full gate — the script runs the PM gate immediately after this merge and the timings/CAD gate after P6-03) records: clocks ±1 MHz, voltages ±10 mV, and the 1792-vs-1800 delta root-caused (the set-point register vs the measured PM f32) with its evidence in the script output.
**Quality gates:** `bash -n`, shellcheck; reviewer traces the comparison thresholds against handover §5.1; no-panic: every external command failure degrades to exit 2 with a message — never a partial/ambiguous verdict.

### Chunk 2 — P6-02 · `crates/ramsleuth-telemetry/src/amd_smn.rs` (NEW) + ONE wiring line in `src/lib.rs` (`pub mod amd_smn;`) · [ISOLATED — interface-freezing for P6-03: its pub surface is frozen at merge] · branch `branch/chunk-p6-02-smn`
**Gate: none (unit-testable without hardware; the live read degrades gracefully).**
**Scope (the P5-15-style single-file exception — the module payload IS the const table + pure decoders; non-test code ≈ 180–220 lines, tests ≈ 120–160):**
1. **Accessor** (D1): `SYSFS_SMN_CANDIDATES` (`ryzen_smu_drv` first, legacy second); `pub fn read_smn_register(address: u32) -> TelemetryResult<u32>` — vendor gate (non-AMD → `UnsupportedHardware` before I/O) → Linux cfg split → `O_RDWR` open of the first existing candidate → `lseek(0)` → write 4-byte LE address (short write → `Parse`) → `lseek(0)` → read 4 bytes (short/empty → `Parse`) → `u32::from_le_bytes`; error classification via the existing `classify_io_error` (NotFound → `DriverMissing { driver: "ryzen_smu" }`, EACCES → `InsufficientPrivilege`, other → `Io`); `FdGuard` closes once (the `amd_smu` pattern, local copy — that one is private).
2. **Table + pure decoders** (D3): the 13 registers of §1 as `const` addresses; one pure `pub(crate)` bit-field extractor per decoded field (the `intel_readout::decode_*` shape, e.g. `fn tcl(reg: u32) -> u16` = `reg & 0x3F`); the `0x50264` mirror/sentinel rule; `pub fn decode_smn(regs: &[(u32, Option<u32>)]) -> SmnFields` — a pure core mapping the optional raw words onto a struct carrying the 27 timings + GDM (+ PDM/CAD only if confirmed, else the field is absent → zero), every field saturating-bounded (`u16`), `0` for any `None`/out-of-band word (no panic, no garbage).
3. **Overlay:** `pub fn apply_smn(snap: &mut AmdPmSnapshot) -> ()` (D2) — reads the register set via `read_smn_register`, decodes via `decode_smn`, writes only `gdm` / `pdm` / `timings` / `cad_bus`; any per-register `Err` leaves that register's fields zeroed; a `DriverMissing` no-ops the whole overlay; PM-table fields never touched.
4. **Verification step (documented in the module):** confirm-or-Na for CAD + PDM per D3 (installed driver source at `/opt/ryzen-smu-src` + AMD-published UMC maps); the unconfirmed fields stay zero with the reason documented.
**Acceptance criteria:** every `TableVersionId`-style pin test for the candidate list (verified-kobject-first); the LE write/read protocol pinned with synthetic 4-byte payloads (short read → `Parse`, never panic); `decode_smn` pinned against the `monitor_cpu`-equivalent synthetic words (e.g. `0x50200 = 0x00001539` → GDM off, set-point 1900 MHz, per §1's README example) and the README's live value; `apply_smn` on a zeroed snapshot with a synthetic register feed populates exactly the SMN fields and leaves clocks/voltages untouched; `DriverMissing`/`InsufficientPrivilege`/non-AMD all no-op or structurally `Err` without panic; the graceful host test (`read_smn_register(0x50200)` → `Ok` or a listed `Err`, never a panic); clippy `-D warnings`; 337/337 baseline + new tests green (debug+release).
**Quality gates:** in-file unit tests; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` debug+release; frozen-interface audit (only `amd_smn.rs` + the one `lib.rs` wiring line diff); no-panic audit (no `unwrap`/`expect` on any hardware-derived value; every `unsafe` — none expected, the nix wrappers are safe — if any is introduced it carries `// SAFETY:`).

### Chunk 3 — P6-03 · `crates/ramsleuth-telemetry/src/facade.rs` · [COUPLED-TO: P6-02] · branch `branch/chunk-p6-03-smn-facade`
**Gate: none.**
**Scope (~30–50 lines):** `amd_branch` becomes `let ctx = amd_smu::acquire()?; let mut snap = amd_pm::parse(&ctx)?; amd_smn::apply_smn(&mut snap); Ok(amd_readout::map_amd(&snap))` (D2 — the overlay is infallible: the branch's error semantics are unchanged; only the PM failure still fails the branch). Module-doc branch-topology line updated (`acquire → parse → apply_smn (overlay) → map_amd`); the two structural tests (`collect_on_this_host_is_structural_and_panic_free`, the bincode round-trips) re-run unchanged; add one test asserting `apply_smn` on a host without the `smn` attr leaves the snapshot's SMN fields zero while PM clocks/voltages survive (the status quo is preserved on older module builds).
**Acceptance criteria:** workspace tests green (debug+release, count 337 + new ≥); clippy clean; the AMD branch's `Na`/`Value` semantics for every `TelemetryError` variant unchanged (the `reason_from` mapping untouched); on the live 5950X (operator re-run): `ramsleuth-client -- dump` shows populated DRAM timings (+ GDM) or an honest `Na` with reason — **never** a daemon crash.
**Quality gates:** in-file tests; clippy; the live operator re-run is the ground truth (AHEAD note carries it).

### Chunk 4 — P6-04 · `crates/ramsleuth-telemetry/src/spd_decode.rs` · [ISOLATED] · branch `branch/chunk-p6-04-spdcodes`
**Gate: none (the live host re-run confirms; unit fixtures are the gate).**
**Scope (~40–70 lines, D-reconcile the two observed codes — handover §5.3b):**
1. **Maker `0xC1`:** reconcile against JEP106-0001 **and** the live module's part-number string (the part identifies the vendor line — the dump already prints it). If `0xC1` is an assigned JEP106 code → add `(0xC1, "<name>")` to the `JEP106` const table + a fixture test; if it is genuinely unassigned → keep the raw-hex rendering (a `Value`, today's behavior) and document the spec gap + the part-number identification in the module doc (no table entry invented out of thin air).
2. **Density `0x0D`:** reconcile against JESD79-4 byte 19 (codes `0x10..=0x17` → 1..128 Gb — `0x0D` is outside the published set) using the live module's capacity implied by its part number. If the capacity is consistent with a documented/legacy encoding → extend `decode_density`'s DDR4 branch with `0x0D → <Gb>` (source cited in the doc comment); otherwise keep `Na(ParseError)` with the detail improved to name the code + the suspected vendor encoding. Either outcome is an honest one (no-panic preserved: unknown codes still `Na`).
3. **Fixtures:** add the live-host configuration as a synthetic DDR4 image (density `0x0D` + maker bytes producing `0xC1`, rank-1, 3200 MT/s) to the test module; the prefix-sweep + bincode round-trip tests re-run unchanged (shapes frozen — `SpdModule`/`SpdProfile` untouched).
**Acceptance criteria:** `JEP106` table and `decode_density` updated per the reconciliation (or documented as spec-gap with improved detail); new fixture decodes to the recorded outcome; on the live 5950X the dump shows the module maker (name or documented raw-hex) + density (Mbit or documented `Na`) — matching the module's part number; 337/337 + new tests green; clippy clean.
**Quality gates:** in-file unit tests; clippy; frozen-interface audit (one file diff only); the live re-run confirms (AHEAD note).

### Chunk 5 — P6-05 · `crates/ramsleuth-bench/src/worker.rs` · [ISOLATED] · branch `branch/chunk-p6-05-smalltier`
**Gate: none (pure CPU; host-independent tests).**
**Scope (~60–90 lines, D5):**
1. `fn small_tier_iters(total: usize) -> u32` — pure: `total < 4 MiB → (32 MiB / total).min(65536).max(1)`, else `1` (u32-safe: `total ≥ 64` → quotient ≤ 524288; the 65536 cap keeps tiny test buffers bounded). Pinned test: 32 KiB → 1024, 1 MiB → 32, 64 B → 65536 (cap), 4 MiB → 1, 256 MiB → 1.
2. `run_pinned`: after the contract checks + the empty early-return, compute `iters` once; each worker, after the barrier, loops its `run_kernel` call `iters` times accumulating `bytes` / `checksum` (wrapping); the aggregation sums as today; `debug_assert_eq!(total_bytes, (total as u64).wrapping_mul(iters as u64))` (replaces the single-pass assert); `WorkerResult` doc lines updated (`total_bytes` = bytes moved across all inner iterations; `checksum` = wrapping sum of the per-pass checksums = `iters ×` the single-pass value; write keeps `checksum == total_bytes`).
3. **Frozen conventions preserved (audit points):** the kernel functions are untouched (per-pass word-sum / byte-counter semantics unchanged); the per-pass partition invariant (no overlap, no gap) is unchanged; `run_pinned`'s signature + the root re-export are unchanged (no wire impact — `WorkerResult`'s bincode shape is untouched); `bench_bandwidth`/`run_streamed`/`bench_job` untouched (GB/s = `total_bytes / best_elapsed` stays consistent); large tiers (`total ≥ 4 MiB`) run `iters = 1` → **byte-identical behavior to today**.
4. Update the affected in-file tests (`run_pinned_covers_buffer_once_per_op`, `read_aggregate_checksum_matches_whole_buffer`, `tail_buffer_is_covered_exactly_once`, `empty_topology_falls_back…`, `preflight_pin_restores…` — all use sub-4 MiB buffers and must assert the `iters ×` scaling) + add the `small_tier_iters` table test and a large-tier (`8 MiB` → `iters = 1`) invariance test.
**Acceptance criteria:** all bench + workspace tests green (debug+release); clippy clean; on the 5950X the L1/L2 grid cells become data-dominated (repeated `ramsleuth-bench` runs: L1/L2 GB/s stable within run-to-run noise, no longer pinned to the thread-spawn floor; L3/DRAM cells unchanged within noise); the checksum invariants hold (`read_aggregate_checksum_matches_whole_buffer` at the scaled value; write `checksum == total_bytes`).
**Quality gates:** in-file unit tests; clippy; `cargo test --workspace` debug+release; frozen-interface audit (one file diff; `WorkerResult`/`WorkerError`/`BenchOp` shapes + `run_pinned` signature untouched).

### Chunk 6 — P6-06 · `crates/ramsleuth-telemetry/src/intel_readout.rs` · [ISOLATED] · branch `branch/chunk-p6-06-intel`
**Gate: `hardware` — HARDWARE-GATED: BLOCKED on the operator providing the i5-6600 machine + sudo. Parked; the pipeline runs the unblocked chunks first and never waits on this one.**
**Scope (0–60 lines once the gate lifts, D6):**
1. Live run on the i5-6600 (root): `sudo cargo run -p ramsleuth-telemetry --release` (or root daemon + `ramsleuth-client -- dump`); record the raw per-channel IMC words (`IMC_FREQ_RATIO` + the four `MCS_CH*` command registers) and the decoded values.
2. Verify against known-good: the installed DIMM's JEDEC minimum data rate (the live SPD prints it) + BIOS config → expected tCL/tRCD/tRP/tRAS (ticks), command rate (1N/2N), gear (1×/2×/4×), MCLK; confirm `channel_count(Skylake) = 2` channels decode and the third/fourth never appear.
3. Outcome A (verified): flip the module-doc status line from "documented model … to be reconciled against real Intel silicon" to "live-verified on Skylake (i5-6600, <date>)" + align the fixture register values to the live words; no const changes.
4. Outcome B (divergence): correct the offset/bitfield `const`s + decoders in this file (this plan is the plan-edit per protocol; the **wire** `IntelChannel`/`IntelReadout` shapes never change; re-run the AMD-host gate tests — zero `/dev/mem` touch on non-Intel).
5. Confirm out-of-scope sections render `Na(NotApplicable)` live: Intel voltages + CAD bus (every field).
**Acceptance criteria:** per-channel tCL/tRCD/tRP/tRAS + command-rate/gear decode exactly matches known-good for the i5-6600's DIMMs; 2 channels; Intel voltages/CAD all `Na(NotApplicable)`; no panic in any privilege state; AMD-host regression: `Na(UnsupportedHardware)` with zero `/dev/mem` access (the existing gate tests + a live 5950X run); clippy + workspace tests green.
**Quality gates:** live hardware run (operator sudo); in-file fixture updates; clippy; frozen-interface audit (wire shapes untouched).

### Chunk 7 — P6-07 · `Docs/MSRV-DECISION.md` (NEW, DOCS-ONLY) · [ISOLATED] · branch `branch/chunk-p6-07-msrv`
**Gate: `operator-decision` — BLOCKED on operator sign-off; parked (authoring is unblocked, but the decision event waits). Never stalls the pipeline.**
**Scope (~80–120 lines of documentation; zero code/manifest changes):**
- **Fact matrix (current state, 2026-09-14):** workspace `rust-version = "1.75"`; 441 lockfile packages all MSRV ≤ 1.75 (P3 QA); the ratatui 0.29 / crossterm 0.28 TUI stack held ≤ 1.75 by two lockfile pins; the egui/eframe 0.27.2 GUI line = the newest 1.75-compatible; `kernel_512.rs` carries `#[clippy::msrv = "1.89"]` on its three 512-bit bodies (the conservative annotation) while the CI `1.75` leg compiles + tests them green on a clean runner — i.e. the 512 bodies build on both legs today and are runtime-gated (never exercised on the Zen 3 host, which lacks AVX-512F).
- **Option A — bump to 1.89:** consequences = workspace `rust-version` edit, CI matrix `["1.89", "stable"]` (the 1.75 leg is dropped — no 1.75 leg can build a 1.89-MSRV workspace), lockfile pins re-verified (no regeneration expected — the pins are ≤ 1.75 ⊂ ≤ 1.89), the `#[clippy::msrv]` annotations become redundant-but-harmless, contributor floor rises. Pros: declaration matches the annotations + headroom for newer std. Cons: real floor raise for a no-benefit-yet feature (the 512 bodies already build on 1.75).
- **Option B — keep 1.75 (the Phase 5 §6 recommendation, restated):** status quo; the CI 1.75 leg remains the continuous MSRV proof; the annotation stays a conservative hint; pins untouched.
- **Decision:** __________ (operator, date) — the follow-up event (if A: a standalone micro-chunk — workspace `Cargo.toml` + CI edit + pin re-verification — scheduled after this cycle's compaction; if B: nothing).
**Acceptance criteria:** the doc renders the matrix factually (every claim traceable to `Cargo.toml` / `Cargo.lock` / `ci.yml` / `kernel_512.rs`); zero non-doc diffs; operator sign-off recorded (the gate lifts only on that).
**Quality gates:** reviewer cross-checks each fact against the cited files; no-panic n/a (docs).

### Chunk 8 — P6-08 · `Docs/PUSH-RUNBOOK.md` (NEW, DOCS-ONLY) · [ISOLATED] · branch `branch/chunk-p6-08-push`
**Gate: `operator-decision` — BLOCKED on operator go-ahead; parked. The pipeline authors the runbook but performs NO remote operation (handover §8: local-only until explicit go-ahead; never force-push; no remote branch deleted).**
**Scope (~60–90 lines of documentation):**
- **Pre-push checklist:** CI green on the local tip; workspace tests green (337 + P6 additions, debug+release); clippy `-D warnings` clean; tree clean; `git fetch` + confirm `origin/v2-development` is a strict ancestor of local (the push MUST be a fast-forward — if not, stop and report, never force).
- **Step 1 — fast-forward:** `git push origin v2-development` (strict ff to the local tip, ≥ the current head; NEVER force-push).
- **Step 2 — prune the 17 fully-merged remote branches** (each verified merged into `v2-development`; the `--no-ff` history stays on the branch): `branch/chunk-p5-01`, `-02`, `-02-fix`, `-03`, `-04`, `-04-fix`, `-05`, `-06`, `-07`, `-08-sysfs`, `-09-script`, `-10-readme`, `-11-dkms`, `-12-dkmsconf`, `-13-dkmsver`, `-14-dkmsinstall`, `-15-pmtable` (via `git push origin --delete <branch>` one at a time, or `git fetch --prune` after confirming every one is merged).
- **Step 3 — optional tag:** `v2.0.0` on the pushed tip — only with explicit sign-off at push time (decides the AUR `git describe` pkgver; the PKGBUILD no-tag fallback keeps the package buildable either way).
- **Step 4 — post-push verification:** `git fetch --prune`; the 17 branches gone; `origin/v2-development` == local; CI green on origin; `origin/master` untouched (divergent legacy, read-only reference — never merged into, never forced onto).
- **Rollback notes:** a pushed ff is permanent (no un-push); a tag can be deleted (`git push origin :refs/tags/v2.0.0`); the only safe "undo" for the prune is re-pushing the branches — hence the pre-push checklist is the gate.
**Acceptance criteria:** the runbook matches handover §8 verbatim in policy (ff-only, no force, no pre-go-ahead remote mutation); zero non-doc diffs; operator go-ahead recorded (the event executes the runbook, not the pipeline).
**Quality gates:** reviewer cross-checks the 17-branch list against `git branch -r`; no-panic n/a (docs).

## 5. Phase 6 exit criteria

1. **AMD ground truth (O1):** matched-condition cross-check (P6-01 script, operator run after P6-03): PM clocks within ±1 MHz, VDDCR_SOC within ±10 mV, and (post-SMN) the 27 decoded timings within ±1 tick of `monitor_cpu -m` — or any deviation documented with its evidence; the 1792-vs-1800 delta root-caused (set-point `0x50200[6:0]` vs measured PM f32) and recorded.
2. **SMN path (O3a):** the live 5950X dump shows populated DRAM timings (+ GDM) from the `smn` attr — or an honest `Na` with reason on any failure mode (attr absent, short read, privilege) — never a daemon crash; CAD/PDM in their documented confirm-or-Na state.
3. **SPD (O3b):** the live module's maker + density are reconciled (named / documented raw-hex / documented `Na`) and consistent with the part number.
4. **L1/L2 (O4):** the small-tier cells are data-dominated (stable, no thread-spawn floor); large tiers byte-identical in behavior; checksum conventions intact.
5. **Intel (O2/O3c) — gated:** the i5-6600 live decode validates (or reconciles) the skeleton IMC table; voltages/CAD = `Na(NotApplicable)`; AMD-host zero-`/dev/mem` regression holds.
6. **MSRV + push (O5/O6) — gated:** the decision recorded with sign-off; the push executed per the runbook (ff-only, 17 pruned, optional tag) — a deliberate operator event after this cycle's QA.
7. **QA audit:** workspace tests green debug+release (337 + P6 additions); `cargo clippy --workspace --all-targets -- -D warnings` clean; frozen-interface diff audit passes (exactly: `amd_smn.rs` + `lib.rs` wiring line, `facade.rs`, `spd_decode.rs`, `worker.rs`, `intel_readout.rs` [gated], 2 docs, 1 script — nothing else, no `Cargo.toml`/`Cargo.lock`/`systemd/` changes); zero panics / segfaults in every privilege × CPU state run; then standard compaction (`MASTER_LOG.md`, prune merged P6 branches, reset `DEV_LOG.md`, handover refresh).

## 6. Execution order (gated pipeline)

**Unblocked — run sequentially, each: branch off `v2-development` → implement → quality gates → lease-signed review → `--no-ff` merge → prune branch:**
1. **P6-01** (cross-check script; bash gates) — *then operator live run #1 (PM clocks/voltages gate; timings/CAD deferred notes)*
2. **P6-02** (new `amd_smn` module — interface-freezing for P6-03)
3. **P6-03** (facade overlay wiring — *then operator live run #2: full cross-check incl. timings vs `monitor_cpu -m` + CAD gate state + the 1792-vs-1800 record*)
4. **P6-04** (SPD `0x0D` / `0xC1` reconciliation — live re-run confirms)
5. **P6-05** (small-tier inner iterations — L1/L2 cells become data-dominated)

**Parked — never stall the pipeline; picked up when their gate lifts:**
- **P6-06** — `hardware` gate: the i5-6600 machine + operator sudo (live MCHBAR validation; reconcile-if-divergent).
- **P6-07** — `operator-decision` gate: MSRV 1.75 vs 1.89 sign-off (docs-only decision record; any resulting code change is a follow-up micro-chunk after compaction).
- **P6-08** — `operator-decision` gate: push go-ahead (docs-only runbook; the operator executes the ff push + 17-branch prune + optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).

**Cycle 5 exit →** standard compaction per the operating protocol (handover §12.6): `MASTER_LOG.md` merge history, prune the merged `branch/chunk-p6-*`, reset `DEV_LOG.md`, refresh `Docs/HANDOVER.md` + `FULLSCOPEvsCOMPLETED.md` for the next cycle.
