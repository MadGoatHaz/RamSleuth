// SPDX-License-Identifier: GPL-2.0
/*
 * ramsleuth_intel.c - Intel client IMC raw register reader (RamSleuth)
 *
 * Probes the host bridge at PCI 0000:00:00.0, decodes MCHBAR from
 * config space (offset 0x48 = low dword, 0x4C = high dword; bit 0 of
 * the low dword = MCHBAR_EN; base = raw & 0x0000007F_FFFF_F000),
 * ioremaps the 64 KiB window, and publishes the raw IMC registers as
 * world-readable (0444) sysfs attributes under
 * /sys/kernel/ramsleuth_intel/.
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
#define MCHBAR_EN		BIT(0)		/* bit 0: MCHBAR enable */
#define MCHBAR_ADDR_MASK	0x0000007FFFFFF000ULL	/* bits 12..38 */

/* Tier 1: map the 64 KiB window covering every register we expose
 * (the highest exposed offset is MC_BIOS_REQ at 0x5E00). */
#define IMC_MAP_SIZE	0x10000

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

/* Global (not per-channel) IMC channel-mode / DIMM-geometry
 * registers (raw only; the userspace reader decodes them). */
#define REG_MAD_INTER_CHANNEL	0x5000
#define REG_MAD_INTRA_CH0	0x5004
#define REG_MAD_INTRA_CH1	0x5008
#define REG_MAD_DIMM_CH0	0x500C
#define REG_MAD_DIMM_CH1	0x5010

static u64 mchbar_base;		/* masked MCHBAR physical base */
static void __iomem *mchbar_mmio;
static struct kobject *ramsleuth_kobj;

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
RAW_ATTR(mad_inter_channel, REG_MAD_INTER_CHANNEL)
RAW_ATTR(mad_intra_ch0, REG_MAD_INTRA_CH0)
RAW_ATTR(mad_intra_ch1, REG_MAD_INTRA_CH1)
RAW_ATTR(mad_dimm_ch0, REG_MAD_DIMM_CH0)
RAW_ATTR(mad_dimm_ch1, REG_MAD_DIMM_CH1)

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
	NULL,
};
static struct attribute_group ramsleuth_group = {
	.attrs = ramsleuth_attrs,
};

static int __init ramsleuth_intel_init(void)
{
	struct pci_dev *pdev;
	u32 lo, hi;
	u64 raw;
	u16 vendor;
	int ret;

	pdev = pci_get_domain_bus_and_slot(0, 0, PCI_DEVFN(0, 0));
	if (!pdev) {
		pr_err(DRIVER_NAME ": no host bridge at 0000:00:00.0\n");
		return -ENODEV;
	}

	vendor = pdev->vendor;
	pci_read_config_dword(pdev, MCHBAR_LO_OFF, &lo);
	pci_read_config_dword(pdev, MCHBAR_HI_OFF, &hi);
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
		return -ENODEV;
	}

	raw = ((u64)hi << 32) | lo;
	if (!(raw & MCHBAR_EN)) {
		pr_err(DRIVER_NAME
		       ": MCHBAR not enabled (bit 0 of config 0x48 clear)\n");
		return -ENODEV;
	}

	mchbar_base = raw & MCHBAR_ADDR_MASK;
	if (!mchbar_base) {
		pr_err(DRIVER_NAME ": MCHBAR base is zero\n");
		return -ENODEV;
	}

	mchbar_mmio = ioremap(mchbar_base, IMC_MAP_SIZE);
	if (!mchbar_mmio) {
		pr_err(DRIVER_NAME ": ioremap of MCHBAR 0x%llx failed\n",
		       (unsigned long long)mchbar_base);
		return -ENOMEM;
	}

	ramsleuth_kobj = kobject_create_and_add(DRIVER_NAME, kernel_kobj);
	if (!ramsleuth_kobj) {
		iounmap(mchbar_mmio);
		mchbar_mmio = NULL;
		pr_err(DRIVER_NAME ": failed to create the sysfs kobject\n");
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
		return ret;
	}

	pr_info(DRIVER_NAME
		": MCHBAR base 0x%llx, %zu attributes under /sys/kernel/%s/\n",
		(unsigned long long)mchbar_base,
		ARRAY_SIZE(ramsleuth_attrs) - 1, DRIVER_NAME);
	return 0;
}

static void __exit ramsleuth_intel_exit(void)
{
	if (ramsleuth_kobj)
		kobject_put(ramsleuth_kobj);
	if (mchbar_mmio)
		iounmap(mchbar_mmio);
}

module_init(ramsleuth_intel_init);
module_exit(ramsleuth_intel_exit);

MODULE_AUTHOR("RamSleuth project (MadGoatHaz)");
MODULE_DESCRIPTION("Raw Intel IMC register reader for RamSleuth (sysfs, no decoding)");
MODULE_VERSION("1.0.0");
MODULE_LICENSE("GPL");
