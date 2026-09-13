# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE5.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases — P5-04 merged into v2-development)

@@@ CURRENT_STATE @@@
P5-02 (+fix) MERGED — the QA follow-up is resolved: the `ramsleuth` group is now created on the TARGET system by ramsleuth-git.install pre_install/pre_upgrade (idempotent getent||groupadd -r, runs as root); the ineffective build-env groupadd is removed from PKGBUILD package() (replaced by a NOTE comment); all 6 pacman hooks present, bash -n clean; --no-ff merged into v2-development and pushed to origin (ddf9650, merge a27d976); zero source changes; local branch deleted. Install flow verified: pre_install creates group -> unpack -> post_install enable --now resolves Group=. P5-03 MERGED: packaging/ryzen-smu-dkms/dkms.conf (28 lines: header + PACKAGE_NAME/VERSION, BUILT_MODULE_NAME[0], DEST_MODULE_LOCATION[0]=/extra, AUTOINSTALL=yes, MAKE/CLEAN, MODULE_STRIP[0]="") --no-ff merged into v2-development and pushed to origin (2c2346e, merge 093f497); zero source changes; local branch deleted. P5-04 MERGED: scripts/install-ryzen-smu-dkms.sh (95 lines, mode 755; full HANDOVER §7 workflow: idempotent pm_table fast-path, sudo re-exec, pacman dkms/base-devel + never-guess headers guard with candidate listing, verified-upstream shallow clone to /opt/ryzen-smu-src (RYZEN_SMU_URL override, pull --ff-only tolerated), P5-03 dkms.conf fallback, dkms add||true + dkms install -k, modprobe + /etc/modules-load.d, pm_table verify with dmesg hint; bash -n PASS + shellcheck zero findings; zero source changes; --no-ff merged into v2-development and pushed to origin (150d88f, merge 28071d1); local branch deleted). Next: P5-05 ryzen-smu-dkms/PKGBUILD (coupled to P5-03).
- [DONE] ID: P5-04 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-04
- [DONE] ID: P5-03 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-03
- [DONE] ID: P5-02-fix | STATUS: SUCCESS | BRANCH: branch/chunk-p5-02-fix
- [DONE] ID: P5-02 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-02
- [DONE] ID: P5-01 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-01
