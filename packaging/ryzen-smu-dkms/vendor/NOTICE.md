# NOTICE — vendored `ryzen_smu` kernel-module source

**Upstream:** https://github.com/amkillam/ryzen_smu
**Pinned commit:** `d2983668300dd2a598e5a7dc40e71ce0678cc270`
("Fix cpuid include on 7.2+ kernels (#53)")
**Vendored:** 2026-09-20 (RamSleuth Cycle 21, chunk C21-07)
**Extended:** 2026-11-09 (APOB support for sIOD platforms — chunk/apob-kernel)

## Contents

The eight files under `ryzen-smu/`:

- `LICENSE`, `dkms.conf` — unmodified, byte-identical copies of the upstream
  files at the pinned commit.
- `smu.c`, `smu.h` — upstream files with later RamSleuth fixes applied
  (3990X PM-table misclassification fix, F-01/F-03/F-06/F-09 forum
  follow-ups).
- `drv.c` — upstream file extended with APOB sysfs exposure
  (`apob_available`, `apob_size`, `apob_version`, `apob` binary attribute).
- `Makefile` — upstream Makefile with `apob.o` added to `$(MOD)-objs`.
- `apob.c`, `apob.h` — NEW RamSleuth-authored files implementing APOB
  (AGESA Pre-Output Buffer) discovery: scans three candidate physical
  addresses for the "APOB" signature, ioremapps the table, and exposes the
  raw blob via the sysfs binary attribute. No parsing is done in the kernel;
  all format knowledge lives in the userspace Rust parser
  (`crates/ramsleuth-telemetry/src/amd_apob.rs`).

`SUMS.sha256` records the sha256 of each of the eight files. The files are
**frozen**: no later change may edit them without updating `SUMS.sha256`
and this NOTICE (anti-contamination — the same pin rule the install helper
enforces at fetch time, `scripts/install-ryzen-smu-dkms.sh`).

## License (separate work)

This vendored source is **GPL-2.0** (`ryzen-smu/LICENSE`, the verbatim
upstream text). It is a **separate work** from RamSleuth, which is
MIT-licensed: it is **never** compiled into, linked with, or distributed
as part of any MIT-licensed RamSleuth binary. It is built **only** by
DKMS in `/usr/src/ryzen_smu-<version>` on an AMD target, producing the
standalone `ryzen_smu` kernel module. No MIT-licensed file depends on
it; the sole in-repo consumer is the install helper, which verifies the
`SUMS.sha256` manifest before building. The `ryzen-smu-dkms` AUR
package lists both licenses (`MIT GPL-2.0-only`); the other RamSleuth
packages install no file from this directory.

The RamSleuth-authored APOB additions (`apob.c`, `apob.h`, and the
`drv.c`/`Makefile` modifications) are GPL-2.0 by virtue of being compiled
into the kernel module.
