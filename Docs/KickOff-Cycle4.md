# RamSleuth v2 — Cycle 4 Kickoff (Phase 5: Packaging & Distribution)

Role: Lead Systems Architect & Autonomous Engineering Director

Workspace: /home/madgoat/Documents/RamSleuth/

GitHub: Authenticated as MadGoatHaz (gh CLI operational). Upstream: https://github.com/MadGoatHaz/RamSleuth (has a divergent legacy `master`).

PUSH POLICY: The "confirmed, tested, working end-result app" condition is now MET (Phases 1–4 complete, QA-passed, live-verified). Pushing to `origin/v2-development` is therefore PERMITTED (fast-forward, NEVER force-push) — but only on explicit operator go-ahead, and keep working local on `v2-development` regardless. Never force-push.

Primary language: 100% pure Rust (Cargo workspace). GUI stack: egui + eframe. TUI stack: ratatui + crossterm. No Python/C++/Qt/Electron/Tauri.

## FIRST: read `Docs/HANDOVER.md` in full (authoritative handover for this cycle), then `FULLSCOPEvsCOMPLETED.md` (full scope vs. completed + mermaid map), then `Docs/Grand Design & Architecture Specification.md` and `Docs/RamSleuth-v2.md`.

## Current state

- Phases 1–4 COMPLETE — 7 crates: ramsleuth-bench (P1), ramsleuth-telemetry (P2), ramsleuth-protocol + ramsleuth-daemon + ramsleuth-client (P3), ramsleuth-tui + ramsleuth-gui (P4, folded into Cycle 3).
- 327/327 tests (debug AND release), clippy `-D warnings` zero, MSRV 1.75 (441 locked packages, lockfile-pinned for ratatui/egui transitive deps), release build OK.
- Live-verified end-to-end (all unprivileged): daemon on 0660 Unix socket, client dump/status/bench, TUI (3-zone) under PTY, GUI (3-zone) on Wayland, daemon SIGTERM graceful, no-daemon path friendly — ZERO panics/segfaults.
- CORE GATE passed: unprivileged `cargo run -p ramsleuth-client -- dump` prints full hardware timings against a running daemon.
- All on local-only `v2-development` (~175 commits ahead of divergent `origin/master`, tree clean, single branch — all 32 Cycle-3 chunk branches pruned).

## Verification environment

- Primary AMD dev host: Ryzen 9 5950X (Zen 3), 16C/32T, 64 MiB L3, DDR4, AVX2 (no AVX-512), CachyOS kernel `7.2.3-1-cachyos-custom`. `ryzen_smu` module NOT installed → AMD live subtimings render `N/A (DriverMissing)` (graceful, no panic). To unblock: the DKMS + pacman workflow in HANDOVER §7 (pacman-managed headers + dkms build/install + modules-load.d; AUTOINSTALL=yes auto-rebuilds on kernel updates).
- Intel test machine: LGA-1151 i5-6600 (Skylake, dual-channel) for live Intel MCHBAR decode verification.
- AIDA64 parity gate deferred to a DDR5-6000 AM5 host.

## Open items / acceptance gates (carried into Cycle 4)

1. AMD tick-identical ground truth (needs ryzen_smu module + root; compare vs ryzen_smu CLI: clocks ±1 MHz, voltages ±10 mV, CAD per code table).
2. Intel live MCHBAR decode (on the i5-6600).
3. Model reconciliation: AMD PM byte offsets + Intel IMC offsets are plan-mandated SKELETONS; SPD maker `0xC1` + density `0x0D` outside the frozen tables — reconcile against live silicon.
4. Phase 1 L1/L2 bandwidth overhead refinement (small working sets split across 16 pinned workers → add inner-loop iterations to amortize thread/barrier overhead).
5. MSRV decision: workspace 1.75 vs AVX-512F intrinsics needing 1.89 (deferred; kept 1.75 via lockfile pins — decide bump vs cfg-gate).
6. Push to GitHub (condition met; ready on operator go-ahead; never force-push).

## Next phase — Phase 5 (Packaging & Distribution)

Build the distribution/packaging layer so the app installs cleanly and handles its own dependencies:

- **PKGBUILD / AUR** (`ramsleuth-git`): packages the 7-crate workspace into installable binaries (daemon + client + tui + gui) with correct dependencies.
- **systemd preset + install dependency handling**: ship `systemd/ramsleuth.service` (already exists) + a preset; the install MUST create the `ramsleuth` group (the unit uses `Group=ramsleuth` — the group must exist or the service fails to start; this is the one real install-dependency gap). `/run/ramsleuth` is handled by the unit's `RuntimeDirectory=ramsleuth` (+ daemon fallback). CAP_SYS_RAWIO + sandboxing are in the unit.
- **ryzen_smu DKMS (optional recommended extra)**: package/provision the ryzen_smu DKMS module (AMD live subtimings) per HANDOVER §7, as a recommended extra (not a hard dependency — the app degrades gracefully without it).
- **GitHub Actions CI**: a workflow running `cargo test --workspace` (debug + release) + `cargo clippy --workspace --all-targets -- -D warnings` + a build matrix, on push/PR to `v2-development`.
- **Push to GitHub**: on operator go-ahead, fast-forward `v2-development` to `origin` (never force-push).

Follow Grand Design §4 (privilege model) and §5 (packaging/distribution).

## Operating protocol (strict)

- 100% pure Rust. No Python/C++/Qt/Electron/Tauri. GUI = egui + eframe (egui::Grid, egui_extras::TableBuilder); TUI = ratatui + crossterm.
- Strict phased gating: work linearly through the roadmap; each cycle must compile, run, and pass its exit criteria before advancing. Do not jump ahead.
- Git hygiene: dev branch `v2-development`; granular commits prefixed by phase (e.g. `feat(packaging): ...`, `ci: ...`); single-file micro-chunks on `branch/chunk-N` with per-chunk review/merge.
- Safety & fallbacks: all MMIO/SMU/socket/privilege access must guard against invalid CPUIDs, unknown PM table versions, missing drivers, and insufficient privilege — fall back gracefully to N/A. No unsupported CPU/driver/privilege state may ever panic or segfault. Packaging must not break the no-panic contract.
- MSRV: keep the workspace at 1.75 unless the MSRV decision (open item 5) is made; preserve the lockfile pins (ratatui/egui transitive deps) so all packages stay ≤ 1.75.
- Per-cycle workflow: (A) planning (author single-file micro-chunks to plans/), (B) pipelined implementation (implement → review/merge per chunk, sequential gated), (C) QA audit (full regression + static analysis + live run), (D) compaction (MASTER_LOG.md, prune merged branches, reset DEV_LOG.md). Use the `DEV_LOG.md` lease board and 3-line Semantic Pulse handoffs.

## First actions

1. Read `Docs/HANDOVER.md`, `FULLSCOPEvsCOMPLETED.md`, the Grand Design spec, and `RamSleuth-v2.md` in full.
2. Confirm `v2-development` is checked out and the tree is clean; run `cargo test --workspace` (debug + release) and `cargo clippy --workspace --all-targets -- -D warnings` to confirm the 327/327 green baseline.
3. Plan Phase 5 (packaging: PKGBUILD/AUR + systemd/group install + ryzen_smu DKMS extra + GitHub Actions CI) as single-file micro-chunks, then execute the gated pipeline.
4. (Operator, optional) Run the ryzen_smu DKMS + pacman setup (HANDOVER §7) on the AMD host to unblock AMD live verification (open item 1).
5. (Operator) Give the explicit go-ahead to push `v2-development` to `origin` when the packaging work is confirmed working (open item 6).
