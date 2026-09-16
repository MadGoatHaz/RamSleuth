# RamSleuth v2 — Developer Log / Lease Board

Base branch: `v2-development` @ 9ea4b3b (clean; 515/515 debug+release; clippy zero; MSRV 1.75; 6 binaries; unpushed). Plan: `plans/PLAN-CYCLE8.md` (Cycle 8 — v2.0.0 header truth: AGESA/SMU honest relabel, RAM total/per-slot, N/A gray styling, GEAR_DOWN split; the Cycle 7 plan is retained as `plans/PLAN-CYCLE7.md`, Cycle 6 as `plans/PLAN-CYCLE6.md`, Phase 6/Cycle 5 as `plans/PLAN-PHASE6.md`, Phase 5 as `plans/PLAN-PHASE5.md`, Phase 3 as `plans/PLAN-PHASE3.md`, Phase 2 as `plans/PLAN-PHASE2.md`, Phase 1 as `plans/PLAN.md`). Sign in/out under `@@@ ACTIVE_WORKERS @@@` per the lease protocol. Durable cycle history lives in `MASTER_LOG.md`.

@@@ ACTIVE_WORKERS @@@
(no active leases)

@@@ CURRENT_STATE @@@
Cycle 8 (v2.0.0 header truth) IN FLIGHT — C8-01 (wire freeze) MERGED (c6ef83d; 516/516). C8-02 (spd_decode memory-type classification re-anchored to byte 0x02) MERGED into v2-development @ 8e4ceb7 (2026-09-16; review C8-02-REVIEW passed: only spd_decode.rs changed, no Cargo.toml/lock diff; byte-0x02 primary — 0x0C -> DDR4 512 B / DDR5 1024 B, 0x0B -> DDR3; legacy byte-0x00 check + image-length kept as documented fallbacks; live shape 0x00=0x23 + 0x02=0x0C classifies DDR4, part decodes from the 0x149 primary; ddr5 fixture maker re-anchored 0x92 -> 0xC2; total-device decode untouched = C8-03; 518/518 debug+release, clippy -D warnings zero; local branch + impl worktree pruned; v2-development unpushed, operator-gated). C8-03 (same file, total-device decode) is next — fork/rebase onto 8e4ceb7. C8-04 (facade.rs) implemented @ f765b76 (pushed; MemTotal-preferred total_capacity + new pure sum_dimm_sizes; 517/517, clippy zero, no manifest diff) — awaiting review (C8-04-REVIEW).

- [DONE] ID: C8-02-REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-c8-02 -> v2-development (merge 8e4ceb7; local branch pruned, impl worktree removed)
DECISION: review pass — only spd_decode.rs changed (181-line diff, no manifest diff); classification re-anchored to byte 0x02 (0x0C key -> DDR4 512 B / DDR5 1024 B, 0x0B -> DDR3) with the legacy byte-0x00 check + image-length last resort preserved; decode_part hits the 0x149 primary on the live shape; ddr5 fixture maker re-anchored 0x92 -> 0xC2 (the 0x02 JEP106 continuation sharing); total-device decode (0x80/0x81/0x0C) untouched for C8-03; 518/518 debug+release, clippy -D warnings zero; merged --no-ff, v2-development left unpushed.
AHEAD: new v2-development tip 8e4ceb7 — C8-03 (same file) forks/rebases onto it before review; its byte-0x02 anchors are the rebase target.

- [DONE] ID: C8-04 | STATUS: SUCCESS | BRANCH: branch/chunk-c8-04 (f765b76, pushed origin)
DECISION: total_capacity flipped to MemTotal-preferred (D-3) with new pure sum_dimm_sizes as the non-meminfo fallback (SPD sum demoted; per-slot dimm_sizes stay SPD-derived, devices-total note per D-2); tests re-pointed (f3 → sum_dimm_sizes) + new (f3′) prefers-meminfo / non-meminfo-fallback; 517/517 debug+release, clippy zero.
AHEAD: C8-03 (spd_decode.rs, different file — no shared lines) lands the devices-total decode giving per-slot 16 GiB; the total stays MemTotal-preferred regardless, no rebase interaction expected.

- [DONE] ID: C8-02 | STATUS: SUCCESS | BRANCH: branch/chunk-c8-02 (5bc64a6, pushed origin)
DECISION: SPD classification re-anchored to byte 0x02 (SPD_TYPE_DDR4 key 0x0C = DDR4 512 B / DDR5 1024 B, DDR3 0x0B) with the legacy byte-0x00 check + image-length kept as documented fallbacks; live host shape (0x00 = 0x23 junk, 0x02 = 0x0C) now classifies DDR4 and the part decodes from the 0x149 primary; 2 tests added, 518/518 debug+release, clippy zero.
AHEAD: C8-03 (same file) builds on the classify_ddr5/is_ddr4 byte-0x02 anchors — the 0x02 byte is shared with the JEP106 continuation nibble (the ddr5 fixture maker moved 0x92 -> 0xC2); total-device decode (0x0C/0x0D/0x80/0x81) stays untouched until C8-03.

- [DONE] ID: C8-01 | STATUS: SUCCESS | BRANCH: branch/chunk-c8-01 (f1c039d, pushed origin)
DECISION: wire freeze landed — SystemPlatform += smu_version (appended, shape-checked, agesa narrowed to BIOS-string-only); 27 test literals + 2 facade asserts co-landed across 16 files (incl. 4 unlisted daemon/protocol/telemetry-main); 516/516 debug+release, clippy zero.
AHEAD: wave-2 chunks (C8-11 main.rs, C8-06/C8-10 telemetry_zone.rs, C8-07 status_zone.rs, C8-09 graph.rs, C8-04 facade.rs) must rebase onto f1c039d — their SystemPlatform literals now require the smu_version line.

- [DONE] ID: C8-01-REVIEW | STATUS: SUCCESS | BRANCH: branch/chunk-c8-01 → v2-development (merge c6ef83d; local branch pruned, impl worktree absent)
DECISION: review pass — platform.rs matches the §3 frozen shape exactly (smu_version appended last, agesa narrowed to BIOS-string-only, shape-checked smu_version_from); 516/516 debug+release, clippy -D warnings zero; merged --no-ff, v2-development left unpushed.
AHEAD: new v2-development tip c6ef83d — every in-flight Cycle 8 branch (C8-02 onward) rebases onto it before review.
- [ARCHIVED] Cycle 7 (v2.0.0 polish) — 22 chunks C7-01..C7-22 merged 2026-09-16; polling lifecycle + settings + AGESA/vendor + Panel 1/2 + burn-in + SPD part + Graphs window; 515/515; see MASTER_LOG.md Cycle 7 section + plans/PLAN-CYCLE7.md.
- [ARCHIVED] Cycle 6 (GUI workstream) — 29 chunks C6-01..C6-30 (C6-08 retired) merged 2026-09-15; data-model wave + GUI wave; 452/452; see MASTER_LOG.md Cycle 6 section + plans/PLAN-CYCLE6.md.
- [ARCHIVED] Cycle 5 (Phase 5) — P5-13/P5-14 ryzen_smu DKMS install fixes (build/install version args + install-state skip-check) merged; see MASTER_LOG.md.
