# RamSleuth v2 — Master Log

Durable per-cycle compaction of `DEV_LOG.md`. Newest cycle first.

## Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) — 2026-09-16 — COMPLETE

### What was delivered
Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) is COMPLETE: **all 8 chunks merged into `v2-development` (C9-01…C9-08)** from baseline `6cc9840` (the Cycle 8 compaction) to tip `c2f40a5` — the four operator tasks:

**Operator task 1 — RAM topology header:**
- The header RAM line renders the topology grouped by (size, rank word) — `2x16 GiB Single-Rank` (the Na/0 rank omits the word) — plus a slot note (`2 of 4 slots SPD-visible`) when MemTotal exceeds the SPD sum (D-1, C9-01).

**Operator task 2 — Graphs window telemetry lifecycle + poll control:**
- Opening the Graphs window forces auto-refresh ON (saving the original) and closing reverts to the saved original; one transition detector covers both close paths (the button toggle + the WM close_requested) (D-2, C9-02).
- The in-window `Poll` combo (500/1000/2000/5000/10000 ms presets; a non-preset stored value shows its exact ms figure) writes the shared `settings.poll_interval_ms`, which the poller re-reads live (D-2, C9-03 co-land).

**Operator task 3 — CPU temperature graphing:**
- `read_cpu_temp_c` is now an ordered scan: the hwmon source first (each `hwmon*` `name` matched case-insensitive + trimmed, `k10temp` > `zenpower`, reading `temp1_input` ÷1000 — never a fixed `hwmonN`), then the pre-existing `cpu_thermal` zone scan as fallback; first finite wins, every failure class → NaN (D-3, C9-04). On the live host (hwmon4 = `k10temp`) the CPU TEMP row now plots instead of N/A.

**Operator task 4 — main window layout uniformity:**
- The left-column frames (bench C9-06, telemetry C9-08) gain `set_min_width(available_width)` so their borders fill the column (D-4a).
- The right column is vertically split — bench at natural height, status at the remaining-height slice, inside the retained ScrollArea overflow fallback (D-4b, C9-05) — and the status frame fills the full width AND the remaining height (C9-07).

**Chunks (all `--no-ff` merged):**
- **C9-01** RAM header — `dimm_summary` groups by (size, rank word) → `2x16 GiB Single-Rank` (Na/0 omits the word); `rank_word` + `slot_note` render `2 of 4 slots SPD-visible` when MemTotal > the SPD sum; main.rs only, no wire/deps diff (66c7246).
- **C9-02** Graphs lifecycle — force-on on the `graphs_open` rising edge (saving the original), restore of the saved original on the falling edge; one detector covers both close paths; main.rs only (1e20bfd).
- **C9-03** poll control co-land — the in-Graphs `Poll` combo writing `settings.poll_interval_ms` (&mut u64 co-land; the child frame flips read→write lock, the D6 settings-write precedent); graph.rs + main.rs (985248f).
- **C9-04** CPU temp hwmon — `read_cpu_temp_c` ordered scan: `k10temp` > `zenpower` name-match first (`temp1_input` ÷1000 via the shared `parse_millidegrees`), the `cpu_thermal` zone scan retained verbatim as fallback; graph.rs only (02af5f2).
- **C9-05** right-column split — the right column = two stacked slices inside the retained ScrollArea overflow fallback (bench at natural height, status at the remaining `row_h − bench_h − 8 − item-gap`); main.rs only (502c7b7).
- **C9-06** bench width — the bench frame gains `set_min_width(available_width)` (D-4a left-column width-fill); bench_zone.rs only (b88784f).
- **C9-07** status fill — the status frame gains `set_min_width(available_width)` + `set_min_height(available_height)` (D-4a width + D-4b vertical fill of the C9-05 remaining-height slice); status_zone.rs only (c2f40a5).
- **C9-08** telemetry width — the telemetry frame gains `set_min_width(available_width)` (D-4a left-column width-fill); telemetry_zone.rs only (3899f21).

### Key plan decisions
- **D-1:** RAM topology is a no-wire-change — the rank is already on the wire via `SpdModule.rank` (C8-03); the rank word + slot note are joined GUI-locally in the header.
- **D-2:** Graphs force-on open / revert close (one transition detector covers both close paths) + the in-window `Poll` combo writing `settings.poll_interval_ms` (persists to Settings, not a Graphs-local override).
- **D-3:** CPU temp via the hwmon source by name-match — walk `/sys/class/hwmon/`, `k10temp` > `zenpower` (never a fixed `hwmonN`), read `temp1_input` ÷1000; the `cpu_thermal` zone scan retained as fallback.
- **D-4:** layout uniformity — `set_min_width` / `set_min_height` fill on the zone frames (left-column width fill + the status full width & remaining height) + the right-column vertical split (bench natural height, status remaining slice).

### Quality
- **541/541 tests green (debug AND release, whole workspace)** — up from 527 at the Cycle 8 close; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps**; **zero wire changes** (all 8 chunks are GUI-local — no protocol / `messages.rs` shape delta; per-merge frozen-shape audits clean).
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 8 chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`c2f40a5`** (Cycle 9 range `6cc9840..c2f40a5`; `origin/v2-development` still `b908f7b`) — **unpushed, operator-gated** (this compaction performs no push). The 8 remote `branch/chunk-c9-*` chunk branches (pushed during the cycle; the local branches + worktrees already pruned) are the handover prune target (all fully merged into `v2-development`; tip `c2f40a5` untouched). On the operator's go-ahead: **fast-forward to `c2f40a5` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE9-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the RAM topology header (`2x16 GiB Single-Rank` + slot note), the Graphs force-on/revert lifecycle + in-window Poll combo, the CPU TEMP row now plotting k10temp, the full-width/full-height zone frames — closes the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (ff to `c2f40a5`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. The remaining Cycle 8 open items carry as listed in the Cycle 8 section (the Cycle 8 live GUI run; Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; CAD/RTT/drive SMN bitfields; AIDA64 parity gate).

Cycle 9 close-out (2026-09-16): this compaction recorded the Cycle 9 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `c2f40a5`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 8 (v2.0.0 header truth) — 2026-09-16 — COMPLETE

### What was delivered
Cycle 8 (v2.0.0 header truth) is COMPLETE: **all 11 chunks merged into `v2-development` (C8-01…C8-11)** — the four operator tasks:

**Operator task 1 — AGESA relabel (honest provenance):**
- Wire freeze adds the additive `smu_version` field (C8-01); the header now renders a provenance fragment — "AGESA <v>" (AGESA present, suppresses the SMU value) / "SMU <v>" (SMU only) / "AGESA N/A" — never a hybrid or a fabricated string (D-1, C8-11).

**Operator task 2 — RAM capacity:**
- SPD type classification re-anchored to byte 0x02 (0x0C → DDR4 512 B / DDR5 1024 B, 0x0B → DDR3); the live-host shape now classifies DDR4 and the part decodes from the 0x149 primary (C8-02).
- Total-device decode per JESD79-4/5 (devices = total) → per-slot 16 GiB (D-2, C8-03).
- Total flipped to MemTotal-preferred (OS ground truth); the SPD sum demoted to a pure `sum_dimm_sizes` non-meminfo fallback (D-3, C8-04).

**Operator task 3 — N/A formatting + styling:**
- `NA_GRAY` muted gray (0x8A8A94) const added to the palette (D-5, C8-05).
- Zone 1 (C8-06), Zone 3 SPD cards (C8-07), Zone 2 bench cells (C8-08), and the Graphs no-source note (C8-09) all render bare "N/A" in NA_GRAY (D-4/D-5); fault-red preserved — the daemon status + `!` error line stay CRIMSON.

**Operator task 4 — gear-metric removal:**
- The redundant AMD gear row removed (gear_mode is always NotApplicable) + the combined GDM/CR row split into separate GEAR_DOWN + CR rows; Intel keeps its gear row (D-6, C8-10).

**Chunks (all `--no-ff` merged):**
- **C8-01** wire freeze — `SystemPlatform += smu_version` (additive, shape-checked; agesa narrowed to BIOS-string-only); 27 test literals co-landed across 16 files (c6ef83d).
- **C8-02** byte-0x02 classification — the SPD type key re-anchored with the legacy byte-0x00 + image-length fallbacks preserved (8e4ceb7).
- **C8-03** total-device decode — devices = total per JESD79-4/5 (per-slot 16 GiB) (c83bc6f).
- **C8-04** MemTotal total — `total_capacity` prefers `/proc/meminfo`; the SPD sum demoted to `sum_dimm_sizes` (678430b).
- **C8-05** NA_GRAY — the muted gray 0x8A8A94 const + re-export (14084b4).
- **C8-06** Zone 1 N/A — six na_text arms + the no-telemetry placeholder render bare N/A; N/A cells recolored CRIMSON → NA_GRAY (7cdb4f8).
- **C8-07** Zone 3 N/A — the SPD-card fields + placeholders render bare N/A in NA_GRAY; the daemon/error lines stay CRIMSON (f63cf1a).
- **C8-08** Zone 2 N/A — bench unmeasured + NotStarted cells recolored CRIMSON → NA_GRAY (5ed3cff).
- **C8-09** graph N/A — the no-source note + the permanent VDDCR_CPU row label render bare "N/A" (558d04a).
- **C8-10** gear row + split — the AMD gear row removed; the combined GDM/CR row split into GEAR_DOWN + CR rows (0bbab2b).
- **C8-11** header relabel — the provenance fragment in the header line per D-1 (+2 tests pinning the live "SMU 56.78.0" shape) (51d660f).

### Key plan decisions
- **D-1:** honest AGESA/SMU relabel by provenance — AGESA present → "AGESA <v>" (suppresses SMU); SMU only → "SMU <v>"; neither → "AGESA N/A"; never a hybrid or a fabricated string.
- **D-2:** per-slot total-device decode — devices = total (JESD79-4/5); per-slot DIMM sizes stay SPD-derived.
- **D-3:** MemTotal-preferred total — OS `/proc/meminfo` ground truth; the SPD sum demoted to the non-meminfo fallback.
- **D-4:** bare "N/A" in the GUI — verbose parentheticals stripped; the reason stays on the wire.
- **D-5:** `NA_GRAY` (0x8A8A94) for N/A (unavailable, not critical) + fault-red preserved (the daemon status + error line stay CRIMSON).
- **D-6:** the redundant AMD gear row removed + the combined GDM/CR row split into GEAR_DOWN + CR rows (Intel keeps its gear row).

### Quality
- **527/527 tests green (debug AND release, whole workspace)** — up from 515 at the Cycle 7 close; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps** (empty `Cargo.toml`/`Cargo.lock` diff over `9ea4b3b..5730b33`).
- **Frozen-shape audit:** the ONLY wire change is the additive `SystemPlatform.smu_version: Section<String>` (appended after `agesa`; all pre-existing fields byte-identical; `SpdModule` field lines byte-identical — `devices` stays `Section<u8>`, a value-semantics fix per D-2; `ClockReadout` + `AmdPmSnapshot` unchanged; `messages.rs` changed only inside `mod tests`).
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 11 chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`5730b33`** — **unpushed, operator-gated** (this compaction performs no push). The 11 merged `branch/chunk-c8-*` chunk branches are the handover prune target (all fully merged into `v2-development`; the tip `5730b33` untouched). On the operator's go-ahead: **fast-forward to `5730b33` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE8-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the header SMU relabel, RAM 62.68 GiB (2×16 GiB), the gray N/As, the GEAR_DOWN/CR split, the Graphs note, the flapping-daemon no-crash — closes the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (ff to `5730b33`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. The remaining Cycle 7 open items carry as listed in the Cycle 7 section (Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; CAD/RTT/drive SMN bitfields; AIDA64 parity gate).

Cycle 8 close-out (2026-09-16): this compaction recorded the Cycle 8 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `5730b33`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 7 (v2.0.0 Polish: UI Refactor, Telemetry Lifecycle, Burn-In, Graphs Window) — 2026-09-16 — COMPLETE

### What was delivered
The v2.0.0 polish cycle is COMPLETE: **all 22 chunks merged into `v2-development` (C7-01…C7-22)** — the UI refactor, telemetry polling lifecycle, burn-in, and the native graphs window:

**Polling lifecycle:**
- Baseline-once fetch per socket regardless of refresh (D-8, C7-08).
- Auto-refresh default OFF (C7-09, incl. the main.rs test/doc ripple).

**Settings:**
- Unique combo widget ids fix the dropdown desync (C7-10).
- Socket field 240 pt min width (no truncation); capacity/clock units wired into the header (C7-11) + Zone 1 clock rows (C7-15).

**Platform:**
- AGESA validator group2 relaxed to 1–6 digits (D-6, C7-01) — the host's ryzen_smu version "56.78.0" now resolves.
- Vendor-conditional timing blocks omit the unsupported vendor's N/A block (C7-14).

**Panel 1:**
- 3-column × 2-row compact layout, no vertical scroll at 1400×900 (C7-12).
- GDM/CR explicit labels "GEAR_DOWN: Enabled/Disabled · CR: 1T/2T" (D-7, C7-13).

**Panel 2 + burn-in:**
- Flat "Status: Idle/Running…/Done" label replaces the button-like progress pill (C7-17).
- Burn-in mode: `run_cell_pass` extraction (C7-05); engine with duration deadline + per-iteration emit (C7-06); wire freeze StartBurnIn/BurnInProgress (D-1/D-2, C7-07, single commit); GUI path with 120 s stream timeout (C7-16); controls — minutes input 0=∞, Run Burn-In, live per-iteration cells (C7-18).

**SPD:**
- Per-generation module part decode — DDR4 0x149 (20 chars, 0x81 fallback) / DDR5 0x200 (32 chars) / DDR3-2 0x81 (C7-02).
- XMP3_BASE 0x200→0x300 (JESD79-5) part/profile coexistence (C7-03); GUI fixture ripple (C7-04).

**Graphs window:**
- Embedded TREND HISTORY strip removed, columns reclaim full height (C7-19).
- Graph state — 1800-sample ring, Na-guarded 5-series recording, thermal-zone temp scan, basic render (C7-20).
- Native eframe 0.27 multi-viewport spawn via `show_viewport_deferred` (D-3, C7-21; live Wayland second-window gate deferred to operator).
- Interactivity — hover crosshair with all-series tooltip, horizontal pan, 1/5/15/60-min window selector (C7-22).

### Key plan decisions
- **D-1:** new `StartBurnIn` arm (existing bench wire byte-frozen).
- **D-2:** bench-owned `BurnInTick`.
- **D-3:** eframe 0.27.2 native multi-viewport (spike-verified; the D-3b thread fallback unused).
- **D-4:** no-source series render label + "N/A (no source)" (VDDCR_CPU always; CPU temp when no AMD thermal zone).
- **D-5:** bandwidth = step series from the latest bench/burn-in Memory·Read (no continuous sampler).
- **D-6:** AGESA validator relaxation (group2 1–6 digits).
- **D-7:** GDM/CR explicit labels.
- **D-8:** baseline-once fetch per socket.

### Quality
- **515/515 tests green (debug AND release, whole workspace)** — up from 452 at the Cycle 6 close; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build.
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 22 chunks merged; the 5 operator live checks are the remaining manual gate (Wayland second window, burn-in live updates, AGESA in header, SPD part on live host, Panel 1 no-scroll).

### Push state (operator gate)
Local `v2-development` tip = **`a2577c0`** — **unpushed, operator-gated** (this compaction performs no push). The **22 merged `branch/chunk-c7-*` chunk branches were pruned (deleted) at compaction** (all fully merged into `v2-development`; the tip `a2577c0` untouched). On the operator's go-ahead: **fast-forward to `a2577c0` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Follow-ups (non-blocking, carried)
1. "q"-focus Quit guard (key-handling edge).
2. `SpdModule.die_type` label mapping.
3. Settings XDG persistence.
4. VDDCR_CPU + CPU temp have no source on this host (honest N/A by design).

### Open items carried forward
1. **Operator live-run sign-off** — the 5 deferred live checks (Wayland second window, burn-in live updates, AGESA in header, SPD part on live host, Panel 1 no-scroll) close the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (fast-forward to `a2577c0`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **Intel MCHBAR decode** — hardware-gated (the i5-6600 not yet attached; IMC offsets still SKELETON).
4. **MSRV 1.75 vs newer** — operator call (workspace deliberately kept at 1.75; the CI 1.75 leg is the continuous proof).
5. **CAD/RTT/drive SMN bitfields** — unconfirmed in the ryzen_smu driver source → honest N/A; pending driver-side confirmation.
6. **AIDA64 parity gate** — deferred to the DDR5-6000 AM5 host (this host is Zen 3 / DDR4).

Cycle 7 close-out (2026-09-16): this compaction recorded the Cycle 7 section in `MASTER_LOG.md` (the MASTER_LOG-only compaction — `DEV_LOG.md` and `Docs/HANDOVER.md` untouched; no commit, no push). The 22 merged `branch/chunk-c7-*` chunk branches were pruned at compaction (`v2-development` tip `a2577c0` untouched).

## Cycle 6 (Phase 7: GUI Workstream) — 2026-09-15 — COMPLETE

### What was delivered
The GUI workstream is COMPLETE: **all 29 chunks merged into `v2-development`** — the data-model wave C6-01…C6-19 (C6-08 retired, no branch/worktree) + the GUI wave C6-20…C6-30 — closing **all 8 GUI Gap items** against Grand Design §3.1/§3.2 (`FULLSCOPEvsCOMPLETED.md` §"GUI Gap").

**Data-model / wire (interface freeze):**
- New `SystemPlatform` (cpu_clock_mhz / motherboard / bios / agesa; DMI+ and `/proc`-sourced; N/A-safe) + `mem_total_gib`.
- `SystemMemoryTelemetry` += `platform` / `total_capacity` / `dimm_sizes` (per-DIMM = density × devices / 8192; total = Σ dimm_sizes, else `/proc/meminfo`).
- `SpdModule` += `die_maker` / `die_type` / `devices`.
- `CommandRate` enum + `ClockReadout.command_rate` (raw 0 → 1T, 1 → 2T, else `Na`; Intel honest `Na`).
- `AmdPmSnapshot.command_rate` raw slot; `SmnFields.command_rate` (0x50200 bit 10) with `apply_smn` now surfacing gdm + timings + command_rate.
- Cross-crate test-fixture ripple (C6-09…C6-19) closed the red window introduced by the wire-shape changes; hard workspace gate post-C6-19.

**GUI (egui/eframe, items 1–8):**
1. 3-line header (platform tag; CPU + motherboard/BIOS/AGESA; RAM total + per-DIMM + channel + sync-mode).
2. Zone 1 regrouped: 2 sub-columns × 3 section-pairs.
3. GDM / command-rate row.
4. Zone 2 per-cell live bench fill.
5. Zone 3 SPD cards (product line, DRAM die, rank label).
6. History: 300-sample `RingBuffer` + hand-rolled sparklines (no new dep).
7. Settings: `GuiSettings` + dynamic clamped poll interval + refresh gate.
8. Export parity: F2 640×420 validation-card PNG, F3 JSON + bench key, keyboard F2/F3/Q, dynamic socket.

### Key plan decisions
- **D-C10 (red window):** production-compile-clean per merge; test-fixture red window allowed during the wire freeze, closed by the C6-09…C6-19 ripple, hard workspace gate post-C6-19.
- **D-C11:** Intel `command_rate` honest `Na` (no hardware slot) — no speculative mapping.
- Refined circuit-breaker (recorded in `plans/PLAN.md`).

### Quality
- **452/452 tests green (debug AND release, whole workspace)** — up from 371 at the Cycle 5 close; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build.
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 8 GUI Gap items closed; the operator live run (`plans/CYCLE6-LIVE-CHECKLIST.md`) is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`33dd08b`** — **95 commits ahead of `origin/v2-development` @ `b908f7b`** (the 19 Cycle-5 commits + 76 C6 implementation/merge commits, range `51f4c4f..33dd08b`; Rust delta: 26 `.rs` files, +5850/−508). All **local-only, unpushed** (this compaction performs no push). No remote `branch/chunk-c6-*` branches exist; the **31 merged c6/p6 chunk branches + 29 c6/p6 worktrees were pruned (deleted) at compaction** (all fully merged into `v2-development` — the 23 c6 + 8 p6 branches, 21 c6 + 8 p6 worktrees; the `v2-development` tip `33dd08b` is untouched). On the operator's go-ahead: **fast-forward to `33dd08b` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Follow-ups (non-blocking, deferred)
1. "q"-focus Quit guard (key-handling edge).
2. `die_type` label mapping.
3. Settings XDG persistence.
4. Zones Units/Theme formatters.

### Open items carried to Cycle 7
1. **Operator live-run sign-off** — `plans/CYCLE6-LIVE-CHECKLIST.md` (untracked in the working tree): the PASS-WITH-MANUAL-LIVE-VERIFY verdict closes on this manual run.
2. **Push to GitHub** — operator go-ahead (ff to `33dd08b`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **Intel MCHBAR decode** — hardware-gated (the i5-6600 not yet attached; IMC offsets still SKELETON, P6-06 parked).
4. **MSRV 1.75 vs 1.89** — operator call (441 lockfile packages all MSRV ≤ 1.75; the CI 1.75 leg is the continuous proof).
5. **CAD/RTT/drive + PDM SMN bitfields** — unconfirmed in the ryzen_smu driver source → honest N/A; pending driver-side confirmation.
6. **AIDA64 parity gate** — still deferred to the DDR5-6000 AM5 host (this host is Zen 3 / DDR4).

Cycle 6 close-out (2026-09-15): this compaction recorded the Cycle 6 section in `MASTER_LOG.md` (with the reset `DEV_LOG.md`, the refreshed `Docs/HANDOVER.md`, and the live-run checklist `plans/CYCLE6-LIVE-CHECKLIST.md`; one local commit, no push). The 31 merged c6/p6 chunk branches + 29 c6/p6 worktrees were pruned at compaction (`v2-development` tip `33dd08b` untouched). Per the "newest cycle first" header convention, the section was relocated from the end to the top of the file by this compaction (reordering closed).

## Dev-Cycle Decisions & Hardware Context — 2026-09-12

Confirmed by director/user 2026-09-12; captured in `Docs/RamSleuth-v2.md`, `Docs/Grand Design & Architecture Specification.md`, and `Docs/HANDOVER.md`.

- **Push policy:** stay 100% local on `v2-development`; do NOT push to the GitHub upstream (`https://github.com/MadGoatHaz/RamSleuth`, divergent legacy `master`) until there is a confirmed, tested, working end-result app. No force-pushes without explicit sign-off.
- **AMD host / `ryzen_smu`:** the `ryzen_smu` kernel module is NOT installed on the primary dev host (AMD Ryzen 9 5950X, Zen 3, 16C/32T, 64 MiB L3, DDR4, AVX2; **CachyOS, kernel `7.2.3-1-cachyos-custom`**): `sudo modprobe ryzen_smu` → `FATAL: Module ryzen_smu not found in directory /lib/modules/7.2.3-1-cachyos-custom`; `/sys/kernel/ryzen_smu/` absent. For AMD live subtiming verification the module MUST be built + installed + loaded (headers for `7.2.3-1-cachyos-custom` → build out-of-tree → `depmod -a` → `modprobe` → verify `pm_table`); steps documented in `Docs/RamSleuth-v2.md`. The codebase degrades gracefully today: `N/A (DriverMissing)`, never panics.
- **Intel test machine:** LGA-1151 **Intel i5-6600 (Skylake, 6th-gen), dual-channel (2 DIMM channels)** — available for live Intel MCHBAR decode verification; matches `channel_count(Skylake) = 2`.
- **Cycle position:** Phase 1 (Native Benchmark Engine, 11 chunks) and Phase 2 (Live Memory Controller Telemetry, 11 chunks) are COMPLETE and QA-passed (159/159 tests, clippy clean). **The next development cycle starts at Phase 3** (privilege-separated daemon + Unix socket + clients).
- **CPUID note:** the frozen P2-01 map classifies the 5950X reference host as family `0x19` → `Amd(Zen3)`; desktop Zen 4/5 silicon also reports family `0x19` on some boards, so the AMD PM parse (P2-04) keys on the **SMU version** (7.11.x / 12.x / 13.x), not on `AmdZen` — generation ambiguity cannot break the PM layout.

## RamSleuth v2 — Cycle 5 (Phase 6: Live-Hardware Verification & Remaining Reconciliation) — 2026-09-15

### What was delivered
Live-hardware verification of the completed 7-crate app + the remaining model reconciliation — 8 chunks, all `--no-ff` merged into `v2-development` (the Phase 6 plan landed as `1ad1afe`; the 16 P6 implementation + merge commits follow it):
- **P6-01** `scripts/amd-ground-truth.sh` — the matched-condition cross-check of `monitor_cpu` vs `ramsleuth-client -- dump` (root daemon): preconditions (root / `monitor_cpu` / reachable daemon / driver attrs → exit 2 + actionable), sequential capture (PM frame → dump → one-shot `-m`), gates PM clocks ±1 MHz + VDDCR_SOC ±10 mV + the 27 timings ±1 tick (active only when the dump cell holds a value; N/A cells deferred), the 1792-vs-1800 investigation record (the 0x50200 set-point + both MCLK sources + the PM f32 @0x0CC), exit 0/1/2 (1cdf16f → merge 2a1ead2).
- **P6-02** `crates/ramsleuth-telemetry/src/amd_smn.rs` — the SMN sysfs accessor (the driver's `smn` attr — write-address→read-value protocol, `ryzen_smu_drv` kobject first, `FdGuard` RAII, error classification via the existing `classify_io_error`; never raw MMIO) + the verified 13-register bitfield table (0x50200–0x50264: MCLK set-point + **GDM = bit 11** + the 27 DRAM subtimings; the +0x100000 two-stage offset rule on marker 0x300; the 0x21060138 tRFC mirror/sentinel; **tRP = 6 bits at bit 16 (21:16)** per the driver reference — the plan table's 22:16 was a transcription error) + the no-panic `apply_smn` overlay (DriverMissing = whole no-op, per-register containment, writes only gdm + the 27 timings; 19 tests — the documented single-file exception, P5-15 precedent) (6985e16 → merge 9c1cdf2; 337→356).
- **P6-03** `crates/ramsleuth-telemetry/src/facade.rs` — the AMD branch becomes `acquire → parse → apply_smn → map`; the overlay is infallible (branch error semantics unchanged; PM fields never touched); 4 wiring tests (3951ff0 → merge d76e411; 356→360).
- **P6-04** `crates/ramsleuth-telemetry/src/spd_decode.rs` — SPD density `0x0D` → 16 Gb (documented vendor/legacy encoding outside the published `0x10..=0x17` family — checked before the family so it cannot shadow it) + maker `0xC1` → G.Skill (JEP106), reconciled against the live module part number `F4-3600C18-32GVK` (live image fixture + 4 regression tests; 0cc24bb → merge 982c948; 360→364).
- **P6-05** `crates/ramsleuth-bench/src/worker.rs` — sub-4 MiB (L1/L2) inner-loop iterations: `small_tier_iters = clamp(32 MiB/total, 1..=65536)` (32 KiB → 1024, 1 MiB → 32, 64 B → 65536 cap; work/pass ≤ 32 MiB) so the small-tier cells are data-dominated; large tiers `iters = 1` byte-identical; checksum conventions frozen (aggregate = `iters ×` single-pass; write `checksum == total_bytes`); `run_pinned` signature + `WorkerResult`/`WorkerError`/`BenchOp` wire shapes unchanged; 3 tests (def1021 → merge 36d1851; 364→367).
- **P6-09** (retroactive — operator live run #1) `scripts/amd-ground-truth.sh` — `monitor_cpu -f` PM-frame capture (without `-f`, `start_pm_monitor()` hard-gates on PM-table version 0x240903 and exits with ZERO frames on this host's 0x380805 table); LC_ALL=C box-drawing parse + exact label match ("Fabric Clock" must not catch "Fabric Clock (Average)"); the 0x50200 two-stage protocol replicated verbatim from `monitor_cpu.c` (marker 0x300 → +0x100000 re-read; set-point `(v & 0x7f)/3×100` at `%.0f`) with the **corrected LE address bytes `00 02 05 00`** (the P6-01 draft wrote `00 02 50 00` = 0x500200 byte-swapped — the origin of the live sentinel); `0xffffffff` → READ-FAILED (never the 4233-MHz misread) + GDM UNKNOWN + the three-source cross-check; the CAD message reworded to pending driver-side confirmation (f05aa92 → merge 667e11d; script-only, no Rust delta).
- **P6-10** (retroactive) `crates/ramsleuth-telemetry/src/amd_smn.rs` — the ryzen_smu driver leaves `0xffffffff` in `smn_result` on a failed PCI read (userspace `read()` still succeeds): the sentinel is treated as a read failure everywhere — a probe sentinel arm (no set-point, no relocation, the 12-register loop at bare addresses), `read_word` collapses Err + sentinel → None, and `word_at` normalizes at the decode level so the all-ones bitfields are never decoded (the coincidental bit-11-of-all-ones GDM=1 is gone); per-register containment; a sentinel probe keeps the PM-table parse value (parse zeroes gdm/timings); the offset rule is first-read-only; 4 tests (186f965 → merge 5817e28; 367→371).
- **P6-11** (retroactive — operator live run #3) `scripts/amd-ground-truth.sh` — with `-f`, `monitor_cpu` runs an INFINITE frame loop (~1 frame/sec) that `timeout 2` always SIGTERM-kills — **rc=124 is the normal capture path** and is now accepted (1–2 frames before the kill); a timeout-kill with zero captured rows is an explicit "produced nothing" failure; every other non-zero rc (126/127 exec failure, `monitor_cpu`'s own exit) unchanged; the `-m` one-shot (exits 0 on its own, no timeout wrapper) untouched; multi-frame parse = last-occurrence-wins per label (0a7dd79 → merge 51f4c4f; script-only, no Rust delta).
- **Parked (never stalled the pipeline):** P6-06 (Intel live decode — hardware-gated, the i5-6600 not yet attached), P6-07 (MSRV decision record — operator-gated, not authored), P6-08 (push runbook — operator-gated, not authored; the runbook content = HANDOVER §8).

### Live verification result (O1 — CLOSED)
The operator's final live run (post P6-11; `scripts/amd-ground-truth.sh`, root daemon, matched conditions, 5950X): **VERDICT: PASS — 31 active gates in tolerance** — MCLK/UCLK/FCLK = 1800 MHz vs dump 1800.00 MHz (±1 MHz); VDDCR_SOC = 1.1375 V vs 1.138 V (±10 mV); **27/27 DRAM timings tick-identical** vs `monitor_cpu -m` (±1 tick, 0 deferred); the 0x50200 set-point read = 1800 MHz (raw `0x00001936`, two-stage +0x100000 path); **GDM triple-confirmed** (smn bit 11 / `monitor_cpu -m` Enabled / dump on — all agree); CAD = honest N/A (bitfields unconfirmed in the driver source — informational, non-fatal). The four operator live re-runs drove the three retroactive fixes (P6-09/10/11). The earlier "1792 vs 1800" delta was root-caused as **transient retraining, not a scaling bug** (the kit is DDR4-3600; 1800 MHz is the set-point).

### Key findings (recorded)
1. The 5950X host runs a **G.Skill F4-3600C18-32GVK kit = DDR4-3600** (1800 MHz MCLK set-point) — the kickoff's "DDR4-3200" was nominal/wrong (the SPD's 3200 MT/s *base* speed misled it).
2. **`monitor_cpu` hard-gates its PM-frame path on PM-table version 0x240903** and exits with ZERO frames unless `-f` is passed — this host's table is 0x380805, so the capture must use `monitor_cpu -f` (infinite frame loop → the `timeout` rc=124 kill is the normal capture path).
3. **The ryzen_smu driver leaves `0xffffffff` in `smn_result` on a failed PCI read** (the userspace `read()` still succeeds) — it is a failure sentinel, never data. Register **0x50200 specifically fails at the PCI level on this firmware (56.78.0)** while 0x50204–0x50264 read fine. The daemon previously decoded bit 11 of the all-ones word as GDM=1 **by coincidence** (P6-10: sentinel → Na; the PM-table parse value survives). The script's set-point falls back to the PM f32 @0x0CC, which is authoritative.
4. **SPD:** density `0x0D` = 16 Gb (vendor/legacy encoding outside the published `0x10..=0x17` family); maker `0xC1` = G.Skill (JEP106) (P6-04).
5. **The tRP mask is 6 bits at bit 16 (21:16)** per the driver reference — the original plan table's 22:16 was a transcription error (the reference wins).
6. **The P6-01 draft had the 0x50200 address bytes swapped** (`00 02 50 00` = 0x500200); the correct LE bytes are `00 02 05 00` — the wrong register is the origin of the live sentinel (P6-09).

### Quality
- **371/371 tests green (debug AND release, whole workspace)** — 337 at cycle start; +19 (P6-02) +4 (P6-03) +4 (P6-04) +3 (P6-05) +4 (P6-10); P6-09/11 script-only (no Rust delta). **Zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **`Cargo.lock` untouched by any P6 chunk** (no manifest changes).
- Scripts: `amd-ground-truth.sh` 317 lines mode 755; `bash -n` + shellcheck 0.11.0 zero findings; the stub harness (19 executions / 75 assertions, all green — the 16 P6-09 paths + 2 looping paths: rc=124-accepted → VERDICT PASS with loop-iter ≥ 2, and rc=124 zero-output → exit 2) validated the logic headlessly (the uncommitted harness lives in a retained worktree at `.tests/gt-stub/` for future reuse).
- No-panic / graceful-degradation contract preserved; zero panics / segfaults in every privilege × CPU state.

### Push state (operator gate)
Local `v2-development` tip = **`51f4c4f`** — **19 commits ahead of `origin/v2-development` @ `b908f7b`** (the P5-15 review/merge commit): the 2 Cycle-4 close-out commits (`2409947`, `b890ecf`) + the Phase 6 plan (`1ad1afe`) + the 16 P6 implementation/merge commits. All **local-only, unpushed** (this compaction performs no push). **The 17 remote `branch/chunk-p5-*` branches are all fully merged** (prune candidates); the 8 local `branch/chunk-p6-*` branches are fully merged too (worktrees retained — prune at the next compaction). On the operator's go-ahead: **fast-forward to `51f4c4f` (NEVER force-push) → prune the 17 remote `p5-*` branches → optional `v2.0.0` tag** (HANDOVER §8; the P6-08 runbook was parked, not authored).

### Open items carried to Cycle 6
1. **GUI — the primary next workstream** (operator: "we have not even gotten near the GUI yet"). The initial 3-zone dashboard (Cycle 3, Phase 4) exists; the gap against the Grand Design §3.1/§3.2 = `FULLSCOPEvsCOMPLETED.md` §"GUI Gap" (the header + its data-model gaps — motherboard/BIOS/AGESA, total RAM, the command-rate slot, the die maker; the grouped timing sections; per-cell live bench updates; the SPD card content; history/charting; settings; export parity; keyboard actions). Multi-cycle; **not hardware-gated** — the whole list is implementable on the 5950X host.
2. **O2 — Intel live MCHBAR decode** — hardware-gated (the i5-6600 not yet attached); the IMC offsets are still SKELETON (P6-06 parked; verify-first, reconcile-if-divergent — plan D6; Intel voltages/CAD = `Na(NotApplicable)`).
3. **O5 — MSRV 1.75 vs. 1.89** — operator call (facts: 441 lockfile packages all MSRV ≤ 1.75; the AVX-512F bodies compile green on both CI legs and are runtime-gated — never exercised on this AVX2-only host; the CI 1.75 leg is the continuous proof; if bump → `rust-version` + CI matrix + pin re-verification as a follow-up micro-chunk).
4. **O6 — push to GitHub** — operator go-ahead (fast-forward to `51f4c4f`, prune the 17 remote `p5-*` branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
5. **CAD/RTT/drive + PDM SMN bitfields** — unconfirmed in the ryzen_smu driver source (the amkillam v0.1.7 audit, P6-02) → honest N/A (confirm-or-Na); pending driver-side confirmation (an AMD UMC register map or an upstream publication); non-fatal.

### Per-chunk history (summarized from DEV_LOG.md)
8 branches (P6-01…P6-05, P6-09, P6-10, P6-11), all reviewed and `--no-ff` merged into `v2-development` (local only, never pushed):
- P6-01: the ground-truth cross-check script (preconditions → exit 2; sequential capture; ±1 MHz / ±10 mV / ±1 tick gates; the 1792-vs-1800 record) (1cdf16f → 2a1ead2).
- P6-02: the `amd_smn` accessor + 13-register bitfield table + no-panic overlay (19 tests; 337→356) (6985e16 → 9c1cdf2).
- P6-03: the facade SMN overlay wiring (`acquire → parse → apply_smn → map`; 4 wiring tests; 356→360) (3951ff0 → d76e411).
- P6-04: the SPD `0x0D`/`0xC1` reconciliation + the live 5950X fixture (4 tests; 360→364) (0cc24bb → 982c948).
- P6-05: the L1/L2 inner-loop iterations (3 tests; 364→367) (def1021 → 36d1851).
- P6-09: the script `gtfix` — `monitor_cpu -f` capture, box-drawing parse, two-stage 0x50200 + the corrected LE address bytes, sentinel → READ-FAILED + GDM UNKNOWN + three-source cross-check, CAD message (stub harness 16 exec / 71 assertions) (f05aa92 → 667e11d).
- P6-10: the daemon sentinel handling — `0xffffffff` never decoded; the coincidental GDM=1 gone; the parse value survives (4 tests; 367→371) (186f965 → 5817e28).
- P6-11: the script `gttimeout` — rc=124 timeout-kill accepted as the normal `-f` capture path; zero-rows-with-kill = "produced nothing" (stub harness rebuilt: 19 exec / 75 assertions) (0a7dd79 → 51f4c4f).
- P6-QA: the final operator live cross-check → **VERDICT PASS (31 active gates in tolerance)**; full audit: 371/371 debug + release, clippy 0, MSRV 1.75, `Cargo.lock` untouched, 6 release binaries, zero panics/segfaults.

Cycle 5 close-out (2026-09-15): the full per-lease history of the cycle is compacted here; `DEV_LOG.md` reset (ACTIVE_WORKERS = no leases, CURRENT_STATE = Cycle 5 COMPLETE + ready for Cycle 6); the local `branch/chunk-p6-*` branches/worktrees retained until the next compaction (the stub harness lives in one of them); no push, no remote branch deletion.

## RamSleuth v2 — Cycle 4 (Phase 5: Packaging & Distribution) — 2026-09-13

### What was delivered
Packaging & distribution for the completed 7-crate pure-Rust workspace — 7 chunks (P5-01…P5-07) + 2 fixes, all `--no-ff` merged into `v2-development` (range `51f872f..e415306`):
- **P5-01** `packaging/ramsleuth-git/ramsleuth.preset` — systemd preset, enables `ramsleuth.service` (eeaf131 → merge 3f98513).
- **P5-02** `packaging/ramsleuth-git/PKGBUILD` + `packaging/ramsleuth-git/ramsleuth-git.install` — AUR package: builds the 7-crate workspace, installs the 6 binaries (`ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui`, `ramsleuth-bench`, `ramsleuth-telemetry`) to `/usr/bin` plus the daemon unit and preset (merged 396a72d). **P5-02-fix**: the `ramsleuth` group is created on the TARGET system by the `.install` `pre_install`/`pre_upgrade` hooks (idempotent `getent || groupadd -r`, runs as root); the ineffective build-env `groupadd` is removed from `package()` (replaced by a NOTE comment) (ddf9650 → merge a27d976).
- **P5-03** `packaging/ryzen-smu-dkms/dkms.conf` — `AUTOINSTALL=yes` (module auto-rebuilds on kernel change), optional AMD extra (2c2346e → merge 093f497).
- **P5-04** `scripts/install-ryzen-smu-dkms.sh` — idempotent operator-run DKMS helper implementing HANDOVER §7 (pm_table fast-path, sudo re-exec, never-guess kernel-headers guard with candidate listing, verified-upstream shallow clone, `dkms add`/`install`, modprobe + `/etc/modules-load.d`, `pm_table` verify with dmesg hint) (150d88f → merge 28071d1). **P5-04-fix**: the dkms.conf fallback now resolves the repo path first, then the installed `/usr/share/ryzen-smu-dkms/` path, so the helper works both from a checkout and from the AUR package (e41d4ae → merge 46c4b8e).
- **P5-05** `packaging/ryzen-smu-dkms/PKGBUILD` — optional provisioning AUR package with a safe no-in-chroot design: ships the P5-03 dkms.conf + P5-04 helper as `/usr/bin/ryzen-smu-dkms-install`; the operator runs the helper on the target (430b2f3 → merge c93410e).
- **P5-06** `.github/workflows/ci.yml` — GitHub Actions: `cargo test --workspace` (debug + release) and clippy `-D warnings` on a `["1.75", "stable"]` matrix (`fail-fast: false`), plus a release build job uploading a 6-binary artifact (d5c384b → merge f3c6d25). Committed `Cargo.lock` untouched — the MSRV 1.75 leg is verified by CI (host has no rustup; local MSRV check skipped and documented).
- **P5-07** `packaging/README.md` — operator/end-user packaging guide: install table (6 binaries → `/usr/bin`, unit + preset → systemd paths, group via `.install` hooks), AUR + `makepkg -si` paths, `usermod` / `Group=wheel` fallback, sandboxed unit day-2 commands, ryzen-smu-dkms extra, CI matrix summary, no-panic note (cf15137 → merge 63b1ea4).

### Quality
- **327/327 tests green (debug AND release, whole workspace), 0 failures**; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); release build OK.
- **9/9 packaging/CI/systemd files valid**: all present and non-empty (`ci.yml`, `packaging/README.md`, ramsleuth-git `PKGBUILD` + `ramsleuth-git.install` + `ramsleuth.preset`, ryzen-smu-dkms `PKGBUILD` + `dkms.conf`, install script, `systemd/ramsleuth.service`); `bash -n` PASS ×4; shellcheck zero findings; `ci.yml` YAML-valid (python3 + pyyaml); install script mode 755.
- **ZERO Rust source changes**: range `51f872f..e415306` touched no `.rs` and no `Cargo.*` files (packaging/CI/docs only); committed `Cargo.lock` untouched by any Phase 5 chunk; no-panic / graceful-degradation contract preserved.
- QA audit on `v2-development` @ e415306: **PASS** (green cycle) — full re-verification after the P5-02-fix target-group follow-up; P5-QA closed.

### Push state (operator gate — updated at the Cycle 5 handover, 2026-09-13)
During the cycle `v2-development` (== e415306 at the initial compaction) and the first 9 `branch/chunk-p5-*` branches were fast-forwarded to origin (no force-push); the post-merge follow-ups (P5-08…P5-15 branches + `origin/v2-development` through the P5-15 merge commit **b908f7b**) were likewise fast-forwarded. **Current state:** local `v2-development` is **2 commits ahead of `origin/v2-development` @ b908f7b** — **2409947** (the final compaction commit) + the Cycle 5 docs handover commit (this MASTER_LOG update + the `Docs/HANDOVER.md` / `FULLSCOPEvsCOMPLETED.md` / `Docs/RamSleuth-v2.md` / Grand Design refresh) — both unpushed (LOCAL only). **17 remote `branch/chunk-p5-*` branches remain on origin — all fully merged into `v2-development`** (p5-01, p5-02, p5-02-fix, p5-03, p5-04, p5-04-fix, p5-05, p5-06, p5-07, p5-08-sysfs, p5-09-script, p5-10-readme, p5-11-dkms, p5-12-dkmsconf, p5-13-dkmsver, p5-14-dkmsinstall, p5-15-pmtable) → prune candidates. Push (ff to the local tip, ≥ 2409947), pruning, and an optional tag (e.g. `v2.0.0`) all remain **pending explicit operator go-ahead** (HANDOVER §8) — this compaction performs no push and deletes no remote branch.

### Open items carried to Cycle 5
1. AMD tick-identical ground truth — needs the `ryzen_smu` module built + loaded + root on the 5950X host (P5-03/P5-04/P5-05 now make this a buildable/installable path; P5-08/09/10 reconciled the upstream to `amkillam/ryzen_smu` + the canonical `ryzen_smu_drv` sysfs path — install now unblocked).
2. Intel live MCHBAR decode — on the LGA-1151 i5-6600 (Skylake, dual-channel) test machine.
3. Model reconciliation — AMD PM byte offsets + Intel IMC offsets are plan-mandated skeletons; SPD maker `0xC1` + density `0x0D` codes are outside the frozen tables. (Updated below: the **AMD PM model was reconciled for Vermeer in P5-15** — f32 layout + `TableVersionId` sets; remaining: SMN-attr CAD/timings, Intel IMC offsets, SPD codes.)
4. P1 L1/L2 bandwidth overhead refinement (inner-loop iterations for the small-tier working sets).
5. MSRV 1.75 → 1.89 decision (deferred for AVX-512F; workspace deliberately kept at 1.75 via lockfile pins; the first GitHub Actions run is the MSRV 1.75 checkpoint).
6. Push to GitHub — `v2-development` already fast-forwarded to origin during the cycle; remote chunk-branch prune + optional tag pending explicit operator go-ahead.

### Per-chunk history (summarized from DEV_LOG.md)
9 branches (P5-01…P5-07 + P5-02-fix + P5-04-fix), all reviewed and `--no-ff` merged into `v2-development`, then fast-forwarded to origin:
- P5-01: systemd preset enabling `ramsleuth.service` (eeaf131 → merge 3f98513).
- P5-02: ramsleuth-git AUR PKGBUILD + `.install` (6 bins, daemon unit, ramsleuth group; merged 396a72d); P5-02-fix: target-side `ramsleuth` group via `pre_install`/`pre_upgrade`, build-env `groupadd` removed (ddf9650 → merge a27d976).
- P5-03: ryzen-smu-dkms `dkms.conf` (`AUTOINSTALL=yes`, optional AMD extra) (2c2346e → merge 093f497).
- P5-04: idempotent operator-run DKMS install helper (HANDOVER §7) (150d88f → merge 28071d1).
- P5-04-fix: dkms.conf fallback resolves repo + installed `/usr/share` (e41d4ae → merge 46c4b8e).
- P5-05: optional ryzen-smu-dkms provisioning AUR package (safe no-in-chroot design) (430b2f3 → merge c93410e).
- P5-06: GitHub Actions CI — 1.75/stable matrix (test debug+release, clippy `-D warnings`) + 6-binary artifact (d5c384b → merge f3c6d25).
- P5-07: packaging README (operator/end-user guide) (cf15137 → merge 63b1ea4).
- P5-QA: full audit @ e415306 — 327/327 debug + release, clippy 0, 9/9 files valid, zero `.rs` / zero `Cargo.*` in range; green cycle.

### ryzen_smu uAPI reconciliation (post-merge, 2026-09-13)
Post-compaction follow-up: operator ran `scripts/install-ryzen-smu-dkms.sh` on the 5950X host; the default `git clone` of the dead `53XU/ryzen_smu` repo 404'd and fell back to an interactive GitHub credential prompt. Upstream review of 3 candidates:
- `leogx9r/ryzen_smu` — original, frozen 2021 (exact `ryzen_smu` module, `/sys/kernel/ryzen_smu`).
- `FlyGoat/ryzen_nb_smu` — unrelated experiment (module `ry_nb_pp`) — rejected.
- `amkillam/ryzen_smu` — active fork, v0.1.7 (2026-08), kernel 7.2+ fix; exact `ryzen_smu` module, exposes `/sys/kernel/ryzen_smu_drv/pm_table` (kobject `ryzen_smu_drv`), `monitor_cpu` CLI, Zen 3 + kernel 7.2+ support. **CHOSEN: `amkillam/ryzen_smu` (branch `main`)**.

**CRITICAL pre-existing bug fixed:** the daemon read the wrong sysfs path (`/sys/kernel/ryzen_smu/pm_table`); the real module exposes `/sys/kernel/ryzen_smu_drv/pm_table`.

- **P5-08** `crates/ramsleuth-telemetry/src/amd_smu.rs` — candidate list: canonical `/sys/kernel/ryzen_smu_drv/pm_table` first + legacy `/sys/kernel/ryzen_smu/pm_table` fallback; +1 regression test (327→328).
- **P5-09** `scripts/install-ryzen-smu-dkms.sh` — default upstream now `amkillam/ryzen_smu` (`RYZEN_SMU_URL` override kept) + `ryzen_smu_drv` path in fast-path/verify.
- **P5-10** `packaging/README.md` — path + upstream consistency (verify step, `monitor_cpu` ground-truth note).

**QA re-audit: PASS** (66c3122) — 328/328 (debug + release), clippy 0 warnings; daemon/script/README consistent; no-panic graceful degradation preserved (`acquire_on_this_host_is_graceful`). Residual `53XU` / non-`_drv` references are intentional: legacy-fallback candidate list, the script's explicit "53XU is DEAD" warning, historical/spec docs — no doc-scrub needed.

### ryzen_smu install-path fixes (post-merge, 2026-09-13)
Post-reconciliation live runs: operator re-ran `scripts/install-ryzen-smu-dkms.sh` on the 5950X host; the amkillam clone SUCCEEDED but `dkms add` / `dkms install` failed with "Arguments <module> and <module-version> are not specified." (first run); a second re-run then succeeded through staging + `monitor_cpu` + `dkms add` (the symlink was created) but `dkms build` died with the same "Arguments ... not specified" message. Three independent root causes, three merged chunks:
- **P5-11** `scripts/install-ryzen-smu-dkms.sh` — the script cloned to `/opt/ryzen-smu-src` but never staged the source into `/usr/src/ryzen_smu-<pkgver>/` (where DKMS discovers modules), so `dkms add ryzen_smu` had nothing to find. Fix: stage `{LICENSE,Makefile,dkms.conf,drv.c,smu.c,smu.h}` into `/usr/src/ryzen_smu-$PKGVER/` (`PKGVER` = `git rev-list --count .` + `git rev-parse --short`) **before** `dkms add ryzen_smu/$PKGVER` → build → install → modprobe; added a `/usr/lib/depmod.d/ryzen_smu.conf` override + built/installed the `monitor_cpu` CLI to `/usr/bin` (ground-truth for open item 1).
- **P5-12** `packaging/ryzen-smu-dkms/dkms.conf` — the repo's concrete dkms.conf (P5-03) had a `MAKE` line `M=${dkms_tree}/...` missing the `/build` dir, so `dkms build` would still fail. Fix: aligned `MAKE` / `CLEAN` to the authoritative upstream amkillam pattern (`make TARGET=${kernelver}`; the staged Makefile resolves the kernel KDIR + `M=$(CURDIR)`); `PACKAGE_VERSION` in sed-compatible `@VERSION@` form (the script's whole-line `sed` aligns it to `$PKGVER`); `DEST_MODULE_LOCATION` kept `/extra` to match the script's depmod override.
- **P5-13** `scripts/install-ryzen-smu-dkms.sh` — second operator re-run: staging + `monitor_cpu` + `dkms add` SUCCEEDED (the symlink was created), but `dkms build` then died with "Error! Arguments <module> and <module-version> are not specified. Usage: add ...". Precise root cause (diagnostic): L136 `dkms build "${MODULE}"` and L141 `dkms install "${MODULE}"` passed the BARE module name (no version) while L131 `dkms add "${MODULE}/${PKGVER}"` passed module/version; DKMS 3.4.3 does NOT auto-resolve a bare name to the single registered version, so `do_build` → `is_module_added "ryzen_smu" ""` (empty version) → looks un-added → internally calls `add_module` → `check_module_args` dies with the hardcoded "Usage: add ..." message (a build invocation printing an add usage). Live state corroborated: `dkms status` = `ryzen_smu/1.d298366: added` only, empty `build/` dir, no kernel subdir; kernel build dir valid; staged dkms.conf `PACKAGE_VERSION` correctly substituted (ruled out); the "Deprecated feature: CLEAN" line was a red herring (harmless stderr from the successful add). Fix: L136 → `dkms build "${MODULE}/${PKGVER}" -k "${KERNEL}"`; L141 → `dkms install "${MODULE}/${PKGVER}" -k "${KERNEL}"` (symmetric with L131); removed the deprecated `CLEAN="make clean"` line from `packaging/ryzen-smu-dkms/dkms.conf` (DKMS 3.4.3 ignores it; silences the warning). No `dkms remove` needed to recover — the fixed re-run hits the "already registered — continuing" path and proceeds to a real build.
- **P5-14** `scripts/install-ryzen-smu-dkms.sh` — third operator re-run: the module now BUILDS successfully ("Building module(s)... done." + signed `ryzen_smu.ko` at `/var/lib/dkms/ryzen_smu/1.d298366/build/`), but the "already installed" check (L138) printed "already installed ... skipping dkms install" and SKIPPED the install, so the `.ko` was never copied into `/lib/modules/` and `modprobe ryzen_smu` failed with "Module not found." Root cause: L138 `[[ -d "/var/lib/dkms/${MODULE}/${PKGVER}/${KERNEL}/" ]]` tested for the BUILD dir (created by `dkms build`), not the install state — so after a build (before an install) it wrongly skipped `dkms install`. Live state corroborated: `dkms status` = `ryzen_smu/1.d298366, 7.2.3-1-cachyos-custom, x86_64: built` (built, not installed); `/lib/modules/7.2.3-1-cachyos-custom/extra/` absent; `modinfo ryzen_smu` = not found. Fix: L138 now uses `dkms status | grep -qE "^${MODULE}/${PKGVER},[[:space:]]*${KERNEL},[[:space:]]*[^:]*:[[:space:]]*installed[[:space:]]*$"` (matches the "installed" state specifically, not "built"); verified behaviorally (built → no match → install runs; installed → match → skip). Safe failure mode = re-install (idempotent), never a wrong skip.
- **Reference:** the operator provided the working AUR `aur/ryzen_smu-dkms-git` PKGBUILD as the staging reference; we deliberately do NOT depend on that external AUR package (self-contained tooling in RamSleuth; only the module source is fetched from amkillam git at install time).
- **Status:** install path fully fixed (staging + dkms.conf MAKE + build/install version args + install-state check); operator re-run on the real kernel is the ground-truth verification (the module is currently built-not-installed, so the fixed re-run will now actually run `dkms install`).

### AMD PM-table model reconciliation (P5-15, open item 3, post-merge, 2026-09-13)
Post-install live run: after the P5-08 path fix + release rebuild, the daemon read the `pm_table` but returned `N/A (unknown PM table version)` — the SKELETON version check read the wrong source (blob bytes 0-3 = PPT-limit float) and guarded the wrong domain (SMU-FW 7.11.x/12.x/13.x, a plan artifact matching no real `TableVersionId`).
- **Research (`pmtable-map`):** the PM-table VERSION is the `TableVersionId` in the SIBLING sysfs file `/sys/kernel/ryzen_smu_drv/pm_table_version` (4 LE bytes), NOT in the blob (a headerless `f32` array). The `monitor_cpu` parser is the verified reference: 326-`f32` array (0x518 bytes), FCLK 0x0C0, UCLK 0x0C8, MCLK 0x0CC (MHz), VDDCR_SOC 0x0B0 (volts→mV×1000). CAD/timings/GDM/PDM/VDDIO/VPP are NOT in the table (SMN regs via the driver `smn` attr) → degrade to honest `Na`. Vermeer/Zen 3 accepted `TableVersionId`s: {0x2D0803,0x2D0903,0x380005,0x380505,0x380605,0x380705,0x380804,0x380805,0x380904,0x380905} (+ optional Matisse 0x24xxxx set). Operator's live `TableVersionId` = 0x380805 (in-set); size attr 2288 = blob length.
- **Fix (P5-15, `amd_smu.rs` + `amd_pm.rs`):** version re-sourced from the sibling `pm_table_version` file (candidate list, `ryzen_smu_drv` first) with first-word-of-blob fallback (degrades to `UnknownPmTableVersion`); size validated against `pm_table_size` (mismatch → `Parse`); `amd_pm.rs` switched from the `u16`-region SKELETON to the `f32` layout (FCLK/UCLK/MCLK/VDDCR_SOC offsets + MIN_LEN 0x518) accepting the Vermeer (+ Matisse) `TableVersionId` sets; CAD/timings/other rails = 0 → honest `Na` under existing sanity gates.
- **Result:** 337/337 debug+release (was 328, +9 net new tests), clippy clean, no-panic preserved. Live end-to-end verified on the host: AMD section fully populated (mclk/uclk/fclk ≈ 1800 MHz OneToOne, vddcr_soc ≈ 1138 mV, timings/CAD honest `Na`) instead of `N/A (unknown PM table version)`.
- **Note:** plan D2's "5950X = SMU 7.11.x" was wrong (Vermeer PM `TableVersionId` family bytes are 0x2D/0x38); the sanctioned driver `smn` sysfs attr (not raw MMIO) is the path to the 27 DRAM subtimings + GDM if those are wanted later (currently `Na`).

Cycle 4 close-out (2026-09-13, updated): initial compaction recorded P5-01…P5-07 + 2 fixes + P5-QA @ e415306 (327/327); this update records the ryzen_smu uAPI reconciliation (P5-08/09/10 + QA re-audit @ 66c3122, 328/328, clippy 0) **and** the install-path fixes from the operator live runs (P5-11 source staging into `/usr/src` + P5-12 dkms.conf `MAKE`/`CLEAN` aligned to upstream amkillam + P5-13 `dkms build`/`install` passing `${MODULE}/${PKGVER}` and removing the deprecated `CLEAN` directive + P5-14 install-state skip-check fix — all four merged; packaging/script/dkms.conf only, zero Rust changes) **and** the AMD PM-table model reconciliation (P5-15, open item 3, Vermeer — the first Rust-source change of the cycle: `amd_smu.rs` + `amd_pm.rs` only, version re-sourced from the sibling `pm_table_version` attr, `f32` layout FCLK/UCLK/MCLK/VDDCR_SOC, 337/337, clippy 0). `DEV_LOG.md` reset (ACTIVE_WORKERS = no leases, CURRENT_STATE = Cycle 4 complete + QA passed 337/337 + clippy clean; ready for Cycle 5). Local-only commit; no push, no remote branch deletion.

## RamSleuth v2 — Cycle 3 (Phase 3: Privilege-Separated Architecture) — 2026-09-13

### What was delivered
Privilege-separated architecture: one privileged daemon owns all hardware probing; all clients are fully unprivileged and reach it over a Unix socket. Five new crates + a serde wire foundation:
- **`ramsleuth-protocol`** — the wire contract: `Request` / `Response` / `BenchMode` / `Message` (serde-derived, payloads reused verbatim from telemetry + bench) + `DEFAULT_SOCKET_PATH` + a length-prefixed Bincode frame codec (`frame.rs`) with a 16 MiB `MAX_FRAME_SIZE` guard (`encode_frame` / `decode_frame` / `Frame` / `FrameError`); zero `unsafe`, tokio-free.
- **`ramsleuth-daemon`** (lib + bin) — `caps` SOFT privilege probe (geteuid + `CapEff` bit 21; never panics/exits, warns softly); `socket` listener (mode **0660**, stale-file probe → live `AlreadyRunning` / dead rebind, best-effort `chown ramsleuth:wheel`); `TelemetryCache` (TTL-gated, injectable collector, clone-within-TTL); `BenchJobManager` (single-flight, clean cancel, per-cell progress); `rpc` (per-connection async RPC over the frozen wire contract); `main` bin (`--socket` / `--max-age`, SIGTERM/SIGINT → graceful stop + socket removed, no-panic contract) + **`systemd/ramsleuth.service`** (packaging artifact; CAP_SYS_RAWIO-only unit).
- **`ramsleuth-client`** (lib + bin) — synchronous `UnixStream` transport (read/write timeouts, 3-retry backoff, friendly `DaemonDown` hint); pure `dump` dashboard renderer (full hardware timings, every N/A cell with a reason); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes 0/1/2).
- **`ramsleuth-tui`** (lib + bin) — ratatui 0.29 + crossterm 0.28; pure `key_to_action` (R / S / Q); 3-zone non-scrolling dashboard; terminal loop with a background 2 s updater; MSRV-safe lockfile pins for ratatui transitive deps.
- **`ramsleuth-gui`** (lib + bin) — egui / eframe / egui_extras 0.27.2; semantic palette + **F2** PNG / **F3** JSON export to `$HOME`; `TelemetryData` + background poller with a bench command channel + cancel; 3 zones; eframe app 1400×900 @ ~60 FPS.
- **Serde foundation** — serde derives on all telemetry (CPUID / AMD / Intel / SPD / facade) + bench (worker / orchestrator / streamed) public wire types; `SystemMemoryTelemetry`, `BenchmarkGrid`, `StreamProgress` (and `WorkerError`) are wire-serializable, each with bincode round-trip tests.

### Quality
- **327/327 tests green (debug AND release, whole workspace)**; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); release build OK.
- **MSRV = 1.75** (max `rust_version` across all **441 packages**); ratatui / egui transitive deps held ≤ 1.75 via lockfile pins (`instability` ≤ 0.3.10, `unicode-segmentation` ≤ 1.12.0; existing tokio 1.53.1 / serde 1.0.229 pins untouched).
- **Live end-to-end verified** (this host, unprivileged): daemon up with a 0660 socket; unprivileged `ramsleuth-client` `dump` / `status` / `bench` all **exit 0** (bench: `BenchStarted` + live progress + full 4×4 grid); TUI rendered under a PTY (3 zones live); GUI ran on Wayland (1400×900, all zones, F2/F3 export, clean Q); daemon `SIGTERM` → graceful stop, exit 0, socket removed; no-daemon path → **exit 1** + friendly `DaemonDown` start hint; **zero panics / segfaults**.
- QA audit on `v2-development` (post-merge, live unprivileged run): **CORE GATE PASSED** — unprivileged `ramsleuth-client -- dump` prints full hardware timings against a running daemon (the Phase 3 exit criterion).

### Live result (this host: Ryzen 9 5950X / Zen 3 / DDR4, `ryzen_smu` absent, unprivileged)
- Graceful degradation confirmed end-to-end: **AMD N/A (`DriverMissing`, no `ryzen_smu`)**; **Intel N/A (`UnsupportedHardware`)**; **SPD density `0x0D` parse-error → N/A**; **SPD maker `0xC1` shown raw-hex**; 2× DDR4 SPD modules at 3200 MT/s with per-profile timings; live CPU readout (Amd(Zen3), brand string). No panic in any privilege/CPU state.

### Key decisions
- One privileged daemon is the sole privilege boundary: all hardware I/O (SMU/IMC/SPD) stays daemon-side; the client/TUI/GUI chain is tokio-free and never touches hardware.
- Synchronous length-prefixed Bincode frames with a 16 MiB size guard over a 0660 Unix socket; tokio confined to the daemon (async accept loop + `spawn_blocking` pumps); std-only client transport.
- Serde added to every wire-crossing public type up front, so the protocol crate reuses payload types verbatim (no duplication, no boxing of frozen arm shapes).

### Open items carried to Cycle 4
1. AMD tick-identical ground truth — needs `ryzen_smu` module built + loaded + root on the 5950X host.
2. Intel live MCHBAR decode — on the LGA-1151 i5-6600 (Skylake, dual-channel) test machine.
3. Model reconciliation — AMD PM + Intel IMC byte offsets are plan-mandated skeletons; SPD maker `0xC1` + density `0x0D` codes are outside the frozen tables.
4. P1 L1/L2 bandwidth overhead refinement (inner-loop iterations for the small-tier working sets).
5. MSRV 1.75 → 1.89 bump decision (deferred; workspace deliberately kept at 1.75 via lockfile pins).
6. Push to GitHub — condition met (confirmed working app) but **held local per policy**; ready on explicit go-ahead. No force-pushes without sign-off.

### Per-chunk history (summarized from DEV_LOG.md)
30 single-file micro-chunks (P3-01…P3-30) + 2 doc-drift fixes (DocFix, DocFix2), all reviewed and merged into `v2-development` via `--no-ff` (local only, never pushed):
- P3-01…P3-06: serde foundation — derives + bincode round-trip tests on the telemetry wire types (`NaReason` / `Section<T>`, CPUID, AMD, Intel channel/readout, SPD decode, `SystemMemoryTelemetry` facade root + `PartialEq`).
- P3-07…P3-09: bench serde + streaming — `BenchOp` / `WorkerResult` (+ `WorkerError` wire-serializable), `Tier` / `Metric` / `BenchmarkGrid`, `streamed.rs` (`run_streamed` with clean cancel + per-cell progress, `StreamTarget` / `StreamProgress` / `StreamError`).
- P3-10…P3-11: `ramsleuth-protocol` birth — `Request` / `Response` / `BenchMode` / `Message` + `DEFAULT_SOCKET_PATH`; length-prefixed Bincode frame codec + 16 MiB guard.
- P3-12…P3-17: `ramsleuth-daemon` birth — crate scaffold + `caps` SOFT probe; `socket` (0660 + stale-rebind + chown); `cache` (TTL, injectable collector); `bench_job` (single-flight + cancel); `rpc` (per-connection async); `main` bin + `systemd/ramsleuth.service`.
- P3-18…P3-21: `ramsleuth-client` — `UnixStream` transport (timeouts / retries / `DaemonDown`); pure `dump` renderer (the exit-criterion command); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes).
- P3-22…P3-24: `ramsleuth-tui` — crate birth + `events` (`key_to_action` R/S/Q, MSRV lockfile pins); 3-zone dashboard; terminal loop + background 2 s updater.
- P3-25…P3-30: `ramsleuth-gui` — style/semantic palette; shared state + background poller (bench command channel + cancel); telemetry zone (matrix); bench zone (4×4 + progress + run/cancel); status zone (SPD cards + daemon status + F2/F3/Q actions); eframe app shell (1400×900, ~60 FPS).
- DocFix / DocFix2: residual `--max-age` doc-drift fixes (`5 s` → `2 s` to match the real 2 s daemon default); doc-only, zero code/behavior change.

Cycle 3 close-out (2026-09-13): all 30 `branch/chunk-P3-*` plus `branch/chunk-docfix` / `branch/chunk-docfix2` verified fully merged into `v2-development` and pruned (`git branch -d` only; no unmerged branch touched). `DEV_LOG.md` reset (ACTIVE_WORKERS = no leases, CURRENT_STATE = Cycle 3 complete + ready for Cycle 4).

## RamSleuth v2 — Cycle 2 (Phase 2: Live Memory Controller Telemetry) — 2026-09-12

### What was delivered
`ramsleuth-telemetry` complete (11 chunks, P2-01…P2-11):
- CPUID vendor / Zen-family detection (interface freeze) — P2-01.
- Error + `Section<T>` no-panic contract (interface freeze) — P2-02.
- Privilege-guarded AMD SMU access (sysfs→char-dev, sole `nix` dep) — P2-03.
- Version-guarded bounds-checked AMD PM parse (SMU 7.11.x / 12.x / 13.x) — P2-04.
- Shared display types + AMD mapping (sanity-gated) — P2-05.
- Intel MCHBAR acquire + read-only /dev/mem mmap guard (Intel-gated; STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege; volatile MMIO read) — P2-06.
- Intel per-channel IMC decode — P2-07.
- Unprivileged SPD EEPROM acquisition — P2-08.
- SPD decode (JEP106, rank, XMP/EXPO) — P2-09.
- `SystemMemoryTelemetry` facade + `collect()` (per-branch error containment) — P2-10.
- Verification CLI (`cargo run -p ramsleuth-telemetry [--json]`) — P2-11.

### Quality
- 159/159 tests green (debug + release, whole workspace); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live telemetry CLI exit 0 (text + `--json`).
- QA audit 2026-09-12 on `v2-development` @ 367bef9: all 8 runnable gates PASS.

### Live result (this host: Zen 3 / DDR4, `ryzen_smu` module absent)
- `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD: N/A (DriverMissing)`; `Intel: N/A (UnsupportedHardware)`; SPD 2× DDR4 modules (rank=1, speed=3200 MT/s, maker `0xC1` raw-hex, density `0x0D` → Na). No panic in any privilege/CPU state.

### Key decisions
- Direct SMU access (no `ryzen_smu` crate): sysfs-first + char-dev read; `nix` (features `fs`/`ioctl`/`mman`) is the only new dependency of Phase 2.
- AMD PM + Intel IMC byte offsets are plan-mandated SKELETONS pending live-silicon reconciliation.

### BLOCKED acceptance gates (hardware/driver, not code)
1. AMD tick-identical ground truth — needs `ryzen_smu` module loaded + root.
2. Intel live MCHBAR decode — needs Intel silicon.
3. Model reconciliation — AMD PM byte offsets, Intel IMC offsets, SPD maker `0xC1` + density `0x0D` codes.

### Per-chunk history (summarized from DEV_LOG.md)
- P2-01: CPUID vendor/Zen-family detection (interface freeze).
- P2-02: `TelemetryError` + `Section<T>` no-panic contract (interface freeze); 14/14 tests.
- P2-03: AMD SMU acquire (sysfs→char-dev; `nix` 0.29 sole new dep); 20/20; live → `DriverMissing{ryzen_smu}`.
- P2-04: version-guarded bounds-checked AMD PM parse; 29/29; offsets = plan skeleton.
- P2-05: shared display types + sanity-gated AMD mapping; 39/39.
- P2-06: Intel MCHBAR + read-only /dev/mem guard; 48/48. *Process note:* review FAILED (F1 STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege, F2 volatile read) → fix `85114a6` → re-review PASS.
- P2-07: Intel per-channel IMC decode; 66/66; offsets = documented model/skeleton.
- P2-08: unprivileged SPD EEPROM acquire (world-readable sysfs `eeprom`); 74/74; live → 2× 512B DDR4.
- P2-09: SPD decode (JEP106, rank, XMP/EXPO); 85/85. *Process note:* recovered prior subagent's uncommitted/empty payload (3 compile fixes + missing 11-test module).
- P2-10: `SystemMemoryTelemetry` facade + `collect()` (per-branch containment); 90/90.
- P2-11: verification CLI (hand-rolled JSON, no serde/clap); 96/96. *Process note:* recovered prior subagent's empty payload.
- phase2-qa: full audit 2026-09-12 — 8/8 runnable gates PASS; 159/159 tests.

Cycle 2 close-out (2026-09-12): all 11 `branch/chunk-P2-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.

## RamSleuth v2 — Cycle 1 (Phase 1: Native Benchmark Engine) — 2026-09-11

### Delivered
- 6-crate Cargo workspace scaffolded (P1-01); runtime AVX2/AVX-512F feature detection (P1-02, `detect() -> CpuTopology`).
- `ramsleuth-bench` benchmark engine complete:
  - /sys physical-core enumeration with SMT filter + total/per-CCD L3 (P1-02).
  - L1/L2/L3/DRAM buffer sizing via pure `plan(&CpuTopology) -> BufferPlan`, all 64-byte aligned: sysfs L1/L2 with safe fallbacks, per-CCD L3 slice, DRAM max(256 MiB, 3× total L3), 128 MiB latency ring (P1-03).
  - AVX2 streaming read kernel — unrolled 4-wide aligned loads, lane accumulation, word-sum checksum, feature-dispatched scalar fallback (P1-04); AVX2 non-temporal write kernel — NT stores + single trailing SFENCE (P1-05); AVX2 copy kernel — load/NT-store pairs, destination checksum via avx2_read (P1-06).
  - AVX-512F read/write/copy variants (`_mm512_*` aligned/NT/SFENCE shapes) with documented AVX2 fallback when AVX-512F is absent; clippy msrv 1.89 gating (P1-07).
  - Pinned barrier-synced multi-thread worker dispatch — one `sched_setaffinity`-pinned worker per physical core, Barrier lockstep, exact-once block-aligned partition, checksum aggregation, graceful unpinned fallback; libc 0.2 the only new dep (P1-08).
  - 64-byte-stride pointer-chase latency kernel — SplitMix64 Fisher-Yates single-cycle ring, strictly dependent loads, serialized `__rdtscp` with self-calibrating Instant conversion + non-x86 fallback (P1-09).
  - Orchestrator aggregating the 4×4 AIDA64-style grid (Memory/L3/L2/L1 × Read/Write/Copy + Latency); DRAM-tier latency chases the materialized full-DRAM-size buffer (beyond L3); safe 64B-aligned `AlignedBuf`; best-of-3 Instant timing owned by the orchestrator (P1-10).
  - Verification CLI: `cargo run -p ramsleuth-bench [--avx512] [--json]` — std-only option parsing (unknown flag → exit 2), serde-free JSON (`read_gbps/write_gbps/copy_gbps/latency_ns`, non-finite → null), fixed-width grid renderer, graceful exit 1 on detect/run errors, documented AVX-512→AVX2 fallback note (P1-11).

### Quality
- 63/63 tests green (debug + release); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live bench run exit 0 (default, `--json` output parsed, `--avx512` fallback path verified).
- QA audit 2026-09-12 on `v2-development` @ b5eea35: 7/7 runnable gates PASS; AIDA64 parity gate deferred to the DDR5-6000 AM5 host (this host is Zen 3 / DDR4).

### Measured grid (this host: Ryzen 9 5950X, Zen 3, 16 phys / 32 logical, 64 MiB L3, DDR4, AVX2-only — informational)
| Tier | Read (GB/s) | Write (GB/s) | Copy (GB/s) | Latency (ns) |
| --- | --- | --- | --- | --- |
| Memory (DRAM) | 49.13 | 43.83 | 15.75 | 81.5 |
| L3 | 59.03 | 31.28 | 16.29 | 56.5 |
| L2 | 1.33 | 1.41 | 1.40 | 5.6 |
| L1 | 0.08 | 0.09 | 0.09 | 1.2 |

Repeat runs (variance context): `--json` → DRAM 47.58 / 43.37 / 15.68 GB/s @ 81.4 ns; L3 63.04 / 32.02 / 16.88 GB/s @ 39.5 ns. `--avx512` → DRAM 48.89 / 43.48 / 15.67 GB/s @ 82.9 ns. L3 drifts ~±25% run-to-run (best-of-3 at these sizes is noisy). Sanity: latency ordering monotonic L1 < L2 < L3 < DRAM; no negative/zero/NaN cells; DRAM read ≈ 48% of DDR4-3200 4-channel theoretical peak.

### Known follow-ups
1. L1/L2 bandwidth cells are overhead-limited: the small tier working set (32 KiB L1d / 1 MiB L2) split across 16 pinned workers is dominated by thread/barrier/timing overhead per pass. Add inner-loop iterations to amortize that overhead for small tiers (Phase 2 design note; cells not comparable across tiers).
2. Confirm §1.3 AIDA64 parity (DRAM BW ±5%, latency 60–75 ns band) on the target DDR5-6000 AM5 machine — this host measured DRAM latency 81.4–83.5 ns (out of band as expected on DDR4; reported, not failed).
3. Workspace MSRV 1.75 vs AVX-512F intrinsics requiring Rust 1.89 — decide on an MSRV bump.
4. Push strategy for `v2-development` vs the divergent legacy upstream `master` is pending user decision (no push from this cycle).

### Per-chunk history (summarized from DEV_LOG.md)
- P1-01: runtime CPU feature detection module (merged f0cb26e).
- P1-02: `detect() -> Result<CpuTopology, TopologyError>` — /sys topology enumeration, SMT filter; 13/13 tests.
- P1-03: pure buffer planning/sizing, 64-byte aligned; 23/23 tests.
- P1-04: AVX2 streaming read + word-sum checksum; 27/27 tests.
- P1-05: AVX2 non-temporal write; 31/31 tests.
- P1-06: AVX2 copy (NT stores + SFENCE); 35/35 tests.
- P1-07: AVX-512F read/write/copy with AVX2 fallback; 40/40 tests; 512-bit bodies compiled but not directly exercised on this AVX2-only host.
- P1-08: pinned barrier-synced worker dispatch; 47/47 tests.
- P1-09: pointer-chase latency kernel; 53/53 tests; flagged that a true DRAM row requires the chased working set to exceed L3 (resolved in P1-10).
- P1-10: orchestrator + 4×4 grid + DRAM latency materialization; 60/60 tests.
- P1-11: verification CLI; 63/63 tests; contract review passed.
- phase1-qa: full audit 2026-09-12 — 7/7 runnable gates PASS, AIDA64 parity gate deferred to AM5.

Cycle 1 close-out (2026-09-12): all 11 `branch/chunk-P1-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.
