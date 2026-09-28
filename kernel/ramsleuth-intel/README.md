# ramsleuth_intel

An original, in-repo RamSleuth creation (no upstream project): a read-only
out-of-tree kernel module for RamSleuth's live Intel memory controller (IMC)
telemetry. It probes the host bridge (PCI `0000:00:00.0`), decodes **MCHBAR**
from PCI config space, `ioremap`s the MCHBAR window, and publishes the raw
IMC / MAD / capability registers as world-readable (`0444`) sysfs attributes
under `/sys/kernel/ramsleuth_intel/`. When the module is absent, RamSleuth
degrades gracefully (Intel section `N/A (DriverMissing)`, exit 0, no panic);
the `/dev/mem` fallback path remains available where unblocked.

## What it is

A read-only out-of-tree module. It maps the Intel host-bridge MCHBAR window
and exposes the raw IMC/MAD/capability registers as sysfs. The module is
generation-agnostic (raw words only); all decode (channel mode, ECC,
subtimings) happens in userspace.

The 24 raw register attributes each emit the 32-bit register exactly as read
— one line, `0x%08x`, little-endian — plus two MCHBAR diagnostic attributes
and `capid0a` (below). Nothing is ever written to the register space (read-
only `ioread32`), so the module is safe to load on any Intel client host.

## When it loads (probe sequence)

The `ramsleuth_intel` kobject is created **only if all of the following
succeed**; every failure path unmaps/drops refs and leaves no kobject:

1. **Find the host bridge** at PCI `0000:00:00.0` (absent → `-ENODEV`).
2. **Read MCHBAR** from config dwords `0x48` (low) / `0x4C` (high) = the
   64-bit MCHBAR. **`0x40` is the EPBAR — a known trap; the IMC window is
   `0x48`.**
3. **Keep a module-lifetime `pci_dev` ref** to the host bridge (so `capid0a`
   can read its config space on demand — `/sys/.../config` is
   64-byte-truncated).
4. **Vendor gate**: `vendor == 0x8086` (Intel-only by design; the module
   loads a no-op on AMD — `-ENODEV`, which is expected and correct).
5. **Window size**: 256 KiB only for device IDs in `tier3_device_ids[]` —
   a table that ships **empty** (OQ-14), so **every released build maps the
   default 64 KiB**.
6. **Require `MCHBAR_EN`** (bit 0 of the raw value) set and a nonzero base
   (`raw & 0x7FFFFFF000`, bits 38:16, 64 KiB-aligned); a zero base means
   unpopulated/virtualized → `-ENODEV`.
7. **`ioremap(base, map_size)`**, then create the `ramsleuth_intel` kobject.

## MCHBAR decode

The 64-bit raw value (config `0x48` low / `0x4C` high):

| Bit(s) | Field | Meaning |
|---|---|---|
| `[0]` | `MCHBAR_EN` | window enable |
| `[15:1]` | reserved | 0 |
| `[38:16]` | physical base | 64 KiB-aligned window base |

Worked example: an enabled Skylake raw value `0x0000_0000_FED1_0001` →
`MCHBAR_EN=1`, base `0xFED10000`.

## The 25 attributes (what each register is)

25 read-only attributes, all `0444`, under `/sys/kernel/ramsleuth_intel/`
(the frozen contract the Rust reader parses — name, source, and line format
are stable):

- `mchbar_base` / `mchbar_enabled` — diagnostics: the masked MCHBAR physical
  base in 16 hex digits, no prefix (e.g. `00000000fed10000`), and a constant
  `1` by construction (the kobject only exists while the BAR is enabled).
- `mcbios_req` @ MCHBAR+`0x5E00` (MC_BIOS_REQ): `[7:0]` CLK_RATIO, `[8]`
  REF_CLK (0=133.3333 MHz, 1=100 MHz), `[17:16]` GEAR (Rocket Lake=Gear2,
  Alder Lake+=Gear4), `[31]` RUN_BUSY. **MCLK = ratio × refclk (no ÷2);
  MT/s = 2 × MCLK.**
- ch0/ch1 `tc_dbp/tc_rap/tc_rfp/tc_rap2/tc_rdrd/tc_rdwr/tc_wrrd/tc_wrwr`
  @ `+0x4000` / `+0x4400` (stride `0x400`) — the Tier-1 TC block:
  - **TC_DBP**: `[5:0]` tCL, `[13:8]` tCWL, `[21:16]` tRCD, `[29:24]` tRP.
  - **TC_RAP**: `[5:0]` tRRD_S, `[11:6]` tRTP, `[15:12]` tCKE, `[23:16]`
    tFAW, `[31:24]` tRAS.
  - **TC_RFP**: `[10:0]` tRFC, `[27:16]` tREFI.
  - **TC_RAP2**: `[5:0]` tRRD_L, `[13:8]` tWR.
  - The four turnaround registers (`+0x20..+0x2C`) = 4×6-bit
    same-bank-group / diff-group / diff-rank / diff-DIMM.
  - Units = integer DRAM clock cycles. **tRC has no register — userspace
    synthesizes tRC = tRAS + tRP.**
- `mad_inter_channel` / `mad_intra_ch0` / `mad_intra_ch1` / `mad_dimm_ch0`
  / `mad_dimm_ch1` @ `+0x5000..+0x5010` (MAD block).
  **MAD_INTER_CHANNEL**: `[1:0]` channel config (00 dual-symmetric / 01
  dual-flex / 10 single / 11 reserved), `[2]` CH_L_MAP, `[7:4]`
  HASH_LSB_MASK. **MAD_DIMM_CH0/1**: slot-size words (DIMM0 = bits `[5:0]`,
  DIMM1 = bits `[21:16]`, 1 GiB index). Feed the userspace channel-mode
  population cross-check.
- `capid0a` — host-bridge **CONFIG** offset `0xE4` (CAPID0_A), read
  in-kernel via `pci_read_config_dword` (because `/sys/.../config` is
  64-byte-truncated). Bit 17 = `ECC_DIS` (set → Not Capable, clear →
  Capable); `0xffffffff` sentinel when unreadable.

| Attribute | Source | Line format |
|---|---|---|
| `mchbar_base` | PCI cfg `0x48`/`0x4C`, masked | 16 hex digits, no prefix — e.g. `00000000fed10000` |
| `mchbar_enabled` | MCHBAR_EN (bit 0) | `1` (constant while the kobject exists) |
| `mcbios_req` | MCHBAR + `0x5E00` | `0x%08x` |
| `ch0_tc_dbp` | MCHBAR + `0x4000` | `0x%08x` |
| `ch0_tc_rap` | MCHBAR + `0x4004` | `0x%08x` |
| `ch0_tc_rfp` | MCHBAR + `0x4008` | `0x%08x` |
| `ch0_tc_rap2` | MCHBAR + `0x400C` | `0x%08x` |
| `ch0_tc_rdrd` | MCHBAR + `0x4020` | `0x%08x` |
| `ch0_tc_rdwr` | MCHBAR + `0x4024` | `0x%08x` |
| `ch0_tc_wrrd` | MCHBAR + `0x4028` | `0x%08x` |
| `ch0_tc_wrwr` | MCHBAR + `0x402C` | `0x%08x` |
| `ch1_tc_dbp` | MCHBAR + `0x4400` | `0x%08x` |
| `ch1_tc_rap` | MCHBAR + `0x4404` | `0x%08x` |
| `ch1_tc_rfp` | MCHBAR + `0x4408` | `0x%08x` |
| `ch1_tc_rap2` | MCHBAR + `0x440C` | `0x%08x` |
| `ch1_tc_rdrd` | MCHBAR + `0x4420` | `0x%08x` |
| `ch1_tc_rdwr` | MCHBAR + `0x4424` | `0x%08x` |
| `ch1_tc_wrrd` | MCHBAR + `0x4428` | `0x%08x` |
| `ch1_tc_wrwr` | MCHBAR + `0x442C` | `0x%08x` |
| `mad_inter_channel` | MCHBAR + `0x5000` | `0x%08x` |
| `mad_intra_ch0` | MCHBAR + `0x5004` | `0x%08x` |
| `mad_intra_ch1` | MCHBAR + `0x5008` | `0x%08x` |
| `mad_dimm_ch0` | MCHBAR + `0x500C` | `0x%08x` |
| `mad_dimm_ch1` | MCHBAR + `0x5010` | `0x%08x` |
| `capid0a` | host-bridge PCI cfg `0xE4` (read in-kernel — the `/sys` config space is 64-byte truncated) | `0x%08x` (`0xffffffff` sentinel on unreadable) |

Channel 1 mirrors channel 0 across the `0x400` dual-controller stride; the
turnaround quartet sits at `+0x20`…`+0x2C` inside each channel block. The
five `mad_*` attributes are global (not per-channel) IMC registers. The
single `capid0a` attribute is read from PCI config space rather than the
MCHBAR window — the input to the userspace ECC decode.

## Tier 1 / 2 / 3 (and the honest gap)

The module is generation-agnostic; tier logic lives in userspace
`intel_gen::profile_for`:

- **Tier 1** = Skylake / Kaby / Coffee / Comet (64 KiB, 2-channel, no gear)
- **Tier 2** = Rocket Lake (64 KiB, Gear2)
- **Tier 3** = Alder / Raptor / Meteor / Arrow (256 KiB, DDR5 4-subchannel,
  MCL fallback, Gear4)

**Honest gap:** the module exposes only the Tier-1-shaped surface (the two
`0x4000/0x4400` channel blocks). It does **NOT** expose the Tier-3 ch2/ch3
mirror blocks (`0x4800/0x4C00`), the native MCL blocks (`0xD000/0xD800`), or
`mad_dimm_ch2/3` (`0x5060/0x5064`); userspace defines those extra attributes
and degrades them to `None` when absent. So on a Tier-3 host a loaded module
yields **LESS** decode coverage than no module (the DDR5 4-subchannel +
MCL-fallback path exists only via the `/dev/mem` 256 KiB path). OQ-14: the
in-kernel 256 KiB device-ID table is empty pending live-box verification, so
every released build maps 64 KiB.

## Building + installing (DKMS)

- `Makefile`: out-of-tree `obj-m`; `KDIR=/lib/modules/$(KVER)/build`;
  auto-detects clang kernels (`CONFIG_CC_IS_CLANG` → `LLVM=1`).
- `dkms.conf`: `PACKAGE_VERSION` sed'd by the helper; `AUTOINSTALL=yes`
  (rebuild on kernel updates); `DEST_MODULE_LOCATION=/extra` is a no-op on
  DKMS 3.x (the module deploys to `updates/dkms/`, no depmod.d override).
- The helper (`scripts/install-intel-dkms.sh`): idempotent + self-healing
  (removes every registered version then re-adds; `dkms install --force`
  over a residual `.ko`); resolves source offline (in-repo → installed
  `/usr/share/ramsleuth-intel-dkms/src/`); sha256-provenances the staged
  files; auto-continues in non-TTY; writes
  `/etc/modules-load.d/ramsleuth_intel.conf` only on a successful Intel
  load.
- **Secure Boot:** the one-click detects SB, signs the module with a
  persistent key at `/var/lib/ramsleuth/ramsleuth-intel-signing/` (via an
  `/etc/dkms/framework.conf.d/` drop-in — DKMS 3.4.3 doesn't read
  per-module `SIGN` in dkms.conf), runs `mokutil --import`, and exits 10 →
  the GUI's amber "one step left" (reboot → MOK enroll → re-click).
- Non-Intel-safe: builds on AMD; load is a no-op (`-ENODEV` vendor gate);
  helper exits 0. CI fails on any compiler warning.
- **Version:** the module's `MODULE_VERSION` tracks the release (2.4.13);
  the DKMS version is the workspace version for a source-checkout install
  and the documented `2.2.1` fallback for a package install (the installed
  copy has no workspace `Cargo.toml`).

Manual one-off (the recommended path is the DKMS helper):

    make                     # ramsleuth_intel.ko vs the running kernel
    make KVER=<ver>          # ...or a specific kernel's headers
    sudo insmod ramsleuth_intel.ko
    sudo rmmod ramsleuth_intel

## Known hardware limitations

- **Low-power Intel SoCs (N100 class):** no client IMC timing registers /
  no MCHBAR decode / SPD not bound → the module loads (host bridge present)
  but the userspace readout is a clean `N/A (unsupported hardware)` —
  expected, not an error.
- **Meteor/Arrow Lake tile routing (OQ-5):** where the physical IMC sits
  behind the secondary SoC-tile range the module doesn't expose → clean
  N/A, never a fabricated decode.
- **Secure Boot / integrity lockdown:** handled by the one-click signing +
  MOK flow above.
- **ECC-capable "consumer" parts:** the `0x191F` Xeon-E3-class host bridge
  on some client boards is ECC-capable — why `capid0a` is keyed on the
  register bit, never the CPU string.

## License

GPL-2.0 (see the SPDX header in `ramsleuth_intel.c`).
