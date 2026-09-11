//! CPU topology enumeration from sysfs (`/sys/devices/system/cpu`).
//!
//! **Chunk P1-02 — [CRITICAL-PATH] interface freeze.** Pinned worker
//! dispatch (P1-08) pins exactly one worker per
//! [`CpuTopology::physical_cores`] entry (SMT siblings filtered out),
//! and DRAM buffer sizing (P1-03) consumes [`CpuTopology::total_l3_bytes`]
//! / [`CpuTopology::ccd_l3_bytes`]. The public signature is frozen at
//! merge; any change requires a plan edit + rebase.
//!
//! All sysfs reads are fallible: a missing or malformed entry yields a
//! [`TopologyError`] — this module never guesses and never substitutes
//! defaults. No `unsafe`: pure `std::fs` parsing.

use std::collections::{hash_map, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// sysfs root holding the per-CPU `cpuN/` directories.
const SYSFS_CPU_ROOT: &str = "/sys/devices/system/cpu";

/// A frozen snapshot of the host CPU topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTopology {
    /// One representative (lowest-index) logical CPU per physical core,
    /// ascending. SMT/Hyper-Threading siblings are filtered out; P1-08
    /// pins exactly one worker per entry.
    pub physical_cores: Vec<usize>,
    /// All logical CPUs present under sysfs (`cpuN`), ascending.
    pub logical_cpus: Vec<usize>,
    /// Total bytes across all distinct level-3 caches, deduplicated by
    /// their `shared_cpu_list`.
    pub total_l3_bytes: u64,
    /// Bytes of the largest single level-3 cache — i.e. one CCD/cluster
    /// slice (uniform across a given socket).
    pub ccd_l3_bytes: u64,
    /// `true` when any physical core exposes more than one SMT thread
    /// (equivalently: `physical_cores.len() < logical_cpus.len()`).
    pub has_smt: bool,
}

/// Failure to enumerate the CPU topology from sysfs.
#[derive(Debug)]
pub enum TopologyError {
    /// An expected sysfs file could not be read.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A sysfs file existed but its contents are malformed.
    InvalidFormat {
        /// The path whose contents were malformed.
        path: PathBuf,
        /// The offending (trimmed) raw contents.
        content: String,
    },
    /// No `cpuN` directory was found under the sysfs cpu root.
    NoCpus,
    /// No level-3 cache was found on any logical CPU.
    NoL3Cache,
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::InvalidFormat { path, content } => write!(
                f,
                "malformed sysfs content {content:?} in {}",
                path.display()
            ),
            Self::NoCpus => write!(f, "no cpuN directories found under {SYSFS_CPU_ROOT}"),
            Self::NoL3Cache => {
                write!(f, "no level-3 cache found under {SYSFS_CPU_ROOT}/cpu*/cache")
            }
        }
    }
}

impl std::error::Error for TopologyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Enumerate the host CPU topology from sysfs.
///
/// Reads `cpuN/topology/{core_id,physical_package_id,thread_siblings_list}`
/// for every logical CPU, groups by `(physical_package_id, core_id)` to
/// select one representative per physical core, and sums the distinct
/// level-3 caches (deduplicated by `shared_cpu_list`).
///
/// # Errors
///
/// Returns [`TopologyError`] when the expected sysfs layout is missing
/// or malformed — it never guesses or falls back to defaults.
pub fn detect() -> Result<CpuTopology, TopologyError> {
    let root = Path::new(SYSFS_CPU_ROOT);
    let logical_cpus = list_logical_cpus(root)?;
    let records = logical_cpus
        .iter()
        .map(|&cpu| read_cpu_record(root, cpu))
        .collect::<Result<Vec<_>, _>>()?;
    let physical_cores = pick_core_representatives(&records);
    let (total_l3_bytes, ccd_l3_bytes) = scan_l3(root, &logical_cpus)?;
    let has_smt = records.iter().any(|r| r.siblings.len() > 1);
    Ok(CpuTopology {
        physical_cores,
        logical_cpus,
        total_l3_bytes,
        ccd_l3_bytes,
        has_smt,
    })
}

/// Per-CPU topology record: identity plus its SMT sibling set.
struct CpuRecord {
    /// Logical CPU index (`cpuN`).
    cpu: usize,
    /// `topology/physical_package_id`.
    package: u32,
    /// `topology/core_id` (unique within a package).
    core_id: u32,
    /// Parsed `topology/thread_siblings_list`.
    siblings: Vec<usize>,
}

/// List logical CPUs (`cpuN` directories) under `root`, ascending.
fn list_logical_cpus(root: &Path) -> Result<Vec<usize>, TopologyError> {
    let entries = fs::read_dir(root)
        .map_err(|source| TopologyError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    let mut cpus = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(digits) = name.strip_prefix("cpu") else {
            continue;
        };
        let Ok(cpu) = digits.parse::<usize>() else {
            continue;
        };
        cpus.push(cpu);
    }
    cpus.sort_unstable();
    cpus.dedup();
    if cpus.is_empty() {
        return Err(TopologyError::NoCpus);
    }
    Ok(cpus)
}

/// Read the topology attributes of one logical CPU.
fn read_cpu_record(root: &Path, cpu: usize) -> Result<CpuRecord, TopologyError> {
    let topology = root.join(format!("cpu{cpu}")).join("topology");
    let package = read_u32(&topology.join("physical_package_id"))?;
    let core_id = read_u32(&topology.join("core_id"))?;
    let siblings = read_cpu_list(&topology.join("thread_siblings_list"))?;
    Ok(CpuRecord {
        cpu,
        package,
        core_id,
        siblings,
    })
}

/// Select one representative (lowest-index) logical CPU per physical
/// core, keyed by `(physical_package_id, core_id)`; result ascending.
fn pick_core_representatives(records: &[CpuRecord]) -> Vec<usize> {
    let mut best: HashMap<(u32, u32), usize> = HashMap::new();
    for r in records {
        let slot = best.entry((r.package, r.core_id));
        match slot {
            hash_map::Entry::Occupied(mut occupied) => {
                if r.cpu < *occupied.get() {
                    occupied.insert(r.cpu);
                }
            }
            hash_map::Entry::Vacant(vacant) => {
                vacant.insert(r.cpu);
            }
        }
    }
    let mut reps: Vec<usize> = best.into_values().collect();
    reps.sort_unstable();
    reps
}

/// Sum the distinct level-3 caches across all logical CPUs.
///
/// The same physical L3 is visible from every CPU that shares it, so
/// caches are deduplicated by their parsed `shared_cpu_list`. Returns
/// `(total_l3_bytes, ccd_l3_bytes)`.
///
/// Unreadable or unidentifiable cache *index entries* are skipped (a
/// malformed non-L3 index must not sink the whole scan); a level-3 entry
/// with malformed `size`/`shared_cpu_list` is a hard error, and a scan
/// that finds no level-3 cache at all is [`TopologyError::NoL3Cache`].
fn scan_l3(root: &Path, logical_cpus: &[usize]) -> Result<(u64, u64), TopologyError> {
    let mut per_cache: HashMap<Vec<usize>, u64> = HashMap::new();
    for &cpu in logical_cpus {
        let cache_dir = root.join(format!("cpu{cpu}")).join("cache");
        let Ok(entries) = fs::read_dir(&cache_dir) else {
            continue; // no cache directory for this CPU: contributes nothing
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if !name.starts_with("index") {
                continue; // e.g. `uevent`
            }
            let dir = entry.path();
            let Ok(level) = read_u32(&dir.join("level")) else {
                continue; // unidentifiable index: not a level-3 candidate
            };
            if level != 3 {
                continue;
            }
            let size = read_size(&dir.join("size"))?;
            let shared = read_cpu_list(&dir.join("shared_cpu_list"))?;
            let slot = per_cache.entry(shared);
            match slot {
                hash_map::Entry::Occupied(mut occupied) => {
                    if size > *occupied.get() {
                        occupied.insert(size);
                    }
                }
                hash_map::Entry::Vacant(vacant) => {
                    vacant.insert(size);
                }
            }
        }
    }
    if per_cache.is_empty() {
        return Err(TopologyError::NoL3Cache);
    }
    let total: u64 = per_cache.values().sum();
    let ccd = per_cache.values().max().copied().unwrap_or(0);
    Ok((total, ccd))
}

/// Read and trim a sysfs file.
fn read_sysfs(path: &Path) -> Result<String, TopologyError> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|source| TopologyError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Read a decimal `u32` sysfs attribute (`core_id`, `physical_package_id`,
/// `level`).
fn read_u32(path: &Path) -> Result<u32, TopologyError> {
    let content = read_sysfs(path)?;
    content
        .parse::<u32>()
        .map_err(|_| TopologyError::InvalidFormat {
            path: path.to_path_buf(),
            content,
        })
}

/// Read a sysfs cpu-list attribute (`thread_siblings_list`,
/// `shared_cpu_list`).
fn read_cpu_list(path: &Path) -> Result<Vec<usize>, TopologyError> {
    let content = read_sysfs(path)?;
    parse_cpu_list(&content).ok_or_else(|| TopologyError::InvalidFormat {
        path: path.to_path_buf(),
        content,
    })
}

/// Read a sysfs cache-size attribute (`4096`, `512K`, `32768K`, `16M`).
fn read_size(path: &Path) -> Result<u64, TopologyError> {
    let content = read_sysfs(path)?;
    parse_size(&content).ok_or_else(|| TopologyError::InvalidFormat {
        path: path.to_path_buf(),
        content,
    })
}

/// Parse a sysfs cpu-list string: `"0"`, `"0,4"`, or `"0-3,4-7"`.
///
/// Returns the indices sorted ascending and deduplicated, or `None` if
/// malformed (empty, non-numeric, or reversed range).
fn parse_cpu_list(s: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((lo, hi)) => {
                let lo: usize = lo.trim().parse().ok()?;
                let hi: usize = hi.trim().parse().ok()?;
                if lo > hi {
                    return None;
                }
                cpus.extend(lo..=hi);
            }
            None => {
                cpus.push(part.parse().ok()?);
            }
        }
    }
    if cpus.is_empty() {
        return None;
    }
    cpus.sort_unstable();
    cpus.dedup();
    Some(cpus)
}

/// Parse a sysfs cache-size string with a binary suffix: `"4096"`,
/// `"512K"`, `"16M"`, `"1G"`, `"2T"`.
///
/// Returns `None` if malformed or overflowing.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, suffix) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
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

    /// `parse_cpu_list` handles single values.
    #[test]
    fn parse_cpu_list_single() {
        assert_eq!(parse_cpu_list("0"), Some(vec![0]));
        assert_eq!(parse_cpu_list("42"), Some(vec![42]));
    }

    /// `parse_cpu_list` handles comma lists (order-insensitive, deduped).
    #[test]
    fn parse_cpu_list_comma() {
        assert_eq!(parse_cpu_list("0,4"), Some(vec![0, 4]));
        assert_eq!(parse_cpu_list("4,0"), Some(vec![0, 4]));
        assert_eq!(parse_cpu_list("0,1,0,2"), Some(vec![0, 1, 2]));
    }

    /// `parse_cpu_list` handles ranges, including the SMT shapes seen in
    /// the wild (`"0-3,4-7"`).
    #[test]
    fn parse_cpu_list_ranges() {
        assert_eq!(
            parse_cpu_list("0-3,4-7"),
            Some(vec![0, 1, 2, 3, 4, 5, 6, 7])
        );
        assert_eq!(parse_cpu_list("2-2"), Some(vec![2]));
        assert_eq!(parse_cpu_list("0-1,3"), Some(vec![0, 1, 3]));
    }

    /// `parse_cpu_list` rejects malformed input instead of guessing.
    #[test]
    fn parse_cpu_list_malformed() {
        for bad in ["", ",", "0-", "-3", "0-1-2", "5-x", "3-1", "x"] {
            assert_eq!(parse_cpu_list(bad), None, "expected {bad:?} to be rejected");
        }
    }

    /// `parse_size` decodes binary suffixes.
    #[test]
    fn parse_size_units() {
        assert_eq!(parse_size("512K"), Some(512 * 1024));
        assert_eq!(parse_size("32768K"), Some(32768 * 1024));
        assert_eq!(parse_size("16M"), Some(16 * 1024 * 1024));
        assert_eq!(parse_size("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(
            parse_size("2T"),
            Some(2 * 1024 * 1024 * 1024 * 1024)
        );
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("16m"), Some(16 * 1024 * 1024));
    }

    /// `parse_size` rejects malformed or overflowing input.
    #[test]
    fn parse_size_malformed() {
        for bad in ["", "K", "12X", "-4", "99999999999999999999999G"] {
            assert_eq!(parse_size(bad), None, "expected {bad:?} to be rejected");
        }
    }

    fn record(cpu: usize, package: u32, core_id: u32, siblings: &[usize]) -> CpuRecord {
        CpuRecord {
            cpu,
            package,
            core_id,
            siblings: siblings.to_vec(),
        }
    }

    /// SMT siblings share `(package, core_id)` and collapse to one
    /// representative: the lowest logical CPU.
    #[test]
    fn representatives_collapse_smt_siblings() {
        let records = vec![
            record(0, 0, 0, &[0, 2]),
            record(2, 0, 0, &[0, 2]),
            record(1, 0, 1, &[1, 3]),
            record(3, 0, 1, &[1, 3]),
            record(4, 1, 0, &[4, 6]),
            record(6, 1, 0, &[4, 6]),
        ];
        assert_eq!(pick_core_representatives(&records), vec![0, 1, 4]);
    }

    /// `core_id` is per-package: the same `core_id` on two packages is
    /// two distinct physical cores.
    #[test]
    fn representatives_key_includes_package() {
        let records = vec![record(0, 0, 0, &[0]), record(1, 1, 0, &[1])];
        assert_eq!(pick_core_representatives(&records), vec![0, 1]);
    }

    /// `detect()` on the build host: non-empty and internally consistent.
    #[test]
    fn detect_on_host() {
        let topo = detect().expect("detect() succeeds on a sysfs host");

        assert!(!topo.logical_cpus.is_empty());
        assert!(!topo.physical_cores.is_empty());
        assert!(topo.total_l3_bytes > 0);
        assert!(topo.ccd_l3_bytes > 0);
        assert!(topo.total_l3_bytes >= topo.ccd_l3_bytes);

        // logical_cpus: strictly ascending (hence unique).
        assert!(
            topo.logical_cpus.windows(2).all(|w| w[0] < w[1]),
            "logical_cpus must be strictly ascending: {topo:?}"
        );

        // every physical-core entry is a valid logical CPU index.
        for &rep in &topo.physical_cores {
            assert!(rep < topo.logical_cpus.len(), "representative {rep} out of range");
            assert!(
                topo.logical_cpus.contains(&rep),
                "representative {rep} missing from logical_cpus"
            );
        }

        // representatives: strictly ascending (hence unique).
        assert!(
            topo.physical_cores.windows(2).all(|w| w[0] < w[1]),
            "physical_cores must be strictly ascending: {topo:?}"
        );

        // SMT relation.
        if topo.has_smt {
            assert!(
                topo.physical_cores.len() < topo.logical_cpus.len(),
                "SMT host: expected fewer physical cores than logical CPUs"
            );
        } else {
            assert_eq!(
                topo.physical_cores.len(),
                topo.logical_cpus.len(),
                "non-SMT host: expected one representative per logical CPU"
            );
        }
    }
}
