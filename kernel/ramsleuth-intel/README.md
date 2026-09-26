# ramsleuth_intel

An original, in-repo RamSleuth creation (no upstream project): an
out-of-tree kernel module for RamSleuth's live Intel memory controller
(IMC) telemetry. It probes the host bridge (PCI `0000:00:00.0`), decodes
**MCHBAR** from PCI config space, maps the 64 KiB window, and publishes
the raw IMC registers as world-readable (`0444`) sysfs attributes under
`/sys/kernel/ramsleuth_intel/`.

The module does **no decoding**: every register attribute is the raw
32-bit value (little-endian, one line, `0x%08x`). The RamSleuth
userspace reader (`ramsleuth-telemetry`) decodes the bitfields per CPU
generation, so one module spans the supported client generations.
Nothing is ever written to the register space (read-only `ioread32`).

## When it loads

The kobject is created **only on a fully successful probe**:

1. PCI `0000:00:00.0` exists and is an **Intel** host bridge
   (vendor `0x8086`),
2. **MCHBAR_EN** is set (bit 0 of config dword `0x48`),
3. the masked MCHBAR base (`raw & 0x0000007F_FFFF_F000`) is non-zero,
4. the 64 KiB `ioremap` succeeds.

Any failure leaves **no kobject** behind and a clean `-E*` return.
**On non-Intel hardware (e.g. an AMD dev box) a load failure is
expected and correct** — the module is Intel-only by design, and the
vendor gate rejects the AMD host bridge with `-ENODEV` before MCHBAR is
even decoded.

## Build and load

One-off, manual:

    make                     # ramsleuth_intel.ko vs the running kernel
    make KVER=<ver>          # ...or a specific kernel's headers
    sudo insmod ramsleuth_intel.ko
    sudo rmmod ramsleuth_intel

The recommended path is DKMS (the `ramsleuth-intel-dkms` AUR extra /
`scripts/install-intel-dkms.sh` helper do this for you). Manual DKMS:

    V=$(git show -s --format=%s /dev/null 2>/dev/null; echo 2.4.5)  # ramsleuth workspace version
    sudo cp -r . /usr/src/ramsleuth_intel-$V
    sudo sed -i "s/@VERSION@/$V/" /usr/src/ramsleuth_intel-$V/dkms.conf
    sudo dkms add -m ramsleuth_intel/$V
    sudo dkms build -m ramsleuth_intel/$V
    sudo dkms install -m ramsleuth_intel/$V
    sudo modprobe ramsleuth_intel

`AUTOINSTALL=yes` in `dkms.conf` makes DKMS rebuild the module on
kernel updates automatically. `dkms status` shows the bookkeeping.

## Frozen sysfs interface

24 attributes, all read-only (`0444`), under `/sys/kernel/ramsleuth_intel/`.
This list is the frozen contract the Rust reader (INTEL-03) parses:
name, source, and line format are stable.

| Attribute | Source | Line format |
|---|---|---|
| `mchbar_base` | PCI cfg `0x48`/`0x4C`, masked `0x0000007F_FFFF_F000` | 16 hex digits, no prefix — e.g. `00000000fed10000` |
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

Channel 1 mirrors channel 0 across the `0x400` dual-controller stride;
the turnaround quartet sits at `+0x20`…`+0x2C` inside each channel
block. The five `mad_*` attributes are global (not per-channel)
IMC registers: `mad_inter_channel` (channel mode / interleave
configuration), `mad_intra_ch0`/`mad_intra_ch1` (channel 0/1
rank/geometry), and `mad_dimm_ch0`/`mad_dimm_ch1` (channel 0/1 DIMM
capacity).

## Known limitation: Secure Boot / lockdown

On UEFI Secure Boot hosts (integrity lockdown), the kernel blocks both
`/dev/mem` **and** unsigned out-of-tree modules. The only way to load
this module there is to MOK-enroll it first (`mokutil --import
ramsleuth_intel.ko`). This is the same class of limitation the AMD
`ryzen_smu` extra carries. When the module cannot be loaded, RamSleuth
degrades gracefully: the Intel section reads `N/A (DriverMissing)`,
exit 0, no panic.

## License

GPL-2.0 (see the SPDX header in `ramsleuth_intel.c`).
