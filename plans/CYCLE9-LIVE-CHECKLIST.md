# Cycle 9 — Live-Run Verification Checklist (operator)

The automated QA gates for Cycle 9 passed fully on `v2-development @ c2f40a5`
(541/541 tests, clippy zero, all 6 binaries built, **zero-wire** — no telemetry
shape change this cycle). The **live-run check is deferred to the operator**
because it needs **interactive sudo** — the daemon reads SMU/PM telemetry as
root — plus an operator-attended display on the 5950X host.

## How to run (5950X host)

Start the GUI with sudo (the daemon reads telemetry as root):

```bash
sudo target/release/ramsleuth-gui
```

Wait for the daemon socket to connect, then check each item below against what
is actually rendered.

## Checks

- [ ] **(a) RAM line** reads **"RAM: 62.7 GiB (2×16 GiB Single-Rank) 3200 MT/s | Dual-Channel"**
      and, because the OS total (62.7 GiB, all 4 DIMMs) exceeds the SPD sum
      (32 GiB, only 2 bound to `ee1004`), carries the slot note **"2 of 4 slots
      SPD-visible"** (e.g. `… | Dual-Channel | 2 of 4 slots SPD-visible`). It
      must show the **rank word** `Single-Rank` — **NOT** the rank-less
      `"2x16 GiB"`, and **NOT** the false `"2x32 GiB Dual-Rank"` (the DIMMs are
      16 GiB single-rank; the 62.7 GiB total reflects 4 physical DIMMs of
      which 2 have no bound SPD EEPROM).
- [ ] **(b) Graphs window lifecycle** — opening the Graphs window
      **force-enables telemetry streaming even with auto-refresh OFF** in
      Settings; closing it — via **either** the button **or** the WM close
      button — **reverts to the original Settings value** (auto-refresh back to
      OFF).
- [ ] **(c) Graphs "Poll" combo** — the `Poll` combo in the Graphs header
      (presets **500 / 1000 / 2000 / 5000 / 10000 ms**) **changes the sample
      cadence live** (no restart) and **matches the Settings poll interval**
      (it writes the shared `settings.poll_interval_ms`, so the two never
      diverge).
- [ ] **(d) CPU TEMP (°C) track** — plots a **finite value** (**~29–33 °C**,
      from the `k10temp` hwmon sensor) instead of `N/A`.
- [ ] **(e) Right column layout** — the **BENCHMARK ENGINE** and
      **HARDWARE & SPD** panels have **identical full-width borders**;
      **HARDWARE & SPD snaps directly beneath BENCHMARK ENGINE**; and both
      **uniformly fill the column (no dead space)**. Zone 1
      (**MEMORY CONTROLLER**) **fills the left column**.
- [ ] **(f) Small window** — at a small window size the **right column falls
      back to scrolling** (the `ScrollArea` overflow fallback) — **no crash**.
- [ ] **(g) Flapping daemon** — a **flapping daemon degrades to placeholders**
      (+ the red status line) **without crashing**.

## Reporting

**Report any mismatch back to the operator** with the exact string rendered vs.
the expected string. These 7 checks are the **only deferred verifications** —
all automated gates passed (541/541 tests, clippy zero, 6 binaries,
zero-wire).
