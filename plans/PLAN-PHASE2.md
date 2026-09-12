# RamSleuth v2 — Phase 2 Plan: Hardware Telemetry & Register Extraction (`crates/ramsleuth-telemetry`)

> **Base branch:** `v2-development` — every `branch/chunk-P2-xx` forks from and merges back here (NOT `main`).
> **Sources of truth:** `Docs/Grand Design & Architecture Specification.md` §5 (Hardware Telemetry), §3.1 (dashboard layout/units), §4 (privilege model), §7 (acceptance); `Docs/RamSleuth-v2.md` Phase 2 (§2.1–2.2).
> **Mandate:** 100% pure Rust (Edition 2021), Cargo workspace.
> **Chunk discipline:** one target source file per chunk, ≤ ~50–100 lines of source changed. Each chunk adds exactly one `mod <name>;` line to `src/lib.rs` (wiring only, not counted against the 50–100 budget).
>
> **Test-host reality (critical):** AMD Ryzen 9 5950X — Zen 3 (family 0x18), 16C/32T, 64 MiB L3, **DDR4**, AVX2 (no AVX-512).
> - **AMD path = LIVE-TESTABLE here** (requires root or CAP_SYS_RAWIO + `ryzen_smu` kernel module loaded).
> - **Intel MCHBAR path = NOT runtime-testable here** (no Intel CPU). It is validated only as: correct vendor/CPUID detection, returns N/A on non-Intel, **never panics / never touches /dev/mem on this host**.
> - **SPD/ee1004 path = live-testable if the kernel driver is present** (no privilege required; pure sysfs reads).

---

## 1. Phase 2 Scope (extracted verbatim from the specs)

**Objective (v2 §2):** Extract real, active memory controller subtimings, clocks, voltages, and SPD information directly from hardware.

### 1.1 AMD Zen telemetry (v2 §2.1 + Grand Design §5.1)

- Inspect CPUID family/model (Zen 1 through Zen 5).
- Interface the **ryzen_smu kernel driver**: open `/dev/ryzen_smu` **or** read `/sys/kernel/ryzen_smu/pm_table` (spec names the kernel driver as the channel: "sends message requests through the SMU mailbox registers to read the cryptographic Power Management (PM) table").
- Parse SMU PM tables to extract:
  - **Clocks:** MCLK, UCLK, FCLK, DivMode (1:1 vs 1:2), Gear Down Mode (GDM), Power Down Mode (PDM).
  - **Timings:** tCL, tRCDWR, tRCDRD, tRP, tRAS, tRC, tRRDS, tRRDL, tFAW, tWTRS, tWTRL, tWR, tRFC1, tRFC2, tRFCsb, tCWL, tRTP, tRDWR, tWRRD + tertiary/turnaround group (tRDRD & tWRWR across same-diff-SC / same-CCD / SCL / SC).
  - **Drive strengths & resistances (CAD bus):** ProcODT, RttNom, RttWr, RttPark, ClkDrv, AddrCmdDrv (dashboard also lists CsOdtDrv, CkeDrv) → displayed in **ohms**.
  - **Voltages:** VDDCR_SOC, VDDIO_MEM, VDD_MISC (dashboard also lists VPP) → displayed in **volts**.
- UMC note: UMC (Data Fabric) has no sanctioned userspace register access; the SMU PM table is the documented source of the UMC-trained values (Grand Design §5.1). Raw SMN MMIO bypass is **rejected** (unsafe, version-coupled, outside the spec's driver channel).

### 1.2 Intel Core telemetry (v2 §2.1 + Grand Design §5.2)

- Read PCI config space **00:00.0 offset 0x48** → 64-bit MCHBAR base address.
- Map the physical range via `/dev/mem` (fallback `/dev/fmem`) with `mmap()`, **read-only**.
- Extract per channel (0–3): **tCL, tRCD, tRP, tRAS, Command Rate (1N/2N), Gear Mode (1/2/4)** (SA clock multiplier), plus turnaround/tertiary: **RTL, tCCD_L, tCCD_S, tRDRD, tRDWR** (and tWRWR/tWRRD where decodable) → same display units as AMD (ticks / MHz / 1N-2N / gear).
- Intel voltages/CAD bus: **out of scope for Phase 2 live readout** → sections return `Na` (mapped cleanly by the shared display types).

### 1.3 SPD EEPROM parser (v2 §2.1 + Grand Design §5.3)

- Query `/sys/bus/i2c/drivers/ee1004/*/eeprom` (raw **512-byte DDR4 / 1024-byte DDR5** blocks); no privilege required.
- Decode: JEP106 module + DRAM die manufacturer IDs (e.g. SK Hynix A-die/M-die, Samsung B-die, Micron), die stepping, part number, serial number, rank organization, and **XMP 2.0 / 3.0 + EXPO** profiles (factory-rated, shown alongside live values).

### 1.4 Target visual/UX contract (Grand Design §3.1 dashboard)

Telemetry feeds the left dashboard panel "LIVE MEMORY CONTROLLER & SUBTIMINGS":
- **Clocks & Ratios:** MCLK / UCLK / FCLK (MHz), UCLK:MCLK (1:1 / 1:2), Gear Mode, GDM / CR (1T).
- **Primary:** tCL, tRCDWR, tRCDRD, tRP, tRAS (ticks).
- **Secondary:** tRC, tRRDS/tRRDL, tFAW, tWTRS/tWTRL, tWR, tRFC1/tRFC2, tRFCsb (ticks).
- **Tertiary & turnarounds:** tRDRDSD/DD/SCL/SC, tWRWRSD/DD/SCL/SC, tRDWR, tWRRD (ticks).
- **CAD bus:** ProcODT (Ω), RttNom/RttWr/RttPark (Disabled / RZQ/2 (120 Ω) / RZQ/4 / RZQ/5 (48 Ω) style), ClkDrv/AddrCmdDrv/CsOdtDrv/CkeDrv (Ω).
- **Voltages:** VDDCR_SOC, VDDIO_MEM, VDD_MISC, VPP (V).
- Right panel "HARDWARE & SPD MODULE TELEMETRY": per-slot module name, DRAM die + rank, EXPO/XMP profile summary.
- Every missing value renders as `N/A (<reason>)` — never blank, never a crash.

### 1.5 Phase 2 exit criteria (v2 §2.2 + Global §7)

1. **Populated struct:** the telemetry library produces a populated **`SystemMemoryTelemetry`** struct containing **verified live timings** on test hardware.
2. **Graceful degradation:** returns **structured errors (`UnsupportedHardware`, `DriverMissing`, …)** without panicking — no unhandled segfault, no panic, on insufficient privilege / unsupported CPU / unknown PM-table version / missing driver.
3. **Feature completeness (Global §7):** live readout of active subtimings matching **ZenTimings** on AMD (AM4/AM5) — acceptance on this host: values tick-identical to the `ryzen_smu` CLI ground truth; clocks within ±1 MHz; voltages within ±10 mV; CAD values equal to the RZQ/code-table mapping.
4. **System safety (Global §7):** Intel path on this (AMD) host: correct vendor detection → all Intel sections `Na(UnsupportedVendor)` → **no /dev/mem access attempted, no panic**.

---

## 2. Key Architectural Decisions

### D1 — SMU access: **implement directly against the ryzen_smu *kernel driver* — NO `ryzen_smu` crate dependency**

**Decision:** the AMD provider reads the PM table **directly**: (a) primary path = read `/sys/kernel/ryzen_smu/pm_table` (plain sysfs blob, `std::fs` only); (b) fallback = open `/dev/ryzen_smu` and issue the driver's read ioctl (via `nix` — already required for the Intel mmap). The **version-guarded PM parse is ours** (`amd_pm.rs`), and all mapping is ours (`amd_readout.rs`).

**Rationale:**
1. **The spec prescribes direct driver access.** v2 §2.1: "Open `/dev/ryzen_smu` or read `/sys/kernel/ryzen_smu/pm_table`. Parse SMU PM tables to extract …" — and the brief's mandated chunk "AMD UMC/PM table parse (version-guarded against unknown PM table versions)" only makes sense if we own the parse. A wrapper crate would hide the version guard behind its own, defeating the mandated safety design.
2. **Dependency hygiene / 100% pure Rust.** Phase 1 set the precedent (std + `core::arch` only); the workspace scaffold comment anticipates "ryzen_smu / nix / memoffset etc." but does not mandate the crate. Direct access needs **zero new crates for AMD** (`std::fs` + `nix::ioctl` for the fallback only).
3. **Third-party risk is avoided.** An external parser's SMU-version coverage, maintenance state, and license are all unverified; a broken/incompatible crate would couple Phase 2's exit to an upstream we cannot control.
4. **Attack surface.** The daemon (Phase 3) is the only privileged process; a minimal, auditable, self-owned SMU read path is strictly easier to sandbox than one wrapping an external ioctl/parse crate.

**Pivot trigger (recorded, not planned):** if live validation on this host shows the sysfs blob lacks fields the crate would have provided (e.g. a field the driver only exposes via ioctl), P2-03's ioctl fallback already covers it. If neither driver interface suffices, that is a plan edit + re-verification against the loaded driver version — not a silent dependency addition.

### D2 — UMC = via SMU PM table
No direct UMC MMIO from userspace exists; the SMU PM table (written by AGESA training) is the sanctioned source (Grand Design §5.1). This host's 5950X = **SMU 7.11.x (Zen 3)** — exact PMFW version byte is verified in P2-03; the layout table (P2-04) covers the 7.11.x family (7.11.2/7.11.3) and 12.x/13.x (Zen 4/5) skeletons, unknown versions → `Na(UnknownPmTableVersion)`.

### D3 — Shared display types live in `amd_readout.rs` (P2-05), re-exported from `lib.rs`
`ClockReadout`, `TimingSet`, `CadBus`, `VoltageSet` are **vendor-neutral** (ticks, MHz, Ω, V) and are the single mapping target for both AMD (P2-05) and Intel (P2-07). Defined once (P2-05, after the AMD parse), consumed by Intel later in topological order.

### D4 — Intel MCHBAR = PCI config via sysfs + read-only `/dev/mem` mmap
BAR5 base read from `/sys/bus/pci/devices/0000:00:00.0/config` bytes `0x48..0x50` (std, no ioctl needed). Map with `nix::sys::mmap` (`PROT_READ` only). **Hard gate: `CpuInfo` vendor must be Intel before any file/mmap access** (P2-06) → on this host the Intel provider returns `Na(UnsupportedVendor)` and never touches `/dev/mem`. STRICT_DEVMEM rejections → `Na(InsufficientPrivilege)`. A bounds-checked guard struct owns the mapping and `munmap`s in `Drop` (no dangling raw pointers, no panics on short/invalid reads).

### D5 — Graceful-degradation contract (applies to every public API)
- Fallible operations return `TelemetryResult<T>` (`Result<T, TelemetryError>`).
- Displayable values are wrapped in `Section<T> = { Value(T) | Na(TelemetryError) }` so a partially-populated platform (e.g. Intel: timings yes, voltages no) is expressible without per-field `Option` ambiguity.
- **No `panic!`, no `unwrap()`/`expect()` on hardware-derived data, no unguarded deref** anywhere in the crate; every `unsafe` block carries `// SAFETY:` and is bounds-checked against the mapped region / blob length.
- `TelemetryError` is frozen in P2-02 (interface freeze) — includes `UnsupportedHardware, UnsupportedVendor, DriverMissing, InsufficientPrivilege, UnknownPmTableVersion, NoDevmem, NotApplicable, InvalidValue(String), Io(String)`.

### D6 — Dependency policy
**Exactly one new external crate for all of Phase 2: `nix`** (features `mmap`, `ioctl`, `err`) — added in **P2-03's** Cargo.toml edit (owned by `amd_smu.rs`, shared by `intel_mchbar.rs`).
- **Rejected:** `ryzen_smu` crate (D1), `memoffset` (plain offset arithmetic + typed u32 reads suffice; avoids a dep for pointer math).
- `[[bin]]` is auto-detected when `src/main.rs` lands (P2-11) — no manifest target section needed. JSON output is hand-rolled (consistent with P1-11; **no serde/serde_json**).

---

## 3. Module layout (target file tree)

```
crates/ramsleuth-telemetry/
├── Cargo.toml            (P2-03 adds `nix` dep — the only new dep of Phase 2)
└── src/
    ├── lib.rs            (wiring only — each chunk adds one `mod` line; P2-10 adds re-exports)
    ├── cpuid.rs          P2-01  CPUID vendor + family/generation detection          [CRITICAL-PATH]
    ├── error.rs          P2-02  TelemetryError / TelemetryResult / Section<T>       [CRITICAL-PATH]
    ├── amd_smu.rs        P2-03  SMU access layer (sysfs pm_table / char-dev ioctl), privilege-guarded
    ├── amd_pm.rs         P2-04  version-guarded PM table parse → AmdPmSnapshot
    ├── amd_readout.rs    P2-05  shared display types + AMD mapping (clocks/timings/CAD/voltages)
    ├── intel_mchbar.rs   P2-06  PCI BAR5 + read-only /dev/mem mmap guard
    ├── intel_readout.rs  P2-07  Intel per-channel decode → shared display types
    ├── spd_eeprom.rs     P2-08  ee1004 raw image acquisition (sysfs, unprivileged)
    ├── spd_decode.rs     P2-09  JEP106 / die / rank / XMP / EXPO decode → SpdModule
    ├── facade.rs         P2-10  SystemMemoryTelemetry + collect() vendor dispatch
    └── main.rs           P2-11  verification CLI (dashboard listing / structured N/A, --json)
```

**Interface freezes:** P2-01 (`CpuInfo`/`Vendor`) and P2-02 (`TelemetryError`/`Section<T>`/`TelemetryResult`) are `[CRITICAL-PATH]` — their public signatures are frozen at merge; everything downstream branches on them. P2-05 freezes the four shared display types (AMD+Intel+facade consume them). No silent signature changes — any change is a plan edit + rebase.

---

## 4. Micro-chunks (ordered — dependencies first)

### Chunk P2-01 — CPUID vendor + family/generation detection  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-telemetry/src/cpuid.rs` (+1 `mod cpuid;` in `src/lib.rs`)
- **Scope Boundary:** CPUID via `core::arch::x86_64::__cpuid` (`#[cfg(target_arch = "x86_64")]`; non-x86_64 compiles to `Unknown`): vendor string (leaf 0); **AMD** extended family (leaf `0x80000001`) → `ZenGen` map: `0x16/0x00–0x3F` Zen 1, `0x16/0x80–0xFF` Zen+, `0x17` Zen 2, `0x18` Zen 3 (this host), `0x19` Zen 4, `0x1A` Zen 5, else `Amd(Unrecognized)`; **Intel** family `0x6` → `IntelGen` table covering client/server models Skylake `0x4F`/`0x5E` through Raptor Lake / Arrow Lake `0x8A`-class ranges, else `Intel(Unrecognized)`. Expose frozen `CpuInfo { vendor: Vendor, generation: Option<Generation> }` + `CpuInfo::detect()`.
- **Dependency:** `[CRITICAL-PATH]` (interface freeze — every provider gates on `CpuInfo`; D4's "Intel-before-mmap" gate consumes it).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; no `unsafe` (pure intrinsics); unit tests: vendor-string mapping fixtures + this host asserts `Amd(Zen3)`.
- **Exit Criteria:** `detect()` returns `Amd(Zen3)` on the 5950X host; non-x86_64 target returns `Unknown` without failing to compile; `CpuInfo`/`Vendor` signatures frozen.

### Chunk P2-02 — Telemetry error / N-A result types  `[CRITICAL-PATH]`
- **Target File:** `crates/ramsleuth-telemetry/src/error.rs` (+1 `mod error;` in `src/lib.rs`)
- **Scope Boundary:** `pub enum TelemetryError` — `UnsupportedHardware`, `UnsupportedVendor`, `DriverMissing`, `InsufficientPrivilege`, `UnknownPmTableVersion`, `NoDevmem`, `NotApplicable`, `InvalidValue(String)`, `Io(String)` — with `Display` + `std::error::Error` impls; `pub type TelemetryResult<T> = Result<T, TelemetryError>`; `pub enum Section<T> { Value(T), Na(TelemetryError) }` with `Section::value()`, `Section::na()`, `is_value()`, `as_option()`, `reason()`; documented no-panic contract (D5).
- **Dependency:** `[CRITICAL-PATH]` (interface freeze — all 9 downstream chunks return `TelemetryResult`/`Section`).
- **Quality Gates:** idiomatic Rust; zero clippy warnings; pure types (no IO, no `unsafe`); unit tests: every variant's `Display` text + `Section` value/na round-trips.
- **Exit Criteria:** signatures frozen; compiles with zero deps; Display texts are user-presentable (they surface verbatim in the P2-11 CLI `N/A (<reason>)`).

### Chunk P2-03 — AMD SMU access layer (privilege-guarded)
- **Target File:** `crates/ramsleuth-telemetry/src/amd_smu.rs` (+1 `mod amd_smu;` in `src/lib.rs`; **Cargo.toml edit: add `nix` (`mmap`, `ioctl`, `err`) — the only new dependency of Phase 2**)
- **Scope Boundary:** given a `CpuInfo` AMD gate (non-AMD → `Na(UnsupportedVendor)` **before any file access**): (a) primary: read `/sys/kernel/ryzen_smu/pm_table` via `std::fs`; (b) fallback: `open("/dev/ryzen_smu")` + driver read **ioctl** via `nix` (no raw SMN MMIO — D1/D2); extract SMU version from the PM header; classify failures: no module/paths → `DriverMissing`, open/read EPERM/EACCES → `InsufficientPrivilege`, empty/short blob → `InvalidValue`. Returns `SmuContext { version: SmuVersion, pm: Vec<u8>, source: SmuSource }` as `TelemetryResult<SmuContext>`.
- **Dependency:** `[COUPLED-TO: P2-01, P2-02]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; every ioctl in `unsafe` with `// SAFETY:` (fd validity + nix error mapping); no `unwrap` on IO; unit tests: missing-paths fixture → `DriverMissing`; injected EPERM → `InsufficientPrivilege`; header-byte version extraction fixture.
- **Exit Criteria:** on this host (module loaded + root) returns a non-empty PM blob + `SmuVersion` (7.11.x); without the module → `DriverMissing`; non-root → `InsufficientPrivilege`; non-AMD → `UnsupportedVendor` with zero file accesses. **Live-verification gate:** confirm this host's exact PMFW version byte and that the sysfs blob is complete — result recorded for P2-04's layout table.

### Chunk P2-04 — AMD PM table parse (version-guarded)
- **Target File:** `crates/ramsleuth-telemetry/src/amd_pm.rs` (+1 `mod amd_pm;` in `src/lib.rs`)
- **Scope Boundary:** `SmuVersion` → field offset/width **layout table**: `Smu7` (7.11.2/7.11.3 — this host), `Smu12` (Zen 4), `Smu13` (Zen 5) skeletons; any other version → `UnknownPmTableVersion` (D2). Bounds-checked readers (`read_u8/u16/u32_at(&[u8], off) -> Option<_>`, short blob → `None`, never panics) extract into `AmdPmSnapshot { clocks: Option<ClockRaw>, timings: Option<TimingRaw>, cad: Option<CadRaw>, voltages: Option<VoltageRaw> }` (raw units: MHz-ish PM values, tick counts, RZQ/driver codes, mV).
- **Dependency:** `[COUPLED-TO: P2-03, P2-02]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; **no `unsafe`** (pure slice parsing); unit tests: synthetic SMU7 blob with known 5950X DDR4-3200 values → exact fields; truncated blob → all `None` + no panic; unknown version → `UnknownPmTableVersion`.
- **Exit Criteria:** parsing a live P2-03 blob yields coherent `AmdPmSnapshot` (MCLK/UCLK/FCLK present); synthetic + truncated + unknown-version fixtures all behave per contract.

### Chunk P2-05 — AMD subtiming readout + shared display types
- **Target File:** `crates/ramsleuth-telemetry/src/amd_readout.rs` (+1 `mod amd_readout;` in `src/lib.rs`)
- **Scope Boundary:** **Define the four vendor-neutral display types (D3, frozen here):** `ClockReadout { mclk_mhz, uclk_mhz, fclk_mhz: f32, div_mode: DivMode, gear_mode: GearMode, gdm: bool, pdm: bool }`, `TimingSet { cl, rcwdwr, rcdrd, rp, ras, rc, rrds, rrld, faw, wtrs, wtrl, wr, rfc1, rfc2, rfcsb, cwl, rtp, rdwr, wrrd: u16 …; rdrd_sd/rrdr_dd/rdrd_scl/rdrd_sc, wrwr_sd/wrwr_dd/wrwr_scl/wrwr_sc: Option<u16> }` (ticks), `CadBus { pro_odt_ohms: Option<f32>, rtt_nom/rtt_wr/rtt_park: Option<RttValue>, clk_drv/addr_cmd_drv/cs_odt_drv/cke_drv: Option<f32> }` (`RttValue { Disabled, Rzq(u32 /*×RZQ/…*/), Ohms(f32) }` — RZQ=240 Ω base code table), `VoltageSet { vddcr_soc_v, vddio_mem_v, vdd_misc_v, vpp_v: Option<f32> }`. Map `AmdPmSnapshot` → these (code→Ω table, mV→V, ratio computation) with sanity-range validation (out-of-band → per-field `None`).
- **Dependency:** `[COUPLED-TO: P2-04, P2-02]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; **no IO, no `unsafe`** (pure mapping); unit tests: fixture snapshot → expected dashboard values (e.g. DDR4-3200: MCLK 1600, CL16 → `cl: 16`), CAD code round-trips (RZQ/5 → 48 Ω), voltage mV→V (1350 mV → 1.35), out-of-band value → `None` not panic.
- **Exit Criteria:** the four type + `DivMode`/`GearMode`/`RttValue` signatures frozen (Intel P2-07 + facade P2-10 consume); AMD mapping produces a fully-populated display set from the live snapshot.

### Chunk P2-06 — Intel MCHBAR MMIO map (guarded, read-only)
- **Target File:** `crates/ramsleuth-telemetry/src/intel_mchbar.rs` (+1 `mod intel_mchbar;` in `src/lib.rs`)
- **Scope Boundary:** **Vendor gate first (D4):** `CpuInfo` must be Intel, else `Na(UnsupportedVendor)` with **no file access whatsoever**. Read BAR5 (64-bit) from `/sys/bus/pci/devices/0000:00:00.0/config` bytes `0x48..0x50` (std); zero/invalid BAR5 → `Na(InvalidValue)`. Open `/dev/mem` (fallback `/dev/fmem`), `mmap` base range `PROT_READ` (via `nix`), wrap in `MchBar` guard: bounds-checked `read_u32(offset) -> TelemetryResult<u32>`, `munmap` in `Drop`; STRICT_DEVMEM/EPERM → `Na(InsufficientPrivilege)`; missing file → `Na(NoDevmem)`.
- **Dependency:** `[COUPLED-TO: P2-01, P2-02]` (`nix` already added by P2-03)
- **Quality Gates:** idiomatic Rust; zero clippy warnings; every mmap/volatile read in `unsafe` with `// SAFETY:` (mapping bounds, alignment, read-only, lifetime-ownership in guard); unit tests: config-byte parsing fixtures; injected non-Intel `CpuInfo` → `UnsupportedVendor` and verifiably zero `open()`/`mmap` calls; out-of-bounds offset → `InvalidValue` not SIGSEGV.
- **Exit Criteria:** compiles; **on this host: returns `Na(UnsupportedVendor)` without touching `/dev/mem`** (satisfies the brief's Intel validation contract); on an Intel box the guard maps and reads (not runtime-verifiable here — code-reviewed + fixture-tested only).

### Chunk P2-07 — Intel subtiming readout + mapping
- **Target File:** `crates/ramsleuth-telemetry/src/intel_readout.rs` (+1 `mod intel_readout;` in `src/lib.rs`)
- **Scope Boundary:** per-channel (0–3) MCHBAR register offset tables (freq-ratio/IMC registers, `const` tables); decode **tCL, tRCD, tRP, tRAS** (ticks), **Command Rate 1N/2N**, **Gear Mode 1/2/4** (SA:MEM multiplier), **RTL, tCCD_L, tCCD_S, tRDRD, tRDWR, tWRWR, tWRRD** → map into the **shared `TimingSet`/`ClockReadout`** (D3; gear 1/2/4 → `div_mode`/`gear_mode`); CAD bus & voltages → `Na(NotApplicable)` (Intel PMU readout out of Phase 2 scope); any register read failure → per-field `None`, never panic.
- **Dependency:** `[COUPLED-TO: P2-06, P2-05, P2-02]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; offset tables `const` + bounds-checked; **no new `unsafe`** (consumes `MchBar` guard only); unit tests: synthetic register fixtures → decoded tick values (incl. 1N/2N + gear cases); guard-disabled fixture → all-`Na` without panics.
- **Exit Criteria:** on this host, the Intel readout path (driven by P2-01 detection) yields all-`Na` sections gracefully; fixture tests prove the decode logic is sound for a real Intel layout.

### Chunk P2-08 — SPD EEPROM raw acquisition (ee1004, unprivileged)
- **Target File:** `crates/ramsleuth-telemetry/src/spd_eeprom.rs` (+1 `mod spd_eeprom;` in `src/lib.rs`)
- **Scope Boundary:** enumerate active I2C adapters (`/sys/bus/i2c/devices/i2c-*`, `/sys/class/i2c-dev/`) and bound `ee1004` devices (`/sys/bus/i2c/drivers/ee1004/*/`); read each raw `eeprom` file (512 B DDR4 / 1024 B DDR5 — DDR5 detected via 1024-byte image + header signature) → `Vec<SpdRaw { slot: String, bytes: Vec<u8> }>`; no driver/devices present → empty `Vec` + `DriverMissing` warning (facade renders "SPD: N/A"). **No privilege required.**
- **Dependency:** `[ISOLATED]` (only P2-02 for the error type)
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `std::fs` only, **no `unsafe`, no new deps**; unit tests: image-length/DDR5-signature classification fixtures; missing-dir handling.
- **Exit Criteria:** on this host (DDR4, `ee1004` typically loaded) returns 1–2 raw 512-byte images; never fails the process when the driver is absent.

### Chunk P2-09 — SPD decode (JEP106 / die / rank / XMP / EXPO)
- **Target File:** `crates/ramsleuth-telemetry/src/spd_decode.rs` (+1 `mod spd_decode;` in `src/lib.rs`)
- **Scope Boundary:** decode `SpdRaw` → `SpdModule { slot, module_maker: String, die_maker: String, die_stepping: Option<String>, part_number: Option<String>, serial: Option<String>, ranks: Option<u8>, max_speed_mts: Option<u32>, profiles: Vec<ProfileSummary> }`: JEP106 ID (bytes 0x01/0x2E, continued-ID support) for module + DRAM die makers (SK Hynix A-die/M-die, Samsung B-die, Micron…), rank organization bits (byte 128), ASCII part/serial (bytes 128–151), JEDEC speed; **XMP 2.0** (DDR4, header byte 131 region) and **XMP 3.0 / EXPO** (DDR5 SPD bytes 512–1024) profile count + key fields (speed, CL, volt); checksum failure → profile skipped, module still shown; garbage/truncated → no panic, `None` fields.
- **Dependency:** `[COUPLED-TO: P2-08, P2-02]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; pure bit-decode, **no IO/`unsafe`/deps**; unit tests: synthetic 512 B DDR4 fixture (known JEP106 codes + XMP 2.0) and synthetic 1024 B DDR5 fixture (EXPO profile) → expected decodes; corrupted-checksum + truncated fixtures → graceful.
- **Exit Criteria:** both fixture families decode to expected ground truth; real `SpdRaw` from P2-08 decodes without error on this host.

### Chunk P2-10 — Telemetry facade / public API
- **Target File:** `crates/ramsleuth-telemetry/src/facade.rs` (+1 `mod facade;` in `src/lib.rs`; re-export block in `lib.rs`)
- **Scope Boundary:** define the exit-criteria struct **`SystemMemoryTelemetry { vendor: Vendor, clocks: Section<ClockReadout>, timings: Section<TimingSet>, cad_bus: Section<CadBus>, voltages: Section<VoltageSet>, spd: Vec<SpdModule>, warnings: Vec<TelemetryError> }`** + **`collect() -> SystemMemoryTelemetry`**: run P2-01 `detect()` → **AMD branch** (P2-03→P2-04→P2-05), **Intel branch** (P2-06→P2-07), unknown vendor → all sections `Na(UnsupportedHardware)`; SPD (P2-08/09) runs on both vendors; every branch independently error-contained so one failure can never take down the others (D5).
- **Dependency:** `[COUPLED-TO: P2-01 … P2-09]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; `lib.rs` re-exports the public surface (`collect`, `SystemMemoryTelemetry`, `Section`, `TelemetryError`, display types); unit tests: `collect()` on this host → `vendor = Amd(Zen3)` + non-`Na` clocks/timings (root) or `Na(InsufficientPrivilege)` (non-root, still exit-safe); injected non-x86 `CpuInfo` → all `Na(UnsupportedHardware)`.
- **Exit Criteria:** `SystemMemoryTelemetry` exists and is populated on this test hardware per v2 §2.2 exit criterion 1; no code path in `collect()` can panic.

### Chunk P2-11 — Standalone verification harness (CLI)
- **Target File:** `crates/ramsleuth-telemetry/src/main.rs` (auto-detects `[[bin]]`; no manifest target change)
- **Scope Boundary:** `cargo run -p ramsleuth-telemetry [--json]` entrypoint: call `facade::collect()`, print the **dashboard-style listing** (Grand Design §3.1 layout: Clocks & Ratios / Primary / Secondary / Tertiary / CAD bus / Voltages / SPD slots) with every cell rendered as its value or `N/A (<reason>)` from `Section::reason()`; hand-rolled JSON block (P1-11-style, **no serde**); `--json` vs text flag via `std::env::args`; **exit 0 even when all sections are `Na`** (N/A is a valid, structured outcome) — non-zero only on an internal bug (should be unreachable).
- **Dependency:** `[COUPLED-TO: P2-10]`
- **Quality Gates:** idiomatic Rust; zero clippy warnings; no clap; graceful error text, no panics on any input state.
- **Exit Criteria:** as **root** on this host: live MCLK/UCLK/FCLK + tCL…tWRRD + CAD + voltages printed, tick-identical to `ryzen_smu` CLI ground truth (±1 MHz clocks, ±10 mV voltages); as **unprivileged user**: full `N/A (InsufficientPrivilege/DriverMissing)` listing, exit 0; `--json` emits parseable JSON with the same content.

---

## 5. Phase 2 Acceptance & Exit Criteria (consolidated)

1. **Live population (v2 §2.2.1):** `SystemMemoryTelemetry` populated with verified live timings on this hardware (5950X/DDR4) — clocks ±1 MHz, timings tick-identical, voltages ±10 mV vs the `ryzen_smu` tool's own readout (ground truth); CAD Ω values equal to the RZQ/driver code-table mapping.
2. **Graceful errors (v2 §2.2.2):** `UnsupportedHardware` / `DriverMissing` / `InsufficientPrivilege` / `UnknownPmTableVersion` returned as structured `Section::Na(…)` — verified by running the harness (a) unprivileged and (b) with the module unloaded; **no panic, no segfault** in any state.
3. **Intel contract (host-specific, per brief):** correct vendor/CPUID detection; on this AMD host every Intel path returns `Na(UnsupportedVendor)`; **zero `/dev/mem` access**; never panics (P2-06/P2-07 evidence).
4. **SPD (live if available):** `ee1004` present → per-slot maker/die/rank/speed + XMP 2.0 (this host) rendered; absent → "SPD: N/A (DriverMissing)", exit 0.
5. **Quality (Global §7):** `cargo clippy -p ramsleuth-telemetry --all-targets -- -D warnings` clean; `cargo test -p ramsleuth-telemetry` green; every `unsafe` block carries `// SAFETY:`; exactly one new dependency (`nix`).
6. **Merge model:** `branch/chunk-P2-xx` → `v2-development` via `git merge --no-ff` after review + tests; never force-push.

## 6. Global quality gates (apply to every chunk)

- Idiomatic Rust, Edition 2021; all `unsafe` confined to the SMU ioctl (P2-03) and MCHBAR mmap/reads (P2-06) with `// SAFETY:` comments.
- `cargo clippy -p ramsleuth-telemetry --all-targets -- -D warnings` → clean.
- `cargo test -p ramsleuth-telemetry` → green (unit tests live in the same file as the code they test, `#[cfg(test)]`).
- `cargo build -p ramsleuth-telemetry --release` → succeeds.
- **No new third-party dependency except `nix`** (added once, in P2-03; features `mmap`+`ioctl`+`err`). `ryzen_smu` crate **rejected** (decision D1); `memoffset` **rejected** (D6).
- No-panic contract (D5): no `panic!`/`unwrap()`/`expect()` on hardware-derived data anywhere in the crate; all reads bounds-checked.

## 7. Dependency ledger (DO NOT add now — owned by the named chunk)

| Dependency | Features | Added in chunk | Owner file(s) |
|---|---|---|---|
| `nix` | `mmap`, `ioctl`, `err` | **P2-03** | `amd_smu.rs` (ioctl fallback), `intel_mchbar.rs` (mmap) |

Rejected: `ryzen_smu` (D1), `memoffset` (D6), `serde`/`serde_json` (P1-11 precedent: hand-rolled JSON), `clap` (std `env::args`).

## 8. Topological execution order

```
P2-01 (cpuid) ─┬─> P2-03 (amd_smu) ─> P2-04 (amd_pm) ─> P2-05 (amd_readout) ─┬─> P2-07 (intel_readout) ─┐
               │                                                              │                          │
P2-02 (error) ─┴─> P2-06 (intel_mchbar) ──────────────────────────────────────┴──────────────────────────┤
              └─> P2-08 (spd_eeprom) ─> P2-09 (spd_decode) ───────────────────────────────────────────────┴─> P2-10 (facade) ─> P2-11 (main)
```
Critical-path (interface) freezes merge first: **P2-01, P2-02** (independent of each other — may run in parallel).
