# Changelog

All notable per-release changes to RamSleuth. Newest first.

**Versioning policy.** The single source of truth for the version is `[workspace.package].version` in the root `Cargo.toml`; every member crate inherits it. A release = a version bump + the git tag `v<ver>` + the release workflow (`.github/workflows/release.yml`) publishing the binary tarball `ramsleuth-<ver>-x86_64.tar.zst` + its `.sha256`. The AUR packages (`ramsleuth`, `ramsleuth-bin`) track this versioning and are maintained at the same pace as the project.

## v2.3.0 (2026-09-23)

**Intel full parity** — live DRAM subtimings on Intel, matching the AMD experience:

- **Added:** live Intel DRAM subtimings (v1 / Tier-1: Skylake, Kaby Lake, Coffee Lake, Comet Lake — DDR4). The new `ramsleuth_intel` out-of-tree kernel module exposes the raw IMC registers under `/sys/kernel/ramsleuth_intel/`, provisioned by the `ramsleuth-intel-dkms` AUR extra, the `install-intel-dkms.sh` operator helper, and the now-vendor-aware `ramsleuth-setup.sh`. Telemetry is sysfs-first with a `/dev/mem` fallback (used on `DriverMissing` only).
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
