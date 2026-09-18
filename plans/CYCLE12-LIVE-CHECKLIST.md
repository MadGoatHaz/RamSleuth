# Cycle 12 — Live-Run Checklist (5950X host, GUI)

Operator-gated manual verification for the C12 atomic merge
(`0003f6f`: SMU Vcore from PM table `0x0A0` → the one additive wire
field `VoltageSet.vcore_mv`, surfaced as the GUI `VDDCR_CPU` graph row
and the `VDDCR_VDD` voltage row; DMI-keyed board-profile VDDIO_MEM from
the NCT6798 `in13` (Crosshair VIII Hero), fill-when-Na, graceful all-Na
fallback on unknown boards / absent device). All automated gates passed
(bottom of this file); the five checks below are the only deferred
verifications and require a live run on the 5950X host.

## How to run

On the 5950X host (interactive sudo required — the GUI reads the
SPD EEPROMs, the AMD PM/SMN paths, and the `nct6798` Super I/O hwmon
channels as root):

```
cd /path/to/RamSleuth
sudo target/release/ramsleuth-gui
```

Wait a few seconds for the first telemetry baseline (the `[Active
System Voltages]` block and the Graphs window must stop reading `N/A`).
Then check (a)–(e).

## Check (a) — VDDIO_MEM now shows volts (board-profile, NCT6798 in13)

In the **[Active System Voltages]** block, the **VDDIO_MEM** row must
now show a **volt value ≈ 1.2–1.4 V** (not `N/A`) — the DMI-keyed
Crosshair VIII Hero board profile maps the `nct6798` `in13` input
channel to VDDIO_MEM. On this host the C12-03 binder saw `in13` =
1376 mV, so expect ≈ **1.3–1.4 V**.

- **Tripwire:** if the row reads **> 2 V**, that is the **8×
  re-scaling failure** — the binder re-applied LibreHardwareMonitor's
  raw-ADC scaling on top of the already-mV value. Report it: the
  `board_vrm` binder must read the hwmon channel's mV value directly
  (the `in13_input` file is already millivolts), never re-apply the
  LHM ADC factor.
- A value in the 1.2–1.4 V band confirms the profile → in13 → VDDIO_MEM
  path and that no double-scaling occurred.

## Check (b) — VDDCR_CPU graph plots the Vcore trace + VDDCR_VDD row

In the **Graphs** window, the **VDDCR_CPU** row must now plot a live
**Vcore trace** (not a permanent `N/A`) — sourced from SMU PM table
`0x0A0` (volts × 1000 → mV), which is board-agnostic on any accepted
AM4 PM layout (this is the additive wire field `VoltageSet.vcore_mv`
the GUI already receives; `GraphSample.vddcr_cpu_mv` is GUI-local).

- The **[Active System Voltages] VDDCR_VDD** row must show the same
  Vcore **volts** (≈ the 5950X core-rail reading, typically ~1.2–1.4 V
  at idle, higher under load).
- Cross-check: the VDDCR_CPU graph value and the VDDCR_VDD row value
  should agree (both are the same frozen `vcore_mv` field rendered two
  ways — graph in mV, voltage row in volts).

## Check (c) — VDDCR_SOC unchanged (PM table 0x0B0)

The **VDDCR_SOC** row (graph + voltage) must still show the PM-table
`0x0B0` value — this cycle did not touch the SOC offset, only added the
`0x0A0` Vcore read alongside it. Confirm it reads the same value you
saw in Cycle 11 (the unchanged `0x0B0` SOC rail).

## Check (d) — VPP + VDD_MISC (Chipset) remain N/A (by design)

The **VPP** and **VDD_MISC (Chipset)** rows must remain **`N/A` (muted
gray)** — the LibreHardwareMonitor Crosshair VIII Hero profile does not
map these two rails (they are not exposed on this board's `nct6798`
channels). This is graceful and **by design**, not a regression. The
Vcore and VDDIO_MEM additions do not imply these two are now populated.

## Check (e) — graceful all-Na fallback (non-Hero board / absent nct6798)

On a **non-Crosshair-VIII-Hero** board (or if the `nct6798` device is
absent), the **VDDIO_MEM** row must degrade to **`N/A` (muted gray)**
with **no crash / no panic** — the `board_vrm` binder's
`BoardVrmReadout::all_na()` all-Na fallback (unknown board → no profile
match → every rail `Na(NotApplicable)`). Vcore (check b) is
board-agnostic and should still populate from the PM table; only the
board-profile VDDIO_MEM degrades. This check is the safety guarantee
that an unrecognized board never panics the GUI.

## Reporting

Report any mismatch (which check, what the GUI showed vs expected —
especially the check (a) > 2 V 8× re-scaling tripwire) back to the
operator before the v2-development push. If all five pass, the cycle is
verified end-to-end. These are the only deferred verifications — every
automated gate below passed.

---

## Automated gates (all passed, 2026-09-18, tip 0003f6f)

- **Full regression debug**: `cargo test --workspace` → **555 passed /
  0 failed**. (Cycle 11 baseline 546 + C12 deltas: +1
  `parse_vcore_volts_to_mv` (C12-01), +1 `record_graph_sample_carries_vcore`
  (C12-02), +6 `board_vrm` (C12-03), +1 facade merge
  `merge_board_vrm_fills_na_vddio_only` (C12-04), +0 C12-05 display
  asserts landed inside an existing GUI test — = 555.)
- **Full regression release**: `cargo test --workspace --release` →
  **555 passed / 0 failed**.
- **Clippy**: `cargo clippy --workspace --all-targets -- -D warnings` →
  **zero warnings** (exit 0).
- **MSRV**: root `Cargo.toml` `rust-version = "1.75"` (unchanged).
- **Zero-deps**: `git diff 84f6036..HEAD -- Cargo.toml Cargo.lock` →
  empty (no new dependencies).
- **Six release binaries**: `cargo build --release --workspace` → all 6
  built: bench 650,424 B, client 760,984 B, daemon 1,945,696 B, gui
  16,219,112 B, telemetry 706,792 B, tui 1,384,008 B.
- **Wire audit** (exactly ONE additive wire field):
  - `crates/ramsleuth-protocol/` → **0 diff** (byte-identical).
  - `crates/ramsleuth-telemetry/` → the wire delta is exactly the
    additive `VoltageSet.vcore_mv: Section<u16>` (appended after
    `vpp_mv`, `serde::Serialize + Deserialize`). `AmdPmVoltages.vcore_mv:
    u16` is telemetry-INTERNAL (`#[derive(Debug, Clone, Copy, PartialEq,
    Eq)]` — no serde, never crosses the wire). `board_vrm.rs` is a NEW
    provider module whose `BoardVrmReadout` (`#[derive(Debug, Clone,
    PartialEq)]` — no serde) is consumed in-crate only (facade fill),
    never embedded in any wire struct. The 8 frozen wire structs
    (`SystemMemoryTelemetry`, `SpdModule`, `SystemPlatform`,
    `AmdPmSnapshot`, `ClockReadout`, `TimingSet`, `CadBus`,
    `IntelReadout`) are byte-identical — no struct definition touched,
    only construction literals gained the `vcore_mv` field. Files:
    amd_pm.rs, amd_readout.rs, amd_smn.rs, board_vrm.rs (new),
    facade.rs, intel_readout.rs, lib.rs.
  - `crates/ramsleuth-gui/` → `GraphSample.vddcr_cpu_mv: f64` is
    GUI-LOCAL (`#[derive(Debug, Clone, Copy, PartialEq)]` — no serde,
    never serialized). Files touched: graph.rs, telemetry_zone.rs,
    update.rs.
  - `crates/ramsleuth-tui/`, `crates/ramsleuth-client/`,
    `crates/ramsleuth-daemon/`, `crates/ramsleuth-bench/` → production
    code unchanged. daemon + bench fully untouched; tui/ui.rs and
    client/dump.rs each +1 line, both inside `mod tests` (the new
    `vcore_mv` field added to a test `VoltageSet` literal only). The TUI
    / dump VDDIO_MEM rows auto-render the C12-04-filled value with zero
    production code change.
- **No-panic**: no `panic!` / bare `unwrap()` / `expect(` on
  hardware-derived data in the new/modified production code — verified
  across all six telemetry files (board_vrm, facade, amd_pm, amd_readout,
  amd_smn, intel_readout: production sections all clean; every panic/
  unwrap/expect hit is inside `mod tests`) and the three GUI files
  (graph.rs, telemetry_zone.rs clean; `value_token` degrades NaN → "N/A"
  gracefully). `board_vrm.rs`'s all-Na path (`read_board_vrm` →
  `BoardVrmReadout::all_na()` on unknown board / absent device) is the
  graceful fallback and contains no production panic. The only `unwrap()`
  in the modified GUI files are pre-existing `RwLock` state guards in
  update.rs (GUI state, not hardware data, not introduced this cycle).
