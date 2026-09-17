# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 5610e67 (Cycle 10 compacted; 546/546 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed; operator-gated). Plan: `plans/PLAN-CYCLE10.md` (Cycle 10 — 5950X density misdecode 0x0D→32 Gb + daemon DRAM-spike-before-MCLK-read; the Cycle 9 plan is retained as `plans/PLAN-CYCLE9.md`, Cycle 8 as `plans/PLAN-CYCLE8.md`, Cycle 7 as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 10 (v2.0.0 density 0x0D→32Gb + daemon DRAM-spike-before-MCLK-read) COMPLETE — 4 chunks merged (C10-01..C10-04), QA PASS-WITH-MANUAL-LIVE-VERIFY (546/546, clippy zero, 6 binaries, zero-wire); compacted to MASTER_LOG; v2-development @ 5610e67 unpushed (operator-gated); operator live GUI run pending.

- [ARCHIVED] Cycle 10 (v2.0.0 density 0x0D→32Gb + daemon DRAM-spike-before-MCLK-read) — 4 chunks C10-01..C10-04 merged 2026-09-17 (f4ee93b..5610e67; Wave 1 atomic --no-ff 5ddec39 = C10-01+C10-02, Wave 2 chain --no-ff 0d86359 = C10-03+C10-04); density 0x0D 16→32 Gb (32 GiB DIMM, 2x32=64 GiB kit; 5 test spots re-anchored) + daemon dram_spike module (256 MiB / 250 ms single-flight, std-only) + main.rs collector wrap (spike before each MCLK re-read); QA PASS-WITH-MANUAL-LIVE-VERIFY 546/546, clippy zero, 6 binaries, zero-wire; see MASTER_LOG.md Cycle 10 section + plans/PLAN-CYCLE10.md.
- [ARCHIVED] Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) — 8 chunks C9-01..C9-08 merged 2026-09-16 (6cc9840..c2f40a5); RAM topology header (rank word + slot note, no wire change) + Graphs telemetry lifecycle (force-on open / revert close) + in-window Poll control + CPU-temp hwmon (k10temp/zenpower name-match) + right-column width/height fill; QA PASS-WITH-MANUAL-LIVE-VERIFY 541/541, clippy zero, 6 binaries, zero-wire; see MASTER_LOG.md Cycle 9 section + plans/PLAN-CYCLE9.md.
- [ARCHIVED] Cycle 8 (v2.0.0 header truth) — 11 chunks C8-01..C8-11 merged 2026-09-16 (c6ef83d..5730b33); AGESA/SMU provenance relabel + RAM capacity + N/A bare/gray styling + gear-metric split; QA PASS-WITH-MANUAL-LIVE-VERIFY 527/527, clippy zero, 6 binaries, zero new deps; see MASTER_LOG.md Cycle 8 section + plans/PLAN-CYCLE8.md.
- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
