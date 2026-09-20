#!/usr/bin/env bash
#
# ramsleuth-setup.sh — pkexec-able root setup helper (Cycle 21, C21-01).
#
# The single privileged entrypoint the one-click flow funnels into (plan
# PLAN-CYCLE21-STREAMLINE-INSTALL.md, decisions A/B/D): runs as root — via
# `pkexec ramsleuth-setup` from the GUI (polkit policy, C21-02) or
# `sudo ramsleuth-setup` from a terminal; a bare invocation re-execs under
# sudo — and performs every privileged step the fresh-install flow needs in
# one session. Each step is idempotent (a re-run is a no-op); any failure
# prints a structured message + exits non-zero, never a silent half-state.
#
# Steps:
#   1. `systemctl daemon-reload` + `enable --now ramsleuth.service` (the unit
#      + group are installed by AUR/install.sh; the group is created
#      defensively if missing).
#   2. `usermod -aG ramsleuth <user>` — persisted for FUTURE logins (skipped
#      when the user is already a member).
#   3. Append `<user>` to /etc/ramsleuth/authorized-users — the state file the
#      daemon re-applies as socket ACLs on every (re)start (C21-03).
#   4. Best-effort `setfacl -m u:<user>:rw` on the LIVE socket — immediate
#      current-session access, no re-login (warn-only if acl is absent).
#   5. `--with-dkms`: `exec` the installed ramsleuth-install-ryzen-smu-dkms
#      helper (the offline AMD driver; its exit code is returned).
#
# Usage:  ramsleuth-setup [--with-dkms] [--user <name>]
#
#   --user <name>  user to authorize. Default: $SUDO_USER (the sudo path).
#                  Under pkexec $SUDO_USER is unset, so the caller must pass
#                  it explicitly (the GUI does: setup_argv, C21-04). An
#                  unknown/nonexistent user is a usage error (exit 2).
#   --with-dkms    also build + load the pinned ryzen_smu AMD module by
#                  delegating to /usr/bin/ramsleuth-install-ryzen-smu-dkms
#                  (never duplicated here).
#
# Exit codes:  0 = success (steps done, or idempotently skipped)
#              1 = hard failure (structured message on stderr)
#              2 = usage error (unknown flag / missing value / unknown user)
#
# FROZEN CONTRACT (C21-03/04/16 code against this — do not change):
#   CLI:  ramsleuth-setup [--with-dkms] [--user <name>]   (any order)
#   Exit: 0 success | 1 hard failure | 2 usage, as above.
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

log() { printf '[ramsleuth-setup] %s\n' "$*"; }
die() { local code="${2:-1}"; printf '[ramsleuth-setup] ERROR: %s\n' "$1" >&2; exit "${code}"; }

usage() {
  cat <<'EOF'
ramsleuth-setup — one-click privileged RamSleuth setup (root via pkexec/sudo)

Usage: ramsleuth-setup [--with-dkms] [--user <name>]

  --user <name>  user to authorize (default: $SUDO_USER; under pkexec the
                 caller must pass it explicitly). Must be an existing user.
  --with-dkms    additionally build + load the pinned ryzen_smu AMD module
                 (delegates to /usr/bin/ramsleuth-install-ryzen-smu-dkms).
  -h, --help     show this help and exit.

Exit codes: 0 = success (or idempotent no-op) | 1 = hard failure | 2 = usage
EOF
}

# --- Argument parsing (the frozen CLI) ----------------------------------------
WITH_DKMS=0
USER_OVERRIDE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-dkms) WITH_DKMS=1 ;;
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

# --- Step 1: daemon enabled + started (hard — the daemon is the point) --------
systemctl daemon-reload 2>/dev/null || true
systemctl enable --now "$UNIT" \
  || die "systemctl enable --now $UNIT failed — is the unit installed? (AUR/install.sh install it)" 1
log "daemon: $UNIT enabled + running"

# --- Step 2: persisted group membership (idempotent) ---------------------------
if getent group "$GROUP" >/dev/null 2>&1; then
  log "group: '$GROUP' already present"
else
  groupadd -r "$GROUP" || die "groupadd -r $GROUP failed" 1
  log "group: '$GROUP' created (system group)"
fi
if id -nG "$USER_NAME" | tr ' ' '\n' | grep -qx "$GROUP"; then
  log "group: $USER_NAME already a member of '$GROUP' (skipped)"
else
  usermod -aG "$GROUP" "$USER_NAME" || die "usermod -aG $GROUP $USER_NAME failed" 1
  log "group: $USER_NAME added to '$GROUP' (effective at next login; the current session is covered by the ACL)"
fi

# --- Step 3: the B state file (idempotent append; the daemon's persistence
# --- anchor, re-applied on every socket creation — C21-03) --------------------
if [[ ! -d "$STATE_DIR" ]]; then
  install -d -m 0755 "$STATE_DIR" || die "cannot create $STATE_DIR" 1
fi
if [[ ! -f "$STATE_FILE" ]]; then
  install -m 0644 /dev/null "$STATE_FILE" || die "cannot create $STATE_FILE" 1
fi
if grep -qxF "$USER_NAME" "$STATE_FILE"; then
  log "state: $USER_NAME already in $STATE_FILE (skipped)"
else
  printf '%s\n' "$USER_NAME" >> "$STATE_FILE" || die "cannot append $USER_NAME to $STATE_FILE" 1
  log "state: $USER_NAME appended to $STATE_FILE"
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

# --- Summary + the optional DKMS delegation ------------------------------------
log "done: daemon running; $USER_NAME in group '$GROUP' + authorized for immediate access"
if [[ "$WITH_DKMS" -ne 1 ]]; then
  exit 0
fi
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
