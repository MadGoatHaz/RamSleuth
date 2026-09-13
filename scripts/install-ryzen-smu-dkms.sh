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
# Upstream default from the code's uAPI hint (53XU/ryzen_smu). VERIFY the
# correct upstream at setup time — do NOT trust a cached URL (HANDOVER §7
# step 2); override with RYZEN_SMU_URL if it moves.
UPSTREAM_URL="${RYZEN_SMU_URL:-https://github.com/53XU/ryzen_smu.git}"
SRC_DIR="/opt/ryzen-smu-src"
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"    # this script sits in <repo>/scripts
DKMS_CONF_SRC="${REPO_ROOT}/packaging/ryzen-smu-dkms/dkms.conf"       # repo-provided fallback (P5-03)

# --- Helpers -----------------------------------------------------------------
log() { printf '[ryzen-smu-dkms] %s\n' "$*"; }
die() { printf '[ryzen-smu-dkms] ERROR: %s\n' "$*" >&2; exit 1; }

# --- Idempotent fast path ------------------------------------------------------
# Already loaded (re-run, or AUTOINSTALL=yes rebuilt after a kernel update).
if [[ -e "/sys/kernel/${MODULE}/pm_table" ]]; then
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

# --- Step 3: dkms.conf fallback (repo-provided, P5-03) --------------------------
if [[ ! -f "${SRC_DIR}/dkms.conf" ]]; then
  [[ -f "${DKMS_CONF_SRC}" ]] || die "source lacks dkms.conf and repo fallback ${DKMS_CONF_SRC} is missing"
  log "Source lacks dkms.conf — installing repo-provided fallback..."
  install -m 644 "${DKMS_CONF_SRC}" "${SRC_DIR}/dkms.conf"
fi

# --- Step 4: dkms add + build + install -----------------------------------------
dkms add "${MODULE}" || true   # "already present" is fine (idempotent)
dkms install "${MODULE}" -k "${KERNEL}" \
  || die "dkms install failed — run 'dmesg | tail' / 'dkms status' to inspect build errors"

# --- Step 5: load now + at boot ---------------------------------------------------
modprobe "${MODULE}" || die "modprobe ${MODULE} failed — run 'dmesg | tail' to inspect load errors"
printf '%s\n' "${MODULE}" > "/etc/modules-load.d/${MODULE}.conf"   # idempotent overwrite

# --- Step 6: verify ------------------------------------------------------------------
if [[ -e "/sys/kernel/${MODULE}/pm_table" ]]; then
  log "SUCCESS: ${MODULE} loaded; /sys/kernel/${MODULE}/pm_table present."
  log "Start the daemon for live subtimings: systemctl start ramsleuth (or sudo ramsleuth-daemon)"
else
  die "pm_table missing after load — run 'dmesg | tail' to inspect. Without the
module the app still works: N/A (DriverMissing), exit 0, no panic."
fi
