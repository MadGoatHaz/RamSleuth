# RamSleuth — Project Notes

RamSleuth is a Rust Cargo workspace (v2.4.17) providing live RAM telemetry plus a
Memory & Latency Benchmark for AMD/Intel desktops: a privileged root daemon
(`CAP_SYS_RAWIO` only, Unix-socket IPC) feeding dual frontends (ratatui TUI + egui
GUI), with CLI and standalone tools.

## Documentation layout

**Repo-facing user documentation — committed and shipped:**

- `README.md` — the entry point: what RamSleuth is, installation, quickstart, and
  pointers to the documents below.
- `Docs/Architecture.md` — the deep technical design: the two-layer model, the
  daemon and wire protocol, the 9-crate workspace map, telemetry sources, the
  benchmark engine, install/deployment, and testing/CI.
- `Docs/User_Guide.md` — how to install, run, and read the data (GUI, TUI, CLI,
  standalone tools, N/A reasons, troubleshooting, uninstall).
- `packaging/README.md` — the packaging / AUR / operator guide: the AUR package
  model (2 main packages + 2 optional vendor DKMS extras), the version-bump
  standing policy, the `ryzen-smu-dkms` extra, and the CI/release artifact
  contracts.

**Development documentation — local-only, gitignored, never commit:**

- `work/` — the SINGLE gitignored dev-work space. It holds the active lease
  board (`work/DEV_LOG.md`), the master log (`work/MASTER_LOG.md`), plans
  (`work/plans/`), research notes (`work/research/`), and scratch
  (`work/scratch/`), plus the other dev-docs: general specs (`work/docs/`),
  handovers (`work/handovers/`), the AUR ops reference (`work/aur-ops/`), scope
  tracking (`work/scope/`), and SPD image dumps (`work/SPD_Images/`). It is NOT
  repo-facing — never `git add -A` it.
- `plans/` — local plan files (gitignored).
- `MASTER_LOG.md`, `DEV_LOG.md`, `.kilo/` — developer log and agent state
  (gitignored; working-tree only).

## Layout notes

- The systemd unit lives at repo-root `systemd/ramsleuth.service` (not under
  `packaging/`).
- Packaging (AUR packages, DKMS extra, release artifacts) lives under
  `packaging/`; the polkit policy at `packaging/polkit/`, the one-click setup
  helper at `scripts/ramsleuth-setup.sh`.
