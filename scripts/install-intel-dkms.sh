#!/usr/bin/env bash
#
# install-intel-dkms.sh — idempotent operator helper (INTEL-06, HANDOVER §7).
#
# Installs the ramsleuth_intel Intel kernel module via DKMS so ramsleuth can
# serve live Intel IMC subtimings. OPTIONAL recommended extra: without the
# module the app degrades to N/A (DriverMissing) (exit 0, no panic); the
# frozen systemd unit never loads the module itself.
#
# Usage: scripts/install-intel-dkms.sh [--dry-run] (re-execs under sudo if not
# root — the operator's sudo prompt appears there). Safe to re-run: every step
# is guarded; any failure prints a clear message + exits non-zero, never
# leaving DKMS in a silent half-state. Never touches ramsleuth state.
#
# VENDOR-AWARE / NON-INTEL-SAFE: the module is Intel-only by design — it
# probes the host bridge at PCI 0000:00:00.0 and rejects a non-Intel vendor
# with a clean -ENODEV before any kobject is created. On a non-Intel host
# (e.g. an AMD dev box) the module BUILDS fine; `modprobe` then exits
# non-zero and no kobject appears. This helper detects that via the
# host-bridge PCI vendor (0x8086 = Intel) and exits 0 with a clear note
# ("built OK; no Intel host bridge detected — module idle, /dev/mem fallback
# will be used") instead of a confusing non-zero. It never persists an
# at-boot load entry on the non-Intel path (and removes a stale one).
#
# In-repo source (simplified vs the AMD ryzen-smu helper — no pin/clone/vendor
# SUMS logic, since the module is vendored in this repo):
#   1. Repo-relative kernel/ramsleuth-intel/ (resolved via readlink -f of this
#      script, AMD pattern — the script's real path canonicalizes a symlinked
#      invocation into the repo it points into).
#   2. Installed copy /usr/share/ramsleuth-intel-dkms/src/ (the ramsleuth-intel
#      -dkms AUR extra, INTEL-07).
#   Version = the source tree's Cargo.toml [workspace.package] version (a fixed
#   version -> idempotent `dkms add`); documented fallback 2.2.1 when the
#   installed copy carries no workspace Cargo.toml.
#
# --dry-run: performs ONLY the read-only prep (source resolution, version,
# kernel-build-tree check, host-bridge vendor detection) and previews the
# privileged commands; it stops BEFORE any privileged op (no sudo, no dkms, no
# modprobe, no /usr/src, /usr/lib/depmod.d, or /etc/modules-load.d writes) and
# exits 0. A non-Intel host is predicted from the vendor at dry-run time.

set -euo pipefail

# --- Constants ---------------------------------------------------------------
KERNEL="$(uname -r)"
MODULE="ramsleuth_intel"            # dkms package name + built module (underscore)
DEFAULT_VERSION="2.2.1"            # ramsleuth workspace version (fallback only)
EXPECTED_ATTRS=19                  # frozen sysfs interface (README "Frozen sysfs interface")
KOBJ="/sys/kernel/${MODULE}"        # kobject path (never created on a non-Intel host)
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"           # <repo> (or /usr when installed)
INSTALLED_SRC="/usr/share/ramsleuth-intel-dkms/src"
STAGE_FILES=(dkms.conf Makefile ramsleuth_intel.c README.md)

# --- CLI ---------------------------------------------------------------------
DRY_RUN=0
usage() {
  cat <<'USAGE'
Usage: install-intel-dkms.sh [--dry-run]

Idempotent operator helper: build + install the optional ramsleuth_intel
Intel DKMS module (kernel/ramsleuth-intel) so ramsleuth can serve live
Intel IMC subtimings. Re-execs under sudo if not root.

  --dry-run   Read-only preview: resolves the source, version, kernel build
              tree, and host-bridge vendor, then prints the privileged
              commands that would run. Performs NO privileged operation
              (no sudo/dkms/modprobe; no system writes) and exits 0.

On a non-Intel host the module builds fine but the load leaves it idle (no
kobject); the helper exits 0 with a clear note and the app uses the /dev/mem
fallback.
USAGE
}
for arg in "$@"; do
  case "${arg}" in
    -h|--help) usage; exit 0 ;;
    --dry-run) DRY_RUN=1 ;;
    *) printf '[intel-dkms] ERROR: unknown argument: %s (see --help)\n' "${arg}" >&2; exit 1 ;;
  esac
done

# --- Helpers -----------------------------------------------------------------
log() { printf '[intel-dkms] %s\n' "$*"; }
die() { printf '[intel-dkms] ERROR: %s\n' "$*" >&2; exit 1; }

# Source resolution (AMD pattern): first EXISTING of the repo-relative in-repo
# tree (dev checkout — readlink -f canonicalizes a symlinked invocation) or the
# installed /usr/share copy (the ramsleuth-intel-dkms package, INTEL-07).
# Prints the chosen dir to stdout; fails if neither exists.
resolve_src() {
  local real dir c
  real="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || printf '%s' "${BASH_SOURCE[0]}")"
  dir="$(cd -- "$(dirname -- "${real}")/.." 2>/dev/null && pwd || true)"
  for c in "${dir}/kernel/ramsleuth-intel" "${INSTALLED_SRC}"; do
    if [[ -d "${c}" ]]; then printf '%s\n' "${c}"; return 0; fi
  done
  return 1
}

# Version = the source tree's Cargo.toml [workspace.package] version. For the
# repo layout SRC_DIR is <root>/kernel/ramsleuth-intel, so the workspace
# Cargo.toml is two levels up; for the installed copy there is none, so fall
# back to the documented default. Prints ONLY the version to stdout (the
# fallback note goes to stderr so it never pollutes the captured value).
resolve_version() {
  local root cargo v
  root="$(cd -- "$(dirname -- "$(dirname -- "${SRC_DIR}")")" 2>/dev/null && pwd || true)"
  cargo="${root}/Cargo.toml"
  if [[ -f "${cargo}" ]]; then
    v="$(awk '
      /^[[:space:]]*\[workspace\.package\]/ { ws = 1; next }
      ws && /^[[:space:]]*\[/              { ws = 0 }
      ws && /^[[:space:]]*version[[:space:]]*=/ {
        sub(/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/, "")
        sub(/".*$/, "")
        print; exit
      }
    ' "${cargo}")"
    if [[ -n "${v}" ]]; then printf '%s\n' "${v}"; return 0; fi
  fi
  printf '[intel-dkms] no workspace Cargo.toml near %s — using the documented fallback version %s\n' \
    "${SRC_DIR}" "${DEFAULT_VERSION}" >&2
  printf '%s\n' "${DEFAULT_VERSION}"
}

# Host-bridge PCI vendor id at 0000:00:00.0 (the slot the module probes).
# The sysfs file may read "8086" or "0x8086" depending on the kernel; both are
# normalized to bare hex by normalize_vendor. Prints the raw value; fails if
# the slot is absent.
detect_vendor() {
  local f="/sys/bus/pci/devices/0000:00:00.0/vendor"
  [[ -f "${f}" ]] || return 1
  cat "${f}"
}
# Strip a leading 0x/0X so a bare-hex comparison against 8086 is safe.
normalize_vendor() {
  local v="${1:-}"
  v="${v#0x}"
  v="${v#0X}"
  printf '%s\n' "${v}"
}

# Kernel build tree present for the running kernel (DKMS needs it).
kernel_tree_present() { [[ -d "/lib/modules/${KERNEL}/build" ]]; }

# kobject ready: the dir exists and mchbar_enabled reads "1" (that attribute
# is a constant 1 by construction while the kobject exists, so it is the
# canonical loaded signal). Reports the observed attribute count.
verify_kobject() {
  [[ -d "${KOBJ}" ]] || return 1
  local en n
  en="$(cat "${KOBJ}/mchbar_enabled" 2>/dev/null || true)"
  [[ "${en}" == "1" ]] || return 1
  n="$(ls -1 "${KOBJ}" 2>/dev/null | wc -l | tr -d '[:space:]')"
  log "kobject ready: ${KOBJ} (${n} attributes; expected ${EXPECTED_ATTRS})"
  return 0
}

# --- Idempotent fast path ------------------------------------------------------
# Already loaded (re-run, or AUTOINSTALL=yes rebuilt after a kernel update).
# On a non-Intel host the kobject never exists, so this never triggers there.
if [[ -e "${KOBJ}/mchbar_enabled" ]]; then
  log "${MODULE} already loaded — continuing to rebuild from current source so the loaded module matches."
fi

# --- Dry-run: read-only preview, stops BEFORE any privileged op ----------------
if [[ "${DRY_RUN}" -eq 1 ]]; then
  log "DRY-RUN: no privileged operations will be performed (no sudo/dkms/modprobe; no writes to /usr/src, /usr/lib/depmod.d, or /etc/modules-load.d)"
  SRC_DIR="$(resolve_src)" \
    || die "no Intel module source found (expected ${REPO_ROOT}/kernel/ramsleuth-intel or ${INSTALLED_SRC})"
  VERSION="$(resolve_version)"
  log "DRY-RUN: source  = ${SRC_DIR}"
  log "DRY-RUN: version = ${VERSION}  (would stage to /usr/src/${MODULE}-${VERSION}; dkms.conf @VERSION@ -> ${VERSION})"
  if kernel_tree_present; then
    log "DRY-RUN: kernel build tree OK: /lib/modules/${KERNEL}/build"
  else
    log "DRY-RUN: WARNING: kernel build tree MISSING for ${KERNEL} — the real build would fail; install the matching headers package first"
  fi
  VENDOR="$(detect_vendor || true)"
  VENDOR_NORM="$(normalize_vendor "${VENDOR}")"
  if [[ -n "${VENDOR_NORM}" && "${VENDOR_NORM}" == "8086" ]]; then
    log "DRY-RUN: host bridge 0000:00:00.0 is Intel (vendor 0x8086) — the real run would build + load the module"
  elif [[ -n "${VENDOR_NORM}" ]]; then
    log "DRY-RUN: host bridge 0000:00:00.0 is NON-Intel (vendor 0x${VENDOR_NORM}) — the real run would build OK, modprobe would leave the module idle (no kobject), and this helper would exit 0 with the /dev/mem-fallback note"
  else
    log "DRY-RUN: host bridge vendor at 0000:00:00.0 unreadable — the real run would classify the modprobe outcome at load time"
  fi
  log "DRY-RUN: the real run would execute:"
  log "  dkms remove ${MODULE}/${VERSION} -k ${KERNEL} --no-depmod   (safe no-op on a first run)"
  log "  dkms add ${MODULE}/${VERSION}"
  log "  dkms build ${MODULE}/${VERSION} -k ${KERNEL}"
  log "  dkms install ${MODULE}/${VERSION} -k ${KERNEL}"
  log "  rmmod ${MODULE}   (safe no-op if the module is not loaded)"
  log "  modprobe ${MODULE}"
  log "  echo ${MODULE} > /etc/modules-load.d/${MODULE}.conf   (only on a successful Intel load)"
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
# DKMS needs a kernel build tree; we never guess a custom-kernel headers
# package (HANDOVER §7 step 1) — list candidates and stop with instructions.
if ! kernel_tree_present; then
  log "Kernel build tree MISSING for ${KERNEL} — candidate headers packages:"
  CANDIDATES="$(pacman -Qs headers 2>/dev/null | grep -iE 'cachyos|custom|linux-headers' || true)"
  [[ -n "${CANDIDATES}" ]] && printf '%s\n' "${CANDIDATES}"
  die "cannot determine the ${KERNEL} headers package automatically. Install it
  manually (e.g. the matching 'linux-headers' / cachyos-custom package),
  then re-run this script."
fi
log "Kernel build tree OK: /lib/modules/${KERNEL}/build"

# --- Step 2: resolve the in-repo source + fixed version ------------------------
SRC_DIR="$(resolve_src)" \
  || die "no Intel module source found (expected ${REPO_ROOT}/kernel/ramsleuth-intel or ${INSTALLED_SRC})"
VERSION="$(resolve_version)"
log "Intel module source: ${SRC_DIR}"
log "Version: ${VERSION} (fixed -> idempotent dkms add)"
# Provenance (auditable): the sha256 fingerprint of the files that get staged.
for f in "${STAGE_FILES[@]}"; do
  if [[ -f "${SRC_DIR}/${f}" ]]; then
    sha256sum "${SRC_DIR}/${f}"
  else
    log "  (no ${f} in ${SRC_DIR} — tolerated, not staged)"
  fi
done

# --- Step 3: stage source into /usr/src/<module>-<version> + depmod override ----
# DKMS only discovers a module whose source is staged in
# /usr/src/<module>-<version>/ containing a dkms.conf. The in-repo dkms.conf
# carries PACKAGE_VERSION="@VERSION@"; we sed it to the ramsleuth workspace
# version (the documented contract in dkms.conf). DEST_MODULE_LOCATION stays
# "/extra", so the depmod override below resolves it from /extra.
STAGE_DIR="/usr/src/${MODULE}-${VERSION}"
install -d "${STAGE_DIR}"
for f in "${STAGE_FILES[@]}"; do
  [[ -f "${SRC_DIR}/${f}" ]] || continue      # tolerate absent extras (e.g. README)
  install -m 644 "${SRC_DIR}/${f}" "${STAGE_DIR}/${f}"
done
[[ -f "${STAGE_DIR}/dkms.conf" ]] || die "staging produced no dkms.conf in ${STAGE_DIR} — inspect the source at ${SRC_DIR}"
sed -i "s/@VERSION@/${VERSION}/g" "${STAGE_DIR}/dkms.conf"
log "Staged ${MODULE} source to ${STAGE_DIR} (version ${VERSION})"
# depmod conf so the out-of-tree /extra module resolves cleanly (idempotent overwrite).
install -d /usr/lib/depmod.d
printf '%s\n' '# RamSleuth INTEL-06: resolve the out-of-tree ramsleuth_intel module from /extra.' \
  "override ${MODULE} /extra/${MODULE}.ko" > "/usr/lib/depmod.d/${MODULE}.conf"

# --- Verify-pause: confirm the source before the first system mutation ---------
# The provenance above (the source path + its checksums, the staged copy
# inspectable at ${STAGE_DIR}) is the human gate. A non-TTY (fully scripted)
# context logs and continues; answering `n` is a clean skip (exit 0 — the app
# keeps working without the module; re-run this script to build later).
if [[ -t 0 ]]; then
  read -r -p "Build + install ${MODULE}/${VERSION} into DKMS now? [Y/n] " ANS || ANS=""
  case "${ANS:-Y}" in
    n|N)
      log "Skipped — no DKMS build performed; the staged source stays at ${STAGE_DIR}."
      exit 0
      ;;
  esac
else
  log "No TTY (scripted context) — continuing without an interactive confirm"
fi

# --- Step 4: dkms add + build + install ----------------------------------------
# `dkms add <module>/<version>` finds the staged /usr/src tree above. We capture
# `dkms status` once and match it via here-string grep (no pipe -> no SIGPIPE
# under pipefail).
DKMS_STATUS="$(dkms status 2>/dev/null || true)"
status_registered() { grep -qE "^${MODULE}/${VERSION}," <<<"${DKMS_STATUS}"; }
status_installed()  { grep -qE "^${MODULE}/${VERSION},[[:space:]]*${KERNEL}.*:[[:space:]]*installed[[:space:]]*$" <<<"${DKMS_STATUS}"; }
# Clear any prior registration first: on a re-run the version is already
# registered, and without this `dkms build` below would be a no-op on the
# freshly-staged source. `|| true` keeps first runs (nothing registered) safe.
dkms remove "${MODULE}/${VERSION}" -k "${KERNEL}" --no-depmod 2>/dev/null || true
if ! dkms add "${MODULE}/${VERSION}"; then
  if status_registered; then
    log "${MODULE}/${VERSION} already registered with DKMS — continuing"
  else
    die "dkms add ${MODULE}/${VERSION} failed — run 'dkms status' to inspect"
  fi
fi
dkms build "${MODULE}/${VERSION}" -k "${KERNEL}" \
  || die "dkms build ${MODULE} -k ${KERNEL} failed — run 'dmesg | tail' / 'dkms status' to inspect build errors"
DKMS_STATUS="$(dkms status 2>/dev/null || true)"     # refresh after the build
if status_installed; then
  log "${MODULE}/${VERSION} already installed for ${KERNEL} — skipping dkms install"
else
  dkms install "${MODULE}/${VERSION}" -k "${KERNEL}" \
    || die "dkms install ${MODULE}/${VERSION} -k ${KERNEL} failed — run 'dmesg | tail' / 'dkms status' to inspect"
fi

# --- Step 5: load now + verify (vendor-aware, non-Intel-safe) ------------------
# modprobe runs module_init, which on a non-Intel host returns -ENODEV (vendor
# != 0x8086) -> modprobe exits non-zero and no kobject is created. That is the
# EXPECTED clean outcome (the module is idle; the /dev/mem fallback is used).
# We discriminate that from a genuine Intel-side failure (Secure Boot, lockdown,
# MCHBAR disabled) by the host-bridge vendor, and by whether the kobject appears.
# Unload the resident .ko (if any) so modprobe loads the freshly-built one
# (modprobe is a no-op on an already-loaded module). `|| true` keeps the
# not-loaded case safe.
rmmod "${MODULE}" 2>/dev/null || true
if modprobe "${MODULE}"; then
  if verify_kobject; then
    install -d /etc/modules-load.d
    printf '%s\n' "${MODULE}" > "/etc/modules-load.d/${MODULE}.conf"   # load at boot (success only)
    log "SUCCESS: ${MODULE} loaded; ${KOBJ} present with its IMC register attributes."
    log "Next: start the ramsleuth daemon for live Intel IMC subtimings."
    exit 0
  fi
  die "modprobe ${MODULE} succeeded but ${KOBJ}/mchbar_enabled is missing — run 'dmesg | tail' + 'dkms status' to inspect"
fi
# modprobe failed: classify the outcome by the host-bridge vendor.
VENDOR="$(detect_vendor || true)"
VENDOR_NORM="$(normalize_vendor "${VENDOR}")"
if [[ -n "${VENDOR_NORM}" && "${VENDOR_NORM}" != "8086" ]]; then
  # Non-Intel host (e.g. an AMD dev box): built OK, load idle by design.
  rm -f "/etc/modules-load.d/${MODULE}.conf"   # never auto-attempt at boot on non-Intel
  log "NOTE: built OK; no Intel host bridge detected (PCI 0000:00:00.0 vendor 0x${VENDOR_NORM}) — module idle, /dev/mem fallback will be used"
  log "This host is not Intel; ramsleuth will report the Intel section as N/A (DriverMissing) and use the /dev/mem fallback where available. Nothing to fix here."
  exit 0
fi
if [[ -z "${VENDOR_NORM}" ]]; then
  die "modprobe ${MODULE} failed and the host-bridge vendor at 0000:00:00.0 could not be read — run 'dmesg | tail' + 'dkms status' to inspect"
fi
die "modprobe ${MODULE} failed on an Intel host (vendor 0x8086) — inspect 'dmesg | tail' + 'dkms status' (Secure Boot/lockdown? MCHBAR disabled?)"
