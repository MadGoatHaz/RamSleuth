# Changelog

All notable per-release changes to RamSleuth. Newest first.

**Versioning policy.** The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml`; every member crate inherits it. A release = a version bump + the git tag `v<ver>` + the release workflow (`.github/workflows/release.yml`) publishing the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`. The AUR packages (`ramsleuth`, `ramsleuth-bin`, and the `ramsleuth-intel-dkms` extra) track this versioning and are maintained at the same pace as the project.

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
