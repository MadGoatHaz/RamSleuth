//! Cache-hierarchy buffer partitioning & sizing.
//!
//! **Chunk P1-03 — [COUPLED-TO: P1-02].** Computes the aligned byte size of
//! every buffer the benchmark engine uses. This module is sizing only: it
//! *never allocates* — buffer allocation belongs to the worker and
//! orchestrator chunks (P1-08 / P1-10).
//!
//! Sizing rules (PLAN.md §1.2 + Grand Design):
//!
//! | Tier            | Rule                                              |
//! |-----------------|---------------------------------------------------|
//! | L1 (`l1`)       | host L1d size from sysfs; 32 KiB fallback         |
//! | L2 (`l2`)       | host L2 size from sysfs; 1 MiB fallback           |
//! | L3 (`l3`)       | one per-CCD slice ([`CpuTopology::ccd_l3_bytes`]) |
//! | DRAM (`dram`)   | `max(256 MiB, 3 × total system L3)`               |
//! | Ring (`latency_ring`) | fixed 128 MiB; the 64-byte chase stride is applied by the P1-09 kernel |
//!
//! **L3 choice (documented):** the L3 bandwidth buffer is sized at the
//! *per-CCD* granularity (`ccd_l3_bytes`), not the whole-socket total —
//! PLAN.md §1.2 keeps the L3 pass within a single CCD's L3, and a
//! single-CCD socket's slice is exactly its total L3. The DRAM rule, by
//! contrast, consumes the *total* system L3.
//!
//! **L1d/L2 discovery (documented):** cache-index numbering is
//! layout-dependent (standard x86: `index0` = L1d, `index1` = L1i,
//! `index2` = L2, `index3` = L3), so the host's L1d and L2 are found by
//! scanning `cpu0/cache/index*` and matching on the `level`/`type`
//! attributes — never by a bare index number. Any unreadable or malformed
//! entry falls back to the documented default: `plan` is fallible-safe and
//! never fails.
//!
//! Every size is rounded **up** to a 64-byte (cache-line) multiple; the
//! defaults and sysfs sizes are themselves page multiples, so every
//! reported size is also page-aligned.

use std::fs;
use std::path::{Path, PathBuf};

use crate::topology::CpuTopology;

/// Cache-line alignment every planned size is rounded up to.
const ALIGN_64: u64 = 64;
/// L1d fallback when the sysfs entry is unreadable or malformed.
const L1D_DEFAULT: u64 = 32 * 1024;
/// L2 fallback when the sysfs entry is unreadable or malformed.
const L2_DEFAULT: u64 = 1024 * 1024;
/// Grand Design DRAM floor: ≥256 MiB.
const DRAM_FLOOR: u64 = 256 * 1024 * 1024;
/// DRAM is `max(DRAM_FLOOR, DRAM_L3_MULTIPLIER × total system L3)`.
const DRAM_L3_MULTIPLIER: u64 = 3;
/// Fixed pointer-chase latency ring size: 128 MiB.
const LATENCY_RING: u64 = 128 * 1024 * 1024;
/// sysfs root holding the per-CPU `cpuN/` directories.
const SYSFS_CPU_ROOT: &str = "/sys/devices/system/cpu";

/// Frozen buffer sizes (bytes) for the four bandwidth tiers plus the
/// latency ring.
///
/// All fields are rounded up to a 64-byte multiple (and remain page-
/// aligned). See the module docs for the sizing rules; this struct is
/// sizing only — allocation happens in the worker/orchestrator chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferPlan {
    /// L1d-tier bandwidth buffer: host L1d size (32 KiB fallback).
    pub l1: usize,
    /// L2-tier bandwidth buffer: host L2 size (1 MiB fallback).
    pub l2: usize,
    /// L3-tier bandwidth buffer: one per-CCD L3 slice.
    pub l3: usize,
    /// DRAM-tier bandwidth buffer: `max(256 MiB, 3 × total system L3)`.
    pub dram: usize,
    /// Pointer-chase latency ring: 128 MiB. The 64-byte chase stride is a
    /// property of the P1-09 kernel, not of the ring's size.
    pub latency_ring: usize,
}

/// Compute the buffer size plan for `topo`.
///
/// Pure sizing, fallible-safe: L1d/L2 come from the host's sysfs when
/// readable (documented defaults otherwise), and L3/DRAM/ring derive from
/// the frozen [`CpuTopology`]. Never allocates buffers and never fails.
pub fn plan(topo: &CpuTopology) -> BufferPlan {
    BufferPlan {
        l1: size_of(read_l1d_size().unwrap_or(L1D_DEFAULT)),
        l2: size_of(read_l2_size().unwrap_or(L2_DEFAULT)),
        l3: size_of(topo.ccd_l3_bytes),
        dram: size_of(dram_bytes(topo.total_l3_bytes)),
        latency_ring: size_of(LATENCY_RING),
    }
}

/// Host L1 *data* cache size in bytes, or `None` if not discoverable.
fn read_l1d_size() -> Option<u64> {
    read_tier_size(1, Some("Data"))
}

/// Host level-2 cache size in bytes, or `None` if not discoverable.
fn read_l2_size() -> Option<u64> {
    read_tier_size(2, None)
}

/// Grand Design DRAM sizing: `max(256 MiB, 3 × total system L3)`, bytes.
fn dram_bytes(total_l3: u64) -> u64 {
    DRAM_FLOOR.max(total_l3.saturating_mul(DRAM_L3_MULTIPLIER))
}

/// Round `bytes` up to the next 64-byte multiple and convert to `usize`,
/// saturating (no realistic topology overflows 64 bits).
fn size_of(bytes: u64) -> usize {
    let remainder = bytes % ALIGN_64;
    let aligned = if remainder == 0 {
        bytes
    } else {
        bytes
            .checked_add(ALIGN_64)
            .and_then(|b| b.checked_sub(remainder))
            .unwrap_or(u64::MAX)
    };
    usize::try_from(aligned).unwrap_or(usize::MAX)
}

/// Scan `cpu0/cache/index*` for the first entry (lowest index) whose
/// `level` equals `level` and, when `type_name` is given, whose `type`
/// equals it; return that entry's `size` in bytes.
///
/// Index numbering is layout-dependent, so the tier is matched by its
/// `level`/`type` attributes rather than a fixed index. Any I/O or parse
/// failure yields `None` (fallible-safe).
fn read_tier_size(level: u32, type_name: Option<&str>) -> Option<u64> {
    let cache_dir = Path::new(SYSFS_CPU_ROOT).join("cpu0").join("cache");
    let mut candidates: Vec<(usize, PathBuf)> = Vec::new();
    for entry in fs::read_dir(cache_dir).ok()?.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(digits) = name.strip_prefix("index") else {
            continue; // e.g. `uevent`
        };
        let Ok(num) = digits.parse::<usize>() else {
            continue;
        };
        candidates.push((num, entry.path()));
    }
    candidates.sort_by_key(|(num, _)| *num);
    for (_, dir) in &candidates {
        if tier_matches(dir, level, type_name) {
            return read_attr(dir, "size").and_then(|s| parse_cache_size(&s));
        }
    }
    None
}

/// True when `dir` holds a cache index whose `level` equals `level` and
/// whose `type` equals `type_name` (when provided).
fn tier_matches(dir: &Path, level: u32, type_name: Option<&str>) -> bool {
    let Some(actual_level) = read_attr(dir, "level").and_then(|s| s.parse::<u32>().ok()) else {
        return false;
    };
    if actual_level != level {
        return false;
    }
    match type_name {
        Some(expected) => read_attr(dir, "type").as_deref() == Some(expected),
        None => true,
    }
}

/// Read and trim one sysfs attribute file (`level`, `type`, `size`).
fn read_attr(dir: &Path, file: &str) -> Option<String> {
    fs::read_to_string(dir.join(file))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Parse a sysfs cache `size` value (`32K`, `512K`, `32768K`, `16M`) into
/// bytes, or `None` if malformed or overflowing.
fn parse_cache_size(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let (num, suffix) = match raw.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&raw[..i], &raw[i..]),
        None => (raw, ""),
    };
    let bytes: u64 = num.parse().ok()?;
    let multiplier = match suffix.trim() {
        "" => 1,
        "K" | "k" => 1024,
        "M" | "m" => 1024 * 1024,
        "G" | "g" => 1024 * 1024 * 1024,
        "T" | "t" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    bytes.checked_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::detect;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;

    /// Synthetic socket: 4 physical cores, 8 SMT logical CPUs, with the
    /// given total/per-CCD L3 sizes (bytes).
    fn synthetic_topo(total_l3: u64, ccd_l3: u64) -> CpuTopology {
        CpuTopology {
            physical_cores: vec![0, 1, 2, 3],
            logical_cpus: vec![0, 1, 2, 3, 4, 5, 6, 7],
            total_l3_bytes: total_l3,
            ccd_l3_bytes: ccd_l3,
            has_smt: true,
        }
    }

    /// (a) 32 MiB total L3 on a single-CCD socket: DRAM hits the 256 MiB
    /// floor and L3 is the per-CCD slice (here `ccd == total == 32 MiB`).
    #[test]
    fn small_l3_dram_hits_floor_l3_is_ccd_slice() {
        let plan = plan(&synthetic_topo(32 * MIB, 32 * MIB));
        assert_eq!(plan.l3, (32 * MIB) as usize);
        assert_eq!(plan.dram, (256 * MIB) as usize);
    }

    /// (b) 128 MiB total L3 (four 32 MiB CCDs): DRAM = 3× total = 384 MiB.
    #[test]
    fn large_l3_dram_is_3x_total() {
        let plan = plan(&synthetic_topo(128 * MIB, 32 * MIB));
        assert_eq!(plan.l3, (32 * MIB) as usize);
        assert_eq!(plan.dram, (384 * MIB) as usize);
    }

    /// (c) Every planned size is a multiple of 64 bytes.
    #[test]
    fn all_sizes_are_64_byte_aligned() {
        for (total, ccd) in [(32 * MIB, 32 * MIB), (128 * MIB, 32 * MIB)] {
            let plan = plan(&synthetic_topo(total, ccd));
            for size in [plan.l1, plan.l2, plan.l3, plan.dram, plan.latency_ring] {
                assert_eq!(size % 64, 0, "size {size} is not 64-byte aligned");
            }
        }
    }

    /// (d) The latency ring is exactly 128 MiB and 64-byte aligned.
    #[test]
    fn latency_ring_is_128_mib_and_aligned() {
        let plan = plan(&synthetic_topo(32 * MIB, 32 * MIB));
        assert_eq!(plan.latency_ring, (128 * MIB) as usize);
        assert_eq!(plan.latency_ring % 64, 0);
    }

    /// DRAM invariant (PLAN.md §1.2): always ≥ the 256 MiB floor and
    /// strictly above 2× the total system L3, so the DRAM pass misses
    /// every cache tier.
    #[test]
    fn dram_exceeds_2x_total_l3() {
        for total in [16 * MIB, 32 * MIB, 128 * MIB, 512 * MIB] {
            let plan = plan(&synthetic_topo(total, total));
            assert!(
                plan.dram as u64 >= DRAM_FLOOR,
                "dram {plan:?} below the 256 MiB floor"
            );
            assert!(
                plan.dram as u64 > total.saturating_mul(2),
                "dram {plan:?} not above 2x total L3 {total}"
            );
        }
    }

    /// L3 buffer equals the per-CCD slice: rounded up by at most one
    /// 64-byte step (sysfs slices are already page-aligned, so exact).
    #[test]
    fn l3_fits_ccd_slice() {
        for ccd in [8 * MIB, 32 * MIB] {
            let plan = plan(&synthetic_topo(ccd * 4, ccd));
            assert!(
                plan.l3 as u64 <= ccd,
                "L3 buffer {} exceeds the CCD slice {ccd}",
                plan.l3
            );
        }
    }

    /// `size_of` rounds up to the next 64-byte multiple (identity when
    /// already aligned, 0 stays 0).
    #[test]
    fn size_of_rounds_up_to_64() {
        assert_eq!(size_of(0), 0);
        assert_eq!(size_of(64), 64);
        assert_eq!(size_of(65), 128);
        assert_eq!(size_of(1023), 1024);
        assert_eq!(size_of(32 * KIB), (32 * KIB) as usize);
        assert_eq!(size_of(1024 * 1024), MIB as usize);
    }

    /// `parse_cache_size` decodes the sysfs suffixes.
    #[test]
    fn parse_cache_size_units() {
        assert_eq!(parse_cache_size("32K"), Some(32 * KIB));
        assert_eq!(parse_cache_size("512K"), Some(512 * KIB));
        assert_eq!(parse_cache_size("32768K"), Some(32768 * KIB));
        assert_eq!(parse_cache_size("1M"), Some(MIB));
        assert_eq!(parse_cache_size("16M"), Some(16 * MIB));
        assert_eq!(parse_cache_size("4096"), Some(4096));
    }

    /// `parse_cache_size` rejects malformed input instead of guessing.
    #[test]
    fn parse_cache_size_malformed() {
        for bad in ["", "K", "-4", "1.5M", "12X", "99999999999999999999999G"] {
            assert_eq!(parse_cache_size(bad), None, "expected {bad:?} to be rejected");
        }
    }

    /// `plan` on the real host (when sysfs is available): DRAM matches the
    /// spec formula exactly, L3 stays within one rounding step of the
    /// per-CCD slice, and every size is 64-byte aligned.
    #[test]
    fn host_plan_matches_spec() {
        let Ok(topo) = detect() else {
            return; // non-sysfs host: nothing to check
        };
        let plan = plan(&topo);
        let expected_dram =
            DRAM_FLOOR.max(topo.total_l3_bytes.saturating_mul(DRAM_L3_MULTIPLIER));
        assert_eq!(
            plan.dram as u64,
            expected_dram,
            "dram sizing deviates from max(256 MiB, 3x total L3)"
        );
        assert!(plan.l3 as u64 >= topo.ccd_l3_bytes);
        assert!(
            plan.l3 as u64 <= topo.ccd_l3_bytes + ALIGN_64,
            "L3 buffer more than one alignment step above the CCD slice"
        );
        for size in [plan.l1, plan.l2, plan.l3, plan.dram, plan.latency_ring] {
            assert_eq!(size % 64, 0, "size {size} is not 64-byte aligned");
        }
    }
}
