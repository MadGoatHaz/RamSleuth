# RamSleuth — User Guide

**Version:** 2.2.1 · **Platform:** Arch Linux (x86_64) · **License:** MIT · **Repository:** [github.com/MadGoatHaz/RamSleuth](https://github.com/MadGoatHaz/RamSleuth)

RamSleuth v2 is a live RAM telemetry suite and an AIDA64-style memory benchmark for
AMD and Intel desktops. It watches your memory system in real time — clocks, the
27 live DRAM subtimings, voltages, temperatures, bandwidth — and it measures your
memory subsystem with a proper four-by-four benchmark grid (DRAM, L1, L2, L3 ×
Read, Write, Copy, Latency).

Everything privileged happens in one small root daemon; everything you actually
look at is an unprivileged client that talks to that daemon over a local Unix
socket. There is a desktop **GUI**, a terminal **TUI**, a **CLI**, and two
**standalone tools** you can run without the daemon at all.

This guide is self-contained: it covers installation (every method), the one-click
setup, daily use in each frontend, how to read every value, what the structured
`N/A (<reason>)` markers mean and how to fix them, day-2 troubleshooting, and
uninstalling. For the deep technical design (wire protocol, crate map, sandbox
details) see `Docs/Architecture.md`; for the packaging/operator view (AUR tier
policy, version-bump process, CI) see `packaging/README.md`.

---

## Table of contents

1. [What you can do with RamSleuth](#1-what-you-can-do-with-ramsleuth)
2. [Installation](#2-installation)
   - [2.1 One-command install from GitHub (`install.sh`)](#21-one-command-install-from-github-installsh)
   - [2.2 Arch User Repository — the three published packages](#22-arch-user-repository-the-three-published-packages)
   - [2.3 Manual install from a source checkout (`makepkg`)](#23-manual-install-from-a-source-checkout-makepkg)
   - [2.4 Build from source (`cargo`)](#24-build-from-source-cargo)
   - [2.5 Optional extra: `ryzen-smu-dkms` (AMD live subtimings)](#25-optional-extra-ryzen-smu-dkms-amd-live-subtimings)
3. [First run & one-click setup](#3-first-run--one-click-setup)
4. [Running the GUI (`ramsleuth`)](#4-running-the-gui-ramsleuth)
5. [Running the TUI (`ramsleuth-tui`)](#5-running-the-tui-ramsleuth-tui)
6. [Running the CLI (`ramsleuth-client`)](#6-running-the-cli-ramsleuth-client)
7. [Standalone tools](#7-standalone-tools)
8. [Reading the data](#8-reading-the-data)
9. [N/A — what the structured reasons mean](#9-na--what-the-structured-reasons-mean)
10. [AMD live subtimings: the `ryzen_smu` requirement](#10-amd-live-subtimings-the-ryzen_smu-requirement)
11. [Day-2 operations & troubleshooting](#11-day-2-operations--troubleshooting)
12. [Uninstall](#12-uninstall)

---

## 1. What you can do with RamSleuth

RamSleuth answers two questions a lot of RAM owners have, live:

**"What is my memory system doing right now?"** The daemon reads the
memory controller's own state and shows it in a dense dashboard:

- **Clocks** — MCLK (the memory clock), UCLK (the memory-controller clock),
  FCLK (the Infinity Fabric clock on AMD), the UCLK:MCLK sync ratio (1:1
  synchronous or 1:2 asynchronous), the DRAM command rate (1T/2T), and the
  CPU's live core frequency.
- **Live DRAM subtimings** — all 27 of them (tCL, tRCD, tRP, tRAS, tRFC, …)
  straight from the SMU PM table on AMD, decoded per-channel from the memory
  controller registers on Intel. Not the values baked into an XMP profile —
  the values the controller is *actually running right now*.
- **Signal integrity settings** — CAD-bus ODT and driver strengths (in ohms,
  RZQ-relative) and the RTT modes.
- **Voltages** — the VDDCR_CPU (Vcore), VDDCR_SOC, VDDIO_MEM, VDD_MISC, and
  VPP rails, in millivolts.
- **Temperature** — the CPU temperature, read from `k10temp`/`zenpower`
  hwmon (with the `cpu_thermal` thermal zone as fallback).
- **SPD** — everything your DIMMs report about themselves: maker, DRAM die,
  part number, serial, rank, density, base speed, and the factory XMP/EXPO
  profiles stored on each module.

**"How fast is it, really?"** RamSleuth ships a full benchmark engine in the
same spirit as AIDA64's memory benchmark:

- A **4×4 grid** — tier rows (Memory/DRAM, L1, L2, L3) × metric columns
  (Read, Write, Copy, Latency), with bandwidth in GB/s and latency in
  ns/hop.
- **AVX2 and AVX-512F kernels**, one worker pinned per physical core, with
  runtime feature detection (it falls back to AVX2 on CPUs without AVX-512).
- A **burn-in mode** — a repeatable soak (5 minutes by default, up to 24 h,
  or infinite) that re-runs the passes continuously, for stability testing.
- Runs execute **inside the daemon** (single-flight: one run at a time), so
  you can start one from the GUI, the TUI, or the CLI and watch it stream.

You can consume all of this from three frontends plus two standalone tools:

| Tool | What it is | Privilege |
|---|---|---|
| `ramsleuth` (GUI) | The full desktop dashboard: live telemetry matrix, benchmark grid with Run/Cancel/burn-in, hardware & SPD cards, a dedicated graphs window, settings, and the one-click setup. | none — talks to the daemon over the socket |
| `ramsleuth-tui` | The same dashboard in your terminal (ratatui): a 16-key control surface, graphs overlay, snapshot & JSON export. | none — talks to the daemon over the socket |
| `ramsleuth-client` | The CLI: `dump` (the full telemetry listing), `bench` (a streamed benchmark run), `status` (per-section health). | none — talks to the daemon over the socket |
| `ramsleuth-bench` | The standalone benchmark verification CLI — runs the 4×4 grid directly on this machine, no daemon. | none (just allocates memory) |
| `ramsleuth-telemetry` | The standalone telemetry front end — reads the hardware directly and prints the full snapshot. | none (reads what it can without root; privileged fields show `N/A`) |
| `ramsleuth-daemon` | The privileged backend that does the only privileged reads. | root with **`CAP_SYS_RAWIO` only** |

The architecture point that matters for the rest of this guide: **only the
daemon is privileged**, and it is a sandboxed systemd service holding exactly
one capability (`CAP_SYS_RAWIO`, needed for the SMU and `/dev/mem` reads).
Every frontend is unprivileged; it connects to `/run/ramsleuth/ramsleuth.sock`
and the socket's permissions decide who may read. A missing driver, a missing
privilege, or unsupported hardware never crashes anything — the affected
fields simply show a structured `N/A (<reason>)` and every process exits 0.

---

## 2. Installation

RamSleuth is a pure-Rust Cargo workspace (8 crates: 7 product crates + the
`tools/gen-icon` dev tool), MIT-licensed, for x86_64 Linux. It is packaged for
Arch Linux: an AUR three-tier model, a self-contained one-command installer,
and a plain `makepkg`/`cargo` path for building it yourself.

Whichever method you pick, the install lands the same six binaries
(`ramsleuth-daemon`, `ramsleuth-client`, `ramsleuth-tui`, `ramsleuth`,
`ramsleuth-bench`, `ramsleuth-telemetry`) into `/usr/bin`, the frozen daemon
unit `systemd/ramsleuth.service`, the systemd preset, the `ramsleuth` system
group, the app-menu entry, the hicolor icons, the one-click setup helper
(`ramsleuth-setup` + its polkit policy), and the pinned AMD-driver helper
(`ramsleuth-install-ryzen-smu-dkms`). The `ramsleuth-protocol` crate is
library-only and is never installed. The daemon is enabled and started
automatically at the end of every install path.

Choose your method below. If you are on Arch and just want it to work,
**`yay -S ramsleuth`** (or the precompiled `ramsleuth-bin`) is the default
recommendation; **`./install.sh`** is the transparency-forward GitHub path;
`makepkg`/`cargo` are for building and inspecting it yourself.

### 2.1 One-command install from GitHub (`install.sh`)

```sh
git clone https://github.com/MadGoatHaz/RamSleuth && cd RamSleuth && ./install.sh
```

That is the whole install. `install.sh` lives at the root of the repository and
is **self-contained** — it does not reach for any other AUR package, and it
builds *only this repository* with `cargo build --release --workspace --locked`
(locked to the committed `Cargo.lock`, from the exact commit you cloned).

What makes it worth using:

- **Interactive and transparent.** Before anything touches the system it
  prints your host facts (distro, kernel, CPU vendor), the **exact commit and
  branch** it will install, a full inventory of every artifact and its
  destination under `/usr`, and an explicit **third-party source statement**:
  the only third-party code anywhere in the flow is the *optional* AMD
  `ryzen_smu` module, pinned to `amkillam/ryzen_smu @
  d2983668300dd2a598e5a7dc40e71ce0678cc270` (shown, checksummed, and confirmed
  before any build — and never built unless you opt in). It then asks
  `Proceed? [Y/n]`; answering `n` aborts with exit 0 and **no changes**.
- **AUR-equivalent.** After you authorize the `sudo` prompt it installs the
  same file set as the AUR packages: the 6 binaries, the frozen daemon unit
  (verbatim copy of `systemd/ramsleuth.service`), the systemd preset, the
  desktop entry, the icon tree, the `ramsleuth` system group
  (`groupadd -r`), the DKMS helper, the one-click setup helper + polkit
  policy, and `install.sh` itself (kept at `/usr/share/ramsleuth/install.sh`
  for audit and re-runs). It finishes with `systemctl daemon-reload` and
  `systemctl enable --now ramsleuth` so the daemon is running when it exits.
- **Idempotent.** Every step is a guarded no-op if already present, so
  **re-running it is always safe** — it is also your clean upgrade path from
  a source checkout (`git pull && ./install.sh`).
- **Group seeding up front.** On the `sudo` path it asks to add *you*
  (`$SUDO_USER`) to the `ramsleuth` group and seeds your username into
  `/etc/ramsleuth/authorized-users` — the current-session socket ACL state
  file — so the daemon grants you immediate access (see
  [Section 3](#3-first-run--one-click-setup)).
- **AMD host?** After the install it asks one more question: *Install the
  `ryzen_smu` DKMS module now? [y/N]* — the optional kernel module that
  unlocks live AMD subtimings. The default is `N`; on `y` it hands off to the
  pinned helper. On Intel hosts it prints a note that the built-in MCHBAR
  decode needs no extra driver. You can always run
  `sudo ramsleuth-install-ryzen-smu-dkms` later (or install the
  [ryzen-smu-dkms extra](#25-optional-extra-ryzen-smu-dkms-amd-live-subtimings)).

Prerequisites and exit codes:

- The script must run from a genuine git checkout of RamSleuth (it refuses
  otherwise — exit 2), on an Arch-based system (it checks for `pacman` —
  exit 2 on anything else), and with a Rust toolchain present (`cargo` +
  `rustc`). It **never auto-installs Rust**; if it is missing it prints the
  rustup one-liner and exits 2:
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- Exit codes: **0** = installed (or you declined — no change), **1** = hard
  failure (a clear message says why — never a silent half-state), **2** =
  usage/preflight refusal.

After a successful run, the summary block tells you the next steps: start the
GUI with `ramsleuth` (one click on *Set up RamSleuth*), use
`sudo ramsleuth-setup` as the terminal equivalent, and for day-2,
`systemctl status ramsleuth` and `journalctl -u ramsleuth -f`.

### 2.2 Arch User Repository — the three published packages

Arch users can install from the AUR with any assistant (`yay` shown; `paru`
works identically):

```sh
yay -S ramsleuth       # STABLE — the default recommendation
yay -S ramsleuth-bin   # PRECOMPILED — the fastest install
yay -S ramsleuth-git   # BLEEDING-EDGE — development/testing only
```

The three tiers are deliberately distinct:

- **`ramsleuth` — STABLE source.** Builds the workspace from the official
  git tag `v$pkgver` (currently `v2.2.1`). A tag is a reproducible,
  auditable snapshot — this is the **default recommendation for production
  installs**. It compiles only the pinned repository (`--locked`), with no
  third-party code in the build.
- **`ramsleuth-bin` — PRECOMPILED.** Downloads the release binary tarball
  `ramsleuth-2.2.1-x86_64.tar.zst` from the official GitHub Release (pinned
  by its `sha256sums`) and installs it as-is — **no build, no makedepends**.
  This is the **fastest install path**.
- **`ramsleuth-git` — BLEEDING-EDGE.** Tracks the *moving* `v2-development`
  branch (`pkgver()` recomputes from `git describe` on every fetch). For
  testing unreleased work only — not a stable install, and its version number
  moves.

Two of these three install the **identical file set**, so `ramsleuth` and
`ramsleuth-bin` declare each other in `conflicts=` — **pick exactly one**;
pacman will refuse to install both. `ramsleuth-git` is a separate package and
coexists fine (it is the branch-tracking variant).

Package pages:

- <https://aur.archlinux.org/packages/ramsleuth>
- <https://aur.archlinux.org/packages/ramsleuth-bin>
- <https://aur.archlinux.org/packages/ramsleuth-git>

All three packages create the `ramsleuth` system group (via their `.install`
hooks), install the daemon unit + preset, the one-click setup helper + polkit
policy, and the pinned DKMS helper, then `systemctl enable --now ramsleuth`.
When the install is invoked under `sudo`, the hooks additionally grant the
invoking user group membership (persistent) and seed
`/etc/ramsleuth/authorized-users` (immediate current-session socket access),
then restart the daemon so the ACL applies — the same result as the GUI
one-click setup, done at install time.

The optional AMD driver extra lives in the AUR as its own package — see
[2.5](#25-optional-extra-ryzen-smu-dkms-amd-live-subtimings).

### 2.3 Manual install from a source checkout (`makepkg`)

If you already have a checkout (e.g. from the [install.sh
path](#21-one-command-install-from-github-installsh) or your own clone), you
can build and install any of the three packages directly from the `packaging/`
directories:

```sh
cd packaging/ramsleuth      # STABLE source (builds from the v2.2.1 tag)
# cd packaging/ramsleuth-bin   # PRECOMPILED (downloads the release tarball)
# cd packaging/ramsleuth-git   # BLEEDING-EDGE (builds from v2-development)
makepkg -si
```

`makepkg -si` builds the package and installs it, running the `.install`
hooks (group creation, daemon enable/start) exactly like the AUR path. The
source packages (`ramsleuth`, `ramsleuth-git`) need build dependencies:
`rust`, `cargo`, `pkgconf`, and the X11/Wayland/GL library set
(`libx11`, `libxkbcommon`, `wayland`, `wayland-protocols`, `libxrandr`,
`libxi`, `libxcursor`, `libxinerama`, `mesa`) — the usual suspects for the
GUI; `libxkbcommon` is the only strict link-time dependency.
`ramsleuth-bin` compiles nothing.

### 2.4 Build from source (`cargo`)

RamSleuth builds with a plain Cargo workspace. Requirements:

- **Rust MSRV 1.75** (workspace `edition 2021`, `resolver 2`);
- for the **GUI only**, the X11/Wayland/GL system libraries — on Arch:
  `libx11 libxkbcommon wayland wayland-protocols libxrandr libxi
  libxcursor libxinerama mesa` plus `pkgconf` (Debian/Ubuntu equivalents:
  `libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev
  libxcursor-dev libxinerama-dev libgl1-mesa-dev pkgconf`). The TUI, CLI,
  daemon, and standalone tools need no extra system libraries at all.

From the repository root:

```sh
cargo build --workspace --release        # AUR packages use --locked (the committed pins)
```

The six installable binaries land in `target/release/`:

```
target/release/ramsleuth-daemon    target/release/ramsleuth
target/release/ramsleuth-client    target/release/ramsleuth-bench
target/release/ramsleuth-tui       target/release/ramsleuth-telemetry
```

Run them straight from there if you like (`./target/release/ramsleuth`, …).
Note there is **no crates.io / `cargo install` path** — RamSleuth is not
published to crates.io; it ships via the AUR packages, the GitHub Release
binary tarball, and `install.sh`. (The dev tool `ramsleuth-gen-icon` also
builds in the workspace; it is a maintenance utility and never installed.)

For the unprivileged dev workflow — no daemon install at all — see the
`--socket` flags in [Section 6](#6-running-the-cli-ramsleuth-client):
`cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock` gives you a
daemon on a throwaway socket, and every client accepts `--socket` to point at
it.

### 2.5 Optional extra: `ryzen-smu-dkms` (AMD live subtimings)

**AMD hosts only, and optional.** The live AMD DRAM subtimings come from the
`ryzen_smu` kernel module. Without it, RamSleuth runs perfectly — the AMD
section simply reads `N/A (DriverMissing)` and everything else works
(see [Section 10](#10-amd-live-subtimings-the-ryzen_smu-requirement)).

Install the provisioning extra from the AUR:

```sh
yay -S ryzen-smu-dkms
sudo ryzen-smu-dkms-install
```

The package is deliberately thin: it ships the DKMS configuration, the
install helper (at `/usr/bin/ryzen-smu-dkms-install`), and a **byte-frozen,
offline-vendored copy of the pinned module source**
(`amkillam/ryzen_smu @ d2983668300dd2a598e5a7dc40e71ce0678cc270`, verified
2026-08-15) — the build never needs the network. No module build happens in
the AUR chroot (that would build against the wrong kernel); the actual
`dkms add/build/install` + `modprobe` runs on **your** machine via the
helper, which verifies the vendored files against `SUMS.sha256` and pauses
for your confirmation before the first system mutation. `AUTOINSTALL=yes` in
the bundled `dkms.conf` rebuilds the module automatically on kernel updates,
and the helper persists `/etc/modules-load.d/ryzen_smu.conf` so it loads at
boot.

You do not need this package to get the driver: every ramsleuth install also
ships the same helper under its own name at
`/usr/bin/ramsleuth-install-ryzen-smu-dkms`, and the one-click setup can run
it for you (`sudo ramsleuth-setup --with-dkms`, or the GUI's
*"Set up RamSleuth + AMD driver"* button). The AUR extra and those built-in
paths are co-install-safe (the two names never conflict); use whichever one
your install provided.

---

## 3. First run & one-click setup

Start the GUI:

```sh
ramsleuth
```

On a fresh install the daemon is already running, but *your* user account may
not yet be authorized on its socket. The app knows exactly that, and it
handles it in one action.

**The SETUP strip.** If anything is missing — the daemon not connected, your
account not authorized, or (AMD) the `ryzen_smu` driver absent — the GUI
shows a cyan-bordered **`SETUP — get the most out of RamSleuth`** strip
between the header and the dashboard. It lists each problem with the exact
fix command and a **Copy** button, and its primary button is
**`Set up RamSleuth`** — labelled **`Set up RamSleuth + AMD driver`** when
the missing piece is the AMD module (in which case the one click also builds
and loads the driver).

**What one click does.** The button triggers a single **polkit password
prompt** (the installed `90-ramsleuth-setup.policy`, `auth_admin`) and then
runs, detached and off the render thread:

```sh
pkexec /usr/bin/ramsleuth-setup --user <you>        # … + --with-dkms on the AMD variant
```

That helper performs every privileged step in one root session, all
idempotent:

1. `systemctl daemon-reload` + `systemctl enable --now ramsleuth.service` —
   the daemon is running (the one hard step).
2. `usermod -aG ramsleuth <you>` — group membership, **persisted for future
   logins** (skipped if you are already a member).
3. Appends `<you>` to `/etc/ramsleuth/authorized-users` — the state file the
   daemon re-applies as a per-user socket ACL on **every** socket creation
   (the socket lives on tmpfs `/run`, so that is every daemon (re)start and
   every boot).
4. Best-effort `setfacl -m u:<you>:rw` on the **live** socket — so **your
   current session gets immediate access with no re-login and no reboot**.
5. With `--with-dkms` (the AMD variant): delegates to the installed DKMS
   helper — the offline vendored build + `modprobe`, immediate.

The strip's status line then reads `done — restart RamSleuth to activate`,
and a **"Setup complete"** modal appears with two choices:

- **`Restart now`** — spawns a fresh detached instance of the app and exits
  the current one, so the new window starts with your membership active;
- **`Later`** — dismisses it; the current window keeps running as-is (the
  ACL already gives it socket access, so nothing is lost — the restart only
  picks up the persisted group state cleanly).

If the helper fails, no modal appears — the strip shows
`failed: <diagnostic>` and the manual `sudo` pointer, and the per-row Copy
buttons remain as the polkit-less fallback. The strip also has a
**`Got it — keep using RamSleuth`** button to hide it; it reappears on its
own whenever a requirement exists and can be reopened any time with the
header's **Setup** button.

**The group + ACL model, in plain terms.** The daemon's socket
(`/run/ramsleuth/ramsleuth.sock`) is created mode `0660`, owned by
`root:ramsleuth` — so by default *only* root and members of the `ramsleuth`
group may connect, and the GUI/TUI/CLI need **no privileges of their own**.
There are two complementary access mechanisms, and after setup you have
both:

1. **Current session — immediate.** A POSIX ACL (`u:<you>:rw`) granted
   straight onto the live socket. This is what makes "no re-login, no
   reboot" true: the moment setup finishes, your already-running session can
   read telemetry. The daemon re-applies these ACLs from
   `/etc/ramsleuth/authorized-users` on every socket creation, so the access
   survives daemon restarts and reboots.
2. **Future logins — persistent.** Group membership via `usermod -aG`.
   PAM applies groups at login time, so this covers your next and later
   sessions even without the ACL machinery.

The same setup from a terminal (headless boxes, or if you prefer) is just:

```sh
sudo ramsleuth-setup               # daemon + group + current-session access
sudo ramsleuth-setup --with-dkms   # + build & load the AMD driver (AMD hosts)
```

(Run bare, it re-execs under `sudo` itself.) Once setup is done, launch
`ramsleuth` (or `ramsleuth-tui`) and the dashboard fills in on its own.

---

## 4. Running the GUI (`ramsleuth`)

The GUI is the `ramsleuth` binary (the `ramsleuth-gui` crate): a 968×600
egui/eframe window in the dark-slate theme, repainting at ~60 FPS. It needs a
display; the only command-line option is the daemon socket:

```sh
ramsleuth                          # connects to /run/ramsleuth/ramsleuth.sock
ramsleuth --socket /tmp/dev.sock   # a custom socket (dev workflow)
```

Exit codes: `0` a clean quit, `1` an eframe/display failure, `2` a usage
error (unknown flag). A missing daemon never crashes the app — the header
shows `Disconnected` with a hint, and the dashboard stays responsive.

### 4.1 The 3-line header

**Line 1** — the title **`RamSleuth v2.2.1`**, a **platform tag**
(`[AMD AM4 Platform]` for Zen 1–3, `[AMD AM5 Platform]` for Zen 4/5,
`[Intel LGA Platform]`, or a bare `[Platform]` when the vendor is unknown),
the **daemon status** (`Daemon: Connected (IPC: /run/ramsleuth/ramsleuth.sock)`
in cyan, or `Disconnected` in crimson — it names the socket actually in use),
then, right-aligned, the **Setup**, **Settings**, and **Graphs** toggle
buttons and the key legend **`[F2] snapshot · [F3] export · [Q] quit`**.

**Line 2** — CPU and platform identity:
`CPU: <brand> @ <core clock> | Motherboard: <board> (BIOS: <version>, <AGE token>)`,
where the AGE token is labelled honestly by source — `AGESA <v>` for a true
AGESA string found in the BIOS data, `SMU <v>` for the `ryzen_smu` firmware
version, or `AGESA N/A` when neither is available.

**Line 3** — the RAM summary: `RAM: <total> (<per-DIMM breakdown>) <max
module speed> MT/s | <channel mode> | Mode: <sync state>` — for example
`RAM: 64 GiB (4x16 GiB Dual-Rank) 6000 MT/s | Quad-Channel | Mode:
Synchronous 1:1 (UCLK = MCLK = 3000 MHz)`. The UCLK:MCLK sync segment is
colour-coded: **amber** for `Synchronous 1:1` (UCLK = MCLK — the healthy
memory-clock configuration), **crimson** for `Asynchronous 1:2` (UCLK = MCLK/2
— the fallback when the fabric cannot keep up), and plain text for an honest
`N/A`. When the OS total exceeds what SPD can see (e.g. four DIMMs installed
but only two bound to the SPD bus), a slot note is appended —
`2 of 4 slots SPD-visible`.

### 4.2 The three zones

The central panel splits into three zones; all values update at the
configured poll interval (default 2 s).

**Zone 1 — `1 · MEMORY CONTROLLER & SUBTIMINGS`** (left). The live telemetry
matrix: every cell of the AMD readout (or the Intel per-channel readout, when
that is the platform) as a `key: value` row — the clocks and ratios
(MCLK/UCLK/FCLK, UCLK:MCLK divide mode, GDM/PDM flags, command rate), the
27 DRAM subtimings in ticks, the CAD-bus ODT/driver strengths in ohms, and
the voltage rails. Missing cells render `N/A` in muted grey — never a crash,
never a blank mystery (see [Section 9](#9-na--what-the-structured-reasons-mean)).

**Zone 2 — `2 · BENCHMARK ENGINE`** (right, top). The AIDA64-style **4×4
grid**: tier rows **Memory / L1 / L2 / L3** × metric columns **Read / Write /
Copy / Latency**, bandwidth cells in GB/s (cyan) and latency cells in ns
(amber). The grid is **live during a run** — each streamed progress event
updates its cell with a dimmed `…` suffix — and shows the terminal result of
the last completed run at rest. The controls:

- **`Run Full`** — the complete run: all 12 bandwidth cells plus the four
  latency passes;
- **`Memory Only`** — just the DRAM tier (bandwidth + latency);
- a **burn-in minutes knob** (default 5, `0` = infinite, up to 24 h) with a
  **`Run Burn-In`** button — the soak mode, re-running the passes
  continuously and updating the grid per iteration;
- **`Cancel`** — shown only while a run is in flight (the daemon is
  single-flight: a second start gets a structured "already running" error,
  and all run buttons disable while one runs).

Below the grid, a flat status line reads `Status: Idle` at rest,
`Status: Running… <m:ss>` (or `… (burn-in <m:ss>, iter <n>)` for a soak)
while in flight, and `Status: Done` when finished.

**Zone 3 — `3 · HARDWARE & SPD`** (right, bottom). One **card per bound SPD
slot** — headed `slot 0xNN (DDR4|DDR5)` — carrying the product line
(`<maker> (<part>)`), the DRAM die line (`<die maker> (<die type>, <density
Gb>)`), the human rank label (`Single-Rank` / `Dual-Rank` / `<n>-Rank`), and
the raw maker / part / rank / density / speed cells, plus **one row per XMP /
EXPO profile** in the form `<speed> MT/s <CL>-<tRCD>-<tRP>-<tRAS> @ <volts>`
(or a `profiles: none` placeholder). Below the cards: the **daemon status
line** (`daemon: <status> · <N.N>s ago`, cyan when healthy, crimson when
disconnected or errored, with the structured error line underneath when
present) and the **ACTIONS** row — `F2 · Snapshot PNG`, `F3 · Export JSON`,
`Q · Quit` — clickable, and identical in behaviour to the keys.

### 4.3 The Graphs window

The header's **Graphs** button opens (and closes) a **dedicated second OS
window** — `RAM & SYSTEM GRAPHS` — with five time-series:

- **CPU FREQ (MHz)** — the live core frequency;
- **VDDCR_CPU (mV)** — the Vcore rail;
- **VDDCR_SOC (mV)** — the AMD SOC rail;
- **CPU TEMP (°C)** — from `k10temp`/`zenpower` hwmon (the `cpu_thermal`
  thermal zone as fallback);
- **MEM BANDWIDTH (GB/s)** — a step series riding the latest bench/burn-in
  `Memory · Read` figure (it appears after your first benchmark run).

A sample is recorded on every successful poll into a 1800-deep ring (60
minutes at the default 2 s cadence). The window's controls: four **window
buttons** — `1 / 5 / 15 / 60 min` (default 5) — and a **Poll** combo
(0.5 s / 1 s / 2 s / 5 s / 10 s, the same shared knob as the settings
panel). Drag horizontally on any row to **pan** into the past; hover for a
full-height crosshair and a tooltip with every series' exact value at that
timestamp. A series with no data in the window shows an honest `N/A` note
(never a fake flat zero line) that clears itself when data arrives.

### 4.4 The Settings panel

The header's **Settings** button opens a strip below the header with the
in-memory knobs (applied live — a changed value takes effect on the next
poll cycle, no restart; note they are not persisted to disk across restarts):

- **Socket** — the daemon Unix socket (seeded from `--socket`; edit it to
  retarget the poller live, e.g. at a dev daemon);
- **Poll interval** — a drag value in ms, clamped to 100 ms – 60 s (default
  2 s);
- **Capacity units** — `GiB (binary, 1024³)` or `GB (decimal, 1000³)`;
- **Clock units** — `MHz` or `GHz`;
- **Theme** — `Dark Slate` (the current and only theme);
- **Refresh** — the auto-refresh gate. **Off by default in the GUI**: the
  app performs one baseline fetch on connect and then freezes the view (the
  `…s ago` stamp advances); tick it to poll continuously at the interval
  above. (The TUI inverts this default — its refresh is on by default; see
  Section 5.)

### 4.5 Keys & exports

- **`F2` — Snapshot PNG.** Renders the current benchmark grid (with the
  CPU/RAM header lines over it) to **`$HOME/ramsleuth-snapshot-<unix-ts>.png`**;
  a transient header notice shows the written path (and `! …` on failure).
  Nothing to export (no grid yet) → an amber "nothing to export yet" notice.
- **`F3` — Export JSON.** Writes **`$HOME/ramsleuth-export-<unix-ts>.json`**
  carrying the current telemetry snapshot plus the terminal benchmark grid
  (`null` before the first completed run).
- **`Q` — Quit.** Clean exit (the OS close button does the same); the
  background poller is stopped and joined before the process ends.

Keys and buttons are behaviourally identical; a held key fires exactly once
(key-repeat is filtered).

### 4.6 Day-2 commands

```sh
systemctl status ramsleuth     # is the daemon alive?
journalctl -u ramsleuth        # its log (-f to follow)
```

The unit is sandboxed (`CAP_SYS_RAWIO` only, `ProtectSystem=strict`,
`ProtectHome`, `NoNewPrivileges`, `Restart=on-failure`) — see
`packaging/README.md` for the full unit listing.

---

## 5. Running the TUI (`ramsleuth-tui`)

The TUI is the terminal twin of the GUI: the same three-zone dashboard drawn
with ratatui, driven by a **16-key control surface**. It is unprivileged and
talks to the daemon over the socket:

```sh
ramsleuth-tui                          # /run/ramsleuth/ramsleuth.sock
ramsleuth-tui --socket /tmp/dev.sock   # a custom socket (dev workflow)
```

On entry it switches to raw mode + the alternate screen; a drop-safe guard
**restores the terminal (disables raw mode, leaves the alternate screen,
shows the cursor) on every exit path** — a normal `q`, an early return, or an
unwind — so your shell is never left in raw mode. Exit codes: `0` a normal
quit, `1` terminal init failure, `2` a usage error.

### 5.1 The 16-key table

| Key | Action | What it does |
|---|---|---|
| `r` | **Refresh** | Force a telemetry refresh *now* (works even with auto-refresh off). |
| `s` | **Snapshot** | Write a timestamped dashboard snapshot **`ramsleuth-tui-<unix-ts>.txt`** to the **current working directory**. |
| `q` | **Quit** | Restore the terminal and exit `0`. |
| `b` | **Bench (full)** | Start a full benchmark run (all 12 bandwidth cells + the 4 latency passes). |
| `m` | **Memory** | Start a memory-only benchmark run (the DRAM tier). |
| `x` | **Burn-in** | Start a **5-minute burn-in soak** (the GUI default minutes). |
| `c` | **Cancel** | Cancel the in-flight bench/burn-in run. |
| `g` | **Graphs** | Toggle the 5-series graphs overlay panel. |
| `t` | **Settings** | Toggle the settings strip. |
| `d` | **Requirements** | Toggle the setup requirements strip (the GUI SETUP mirror — display-only: it shows the exact fix commands, e.g. `sudo ramsleuth-install-ryzen-smu-dkms`). |
| `e` | **Export** | Write a `{ telemetry, bench }` JSON export **`ramsleuth-export-<unix-ts>.json`** to **`$HOME`** (the CWD when `$HOME` is unset). |
| `p` | **Poll** | Cycle the poll interval: **100 ms → 500 ms → 1 s → 2 s → 5 s → 10 s → 30 s → 60 s** (wraps; default 2 s). |
| `u` | **Capacity** | Toggle the capacity units: **GiB ↔ GB**. |
| `k` | **Clock** | Toggle the clock units: **MHz ↔ GHz**. |
| `a` | **Refresh (auto)** | Toggle the periodic data poll **on ↔ off** (the TUI default is **on** — the continuous live poll; off freezes the view, but `r` and reconnect baselines still fetch). |
| `w` | **Window** | Cycle the graphs window: **1 → 5 → 15 → 60 min** (default 5). |

Keys are case-insensitive; modifier keys are ignored (a `Ctrl`-prefixed
action char still maps). Every other key — `Esc`, `Enter`, arrows, function
keys, mouse — is ignored (window resize needs no key: the surface is
re-read every frame).

### 5.2 The layout

The screen is a **3-line header** (title + platform tag + daemon status +
key legend; the CPU/platform identity line; the RAM summary line with the
channel mode and colour-coded UCLK:MCLK sync state — the GUI header's
mirror), then, while open, the one-line **settings strip**
(`Settings: <poll> · <capacity> · <clock> · <refresh on/off> · <socket>`) and
the **requirements strip** (the amber `SETUP — requirements` block: one row
per diagnosed problem with its exact fix command; it auto-opens while a
requirement exists and vanishes on its own once resolved), then the three
zones side by side (40 / 32 / 28 percent of the width):

- **Zone 1 — `1 · MEMORY CONTROLLER`** — every cell of the AMD (and Intel,
  if present) readout as `key: value` or `key: N/A (<reason>)` — the clocks
  and ratios, the GDM/CR rows, the 27 subtimings, the CAD-bus ohms, and the
  voltages (Vcore first). A whole-section degradation renders as a single
  `N/A (<reason>)` line.
- **Zone 2 — `2 · BENCH (GB/s)`** — the 4×4 grid (header + Memory/L1/L2/L3
  rows × Read/Write/Copy/Latency columns), **live during a run** (measured
  cells dimmed with a `…`, unstarted cells `N/A`), then the last completed
  run's terminal grid; below it the flat status line (`Status: Idle` /
  `Status: Running… <m:ss>` / `Status: Running… (burn-in <m:ss>, iter <n>)` /
  `Status: Done`), the controls line (`[B] Full  [M] Mem  [X] Burn-in(5m)` —
  dimmed while any run is in flight — plus `[C] Cancel` shown while a run is
  in flight), and a live burn-in row
  (`Burn-in: iteration <n> · <m:ss> elapsed · Memory Read <…> · <latency>`).
- **Zone 3 — `3 · HARDWARE & SPD`** — one block per SPD slot (maker / DRAM
  die / part / rank / density / speed + XMP/EXPO profiles), the daemon
  status line with its `…s ago` stamp, and the crimson error line when
  present.

The **graphs overlay** (`g`) is drawn topmost over the zone area —
`GRAPHS (last <N> min)` — with the same five series as the GUI Graphs window
(CPU FREQ, VDDCR_CPU, VDDCR_SOC, CPU TEMP, MEM BANDWIDTH) as sparkline rows
over the shared 1800-sample ring (60 min at the 2 s cadence); `w` changes
the visible window.

The daemon-down state is fully graceful: the header shows `down` /
`disconnected`, zone 3 carries the daemon-start hint, and the requirements
strip tells you exactly which command to run — the loop keeps rendering
throughout.

---

## 6. Running the CLI (`ramsleuth-client`)

The CLI is the unprivileged command-line client — the same daemon socket,
three subcommands. If you omit the subcommand, `dump` runs.

```text
Usage: ramsleuth-client [SUBCOMMAND] [OPTIONS]

Subcommands:
  dump     Print the dashboard-style telemetry listing (default)
  bench    Run a streamed benchmark (progress lines + the 4x4 grid)
  status   Print the per-section health summary

Options:
  --socket <path>                  Daemon Unix socket
                                   (default: /run/ramsleuth/ramsleuth.sock)
  --tier <memory|l1|l2|l3|full>    Benchmark tier scope (default: full)
  --mode <full|memory-only>        Benchmark scope (default: full)

Exit codes: 0 success, 1 daemon/client error, 2 usage error
```

- **`dump`** — the Phase-3 exit-criterion command: one `GetTelemetry` round
  trip, rendered as a dashboard-style listing with four sections —
  `=== CPU ===` (vendor + brand), `=== AMD ===` (clocks & ratios, the 27
  primary/secondary/tertiary + turnaround subtimings in ticks, the CAD bus
  in ohms, the voltages), `=== Intel ===` (the per-channel IMC decode, plus
  RTL), and `=== SPD ===` (per-slot modules + XMP/EXPO profiles). Every cell
  prints either its value or a structured `N/A (<reason>)`, so a fully
  degraded host still gets a complete, honest dashboard.
- **`bench`** — a **streamed** benchmark run against the daemon: first
  `benchmark started (run <id>)`, then one live line per completed bandwidth
  cell — `  [1/12] Memory/Read: 512.00 GB/s` and so on — and finally the
  terminal **4×4 grid** (tier rows × Read/Write/Copy/Latency columns,
  bandwidth in GB/s, latency in ns/hop, unmeasured cells `N/A`). `--tier`
  scopes which tier rows run (`memory` / `l1` / `l2` / `l3` / `full`,
  default `full`); `--mode` scopes the run (`full` = all 12 bandwidth cells
  plus the four latency passes, or `memory-only`; default `full`). A run in
  flight elsewhere (single-flight daemon) answers with a structured
  "benchmark already running" error.
- **`status`** — the one-line-per-section health summary: `CPU: ok` (the
  snapshot root always carries the detected vendor + brand), `AMD: ok` /
  `AMD: N/A (<reason>)`, `Intel: ok` / `Intel: N/A (<reason>)`,
  `SPD: ok (n modules)` / `SPD: N/A (no modules)`, plus the daemon socket
  line. This is the fastest way to see *what* is degraded.

**Exit codes:** `0` success; `1` a daemon/client error (connect, timeout,
protocol, or I/O — each with its structured diagnostic); `2` a usage error
(unknown subcommand/flag, a missing flag value, a bad enum value — the usage
text is printed).

**Daemon down?** The connect is retried (3 attempts with short backoff, so a
daemon mid-startup gets a chance), then you get a friendly, actionable
diagnostic rather than a stack trace:

```
ramsleuth-client: cannot connect to /run/ramsleuth/ramsleuth.sock: daemon not running? start it with `cargo run -p ramsleuth-daemon` or `systemctl start ramsleuth` (last error: Connection refused (os error 111))
```

Start the service (`systemctl start ramsleuth` — or the one-click setup,
Section 3) and re-run.

---

## 7. Standalone tools

Two of the six binaries run **without the daemon at all** — useful for
verifying a machine in isolation (another box, a VM, a live USB session).

### 7.1 `ramsleuth-bench` — the standalone benchmark

```sh
ramsleuth-bench
ramsleuth-bench --avx512        # force the AVX-512F kernel path
ramsleuth-bench --json          # also print the grid as a JSON object
ramsleuth-bench -h              # help
```

It is a thin verification harness around the benchmark engine library: it
**detects the host's CPU features (AVX2 / AVX-512F) and topology** (physical
cores, logical CPUs, SMT, total L3), prints a summary line —

```
ramsleuth-bench: 16 physical cores (32 logical, SMT on), total L3 64 MiB, AVX2 yes, AVX-512 no
```

— runs the full orchestrator (`run_all`), and prints the AIDA64-style grid
as a fixed-width table:

```
Tier            Read (GB/s)  Write (GB/s)  Copy (GB/s)  Latency (ns)
Memory (DRAM)       512.00       410.00       455.00        88.0
L3                   198.50       152.00       176.50        12.7
L2                   402.00       311.50       349.00         3.4
L1                   897.50       823.00       851.50         1.1
```

(`--json` appends the same grid as
`{"read_gbps":[…],"write_gbps":[…],"copy_gbps":[…],"latency_ns":[…]}`,
values in tier order Memory, L1, L2, L3.) `--avx512` forces the AVX-512F
kernel path; on a CPU without AVX-512F it prints a note and **falls back to
AVX2** — never a failure. Exit codes: `0` success, `1` a topology/benchmark
failure, `2` an unknown flag.

### 7.2 `ramsleuth-telemetry` — the standalone telemetry front end

```sh
ramsleuth-telemetry
ramsleuth-telemetry --json
ramsleuth-telemetry -h
```

A thin front end for the telemetry collector: it **reads the hardware
directly — no daemon, no socket** — and prints the full
`SystemMemoryTelemetry` snapshot either as a dashboard listing

```
CPU: Amd(Zen5) — AMD Ryzen 9 7950X 16-Core
AMD: <full readout, or N/A (<reason>)>
Intel: <per-channel readout, or N/A (<reason>)>
SPD[0]: <module: maker, die, part, rank, density, speed, profiles>
```

or as a JSON block (`{"cpu":{"vendor","brand"},"amd":…,"intel":…,"spd":[…]}`,
with `null` for any unavailable section). Two properties make it a good
first diagnostic on an unfamiliar box:

- **It exits `0` even when everything is N/A** — N/A is a valid, structured
  outcome (the no-panic contract), not a failure;
- **The only non-zero exit is `2`** (an unknown flag).

Read directly, it sees only what is visible unprivileged: the SPD EEPROM
(`ee1004`), CPUID, DMI, and `/proc` work fine; the privileged SMU/`/dev/mem`
branches degrade to their `N/A` reasons — which is exactly the information
you need to know what to install (Section 10). For the full privileged
readout, run the daemon and use `ramsleuth-client dump` instead.

---

## 8. Reading the data

Every number in the dashboard has a precise meaning. Here is the plain-
language tour.

### 8.1 The clocks

- **MCLK** — the *memory clock*: the rate the DRAM interface runs at, in MHz.
  For DDR5, the data rate (MT/s) is 2× MCLK (DDR5 "6000 MT/s" memory runs at
  MCLK = 3000 MHz).
- **UCLK** — the *memory-controller (uncore) clock*: the internal clock the
  CPU's memory controller uses. Its relationship to MCLK is the sync mode
  below.
- **FCLK** — the *Infinity Fabric clock* (AMD): the on-die interconnect
  between CPU complexes, the cache, and I/O. The classic AMD tuning target
  is FCLK = MCLK (fabric running at the memory clock).
- **UCLK:MCLK sync** — the dashboard's colour-coded segment.
  **`1:1` (Synchronous, amber)** means UCLK = MCLK — the memory controller
  keeps pace with the DRAM at full rate; this is the healthy configuration
  for high-MT/s memory. **`1:2` (Asynchronous, crimson)** means UCLK =
  MCLK/2 — the controller falls back to half the memory rate, which caps
  achievable bandwidth; see it and the memory is not running as fast as the
  DIMMs would like.
- **Command rate (CR)** — `1T` (one-transaction) / `2T` (two-transaction)
  DRAM bus encoding: how the controller spaces commands on the bus; `2T` is
  the more relaxed, more stable mode at high speeds.
- **GDM / PDM** — *Gear Down Mode* / *Power Down Mode* flags from the SMU.
  GDM active means the controller dropped to a lower gear (lower effective
  performance, usually under power/thermal management); PDM active means
  the memory is in its power-down state.
- **Gear mode** — the SA:MEM multiplier; an Intel-side concept (1×/2×/4×) —
  `N/A` on AMD, whose PM tables do not report it.
- **CPU clock** — the live core frequency, from `/proc` (and the source of
  the Graphs window's CPU FREQ series).

### 8.2 The 27 DRAM subtimings

The live timings are the controller's *actual* timing state, in **ticks**
(clock cycles of the DRAM clock domain — a tick at 3000 MHz is ~0.33 ns).
AMD: all 27, from the SMU PM table. Intel: the subset its MCHBAR registers
expose (tCL/tRCD/tRP/tRAS, tCCD_S/L, the RDRD/RDWR/WRWR/WRRD set, RTL),
with the rest `N/A (not applicable)` by design.

| # | Timing | Meaning |
|---|---|---|
| 1 | **tCL** | CAS latency — cycles from a read command to data on the bus. |
| 2 | **tRCDWR** | RAS-to-CAS delay, *write* — row open to write access. |
| 3 | **tRCDRD** | RAS-to-CAS delay, *read* — row open to read access. |
| 4 | **tRP** | Row precharge — close a row before opening another. |
| 5 | **tRAS** | Active time — minimum row-open duration. |
| 6 | **tRC** | Row cycle — full precharge + activate (tRAS + tRP). |
| 7 | **tRRDS** | Row-to-row activate, *same* bank group. |
| 8 | **tRRDL** | Row-to-row activate, *different* bank group (longer). |
| 9 | **tFAW** | Four-activate window — max activations per window. |
| 10 | **tWTRS** | Write-to-read turnaround, *same* rank. |
| 11 | **tWTRL** | Write-to-read turnaround, *different* rank. |
| 12 | **tWR** | Write recovery. |
| 13 | **tRFC1** | Refresh cycle, rank 1. |
| 14 | **tRFC2** | Refresh cycle, rank 2. |
| 15 | **tRFCsb** | Same-bank refresh. |
| 16 | **tCWL** | CAS *write* latency. |
| 17 | **tRTP** | Read-to-precharge. |
| 18 | **tRDWR** | Read-to-write turnaround. |
| 19 | **tWRRD** | Write-to-read-read turnaround. |
| 20–23 | **tRDRD** (×4) | Read-to-read: same DIMM / same CCD / via SCL / via SC — four scope variants. |
| 24–27 | **tWRWR** (×4) | Write-to-write: the same four scope variants. |

Lower is generally faster, but the *set* must match the DRAM's spec at the
running speed — a mismatch (e.g. aggressive XMP timings the controller
cannot honour) is how stability problems start.

### 8.3 GDM, CAD, voltages

- **GDM (Gear Down Mode)** — covered under clocks; a flag, not a value.
- **CAD bus** — the command/address/**data** bus signal-integrity settings:
  processor ODT (on-die termination), the RTT nominal/write/park modes, and
  the clock / address-command / CS-ODT / CKE **driver strengths**, all in
  **ohms**. Termination is expressed relative to RZQ (the package reference
  impedance, 240 Ω): e.g. `RZQ/6` = 40 Ω. These are the values motherboard
  tuning affects signal quality at high frequencies.
- **Voltages** (mV):
  - **VDDCR_CPU (Vcore)** — the CPU core rail;
  - **VDDCR_SOC** — the SOC/uncore rail (on AMD this feeds the memory
    controller);
  - **VDDIO_MEM** — the memory I/O rail (the DRAM interface itself);
  - **VDD_MISC** — the misc rail;
  - **VPP** — the high voltage that enables on-die charge pumps in the
    DRAM (typically 1.8 V class).
- **CPU temperature (°C)** — `k10temp`/`zenpower` hwmon `temp1_input` first,
  the `cpu_thermal` thermal zone as fallback (a related source, read by the
  client itself — one of the few reads that does not go through the daemon).

### 8.4 Memory bandwidth

**MEM BW** is the benchmark engine's `Memory · Read` figure in GB/s — the
sustained DRAM read bandwidth your system achieves (the bandwidth column of
the 4×4 grid). The Graphs window's MEM BANDWIDTH series is a step function of
these: it moves when a bench/burn-in run completes, and it is the number to
watch while comparing configurations (XMP vs JEDEC, 1:1 vs 1:2 sync,
single vs dual channel).

### 8.5 SPD — what your DIMMs report

SPD is the serial presence-detect EEPROM on each module, read over `ee1004`
(unprivileged). Per slot you get:

- **maker** — the module manufacturer (JEP106 code decoded to a name —
  Samsung, SK hynix, Micron, G.Skill, …);
- **dram die** — the DRAM manufacturer + die type + density per die
  (e.g. `SK hynix (16Gb)`);
- **part** / **serial** — the module part number and serial;
- **rank** — how many DRAM ranks the module carries (`Single-Rank` /
  `Dual-Rank` / …);
- **density** — DRAM density per die (Mbit, shown as Gb);
- **speed** — the module's **base (JEDEC) data rate** in MT/s — the speed it
  is guaranteed to run at without any profile;
- **profiles** — the factory-validated overclock profiles stored on the
  module: **XMP 2.0** (DDR4; two slots) or **XMP 3.0 / EXPO** (DDR5; four
  slots), each rendered as `<speed> MT/s <CL>-<tRCD>-<tRP>-<tRAS> @ <volts>`.
  Enabling one in BIOS is how you move from the base speed to the rated
  speed — and the dashboard then shows you *what the controller actually
  settled on* live.

### 8.6 Channel mode & the RAM summary

The header's RAM line derives the **channel mode** from the installed DIMM
count: 1 → `Single-Channel`, 2 → `Dual-Channel`, 4 → `Quad-Channel` (other
counts read `N/A` — an odd configuration the model does not map). Channel
count is the single biggest bandwidth lever: dual channel roughly doubles
the theoretical interface width, which is why the summary also shows the
per-DIMM breakdown (`2x16 GiB Single-Rank`) and, when the OS sees more
capacity than the SPD bus binds, a slot note (`2 of 4 slots SPD-visible`).

---

## 9. N/A — what the structured reasons mean

RamSleuth follows a **no-panic contract**: a missing driver, a missing
privilege, or unsupported hardware never crashes the daemon or a client —
the affected field degrades to a structured **`N/A (<reason>)`** and every
process exits `0`. The reason is one of exactly **six** variants, and the
reason tells you precisely what is (or is not) wrong.

Two renderings exist. The GUI, the TUI, and `ramsleuth-client status` show
the reason's name (`N/A (DriverMissing)`); the `ramsleuth-client dump`
listing and `ramsleuth-telemetry` show a human-readable phrase
(`N/A (driver missing)`). Same information, two spellings.

| Reason (as shown) | What it means | What to do |
|---|---|---|
| **`DriverMissing`** — *driver missing* | A required kernel driver is not loaded. On **AMD** this is the `ryzen_smu` module absent, so the live AMD subtimings cannot be read. On **Intel** it means the host-bridge PCI config device or `/dev/mem` is absent. | **AMD:** install the driver — `sudo ramsleuth-setup --with-dkms`, `sudo ramsleuth-install-ryzen-smu-dkms`, or the `ryzen-smu-dkms` AUR extra + `sudo ryzen-smu-dkms-install` (Section 10). **Intel:** check that `/dev/mem` exists and your kernel config exposes it. Everything else in RamSleuth works without it. |
| **`UnsupportedHardware`** — *unsupported hardware* | The detected hardware is not supported by this telemetry source. The canonical case: a **virtualized Intel host** whose MCHBAR (BAR5) decodes to `0` — there is no physical memory controller to read. | Nothing — this is the expected, honest outcome in that environment (VMs, or silicon outside the supported set). The other sections of the snapshot remain fully live. |
| **`InsufficientPrivilege`** — *insufficient privilege* | The operation needs more privilege than the caller has (e.g. a `/dev/mem` open/map permission denial, or a `STRICT_DEVMEM` range rejection). | Run the read through the **daemon** (it holds the one capability this needs) — i.e. use the GUI/TUI/`ramsleuth-client` against a running `ramsleuth.service` rather than a bare unprivileged read. |
| **`UnknownPmTableVersion`** — *unknown PM table version* | The AMD SMU PM-table version the firmware reports is outside the layout set RamSleuth knows how to parse. | Update the **AGESA/firmware** (the table versions move with the SMU firmware) or the `ryzen_smu` driver, and retry. If it persists, it is a genuine "newer firmware than supported" case — report the version word upstream. |
| **`NotApplicable`** — *not applicable* | This field does not apply to the detected platform by design — e.g. Intel does not expose the CAD bus or voltage rails through MCHBAR, AMD does not report gear mode, Intel voltages are out of the readout's scope. | Nothing — it is a *correct* blank, not a failure. |
| **`ParseError("<detail>")`** — *parse error: \<detail\>* | A payload read from hardware was malformed or outside its plausible range; the detail names the field (e.g. a clock reading of 0, a truncated block). | Usually transient or firmware-specific; check `journalctl -u ramsleuth`. A persistent one on a specific field is worth reporting with the detail text. |

The daemon's **`status`** subcommand is the fastest way to see which of the
four major sections (CPU / AMD / Intel / SPD) is degraded and why — one line
each, plus the socket path.

---

## 10. AMD live subtimings: the `ryzen_smu` requirement

This is the one hardware-specific requirement in the whole product, so it
deserves its own section.

**Live AMD DRAM subtimings need the `ryzen_smu` kernel module.** The SMU
power-management table (clocks, the 27 subtimings, CAD settings, voltages)
is only readable through that module's sysfs interface; it is not part of
the mainline kernel.

- **With the module:** the AMD section is fully live — clocks, all 27
  subtimings, CAD, voltages — updating at your poll interval.
- **Without it:** the AMD section reads **`N/A (DriverMissing)`** and
  *nothing else changes*: the daemon runs, the GUI/TUI/CLI work, the
  benchmark works, SPD/Intel/platform/temperature all work, and every
  process exits 0. RamSleuth is fully useful on an AMD machine without the
  module; the module only unlocks the live-subtiming depth.

Three ways to get it (all build the same **pinned** upstream source —
`amkillam/ryzen_smu @ d2983668300dd2a598e5a7dc40e71ce0678cc270`, verified
2026-08-15 — never a branch HEAD):

1. **One-click, from the GUI** — the SETUP strip's
   **`Set up RamSleuth + AMD driver`** button (Section 3), or
   **`sudo ramsleuth-setup --with-dkms`** from a terminal.
2. **The built-in helper** — every ramsleuth install ships
   **`sudo ramsleuth-install-ryzen-smu-dkms`**: it verifies the matching
   kernel headers, resolves the source (the offline-vendored tree first, the
   pinned git clone as fallback), shows you the URL + full pin + file
   checksums and **pauses for confirmation before any build**, then
   `dkms add/build/install` + `modprobe` — **immediate, no reboot** — and
   persists `/etc/modules-load.d/ryzen_smu.conf` for boot.
3. **The AUR extra** — `yay -S ryzen-smu-dkms && sudo ryzen-smu-dkms-install`
   (Section 2.5): the same helper, plus the vendored source shipped with the
   package, so the build is fully network-free.

Verify it is working:

```sh
ls /sys/kernel/ryzen_smu_drv/pm_table      # the module's live interface
systemctl status ramsleuth                 # daemon healthy
ramsleuth-client status                    # AMD: ok
```

`AUTOINSTALL=yes` in the bundled `dkms.conf` rebuilds the module
automatically on kernel updates. Two operational notes: the module is built
against your *current* kernel and staged (inspectable) under
`/usr/src/ryzen_smu-…`, and the ramsleuth systemd unit **never loads the
module itself** — loading is the helper's job, kept separate so the daemon
stays sandboxed and the driver stays an explicit, audited opt-in.

---

## 11. Day-2 operations & troubleshooting

### 11.1 The daemon is down

Symptom: the GUI header shows `Disconnected` (crimson) or the SETUP strip
offers *"Start the ramsleuth daemon"*; the CLI prints the friendly
`DaemonDown` diagnostic (Section 6); the TUI shows `down`.

```sh
systemctl status ramsleuth          # what state is it in? why?
journalctl -u ramsleuth -f          # its log (follow) — the unit auto-restarts on failure
systemctl restart ramsleuth         # bring it back
sudo systemctl enable --now ramsleuth  # the canonical start-if-not-enabled (what the SETUP strip suggests)
```

The unit is `Restart=on-failure`, so most transient crashes recover on
their own — the journal tells you whether you are looking at something that
recurred and stuck.

### 11.2 `N/A (DriverMissing)` on AMD

The `ryzen_smu` module is not loaded (fresh boot after a kernel update
without the DKMS rebuild, never installed, or the module was removed):

```sh
sudo ramsleuth-setup --with-dkms        # one-click: build + load (the GUI button equivalent)
# or
sudo ramsleuth-install-ryzen-smu-dkms   # the helper directly
# or, AUR extra: yay -S ryzen-smu-dkms && sudo ryzen-smu-dkms-install
ls /sys/kernel/ryzen_smu_drv/pm_table   # confirm it is live
```

Then re-open the GUI / re-run `ramsleuth-client status` — the AMD section
fills in on the next poll (no app restart needed; the daemon re-reads on
every telemetry pass).

### 11.3 Group membership / socket access problems

Symptom: a **permission** error from the clients (the socket is
`0660 root:ramsleuth`), e.g. the SETUP strip's *"Join the `ramsleuth`
group"* row, or `Permission denied (os error 13)` in the CLI diagnostic.

- **Fastest fix — re-run setup:** `sudo ramsleuth-setup` re-applies
  everything idempotently: the group membership, the
  `/etc/ramsleuth/authorized-users` seed, and a fresh `setfacl` on the live
  socket — **your current session is fixed immediately, no re-login**.
- **Manual:** `sudo usermod -aG ramsleuth $USER` — then the group half
  applies at your **next login** (PAM); for the current session you still
  need the ACL half (re-run `sudo ramsleuth-setup`, or restart the daemon —
  it re-applies the ACLs from the state file at its next socket creation).
- If the ACL is absent/stale and you cannot re-run setup,
  `systemctl restart ramsleuth` regenerates the socket and re-applies the
  ACLs from `/etc/ramsleuth/authorized-users`.

### 11.4 Upgrading

- **AUR:** `yay -S ramsleuth` (or your tier) re-runs the update; the hooks
  re-apply the group/ACL grants for the invoking user when run under `sudo`.
- **install.sh:** `git pull && ./install.sh` — idempotent, same file set,
  the daemon is reloaded at the end.
- The `ryzen_smu` module needs no manual action on kernel updates
  (`AUTOINSTALL=yes` rebuilds it).

### 11.5 A few honest limitations

- **Virtualized Intel** (and similar non-physical environments) read
  `N/A (unsupported hardware)` for the Intel section by design — the
  MCHBAR register does not exist in the VM (Section 9).
- **Intel voltages & CAD** are `N/A (not applicable)` by design — the
  MCHBAR window does not expose them the way the AMD SMU does.
- **SPD sees only bound modules**: if the OS total exceeds the SPD-visible
  sum, the header's slot note (`2 of 4 slots SPD-visible`) tells you — that
  is the platform, not a bug.
- The GUI's **settings knobs are in-memory** — they do not persist across
  restarts (a documented follow-up); the daemon socket, poll interval, and
  units reset to defaults on each launch (the `--socket` flag is the
  persistent override).

---

## 12. Uninstall

Uninstall the way you installed.

**AUR installs** — the package manager removes the binaries, unit, preset,
icons, desktop entry, helper scripts, and polkit policy, and runs the
`.install` hooks' cleanup:

```sh
yay -Rns ramsleuth        # or: ramsleuth-bin / ramsleuth-git (whichever you have)
yay -Rns ryzen-smu-dkms   # only if you installed the AMD extra
```

**`install.sh` installs** — there is no package metadata, so remove the
pieces by hand (adjust to what you actually installed; every path is the
one the installer used):

```sh
# 1. Stop and disable the daemon
sudo systemctl disable --now ramsleuth

# 2. Remove the unit + preset, then reload systemd
sudo rm /usr/lib/systemd/system/ramsleuth.service
sudo rm /usr/lib/systemd/system-preset/ramsleuth.preset
sudo systemctl daemon-reload

# 3. Remove the six binaries
sudo rm /usr/bin/ramsleuth-daemon /usr/bin/ramsleuth-client /usr/bin/ramsleuth-tui \
        /usr/bin/ramsleuth /usr/bin/ramsleuth-bench /usr/bin/ramsleuth-telemetry

# 4. Remove the helpers + the kept installer
sudo rm /usr/bin/ramsleuth-setup /usr/bin/ramsleuth-install-ryzen-smu-dkms
sudo rm -rf /usr/share/ramsleuth

# 5. Remove the polkit policy
sudo rm /usr/share/polkit-1/actions/90-ramsleuth-setup.policy

# 6. Remove the desktop entry + icons
sudo rm /usr/share/applications/RamSleuth.desktop
sudo rm /usr/share/pixmaps/ramsleuth.png /usr/share/pixmaps/RamSleuth.png
sudo find /usr/share/icons/hicolor \( -name 'ramsleuth.png' -o -name 'RamSleuth.png' \) -delete

# 7. Remove the state (the authorized-users file + its directory)
sudo rm -rf /etc/ramsleuth

# 8. Remove the runtime socket directory (it is on tmpfs; gone at reboot anyway)
sudo rm -rf /run/ramsleuth

# 9. Remove the ramsleuth group (system group; remove members first if you like)
sudo groupdel ramsleuth
```

And if the **AMD `ryzen_smu` module** was installed, remove it:

```sh
sudo rmmod ryzen_smu
sudo dkms remove ryzen_smu/1.d298366   # the installed DKMS version (list with: dkms status)
sudo rm /etc/modules-load.d/ryzen_smu.conf
sudo rm -rf /usr/src/ryzen_smu-*
```

Files you create outside the install (e.g. TUI snapshot `.txt` files in
your working directories, the `ramsleuth-snapshot-*.png` /
`ramsleuth-export-*.json` exports in `$HOME`) are yours to keep or delete.
After a clean uninstall, nothing RamSleuth-related remains: no service, no
group, no socket, no driver.

---

*This guide describes RamSleuth v2.2.1. For the technical design, see
`Docs/Architecture.md`; for the packaging operator guide, see
`packaging/README.md`.*
