# Cycle 8 — Live-Run Verification Checklist (operator)

The automated QA gates for Cycle 8 passed fully on `v2-development @ 5730b33`
(527/527 tests, clippy zero, all 6 binaries built; the frozen-shape change is
additive `smu_version` only). The **live-run check is deferred to the operator**
because it needs **interactive sudo** — the daemon reads SMU/PM telemetry as
root — plus an operator-attended display on the 5950X host.

## How to run (5950X host)

Start the GUI with sudo (the daemon reads telemetry as root):

```bash
sudo target/release/ramsleuth-gui
```

(or the equivalent GUI launch for this host). Wait for the daemon socket to
connect, then check each item below against what is actually rendered.

## Checks

- [ ] **(a) Header** reads **"(BIOS: 5601, SMU 56.78.0)"** — NOT "AGESA 56.78.0".
- [ ] **(b) RAM line** reads **"RAM: 62.68 GiB (2x16 GiB) 3200 MT/s | Dual-Channel"**
      — NOT "4 GiB (2x2 GiB)".
- [ ] **(c) SPD part number** reads **"F4-3600C18-32GVK"** (decoded from byte `0x149`).
- [ ] **(d) Zone 1 (Clocks & Ratios)** has **NO standalone "gear" row**;
      `GEAR_DOWN` and `CR` are **SEPARATE rows**; all N/A values are gray bare
      "N/A" (no red, no verbose "(parse error: ...)" strings); the AMBER
      warnings (1:2 desync, VDDCR_SOC high) **still show**.
- [ ] **(e) Zone 2 (bench)**: unmeasured / not-started cells read gray bare "N/A".
- [ ] **(f) Zone 3 (SPD cards)**: absent fields read gray bare "N/A"; the
      daemon-disconnected line and the `! <error>` line **STAY red (CRIMSON)**.
- [ ] **(g) Graphs window**: the `VDDCR_CPU` row reads **"VDDCR_CPU — N/A"**
      (bare, AMBER accent kept).

## Reporting

**Report any mismatch back to the operator** with the exact string rendered vs.
the expected string. These 7 checks are the **only deferred verifications** —
all automated gates passed (527/527 tests, clippy zero, 6 binaries,
frozen-shape additive `smu_version` only).
