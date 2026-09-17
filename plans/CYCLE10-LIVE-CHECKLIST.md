# Cycle 10 — Live-Run Verification Checklist (operator)

The automated QA gates for Cycle 10 passed fully on `v2-development @ 5610e67`
(546/546 tests debug AND release, clippy zero, all 6 binaries built,
**zero-wire** — protocol/gui/tui/client/bench byte-identical to the Cycle 9
baseline, and the only telemetry change is the `0x0D` density arm of
`spd_decode.rs` re-anchored from 16 Gb to 32 Gb, with no wire-struct or serde
change). The **live-run check is deferred to the operator** because it needs
**interactive sudo** — the daemon reads SMU/PM telemetry as root — plus an
operator-attended display on the 5950X host.

## How to run (5950X host)

Start the GUI with sudo (the daemon reads telemetry as root):

```bash
sudo target/release/ramsleuth-gui
```

Wait for the daemon socket to connect, then check each item below against what
is actually rendered.

## Checks

- [ ] **(a) RAM line** reads **"RAM: 62.7 GiB (2x32 GiB Single-Rank) 3200 MT/s | Dual-Channel"**
      — the per-DIMM capacity is now **32 GiB** (the operator-confirmed truth;
      the `0x0D` density code was re-anchored from 16 Gb to 32 Gb). It must
      **NOT** read `"2x16 GiB"` (the old P6-04 part-number-suffix guess), and
      it must carry the **rank word** `Single-Rank` (**NOT** `Dual-Rank`). The
      slot note **"2 of 4 slots SPD-visible" is GONE**: MemTotal (62.7 GiB)
      now falls at or below the SPD sum (64 GiB), so the note's condition no
      longer holds.
- [ ] **(b) Live MCLK** — the **Clocks & Ratios** row and the **10-min
      sparkline** read the **operating frequency** during polling (**not** a
      low idle frequency). The daemon spikes DRAM (~250 ms) immediately before
      each re-collect, so the MCLK sample is taken at the operating frequency.
- [ ] **(c) Spike latency** — the spike adds **~250 ms latency only on the
      re-collect** (a cold cache or TTL-expired `get()`); the **in-TTL clone
      path is latency-unchanged** (no spike runs, the collector is not
      called). The GUI's **2 s poll cadence absorbs it** — no visible stutter.
- [ ] **(d) No "spiking" indicator** — there is **no new indicator** in the
      GUI (C10-05 no-op); the spike's effect rides the **existing MCLK field**
      only.

## Reporting

**Report any mismatch back to the operator** with the exact string rendered
vs. the expected string. These 4 checks are the **only deferred
verifications** — all automated gates passed (546/546 tests debug + release,
clippy zero, 6 binaries, zero-wire).
