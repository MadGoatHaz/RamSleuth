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
 * register we expose (the highest exposed offset is MC_BIOS_REQ at
 * 0x5E00). */
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
RAW_ATTR(mad_inter_channel, mad_offs.inter)
RAW_ATTR(mad_intra_ch0, mad_offs.intra0)
RAW_ATTR(mad_intra_ch1, mad_offs.intra1)
RAW_ATTR(mad_dimm_ch0, mad_offs.dimm0)
RAW_ATTR(mad_dimm_ch1, mad_offs.dimm1)

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
	int ret;

	pdev = pci_get_domain_bus_and_slot(0, 0, PCI_DEVFN(0, 0));
	if (!pdev) {
		pr_err(DRIVER_NAME ": no host bridge at 0000:00:00.0\n");
		return -ENODEV;
	}

	vendor = pdev->vendor;
	pci_read_config_dword(pdev, MCHBAR_LO_OFF, &lo);
	pci_read_config_dword(pdev, MCHBAR_HI_OFF, &hi);

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

	pr_info(DRIVER_NAME
		": MCHBAR base 0x%llx, IMC base 0x%x (model 0x%02x), %zu attributes under /sys/kernel/%s/\n",
		(unsigned long long)mchbar_base,
		imc_base, model,
		ARRAY_SIZE(ramsleuth_attrs) - 1, DRIVER_NAME);
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
MODULE_VERSION("2.4.13"); /* should track the RamSleuth release version — bump with the workspace version at each release */
MODULE_LICENSE("GPL");
