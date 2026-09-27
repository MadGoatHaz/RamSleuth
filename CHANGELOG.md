# Changelog

All notable per-release changes to RamSleuth. Newest first.

**Versioning policy.** The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml`; every member crate inherits it. A release = a version bump + the git tag `v<ver>` + the release workflow (`.github/workflows/release.yml`) publishing the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`. The AUR packages (`ramsleuth`, `ramsleuth-bin`, and the `ramsleuth-intel-dkms` extra) track this versioning and are maintained at the same pace as the project.

## [2.4.10] - 2026-09-27

### Changed
- **One-click setup is now fully non-interactive on a fresh machine**: the DKMS helpers install build tooling with `pacman -S --needed --noconfirm`, so the GUI/pkexec path no longer stalls on a `Proceed? [Y/n]` prompt (the confirmed root cause of the fresh-install setup loop); the ryzen helper's git-clone fallback also sets `GIT_TERMINAL_PROMPT=0`, so a credential prompt can never hang the flow
- **The setup flow logs its full output + a state snapshot to `/var/lib/ramsleuth/setup.log` (world-readable)**: every run of `ramsleuth-setup` is self-documenting (a run header, the whole stdout + stderr transcript, and a unit/module/socket/group snapshot — appended before the done line and via an EXIT trap after every failure), the vendor helpers inherit the stream (a deduped tee guard), and the app shows that path on any setup failure so problems are diagnosable off-band
- **Daemon startup is journaled**: every (re)start writes a line to the journal (`journalctl -u ramsleuth`) with the version + the resolved args (socket path, max-age, spd-autobind), anchoring the startup output that already reaches the journal

## [2.4.9] - 2026-09-26

### Changed
- **Self-explanatory N/A (GUI + TUI)**: every `N/A` now shows the reason in plain language (e.g. "No data — this CPU doesn't expose the memory-controller registers RamSleuth reads. Expected on this part, not an error") with the daemon status kept **green** — "no data on this part" (unsupported hardware, unloaded driver, privilege, decode) is rendered as a healthy state, never mistaken for a failure
- **The setup prompt trigger is now a named, tested liveness predicate** (`first_run::requirements_strip_visible`): daemon down → the one-click setup prompt shows (fresh install, stopped daemon, and leftover-partial install all converge on it); daemon up → telemetry (the strip clears the moment the daemon serves, even when the served telemetry is all `N/A`)

## [2.4.8] - 2026-09-26

### Changed
- **Idempotent one-click DKMS install over a stale residual module** (both the AMD `ryzen_smu` and Intel `ramsleuth_intel` helpers): a re-run over an already-installed module (a residual `.ko` of the same module left by a prior build, or a wiped DKMS DB) previously aborted on DKMS's identical-module check ("already installed at version … override by specifying --force"); both helpers now pass `dkms install --force`, which overwrites the residual (a no-op on a clean first run) — so a re-click **always** succeeds with no manual cleanup
- **The broken `/usr/lib/depmod.d/<module>.conf` override is removed** from both helpers — the `override <mod> /extra/<mod>.ko` line is invalid syntax `depmod` rejects and is pointless on dkms 3.4.3 (`DEST_MODULE_LOCATION "/extra"` never materializes; the module deploys to `updates/dkms/` and resolves from there) — plus a **self-heal** that removes a stale entry an older helper left behind
- **The DKMS self-heal removal now uses the valid `dkms remove <module>/<version> --all --no-depmod` cleanup syntax** (verified against the installed dkms 3.4.3 CLI, which rejects `--all-kernels`), so the helper removes every registered version of the module before re-adding from the freshly staged source
- **The `ramsleuth_intel` module's `MODULE_VERSION` now tracks releases** (was the constant `1.0.0`; now `2.4.8`) with a track-releases note — the AMD vendored `ryzen_smu` module's `MODULE_VERSION` (`0.1.7`) is left untouched (a SUMS-verified frozen upstream copy; its DKMS version is the git-derived PKGVER, not that constant)
- **The makepkg 7.x source-integrity pin fix is carried forward**: all three git-source AUR packages (`ramsleuth`, `ramsleuth-intel-dkms`, `ryzen-smu-dkms`) keep the standard VCS `#commit=`-fragment source + content-addressed `sha256sums` (the sha256 of the pinned commit's `git archive` tarball) — the form makepkg 7.x generates (`makepkg -g`) and its integrity gate verifies (a bare `-` fails the gate on 7.x; `SKIP` passes only as a no-op)

## [2.4.7] - 2026-09-26

### Added
- **Secure-Boot-aware one-click DKMS** (both the AMD `ryzen_smu` and Intel `ramsleuth_intel` helpers): on a Secure Boot host the helper now detects Secure Boot **early** (`mokutil --sb-state`, with an EFI/lockdown fallback), **signs** the built module with a **persistent** RamSleuth key pair (generated once, idempotent, 10-year, at `/var/lib/ramsleuth/<module>-signing/`, wired through a per-module `/etc/dkms/framework.conf.d/` drop-in), **stages the cert for the one-time MOK enrollment** (`mokutil --import`), and exits **10** with **one-step guidance** (reboot → at the blue MOK screen choose `Enroll MOK key(s)` → `Continue` → `Yes` → re-click Setup). After the MOK step the re-click succeeds — the driver is already built + installed + signed, so nothing is rebuilt
- **Never-mysterious diagnostics**: the one-click flow surfaces the real failure — the GUI renders the first `ERROR:` line on stderr **plus the exit code** (`(exit N)`) verbatim, and maps exit **10** to an amber **"one step left"** state (the Secure Boot MOK enrollment is pending) **instead of a failure**; `ramsleuth-setup` passes the helper's exit codes through verbatim

### Changed
- **Idempotent, self-healing setup**: `ramsleuth-setup` now clears its own stale prior-run state at the start of **every** invocation — a `FAILED` unit (`reset-failed`), the `ramsleuth` group (ensured *before* the unit starts), the `authorized-users` state file (atomic self-healing overwrite: keep valid users, ensure ours, drop blanks/dups), a stale daemon socket (`restart`), and a stale polkit action pool (`restart`) — so a re-run **always** succeeds with no manual cleanup (the 2.4.5-failed → uninstall → 2.4.6-still-fails class is closed); the DKMS helpers likewise self-heal by removing **every registered version** of the module (`dkms remove … --all --no-depmod`) before re-adding from the freshly staged source
- **The repo's `ryzen-smu-dkms/dkms.conf` (`DEST=/extra`) now ships on every install path** — the release tarball (27 → **28** artifacts: 6 binaries + 22 auxiliary), both main AUR packages (new guarded `package()` step 14 → `/usr/share/ryzen-smu-dkms/dkms.conf`), and `install.sh` — so the DKMS helper resolves the repo's config (matching its `depmod` override) at its installed location; the vendored copy's `/kernel/drivers/ryzen_smu` is the last-resort fallback only

## [2.4.6] - 2026-09-26

### Added
- The AMD `ryzen_smu` driver source is now **bundled in both main packages** (`ramsleuth`, `ramsleuth-bin` → `/usr/share/ryzen-smu-dkms/vendor/`, SUMS-verified, guarded for pre-vendor tags) — the in-app one-click installs the AMD driver **offline** on a clean install (no git clone, no separate AUR extra), symmetric with the bundled Intel module source

### Changed
- The `ryzen-smu-dkms` AUR extra is now **mutually exclusive** with the main packages (like `ramsleuth-intel-dkms`): both bundle the same vendored source, so installing the extra removes a main package first — it is the **standalone provisioning path** only
- The daemon `CAP_SYS_RAWIO` privilege probe checks bit **17** (`CAP_SYS_RAWIO`) instead of bit 21 (`CAP_SYS_ADMIN`) — fixes the false `InsufficientPrivilege` warning on correctly-privileged daemons
- polkit branded-dialog fix: the inert `exec.arguments` annotation is removed (not a real polkit key), and the install paths (the AUR `.install` hooks + `install.sh`) restart polkit so the `org.freedesktop.ramsleuth.setup` action loads on install — the one-click prompt shows RamSleuth's branded message
- The release tarball grows to 27 artifacts (6 binaries + 21 auxiliary, including the 8 vendored `ryzen_smu` files); `ryzen-smu-dkms` keeps its own version line (pkgver 1.0, branch-pinned) and does not track the workspace version

## [2.4.5] - 2026-09-25

### Added
- ECC detection on **both** platforms: AMD via `UmcCapHi` (bit 30/31) and Intel via `CAPID0_A` (a new `capid0a` sysfs attribute — the 25th — decoded from bit 17)
- AMD channel-mode detection from the SMN CS-population readout (Single / Dual-Channel (Symmetric) / Dual-Channel (Flex))
- a 25-attribute `ramsleuth_intel` kernel module (adds `capid0a`)

### Changed
- SPD decoder re-baselined to **JESD79-4 Annex L**: correct maker / die-maker JEP106, density, rank, width, serial, XMP 2.0 profiles, and JEDEC base speed
- XMP 2.0 timing decode (tRCD / tRP / tRAS)
- Intel channel-mode DIMM-population cross-check: a firmware Dual-Symmetric demotes to Flex on asymmetric population
- SPD EEPROM auto-bind now falls back to the `ee1004` driver `bind` file (one attempt per process lifetime; the per-collect warning loop is gone)
- TUI/GUI polish: terminal ghosting fix, panel layout, header rework, probe modal, and the title-only GitHub issue URL

## [2.4.2] - 2026-09-25

### Added
- Intel generational expansion: Tier 1 (Skylake/Kaby), Tier 2 (Coffee Lake/Rocket Lake with Gear2 + brand-based gen disambiguation), Tier 3 (Alder/Raptor/Meteor/Arrow Lake with DDR5 4-subchannel, Gear4, 256 KiB window, MCL fallback, tile-routing probe)
- In-app "Submit Probe Report" feature: consent-gated markdown report (telemetry + raw IMC registers + system info) with GUI (GitHub issue / clipboard) and TUI (file / clipboard) flows
- `.github/ISSUE_TEMPLATE/probe-report.md` for pre-filled issue submission

### Changed
- Packaging: both main AUR packages now bundle `kernel/ramsleuth-intel/` source (guarded install); mutual exclusion with the standalone `ramsleuth-intel-dkms` extra
- Intel IMC decode now dispatches via `GenProfile` (window size, MCHBAR mask, channel count, gear cap, register map)

## v2.4.0 (2026-09-23)

**Intel hardening + ergonomics** — channel-mode decode, SPD auto-bind, and the MCLK/MCHBAR fixes that completed Intel parity:

- **Added:** five new Intel MAD channel/geometry registers (`mad_inter_channel`, `mad_intra_ch0`, `mad_intra_ch1`, `mad_dimm_ch0`, `mad_dimm_ch1`) exposed by the `ramsleuth_intel` module — sysfs attributes 19 → 24.
- **Added:** the hardware-derived Intel channel-mode label — the header channel label and the `Mode:` slot now come from the `MAD_INTER_CHANNEL` register (e.g. "Dual-Channel (Flex)" for asymmetric DIMMs) when the module is loaded, falling back to the installed-DIMM count otherwise.
- **Added:** SPD EEPROM auto-bind fallback — the daemon (as root) now attempts to bind SPD EEPROMs the kernel's `ee1004` driver missed (common on boards whose DSDT advertises a single DIMM slot). On by default; disable with `--no-spd-autobind`. Non-fatal and never unbinds.
- **Fixed:** the Intel MCLK decode — removed an erroneous ÷2 (MCLK = `CLK_RATIO` × refclk; MT/s = 2 × MCLK). A DDR4-2133 system now reports 1066.67 MHz instead of 533.33 MHz.
- **Fixed:** the Intel MCHBAR window mapped 1 MiB → 64 KiB (the Tier-1 datasheet window) in both the runtime and `/dev/mem` fallback maps, avoiding overlap with adjacent host-bridge BARs.
- **Fixed (DKMS):** the Intel DKMS install script now always rebuilds and reloads the module from the current source on re-run (previously an already-loaded module was left stale).
- **Changed:** docs — corrected the Intel MCLK formula and DDR4-2400 fixture (ratio 9 @ 133.3333 MHz refclk) in `Docs/Architecture.md` and the research doc; updated the `Docs/User_Guide.md` Intel sections (channel mode, SPD auto-bind, 24 attributes).

**Operator-gated release steps (out of scope for this commit):** the git tag `v2.4.0`, the release re-cut, the `ramsleuth-bin` tarball sha256 re-finalization (the in-tree pin is still the published v2.2.1 asset), and the AUR resubmits (`ramsleuth`, `ramsleuth-bin`, `ramsleuth-intel-dkms`).

## v2.3.0 (2026-09-23)

**Intel full parity** — live DRAM subtimings on Intel, matching the AMD experience:

- **Added:** live Intel DRAM subtimings (v1 / Tier-1: Skylake, Kaby Lake, Coffee Lake, Comet Lake — DDR4). The new, in-repo original `ramsleuth_intel` kernel module (written by the project, no upstream) exposes the raw IMC registers under `/sys/kernel/ramsleuth_intel/`, provisioned by the `ramsleuth-intel-dkms` AUR extra, the `install-intel-dkms.sh` operator helper, and the now-vendor-aware `ramsleuth-setup.sh`. Telemetry is sysfs-first with a `/dev/mem` fallback (used on `DriverMissing` only).
- **Fixed:** the Intel MCHBAR decode (bit 0 is `MCHBAR_EN`, not a PCI I/O-space flag — previously every enabled MCHBAR was rejected → all-N/A); the IMC register map (corrected to the verified Tier-1 layout: `MC_BIOS_REQ@0x5E00`, per-channel `TC_*` @ `0x4000`/`0x4400`).
- **CI:** an independent `kernel-module` build job (fails on any compiler warning).
- **Fixed (MSRV):** the pre-existing 1.75 test-profile E0382 in `facade.rs` — the MSRV test job is green again.

**Operator-gated release steps (out of scope for this commit):** the git tag `v2.3.0`, the release re-cut, the `ramsleuth-bin` tarball sha256 re-finalization, and the AUR resubmits (`ramsleuth`, `ramsleuth-bin`, `ramsleuth-intel-dkms`).

## v2.2.1 (2026-09-22)

**TUI parity** — the terminal TUI now matches the GUI:

- **16-key contract** (case-insensitive, modifiers ignored): bench `b` / memory-only `m` / burn-in `x` / cancel `c`; graphs `g`; settings `t`; requirements `d`; JSON export `e`; poll `p` / capacity units `u` / clock units `k` / refresh `a` / graphs window `w` — plus the original refresh `r` / snapshot `s` / quit `q`.
- **5-series graphs overlay** over a 1800-sample ring.
- **Requirements strip** — a mirror of the GUI `SETUP` (presence-driven prerequisites).
- **JSON export** to `$HOME` (`{ telemetry, bench }`) — F3 parity.

**CI portability:** vendor-conditional telemetry tests + graceful MCHBAR `BAR5=0` degradation (Intel + AMD + virtualized hosts).

**TUI display:** bare grey `N/A` (no parse-error detail); lighter-grey titles.

## v2.2.0

The one-click setup / first-run release: the in-app **Set up RamSleuth** (one polkit pass: daemon enable + start, `ramsleuth` group join, current-session socket ACL — no re-login), the offline-vendored `ryzen-smu` DKMS extra, and the establishment of the **15-artifact release contract** (the published tarball + `.sha256`).

## v2.1.1

Published milestone (release tag `v2.1.1`).

## v2.1.0

Published milestone (release tag `v2.1.0`).

## v2.0.0

Published milestone (release tag `v2.0.0`) — the RamSleuth v2 launch.
