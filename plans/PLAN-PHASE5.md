# RamSleuth v2 — Phase 5 Plan: Packaging & Distribution

> **Base branch:** `v2-development` — every `branch/chunk-P5-xx` forks from and merges back here (NOT `main`).
> **Sources of truth:** `Docs/RamSleuth-v2.md` Phase 5 (5.1/5.2); `Docs/HANDOVER.md` §7 (ryzen_smu DKMS workflow), §8 (push policy), §9 (open items), §10 (Phase 5 scope); `systemd/ramsleuth.service` (**frozen — do not modify**); `plans/PLAN-PHASE3.md` decisions (D1 MSRV, D5 no-panic, D7 dependency policy).
> **Mandate:** 100% pure Rust, Cargo workspace, Edition 2021, `rust-version = 1.75` (KEPT this cycle — see §6). Phase 5 is packaging/distribution only: **zero source-code changes in any crate** — every chunk is a NEW file under `packaging/`, `scripts/`, or `.github/`; the 327/327 (debug+release) + clippy `-D warnings`-clean baseline must remain untouched.
> **Chunk discipline:** one target file per chunk, ~50–100 lines each (the PKGBUILD runs to ~100). No chunk may edit `systemd/ramsleuth.service`, anything under `crates/**`, `Cargo.toml`, `Cargo.lock`, or `Docs/**`.
> **IDs:** Chunk 1..7 = P5-01..P5-07; the order below is the recommended execution order.

---

## 1. Confirmed facts (from recon — use, do not re-derive)

- 7 crates; **6 installable binaries** → `/usr/bin/`: `ramsleuth-daemon` (the unit's ExecStart), `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui`, `ramsleuth-bench`, `ramsleuth-telemetry`. `ramsleuth-protocol` is lib-only (not installed).
- Workspace `Cargo.toml`: version 0.1.0, edition 2021, `rust-version = 1.75`, resolver 2, license MIT, repository https://github.com/MadGoatHaz/RamSleuth.
- `systemd/ramsleuth.service` exists and is frozen: `Group=ramsleuth`, `RuntimeDirectory=ramsleuth`, `ReadWritePaths=/run/ramsleuth`, `CapabilityBoundingSet`/`AmbientCapabilities=CAP_SYS_RAWIO`, `NoNewPrivileges=true`, `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`, `Restart=on-failure`, `WantedBy=multi-user.target`, `ExecStart=/usr/bin/ramsleuth-daemon --socket /run/ramsleuth/ramsleuth.sock`.
- **THE ONE REAL INSTALL GAP:** the unit uses `Group=ramsleuth` — the installer MUST create the `ramsleuth` group or the service fails to start (the unit documents `Group=wheel` as the fallback on systems without it).
- ryzen_smu DKMS workflow (HANDOVER §7): full step list reproduced in the Chunk 4 scope. `AUTOINSTALL=yes` = auto-rebuild on kernel updates; the systemd unit does NOT load the module (assumes loaded); without it the app degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic).
- MSRV: **KEEP 1.75** this cycle (§6). The AVX-512F intrinsics in `ramsleuth-bench` (`kernel_512.rs`) are cfg-gated (`#[cfg(target_arch = "x86_64")]` + runtime `CpuFeatures::detect().avx512f` dispatch + `#[clippy::msrv = "1.89"]` annotations) — the 1.75 build passes (green baseline); the 512-bit path is never exercised on this Zen 3 (5950X) host.
- GUI system-lib surface (egui/eframe/winit 0.27 stack, verified against `crates/ramsleuth-gui/Cargo.toml` — egui/eframe/egui_extras 0.27, png, serde_json): `pkgconf` at build time (`libxkbcommon` via `xkbcommon-sys` is the only strict build-time link dep) + the X11/Wayland/GL dlopen/fallback surfaces (exact lists in Chunks 2/6).

## 2. Phase 5 scope (v2 §5.1)

1. **PKGBUILD / AUR package (`ramsleuth-git`)** — build all 7 crates, ship `systemd/ramsleuth.service`, install the 6 binaries + unit + preset + `ramsleuth` group. → Chunks 1–2.
2. **systemd preset + `ramsleuth` group creation at install** — the one real gap. → Chunks 1–2.
3. **Optional `ryzen_smu` DKMS module provision** (live AMD subtimings) as a recommended extra, not a hard dependency. → Chunks 3–5.
4. **GitHub Actions CI** — test + clippy + build matrix for `x86_64-unknown-linux-gnu`. → Chunk 6.
5. **Push on go-ahead** (v2 §5.1e) — a post-cycle operator event, not a chunk (§7).
6. **"Kernel Driver Hooks" (v2 §5.1a)** — *no new source code*: the daemon's existing SOFT caps probe + `Na(DriverMissing)` already notify (stderr warning + structured N/A sections); the installer script's verification + graceful note (Chunk 4) and the README (Chunk 7) complete the story.

## 3. Key decisions

**D1 — packaging tree layout (all NEW files, zero source diffs):**
```
packaging/
├── README.md                  Chunk 7
├── ramsleuth-git/
│   ├── PKGBUILD               Chunk 2
│   └── ramsleuth.preset       Chunk 1
└── ryzen-smu-dkms/
    ├── dkms.conf              Chunk 3
    └── PKGBUILD               Chunk 5 (optional)
scripts/
└── install-ryzen-smu-dkms.sh  Chunk 4
.github/workflows/
└── ci.yml                     Chunk 6
```

**D2 — `makepkg -si` from `packaging/ramsleuth-git/` is the single install path** (an AUR submission is a downstream convenience, not part of this cycle). The v2 5.2 `cargo install` alternative is NOT implemented: 7 path-dependent crates + a systemd unit + a system group cannot be covered by `cargo install`; the README (Chunk 7) documents the makepkg path.

**D3 — the `ramsleuth` group is created in `package()`** via `getent group ramsleuth >/dev/null || groupadd -r ramsleuth` (system-ID range, idempotent) — closing the one real install gap. The unit's documented `Group=wheel` fallback remains available to operators on systems without the group.

**D4 — install locations:** unit → `/usr/lib/systemd/system/ramsleuth.service` (distro default, NOT `/etc`); preset → `/usr/lib/systemd/system-preset/ramsleuth.preset` with content `00 enable ramsleuth.service` (the priority token is required by the preset(5) line format; `00` = first, matching Arch's `00 enable *` default).

**D5 — the DKMS module is NOT part of `ramsleuth-git`:** a separate optional artifact (`packaging/ryzen-smu-dkms/`) + operator-run helper (`scripts/install-ryzen-smu-dkms.sh`). The unit never loads the module (frozen); a bare install must work end-to-end without it (degraded per the no-panic contract).

**D6 — CI matrix:** rust `1.75` (MSRV) + `stable` × `ubuntu-latest`, `fail-fast: false`; jobs use the committed `Cargo.lock` (never regenerated in CI); GUI system libs installed for the eframe/winit build + dlopen surface (Chunk 6). The MSRV leg continuously proves the 1.75 + lockfile-pins + cfg-gated-AVX-512 claim on a clean runner.

**D7 — no-panic contract through packaging:** no packaging artifact may hard-fail installation because the ryzen_smu module, AVX-512, or a display is absent; the only mandatory system mutation is the idempotent group creation; after a bare install the service must start and serve `Na(DriverMissing)` sections (exit 0, no panic).

## 4. Chunks (7 single-file micro-chunks; no source changes)

### Chunk 1 — P5-01 · `packaging/ramsleuth-git/ramsleuth.preset` · [ISOLATED]
**Scope (~5 lines):** a systemd preset file: a 2-line comment header (what it enables + install location) + one directive line `00 enable ramsleuth.service`. Installed by P5-02 to `/usr/lib/systemd/system-preset/ramsleuth.preset`.
**Quality gate:** no compilation; reviewer verifies the line format against `preset(5)` (priority + action + pattern); the file is inert until installed.
**No-panic:** the preset is convenience, not requirement — absence of the preset still allows manual `systemctl enable ramsleuth`; a bare install never breaks because of it.

### Chunk 2 — P5-02 · `packaging/ramsleuth-git/PKGBUILD` · [COUPLED-TO: Chunk 1]
**Scope (~100 lines):** the main AUR package.
- Header: `pkgname=ramsleuth-git`, `pkgrel=1`, `arch=(x86_64)`, `license=(MIT)`, `url`, `pkgdesc` (memory telemetry + benchmark daemon and unprivileged clients).
- `pkgver()`: from `git describe --tags --long --dirty` with `sed` normalization, **with a fallback to a short SHA** (`git rev-parse --short HEAD`) when no tag exists — the tag decision is open (§7) and the package must build either way.
- `source=("$pkgname::git+https://github.com/MadGoatHaz/RamSleuth.git#branch=v2-development")`.
- `makedepends=(rust cargo pkgconf libx11 libxkbcommon wayland wayland-protocols libxrandr libxi libxcursor libxinerama mesa)` — the eframe/winit build surface: `libxkbcommon` is the only strict build-time link dep (`xkbcommon-sys`); the rest cover pkgconf probes + dlopen/fallback surfaces. The C compiler toolchain comes from the makepkg base environment (AUR convention — the compiler is not listed in `makedepends`).
- `depends=(libx11 libxkbcommon wayland libxrandr libxi libxcursor libxinerama mesa)` — the runtime dlopen surface (Wayland + X11 fallback + GL; on Arch `mesa` covers the GL loader + DRI).
- `build()`: `cd "$srcdir/ramsleuth-git" && cargo build --release --locked` (builds all 7 crates; `--locked` = the committed pins, reproducible).
- `package()`: (1) install the 6 binaries → `/usr/bin/` (`ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth-gui`, `ramsleuth-bench`, `ramsleuth-telemetry`); (2) `install -Dm644 systemd/ramsleuth.service /usr/lib/systemd/system/ramsleuth.service` (verbatim copy — never modified, D4); (3) `install -Dm644 ramsleuth.preset /usr/lib/systemd/system-preset/ramsleuth.preset` (Chunk 1); (4) `getent group ramsleuth >/dev/null || groupadd -r ramsleuth` (D3, idempotent).
**Quality gate:** `bash -n PKGBUILD` + `makepkg --printsrcinfo` (syntax/srcinfo sanity ONLY — do NOT run a full `makepkg`); reviewer cross-checks every installed path against the unit's `ExecStart`/socket expectations and the 6 binary names as emitted by `target/release/`.
**No-panic:** installation never touches `/run`, the kernel module, or a display; group creation is idempotent; a bare install yields a service that serves `Na(DriverMissing)` (exit 0).

### Chunk 3 — P5-03 · `packaging/ryzen-smu-dkms/dkms.conf` · [ISOLATED]
**Scope (~12 lines):** the DKMS config for `ryzen_smu` (used only if the upstream source tree lacks one — HANDOVER §7 step 3):
```
PACKAGE_NAME=ryzen_smu
# PACKAGE_VERSION is taken from the upstream source tree
BUILT_MODULE_NAME=ryzen_smu
DEST_MODULE_LOCATION=/extra
AUTOINSTALL=yes
MAKE="make -C ${kernel_source_dir} M=${dkms_tree}/ryzen_smu/${PACKAGE_VERSION} modules"
```
`AUTOINSTALL=yes` = auto-rebuild on every kernel update (pacman-managed — no manual rebuild step).
**Quality gate:** no compilation; reviewer checks the keys against `dkms(8)`; the `MAKE` string matches HANDOVER §7 verbatim.
**No-panic:** n/a — the file is inert until DKMS consumes it; its absence never breaks the app (the module is optional).

### Chunk 4 — P5-04 · `scripts/install-ryzen-smu-dkms.sh` · [COUPLED-TO: Chunk 3]
**Scope (~100 lines, bash, `set -euo pipefail`, idempotent, operator-run; requires root — re-execs under `sudo` when not root; on non-pacman hosts it prints the equivalents instead of silently failing):**
1. Verify the kernel build tree: `ls /lib/modules/$(uname -r)/build` → if absent, print the headers lookup (`pacman -Qs headers | grep -iE 'cachyos|custom'` — this host: `7.2.3-1-cachyos-custom`) and stop with instructions (the script auto-installs only `dkms`/`base-devel` via `pacman -S --needed`, never custom-kernel headers).
2. Clone the `ryzen_smu` source (default `RYZEN_SMU_REPO=https://github.com/53XU/ryzen_smu`, env-overridable) into a dedicated directory; **the script prints the upstream URL and pauses requiring the operator to verify the upstream at setup** (HANDOVER §7: do not trust a cached URL).
3. If the source tree lacks a `dkms.conf`, copy `packaging/ryzen-smu-dkms/dkms.conf` (Chunk 3; path resolved relative to the repo root).
4. `sudo dkms install ryzen_smu -k $(uname -r)` (one command = add + build + install).
5. `sudo modprobe ryzen_smu` + `echo ryzen_smu | sudo tee /etc/modules-load.d/ryzen_smu.conf` (idempotent via grep).
6. Verify: `/sys/kernel/ryzen_smu/pm_table` exists → print success.
7. **Graceful note (always printed at the end, and on failure):** without the module the app degrades to `N/A (DriverMissing)`, exit 0, no panic; the systemd unit never loads the module; on custom-kernel hosts the module build may need Kconfig/version-guard adjustments (HANDOVER §6a caution). On any failure: print the note + exit 1 (operator action required).
**Quality gate:** `bash -n` + `shellcheck` if available (if absent, log it and fall back to `bash -n` alone); reviewer traces each step against HANDOVER §7 (the step list above is the 1:1 mapping).
**No-panic:** the script never touches ramsleuth binaries/unit/service state; a failed module install leaves the app fully operational in degraded mode.

### Chunk 5 — P5-05 · `packaging/ryzen-smu-dkms/PKGBUILD` · [COUPLED-TO: Chunk 3] · (optional convenience)
**Scope (~80 lines):** an optional AUR wrapper for the DKMS module: `pkgname=ryzen-smu-dkms`, `depends=(dkms)`, `makedepends=(git base-devel)`; `source` = the ryzen_smu git repo (`#branch=main` — upstream verified at review time, never trusted from cache); `build()` = `dkms add` → `dkms build -k $(uname -r)` → `dkms install -k $(uname -r)`; `package()` = install the source tree + `dkms.conf` under `/usr/src/ryzen_smu/${pkgver}/` so `AUTOINSTALL` rebuilds on future kernel updates. Custom-kernel (CachyOS) caution noted in the header comment: matching headers required; **the operator script (Chunk 4) is the primary path** — this package targets standard-kernel systems.
**Quality gate:** `bash -n` + `makepkg --printsrcinfo` syntax sanity (do NOT run a full build — a DKMS build needs a kernel; CI never builds this package).
**No-panic:** optional; it never touches ramsleuth artifacts.

### Chunk 6 — P5-06 · `.github/workflows/ci.yml` · [ISOLATED]
**Scope (~60 lines):**
```yaml
name: ci
on:
  push: { branches: [v2-development] }
  pull_request: { branches: [v2-development] }
jobs:
  test:
    runs-on: ubuntu-latest
    strategy: { fail-fast: false, matrix: { rust: ["1.75", "stable"] } }
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@v1   # inputs: toolchain: ${{ matrix.rust }}, components: clippy, cache: true
      - name: GUI system libs
        run: sudo apt-get update && sudo apt-get install -y pkgconf libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev libgl1-mesa-dev
      - run: cargo test --workspace
      - run: cargo test --workspace --release
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo build --release --workspace
```
The committed `Cargo.lock` is used as-is (no `cargo update`, no lockfile regeneration) — the MSRV leg proves 1.75 + pins + cfg-gated AVX-512 on a clean runner.
**Quality gate:** YAML validity (e.g. `python3 -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/ci.yml` or equivalent; `actionlint` if available — not required).
**No-panic:** additive; no source changes; a CI failure never affects the installed app.

### Chunk 7 — P5-07 · `packaging/README.md` · [COUPLED-TO: all]
**Scope (~90 lines):** the packaging documentation:
- What is installed (6 binaries, the unit, the preset, the `ramsleuth` group) and where.
- Install: `makepkg -si` from `packaging/ramsleuth-git/` (Arch/AUR); the group-gap note + the `Group=wheel` fallback; enablement (`systemctl preset` / `enable --now`), `systemctl status` / `journalctl -u ramsleuth` hints.
- Usage: unprivileged `ramsleuth-client dump` (the exit-criterion command), the TUI, the GUI (needs a display), `ramsleuth-telemetry` / `ramsleuth-bench` direct verification CLIs.
- The optional `ryzen_smu` DKMS extra: the operator script (Chunk 4, primary path), the optional AUR wrapper (Chunk 5), `AUTOINSTALL` auto-rebuild, the custom-kernel caution.
- Graceful degradation: without the module → `N/A (DriverMissing)`, exit 0, no panic; the unit never loads the module.
- CI: the matrix + what each leg proves (MSRV 1.75, stable).
**Quality gate:** no compilation; reviewer verifies every path/flag/command against the sibling files (no drift between README, PKGBUILD, and script).
**No-panic:** the documentation must not promise more than the no-panic contract.

## 5. Phase 5 exit criteria

1. **Single-command install** — `makepkg -si` in `packaging/ramsleuth-git/` on an Arch-based system yields: 6 binaries in `/usr/bin`, the unit in `/usr/lib/systemd/system/`, the preset installed, and the `ramsleuth` group present (`getent group ramsleuth` succeeds before first start).
2. **Working daemon + UI binaries** — the service starts (`active (running)`); unprivileged `ramsleuth-client dump` prints full hardware timings (bare install: AMD section `N/A (DriverMissing)`; dev host with `ryzen_smu` loaded: values populated) — exit 0, no panic.
3. **CI green** — the workflow passes on push/PR to `v2-development` on both matrix legs (1.75 + stable): test debug, test release, clippy `-D warnings`, build release.
4. **No-panic contract preserved** — zero source diffs (`git diff` of `crates/`, `Cargo.toml`, `Cargo.lock`, `systemd/` is empty after all merges); the host baseline 327/327 (debug+release) remains green; bare install without `ryzen_smu` degrades gracefully.
5. **Optional extras verified** — the DKMS script runs the full HANDOVER §7 flow to the `pm_table` verification on the dev host, or fails gracefully with the note (no partial state left).

## 6. MSRV decision (open item 5)

**The fork:**
- **Option A — bump workspace `rust-version` 1.75 → 1.89** to match the AVX-512F intrinsics in `ramsleuth-bench` (`kernel_512.rs`; intrinsics stabilized in 1.89; clippy already annotated `#[clippy::msrv = "1.89"]` on those items).
- **Option B — keep 1.75 + cfg-gate** (the current state): the 512-bit bodies compile only on x86_64 and dispatch at runtime; the 327/327 + clippy-clean baseline is green; lockfile pins keep all deps ≤ 1.75.

**Recommendation: KEEP 1.75 for this cycle.** The lockfile pins protect it; the AVX-512 path is never exercised on this Zen 3 (5950X) host (the runtime gate selects AVX2); the CI MSRV leg (Chunk 6) makes the claim continuous. **This is an operator decision — do NOT implement a bump in this cycle.** If Option A is chosen later, it is a standalone micro-chunk (workspace `Cargo.toml` edit + lockfile-pin re-verification + clippy re-baseline) scheduled after Phase 5.

## 7. Push Policy

- `v2-development` → origin: **fast-forward only** (`git push origin v2-development`); **NEVER force-push**; **on explicit operator go-ahead only** (HANDOVER §8: nothing has been pushed; upstream `master` is divergent legacy).
- Decide at push time: (a) tag first (e.g. `v2.0.0`) so the AUR `git describe` pkgver gains a stable value, or (b) open a fresh branch (e.g. `v2`) instead of pushing `v2-development` into a repo whose `master` is divergent legacy. The PKGBUILD pkgver has a no-tag fallback (Chunk 2), so the package works either way.
- The push is not part of any chunk; it is a post-cycle operator event.

## 8. Dependency ledger (Phase 5)

Zero new Rust crates (no `Cargo.toml` / `Cargo.lock` changes). Additions: the packaging files, the install script, and the CI workflow only. The GUI system-lib surface is fixed by the existing eframe 0.27 / winit stack (Chunks 2/6).

---

**Execution order (gated pipeline):** 1 → 2 (coupled) → 3 → 4 (coupled) → 5 → 6 → 7. Each chunk: `branch/chunk-P5-NN` off `v2-development`, `--no-ff` merge back, lease sign-in/out in `DEV_LOG.md`, the chunk's quality gate before merge. No implementation begins until the operator green-lights the pipeline.
