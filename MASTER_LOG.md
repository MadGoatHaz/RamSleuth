# RamSleuth v2 — Master Log

Durable per-cycle compaction of `DEV_LOG.md`. Newest cycle first.

## Dev-Cycle Decisions & Hardware Context — 2026-09-12

Confirmed by director/user 2026-09-12; captured in `Docs/RamSleuth-v2.md`, `Docs/Grand Design & Architecture Specification.md`, and `Docs/HANDOVER.md`.

- **Push policy:** stay 100% local on `v2-development`; do NOT push to the GitHub upstream (`https://github.com/MadGoatHaz/RamSleuth`, divergent legacy `master`) until there is a confirmed, tested, working end-result app. No force-pushes without explicit sign-off.
- **AMD host / `ryzen_smu`:** the `ryzen_smu` kernel module is NOT installed on the primary dev host (AMD Ryzen 9 5950X, Zen 3, 16C/32T, 64 MiB L3, DDR4, AVX2; **CachyOS, kernel `7.2.3-1-cachyos-custom`**): `sudo modprobe ryzen_smu` → `FATAL: Module ryzen_smu not found in directory /lib/modules/7.2.3-1-cachyos-custom`; `/sys/kernel/ryzen_smu/` absent. For AMD live subtiming verification the module MUST be built + installed + loaded (headers for `7.2.3-1-cachyos-custom` → build out-of-tree → `depmod -a` → `modprobe` → verify `pm_table`); steps documented in `Docs/RamSleuth-v2.md`. The codebase degrades gracefully today: `N/A (DriverMissing)`, never panics.
- **Intel test machine:** LGA-1151 **Intel i5-6600 (Skylake, 6th-gen), dual-channel (2 DIMM channels)** — available for live Intel MCHBAR decode verification; matches `channel_count(Skylake) = 2`.
- **Cycle position:** Phase 1 (Native Benchmark Engine, 11 chunks) and Phase 2 (Live Memory Controller Telemetry, 11 chunks) are COMPLETE and QA-passed (159/159 tests, clippy clean). **The next development cycle starts at Phase 3** (privilege-separated daemon + Unix socket + clients).
- **CPUID note:** the frozen P2-01 map classifies the 5950X reference host as family `0x19` → `Amd(Zen3)`; desktop Zen 4/5 silicon also reports family `0x19` on some boards, so the AMD PM parse (P2-04) keys on the **SMU version** (7.11.x / 12.x / 13.x), not on `AmdZen` — generation ambiguity cannot break the PM layout.

## RamSleuth v2 — Cycle 4 (Phase 5: Packaging & Distribution) — 2026-09-13

### What was delivered
Packaging & distribution for the completed 7-crate pure-Rust workspace — 7 chunks (P5-01…P5-07) + 2 fixes, all `--no-ff` merged into `v2-development` (range `51f872f..e415306`):
- **P5-01** `packaging/ramsleuth-git/ramsleuth.preset` — systemd preset, enables `ramsleuth.service` (eeaf131 → merge 3f98513).
- **P5-02** `packaging/ramsleuth-git/PKGBUILD` + `packaging/ramsleuth-git/ramsleuth-git.install` — AUR package: builds the 7-crate workspace, installs the 6 binaries (`ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui`, `ramsleuth-bench`, `ramsleuth-telemetry`) to `/usr/bin` plus the daemon unit and preset (merged 396a72d). **P5-02-fix**: the `ramsleuth` group is created on the TARGET system by the `.install` `pre_install`/`pre_upgrade` hooks (idempotent `getent || groupadd -r`, runs as root); the ineffective build-env `groupadd` is removed from `package()` (replaced by a NOTE comment) (ddf9650 → merge a27d976).
- **P5-03** `packaging/ryzen-smu-dkms/dkms.conf` — `AUTOINSTALL=yes` (module auto-rebuilds on kernel change), optional AMD extra (2c2346e → merge 093f497).
- **P5-04** `scripts/install-ryzen-smu-dkms.sh` — idempotent operator-run DKMS helper implementing HANDOVER §7 (pm_table fast-path, sudo re-exec, never-guess kernel-headers guard with candidate listing, verified-upstream shallow clone, `dkms add`/`install`, modprobe + `/etc/modules-load.d`, `pm_table` verify with dmesg hint) (150d88f → merge 28071d1). **P5-04-fix**: the dkms.conf fallback now resolves the repo path first, then the installed `/usr/share/ryzen-smu-dkms/` path, so the helper works both from a checkout and from the AUR package (e41d4ae → merge 46c4b8e).
- **P5-05** `packaging/ryzen-smu-dkms/PKGBUILD` — optional provisioning AUR package with a safe no-in-chroot design: ships the P5-03 dkms.conf + P5-04 helper as `/usr/bin/ryzen-smu-dkms-install`; the operator runs the helper on the target (430b2f3 → merge c93410e).
- **P5-06** `.github/workflows/ci.yml` — GitHub Actions: `cargo test --workspace` (debug + release) and clippy `-D warnings` on a `["1.75", "stable"]` matrix (`fail-fast: false`), plus a release build job uploading a 6-binary artifact (d5c384b → merge f3c6d25). Committed `Cargo.lock` untouched — the MSRV 1.75 leg is verified by CI (host has no rustup; local MSRV check skipped and documented).
- **P5-07** `packaging/README.md` — operator/end-user packaging guide: install table (6 binaries → `/usr/bin`, unit + preset → systemd paths, group via `.install` hooks), AUR + `makepkg -si` paths, `usermod` / `Group=wheel` fallback, sandboxed unit day-2 commands, ryzen-smu-dkms extra, CI matrix summary, no-panic note (cf15137 → merge 63b1ea4).

### Quality
- **327/327 tests green (debug AND release, whole workspace), 0 failures**; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); release build OK.
- **9/9 packaging/CI/systemd files valid**: all present and non-empty (`ci.yml`, `packaging/README.md`, ramsleuth-git `PKGBUILD` + `ramsleuth-git.install` + `ramsleuth.preset`, ryzen-smu-dkms `PKGBUILD` + `dkms.conf`, install script, `systemd/ramsleuth.service`); `bash -n` PASS ×4; shellcheck zero findings; `ci.yml` YAML-valid (python3 + pyyaml); install script mode 755.
- **ZERO Rust source changes**: range `51f872f..e415306` touched no `.rs` and no `Cargo.*` files (packaging/CI/docs only); committed `Cargo.lock` untouched by any Phase 5 chunk; no-panic / graceful-degradation contract preserved.
- QA audit on `v2-development` @ e415306: **PASS** (green cycle) — full re-verification after the P5-02-fix target-group follow-up; P5-QA closed.

### Push state (operator gate)
`v2-development` (== e415306) and all 9 `branch/chunk-p5-*` branches were fast-forwarded to origin during the cycle (no force-push); `origin/v2-development` == local HEAD. Remote chunk-branch pruning and an optional tag remain pending explicit operator go-ahead — this compaction performs no push and deletes no remote branch.

### Open items carried to Cycle 5
1. AMD tick-identical ground truth — needs the `ryzen_smu` module built + loaded + root on the 5950X host (P5-03/P5-04/P5-05 now make this a buildable/installable path; P5-08/09/10 reconciled the upstream to `amkillam/ryzen_smu` + the canonical `ryzen_smu_drv` sysfs path — install now unblocked).
2. Intel live MCHBAR decode — on the LGA-1151 i5-6600 (Skylake, dual-channel) test machine.
3. Model reconciliation — AMD PM byte offsets + Intel IMC offsets are plan-mandated skeletons; SPD maker `0xC1` + density `0x0D` codes are outside the frozen tables.
4. P1 L1/L2 bandwidth overhead refinement (inner-loop iterations for the small-tier working sets).
5. MSRV 1.75 → 1.89 decision (deferred for AVX-512F; workspace deliberately kept at 1.75 via lockfile pins; the first GitHub Actions run is the MSRV 1.75 checkpoint).
6. Push to GitHub — `v2-development` already fast-forwarded to origin during the cycle; remote chunk-branch prune + optional tag pending explicit operator go-ahead.

### Per-chunk history (summarized from DEV_LOG.md)
9 branches (P5-01…P5-07 + P5-02-fix + P5-04-fix), all reviewed and `--no-ff` merged into `v2-development`, then fast-forwarded to origin:
- P5-01: systemd preset enabling `ramsleuth.service` (eeaf131 → merge 3f98513).
- P5-02: ramsleuth-git AUR PKGBUILD + `.install` (6 bins, daemon unit, ramsleuth group; merged 396a72d); P5-02-fix: target-side `ramsleuth` group via `pre_install`/`pre_upgrade`, build-env `groupadd` removed (ddf9650 → merge a27d976).
- P5-03: ryzen-smu-dkms `dkms.conf` (`AUTOINSTALL=yes`, optional AMD extra) (2c2346e → merge 093f497).
- P5-04: idempotent operator-run DKMS install helper (HANDOVER §7) (150d88f → merge 28071d1).
- P5-04-fix: dkms.conf fallback resolves repo + installed `/usr/share` (e41d4ae → merge 46c4b8e).
- P5-05: optional ryzen-smu-dkms provisioning AUR package (safe no-in-chroot design) (430b2f3 → merge c93410e).
- P5-06: GitHub Actions CI — 1.75/stable matrix (test debug+release, clippy `-D warnings`) + 6-binary artifact (d5c384b → merge f3c6d25).
- P5-07: packaging README (operator/end-user guide) (cf15137 → merge 63b1ea4).
- P5-QA: full audit @ e415306 — 327/327 debug + release, clippy 0, 9/9 files valid, zero `.rs` / zero `Cargo.*` in range; green cycle.

### ryzen_smu uAPI reconciliation (post-merge, 2026-09-13)
Post-compaction follow-up: operator ran `scripts/install-ryzen-smu-dkms.sh` on the 5950X host; the default `git clone` of the dead `53XU/ryzen_smu` repo 404'd and fell back to an interactive GitHub credential prompt. Upstream review of 3 candidates:
- `leogx9r/ryzen_smu` — original, frozen 2021 (exact `ryzen_smu` module, `/sys/kernel/ryzen_smu`).
- `FlyGoat/ryzen_nb_smu` — unrelated experiment (module `ry_nb_pp`) — rejected.
- `amkillam/ryzen_smu` — active fork, v0.1.7 (2026-08), kernel 7.2+ fix; exact `ryzen_smu` module, exposes `/sys/kernel/ryzen_smu_drv/pm_table` (kobject `ryzen_smu_drv`), `monitor_cpu` CLI, Zen 3 + kernel 7.2+ support. **CHOSEN: `amkillam/ryzen_smu` (branch `main`)**.

**CRITICAL pre-existing bug fixed:** the daemon read the wrong sysfs path (`/sys/kernel/ryzen_smu/pm_table`); the real module exposes `/sys/kernel/ryzen_smu_drv/pm_table`.

- **P5-08** `crates/ramsleuth-telemetry/src/amd_smu.rs` — candidate list: canonical `/sys/kernel/ryzen_smu_drv/pm_table` first + legacy `/sys/kernel/ryzen_smu/pm_table` fallback; +1 regression test (327→328).
- **P5-09** `scripts/install-ryzen-smu-dkms.sh` — default upstream now `amkillam/ryzen_smu` (`RYZEN_SMU_URL` override kept) + `ryzen_smu_drv` path in fast-path/verify.
- **P5-10** `packaging/README.md` — path + upstream consistency (verify step, `monitor_cpu` ground-truth note).

**QA re-audit: PASS** (66c3122) — 328/328 (debug + release), clippy 0 warnings; daemon/script/README consistent; no-panic graceful degradation preserved (`acquire_on_this_host_is_graceful`). Residual `53XU` / non-`_drv` references are intentional: legacy-fallback candidate list, the script's explicit "53XU is DEAD" warning, historical/spec docs — no doc-scrub needed.

### ryzen_smu install-path fixes (post-merge, 2026-09-13)
Post-reconciliation live runs: operator re-ran `scripts/install-ryzen-smu-dkms.sh` on the 5950X host; the amkillam clone SUCCEEDED but `dkms add` / `dkms install` failed with "Arguments <module> and <module-version> are not specified." (first run); a second re-run then succeeded through staging + `monitor_cpu` + `dkms add` (the symlink was created) but `dkms build` died with the same "Arguments ... not specified" message. Three independent root causes, three merged chunks:
- **P5-11** `scripts/install-ryzen-smu-dkms.sh` — the script cloned to `/opt/ryzen-smu-src` but never staged the source into `/usr/src/ryzen_smu-<pkgver>/` (where DKMS discovers modules), so `dkms add ryzen_smu` had nothing to find. Fix: stage `{LICENSE,Makefile,dkms.conf,drv.c,smu.c,smu.h}` into `/usr/src/ryzen_smu-$PKGVER/` (`PKGVER` = `git rev-list --count .` + `git rev-parse --short`) **before** `dkms add ryzen_smu/$PKGVER` → build → install → modprobe; added a `/usr/lib/depmod.d/ryzen_smu.conf` override + built/installed the `monitor_cpu` CLI to `/usr/bin` (ground-truth for open item 1).
- **P5-12** `packaging/ryzen-smu-dkms/dkms.conf` — the repo's concrete dkms.conf (P5-03) had a `MAKE` line `M=${dkms_tree}/...` missing the `/build` dir, so `dkms build` would still fail. Fix: aligned `MAKE` / `CLEAN` to the authoritative upstream amkillam pattern (`make TARGET=${kernelver}`; the staged Makefile resolves the kernel KDIR + `M=$(CURDIR)`); `PACKAGE_VERSION` in sed-compatible `@VERSION@` form (the script's whole-line `sed` aligns it to `$PKGVER`); `DEST_MODULE_LOCATION` kept `/extra` to match the script's depmod override.
- **P5-13** `scripts/install-ryzen-smu-dkms.sh` — second operator re-run: staging + `monitor_cpu` + `dkms add` SUCCEEDED (the symlink was created), but `dkms build` then died with "Error! Arguments <module> and <module-version> are not specified. Usage: add ...". Precise root cause (diagnostic): L136 `dkms build "${MODULE}"` and L141 `dkms install "${MODULE}"` passed the BARE module name (no version) while L131 `dkms add "${MODULE}/${PKGVER}"` passed module/version; DKMS 3.4.3 does NOT auto-resolve a bare name to the single registered version, so `do_build` → `is_module_added "ryzen_smu" ""` (empty version) → looks un-added → internally calls `add_module` → `check_module_args` dies with the hardcoded "Usage: add ..." message (a build invocation printing an add usage). Live state corroborated: `dkms status` = `ryzen_smu/1.d298366: added` only, empty `build/` dir, no kernel subdir; kernel build dir valid; staged dkms.conf `PACKAGE_VERSION` correctly substituted (ruled out); the "Deprecated feature: CLEAN" line was a red herring (harmless stderr from the successful add). Fix: L136 → `dkms build "${MODULE}/${PKGVER}" -k "${KERNEL}"`; L141 → `dkms install "${MODULE}/${PKGVER}" -k "${KERNEL}"` (symmetric with L131); removed the deprecated `CLEAN="make clean"` line from `packaging/ryzen-smu-dkms/dkms.conf` (DKMS 3.4.3 ignores it; silences the warning). No `dkms remove` needed to recover — the fixed re-run hits the "already registered — continuing" path and proceeds to a real build.
- **P5-14** `scripts/install-ryzen-smu-dkms.sh` — third operator re-run: the module now BUILDS successfully ("Building module(s)... done." + signed `ryzen_smu.ko` at `/var/lib/dkms/ryzen_smu/1.d298366/build/`), but the "already installed" check (L138) printed "already installed ... skipping dkms install" and SKIPPED the install, so the `.ko` was never copied into `/lib/modules/` and `modprobe ryzen_smu` failed with "Module not found." Root cause: L138 `[[ -d "/var/lib/dkms/${MODULE}/${PKGVER}/${KERNEL}/" ]]` tested for the BUILD dir (created by `dkms build`), not the install state — so after a build (before an install) it wrongly skipped `dkms install`. Live state corroborated: `dkms status` = `ryzen_smu/1.d298366, 7.2.3-1-cachyos-custom, x86_64: built` (built, not installed); `/lib/modules/7.2.3-1-cachyos-custom/extra/` absent; `modinfo ryzen_smu` = not found. Fix: L138 now uses `dkms status | grep -qE "^${MODULE}/${PKGVER},[[:space:]]*${KERNEL},[[:space:]]*[^:]*:[[:space:]]*installed[[:space:]]*$"` (matches the "installed" state specifically, not "built"); verified behaviorally (built → no match → install runs; installed → match → skip). Safe failure mode = re-install (idempotent), never a wrong skip.
- **Reference:** the operator provided the working AUR `aur/ryzen_smu-dkms-git` PKGBUILD as the staging reference; we deliberately do NOT depend on that external AUR package (self-contained tooling in RamSleuth; only the module source is fetched from amkillam git at install time).
- **Status:** install path fully fixed (staging + dkms.conf MAKE + build/install version args + install-state check); operator re-run on the real kernel is the ground-truth verification (the module is currently built-not-installed, so the fixed re-run will now actually run `dkms install`).

Cycle 4 close-out (2026-09-13, updated): initial compaction recorded P5-01…P5-07 + 2 fixes + P5-QA @ e415306 (327/327); this update records the ryzen_smu uAPI reconciliation (P5-08/09/10 + QA re-audit @ 66c3122, 328/328, clippy 0) **and** the install-path fixes from the operator live runs (P5-11 source staging into `/usr/src` + P5-12 dkms.conf `MAKE`/`CLEAN` aligned to upstream amkillam + P5-13 `dkms build`/`install` passing `${MODULE}/${PKGVER}` and removing the deprecated `CLEAN` directive — all three merged; packaging/script/dkms.conf only, zero Rust changes). `DEV_LOG.md` reset (ACTIVE_WORKERS = no leases, CURRENT_STATE = Cycle 4 complete + QA passed; operator re-run of `scripts/install-ryzen-smu-dkms.sh` is the live ground-truth; ready for Cycle 5). Local-only commit; no push, no remote branch deletion.

## RamSleuth v2 — Cycle 3 (Phase 3: Privilege-Separated Architecture) — 2026-09-13

### What was delivered
Privilege-separated architecture: one privileged daemon owns all hardware probing; all clients are fully unprivileged and reach it over a Unix socket. Five new crates + a serde wire foundation:
- **`ramsleuth-protocol`** — the wire contract: `Request` / `Response` / `BenchMode` / `Message` (serde-derived, payloads reused verbatim from telemetry + bench) + `DEFAULT_SOCKET_PATH` + a length-prefixed Bincode frame codec (`frame.rs`) with a 16 MiB `MAX_FRAME_SIZE` guard (`encode_frame` / `decode_frame` / `Frame` / `FrameError`); zero `unsafe`, tokio-free.
- **`ramsleuth-daemon`** (lib + bin) — `caps` SOFT privilege probe (geteuid + `CapEff` bit 21; never panics/exits, warns softly); `socket` listener (mode **0660**, stale-file probe → live `AlreadyRunning` / dead rebind, best-effort `chown ramsleuth:wheel`); `TelemetryCache` (TTL-gated, injectable collector, clone-within-TTL); `BenchJobManager` (single-flight, clean cancel, per-cell progress); `rpc` (per-connection async RPC over the frozen wire contract); `main` bin (`--socket` / `--max-age`, SIGTERM/SIGINT → graceful stop + socket removed, no-panic contract) + **`systemd/ramsleuth.service`** (packaging artifact; CAP_SYS_RAWIO-only unit).
- **`ramsleuth-client`** (lib + bin) — synchronous `UnixStream` transport (read/write timeouts, 3-retry backoff, friendly `DaemonDown` hint); pure `dump` dashboard renderer (full hardware timings, every N/A cell with a reason); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes 0/1/2).
- **`ramsleuth-tui`** (lib + bin) — ratatui 0.29 + crossterm 0.28; pure `key_to_action` (R / S / Q); 3-zone non-scrolling dashboard; terminal loop with a background 2 s updater; MSRV-safe lockfile pins for ratatui transitive deps.
- **`ramsleuth-gui`** (lib + bin) — egui / eframe / egui_extras 0.27.2; semantic palette + **F2** PNG / **F3** JSON export to `$HOME`; `TelemetryData` + background poller with a bench command channel + cancel; 3 zones; eframe app 1400×900 @ ~60 FPS.
- **Serde foundation** — serde derives on all telemetry (CPUID / AMD / Intel / SPD / facade) + bench (worker / orchestrator / streamed) public wire types; `SystemMemoryTelemetry`, `BenchmarkGrid`, `StreamProgress` (and `WorkerError`) are wire-serializable, each with bincode round-trip tests.

### Quality
- **327/327 tests green (debug AND release, whole workspace)**; **zero clippy warnings** (`clippy --workspace --all-targets -- -D warnings`); release build OK.
- **MSRV = 1.75** (max `rust_version` across all **441 packages**); ratatui / egui transitive deps held ≤ 1.75 via lockfile pins (`instability` ≤ 0.3.10, `unicode-segmentation` ≤ 1.12.0; existing tokio 1.53.1 / serde 1.0.229 pins untouched).
- **Live end-to-end verified** (this host, unprivileged): daemon up with a 0660 socket; unprivileged `ramsleuth-client` `dump` / `status` / `bench` all **exit 0** (bench: `BenchStarted` + live progress + full 4×4 grid); TUI rendered under a PTY (3 zones live); GUI ran on Wayland (1400×900, all zones, F2/F3 export, clean Q); daemon `SIGTERM` → graceful stop, exit 0, socket removed; no-daemon path → **exit 1** + friendly `DaemonDown` start hint; **zero panics / segfaults**.
- QA audit on `v2-development` (post-merge, live unprivileged run): **CORE GATE PASSED** — unprivileged `ramsleuth-client -- dump` prints full hardware timings against a running daemon (the Phase 3 exit criterion).

### Live result (this host: Ryzen 9 5950X / Zen 3 / DDR4, `ryzen_smu` absent, unprivileged)
- Graceful degradation confirmed end-to-end: **AMD N/A (`DriverMissing`, no `ryzen_smu`)**; **Intel N/A (`UnsupportedHardware`)**; **SPD density `0x0D` parse-error → N/A**; **SPD maker `0xC1` shown raw-hex**; 2× DDR4 SPD modules at 3200 MT/s with per-profile timings; live CPU readout (Amd(Zen3), brand string). No panic in any privilege/CPU state.

### Key decisions
- One privileged daemon is the sole privilege boundary: all hardware I/O (SMU/IMC/SPD) stays daemon-side; the client/TUI/GUI chain is tokio-free and never touches hardware.
- Synchronous length-prefixed Bincode frames with a 16 MiB size guard over a 0660 Unix socket; tokio confined to the daemon (async accept loop + `spawn_blocking` pumps); std-only client transport.
- Serde added to every wire-crossing public type up front, so the protocol crate reuses payload types verbatim (no duplication, no boxing of frozen arm shapes).

### Open items carried to Cycle 4
1. AMD tick-identical ground truth — needs `ryzen_smu` module built + loaded + root on the 5950X host.
2. Intel live MCHBAR decode — on the LGA-1151 i5-6600 (Skylake, dual-channel) test machine.
3. Model reconciliation — AMD PM + Intel IMC byte offsets are plan-mandated skeletons; SPD maker `0xC1` + density `0x0D` codes are outside the frozen tables.
4. P1 L1/L2 bandwidth overhead refinement (inner-loop iterations for the small-tier working sets).
5. MSRV 1.75 → 1.89 bump decision (deferred; workspace deliberately kept at 1.75 via lockfile pins).
6. Push to GitHub — condition met (confirmed working app) but **held local per policy**; ready on explicit go-ahead. No force-pushes without sign-off.

### Per-chunk history (summarized from DEV_LOG.md)
30 single-file micro-chunks (P3-01…P3-30) + 2 doc-drift fixes (DocFix, DocFix2), all reviewed and merged into `v2-development` via `--no-ff` (local only, never pushed):
- P3-01…P3-06: serde foundation — derives + bincode round-trip tests on the telemetry wire types (`NaReason` / `Section<T>`, CPUID, AMD, Intel channel/readout, SPD decode, `SystemMemoryTelemetry` facade root + `PartialEq`).
- P3-07…P3-09: bench serde + streaming — `BenchOp` / `WorkerResult` (+ `WorkerError` wire-serializable), `Tier` / `Metric` / `BenchmarkGrid`, `streamed.rs` (`run_streamed` with clean cancel + per-cell progress, `StreamTarget` / `StreamProgress` / `StreamError`).
- P3-10…P3-11: `ramsleuth-protocol` birth — `Request` / `Response` / `BenchMode` / `Message` + `DEFAULT_SOCKET_PATH`; length-prefixed Bincode frame codec + 16 MiB guard.
- P3-12…P3-17: `ramsleuth-daemon` birth — crate scaffold + `caps` SOFT probe; `socket` (0660 + stale-rebind + chown); `cache` (TTL, injectable collector); `bench_job` (single-flight + cancel); `rpc` (per-connection async); `main` bin + `systemd/ramsleuth.service`.
- P3-18…P3-21: `ramsleuth-client` — `UnixStream` transport (timeouts / retries / `DaemonDown`); pure `dump` renderer (the exit-criterion command); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes).
- P3-22…P3-24: `ramsleuth-tui` — crate birth + `events` (`key_to_action` R/S/Q, MSRV lockfile pins); 3-zone dashboard; terminal loop + background 2 s updater.
- P3-25…P3-30: `ramsleuth-gui` — style/semantic palette; shared state + background poller (bench command channel + cancel); telemetry zone (matrix); bench zone (4×4 + progress + run/cancel); status zone (SPD cards + daemon status + F2/F3/Q actions); eframe app shell (1400×900, ~60 FPS).
- DocFix / DocFix2: residual `--max-age` doc-drift fixes (`5 s` → `2 s` to match the real 2 s daemon default); doc-only, zero code/behavior change.

Cycle 3 close-out (2026-09-13): all 30 `branch/chunk-P3-*` plus `branch/chunk-docfix` / `branch/chunk-docfix2` verified fully merged into `v2-development` and pruned (`git branch -d` only; no unmerged branch touched). `DEV_LOG.md` reset (ACTIVE_WORKERS = no leases, CURRENT_STATE = Cycle 3 complete + ready for Cycle 4).

## RamSleuth v2 — Cycle 2 (Phase 2: Live Memory Controller Telemetry) — 2026-09-12

### What was delivered
`ramsleuth-telemetry` complete (11 chunks, P2-01…P2-11):
- CPUID vendor / Zen-family detection (interface freeze) — P2-01.
- Error + `Section<T>` no-panic contract (interface freeze) — P2-02.
- Privilege-guarded AMD SMU access (sysfs→char-dev, sole `nix` dep) — P2-03.
- Version-guarded bounds-checked AMD PM parse (SMU 7.11.x / 12.x / 13.x) — P2-04.
- Shared display types + AMD mapping (sanity-gated) — P2-05.
- Intel MCHBAR acquire + read-only /dev/mem mmap guard (Intel-gated; STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege; volatile MMIO read) — P2-06.
- Intel per-channel IMC decode — P2-07.
- Unprivileged SPD EEPROM acquisition — P2-08.
- SPD decode (JEP106, rank, XMP/EXPO) — P2-09.
- `SystemMemoryTelemetry` facade + `collect()` (per-branch error containment) — P2-10.
- Verification CLI (`cargo run -p ramsleuth-telemetry [--json]`) — P2-11.

### Quality
- 159/159 tests green (debug + release, whole workspace); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live telemetry CLI exit 0 (text + `--json`).
- QA audit 2026-09-12 on `v2-development` @ 367bef9: all 8 runnable gates PASS.

### Live result (this host: Zen 3 / DDR4, `ryzen_smu` module absent)
- `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD: N/A (DriverMissing)`; `Intel: N/A (UnsupportedHardware)`; SPD 2× DDR4 modules (rank=1, speed=3200 MT/s, maker `0xC1` raw-hex, density `0x0D` → Na). No panic in any privilege/CPU state.

### Key decisions
- Direct SMU access (no `ryzen_smu` crate): sysfs-first + char-dev read; `nix` (features `fs`/`ioctl`/`mman`) is the only new dependency of Phase 2.
- AMD PM + Intel IMC byte offsets are plan-mandated SKELETONS pending live-silicon reconciliation.

### BLOCKED acceptance gates (hardware/driver, not code)
1. AMD tick-identical ground truth — needs `ryzen_smu` module loaded + root.
2. Intel live MCHBAR decode — needs Intel silicon.
3. Model reconciliation — AMD PM byte offsets, Intel IMC offsets, SPD maker `0xC1` + density `0x0D` codes.

### Per-chunk history (summarized from DEV_LOG.md)
- P2-01: CPUID vendor/Zen-family detection (interface freeze).
- P2-02: `TelemetryError` + `Section<T>` no-panic contract (interface freeze); 14/14 tests.
- P2-03: AMD SMU acquire (sysfs→char-dev; `nix` 0.29 sole new dep); 20/20; live → `DriverMissing{ryzen_smu}`.
- P2-04: version-guarded bounds-checked AMD PM parse; 29/29; offsets = plan skeleton.
- P2-05: shared display types + sanity-gated AMD mapping; 39/39.
- P2-06: Intel MCHBAR + read-only /dev/mem guard; 48/48. *Process note:* review FAILED (F1 STRICT_DEVMEM EIO/ENODATA→InsufficientPrivilege, F2 volatile read) → fix `85114a6` → re-review PASS.
- P2-07: Intel per-channel IMC decode; 66/66; offsets = documented model/skeleton.
- P2-08: unprivileged SPD EEPROM acquire (world-readable sysfs `eeprom`); 74/74; live → 2× 512B DDR4.
- P2-09: SPD decode (JEP106, rank, XMP/EXPO); 85/85. *Process note:* recovered prior subagent's uncommitted/empty payload (3 compile fixes + missing 11-test module).
- P2-10: `SystemMemoryTelemetry` facade + `collect()` (per-branch containment); 90/90.
- P2-11: verification CLI (hand-rolled JSON, no serde/clap); 96/96. *Process note:* recovered prior subagent's empty payload.
- phase2-qa: full audit 2026-09-12 — 8/8 runnable gates PASS; 159/159 tests.

Cycle 2 close-out (2026-09-12): all 11 `branch/chunk-P2-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.

## RamSleuth v2 — Cycle 1 (Phase 1: Native Benchmark Engine) — 2026-09-11

### Delivered
- 6-crate Cargo workspace scaffolded (P1-01); runtime AVX2/AVX-512F feature detection (P1-02, `detect() -> CpuTopology`).
- `ramsleuth-bench` benchmark engine complete:
  - /sys physical-core enumeration with SMT filter + total/per-CCD L3 (P1-02).
  - L1/L2/L3/DRAM buffer sizing via pure `plan(&CpuTopology) -> BufferPlan`, all 64-byte aligned: sysfs L1/L2 with safe fallbacks, per-CCD L3 slice, DRAM max(256 MiB, 3× total L3), 128 MiB latency ring (P1-03).
  - AVX2 streaming read kernel — unrolled 4-wide aligned loads, lane accumulation, word-sum checksum, feature-dispatched scalar fallback (P1-04); AVX2 non-temporal write kernel — NT stores + single trailing SFENCE (P1-05); AVX2 copy kernel — load/NT-store pairs, destination checksum via avx2_read (P1-06).
  - AVX-512F read/write/copy variants (`_mm512_*` aligned/NT/SFENCE shapes) with documented AVX2 fallback when AVX-512F is absent; clippy msrv 1.89 gating (P1-07).
  - Pinned barrier-synced multi-thread worker dispatch — one `sched_setaffinity`-pinned worker per physical core, Barrier lockstep, exact-once block-aligned partition, checksum aggregation, graceful unpinned fallback; libc 0.2 the only new dep (P1-08).
  - 64-byte-stride pointer-chase latency kernel — SplitMix64 Fisher-Yates single-cycle ring, strictly dependent loads, serialized `__rdtscp` with self-calibrating Instant conversion + non-x86 fallback (P1-09).
  - Orchestrator aggregating the 4×4 AIDA64-style grid (Memory/L3/L2/L1 × Read/Write/Copy + Latency); DRAM-tier latency chases the materialized full-DRAM-size buffer (beyond L3); safe 64B-aligned `AlignedBuf`; best-of-3 Instant timing owned by the orchestrator (P1-10).
  - Verification CLI: `cargo run -p ramsleuth-bench [--avx512] [--json]` — std-only option parsing (unknown flag → exit 2), serde-free JSON (`read_gbps/write_gbps/copy_gbps/latency_ns`, non-finite → null), fixed-width grid renderer, graceful exit 1 on detect/run errors, documented AVX-512→AVX2 fallback note (P1-11).

### Quality
- 63/63 tests green (debug + release); zero clippy warnings (`clippy --workspace --all-targets -- -D warnings`); release build OK; live bench run exit 0 (default, `--json` output parsed, `--avx512` fallback path verified).
- QA audit 2026-09-12 on `v2-development` @ b5eea35: 7/7 runnable gates PASS; AIDA64 parity gate deferred to the DDR5-6000 AM5 host (this host is Zen 3 / DDR4).

### Measured grid (this host: Ryzen 9 5950X, Zen 3, 16 phys / 32 logical, 64 MiB L3, DDR4, AVX2-only — informational)
| Tier | Read (GB/s) | Write (GB/s) | Copy (GB/s) | Latency (ns) |
| --- | --- | --- | --- | --- |
| Memory (DRAM) | 49.13 | 43.83 | 15.75 | 81.5 |
| L3 | 59.03 | 31.28 | 16.29 | 56.5 |
| L2 | 1.33 | 1.41 | 1.40 | 5.6 |
| L1 | 0.08 | 0.09 | 0.09 | 1.2 |

Repeat runs (variance context): `--json` → DRAM 47.58 / 43.37 / 15.68 GB/s @ 81.4 ns; L3 63.04 / 32.02 / 16.88 GB/s @ 39.5 ns. `--avx512` → DRAM 48.89 / 43.48 / 15.67 GB/s @ 82.9 ns. L3 drifts ~±25% run-to-run (best-of-3 at these sizes is noisy). Sanity: latency ordering monotonic L1 < L2 < L3 < DRAM; no negative/zero/NaN cells; DRAM read ≈ 48% of DDR4-3200 4-channel theoretical peak.

### Known follow-ups
1. L1/L2 bandwidth cells are overhead-limited: the small tier working set (32 KiB L1d / 1 MiB L2) split across 16 pinned workers is dominated by thread/barrier/timing overhead per pass. Add inner-loop iterations to amortize that overhead for small tiers (Phase 2 design note; cells not comparable across tiers).
2. Confirm §1.3 AIDA64 parity (DRAM BW ±5%, latency 60–75 ns band) on the target DDR5-6000 AM5 machine — this host measured DRAM latency 81.4–83.5 ns (out of band as expected on DDR4; reported, not failed).
3. Workspace MSRV 1.75 vs AVX-512F intrinsics requiring Rust 1.89 — decide on an MSRV bump.
4. Push strategy for `v2-development` vs the divergent legacy upstream `master` is pending user decision (no push from this cycle).

### Per-chunk history (summarized from DEV_LOG.md)
- P1-01: runtime CPU feature detection module (merged f0cb26e).
- P1-02: `detect() -> Result<CpuTopology, TopologyError>` — /sys topology enumeration, SMT filter; 13/13 tests.
- P1-03: pure buffer planning/sizing, 64-byte aligned; 23/23 tests.
- P1-04: AVX2 streaming read + word-sum checksum; 27/27 tests.
- P1-05: AVX2 non-temporal write; 31/31 tests.
- P1-06: AVX2 copy (NT stores + SFENCE); 35/35 tests.
- P1-07: AVX-512F read/write/copy with AVX2 fallback; 40/40 tests; 512-bit bodies compiled but not directly exercised on this AVX2-only host.
- P1-08: pinned barrier-synced worker dispatch; 47/47 tests.
- P1-09: pointer-chase latency kernel; 53/53 tests; flagged that a true DRAM row requires the chased working set to exceed L3 (resolved in P1-10).
- P1-10: orchestrator + 4×4 grid + DRAM latency materialization; 60/60 tests.
- P1-11: verification CLI; 63/63 tests; contract review passed.
- phase1-qa: full audit 2026-09-12 — 7/7 runnable gates PASS, AIDA64 parity gate deferred to AM5.

Cycle 1 close-out (2026-09-12): all 11 `branch/chunk-P1-*` branches verified fully merged into `v2-development` and pruned; no unmerged branch touched.
