# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE5.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 4 (Phase 5: Packaging + ryzen_smu uAPI/install reconciliation) COMPLETE + QA passed (328/328, clippy clean); compacted to MASTER_LOG. ryzen_smu: upstream amkillam/ryzen_smu (main), daemon reads /sys/kernel/ryzen_smu_drv/pm_table, install script stages to /usr/src + upstream dkms.conf MAKE + dkms build/install MODULE/VERSION + dkms status install-state check + monitor_cpu. Operator re-run of scripts/install-ryzen-smu-dkms.sh is the live ground-truth. Ready for Cycle 5 (AMD ground truth, Intel i5-6600 MCHBAR, model reconciliation, P1 L1/L2 refinement, MSRV decision, finalize GitHub push/tag on operator go-ahead).

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
- [DONE] ID: P5-12-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-12 (branch deleted): valid DKMS syntax, MAKE="make TARGET=${kernelver}" matches upstream amkillam pattern (broken M=${dkms_tree}/... missing-/build line gone), @CFLGS@ hook deliberately omitted (documented), single ^PACKAGE_VERSION= line sed-aligned to $PKGVER by the script, DEST_MODULE_LOCATION /extra matches the depmod override, zero other-file changes; cargo test skipped (no source changes).
AHEAD: ryzen_smu install path fully fixed (staging + dkms.conf); operator re-run is the ground-truth test (sudo, real kernel + headers).
- [DONE] ID: P5-13 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-13-dkmsver
DECISION: Root-caused + fixed the ryzen_smu dkms build/install failure: L136/L141 now pass the staged ${MODULE}/${PKGVER} (symmetric with L131 dkms add) instead of the bare ${MODULE}, since DKMS 3.4.3 does not resolve a bare name to the single registered version (do_build fell into add_module with an empty version and died on the hardcoded Usage: add message); also dropped the deprecated CLEAN directive from dkms.conf and adjusted its two comment references.
AHEAD: Reviewer to merge into v2-development; the operator re-run of scripts/install-ryzen-smu-dkms.sh is the ground truth - current DKMS state is added plus an empty build dir, so a fixed re-run hits the already-registered-continuing path and proceeds to a real build; no dkms remove is needed to recover.
- [DONE] ID: P5-13-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-13 (branch deleted): L136/L141 dkms build/install now pass ${MODULE}/${PKGVER}, symmetric with L131 dkms add; die/guard strings (L137/L142) intact; dkms.conf CLEAN line gone (grep empty) with all other directives intact; rest of script unchanged; bash -n clean, shellcheck info-only (pre-existing SC2015 on the add guard); zero other-file changes; cargo test skipped (no source changes).
AHEAD: Operator re-run of scripts/install-ryzen-smu-dkms.sh is the live ground truth (no dkms remove needed); ready for Cycle 5.
- [DONE] ID: P5-14 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-14-dkmsinstall
DECISION: Fixed the ryzen_smu install skip-check (L138): replaced the build-dir existence test ([[ -d /var/lib/dkms/$MODULE/$PKGVER/$KERNEL ]]) with a real DKMS install-state check (dkms status | grep -qE "^$MODULE/$PKGVER, <kernel>, <arch>: installed"), so a built-but-not-installed module is no longer wrongly skipped; the re-run now runs dkms install and copies the .ko into /lib/modules/<kernel>/. Also added $PKGVER to the install die message.
AHEAD: Reviewer to merge into v2-development; operator re-run of scripts/install-ryzen-smu-dkms.sh is the ground truth (module is currently built-not-installed, so a fixed re-run installs it); no dkms remove needed.
- [DONE] ID: P5-14-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-14 (branch deleted): skip-check (L138) is now `dkms status | grep -qE '^${MODULE}/${PKGVER}, <kernel>, <arch>: installed$'` - anchored to the module/version/kernel line and requiring the state field to be exactly 'installed', so 'built' does not match -> dkms install runs (bug fixed) and 'installed' -> idempotent skip; install die message now cites ${MODULE}/${PKGVER} (consistent); dkms add/build + modprobe + modules-load.d + verify all unchanged; bash -n clean, shellcheck info-only (pre-existing SC2015 on add guard); zero other-file changes (script + DEV_LOG only); cargo test skipped (no source changes).
AHEAD: ryzen_smu install path fully fixed (staging + dkms.conf + build/install version args + install-state check); operator re-run of scripts/install-ryzen-smu-dkms.sh is the ground truth (no dkms remove needed); ready for Cycle 5.
