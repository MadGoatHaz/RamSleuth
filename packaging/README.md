# RamSleuth v2 — Packaging

Operator/end-user guide for installing and running RamSleuth v2: the two AUR packages (`ramsleuth` stable source, `ramsleuth-bin` precompiled), **one-click setup** (no re-login, no reboot), the `ramsleuth` group, the sandboxed systemd unit, the optional **offline** `ryzen_smu` DKMS extra, the unified version bump, and CI.

## Overview

RamSleuth v2 is 100% pure Rust — a Cargo workspace of 8 crates (7 product crates + the `tools/gen-icon` dev tool), built with `cargo build --release --locked`, producing one privileged daemon and five unprivileged clients:

- `ramsleuth-daemon` — runs as root but holds only `CAP_SYS_RAWIO` (the capability required for the SMU and `/dev/mem` MCHBAR reads) and listens on a Unix socket.
- `ramsleuth-client` (CLI), `ramsleuth-tui`, `ramsleuth` (the GUI — needs a display, the `ramsleuth-gui` crate), `ramsleuth-bench`, `ramsleuth-telemetry`.

The socket lands at `/run/ramsleuth/ramsleuth.sock` (mode `0660`, group-owned; systemd creates the directory via `RuntimeDirectory=ramsleuth`). Clients need no privileges — only membership in the `ramsleuth` group **or** a per-user ACL granted by one-click setup (see below — the current session works immediately, no re-login).

## What the install does

The AUR packages (or `makepkg -si` from `packaging/ramsleuth/` or `packaging/ramsleuth-bin/`) install:

| Artifact | Destination |
| --- | --- |
| 6 binaries: `ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth`, `ramsleuth-bench`, `ramsleuth-telemetry` | `/usr/bin/` |
| The frozen daemon unit (`systemd/ramsleuth.service`) | `/usr/lib/systemd/system/ramsleuth.service` |
| The systemd preset (`00 enable ramsleuth.service`) | `/usr/lib/systemd/system-preset/ramsleuth.preset` |
| The `ramsleuth` system group | created on the target by the `.install` `pre_install`/`pre_upgrade` hooks (idempotent `groupadd -r`) |
| The ramsleuth-owned copy of the pinned `ryzen_smu` DKMS helper (`scripts/install-ryzen-smu-dkms.sh`) | `/usr/bin/ramsleuth-install-ryzen-smu-dkms` |
| The one-click setup helper (`scripts/ramsleuth-setup.sh` — the pkexec-able root helper) | `/usr/bin/ramsleuth-setup` |
| The shared polkit policy (`packaging/polkit/90-ramsleuth-setup.policy` — the `org.freedesktop.ramsleuth.setup` action) | `/usr/share/polkit-1/actions/90-ramsleuth-setup.policy` |
| The self-contained installer (`install.sh` — the transparency artifact, re-runnable/auditable post-install) | `/usr/share/ramsleuth/install.sh` |

`ramsleuth-protocol` is a library-only crate and is never installed. Both packages install the two one-click artifacts (the helper + polkit policy): `ramsleuth` builds them from source, and `ramsleuth-bin` takes them from the release tarball (which has carried them since the v2.2.0 re-cut).

## Installation

### AUR — the two-package model

```sh
yay -S ramsleuth      # STABLE source — builds from the official git tag v$pkgver
yay -S ramsleuth-bin  # PRECOMPILED — the binary from the GitHub Release (fastest install, no build)
```

(or `paru -S ...` in place of `yay`.)

- **`ramsleuth`** — the stable source package: builds the workspace from the official `v$pkgver` git tag (a reproducible, auditable snapshot). The default recommendation for production installs.
- **`ramsleuth-bin`** — the precompiled package: downloads the release binary tarball (`ramsleuth-$pkgver-x86_64.tar.zst`) from the official GitHub Release and installs it as-is — no build, no makedepends. The fastest install path.

**Mutual conflict:** `ramsleuth` and `ramsleuth-bin` install the identical file set, so each declares the other in `conflicts=` — the user picks exactly one. (The optional `ryzen-smu-dkms` extra stays co-install-safe with both — see below.)

**Package pages:** [ramsleuth](https://aur.archlinux.org/packages/ramsleuth) · [ramsleuth-bin](https://aur.archlinux.org/packages/ramsleuth-bin).

**Zero-touch on a sudo-invoked install:** both packages' `post_install` hooks, when run under `sudo`, additionally grant the invoking user group membership (persistent; effective at the next login) + seed `/etc/ramsleuth/authorized-users` (current-session socket access), then restart the daemon so the ACL applies immediately — every step guarded and never fatal. The hook then prints the next steps: start the GUI with `ramsleuth` (the TUI with `ramsleuth-tui`), one-click setup (the GUI's "Set up RamSleuth" button, or `sudo ramsleuth-setup` — see below), and on AMD hosts the optional `sudo ramsleuth-install-ryzen-smu-dkms` (the AUR-extra alternative is `yay -S ryzen-smu-dkms`), while Intel hosts get the note that the built-in MCHBAR decode needs no extra driver.

### Manual (from a source checkout)

```sh
cd packaging/ramsleuth   # or: packaging/ramsleuth-bin
makepkg -si
```

Build deps (the source package `ramsleuth`; `ramsleuth-bin` compiles nothing): `rust`, `cargo`, `pkgconf`, plus the X11/Wayland/GL library set in the PKGBUILD (`libxkbcommon` is the only strict build-time link dep). `./install.sh` from a fresh checkout does the same thing self-contained. The `post_install` hook runs `systemctl enable --now ramsleuth.service` — the daemon is started and enabled automatically, and the preset keeps it enabled on future `systemctl preset` runs. It performs the same zero-touch grant as the AUR path (group + ACL seed + daemon restart, guarded), and prints the next steps: start the GUI with `ramsleuth` (the TUI with `ramsleuth-tui`), one-click setup (the GUI's "Set up RamSleuth" button, or `sudo ramsleuth-setup` — see below), and on AMD hosts the optional `sudo ramsleuth-install-ryzen-smu-dkms` (offline vendored build, verified before any build; the AUR-extra alternative is `yay -S ryzen-smu-dkms`), while Intel hosts get the note that the built-in MCHBAR decode needs no extra driver.

## One-click setup

After install (any path), the remaining privileged work is **one action** — no copy-paste, **no re-login, no reboot**:

- **GUI** — the first-run SETUP strip's primary **"Set up RamSleuth"** button (labelled `… + AMD driver` when the AMD module is the missing piece). One click → **one polkit password prompt** (`auth_admin`, via the installed `90-ramsleuth-setup.policy`) → the helper performs every privileged step as root in a single session; the strip's status line then reads `done — full capabilities active` (on failure it prints the manual `sudo` pointer instead; the per-row Copy fallback is kept as the polkit-less grace path).
- **CLI** — the same entrypoint from a terminal:

  ```sh
  sudo ramsleuth-setup              # daemon + group + current-session access
  sudo ramsleuth-setup --with-dkms  # + build & load the AMD driver (offline vendor)
  ```

  The GUI invokes the identical helper as `pkexec /usr/bin/ramsleuth-setup --user <you>` (under `pkexec` `$SUDO_USER` is unset, so the caller passes the user explicitly; from `sudo` the default is `$SUDO_USER`).

What each step does (all **idempotent** — a re-run is a no-op; any failure prints a structured message and exits non-zero, never a silent half-state):

1. `systemctl daemon-reload` + `systemctl enable --now ramsleuth.service` — the daemon is running (the one hard step).
2. `usermod -aG ramsleuth <user>` — group membership, **persisted for future logins** (skipped when already a member).
3. Append `<user>` to `/etc/ramsleuth/authorized-users` — the state file the daemon re-applies as a per-user socket ACL on **every** socket creation (the socket lives on tmpfs `/run`, so on every daemon (re)start and every boot).
4. Best-effort `setfacl -m u:<user>:rw` on the **live** socket — the **current session** gets immediate access (warn-only if the `acl` package is absent: the daemon re-applies from the state file at its next (re)start; `acl` is in Arch base).
5. With `--with-dkms`: `exec` the installed DKMS helper (`/usr/bin/ramsleuth-install-ryzen-smu-dkms`) — the offline vendored build + `modprobe` (immediate; the frozen unit never loads the module itself).

**polkit + the sudo floor:** the policy (action `org.freedesktop.ramsleuth.setup`, `auth_admin` for any/active/inactive/other) maps `pkexec` to `/usr/bin/ramsleuth-setup`; polkit is in Arch **base**, so the prompt machinery is present on every stock system (a graphical agent provides the visible prompt — standard on GNOME/KDE/X11). The helper never *requires* polkit: a bare invocation re-execs under `sudo`, which is the floor — headless boxes just use `sudo ramsleuth-setup`.

**`ramsleuth-bin`:** the v2.2.0 re-cut has landed, so the release tarball now carries the helper + policy — a `-bin` machine has the full one-click entrypoint (no more degraded copy-paste path).

## Version bump (unified versioning — standing policy)

This is the **standing** release policy: it applies to **every** release, not a one-time note. The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml` — all member crates inherit it, and the GUI window titles derive it at compile time via `env!("CARGO_PKG_VERSION")`, so a bump updates the titles automatically (no manual edit). The full bump flow:

1. Bump `[workspace.package].version` in the root `Cargo.toml` (e.g. `2.1.1` → `2.2.0`).
2. `cargo update -w` to sync `Cargo.lock` (the member lines).
3. Commit + push.
4. Cut the git tag `v<ver>` — the release workflow (`.github/workflows/release.yml`) fires on the tag and auto-builds + publishes the GitHub Release with the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` and its `sha256` companion.
5. Update the AUR packages: `pkgver` in `packaging/ramsleuth/PKGBUILD` (the git-tag source) and `pkgver` in `packaging/ramsleuth-bin/PKGBUILD` (the tarball download), then finalize the `ramsleuth-bin` `sha256sums` from the **published** release `.sha256` (AUR requires a real `sha256` — no SKIP; the value is a placeholder until the release exists).
6. Push both AUR packages.

## The `ramsleuth` group

The socket is `0660` group-owned; clients must be in the group — or hold a per-user ACL — to read telemetry. The group is created automatically on install — without it the unit fails to start (the one real install gap). There are **two** access mechanisms; after setup (the wizard, the AUR zero-touch hook, `install.sh`, or `ramsleuth-setup`) **neither a re-login nor a reboot is needed**:

1. **Current session — immediate, no re-login.** A POSIX ACL on the socket (`u:<user>:rw`) grants the user access right away. The daemon re-applies these ACLs from `/etc/ramsleuth/authorized-users` (dir 0755, file 0644, one username per line) on **every** socket creation — every daemon (re)start and every boot, since the socket lives on tmpfs `/run`.
2. **Future logins — persistence.** Group membership via `sudo usermod -aG ramsleuth <user>` (the AUR hooks / `install.sh` / the helper do this automatically for the invoking user). PAM applies group membership only at login time, so this half covers the next and later sessions — it is kept so a plain re-login keeps working.

Manual grant (only needed if no path did it for you):

```sh
sudo usermod -aG ramsleuth <user>     # effective at the next login
```

…or just run `sudo ramsleuth-setup` for immediate current-session access (it does the group join and the ACL in one go). On systems that cannot provide the group, the unit documents a `Group=wheel` fallback (edit the installed unit, then `systemctl daemon-reload`).

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

### Source: offline vendor, pinned git as fallback

The module source is **pinned, never branch-HEAD**: `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD). The helper resolves the source in this order:

1. **Vendored (offline — zero network).** A byte-frozen copy of the six pinned files (`LICENSE`, `Makefile`, `dkms.conf`, `drv.c`, `smu.c`, `smu.h`) lives in-repo at `packaging/ryzen-smu-dkms/vendor/ryzen-smu` (a dev checkout) and is installed to `/usr/share/ryzen-smu-dkms/vendor/ryzen-smu` by the `ryzen-smu-dkms` package. Before any build, every file is verified against `vendor/SUMS.sha256` — a mismatch dies (no silent fallback) — and the DKMS version is the fixed `1.d298366`, stable across kernel updates (which makes `dkms add` idempotent).
2. **Pinned git clone (the fallback).** Used when no vendor tree is present (e.g. a `ramsleuth-bin` tarball install — the tarball deliberately excludes the vendor dir for size): shallow clone → fetch → checkout, with a **hard verify that `HEAD` equals the pin** before any build (no silent branch-HEAD fallback; if the pin vanished upstream, a one-line `RYZEN_SMU_PIN` re-pin is the fix).

Env overrides (git path): `RYZEN_SMU_PIN` selects a specific upstream commit (the vendor carries only the default pin, so an override takes the git path), `RYZEN_SMU_URL` overrides the clone URL, and `RYZEN_SMU_FORCE_REMOTE=1` forces the git path even when a vendor tree is present.

Licensing: the source is **GPL-2.0** — a **separate work** from the MIT RamSleuth code. It is byte-identical and unmodified, carries its verbatim `LICENSE` + `NOTICE.md`, and is built **only** by DKMS in `/usr/src/ryzen_smu-<version>` — never compiled into, linked with, or bundled as part of any RamSleuth binary (see `packaging/ryzen-smu-dkms/vendor/NOTICE.md`). The `ryzen-smu-dkms` package lists both licenses (`license=(MIT GPL-2.0-only)`); the two `ramsleuth*` packages install **no** vendor files (their `license=(MIT)` stays accurate).

### Enabling live subtimings on an AMD host

1. One of:
   - `sudo ramsleuth-setup --with-dkms` — the one-click path (all ramsleuth installs ship the helper; see the name table below); or
   - `yay -S ryzen-smu-dkms` (or `paru -S ryzen-smu-dkms`) — the provisioning extra: bundles the DKMS config at `/usr/share/ryzen-smu-dkms/dkms.conf`, the helper, **the vendored source** (so the build is network-free), and docs. It is thin by design — no module build in the build chroot (it would build against the chroot's kernel, not the target's); the module is built on the **target** via:

     ```sh
     sudo ryzen-smu-dkms-install
     ```

   - or, from a source checkout (no package needed): `scripts/install-ryzen-smu-dkms.sh`.

2. The helper is idempotent and re-execs under `sudo`; it requires the matching kernel headers (build tree `/lib/modules/$(uname -r)/build` — for custom-kernel hosts it lists candidate packages and stops, never guessing), prints the provenance (the URL + full pin + short sha; after resolution, the `sha256` fingerprints of the six staged files) and pauses for confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0), then runs `dkms add/build/install`, `modprobe` (immediate — **no reboot**), and persists `/etc/modules-load.d/ryzen_smu.conf`. The staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366` (vendor path) or `/usr/src/ryzen_smu-<revcount>.<short>` (git path).

### The two DKMS helper names

The **same script** (`scripts/install-ryzen-smu-dkms.sh`) is installed under **two** names, depending on which package provides it:

| Name | Provided by |
| --- | --- |
| `/usr/bin/ramsleuth-install-ryzen-smu-dkms` | `install.sh` and both ramsleuth AUR packages (`ramsleuth`, `ramsleuth-bin`) — this is the name `ramsleuth-setup --with-dkms` delegates to |
| `/usr/bin/ryzen-smu-dkms-install` | the standalone `ryzen-smu-dkms` AUR extra only (plan D-18.4) |

The distinct names are deliberate: they keep the extra **co-install-safe** with any ramsleuth package (no pacman file conflict). Both behave identically — use the one your install provides.

### Day-2

`AUTOINSTALL=yes` in the bundled `dkms.conf` auto-rebuilds the module on kernel updates. Verify:

```sh
ls /sys/kernel/ryzen_smu_drv/pm_table
```

Two operational notes: the `monitor_cpu` ground-truth CLI is built only when the source includes a `userspace/` dir — the vendored tree omits it, so a vendor-path build prints the pre-existing `monitor_cpu unavailable` warning (the module itself is unaffected; a git-path build includes it). And on a box that registered an old git-path DKMS version (`<revcount>.d298366`) before the vendor landed, the vendor path registers `1.d298366` **alongside** it — harmless (AUTOINSTALL rebuilds both); tidy up with `sudo dkms remove ryzen_smu/<old>`.

### AUR parity & transparency

An AUR install gets the same scripting as the GitHub path (`./install.sh` from a source checkout) — plus a third, driver-less path:

- `sudo ramsleuth-install-ryzen-smu-dkms` — the helper both AUR packages ship at that path (the ramsleuth-owned copy of `scripts/install-ryzen-smu-dkms.sh`) — or `sudo ramsleuth-setup --with-dkms` for the one-click route;
- the separate `ryzen-smu-dkms` extra (the flow above): `yay -S ryzen-smu-dkms && sudo ryzen-smu-dkms-install`;
- no driver at all — RamSleuth runs and degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic).

Whichever path builds the module, the upstream source is **pinned, never branch-HEAD**: `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD). The helper prints the URL, the full pin, and the short sha before any fetch, shows the `sha256` fingerprints of the six staged source files after resolution, hard-verifies the source (the vendor `SUMS.sha256` manifest on the vendor path; `HEAD == pin` on the git path — no silent branch-HEAD fallback), and pauses for a confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0). The staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366` (vendor) or `/usr/src/ryzen_smu-<revcount>.<short>` (git).

Build posture: the `ramsleuth` package compiles **only this repository** (tag-pinned, `--locked`, no third-party code in the build chroot), and `ramsleuth-bin` compiles nothing — it downloads the release tarball pinned by `sha256sums`. None depends on **any third-party AUR package** — the `ryzen-smu-dkms` extra is optional and co-install-safe (it ships the same helper under a different name, so it never conflicts with the RamSleuth packages). The vendored GPL-2.0 driver source ships **only** via that extra — the `ramsleuth*` packages and the release tarball carry none of it. Every installed file is byte-identical to a file in this repository (auditable via `git show`), including the shipped helper and `install.sh` (`/usr/share/ramsleuth/install.sh`), so the whole flow can be re-run and audited post-install.

### The release → AUR lockstep (standing process)

The v2.2.0 re-cut is **done** (the one-click helper + polkit policy now ship in every tarball and source build), and v2.2.1 is the current cut (tag `v2.2.1` @ `c82a9ad`). The standing process for **each** release is:

1. Cut the git tag `v<ver>` — the release workflow (`.github/workflows/release.yml`) publishes the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`.
2. **`ramsleuth`** (stable source) — bump `pkgver` to `<ver>`; it builds from the `v<ver>` tag. Push.
3. **`ramsleuth-bin`** (precompiled) — bump `pkgver` to `<ver>` and re-pin `sha256sums` to the **published** release `.sha256` (a real sha256, no placeholder). Push.

## CI

GitHub Actions runs on push and pull requests to `v2-development`:

- **test** — matrix `1.75` (MSRV) × `stable` on `ubuntu-latest` (`fail-fast: false`): `cargo test --workspace` (debug), `cargo test --workspace --release`, `cargo clippy --workspace --all-targets -- -D warnings`. The committed `Cargo.lock` is used as-is; the MSRV leg proves the 1.75 claim on a clean runner.
- **build** — `cargo build --release --workspace` on stable, uploading the 6 release binaries as an artifact.

## No-panic contract

Absent driver, privilege, or hardware never crashes the service or the clients: the affected fields read a structured `N/A (<reason>)` (e.g. `N/A (DriverMissing)` for a missing `ryzen_smu` module) and the process exits 0.
