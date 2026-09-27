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
# Secure Boot (SB-aware, never-mysterious): on a Secure Boot host the kernel
# refuses an UNSIGNED out-of-tree module, so when this helper detects Secure
# Boot EARLY (mokutil --sb-state, else the EFI + kernel-lockdown fallback) it
# (1) generates a persistent signing key pair ONCE (idempotent, 10-year) at
# /var/lib/ramsleuth/ryzen-smu-signing/ (key.pem + cert.pem — NOT
# package-owned: it survives reinstalls + upgrades), (2) signs the built .ko
# with it via a per-module /etc/dkms/framework.conf.d/ entry (the signing
# mechanism the installed DKMS 3.x reads — it does NOT support per-module
# SIGN/KEY_* options in dkms.conf, verified against the installed dkms),
# (3) stages the cert for the one-time MOK enrollment (mokutil --import),
# and (4) when the load is pending that one-time reboot, prints the clear
# one-time guidance and exits 10 — a DISTINCT non-fatal code (the GUI renders
# it as an amber "one step left" state, not a failure; build + install
# succeeded). A non-Secure-Boot host takes EXACTLY the pre-SB-aware flow: no
# key, no drop-in, no enrollment, exit 0 on success (a stale drop-in is
# removed then — self-heal).
#
# Source resolution (C21-08, vendor-first):
#   1. Local vendored source — the PRIMARY path, offline, zero network: the
#      repo's packaging/ryzen-smu-dkms/vendor/ryzen-smu (dev checkout) or
#      the installed /usr/share/ryzen-smu-dkms/vendor/ryzen-smu (bundled in
#      the v2.4.6+ release assets by both main packages, C21-09) — every
#      file verified against the sibling vendor/SUMS.sha256 before staging;
#      a mismatch dies (no silent fallback). Since v2.4.6 the bundled tree
#      is ALWAYS present on a packaged install, so the in-app one-click
#      never needs the network — a failed git clone (the 2.4.5-era failure
#      mode) is not reachable from it.
#   2. Pinned git clone — a FALLBACK only: used when no vendor tree is
#      present (pre-2.4.6 assets, e.g. a ramsleuth-bin tarball install from
#      before the re-cut) or when RYZEN_SMU_URL / RYZEN_SMU_PIN env
#      overrides are set (an override selects a specific upstream commit;
#      the vendor, which carries only the default pin, is then skipped).
#      RYZEN_SMU_FORCE_REMOTE=1 forces this path even when a vendor tree
#      is present.

set -euo pipefail

# --- Durable setup-log capture (no-op standalone) ----------------------------
# When invoked via `ramsleuth-setup` (which exports RAMSLEUTH_SETUP_LOG and
# `exec`s us over its tee'd stdout), our output ALREADY reaches the log
# through the inherited pipe — a re-tee here would duplicate every line, so
# we tee only when NOT inside that stream (fd1 a pipe/socket = the setup's
# capture; a terminal/file/null = a manual run with the log env var set).
# Standalone (env unset — the TUI / operator path) is an exact no-op; a
# failed redirect falls back to terminal-only output (never breaks the run).
if [[ -n "${RAMSLEUTH_SETUP_LOG:-}" && -w "${RAMSLEUTH_SETUP_LOG}" ]]; then
  case "$(readlink -- "/proc/$$/fd/1" 2>/dev/null || true)" in
    pipe:* | socket:*)
      # Inside ramsleuth-setup's tee stream: inheritance covers the log.
      : ;;
    *)
      if ! exec > >(tee -a "$RAMSLEUTH_SETUP_LOG") 2>&1; then
        printf '[ryzen-smu-dkms] WARN: cannot tee to %s — terminal-only output\n' "$RAMSLEUTH_SETUP_LOG" >&2
      fi
      ;;
  esac
fi

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
    printf '[ryzen-smu-dkms] ERROR: Secure Boot is on — ONE-TIME: reboot, at the blue MOK screen choose Enroll MOK key(s) → Continue → Yes, then re-click Setup (driver built + installed + signed; only the one-time MOK enrollment is pending); alternative: disable Secure Boot in UEFI and re-click Setup\n' >&2
  else
    log "================================  Secure Boot  ===================================="
    log "Secure Boot is enabled on this host, but mokutil is not installed, so the signed"
    log "key (${SB_SIGN_DIR}/cert.pem) cannot be MOK-enrolled automatically. The ${MODULE}"
    log "module was BUILT + INSTALLED + signed."
    log "Fix: disable Secure Boot in UEFI, then re-click 'Set up RamSleuth' — the"
    log "module loads without the MOK step."
    log "===================================================================================="
    printf '[ryzen-smu-dkms] ERROR: Secure Boot is on and mokutil is absent — the signed key cannot be enrolled; disable Secure Boot in UEFI, then re-click Setup (driver built + installed)\n' >&2
  fi
}
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

# --- Secure Boot detection (EARLY — before any build; the flow branches on it) ---
SB_STATE="$(detect_secure_boot)"
if [[ "${SB_STATE}" == "enabled" ]]; then
  log "Secure Boot: ENABLED on this host — the module will be signed with the persistent RamSleuth key + staged for the one-time MOK enrollment (a NON-FATAL one-time step: exit 10 until it is done, not a hard failure)"
fi

# --- Step 1: prereqs + kernel build tree ---------------------------------------
log "Installing build tooling: dkms + base-devel (pacman, idempotent)..."
pacman -S --needed --noconfirm dkms base-devel
# DKMS needs a kernel build tree; we never guess a custom-kernel headers
# package (HANDOVER §7 step 1) — list candidates and stop with instructions.
if [[ ! -d "/lib/modules/${KERNEL}/build" ]]; then
  log "Kernel build tree MISSING for ${KERNEL} — candidate headers packages:"
  CANDIDATES="$(pacman -Qs headers 2>/dev/null | grep -iE 'cachyos|custom|linux-headers' || true)"
  [[ -n "${CANDIDATES}" ]] && printf '%s\n' "${CANDIDATES}"
  die "cannot determine the ${KERNEL} headers package automatically — install the matching 'linux-headers' / cachyos-custom package, then re-run this script"
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
  # Non-TTY robustness (the pkexec path has no controlling TTY): never let
  # git prompt for credentials (a private RYZEN_SMU_URL override would hang
  # the one-click flow; fail + die cleanly instead).
  export GIT_TERMINAL_PROMPT=0
  if [[ -d "${SRC_DIR}/.git" ]]; then
    # Existing tree: skip the fetch when already at the pin (idempotent +
    # offline-tolerant). A prior run may have died mid-clone leaving a
    # BROKEN .git (rev-parse fails on it) — that cannot reliably fetch /
    # checkout, so wipe it and start a fresh clone (self-heal).
    head="$(git -C "${SRC_DIR}" rev-parse HEAD 2>/dev/null || true)"
    if [[ "${head}" == "${UPSTREAM_PIN}" ]]; then
      log "Source tree already at the pinned commit — skipping fetch"
    else
      if [[ -z "${head}" ]]; then
        log "Existing ${SRC_DIR} has a broken/partial .git (a prior run died mid-clone) — wiping and re-cloning"
        rm -rf "${SRC_DIR}"
        git clone --depth 1 "${UPSTREAM_URL}" "${SRC_DIR}" \
          || die "git clone of ${UPSTREAM_URL} failed — verify the URL (and network) and retry"
      fi
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
# No /usr/lib/depmod.d override is written: on DKMS 3.x the built module
# deploys to /usr/lib/modules/<kernel>/updates/dkms/ (DEST_MODULE_LOCATION
# "/extra" is a no-op there) and depmod resolves it from updates/dkms/ — the
# older helper's `override <mod> /extra/<mod>.ko` line was invalid syntax
# depmod rejected AND pointless. A stale one is removed (self-heal).
if [[ -f "/usr/lib/depmod.d/${MODULE}.conf" ]]; then
  rm -f "/usr/lib/depmod.d/${MODULE}.conf"
  log "removed the stale /usr/lib/depmod.d/${MODULE}.conf (invalid 'override' line from an older helper; the module resolves via updates/dkms/)"
fi
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
SB_SIGN_DIR="/var/lib/ramsleuth/ryzen-smu-signing"
SB_DROPIN="/etc/dkms/framework.conf.d/ramsleuth-${MODULE}.conf"
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

# --- Step 4: dkms self-heal + add + build + install ------------------------------
# Self-heal: clear every stale DKMS registration of this module BEFORE the
# add — `dkms add <module>/<version>` FAILS if the module/version is already
# registered (the state a prior partial/failed run, an older module version,
# or a previous kernel leaves behind), and a stale registration can carry a
# broken build. `dkms status`-driven: each registered <module>/<version> of
# THIS module (parsed from the status line's first field — the form dkms
# 3.4.3's `remove` requires; a bare <module> is rejected with "Arguments
# <module> and <module-version> are not specified") is removed with --all
# (all kernels — the installed dkms 3.4.3 CLI takes --all, not the man
# page's --all-kernels) + --no-depmod, then re-added below from the freshly
# staged source. First runs (nothing registered) are an untouched no-op;
# foreign modules are never touched.
DKMS_STATUS="$(dkms status 2>/dev/null || true)"
if grep -qE "^${MODULE}/" <<<"${DKMS_STATUS}"; then
  while IFS= read -r entry; do
    [[ -n "${entry}" ]] || continue
    reg="${entry%%,*}"
    if dkms remove "${reg}" --all --no-depmod 2>/dev/null; then
      log "self-heal: removed stale DKMS registration ${reg} (re-adding ${MODULE}/${PKGVER} below)"
    fi
  done < <(grep -E "^${MODULE}/" <<<"${DKMS_STATUS}")
fi
# `dkms add <module>/<version>` now finds the staged /usr/src tree above.
if ! dkms add "${MODULE}/${PKGVER}"; then
  dkms status | grep -q "${MODULE}/${PKGVER}" \
    && log "${MODULE}/${PKGVER} already registered with DKMS — continuing" \
    || die "dkms add ${MODULE}/${PKGVER} failed — run 'dkms status' to inspect"
fi
if ! dkms build "${MODULE}/${PKGVER}" -k "${KERNEL}"; then
  MAKE_LOG="/var/lib/dkms/${MODULE}/${PKGVER}/${KERNEL}/build/make.log"
  if [[ -f "${MAKE_LOG}" ]]; then
    log "dkms build failed — last 40 lines of ${MAKE_LOG}:"
    tail -n 40 "${MAKE_LOG}" >&2
  fi
  die "dkms build ${MODULE} -k ${KERNEL} failed — compile break (the make.log tail above names the file + line; build dir /var/lib/dkms/${MODULE}/${PKGVER}/${KERNEL}/build/)"
fi
if dkms status | grep -qE "^${MODULE}/${PKGVER},[[:space:]]*${KERNEL},[[:space:]]*[^:]*:[[:space:]]*installed[[:space:]]*$"; then
  log "${MODULE}/${PKGVER} already installed for ${KERNEL} — skipping dkms install"
else
  # --force: DKMS 3.4.3's identical-module check ABORTS when a residual .ko of
  # this module is already in the kernel tree (e.g. the DKMS DB was wiped but
  # the physical file survived) — "already installed at version <X> … override
  # by specifying --force". --force overwrites the residual; on a clean first
  # run there is no residual, so --force is a no-op (no regression).
  dkms install "${MODULE}/${PKGVER}" -k "${KERNEL}" --force \
    || die "dkms install ${MODULE}/${PKGVER} -k ${KERNEL} failed — run 'dmesg | tail' / 'dkms status' to inspect"
fi

# --- Step 5: load now + at boot (Secure Boot aware) --------------------------------
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
if modprobe "${MODULE}"; then
  printf '%s\n' "${MODULE}" > "/etc/modules-load.d/${MODULE}.conf"   # idempotent overwrite
else
  # The load is pending the one-time MOK step (or a detection-missed SB
  # rejection): build + install succeeded — actionable guidance + the
  # distinct non-fatal exit 10 (the GUI renders an amber "one step left").
  if sb_load_rejected; then
    print_secure_boot_guidance
    exit 10
  fi
  die "modprobe ${MODULE} failed — run 'dmesg | tail' to inspect load errors"
fi

# --- Step 6: verify ------------------------------------------------------------------
# Matches the daemon (P5-08): canonical ryzen_smu_drv path first, legacy second.
if [[ -e "${PM_TABLE}" ]]; then
  log "SUCCESS: ${MODULE} loaded; ${PM_TABLE} present."
elif [[ -e "${PM_TABLE_LEGACY}" ]]; then
  log "SUCCESS: ${MODULE} loaded; legacy ${PM_TABLE_LEGACY} present (daemon prefers ${PM_TABLE})."
else
  die "pm_table missing after load (looked for ${PM_TABLE}, then ${PM_TABLE_LEGACY}) — run 'dmesg | tail' + 'dkms status' to inspect; without the module the app works (N/A DriverMissing, exit 0, no panic)"
fi
log "Next: start the ramsleuth daemon; ground-truth CLI is now at /usr/bin/monitor_cpu."
