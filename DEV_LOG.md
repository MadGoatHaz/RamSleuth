# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE3.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 3 (Phase 3) - both foundation chains merged into v2-development: P3-02..P3-06 (telemetry serde: cpuid/amd/intel/spd/facade derives + whole-snapshot round-trip, merged as merge: P3-02..P3-06) and P3-07..P3-09 (bench serde: worker/orchestrator derives, new streamed.rs with run_streamed + 7 tests, WorkerError serde via String mirror, AlignedBuf pub(crate) widening, merged as merge: P3-07..P3-09). Workspace green post-merge (tests, clippy -D warnings, build). Next: P3-10 + P3-11 (protocol crate - the payload root SystemMemoryTelemetry and WorkerResult/BenchmarkGrid are wire-ready) and P3-15 (bench_job calls run_streamed).
@@@ HISTORY @@@
- [DONE] ID: P3-01-Review | STATUS: SUCCESS | BRANCH: branch/chunk-P3-01
DECISION: Merged P3-01 (serde derives on NaReason + Section<T>) after clean scope/test/clippy/build audit; committed worker Cargo.lock (serde 1.0.229 + bincode 1.3.3) as 34543f8.
AHEAD: P3-02 may consume the NaReason/Section serde derives; runtime bincode stays owned by ramsleuth-protocol (P3-10).
- [DONE] ID: P3-04 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-04
DECISION: serde derives on IntelChannel + IntelReadout (the only pub structs in intel_readout.rs) plus a bincode round-trip test (populated + all-Na multi-channel); 96/96 tests, clippy clean, workspace build green.
AHEAD: P3-05 (spd_decode) is independent; P3-06's Section<IntelReadout> needs P3-04 merged first.
- [DONE] ID: P3-05 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-05
DECISION: serde derives on SpdProfile + SpdModule (the only pub structs in spd_decode.rs; the JEP106 const and all decode logic untouched) plus a bincode round-trip test (representative SpdModule, 2-module DDR4+DDR5 fixture, all-Na with empty profiles); 97/97 lib + 6/6 bin tests, clippy clean, workspace build green. Pending P3-04 sign-out was first committed on branch/chunk-P3-04 (d7c07ee) for a clean base.
AHEAD: P3-06 (facade) can now derive SystemMemoryTelemetry - the spd section is wire-ready; maker 0xC1 / density 0x0D still pending live reconciliation (no decode logic changed here).

- [DONE] ID: P3-06 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-06
DECISION: serde derive + PartialEq on SystemMemoryTelemetry (the payload root) in facade.rs only - all field types already serde-derived by P3-01..P3-05 - plus whole-snapshot bincode round-trip tests (representative Value+Na mix across CPU/AMD/Intel/SPD, all-Na, live collect()); 99/99 lib + 6/6 bin tests, clippy clean, workspace build green. Pending P3-05 sign-out was first committed on branch/chunk-P3-05 (d533e6e) for a clean base.
AHEAD: telemetry serde foundation is complete - P3-07..P3-09 (bench) are now independent; P3-10's Response::Telemetry payload root is wire-ready.
- [DONE] ID: P3-07 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-07
DECISION: serde derives on BenchOp + WorkerResult (the two wire-crossing pub types in worker.rs; run_pinned + WorkerError untouched - WorkerError never crosses the wire) plus bincode round-trip tests (all 3 op arms, representative results incl. the write checksum==total_bytes invariant); 65/65 tests, clippy clean, workspace build green; bincode pinned 1.3 per the P3-01 precedent + ledger (resolves to the same locked 1.3.3).
AHEAD: WorkerResult is wire-ready for the protocol payload inventory (P3-10); P3-08 (orchestrator serde + pub(crate) widening) is independent of this chunk.
- [DONE] ID: P3-08 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-08
DECISION: serde derives on Tier/Metric/BenchmarkGrid (the wire-crossing pub types in orchestrator.rs) + bincode round-trip test (f64 arrays bit-exact, enum arms serde-ready) + five pass helpers widened from private to pub(crate) - AlignedBuf, normalize_size, fill_pattern, bench_bandwidth, bench_latency - for P3-09's run_streamed reuse; no signature edits, materialize_chase/chase_materialized/run_all_sized/run_all untouched; 66/66 tests (8 orchestrator), clippy -D warnings clean, workspace build green.
AHEAD: P3-09 (streamed.rs) can reuse the widened passes; BenchmarkGrid is wire-ready for the protocol payload inventory (P3-10).
- [DONE] ID: P3-09 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-09
DECISION: built streamed.rs - StreamTarget/StreamOptions/StreamProgress/StreamError + run_streamed (clean relaxed cancel before start and between cells, per-cell progress via .ok(), P3-08 pass reuse, host-resolved topology/kernel/threads) + 7 tests; two documented scope deviations forced by the frozen contract: WorkerError serde (Serialize derive + hand-written Deserialize via String mirror for the &'static str field) and AlignedBuf method pub(crate) widening; Cargo.toml comment corrected; 73/73 bench tests, clippy -D warnings clean, workspace build green.
AHEAD: P3-15 must call ramsleuth_bench::run_streamed(&StreamOptions { target, threads, cancel: Arc<AtomicBool> }, Option<mpsc::Sender<StreamProgress>>) -> Result<BenchmarkGrid, StreamError>; StreamProgress/StreamError are bincode-ready and WorkerError is now wire-serializable (P3-10 payload audit should note it).
