//! Runtime CPU feature detection (AVX2 / AVX-512F).
//!
//! **Chunk P1-01 — [CRITICAL-PATH] interface freeze.** Every SIMD kernel
//! (P1-04…P1-07) branches on [`CpuFeatures`]. The public signature is
//! frozen at merge; any change requires a plan edit + rebase.
//!
//! Detection uses the safe `std::is_x86_feature_detected!` macro — there
//! is no `unsafe` in this module.

/// Runtime CPU feature flags used to gate SIMD kernel dispatch.
///
/// SSE2 is the x86_64 architectural baseline and is always implied `true`
/// on that target (guaranteed by the ABI), so it is not stored as a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFeatures {
    /// AVX2 (256-bit) support — gates `kernel_read`/`kernel_write`/`kernel_copy`.
    pub avx2: bool,
    /// AVX-512F (512-bit) support — gates the `kernel_512` upgrade path.
    pub avx512f: bool,
}

impl CpuFeatures {
    /// Detect CPU features at runtime.
    ///
    /// x86_64: queries the CPUID-backed detection macro from std.
    /// Non-x86_64: no x86 feature flags exist — both are reported `false`.
    #[cfg(target_arch = "x86_64")]
    pub fn detect() -> Self {
        Self {
            avx2: std::is_x86_feature_detected!("avx2"),
            avx512f: std::is_x86_feature_detected!("avx512f"),
        }
    }

    /// Fallback for non-x86_64 targets.
    #[cfg(not(target_arch = "x86_64"))]
    pub fn detect() -> Self {
        Self {
            avx2: false,
            avx512f: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `detect()` must be total: it returns without panicking on the host.
    #[test]
    fn detect_does_not_panic() {
        let features = CpuFeatures::detect();
        let _ = features; // flags are host-dependent; no assertion on values
    }

    /// SSE2 is the x86_64 baseline: the detection macro reports it present
    /// (it is architecturally guaranteed, not an optional feature).
    #[test]
    fn sse2_baseline_is_implied_true() {
        #[cfg(target_arch = "x86_64")]
        assert!(std::is_x86_feature_detected!("sse2"));
    }

    /// AVX-512F builds on the 256-bit register file: a host may not report
    /// `avx512f` without `avx2` (that would be a detection bug).
    #[test]
    fn avx512f_implies_avx2() {
        let features = CpuFeatures::detect();
        assert!(!features.avx512f || features.avx2);
    }

    /// Non-x86_64 arm: the fallback reports no features. (Compiles only
    /// off-x86_64; kept as the guard for the `cfg` split above.)
    #[test]
    fn fallback_reports_no_features_on_non_x86_64() {
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(
            CpuFeatures::detect(),
            CpuFeatures {
                avx2: false,
                avx512f: false,
            }
        );
    }
}
