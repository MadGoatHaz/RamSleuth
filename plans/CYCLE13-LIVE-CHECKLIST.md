# Cycle 13 — Live GUI Verification Checklist (5950X operator run)

Cycle 13 is a **pure GUI-layout** cycle: no wire changes, no new
dependencies, no telemetry-logic changes. All automated gates passed
(see the tail of this file); what remains is the interactive,
hardware-in-the-loop confirmation below, which needs the 5950X host
with root (the daemon reads SPD/SMU through privileged paths).

## How to run

On the 5950X host, from the repo root (after `cargo build
--release --workspace`):

```
sudo target/release/ramsleuth-gui
```

Run with the dev daemon live so all three zones populate:

```
sudo target/release/ramsleuth-daemon --socket /tmp/ramsleuth.sock &
sudo target/release/ramsleuth-gui --socket /tmp/ramsleuth.sock
```

Confirm each check by eye; mark PASS / MISMATCH and note what you
saw.

## Check (a) — Window opens tight at ~960×600

- [ ] The main window opens at a tight **~960×600** default — NOT the
      old oversized 1400×900.
- [ ] No awkward dead zones at the default size: all three panels
      (1 · MEMORY CONTROLLER, 2 · BENCHMARK ENGINE, 3 · HARDWARE &
      SPD) carry content edge-to-edge.
- [ ] Drag the window down toward the **~884×600 minimum**: the
      layout degrades gracefully — the footer (daemon socket path
      line) is **not clipped**, and no panel collapses to a negative
      allocation.

## Check (b) — Panel 3 DIMM cards side-by-side (2-column grid)

- [ ] The HARDWARE & SPD panel's DIMM cards (**0x52** and **0x53**)
      now sit **SIDE-BY-SIDE** in a horizontal 2-column grid, not
      stacked vertically as before.
- [ ] The two cards are top-aligned on the same row even when one
      is populated and the other all-`N/A` (unequal heights, no
      stagger).
- [ ] Card values wrap inside their half-column (a long product
      line does not clip or overlap the neighbour card).
- [ ] (If 4 DIMMs are present on a 4-slot board: they'd flow into a
      balanced **2×2** grid — verify if applicable.)

## Check (c) — Panel 1's three columns fill the parent width

- [ ] The MEMORY CONTROLLER panel's 3 section columns
      (Clocks & Ratios / Primary Timings · Secondary Timings /
      Tertiary & Turnarounds · CAD Bus Drive & Termination / Active
      System Voltages) are **equal-width** and span the panel's full
      width — **no large dead void** between the 3rd column
      (CAD Bus Drive & Termination / Active System Voltages) and the
      right border.
- [ ] Long section titles wrap onto a second line inside their
      column rather than bleeding into the next one.

## Check (d) — Panels 2 and 3 stretch to the right window margin

- [ ] Panel 2 (BENCHMARK ENGINE) and Panel 3 (HARDWARE & SPD)
      **stretch to the right window margin uniformly** — no dead
      space along the far-right border.
- [ ] The benchmark table's last metric column fills the remaining
      table width (`remainder()` column), so the table spans the
      frame with no right-border gap.
- [ ] Resizing the window wider keeps panels 2 and 3 filling the new
      width (no fixed-width dead strip appearing).

## Check (e) — High-DPI / scaled display

- [ ] On a scaled (high-DPI) display the layout stays
      **proportionally clean** — egui works in logical points and
      eframe hands OS scaling to the compositor, so text and frames
      should scale uniformly with no clipping, no dead zones, and no
      garbled rows at the 960×600 default.

## Automated gates (all passed — no action needed)

- Regression: **555/555 debug, 555/555 release** (Cycle 12 baseline
  count unchanged; the re-anchored layout pins were re-verified in
  the run above).
- Clippy `--workspace --all-targets -D warnings`: **zero**.
- MSRV: `rust-version = "1.75"` in the root `Cargo.toml`.
- Six release binaries build: bench, client, daemon, gui,
  telemetry, tui.
- No-wire audit vs baseline d62b21f: **zero** changes in
  `ramsleuth-protocol/`, `ramsleuth-telemetry/`, `ramsleuth-tui/`,
  `ramsleuth-client/`, `ramsleuth-daemon/`, `ramsleuth-bench/`, and
  `Cargo.toml` / `Cargo.lock` (no new deps). The only modified
  sources are `crates/ramsleuth-gui/src/`: `main.rs`,
  `telemetry_zone.rs`, `status_zone.rs`, `bench_zone.rs`.

---

**If any check above is a MISMATCH, report it back to the operator
with what you saw (panel, symptom, window size, display scale).**
These live checks are the **only deferred verifications** for
Cycle 13 — every automated gate passed.
