# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE5.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ HISTORY @@@
- [DONE] ID: P5-08 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-08-sysfs
DECISION: Corrected the AMD SMU PM-table sysfs path in amd_smu.rs to the verified kobject /sys/kernel/ryzen_smu_drv/pm_table (amkillam drv.c); legacy /sys/kernel/ryzen_smu kept as fallback candidate; full regression 328/328 + clippy clean.
AHEAD: Reviewer merges branch/chunk-p5-08-sysfs into v2-development; docs/packaging still cite the legacy path until a later chunk.

- [DONE] ID: P5-08-review | STATUS: SUCCESS | BRANCH: v2-development
DECISION: Reviewed and merged P5-08: canonical /sys/kernel/ryzen_smu_drv/pm_table tried first, legacy /sys/kernel/ryzen_smu fallback second, char-dev fallback unchanged, no-panic degradation preserved; full regression 328/328 debug + release, clippy -D warnings clean.
AHEAD: Next up is Cycle 5 (live-hardware verification, model reconciliation, P1 refinement, MSRV decision, finalize push/tag).

@@@ CURRENT_STATE @@@
P5-08 (AMD SMU sysfs PM-table path fix) REVIEWED + MERGED into v2-development (canonical /sys/kernel/ryzen_smu_drv/pm_table first, legacy fallback second, char-dev unchanged; 328/328 debug + release, clippy -D warnings clean, no-panic graceful degradation preserved: driver absent -> N/A (DriverMissing)). Prior: Cycle 4 COMPLETE + QA passed (327/327); packaging AUR + ryzen-smu-dkms + systemd + CI; ready for Cycle 5 (live-hardware verification, model reconciliation, P1 refinement, MSRV decision, finalize push/tag).
