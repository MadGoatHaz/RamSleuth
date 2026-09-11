//! ramsleuth-bench — Native AVX2/AVX-512 SIMD memory bandwidth & latency engine.
//!
//! Phase 1 scaffold: **library target only**. The public API is defined
//! incrementally by the Phase 1 micro-chunks in `plans/PLAN.md`:
//!
//! - CPU feature detection (AVX2 / AVX-512)
//! - streaming read / non-temporal write / copy kernels
//! - physical-core enumeration & pinned worker dispatch
//! - pointer-chasing latency kernel (`__rdtscp`)
//! - cache-hierarchy buffer partitioning (L1/L2/L3/DRAM)
//! - benchmark orchestrator + result aggregation
//!
//! A standalone verification CLI (`src/main.rs`, `cargo run -p ramsleuth-bench`)
//! printing the AIDA64-style grid is added in the final Phase 1 chunk.
//!
//! No third-party dependencies are required: all kernels use `core::arch`
//! intrinsics from the standard library.

// Intentionally minimal at scaffold stage — kernels land in later chunks.
