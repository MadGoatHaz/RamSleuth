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

- [DONE] ID: P5-09-review | STATUS: SUCCESS | BRANCH: v2-development
DECISION: Reviewed and merged P5-09: install-ryzen-smu-dkms.sh now defaults to verified amkillam/ryzen_smu.git (RYZEN_SMU_URL override kept); fast-path + final verify check canonical /sys/kernel/ryzen_smu_drv/pm_table primary with legacy /sys/kernel/ryzen_smu/pm_table fallback (matches P5-08 daemon); clone flow, dkms.conf fallback, dkms add/install, modprobe+modules-load.d and guarded die paths unchanged; bash -n + shellcheck clean; only .sh + DEV_LOG.md touched, no source change so cargo test skipped.
AHEAD: Branch branch/chunk-p5-09-script deleted after merge; next: Cycle 5 (live-hardware verification, model reconciliation, P1 refinement, MSRV decision, finalize push/tag).

- [DONE] ID: P5-10 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-10-readme
DECISION: packaging/README.md now cites verified upstream amkillam/ryzen_smu (branch main, v0.1.7) and canonical /sys/kernel/ryzen_smu_drv/pm_table in the verify step, plus a brief monitor_cpu ground-truth note (clocks ±1 MHz, voltages ±10 mV, CAD/subtimings); install/group/systemd/CI sections untouched; markdown fences/headers well-formed, no stale 53XU or non-_drv path references remain.
AHEAD: Reviewer merges branch/chunk-p5-10-readme into v2-development; docs/packaging README now matches P5-08/P5-09 corrected reality, unblocks Cycle 5 live-hardware verification.

- [DONE] ID: P5-10-review | STATUS: SUCCESS | BRANCH: v2-development
DECISION: Reviewed and merged P5-10: packaging/README.md cites verified upstream amkillam/ryzen_smu (branch main, v0.1.7) and canonical /sys/kernel/ryzen_smu_drv/pm_table in the verify step plus monitor_cpu ground-truth note; install/group/systemd/CI sections intact; markdown well-formed, zero stale 53XU or non-_drv path references; only README + DEV_LOG touched, no source change so cargo test skipped.
AHEAD: Branch branch/chunk-p5-10-readme deleted after merge; P5-08/P5-09/P5-10 ryzen_smu uAPI reconciliation merged; ready for QA re-audit.

@@@ CURRENT_STATE @@@
P5-08/P5-09/P5-10 ryzen_smu uAPI reconciliation merged into v2-development (P5-08: daemon canonical /sys/kernel/ryzen_smu_drv/pm_table first + legacy fallback; P5-09: install script amkillam/ryzen_smu v0.1.7 upstream + _drv path; P5-10: README amkillam upstream + _drv verify path + monitor_cpu note; 328/328 debug+release, clippy -D warnings clean, no-panic degradation; all three review branches deleted after merge); ready for QA re-audit; then Cycle 5 (live-hardware verification, model reconciliation, P1 refinement, MSRV decision, finalize push/tag).
