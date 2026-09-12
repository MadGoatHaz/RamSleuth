# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development`. Plan: `plans/PLAN-PHASE3.md` (Phase 1 retained as `plans/PLAN.md`, Phase 2 as `plans/PLAN-PHASE2.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 3 (Phase 3) - P3-09 COMPLETE on branch/chunk-P3-09 (unmerged, awaiting review; CRITICAL-PATH interface freeze): new streamed.rs - StreamTarget/StreamOptions/StreamProgress/StreamError + run_streamed (target-driven cell list Full=12/Tier=3/Cell=1 bandwidth cells, per-tier latency for whole-tier targets, relaxed cancel gates before start and between cells with clean halt + partial grid discarded, one progress event per completed bandwidth cell with .ok() swallow of dropped receivers, reuses the P3-08-widened passes; topology, kernel family, and pinned thread count resolved on the host) + 7 unit tests; documented deviations beyond the two-file scope: WorkerError is now serde (Serialize derived + hand-written Deserialize via a String-bearing mirror - the Misaligned &'static str field cannot derive for a generic 'de, and it is the StreamError::Worker wire payload) and AlignedBuf's new/as_slice/as_mut_slice widened to pub(crate) (P3-08 exported the struct but left its methods private, so cross-module reuse was impossible); stale P3-07 Cargo.toml comment corrected; 73/73 bench tests green (70 lib + 3 main), clippy -D warnings clean, workspace build green.
Next: P3-09 review/merge (freeze first), then P3-10+P3-11 (protocol) and P3-15 (bench_job calls run_streamed).
@@@ HISTORY @@@
- [DONE] ID: P3-01-Review | STATUS: SUCCESS | BRANCH: branch/chunk-P3-01
DECISION: Merged P3-01 (serde derives on NaReason + Section<T>) after clean scope/test/clippy/build audit; committed worker Cargo.lock (serde 1.0.229 + bincode 1.3.3) as 34543f8.
AHEAD: P3-02 may consume the NaReason/Section serde derives; runtime bincode stays owned by ramsleuth-protocol (P3-10).
- [DONE] ID: P3-07 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-07
DECISION: serde derives on BenchOp + WorkerResult (the two wire-crossing pub types in worker.rs; run_pinned + WorkerError untouched - WorkerError never crosses the wire) plus bincode round-trip tests (all 3 op arms, representative results incl. the write checksum==total_bytes invariant); 65/65 tests, clippy clean, workspace build green; bincode pinned 1.3 per the P3-01 precedent + ledger (resolves to the same locked 1.3.3).
AHEAD: WorkerResult is wire-ready for the protocol payload inventory (P3-10); P3-08 (orchestrator serde + pub(crate) widening) is independent of this chunk.
- [DONE] ID: P3-08 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-08
DECISION: serde derives on Tier/Metric/BenchmarkGrid (the wire-crossing pub types in orchestrator.rs) + bincode round-trip test (f64 arrays bit-exact, enum arms serde-ready) + five pass helpers widened from private to pub(crate) - AlignedBuf, normalize_size, fill_pattern, bench_bandwidth, bench_latency - for P3-09's run_streamed reuse; no signature edits, materialize_chase/chase_materialized/run_all_sized/run_all untouched; 66/66 tests (8 orchestrator), clippy -D warnings clean, workspace build green.
AHEAD: P3-09 (streamed.rs) can reuse the widened passes; BenchmarkGrid is wire-ready for the protocol payload inventory (P3-10).
- [DONE] ID: P3-09 | STATUS: SUCCESS | BRANCH: branch/chunk-P3-09
DECISION: built streamed.rs - StreamTarget/StreamOptions/StreamProgress/StreamError + run_streamed (clean relaxed cancel before start and between cells, per-cell progress via .ok(), P3-08 pass reuse, host-resolved topology/kernel/threads) + 7 tests; two documented scope deviations forced by the frozen contract: WorkerError serde (Serialize derive + hand-written Deserialize via String mirror for the &'static str field) and AlignedBuf method pub(crate) widening; Cargo.toml comment corrected; 73/73 bench tests, clippy -D warnings clean, workspace build green.
AHEAD: P3-15 must call ramsleuth_bench::run_streamed(&StreamOptions { target, threads, cancel: Arc<AtomicBool> }, Option<mpsc::Sender<StreamProgress>>) -> Result<BenchmarkGrid, StreamError>; StreamProgress/StreamError are bincode-ready and WorkerError is now wire-serializable (P3-10 payload audit should note it).
