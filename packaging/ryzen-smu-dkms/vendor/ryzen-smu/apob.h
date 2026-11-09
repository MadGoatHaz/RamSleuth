/* SPDX-License-Identifier: GPL-2.0 */
/* APOB (AGESA Pre-Output Buffer) discovery and sysfs exposure */

#ifndef __APOB_H__
#define __APOB_H__

#include <linux/types.h>
#include <asm/io.h>

/*
 * APOB (AGESA Pre-Output Buffer)
 *
 * The APOB is a boot-time firmware structure placed in reserved physical RAM
 * by AGESA after memory training completes. It contains post-training hardware
 * configuration data, including DRAM timings.
 *
 * On sIOD platforms (Threadripper, EPYC), the SMN shadow registers are not
 * updated post-training, making APOB the only OS-accessible source of active
 * DRAM timing data.
 *
 * The APOB is NOT exposed through any standard OS interface (no ACPI, no
 * SMBIOS, no MMIO BAR). It must be discovered by scanning known candidate
 * physical addresses for the "APOB" signature.
 *
 * Reference: ZenStates-Core (github.com/irusanov/ZenStates-Core)
 *            APOB format documentation in work/docs/research/
 *
 * This module provides:
 *   - apob_init(): scan candidate addresses, map the table if found
 *   - apob_exit(): unmap and release on module exit
 *
 * The kernel exposes the RAW APOB blob via a sysfs binary attribute — no
 * parsing is done here. All format knowledge (header layout, entry structure,
 * timing field offsets) lives in the userspace Rust parser. This is the
 * safest and most testable approach: binary format evolution (new AGESA
 * versions) only requires updating the Rust parser, not rebuilding the
 * kernel module.
 */

/* APOB magic signature: "APOB" in little-endian */
#define APOB_SIGNATURE          0x424F5041u

/* Known APOB physical addresses (all < 4 GiB, in reserved RAM).
 * These are the addresses where AGESA has been observed to place the
 * APOB on AMD platforms. The table is 64 KB aligned and resides in
 * the first 16 MB of physical DRAM.
 *
 * Source: ZenStates-Core Apob.cs - ApobDiscoveryAddresses.
 */
#define APOB_CANDIDATE_0        0x0A200000ul  /* Primary: commonly used by AGESA */
#define APOB_CANDIDATE_1        0x09F00000ul  /* Secondary */
#define APOB_CANDIDATE_2        0x04000000ul  /* Tertiary */

#define APOB_CANDIDATE_ADDRS    3

/* Maximum reasonable APOB table size. The APOB contains the entire
 * platform configuration manifest — it is typically a few KiB to a
 * few hundred KiB. A size larger than 1 MiB is almost certainly
 * corrupted or a false positive signature.
 */
#define APOB_MAX_TABLE_SIZE     (1024 * 1024)  /* 1 MiB safety cap */

/*
 * APOB state — owned by the driver, initialized by apob_init().
 *
 * The driver allocates one instance per PCI probe and passes a pointer
 * to apob_init()/apob_exit(). The state is NOT global; this allows the
 * driver to be loaded/unloaded cleanly and avoids use-after-free if the
 * module is removed while the APOB is mapped.
 */
struct apob_data {
    /* ioremap'd virtual address of the APOB table (NULL if not found).
     * Using u8 __iomem * (consistent with smu.c) to allow pointer
     * arithmetic for offset-based reads in the sysfs bin_read handler. */
    u8 __iomem *vaddr;

    /* Physical address where the APOB was found (0 if not found) */
    phys_addr_t   paddr;

    /* Total table size in bytes (0 if not found) */
    size_t        size;

    /* APOB format version (from the header's Version field) */
    u32           version;

    /* True if the APOB was successfully discovered */
    bool          found;
};

/**
 * apob_init() - Scan candidate physical addresses for the APOB table.
 *
 * For each candidate address:
 *   1. ioremap_cache the first PAGE_SIZE bytes
 *   2. Check for the APOB signature at offset 0
 *   3. If found, read the TableSize from offset 8
 *   4. Validate the size (0 < size <= APOB_MAX_TABLE_SIZE)
 *   5. If the initial mapping doesn't cover the full table, iounmap it
 *      and ioremap_cache the full table size
 *   6. Store the physical address, virtual address, size, and version
 *   7. Stop scanning (first valid APOB wins)
 *
 * Returns 0 if the APOB was found, -ENOENT if not found at any candidate,
 * or -ENOMEM if ioremap failed.
 *
 * Called from ryzen_smu_probe() after smu_init() succeeds. The APOB
 * discovery is independent of the SMU mailbox — it works on all AMD
 * platforms where the APOB is present in physical RAM.
 *
 * NOTE: APOB not found is NOT an error. The driver must load successfully
 * even without APOB (for cIOD platforms that don't need it, or systems
 * where APOB was cleared).
 */
int apob_init(struct apob_data *apob);

/**
 * apob_exit() - Release the APOB ioremap on module exit.
 *
 * Called from ryzen_smu_remove(). Safe to call if discovery failed
 * (no-op in that case).
 */
void apob_exit(struct apob_data *apob);

#endif /* __APOB_H__ */
