# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 9ea4b3b (clean; 515/515 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed). Plan: `plans/PLAN-CYCLE8.md` (Cycle 8 — v2.0.0 header truth: AGESA/SMU honest relabel, RAM total/per-slot, N/A gray styling, GEAR_DOWN split; the Cycle 7 plan is retained as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
- [ACTIVE] ID: C8-01-REVIEW | AGENT: general (Code Reviewer) | BRANCH: branch/chunk-c8-01 | FILES: DEV_LOG.md (lease record only)

@@@ CURRENT_STATE @@@
Cycle 8 (v2.0.0 header truth) IN FLIGHT — C8-01 (wire freeze) implemented + pushed f1c039d on branch/chunk-c8-01, 516/516 green debug+release, clippy zero, MSRV 1.75 held; awaiting review/merge. Next: C8-02 (spd_decode byte-0x02 classification). C8-01 co-landed the smu_version literal in 4 files the plan list missed (daemon/rpc.rs, daemon/cache.rs, protocol/messages.rs, telemetry/main.rs) — all mechanical one-line test literals; wave-2 chunks touching the 13 listed files rebase onto f1c039d.

- [DONE] ID: C8-01 | STATUS: SUCCESS | BRANCH: branch/chunk-c8-01 (f1c039d, pushed origin)
DECISION: wire freeze landed — SystemPlatform += smu_version (appended, shape-checked, agesa narrowed to BIOS-string-only); 27 test literals + 2 facade asserts co-landed across 16 files (incl. 4 unlisted daemon/protocol/telemetry-main); 516/516 debug+release, clippy zero.
AHEAD: wave-2 chunks (C8-11 main.rs, C8-06/C8-10 telemetry_zone.rs, C8-07 status_zone.rs, C8-09 graph.rs, C8-04 facade.rs) must rebase onto f1c039d — their SystemPlatform literals now require the smu_version line.

- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
