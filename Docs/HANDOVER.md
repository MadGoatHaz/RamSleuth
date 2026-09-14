# RamSleuth v2 — Development Handover

> **Audience:** the next development cycle (**Cycle 5 — live-hardware verification + remaining model reconciliation**). Self-contained: read this first, then `FULLSCOPEvsCOMPLETED.md`, the other `Docs/` files, and `plans/`.
> **Date:** 2026-09-13 (Cycle 4 close-out). **Branch:** `v2-development` — local tip = the Cycle 4 final compaction **`2409947`** + this docs handover commit, i.e. **2 commits ahead of `origin/v2-development` @ `b908f7b`** (both local-only, unpushed; see §8).
> **Sources of truth:** this file + `FULLSCOPEvsCOMPLETED.md` (full scope vs. completed, with the mermaid map) + `Docs/RamSleuth-v2.md` (roadmap) + `Docs/Grand Design & Architecture Specification.md` (spec) + `plans/PLAN*.md` (per-phase plans — Phases 1–5 all done) + `MASTER_LOG.md` (durable per-cycle history, Cycles 1–4) + `DEV_LOG.md` (lease board — reset at Cycle 4 close, no active leases).
>
> **Full scope reference:** for the full project scope vs. completed breakdown (with mermaid map), see `FULLSCOPEvsCOMPLETED.md` at the workspace root.

---

## 1. Project Overview

RamSleuth v2 is a dual-layer hardware introspection + benchmarking utility for Linux x86_64:

1. **Live memory-controller telemetry** — active trained subtimings, clocks (MCLK / UCLK / FCLK, gear/div modes, GDM/PDM), CAD-bus drive strengths & terminations, and voltages (AMD via the `ryzen_smu` SMU PM table; Intel via MCHBAR MMIO), plus SPD EEPROM decode (JEP106 module/die makers, rank, XMP 2.0/3.0, EXPO).
2. **AIDA64-style benchmark engine** — multi-threaded AVX2/AVX-512 non-temporal SIMD bandwidth (Read / Write / Copy × Memory / L3 / L2 / L1) and pointer-chase latency.

End goal: unprivileged TUI (ratatui) and GUI (egui/eframe) frontends served by a privileged daemon — **delivered in Cycle 3 (Phases 3 + 4)**; packaging & distribution — **delivered in Cycle 4 (Phase 5)**. The application is complete. **Cycle 5 is the live-hardware verification + remaining model-reconciliation cycle** (open items, §5): the AMD ground-truth cross-check is now unblocked (ryzen_smu is installed and live AMD subtimings decode), Intel MCHBAR decode awaits the i5-6600 machine, and the remaining model fields (CAD/timings via the driver's `smn` attr, SPD maker/density codes, Intel IMC offsets) are still to reconcile.

**Mandate:** 100% pure Rust, Cargo workspace (Edition 2021, resolver 2), Linux x86_64. No C++/Python/JS components.

## 2. Current State (2026-09-13 — Cycle 4 close-out, ready for Cycle 5)

- **Phases 1–5: ALL COMPLETE.** 7 crates: `ramsleuth-bench` (P1), `ramsleuth-telemetry` (P2), `ramsleuth-protocol` + `ramsleuth-daemon` + `ramsleuth-client` (P3), `ramsleuth-tui` + `ramsleuth-gui` (P4, folded into Cycle 3), and Phase 5 = packaging & distribution (Cycle 4, §3).
- **Cycle 4 (Phase 5) deliverables — 15 chunks (P5-01…P5-15), all reviewed, `--no-ff` merged into `v2-development`, compacted into `MASTER_LOG.md`:**
  - **P5-01…P5-07 — packaging:** AUR `ramsleuth-git` (`PKGBUILD` + `.install` hooks creating the `ramsleuth` group on the target + systemd `preset` enabling the service), optional `ryzen-smu-dkms` AUR extra (`PKGBUILD` + `dkms.conf`, safe no-in-chroot design), the idempotent operator install helper `scripts/install-ryzen-smu-dkms.sh`, GitHub Actions CI (`1.75`/`stable` matrix: test debug+release, clippy `-D warnings`, 6-binary artifact), and the packaging README.
  - **P5-08/09/10 — ryzen_smu uAPI reconciliation:** the daemon's canonical sysfs path is now `/sys/kernel/ryzen_smu_drv/pm_table` (the amkillam upstream registers the kobject as `ryzen_smu_drv`; the legacy `/sys/kernel/ryzen_smu/` path remains a secondary candidate), the install script's default upstream is `amkillam/ryzen_smu`, and the README is consistent.
  - **P5-11/12/13/14 — ryzen_smu install-path fixes** (root-caused from three live operator runs): stage the source into `/usr/src/ryzen_smu-$PKGVER` before `dkms add` (P5-11); dkms.conf `MAKE`/`CLEAN` aligned to the upstream amkillam pattern (the missing `/build` dir broke `dkms build`) (P5-12); `dkms build`/`install` now pass `${MODULE}/${PKGVER}` (DKMS 3.4.3 does not resolve a bare name) (P5-13); the "already installed" skip-check now uses `dkms status` install-state instead of build-dir existence (a built-not-installed module was wrongly skipped) (P5-14).
  - **P5-15 — AMD PM-table model reconciliation** (open item 3, Vermeer): the first Rust-source change of the cycle (`amd_smu.rs` + `amd_pm.rs` only) — the version is now sourced from the sibling `pm_table_version` file (`TableVersionId`), the blob is parsed as a headerless `f32` array (FCLK 0x0C0 / UCLK 0x0C8 / MCLK 0x0CC / VDDCR_SOC 0x0B0, MIN_LEN 0x518) accepting the Vermeer + Matisse `TableVersionId` sets (full model in §4). 328→337 tests.
- **QA baseline (re-verified on 2026-09-13 by this handover pass):** **337/337 whole-workspace tests green in debug AND release**; `cargo clippy --workspace --all-targets -- -D warnings` → **zero warnings**; **MSRV 1.75** held (lockfile-pinned; AVX-512F intrinsics cfg-gated, never exercised on this Zen 3 host); release build OK.
- **Live AMD subtimings: NOW WORKING on the 5950X.** `ryzen_smu` (amkillam v0.1.7) is installed + loaded via DKMS; the daemon reads the PM table and reports **MCLK/UCLK/FCLK ≈ 1792 MHz (OneToOne)** and **VDDCR_SOC ≈ 1.128 V** (matches the `monitor_cpu` ground truth within ±10 mV). CAD / timings / other rails = honest `Na` — they live in SMN registers exposed by the driver's `smn` sysfs attr (Cycle 5 follow-up, §5.3a; **not** raw MMIO). The 1792-vs-1800 MHz delta needs investigation under matched conditions (§5.1).
- **Live result on the reference host today (unprivileged client against a root daemon):** `CPU: Amd(Zen3) — AMD Ryzen 9 5950X`; `AMD:` populated (clocks + VDDCR_SOC; CAD/timings honest `Na`); `Intel: N/A (UnsupportedHardware)`; SPD: 2× DDR4 modules (rank=1, 3200 MT/s; density `0x0D` → `N/A`, maker `0xC1` shown raw-hex — open item §5.3b). No panic in any privilege/CPU state.
- **Verification CLIs (all work today, exit 0):**
  - `target/release/ramsleuth-daemon` — the single privileged process (root; or `--socket /tmp/ramsleuth.sock` for an unprivileged dev socket — the SOFT caps probe warns and keeps serving).
  - `target/release/ramsleuth-client -- dump | status | bench [--tier memory|l1|l2|l3|full] [--mode full|memory-only]` — **no sudo needed**; `dump` is the exit-criterion command; no daemon running → exit 1 + friendly `DaemonDown` hint (never a crash).
  - `target/release/ramsleuth-tui` — terminal dashboard (R = refresh, S = snapshot, Q = quit; background 2 s updater).
  - `target/release/ramsleuth-gui` — desktop dashboard (F2 = PNG snapshot, F3 = JSON export to `$HOME`, Q = quit).
  - `sudo monitor_cpu` — the ryzen_smu ground-truth CLI (installed at `/usr/bin/monitor_cpu`, mode 700 root; §6) — the comparison target for open item 1.
  - Direct verification CLIs (no daemon; from the Cycle 1/2 lineage, still valid): `cargo run -p ramsleuth-bench` — full 4×4 grid (text + hand-rolled JSON); optional `--avx512` (falls back to AVX2 when the host lacks AVX-512F — as on this host) and `--json` flags; unknown flag → exit 2. And `cargo run -p ramsleuth-telemetry` — dashboard-style telemetry listing; every cell renders as a value or `N/A (<reason>)`; optional `--json`; **exit 0 even when all sections are `Na`** (a structured N/A is a valid outcome). Run as root for live AMD data (the module is installed — §6).

## 3. Cycle 4 Summary (2026-09-13)

What was built: **packaging & distribution** for the completed 7-crate app, plus the **ryzen_smu install** (live AMD subtimings) and the **AMD PM-table model reconciliation** — 15 chunks total, all single-file, all `--no-ff` merged (see §2 for the per-chunk breakdown and `MASTER_LOG.md` for the full per-chunk history):

- **Packaging (P5-01…P5-07):** the app installs via `makepkg -si` from `packaging/ramsleuth-git/` (the single install path — AUR submission is a downstream convenience): 6 binaries → `/usr/bin` (`ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui`, `ramsleuth-bench`, `ramsleuth-telemetry`; `ramsleuth-protocol` is lib-only, not installed), the frozen `systemd/ramsleuth.service` → `/usr/lib/systemd/system/`, the enabling preset → `/usr/lib/systemd/system-preset/`, and the **`ramsleuth` group created on the target** by the `.install` `pre_install`/`pre_upgrade` hooks (idempotent `getent || groupadd -r`) — closing the one real install-dependency gap (the unit uses `Group=ramsleuth`). Build is `cargo build --release --locked` (committed pins, reproducible); makedepends cover the eframe/winit GUI surface (`pkgconf` + `libxkbcommon` the only strict build-time link dep). Zero `.rs` / zero `Cargo.*` changes in P5-01…P5-07.
- **Optional AMD extra (P5-03/04/05 + P5-11…P5-14):** `packaging/ryzen-smu-dkms/` ships the `dkms.conf` + the install helper (safe no-in-chroot: the operator runs `ryzen-smu-dkms-install` on the target); the helper (final state after P5-11…P5-14) is the fully-working DKMS path — staging → `dkms add/build/install ${MODULE}/${PKGVER}` → `modprobe` → verify — with `AUTOINSTALL=yes` auto-rebuild on kernel updates.
- **CI (P5-06):** `.github/workflows/ci.yml` — `cargo test --workspace` (debug + release) + clippy `-D warnings` on a `["1.75", "stable"]` matrix (`fail-fast: false`) + a release build job uploading the 6-binary artifact. The committed `Cargo.lock` is never regenerated in CI; the MSRV 1.75 leg continuously proves the lockfile-pins + cfg-gated-AVX-512 claim on a clean runner (this host has no rustup, so the local MSRV check is delegated to CI).
- **ryzen_smu uAPI + model reconciliation (P5-08…P5-10, P5-15):** see §4 (the model) and §6 (the install). P5-15 is the only Rust-source change of the cycle (`amd_smu.rs` + `amd_pm.rs`); everything else was packaging/script/docs.

QA: 337/337 tests (debug + release), zero clippy warnings, MSRV 1.75, 9/9 packaging/CI/systemd files valid (`bash -n` ×4, shellcheck zero findings, YAML valid), live end-to-end on the host (AMD section populated), **zero panics / segfaults**. Compacted into `MASTER_LOG.md`.

## 4. The AMD PM-Table Model (reconciled in Cycle 4 — P5-15)

The Phase 2 AMD parse was a plan-mandated SKELETON (version-guarded + bounds-checked, but the numeric model unverified). P5-15 reconciled it against live Vermeer silicon + the `monitor_cpu` parser (the verified reference). Current model (`crates/ramsleuth-telemetry/src/amd_smu.rs` + `amd_pm.rs`):

- **Version source — the sibling file:** the PM-table version is the `TableVersionId` in `/sys/kernel/ryzen_smu_drv/pm_table_version` (4-byte LE `u32`), **not** in the blob. The blob is a headerless `f32` array — its first word is a PPT-limit float, which the old skeleton misread as the version (hence the prior `N/A (unknown PM table version)`). Candidate list: `ryzen_smu_drv` first, legacy `ryzen_smu` second; when the sibling attr is absent, the first word of the blob is used as a fallback (degrades to `UnknownPmTableVersion`, never a hard error; a truncated attr → `Parse`). The sibling `pm_table_size` attr (8-byte LE `u64`) is cross-checked against the blob length (mismatch → `Parse`; absent → no check).
- **Layout — headerless LE `f32` array, MIN_LEN 0x518 (326 × f32):** `VDDCR_SOC` @ 0x0B0 (volts → mV ×1000), `FCLK` @ 0x0C0, `UCLK` @ 0x0C8, `MCLK` @ 0x0CC (MHz). `div_mode` is derived (UCLK == MCLK → OneToOne).
- **Accepted versions:** **Vermeer / Zen 3** (this host): `{0x2D0803, 0x2D0903, 0x380005, 0x380505, 0x380605, 0x380705, 0x380804, 0x380805, 0x380904, 0x380905}` — the operator's live `TableVersionId` = **0x380805** (in-set; the live `pm_table_version` bytes are `05 08 38 00`, `pm_table_size` = 2288). **Matisse / Zen 2**: `{0x240003, 0x240503, 0x240603, 0x240703, 0x240802, 0x240803, 0x240902, 0x240903}` (shares the layout). The legacy 7.11.x/12.x/13.x SMU-FW major/minor match, the u16-region `PmTableLayout`, and the blob-header cross-check are all **gone** (tests pin rejection of the legacy words). Note: plan D2's "5950X = SMU 7.11.x" was wrong — Vermeer's PM `TableVersionId` family bytes are 0x2D/0x38.
- **What is NOT in the PM table:** CAD-bus drive strengths/terminations, the 27 DRAM subtimings, GDM/PDM, VDDIO_MEM, VPP — these live in **SMN registers**. The driver exposes an `smn` sysfs attr (`/sys/kernel/ryzen_smu_drv/smn`, confirmed present on this host) — **the sanctioned read channel for the Cycle 5 follow-up (open item 3a) is that driver attr (SMN regs `0x50200`–`0x50264` bitfields), NOT raw MMIO.** Until then those fields are honest `Na` (zeroed under the existing P2-05 sanity gates).
- **Char-dev fallback:** the `/dev/ryzen_smu` ioctl path (P2-03) is retained as a 53XU-fork tolerance only — the amkillam module does not create that char-dev.
- **Live result:** MCLK/UCLK/FCLK ≈ 1792 MHz OneToOne, VDDCR_SOC ≈ 1.128 V on the 5950X (matches `monitor_cpu` within ±10 mV).

## 5. Open Items for Cycle 5 (live-hardware verification + remaining reconciliation)

1. **AMD tick-identical ground truth (open item 1) — NOW UNBLOCKED.** Do the cross-check: under matched conditions (same system state, idle, no load), run `sudo monitor_cpu` and `ramsleuth-client -- dump` (root daemon) and compare: **clocks ±1 MHz, voltages ±10 mV, CAD per the code table** (CAD is currently `Na` — the comparison waits on item 3a's SMN path). **Investigate the 1792-vs-1800 MHz delta** — is it system-state variation (the two tools sampled at different instants) or a scaling issue? Confirm under matched conditions (sample both at the same instant; also read the driver's `codename` / `drv_version` / `version` attrs for context).
2. **Intel live MCHBAR decode (open item 2) — needs the i5-6600 machine.** On the LGA-1151 i5-6600 (Skylake, dual-channel = `channel_count(Skylake) = 2`): run the daemon as root (or `sudo cargo run -p ramsleuth-telemetry --release`); confirm per-channel tCL/tRCD/tRP/tRAS + command-rate/gear decode against known-good values; confirm Intel voltages/CAD = `Na(NotApplicable)`. The Intel IMC register offsets are still SKELETON (item 3c) — so the first live pass also serves to validate the decode pipeline + reconcile the offsets against the live registers.
3. **Model reconciliation (open item 3) — PARTIALLY DONE** (the Vermeer PM table was reconciled in P5-15). Remaining:
   - **(a) CAD/timings via the driver's `smn` sysfs attr** — the 27 DRAM subtimings + GDM/PDM + CAD drive strengths/terminations live in SMN regs `0x50200`–`0x50264` (bitfields). The sanctioned channel = the driver's `smn` attr (read/write via sysfs, as `monitor_cpu` does), **NOT raw MMIO** — this is a daemon-side extension to the existing AMD read path (a new `smn` accessor + decode tables; the no-panic contract applies — unknown register content → honest `Na`).
   - **(b) SPD density `0x0D` + maker `0xC1`** — observed on the 5950X host, outside the frozen decode tables (density → `Na`, maker shown raw-hex). Reconcile against the SPD spec / live module, then update the decode tables + fixtures.
   - **(c) Intel IMC offsets** — still SKELETON (plan-mandated); needs the i5-6600 for live validation (item 2).
4. **Phase 1 L1/L2 bandwidth overhead refinement (open item 4) — small code change.** The small-tier working sets (32 KiB L1d / 1 MiB L2) split across 16 pinned workers are dominated by per-pass thread/barrier/timing overhead → the cells are not comparable across tiers. Fix: **add inner-loop iterations** in the small-tier passes to amortize that overhead (the large-tier passes stay as-is; checksum conventions frozen — §9 item 6).
5. **MSRV decision (open item 5) — operator call.** Keep the workspace at **1.75** (lockfile-pinned for the ratatui/egui transitive deps; the AVX-512F intrinsics are cfg-gated and never exercised on Zen 3; CI's `1.75` leg proves it on a clean runner) **vs. bump to 1.89**. Deferred; whichever is chosen, preserve/adjust the lockfile pins accordingly (bumping requires re-verifying the ratatui/egui MSRV pins and the CI matrix).
6. **Push to GitHub (open item 6) — on operator go-ahead** (see §8): fast-forward `origin/v2-development` to the local tip (which includes `2409947`), prune the 17 fully-merged remote `branch/chunk-p5-*` branches, optional tag (e.g. `v2.0.0`) — all as a deliberate, reviewed event; never force-push.

## 6. ryzen_smu Installation (current state + how it is managed)

- **Upstream: `amkillam/ryzen_smu`** (branch `main`, **v0.1.7** — actively maintained, kernel 7.2+ fix, Zen 3 support; builds the `ryzen_smu` module, registers the `ryzen_smu_drv` kobject, ships a `dkms.conf` + the `monitor_cpu` userspace CLI). Upstream selection was reconciled in P5-08…P5-10 (post-merge, 2026-09-13): the original `53XU/ryzen_smu` is **DEAD (HTTP 404)** — the script warns about it explicitly — and `leogx9r/ryzen_smu` (original, frozen 2021) was rejected in favor of the active amkillam fork. The URL remains overridable via `RYZEN_SMU_URL`; **verify the upstream at setup time, do not trust a cached URL**.
- **Installed on this host (verified 2026-09-13):** `dkms status` → `ryzen_smu/1.d298366, 7.2.3-1-cachyos-custom, x86_64: installed`; `modinfo ryzen_smu` → v0.1.7 (author Leonardo Gates); module loaded (`lsmod`). Sysfs at `/sys/kernel/ryzen_smu_drv/`: `pm_table` (2288-byte blob), `pm_table_version` (`05 08 38 00` = 0x380805), `pm_table_size` (0x8F0 = 2288), **`smn` (rw — the item 3a channel)**, `codename`, `drv_version`, `version`, `hsmp_smu_cmd`, `mp1_smu_cmd`, `mp1_if_version`, `rsmu_cmd`, `smu_args`.
- **Ground-truth CLI: `/usr/bin/monitor_cpu`** (mode 700, root-only — the install script builds it from upstream's `userspace/` dir). The verified reference for the PM-table parse; the comparison target for open item 1.
- **Install / management: `scripts/install-ryzen-smu-dkms.sh`** — idempotent, operator-run (re-execs under sudo when not root):
  1. **Fast path:** `pm_table` present (canonical `ryzen_smu_drv` first, legacy `ryzen_smu` second) → "nothing to do", exit 0. (Re-runs after a kernel update hit this once DKMS `AUTOINSTALL=yes` has rebuilt + the module is loaded.)
  2. **Prereqs:** `pacman -S --needed dkms base-devel`; kernel build-tree guard — `[[ -d /lib/modules/$(uname -r)/build ]]` or stop with the candidate headers packages listed (**never guesses a custom-kernel headers package** — on this host: the `7.2.3-1-cachyos-custom` headers).
  3. **Clone:** shallow clone of the verified upstream → `/opt/ryzen-smu-src` (re-runs `git pull --ff-only`, tolerating offline).
  4. **Stage (P5-11):** copy `{LICENSE, Makefile, dkms.conf, drv.c, smu.c, smu.h}` into `/usr/src/ryzen_smu-$PKGVER` (`PKGVER` = `git rev-list --count HEAD`.`short-hash`); prefer the repo's `packaging/ryzen-smu-dkms/dkms.conf` (dual-location fallback: repo path, then installed `/usr/share/ryzen-smu-dkms/`), with its `PACKAGE_VERSION` sed-aligned to `$PKGVER`; write `/usr/lib/depmod.d/ryzen_smu.conf` (`override ryzen_smu /extra/ryzen_smu.ko`); build + install `monitor_cpu` → `/usr/bin` (non-fatal if absent).
  5. **DKMS (P5-12/13/14):** `dkms add ${MODULE}/${PKGVER}` (already-registered → continue) → `dkms build ${MODULE}/${PKGVER} -k ${KERNEL}` → install-state check via `dkms status` (state exactly `installed` → skip; `built` → proceed) → `dkms install ${MODULE}/${PKGVER} -k ${KERNEL}`. The staged `dkms.conf` uses the authoritative upstream pattern (`MAKE="make TARGET=${kernelver}"`; DKMS runs in `.../build`; `DEST_MODULE_LOCATION=/extra` matches the depmod override).
  6. **Load + verify:** `modprobe ryzen_smu` + `/etc/modules-load.d/ryzen_smu.conf` (boot persistence); verify `pm_table` present (canonical first, legacy second) with a `dmesg`/`dkms status` hint on failure. Without the module the app still works: `N/A (DriverMissing)`, exit 0, no panic.
- **Key carried facts:** the frozen `systemd/ramsleuth.service` **never loads the module** (it assumes loaded; the `.install`/DKMS `AUTOINSTALL` path handles it) — a bare install degrades gracefully per the no-panic contract. The AUR extra `packaging/ryzen-smu-dkms/` (P5-05) is a safe no-in-chroot provisioning package: it ships the `dkms.conf` + the helper as `/usr/bin/ryzen-smu-dkms-install`; the operator runs the helper on the target. We deliberately do **not** depend on the external AUR `ryzen-smu-dkms-git` package (self-contained tooling; only the module source is fetched from amkillam git at install time).

## 7. Verification Environment

### (a) Primary AMD dev host — Ryzen 9 5950X (Zen 3, Vermeer)

- 16C/32T, 64 MiB L3, DDR4-3200 (2× modules, rank-1), AVX2 — **no AVX-512** (the `--avx512` path falls back to AVX2; the 512-bit kernels compile but the runtime gate selects AVX2).
- **OS: CachyOS, kernel `7.2.3-1-cachyos-custom`.**
- **`ryzen_smu` kernel module: NOW INSTALLED + LOADED** (amkillam/ryzen_smu v0.1.7 via DKMS — §6). Live AMD subtimings **work**: the daemon decodes the PM table (MCLK/UCLK/FCLK ≈ 1792 MHz OneToOne, VDDCR_SOC ≈ 1.128 V; CAD/timings honest `Na` until the item 3a SMN path).
- Ground truth: `sudo monitor_cpu` (`/usr/bin/monitor_cpu`); the driver's `smn` attr is available for the CAD/timings follow-up.

### (b) Intel test machine — i5-6600 (Skylake)

- LGA-1151, Intel Core i5-6600 (6th-gen Skylake), **dual-channel (2 DIMM channels)** — matches `channel_count(Skylake) = 2` in the Intel decode model.
- Purpose: **live Intel MCHBAR decode verification** (BAR5 → read-only `/dev/mem` mmap → per-channel IMC registers, open item 2). The Phase 2 Intel path is Intel-gated and returns `N/A (UnsupportedHardware)` on the AMD host, so this machine is where the decode gets its live proof.
- Intel voltages/CAD-bus sections are out of Phase 2 scope by design → `Na(NotApplicable)` on Intel.

### (c) Deferred target

- The AIDA64 parity gate (±5% DRAM bandwidth; 60–75 ns DDR5-6000 AM5 latency band) is deferred to a **DDR5-6000 AM5 host** — the 5950X host is DDR4 (measured DRAM latency ~81.5 ns is out of that band as expected; reported, not failed).

## 8. Push Policy & Git State

- **Git state (2026-09-13, verified):** local `v2-development` is **2 commits ahead of `origin/v2-development` @ `b908f7b`** (the P5-15 review/merge commit — everything up to `b908f7b` is on origin: all 15 P5 chunks + the 2 earlier fixes, fast-forwarded during the cycle, no force-push): **`2409947`** (the Cycle 4 final compaction commit) + this docs handover commit. Both are **local-only, unpushed**. Tree clean; single local branch.
- **17 remote `branch/chunk-p5-*` branches on origin — all fully merged into `v2-development`** → prune candidates on go-ahead: `p5-01`, `p5-02`, `p5-02-fix`, `p5-03`, `p5-04`, `p5-04-fix`, `p5-05`, `p5-06`, `p5-07`, `p5-08-sysfs`, `p5-09-script`, `p5-10-readme`, `p5-11-dkms`, `p5-12-dkmsconf`, `p5-13-dkmsver`, `p5-14-dkmsinstall`, `p5-15-pmtable`.
- **`origin/master` = divergent legacy (untouched).** The operator confirmed `v2-development` is the canonical line — **no need to preserve or branch from `master`**; treat upstream (`https://github.com/MadGoatHaz/RamSleuth`) as a read-only reference. Never merge into or force onto it.
- **Push policy: LOCAL only until the operator's explicit go-ahead — do not push, do not delete any remote branch (no exceptions).** On go-ahead, in order: (1) **fast-forward** `origin/v2-development` to the local tip (≥ `2409947`) (NEVER force-push — the push is a strict ff); (2) prune the 17 remote chunk branches (`git push origin --delete <branch>` — every one verified fully merged; the `--no-ff` merge history lives on `v2-development`); (3) **optional tag** (e.g. `v2.0.0`) — decide at push time, with explicit sign-off.
- `DEV_LOG.md` is the active lease board (reset at Cycle 4 close-out — no active leases); `MASTER_LOG.md` holds the durable per-cycle history (Cycles 1–4 + the 2026-09-12 decisions record).

## 9. Key Architectural Decisions & Frozen Interfaces (carried forward — still accurate)

1. **No-panic `Section<T>` contract (P2-02, frozen):** displayable telemetry values are `Section<T> { Value(T), Na(TelemetryError) }`; fallible operations return `TelemetryResult<T>`. `TelemetryError` variants: `UnsupportedHardware`, `UnsupportedVendor`, `DriverMissing`, `InsufficientPrivilege`, `UnknownPmTableVersion`, `NoDevmem`, `NotApplicable`, `InvalidValue(String)`, `Io(String)`. **No `panic!`, no `unwrap()`/`expect()` on hardware-derived data, no unguarded deref** anywhere in the telemetry crate; every `unsafe` block carries `// SAFETY:` and is bounds-checked. The CLI renders every missing value as `N/A (<reason>)` and exits 0.
2. **Direct SMU access — no `ryzen_smu` Rust crate (P2-03, decision D1):** sysfs-first read of the PM table — canonical `/sys/kernel/ryzen_smu_drv/pm_table` (P5-08) with the legacy `/sys/kernel/ryzen_smu/pm_table` as a secondary candidate — plus the sibling `pm_table_version` / `pm_table_size` attrs (§4); fallback open of `/dev/ryzen_smu` + driver ioctl via `nix` (53XU-fork tolerance only). We own the version-guarded PM parse and all mapping. `nix` (features `fs`/`ioctl`/`mman`) is the **only** Phase 2 dependency and the only telemetry dep. Rejected: `ryzen_smu` crate, `memoffset`, serde/serde_json (hand-rolled JSON in the telemetry CLI), clap (std `env::args`).
3. **AMD PM model RECONCILED for Vermeer (P5-15, §4): the f32 layout + `TableVersionId` sets are now live-verified** (clocks + VDDCR_SOC decode on the 5950X). **Intel IMC register offsets (P2-07) remain plan-mandated SKELETONS** pending live validation on the i5-6600 (open item 3c). CAD/timings/GDM/PDM/other rails are honest `Na` until the `smn`-attr path (open item 3a).
4. **Intel MCHBAR gating (P2-06, decision D4):** `CpuInfo` must be Intel before **any** file/mmap access; `/dev/mem` is mapped **read-only** behind a guard struct that bounds-checks reads and `munmap`s in `Drop`; STRICT_DEVMEM EIO/ENODATA → `InsufficientPrivilege`; missing file → `NoDevmem`. On the AMD host the path returns `Na(UnsupportedVendor)`/`Na(UnsupportedHardware)` and verifiably never touches `/dev/mem`.
5. **SPD is unprivileged (P2-08 / P2-09):** raw `ee1004` sysfs images (512 B DDR4 / 1024 B DDR5); JEP106 module + die makers (continued-ID support), rank bits, part/serial, JEDEC speed, XMP 2.0/3.0 + EXPO profile summaries; a checksum failure skips the profile, never the module; no panic on truncated/garbage data.
6. **Phase 1 checksum conventions (frozen):** read/copy kernels return the **wrapping sum of the buffer's little-endian 64-bit words** (data-sensitive DCE sink; `0` for a zeroed buffer); the scalar fallback has identical semantics; write kernels' frozen return is the **byte counter** (so `checksum == total_bytes` for Write); per-slice word-sums wrapping-add to exactly the whole-buffer checksum (proves the partition has neither overlap nor gap); all tier buffers are 64-byte aligned (`AlignedBuf`); bandwidth cells are **best-of-3** `Instant` timings, latency cells the **median** of 3 runs of a ≥1 M-hop chase over the *materialized* full-tier ring (Memory tier exceeds L3 → true DRAM latency); non-x86_64 latency falls back to `Instant` only (documented precision loss).
7. **CPUID family mapping (P2-01, frozen):** the 5950X reference host reports **family `0x19` → `Amd(Zen3)`** (frozen map: `0x15`→Zen 1, `0x17`→Zen 2, `0x19`→Zen 3, `0x1A`→Zen 4, `0x1C`→Zen 5, else `Unknown`). **Note: desktop Zen 4/5 silicon also reports family `0x19` on some boards, so the AMD PM parse (P2-04) keys on the SMU version — now the exact `TableVersionId` sets (§4) — not on `AmdZen`** — generation ambiguity cannot break the PM layout. Intel: family `0x6` model table covers Skylake (`0x4F`/`0x56`/`0x5E`) through Arrow Lake; unrecognized models → `IntelGen::Unrecognized` (still Intel-gated, not `Unknown`).
8. **Topology & buffers are pure (P1-02 / P1-03):** `detect() -> Result<CpuTopology, TopologyError>` over /sys (SMT siblings filtered; per-CCD L3 slices; total L3); `plan(&CpuTopology) -> BufferPlan` is a pure function (L1 16 KiB, L2 256 KiB, L3 = 50% of a CCD slice, DRAM = max(256 MiB, 3× total L3), latency ring 128 MiB; all 64-byte aligned; safe fallbacks when sysfs fields are missing).
9. **Phase 3/4 privilege + wire contract (Cycle 3, frozen):** one privileged daemon is the **sole privilege boundary** — all hardware I/O (ryzen_smu sysfs/char-dev, MCHBAR `/dev/mem`, ee1004) stays daemon-side; the client/TUI/GUI chain is tokio-free and never touches hardware. Wire: synchronous length-prefixed **Bincode frames** with a **16 MiB `MAX_FRAME_SIZE` guard** over a **0660 Unix socket** (`/run/ramsleuth/ramsleuth.sock`); tokio confined to the daemon (async accept loop + `spawn_blocking` pumps); serde derives on every wire-crossing public type (payloads reused verbatim — no duplication, no boxing of frozen arm shapes). The daemon **inherits the no-panic contract**: any absent driver/privilege/hardware surfaces as structured `Na` sections in the payload — never a crash of the service or the clients.

## 10. Workspace Layout (7 crates)

| Crate | Status | Contents |
|---|---|---|
| `crates/ramsleuth-bench` | **DONE (Phase 1)** | `features` (AVX2/AVX-512F runtime detect), `topology` (/sys physical-core enumeration, SMT filter, total/per-CCD L3), `buffers` (pure 64B-aligned sizing plan), `kernel_read` / `kernel_write` / `kernel_copy` (AVX2 streaming / NT / copy), `kernel_512` (AVX-512F variants + AVX2 fallback), `worker` (pinned barrier-synced per-core dispatch), `latency` (pointer-chase ring, `__rdtscp`), `orchestrator` (4×4 grid, best-of-3 / median), `main` (verification CLI; hand-rolled JSON, no serde/clap). Deps: `libc` only. |
| `crates/ramsleuth-telemetry` | **DONE (Phase 2) + AMD model reconciled (P5-15)** | `cpuid` (vendor + frozen generation map), `error` (`TelemetryError` / `Section<T>` no-panic contract), `amd_smu` (sysfs candidate list — `ryzen_smu_drv` first; sibling `pm_table_version`/`pm_table_size`; `/dev/ryzen_smu` ioctl fallback), `amd_pm` (live-verified f32 PM layout + `TableVersionId` sets, §4), `amd_readout` (shared display types + AMD mapping), `intel_mchbar` (PCI BAR5 + read-only `/dev/mem` mmap guard), `intel_readout` (per-channel IMC decode — offsets still skeleton), `spd_eeprom` (ee1004 unprivileged acquire), `spd_decode` (JEP106 / rank / XMP / EXPO), `facade` (`SystemMemoryTelemetry` + `collect()`), `main` (verification CLI). Deps: `nix` 0.29 (`fs`/`ioctl`/`mman`) only. |
| `crates/ramsleuth-protocol` | **DONE (Phase 3)** | Wire contract: `Request` / `Response` / `BenchMode` / `Message` serde enums (payload types reused verbatim from telemetry + bench), `DEFAULT_SOCKET_PATH` (`/run/ramsleuth/ramsleuth.sock`), length-prefixed Bincode frame codec with a 16 MiB `MAX_FRAME_SIZE` guard (`encode_frame` / `decode_frame` / `Frame` / `FrameError`). Zero `unsafe`, tokio-free. |
| `crates/ramsleuth-daemon` | **DONE (Phase 3)** | `caps` SOFT privilege probe (geteuid + `CapEff` bit 21; never panics/exits), `socket` listener (mode **0660**, stale-file probe → live `AlreadyRunning` / dead rebind, best-effort `chown`), TTL `TelemetryCache` (injectable collector), single-flight `BenchJobManager` (clean cancel + per-cell progress), `rpc` (per-connection async over the frozen wire contract), `main` bin (`--socket` / `--max-age`, SIGTERM/SIGINT → graceful stop + socket removal), `systemd/ramsleuth.service` (CAP_SYS_RAWIO-clamped, sandboxed unit). Deps: tokio (daemon-only). |
| `crates/ramsleuth-client` | **DONE (Phase 3)** | Synchronous `UnixStream` transport (read/write timeouts, 3-retry backoff, friendly `DaemonDown` diagnostics); pure `dump` dashboard renderer (full hardware timings, every N/A cell with its reason — **the Phase 3 exit criterion**); `bench` / `status` commands; CLI (`dump` / `bench` / `status` + `--socket` / `--tier` / `--mode`, exit codes 0/1/2). std-only, no tokio — the transport shared by the CLI and by `tui`/`gui`. |
| `crates/ramsleuth-tui` | **DONE (Phase 4)** | ratatui 0.29 + crossterm 0.28 (MSRV ≤ 1.75 via two lockfile pins); pure `key_to_action` (R / S / Q); 3-zone non-scrolling layout (timing matrix / bench grid + progress / SPD + daemon status); terminal loop with a background 2 s updater. |
| `crates/ramsleuth-gui` | **DONE (Phase 4)** | egui / eframe / egui_extras 0.27.2 (newest 1.75-compatible line); semantic palette (cyan / amber / slate / crimson); F2 PNG snapshot / F3 JSON export to `$HOME`; `TelemetryData` behind `Arc<RwLock>` + background poller (bench command channel + cancel); 3 zones; 1400×900 @ ~60 FPS eframe app with no UI-thread blocking. |

Root `Cargo.toml`: **7 members**, `version 0.1.0`, `edition 2021`, `rust-version = "1.75"`, license MIT, repo `https://github.com/MadGoatHaz/RamSleuth`. Packaging: `packaging/` (AUR `ramsleuth-git` + optional `ryzen-smu-dkms` extra + README), `scripts/install-ryzen-smu-dkms.sh`, `.github/workflows/ci.yml`, `systemd/ramsleuth.service` (frozen).

## 11. How to Run (quick reference, release)

The QA-verified flow: one privileged daemon; all frontends unprivileged over the socket.

```bash
# Build everything
cargo build --workspace --release

# 1) Daemon — default socket /run/ramsleuth/ramsleuth.sock (root, CAP_SYS_RAWIO via the unit)
sudo target/release/ramsleuth-daemon
#    or unprivileged local dev (0660 socket, SOFT caps probe warns and keeps serving):
target/release/ramsleuth-daemon --socket /tmp/ramsleuth.sock

# 2) CLI client — no sudo needed
target/release/ramsleuth-client -- dump                       # full hardware timings (exit-criterion command; AMD section populated on this host)
target/release/ramsleuth-client -- status                     # daemon + hardware status
target/release/ramsleuth-client -- bench --tier l3 --mode memory-only
#    with a dev socket:
target/release/ramsleuth-client --socket /tmp/ramsleuth.sock -- dump
#    no daemon running → exit 1 + friendly DaemonDown start hint (never a crash)

# 3) TUI (R = refresh, S = snapshot, Q = quit) / 4) GUI (F2 = PNG, F3 = JSON, Q = quit)
target/release/ramsleuth-tui
target/release/ramsleuth-gui

# Ground truth (open item 1 comparison target)
sudo monitor_cpu

# Direct verification CLIs (no daemon; from the Cycle 1/2 lineage)
cargo run -p ramsleuth-bench --release            # [--avx512] [--json]
sudo cargo run -p ramsleuth-telemetry --release   # [--json]; root + ryzen_smu (installed) for live AMD

# systemd (unit shipped at systemd/ramsleuth.service; packaged by the AUR ramsleuth-git)
sudo cp systemd/ramsleuth.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now ramsleuth

# Packaging (P5-02…P5-07): makepkg -si from packaging/ramsleuth-git/; optional AMD extra via scripts/install-ryzen-smu-dkms.sh (already installed on this host — §6)
```

`--tier` accepts `memory | l1 | l2 | l3 | full` (default `full`); `--mode` accepts `full | memory-only` (default `full`).

## 12. Operating Protocol Summary (how work runs in this repo)

1. **Strict phased gating:** each cycle compiles + runs + passes its exit criteria before the next starts. Each phase is planned first (`plans/PLAN-PHASE<N>.md`) with micro-chunks, dependency tags (`[ISOLATED]` / `[COUPLED-TO: …]` / `[CRITICAL-PATH]`), frozen-interface markers, and per-chunk quality gates.
2. **Single-file micro-chunking:** one target source file per chunk, ≤ ~50–100 lines changed; each chunk adds exactly one `mod <name>;` wiring line to `src/lib.rs` (wiring not counted against the budget).
3. **Per-chunk flow:** implement on `branch/chunk-<ID>` (forked from `v2-development`) → in-file unit tests (`#[cfg(test)]`) → clippy `-D warnings` → **per-chunk code review** (lease-signed) → `git merge --no-ff` into `v2-development` → branch pruned. Interface-freeze chunks (`[CRITICAL-PATH]`) merge first; no silent signature changes — changes are a plan edit + rebase.
4. **Lease board:** `DEV_LOG.md` `@@@ ACTIVE_WORKERS @@@` — sign in with target files before work; sign out with `[DONE]`/`FAILED` + one-line decision + ahead-note; update `@@@ CURRENT_STATE @@@`. (Board is empty right now.)
5. **Handoffs:** 3-line **Semantic Pulse** — `STATUS` / `DECISION` / `AHEAD`, one short line each. No full-file dumps or code in chat; all detail goes to disk (plans, logs, docs).
6. **QA audit per cycle:** independent pass of every runnable gate (build, tests debug+release, clippy, live CLI runs in multiple privilege/CPU states) → then **compaction**: merge history into `MASTER_LOG.md`, prune chunk branches, reset `DEV_LOG.md`.
7. **Safety/fallbacks (hard rule):** unsupported CPU, missing driver, missing privilege, unknown PM-table version, absent `/dev/mem` → structured `Na(<reason>)`; **never panic, never segfault, never `unwrap`/`expect` on hardware data**; the Intel path never touches `/dev/mem` on non-Intel hardware; non-x86_64 compiles to a documented degraded path.
8. **MSRV:** keep the workspace at **1.75** and the lockfile pins intact unless the decision in §5.5 is made; the CI `1.75` leg is the continuous MSRV proof.
9. **Docs discipline:** `Docs/` holds the roadmap + spec + this handover + the full scope reference (`FULLSCOPEvsCOMPLETED.md`); `plans/` holds per-phase plans; `MASTER_LOG.md` is the durable record. Update them every cycle; this document is the standing handover — refresh it at each cycle close-out.

## 13. First Actions for the New Director (Cycle 5)

1. **Confirm the baseline** (already re-verified by this handover pass on 2026-09-13): `v2-development` checked out (tip = `2409947` + this docs commit) with a clean tree; `cargo test --workspace` (debug + release) → **337/337**; `cargo clippy --workspace --all-targets -- -D warnings` → zero; `cargo build --workspace --release` → OK.
2. **Confirm the live state:** `ls /sys/kernel/ryzen_smu_drv/` (expect `pm_table` + `pm_table_version` + `pm_table_size` + `smn`); `sudo monitor_cpu` (ground truth runs); root daemon + `ramsleuth-client -- dump` (AMD section populated — clocks ≈ 1792 MHz OneToOne, VDDCR_SOC ≈ 1.128 V; CAD/timings honest `Na`).
3. **Decide the push go-ahead** (operator gate — §8): on go-ahead, fast-forward `origin/v2-development` to the local tip (≥ `2409947`), prune the 17 fully-merged remote `branch/chunk-p5-*` branches, optional `v2.0.0` tag. Until then: strictly local — no push, no remote-branch deletion.
4. **Start Cycle 5** — plan first (`plans/PLAN-CYCLE5.md` or similar, single-file micro-chunks, gated pipeline), then pick the entry point:
   - **AMD ground-truth cross-check (§5.1)** — the natural first task (unblocked): matched-condition `monitor_cpu` vs `ramsleuth-client -- dump`; investigate the 1792-vs-1800 MHz delta.
   - **Intel MCHBAR decode (§5.2)** — needs the i5-6600 machine on hand.
   - **Model reconciliation (§5.3)** — the `smn`-attr SMN path for CAD/timings (the biggest remaining model gap), SPD maker `0xC1`/density `0x0D`, Intel IMC offsets.
   - **L1/L2 overhead refinement (§5.4)** — small bench code change (inner-loop iterations).
   - **MSRV decision (§5.5)** — operator call; if first, it is a packaging/CI-affecting decision, not a code chunk.
5. **Cycle 5 exit** → standard compaction: `MASTER_LOG.md`, prune merged chunk branches, reset `DEV_LOG.md`; refresh this handover for the next cycle.
