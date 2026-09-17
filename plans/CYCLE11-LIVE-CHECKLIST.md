# Cycle 11 — Live-Run Checklist (5950X host, GUI)

Operator-gated manual verification for the C11 atomic merge
(`d170a3f`: DDR4 rank decode reads byte `0x0C` bits 3:4 per
JESD79-4; density `0x0D` reverted to 16 Gb — the P6-04 value).
All automated gates passed (bottom of this file); the three
checks below are the only deferred verifications and require a
live SPD read on the 5950X host.

## How to run

On the 5950X host (interactive sudo required — the GUI reads the
SPD EEPROMs + AMD PM/SMN paths as root):

```
cd /path/to/RamSleuth
sudo target/release/ramsleuth-gui
```

Wait a few seconds for the first telemetry baseline (the RAM
header must stop reading `N/A`). Then check (a)–(c).

## Check (a) — RAM header line (top of the GUI)

The header line must now read:

```
RAM: 62.7 GiB (2x32 GiB Dual-Rank) 3200 MT/s | Dual-Channel | Mode: ...
```

- The rank word is **Dual-Rank** (not `Single-Rank`) — `rank_word`
  renders `rank == 2` as `Dual-Rank` (GUI code unchanged this
  cycle; the decode now feeds it rank 2).
- The per-DIMM capacity is **32 GiB** (16 Gb per die × 16
  devices), so the breakdown group is `2x32 GiB` — not the C10
  `2x16 GiB`.
- There is **no** `2 of 4 slots SPD-visible` note: the OS total
  (MemTotal 62.7 GiB) is ≤ the SPD sum (2 × 32 = 64 GiB), so
  `slot_note` returns `None` (no false alarm). In C10 the 32 GiB
  total against two 16 GiB slots triggered the note.

## Check (b) — the decode is correct, not painted

Confirm the live values reconcile through the JESD79-4 decode
(the point of C11 — the 5950X G.Skill `F4-3600C18-32GVK` module):

- **Density**: SPD byte `0x13 = 0x0D` decodes to **16 Gb** per
  die (the P6-04 value, restored by C11 — C10's 32 Gb paint is
  reverted). In the SPD cards / JSON export the density cell
  reads 16384 Mbit, not 32768.
- **Rank**: SPD byte `0x0C = 0x09` decodes to **2 ranks**
  (dual-rank) via bits 3:4 = `01` + 1, per JESD79-4 (the C10
  bit-1 read gave the wrong 1). The rank cell reads 2.
- **Devices**: **16** total = 2 ranks × 8 per rank (the vendor's
  `0x80` per-rank nibble fails the consistency rule, so the
  per-rank is derived as 64 / 8 = 8; the devices cell reads 16,
  not 8).
- **Per-DIMM capacity arithmetic**: 16384 Mbit × 16 devices /
  8192 = **32 GiB** — the same 32 GiB per DIMM C10 painted, now
  reached through the correct decode instead of a special-case
  override. Cross-check against the SPD card's capacity display
  and `target/release/ramsleuth-client dump` (or the JSON
  export) for the same three cells (density 16384, rank 2,
  devices 16).

## Check (c) — live MCLK unchanged

The `Clocks & Ratios` block (left column) — the **MCLK** row —
and the Graphs sparkline must still read the module's
operating frequency (C10's daemon DRAM-spike-before-MCLK-read
behavior is untouched this cycle: the daemon binary is
byte-identical in behavior — zero daemon/wire changes). Expect
the same MCLK value you saw after Cycle 10's fix (the live
operating frequency, not a zeroed/stale read), with the
sparkline tracking it across polls.

## Reporting

Report any mismatch (which check, what the GUI showed vs
expected) back to the operator before the v2-development push.
If all three pass, the cycle is verified end-to-end.

---

## Automated gates (all passed, 2026-09-17, tip d170a3f)

- **Full regression debug**: `cargo test --workspace` → **546
  passed / 0 failed** (same count as the Cycle 10 baseline; the
  7 re-anchored spots carry C11 values — rank 2, 16 devices,
  16384 Mbit).
- **Full regression release**: `cargo test --workspace
  --release` → **546 passed / 0 failed**.
- **Clippy**: `cargo clippy --workspace --all-targets --
  -D warnings` → **zero warnings** (exit 0).
- **MSRV**: root `Cargo.toml` `rust-version = "1.75"` (unchanged).
- **Zero-deps**: `git diff a7bf7bb..HEAD -- Cargo.toml Cargo.lock`
  → empty (no new dependencies).
- **Six release binaries**: `cargo build --release --workspace`
  → all 6 built: bench 650,424 B, client 759,456 B, daemon
  1,926,776 B, gui 16,216,216 B, telemetry 687,216 B, tui
  1,382,224 B.
- **Zero-wire**: `git diff a7bf7bb..HEAD` touches exactly one
  file — `crates/ramsleuth-telemetry/src/spd_decode.rs`.
  `crates/ramsleuth-protocol/` diff empty; the telemetry diff
  changes no struct definition, no serde derive, and no wire
  field (the frozen `SpdModule` field list — `density_mbit:
  Section<u16>`, `rank: Section<u8>`, `devices: Section<u8>` —
  plus `SpdProfile`, `SystemMemoryTelemetry`, `AmdPmSnapshot`
  are byte-identical); only the `rank_count` decode (bit 1 →
  bits 3:4), the `0x0D` density arm (32 → 16), doc comments,
  and the 7 re-anchored test spots changed. GUI / TUI / client /
  bench / daemon diffs all empty (the `rank_word` helper already
  handles rank 2).
- **No-panic**: the modified decode path contains no
  `panic!` / bare `unwrap()` / `expect(` on hardware-derived
  data (only pre-existing `unwrap_or(0)` Option defaults in
  non-test code; every panic site in the file is inside
  `mod tests`).
