# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 6cc9840 (clean; 527/527 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed; operator-gated). Plan: `plans/PLAN-CYCLE9.md` (Cycle 9 — v2.0.0 RAM topology header + slot note, Graphs telemetry lifecycle + in-window poll-interval control, CPU-temp k10temp/zenpower hwmon source, right-column width/height fill; the Cycle 8 plan is retained as `plans/PLAN-CYCLE8.md`, Cycle 7 as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)
- [ACTIVE] ID: C9-01 | AGENT: general (implementer) | BRANCH: branch/chunk-c9-01 | FILES: crates/ramsleuth-gui/src/main.rs
@@@ CURRENT_STATE @@@
Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) IN PROGRESS — C9-06 implemented + pushed (branch/chunk-c9-06 @ 0a24c30: bench zone D-4a width-fill `set_min_width(available_width)` + the width-fill test, 528/528 debug+release, clippy zero, MSRV 1.75, zero new deps, no manifest diff), awaiting review; C9-01 (main.rs) in flight (parallel per the briefing); zero wire changes (D-1: the rank is already on the wire via `SpdModule.rank`, C8-03 — the header joins it GUI-locally); the main.rs 4-way chain (C9-01→C9-02→C9-03→C9-05) + the graph.rs 2-way (C9-03→C9-04) are strictly sequenced, the three zone files (C9-06/07/08) are disjoint + parallel-safe after C9-05.

**C9-06 — [DONE] 2026-09-17, general (implementer)** — branch/chunk-c9-06 @ 0a24c30 pushed (feat 0a24c30 + this sign-out).
DECISION: bench frame closure gains `ui.set_min_width(ui.available_width())` as its first line (D-4a width-fill) + new test `render_bench_zone_frame_fills_available_width` (bounded panel, frame allocation == available width); 528/528 debug+release, clippy `-D warnings` zero, no manifest diff.
AHEAD: reviewer — rebase onto the v2-development tip before review (house rule; C9-05 lands before the zone group); live-check: the `2 · BENCHMARK ENGINE` border spans the full right-column width, matching the status panel.

- [ARCHIVED] Cycle 8 (v2.0.0 header truth) — 11 chunks C8-01..C8-11 merged 2026-09-16 (c6ef83d..5730b33); AGESA/SMU provenance relabel + RAM capacity + N/A bare/gray styling + gear-metric split; QA PASS-WITH-MANUAL-LIVE-VERIFY 527/527, clippy zero, 6 binaries, zero new deps; see MASTER_LOG.md Cycle 8 section + plans/PLAN-CYCLE8.md.
- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
