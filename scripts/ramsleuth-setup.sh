#!/usr/bin/env bash
#
# ramsleuth-setup.sh — pkexec-able root setup helper (Cycle 21, C21-01).
#
# The single privileged entrypoint the one-click flow funnels into (plan
# PLAN-CYCLE21-STREAMLINE-INSTALL.md, decisions A/B/D): runs as root — via
# `pkexec ramsleuth-setup` from the GUI (polkit policy, C21-02) or
# `sudo ramsleuth-setup` from a terminal; a bare invocation re-execs under
# sudo — and performs every privileged step the fresh-install flow needs in
# one session.
#
# Every run is SELF-HEALING + IDEMPOTENT: before the steps below, the known
# stale state a prior partial/failed run can leave behind is detected and
# cleared — ONLY RamSleuth-owned state (the `ramsleuth` group, the
# `ramsleuth.service` unit, /etc/ramsleuth/authorized-users, the RamSleuth
# socket ACL, the polkit action; the DKMS module state is healed by the
# vendor helper this script delegates to — its registrations are removed
# before re-add, its bundled/offline source is primary since v2.4.6). A
# re-run therefore ALWAYS succeeds (no manual cleanup, no hoops), including
# after a failed 2.4.5-era run followed by an uninstall + 2.4.6 install.
# Any failure prints a structured message (the step, the command, and the
# reason — on stderr, the last line, so the GUI can show the real cause)
# + exits non-zero; never a silent half-state.
#
# Self-heal (runs on every invocation, before the steps):
#   0a. `ramsleuth.service` left in a FAILED state (a prior run died mid-
#       start, or the unit/daemon were out of sync during an uninstall +
#       reinstall) → `systemctl reset-failed` (a no-op when healthy).
#   0b. the `ramsleuth` group is ensured BEFORE the unit starts — the unit
#       runs as Group=ramsleuth, and a missing group fails the start (the
#       classic half-state that then wedges every later run).
#   0c. /etc/ramsleuth/authorized-users is normalized (blank lines dropped,
#       duplicates removed, our user ensured) by an atomic tmp+rename
#       rewrite — never append-duplicate.
#   0d. the daemon is restarted so the normalized state file is applied to a
#       FRESH socket (heals a stale socket/ACL; the daemon removes the
#       socket on clean shutdown and re-applies the ACLs on creation,
#       C21-03), and polkit is restarted so the branded
#       org.freedesktop.ramsleuth.setup action is live (heals a stale
#       action pool — the .install hook's restart is non-fatal and may
#       have failed).
#
# Steps:
#   1. `systemctl daemon-reload` + `enable --now ramsleuth.service` (hard —
#      the daemon is the point; the unit is installed by AUR/install.sh).
#   2. `usermod -aG ramsleuth <user>` — persisted for FUTURE logins
#      (skipped when the user is already a member; `usermod -aG` is
#      idempotent).
#   3. Authorize `<user>` in /etc/ramsleuth/authorized-users — the state
#      file the daemon re-applies as socket ACLs on every (re)start
#      (C21-03). (Normalized in 0c above.)
#   4. Best-effort `setfacl -m u:<user>:rw` on the LIVE socket — immediate
#      current-session access, no re-login (warn-only if acl is absent).
#   5. DKMS delegation (vendor-aware, INTEL-08): `--with-dkms` routes by the
#      CPU vendor (the /proc/cpuinfo `vendor_id`) — AMD: `exec` the installed
#      ramsleuth-install-ryzen-smu-dkms helper (the offline AMD driver; its
#      exit code is returned; unchanged behavior); Intel: `exec`
#      ramsleuth-install-intel-dkms (the INTEL-06 ramsleuth_intel helper).
#      `--with-intel-dkms` forces the Intel arm (Intel-only fast path).
#      Other/unknown vendor: a clear error, no DKMS install.
#
# Usage:  ramsleuth-setup [--with-dkms] [--with-intel-dkms] [--user <name>]
#
#   --user <name>  user to authorize. Default: $SUDO_USER (the sudo path).
#                  Under pkexec $SUDO_USER is unset, so the caller must pass
#                  it explicitly (the GUI does: setup_argv, C21-04). An
#                  unknown/nonexistent user is a usage error (exit 2).
#   --with-dkms    also build + load the pinned vendor DKMS module,
#                  auto-routed by the CPU vendor (AMD →
#                  /usr/bin/ramsleuth-install-ryzen-smu-dkms; Intel →
#                  /usr/bin/ramsleuth-install-intel-dkms; other/unknown → a
#                  clear error, no install). Never duplicated here.
#   --with-intel-dkms
#                  force the Intel arm: build + load the ramsleuth_intel
#                  module via /usr/bin/ramsleuth-install-intel-dkms
#                  (Intel-only fast path; a hard failure on non-Intel
#                  silicon).
#
# Exit codes:  0 = success (steps done, or idempotently skipped)
#              1 = hard failure (structured message on stderr)
#              2 = usage error (unknown flag / missing value / unknown user)
#              10 = (DKMS delegation only) passed through verbatim from the
#                   vendor helper: the Secure Boot one-time step is pending —
#                   the module is built + installed + signed and only the
#                   one-time MOK enrollment (reboot) is left; the GUI renders
#                   it as an amber "one step left" state, NOT a failure. The
#                   setup steps above only ever produce 0/1/2 themselves.
#
# FROZEN CONTRACT (C21-03/04/16 code against this — do not change):
#   CLI:  ramsleuth-setup [--with-dkms] [--with-intel-dkms] [--user <name>]
#         (any order; pre-INTEL-08 invocations keep their exact semantics —
#         --with-dkms on AMD behaves byte-identically to before;
#         --with-intel-dkms is an additive fast path)
#   Exit: 0 success | 1 hard failure | 2 usage, as above (the DKMS
#         delegation `exec`s the vendor helper, so its exit codes pass
#         through verbatim — including 10, the Secure Boot one-time step
#         pending; the setup steps above only ever produce 0/1/2).
#   State file: /etc/ramsleuth/authorized-users — dir 0755, file 0644, one
#   username per line (LF, no comments, no duplicates, order-insensitive).
#
set -euo pipefail

GROUP="ramsleuth"
UNIT="ramsleuth.service"
STATE_DIR="/etc/ramsleuth"
STATE_FILE="${STATE_DIR}/authorized-users"
SOCKET="/run/ramsleuth/ramsleuth.sock"
DKMS_HELPER="/usr/bin/ramsleuth-install-ryzen-smu-dkms"
INTEL_DKMS_HELPER="/usr/bin/ramsleuth-install-intel-dkms"

log() { printf '[ramsleuth-setup] %s\n' "$*"; }
die() { local code="${2:-1}"; printf '[ramsleuth-setup] ERROR: %s\n' "$1" >&2; exit "${code}"; }

# --- CPU vendor detection (the /proc/cpuinfo `vendor_id` line) ----------------
# Prints GenuineIntel / AuthenticAMD / the raw vendor string, or nothing when
# undetectable (no /proc/cpuinfo). INTEL-08: the key for the vendor-aware
# DKMS routing.
detect_cpu_vendor() {
  [[ -r /proc/cpuinfo ]] || return 0
  awk -F': *' '/^vendor_id/ { print $2; exit }' /proc/cpuinfo 2>/dev/null || true
}

usage() {
  cat <<'EOF'
ramsleuth-setup — one-click privileged RamSleuth setup (root via pkexec/sudo)

Usage: ramsleuth-setup [--with-dkms] [--with-intel-dkms] [--user <name>]

  --user <name>  user to authorize (default: $SUDO_USER; under pkexec the
                 caller must pass it explicitly). Must be an existing user.
  --with-dkms    additionally build + load the pinned vendor DKMS module,
                 auto-routed by the CPU vendor: AMD → ryzen_smu via
                 /usr/bin/ramsleuth-install-ryzen-smu-dkms; Intel →
                 ramsleuth_intel via /usr/bin/ramsleuth-install-intel-dkms
                 (other/unknown vendor: a clear error, no install).
  --with-intel-dkms
                 force the Intel DKMS arm (Intel-only fast path; a hard
                 failure on non-Intel silicon).
  -h, --help     show this help and exit.

Exit codes: 0 = success (or idempotent no-op) | 1 = hard failure | 2 = usage
EOF
}

# --- Argument parsing (the frozen CLI) ----------------------------------------
WITH_DKMS=0
WITH_INTEL_DKMS=0
USER_OVERRIDE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-dkms) WITH_DKMS=1 ;;
    --with-intel-dkms) WITH_INTEL_DKMS=1 ;;
    --user)
      [[ $# -ge 2 ]] || die "--user requires a value (see --help)" 2
      USER_OVERRIDE="$2"
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" 2 ;;
  esac
  shift
done

# --- Privilege: re-exec under sudo if not root (sudo is the floor; the pkexec
# --- GUI path arrives already root — polkit is a convenience, never required) ---
if [[ "$(id -u)" -ne 0 ]]; then
  command -v sudo >/dev/null 2>&1 || die "not root and sudo unavailable — invoke via pkexec/sudo" 2
  log "Not root — re-running under sudo (your prompt appears below)..."
  exec sudo "$0" "$@"
fi

# --- Resolve the user to authorize: explicit --user > $SUDO_USER > error ------
USER_NAME="${USER_OVERRIDE:-${SUDO_USER:-}}"
[[ -n "$USER_NAME" && "$USER_NAME" != "root" ]] \
  || die "cannot determine the invoking user (no --user, no SUDO_USER, or root) — pass --user <name>" 2
id -u "$USER_NAME" >/dev/null 2>&1 || die "unknown user: $USER_NAME" 2
# Keep the line-oriented state file clean (sanity gate on user-supplied input).
[[ "$USER_NAME" =~ ^[A-Za-z_][A-Za-z0-9_-]*$ ]] || die "invalid username: $USER_NAME" 2

# --- Self-heal 0a: a unit left in a FAILED state (a prior partial/failed run,
# --- or the unit/daemon out of sync during an uninstall + reinstall) --------
# `systemctl enable --now` below would fail on it; reset-failed clears the
# failed flag. A no-op when the unit is healthy (or the file is absent).
if systemctl is-failed "$UNIT" 2>/dev/null; then
  log "self-heal: $UNIT was in a FAILED state — clearing it (systemctl reset-failed)"
  systemctl reset-failed "$UNIT" 2>/dev/null || true
fi

# --- Self-heal 0b: the group must exist BEFORE the unit starts — the unit
# --- runs as Group=ramsleuth; a missing group fails the start (and leaves
# --- the failed state from 0a). Created idempotently (getent guard).
if getent group "$GROUP" >/dev/null 2>&1; then
  log "group: '$GROUP' already present"
else
  groupadd -r "$GROUP" || die "group: groupadd -r $GROUP failed" 1
  log "group: '$GROUP' created (system group)"
fi

# --- Step 1: daemon enabled + started (hard — the daemon is the point) --------
systemctl daemon-reload 2>/dev/null || true
systemctl enable --now "$UNIT" \
  || die "systemd: systemctl enable --now $UNIT failed — is the unit installed? (AUR/install.sh install it)" 1
log "systemd: $UNIT enabled + running"

# --- Step 2: persisted group membership (idempotent — usermod -aG is a
# --- no-op-safe add; skipped when the user is already a member) --------------
if id -nG "$USER_NAME" | tr ' ' '\n' | grep -qx "$GROUP"; then
  log "group: $USER_NAME already a member of '$GROUP' (skipped)"
else
  usermod -aG "$GROUP" "$USER_NAME" || die "group: usermod -aG $GROUP $USER_NAME failed" 1
  log "group: $USER_NAME added to '$GROUP' (effective at next login; the current session is covered by the ACL)"
fi

# --- Step 3: the B state file — self-healing OVERWRITE (the daemon's
# --- persistence anchor, re-applied on every socket creation — C21-03) -------
# A prior partial run may have left a stale file (duplicated lines, blank
# lines, users from a previous install). We never append-duplicate: keep
# every existing valid user, ensure ours, drop duplicates, and write
# atomically (tmp + rename inside the same dir).
if [[ ! -d "$STATE_DIR" ]]; then
  install -d -m 0755 "$STATE_DIR" || die "state: cannot create $STATE_DIR" 1
fi
TMP_STATE="$(mktemp "${STATE_DIR}/.authorized-users.tmp.XXXXXX")" \
  || die "state: cannot create a temp file in $STATE_DIR" 1
{
  if [[ -f "$STATE_FILE" ]]; then
    grep -vE '^[[:space:]]*$' "$STATE_FILE" || true
  fi
  printf '%s\n' "$USER_NAME"
} | awk 'NF && !seen[$0]++' > "$TMP_STATE" \
  || { rm -f "$TMP_STATE"; die "state: cannot normalize $STATE_FILE" 1; }
mv -f "$TMP_STATE" "$STATE_FILE" || die "state: cannot write $STATE_FILE" 1
chmod 0644 "$STATE_FILE" 2>/dev/null || true
log "state: $STATE_FILE normalized (blanks/duplicates dropped; $USER_NAME authorized)"

# --- Self-heal 0d: restart the daemon so the normalized state file is applied
# --- to a FRESH socket — heals a stale socket/ACL left by a prior partial run
# --- (the daemon removes the socket on clean shutdown and re-applies the
# --- ACLs from the state file on creation, C21-03). Warn-only: a failed
# --- restart leaves the previous socket in place (the state file persists).
if systemctl restart "$UNIT" 2>/dev/null; then
  log "self-heal: $UNIT restarted — the socket was re-created with the normalized ACLs"
else
  log "WARN: could not restart $UNIT to re-apply the socket ACLs — they take effect on the daemon's next (re)start"
fi

# --- Step 4: immediate current-session access — best-effort ACL on the live
# --- socket (warn-only: no acl package / no socket yet is not fatal — the
# --- daemon re-applies the ACL from the state file on its next socket creation)
if ! command -v setfacl >/dev/null 2>&1; then
  log "WARN: setfacl not found (install the 'acl' package) — current-session access waits for the daemon's next (re)start; re-login works via the group"
elif [[ ! -S "$SOCKET" ]]; then
  log "WARN: socket $SOCKET not present — the daemon applies the ACL at (re)start; the state file is written"
elif setfacl -m "u:${USER_NAME}:rw" "$SOCKET" 2>/dev/null; then
  log "acl: $USER_NAME granted rw on $SOCKET — the current session has immediate access (no re-login)"
else
  log "WARN: setfacl on $SOCKET failed — the daemon re-applies the ACL on its next socket creation"
fi

# --- Self-heal 0e: polkit restart (idempotent) — a polkitd that started before
# --- the policy was (re)installed may serve a STALE action pool (its inotify
# --- hot-reload is unreliable; observed 2026-09-25), which makes the GUI's
# --- one-click prompt show the generic dialog. The .install hook already tries
# --- this, but non-fatally; doing it here — inside the privileged run — heals
# --- it. Guarded: it never fails the setup.
if systemctl restart polkit 2>/dev/null; then
  log "polkit: restarted — the branded org.freedesktop.ramsleuth.setup action is live"
else
  log "WARN: polkit restart failed (non-fatal) — if the GUI's setup prompt shows the generic polkit dialog, run: systemctl restart polkit (or reboot)"
fi

# --- Summary + the optional DKMS delegation (vendor-aware: INTEL-08) ----------
log "done: daemon running; $USER_NAME in group '$GROUP' + authorized for immediate access"
if [[ "$WITH_DKMS" -ne 1 && "$WITH_INTEL_DKMS" -ne 1 ]]; then
  exit 0
fi

# Intel arm: delegate to the ramsleuth_intel DKMS helper (INTEL-06), resolving
# it with the same three-tier chain as the AMD arm (installed path → PATH →
# dev checkout). The helper is never duplicated here.
run_intel_dkms() {
  local HELPER CANDIDATE
  HELPER="$INTEL_DKMS_HELPER"
  if [[ ! -x "$HELPER" ]]; then
    HELPER="$(command -v ramsleuth-install-intel-dkms 2>/dev/null || true)"
  fi
  if [[ -z "$HELPER" ]]; then
    # Dev-checkout fallback (this script lives in <repo>/scripts; uninstalled).
    CANDIDATE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/install-intel-dkms.sh"
    if [[ -x "$CANDIDATE" ]]; then HELPER="$CANDIDATE"; fi
  fi
  [[ -n "$HELPER" && -x "$HELPER" ]] \
    || die "Intel: helper ramsleuth-install-intel-dkms not found ($INTEL_DKMS_HELPER) — install RamSleuth first" 1
  log "Intel: delegating the ramsleuth_intel DKMS build to: $HELPER"
  exec "$HELPER"
}

CPU_VENDOR="$(detect_cpu_vendor)"
if [[ "$WITH_INTEL_DKMS" -eq 1 && "$CPU_VENDOR" != "GenuineIntel" ]]; then
  die "--with-intel-dkms is an Intel-only fast path: this host's CPU vendor is '${CPU_VENDOR:-undetectable}', not GenuineIntel" 1
fi

case "$CPU_VENDOR" in
  GenuineIntel)
    run_intel_dkms
    ;;
  AuthenticAMD)
    HELPER="$DKMS_HELPER"
    if [[ ! -x "$HELPER" ]]; then
      HELPER="$(command -v ramsleuth-install-ryzen-smu-dkms 2>/dev/null || true)"
    fi
    if [[ -z "$HELPER" ]]; then
      # Dev-checkout fallback (this script lives in <repo>/scripts; uninstalled).
      CANDIDATE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/install-ryzen-smu-dkms.sh"
      if [[ -x "$CANDIDATE" ]]; then HELPER="$CANDIDATE"; fi
    fi
    [[ -n "$HELPER" && -x "$HELPER" ]] \
      || die "AMD: helper ramsleuth-install-ryzen-smu-dkms not found ($DKMS_HELPER) — install RamSleuth first" 1
    log "AMD: delegating the ryzen_smu DKMS build to: $HELPER"
    exec "$HELPER"
    ;;
  *)
    die "no RamSleuth DKMS module applies to CPU vendor '${CPU_VENDOR:-undetectable}' (AMD/Intel only) — re-run without the DKMS flag" 1
    ;;
esac
