# RamSleuth v2 — Master Log

Durable per-cycle compaction of `DEV_LOG.md`. Newest cycle first.

## Cycle 16 (documentation review/update, README v2.0 creation, gitignore Docs/ + plans/ + .kilo/ + untrack of the 21 tracked working docs, pre-push personal-info audit, operator-authorized push) — 2026-09-18 — COMPLETE (push pending C16-11)

### What was delivered
Cycle 16 (documentation review/update + README v2.0 + gitignore/untrack + PII audit + push) is COMPLETE: **all 8 implementation chunks merged into `v2-development` (C16-01..C16-08; C16-04..C16-07 as working-tree edits to now-untracked files) + C16-09 QA PASS + C16-10 this compaction** from baseline `8ee2f60` (the Cycle 15 compaction) to tip `12b8e22` — driven by the operator 5 instructions (verbatim scope in `plans/PLAN-CYCLE16.md` §1):

**Operator instructions (the cycle drivers):**
1. Review and update all documentation.
2. Rewrite the README for the new v2.0. — **FINDING: no root `README.md` existed** → the v2.0 README is **created**.
3. Add `Docs/` and `plans/` to `.gitignore`.
4. Review for personal info before pushing (the operator noreply email `111608787+MadGoatHaz@users.noreply.github.com`, the `MadGoatHaz` handle, and the project repo URL are **whitelisted**).
5. Push to GitHub (go-ahead **given** this cycle).

**Chunks (all merged; range `8ee2f60..12b8e22` = 274 commits over `origin/v2-development` @ `b908f7b`; zero source-code changes):**
- **C16-01** [CRITICAL-PATH] `.gitignore` + 21-file untrack (merge `a505fcc`) — `.gitignore` gains `Docs/` + `plans/` (the operator ask) + `.kilo/` (a reported safety addition: the agent state carries a full `origin/master` checkout, never committable); `git rm -r --cached Docs/ plans/` untracks the 21 tracked working docs (local files preserved, now gitignored). Zero source change.
- **C16-02** [ISOLATED] `README.md` (new, 142 lines, 12 sections) (merge `3886bac`) — created the root v2.0 README: title / what-it-is (the dual layer + privilege separation: one `CAP_SYS_RAWIO` daemon, 0660 Unix socket `/run/ramsleuth/ramsleuth.sock`, unprivileged clients) / features / the 7-crate architecture / the 6 binaries with key flags + exit codes / build (MSRV 1.75 + the GUI system-library set from `ci.yml`) / running / install (AUR `ramsleuth-git` + the optional `ryzen-smu-dkms` extra) / the AMD telemetry requirement / testing (556/556) / project layout / MIT license (the v2.0.0-development-line footer). Every claim grounded in the verified source; PII clean.
- **C16-03** [ISOLATED] `FULLSCOPEvsCOMPLETED.md` refresh (merge `3e8bed6`) — refreshed the most-stale doc to Cycle 15 ground truth: tip `8ee2f60` / 260-ahead, the GUI row → ✅ COMPLETE (Cycles 6–15), the ALL-8-ITEMS-CLOSED banner, O1 closed / O5 resolved / O6 in-progress (push this cycle) / O2 + OC open.
- **C16-04** [ISOLATED] `Docs/HANDOVER.md` — the 4 stale spots (the header audience/tip/260-ahead, §8 current state, L407 board line, §13 re-anchored to Cycle 16). **Working-tree edit of a now-untracked file — no commit (declared deviation, D-16.3).**
- **C16-05** [ISOLATED] `Docs/RamSleuth-v2.md` — the 5 status spots (Edition 2021; the Cycle-15 status line; 556/556; `KickOffPrompt.md` → `KickOff-Cycle4.md`; the 2026-09-12 snapshot annotated historical). **Working-tree edit of a now-untracked file — no commit (declared deviation).**
- **C16-06** [ISOLATED] `plans/PLAN-CYCLE8.md` — L16 PII fix `madgoat` → `the local OS user`. **Working-tree edit of a now-untracked file — no commit (declared deviation).**
- **C16-07** [ISOLATED] `Docs/KickOff-Cycle4.md` — L5 PII fix `/home/madgoat/...` → `the repository root (RamSleuth/)`. **Working-tree edit of a now-untracked file — no commit (declared deviation).**
- **C16-08** [ISOLATED] `packaging/ryzen-smu-dkms/dkms.conf` (merge `1945a71`) — the L32 `Purpose:` comment stale path `/sys/kernel/ryzen_smu/pm_table` → `/sys/kernel/ryzen_smu_drv/pm_table` (the P5-08 canonical kobject). Comment-only; all functional keys byte-identical.
- **C16-09** QA PASS @ `12b8e22` — full regression + static audit on the pre-push tip: scope clean (the diff vs base `8ee2f60` is exactly the 8 chunk files + the 21 untrack deletions + the bookkeeping — **zero `.rs` / `Cargo.*` / protocol / telemetry changes**), 556/556 debug + release (20 targets), clippy `--workspace --all-targets -- -D warnings` zero, 6 release binaries, and the **clean PII certificate** (see Quality).
- **C16-10** this compaction — record Cycle 16 in `MASTER_LOG.md`, reset `DEV_LOG.md`, capture the pre-push branch state + the C16-11 push plan.

**Notable incidents:**
1. **Post-untrack worktree hazard** — after C16-01 untracked the 21 docs, any checkout crossing the tracked→untracked boundary deletes those 21 docs from disk (git removes them on the switch). Handled via **backup-first + isolated worktrees** (each merge restored the local docs byte-identical; the C16-02 review confirmed 31/31 local docs intact). All 31 local docs intact at this compaction.
2. **C16-04..C16-07 target gitignored files** — their targets (`Docs/HANDOVER.md`, `Docs/RamSleuth-v2.md`, `plans/PLAN-CYCLE8.md`, `Docs/KickOff-Cycle4.md`) became untracked after C16-01, so the plan branch/commit prescriptions were void for them; they were applied as **working-tree edits** (declared deviations, recorded here).

**Process note (rebase conflict — bookkeeping-only, not a code defect):** C16-02 first review rebase onto the running tip conflicted only in the `DEV_LOG.md` lease region (its lease entries forked from the pre-merge tip); resolved keeping the rebase side state (C16-01 history preserved below). All merges verified clean; single-file purity confirmed.

### Key plan decisions
- **D-16.1 (gitignore + untrack):** `.gitignore` gains `Docs/` + `plans/` (the operator ask) + `.kilo/` (the reported safety addition — the agent state carries a full `origin/master` checkout and must never be committable) + the same-chunk `git rm -r --cached` untrack of the 21 files (one logical change; local files preserved).
- **D-16.2 (README is a creation, fully grounded):** no root README existed → created; every factual claim grounded in the verified source (no invented features); the workspace `version` stays `0.1.0` (the `v2.0.0` tag is a separate operator decision — not this cycle).
- **D-16.3 (surgical staleness updates):** `FULLSCOPEvsCOMPLETED.md` gets the full Cycle-15 refresh; `HANDOVER.md` the 4 stale spots only (its §3 per-cycle history preserved); `RamSleuth-v2.md` the status block only; the two PII fixes are single-line; `dkms.conf` the one-line comment. No change to the Grand Design spec / the two research notes / the `KickOff` body / `MASTER_LOG` (C16-10 only) / `packaging/README.md` / `ci.yml` / the systemd unit / `scripts/*` / any `Cargo.toml`.
- **D-16.4 (PII audit over the push-bound tree):** the push-bound `git ls-files` scan (`/home/`, `madgoat` case-insensitive, the email regex) + `git log --format=%ae %ce | sort -u`; the whitelist is exactly the operator noreply email, the `MadGoatHaz` handle, the project repo URL, and the six third-party upstream refs.
- **D-16.5 (push mechanics, C16-11):** strict fast-forward (a moved origin stops the cycle — **never force**), plain push, an ancestry-gated prune of the merged remote chunk refs (the live count re-measured, not asserted), verify `ls-remote` = 2 heads, no `v2.0.0` tag, `origin/master` untouched.

### Quality
- **556/556 tests green (debug AND release, whole workspace)** — the Cycle 15 count held (docs-only cycle, no tests added/removed); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps** (the `Cargo.toml`/`Cargo.lock` diff over `8ee2f60..12b8e22` is empty).
- **Scope audit = zero source-code changes** — the cycle diff vs base `8ee2f60` is exactly the 8 chunk files (`.gitignore`, `README.md` new, `FULLSCOPEvsCOMPLETED.md`, `Docs/HANDOVER.md`, `Docs/RamSleuth-v2.md`, `plans/PLAN-CYCLE8.md`, `Docs/KickOff-Cycle4.md`, `packaging/ryzen-smu-dkms/dkms.conf`) + the 21 untrack deletions + the bookkeeping — **no `.rs` / `Cargo.*` / protocol / telemetry / daemon / TUI / GUI / bench delta**.
- **PII certificate CLEAN** (C16-09, over all 82 tracked files): zero email addresses, zero personal home paths, zero phones, zero secrets; the only `/home/` survivor is the `crates/ramsleuth-gui/src/main.rs:1582` `"/home/x/ramsleuth-export-1.json"` generic test fixture (user `x` — the reviewed false-positive, kept); the only `madgoat` hits are the whitelisted `MadGoatHaz` handle / repo-URL forms. **Full git history: exactly one author + committer identity** — the operator noreply `111608787+MadGoatHaz@users.noreply.github.com` (526/526 at the QA-audit tip `7a4a740`; 527 at this compaction tip `12b8e22`) + the single name `MadGoatHaz`.
- **QA verdict: PASS** — C16-09 ran the full regression + the static audits on `12b8e22`; the branch is push-ready.

### Push state (operator gate) — C16-11 pending
Local `v2-development` tip = **`12b8e22`** (the C16-09 QA bookkeeping — the Cycle 16 pre-push tip; this compaction C16-10 commits on top of it, so the C16-11 push target is this compaction resulting sha, measured at push) — **unpushed, operator-gated** (this compaction performs no push). `origin/v2-development` = **`b908f7b`** (a strict ancestor of `12b8e22` — verified strict ff, **274 commits ahead**; re-measure at push). The remote currently holds **103 `branch/chunk-*` refs** to prune (99 legacy p5/c6–c14 + 4× C16: `c16-01` `8dfaa7f`, `c16-02` `61827a6`, `c16-03` `779b173`, `c16-08` `ae52b63`) — **C16-11 must re-measure the live count, not assert it**. `origin/master` = **`782022a`** (the divergent legacy Python-v1 line) — **NEVER touched** (not merged, not branched from, not deleted). **No `v2.0.0` tag** (not requested — a separate operator decision, recorded as open). The 21 untracked working docs are removed from the remote by the push — the explicit intent of the operator (instruction 3); they remain on disk locally, gitignored.

**The C16-11 push plan (strict ff, never force):**
1. `git fetch origin` → re-verify strict-ff ancestry (`git merge-base --is-ancestor origin/v2-development v2-development`); any non-ff → **stop and report**.
2. `git push origin v2-development` (plain; **NEVER force**).
3. **Prune gate:** for each live `branch/chunk-*` ref, `git merge-base --is-ancestor <sha> v2-development` — all must pass; the count is re-measured (not asserted); assert none is `master` / `v2-development`.
4. `git push origin --delete <the merged chunk refs>` (one generated command).
5. Verify `git ls-remote --heads origin` → exactly **2 refs** (`master`, `v2-development`).
6. **No `v2.0.0` tag**; `origin/master` **untouched**.
7. A final bookkeeping commit records the remote state (the push itself creates no commit).

### Open items carried
1. **Push to GitHub (C16-11, next)** — operator go-ahead given: strict ff to the post-compaction tip (**never force**) → the live-measured prune of the merged remote chunk refs → verify 2 heads → **no tag**; `origin/master` untouched.
2. **Operator live GUI runs (pending)** — `plans/CYCLE15-LIVE-CHECKLIST.md` + `plans/CYCLE14-LIVE-CHECKLIST.md` (now local-only files after C16-01; needs interactive sudo) + the remaining hardware-gated items (Intel MCHBAR decode — i5-6600; Domain A; MSRV 1.75 vs newer; AIDA64 parity gate).
3. **`v2.0.0` tag / workspace version bump** — separate operator decisions, not this cycle (the workspace version stays `0.1.0`).

Cycle 16 close-out (2026-09-18): this compaction (C16-10) recorded the Cycle 16 section in `MASTER_LOG.md`, reset `DEV_LOG.md` (base line → the C16 pre-push tip `12b8e22`, ACTIVE_WORKERS cleared, CURRENT_STATE = Cycle 16 COMPLETE + push-pending, the per-lease history archived), and captured the pre-push branch state + the C16-11 push plan. No push of `v2-development` (that is C16-11, landing on top of this commit).

## Cycle 15 (GUI layout fix: missing right borders + viewport clipping on right-column panels — symmetric 8 pt outer margin + widened 968×600/892×600 window + stroke-aware split) — 2026-09-18 — COMPLETE

### What was delivered
Cycle 15 (GUI layout fix) is COMPLETE: **all 2 code chunks merged into `v2-development` (C15-01, C15-02), C15-03 QA PASS, C15-04 this compaction** from baseline `4f0c6d9` (the Cycle 14 compacted state) to tip `035ad3e` — driven by the operator's defect report on the main-view right column:

**Operator defect report (the cycle driver):**
- **(DEFECT-1)** Panels **"2 · BENCHMARK ENGINE"** and **"3 · HARDWARE & SPD"** (the right column) are **missing their right-side cyan border strokes**.
- **(DEFECT-2)** The right-column frames **expand past / are clipped by the viewport right edge** (top / left / bottom borders intact).
- **(DEFECT-3)** **Asymmetry:** panel 1 ("1 · MEMORY CONTROLLER & SUBTIMINGS") has balanced outer padding and a fully enclosed 4-sided border; the right side lacks the matching outer margin.

**Root causes:**
- **Flush right edge** — the Cycle-13 "flush" split made `left_w + COLUMN_GAP + right_w == avail.x` exactly, so the right allocation ended exactly at the CentralPanel edge with zero trailing space.
- **Zero right margin in the budget** — the 960 pt window budget was `8 + 504 + 8 + 440` (0 pt right margin).
- **Centered stroke clipped** — the zone frames are 1.0 px CYAN strokes centered on the frame rect; half the stroke (0.5 px) paints outside the allocation, and with the right allocation flush at the panel edge that outer half lands in the clip rect and is clipped → the visually missing right border (panel 1 survives: its left edge sits at x = 8, so its outer half lands inside the panel); egui's automatic 8 pt row `item_spacing` was the residual gap that made the right edge flush.

**Chunks (all `--no-ff` merged; range `4f0c6d9..035ad3e` = 8 commits):**
- **C15-01** `crates/ramsleuth-gui/src/main.rs` — the fix: new `OUTER_MARGIN: f32 = 8.0` (the central panel's symmetric side margin), `DEFAULT_WINDOW_SIZE` `[960,600]` → `[968,600]`, `MIN_WINDOW_SIZE` `[884,600]` → `[892,600]`, the `render_zones` split computed against `inner = (avail.x − OUTER_MARGIN).max(0.0)` with the new exact-sum invariant `left_w + COLUMN_GAP + right_w + OUTER_MARGIN == avail.x`, a trailing `OUTER_MARGIN` after the right-column allocation, the row's automatic main-axis `item_spacing` zeroed (the injected 8 pt was the true root cause of the flush/clipped right edge) with the original spacing restored in both column closures, 6 prose sites 960→968, and the co-landed test re-anchored with the margin-symmetry (both margins == `OUTER_MARGIN`, ±1.0 pt) + 0.5 pt stroke-clearance pins (case 0 only) (89f174a → merge 6a378e4).
- **C15-02** `crates/ramsleuth-gui/src/telemetry_zone.rs` — docs-only: the 7 default-window doc sites reworded 960×600 → 968×600 (the 952 × 0.55 ≈ 524 → 504 arithmetic kept); no production change, no test-literal change (a9a8c5c → merge 035ad3e).

**Process note (rebase conflict — bookkeeping-only, not a code defect):**
- C15-02's first review rebase onto the merged C15-01 tip conflicted in `DEV_LOG.md` (its lease-board entries forked from the pre-merge tip); per protocol the reviewer did not force-resolve — rebase aborted, branch restored (REVIEW-C15-02 FAILED) — and the Conflict Resolution Specialist resolved the conflict as bookkeeping-only before the full review re-ran clean and merged. The branch's code (the telemetry_zone.rs docs commit) was never affected.

### Key plan decisions
- **D-15.1 (symmetric outer margin + an 8 pt-widened window):** the 8 pt right margin is bought by widening the window 960 → 968 (min 884 → 892) — every zone-level geometry pin stays byte-identical (at the 968 default: `inner = 952`, `left_w = 504`, `right_w = 440` — the content geometry is unchanged; the only visible delta is the new right margin + the restored right borders).
- **D-15.2 (zone frames unchanged):** all three zone frames keep their identical 1.0 px CYAN stroke + (10,6) inner margin + `set_min_width(available_width)` — "identical in stroke to panel 1" is met by identity; the defect was purely the allocation being flush at the clip edge (fixed in D-15.1) — so no production change in `status_zone.rs`, `bench_zone.rs`, or `telemetry_zone.rs` (the latter docs-only).
- **D-15.3 (full test re-anchoring survey):** every geometry pin enumerated — the only one needing a re-anchor was `render_zones_right_column_split_two_stacked_slices` (C15-01); all other main.rs / telemetry / status / bench tests verified unaffected (the 556 count held).

### Quality
- **556/556 tests green (debug AND release, whole workspace)** — the Cycle 14 count held (assertions added in place to one existing test; zero added, zero removed); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps** (`Cargo.toml`/`Cargo.lock` diff over `4f0c6d9..035ad3e` = empty).
- **Wire audit = zero protocol / telemetry / TUI / CLI / bench changes** — the cycle diff is exactly `main.rs` (the layout fix) + `telemetry_zone.rs` (docs-only) in `crates/ramsleuth-gui`; nothing outside `crates/ramsleuth-gui/` changes.
- **No-panic audit clean** (no new `unwrap` / `expect` / `panic!` in the changed code).
- **QA verdict: PASS** — C15-03 ran the full regression + audits on `035ad3e`; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`035ad3e`** (the C15-02 merge — the Cycle 15 tip; this compaction commits on top of it) — **unpushed, operator-gated** (this compaction performs no push of `v2-development`). The merged local `branch/chunk-c15-01` / `branch/chunk-c15-02` branches are deleted by this compaction, and their remote counterparts on origin are pruned by it as well (the operator-authorized prune of the two fully-merged C15 branches — strictly no force-push; `v2-development` itself stays local). On the operator's go-ahead: **fast-forward to the post-compaction tip (NEVER force-push) → prune the remaining remote chunk branches (p5/c6–c14) → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE15-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): visual confirmation of the border fix — panels 2 & 3 complete 4-sided CYAN borders identical to panel 1, symmetric 8 pt side margins, no right-column viewport clipping, the 892×600 min + 150% DPI, no regression (Graphs window, bench/burn-in, SPD cards, settings).
2. **Push to GitHub** — operator go-ahead (ff to the post-compaction tip, prune the remaining remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **The Cycle 14 / Cycle 13 open items carry** — the operator live GUI runs (`plans/CYCLE14-LIVE-CHECKLIST.md`, `plans/CYCLE13-LIVE-CHECKLIST.md`) and the remaining Cycle 12/11 items (Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; AIDA64 parity gate).

Cycle 15 close-out (2026-09-18): this compaction recorded the Cycle 15 section in `MASTER_LOG.md`, reset `DEV_LOG.md` (base line → `035ad3e`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived), refreshed `Docs/HANDOVER.md` (header + §2 + the Cycle 15 section), committed `plans/CYCLE15-LIVE-CHECKLIST.md`, and deleted the merged local `branch/chunk-c15-*` branches (their remote counterparts pruned per the operator's authorization). No push of `v2-development`.

## Cycle 14 (runtime bug fixes: clock first-sample warm-up+settle, non-blocking burn-in per-tick brief lock) — 2026-09-18 — COMPLETE

### What was delivered
Cycle 14 (runtime bug fixes) is COMPLETE: **all 3 chunks merged into `v2-development` (C14-01..C14-03)** from baseline `d60cd34` (the Cycle 13 compacted state) to tip `80b2374` — driven by the operator's two bug reports:

**Operator bug reports (the cycle drivers):**
- **(BUG-1)** "MCLK, UCLK, FCLK don't always report correctly when first starting the app" — intermittent, first-startup only.
- **(BUG-2)** "'Run Burn-in' locks up the app UI while it's running for the duration selected. Graphs lock up as well."

**Root causes:**
- **BUG-1** — the first PM-table read is LAZY (fires on the GUI's immediate first poll, <1 s after daemon boot, the least-settled moment), reads with ZERO settle time after the 250 ms spike, and the low idle value PASSES the 1–4096 MHz gate and gets CACHED for the full 2 s TTL (the GUI polls at exactly 2 s, so the visible first sample IS the unvalidated cold value).
- **BUG-2** — the GUI poller thread (a background std::thread) held the shared-state RwLock WRITE guard ACROSS THE ENTIRE bench/burn-in stream drain (update.rs `run_burn_in`/`run_bench` called with `&mut state.write().unwrap()`), starving the egui main thread's per-frame `state.read()` — freezing BOTH viewports (main + Graphs share one eframe event loop) for the whole run. "Run Full"/"Memory Only" shared the defect (just shorter).

**Chunks (all `--no-ff` merged; range `d60cd34..80b2374`):**
- **C14-01** first-read warm-up (BUG-1, M2) — `crates/ramsleuth-daemon/src/cache.rs`: a `warmed: bool` field; on a cold cache the collector is called TWICE, the first discarded as warm-up, the second (settled) value cached+returned, so the first served clock sample is settled; warm-path TTL logic unchanged (b6982a0 → merge 9903d86).
- **C14-02** post-spike settle delay (BUG-1, M1) — `crates/ramsleuth-daemon/src/main.rs`: a named const `CLOCK_SETTLE` = 150 ms sleep after the spike, before the PM-table read, giving the SMU time to move MCLK to the operating freq; spike params (256 MiB/250 ms) and TTL unchanged (a9c3362 → merge bfe8460).
- **C14-03** per-tick brief state lock (BUG-2, O1) — `crates/ramsleuth-gui/src/update.rs`: `run_bench`/`run_burn_in` now take the `Arc<RwLock<TelemetryData>>` and acquire the write lock ONLY briefly per tick (acquire→update→release) between `recv()` calls, so the lock is never held across the drain and the UI stays responsive during runs; + a new in-flight reader regression test (064b432 → merge 83b1b98).

**Process note (stranded commit + stale-parent rebase — process artifacts, not code defects):**
- C14-02's commit was briefly stranded on C14-01's branch (a worktree flip) and rescued; C14-02/C14-03 were rebased onto the current `v2-development` tip before merge (stale-parent artifacts, not code defects — all merges verified clean, single-file purity confirmed).

### Key plan decisions
- **D-1 (BUG-1, warm-up):** the first PM-table read is warmed — on a cold cache the collector runs twice (first discarded as warm-up, second settled and cached), so the first served clock sample is the settled operating frequency, never the <1 s idle cold value.
- **D-2 (BUG-1, settle):** a 150 ms settle delay (`CLOCK_SETTLE`) after the 250 ms DRAM spike, before the PM-table read, gives the SMU time to move MCLK to the operating frequency.
- **D-3 (BUG-2, brief lock):** the GUI poller acquires the shared-state write lock only per tick (acquire→update→release) between stream `recv()` calls — never held across the drain — so the egui main thread's per-frame read is not starved and both viewports (main + Graphs) stay responsive for the whole run.

### Quality
- **556/556 tests green (debug AND release, whole workspace)** — up from the 555 baseline (555 + 1 new in-flight reader regression test, C14-03); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps**; **no-panic clean**.
- **Wire audit = zero protocol / telemetry / TUI / CLI / bench changes** — the cycle diff is exactly the daemon `cache.rs` + `main.rs` and the GUI `update.rs` (no wire / telemetry / TUI / CLI / bench delta).
- **QA verdict: PASS** — all 3 chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`80b2374`** (Cycle 14 range `d60cd34..80b2374`) — **unpushed, operator-gated** (this compaction performs no push). The merged `branch/chunk-c14-*` chunk branches are the handover prune target (all fully merged into `v2-development`; tip `80b2374` untouched). On the operator's go-ahead: **fast-forward to `80b2374` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE14-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): clock first-sample correctness across 3-5 restarts, burn-in UI responsiveness ≥2 min, Run Full responsiveness, no regression — closes the PASS verdict.
2. **Push to GitHub** — operator go-ahead (ff to `80b2374`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **The Cycle 13 open items carry** — the operator live GUI run (`plans/CYCLE13-LIVE-CHECKLIST.md`) and the remaining Cycle 12/11 items (Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; AIDA64 parity gate).

Cycle 14 close-out (2026-09-18): this compaction recorded the Cycle 14 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `80b2374`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 13 (GUI LAYOUT refactoring: horizontal 2×2 SPD card grid + 3 equal-width column fill + window auto-size 960×600 + bench right-margin fill) — 2026-09-18 — COMPLETE

### What was delivered
Cycle 13 (GUI LAYOUT refactoring) is COMPLETE: **all 4 chunks merged into `v2-development` (C13-01..C13-04)** from baseline `d62b21f` (the Cycle 12 compaction) to tip `cae8064` — driven by the operator's directive:

**Operator directive (the cycle driver):**
- **GUI LAYOUT refactoring** — the three requirements:
  - **(R1)** Panel 3 (HARDWARE & SPD): the DIMM slot cards go horizontal side-by-side — a balanced 2×2 for up to 4 DIMMs — instead of the vertical stack.
  - **(R2)** Better horizontal space utilization — Panel 1 (MEMORY CONTROLLER) 3 columns fill the parent width (kill the dead void right of column 3) and Panels 2+3 stretch to the right margin uniformly.
  - **(R3)** Initial window auto-sizing — open tight around content (~960×600 default, ~884×600 min) instead of the old oversized 1400×900, respect DPI, no clipping of panels/footer/daemon socket path.

**Chunks (all `--no-ff` merged; range `d62b21f..cae8064`):**
- **C13-01** window auto-size + flush split (R3) — main.rs: `WINDOW_SIZE` renamed `DEFAULT_WINDOW_SIZE` `[960,600]` + new `MIN_WINDOW_SIZE` `[884,600]`, viewport `with_min_inner_size`, the right-column split math flushed so `left + gap + right == avail.x` exactly (55% ratio + MIN_LEFT_W/MIN_RIGHT_W clamp preserved, non-negative at the 884×600 minimum), co-landed test pin re-anchored to the constants (d75b159 → merge 14c8933).
- **C13-02** horizontal SPD card grid (R1) — status_zone.rs: `render_spd_cards` vertical stack → horizontal 2-column grid, side-by-side, 2+1 / balanced 2×2 flow for up to 4 DIMMs; the `render_spd_card` body unchanged (3af7787 → merge fc109d9).
- **C13-04** bench table right-margin fill (R2b) — bench_zone.rs: the bench TableBuilder's last metric column `initial(70)` → `Column::remainder()` (90 + 3×`initial(70)` + remainder) so the table fills the frame's full inner width — the right-border dead space gone (fa733bc → merge 4dc3a2a).
- **C13-03** 3 equal-width section columns (R2a) — telemetry_zone.rs: `render_section_grid` three natural-width columns → three equal-width top-aligned columns, each `(available − 2·gap)/3` (`3·col_w + 2·gap == available` exactly — the dead void right of column 3 gone), test pins re-anchored to the 960×600 default (ZONE_CONTENT_BUDGET 484, ZONE_W 504, ZONE_H 600), `SECTION_SPACING.y` 1→0 tightening (section titles now `.wrap(true)` at 960×600) (217651d → merge cae8064, tip).

**Process note (stale-parent rebase — process artifact, not a code defect):**
- C13-02/C13-04/C13-03 were initially forked from stale pre-merge parents (C13-02 from `d62b21f` pre-C13-01; C13-04 from the `d62b21f`-based review-FAIL state pre-C13-02; C13-03 from `fc109d9` pre-C13-04), so their first-pass diffs appeared to REVERT already-merged chunks (C13-02's main.rs carried the old 1400×900 / no-min-size; C13-04's status_zone.rs reverted the merged C13-02 grid; C13-03's bench_zone.rs reverted the merged C13-04 `remainder()`). Each was caught at the purity review, TRUE-rebased onto the current `v2-development` tip (gates re-run: 162/162 debug + release, clippy zero), re-reviewed PASS, then merged — the three 3-way merges verified clean.

### Key plan decisions
- **D-1 (R1):** Panel 3 DIMM slot cards = horizontal 2-column grid (2+1 / balanced 2×2 for up to 4 DIMMs); the card body untouched.
- **D-2 (R2):** Panels 2+3 stretch to the right margin uniformly — Panel 1's 3 section columns become equal-width top-aligned `(available − 2·gap)/3` (no dead void) and the bench table's last metric column becomes `Column::remainder()` (R2a + R2b).
- **D-3 (R3):** initial window auto-sizing — `DEFAULT_WINDOW_SIZE [960,600]` + `MIN_WINDOW_SIZE [884,600]` via `with_min_inner_size` (DPI-respected; no clipping of panels/footer/daemon socket path); the right-column split flushed so `left + gap + right == avail.x` exactly.

### Quality
- **555/555 tests green (debug AND release, whole workspace)** — held from the Cycle 12 baseline (no count change; the 6 layout pins re-anchored to 960×600); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps** (empty `Cargo.toml`/`Cargo.lock` diff over the cycle range); **no-panic clean** (the SPD 2-col grid is idx<n-guarded with the empty/all-Na placeholder fallback; the column-split clamp is non-negative at the 884×600 minimum).
- **Wire audit = zero protocol / telemetry / TUI / CLI / daemon / bench changes** — the GUI diff is exactly the 4 layout files (main.rs, telemetry_zone.rs, status_zone.rs, bench_zone.rs; 169 ins / 57 del).
- **QA verdict: PASS** — all 4 chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`cae8064`** (Cycle 13 range `d62b21f..cae8064`) — **unpushed, operator-gated** (this compaction performs no push). The merged `branch/chunk-c13-*` chunk branches are the handover prune target (all fully merged into `v2-development`; tip `cae8064` untouched). On the operator's go-ahead: **fast-forward to `cae8064` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE13-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the horizontal 2×2 SPD cards, the 3 equal-width columns filling the parent width (no dead void), the bench table filling the right margin, the tight 960×600 default (884×600 min, DPI-respected, no panel/footer/daemon-socket-path clipping) — closes the PASS verdict.
2. **Push to GitHub** — operator go-ahead (ff to `cae8064`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **The Cycle 12 open items carry** — the operator live GUI run (`plans/CYCLE12-LIVE-CHECKLIST.md`: the Vcore value + the VDDIO_MEM display) and the remaining Cycle 11 items (Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; AIDA64 parity gate).

Cycle 13 close-out (2026-09-18): this compaction recorded the Cycle 13 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `cae8064`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 12 (SMU Vcore additive wire field + board-profile VDDIO_MEM via NCT6798 in13) — 2026-09-18 — COMPLETE

### What was delivered
Cycle 12 (SMU Vcore additive wire field + board-profile VDDIO_MEM via NCT6798 in13) is COMPLETE: **all 5 chunks merged into `v2-development` (C12-01..C12-05)** from baseline `84f6036` (the Cycle 11 compaction) to tip `0003f6f` — driven by the operator's directive:

**Operator directive (the cycle driver):**
- Add the missing VRM-rail telemetry per the "Low-Level Hardware Telemetry & Register Gathering" doc, using **SMU direct telemetry (Priority 1, board-agnostic)** + a **DMI-keyed board profile database** for the Super I/O (graceful N/A fallback) — **not guessing raw ADC magnitudes**.

**The 2 rails added:**
1. **Vcore (VDDCR_VDD) — board-agnostic:** from the `ryzen_smu` PM table offset **0x0A0** (f32 volts — the read value, not the 0x09C setpoint, per `Docs/C12-RESEARCH-RYZEN-SMU.md`), carried as a **NEW ADDITIVE wire field** `VoltageSet.vcore_mv: Section<u16>` (the C8-01 `smu_version` precedent: appended, shape-checked, all 14 construction literals co-landed).
2. **VDDIO_MEM (DRAM):** from the **NCT6798 Super I/O in13** via the DMI-keyed board profile (the LibreHardwareMonitor Crosshair VIII Hero table: in13→DRAM, in0→Vcore, in6→SoC), filling the **EXISTING `vddio_mem_mv` slot** (zero-wire).

**Graceful-degradation rulings:**
- VPP / VDD_MISC (Chipset) unmapped by the LHM tables → N/A muted gray (by design).
- Unknown board / no nct6798 present → all-Na (no panic).
- hwmon unit is mV (driver-scaled — no re-application of LHM's 8 mV/count).
- Fill-when-Na only: an existing Value is never clobbered; AMD branch only.

**Chunks (all `--no-ff` merged; range `84f6036..0003f6f`):**
- **C12-01** wire-freeze — additive `VoltageSet.vcore_mv: Section<u16>` + `AmdPmVoltages.vcore_mv` from PM table 0x0A0 (f32 V ×1000 → mV, non-finite/negative → 0 → gate → Na) + all 14 `VoltageSet`/`AmdPmVoltages` construction literals co-landed in ONE commit (75cdce5 → merge 30e613a).
- **C12-02** GUI Vcore — the VDDCR_CPU row data-driven from the frozen `vcore_mv` field (GUI-local `GraphSample.vddcr_cpu_mv`; VDDCR_VDD prepended in voltage_rows) (c853ac9 → merge 318a76e).
- **C12-03** board_vrm.rs — the DMI-keyed profile database (PROFILES registry + CROSSHAIR_VIII_HERO: in13→VddioMem, in0→Vcore, in6→Soc) + the NCT6798 name-only hwmon binder (per-rail `in{N}_input` mV reads, gate bands, per-rail containment, all-Na fallback) (7ff5367 → merge c49d4fc).
- **C12-04** facade merge — fill-when-Na VDDIO_MEM from the board_vrm binder (AMD branch only; a carried Value is never clobbered; VPP/VDD_MISC untouched; zero-wire — the existing slot) (1228568 → merge f8c0194).
- **C12-05** GUI VDDIO_MEM display assertions — test-only pins in telemetry_zone.rs (the Value path renders volts, Na renders bare N/A) (14e507a → merge 46faf89).
- **Final tip `0003f6f`** — the C12-05 review lease sign-out (state update; the C12-06 QA close).

### Key plan decisions
- **D-1:** SMU direct telemetry is Priority 1 and board-agnostic — Vcore from the PM table 0x0A0 (the researched read value, not the 0x09C setpoint).
- **D-2:** Super I/O rails come from a DMI-keyed board profile database (the LibreHardwareMonitor table per board), never raw ADC guesses — the Crosshair VIII Hero: in13→DRAM, in0→Vcore, in6→SoC.
- **D-3:** additive-only wire change — exactly one new field, `VoltageSet.vcore_mv: Section<u16>` (the C8-01 `smu_version` precedent); VDDIO_MEM fills the existing slot (zero-wire).
- **D-4:** graceful degradation — unmapped rails (VPP/VDD_MISC) → N/A muted gray; unknown board / no nct6798 → all-Na; no panic.
- **D-5:** hwmon `in{N}_input` is already mV (driver-scaled) — no re-application of LHM's 8 mV/count scaling.
- **D-6:** fill-when-Na only — a carried Value is never clobbered; the AMD branch only.

### Quality
- **555/555 tests green (debug AND release, whole workspace)** — up from the 546 baseline (546 + 9 net-new C12 tests); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps** (empty `Cargo.toml`/`Cargo.lock` diff over the cycle range); **no-panic clean** (all telemetry + GUI production sections free of panic/unwrap/expect on hw data; board_vrm all-Na graceful).
- **Wire audit = exactly the additive `VoltageSet.vcore_mv` field** — protocol 0-diff; all 8 frozen wire structs byte-identical; `AmdPmVoltages.vcore_mv` (u16, internal, no serde), `BoardVrmReadout` (in-crate, no serde), and GUI-local `GraphSample.vddcr_cpu_mv` (f64, no serde) are all off-wire.
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 5 chunks merged; the operator live GUI run is the remaining manual gate.

### Live finding
- On the 5950X Hero host, **nct6798 in13 = 1376 mV (1.376 V)** — verified as a **Value** by the C12-03 worker's live host-tolerant test (the DRAM rail now carries data, not N/A).

### Push state (operator gate)
Local `v2-development` tip = **`0003f6f`** (Cycle 12 range `84f6036..0003f6f`; `origin/v2-development` still `b908f7b`) — **unpushed, operator-gated** (this compaction performs no push). The 5 merged `branch/chunk-c12-*` chunk branches were pruned during the cycle (none remain local; all fully merged into `v2-development`). On the operator's go-ahead: **fast-forward to `0003f6f` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE12-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the 5 live checks (a–e) for the Vcore value + the VDDIO_MEM display — closes the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (ff to `0003f6f`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. **Domain A follow-up (NOT in this cycle — a future cycle)** — the SMN CAD/ProcODT/RTT PHY registers (root, UMC-space `0x00050000`/`0x00051000`/`0x0001B000`; decode-table reconciliation; active-rank indexing).
4. The remaining Cycle 11 open items carry as listed in the Cycle 11 section (Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; AIDA64 parity gate).

Cycle 12 close-out (2026-09-18): this compaction recorded the Cycle 12 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `0003f6f`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 11 (v2.0.0 DIMM identity: revert C10 density paint + fix DDR4 rank decode to JESD79-4 bits 3:4) — 2026-09-17 — COMPLETE

### What was delivered
Cycle 11 (v2.0.0 DIMM identity: revert C10 density paint + fix DDR4 rank decode to JESD79-4 bits 3:4) is COMPLETE: **all 2 chunks merged into `v2-development` (C11-01+C11-02, one atomic `--no-ff` merge)** from baseline `a7bf7bb` (the Cycle 10 compaction) to tip `d170a3f` — driven by the operator's DIMM-identity pushback:

**Operator pushback (the cycle driver):**
- "We need to make sure we are identifying the DIMMs correctly and not just painting a value or guessing." — Cycle 10 had painted the density `0x0D` → 32 Gb to make the total capacity work out, but that only reconciled under a wrong rank-1 × 8-device decode; the operator confirmed the DIMMs are **dual-rank**.

**The fix (C11-01, two coupled parts):**
1. **Revert the C10 density paint** — density `0x0D` → **16 Gb** (the P6-04 value restored). 16 Gb per die is correct for a 32 GiB dual-rank DIMM: **16 Gb × 16 devices = 32 GiB**.
2. **Fix the DDR4 rank decode** — read **bits 3:4 of byte 0x0C** (the JESD79-4 "Number of Ranks" field: 00=1 / 01=2 / 10=3 / 11=4) instead of bit 1 (which was actually part of the width field). For the live `0x0C = 0x09`: `(0x09 >> 3) & 0x03 = 1` → **rank 2 (dual-rank)** (and width bits 2:0 = 1 → x8).

**Why this is correct (not a guess):**
- The live SPD bytes were **read directly from the EEPROMs** — `0x0C = 0x09`, `0x13 = 0x0D`, both DIMMs identical.
- The JESD79-4 byte 0x0C layout (width bits 2:0, rank bits 3:4) was **verified against 2 working decoders + JESD79-4** (HIGH confidence).
- `0x09` decodes to **(x8, 2 ranks)**, matching the operator's confirmed dual-rank truth; **16 Gb × 16 devices = 32 GiB** per DIMM matches the 64 GiB total (2x32 GiB kit).

**Chunks (one atomic `--no-ff` merge):**
- **C11-01** rank decode + density revert — DDR4 rank decode reads byte 0x0C bits 3:4 (not bit 1); density `0x0D` reverted from 32 Gb to 16 Gb (the P6-04 value); 6 doc sites reworded; telemetry-only (81245f2).
- **C11-02** test re-anchoring — the 7 pinned tests + the live fixture doc in `spd_decode.rs` re-anchored to rank 2 / 16 devices / 16 Gb (16384 Mbit); telemetry-only (f781b5a).
- **Atomic merge** — C11-01 + C11-02 landed as one `--no-ff` merge **d170a3f** (the decode fix and its test re-anchoring are inseparable; c11-01/c11-02 branches pruned).

### Key plan decisions
- **D-1:** density `0x0D` → **16 Gb** (the P6-04 value restored) — the C10 32 Gb paint is reverted; 16 Gb × 16 devices = 32 GiB per dual-rank DIMM.
- **D-2:** DDR4 rank = **byte 0x0C bits 3:4** (JESD79-4 "Number of Ranks": 00=1 / 01=2 / 10=3 / 11=4), not bit 1 (a width-field bit); width = bits 2:0 (live `0x09` → x8, rank 2).
- **D-3:** identity over capacity-math — the decode is anchored to the live EEPROM bytes + the verified JESD79-4 field layout (2 working decoders, HIGH confidence), never painted to make the total work out.

### Quality
- **546/546 tests green (debug AND release, whole workspace)** — held from the Cycle 10 close (the 7 C11-02 tests re-anchored in place, no count change); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps**; **zero-wire audit clean** (only `crates/ramsleuth-telemetry/src/spd_decode.rs` changed vs `a7bf7bb`; frozen structs / serde shapes byte-identical; protocol + all other crate diffs empty).
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — both chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`d170a3f`** (Cycle 11 range `a7bf7bb..d170a3f`; `origin/v2-development` still `b908f7b`) — **unpushed, operator-gated** (this compaction performs no push). The c11-01/c11-02 chunk branches were pruned at the atomic merge (all fully merged into `v2-development`; tip `d170a3f` untouched). On the operator's go-ahead: **fast-forward to `d170a3f` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE11-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the header RAM line showing 2x32 GiB **Dual-Rank** (64 GiB kit) with no slot note, the 16 Gb / 16 devices / 16384 Mbit decode, and the unchanged MCLK — closes the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (ff to `d170a3f`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. The remaining Cycle 10 open items carry as listed in the Cycle 10 section (the Cycle 10 live GUI run — superseded by the Cycle 11 live run above; Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; CAD/RTT/drive SMN bitfields; AIDA64 parity gate).

Cycle 11 close-out (2026-09-17): this compaction recorded the Cycle 11 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `d170a3f`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

## Cycle 10 (v2.0.0 density 0x0D→32Gb + daemon DRAM-spike-before-MCLK-read) — 2026-09-17 — COMPLETE

### What was delivered
Cycle 10 (v2.0.0 density 0x0D→32Gb + daemon DRAM-spike-before-MCLK-read) is COMPLETE: **all 4 chunks merged into `v2-development` (C10-01…C10-04)** from baseline `f4ee93b` (the Cycle 9 compaction) to tip `5610e67` — the two operator tasks:

**Operator task 1 — RAM density misdecode fix:**
- The header RAM line showed "2x16 GiB" but the operator-confirmed truth is **2x32 GiB = 64 GiB total**. Root cause: the P6-04 0x0D → 16 Gb special case was wrong — it is actually **32 Gb per die**; the `decode_density` arm re-anchored with the live-module tests re-anchored to match (D-1, C10-01 + C10-02).

**Operator task 2 — daemon DRAM-spike before MCLK re-read:**
- Live MCLK reads low when the DIMMs sit in a low-power idle state; the daemon now spikes ~250 ms of DRAM load before each re-collect so the SMU PM `MCLK` reads the operating frequency (D-2/D-3, C10-03 + C10-04). The GUI "spiking" indicator (C10-05) was reviewed and documented no-op — it would have been a wire change; the effect rides the existing MCLK field (D-5).

**Chunks (all `--no-ff` merged):**
- **C10-01** density decode — `decode_density` 0x0D arm 16→32 Gb (32 GiB DIMM) + the P6-04 comment/doc reworded to the C10 reconciliation; telemetry-only, fixture bytes unchanged (8ac43e6).
- **C10-02** test re-anchoring — the 5 live-module spots in `spd_decode.rs` tests re-anchored to 32 Gb / 32768 Mbit / 32 GiB (fixture doc "64 GiB kit (2x32 GiB)"); telemetry-only (60f3345).
- **C10-03** spike module — new `crates/ramsleuth-daemon/src/dram_spike.rs` (256 MiB buffer, 250 ms duration, single-flight `AtomicBool` gate + RAII `RunningGuard`, strided read/write loop with a `black_box` sink, std-only surface, 5 tests) + 3-line `lib.rs` registration (5e89c7e).
- **C10-04** collector wrap — the `main.rs` `TelemetryCache` collector wrapped `|| { spike(); ramsleuth_telemetry::collect() }` (the D-2 hook at the collector boundary, not a pre-lock call in rpc.rs); every re-collect runs `spike()` first; `rpc.rs`/`cache.rs`/CLI untouched (842adba).
- **Wave 1 atomic merge** — C10-01 + C10-02 landed as one `--no-ff` merge **5ddec39** (c10-01/c10-02 branches pruned; the decode fix and its test re-anchoring are inseparable).
- **Wave 2 chain merge** — C10-03 + C10-04 landed as one `--no-ff` merge **0d86359** (c10-03 pruned with c10-04, c10-04 worktree removed; the two waves file-disjoint, no rebase).

### Key plan decisions
- **D-1:** density 0x0D → **32 Gb** (32 GiB DIMM, 2x32=64 GiB kit) — the P6-04 "2x16 GiB kit" assumption from the part number was wrong; operator-confirmed 2x32 GiB.
- **D-2:** the spike hook is the collector wrap in `main.rs` (`|| { spike(); collect() }` at the `TelemetryCache::new` site), NOT a pre-lock call in rpc.rs:207.
- **D-3:** the spike module (`dram_spike.rs`) = 256 MiB buffer, 250 ms duration, single-flight `AtomicBool` gate + RAII `RunningGuard`, std-only surface.
- **D-4:** ZERO-WIRE — the density fix changes a decoded value, not the wire format; the spike is daemon-internal; protocol + telemetry wire shapes byte-identical.
- **D-5:** no GUI change — C10-05 (the "spiking" indicator) documented no-op; a "spiking" indicator would be a wire change, so the effect rides the existing MCLK field.

### Quality
- **546/546 tests green (debug AND release, whole workspace)** — up from 541 at the Cycle 9 close (541 baseline + 5 spike tests); **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); **MSRV 1.75** held; **6 release binaries** build; **zero new deps**; **zero-wire audit clean** (D-4: no protocol / `messages.rs` shape delta).
- **QA verdict: PASS-WITH-MANUAL-LIVE-VERIFY** — all 4 chunks merged; the operator live GUI run is the remaining manual gate.

### Push state (operator gate)
Local `v2-development` tip = **`5610e67`** (Cycle 10 range `f4ee93b..5610e67`; `origin/v2-development` still `b908f7b`) — **unpushed, operator-gated** (this compaction performs no push). The 4 merged `branch/chunk-c10-*` chunk branches are the handover prune target (all fully merged into `v2-development`; tip `5610e67` untouched). On the operator's go-ahead: **fast-forward to `5610e67` (NEVER force-push) → prune the remote chunk branches → optional `v2.0.0` tag**.

### Open items carried
1. **Operator live GUI run (pending)** — `plans/CYCLE10-LIVE-CHECKLIST.md` (5950X host; needs interactive sudo): the header RAM line now showing 2x32 GiB (64 GiB kit) + MCLK reading the operating frequency after the pre-collect spike — closes the PASS-WITH-MANUAL-LIVE-VERIFY verdict.
2. **Push to GitHub** — operator go-ahead (ff to `5610e67`, prune the remote chunk branches, optional `v2.0.0` tag; strictly no force-push, no pre-go-ahead remote mutation).
3. The remaining Cycle 9 open items carry as listed in the Cycle 9 section (the Cycle 9 live GUI run; Intel MCHBAR decode — hardware-gated; MSRV 1.75 vs newer; CAD/RTT/drive SMN bitfields; AIDA64 parity gate).

Cycle 10 close-out (2026-09-17): this compaction recorded the Cycle 10 section in `MASTER_LOG.md` and reset `DEV_LOG.md` (base line → `5610e67`, ACTIVE_WORKERS cleared, CURRENT_STATE = COMPLETE, the per-lease history archived). No commit and no push (the commit lands in a separate follow-up subtask).

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
