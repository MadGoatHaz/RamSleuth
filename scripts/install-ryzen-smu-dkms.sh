#!/usr/bin/env bash
#
# install-ryzen-smu-dkms.sh — idempotent operator helper (HANDOVER §7).
#
# Installs the ryzen_smu AMD kernel module via DKMS so ramsleuth can serve
# live AMD subtimings. OPTIONAL recommended extra: without the module the
# app degrades to N/A (DriverMissing) (exit 0, no panic); the frozen systemd
# unit never loads the module itself.
#
# Usage: scripts/install-ryzen-smu-dkms.sh (re-execs under sudo if not root —
# the operator's sudo prompt appears there). Safe to re-run: every step is
# guarded; any failure prints a clear message + exits non-zero, never leaving
# DKMS in a silent half-state. Never touches ramsleuth state.
#
# Source resolution order (C21-08, vendor-first):
#   1. RYZEN_SMU_URL / RYZEN_SMU_PIN env overrides — honored by the git
#      path (an override selects a specific upstream commit; the vendor,
#      which carries only the default pin, is then skipped).
#   2. Local vendored source (offline, zero network): the repo's
#      packaging/ryzen-smu-dkms/vendor/ryzen-smu (dev checkout) or the
#      installed /usr/share/ryzen-smu-dkms/vendor/ryzen-smu (C21-09) — every
#      file verified against vendor/SUMS.sha256 before staging; a mismatch
#      dies (no silent fallback).
#   3. Pinned git clone (the unchanged fallback when no vendor tree is
#      present, e.g. the ramsleuth-bin tarball install) —
#      RYZEN_SMU_FORCE_REMOTE=1 forces this path even when a vendor tree
#      is present.

set -euo pipefail

# --- Constants --------------------------------------------------------------
KERNEL="$(uname -r)"
MODULE="ryzen_smu"
# Upstream (VERIFIED): amkillam/ryzen_smu — PINNED to commit
# d2983668300dd2a598e5a7dc40e71ce0678cc270 (verified 2026-08-15: the current
# `main` HEAD, "Fix cpuid include on 7.2+ kernels (#53)", and exactly what
# the dev host already runs as 1.d298366). Builds module `ryzen_smu`,
# exposes /sys/kernel/ryzen_smu_drv/pm_table, ships its own dkms.conf +
# monitor_cpu CLI (Zen3+, kernel 7.2+). Never track branch HEAD
# (anti-contamination): the pin is a hard freeze — shown + checksummed +
# confirmed before any build; if upstream deletes/force-pushes it, this
# helper dies with a clear message and the fix is a one-line RYZEN_SMU_PIN
# re-pin. The former 53XU/ryzen_smu default is DEAD (HTTP 404). The URL is
# still overridable via RYZEN_SMU_URL — verify the upstream at setup time
# (HANDOVER §7 step 2); do NOT trust a cached URL if it moves.
UPSTREAM_URL="${RYZEN_SMU_URL:-https://github.com/amkillam/ryzen_smu.git}"
UPSTREAM_PIN="${RYZEN_SMU_PIN:-d2983668300dd2a598e5a7dc40e71ce0678cc270}"
# Verified sysfs kobject (amkillam drv.c; matches the daemon, P5-08):
# canonical ryzen_smu_drv path first, legacy ryzen_smu path as secondary.
PM_TABLE="/sys/kernel/ryzen_smu_drv/pm_table"
PM_TABLE_LEGACY="/sys/kernel/${MODULE}/pm_table"
SRC_DIR="/opt/ryzen-smu-src"
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"    # this script sits in <repo>/scripts

# --- Helpers -----------------------------------------------------------------
log() { printf '[ryzen-smu-dkms] %s\n' "$*"; }
die() { printf '[ryzen-smu-dkms] ERROR: %s\n' "$*" >&2; exit 1; }
# Fallback dkms.conf (P5-03; dual-location, P5-04-fix): first existing of the
# repo-relative path (run from the repo) or the installed /usr/share path (the
# P5-05 package installs this helper to /usr/bin, where REPO_ROOT resolves to
# /usr). Prints the chosen path; fails if neither exists.
resolve_dkms_conf() {
  local c
  for c in "${REPO_ROOT}/packaging/ryzen-smu-dkms/dkms.conf" \
           "/usr/share/ryzen-smu-dkms/dkms.conf"; do
    if [[ -f "${c}" ]]; then printf '%s\n' "${c}"; return 0; fi
  done
  return 1
}

# Vendor-source resolution (C21-07/C21-08, offline): first EXISTING of the
# repo-relative vendored tree (dev checkout — the script's real path is
# canonicalized via readlink, so a symlinked invocation resolves to the repo
# it points into) or the installed /usr/share copy (the ryzen-smu-dkms
# package, C21-09). Prints the chosen dir; fails if neither exists.
resolve_vendor_src() {
  local real dir c
  real="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || printf '%s' "${BASH_SOURCE[0]}")"
  dir="$(cd -- "$(dirname -- "${real}")/.." 2>/dev/null && pwd)"
  for c in "${dir}/packaging/ryzen-smu-dkms/vendor/ryzen-smu" \
           "/usr/share/ryzen-smu-dkms/vendor/ryzen-smu"; do
    if [[ -d "${c}" ]]; then printf '%s\n' "${c}"; return 0; fi
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
# DKMS needs a kernel build tree; we never guess a custom-kernel headers
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

# --- Step 2: fetch the PINNED ryzen_smu source (never branch HEAD) --------------
# Provenance BEFORE the fetch (transparency): the URL + the full pin + the
# short pin, so the operator sees exactly which commit will be built.
log "Upstream: ${UPSTREAM_URL}"
log "Pinned commit: ${UPSTREAM_PIN} (short: ${UPSTREAM_PIN:0:7}) — never branch HEAD"
# Vendor-first (C21-07/08, offline): a local, SUMS-verified vendored source
# replaces the network fetch when (a) a vendor tree is present, (b) no
# RYZEN_SMU_PIN override is set (an override selects a specific upstream
# commit — only the git path can build it), and (c) RYZEN_SMU_FORCE_REMOTE
# is not 1 (the escape that keeps the git path reachable).
VENDOR_SRC="$(resolve_vendor_src || true)"
USE_VENDOR=0
if [[ -n "${VENDOR_SRC}" ]]; then
  if [[ "${RYZEN_SMU_FORCE_REMOTE:-0}" == "1" ]]; then
    log "RYZEN_SMU_FORCE_REMOTE=1 — using the git path (the vendored source at ${VENDOR_SRC} is ignored)"
  elif [[ -n "${RYZEN_SMU_PIN:-}" || -n "${RYZEN_SMU_URL:-}" ]]; then
    log "RYZEN_SMU_PIN override set — using the git path (the vendor carries only the default pin)"
  else
    USE_VENDOR=1
    SRC_DIR="${VENDOR_SRC}"
    PKGVER="1.${UPSTREAM_PIN:0:7}"    # fixed version — no git rev-list on the vendor path
    log "Vendored source (offline, no network fetch): ${SRC_DIR}"
    log "Pinned commit: ${UPSTREAM_PIN} (short: ${UPSTREAM_PIN:0:7}) — vendored copy (no git metadata)"
    SUMS_FILE="$(dirname -- "${SRC_DIR}")/SUMS.sha256"
    [[ -f "${SUMS_FILE}" ]] \
      || die "vendor tree found at ${SRC_DIR} but its ${SUMS_FILE} manifest is missing — refusing to build from unverified source"
    (cd -- "$(dirname -- "${SRC_DIR}")" && sha256sum -c --quiet SUMS.sha256) \
      || die "vendor source verification FAILED (${SUMS_FILE} mismatch) — no silent fallback: restore the vendored files (C21-07) or set RYZEN_SMU_FORCE_REMOTE=1 for the git path"
    log "Vendor source verified against SUMS.sha256 (offline provenance OK)"
  fi
fi
if [[ "${USE_VENDOR}" -eq 0 ]]; then
  if [[ -d "${SRC_DIR}/.git" ]]; then
    # Existing tree: skip the fetch when already at the pin (idempotent +
    # offline-tolerant), else fetch + checkout the pin.
    if [[ "$(git -C "${SRC_DIR}" rev-parse HEAD)" == "${UPSTREAM_PIN}" ]]; then
      log "Source tree already at the pinned commit — skipping fetch"
    else
      git -C "${SRC_DIR}" fetch --depth 1 origin "${UPSTREAM_PIN}" \
        || die "git fetch of pinned commit ${UPSTREAM_PIN:0:7} from ${UPSTREAM_URL} failed — verify the network (and that the pin still exists upstream) and retry"
      git -C "${SRC_DIR}" checkout -q "${UPSTREAM_PIN}" \
        || die "git checkout of pinned commit ${UPSTREAM_PIN:0:7} failed — inspect ${SRC_DIR} and retry"
    fi
  else
    # Fresh shallow clone, then fetch + checkout the pinned commit; wipe a
    # stale non-git dir of unknown provenance first.
    rm -rf "${SRC_DIR}"
    git clone --depth 1 "${UPSTREAM_URL}" "${SRC_DIR}" \
      || die "git clone of ${UPSTREAM_URL} failed — verify the URL (and network) and retry"
    git -C "${SRC_DIR}" fetch --depth 1 origin "${UPSTREAM_PIN}" \
      || die "git fetch of pinned commit ${UPSTREAM_PIN:0:7} failed — verify the pin still exists upstream (fix: a one-line RYZEN_SMU_PIN re-pin) and retry"
    git -C "${SRC_DIR}" checkout -q "${UPSTREAM_PIN}" \
      || die "git checkout of pinned commit ${UPSTREAM_PIN:0:7} failed — inspect ${SRC_DIR} and retry"
  fi
  # Hard verify (the anti-contamination core): the tree must be exactly at the
  # pin — no silent branch-HEAD fallback. If upstream deleted/force-pushed the
  # commit, the fetch above dies; the fix is a one-line RYZEN_SMU_PIN re-pin.
  [[ "$(git -C "${SRC_DIR}" rev-parse HEAD)" == "${UPSTREAM_PIN}" ]] \
    || die "pinned source check failed (no silent branch-HEAD fallback)"
  log "Pinned source: $(git -C "${SRC_DIR}" log -1 --format='%h  %ad  %an  %s' --date=short)"
fi
# Provenance AFTER the source resolution (auditable, both paths): the
# sha256sum fingerprint of the six files that get staged (the staged copies
# land verbatim in ${STAGE_DIR}).
for f in LICENSE Makefile dkms.conf drv.c smu.c smu.h; do
  if [[ -f "${SRC_DIR}/${f}" ]]; then
    sha256sum "${SRC_DIR}/${f}"
  else
    log "  (no ${f} in ${SRC_DIR} — tolerated, not staged)"
  fi
done

# --- Step 3: stage source into /usr/src/<module>-<version> (P5-11 fix) -----------
# DKMS only discovers a module whose source is staged in
# /usr/src/<module>-<version>/ containing a dkms.conf; the original script
# cloned to $SRC_DIR and never staged it -> "Arguments <module> and
# <module-version> are not specified". Flow follows the working AUR
# ryzen_smu-dkms-git PKGBUILD (pkgver = rev-count . short-hash; the vendor
# path above uses the fixed 1.<short> — no git rev-list).
if [[ "${USE_VENDOR}" -eq 0 ]]; then
  PKGVER="$(cd -- "${SRC_DIR}" && printf '%s.%s' "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)")"
fi
STAGE_DIR="/usr/src/${MODULE}-${PKGVER}"
install -d "${STAGE_DIR}"
for f in LICENSE Makefile dkms.conf drv.c smu.c smu.h; do
  [[ -f "${SRC_DIR}/${f}" ]] || continue      # tolerate absent extras
  install -m 644 "${SRC_DIR}/${f}" "${STAGE_DIR}/${f}"
done
# Concrete dkms.conf: prefer the repo's (dual-location fallback, P5-04-fix);
# the AUR placeholder variant is NOT used. Align its PACKAGE_VERSION with the
# staged dir (proven AUR pattern) so the MAKE M= path resolves; if the repo's
# is absent, keep the source's own and substitute its @VERSION@/@CFLGS@.
REPO_CONF="$(resolve_dkms_conf || true)"
if [[ -n "${REPO_CONF}" ]]; then
  install -m 644 "${REPO_CONF}" "${STAGE_DIR}/dkms.conf"
  sed -i "s/^PACKAGE_VERSION=.*/PACKAGE_VERSION=\"${PKGVER}\"/" "${STAGE_DIR}/dkms.conf"
else
  [[ -f "${STAGE_DIR}/dkms.conf" ]] || die "No dkms.conf available (repo fallback missing; source ${SRC_DIR}/dkms.conf absent)."
  sed -i "s/@VERSION@/${PKGVER}/g; s/@CFLGS@//g" "${STAGE_DIR}/dkms.conf"
fi
log "Staged ${MODULE} source to ${STAGE_DIR} (version ${PKGVER})"
# depmod conf so the out-of-tree /extra module resolves cleanly (idempotent overwrite).
install -d /usr/lib/depmod.d
printf '%s\n' '# RamSleuth P5-11: resolve the out-of-tree ryzen_smu module from /extra.' \
  "override ${MODULE} /extra/${MODULE}.ko" > "/usr/lib/depmod.d/${MODULE}.conf"
# Userspace CLI (bonus, ground truth): build monitor_cpu; non-fatal if absent — the module install is the priority.
if [[ -d "${SRC_DIR}/userspace" ]] && make -C "${SRC_DIR}/userspace" && [[ -f "${SRC_DIR}/userspace/monitor_cpu" ]]; then
  install -Dm 700 "${SRC_DIR}/userspace/monitor_cpu" /usr/bin/monitor_cpu \
    || log "WARNING: could not install monitor_cpu to /usr/bin — continuing (module install is priority)"
else
  log "WARNING: monitor_cpu unavailable (no userspace dir / build failed) — continuing"
fi

# --- Verify-pause: confirm the pinned source before the first system mutation ----
# The provenance above (the exact commit + its checksums, the staged source
# inspectable at ${SRC_DIR} / ${STAGE_DIR}) is the human gate. A non-TTY
# (fully scripted) context logs and continues; answering `n` is a clean skip
# (exit 0 — the app keeps working without the module; re-run this script to
# build later).
if [[ -t 0 ]]; then
  read -r -p "Build this pinned source now? [Y/n] " ANS || ANS=""
  case "${ANS:-Y}" in
    n|N)
      log "Skipped — no DKMS build performed; the pinned source stays staged at ${STAGE_DIR}."
      exit 0
      ;;
  esac
else
  log "No TTY (scripted context) — continuing with the pinned source without an interactive confirm"
fi

# --- Step 4: dkms add + build + install -----------------------------------------
# `dkms add <module>/<version>` now finds the staged /usr/src tree above.
if ! dkms add "${MODULE}/${PKGVER}"; then
  dkms status | grep -q "${MODULE}/${PKGVER}" \
    && log "${MODULE}/${PKGVER} already registered with DKMS — continuing" \
    || die "dkms add ${MODULE}/${PKGVER} failed — run 'dkms status' to inspect"
fi
dkms build "${MODULE}/${PKGVER}" -k "${KERNEL}" \
  || die "dkms build ${MODULE} -k ${KERNEL} failed — run 'dmesg | tail' / 'dkms status' to inspect build errors"
if dkms status | grep -qE "^${MODULE}/${PKGVER},[[:space:]]*${KERNEL},[[:space:]]*[^:]*:[[:space:]]*installed[[:space:]]*$"; then
  log "${MODULE}/${PKGVER} already installed for ${KERNEL} — skipping dkms install"
else
  dkms install "${MODULE}/${PKGVER}" -k "${KERNEL}" \
    || die "dkms install ${MODULE}/${PKGVER} -k ${KERNEL} failed — run 'dmesg | tail' / 'dkms status' to inspect"
fi

# --- Step 5: load now + at boot ---------------------------------------------------
modprobe "${MODULE}" || die "modprobe ${MODULE} failed — run 'dmesg | tail' to inspect load errors"
printf '%s\n' "${MODULE}" > "/etc/modules-load.d/${MODULE}.conf"   # idempotent overwrite

# --- Step 6: verify ------------------------------------------------------------------
# Matches the daemon (P5-08): canonical ryzen_smu_drv path first, legacy second.
if [[ -e "${PM_TABLE}" ]]; then
  log "SUCCESS: ${MODULE} loaded; ${PM_TABLE} present."
elif [[ -e "${PM_TABLE_LEGACY}" ]]; then
  log "SUCCESS: ${MODULE} loaded; legacy ${PM_TABLE_LEGACY} present (daemon prefers ${PM_TABLE})."
else
  die "pm_table missing after load (looked for ${PM_TABLE}, then ${PM_TABLE_LEGACY}) — run 'dmesg | tail' + 'dkms status' to inspect. Without the
module the app still works: N/A (DriverMissing), exit 0, no panic."
fi
log "Next: start the ramsleuth daemon; ground-truth CLI is now at /usr/bin/monitor_cpu."
