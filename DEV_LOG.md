# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 9ed99f5 (clean; 530/530 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed; operator-gated). Plan: `plans/PLAN-CYCLE9.md` (Cycle 9 — v2.0.0 RAM topology header + slot note, Graphs telemetry lifecycle + in-window poll-interval control, CPU-temp k10temp/zenpower hwmon source, right-column width/height fill; the Cycle 8 plan is retained as `plans/PLAN-CYCLE8.md`, Cycle 7 as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)
@@@ CURRENT_STATE @@@
Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) IN PROGRESS — C9-01 reviewed & merged (v2-development @ 66c7246; 529/529 debug+release, clippy zero); C9-06 reviewed & merged (v2-development @ 9ed99f5; 528/528 on-branch, 530/530 post-merge, clippy zero); C9-08 implemented & pushed (branch/chunk-c9-08 @ ab529bf — awaiting review/merge); C9-02 in flight (worktree branch/chunk-c9-02, forked onto 66c7246), then C9-03 (graph.rs + main.rs co-land), C9-04 (graph.rs), C9-05 (main.rs), C9-07 (zone file); zero wire changes (D-1: the rank is already on the wire via `SpdModule.rank`, C8-03 — the header joins it GUI-locally).

- [DONE] ID: C9-06-REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-c9-06 @ 0a24c30 merged --no-ff into v2-development @ 9ed99f5 (528/528 on-branch; 530/530 post-merge debug+release; clippy --all-targets zero; audit: bench_zone.rs only +46, no Cargo.toml/Cargo.lock diff, MSRV 1.75)
DECISION: frame closure gains `ui.set_min_width(ui.available_width())` as its first line (D-4a width-fill) + new bounded-panel test (frame allocation == available width); clean 3-way merge — main.rs retains C9-01, no reverts; worktree already pruned.
AHEAD: v2-development tip is 9ed99f5 (unpushed, operator-gated); C9-08 (telemetry_zone.rs) is disjoint and mergeable in parallel; C9-07 forks from here.

- [DONE] ID: C9-01-REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-c9-01 @ 3717434 merged --no-ff into v2-development @ 66c7246 (529/529 debug+release; clippy --all-targets zero; D-1 frozen-shape audit: main.rs only +252/-22, no wire/deps diff, worktree pruned)
DECISION: `dimm_summary(&[Section<f64>], &[SpdModule], &Units)` groups by (size, rank word) → `2x16 GiB Single-Rank` (Na/0 omits the word); private `rank_word` + `slot_note` render `2 of 4 slots SPD-visible` when MemTotal > SPD sum; live-host line matches §7(a).
AHEAD: v2-development tip is 66c7246; C9-02 forks from here (same main.rs, strictly after C9-01); C9-06 review is independent (bench_zone.rs).
- [DONE] ID: C9-08 | STATUS: SUCCESS | BRANCH: branch/chunk-c9-08 @ ab529bf (pushed; gates: check clean, clippy zero, 528/528 debug+release)
DECISION: `render_telemetry_zone` frame closure gains `ui.set_min_width(ui.available_width())` as first line (D-4a) → zone 1's border fills the full left column (previously shrank to the grid's natural width, ≈469 of 584 pt headless); telemetry_zone.rs only (+63), new test (m) asserts the painted CYAN-stroked border == available width (populated + placeholder).
AHEAD: reviewer — disjoint from main.rs/graph.rs, clean merge onto 66c7246; C9-07 (status fill) parallel-safe; live: zone 1 border full-width, balanced vs the two right panels.

- [DONE] ID: C9-01 | STATUS: SUCCESS | BRANCH: branch/chunk-c9-01 @ 3717434 (pushed; gates: check clean, clippy zero, 529/529 debug+release)
DECISION: `dimm_summary` gains the parallel `&[SpdModule]` and groups by `(size, rank word)` (e.g. `2x16 GiB Single-Rank`; Na/0 omits the word); new private `rank_word` + `slot_note`; `ram_line_prefix` appends the total-vs-breakdown note (`2 of 4 slots SPD-visible`) when MemTotal > SPD sum; main.rs only (+252/-22).
AHEAD: C9-02 (same main.rs, lands after) — fork/rebase onto this merge; `dimm_summary` is now 3-arg; `rank_word`/`slot_note` are private to main.rs.

- [ARCHIVED] Cycle 8 (v2.0.0 header truth) — 11 chunks C8-01..C8-11 merged 2026-09-16 (c6ef83d..5730b33); AGESA/SMU provenance relabel + RAM capacity + N/A bare/gray styling + gear-metric split; QA PASS-WITH-MANUAL-LIVE-VERIFY 527/527, clippy zero, 6 binaries, zero new deps; see MASTER_LOG.md Cycle 8 section + plans/PLAN-CYCLE8.md.
- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
