#!/usr/bin/env bash
#
# install-ryzen-smu-dkms.sh — idempotent operator helper (HANDOVER §7).
#
# Installs the ryzen_smu AMD kernel module via DKMS so ramsleuth can serve
# live AMD subtimings. OPTIONAL recommended extra: without the module the app
# degrades to N/A (DriverMissing) (exit 0, no panic); the frozen systemd unit
# never loads the module itself.
#
# Usage: scripts/install-ryzen-smu-dkms.sh (re-execs under sudo if not root —
# the operator's sudo prompt appears there). Safe to re-run: every step is
# guarded and any failure prints a clear message + exits non-zero, never
# leaving DKMS in a silent half-state. Never touches ramsleuth state.

set -euo pipefail

# --- Constants --------------------------------------------------------------
KERNEL="$(uname -r)"
MODULE="ryzen_smu"
# Upstream default (VERIFIED): amkillam/ryzen_smu — default branch `main`,
# v0.1.7, actively maintained; builds module `ryzen_smu`, exposes
# /sys/kernel/ryzen_smu_drv/pm_table, ships its own dkms.conf + monitor_cpu
# CLI (Zen3+, kernel 7.2+). The former 53XU/ryzen_smu default is DEAD
# (HTTP 404 -> credential prompt). Still overridable via RYZEN_SMU_URL, and
# verify the upstream at setup time (HANDOVER §7 step 2) — do NOT trust a
# cached URL if it moves.
UPSTREAM_URL="${RYZEN_SMU_URL:-https://github.com/amkillam/ryzen_smu.git}"
# Verified sysfs kobject (amkillam drv.c; matches the daemon, P5-08):
# canonical ryzen_smu_drv path first, legacy ryzen_smu path as a secondary
# existence check only.
PM_TABLE="/sys/kernel/ryzen_smu_drv/pm_table"
PM_TABLE_LEGACY="/sys/kernel/${MODULE}/pm_table"
SRC_DIR="/opt/ryzen-smu-src"
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"    # this script sits in <repo>/scripts

# --- Helpers -----------------------------------------------------------------
log() { printf '[ryzen-smu-dkms] %s\n' "$*"; }
die() { printf '[ryzen-smu-dkms] ERROR: %s\n' "$*" >&2; exit 1; }
# Fallback dkms.conf (P5-03): first existing of the repo-relative path (run
# from the repo) or the installed /usr/share path (the P5-05 package installs
# this helper to /usr/bin, where REPO_ROOT resolves to /usr and the repo
# path does not exist). Prints the chosen path; fails if neither exists.
resolve_dkms_conf() {
  local c
  for c in "${REPO_ROOT}/packaging/ryzen-smu-dkms/dkms.conf" \
           "/usr/share/ryzen-smu-dkms/dkms.conf"; do
    if [[ -f "${c}" ]]; then printf '%s\n' "${c}"; return 0; fi
  done
  return 1
}

# --- Idempotent fast path ------------------------------------------------------
# Already loaded (re-run, or AUTOINSTALL=yes rebuilt after a kernel update).
# Canonical ryzen_smu_drv path first, legacy ryzen_smu path as fallback.
if [[ -e "${PM_TABLE}" || -e "${PM_TABLE_LEGACY}" ]]; then
  log "${MODULE} already loaded (pm_table present) — nothing to do."
  exit 0
fi

# --- Privilege: re-exec under sudo if not root (operator prompt appears here) ---
if [[ "$(id -u)" -ne 0 ]]; then
  log "Re-running under sudo..."
  exec sudo "$0" "$@"
fi

# --- Step 1: prereqs + kernel build tree ---------------------------------------
log "Installing build tooling: dkms + base-devel (pacman, idempotent)..."
pacman -S --needed dkms base-devel
# DKMS needs a kernel build tree. We never guess a custom-kernel headers
# package (HANDOVER §7 step 1) — list candidates and stop with instructions.
if [[ ! -d "/lib/modules/${KERNEL}/build" ]]; then
  log "Kernel build tree MISSING for ${KERNEL} — candidate headers packages:"
  CANDIDATES="$(pacman -Qs headers 2>/dev/null | grep -iE 'cachyos|custom|linux-headers' || true)"
  [[ -n "${CANDIDATES}" ]] && printf '%s\n' "${CANDIDATES}"
  die "cannot determine the ${KERNEL} headers package automatically. Install it
manually (e.g. the matching 'linux-headers' / cachyos-custom package),
then re-run this script."
fi
log "Kernel build tree OK: /lib/modules/${KERNEL}/build"

# --- Step 2: clone the ryzen_smu source (verify URL at setup time) --------------
log "Upstream: ${UPSTREAM_URL}"
if [[ -d "${SRC_DIR}/.git" ]]; then
  # Existing tree: update it, tolerating a failed pull (e.g. offline host).
  git -C "${SRC_DIR}" pull --ff-only || log "git pull --ff-only failed — continuing with existing tree"
else
  # Fresh shallow clone; wipe a stale non-git dir of unknown provenance.
  rm -rf "${SRC_DIR}"
  git clone --depth 1 "${UPSTREAM_URL}" "${SRC_DIR}" \
    || die "git clone of ${UPSTREAM_URL} failed — verify the URL (and network) and retry"
fi

# --- Step 3: dkms.conf fallback (P5-03; repo-relative or installed /usr/share) --
if [[ ! -f "${SRC_DIR}/dkms.conf" ]]; then
  DKMS_CONF="$(resolve_dkms_conf)" \
    || die "No fallback dkms.conf found (looked in <repo>/packaging/ryzen-smu-dkms/ and /usr/share/ryzen-smu-dkms/)."
  log "Source lacks dkms.conf — installing fallback from ${DKMS_CONF}..."
  install -m 644 "${DKMS_CONF}" "${SRC_DIR}/dkms.conf"
fi

# --- Step 4: dkms add + build + install -----------------------------------------
dkms add "${MODULE}" || true   # "already present" is fine (idempotent)
dkms install "${MODULE}" -k "${KERNEL}" \
  || die "dkms install failed — run 'dmesg | tail' / 'dkms status' to inspect build errors"

# --- Step 5: load now + at boot ---------------------------------------------------
modprobe "${MODULE}" || die "modprobe ${MODULE} failed — run 'dmesg | tail' to inspect load errors"
printf '%s\n' "${MODULE}" > "/etc/modules-load.d/${MODULE}.conf"   # idempotent overwrite

# --- Step 6: verify ------------------------------------------------------------------
# Matches the daemon (P5-08): canonical ryzen_smu_drv path first, legacy
# ryzen_smu path second.
if [[ -e "${PM_TABLE}" ]]; then
  log "SUCCESS: ${MODULE} loaded; ${PM_TABLE} present."
elif [[ -e "${PM_TABLE_LEGACY}" ]]; then
  log "SUCCESS: ${MODULE} loaded; legacy ${PM_TABLE_LEGACY} present (daemon prefers ${PM_TABLE})."
else
  die "pm_table missing after load (looked for ${PM_TABLE}, then ${PM_TABLE_LEGACY}) — run 'dmesg | tail' to inspect. Without the
module the app still works: N/A (DriverMissing), exit 0, no panic."
fi
