// SPDX-License-Identifier: GPL-2.0
/*
 * ramsleuth_intel.c - Intel client IMC raw register reader (RamSleuth)
 *
 * Probes the host bridge at PCI 0000:00:00.0, decodes MCHBAR from
 * config space (offset 0x48 = low dword, 0x4C = high dword; bit 0 of
 * the low dword = MCHBAR_EN; base = raw & 0x0000007F_FFFF_F000),
 * ioremaps the MCHBAR window (64 KiB for 6th-11th gen; 256 KiB for the
 * 12th-gen+ Alder/Raptor/Meteor/Arrow family), and publishes the raw IMC
 * registers as world-readable (0444) sysfs attributes under
 * /sys/kernel/ramsleuth_intel/.
 *
 * Chunk 2 (gen-keyed IMC base): the global MAD channel/geometry registers
 * (MAD_INTER_CHANNEL / MAD_INTRA_CH0/1 / MAD_DIMM_CH0/1) moved from
 * MCHBAR + 0x5000 (6th-11th gen) to MCHBAR + 0xd800 (Alder Lake and
 * later) with the System-Agent/arbiter redesign. This module selects the
 * base from the CPU *generation* (the effective 8-bit model from
 * boot_cpu_data, the SDM (ext<<4)|base rule), NOT the overlapping PCI
 * Device-ID ranges: 12th-gen+ -> 0xd800, everything else -> 0x5000. The
 * five MAD attribute offsets are therefore computed at probe time (base
 * relative) and the wider 256 KiB window is mapped for the 12th-gen+
 * family. On a 12th-gen box the pre-Chunk-2 module read 0x5000 (unmapped
 * -> 0x0 / 0xFFFF_FFFF) and misreported the channel mode.
 *
 * Phase 1d (Alder Lake DDR4): on the DDR4 12th-gen+ family the per-MC
 * TC timing registers live at MCHBAR + 0xE000 (MC0) / + 0x1E000 (MC1;
 * +0x10000 inter-controller stride) — NOT the 0x4000/0x4400 Tier-1
 * mirror window (which returns 0xFFFFFFFF on DDR4) — per the Intel
 * 12th-Gen Datasheet Vol. 2, §3.2.66-3.2.87. They are exposed as a
 * second, nameless sysfs attribute group created only when the CPU is
 * the 12th-gen+ family (the same generation that selects the 256 KiB
 * Tier-3 window): the 10-word TC block per MC (the 64-bit TC_PRE split
 * into lo @ +0x00 / hi @ +0x04; plus TC_ACT/TC_RDRD/TC_RDWR/TC_WRRD/
 * TC_WRWR/TC_ODT/TC_RFP/TC_RFTP) and the 8-word native MCL block per
 * MC at 0xD000/0xD800 (the Tier-3 uncore fallback the userspace
 * reader already reads as mcl0_tc_* / mcl1_tc_*). The 0x4000/0x4400
 * Tier-1 mirror blocks stay for Tier-1 compatibility; the
 * pre-12th-gen surface is the core 25 attributes unchanged.
 *
 * M-1 (DDR5 4-subchannel surface): the 12th-gen+ DDR5 family splits each
 * legacy memory channel into two subchannels, so the per-channel TC timing
 * mirrors extend to ch2 (0x4800) and ch3 (0x4C00) — the 0x400-stride
 * mirrors of ch0 (0x4000) / ch1 (0x4400) — and the global MAD block gains
 * the mad_dimm_ch2/3 raws (the gen-keyed MAD base + 0x14/+ 0x18, the
 * 4-byte stride of mad_dimm_ch0/1). The userspace reader (intel_sysfs)
 * reads all 18 of these; the pre-M-1 module created none, so they read
 * 0xFFFFFFFF / were absent. They ride in the same gen-gated nameless group
 * as the Phase 1d DDR4 surface (12th-gen+ only); the pre-12th-gen surface
 * stays the core 25 attributes unchanged.
 *
 * This module performs no decoding: each register attribute is the raw
 * 32-bit value, little-endian, one line, "0x%08x".  The RamSleuth
 * userspace reader decodes the bitfields per CPU generation, so one
 * module spans the supported client generations.
 *
 * The kobject is created only on a fully successful probe (an Intel
 * host bridge at 0000:00:00.0, MCHBAR_EN set, non-zero masked base,
 * ioremap OK).  Any failure leaves no kobject behind and returns a
 * clean -E* code.  On non-Intel platforms (e.g. an AMD host) the load
 * fails with -ENODEV by design: no kobject is ever created.
 */

#include <asm/cpuid/api.h>
#include <linux/errno.h>
#include <linux/init.h>
#include <linux/io.h>
#include <linux/kobject.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/sysfs.h>
#include <linux/types.h>

#define DRIVER_NAME	"ramsleuth_intel"

/* Host bridge MCHBAR (BAR4 of PCI 0000:00:00.0) */
#define MCHBAR_LO_OFF	0x48
#define MCHBAR_HI_OFF	0x4C
/* CAPID0_A: host-bridge capability register (config-space
 * offset 0xE4, 32-bit). Exposed raw (the userspace reader owns bitfield
 * semantics, e.g. bit 17 = ECC_DIS). Read in-kernel via
 * pci_read_config_dword: the host bridge config space is 64-byte truncated
 * in /sys/bus/pci/devices/0000:00:00.0/config. */
#define CAPID0A_OFF	0xE4
#define CAPID0A_UNREADABLE	0xFFFFFFFFu
#define MCHBAR_EN		BIT(0)		/* bit 0: MCHBAR enable */
#define MCHBAR_ADDR_MASK	0x0000007FFFFFF000ULL	/* bits 12..38 */

/* Default window (Tier 1 / Tier 2): the 64 KiB covering every
 * register we expose on a pre-12th-gen box (the highest exposed
 * offset is MC_BIOS_REQ at 0x5E00). The Phase 1d Alder Lake DDR4
 * attributes (0xE000+/0x1E000+ TC, 0xD000/0xD800 MCL) are created
 * only on the 12th-gen+ family, which gets the Tier-3 256 KiB window
 * (the highest exposed offset there is MC1's TC_RFTP at 0x1E43C). */
#define IMC_MAP_SIZE_DEFAULT	0x10000

/* Tier 3 (12th-gen+): the wider 256 KiB window, selected by the CPU
 * generation (is_12th_gen_plus) — the Alder/Raptor/Meteor/Arrow family
 * needs it. Chunk 2: keyed on the CPU model, not the Device-ID (the
 * research Device-ID ranges overlap and are not per-generation). */
#define IMC_MAP_SIZE_TIER3	0x40000

/* IMC register offsets relative to the MCHBAR base */
#define REG_MC_BIOS_REQ	0x5E00

#define REG_TC_DBP_CH0	0x4000
#define REG_TC_RAP_CH0	0x4004
#define REG_TC_RFP_CH0	0x4008
#define REG_TC_RAP2_CH0	0x400C
#define REG_TC_RDRD_CH0	0x4020
#define REG_TC_RDWR_CH0	0x4024
#define REG_TC_WRRD_CH0	0x4028
#define REG_TC_WRWR_CH0	0x402C

#define REG_TC_DBP_CH1	0x4400
#define REG_TC_RAP_CH1	0x4404
#define REG_TC_RFP_CH1	0x4408
#define REG_TC_RAP2_CH1	0x440C
#define REG_TC_RDRD_CH1	0x4420
#define REG_TC_RDWR_CH1	0x4424
#define REG_TC_WRRD_CH1	0x4428
#define REG_TC_WRWR_CH1	0x442C

/*
 * M-1: the DDR5 4-subchannel mirror blocks (12th-gen+ only). ch2 is the
 * 0x4800 mirror and ch3 the 0x4C00 mirror of ch0 (0x4000) / ch1 (0x4400) —
 * the same 8-word TC layout on the 0x400 channel stride.
 */
#define REG_TC_DBP_CH2	0x4800
#define REG_TC_RAP_CH2	0x4804
#define REG_TC_RFP_CH2	0x4808
#define REG_TC_RAP2_CH2	0x480C
#define REG_TC_RDRD_CH2	0x4820
#define REG_TC_RDWR_CH2	0x4824
#define REG_TC_WRRD_CH2	0x4828
#define REG_TC_WRWR_CH2	0x482C

#define REG_TC_DBP_CH3	0x4C00
#define REG_TC_RAP_CH3	0x4C04
#define REG_TC_RFP_CH3	0x4C08
#define REG_TC_RAP2_CH3	0x4C0C
#define REG_TC_RDRD_CH3	0x4C20
#define REG_TC_RDWR_CH3	0x4C24
#define REG_TC_WRRD_CH3	0x4C28
#define REG_TC_WRWR_CH3	0x4C2C

/*
 * Phase 1d (Alder Lake DDR4): the per-MC TC timing registers of the
 * DDR4 12th-gen+ family (Intel 12th-Gen Datasheet Vol. 2,
 * §3.2.66-3.2.87). On DDR4 the TC block is NOT the 0x4000/0x4400
 * Tier-1 mirror window (which returns 0xFFFFFFFF on DDR4) but the
 * native MC0 @ 0xE000 / MC1 @ 0x1E000 (= 0xE000 + the 0x10000
 * inter-controller stride) bases. TC_PRE is a 64-bit register read as
 * two 32-bit words (x86 MMIO is 32-bit granular): lo @ +0x00
 * (tRP / tRDPRE / tPPD), hi @ +0x04 (tWRPRE / tRAS / tRCD).
 */
#define ALK_D4_MC0_BASE		0xE000
#define ALK_D4_MC1_BASE		0x1E000

#define ALK_D4_TC_PRE		0x00
#define ALK_D4_TC_PRE_HI	0x04
#define ALK_D4_TC_ACT		0x08
#define ALK_D4_TC_RDRD		0x0C
#define ALK_D4_TC_RDWR		0x10
#define ALK_D4_TC_WRRD		0x14
#define ALK_D4_TC_WRWR		0x18
#define ALK_D4_TC_ODT		0x70
#define ALK_D4_TC_RFP		0x438
#define ALK_D4_TC_RFTP		0x43C

#define REG_ALK_D4_MC0_TC_PRE_LO	(ALK_D4_MC0_BASE + ALK_D4_TC_PRE)
#define REG_ALK_D4_MC0_TC_PRE_HI	(ALK_D4_MC0_BASE + ALK_D4_TC_PRE_HI)
#define REG_ALK_D4_MC0_TC_ACT		(ALK_D4_MC0_BASE + ALK_D4_TC_ACT)
#define REG_ALK_D4_MC0_TC_RDRD		(ALK_D4_MC0_BASE + ALK_D4_TC_RDRD)
#define REG_ALK_D4_MC0_TC_RDWR		(ALK_D4_MC0_BASE + ALK_D4_TC_RDWR)
#define REG_ALK_D4_MC0_TC_WRRD		(ALK_D4_MC0_BASE + ALK_D4_TC_WRRD)
#define REG_ALK_D4_MC0_TC_WRWR		(ALK_D4_MC0_BASE + ALK_D4_TC_WRWR)
#define REG_ALK_D4_MC0_TC_ODT		(ALK_D4_MC0_BASE + ALK_D4_TC_ODT)
#define REG_ALK_D4_MC0_TC_RFP		(ALK_D4_MC0_BASE + ALK_D4_TC_RFP)
#define REG_ALK_D4_MC0_TC_RFTP		(ALK_D4_MC0_BASE + ALK_D4_TC_RFTP)

#define REG_ALK_D4_MC1_TC_PRE_LO	(ALK_D4_MC1_BASE + ALK_D4_TC_PRE)
#define REG_ALK_D4_MC1_TC_PRE_HI	(ALK_D4_MC1_BASE + ALK_D4_TC_PRE_HI)
#define REG_ALK_D4_MC1_TC_ACT		(ALK_D4_MC1_BASE + ALK_D4_TC_ACT)
#define REG_ALK_D4_MC1_TC_RDRD		(ALK_D4_MC1_BASE + ALK_D4_TC_RDRD)
#define REG_ALK_D4_MC1_TC_RDWR		(ALK_D4_MC1_BASE + ALK_D4_TC_RDWR)
#define REG_ALK_D4_MC1_TC_WRRD		(ALK_D4_MC1_BASE + ALK_D4_TC_WRRD)
#define REG_ALK_D4_MC1_TC_WRWR		(ALK_D4_MC1_BASE + ALK_D4_TC_WRWR)
#define REG_ALK_D4_MC1_TC_ODT		(ALK_D4_MC1_BASE + ALK_D4_TC_ODT)
#define REG_ALK_D4_MC1_TC_RFP		(ALK_D4_MC1_BASE + ALK_D4_TC_RFP)
#define REG_ALK_D4_MC1_TC_RFTP		(ALK_D4_MC1_BASE + ALK_D4_TC_RFTP)

/*
 * Phase 1d: the native MCL (uncore memory-controller) blocks — the
 * Tier-3 uncore fallback, MC0 @ 0xD000 / MC1 @ 0xD800, with the 8-word
 * Alder layout (TC_PRE/TC_ACT/TC_ACT2/TC_WTR/TC_RFP/TC_RFP2/TC_RDRD/
 * TC_WRWR). The userspace reader decodes these as its mcl0/mcl1
 * register set (the mcl0_tc_* / mcl1_tc_* contract names). On the
 * 12th-gen+ family the MCL1 block shares its 0xD800 page with the
 * gen-keyed MAD block (the overlapping words are the same raw
 * registers; the userspace decode disambiguates by generation).
 */
#define MCL0_BASE		0xD000
#define MCL1_BASE		0xD800

#define MCL_TC_PRE		0x00
#define MCL_TC_ACT		0x04
#define MCL_TC_ACT2		0x08
#define MCL_TC_WTR		0x10
#define MCL_TC_RFP		0x14
#define MCL_TC_RFP2		0x18
#define MCL_TC_RDRD		0x20
#define MCL_TC_WRWR		0x28

#define REG_MCL0_TC_PRE		(MCL0_BASE + MCL_TC_PRE)
#define REG_MCL0_TC_ACT		(MCL0_BASE + MCL_TC_ACT)
#define REG_MCL0_TC_ACT2	(MCL0_BASE + MCL_TC_ACT2)
#define REG_MCL0_TC_WTR		(MCL0_BASE + MCL_TC_WTR)
#define REG_MCL0_TC_RFP		(MCL0_BASE + MCL_TC_RFP)
#define REG_MCL0_TC_RFP2	(MCL0_BASE + MCL_TC_RFP2)
#define REG_MCL0_TC_RDRD	(MCL0_BASE + MCL_TC_RDRD)
#define REG_MCL0_TC_WRWR	(MCL0_BASE + MCL_TC_WRWR)

#define REG_MCL1_TC_PRE		(MCL1_BASE + MCL_TC_PRE)
#define REG_MCL1_TC_ACT		(MCL1_BASE + MCL_TC_ACT)
#define REG_MCL1_TC_ACT2	(MCL1_BASE + MCL_TC_ACT2)
#define REG_MCL1_TC_WTR		(MCL1_BASE + MCL_TC_WTR)
#define REG_MCL1_TC_RFP		(MCL1_BASE + MCL_TC_RFP)
#define REG_MCL1_TC_RFP2	(MCL1_BASE + MCL_TC_RFP2)
#define REG_MCL1_TC_RDRD	(MCL1_BASE + MCL_TC_RDRD)
#define REG_MCL1_TC_WRWR	(MCL1_BASE + MCL_TC_WRWR)

/* Global (not per-channel) IMC channel-mode / DIMM-geometry registers
 * (raw only; the userspace reader decodes them). Their MCHBAR-relative
 * offsets are NOT fixed constants: Chunk 2 makes them base-relative
 * (imc_base + 0x00/0x04/0x08/0x0C/0x10), where imc_base is 0x5000 for
 * 6th-11th gen and 0xd800 for the 12th-gen+ family (selected at probe
 * time in _init; see mad_offs below). */

static u64 mchbar_base;		/* masked MCHBAR physical base */
static void __iomem *mchbar_mmio;
static struct pci_dev *host_bridge;
static struct kobject *ramsleuth_kobj;

/*
 * Chunk 2: the gen-selected IMC base (0x5000 / 0xd800) and the five
 * base-relative MAD register offsets, computed at probe time. The
 * RAW_ATTR macro below takes these as runtime expressions (mchbar_mmio +
 * mad_offs.<field>), which is valid C for the sysfs show functions.
 */
static u32 imc_base;
static struct {
	u32 inter;	/* MAD_INTER_CHANNEL = imc_base + 0x00 */
	u32 intra0;	/* MAD_INTRA_CH0     = imc_base + 0x04 */
	u32 intra1;	/* MAD_INTRA_CH1     = imc_base + 0x08 */
	u32 dimm0;	/* MAD_DIMM_CH0      = imc_base + 0x0C */
	u32 dimm1;	/* MAD_DIMM_CH1      = imc_base + 0x10 */
	u32 dimm2;	/* MAD_DIMM_CH2      = imc_base + 0x14 */
	u32 dimm3;	/* MAD_DIMM_CH3      = imc_base + 0x18 */
} mad_offs;

/*
 * One read-only attribute per raw 32-bit IMC register.  The value is
 * the register exactly as read, "0x%08x" on a single line; no
 * decoding (the userspace reader owns bitfield semantics).  The
 * NULL-mmio guard covers reads racing the module exit path.
 */
#define RAW_ATTR(_name, _offset)				\
static ssize_t _name##_show(struct kobject *kobj,		\
			    struct kobj_attribute *attr, char *buf)	\
{							\
	if (!mchbar_mmio)				\
		return -EIO;				\
	return sysfs_emit(buf, "0x%08x\n", ioread32(mchbar_mmio + (_offset))); \
}							\
static struct kobj_attribute dev_attr_##_name = __ATTR_RO(_name);

RAW_ATTR(mcbios_req, REG_MC_BIOS_REQ)
RAW_ATTR(ch0_tc_dbp, REG_TC_DBP_CH0)
RAW_ATTR(ch0_tc_rap, REG_TC_RAP_CH0)
RAW_ATTR(ch0_tc_rfp, REG_TC_RFP_CH0)
RAW_ATTR(ch0_tc_rap2, REG_TC_RAP2_CH0)
RAW_ATTR(ch0_tc_rdrd, REG_TC_RDRD_CH0)
RAW_ATTR(ch0_tc_rdwr, REG_TC_RDWR_CH0)
RAW_ATTR(ch0_tc_wrrd, REG_TC_WRRD_CH0)
RAW_ATTR(ch0_tc_wrwr, REG_TC_WRWR_CH0)
RAW_ATTR(ch1_tc_dbp, REG_TC_DBP_CH1)
RAW_ATTR(ch1_tc_rap, REG_TC_RAP_CH1)
RAW_ATTR(ch1_tc_rfp, REG_TC_RFP_CH1)
RAW_ATTR(ch1_tc_rap2, REG_TC_RAP2_CH1)
RAW_ATTR(ch1_tc_rdrd, REG_TC_RDRD_CH1)
RAW_ATTR(ch1_tc_rdwr, REG_TC_RDWR_CH1)
RAW_ATTR(ch1_tc_wrrd, REG_TC_WRRD_CH1)
RAW_ATTR(ch1_tc_wrwr, REG_TC_WRWR_CH1)
RAW_ATTR(ch2_tc_dbp, REG_TC_DBP_CH2)
RAW_ATTR(ch2_tc_rap, REG_TC_RAP_CH2)
RAW_ATTR(ch2_tc_rfp, REG_TC_RFP_CH2)
RAW_ATTR(ch2_tc_rap2, REG_TC_RAP2_CH2)
RAW_ATTR(ch2_tc_rdrd, REG_TC_RDRD_CH2)
RAW_ATTR(ch2_tc_rdwr, REG_TC_RDWR_CH2)
RAW_ATTR(ch2_tc_wrrd, REG_TC_WRRD_CH2)
RAW_ATTR(ch2_tc_wrwr, REG_TC_WRWR_CH2)
RAW_ATTR(ch3_tc_dbp, REG_TC_DBP_CH3)
RAW_ATTR(ch3_tc_rap, REG_TC_RAP_CH3)
RAW_ATTR(ch3_tc_rfp, REG_TC_RFP_CH3)
RAW_ATTR(ch3_tc_rap2, REG_TC_RAP2_CH3)
RAW_ATTR(ch3_tc_rdrd, REG_TC_RDRD_CH3)
RAW_ATTR(ch3_tc_rdwr, REG_TC_RDWR_CH3)
RAW_ATTR(ch3_tc_wrrd, REG_TC_WRRD_CH3)
RAW_ATTR(ch3_tc_wrwr, REG_TC_WRWR_CH3)
RAW_ATTR(mad_inter_channel, mad_offs.inter)
RAW_ATTR(mad_intra_ch0, mad_offs.intra0)
RAW_ATTR(mad_intra_ch1, mad_offs.intra1)
RAW_ATTR(mad_dimm_ch0, mad_offs.dimm0)
RAW_ATTR(mad_dimm_ch1, mad_offs.dimm1)
RAW_ATTR(mad_dimm_ch2, mad_offs.dimm2)
RAW_ATTR(mad_dimm_ch3, mad_offs.dimm3)

/*
 * Phase 1d (Alder Lake DDR4): the per-MC TC blocks (the 64-bit TC_PRE
 * split into lo/hi; the datasheet §3.2.66-3.2.87 offsets) and the
 * native MCL blocks (the userspace mcl0/mcl1 contract names). Raw
 * words only, like the core set; created only on the 12th-gen+ family
 * (see alk_d4_group below).
 */
RAW_ATTR(alk_d4_mc0_tc_pre_lo, REG_ALK_D4_MC0_TC_PRE_LO)
RAW_ATTR(alk_d4_mc0_tc_pre_hi, REG_ALK_D4_MC0_TC_PRE_HI)
RAW_ATTR(alk_d4_mc0_tc_act, REG_ALK_D4_MC0_TC_ACT)
RAW_ATTR(alk_d4_mc0_tc_rdrd, REG_ALK_D4_MC0_TC_RDRD)
RAW_ATTR(alk_d4_mc0_tc_rdwr, REG_ALK_D4_MC0_TC_RDWR)
RAW_ATTR(alk_d4_mc0_tc_wrrd, REG_ALK_D4_MC0_TC_WRRD)
RAW_ATTR(alk_d4_mc0_tc_wrwr, REG_ALK_D4_MC0_TC_WRWR)
RAW_ATTR(alk_d4_mc0_tc_odt, REG_ALK_D4_MC0_TC_ODT)
RAW_ATTR(alk_d4_mc0_tc_rfp, REG_ALK_D4_MC0_TC_RFP)
RAW_ATTR(alk_d4_mc0_tc_rftp, REG_ALK_D4_MC0_TC_RFTP)
RAW_ATTR(alk_d4_mc1_tc_pre_lo, REG_ALK_D4_MC1_TC_PRE_LO)
RAW_ATTR(alk_d4_mc1_tc_pre_hi, REG_ALK_D4_MC1_TC_PRE_HI)
RAW_ATTR(alk_d4_mc1_tc_act, REG_ALK_D4_MC1_TC_ACT)
RAW_ATTR(alk_d4_mc1_tc_rdrd, REG_ALK_D4_MC1_TC_RDRD)
RAW_ATTR(alk_d4_mc1_tc_rdwr, REG_ALK_D4_MC1_TC_RDWR)
RAW_ATTR(alk_d4_mc1_tc_wrrd, REG_ALK_D4_MC1_TC_WRRD)
RAW_ATTR(alk_d4_mc1_tc_wrwr, REG_ALK_D4_MC1_TC_WRWR)
RAW_ATTR(alk_d4_mc1_tc_odt, REG_ALK_D4_MC1_TC_ODT)
RAW_ATTR(alk_d4_mc1_tc_rfp, REG_ALK_D4_MC1_TC_RFP)
RAW_ATTR(alk_d4_mc1_tc_rftp, REG_ALK_D4_MC1_TC_RFTP)
RAW_ATTR(mcl0_tc_pre, REG_MCL0_TC_PRE)
RAW_ATTR(mcl0_tc_act, REG_MCL0_TC_ACT)
RAW_ATTR(mcl0_tc_act2, REG_MCL0_TC_ACT2)
RAW_ATTR(mcl0_tc_wtr, REG_MCL0_TC_WTR)
RAW_ATTR(mcl0_tc_rfp, REG_MCL0_TC_RFP)
RAW_ATTR(mcl0_tc_rfp2, REG_MCL0_TC_RFP2)
RAW_ATTR(mcl0_tc_rdrd, REG_MCL0_TC_RDRD)
RAW_ATTR(mcl0_tc_wrwr, REG_MCL0_TC_WRWR)
RAW_ATTR(mcl1_tc_pre, REG_MCL1_TC_PRE)
RAW_ATTR(mcl1_tc_act, REG_MCL1_TC_ACT)
RAW_ATTR(mcl1_tc_act2, REG_MCL1_TC_ACT2)
RAW_ATTR(mcl1_tc_wtr, REG_MCL1_TC_WTR)
RAW_ATTR(mcl1_tc_rfp, REG_MCL1_TC_RFP)
RAW_ATTR(mcl1_tc_rfp2, REG_MCL1_TC_RFP2)
RAW_ATTR(mcl1_tc_rdrd, REG_MCL1_TC_RDRD)
RAW_ATTR(mcl1_tc_wrwr, REG_MCL1_TC_WRWR)

/*
 * MCHBAR physical base, 16 hex digits, no prefix
 * (e.g. 00000000fed10000).
 */
static ssize_t mchbar_base_show(struct kobject *kobj,
				struct kobj_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "%016llx\n", (unsigned long long)mchbar_base);
}
static struct kobj_attribute dev_attr_mchbar_base = __ATTR_RO(mchbar_base);

/*
 * Constant "1": the kobject only exists when MCHBAR_EN is set and the
 * window is mapped, so by construction the BAR is enabled while any
 * attribute is visible.
 */
static ssize_t mchbar_enabled_show(struct kobject *kobj,
				   struct kobj_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "1\n");
}
static struct kobj_attribute dev_attr_mchbar_enabled = __ATTR_RO(mchbar_enabled);

/*
 * CAPID0_A raw 32-bit value (config-space offset 0xE4).  Read in-kernel
 * via pci_read_config_dword (the host bridge config space is 64-byte
 * truncated in /sys/.../config).  On a read failure (no retained handle
 * or a config read error) print the 0xffffffff sentinel so the attribute
 * never fails a read; module load is unaffected.
 */
static ssize_t capid0a_show(struct kobject *kobj,
			    struct kobj_attribute *attr, char *buf)
{
	u32 val;

	if (!host_bridge)
		return sysfs_emit(buf, "0x%08x\n", CAPID0A_UNREADABLE);
	if (pci_read_config_dword(host_bridge, CAPID0A_OFF, &val) != 0)
		return sysfs_emit(buf, "0x%08x\n", CAPID0A_UNREADABLE);
	return sysfs_emit(buf, "0x%08x\n", val);
}
static struct kobj_attribute dev_attr_capid0a = __ATTR_RO(capid0a);

static struct attribute *ramsleuth_attrs[] = {
	&dev_attr_mchbar_base.attr,
	&dev_attr_mchbar_enabled.attr,
	&dev_attr_mcbios_req.attr,
	&dev_attr_ch0_tc_dbp.attr,
	&dev_attr_ch0_tc_rap.attr,
	&dev_attr_ch0_tc_rfp.attr,
	&dev_attr_ch0_tc_rap2.attr,
	&dev_attr_ch0_tc_rdrd.attr,
	&dev_attr_ch0_tc_rdwr.attr,
	&dev_attr_ch0_tc_wrrd.attr,
	&dev_attr_ch0_tc_wrwr.attr,
	&dev_attr_ch1_tc_dbp.attr,
	&dev_attr_ch1_tc_rap.attr,
	&dev_attr_ch1_tc_rfp.attr,
	&dev_attr_ch1_tc_rap2.attr,
	&dev_attr_ch1_tc_rdrd.attr,
	&dev_attr_ch1_tc_rdwr.attr,
	&dev_attr_ch1_tc_wrrd.attr,
	&dev_attr_ch1_tc_wrwr.attr,
	&dev_attr_mad_inter_channel.attr,
	&dev_attr_mad_intra_ch0.attr,
	&dev_attr_mad_intra_ch1.attr,
	&dev_attr_mad_dimm_ch0.attr,
	&dev_attr_mad_dimm_ch1.attr,
	&dev_attr_capid0a.attr,
	NULL,
};
static struct attribute_group ramsleuth_group = {
	.attrs = ramsleuth_attrs,
};

/*
 * Phase 1d: the Alder Lake DDR4 attribute group — the two 0xE000/
 * 0x1E000 TC blocks (10 words each) + the two 0xD000/0xD800 native
 * MCL blocks (8 words each) + the M-1 DDR5 4-subchannel surface: the
 * ch2/ch3 TC mirror blocks (8 words each, 0x4800/0x4C00) and the
 * mad_dimm_ch2/3 raws (the gen-keyed MAD block + 0x14/+ 0x18). A nameless
 * group: its attributes land in the kobject root (the same
 * /sys/kernel/ramsleuth_intel/ directory as the core 25). Created only on
 * the 12th-gen+ family (the same generation that selects the 256 KiB
 * Tier-3 window); the pre-12th-gen surface stays the core 25. Removal is
 * implicit: the kobject_put in _exit deletes the whole directory tree.
 */
static struct attribute *alk_d4_attrs[] = {
	&dev_attr_alk_d4_mc0_tc_pre_lo.attr,
	&dev_attr_alk_d4_mc0_tc_pre_hi.attr,
	&dev_attr_alk_d4_mc0_tc_act.attr,
	&dev_attr_alk_d4_mc0_tc_rdrd.attr,
	&dev_attr_alk_d4_mc0_tc_rdwr.attr,
	&dev_attr_alk_d4_mc0_tc_wrrd.attr,
	&dev_attr_alk_d4_mc0_tc_wrwr.attr,
	&dev_attr_alk_d4_mc0_tc_odt.attr,
	&dev_attr_alk_d4_mc0_tc_rfp.attr,
	&dev_attr_alk_d4_mc0_tc_rftp.attr,
	&dev_attr_alk_d4_mc1_tc_pre_lo.attr,
	&dev_attr_alk_d4_mc1_tc_pre_hi.attr,
	&dev_attr_alk_d4_mc1_tc_act.attr,
	&dev_attr_alk_d4_mc1_tc_rdrd.attr,
	&dev_attr_alk_d4_mc1_tc_rdwr.attr,
	&dev_attr_alk_d4_mc1_tc_wrrd.attr,
	&dev_attr_alk_d4_mc1_tc_wrwr.attr,
	&dev_attr_alk_d4_mc1_tc_odt.attr,
	&dev_attr_alk_d4_mc1_tc_rfp.attr,
	&dev_attr_alk_d4_mc1_tc_rftp.attr,
	&dev_attr_mcl0_tc_pre.attr,
	&dev_attr_mcl0_tc_act.attr,
	&dev_attr_mcl0_tc_act2.attr,
	&dev_attr_mcl0_tc_wtr.attr,
	&dev_attr_mcl0_tc_rfp.attr,
	&dev_attr_mcl0_tc_rfp2.attr,
	&dev_attr_mcl0_tc_rdrd.attr,
	&dev_attr_mcl0_tc_wrwr.attr,
	&dev_attr_mcl1_tc_pre.attr,
	&dev_attr_mcl1_tc_act.attr,
	&dev_attr_mcl1_tc_act2.attr,
	&dev_attr_mcl1_tc_wtr.attr,
	&dev_attr_mcl1_tc_rfp.attr,
	&dev_attr_mcl1_tc_rfp2.attr,
	&dev_attr_mcl1_tc_rdrd.attr,
	&dev_attr_mcl1_tc_wrwr.attr,
	/*
	 * M-1: the DDR5 4-subchannel mirror blocks (ch2 @ 0x4800 / ch3 @
	 * 0x4C00, 8 words each) + the mad_dimm_ch2/3 raws (the gen-keyed
	 * MAD block + 0x14/+ 0x18) — the surface the userspace reader
	 * reads but the pre-M-1 module never created.
	 */
	&dev_attr_ch2_tc_dbp.attr,
	&dev_attr_ch2_tc_rap.attr,
	&dev_attr_ch2_tc_rfp.attr,
	&dev_attr_ch2_tc_rap2.attr,
	&dev_attr_ch2_tc_rdrd.attr,
	&dev_attr_ch2_tc_rdwr.attr,
	&dev_attr_ch2_tc_wrrd.attr,
	&dev_attr_ch2_tc_wrwr.attr,
	&dev_attr_ch3_tc_dbp.attr,
	&dev_attr_ch3_tc_rap.attr,
	&dev_attr_ch3_tc_rfp.attr,
	&dev_attr_ch3_tc_rap2.attr,
	&dev_attr_ch3_tc_rdrd.attr,
	&dev_attr_ch3_tc_rdwr.attr,
	&dev_attr_ch3_tc_wrrd.attr,
	&dev_attr_ch3_tc_wrwr.attr,
	&dev_attr_mad_dimm_ch2.attr,
	&dev_attr_mad_dimm_ch3.attr,
	NULL,
};
static struct attribute_group alk_d4_group = {
	.attrs = alk_d4_attrs,
};

/*
 * Chunk 2: the 12th-gen+ (Alder Lake and later) CPU model set — the set
 * whose IMC MAD registers moved to the 0xd800 base. This mirrors the
 * userspace `is_12th_gen_plus` in intel_readout.rs (Chunk 1's gen set):
 * the key is the CPU *model*, NOT the PCI Device-ID (the research
 * Device-ID ranges 0x1900-0x4C00 vs 0x4600-0x7700 overlap and are not
 * per-generation, so keying on the model is unambiguous).
 *
 *   Alder Lake:    0x97, 0x9A, 0xBE
 *   Raptor Lake:   0xB7, 0xBA, 0xBF
 *   Meteor Lake:   0xAC, 0xAA
 *   Arrow Lake:    0xC5, 0xC6, 0xB5
 *   Lunar Lake:    0xBD
 *   (Core Series 1 Mobile shares the 0x9A / 0xBA Alder/Raptor models)
 */
static bool is_12th_gen_plus(u32 model)
{
	switch (model) {
	case 0x97: case 0x9A: case 0xBE:	/* Alder Lake */
	case 0xB7: case 0xBA: case 0xBF:	/* Raptor Lake */
	case 0xAC: case 0xAA:		/* Meteor Lake */
	case 0xC5: case 0xC6: case 0xB5:	/* Arrow Lake */
	case 0xBD:				/* Lunar Lake */
		return true;
	default:
		return false;
	}
}

static int __init ramsleuth_intel_init(void)
{
	struct pci_dev *pdev;
	u32 lo, hi;
	u64 raw;
	u32 model;
	u16 vendor;
	size_t map_size;
	size_t attr_count;
	int ret;

	pdev = pci_get_domain_bus_and_slot(0, 0, PCI_DEVFN(0, 0));
	if (!pdev) {
		pr_err(DRIVER_NAME ": no host bridge at 0000:00:00.0\n");
		return -ENODEV;
	}

	vendor = pdev->vendor;
	ret = pci_read_config_dword(pdev, MCHBAR_LO_OFF, &lo);
	if (ret) {
		pr_err(DRIVER_NAME
		       ": failed to read MCHBAR_LO (PCIe error %d)\n", ret);
		pci_dev_put(pdev);
		return ret;
	}
	ret = pci_read_config_dword(pdev, MCHBAR_HI_OFF, &hi);
	if (ret) {
		pr_err(DRIVER_NAME
		       ": failed to read MCHBAR_HI (PCIe error %d)\n", ret);
		pci_dev_put(pdev);
		return ret;
	}

	/*
	 * Retain a module-lifetime reference to the host bridge so the
	 * capid0a attribute can read its config space (offset 0xE4) on demand
	 * (the 64-byte /sys config-space truncation makes the in-kernel read
	 * the only path).  The original lookup ref is dropped.
	 */
	host_bridge = pci_dev_get(pdev);
	pci_dev_put(pdev);

	/*
	 * Vendor gate: this module reads Intel IMC registers, so only
	 * an Intel host bridge (vendor 0x8086) at 0000:00:00.0 is
	 * valid.  On non-Intel platforms this is a clean -ENODEV with
	 * no kobject.
	 */
	if (vendor != PCI_VENDOR_ID_INTEL) {
		pr_err(DRIVER_NAME
		       ": 0000:00:00.0 is not an Intel host bridge (vendor 0x%04x)\n",
		       vendor);
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENODEV;
	}

	/*
	 * Chunk 2: select the IMC base by CPU generation. The effective
	 * 8-bit model is (ext << 4) | base (the SDM rule, mirroring the
	 * userspace cpuid.rs). 12th-gen+ -> 0xd800, else 0x5000. The five
	 * MAD offsets are base-relative (imc_base + 0x00/04/08/0C/10).
	 */
	{
		u32 eax1 = cpuid_eax(1);
		model = (((eax1 >> 16) & 0xF) << 4) | ((eax1 >> 4) & 0xF);
	}
	imc_base = is_12th_gen_plus(model) ? 0xD800 : 0x5000;
	mad_offs.inter = imc_base;
	mad_offs.intra0 = imc_base + 0x04;
	mad_offs.intra1 = imc_base + 0x08;
	mad_offs.dimm0 = imc_base + 0x0C;
	mad_offs.dimm1 = imc_base + 0x10;
	mad_offs.dimm2 = imc_base + 0x14;
	mad_offs.dimm3 = imc_base + 0x18;

	/*
	 * Window size: 256 KiB for the 12th-gen+ family (the wider
	 * Alder/Raptor/Meteor/Arrow map), the default 64 KiB otherwise —
	 * keyed on the CPU generation (Chunk 2), not the Device-ID.
	 */
	map_size = is_12th_gen_plus(model) ? IMC_MAP_SIZE_TIER3 :
					      IMC_MAP_SIZE_DEFAULT;

	raw = ((u64)hi << 32) | lo;
	if (!(raw & MCHBAR_EN)) {
		pr_err(DRIVER_NAME
		       ": MCHBAR not enabled (bit 0 of config 0x48 clear)\n");
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENODEV;
	}

	mchbar_base = raw & MCHBAR_ADDR_MASK;
	if (!mchbar_base) {
		pr_err(DRIVER_NAME ": MCHBAR base is zero\n");
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENODEV;
	}

	/*
	 * Defensive alignment check: a real MCHBAR base is 64 KiB aligned
	 * (bits 0..15 clear). The mask above clears only the low 12 bits, so
	 * a mis-decoded base that is merely 4 KiB aligned (bits 12..15 set)
	 * would otherwise slip through — reject it.
	 */
	if (mchbar_base & 0xFFFF) {
		pr_err(DRIVER_NAME
		       ": MCHBAR base 0x%llx not 64KiB-aligned\n",
		       (unsigned long long)mchbar_base);
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENODEV;
	}

	mchbar_mmio = ioremap(mchbar_base, map_size);
	if (!mchbar_mmio) {
		pr_err(DRIVER_NAME ": ioremap of MCHBAR 0x%llx failed\n",
		       (unsigned long long)mchbar_base);
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENOMEM;
	}

	ramsleuth_kobj = kobject_create_and_add(DRIVER_NAME, kernel_kobj);
	if (!ramsleuth_kobj) {
		iounmap(mchbar_mmio);
		mchbar_mmio = NULL;
		pr_err(DRIVER_NAME ": failed to create the sysfs kobject\n");
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return -ENOMEM;
	}

	ret = sysfs_create_group(ramsleuth_kobj, &ramsleuth_group);
	if (ret) {
		kobject_put(ramsleuth_kobj);
		ramsleuth_kobj = NULL;
		iounmap(mchbar_mmio);
		mchbar_mmio = NULL;
		pr_err(DRIVER_NAME
		       ": failed to create sysfs attributes (%d)\n", ret);
		pci_dev_put(host_bridge);
		host_bridge = NULL;
		return ret;
	}

	/*
	 * Phase 1d + M-1: the Alder Lake DDR4 surface (the 0xE000/0x1E000
	 * TC blocks + the 0xD000/0xD800 MCL blocks) and the DDR5
	 * 4-subchannel surface (the ch2/ch3 TC mirror blocks at
	 * 0x4800/0x4C00 + the mad_dimm_ch2/3 raws) are exposed only on the
	 * 12th-gen+ family — the same generation that selects the 256
	 * KiB Tier-3 window (map_size above); on a pre-12th-gen box the
	 * surface stays the core 25 (the 0x4000/0x4400 Tier-1 mirror,
	 * kept for Tier-1 compatibility). The nameless group lands its
	 * attributes in the kobject root.
	 */
	attr_count = ARRAY_SIZE(ramsleuth_attrs) - 1;
	if (is_12th_gen_plus(model)) {
		ret = sysfs_create_group(ramsleuth_kobj, &alk_d4_group);
		if (ret) {
			kobject_put(ramsleuth_kobj);
			ramsleuth_kobj = NULL;
			iounmap(mchbar_mmio);
			mchbar_mmio = NULL;
			pr_err(DRIVER_NAME
			       ": failed to create the Alder Lake DDR4 sysfs attributes (%d)\n",
			       ret);
			pci_dev_put(host_bridge);
			host_bridge = NULL;
			return ret;
		}
		attr_count += ARRAY_SIZE(alk_d4_attrs) - 1;
	}

	pr_info(DRIVER_NAME
		": MCHBAR base 0x%llx, IMC base 0x%x (model 0x%02x), %zu attributes under /sys/kernel/%s/\n",
		(unsigned long long)mchbar_base,
		imc_base, model, attr_count, DRIVER_NAME);
	return 0;
}

static void __exit ramsleuth_intel_exit(void)
{
	if (ramsleuth_kobj)
		kobject_put(ramsleuth_kobj);
	if (mchbar_mmio)
		iounmap(mchbar_mmio);
	if (host_bridge)
		pci_dev_put(host_bridge);
}

module_init(ramsleuth_intel_init);
module_exit(ramsleuth_intel_exit);

MODULE_AUTHOR("RamSleuth project (MadGoatHaz)");
MODULE_DESCRIPTION("Raw Intel IMC register reader for RamSleuth (sysfs, no decoding)");
MODULE_VERSION("2.4.16"); /* should track the RamSleuth release version — bump with the workspace version at each release */
MODULE_LICENSE("GPL");
