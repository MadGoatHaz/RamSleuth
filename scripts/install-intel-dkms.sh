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
# Secure Boot (SB-aware, never-mysterious): on a Secure Boot host the kernel
# refuses an UNSIGNED out-of-tree module, so when this helper detects Secure
# Boot EARLY (mokutil --sb-state, else the EFI + kernel-lockdown fallback) it
# (1) generates a persistent signing key pair ONCE (idempotent, 10-year) at
# /var/lib/ramsleuth/ramsleuth-intel-signing/ (key.pem + cert.pem — NOT
# package-owned: it survives reinstalls + upgrades), (2) signs the built .ko
# with it via a per-module /etc/dkms/framework.conf.d/ entry (the signing
# mechanism the installed DKMS 3.x reads — it does NOT support per-module
# SIGN/KEY_* options in dkms.conf, verified against the installed dkms),
# (3) stages the cert for the one-time MOK enrollment (mokutil --import),
# and (4) when the load is pending that one-time reboot, prints the clear
# one-time guidance and exits 10 — a DISTINCT non-fatal code (the GUI renders
# it as an amber "one step left" state, not a failure; build + install
# succeeded). A non-Secure-Boot host takes EXACTLY the pre-SB-aware flow: no
# key, no drop-in, no enrollment (a stale drop-in is removed then —
# self-heal).
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
# kernel-build-tree check, Secure Boot detection, host-bridge vendor
# detection) and previews the privileged commands; it stops BEFORE any
# privileged op (no sudo, no dkms, no modprobe, no /usr/src, /usr/lib/depmod.d,
# or /etc/modules-load.d writes) and exits 0. A non-Intel host is predicted
# from the vendor at dry-run time; the Secure Boot state is previewed too.

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
SB_SIGN_DIR="/var/lib/ramsleuth/ramsleuth-intel-signing"
SB_DROPIN="/etc/dkms/framework.conf.d/ramsleuth-ramsleuth_intel.conf"

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

# Secure Boot detection (read-only): prints "enabled" or "disabled".
# Primary: `mokutil --sb-state` (the authoritative UEFI-side answer: "SecureBoot
# enabled" / "SecureBoot disabled"). Fallback (mokutil absent): an EFI system
# whose active kernel lockdown is neither None/off (no lockdown) nor Integrity
# (module loads still permitted) — i.e. Confidentiality — refuses unsigned
# module loads, which is the case that needs signing + MOK.
detect_secure_boot() {
  local sb_out="" lockdown=""
  if command -v mokutil >/dev/null 2>&1; then
    sb_out="$(mokutil --sb-state 2>/dev/null || true)"
  fi
  case "${sb_out}" in
    *"SecureBoot enabled"*)  printf 'enabled\n'; return 0 ;;
    *"SecureBoot disabled"*) printf 'disabled\n'; return 0 ;;
  esac
  if [[ -d /sys/firmware/efi && -r /sys/kernel/security/lockdown ]]; then
    lockdown="$(awk 'match($0, /\[[^]]*\]/) { s = substr($0, RSTART + 1, RLENGTH - 2); gsub(/[[:space:]]/, "", s); print s; exit }' /sys/kernel/security/lockdown)"
  fi
  case "${lockdown}" in
    ""|none|None|off|OFF|integrity|Integrity) printf 'disabled\n' ;;
    *)                                        printf 'enabled\n' ;;
  esac
  return 0
}

# Classify a failed `modprobe` as a Secure Boot / lockdown rejection (an
# unsigned or un-enrolled-key module): true when SB was detected, or when the
# recent dmesg names the module with a rejection marker (catches the
# SB-on-but-detection-missed case — mokutil absent at detect time, a UEFI
# change since the prior run).
sb_load_rejected() {
  if [[ "${SB_STATE}" == "enabled" ]]; then
    return 0
  fi
  local dmesg_tail
  dmesg_tail="$(dmesg 2>/dev/null | grep -iE "${MODULE}|lockdown|key was rejected|executive|module verification" || true)"
  [[ -n "${dmesg_tail}" ]] \
    && grep -qiE 'lockdown|key was rejected|rejected by service|module verification failed|is not signed|unsigned' <<<"${dmesg_tail}"
}

# The clear one-time instruction block (stdout for the terminal, plus a SINGLE
# stderr line carrying the reason for the GUI's structured tail). The
# non-fatal exit (10) is done by the caller.
print_secure_boot_guidance() {
  if command -v mokutil >/dev/null 2>&1; then
    log "================================  Secure Boot  ===================================="
    log "Secure Boot is enabled on this host. The ${MODULE} module was BUILT + INSTALLED"
    log "and signed with the persistent RamSleuth key (generated once, idempotent, kept"
    log "across reinstalls): ${SB_SIGN_DIR}/cert.pem"
    log ""
    log "ONE-TIME (no hoops afterwards):"
    log "  1. Reboot now."
    log "  2. At the blue MOK screen choose 'Enroll MOK key(s)' → 'Continue' → 'Yes'"
    log "     (set/confirm the MOK password when prompted)."
    log "  3. After the reboot, re-click 'Set up RamSleuth' (or re-run this helper) —"
    log "     the driver loads; every later run (incl. after kernel updates) is a no-op."
    log "Prefer not to reboot? Disable Secure Boot in UEFI, then re-click Setup —"
    log "the signed module loads without the MOK step."
    log "===================================================================================="
    printf '[intel-dkms] ERROR: Secure Boot is on — ONE-TIME: reboot, at the blue MOK screen choose Enroll MOK key(s) → Continue → Yes, then re-click Setup (driver built + installed + signed; only the one-time MOK enrollment is pending); alternative: disable Secure Boot in UEFI and re-click Setup\n' >&2
  else
    log "================================  Secure Boot  ===================================="
    log "Secure Boot is enabled on this host, but mokutil is not installed, so the signed"
    log "key (${SB_SIGN_DIR}/cert.pem) cannot be MOK-enrolled automatically. The ${MODULE}"
    log "module was BUILT + INSTALLED + signed."
    log "Fix: disable Secure Boot in UEFI, then re-click 'Set up RamSleuth' — the"
    log "module loads without the MOK step."
    log "===================================================================================="
    printf '[intel-dkms] ERROR: Secure Boot is on and mokutil is absent — the signed key cannot be enrolled; disable Secure Boot in UEFI, then re-click Setup (driver built + installed)\n' >&2
  fi
}

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
  DRY_SB="$(detect_secure_boot)"
  if [[ "${DRY_SB}" == "enabled" ]]; then
    log "DRY-RUN: Secure Boot is ON — the real run would sign the module with the persistent key (${SB_SIGN_DIR}/), stage the one-time MOK enrollment, and exit 10 with the one-time guidance if the load is pending it"
  else
    log "DRY-RUN: Secure Boot off — the real run would build + load unchanged"
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
  log "  dkms remove <each registered ${MODULE}/<version>> --all --no-depmod   (self-heal: clear stale registrations; a no-op on a first run)"
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

# --- Secure Boot detection (EARLY — before any build; the flow branches on it) ---
SB_STATE="$(detect_secure_boot)"
if [[ "${SB_STATE}" == "enabled" ]]; then
  log "Secure Boot: ENABLED on this host — the module will be signed with the persistent RamSleuth key + staged for the one-time MOK enrollment (a NON-FATAL one-time step: exit 10 until it is done, not a hard failure)"
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
  die "cannot determine the ${KERNEL} headers package automatically — install the matching 'linux-headers' / cachyos-custom package, then re-run this script"
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

# --- Step 4a (Secure Boot only): persistent signing key + the DKMS signing entry ---
# The installed DKMS (verified 3.4.3) signs from /etc/dkms/framework.conf +
# framework.conf.d/*.conf (try_sign_modules / mok_signing_key /
# mok_certificate) — it does NOT read per-module SIGN/KEY_* options from
# dkms.conf — so the signing config is a per-module drop-in pointing at the
# persistent key. The entry is written ONLY on an SB run (and removed on a
# non-SB run — self-heal), so a non-SB build is signed by nobody:
# byte-for-byte the pre-SB-aware behavior. Both vendor helpers write separate
# drop-ins; a host runs at most one (the CPU vendor decides which helper is
# invoked).
if [[ "${SB_STATE}" == "enabled" ]]; then
  if [[ -f "${SB_SIGN_DIR}/key.pem" && -f "${SB_SIGN_DIR}/cert.pem" ]]; then
    log "Secure Boot: reusing the persistent signing key at ${SB_SIGN_DIR}/ (generated once — idempotent)"
  else
    install -d -m 0700 "${SB_SIGN_DIR}"
    openssl req -new -x509 -newkey rsa:2048 -nodes -days 3650 \
      -keyout "${SB_SIGN_DIR}/key.pem" -out "${SB_SIGN_DIR}/cert.pem" \
      -subj "/CN=RamSleuth ${MODULE} signing/" \
      || die "Secure Boot: openssl key generation in ${SB_SIGN_DIR} failed — is openssl installed?"
    chmod 0600 "${SB_SIGN_DIR}/key.pem"
    log "Secure Boot: generated the persistent signing key pair in ${SB_SIGN_DIR}/ (key.pem + cert.pem, 10 years, reused on every later run)"
  fi
  install -d /etc/dkms/framework.conf.d
  printf '%s\n' \
    "# RamSleuth ${MODULE}: sign the out-of-tree module with the persistent" \
    "# RamSleuth key (Secure Boot). DKMS 3.x reads signing from framework.conf" \
    "# (+ framework.conf.d/*.conf) — NOT from dkms.conf." \
    "try_sign_modules=true" \
    "mok_signing_key=${SB_SIGN_DIR}/key.pem" \
    "mok_certificate=${SB_SIGN_DIR}/cert.pem" > "${SB_DROPIN}"
  log "Secure Boot: DKMS will sign the module with ${SB_SIGN_DIR}/cert.pem (drop-in: ${SB_DROPIN})"
else
  if [[ -f "${SB_DROPIN}" ]]; then
    rm -f "${SB_DROPIN}"
    log "Secure Boot off: removed the stale signing drop-in ${SB_DROPIN} (self-heal — the build is unsigned, as before)"
  fi
fi

# --- Step 4: dkms self-heal + add + build + install -----------------------------
# `dkms add <module>/<version>` finds the staged /usr/src tree above. We capture
# `dkms status` once and match it via here-string grep (no pipe -> no SIGPIPE
# under pipefail).
DKMS_STATUS="$(dkms status 2>/dev/null || true)"
status_registered() { grep -qE "^${MODULE}/${VERSION}," <<<"${DKMS_STATUS}"; }
status_installed()  { grep -qE "^${MODULE}/${VERSION},[[:space:]]*${KERNEL}.*:[[:space:]]*installed[[:space:]]*$" <<<"${DKMS_STATUS}"; }
# Self-heal: clear every prior registration of this module BEFORE the add —
# not just the current version: a prior partial/failed run, an older module
# version, or a previous kernel can leave registered builds behind, `dkms add`
# fails on an already-registered module/version, and a stale registration can
# carry a broken build (a `dkms build` on it would be a no-op on the
# freshly-staged source). Each is removed with `--all` (all kernels) +
# `--no-depmod`, then re-added below from the freshly staged source. First
# runs (nothing registered) are an untouched no-op; foreign modules are never
# touched.
if grep -qE "^${MODULE}/" <<<"${DKMS_STATUS}"; then
  while IFS= read -r entry; do
    [[ -n "${entry}" ]] || continue
    reg="${entry%%,*}"
    if dkms remove "${reg}" --all --no-depmod 2>/dev/null; then
      log "self-heal: removed stale DKMS registration ${reg} (re-adding ${MODULE}/${VERSION} below)"
    fi
  done < <(grep -E "^${MODULE}/" <<<"${DKMS_STATUS}")
fi
if ! dkms add "${MODULE}/${VERSION}"; then
  if status_registered; then
    log "${MODULE}/${VERSION} already registered with DKMS — continuing"
  else
    die "dkms add ${MODULE}/${VERSION} failed — run 'dkms status' to inspect"
  fi
fi
if ! dkms build "${MODULE}/${VERSION}" -k "${KERNEL}"; then
  MAKE_LOG="/var/lib/dkms/${MODULE}/${VERSION}/${KERNEL}/build/make.log"
  if [[ -f "${MAKE_LOG}" ]]; then
    log "dkms build failed — last 40 lines of ${MAKE_LOG}:"
    tail -n 40 "${MAKE_LOG}" >&2
  fi
  die "dkms build ${MODULE} -k ${KERNEL} failed — compile break (the make.log tail above names the file + line; build dir /var/lib/dkms/${MODULE}/${VERSION}/${KERNEL}/build/)"
fi
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
# SB run: stage the signing cert for the one-time MOK enrollment BEFORE the
# load (mokutil prompts for the MOK password — expected; first use sets it).
# Non-fatal: a failed import (no TTY / already pending) only warns.
if [[ "${SB_STATE}" == "enabled" ]]; then
  if command -v mokutil >/dev/null 2>&1; then
    log "Secure Boot: staging the signing key for the one-time MOK enrollment (mokutil --import — a MOK password prompt may appear)"
    if ! mokutil --import "${SB_SIGN_DIR}/cert.pem"; then
      log "WARNING: mokutil --import did not complete (no TTY / enrollment already pending) — the MOK step may need a manual pass"
    fi
  else
    log "Secure Boot: mokutil is not installed — automatic MOK enrollment is skipped (disable Secure Boot in UEFI; see the guidance below)"
  fi
fi
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
# modprobe failed: a Secure Boot / lockdown rejection is the one-time MOK
# case — actionable (the distinct non-fatal exit 10; the GUI renders an
# amber "one step left" state), NOT a hard failure (built + installed OK).
if sb_load_rejected; then
  print_secure_boot_guidance
  exit 10
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
