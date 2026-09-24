# Changelog

All notable per-release changes to RamSleuth. Newest first.

**Versioning policy.** The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml`; every member crate inherits it. A release = a version bump + the git tag `v<ver>` + the release workflow (`.github/workflows/release.yml`) publishing the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`. The AUR packages (`ramsleuth`, `ramsleuth-bin`, and the `ramsleuth-intel-dkms` extra) track this versioning and are maintained at the same pace as the project.

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
