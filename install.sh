#!/usr/bin/env bash
#
# install.sh — RamSleuth v2 self-contained installer (any distro with
# pacman, apt, or dnf).
#
# NOTE: on Debian/Ubuntu/Mint an official `.deb` package is published on
# the GitHub Release (ramsleuth_<version>_amd64.deb) — `dpkg -i` is the
# simplest install there. This script remains the universal, any-distro
# path (and the one used on the other distros).
#
# PRIMARY PATH (the default): downloads the PRECOMPILED release binary
# tarball from the official GitHub Release and installs it as-is — NO
# Rust toolchain, NO build, NO git clone required. The tarball's sha256
# is verified against the release's published .sha256 sidecar BEFORE
# anything touches the system. All the script needs is an HTTP client
# (curl or wget), tar, and zstd (auto-installed if missing).
#
# FALLBACK PATH (automatic, or forced with --build): installs the
# distro's build prerequisites, compiles with
# `cargo build --release --workspace --locked`, and installs from
# target/release/. Used when no prebuilt binary exists for this
# architecture (today the releases are x86_64-only, so aarch64 hosts
# take this path) — a Rust toolchain is required for it.
#
# Both paths land RamSleuth in the EXACT state of the AUR
# (ramsleuth / ramsleuth-bin): the 6 binaries + the frozen unit + the
# preset + the app-menu entry + the hicolor icons + the `ramsleuth`
# group + the two DKMS helpers (the AMD pinned ryzen_smu helper + the
# Intel in-repo ramsleuth_intel helper, the latter guarded) + their
# offline source trees (the vendored ryzen_smu + the ramsleuth_intel
# module source) + the one-click setup helper (ramsleuth-setup) + its
# polkit policy into /usr — then activates via ramsleuth-setup (group,
# service, socket ACL, polkit). On AMD/Intel hosts it ASKS about the
# optional DKMS module and, on `y`, hands off to the installed helper.
#
# The ONLY third-party code anywhere is the AMD ryzen_smu module,
# pinned to amkillam/ryzen_smu @ d298366 (shown + checksummed +
# confirmed by the helper before any build); RamSleuth itself compiles
# only this repo (--locked).
#
# Usage:
#   git clone https://github.com/MadGoatHaz/RamSleuth && cd RamSleuth && ./install.sh
#
# Flags:
#   --build            force the source-build path (developers)
#   --version <ver>    install a specific release (default: the latest)
#   -h, --help         show this help
#
# Exit codes:  0 = installed, or declined (no change)   1 = hard failure
#              2 = bad usage / preflight refusal (unsupported arch, no
#                  supported package manager, no Rust on the build path)
#
# Every step is idempotent; a re-run is always safe; a failure prints a
# clear message + exits non-zero (never a silent half-state).
#
set -euo pipefail

# --- Resolve the repo root (this file lives at the repo root) ----------------
# The binary path does NOT require a checkout (run it from anywhere);
# the build path does (preflight_build validates it).
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)" || REPO_ROOT=""
cd -- "$SCRIPT_DIR"

# --- Constants ---------------------------------------------------------------
REPO="MadGoatHaz/RamSleuth"
ARCH="$(uname -m)"
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

# --- Flags / state -------------------------------------------------------------
MODE="auto"          # auto = binary first, source fallback | build = source only
WANT_VERSION=""      # --version <ver>
VERSION=""           # resolved (user phase) / inherited (root phase)
SHA256=""            # the verified tarball hash (binary path)
GROUP_OPTIN=1        # the group-membership choice (asked in the user phase)
DKMS_STATE=""        # set in amd_walk: "skipped" | "intel"
SRC=""               # the install source dir (release tree or repo root)
STAGE_TMP=""         # the binary-path staging dir (cleaned on exit)

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
die()    { printf '\n%s\n' "${C_BOLD}${C_RED}✗ ${1:-}${C_RESET}" >&2; exit "${2:-1}"; }
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

usage() {
  cat <<'EOF'
RamSleuth installer

Usage: ./install.sh [--build] [--version <ver>]

  (default)          install the latest PRECOMPILED release binary from
                     GitHub — no Rust toolchain, no build required
  --build            force the source-build path (installs the distro
                     build prerequisites, then cargo build --locked)
  --version <ver>    install a specific release (e.g. 2.4.18) instead of
                     the latest
  -h, --help         show this help

Exit codes: 0 = installed (or declined) | 1 = hard failure | 2 = bad usage
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) MODE="build" ;;
    --version)
      [[ $# -ge 2 ]] || { usage >&2; die "--version requires a value (e.g. 2.4.18)" 2; }
      WANT_VERSION="$2"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" 2 ;;
  esac
  shift
done

# --- HTTP helpers (curl preferred, wget fallback) ------------------------------
http_get() { # $1=url $2=dest-file
  # No `--` before -o: after `--` curl/wget treat -o/-O as a second URL.
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$1" -O "$2"
  else
    die "no HTTP client found (need curl or wget) — install one and re-run." 2
  fi
}
http_get_stdout() { # $1=url
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$1"
  else
    die "no HTTP client found (need curl or wget) — install one and re-run." 2
  fi
}
# http_ok $1=url — 0 when the URL answers (following redirects).
http_ok() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsIL -o /dev/null -- "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget --spider -q -O /dev/null -- "$1"
  else
    return 1
  fi
}

# --- zstd (the release asset is .tar.zst) ---------------------------------------
# ensure_zstd — the binary path needs a .tar.zst extractor: built-in tar zstd
# support (Debian/Arch/Fedora tar) or the zstd CLI (installed if missing).
ensure_zstd() {
  # Modern GNU tar often links libzstd directly — no separate package needed.
  if tar --zstd -cf "${TMPDIR:-/tmp}/.ramsleuth-zst-test.$$" --files-from=/dev/null 2>/dev/null; then
    rm -f "${TMPDIR:-/tmp}/.ramsleuth-zst-test.$$"
    ok "tar has built-in zstd support"
    return 0
  fi
  if command -v zstd >/dev/null 2>&1; then
    ok "zstd present"
    return 0
  fi
  step "installing zstd (required for the .tar.zst release asset)"
  if command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --needed --noconfirm zstd || die "zstd install failed (pacman) — install it manually and re-run." 1
  elif command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y zstd || die "zstd install failed (apt-get) — install it manually and re-run." 1
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y zstd || die "zstd install failed (dnf) — install it manually and re-run." 1
  else
    die "no zstd and no supported package manager (pacman/apt-get/dnf) to install it — install zstd manually and re-run." 2
  fi
  ok "zstd installed"
}
# extract_zst $1=file $2=dest-dir — tar --zstd first, zstd CLI as fallback.
extract_zst() {
  if tar --zstd -xf "$1" -C "$2" 2>/dev/null; then
    return 0
  fi
  zstd -dc "$1" | tar -xf - -C "$2"
}

# --- Release resolution ----------------------------------------------------------
# resolve_version — $WANT_VERSION (validated) or the latest published release
# (GitHub API first, the unrate-limited releases atom feed as fallback).
resolve_version() {
  if [[ -n "$WANT_VERSION" ]]; then
    [[ "$WANT_VERSION" =~ ^[0-9]+(\.[0-9]+)+$ ]] \
      || die "invalid --version '$WANT_VERSION' (expected MAJOR.MINOR.PATCH, e.g. 2.4.18)" 2
    VERSION="$WANT_VERSION"
    step "version: v$VERSION (pinned by --version)"
    return 0
  fi
  step "resolving the latest release…"
  local json tag feed url
  if json="$(http_get_stdout "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null)"; then
    tag="$(printf '%s' "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
    if [[ -n "$tag" ]]; then
      VERSION="${tag#v}"
      ok "latest release: v$VERSION"
      return 0
    fi
  fi
  if feed="$(http_get_stdout "https://github.com/$REPO/releases.atom" 2>/dev/null)"; then
    # The feed lists releases newest-first — the first tag href is the latest
    # (grep -o returns leftmost-first matches; a greedy sed would pick the LAST).
    url="$(printf '%s' "$feed" | grep -o 'releases/tag/[^"]*' | head -n1)"
    url="${url#releases/tag/}"
    if [[ -n "$url" ]]; then
      VERSION="${url#v}"
      ok "latest release: v$VERSION (via the releases atom feed)"
      return 0
    fi
  fi
  die "could not determine the latest release (the GitHub API and atom feed are both unreachable) — pass --version <ver> explicitly." 1
}
# asset_available $1=version — 0 when the prebuilt tarball for this arch exists.
asset_available() {
  http_ok "https://github.com/$REPO/releases/download/v$1/ramsleuth-$1-$ARCH.tar.zst"
}
# verify_sha256 $1=tarball $2=sidecar — the release's published .sha256
# (sha256sum format) is the trust anchor: a mismatch is a hard stop.
verify_sha256() {
  local expected actual
  expected="$(head -n1 "$2" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
  actual="$(sha256sum "$1" | awk '{print $1}')"
  [[ -n "$expected" && "$expected" == "$actual" ]] \
    || die "sha256 MISMATCH — the downloaded tarball does not match the release's published checksum.
  expected: $expected
  actual:   $actual
Refusing to install an unverified tarball (possible network interception). Re-run to retry." 1
  SHA256="$actual"
  ok "sha256 verified: $actual"
}
# validate_release_tree $1=src — the REQUIRED members of the release tarball
# (an incomplete one is a hard stop); the optional members (the Intel helper,
# the icon tree, the DKMS source trees) are guarded at install time.
validate_release_tree() {
  local src="$1" f missing=0
  for f in \
    "bin/ramsleuth-daemon" "bin/ramsleuth-client" "bin/ramsleuth-tui" \
    "bin/ramsleuth" "bin/ramsleuth-bench" "bin/ramsleuth-telemetry" \
    "systemd/ramsleuth.service" "systemd/ramsleuth.preset" \
    "ramsleuth-setup.sh" "90-ramsleuth-setup.policy" \
    "install.sh" "LICENSE" "scripts/install-ryzen-smu-dkms.sh"
  do
    if [[ ! -f "$src/$f" ]]; then
      warn "release tarball missing: $f"
      missing=1
    fi
  done
  [[ $missing -eq 0 ]] \
    || die "the release tarball is incomplete (the missing files are listed above) — try a newer --version, or the source build (--build)." 1
  ok "release tree complete: $src"
}

# --- Preflight: build path only (checkout + distro deps + Rust) ----------------
preflight_build() {
  header "Preflight — source build (checkout + prerequisites + Rust; escalates via sudo; no RamSleuth files installed yet)"
  # Must be a valid RamSleuth checkout (this file lives at the repo root).
  [[ -n "$REPO_ROOT" ]] || die "Not run from a git checkout (git rev-parse --show-toplevel failed) — the source build needs the full repo.
Get one with: git clone https://github.com/$REPO && cd RamSleuth && ./install.sh" 2
  [[ "$(git -C "$REPO_ROOT" rev-parse --show-toplevel 2>/dev/null)" == "$REPO_ROOT" ]] || die "'$SCRIPT_DIR' is not the root of a git checkout." 2
  [[ -f "$REPO_ROOT/Cargo.toml" ]] || die "No Cargo.toml — not a RamSleuth checkout." 2
  [[ -d "$REPO_ROOT/packaging/ramsleuth" ]] || die "No packaging/ramsleuth/ — not a RamSleuth checkout." 2
  # Distro-aware build prerequisites (the install body is distro-agnostic:
  # install -D, groupadd, systemctl). Preflight runs UNPRIVILEGED (the sudo
  # re-exec happens later, after the transparency block + Proceed), so the
  # package installs escalate with sudo.
  if command -v pacman >/dev/null 2>&1; then
    # Arch/AUR path: base-devel carries the toolchain; --needed makes a re-run a no-op.
    step "ensuring base-devel (pacman, idempotent)"
    sudo pacman -S --needed --noconfirm base-devel \
      || die "pacman prerequisite install failed — fix the error above and re-run." 1
    ok "Arch-based system (pacman present; base-devel ensured)"
  elif command -v apt-get >/dev/null 2>&1; then
    # Debian/Ubuntu/Mint: the C toolchain + pkg-config + the X11/Wayland/GL dev
    # libs the egui GUI links against.
    step "installing build prerequisites (apt-get)"
    sudo apt-get install -y build-essential pkgconf acl \
         libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev \
         libxi-dev libxcursor-dev libxinerama-dev libgl1-mesa-dev \
      || die "apt-get prerequisite install failed — fix the error above and re-run." 1
    ok "Debian-based system (apt-get prerequisites installed)"
  elif command -v dnf >/dev/null 2>&1; then
    # Fedora: the -devel counterparts of the same toolchain + GUI dev libs.
    step "installing build prerequisites (dnf)"
    sudo dnf install -y gcc pkgconf acl \
         libxkbcommon-devel wayland-devel libX11-devel libXrandr-devel \
         libXi-devel libXcursor-devel libXinerama-devel mesa-libGL-devel \
      || die "dnf prerequisite install failed — fix the error above and re-run." 1
    ok "Fedora system (dnf prerequisites installed)"
  else
    die "No supported package manager found (need pacman, apt-get, or dnf).
See Docs/User_Guide.md §2.6 'Install (any distro)' for the distro-agnostic flow." 2
  fi
  # Rust toolchain (never auto-installed — we only print the command).
  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    die "Rust toolchain (cargo/rustc) not found — this installer does NOT auto-install Rust.
Install it, then re-run:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" 2
  fi
  ok "Rust toolchain present"
  # Host facts + the exact commit being built.
  local distro kernel
  distro="$(grep -m1 '^PRETTY_NAME=' /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d '"')"
  [[ -n "$distro" ]] || distro="unknown"
  kernel="$(uname -r)"
  step "distro:     $distro"
  step "kernel:     $kernel"
  step "arch:       $ARCH"
  step "CPU vendor: $CPU_VENDOR"
  step "commit:     $COMMIT  (branch $BRANCH)"
}

# --- Transparency (both paths show the identical install surface) ---------------
transparency_block() { # $1 = the source description
  header "Transparency — exactly what will be installed and where"
  step "source: $1"
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
  step "app-menu entry → /usr/share/applications/RamSleuth.desktop  (the filename matches the GUI's Wayland app_id; C21-45)"
  step "hicolor icons  → /usr/share/icons/hicolor/<size>/apps/ + <size>x<size>/apps/ramsleuth.png  (bare + NxN)"
  step "pixmaps icon   → /usr/share/pixmaps/ramsleuth.png  (the 48px legacy fallback)"
  step "icon name      → RamSleuth.png in the same hicolor + pixmaps dirs  (the app_id direct-icon-name fallback; C21-45)"
  step "system group   → 'ramsleuth'  (groupadd -r; the unit runs as Group=ramsleuth)"
  step "shared helper  → /usr/bin/ramsleuth-install-ryzen-smu-dkms"
  step "DKMS config    → /usr/share/ryzen-smu-dkms/dkms.conf  (the repo's config — DEST=/extra, matching the helper's depmod override; guarded)"
  step "AMD vendor src → /usr/share/ryzen-smu-dkms/vendor/  (the 8 vendored ryzen_smu files — the helper's offline, SUMS-verified copy; guarded)"
  step "Intel helper   → /usr/bin/ramsleuth-install-intel-dkms  (the shared Intel DKMS helper; guarded — skipped if absent in a pre-Intel checkout)"
  step "Intel module   → /usr/share/ramsleuth-intel-dkms/src/  (the in-repo ramsleuth_intel source — the helper's installed copy; guarded)"
  step "setup helper   → /usr/bin/ramsleuth-setup   (one-click privileged setup; pkexec-able)"
  step "polkit policy  → /usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
  step "license        → /usr/share/licenses/ramsleuth/LICENSE"
  step "this installer → /usr/share/ramsleuth/install.sh   (kept for audit / re-run)"
  step "activation     → ramsleuth-setup  (group membership, service start, socket ACL, polkit)"
  printf '\n'
  printf '  %sThird-party source statement:%s\n' "${C_BOLD}" "${C_RESET}"
  printf '   The only third-party code is the OPTIONAL ryzen_smu AMD module, pinned to\n'
  printf '   %s%s @ %s%s (verified 2026-08-15). The shared helper fetches it, shows it,\n' "${C_AMBER}" "$RYZEN_SMU_REPO" "$RYZEN_SMU_PIN" "${C_RESET}"
  printf '   checksums it, and confirms it BEFORE any build. RamSleuth itself compiles\n'
  printf '   ONLY this repo (branch-pinned, --locked) — no third-party code is ever built.\n'
  printf '   The ramsleuth_intel Intel module, by contrast, is RamSleuth'"'"'s own original\n'
  printf '   creation — its source lives in-repo (kernel/ramsleuth-intel/): no upstream,\n'
  printf '   no pin, no third-party code.\n'
}

confirm_proceed() {
  header "Confirm"
  if ask "Proceed with the install above? [Y/n] " "Y"; then
    return 0
  fi
  info "  ${C_DIM}Aborted — no system changes were made.${C_RESET}"
  return 1
}

# group_optin_ask — the opt-in the old installer asked mid-root-phase; it is
# asked HERE (unprivileged, before the sudo re-exec) and carried via env.
group_optin_ask() {
  GROUP_OPTIN=1
  local user
  user="$(id -un)"
  [[ "$user" == "root" ]] && return 0   # true root: no membership question
  if ask "Add $user to the ramsleuth group (unprivileged CLI/TUI access)? [Y/n] " "Y"; then
    GROUP_OPTIN=1
  else
    GROUP_OPTIN=0
    info "  ${C_DIM}Skipped group membership for $user (the daemon still installs + starts).${C_RESET}"
  fi
}

exec_sudo() {
  info "  ${C_DIM}Continuing as root — your sudo prompt appears below. Nothing is${C_RESET}"
  info "  ${C_DIM}installed until after you authorize it.${C_RESET}"
  exec sudo env RAMSLEUTH_SUDO_REEXEC=1 \
              RAMSLEUTH_MODE="$MODE" \
              RAMSLEUTH_SRC="$SRC" \
              RAMSLEUTH_VERSION="$VERSION" \
              RAMSLEUTH_SHA256="$SHA256" \
              RAMSLEUTH_GROUP_OPTIN="$GROUP_OPTIN" \
              RAMSLEUTH_STAGE_TMP="$STAGE_TMP" \
              "${SCRIPT_DIR}/install.sh"
}

# --- Build path: compile (unprivileged — no root-owned files in target/) --------
do_build() {
  header "Build — cargo --locked (the AUR's exact command)"
  step "cargo build --release --workspace --locked"
  info "  ${C_DIM}(the first build can take several minutes)${C_RESET}"
  cargo build --release --workspace --locked
  local b
  for b in "${BINARIES[@]}"; do
    [[ -f "$REPO_ROOT/target/release/$b" ]] || die "the build finished but target/release/$b is missing — incomplete build; re-run." 1
  done
  ok "Build complete → $REPO_ROOT/target/release/"
}

# --- Binary path: download + verify + extract (unprivileged) ---------------------
binary_path() {
  header "Fetch — precompiled release binary (no build required)"
  step "version: v$VERSION"
  step "arch:    $ARCH"
  ensure_zstd
  STAGE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/ramsleuth-install.XXXXXX")"
  trap 'rm -rf "$STAGE_TMP"' EXIT
  local name="ramsleuth-$VERSION-$ARCH.tar.zst"
  local url="https://github.com/$REPO/releases/download/v$VERSION/$name"
  step "downloading: $url"
  http_get "$url" "$STAGE_TMP/$name" || die "download failed — check your network connection and re-run." 1
  step "downloading the published checksum: $name.sha256"
  http_get "$url.sha256" "$STAGE_TMP/$name.sha256" || die "checksum download failed — re-run." 1
  verify_sha256 "$STAGE_TMP/$name" "$STAGE_TMP/$name.sha256"
  step "extracting → $STAGE_TMP"
  extract_zst "$STAGE_TMP/$name" "$STAGE_TMP"
  SRC="$STAGE_TMP/ramsleuth-$VERSION"
  [[ -d "$SRC" ]] || die "extraction produced no ramsleuth-$VERSION/ directory — unexpected tarball layout." 1
  validate_release_tree "$SRC"
  transparency_block "the official release tarball v$VERSION (sha256-verified)"
  group_optin_ask
  if ! confirm_proceed; then
    exit 0
  fi
  exec_sudo
}

# --- Source path: preflight + build, then install from the checkout --------------
source_path() {
  preflight_build
  transparency_block "this checkout (commit $COMMIT, branch $BRANCH)"
  group_optin_ask
  if ! confirm_proceed; then
    exit 0
  fi
  do_build
  SRC="$REPO_ROOT"
  STAGE_TMP=""
  exec_sudo
}

# --- Banner ----------------------------------------------------------------------
banner() {
  local line="────────────────────────────────────────────────────────────"
  printf '\n'
  printf '  %s%s%s\n' "${C_CYAN}" "$line" "${C_RESET}"
  printf '  %s  RamSleuth  v%s%s' "${C_SLAB}${C_CYAN}${C_BOLD}" "${VERSION:-?}" "${C_RESET}"
  printf '   %s—  RAM latency/bandwidth telemetry, self-contained installer%s\n' "${C_DIM}" "${C_RESET}"
  printf '  %s%s%s\n' "${C_CYAN}" "$line" "${C_RESET}"
  printf '\n'
}

# --- Root phase: layout resolution + install + activation ------------------------
resolve_layout() {
  [[ -d "$SRC" ]] || die "install source '$SRC' is missing — the staging dir did not survive the sudo re-exec. Re-run the installer." 1
  # Identical relative paths in both layouts:
  UNIT_SRC="$SRC/systemd/ramsleuth.service"
  AMD_HELPER_SRC="$SRC/scripts/install-ryzen-smu-dkms.sh"
  INTEL_HELPER_SRC="$SRC/scripts/install-intel-dkms.sh"
  RYZEN_CONF_SRC="$SRC/packaging/ryzen-smu-dkms/dkms.conf"
  VENDOR_DIR="$SRC/packaging/ryzen-smu-dkms/vendor"
  INTEL_KERNEL_DIR="$SRC/kernel/ramsleuth-intel"
  INSTALL_SH_SRC="$SRC/install.sh"
  LICENSE_SRC="$SRC/LICENSE"
  case "$MODE" in
    binary)
      # The release tarball layout (top-level dir ramsleuth-<ver>/).
      BIN_DIR="$SRC/bin"
      PRESET_SRC="$SRC/systemd/ramsleuth.preset"
      DESKTOP_SRC="$SRC/RamSleuth.desktop"
      SETUP_SRC="$SRC/ramsleuth-setup.sh"
      POLKIT_SRC="$SRC/90-ramsleuth-setup.policy"
      ICONS_ROOT="$SRC/icons"
      # Layout-drift guard: older/newer tarballs stage the hicolor tree as
      # assets/icons/ — accept either.
      [[ -d "$ICONS_ROOT" ]] || ICONS_ROOT="$SRC/assets/icons"
      ;;
    *)
      # The repo checkout layout.
      BIN_DIR="$SRC/target/release"
      PRESET_SRC="$SRC/packaging/shared/ramsleuth.preset"
      DESKTOP_SRC="$SRC/packaging/shared/RamSleuth.desktop"
      SETUP_SRC="$SRC/scripts/ramsleuth-setup.sh"
      POLKIT_SRC="$SRC/packaging/polkit/90-ramsleuth-setup.policy"
      ICONS_ROOT="$SRC/assets/icons"
      ;;
  esac
}

do_install() {
  header "Install — AUR-mirrored (install -D; idempotent)"
  local b
  for b in "${BINARIES[@]}"; do
    install -Dm755 "$BIN_DIR/$b" "/usr/bin/$b"
    ok "/usr/bin/$b"
  done
  install -Dm644 "$UNIT_SRC" "/usr/lib/systemd/system/ramsleuth.service"
  ok "/usr/lib/systemd/system/ramsleuth.service  (frozen unit, verbatim)"
  install -Dm644 "$PRESET_SRC" "/usr/lib/systemd/system-preset/ramsleuth.preset"
  ok "/usr/lib/systemd/system-preset/ramsleuth.preset"
  install -Dm644 "$DESKTOP_SRC" "/usr/share/applications/RamSleuth.desktop"
  ok "/usr/share/applications/RamSleuth.desktop  (app-menu entry; the filename matches the Wayland app_id, C21-45)"
  # The 8 hicolor icons (AUR-parity: the same sizes every AUR package installs,
  # C21-27) — installed to BOTH the bare <size>/apps/ and the <size>x<size>/
  # apps/ dirs: freedesktop index.theme files that declare only the NxN dirs
  # make KIconLoader skip the bare dirs, so a bare-only install shows a
  # missing-icon placeholder in the menu (C21-42). Every icon is ALSO
  # installed under the name RamSleuth.png (bare + NxN + pixmaps): on Wayland
  # KWin matches the window to a desktop file by app_id, and its direct
  # icon-name fallback uses the app_id verbatim as the icon name (C21-45).
  local size icon
  for size in 16 24 32 48 64 128 256 512; do
    icon="$ICONS_ROOT/hicolor/$size/apps/ramsleuth.png"
    [[ -f "$icon" ]] || die "Missing $icon — incomplete install source; the hicolor icons cannot be installed." 1
    install -Dm644 "$icon" "/usr/share/icons/hicolor/$size/apps/ramsleuth.png"
    install -Dm644 "$icon" "/usr/share/icons/hicolor/${size}x${size}/apps/ramsleuth.png"
    install -Dm644 "$icon" "/usr/share/icons/hicolor/$size/apps/RamSleuth.png"
    install -Dm644 "$icon" "/usr/share/icons/hicolor/${size}x${size}/apps/RamSleuth.png"
    ok "/usr/share/icons/hicolor/$size/{,${size}x${size}/}apps/{ramsleuth,RamSleuth}.png"
  done
  # The 48px legacy pixmaps fallback (the X11 legacy icon lookup path;
  # C21-42) — the source file was die-guarded by the loop above (48) — plus
  # the RamSleuth.png app_id-named counterpart (C21-45).
  install -Dm644 "$ICONS_ROOT/hicolor/48/apps/ramsleuth.png" "/usr/share/pixmaps/ramsleuth.png"
  ok "/usr/share/pixmaps/ramsleuth.png"
  install -Dm644 "$ICONS_ROOT/hicolor/48/apps/ramsleuth.png" "/usr/share/pixmaps/RamSleuth.png"
  ok "/usr/share/pixmaps/RamSleuth.png"
  # The system group (idempotent getent guard).
  if getent group ramsleuth >/dev/null 2>&1; then
    ok "group 'ramsleuth' already present"
  else
    groupadd -r ramsleuth
    ok "group 'ramsleuth' created (system group)"
  fi
  # The shared AMD pinned helper.
  install -Dm755 "$AMD_HELPER_SRC" "/usr/bin/ramsleuth-install-ryzen-smu-dkms"
  ok "/usr/bin/ramsleuth-install-ryzen-smu-dkms  (the shared pinned helper)"
  # The repo's ryzen_smu DKMS config (DEST=/extra — matches the helper's
  # depmod override; the helper resolves it at /usr/share/ryzen-smu-dkms/
  # on an installed run). GUARDED: absent in a pre-dkms.conf source ->
  # clean skip, never a hard fail.
  if [[ -f "$RYZEN_CONF_SRC" ]]; then
    install -Dm644 "$RYZEN_CONF_SRC" "/usr/share/ryzen-smu-dkms/dkms.conf"
    ok "/usr/share/ryzen-smu-dkms/dkms.conf  (the repo's DKMS config — DEST=/extra)"
  else
    warn "/usr/share/ryzen-smu-dkms/dkms.conf skipped — not present in this source; the helper falls back to the vendored dkms.conf"
  fi
  # The vendored ryzen_smu source tree (the 6 module files frozen at upstream
  # d298366 + SUMS.sha256 + NOTICE.md) → /usr/share/ryzen-smu-dkms/vendor/ —
  # the exact path the AMD helper resolves as its OFFLINE vendored source
  # (C21-08/09: SUMS-verified, zero network). GUARDED like the AUR does.
  if [[ -d "$VENDOR_DIR/ryzen-smu" ]]; then
    local f
    for f in LICENSE Makefile dkms.conf drv.c smu.c smu.h; do
      if [[ -f "$VENDOR_DIR/ryzen-smu/$f" ]]; then
        install -Dm644 "$VENDOR_DIR/ryzen-smu/$f" "/usr/share/ryzen-smu-dkms/vendor/ryzen-smu/$f"
      fi
    done
    for f in SUMS.sha256 NOTICE.md; do
      if [[ -f "$VENDOR_DIR/$f" ]]; then
        install -Dm644 "$VENDOR_DIR/$f" "/usr/share/ryzen-smu-dkms/vendor/$f"
      fi
    done
    ok "/usr/share/ryzen-smu-dkms/vendor/  (the vendored ryzen_smu source — the helper's offline copy)"
  else
    warn "skipped /usr/share/ryzen-smu-dkms/vendor/ — no vendored ryzen_smu tree in this source; the AMD helper falls back to its git fetch"
  fi
  # The Intel DKMS helper (INTEL-11; INTEL-06 source scripts/install-intel-dkms.sh).
  # GUARDED — the same no-panic contract the AUR mirrors (INTEL-09/10): absent
  # in a pre-Intel source -> clean skip with a note, never a hard fail.
  if [[ -f "$INTEL_HELPER_SRC" ]]; then
    install -Dm755 "$INTEL_HELPER_SRC" "/usr/bin/ramsleuth-install-intel-dkms"
    ok "/usr/bin/ramsleuth-install-intel-dkms  (the shared Intel DKMS helper)"
  else
    warn "/usr/bin/ramsleuth-install-intel-dkms skipped — not present in this source (pre-Intel); live Intel IMC subtimings unavailable"
  fi
  # The in-repo ramsleuth_intel DKMS module source tree →
  # /usr/share/ramsleuth-intel-dkms/src/ — the exact path the Intel helper
  # resolves as its installed copy (no network, no upstream pin). GUARDED.
  if [[ -d "$INTEL_KERNEL_DIR" ]]; then
    local f
    for f in dkms.conf Makefile ramsleuth_intel.c README.md; do
      if [[ -f "$INTEL_KERNEL_DIR/$f" ]]; then
        install -Dm644 "$INTEL_KERNEL_DIR/$f" "/usr/share/ramsleuth-intel-dkms/src/$f"
      fi
    done
    ok "/usr/share/ramsleuth-intel-dkms/src/  (the in-repo Intel module source)"
  else
    warn "skipped /usr/share/ramsleuth-intel-dkms/src/ — no kernel/ramsleuth-intel tree in this source (pre-Intel)"
  fi
  # The one-click setup pair (AUR-parity: the same two artifacts every AUR package installs).
  [[ -f "$SETUP_SRC" ]] || die "Missing the ramsleuth-setup script in the install source — incomplete; the one-click setup helper cannot be installed." 1
  [[ -f "$POLKIT_SRC" ]] || die "Missing the polkit policy in the install source — incomplete; the polkit policy cannot be installed." 1
  install -Dm755 "$SETUP_SRC" "/usr/bin/ramsleuth-setup"
  ok "/usr/bin/ramsleuth-setup  (the one-click privileged setup helper)"
  install -Dm644 "$POLKIT_SRC" "/usr/share/polkit-1/actions/90-ramsleuth-setup.policy"
  ok "/usr/share/polkit-1/actions/90-ramsleuth-setup.policy  (its polkit policy)"
  # The MIT license (the AUR license-compliance install path).
  if [[ -f "$LICENSE_SRC" ]]; then
    install -Dm644 "$LICENSE_SRC" "/usr/share/licenses/ramsleuth/LICENSE"
    ok "/usr/share/licenses/ramsleuth/LICENSE"
  fi
  # This installer (re-runnable + auditable post-install).
  install -Dm755 "$INSTALL_SH_SRC" "/usr/share/ramsleuth/install.sh"
  ok "/usr/share/ramsleuth/install.sh  (this installer)"
}

# do_activate — the privileged activation: ramsleuth-setup does group
# membership + service + socket ACL + the smart polkit restart in one
# self-healing run; the minimal path (declined membership / true root)
# does group + service + a guarded polkit restart only.
do_activate() {
  header "Activate — service + user access"
  local user="${SUDO_USER:-}"
  if [[ -n "$user" && "$user" != "root" && "$GROUP_OPTIN" == "1" ]]; then
    step "running ramsleuth-setup --user $user  (group, service, socket ACL, polkit)"
    if /usr/bin/ramsleuth-setup --user "$user"; then
      ok "ramsleuth-setup: $user authorized (current session via the socket ACL + future logins via the group)"
    else
      warn "ramsleuth-setup exited non-zero — the files are installed; re-run it manually: sudo ramsleuth-setup"
    fi
    return 0
  fi
  if [[ -n "$user" && "$user" != "root" ]]; then
    info "  ${C_DIM}Group membership declined — the daemon starts, but $user has no unprivileged access.${C_RESET}"
    info "  ${C_DIM}(grant it later with: sudo ramsleuth-setup --user $user)${C_RESET}"
  fi
  # Minimal activation (true root, or declined membership): the daemon is
  # the point — start it; everything else is best-effort (the no-panic
  # install contract).
  systemctl daemon-reload 2>/dev/null || true
  if systemctl enable --now ramsleuth.service 2>/dev/null; then
    ok "systemd: daemon-reload + ramsleuth.service enabled & started"
  else
    warn "systemd: could not start ramsleuth.service (non-fatal) — try: systemctl enable --now ramsleuth"
  fi
  # polkit: make a running polkitd serve the policy installed above. Its
  # inotify hot-reload is not reliable for a file installed while it runs
  # (observed 2026-09-25) — without this restart the GUI's one-click prompt
  # shows the generic polkit dialog instead of RamSleuth's branded message.
  # Guarded, non-fatal.
  if systemctl restart polkit 2>/dev/null; then
    ok "polkit restarted (the branded 'Set up RamSleuth' prompt is active)"
  else
    warn "polkit: restart failed (non-fatal) — if the GUI's setup prompt shows the generic polkit dialog, run: systemctl restart polkit"
  fi
}

amd_walk() {
  local helper
  if [[ "$CPU_VENDOR" == "AuthenticAMD" ]]; then
    header "AMD — optional ryzen_smu kernel module"
    printf '  ryzen_smu enables LIVE AMD subtimings. It is OPTIONAL: without it the app\n'
    printf '  runs fine — the AMD section reads N/A (DriverMissing), exit 0, no panic.\n'
    printf '  Installing it now runs the shared pinned helper, which uses the OFFLINE\n'
    printf '  vendored source (installed above) when present, else fetches\n'
    printf '  %s@ %s, shows + checksums + confirms it, then DKMS-builds it.\n\n' "$RYZEN_SMU_REPO" "$RYZEN_SMU_PIN_SHORT"
    if ask "Install the ryzen_smu DKMS module now? [y/N] " "N"; then
      helper="/usr/bin/ramsleuth-install-ryzen-smu-dkms"
      [[ -x "$helper" ]] || helper="$SRC/scripts/install-ryzen-smu-dkms.sh"
      step "Handing off to: $helper"
      exec "$helper"
    fi
    DKMS_STATE="skipped"
    printf '  Skipped. Run it later with:    %ssudo ramsleuth-install-ryzen-smu-dkms%s\n' "${C_CYAN}" "${C_RESET}"
    printf '  (or the AUR extra:            %syay -S ryzen-smu-dkms%s)\n' "${C_DIM}" "${C_RESET}"
  elif [[ "$CPU_VENDOR" == "GenuineIntel" ]]; then
    header "Intel — optional ramsleuth_intel kernel module"
    printf '  ramsleuth_intel enables LIVE Intel IMC subtimings. It is OPTIONAL: without it\n'
    printf '  the app runs fine — the Intel section reads N/A (DriverMissing) and the\n'
    printf '  built-in MCHBAR /dev/mem fallback is used where available (exit 0, no panic).\n'
    printf '  Installing it now runs the shared helper, which DKMS-builds the in-repo\n'
    printf '  module (RamSleuth'"'"'s own source — no fetch, no pin, no third-party code).\n\n'
    if ask "Install the ramsleuth_intel DKMS module now? (live Intel IMC subtimings) [y/N]" "N"; then
      helper="/usr/bin/ramsleuth-install-intel-dkms"
      [[ -x "$helper" ]] || helper="$SRC/scripts/install-intel-dkms.sh"
      step "Handing off to: $helper"
      exec "$helper"
    fi
    DKMS_STATE="skipped"
    printf '  Skipped. Run it later with:    %ssudo ramsleuth-install-intel-dkms%s\n' "${C_CYAN}" "${C_RESET}"
  else
    header "CPU — $CPU_VENDOR"
    info "  No vendor-specific DKMS module is offered for this CPU."
    DKMS_STATE="intel"
  fi
}

do_summary() {
  header "Summary"
  if [[ "$MODE" == "binary" ]]; then
    printf '  %s✓ Installed RamSleuth v%s%s  (precompiled release, sha256 %s)\n' "${C_GREEN}" "$VERSION" "${C_RESET}" "${SHA256:0:12}…"
  else
    printf '  %s✓ Installed RamSleuth v%s%s  (built from commit %s, branch %s)\n' "${C_GREEN}" "$VERSION" "${C_RESET}" "$COMMIT" "$BRANCH"
  fi
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
    if [[ "$CPU_VENDOR" == "GenuineIntel" ]]; then
      printf '\n  %sIntel next step (optional):%s the ramsleuth_intel DKMS module (in-repo, the\n' "${C_AMBER}" "${C_RESET}"
      printf '  project'"'"'s own source) gives live Intel IMC subtimings. Without it the Intel\n'
      printf '  section reads N/A (DriverMissing) and the app uses the /dev/mem fallback where\n'
      printf '  available (exit 0, no panic). Enable it with:\n'
      printf '     %ssudo ramsleuth-install-intel-dkms%s  (or one-click: sudo ramsleuth-setup --with-dkms)\n' "${C_CYAN}" "${C_RESET}"
    else
      printf '\n  %sDegradation note:%s without the ryzen_smu module the AMD section reads\n' "${C_AMBER}" "${C_RESET}"
      printf '  N/A (DriverMissing), exit 0, no panic. Enable live AMD subtimings with:\n'
      printf '     %ssudo ramsleuth-setup --with-dkms%s  (or the helper: sudo ramsleuth-install-ryzen-smu-dkms)\n' "${C_CYAN}" "${C_RESET}"
    fi
  fi
  printf '\n  %sTransparency footer:%s ' "${C_DIM}" "${C_RESET}"
  if [[ "$MODE" == "binary" ]]; then
    printf 'every installed file comes from the official release tarball v%s\n' "$VERSION"
    printf '  (sha256-verified against the release'"'"'s published .sha256 before install). The only\n'
  else
    printf 'every installed file is byte-identical to a file in this\n'
    printf '  repo (audit with: git show). The build was --locked from commit %s. The only\n' "$COMMIT"
  fi
  printf '  third-party source is the optional AMD module %s%s @ %s%s — pinned, shown,\n' "${C_AMBER}" "$RYZEN_SMU_REPO" "$RYZEN_SMU_PIN_SHORT" "${C_RESET}"
  printf '  checksummed, and confirmed by the helper before any build.\n'
  printf '\n  %s✓ Done — RamSleuth is installed and the daemon is running.%s\n' "${C_BOLD}${C_GREEN}" "${C_RESET}"
}

# --- Phases ----------------------------------------------------------------------
user_phase() {
  if [[ "$MODE" == "build" ]]; then
    # The build path installs what this checkout is.
    VERSION="$(awk -F'"' '$1 ~ /^version/{print $2; exit}' "$REPO_ROOT/Cargo.toml" 2>/dev/null)" || VERSION=""
    [[ -n "$VERSION" ]] || VERSION="dev"
  else
    resolve_version
  fi
  banner
  case "$ARCH" in
    x86_64|aarch64) ;;
    *) die "unsupported architecture '$ARCH' — RamSleuth ships x86_64 release binaries and supports x86_64 / aarch64 source builds." 2 ;;
  esac
  if [[ "$MODE" == "build" ]]; then
    source_path
  elif asset_available "$VERSION"; then
    binary_path
  else
    warn "No prebuilt binary available for $ARCH in v$VERSION yet (the releases are x86_64-only today)."
    info "  ${C_DIM}Falling back to the source build — a Rust toolchain is required for this path.${C_RESET}"
    source_path
  fi
}

root_phase() {
  MODE="${RAMSLEUTH_MODE:-build}"
  VERSION="${RAMSLEUTH_VERSION:-$VERSION}"
  SHA256="${RAMSLEUTH_SHA256:-}"
  GROUP_OPTIN="${RAMSLEUTH_GROUP_OPTIN:-1}"
  STAGE_TMP="${RAMSLEUTH_STAGE_TMP:-}"
  SRC="${RAMSLEUTH_SRC:-$REPO_ROOT}"
  # The staging dir (binary path) is cleaned when this root process exits.
  [[ -n "$STAGE_TMP" ]] && trap 'rm -rf "$STAGE_TMP"' EXIT
  banner
  resolve_layout
  do_install
  do_activate
  amd_walk
  do_summary
}

main() {
  # After the sudo re-exec we are root + marked → the user already confirmed.
  if [[ "$(id -u)" -eq 0 && "${RAMSLEUTH_SUDO_REEXEC:-0}" == "1" ]]; then
    root_phase
  else
    user_phase
  fi
}

# Run only when executed (not sourced) — keeps the functions testable.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
