# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 9ea4b3b (clean; 515/515 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed). Plan: `plans/PLAN-CYCLE8.md` (Cycle 8 — v2.0.0 header truth: AGESA/SMU honest relabel, RAM total/per-slot, N/A gray styling, GEAR_DOWN split; the Cycle 7 plan is retained as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
- [ACTIVE] ID: C8-01 | AGENT: general (Implementation Agent) | BRANCH: branch/chunk-c8-01 | FILES: crates/ramsleuth-telemetry/src/platform.rs, facade.rs, GUI update.rs/telemetry_zone.rs/graph.rs/main.rs/status_zone.rs/style.rs, TUI main.rs/ui.rs, client client.rs/commands.rs/dump.rs (test literals)

@@@ CURRENT_STATE @@@
Cycle 8 (v2.0.0 header truth: AGESA/SMU relabel, RAM capacity, N/A styling, GEAR_DOWN split) PLANNED — 11 chunks (C8-01…C8-11, 2 waves), awaiting implementation. C8-01 (the smu_version wire freeze) is the [CRITICAL-PATH] gate; live evidence in the plan §1 (DMI table 0400 root → no unprivileged real AGESA; live SPD hub 0x80=0x11/0x0C=0x09 → 16 GiB per slot; MemTotal 62.68 GiB).

- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
