# Cycle 14 — Live GUI Verification Checklist (5950X operator run)

Cycle 14 is a **pure bug-fix** cycle: no wire changes, no new
dependencies, no new crates. Two operator-reported bugs are fixed and
need interactive, hardware-in-the-loop confirmation on the 5950X host
(root required — the daemon reads the SMU PM table and SPD through
privileged paths):

- **BUG-1 (M1+M2):** the first clock sample after app start reported a
  low idle frequency (intermittent). Fixed daemon-side: the first
  `TelemetryCache::get()` now does a **warm-up double-collect**
  (`cache.rs`) and the collector closure inserts a **150 ms settle
  delay** after the DRAM spike, before re-reading the PM table
  (`main.rs`, `CLOCK_SETTLE`).
- **BUG-2 (C14-03):** during a long bench/burn-in run, the GUI's
  bench/burn-in stream drained under a single held write lock, so the
  render thread (main window **and** the Graphs window) froze for the
  whole run. Fixed in `update.rs`: `run_bench` / `run_burn_in` now
  take the shared `&RwLock<TelemetryData>` and lock it **briefly, per
  mutation** — the guard is always released before the next stream
  `recv()`.

All automated gates passed (actual results at the tail of this
file). What remains is the checks below.

## How to run

On the 5950X host, from the repo root:

```
sudo target/release/ramsleuth-gui
```

with the daemon running:

```
sudo target/release/ramsleuth-daemon --socket /tmp/ramsleuth.sock &
sudo target/release/ramsleuth-gui --socket /tmp/ramsleuth.sock
```

Confirm each check by eye; mark PASS / MISMATCH and note what you saw.

## Check (a) — CLOCK FIRST-SAMPLE reports operating frequency

- [ ] Start the app **fresh**: stop the daemon, then start the daemon
      and the GUI (a cold `TelemetryCache`).
- [ ] On the **first** telemetry sample, MCLK / UCLK / FCLK report the
      **operating** frequencies — not the low idle values.
- [ ] The first sample may arrive **~1 s later** than in Cycle 13
      (the warm-up double-collect + 150 ms settle). That delay is
      expected, not a hang.
- [ ] MCLK matches the expected operating frequency: for DDR4-3200,
      **MCLK ≈ 1600 MHz** — confirm it is NOT a low idle value such as
      ~400 MHz. (UCLK/FCLK should sit at the corresponding operating
      values for the board.)
- [ ] **Repeat the full app restart 3–5 times.** The bug was
      intermittent, so the fix counts only if the first sample is
      consistently the operating frequency on every restart — a
      single good start is not proof.

## Check (b) — BURN-IN: both windows stay responsive

- [ ] Click **"Run Burn-In"** with a duration of **≥ 2 minutes**.
- [ ] While it runs, the **main window** stays responsive: the mouse
      moves freely, the **per-iteration burn-in row updates LIVE**
      (iteration / elapsed ticking), and the header and all zones keep
      rendering every frame.
- [ ] The **Graphs window** ALSO stays responsive — not frozen (it
      updates continuously, same as before the fix).
- [ ] Click **"Cancel"** mid-run: the run **stops** (burn-in row
      settles, no error banner, the buttons re-enable).
- [ ] (Pre-fix behaviour: both windows froze for the whole duration.)

## Check (c) — RUN FULL / MEMORY ONLY stays responsive

- [ ] Click **"Run Full"** — the UI stays responsive during the
      (short) run. Same defect class as the burn-in freeze, previously
      imperceptible due to the short duration; confirm no stutter /
      frozen frames while the progress list ticks.
- [ ] Repeat once with **Memory Only** — same expectation.

## Check (d) — NO REGRESSION

- [ ] Normal telemetry polling at the **2 s cadence** still works:
      zone values refresh continuously, history / graph curves grow,
      and the in-TTL clone path means the SMU is **not** re-sampled on
      every frame (no extra privileged reads).
- [ ] **Bench results still populate correctly** after a run: the 4×4
      grid fills, the terminal grid lands in state, and the F2/F3
      exports still work.
- [ ] The **cancel path** works for a normal bench run too (Cancel
      mid-run → clean stop, no error).
- [ ] No error banners / red text under normal operation (daemon
      status green, no "unexpected …" contract errors).

## Reporting

Report any MISMATCH back to the operator with the check letter, what
was observed, and (for check a) the exact MCLK value seen. These are
the **only deferred verifications** for Cycle 14 — everything else is
covered by the automated gates below.

## Automated gates (all passed on `v2-development` @ 80b2374, baseline d60cd34)

- **Tests (debug):** `cargo test --workspace` — **556 passed, 0
  failed** (Cycle 13 baseline 555 + 1 new C14-03 in-flight reader
  regression test `run_in_flight_burn_in_keeps_the_lock_brief_for_readers`;
  the cache/update test anchors re-pointed to the warm-up
  double-collect).
- **Tests (release):** `cargo test --workspace --release` — **556
  passed, 0 failed**.
- **Clippy:** `cargo clippy --workspace --all-targets -- -D
  warnings` — **zero warnings**.
- **MSRV:** root `Cargo.toml` `rust-version = "1.75"` — unchanged.
- **Zero deps:** `git diff d60cd34..HEAD -- Cargo.toml Cargo.lock` —
  **empty** (no new dependencies, no new crates).
- **Six release binaries** (`cargo build --release --workspace`):
  `ramsleuth-bench` (650,424 B), `ramsleuth-client` (760,984 B),
  `ramsleuth-daemon` (1,946,280 B), `ramsleuth-gui` (16,227,280 B),
  `ramsleuth-telemetry` (706,792 B), `ramsleuth-tui` (1,384,008 B) —
  all build.
- **No-wire audit** (`git diff d60cd34..HEAD`):
  `crates/ramsleuth-protocol/` — empty; `crates/ramsleuth-telemetry/`
  — empty; `crates/ramsleuth-tui/`, `crates/ramsleuth-client/`,
  `crates/ramsleuth-bench/` — empty. Only `crates/ramsleuth-daemon/`
  (`src/cache.rs`, `src/main.rs`) and `crates/ramsleuth-gui/`
  (`src/update.rs`) changed — zero wire / telemetry / TUI / CLI /
  bench changes.
- **No-panic audit:** the cache warm-up, the settle closure, and the
  per-tick brief locks use no `panic!` / `unwrap()` / `expect()` on
  hardware-derived data (only the codebase-convention
  `state.write().unwrap()` on the shared lock; hardware values are
  validated with `is_finite()` / `> 0.0` before use; the tick's
  `tier` index is bounded to the 4-slot grid). The lock scope is
  verified brief: every guard is dropped **before** the next stream
  `recv()`, and the `CancelBenchmark` is sent with no lock held.
