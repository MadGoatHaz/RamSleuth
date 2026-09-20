# NOTICE — vendored `ryzen_smu` kernel-module source

**Upstream:** https://github.com/amkillam/ryzen_smu
**Pinned commit:** `d2983668300dd2a598e5a7dc40e71ce0678cc270`
("Fix cpuid include on 7.2+ kernels (#53)")
**Vendored:** 2026-09-20 (RamSleuth Cycle 21, chunk C21-07)

## Contents

The six files under `ryzen-smu/` (`LICENSE`, `Makefile`, `dkms.conf`,
`drv.c`, `smu.c`, `smu.h`) are unmodified, byte-identical copies of the
upstream files at the pinned commit; `SUMS.sha256` records the sha256 of
each. The files are **frozen**: no later change may edit them
(anti-contamination — the same pin rule the install helper enforces at
fetch time, `scripts/install-ryzen-smu-dkms.sh`).

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
