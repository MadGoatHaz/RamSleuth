# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE5.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 4 (Phase 5: Packaging & Distribution + ryzen_smu uAPI reconciliation) COMPLETE + QA passed (328/328, clippy clean); compacted to MASTER_LOG. Packaging: AUR ramsleuth-git + ryzen-smu-dkms extra + systemd group install + GitHub Actions CI. ryzen_smu upstream = amkillam/ryzen_smu (main); daemon reads /sys/kernel/ryzen_smu_drv/pm_table. Ready for Cycle 5 (AMD ryzen_smu ground truth [module install now unblocked], Intel i5-6600 MCHBAR, model reconciliation, P1 L1/L2 refinement, MSRV decision, finalize GitHub push/tag on operator go-ahead).
