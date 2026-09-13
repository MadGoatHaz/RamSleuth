# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE5.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 4 (Phase 5: Packaging & Distribution + ryzen_smu uAPI reconciliation) COMPLETE + QA passed (328/328, clippy clean); compacted to MASTER_LOG. Packaging: AUR ramsleuth-git + ryzen-smu-dkms extra + systemd group install + GitHub Actions CI. ryzen_smu upstream = amkillam/ryzen_smu (main); daemon reads /sys/kernel/ryzen_smu_drv/pm_table. Ready for Cycle 5 (AMD ryzen_smu ground truth [P5-11 MERGED: staging-before-add + dkms add module/version fixed; P5-12 (branch/chunk-p5-12-dkmsconf, awaiting review): dkms.conf MAKE/CLEAN aligned to the upstream amkillam /build pattern — M= missing-/build defect fixed], Intel i5-6600 MCHBAR, model reconciliation, P1 L1/L2 refinement, MSRV decision, finalize GitHub push/tag on operator go-ahead).

## History
- [DONE] ID: P5-11 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-11-dkms
DECISION: Rewrote install-ryzen-smu-dkms.sh DKMS flow: stage source to /usr/src/ryzen_smu-$PKGVER (PKGVER = git rev-count.short-hash) with concrete repo dkms.conf (PACKAGE_VERSION aligned) + depmod override + monitor_cpu build/install, then dkms add ryzen_smu/$PKGVER / build / install / modprobe.
AHEAD: Reviewer to verify staging-before-add + idempotency; operator must re-run script (sudo, real kernel) — not executed here by design.
- [DONE] ID: P5-11-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-11 (branch deleted): staging-before-add correct, dkms add ryzen_smu/$PKGVER specifies module+version, idempotent, zero other-file changes; shellcheck clean (info-only).
AHEAD: CRITICAL separate defect (P5-03, NOT in P5-11): packaging/ryzen-smu-dkms/dkms.conf MAKE/CLEAN use M=${dkms_tree}/${PACKAGE_NAME}/${PACKAGE_VERSION} = version root (no Makefile; DKMS copies source to .../build and cd-there) so dkms build will STILL fail. Needs its own chunk: add /build to the M= path (match DKMS default).
- [DONE] ID: P5-12 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-12-dkmsconf
DECISION: Rewrote packaging/ryzen-smu-dkms/dkms.conf MAKE/CLEAN to the authoritative upstream amkillam/ryzen_smu pattern (DKMS runs `make TARGET=${kernelver}` in the staged .../build dir; the staged Makefile resolves the kernel KDIR + M=$(CURDIR) so the .ko lands where DKMS looks); DEST_MODULE_LOCATION stays /extra to match the P5-11 depmod override; PACKAGE_VERSION keeps the upstream @VERSION@ placeholder (script's whole-line sed aligns it to $PKGVER); install script untouched.
AHEAD: Reviewer to verify DKMS-syntax + that `dkms build` now resolves the .ko in .../build; full ground truth needs the operator re-run (sudo, real kernel + headers).
