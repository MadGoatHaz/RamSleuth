# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Phase 1 plan: `plans/PLAN.md`. Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
P1-06 merged to v2-development; ready for P1-07.

## History
- [DONE] ID: P1-06 REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-P1-06
  DECISION: Reviewed frozen avx2_copy contract: unrolled 4-wide aligned _mm256_load_si256(src) + _mm256_stream_si256(dst) with tail, single trailing _mm_sfence(), #[target_feature(enable=avx2)] + cfg(x86_64) gating, CpuFeatures-dispatched copy_from_slice scalar fallback, // SAFETY: on every unsafe block, non-aliasing precondition documented + debug_asserted, no timing, module wired in lib.rs, P1-04 avx2_read word-sum checksum reused; zero clippy warnings; 35/35 tests green debug and release; SIMD body genuinely exercised on this AVX2 host; destination byte-identical to a direct copy_from_slice; merged --no-ff into v2-development.
  AHEAD: P1-07 512-bit variants reuse the (src,dst,len) helper + word-sum checksum convention; P1-08 worker dispatch consumes avx2_copy.
- [DONE] ID: P1-06 | STATUS: SUCCESS | BRANCH: branch/chunk-P1-06
  DECISION: Implemented AVX2 copy kernel: unrolled 4-wide _mm256_load_si256(src) + _mm256_stream_si256(dst) interleaved with tail, single trailing _mm_sfence(); returns P1-04 word-sum checksum of dst via frozen avx2_read (data-sensitive, DCE-defeating); preconditions 32B-aligned both sides, equal 32-multiple length, non-aliasing (debug_asserted, documented); CpuFeatures-dispatched copy_from_slice scalar fallback; 4 unit tests (byte-identical 4 KiB pattern + checksum vs word_sum/direct copy_from_slice, stable/data-sensitive checksum, 32-B minimum, scalar-vs-SIMD identical destination); 35/35 green debug+release on AVX2 host, zero clippy warnings.
  AHEAD: Reviewer: verify NT-store + SFENCE tail shape, non-aliasing precondition, and avx2_read checksum reuse on host; P1-07 512-bit copy reuses the (src,dst,len) helper + checksum convention; P1-08 worker dispatch consumes avx2_copy.
- [DONE] ID: P1-05 REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-P1-05
  DECISION: Reviewed frozen avx2_write contract: NT stores only via _mm256_stream_si256, single trailing _mm_sfence(), AVX2 + cfg gating, scalar fallback with identical contents, SAFETY comments on every unsafe block, no timing, lib.rs wiring, P1-04 pattern reuse; clippy -D warnings zero; 31/31 tests green debug and release; SIMD path exercised on AVX2 host; merged --no-ff into v2-development.
  AHEAD: P1-06 copy kernel reuses (ptr, len) + _mm256_stream_si256 + _mm_sfence tail shape; AVX2 host confirmed.
- [DONE] ID: P1-05 | STATUS: SUCCESS | BRANCH: branch/chunk-P1-05
  DECISION: Implemented AVX2 non-temporal write kernel: _mm256_set_epi64x pattern vector + _mm256_stream_si256 unrolled 4-wide + tail, single _mm_sfence() after the last store; returns bytes written (dst.len()); CpuFeatures-dispatched safe scalar fallback (LE u64 words, identical contents); 4 unit tests (pattern-filled 4 KiB + spot-check, stable/non-zero return, 32-B minimum, scalar-vs-SIMD contents), 31/31 green debug+release on AVX2 host, zero clippy.
  AHEAD: Reviewer: verify NT-store + SFENCE shape on host; P1-06 copy kernel reuses the (ptr,len) + _mm256_stream_si256 + _mm_sfence tail pattern; PLAN.md P1-05 "returns bytes written" signature is the frozen one.
- [DONE] ID: P1-04 | STATUS: SUCCESS | BRANCH: branch/chunk-P1-04
  DECISION: Implemented AVX2 streaming read kernel: _mm256_load_si256 unrolled 4-wide + tail, _mm256_add_epi64 lane accumulation, horizontal u64 reduction; checksum = wrapping sum of LE u64 words (identical SIMD/scalar, 0 for zeros); CpuFeatures-dispatched safe scalar fallback; 4 unit tests (zeroed 4 KiB, stability/DCE, 32-B minimum, scalar fallback), 27/27 green debug+release, zero clippy.
  AHEAD: Reviewer: verify thin-pointer simd helper shape on host; P1-05/P1-06/P1-07 must reuse the word-sum checksum definition + (ptr,len) helper pattern; PLAN.md P1-04 "returns bytes processed" text is superseded by the checksum signature.
- [DONE] ID: P1-02 | STATUS: SUCCESS | BRANCH: branch/chunk-P1-02
  DECISION: Reviewed frozen detect() -> Result<CpuTopology, TopologyError>; merged after zero clippy warnings and 13/13 tests green.
  AHEAD: P1-03/P1-08 must consume CpuTopology fields (physical_cores, total_l3_bytes, ccd_l3_bytes); PLAN.md P1-02 text is stale (discover/PhysicalCore).
- [DONE] ID: P1-03 | STATUS: SUCCESS | BRANCH: branch/chunk-P1-03
  DECISION: Reviewed pure plan(&CpuTopology) -> BufferPlan (L1/L2 sysfs+safe fallbacks, per-CCD L3 slice, DRAM max(256 MiB, 3x total L3), 128 MiB ring, all 64-B aligned); merged after zero clippy warnings and 23/23 tests green.
  AHEAD: P1-04/P1-08/P1-09 consume BufferPlan fields (l1,l2,l3,dram,latency_ring); PLAN.md P1-03 text is stale (BufferProfiles/size_for, 16 KB/256 KB).
