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
and exposes the raw IMC/MAD/capability registers as sysfs. The module
publishes **raw words only** — all decode (channel mode, ECC, subtimings)
happens in userspace — and it is **generation-aware only at the probe
level**: the mapped window size and the MAD register base are keyed on the
CPU generation (see "When it loads" below), so one module spans the
supported client generations. It requires **Linux ≥ 5.13** (its gen-keyed
selection includes `<asm/cpuid/api.h>` — see "Building + installing").

The raw register attributes each emit the 32-bit register exactly as read
— one line, `0x%08x`, little-endian — plus the two MCHBAR diagnostic
attributes and `capid0a` (25 core attributes, listed below; on the
12th-gen+ family **two** gen-gated groups add 54 more — the `alk_d4_tc`
group (20 attributes, the Alder / Raptor Lake DDR4 `0xE000`/`0x1E000`
per-MC TC blocks) and the `tier3_extended` group (34 attributes, the
`0xD000`/`0xD800` native MCL blocks, the `0x4800`/`0x4C00` DDR5 ch2 / ch3
mirror blocks, and the `mad_dimm_ch2`/`mad_dimm_ch3` raws) — see "The
12th-gen+ attribute groups" below). Nothing is ever written to the register
space (read-only `ioread32`), so the module is safe to load on any Intel
client host. On a 12th-gen+ Alder / Raptor DDR4 box the full surface is
**79** attributes; on a DDR5-only box (Meteor / Arrow Lake) **59**; on a
pre-12th-gen box the core **25**.

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
5. **Window size + IMC base (gen-keyed)**: the module computes the effective
   8-bit CPU model from `cpuid_eax(1)` (`(ext << 4) | base`, the SDM rule —
   the same computation the userspace `cpuid.rs` performs) and keys both the
   mapped window and the MAD register base on it: **12th-gen+** (Alder /
   Raptor / Meteor / Arrow / Lunar Lake — `is_12th_gen_plus`) → the
   **256 KiB** window, with the five global MAD channel/geometry registers
   read at `MCHBAR + 0xd800`; everything else (pre-12th-gen) → the
   **64 KiB** window and `MCHBAR + 0x5000`. (The pre-fix module read
   `0x5000` on *every* box, so a 12th-gen+ host's MAD registers were
   unmapped — `0x0` / `0xFFFF_FFFF` — and the channel mode misreported;
   this is the fix. The base is *not* keyed on the PCI Device-ID: the
   research Device-ID ranges overlap across generations and are not
   per-generation, so the CPU model is the unambiguous key.)
6. **Require `MCHBAR_EN`** (bit 0 of the raw value) set and a nonzero base
   (`raw & 0x7FFFFFF000`, bits 38:16, 64 KiB-aligned); a zero base means
   unpopulated/virtualized → `-ENODEV`.
7. **`ioremap(base, map_size)`**, then create the `ramsleuth_intel` kobject
   with the core 25 attributes, and — on the 12th-gen+ family — the two
   gen-gated groups: `alk_d4_tc` (Alder / Raptor Lake DDR4 only) and
   `tier3_extended` (the whole 12th-gen+ family) — see "The 12th-gen+
   attribute groups" below.

## MCHBAR decode

The 64-bit raw value (config `0x48` low / `0x4C` high):

| Bit(s) | Field | Meaning |
|---|---|---|
| `[0]` | `MCHBAR_EN` | window enable |
| `[15:1]` | reserved | 0 |
| `[38:16]` | physical base | 64 KiB-aligned window base |

Worked example: an enabled Skylake raw value `0x0000_0000_FED1_0001` →
`MCHBAR_EN=1`, base `0xFED10000`.

## The 25 core attributes (what each register is)

The **core** set — 25 read-only attributes, all `0444`, under
`/sys/kernel/ramsleuth_intel/`, present on every supported host (the frozen
contract the Rust reader parses — name, source, and line format are stable;
the 12th-gen+ groups below are additive):

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
  / `mad_dimm_ch1` @ the gen-keyed IMC base + `0x00/0x04/0x08/0x0C/0x10`
  (`MCHBAR + 0x5000` on pre-12th-gen, `MCHBAR + 0xd800` on the 12th-gen+
  family — step 5).
  **MAD_INTER_CHANNEL**: `[2:0]` is the **DDR_TYPE** memory-technology field
  (userspace decodes `000`=DDR4, `001`=DDR3/3L, `010`=LPDDR4, `011`=LPDDR5,
  `100`=DDR5) — it is **not** a channel-mode selector. **MAD_DIMM_CH0/1**:
  per-channel slot-size words — `DIMM_L_SIZE` at bits `[6:0]` +
  `DIMM_S_SIZE` at bits `[22:16]`, each a **7-bit index in 0.5 GiB
  increments** (a 16 GiB DIMM is index `32` = `32 × 0.5`); a slot index of
  `0` means unpopulated. Userspace sums the two slots per channel and derives
  the channel mode from the resulting capacity (the 4-tier trust
  cross-check), not from a firmware mode bit.
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
| `mad_inter_channel` | MCHBAR + IMC base + `0x00` (base `0x5000` pre-12th-gen / `0xd800` 12th-gen+) | `0x%08x` |
| `mad_intra_ch0` | MCHBAR + IMC base + `0x04` | `0x%08x` |
| `mad_intra_ch1` | MCHBAR + IMC base + `0x08` | `0x%08x` |
| `mad_dimm_ch0` | MCHBAR + IMC base + `0x0C` | `0x%08x` |
| `mad_dimm_ch1` | MCHBAR + IMC base + `0x10` | `0x%08x` |
| `capid0a` | host-bridge PCI cfg `0xE4` (read in-kernel — the `/sys` config space is 64-byte truncated) | `0x%08x` (`0xffffffff` sentinel on unreadable) |

Channel 1 mirrors channel 0 across the `0x400` dual-controller stride; the
turnaround quartet sits at `+0x20`…`+0x2C` inside each channel block. The
five `mad_*` attributes are global (not per-channel) IMC registers. The
single `capid0a` attribute is read from PCI config space rather than the
MCHBAR window — the input to the userspace ECC decode.

## The 12th-gen+ attribute groups

On the 12th-gen+ family (the same generation that selects the 256 KiB
window, step 5) the module creates **two** gen-gated attribute groups in the
same kobject directory — 54 more read-only `0444` attributes on top of the
core 25. On a pre-12th-gen box neither group is created; the surface is the
core 25 (the `0x4000`/`0x4400` Tier-1 mirror blocks stay for Tier-1
compatibility). All are raw words only — no decoding (the userspace reader
owns the bitfields).

### `alk_d4_tc` group (20 attributes — Alder / Raptor Lake DDR4 only)

Gated on the **DDR4** 12th-gen+ subset (`is_alder_raptor_ddr4` — Alder /
Raptor Lake; the DDR5-only parts — Meteor / Arrow / Lunar Lake — have no
DDR4 TC surface, so this group is not created for them). These are the
per-MC TC timing registers of the **DDR4** Alder/Raptor layout (Intel
12th-Gen Datasheet Vol. 2, §3.2.66-3.2.87):

- **The TC blocks** — `alk_d4_mc0_tc_*` @ MCHBAR+`0xE000` and
  `alk_d4_mc1_tc_*` @ MCHBAR+`0x1E000` (`0xE000` + the `0x10000`
  inter-controller stride), 10 words each. **`TC_PRE` is a 64-bit
  register**, exposed as two 32-bit words — `tc_pre_lo` @ `+0x00`
  (tRP / tRDPRE / tPPD) and `tc_pre_hi` @ `+0x04` (tWRPRE / tRAS / tRCD) —
  since x86 MMIO is 32-bit granular. These are **not** the
  `0x4000`/`0x4400` mirror window (which returns `0xFFFFFFFF` on DDR4).

### `tier3_extended` group (34 attributes — all 12th-gen+)

Gated on the **whole** 12th-gen+ family (`is_12th_gen_plus` — every
Alder / Raptor / Meteor / Arrow / Lunar Lake part needs these). This group
supplies the data the userspace reader needs for the Tier-3 decode that the
core 25 alone cannot provide:

- **The native MCL blocks** — `mcl0_tc_*` @ MCHBAR+`0xD000` and
  `mcl1_tc_*` @ MCHBAR+`0xD800` (the Tier-3 uncore fallback), 8 words each
  (`pre` / `act` / `act2` / `wtr` / `rfp` / `rfp2` / `rdrd` / `wrwr`). On
  the 12th-gen+ family the MCL1 block shares its `0xD800` page with the
  gen-keyed MAD block, so the overlapping words are the same raw registers
  (the userspace decode disambiguates by generation).
- **The DDR5 ch2 / ch3 mirror blocks** — `ch2_tc_*` @ MCHBAR+`0x4800` and
  `ch3_tc_*` @ MCHBAR+`0x4C00` (8 words each — `dbp` / `rap` / `rfp` /
  `rap2` / `rdrd` / `rdwr` / `wrrd` / `wrwr`), the legacy-compatibility
  mirror window the 4-subchannel DDR5 decode reads.
- **The `mad_dimm_ch2` / `mad_dimm_ch3` raws** — at the gen-keyed MAD base +
  `0x60` / `+0x64` (the `MAD_DIMM_CH2_INTRA` / `CH3_INTRA` words; **not**
  the `+0x0C`/`+0x10` stride-continued positions, which on the `0xD800`
  page are occupied by the MCL1 `TC_RFP` / `TC_RFP2` words — the N-02
  collision-free placement). These are the OQ-11 DDR5-subchannel geometry
  input.

**`alk_d4_tc` group (20 attributes):**

| Attribute | Source | Line format |
|---|---|---|
| `alk_d4_mc0_tc_pre_lo` | MCHBAR + `0xE000` + `0x00` | `0x%08x` |
| `alk_d4_mc0_tc_pre_hi` | MCHBAR + `0xE000` + `0x04` | `0x%08x` |
| `alk_d4_mc0_tc_act` | MCHBAR + `0xE000` + `0x08` | `0x%08x` |
| `alk_d4_mc0_tc_rdrd` | MCHBAR + `0xE000` + `0x0C` | `0x%08x` |
| `alk_d4_mc0_tc_rdwr` | MCHBAR + `0xE000` + `0x10` | `0x%08x` |
| `alk_d4_mc0_tc_wrrd` | MCHBAR + `0xE000` + `0x14` | `0x%08x` |
| `alk_d4_mc0_tc_wrwr` | MCHBAR + `0xE000` + `0x18` | `0x%08x` |
| `alk_d4_mc0_tc_odt` | MCHBAR + `0xE000` + `0x70` | `0x%08x` |
| `alk_d4_mc0_tc_rfp` | MCHBAR + `0xE000` + `0x438` | `0x%08x` |
| `alk_d4_mc0_tc_rftp` | MCHBAR + `0xE000` + `0x43C` | `0x%08x` |
| `alk_d4_mc1_tc_pre_lo` | MCHBAR + `0x1E000` + `0x00` | `0x%08x` |
| `alk_d4_mc1_tc_pre_hi` | MCHBAR + `0x1E000` + `0x04` | `0x%08x` |
| `alk_d4_mc1_tc_act` | MCHBAR + `0x1E000` + `0x08` | `0x%08x` |
| `alk_d4_mc1_tc_rdrd` | MCHBAR + `0x1E000` + `0x0C` | `0x%08x` |
| `alk_d4_mc1_tc_rdwr` | MCHBAR + `0x1E000` + `0x10` | `0x%08x` |
| `alk_d4_mc1_tc_wrrd` | MCHBAR + `0x1E000` + `0x14` | `0x%08x` |
| `alk_d4_mc1_tc_wrwr` | MCHBAR + `0x1E000` + `0x18` | `0x%08x` |
| `alk_d4_mc1_tc_odt` | MCHBAR + `0x1E000` + `0x70` | `0x%08x` |
| `alk_d4_mc1_tc_rfp` | MCHBAR + `0x1E000` + `0x438` | `0x%08x` |
| `alk_d4_mc1_tc_rftp` | MCHBAR + `0x1E000` + `0x43C` | `0x%08x` |

**`tier3_extended` group (34 attributes):**

| Attribute | Source | Line format |
|---|---|---|
| `mcl0_tc_pre` | MCHBAR + `0xD000` + `0x00` | `0x%08x` |
| `mcl0_tc_act` | MCHBAR + `0xD000` + `0x04` | `0x%08x` |
| `mcl0_tc_act2` | MCHBAR + `0xD000` + `0x08` | `0x%08x` |
| `mcl0_tc_wtr` | MCHBAR + `0xD000` + `0x10` | `0x%08x` |
| `mcl0_tc_rfp` | MCHBAR + `0xD000` + `0x14` | `0x%08x` |
| `mcl0_tc_rfp2` | MCHBAR + `0xD000` + `0x18` | `0x%08x` |
| `mcl0_tc_rdrd` | MCHBAR + `0xD000` + `0x20` | `0x%08x` |
| `mcl0_tc_wrwr` | MCHBAR + `0xD000` + `0x28` | `0x%08x` |
| `mcl1_tc_pre` | MCHBAR + `0xD800` + `0x00` | `0x%08x` |
| `mcl1_tc_act` | MCHBAR + `0xD800` + `0x04` | `0x%08x` |
| `mcl1_tc_act2` | MCHBAR + `0xD800` + `0x08` | `0x%08x` |
| `mcl1_tc_wtr` | MCHBAR + `0xD800` + `0x10` | `0x%08x` |
| `mcl1_tc_rfp` | MCHBAR + `0xD800` + `0x14` | `0x%08x` |
| `mcl1_tc_rfp2` | MCHBAR + `0xD800` + `0x18` | `0x%08x` |
| `mcl1_tc_rdrd` | MCHBAR + `0xD800` + `0x20` | `0x%08x` |
| `mcl1_tc_wrwr` | MCHBAR + `0xD800` + `0x28` | `0x%08x` |
| `ch2_tc_dbp` | MCHBAR + `0x4800` + `0x00` | `0x%08x` |
| `ch2_tc_rap` | MCHBAR + `0x4800` + `0x04` | `0x%08x` |
| `ch2_tc_rfp` | MCHBAR + `0x4800` + `0x08` | `0x%08x` |
| `ch2_tc_rap2` | MCHBAR + `0x4800` + `0x0C` | `0x%08x` |
| `ch2_tc_rdrd` | MCHBAR + `0x4800` + `0x20` | `0x%08x` |
| `ch2_tc_rdwr` | MCHBAR + `0x4800` + `0x24` | `0x%08x` |
| `ch2_tc_wrrd` | MCHBAR + `0x4800` + `0x28` | `0x%08x` |
| `ch2_tc_wrwr` | MCHBAR + `0x4800` + `0x2C` | `0x%08x` |
| `ch3_tc_dbp` | MCHBAR + `0x4C00` + `0x00` | `0x%08x` |
| `ch3_tc_rap` | MCHBAR + `0x4C00` + `0x04` | `0x%08x` |
| `ch3_tc_rfp` | MCHBAR + `0x4C00` + `0x08` | `0x%08x` |
| `ch3_tc_rap2` | MCHBAR + `0x4C00` + `0x0C` | `0x%08x` |
| `ch3_tc_rdrd` | MCHBAR + `0x4C00` + `0x20` | `0x%08x` |
| `ch3_tc_rdwr` | MCHBAR + `0x4C00` + `0x24` | `0x%08x` |
| `ch3_tc_wrrd` | MCHBAR + `0x4C00` + `0x28` | `0x%08x` |
| `ch3_tc_wrwr` | MCHBAR + `0x4C00` + `0x2C` | `0x%08x` |
| `mad_dimm_ch2` | gen-keyed MAD base + `0x60` | `0x%08x` |
| `mad_dimm_ch3` | gen-keyed MAD base + `0x64` | `0x%08x` |

## Tier 1 / 2 / 3 (and the server line)

The module is generation-agnostic; tier logic lives in userspace
`intel_gen::profile_for`:

- **Tier 1** = Haswell / Broadwell / Skylake / Kaby / Coffee / Comet / Ice /
  Tiger Lake (64 KiB, 2-channel, no gear — the shared 64 KiB map)
- **Tier 2** = Rocket Lake (64 KiB, Gear2)
- **Tier 3** = Alder / Raptor / Meteor / Arrow (256 KiB, DDR4 2-channel or
  DDR5 4-subchannel, MCL fallback, Gear4)
- **Server / HEDT** = Haswell-E / Broadwell-E / Skylake-X / Cascade Lake /
  Sapphire Rapids (no MMIO — userspace decodes from SMBIOS + SPD; the module
  is not the data source for these)

**What the module exposes by generation:** on a pre-12th-gen host the module
exposes the Tier-1-shaped surface (the two `0x4000/0x4400` channel blocks)
over the 64 KiB window — the core 25 attributes. On a 12th-gen+ host it
*also* exposes both gen-gated groups: the `alk_d4_tc` DDR4 TC blocks
(`0xE000`/`0x1E000`, Alder / Raptor DDR4 only) and the `tier3_extended`
block (the `0xD000`/`0xD800` native MCL blocks, the `0x4800`/`0x4C00` DDR5
ch2 / ch3 mirror blocks, and the `mad_dimm_ch2`/`mad_dimm_ch3` raws at the
gen-keyed MAD base + `0x60`/`+0x64`; the whole 12th-gen+ family) — so a DDR4
Tier-3 host (e.g. an i5-12600KF) **and** a DDR5 4-subchannel host both get
full timing decode from the module. The module maps the full 256 KiB window
on a 12th-gen+ host (gen-keyed, step 5). The `/dev/mem` 256 KiB path remains
the fallback only where the module is absent (e.g. an unsigned-module /
Secure-Boot host).

## Building + installing (DKMS)

- `Makefile`: out-of-tree `obj-m`; `KDIR=/lib/modules/$(KVER)/build`;
  auto-detects clang kernels (`CONFIG_CC_IS_CLANG` → `LLVM=1`).
- **Requires Linux ≥ 5.13:** the gen-keyed IMC-base selection (step 5)
  includes `<asm/cpuid/api.h>` for `cpuid_eax(1)`, a header that first landed
  in the 5.13 cpuid-API split — on a ≤ 5.10 kernel the DKMS build fails. The
  impact is contained: the extra is optional, and the absent-module
  degradation path (sysfs missing → the `/dev/mem` fallback + the SMBIOS
  4-tier cross-check, both of which now work standalone) covers it.
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
- **Version:** the module's `MODULE_VERSION` tracks the release (2.4.16);
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
- **Server / HEDT platforms (Haswell-E → Sapphire Rapids):** the module may
  load (the Intel vendor gate passes), but the userspace server decode does
  **not** use its MMIO attributes — these platforms' MC timing registers are
  not OS-accessible, so the readout sources the SMBIOS Type 17 topology + the
  SPD EEPROM instead. The module is the data source for the *client* decode
  only.
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
