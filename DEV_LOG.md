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

- [DONE] ID: P5-09-script | STATUS: SUCCESS | BRANCH: branch/chunk-p5-09-script
DECISION: install-ryzen-smu-dkms.sh now defaults to VERIFIED amkillam/ryzen_smu (branch main, v0.1.7) with RYZEN_SMU_URL override kept; sysfs checks use single PM_TABLE /sys/kernel/ryzen_smu_drv/pm_table (canonical first) with legacy /sys/kernel/ryzen_smu fallback; clone flow, dkms.conf fallback, dkms add/install, modprobe + modules-load.d and guarded failure paths unchanged; bash -n + shellcheck clean.
AHEAD: Reviewer merges branch/chunk-p5-09-script into v2-development; docs/packaging still cite the legacy path until a later chunk.

@@@ CURRENT_STATE @@@
P5-09 (install-script fix: amkillam/ryzen_smu verified upstream + ryzen_smu_drv canonical sysfs path with legacy fallback) IMPLEMENTED on branch/chunk-p5-09-script, pushed, pending review/merge (bash -n + shellcheck clean; script idempotent/non-destructive, clone+dkms+modprobe flow unchanged). Base: P5-08 merged into v2-development (canonical /sys/kernel/ryzen_smu_drv/pm_table first, legacy fallback second, char-dev unchanged; 328/328 debug + release, clippy -D warnings clean, no-panic graceful degradation: driver absent -> N/A (DriverMissing)); packaging AUR + ryzen-smu-dkms + systemd + CI; next: review P5-09, then Cycle 5 (live-hardware verification, model reconciliation, P1 refinement, MSRV decision, finalize push/tag).
