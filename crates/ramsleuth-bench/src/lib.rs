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
//! The kernels require no third-party dependencies (`core::arch`
//! intrinsics from the standard library); the worker dispatch (P1-08)
//! adds `libc` solely for `sched_setaffinity` pinning.

// P1-01: runtime CPU feature detection (interface freeze).
mod features;
pub use features::CpuFeatures;

// P1-02: /sys CPU topology enumeration with SMT filter (interface freeze).
mod topology;
pub use topology::{CpuTopology, TopologyError, detect};

// P1-03: cache-hierarchy buffer sizing (L1/L2/L3/DRAM + latency ring).
mod buffers;
pub use buffers::{BufferPlan, plan};

// P1-04: AVX2 streaming read kernel (256-bit aligned loads, P1-01 dispatch).
mod kernel_read;
pub use kernel_read::avx2_read;

// P1-05: AVX2 non-temporal streaming write kernel (256-bit NT stores + SFENCE, P1-01 dispatch).
mod kernel_write;
pub use kernel_write::avx2_write;

// P1-06: AVX2 copy kernel (256-bit aligned loads + NT stores + SFENCE, P1-01 dispatch).
mod kernel_copy;
pub use kernel_copy::avx2_copy;

// P1-07: AVX-512F read/write/copy kernels (512-bit aligned loads + NT stores + SFENCE,
// runtime-gated on CpuFeatures.avx512f with fallback to the AVX2 kernels).
mod kernel_512;
pub use kernel_512::{avx512_copy, avx512_read, avx512_write};

// P1-08: pinned per-physical-core worker dispatch (one barrier-synced worker per
// CpuTopology.physical_cores entry; sched_setaffinity pinning with unpinned fallback).
mod worker;
pub use worker::{BenchOp, WorkerError, WorkerResult, run_pinned};
