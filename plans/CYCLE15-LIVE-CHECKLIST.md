# Cycle 15 — Live GUI Verification Checklist (5950X operator run)

Cycle 15 is a **pure GUI-layout** cycle: no wire changes, no new
dependencies, no new crates. Three operator-reported defects on the
main-view right column are fixed and need visual confirmation on the
5950X host (root required — the daemon reads the SMU PM table and SPD
through privileged paths):

- **DEFECT-1:** panels **"2 · BENCHMARK ENGINE"** and **"3 · HARDWARE
  & SPD"** were **missing their right-side CYAN border strokes** (the
  1.0 px frame stroke is centered on the frame rect — its 0.5 px outer
  half painted outside the allocation, which was flush at the
  CentralPanel edge, and was clipped).
- **DEFECT-2:** the right-column frames were **clipped at the viewport
  right edge** (the Cycle-13 "flush" split left zero right clearance).
- **DEFECT-3:** **asymmetric outer margins** — panel 1 had a balanced
  8 pt left margin; the right side had none.

The fix (C15-01, `main.rs`): a symmetric `OUTER_MARGIN = 8 pt` on both
sides, the window widened 960 → **968×600** default / 884 →
**892×600** min (the 8 pt right margin bought by the widening — the
content geometry is byte-identical to the old 960 state: left column
504 pt, right 440 pt), the split computed stroke-aware against the
inner width with a trailing margin, and the row's injected 8 pt
`item_spacing` zeroed (the true root cause of the flush edge). C15-02
re-anchored the `telemetry_zone.rs` docs only (960×600 → 968×600).

All automated gates passed (actual results at the tail of this
file). What remains is the visual checks below.

## How to run

On the 5950X host, from the repo root:

```
sudo target/release/ramsleuth-daemon --socket /tmp/ramsleuth.sock &
sudo target/release/ramsleuth-gui --socket /tmp/ramsleuth.sock
```

The window opens at **968×600**. Confirm each check by eye; mark
PASS / MISMATCH and note what you saw.

## Check (a) — PANELS 2 & 3: complete 4-sided CYAN borders

- [ ] Panel **"2 · BENCHMARK ENGINE"** and panel **"3 · HARDWARE &
      SPD"** (the right column) each show a **complete, fully visible
      4-sided CYAN border** — the **right** stroke in particular is no
      longer missing/clipped.
- [ ] The border strokes are **identical to panel 1's** ("1 · MEMORY
      CONTROLLER & SUBTIMINGS") in color and weight (1.0 px CYAN).
- [ ] The borders of all three panels look uniform (the frames were
      always identical; only the right allocation changed).

## Check (b) — SYMMETRIC OUTER MARGINS

- [ ] The gap between **panel 1's left border and the window left
      edge** equals the gap between **panels 2 & 3's right borders and
      the window right edge** — visually balanced (both = 8 pt).
- [ ] The window opens **8 pt wider than before** (968 pt, not 960) —
      the extra width is the new right margin; the content itself is
      unchanged (the same zone widths as the Cycle 14 state).

## Check (c) — NO VIEWPORT CLIPPING

- [ ] **No right-column content is clipped at the viewport right
      edge** — the bench table's last column and the SPD cards end
      cleanly inside their frames (pre-fix, the frames bled to the
      clip edge).
- [ ] Nothing in any of the three panels is cut off at any window
      edge.

## Check (d) — MINIMUM WINDOW + HIGH DPI

- [ ] **Shrink the window to the 892×600 minimum** — nothing clips
      (both columns at/above their minima; the split sum is exact).
- [ ] **At 150% display scaling** the proportions hold (the layout is
      logical points; the margins/borders scale with everything).

## Check (e) — NO REGRESSION

- [ ] The **Graphs window** spawns/closes normally (the second native
      viewport, 5 series).
- [ ] **Bench / burn-in runs** still populate the grid + stay
      responsive during the run (the Cycle 14 fix intact).
- [ ] The **SPD 2×2 cards** (panel 3) render as before; the **Zone 1
      3-column** layout (panel 1) is unchanged; the **settings panel**
      works.
- [ ] The 2 s telemetry cadence is unaffected (values refresh
      continuously; no extra privileged reads).

## Reporting

Report any MISMATCH back to the operator with the check letter and
what was observed (a screenshot of the right column is ideal for
checks a–c). These are the **only deferred verifications** for Cycle
15 — everything else is covered by the automated gates below.

## Automated gates (all passed on `v2-development` @ 035ad3e, baseline 4f0c6d9)

- **Tests (debug):** `cargo test --workspace` — **556 passed, 0
  failed** (the Cycle 14 count held — C15-01 added assertions in
  place to `render_zones_right_column_split_two_stacked_slices`
  (margin symmetry == `OUTER_MARGIN` ± 1.0 pt + 0.5 pt stroke
  clearance, case 0), zero tests added/removed).
- **Tests (release):** `cargo test --workspace --release` — **556
  passed, 0 failed**.
- **Clippy:** `cargo clippy --workspace --all-targets -- -D
  warnings` — **zero warnings**.
- **MSRV:** root `Cargo.toml` `rust-version = "1.75"` — unchanged.
- **Zero deps:** `git diff 4f0c6d9..035ad3e -- Cargo.toml Cargo.lock`
  — **empty** (no new dependencies, no new crates).
- **Six release binaries** (`cargo build --release --workspace`):
  `ramsleuth-bench` (650,424 B), `ramsleuth-client` (760,984 B),
  `ramsleuth-daemon` (1,946,280 B), `ramsleuth-gui` (16,227,504 B),
  `ramsleuth-telemetry` (706,792 B), `ramsleuth-tui` (1,384,008 B) —
  all build.
- **No-wire audit** (`git diff 4f0c6d9..035ad3e`): only
  `crates/ramsleuth-gui/src/main.rs` (the layout fix) +
  `crates/ramsleuth-gui/src/telemetry_zone.rs` (docs-only) — the
  protocol / telemetry / daemon / TUI / CLI / bench crates are all
  0-diff. **Zero wire changes.**
- **No-panic audit:** the split math is all `.max(0.0)`-clamped
  (allocations never negative), no new `panic!` / `unwrap()` /
  `expect()` in the changed code; below-min probes degenerate
  left-first as before.
