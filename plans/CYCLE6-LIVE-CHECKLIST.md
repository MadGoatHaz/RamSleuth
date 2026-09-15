# Cycle 6 — Live-Run Verification Checklist (operator)

The automated QA audit (regression, clippy, build, MSRV) passed fully on
`v2-development @ 33dd08b`, but the **live-run check requires sudo
(SMU/PM access) and an operator-attended display**, so it is deferred to
the operator. This box's audit session had no passwordless sudo
(`sudo -n true` → "a password is required") even though a Wayland display
(`wayland-0`, `DISPLAY=:0`) is present.

Expected hardware context for sign-off: Ryzen 9 5950X (Zen 3),
DDR4-3600 G.Skill F4-3600C18-32GVK (1800 MHz MCLK), `ryzen_smu` v0.1.7
loaded, ground-truth CLI `/usr/bin/monitor_cpu` available for
cross-checks.

## Prerequisites

- [ ] `ryzen_smu` kernel module loaded (`lsmod | grep ryzen_smu`, expect v0.1.7)
- [ ] Release binaries built: `cargo build --workspace --release`
      (all 6: bench, client, daemon, gui, telemetry, tui)
- [ ] sudo available in terminal A (SMU/PM access for the daemon)
- [ ] A graphical display is available for the GUI (Wayland or X11)

## Run

1. **Terminal A (daemon, needs sudo):**
   ```bash
   sudo target/release/ramsleuth-daemon
   ```
   - Listens on **`/run/ramsleuth/ramsleuth.sock`** by default.
   - If unprivileged, override:
     `sudo target/release/ramsleuth-daemon --socket /tmp/ramsleuth.sock`
     (the client/GUI must then be pointed at the same socket).
   - Confirm: daemon banner prints, capability probe reports SOFT (never
     panics/exits), socket file exists.

2. **Terminal B (GUI, no sudo needed):**
   ```bash
   target/release/ramsleuth-gui
   ```

## Verify

- [ ] **Header** shows real CPU / motherboard / BIOS / AGESA strings + RAM summary
      (cross-check CPU string and RAM geometry against `lscpu` /
      `dmidecode` / `monitor_cpu`).
- [ ] **Zone 1** shows the 2×3 grouped sections + the GDM/CR row, with
      live 1800 MHz clocks (cross-check MCLK/fclk/mclk against
      `/usr/bin/monitor_cpu` while it is running).
- [ ] **Zone 2** live-fills cells during a **Run Full Benchmark** (no
      blanks/stale values as the benchmark progresses).
- [ ] **Zone 3** SPD cards show the G.Skill product line, SK hynix die,
      and Single-Rank.
- [ ] **History sparklines** (MCLK / VDDCR_SOC / BANDWIDTH) update over
      ~10 minutes of uptime.
- [ ] **Settings panel**: changing the poll interval and the socket path
      applies live (next poll uses the new settings).
- [ ] **F2** saves a 640×420 validation-card PNG.
- [ ] **F3** saves telemetry + benchmark JSON.
- [ ] **Q** quits the GUI cleanly.
- [ ] Steady **~60 FPS with no freezes** during an active bench run
      (watch the GUI while Zone 2 is filling).
- [ ] After finishing: stop the daemon with **SIGTERM**
      (`kill <daemon pid>` or Ctrl-C in terminal A); expect clean
      shutdown and socket removal.

## Known non-blocking UX follow-up (documented, not a Cycle 6 gate)

- Typing **"q" while the socket TextEdit has focus triggers Quit**.
  A focus-guard (only act on "q" when the TextEdit is not focused) is
  the follow-up; does not block Cycle 6 sign-off.
