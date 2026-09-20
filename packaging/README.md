# RamSleuth v2 — Packaging

Operator/end-user guide for installing and running RamSleuth v2: the three-tier AUR packages (`ramsleuth` stable source, `ramsleuth-bin` precompiled, `ramsleuth-git` bleeding-edge), the `ramsleuth` group, the sandboxed systemd unit, the optional `ryzen_smu` DKMS extra, the unified version bump, and CI.

## Overview

RamSleuth v2 is 100% pure Rust — a Cargo workspace of 7 crates, built with `cargo build --release --locked`, producing one privileged daemon and five unprivileged clients:

- `ramsleuth-daemon` — runs as root but holds only `CAP_SYS_RAWIO` (the capability required for the SMU and `/dev/mem` MCHBAR reads) and listens on a Unix socket.
- `ramsleuth-client` (CLI), `ramsleuth-tui`, `ramsleuth` (the GUI — needs a display, the `ramsleuth-gui` crate), `ramsleuth-bench`, `ramsleuth-telemetry`.

The socket lands at `/run/ramsleuth/ramsleuth.sock` (mode `0660`, group-owned; systemd creates the directory via `RuntimeDirectory=ramsleuth`). Clients need no privileges — only membership in the `ramsleuth` group.

## What the install does

The AUR packages (or `makepkg -si` from any of `packaging/ramsleuth/`, `packaging/ramsleuth-bin/`, `packaging/ramsleuth-git/`) install:

| Artifact | Destination |
| --- | --- |
| 6 binaries: `ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth`, `ramsleuth-bench`, `ramsleuth-telemetry` | `/usr/bin/` |
| The frozen daemon unit (`systemd/ramsleuth.service`) | `/usr/lib/systemd/system/ramsleuth.service` |
| The systemd preset (`00 enable ramsleuth.service`) | `/usr/lib/systemd/system-preset/ramsleuth.preset` |
| The `ramsleuth` system group | created on the target by the `.install` `pre_install`/`pre_upgrade` hooks (idempotent `groupadd -r`) |
| The ramsleuth-owned copy of the pinned `ryzen_smu` DKMS helper (`scripts/install-ryzen-smu-dkms.sh`) | `/usr/bin/ramsleuth-install-ryzen-smu-dkms` |
| The self-contained installer (`install.sh` — the transparency artifact, re-runnable/auditable post-install) | `/usr/share/ramsleuth/install.sh` |

`ramsleuth-protocol` is a library-only crate and is never installed.

## Installation

### AUR — the three-tier model

```sh
yay -S ramsleuth      # STABLE source — builds from the official git tag v$pkgver
yay -S ramsleuth-bin  # PRECOMPILED — the binary from the GitHub Release (fastest install, no build)
yay -S ramsleuth-git  # BLEEDING-EDGE — builds from the moving v2-development branch (dev/testing)
```

(or `paru -S ...` in place of `yay`.)

- **`ramsleuth`** — the stable source package: builds the workspace from the official `v$pkgver` git tag (a reproducible, auditable snapshot). The default recommendation for production installs.
- **`ramsleuth-bin`** — the precompiled package: downloads the release binary tarball (`ramsleuth-$pkgver-x86_64.tar.zst`) from the official GitHub Release and installs it as-is — no build, no makedepends. The fastest install path.
- **`ramsleuth-git`** — the bleeding-edge dev package: tracks the moving `v2-development` branch (its `pkgver()` recomputes from `git describe`). For testing unreleased work; not a stable install.

**Mutual conflict:** `ramsleuth` and `ramsleuth-bin` install the identical file set, so each declares the other in `conflicts=` — the user picks exactly one. (The optional `ryzen-smu-dkms` extra stays co-install-safe with all three — see below.)

### Manual (from a source checkout)

```sh
cd packaging/ramsleuth   # or: packaging/ramsleuth-bin / packaging/ramsleuth-git
makepkg -si
```

Build deps (the source packages `ramsleuth`/`ramsleuth-git`; `ramsleuth-bin` compiles nothing): `rust`, `cargo`, `pkgconf`, plus the X11/Wayland/GL library set in the PKGBUILD (`libxkbcommon` is the only strict build-time link dep). The `post_install` hook runs `systemctl enable --now ramsleuth.service` — the daemon is started and enabled automatically, and the preset keeps it enabled on future `systemctl preset` runs. It also prints the next steps: start the GUI with `ramsleuth` (the TUI with `ramsleuth-tui`), and on AMD hosts the optional `sudo ramsleuth-install-ryzen-smu-dkms` (pinned upstream, shown + confirmed before any build; the AUR-extra alternative is `yay -S ryzen-smu-dkms`), while Intel hosts get the note that the built-in MCHBAR decode needs no extra driver.

## Version bump (unified versioning)

The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml` — all 7 member crates inherit it, and the GUI window titles derive it at compile time via `env!("CARGO_PKG_VERSION")`, so a bump updates the titles automatically (no manual edit). The full bump flow:

1. Bump `[workspace.package].version` in the root `Cargo.toml` (e.g. `2.1.0` → `2.2.0`).
2. `cargo update -w` to sync `Cargo.lock` (the 7 member lines).
3. Commit + push.
4. Cut the git tag `v<ver>` — the release workflow (`.github/workflows/release.yml`) fires on the tag and auto-builds + publishes the GitHub Release with the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` and its `sha256` companion.
5. Update the AUR packages: `pkgver` in `packaging/ramsleuth/PKGBUILD` (the git-tag source) and `pkgver` in `packaging/ramsleuth-bin/PKGBUILD` (the tarball download), then finalize the `ramsleuth-bin` `sha256sums` from the **published** release `.sha256` (AUR requires a real `sha256` — no SKIP; the value is a placeholder until the release exists).
6. Push both AUR packages.

`ramsleuth-git` needs no version bump — its `pkgver()` recomputes from `git describe` on the moving branch.

## The `ramsleuth` group

The socket is `0660` group-owned; clients must be in the group to read telemetry. The group is created automatically on install — without it the unit fails to start (the one real install gap). Grant access to a normal user:

```sh
sudo usermod -aG ramsleuth <user>
```

(re-login required). On systems that cannot provide the group, the unit documents a `Group=wheel` fallback (edit the installed unit, then `systemctl daemon-reload`).

## systemd

The installed unit is sandboxed: `CapabilityBoundingSet`/`AmbientCapabilities=CAP_SYS_RAWIO` (that single capability only), `NoNewPrivileges=true`, `ProtectSystem=strict` (read-only filesystem except `/run/ramsleuth`), `ProtectHome=true`, `PrivateTmp=true`, `Restart=on-failure`, `WantedBy=multi-user.target`:

```
ExecStart=/usr/bin/ramsleuth-daemon --socket /run/ramsleuth/ramsleuth.sock
```

Day-2 commands:

```sh
systemctl start ramsleuth
systemctl status ramsleuth
journalctl -u ramsleuth
```

The unit never loads the `ryzen_smu` module.

## Optional: `ryzen_smu` DKMS extra (live AMD subtimings)

**Not a hard dependency.** Without the module, RamSleuth degrades gracefully: the AMD subtiming fields read `N/A (DriverMissing)`, exit 0, no panic.

To enable live AMD subtimings on an AMD host:

1. Install the provisioning package `ryzen-smu-dkms` (bundles the DKMS config at `/usr/share/ryzen-smu-dkms/dkms.conf`, the helper, and docs):

   ```sh
   yay -S ryzen-smu-dkms   # or: paru -S ryzen-smu-dkms
   ```

2. Build and install the module for the running kernel:

   ```sh
   sudo ryzen-smu-dkms-install
   ```

   Or, from a source checkout (no package needed): `scripts/install-ryzen-smu-dkms.sh`. The helper is idempotent, re-execs under `sudo`, requires the matching kernel headers (build tree `/lib/modules/$(uname -r)/build` — for custom-kernel hosts it lists candidate packages and stops, never guessing), builds the **pinned** upstream `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD; fetched, shown, checksummed, and confirmed before any build; the pin overridable via `RYZEN_SMU_PIN`, the URL via `RYZEN_SMU_URL`) to `/opt/ryzen-smu-src`, then runs `dkms install`, `modprobe`, and persists `/etc/modules-load.d/ryzen_smu.conf`.

`AUTOINSTALL=yes` in the `dkms.conf` auto-rebuilds the module on kernel updates. Verify:

```sh
ls /sys/kernel/ryzen_smu_drv/pm_table
```

The module also ships a `monitor_cpu` CLI for ground-truth comparison: run it side-by-side with RamSleuth and expect clocks within ±1 MHz, voltages within ±10 mV, and matching CAD/subtimings.

### AUR parity & transparency

An AUR install gets the same scripting as the GitHub path (`./install.sh` from a source checkout) — plus a third, driver-less path:

- `sudo ramsleuth-install-ryzen-smu-dkms` — the helper all three AUR packages ship at that path (the ramsleuth-owned copy of `scripts/install-ryzen-smu-dkms.sh`), or
- the separate `ryzen-smu-dkms` extra (the two-step flow above): `yay -S ryzen-smu-dkms && sudo ryzen-smu-dkms-install`, or
- no driver at all — RamSleuth runs and degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic).

Whichever path builds the module, the upstream source is **pinned, never branch-HEAD**: `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD). The helper prints the URL, the full pin, and the short sha before any fetch, shows the fetched commit + the `sha256sum` of the staged source files after the fetch, hard-verifies that `HEAD` equals the pin before any build (no silent branch-HEAD fallback), and pauses for a confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0). The staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366`.

Build posture: the `ramsleuth` and `ramsleuth-git` packages compile **only this repository** (tag-/branch-pinned, `--locked`, no third-party code in the build chroot), and `ramsleuth-bin` compiles nothing — it downloads the release tarball pinned by `sha256sums`. None depends on **any third-party AUR package** — the `ryzen-smu-dkms` extra is optional and co-install-safe (it ships the same helper under a different name, so it never conflicts with the RamSleuth packages). Every installed file is byte-identical to a file in this repository (auditable via `git show`), including the shipped helper and `install.sh` (`/usr/share/ramsleuth/install.sh`), so the whole flow can be re-run and audited post-install.

## CI

GitHub Actions runs on push and pull requests to `v2-development`:

- **test** — matrix `1.75` (MSRV) × `stable` on `ubuntu-latest` (`fail-fast: false`): `cargo test --workspace` (debug), `cargo test --workspace --release`, `cargo clippy --workspace --all-targets -- -D warnings`. The committed `Cargo.lock` is used as-is; the MSRV leg proves the 1.75 claim on a clean runner.
- **build** — `cargo build --release --workspace` on stable, uploading the 6 release binaries as an artifact.

## No-panic contract

Absent driver, privilege, or hardware never crashes the service or the clients: the affected fields read a structured `N/A (<reason>)` (e.g. `N/A (DriverMissing)` for a missing `ryzen_smu` module) and the process exits 0.
