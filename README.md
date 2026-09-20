# RamSleuth v2.2.0

Live memory-controller telemetry and an AIDA64-style benchmark engine for PC RAM — 100% pure Rust, dual frontend (terminal + desktop), one small-capability privileged daemon.

## What it is

RamSleuth v2 is a Cargo workspace of 7 crates with two layers:

1. **Live telemetry** — the memory controller's operating state: SMU clocks (MCLK/UCLK/FCLK), 27 DRAM subtimings, GDM and command rate on AMD; a read-only MCHBAR decode path on Intel; plus fully unprivileged SPD EEPROM decode (JEP106 makers, rank, density, base speed, XMP 2.0/3.0/EXPO profiles).
2. **Benchmark engine** — an AIDA64-style 4×4 bandwidth/latency grid: native AVX2/AVX-512F kernels, one worker thread pinned per physical core, pointer-chase latency, and a burn-in mode.

**Privilege separation.** Every privileged read goes through a single daemon (`ramsleuth-daemon`) that runs as root but holds *only* `CAP_SYS_RAWIO` — the capability the SMU and `/dev/mem` MCHBAR reads require. It listens on a Unix socket at `/run/ramsleuth/ramsleuth.sock` (mode `0660`, group-owned). All clients — the CLI, the TUI, the GUI — run fully unprivileged; you need membership in the `ramsleuth` group (the in-app one-click **Set up RamSleuth** grants it — plus a current-session socket ACL, no re-login; see Quickstart). The wire protocol is length-prefixed Bincode frames (u32-LE length + `bincode` 1.3 payload).

**No-panic contract.** A missing driver, privilege, or hardware never crashes the daemon or any client: the affected fields render as a structured `N/A (<reason>)` (e.g. `N/A (DriverMissing)`) and the process exits 0.

## Features

- **AMD (via the `ryzen_smu` kernel driver):** SMU PM-table clocks (MCLK/UCLK/FCLK), 27 DRAM subtimings (ticks), GDM and command rate — read from the `ryzen_smu_drv` sysfs kobject, with a `/dev/ryzen_smu` char-device fallback.
- **Intel (MCHBAR):** the MCHBAR base is located in the host-bridge PCI config space and the MMIO window is mapped **read-only** through `/dev/mem` (`PROT_READ`, `MAP_PRIVATE`) for per-channel register decode. Vendor-gated, no-panic; hardware that cannot be verified degrades to `N/A (<reason>)`.
- **SPD EEPROM, fully unprivileged:** the raw image of every DIMM the kernel's `ee1004` I2C driver has bound, read through a world-readable sysfs attribute — no root, no `CAP_SYS_RAWIO`, no `unsafe`, no new dependencies. Decodes module/die JEP106 makers, rank, density, base speed, and XMP 2.0 / XMP 3.0-EXPO profiles.
- **Benchmark engine:** native AVX2 / AVX-512F SIMD kernels; `sched_setaffinity` pins one worker thread per physical core; the AIDA64-style 4×4 grid (copy/read/write across l1/l2/l3/full tiers); pointer-chase latency; optional burn-in loop.
- **CLI (`ramsleuth-client`):** `dump` (the dashboard-style telemetry listing), `bench` (a streamed run: a progress line per completed cell, then the terminal 4×4 grid), `status` (the one-line-per-section health summary).
- **TUI (`ramsleuth-tui`):** a live three-zone dashboard on ratatui, polling the daemon every 2 s; `[R]`efresh / `[S]`napshot / `[Q]`uit; the terminal is restored on every exit path — never left in raw mode.
- **GUI (`ramsleuth` — the `ramsleuth-gui` crate):** an egui/eframe desktop dashboard — a three-zone 968×600 window (live telemetry matrix, benchmark grid, hardware/SPD status), a dedicated Graphs window, a Settings panel, an auto-shown `SETUP` requirements strip on first launch (daemon down / missing `ramsleuth` group / missing `ryzen_smu` on AMD — copy-only commands, a "Got it" close, no-panic; it disappears once the requirement clears), `[F2]` PNG snapshot / `[F3]` JSON export / `[Q]`uit, repainting at ~60 FPS.
- **Graceful degradation:** absent driver, privilege, or hardware → structured `N/A (<reason>)` fields, exit 0, no panics.

## Architecture

| Crate | Role |
| --- | --- |
| `ramsleuth-telemetry` | Telemetry backend: AMD SMU PM-table readout, Intel MCHBAR `/dev/mem` decode, unprivileged SPD EEPROM, CPU/board info |
| `ramsleuth-bench` | The AVX2/AVX-512F bandwidth & latency benchmark engine (kernels, per-core dispatch, streaming, burn-in) |
| `ramsleuth-protocol` | The wire protocol: u32-LE length-prefixed Bincode (1.3) frames — the daemon↔client contract (library-only, never installed) |
| `ramsleuth-daemon` | The privileged service: `CAP_SYS_RAWIO` only, Unix-socket listener, TTL telemetry cache, single-flight benchmark job manager |
| `ramsleuth-client` | The unprivileged RPC client (connect + `dump` / `bench` / `status`) |
| `ramsleuth-tui` | The ratatui three-zone terminal dashboard |
| `ramsleuth-gui` | The egui/eframe three-zone desktop dashboard + Graphs window + Settings |

## The 6 binaries

| Binary | Key flags / keys | Exit codes |
| --- | --- | --- |
| `ramsleuth-daemon` | `--socket <path>` (default `/run/ramsleuth/ramsleuth.sock`), `--max-age <secs>` (cache TTL, default 2) | 0 clean signal shutdown · 1 fatal socket/signal setup · 2 CLI error |
| `ramsleuth-client` | subcommands `dump` (default) / `bench` / `status`; `--socket <path>`; `--tier <memory\|l1\|l2\|l3\|full>` (default full); `--mode <full\|memory-only>` (default full) | 0 success · 1 daemon/client error · 2 usage |
| `ramsleuth-tui` | `--socket <path>`; keys `[R]`efresh / `[S]`napshot / `[Q]`uit | 0 quit · 1 terminal init failure · 2 usage |
| `ramsleuth` | `--socket <path>`; keys `[F2]` PNG snapshot / `[F3]` JSON export / `[Q]`uit | 0 quit · 1 eframe/display failure · 2 usage |
| `ramsleuth-bench` | `--avx512` (force the AVX-512 kernel path; falls back to AVX2 when AVX-512F is absent), `--json` (grid as a JSON object) | 0 success · 1 topology detection failure · 2 usage |
| `ramsleuth-telemetry` | `--json` (the snapshot as a hand-rolled JSON block) | 0 snapshot rendered (even if every section is N/A) · 2 unknown flag |

## Build from source

- **Toolchain:** Rust **MSRV 1.75** (edition 2021, resolver 2).
- **System libraries (GUI only):** the GUI links X11/Wayland/GL — on Debian/Ubuntu: `libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev libgl1-mesa-dev pkgconf` (`libxkbcommon` is the only strict build-time link dep).
- **Build:**

  ```sh
  cargo build --workspace --release
  ```

  producing the 6 binaries above in `target/release/`.

## Running

1. **Daemon.** Production: the installed systemd unit runs
   `ExecStart=/usr/bin/ramsleuth-daemon --socket /run/ramsleuth/ramsleuth.sock`
   sandboxed (`CapabilityBoundingSet=CAP_SYS_RAWIO`, `NoNewPrivileges=true`, `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`, `Restart=on-failure`).
   Unprivileged dev run (the privileged fields degrade to `N/A`, with warnings on stderr):
   `cargo run -p ramsleuth-daemon -- --socket /tmp/ramsleuth.sock`.
2. **Clients.** Grant a normal user access to the `0660` group-owned socket — the in-app **Set up RamSleuth** button does it in one click (no re-login; Quickstart), and `sudo ramsleuth-setup` is the same single step from a terminal. Then:

   ```sh
   ramsleuth-client dump      # the full telemetry listing
   ramsleuth-client bench     # the streamed 4×4 grid
   ramsleuth-client status    # the per-section health summary
   ramsleuth-tui              # the live terminal dashboard
   ramsleuth                  # the live desktop dashboard (the ramsleuth-gui crate)
   ```

## Installation

**From GitHub (self-contained):**

```sh
git clone https://github.com/MadGoatHaz/RamSleuth && cd RamSleuth && ./install.sh
```

Interactive and visually sectioned; transparent — it prints every source, the exact commit being installed, and the pinned third-party source before doing anything; AUR-equivalent — the 6 binaries + the frozen daemon unit + the preset + the `ramsleuth` group to `/usr` via sudo; idempotent (a re-run after any failure is safe). It then **asks** about the `ryzen_smu` DKMS module (AMD hosts only) and walks you through the pinned shared helper; the app runs without it — the AMD section reads `N/A (DriverMissing)`.

**AUR (Arch) — the two stable packages:**

```sh
yay -S ramsleuth      # stable — builds from the official git tag (source)
yay -S ramsleuth-bin  # precompiled binary from the GitHub Release — the fastest install
```

(or `paru -S ramsleuth` / `paru -S ramsleuth-bin` in place of `yay`). The two install the same file set and **mutually conflict** — pick exactly one. The bleeding-edge dev option (the moving `v2-development` branch, for testing unreleased work) is `yay -S ramsleuth-git`; the optional AMD telemetry driver is the separate extra `yay -S ryzen-smu-dkms` (then `sudo ryzen-smu-dkms-install` — AMD hosts only; without it the AMD subtimings read `N/A (DriverMissing)`).

**Manual (from a source checkout):**

```sh
cd packaging/ramsleuth   # or: packaging/ramsleuth-bin / packaging/ramsleuth-git
makepkg -si
```

The package installs the 6 binaries to `/usr/bin/`, the frozen daemon unit to `/usr/lib/systemd/system/ramsleuth.service` (plus a preset that keeps it enabled on `systemctl preset` runs), and creates the `ramsleuth` system group (idempotent `groupadd -r`). The `post_install` hook runs `systemctl enable --now ramsleuth` — the daemon starts automatically — and self-grants the invoking user (`SUDO_USER`) the `ramsleuth` group + a socket ACL, so a GUI install opens with full access already in place.

Day-2:

```sh
systemctl status ramsleuth
journalctl -u ramsleuth
```

## Quickstart

1. **Install** — any path above: the AUR triple (`yay -S ramsleuth` / `ramsleuth-bin` / `ramsleuth-git`) or the self-contained `./install.sh`.
2. **Open the app** — the `ramsleuth` binary, or its desktop-menu entry (the `ramsleuth` / `ramsleuth-bin` packages and `install.sh` install one; the app carries its own RamSleuth window icon).
3. **One click** — on first launch the `SETUP` strip offers **Set up RamSleuth** (on AMD hosts: **Set up RamSleuth + AMD driver**). One polkit password prompt runs every privileged step in a single pass: daemon enable + start, `ramsleuth` group join, and a socket ACL for your user.
4. **Full capabilities, immediately** — no re-login, no reboot, no copy-paste. On AMD the pinned `ryzen_smu` driver is built **offline** (vendored, checksum-verified) and `modprobe`d in the same run; group membership is written too, so a plain re-login keeps working on future sessions.

No-desktop box? The same one step from a terminal is `sudo ramsleuth-setup` (add `--with-dkms` on AMD) — appendix below.

## AMD telemetry requirement (live subtimings)

Live AMD SMU telemetry requires the `ryzen_smu` kernel module. **It is not a hard dependency** — without it, the AMD subtiming fields read `N/A (DriverMissing)` and everything else keeps serving.

- **One click (every install path):** the `SETUP` strip's **Set up RamSleuth + AMD driver** button — or `sudo ramsleuth-setup --with-dkms` from a terminal — builds + loads it in the same run (Quickstart).
- **AUR extra:** `yay -S ryzen-smu-dkms`, then `sudo ryzen-smu-dkms-install`.
- **Shared helper / source checkout:** `sudo ramsleuth-install-ryzen-smu-dkms` (installed by every RamSleuth path) or `scripts/install-ryzen-smu-dkms.sh` from a checkout — idempotent, re-execs under `sudo`, requires the matching kernel headers. It builds the **pinned** `ryzen_smu` @ `d298366` **offline from the vendored in-repo source** (every file verified against `vendor/SUMS.sha256` before staging; the git-pinned fetch remains the `RYZEN_SMU_FORCE_REMOTE=1` fallback), then `dkms install` + `modprobe` (immediate — no reboot) + a persistent `/etc/modules-load.d/ryzen_smu.conf`. `AUTOINSTALL=yes` rebuilds the module on kernel updates.

Verify: `ls /sys/kernel/ryzen_smu_drv/pm_table`. The module ships a `monitor_cpu` CLI for ground-truth comparison (expect clocks within ±1 MHz, voltages within ±10 mV); `scripts/amd-ground-truth.sh` runs the side-by-side check.

## Transparency & provenance

The install path is auditable end to end:

- **Own code only in the build.** The `ramsleuth` and `ramsleuth-git` AUR packages and `./install.sh` compile **only this repository** — tag-/branch-pinned, `cargo build --locked`, no third-party code in a build chroot, no `sha256sums` needed. `ramsleuth-bin` compiles nothing: it downloads the release binary tarball, pinned by `sha256`.
- **The only third-party source** is the optional AMD `ryzen_smu` kernel module, **pinned to commit `d2983668300dd2a598e5a7dc40e71ce0678cc270` of `amkillam/ryzen_smu`** (verified 2026-08-15, "Fix cpuid include on 7.2+ kernels (#53)"), **vendored in-repo** at `packaging/ryzen-smu-dkms/vendor/` (byte-identical — the offline build; the git fetch is the fallback). The bundled helper shows, checksums, and confirms it **on the target** (never in a build chroot), and the staged source stays inspectable at `/usr/src/ryzen_smu-1.d298366`.
- **The vendor dir is a separate work.** `ryzen_smu` is **GPL-2.0**; it is never compiled into or linked with any MIT-licensed RamSleuth binary — DKMS builds it standalone in `/usr/src` (its verbatim upstream `LICENSE` + `NOTICE.md` ship with the vendor dir).
- **No dependency on any third-party AUR package.** The `ryzen-smu-dkms` extra is optional and co-install-safe (it ships the same helper under a different filename, so it never conflicts with the RamSleuth packages).
- **Every installed file is byte-identical to a file in this repository** (auditable via `git show`): the helper ships as `/usr/bin/ramsleuth-install-ryzen-smu-dkms` (from `ramsleuth-git`) and `/usr/bin/ryzen-smu-dkms-install` (from the extra), and `install.sh` ships as `/usr/share/ramsleuth/install.sh`, so the whole flow can be re-run and audited post-install.

## Testing

- **556/556 tests green** across the workspace, in debug **and** release.
- `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings.
- CI (GitHub Actions, on push/PR to `v2-development`): a test job on a `1.75` (MSRV) × `stable` matrix runs debug + release tests and clippy; a build job compiles the 6 release binaries and uploads them as an artifact.

## Project layout

```
RamSleuth/
├── Cargo.toml                 # workspace: edition 2021, resolver 2, MSRV 1.75
├── crates/
│   ├── ramsleuth-telemetry/   # telemetry backend (AMD SMU, Intel MCHBAR, SPD)
│   ├── ramsleuth-bench/       # AVX2/AVX-512F benchmark engine
│   ├── ramsleuth-protocol/    # bincode wire protocol (library)
│   ├── ramsleuth-daemon/      # privileged service
│   ├── ramsleuth-client/      # unprivileged RPC client
│   ├── ramsleuth-tui/         # ratatui terminal dashboard
│   └── ramsleuth-gui/         # egui/eframe desktop dashboard
├── systemd/ramsleuth.service  # sandboxed daemon unit (CAP_SYS_RAWIO only)
├── packaging/                 # the 3-tier AUR packages (ramsleuth, ramsleuth-bin, ramsleuth-git) + the ryzen-smu-dkms extra
├── install.sh                 # self-contained installer (the GitHub path — AUR-parity)
├── scripts/                   # install-ryzen-smu-dkms.sh, amd-ground-truth.sh
└── .github/workflows/ci.yml   # MSRV × stable test matrix + release build
```

## Appendix — manual fallback (copy-paste)

For headless boxes, sessions without a graphical polkit agent, or whenever you prefer the terminal, the one-click setup is the same helper with `sudo`:

```sh
sudo ramsleuth-setup              # daemon enable+start + group join + socket ACL
sudo ramsleuth-setup --with-dkms  # the same, plus the offline AMD driver build
```

The finer-grained equivalents (what the GUI's per-row `Copy` buttons also offer):

```sh
sudo systemctl enable --now ramsleuth    # enable + start the daemon
sudo usermod -aG ramsleuth "$USER"       # group membership (effective at your next login)
sudo ramsleuth-install-ryzen-smu-dkms    # AMD: build + load the pinned ryzen_smu module
```

Health: `systemctl status ramsleuth`, `journalctl -u ramsleuth`; confirm the ACL grant with `getfacl /run/ramsleuth/ramsleuth.sock`.

## License

MIT — the workspace `Cargo.toml` declares `license = "MIT"`. Repository: <https://github.com/MadGoatHaz/RamSleuth>.

This README describes **RamSleuth v2.2.0** (branch `v2-development`); the workspace package version is `2.2.0` (bumped in Cycle 21); the earlier release tags (`v2.1.1`, `v2.1.0`) remain in the git history.
