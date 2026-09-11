# RamSleuth v2 — Phase 1 Plan: Native Benchmark Engine (`crates/ramsleuth-bench`)

> **Base branch:** `v2-development` — every `branch/chunk-P1-xx` forks from and merges back here (NOT `main`).
> **Sources of truth:** `Docs/Grand Design & Architecture Specification.md` §6 (Native Benchmark Engine) and `Docs/RamSleuth-v2.md` Phase 1.
> **Mandate:** 100% pure Rust, Cargo workspace, Edition 2021. Kernels/CLI use **std + `core::arch` intrinsics only** (no external crates in Phase 1).
> **Chunk discipline:** one target source file per chunk, ≤ ~50–100 lines of source changed. Each chunk also adds exactly one `mod <name>;` line to `src/lib.rs` (wiring only, not counted against the 50–100 budget).

---

## 1. Phase 1 Scope

**Objective:** Build a standalone, high-performance benchmarking crate that (a) saturates multi-channel memory bandwidth with pinned, non-temporal SIMD workers and (b) measures nanosecond-accurate access latency via a prefetcher-defeating pointer-chase ring. Output is an **AIDA64-style results grid** surfaced by a verification CLI (`cargo run -p ramsleuth-bench`) in text + JSON.

### 1.1 AIDA64-style grid (output contract)

Four rows × four columns, matching the spec dashboard:

|        | Read        | Write       | Copy        | Latency   |
|--------|-------------|-------------|-------------|-----------|
| Memory (DRAM) | MB/s       | MB/s        | MB/s        | ns        |
| L1 Cache | GB/s       | GB/s        | GB/s        | ns        |
| L2 Cache | GB/s       | GB/s        | GB/s        | ns        |
| L3 Cache | GB/s       | GB/s        | GB/s        | ns        |

- Bandwidth cells: total bytes moved ÷ elapsed wall time. DRAM reported in **MB/s**, cache tiers in **GB/s** (per spec).
- Latency cell: per-hop cycles ÷ nominal freq, in **ns**, single-threaded pointer chase.

### 1.2 Cache-hierarchy buffer sizing (telemetry → bench mapping)

| Tier | Size | Rationale (spec §6.1–6.3) |
|------|------|---------------------------|
| L1   | 16 KB | fits fully in 32/48 KB L1D |
| L2   | 256 KB | fits fully in 512 KB–2 MB L2 |
| L3   | 50% of one active CCD slice (e.g. 16 MB on a 32 MB CCD) | per-CCD L3 |
| DRAM | `max(256 MB, 3 × total system L3)`  (v2 floor: ≥256 MB and **> 2× total L3**) | forces every read/write to miss cache and hit the DRAM bus |
| Latency ring | 128 MB | contiguous buffer holding the pointer-chase list |

The DRAM sizing consumes **total system L3** (from `topology.rs`) and the **per-CCD L3 slice** (from `topology.rs`), so buffer sizing is coupled to core enumeration.

### 1.3 Acceptance / exit gates (Phase 1)

1. **Bandwidth parity:** DRAM read/write bandwidth within **±5%** of AIDA64 on the same dual-channel DDR4/DDR5 machine.
2. **Latency parity:** DRAM latency within **±2 ns** of AIDA64; platform baseline **60–75 ns** (DDR5-6000 AM5).
3. **CLI:** `cargo run -p ramsleuth-bench` runs all passes, prints the full grid (text) and JSON, and exits `0`.
4. **Quality:** idiomatic Rust, `cargo clippy -p ramsleuth-bench --all-targets` → **zero warnings**, `cargo test -p ramsleuth-bench` → green.

---

## 2. Module layout (target file tree)

```
crates/ramsleuth-bench/
├── Cargo.toml            (scaffolded; [[bin]] auto-detected once src/main.rs lands in P1-11)
└── src/
    ├── lib.rs            (module wiring only — each chunk adds one `mod` line)
    ├── features.rs       P1-01  CPU feature detection (AVX2 / AVX-512F)   [CRITICAL-PATH]
    ├── topology.rs       P1-02  /sys physical-core enumeration (SMT filter) [CRITICAL-PATH]
    ├── buffers.rs        P1-03  L1/L2/L3/DRAM + latency-ring sizing
    ├── kernel_read.rs    P1-04  AVX2 streaming read (_mm256_load_si256)
    ├── kernel_write.rs   P1-05  AVX2 non-temporal write (_mm256_stream_si256)
    ├── kernel_copy.rs    P1-06  AVX2 copy (load + non-temporal store)
    ├── kernel_512.rs     P1-07  AVX-512 read/write/copy (runtime-gated)
    ├── worker.rs         P1-08  pinned per-physical-core worker dispatch (barrier-synced)
    ├── latency.rs        P1-09  pointer-chase latency kernel (64B stride, __rdtscp)
    ├── orchestrator.rs   P1-10  runs all passes, aggregates the grid
    └── main.rs           P1-11  verification CLI (text + JSON grid)
```

**Interface freeze:** P1-01 (`features`) and P1-02 (`topology`) are `[CRITICAL-PATH]`. Their public signatures are frozen at merge; kernels and the orchestrator branch on them. No silent signature changes — any change is a plan edit + rebase.

---

## 3. Micro-chunks (ordered — dependencies first)

### Chunk P1-01 — CPU feature detection  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-bench/src/features.rs` (+1 `mod features;` in `src/lib.rs`)
- **Scope Boundary:** Runtime-detect **AVX2** and **AVX-512F** via `std::is_x86_feature_detected!`; expose frozen `CpuFeatures { avx2: bool, avx512f: bool }` + `CpuFeatures::detect()`.
- **Dependency:** `[CRITICAL-PATH]` (interface freeze — every SIMD kernel branches on `CpuFeatures`).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `#[cfg(target_arch="x86_64")]`-gated; unit test asserts the host's real flags round-trip.
- **Exit Criteria:** `detect()` compiles + returns correct booleans on the build host; `CpuFeatures` signature frozen.

### Chunk P1-02 — Physical-core enumeration  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-bench/src/topology.rs` (+1 `mod topology;` in `src/lib.rs`)
- **Scope Boundary:** Parse `/sys/devices/system/cpu/cpu*/topology/` (`core_id`, `physical_package_id`, `thread_siblings_list`) to build **one entry per physical core**, filtering out SMT siblings; expose `PhysicalCore { core_id: u32, logical_cpus: Vec<u32> }` + `discover() -> Vec<PhysicalCore>` and `total_l3_bytes()` / `ccd_l3_slice_bytes()` helpers (read from `cpuinfo`/`l3_cache_size`).
- **Dependency:** `[CRITICAL-PATH]` (interface freeze — worker dispatch + DRAM sizing depend on it).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; reads `/sys` via `std::fs` (no external dep); unit test on a synthetic topology fixture.
- **Exit Criteria:** `discover()` returns exactly the physical-core count (logical ÷ SMT width); `total_l3_bytes()` > 0 on x86_64 Linux; signature frozen.

### Chunk P1-03 — Buffer partitioning & sizing
- **Target File:** `crates/ramsleuth-bench/src/buffers.rs` (+1 `mod buffers;` in `src/lib.rs`)
- **Scope Boundary:** Compute aligned buffer sizes for **L1 (16 KB), L2 (256 KB), L3 (50% of a CCD slice), DRAM (`max(256 MB, 3× total L3)`, floor > 2× total L3)** and the **128 MB latency ring**; expose `BufferProfiles { l1,l2,l3,dram,latency_ring: usize }` + `size_for(total_l3, ccd_slice) -> BufferProfiles`.
- **Dependency:** `[COUPLED-TO: P1-02]` (consumes `total_l3_bytes()` / `ccd_l3_slice_bytes()`).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; sizes 64-byte aligned; unit test checks L3 ≤ ccd_slice and DRAM > 2× total L3.
- **Exit Criteria:** `size_for()` returns a coherent, cache-aligned profile set for the host topology.

### Chunk P1-04 — AVX2 streaming READ kernel
- **Target File:** `crates/ramsleuth-bench/src/kernel_read.rs` (+1 `mod kernel_read;` in `src/lib.rs`)
- **Scope Boundary:** Unrolled streaming read over aligned bytes using `_mm256_load_si256`; accumulate to a sink (e.g. XOR-reduce or `black_box`) to defeat dead-store elimination; returns bytes processed.
- **Dependency:** `[COUPLED-TO: P1-01]` (AVX2 gate).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `unsafe` block minimal + `// SAFETY:` documented; aligned pointer precondition asserted in tests.
- **Exit Criteria:** compiles under `cfg(target_arch="x86_64")`; benchmarkable throughput loop with no cache-line misalignment faults.

### Chunk P1-05 — AVX2 non-temporal WRITE kernel
- **Target File:** `crates/ramsleuth-bench/src/kernel_write.rs` (+1 `mod kernel_write;` in `src/lib.rs`)
- **Scope Boundary:** Streaming write using `_mm256_stream_si256` (bypasses cache / avoids RFO) followed by `_mm_sfence()`; returns bytes written.
- **Dependency:** `[COUPLED-TO: P1-01]` (AVX2 gate).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `// SAFETY:` documented; SFENCE placed after the last store.
- **Exit Criteria:** compiles on x86_64; write path is non-temporal (verified by the reduced RFO signature in later orchestration).

### Chunk P1-06 — AVX2 COPY kernel
- **Target File:** `crates/ramsleuth-bench/src/kernel_copy.rs` (+1 `mod kernel_copy;` in `src/lib.rs`)
- **Scope Boundary:** Interleaved aligned `_mm256_load_si256` (src) + `_mm256_stream_si256` (dst) copy loop with a trailing `_mm_sfence()`; returns bytes copied.
- **Dependency:** `[COUPLED-TO: P1-01]` (AVX2 gate).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `// SAFETY:` documented; src≠dst aliasing precondition.
- **Exit Criteria:** compiles on x86_64; copy path validated (dst contents match src in a test).

### Chunk P1-07 — AVX-512 kernel variants
- **Target File:** `crates/ramsleuth-bench/src/kernel_512.rs` (+1 `mod kernel_512;` in `src/lib.rs`)
- **Scope Boundary:** `_mm512_load_si512` (read), `_mm512_stream_si512` (write), and 512-bit copy — **runtime-gated behind `CpuFeatures.avx512f`** (fall back to AVX2 kernels when unavailable).
- **Dependency:** `[COUPLED-TO: P1-01]` (AVX-512F gate).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `#[target_feature(enable = "avx512f")]` on `unsafe` fns; `// SAFETY:` documented.
- **Exit Criteria:** compiles on x86_64; dispatch selects 512-bit path only when `avx512f` is detected.

### Chunk P1-08 — Pinned worker thread dispatch
- **Target File:** `crates/ramsleuth-bench/src/worker.rs` (+1 `mod worker;` in `src/lib.rs`)
- **Scope Boundary:** Spawn **one worker per physical core** (from P1-02), pin each via `sched_setaffinity` (libc) / `core_affinity`, **exclude SMT siblings**, launch in lockstep with a `std::sync::Barrier`, run a chosen bandwidth kernel (read/write/copy) over a per-worker buffer slice, and sum bytes for aggregate GB/s.
- **Dependency:** `[COUPLED-TO: P1-02, P1-04, P1-05, P1-06]` (topology + bandwidth kernels; P1-07 optional upgrade path).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; thread-safe aggregation (atomic/`JoinHandle`); no shared mutable state across threads.
- **Exit Criteria:** spawns exactly the physical-core count; barrier ensures concurrent access; aggregate throughput = Σ bytes ÷ wall time.

### Chunk P1-09 — Pointer-chase LATENCY kernel
- **Target File:** `crates/ramsleuth-bench/src/latency.rs` (+1 `mod latency;` in `src/lib.rs`)
- **Scope Boundary:** Populate a **128 MB ring** (from P1-03) with a pseudo-random **circular linked list at 64-byte strides** (Fisher-Yates over cache-line boundaries) to defeat stream/spatial prefetchers; single-threaded traversal pinned to the preferred core, timed with serialized `core::arch::x86_64::__rdtscp`; returns ns/hop.
- **Dependency:** `[COUPLED-TO: P1-03]` (128 MB ring buffer); uses `__rdtscp` (baseline x86, no AVX gate).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; dependent-chase loop so no out-of-order overlap; `black_box` on the pointer chain; `// SAFETY:` documented.
- **Exit Criteria:** compiles on x86_64; produced ns/hop lands in the platform latency band (e.g. 60–75 ns on DDR5-6000 AM5) at the DRAM tier.

### Chunk P1-10 — Benchmark orchestrator & aggregation
- **Target File:** `crates/ramsleuth-bench/src/orchestrator.rs` (+1 `mod orchestrator;` in `src/lib.rs`)
- **Scope Boundary:** Run all passes for each tier (L1/L2/L3/DRAM) across Read/Write/Copy (via P1-08 worker dispatch + P1-07 512 path) and Latency (P1-09); aggregate into the frozen `BenchmarkGrid` (4 rows × 4 cols) with units + a machine-readable JSON/serde-free serializable struct.
- **Dependency:** `[COUPLED-TO: P1-08, P1-09]` (and transitively P1-01–07, P1-03).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; deterministic pass ordering; clear progress callback for later daemon/TUI reuse.
- **Exit Criteria:** `run() -> BenchmarkGrid` produces all 16 cells; DRAM cells within ±5% of AIDA64 and latency within ±2 ns on the test machine.

### Chunk P1-11 — Verification CLI (AIDA64-style grid)
- **Target File:** `crates/ramsleuth-bench/src/main.rs` (auto-detects the `[[bin]]`; no manifest change)
- **Scope Boundary:** `cargo run -p ramsleuth-bench` entrypoint — detect features/topology, invoke the P1-10 orchestrator, and print the AIDA64-style grid as aligned **text** plus a **JSON** block; `--json`/`--text` flags; exit `0` on success, non-zero on unsupported arch.
- **Dependency:** `[COUPLED-TO: P1-10]` (consumes the orchestrator + `BenchmarkGrid`).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `clap`-free arg parsing (std `env::args`); graceful error text (no panics) on non-x86_64.
- **Exit Criteria:** prints the full 4×4 grid (text + JSON), exits `0` on x86_64 Linux; completes the Phase 1 exit criteria in §1.3.

---

## 4. Global quality gates (apply to every chunk)

- Idiomatic Rust, Edition 2021; all `unsafe` confined to the kernel fns with a `// SAFETY:` comment.
- `cargo clippy -p ramsleuth-bench --all-targets -- -D warnings` → clean.
- `cargo test -p ramsleuth-bench` → green (unit tests live in the same file as the code they test, `#[cfg(test)]`).
- `cargo build -p ramsleuth-bench --release` → succeeds.
- No third-party crate added in Phase 1 (libc is the **only** permitted external dep, for `sched_setaffinity` in P1-08 — introduced in that chunk's commit).
- Merge model: `branch/chunk-P1-xx` → `v2-development` via `git merge --no-ff` after review + tests; never force-push.
