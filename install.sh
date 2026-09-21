#!/usr/bin/env bash
#
# install.sh — RamSleuth v2 self-contained GitHub installer (AUR-parity).
# Plan: PLAN-CYCLE18.md chunk C18-08, design D-18.2.
#
# Lands RamSleuth in the EXACT state of the AUR (ramsleuth-git): the 6 binaries +
# the frozen unit + the preset + the `ramsleuth` group + the one-click setup
# helper (ramsleuth-setup) + its polkit policy into /usr via sudo, built
# with `cargo build --release --workspace --locked`. Interactive, visually
# sectioned, and TRANSPARENT: before anything touches the system it shows the
# host, the exact commit being installed, and every artifact + destination, then
# asks "Proceed?" (a `n` aborts — exit 0, no change). On AMD hosts it then ASKS
# about the optional ryzen_smu DKMS module and, on `y`, hands off to the shared
# pinned helper (scripts/install-ryzen-smu-dkms.sh). The ONLY third-party code
# anywhere is that module, pinned to amkillam/ryzen_smu @ d298366 (shown +
# checksummed + confirmed before any build); RamSleuth itself compiles only this
# repo (--locked).
#
# Usage:
#   git clone https://github.com/MadGoatHaz/RamSleuth && cd RamSleuth && ./install.sh
#
# Exit codes:  0 = installed, or declined (no change)   1 = hard failure
#              2 = bad usage / preflight refusal (not a checkout, non-Arch, no Rust)
#
# Every step is idempotent; a re-run is always safe; a failure prints a clear
# message + exits non-zero (never a silent half-state).
#
set -euo pipefail

# --- Resolve the repo root (this file lives at the repo root) ----------------
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)" || REPO_ROOT=""
cd -- "$SCRIPT_DIR"

# --- Constants ---------------------------------------------------------------
VERSION="$(awk -F'"' '$1 ~ /^version/{print $2; exit}' "$REPO_ROOT/Cargo.toml" 2>/dev/null)" || VERSION=""
[[ -n "$VERSION" ]] || VERSION="2.2.0"
CPU_VENDOR="$(grep -m1 'vendor_id' /proc/cpuinfo 2>/dev/null | awk '{print $3}')" || CPU_VENDOR="unknown"
COMMIT="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null)" || COMMIT="unknown"
BRANCH="$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null)" || BRANCH="unknown"
# The 6 installable binaries (ramsleuth-protocol is lib-only, never installed).
# The GUI binary is `ramsleuth` (the ramsleuth-gui crate — the C18-01 rename).
BINARIES=( ramsleuth-daemon ramsleuth-client ramsleuth-tui ramsleuth ramsleuth-bench ramsleuth-telemetry )
# The ONLY third-party source anywhere: the optional AMD module (D-18.3 pin).
RYZEN_SMU_REPO="amkillam/ryzen_smu"
RYZEN_SMU_PIN="d2983668300dd2a598e5a7dc40e71ce0678cc270"
RYZEN_SMU_PIN_SHORT="d298366"
DKMS_STATE=""   # set in amd_walk: "skipped" | "intel"
# --- Color / TTY (auto-disabled off a TTY) ------------------------------------
if [[ -t 1 ]]; then
  C_RESET=$'\033[0m';  C_BOLD=$'\033[1m';  C_DIM=$'\033[2m'
  C_CYAN=$'\033[96m';  C_GREEN=$'\033[92m'; C_RED=$'\033[91m'
  C_AMBER=$'\033[93m'; C_SLAB=$'\033[48;5;235m'
else
  C_RESET=""; C_BOLD=""; C_DIM=""; C_CYAN=""; C_GREEN=""; C_RED=""; C_AMBER=""; C_SLAB=""
fi
is_tty() { [[ -t 0 && -t 1 ]]; }
# --- Output helpers ------------------------------------------------------------
info()   { printf '%s\n' "$*"; }
header() { printf '\n%s\n' "${C_BOLD}${C_CYAN}━━━  $*  ${C_RESET}"; }
step()   { printf '  %s▸%s %s\n' "${C_CYAN}" "${C_RESET}" "$*"; }
ok()     { printf '  %s✓%s %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
warn()   { printf '  %s!%s %s\n' "${C_AMBER}" "${C_RESET}" "$*"; }
die()    { printf '\n%s\n' "${C_BOLD}${C_RED}✗ $*${C_RESET}" >&2; exit "${2:-1}"; }
# ask <prompt> <default: Y|N> — returns 0 on "yes". Off a TTY it uses the
# default and logs the choice (a fully scripted context never blocks).
ask() {
  local prompt="$1" def="$2" answer
  if is_tty; then
    read -r -p "$prompt" answer || answer=""
  else
    answer="$def"
    info "  ${C_DIM}(non-interactive) assumed '${def:0:1}' — $prompt${C_RESET}"
  fi
  [[ -n "$answer" ]] || answer="$def"
  case "${answer:0:1}" in [Yy]) return 0 ;; *) return 1 ;; esac
}

# --- Phase A: unprivileged (nothing touches the system yet) --------------------
banner() {
  local line="────────────────────────────────────────────────────────────"
  printf '\n'
  printf '  %s%s%s\n' "${C_CYAN}" "$line" "${C_RESET}"
  printf '  %s  RamSleuth  v%s%s' "${C_SLAB}${C_CYAN}${C_BOLD}" "$VERSION" "${C_RESET}"
  printf '   %s—  RAM latency/bandwidth telemetry, self-contained installer%s\n' "${C_DIM}" "${C_RESET}"
  printf '  %s%s%s\n' "${C_CYAN}" "$line" "${C_RESET}"
  printf '\n'
}
preflight() {
  header "Preflight — unprivileged (no system changes yet)"
  # Must be a valid RamSleuth checkout (this file lives at the repo root).
  [[ -n "$REPO_ROOT" ]] || die "Not run from a git checkout (git rev-parse --show-toplevel failed)." 2
  [[ "$(git -C "$REPO_ROOT" rev-parse --show-toplevel 2>/dev/null)" == "$REPO_ROOT" ]] || die "'$SCRIPT_DIR' is not the root of a git checkout." 2
  [[ -f "$REPO_ROOT/Cargo.toml" ]] || die "No Cargo.toml — not a RamSleuth checkout." 2
  [[ -d "$REPO_ROOT/packaging/ramsleuth-git" ]] || die "No packaging/ramsleuth-git/ — not a RamSleuth checkout." 2
  # Arch-based (the Arch/AUR-parity path; no partial install on other distros).
  if ! command -v pacman >/dev/null 2>&1; then
    die "Non-Arch host (no pacman) — this installer is the Arch/AUR-parity path.
See README.md ('Manual build') for the distro-agnostic flow." 2
  fi
  ok "Arch-based system (pacman present)"
  # Rust toolchain (never auto-installed — we only print the command).
  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    die "Rust toolchain (cargo/rustc) not found — this installer does NOT auto-install Rust.
Install it, then re-run:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" 2
  fi
  ok "Rust toolchain present"
  # Host facts + the exact commit being installed.
  local distro kernel
  distro="$(grep -m1 '^PRETTY_NAME=' /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d '"')"
  [[ -n "$distro" ]] || distro="unknown"
  kernel="$(uname -r)"
  step "distro:     $distro"
  step "kernel:     $kernel"
  step "CPU vendor: $CPU_VENDOR"
  step "commit:     $COMMIT  (branch $BRANCH)"
}
transparency_block() {
  header "Transparency — exactly what will be installed and where"
  step "6 binaries → /usr/bin:"
  local b
  for b in "${BINARIES[@]}"; do
    if [[ "$b" == "ramsleuth" ]]; then
      step "   · ramsleuth   (the GUI — the ramsleuth-gui crate; was ramsleuth-gui)"
    else
      step "   · $b"
    fi
  done
  step "frozen unit   → /usr/lib/systemd/system/ramsleuth.service"
  step "systemd preset → /usr/lib/systemd/system-preset/ramsleuth.preset"
  step "app-menu entry → /usr/share/applications/ramsleuth.desktop"
  step "hicolor icons  → /usr/share/icons/hicolor/{16,24,32,48,64,128,256,512}/apps/ramsleuth.png"
  step "system group   → 'ramsleuth'  (groupadd -r; the unit runs as Group=ramsleuth)"
  step "shared helper  → /usr/bin/ramsleuth-install-ryzen-smu-dkms"
  step "setup helper   → /usr/bin/ramsleuth-setup   (one-click privileged setup; pkexec-able)"
  step "polkit policy  → /usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
  step "this installer → /usr/share/ramsleuth/install.sh   (kept for audit / re-run)"
  printf '\n'
  printf '  %sThird-party source statement:%s\n' "${C_BOLD}" "${C_RESET}"
  printf '   The only third-party code is the OPTIONAL ryzen_smu AMD module, pinned to\n'
  printf '   %s%s @ %s%s (verified 2026-08-15). The shared helper fetches it, shows it,\n' "${C_AMBER}" "$RYZEN_SMU_REPO" "$RYZEN_SMU_PIN" "${C_RESET}"
  printf '   checksums it, and confirms it BEFORE any build. RamSleuth itself compiles\n'
  printf '   ONLY this repo (branch-pinned, --locked) — no third-party code is ever built.\n'
}
confirm_proceed() {
  header "Confirm"
  if ask "Proceed with the install above? [Y/n] " "Y"; then
    return 0
  fi
  info "  ${C_DIM}Aborted — no system changes were made.${C_RESET}"
  return 1
}
exec_sudo() {
  info "  ${C_DIM}Continuing as root — your sudo prompt appears below. Nothing is built or${C_RESET}"
  info "  ${C_DIM}installed until after you authorize it.${C_RESET}"
  exec sudo env RAMSLEUTH_SUDO_REEXEC=1 "${SCRIPT_DIR}/install.sh" "$@"
}
# --- Phase B: root (after the sudo re-exec) ------------------------------------
do_build() {
  header "Build — cargo --locked (the AUR's exact command)"
  step "cargo build --release --workspace --locked"
  cargo build --release --workspace --locked
  ok "Build complete → target/release/"
}
do_install() {
  header "Install — AUR-mirrored"
  local b
  for b in "${BINARIES[@]}"; do
    install -Dm755 "target/release/$b" "/usr/bin/$b"
    ok "/usr/bin/$b"
  done
  install -Dm644 "systemd/ramsleuth.service" "/usr/lib/systemd/system/ramsleuth.service"
  ok "/usr/lib/systemd/system/ramsleuth.service  (frozen unit, verbatim)"
  install -Dm644 "packaging/ramsleuth-git/ramsleuth.preset" "/usr/lib/systemd/system-preset/ramsleuth.preset"
  ok "/usr/lib/systemd/system-preset/ramsleuth.preset"
  install -Dm644 "packaging/ramsleuth-git/ramsleuth.desktop" "/usr/share/applications/ramsleuth.desktop"
  ok "/usr/share/applications/ramsleuth.desktop  (app-menu entry)"
  # The 8 hicolor icons (AUR-parity: the same sizes every AUR package installs, C21-27) —
  # they back the app-menu entry's Icon=ramsleuth (the hicolor theme lookup).
  local size
  for size in 16 24 32 48 64 128 256 512; do
    [[ -f "assets/icons/hicolor/$size/apps/ramsleuth.png" ]] || die "Missing assets/icons/hicolor/$size/apps/ramsleuth.png — incomplete checkout; the hicolor icons cannot be installed." 1
    install -Dm644 "assets/icons/hicolor/$size/apps/ramsleuth.png" "/usr/share/icons/hicolor/$size/apps/ramsleuth.png"
    ok "/usr/share/icons/hicolor/$size/apps/ramsleuth.png"
  done
  # The system group (idempotent getent guard).
  if getent group ramsleuth >/dev/null 2>&1; then
    ok "group 'ramsleuth' already present"
  else
    groupadd -r ramsleuth
    ok "group 'ramsleuth' created (system group)"
  fi
  # Opt-in group membership for the invoking user (unprivileged CLI/TUI access).
  if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]]; then
    if ask "Add ${SUDO_USER} to the ramsleuth group (unprivileged CLI/TUI access)? [Y/n] " "Y"; then
      usermod -aG ramsleuth "$SUDO_USER"
      ok "${SUDO_USER} added to group 'ramsleuth' (persistent; effective at next login)"
      # Seed the ACL state file too (the C21 'no re-login' contract): the daemon
      # re-applies a per-user socket ACL from it on every (re)start, so the
      # CURRENT session gets socket access immediately. Idempotent (exact-line
      # dedupe); dir 0755, file 0644, one username per line.
      install -d -m 0755 /etc/ramsleuth
      if ! grep -qx -F -- "$SUDO_USER" /etc/ramsleuth/authorized-users 2>/dev/null; then
        printf '%s\n' "$SUDO_USER" >> /etc/ramsleuth/authorized-users
      fi
      chmod 0644 /etc/ramsleuth/authorized-users
      ok "${SUDO_USER} seeded in /etc/ramsleuth/authorized-users (current-session access, no re-login)"
    else
      info "  ${C_DIM}Skipped group membership for ${SUDO_USER}.${C_RESET}"
    fi
  fi
  # The two shipped transparency artifacts (re-runnable + auditable post-install).
  install -Dm755 "scripts/install-ryzen-smu-dkms.sh" "/usr/bin/ramsleuth-install-ryzen-smu-dkms"
  ok "/usr/bin/ramsleuth-install-ryzen-smu-dkms  (the shared pinned helper)"
  # The one-click setup pair (AUR-parity: the same two artifacts every AUR package installs).
  [[ -f "scripts/ramsleuth-setup.sh" ]] || die "Missing scripts/ramsleuth-setup.sh — incomplete checkout; the one-click setup helper cannot be installed." 1
  [[ -f "packaging/polkit/90-ramsleuth-setup.policy" ]] || die "Missing packaging/polkit/90-ramsleuth-setup.policy — incomplete checkout; the polkit policy cannot be installed." 1
  install -Dm755 "scripts/ramsleuth-setup.sh" "/usr/bin/ramsleuth-setup"
  ok "/usr/bin/ramsleuth-setup  (the one-click privileged setup helper)"
  install -Dm644 "packaging/polkit/90-ramsleuth-setup.policy" "/usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
  ok "/usr/share/polkit-1/actions/90-ramsleuth-setup.policy  (its polkit policy)"
  install -Dm755 "install.sh" "/usr/share/ramsleuth/install.sh"
  ok "/usr/share/ramsleuth/install.sh  (this installer)"
  # systemd (guarded, non-fatal — the no-panic install contract).
  systemctl daemon-reload 2>/dev/null || true
  if systemctl enable --now ramsleuth.service 2>/dev/null; then
    ok "systemd: daemon-reload + ramsleuth.service enabled & started"
  else
    warn "systemd: could not start ramsleuth.service (non-fatal) — try: systemctl enable --now ramsleuth"
  fi
}
amd_walk() {
  if [[ "$CPU_VENDOR" == "AuthenticAMD" ]]; then
    header "AMD — optional ryzen_smu kernel module"
    printf '  ryzen_smu enables LIVE AMD subtimings. It is OPTIONAL: without it the app\n'
    printf '  runs fine — the AMD section reads N/A (DriverMissing), exit 0, no panic.\n'
    printf '  Installing it now runs the shared pinned helper, which fetches\n'
    printf '  %s@ %s, shows + checksums + confirms it, then DKMS-builds it.\n\n' "$RYZEN_SMU_REPO" "$RYZEN_SMU_PIN_SHORT"
    if ask "Install the ryzen_smu DKMS module now? [y/N] " "N"; then
      step "Handing off to: scripts/install-ryzen-smu-dkms.sh"
      exec "scripts/install-ryzen-smu-dkms.sh"
    fi
    DKMS_STATE="skipped"
    printf '  Skipped. Run it later with:    %ssudo ramsleuth-install-ryzen-smu-dkms%s\n' "${C_CYAN}" "${C_RESET}"
    printf '  (or the AUR extra:            %syay -S ryzen-smu-dkms%s)\n' "${C_DIM}" "${C_RESET}"
  else
    header "CPU — $CPU_VENDOR"
    info "  Intel: built-in MCHBAR decode — no extra kernel driver is needed."
    DKMS_STATE="intel"
  fi
}
do_summary() {
  header "Summary"
  printf '  %s✓ Installed RamSleuth v%s%s  (commit %s, branch %s)\n' "${C_GREEN}" "$VERSION" "${C_RESET}" "$COMMIT" "$BRANCH"
  printf '  %sStart now:%s\n' "${C_BOLD}" "${C_RESET}"
  step "ramsleuth                 # the GUI — one click on 'Set up RamSleuth' (no re-login, no reboot)"
  step "sudo ramsleuth-setup        # the same one-click setup from a terminal (CLI equivalent)"
  printf '  %sManual fallback (copy-paste):%s\n' "${C_DIM}" "${C_RESET}"
  step "ramsleuth-tui             # the terminal UI"
  step "ramsleuth-client dump     # one-shot CLI telemetry"
  printf '  %sDay-2:%s\n' "${C_BOLD}" "${C_RESET}"
  step "systemctl status ramsleuth"
  step "journalctl -u ramsleuth -f"
  if [[ "$DKMS_STATE" == "skipped" ]]; then
    printf '\n  %sDegradation note:%s without the ryzen_smu module the AMD section reads\n' "${C_AMBER}" "${C_RESET}"
    printf '  N/A (DriverMissing), exit 0, no panic. Enable live AMD subtimings with:\n'
    printf '     %ssudo ramsleuth-setup --with-dkms%s  (or the helper: sudo ramsleuth-install-ryzen-smu-dkms)\n' "${C_CYAN}" "${C_RESET}"
  fi
  printf '\n  %sTransparency footer:%s every installed file is byte-identical to a file in this\n' "${C_DIM}" "${C_RESET}"
  printf '  repo (audit with: git show). The build was --locked from commit %s. The only\n' "$COMMIT"
  printf '  third-party source is the optional AMD module %s@ %s%s — pinned, shown,\n' "${C_AMBER}" "$RYZEN_SMU_REPO" "${C_RESET}"
  printf '  checksummed, and confirmed by the helper before any build.\n'
  printf '\n  %s✓ Done — RamSleuth is installed and the daemon is running.%s\n' "${C_BOLD}${C_GREEN}" "${C_RESET}"
}
# --- Main dispatch -------------------------------------------------------------
main() {
  banner
  # After the sudo re-exec we are root + marked → the user already confirmed.
  if [[ "$(id -u)" -eq 0 && "${RAMSLEUTH_SUDO_REEXEC:-0}" == "1" ]]; then
    do_build
    do_install
    amd_walk
    do_summary
  else
    preflight
    transparency_block
    if ! confirm_proceed; then
      exit 0
    fi
    exec_sudo "$@"
  fi
}
main "$@"
