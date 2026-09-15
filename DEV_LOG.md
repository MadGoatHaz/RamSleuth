# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-CYCLE6.md` (Cycle 6 GUI workstream — the handover §13 primary name, alt `PLAN-PHASE7.md`; Phase 6/Cycle 5 retained as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`; Phase 1 as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 3 as `plans/PLAN-PHASE3.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@

@@@ CURRENT_STATE @@@
Cycle 6 (GUI workstream) — C6-04 implemented on branch/chunk-c6-04 (pushed, awaiting review/merge); AmdPmSnapshot.command_rate raw slot + same-crate literal ripples (D-C10); telemetry 141/141 + main 6/6 green; clippy -D warnings clean.

## History
- [DONE] ID: C6-04 | STATUS: SUCCESS | BRANCH: branch/chunk-c6-04
DECISION: Added `command_rate: u8` raw slot to `AmdPmSnapshot` (amd_pm.rs, after `pdm` — mirrors the `gdm`/`pdm` pattern: `parse()` zeroes it, the SMN overlay fills it in C6-05) + all three same-crate `AmdPmSnapshot{...}` construction sites co-landed per D-C10 (`good_snapshot` amd_readout.rs, `pm_only_snapshot` facade.rs, `base_snapshot` amd_smn.rs — each `command_rate: 0`) + one new test (`parse()` zeroes `command_rate`; snapshot carries it); module/parse docs updated; `apply_smn`/`decode_smn`/`SmnFields` untouched (C6-05); uncommitted plan rev 2 audit repair co-committed; telemetry 141/141 + main 6/6 green, clippy -D warnings clean.
AHEAD: C6-03 (`CommandRate` enum + `ClockReadout.command_rate`, amd_readout.rs) reads `AmdPmSnapshot.command_rate` — fork from the post-C6-04-merge tip; C6-05 extends the `apply_smn` write surface (gdm + timings + command_rate); C6-01 (96bd078) still pending review/merge into v2-development.
- [DONE] ID: P5-13-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-13 (branch deleted): L136/L141 dkms build/install now pass ${MODULE}/${PKGVER}, symmetric with L131 dkms add; die/guard strings (L137/L142) intact; dkms.conf CLEAN line gone (grep empty) with all other directives intact; rest of script unchanged; bash -n clean, shellcheck info-only (pre-existing SC2015 on the add guard); zero other-file changes; cargo test skipped (no source changes).
AHEAD: Operator re-run of scripts/install-ryzen-smu-dkms.sh is the live ground truth (no dkms remove needed); ready for Cycle 5.
- [DONE] ID: P5-14 | STATUS: SUCCESS | BRANCH: branch/chunk-p5-14-dkmsinstall
DECISION: Fixed the ryzen_smu install skip-check (L138): replaced the build-dir existence test ([[ -d /var/lib/dkms/$MODULE/$PKGVER/$KERNEL ]]) with a real DKMS install-state check (dkms status | grep -qE "^$MODULE/$PKGVER, <kernel>, <arch>: installed"), so a built-but-not-installed module is no longer wrongly skipped; the re-run now runs dkms install and copies the .ko into /lib/modules/<kernel>/. Also added $PKGVER to the install die message.
AHEAD: Reviewer to merge into v2-development; operator re-run of scripts/install-ryzen-smu-dkms.sh is the ground truth (module is currently built-not-installed, so a fixed re-run installs it); no dkms remove needed.
- [DONE] ID: P5-14-REVIEW | STATUS: SUCCESS (merged) | BRANCH: v2-development
DECISION: Reviewed + merged P5-14 (branch deleted): skip-check (L138) is now `dkms status | grep -qE '^${MODULE}/${PKGVER}, <kernel>, <arch>: installed$'` - anchored to the module/version/kernel line and requiring the state field to be exactly 'installed', so 'built' does not match -> dkms install runs (bug fixed) and 'installed' -> idempotent skip; install die message now cites ${MODULE}/${PKGVER} (consistent); dkms add/build + modprobe + modules-load.d + verify all unchanged; bash -n clean, shellcheck info-only (pre-existing SC2015 on add guard); zero other-file changes (script + DEV_LOG only); cargo test skipped (no source changes).
AHEAD: ryzen_smu install path fully fixed (staging + dkms.conf + build/install version args + install-state check); operator re-run of scripts/install-ryzen-smu-dkms.sh is the ground truth (no dkms remove needed); ready for Cycle 5.
