# RamSleuth v2 — Packaging

Operator/end-user guide for installing and running RamSleuth v2: the AUR package model — the **two main packages** (`ramsleuth` stable source, `ramsleuth-bin` precompiled), which **both bundle the vendor driver sources** — the in-repo `ramsleuth_intel` DKMS source (Intel — a first-class, validated feature) **and** the vendored `ryzen_smu` tree (AMD — the offline one-click source) — plus the **two optional standalone vendor DKMS extras** (`ryzen-smu-dkms`, AMD, and `ramsleuth-intel-dkms`, Intel — **mutually exclusive** with the main packages, which already carry their source) — **one-click setup** (no re-login, no reboot), the `ramsleuth` group, the sandboxed systemd unit, the unified version bump, and CI.

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
| The ramsleuth-owned copy of the in-repo `ramsleuth_intel` DKMS helper (`scripts/install-intel-dkms.sh`) | `/usr/bin/ramsleuth-install-intel-dkms` (guarded — the helper is in the current source tree, which the `ramsleuth` source build installs, and in the v2.4.6 `-bin` release tarball; the guard covers pre-2.4.2 tags and the old published v2.2.1 release tarball (the `ramsleuth-bin` asset), where the install is skipped cleanly) |
| The in-repo `ramsleuth_intel` DKMS source tree (`kernel/ramsleuth-intel/` — the 4 files `dkms.conf`, `Makefile`, `ramsleuth_intel.c`, `README.md`) | `/usr/share/ramsleuth-intel-dkms/src/` (the exact path the Intel helper resolves as its installed copy — so the one-click Intel DKMS install works from a bare AUR install with no manual source step; guarded like the helper: a pre-2.4.2 tag / the old v2.2.1 tarball ships nothing and skips cleanly) |
| The vendored `ryzen_smu` DKMS source tree (`packaging/ryzen-smu-dkms/vendor/` — the 6 module files frozen at upstream `d298366` + `SUMS.sha256` + `NOTICE.md`) | `/usr/share/ryzen-smu-dkms/vendor/` (the exact path the AMD helper resolves as its offline vendored source — so the one-click AMD DKMS install works from a bare AUR install with **zero network**; guarded: a pre-vendor tag / the old published tarball ships nothing and skips cleanly) |
| The one-click setup helper (`scripts/ramsleuth-setup.sh` — the pkexec-able root helper) | `/usr/bin/ramsleuth-setup` |
| The shared polkit policy (`packaging/polkit/90-ramsleuth-setup.policy` — the `org.freedesktop.ramsleuth.setup` action) | `/usr/share/polkit-1/actions/90-ramsleuth-setup.policy` |
| The self-contained installer (`install.sh` — the transparency artifact, re-runnable/auditable post-install) | `/usr/share/ramsleuth/install.sh` |

`ramsleuth-protocol` is a library-only crate and is never installed. Both packages install the two one-click artifacts (the helper + polkit policy): `ramsleuth` builds them from source, and `ramsleuth-bin` takes them from the release tarball (which has carried them since the v2.2.0 re-cut). Both also install the ramsleuth-owned copies of the two DKMS helpers (the `/usr/bin/ramsleuth-install-ryzen-smu-dkms` / `/usr/bin/ramsleuth-install-intel-dkms` pair — see the name table in the AMD extra section below), **both bundle the in-repo `ramsleuth_intel` DKMS source tree** to `/usr/share/ramsleuth-intel-dkms/src/` (the exact path the Intel helper resolves, so the one-click Intel DKMS install works from a bare AUR install with no manual source step), **and both bundle the vendored `ryzen_smu` source tree** to `/usr/share/ryzen-smu-dkms/vendor/` (the exact path the AMD helper resolves as its offline vendored source, so the one-click AMD DKMS install works from a bare AUR install with zero network); all four are guarded the same way — they are in the current source tree and in the v2.4.6 `-bin` release tarball (the `ramsleuth` source build installs them), and the guards cover pre-2.4.2 tags (the Intel side), pre-vendor tags (the AMD side), and the old published v2.2.1 tarball (the `ramsleuth-bin` asset), where the install is skipped cleanly.

## Installation

### AUR — two main packages + two optional vendor DKMS extras

```sh
yay -S ramsleuth            # STABLE source — builds from the official git tag v$pkgver
yay -S ramsleuth-bin        # PRECOMPILED — the binary from the GitHub Release (fastest install, no build)
yay -S ryzen-smu-dkms       # OPTIONAL (AMD) — the pinned ryzen_smu DKMS module (live AMD subtimings)
yay -S ramsleuth-intel-dkms # OPTIONAL (Intel) — the standalone ramsleuth_intel provisioning extra; mutually
                            # exclusive with the main packages (installing it removes a main package first —
                            # the mains already bundle the Intel DKMS source, so it is redundant for Intel)
```

(or `paru -S ...` in place of `yay`.)

- **`ramsleuth`** — the stable source package: builds the workspace from the official `v$pkgver` git tag (a reproducible, auditable snapshot). The default recommendation for production installs.
- **`ramsleuth-bin`** — the precompiled package: downloads the release binary tarball (`ramsleuth-$pkgver-x86_64.tar.zst`) from the official GitHub Release and installs it as-is — no build, no makedepends. The fastest install path.

- **The two vendor extras** — **`ryzen-smu-dkms`** (AMD) and **`ramsleuth-intel-dkms`** (Intel): optional, thin provisioning packages that build the matching DKMS module **on the target** (never in the build chroot) and install their own copy of the operator helper under a distinct name. Install the one that matches your silicon — the full name table lives in the AMD extra section below. **Both main packages now bundle both vendor driver sources** — the in-repo `ramsleuth_intel` tree (to `/usr/share/ramsleuth-intel-dkms/src/` — the exact path the Intel helper resolves) **and** the vendored `ryzen_smu` tree (to `/usr/share/ryzen-smu-dkms/vendor/` — the exact path the AMD helper resolves) — so **both** extras declare `conflicts=` against both mains (and they against each extra) — **mutually exclusive**: installing an extra removes a main package first, and each extra is now the **standalone/redundant** provisioning path for its vendor (the Intel module is a first-class, validated feature — in-repo source, built warning-free by the CI `kernel-module` job, 25 frozen sysfs attributes; the AMD module's source is pinned + vendored).

**Mutual conflict:** `ramsleuth` and `ramsleuth-bin` install the identical file set, so each declares the other in `conflicts=` — the user picks exactly one. **Both** vendor extras **conflict with both mains** (and they with each extra): the Intel extra (`ramsleuth-intel-dkms`) shares the `/usr/share/ramsleuth-intel-dkms/src/` source tree the mains now bundle, and the AMD extra (`ryzen-smu-dkms`) shares the `/usr/share/ryzen-smu-dkms/vendor/` tree the mains now bundle — so either extra can only be installed after removing a main package.

**Package pages:** [ramsleuth](https://aur.archlinux.org/packages/ramsleuth) · [ramsleuth-bin](https://aur.archlinux.org/packages/ramsleuth-bin) · [ryzen-smu-dkms](https://aur.archlinux.org/packages/ryzen-smu-dkms) · [ramsleuth-intel-dkms](https://aur.archlinux.org/packages/ramsleuth-intel-dkms).

**Zero-touch on a sudo-invoked install:** both packages' `post_install` hooks, when run under `sudo`, additionally grant the invoking user group membership (persistent; effective at the next login) + seed `/etc/ramsleuth/authorized-users` (current-session socket access), then restart the daemon so the ACL applies immediately — every step guarded and never fatal. The hook then prints the next steps: start the GUI with `ramsleuth` (the TUI with `ramsleuth-tui`), one-click setup (the GUI's "Set up RamSleuth" button, or `sudo ramsleuth-setup` — see below), and on AMD hosts the optional `sudo ramsleuth-install-ryzen-smu-dkms` (**offline** — the vendored source is bundled with the package; the standalone `ryzen-smu-dkms` extra is the only alternative, and it is mutually exclusive with the main packages — installing it removes a main package first), while Intel hosts get the optional `sudo ramsleuth-install-intel-dkms` for live Intel subtimings via the DKMS module (the built-in `/dev/mem` path remains the fallback; the standalone `ramsleuth-intel-dkms` extra is the only alternative, and it is mutually exclusive with the main packages — installing it removes a main package first).

### Manual (from a source checkout)

```sh
cd packaging/ramsleuth   # or: packaging/ramsleuth-bin
makepkg -si
```

Build deps (the source package `ramsleuth`; `ramsleuth-bin` compiles nothing): `rust`, `cargo`, `pkgconf`, plus the X11/Wayland/GL library set in the PKGBUILD (`libxkbcommon` is the only strict build-time link dep). `./install.sh` from a fresh checkout does the same thing self-contained. The `post_install` hook runs `systemctl enable --now ramsleuth.service` — the daemon is started and enabled automatically, and the preset keeps it enabled on future `systemctl preset` runs. It performs the same zero-touch grant as the AUR path (group + ACL seed + daemon restart, guarded), and prints the next steps: start the GUI with `ramsleuth` (the TUI with `ramsleuth-tui`), one-click setup (the GUI's "Set up RamSleuth" button, or `sudo ramsleuth-setup` — see below), and on AMD hosts the optional `sudo ramsleuth-install-ryzen-smu-dkms` (offline vendored build — the vendored source tree ships in the checkout, verified against `SUMS.sha256` before any build; the AUR-extra alternative is `yay -S ryzen-smu-dkms`, **mutually exclusive** with the app), while Intel hosts get the optional `sudo ramsleuth-install-intel-dkms` (the in-repo module; the standalone `ramsleuth-intel-dkms` extra is the only alternative, and it is mutually exclusive with the app) — with the built-in `/dev/mem` path remaining the fallback where the module is absent.

## One-click setup

After install (any path), the remaining privileged work is **one action** — no copy-paste, **no re-login, no reboot**:

- **GUI** — the first-run SETUP strip's primary **"Set up RamSleuth"** button (labelled `… + AMD driver` when the AMD module is the missing piece). One click → **one polkit password prompt** (`auth_admin`, via the installed `90-ramsleuth-setup.policy`) → the helper performs every privileged step as root in a single session; the strip's status line then reads `done — full capabilities active` (on failure it prints the manual `sudo` pointer instead; the per-row Copy fallback is kept as the polkit-less grace path). The GUI's DKMS arm is the AMD driver (the `+ AMD driver` label); on Intel hosts the button performs the same daemon + group + ACL pass, and the Intel driver is added from the terminal with `sudo ramsleuth-setup --with-dkms` (vendor-aware) or `--with-intel-dkms`.
- **CLI** — the same entrypoint from a terminal:

  ```sh
  sudo ramsleuth-setup                  # daemon + group + current-session access
  sudo ramsleuth-setup --with-dkms     # + build & load the vendor DKMS module — auto-routed by the
                                       # CPU vendor (AMD → the offline ryzen_smu vendor;
                                       # Intel → the in-repo ramsleuth_intel module)
  sudo ramsleuth-setup --with-intel-dkms  # + force the Intel arm (Intel-only fast path)
  ```

  The GUI invokes the identical helper as `pkexec /usr/bin/ramsleuth-setup --user <you>` (under `pkexec` `$SUDO_USER` is unset, so the caller passes the user explicitly; from `sudo` the default is `$SUDO_USER`).

What each step does (all **idempotent** — a re-run is a no-op; any failure prints a structured message and exits non-zero, never a silent half-state):

1. `systemctl daemon-reload` + `systemctl enable --now ramsleuth.service` — the daemon is running (the one hard step).
2. `usermod -aG ramsleuth <user>` — group membership, **persisted for future logins** (skipped when already a member).
3. Append `<user>` to `/etc/ramsleuth/authorized-users` — the state file the daemon re-applies as a per-user socket ACL on **every** socket creation (the socket lives on tmpfs `/run`, so on every daemon (re)start and every boot).
4. Best-effort `setfacl -m u:<user>:rw` on the **live** socket — the **current session** gets immediate access (warn-only if the `acl` package is absent: the daemon re-applies from the state file at its next (re)start; `acl` is in Arch base).
5. With `--with-dkms` (**vendor-aware** — routed by the CPU `vendor_id`): `exec` the installed vendor DKMS helper — AMD: `/usr/bin/ramsleuth-install-ryzen-smu-dkms` (the offline vendored build); Intel: `/usr/bin/ramsleuth-install-intel-dkms` (the in-repo module build) — + `modprobe` (immediate; the frozen unit never loads either module itself). `--with-intel-dkms` forces the Intel arm (an Intel-only fast path; a hard failure on non-Intel silicon); any other/unknown vendor gets a clear error, no DKMS install.

**polkit + the sudo floor:** the policy (action `org.freedesktop.ramsleuth.setup`, `auth_admin` for any/active/inactive/other) maps `pkexec` to `/usr/bin/ramsleuth-setup`; polkit is in Arch **base**, so the prompt machinery is present on every stock system (a graphical agent provides the visible prompt — standard on GNOME/KDE/X11). The helper never *requires* polkit: a bare invocation re-execs under `sudo`, which is the floor — headless boxes just use `sudo ramsleuth-setup`. pkexec resolves the action by the `exec.path` annotation alone (against the realpath-resolved program path — the helper's `--user <name>` / `--with-dkms` flags never participate in the match, and polkit has no argument-list key), so every `pkexec ramsleuth-setup …` form gets the branded message. A polkitd that started before the policy file was (re)installed may keep serving a stale action pool (its inotify hot-reload is not reliable for it — observed 2026-09-25), so both AUR packages' `.install` hooks and `install.sh` run `systemctl restart polkit` after installing the policy; a manual copy of the policy needs the same restart (or a reboot) before the branded dialog appears.

**`ramsleuth-bin`:** the v2.2.0 re-cut has landed, so the release tarball now carries the helper + policy — a `-bin` machine has the full one-click entrypoint (no more degraded copy-paste path).

## Version bump (unified versioning — standing policy)

This is the **standing** release policy: it applies to **every** release, not a one-time note. The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml` — all member crates inherit it, and the GUI window titles derive it at compile time via `env!("CARGO_PKG_VERSION")`, so a bump updates the titles automatically (no manual edit). The full bump flow:

1. Bump `[workspace.package].version` in the root `Cargo.toml` (e.g. `2.1.1` → `2.2.0`).
2. `cargo update -w` to sync `Cargo.lock` (the member lines).
3. Commit + push.
4. Cut the git tag `v<ver>` — the release workflow (`.github/workflows/release.yml`) fires on the tag and auto-builds + publishes the GitHub Release with the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` and its `sha256` companion.
5. Update the AUR packages: `pkgver` in `packaging/ramsleuth/PKGBUILD` (the git-tag source) and `pkgver` in `packaging/ramsleuth-bin/PKGBUILD` (the tarball download), the fixed `pkgver` in `packaging/intel-dkms/PKGBUILD` (the Intel extra — it tracks the workspace version, so it bumps with the others), then finalize the `ramsleuth-bin` `sha256sums` from the **published** release `.sha256` (AUR requires a real `sha256` — no SKIP; the value is a placeholder until the release exists).
6. Push all three AUR packages.

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

The unit never loads the `ryzen_smu` or `ramsleuth_intel` module.

## Optional: `ryzen_smu` DKMS extra (live AMD subtimings)

**Not a hard dependency.** Without the module, RamSleuth degrades gracefully: the AMD subtiming fields read `N/A (DriverMissing)`, exit 0, no panic.

### Source: offline vendor, pinned git as fallback

The module source is **pinned, never branch-HEAD**: `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD). The helper resolves the source in this order:

1. **Vendored (offline — zero network).** A byte-frozen copy of the six pinned files (`LICENSE`, `Makefile`, `dkms.conf`, `drv.c`, `smu.c`, `smu.h`) lives in-repo at `packaging/ryzen-smu-dkms/vendor/ryzen-smu` (a dev checkout) and is installed to `/usr/share/ryzen-smu-dkms/vendor/ryzen-smu` by **all three** ramsleuth packages — the `ryzen-smu-dkms` extra **and** both main packages (which bundle it for the offline one-click; guarded: a pre-vendor tag / old tarball ships nothing and skips cleanly). Before any build, every file is verified against `vendor/SUMS.sha256` — a mismatch dies (no silent fallback) — and the DKMS version is the fixed `1.d298366`, stable across kernel updates (which makes `dkms add` idempotent).
2. **Pinned git clone (the fallback).** Used when no vendor tree is present (e.g. a build against a pre-vendor tag or the old published tarball): shallow clone → fetch → checkout, with a **hard verify that `HEAD` equals the pin** before any build (no silent branch-HEAD fallback; if the pin vanished upstream, a one-line `RYZEN_SMU_PIN` re-pin is the fix).

Env overrides (git path): `RYZEN_SMU_PIN` selects a specific upstream commit (the vendor carries only the default pin, so an override takes the git path), `RYZEN_SMU_URL` overrides the clone URL, and `RYZEN_SMU_FORCE_REMOTE=1` forces the git path even when a vendor tree is present.

Licensing: the source is **GPL-2.0** — a **separate work** from the MIT RamSleuth code. It is byte-identical and unmodified, carries its verbatim `LICENSE` + `NOTICE.md`, and is built **only** by DKMS in `/usr/src/ryzen_smu-<version>` — never compiled into, linked with, or bundled as part of any RamSleuth binary (see `packaging/ryzen-smu-dkms/vendor/NOTICE.md`). The `ryzen-smu-dkms` package lists both licenses (`license=(MIT GPL-2.0-only)`); the two `ramsleuth*` packages **bundle the same `ryzen_smu` vendor tree** (a separate GPL-2.0 work) **and** the in-repo `ramsleuth_intel` module source (see the Intel extra section below), so their license field is `license=(MIT GPL-2.0-only)`.

### Enabling live subtimings on an AMD host

1. One of:
   - `sudo ramsleuth-setup --with-dkms` — the one-click path (all ramsleuth installs ship the helper **and the vendored source** — offline; see the name table below); or
   - `yay -S ryzen-smu-dkms` (or `paru -S ryzen-smu-dkms`) — the **standalone** provisioning extra (**mutually exclusive** with the main packages — they bundle the same vendored source, so installing it removes a main package first): bundles the DKMS config at `/usr/share/ryzen-smu-dkms/dkms.conf`, the helper, **the vendored source** (so the build is network-free), and docs. It is thin by design — no module build in the build chroot (it would build against the chroot's kernel, not the target's); the module is built on the **target** via:

     ```sh
     sudo ryzen-smu-dkms-install
     ```

   - or, from a source checkout (no package needed): `scripts/install-ryzen-smu-dkms.sh`.

2. The helper is idempotent and re-execs under `sudo`; it requires the matching kernel headers (build tree `/lib/modules/$(uname -r)/build` — for custom-kernel hosts it lists candidate packages and stops, never guessing), prints the provenance (the URL + full pin + short sha; after resolution, the `sha256` fingerprints of the six staged files) and pauses for confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0), then runs `dkms add/build/install`, `modprobe` (immediate — **no reboot**), and persists `/etc/modules-load.d/ryzen_smu.conf`. The staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366` (vendor path) or `/usr/src/ryzen_smu-<revcount>.<short>` (git path).

### The four DKMS helper names (two per vendor)

**Each vendor's script is installed under two names**, depending on which package provides it — four names in total:

| Script (in-repo) | Installed name | Provided by |
| --- | --- | --- |
| `scripts/install-ryzen-smu-dkms.sh` (AMD) | `/usr/bin/ramsleuth-install-ryzen-smu-dkms` | `install.sh` and both ramsleuth AUR packages (`ramsleuth`, `ramsleuth-bin`) — this is the name `ramsleuth-setup --with-dkms` (the AMD arm) delegates to |
| (same AMD script) | `/usr/bin/ryzen-smu-dkms-install` | the standalone `ryzen-smu-dkms` AUR extra only (plan D-18.4) |
| `scripts/install-intel-dkms.sh` (Intel) | `/usr/bin/ramsleuth-install-intel-dkms` | `install.sh` and both ramsleuth AUR packages (guarded — the helper is in the current source tree and in the v2.4.6 `-bin` release tarball; the guard covers pre-2.4.2 tags and the old v2.2.1 tarball) — this is the name `ramsleuth-setup --with-dkms` (the Intel arm) and `--with-intel-dkms` delegate to |
| (same Intel script) | `/usr/bin/ramsleuth-intel-dkms-install` | the standalone `ramsleuth-intel-dkms` AUR extra only |

The distinct names are deliberate: they keep the **helper binary itself** out of a pacman file conflict (the ONE shared file each extra would otherwise collide on). Both extras are **mutually exclusive** with the mains as a whole — the AMD extra shares the bundled `/usr/share/ryzen-smu-dkms/vendor/` tree, the Intel extra the bundled `/usr/share/ramsleuth-intel-dkms/src/` tree, so pacman enforces the mutual conflict by file and removes a main package first. The two names of one script behave identically — use the one your install provides.

### Day-2

`AUTOINSTALL=yes` in the bundled `dkms.conf` auto-rebuilds the module on kernel updates. Verify:

```sh
ls /sys/kernel/ryzen_smu_drv/pm_table
```

Two operational notes: the `monitor_cpu` ground-truth CLI is built only when the source includes a `userspace/` dir — the vendored tree omits it, so a vendor-path build prints the pre-existing `monitor_cpu unavailable` warning (the module itself is unaffected; a git-path build includes it). And on a box that registered an old git-path DKMS version (`<revcount>.d298366`) before the vendor landed, the vendor path registers `1.d298366` **alongside** it — harmless (AUTOINSTALL rebuilds both); tidy up with `sudo dkms remove ryzen_smu/<old>`.

### AUR parity & transparency

An AUR install gets the same scripting as the GitHub path (`./install.sh` from a source checkout) — plus a third, driver-less path:

- `sudo ramsleuth-install-ryzen-smu-dkms` — the helper both AUR packages ship at that path (the ramsleuth-owned copy of `scripts/install-ryzen-smu-dkms.sh`; **offline** — the vendored source is bundled with the package) — or `sudo ramsleuth-setup --with-dkms` for the one-click route;
- the separate `ryzen-smu-dkms` extra (the flow above): `yay -S ryzen-smu-dkms && sudo ryzen-smu-dkms-install` — **standalone only**: it is mutually exclusive with the main packages (shared vendored source), so on an app host it is an alternative to the app, not an add-on to it;
- no driver at all — RamSleuth runs and degrades gracefully (`N/A (DriverMissing)`, exit 0, no panic).

Whichever path builds the module, the upstream source is **pinned, never branch-HEAD**: `amkillam/ryzen_smu` @ `d2983668300dd2a598e5a7dc40e71ce0678cc270` (verified 2026-08-15, the current `main` HEAD). The helper prints the URL, the full pin, and the short sha before any fetch, shows the `sha256` fingerprints of the six staged source files after resolution, hard-verifies the source (the vendor `SUMS.sha256` manifest on the vendor path; `HEAD == pin` on the git path — no silent branch-HEAD fallback), and pauses for a confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0). The staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366` (vendor) or `/usr/src/ryzen_smu-<revcount>.<short>` (git).

Build posture: the `ramsleuth` package compiles **only this repository** (tag-pinned, `--locked`, no third-party code in the build chroot), and `ramsleuth-bin` compiles nothing — it downloads the release tarball pinned by `sha256sums`. Neither ramsleuth package depends on **any third-party AUR package** — **both** vendor extras are optional and **mutually exclusive** with the mains (the AMD extra shares the bundled `/usr/share/ryzen-smu-dkms/vendor/` tree, the Intel extra the bundled `/usr/share/ramsleuth-intel-dkms/src/` tree — each is the standalone provisioning path for its vendor). GPL-2.0 module source ships via the matching package — the vendored `ryzen_smu` via **both** the `ryzen-smu-dkms` extra **and the two main packages** (which bundle it to `/usr/share/ryzen-smu-dkms/vendor/`; the release tarball carries the vendor dir by the 27-artifact contract), the in-repo `ramsleuth_intel` tree via **both** the `ramsleuth-intel-dkms` extra **and the two main packages** (which bundle it to `/usr/share/ramsleuth-intel-dkms/src/`; their `license=(MIT GPL-2.0-only)` reflects that — the bundled trees are separate works, never compiled into any RamSleuth binary). Every installed file is byte-identical to a file in this repository (auditable via `git show`), including the shipped helpers and `install.sh` (`/usr/share/ramsleuth/install.sh`), so the whole flow can be re-run and audited post-install.

### The release → AUR lockstep (standing process)

The v2.2.0 re-cut is **done** (the one-click helper + polkit policy now ship in every tarball and source build), and the workspace is at **v2.4.6** (the tag + release re-cut are operator-gated), in which the Intel helper (`scripts/install-intel-dkms.sh`) + the `kernel/ramsleuth-intel/` module tree **and** the vendored `ryzen_smu` tree (`packaging/ryzen-smu-dkms/vendor/`) are in the source tree. The **published** `ramsleuth-bin` asset is **v2.4.5** (its tarball predates the AMD vendor bundling); after the v2.4.6 tag, the published asset is **v2.4.6** and its tarball **carries** the Intel helper + the `kernel/ramsleuth-intel/` module tree + the 8 vendored `ryzen_smu` files — per the 27-artifact release contract (27 files: 6 binaries + 21 auxiliary: the 8 pre-Intel auxiliary files, the Intel DKMS helper, the 4 module source files, and the 8 vendored ryzen-smu files; the `assets/icons/` hicolor tree ships as a bundle) — so the `-bin` one-click paths (Intel **and** AMD, the latter offline) are fully active. Both ramsleuth packages **guard** the Intel helper + source-tree install (pre-2.4.2 assets) and the AMD vendor-tree install (pre-vendor assets — a build against the current v2.4.5 tarball skips it cleanly). The standing process for **each** release is:

1. Cut the git tag `v<ver>` — the release workflow (`.github/workflows/release.yml`) publishes the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`.
2. **`ramsleuth`** (stable source) — bump `pkgver` to `<ver>`; it builds from the `v<ver>` tag. Push.
3. **`ramsleuth-bin`** (precompiled) — bump `pkgver` to `<ver>` and re-pin `sha256sums` to the **published** release `.sha256` (a real sha256, no placeholder). Push.
4. **`ramsleuth-intel-dkms`** (Intel extra) — bump its fixed `pkgver` to `<ver>` (it tracks the ramsleuth workspace version; it cannot tag-pin — the module first lands on the development branch, so the package builds from `v2-development`). Push.

## Optional: `ramsleuth_intel` DKMS extra (live Intel subtimings)

**Not a hard dependency.** Without the module, RamSleuth degrades gracefully: the Intel subtiming fields read `N/A (DriverMissing)`, exit 0, no panic — and the built-in `/dev/mem` MCHBAR decode remains the fallback where the kernel allows it. The module is the recommended path: the IMC window (`0xFED10000`) sits in a region modern kernels mark exclusive (`IO_STRICT_DEVMEM`), and UEFI Secure-Boot lockdown disables `/dev/mem` entirely.

### Source: in-repo (no upstream pin, no network)

Unlike the AMD extra (which vendors an *external* upstream, pinned `ryzen_smu`), the Intel module **lives in this repository**: the four-file tree `kernel/ramsleuth-intel/` (`dkms.conf`, `Makefile`, `ramsleuth_intel.c`, `README.md`) is the source of truth. The helper resolves it in this order:

1. **Repo-relative** `kernel/ramsleuth-intel/` — the dev-checkout path (a symlinked invocation canonicalizes into the repo it points into).
2. **Installed copy** `/usr/share/ramsleuth-intel-dkms/src/` — provided by **any** ramsleuth install that bundles it (both main AUR packages, and the `ramsleuth-intel-dkms` extra) (so the build is network-free — and the one-click works from a bare AUR install).

There is no pin, no clone, no vendor-SUMS manifest: there is no upstream to track (no `RYZEN_SMU_*`-class env overrides apply). The DKMS version is the ramsleuth workspace version (a fixed version — `2.4.6` today — which makes `dkms add` idempotent across kernel updates).

Licensing: the module is **GPL-2.0** (SPDX header) — a **separate work** from the MIT RamSleuth code. It is built **only** by DKMS in `/usr/src/ramsleuth_intel-<version>` — never compiled into, linked with, or bundled as part of any RamSleuth binary. The `ramsleuth-intel-dkms` package lists `GPL-2.0-only`; the two `ramsleuth*` packages now **bundle the same module source** (to `/usr/share/ramsleuth-intel-dkms/src/`; their `license=(MIT GPL-2.0-only)` reflects it — a separate work, never compiled into any RamSleuth binary).

### Enabling live subtimings on an Intel host

1. One of:
   - `sudo ramsleuth-setup --with-dkms` — the one-click path (vendor-aware: on Intel silicon it delegates to the installed Intel helper); `sudo ramsleuth-setup --with-intel-dkms` forces the Intel arm (the Intel-only fast path). On a current **v2.4.6** install the ramsleuth-owned Intel helper ships in the source tree (and in the published `ramsleuth-bin` asset since the v2.4.5 re-cut), and **both main packages bundle the module source**, so the one-click works from a bare AUR install with no manual source step — the AUR extra below is the standalone alternative (mutually exclusive with the mains);
   - `yay -S ramsleuth-intel-dkms` (or `paru -S ramsleuth-intel-dkms`) — the provisioning extra: bundles the DKMS config at `/usr/share/ramsleuth-intel-dkms/dkms.conf`, its helper (as `/usr/bin/ramsleuth-intel-dkms-install`), **the module source tree** (at `/usr/share/ramsleuth-intel-dkms/src/` — so the build is network-free), and docs. **Mutually exclusive with the main packages** (shared `/usr/share/ramsleuth-intel-dkms/src/` path — installing it removes a main package first), and since the mains already bundle that source it is the standalone/redundant Intel path. It is thin by design — no module build in the build chroot (it would build against the chroot's kernel, not the target's); the module is built on the **target** via:

      ```sh
      sudo ramsleuth-intel-dkms-install
      ```

   - or, from a source checkout (no package needed): `scripts/install-intel-dkms.sh`.

2. The helper is idempotent (kobject present → nothing to do) and re-execs under `sudo`; it requires the matching kernel headers (build tree `/lib/modules/$(uname -r)/build` — for custom-kernel hosts it lists candidate packages and stops, never guessing), prints the `sha256` fingerprints of the four staged files, and pauses for confirmation before the first system mutation (a non-interactive run logs and continues; `n` skips cleanly, exit 0). It then runs `dkms add/build/install`, `modprobe` (immediate — **no reboot**), and persists `/etc/modules-load.d/ramsleuth_intel.conf` (an Intel success only — on a non-Intel host no at-boot entry is ever written). The staged source stays inspectable at `/usr/src/ramsleuth_intel-<version>`.

**Non-Intel host (safe by design):** the module builds fine, the load rejects the non-Intel host bridge (a clean `-ENODEV` — no kobject), and the helper exits 0 with a clear note; the app keeps working off the `/dev/mem` fallback.

### Day-2

`AUTOINSTALL=yes` in the bundled `dkms.conf` auto-rebuilds the module on kernel updates. Verify:

```sh
ls /sys/kernel/ramsleuth_intel/
```

(the 25 frozen attributes — the 19 IMC-register attributes: `mchbar_base`, `mchbar_enabled`, `mcbios_req`, and the per-channel `tc_*` table, the 5 MAD channel/geometry attributes `mad_inter_channel`, `mad_intra_ch0`/`mad_intra_ch1`, `mad_dimm_ch0`/`mad_dimm_ch1`, and `capid0a` (the raw `CAPID0_A` 32-bit word, host-bridge config offset `0xE4`, `0x%08x`, the `0xffffffff` sentinel on unreadable — the ECC-decode input); `kernel/ramsleuth-intel/README.md` is the reference).

### Secure Boot / lockdown (known limitation)

On UEFI Secure Boot hosts (integrity lockdown), the kernel blocks **both** `/dev/mem` **and** unsigned out-of-tree modules: the only path is to MOK-enroll the module first (`mokutil --import ramsleuth_intel.ko`). This is the same class of limitation the AMD `ryzen_smu` extra carries. When the module cannot be loaded, RamSleuth degrades gracefully: `N/A (DriverMissing)`, exit 0, no panic.

## CI

GitHub Actions runs on push and pull requests to `v2-development`:

- **test** — matrix `1.75` (MSRV) × `stable` on `ubuntu-latest` (`fail-fast: false`): `cargo test --workspace` (debug), `cargo test --workspace --release`, `cargo clippy --workspace --all-targets -- -D warnings`. The committed `Cargo.lock` is used as-is; the MSRV leg proves the 1.75 claim on a clean runner.
- **build** — `cargo build --release --workspace` on stable, uploading the 6 release binaries as an artifact.
- **kernel-module** — independent (a kernel failure never breaks the Rust jobs): builds `kernel/ramsleuth-intel` against the runner's `linux-headers-generic`, asserting a `.ko` is produced with **zero compiler warnings** (the job fails on any warning).

## No-panic contract

Absent driver, privilege, or hardware never crashes the service or the clients: the affected fields read a structured `N/A (<reason>)` (e.g. `N/A (DriverMissing)` for a missing `ryzen_smu` module) and the process exits 0.
