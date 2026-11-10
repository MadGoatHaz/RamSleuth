/* SPDX-License-Identifier: GPL-2.0 */
/* APOB (AGESA Pre-Output Buffer) discovery for ryzen_smu driver */

/*
 * The APOB is a boot-time firmware structure placed in reserved
 * physical RAM by AGESA after memory training completes. It contains
 * post-training configuration data including DRAM timings.
 *
 * On sIOD platforms (Threadripper, EPYC), the SMN shadow registers
 * are not updated post-training, making APOB the only OS-accessible
 * source of active DRAM timing data.
 *
 * Discovery: Check 3 known physical addresses for the APOB signature.
 * Reference: ZenStates-Core Hardware/Apob/Apob.cs
 */

#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/io.h>
#include <linux/ioport.h>
#include <linux/errno.h>

#include "apob.h"

/* Known APOB physical addresses (all < 4 GiB, in reserved RAM)
 * Source: ZenStates-Core Apob.cs - ApobDiscoveryAddresses */
static const phys_addr_t apob_candidate_addrs[APOB_CANDIDATE_ADDRS] = {
    APOB_CANDIDATE_0,  /* Primary: commonly used by AGESA */
    APOB_CANDIDATE_1,  /* Secondary */
    APOB_CANDIDATE_2,  /* Tertiary */
};

int apob_init(struct apob_data *apob)
{
    int i;
    int ret = -ENOENT;

    /* Initialize the state to "not found" */
    apob->found = false;
    apob->vaddr = NULL;
    apob->paddr = 0;
    apob->size = 0;
    apob->version = 0;

    /* Don't re-discover if already found (idempotent) */
    if (apob->found)
        return 0;

    /* Scan candidate addresses for APOB signature */
    for (i = 0; i < APOB_CANDIDATE_ADDRS; i++) {
        phys_addr_t addr = apob_candidate_addrs[i];
        u8 __iomem *page;
        u32 sig;
        u32 version;
        u32 table_size;

        pr_debug("APOB: probing candidate %d at phys 0x%llx\n",
                 i, (unsigned long long)addr);

        /* Skip candidates that overlap System RAM — ioremap on System
         * RAM triggers kernel warnings and can panic on panic_on_warn=1
         * systems. The APOB lives in ACPI Non-volatile Storage (NVS)
         * regions, not System RAM. */
        if (region_intersects(addr, PAGE_SIZE, IORESOURCE_SYSTEM_RAM,
                              IORES_DESC_NONE) != REGION_DISJOINT) {
            pr_debug("APOB: 0x%llx is in System RAM, skipping\n",
                     (unsigned long long)addr);
            continue;
        }

        /* Map one page to check the signature.
         * ioremap_cache() is the correct API for mapping system RAM
         * (cacheable). It is used by the existing PM table path in
         * smu.c (line 1321). Read-only access is safe — the APOB is
         * firmware-owned and we never write to it. */
        page = ioremap_cache(addr, PAGE_SIZE);
        if (!page) {
            pr_debug("APOB: ioremap failed at 0x%llx\n",
                     (unsigned long long)addr);
            continue;
        }

        sig = readl(page);  /* Read first 4 bytes (signature) */

        if (sig == APOB_SIGNATURE) {
            /* Signature found! Read the header fields.
             * Header layout (AMD openSIL xUSL/Apob.c APOB_BASE_HEADER):
             *   +0x00: signature (u32) = "APOB"
             *   +0x04: version (u32)
             *   +0x08: table_size (u32) - total size of APOB table
             *   +0x0C: OffsetOfFirstEntry (u32) - byte offset where the
             *          first entry starts; equivalent to the header size
             */
            u32 offset_of_first_entry;

            version = readl(page + 0x04);
            table_size = readl(page + 0x08);
            offset_of_first_entry = readl(page + 0x0C);

            /* Validate the APOB header using AMD's openSIL rules:
             * - Signature must be "APOB" (already checked)
             * - Version must be >= 5 (versions 1-4 are legacy,
             *   pre-chained-entry format)
             * - OffsetOfFirstEntry must be >= 16 (minimum header:
             *   sig+ver+size+offset)
             * - OffsetOfFirstEntry must be < table_size
             * - table_size must be reasonable (> 0 and < some max)
             */
            if (table_size == 0 || table_size > APOB_MAX_TABLE_SIZE) {
                pr_warn("APOB: invalid table size %u at 0x%llx "
                        "(expected 1..%d)\n",
                        table_size, (unsigned long long)addr,
                        APOB_MAX_TABLE_SIZE);
                iounmap(page);
                continue;
            }

            if (version < 5) {
                pr_debug("APOB: version %u < 5, not supported\n",
                         version);
                iounmap(page);
                continue;
            }

            if (offset_of_first_entry < 16 ||
                offset_of_first_entry >= table_size) {
                pr_debug("APOB: implausible OffsetOfFirstEntry %u "
                         "(table size %u) — rejecting\n",
                         offset_of_first_entry, table_size);
                iounmap(page);
                continue;
            }

            /* If the table spans multiple pages, remap the full size.
             * ioremap_cache() on some architectures requires the size to
             * be page-aligned, so round up for the mapping; the
             * *reported* size (apob->size) stays the firmware's original
             * table_size so userspace knows the real extent. If
             * table_size <= PAGE_SIZE the initial mapping already covers
             * the entire table, so we reuse it. */
            if (table_size > PAGE_SIZE) {
                u8 __iomem *full_map;
                size_t map_size = ALIGN(table_size, PAGE_SIZE);

                iounmap(page);
                full_map = ioremap_cache(addr, map_size);
                if (!full_map) {
                    pr_err("APOB: failed to map full table at 0x%llx "
                           "(%u bytes)\n",
                           (unsigned long long)addr, table_size);
                    continue;
                }
                page = full_map;
            }

            /* Store the discovered APOB state */
            apob->vaddr = page;
            apob->paddr = addr;
            apob->size = table_size;
            apob->version = version;
            apob->found = true;

            pr_info("APOB: found at phys 0x%llx, version 0x%08x, "
                    "size %u bytes, OffsetOfFirstEntry %u\n",
                    (unsigned long long)addr, version, table_size,
                    offset_of_first_entry);
            ret = 0;
            break;
        }

        iounmap(page);
    }

    if (!apob->found)
        pr_info("APOB: not found at any candidate address\n");

    return ret;
}

void apob_exit(struct apob_data *apob)
{
    if (apob->vaddr) {
        iounmap(apob->vaddr);
        apob->vaddr = NULL;
    }
    apob->paddr = 0;
    apob->size = 0;
    apob->version = 0;
    apob->found = false;
}
