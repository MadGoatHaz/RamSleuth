# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ f4ee93b (Cycle 9 compacted; 541/541 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed; operator-gated). Plan: `plans/PLAN-CYCLE10.md` (Cycle 10 — 5950X density misdecode 0x0D→32 Gb + daemon DRAM-spike-before-MCLK-read; the Cycle 9 plan is retained as `plans/PLAN-CYCLE9.md`, Cycle 8 as `plans/PLAN-CYCLE8.md`, Cycle 7 as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 10 (density 0x0D→32Gb + daemon DRAM-spike-before-MCLK-read) — Wave 1 MERGED (atomic): C10-01 (8ac43e6: decode_density 0x0D arm 16→32 Gb + C10 reconciliation doc/inline reword) + C10-02 (60f3345: the 5 live-module test spots re-anchored to 32 Gb / 32768 Mbit / 32 GiB, fixture doc "64 GiB kit (2x32 GiB)") landed as one --no-ff merge 5ddec39 (c10-01/c10-02 branches pruned, no c10-01/02 worktrees existed; review gate at the c10-02 tip: telemetry 187/187 green, clippy -D warnings zero); Wave 2 MERGED (chain): C10-03 (5e89c7e: daemon `dram_spike` module + 3-line `lib.rs` registration) + C10-04 (842adba: the `TelemetryCache` collector wrapped — every re-collect runs `spike()` before `ramsleuth_telemetry::collect()` so the SMU PM `MCLK` is sampled at the operating frequency, D-2; `rpc.rs`/`cache.rs`/CLI untouched) landed as one --no-ff merge 0d86359 (new v2-development tip; c10-03/c10-04 branches pruned, c10-04 worktree removed; review gate at the c10-04 tip: `cargo check --workspace` clean, daemon 34+9 green (5 spike tests + existing), clippy -D warnings zero; both waves file-disjoint — clean merge, no rebase); PENDING: C10-06 QA on the v2-development tip (full workspace regression debug+release, clippy zero, zero-wire audit §8, operator live run)

- [DONE] ID: C10-01 | STATUS: SUCCESS | BRANCH: branch/chunk-c10-01
DECISION: decode_density 0x0D arm 16→32 Gb + P6-04 doc/inline reworded to the C10 reconciliation; check+clippy clean; commit 8ac43e6 pushed
AHEAD: C10-01 NEVER merges standalone — C10-02 must re-anchor the 4 pinned tests to 32768 in the same file, then one atomic --no-ff merge at the c10-02 tip

- [DONE] ID: C10-03 | STATUS: SUCCESS | BRANCH: branch/chunk-c10-03
DECISION: created `crates/ramsleuth-daemon/src/dram_spike.rs` (256 MiB / 250 ms single-flight DRAM load: `SPIKE_RUNNING` AtomicBool gate, RAII `RunningGuard`, `spike()` + `spike_run` strided read/write loop with a `black_box` sink, 5 tests) + registered it in `lib.rs` (mod + doc bullet + `pub use dram_spike::spike`); `cargo test -p ramsleuth-daemon` 34+9 green, clippy `-D warnings` zero, `cargo check --workspace` clean; commit 5e89c7e pushed
AHEAD: C10-04 must fork from `branch/chunk-c10-03` and wrap the injected collector with `spike()` immediately before `ramsleuth_telemetry::collect()` (D-2); `rpc.rs`/`cache.rs` stay untouched

- [DONE] ID: C10-02 | STATUS: SUCCESS | BRANCH: branch/chunk-c10-02
DECISION: re-anchored the 5 live-module spots in `spd_decode.rs`'s test module to 32 Gb / 32768 Mbit / 32 GiB (4 test assertions + fixture doc -> "64 GiB kit (2x32 GiB)"; fixture bytes unchanged); `cargo test -p ramsleuth-telemetry` 187/187 green, clippy `-D warnings` zero; commit 60f3345 pushed
AHEAD: the c10-02 tip (8ac43e6 + 60f3345) is the Wave 1 atomic merge boundary - one --no-ff merge of branch/chunk-c10-02 lands both commits; the untouched DDR5 family + facade synthetic tests stayed green

- [DONE] ID: C10-04 | STATUS: SUCCESS | BRANCH: branch/chunk-c10-04
DECISION: wrapped the `TelemetryCache` collector in `main.rs` with `|| { spike(); ramsleuth_telemetry::collect() }` (the D-2 hook at the collector boundary, not rpc.rs) + `spike` import + header-doc note; worked in worktree ../ramsleuth-wt-c10-04 (main tree was occupied by the C10-02 wave); 34+9 daemon tests green, clippy `-D warnings` zero, `cargo check --workspace` clean; commit 842adba pushed
AHEAD: Wave 2 complete — one review + `--no-ff` merge of branch/chunk-c10-04 lands C10-03+C10-04 together (c10-03 prunes with c10-04, the C9-03 pattern); file-disjoint from the Wave 1 c10-02 atomic merge

- [DONE] ID: C10-REVIEW-WAVE1 | STATUS: SUCCESS | BRANCH: v2-development
DECISION: audited C10-01 (8ac43e6) + C10-02 (60f3345) against plans/PLAN-CYCLE10.md §3 — 0x0D 16→32 Gb + C10 doc/inline reword + the 5 re-anchored test spots exactly per the frozen shape (published family 0x10..=0x17, overflow gate, DDR5 arm, fixture bytes, facade + DDR5-family tests untouched; zero-wire) — merged once as an atomic --no-ff 5ddec39 (both commits in v2-development), c10-01/c10-02 pruned; gate at the c10-02 tip: telemetry 187/187 green, clippy -D warnings zero
AHEAD: Wave 2 (branch/chunk-c10-04 @ 842adba, c10-03 lineage) must rebase onto v2-development @ 5ddec39 before review (house rule; file-disjoint); v2-development stays unpushed (operator-gated)

- [DONE] ID: C10-REVIEW-WAVE2 | STATUS: SUCCESS | BRANCH: v2-development
DECISION: audited C10-03 (5e89c7e) + C10-04 (842adba) together against plans/PLAN-CYCLE10.md §3 (D-2/D-3) — dram_spike (256 MiB / 250 ms, `SPIKE_RUNNING` AtomicBool single-flight + RAII `RunningGuard`, strided read/write loop with a `black_box` sink, 5 tests) + 3-line `lib.rs` registration + `main.rs` collector wrap exactly per the frozen shape; `rpc.rs`/`cache.rs`/CLI/protocol untouched, std-only, zero new deps — merged once as --no-ff 0d86359 (both commits landed in one merge; c10-03 pruned with c10-04, c10-04 worktree removed); gate at the c10-04 tip: `cargo check --workspace` clean, daemon 34+9 green, clippy -D warnings zero
AHEAD: C10-06 QA must run on the v2-development tip 0d86359 (full workspace regression debug+release — 546/546 expected, clippy zero, zero-wire audit §8, operator live run); v2-development stays unpushed (operator-gated)

- [ARCHIVED] Cycle 9 (v2.0.0 RAM topology / Graphs lifecycle / CPU-temp hwmon / right-column layout) — 8 chunks C9-01..C9-08 merged 2026-09-16 (6cc9840..c2f40a5); RAM topology header (rank word + slot note, no wire change) + Graphs telemetry lifecycle (force-on open / revert close) + in-window Poll control + CPU-temp hwmon (k10temp/zenpower name-match) + right-column width/height fill; QA PASS-WITH-MANUAL-LIVE-VERIFY 541/541, clippy zero, 6 binaries, zero-wire; see MASTER_LOG.md Cycle 9 section + plans/PLAN-CYCLE9.md.
- [ARCHIVED] Cycle 8 (v2.0.0 header truth) — 11 chunks C8-01..C8-11 merged 2026-09-16 (c6ef83d..5730b33); AGESA/SMU provenance relabel + RAM capacity + N/A bare/gray styling + gear-metric split; QA PASS-WITH-MANUAL-LIVE-VERIFY 527/527, clippy zero, 6 binaries, zero new deps; see MASTER_LOG.md Cycle 8 section + plans/PLAN-CYCLE8.md.
- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
