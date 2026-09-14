#!/usr/bin/env bash
#
# amd-ground-truth.sh — AMD ground-truth cross-check, matched conditions (P6-01).
#
# Cross-checks the ryzen_smu reference tool (`monitor_cpu`) against the
# ramsleuth stack (`ramsleuth-client dump` on the root daemon) under
# matched conditions (idle, short settle). Captures run SEQUENTIALLY — the
# driver's `smn_result` is one shared global, so never run `monitor_cpu`
# concurrently with the daemon's SMN reads (plan D3):
#
#   (a) monitor_cpu        — one PM-table frame (timeout 2)
#   (b) ramsleuth-client   — dump (dashboard, root daemon)
#   (c) monitor_cpu -m     — one-shot SMN memory timings (the reference)
#
# Gated (a gate is active only when its dump cell holds a value):
#   PM clocks MCLK / UCLK / FCLK : ±1 MHz   (frame vs dump)
#   VDDCR_SOC                    : ±10 mV   (frame vs dump)
#   DRAM timings (27)            : ±1 tick  (dump vs `monitor_cpu -m`;
#                                          an N/A cell is deferred, not failed)
# Recorded (informational, never gated):
#   CAD bus (8 cells)            : populated values recorded; still-N/A cells
#                                  note the deferred gate (SMN CAD bitfields
#                                  unconfirmed until P6-02/03)
#   1792-vs-1800 MCLK delta      : set-point register 0x50200[6:0] via the
#                                  driver's `smn` attr + both MCLK sources +
#                                  the raw PM f32 @0x0CC
#
# Precondition misses and capture failures that block a verdict exit 2 with
# an actionable message — never a partial/ambiguous verdict. A degraded
# (N/A) primary dump cell likewise yields exit 2 (no verdict).
#
# Exit codes: 0 = all active gates in tolerance (deferrals noted);
#             1 = any active gate out of tolerance;
#             2 = preconditions unmet / no verdict possible.

set -euo pipefail

TAG="amd-ground-truth"
SOCKET="/run/ramsleuth/ramsleuth.sock"
CLIENT=""
MONITOR="/usr/bin/monitor_cpu"
SETTLE=2
SYSFS_DIR=""
CLOCK_TOL_MHZ="1"   # ±1 MHz  (plan P6-01; handover §5.1)
VOLT_TOL_V="0.010"  # ±10 mV
TICK_TOL="1"        # ±1 tick

# The 27 dump timing keys -> `monitor_cpu -m` reference labels (the
# `print_memory_timings()` field list, plan §1).
TIMING_PAIRS=(
  "tCL:Tcl" "tRCDWR:Trcdwr" "tRCDRD:Trcdrd" "tRP:Trp" "tRAS:Tras" "tRC:Trc" "tRRDS:Trrds" "tRRLD:Trrdl" "tFAW:Tfaw"
  "tWTRS:Twtrs" "tWTRL:Twtrl" "tWR:Twr" "tRFC1:Trfc" "tRFC2:Trfc2" "tRFCsb:Trfc4" "tCWL:Tcwl" "tRTP:Trtp" "tRDWR:Trdwr"
  "tWRRD:Twrrd" "tRDRD(SD):Trdrdsd" "tRDRD(CCD):Trdrddd" "tRDRD(SCL):Trdrdscl" "tRDRD(SC):Trdrdsc" "tWRWR(SD):Twrwrsd"
  "tWRWR(CCD):Twrwrdd" "tWRWR(SCL):Twrwrscl" "tWRWR(SC):Twrwrsc"
)
# The 8 dump CAD keys (frozen CadBus row names, ramsleuth-client dump.rs).
CAD_KEYS=("proc ODT" "RTT nom" "RTT wr" "RTT park" "CLK drive" "ADD/CMD drive" "CS/ODT drive" "CKE drive")

log()  { printf '[%s] %s\n' "$TAG" "$*"; }
die2() { printf '[%s] ERROR: %s\n' "$TAG" "$*" >&2; exit 2; }
usage() {
  printf 'Usage: %s [--socket <path>] [--client <path>] [--monitor <path>] [--settle <secs>] [-h]\n' "$(basename "$0")"
  printf '  AMD ground-truth cross-check under matched conditions. Exit: 0 in tolerance; 1 out; 2 preconditions / no verdict.\n'
}

# --- Argument parsing --------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket)  [[ $# -ge 2 ]] || die2 "--socket needs a value";  SOCKET="$2";  shift 2 ;;
    --client)  [[ $# -ge 2 ]] || die2 "--client needs a value";  CLIENT="$2";  shift 2 ;;
    --monitor) [[ $# -ge 2 ]] || die2 "--monitor needs a value"; MONITOR="$2"; shift 2 ;;
    --settle)  [[ $# -ge 2 ]] || die2 "--settle needs a value";  SETTLE="$2";  shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die2 "unknown option: $1" ;;
  esac
done
[[ "$SETTLE" =~ ^[0-9]+$ ]] || die2 "invalid --settle value: ${SETTLE} (expected whole seconds)"

# Scratch dir (temp captures only — never ramsleuth state).
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# --- Small helpers -----------------------------------------------------------
# First numeric token of a string (e.g. "1792.00 MHz" -> 1792.00); empty if none.
first_number() { printf '%s\n' "$1" | grep -oE '[0-9]+(\.[0-9]+)?' | head -n 1 || true; }
# Trim surrounding whitespace (pure bash).
trim() { local x="$1"; x="${x#"${x%%[![:space:]]*}"}"; x="${x%"${x##*[![:space:]]}"}"; printf '%s' "$x"; }
# 0 if |$1 - $2| <= $3; 2 if either side is not a plain number (no-verdict).
fcmp_within() { awk -v a="$1" -v b="$2" -v t="$3" 'BEGIN{if(a!~/^[0-9.]+$/||b!~/^[0-9.]+$/)exit 2; d=a-b; if(d<0)d=-d; exit (d<=t+1e-6)?0:1}'; }
# A monitor_cpu frame value by exact label (last occurrence = most recent
# frame), from $WORK/pm.txt (ANSI/box-drawing stripped; rows are `| <label> | <value> |`).
pm_field() { awk -F'|' -v l="$1" '{x=$2; gsub(/^[ \t]+|[ \t]+$/,"",x); if(x==l){v=$3; gsub(/^[ \t]+|[ \t]+$/,"",v); last=v}} END{if(last!="")print last}' "$WORK/pm.txt"; }
# One ramsleuth-dump AMD-section row: prints `VAL<TAB><rest>` or
# `NA<TAB><reason>`, or nothing when the key is absent. Dump rows are
# `  <key padded>  <value>` (dump.rs rows()); keys may be multi-word
# ("proc ODT") and carry regex metacharacters ("tRDRD(SD)"), so the match
# is a plain string compare, never a regex.
dump_field() { awk -v k="$1" '/^=== AMD ===$/{a=1;next} /^=== [A-Z]/{a=0} a&&!d&&substr($0,1,2)=="  "&&substr($0,3,length(k))==k&&substr($0,3+length(k),1)==" "{r=substr($0,4+length(k)); sub(/^[ \t]+/,"",r); sub(/[ \t]+$/,"",r); d=1; if(r~/^N\/A/){sub(/^N\/A[ \t]*/,"",r); print "NA\t" r} else print "VAL\t" r}' "$WORK/dump.txt"; }
# One `monitor_cpu -m` label value ("Tcl: 16" -> 16), from $WORK/smn.txt.
smn_field() { awk -v k="$1" 'index($0,k": ")==1{v=substr($0,length(k)+3); sub(/[ \t]+$/,"",v); print v; exit}' "$WORK/smn.txt"; }
# A driver text attr: trimmed value, or (absent).
attr() { local f="${SYSFS_DIR}/$1" v=""; [[ -r "$f" ]] && v="$(cat "$f" 2>/dev/null || true)"; printf '%s' "${v:-(absent)}"; }
# Read one SMN register: $1 = the 4-byte LE address as printf-\x text
# (e.g. '\x00\x02\x50\x00' = 0x00050200) -> "0x<big-endian hex> <decimal>".
smn_read_word() {
  local f="${SYSFS_DIR}/smn" h
  { printf '%b' "$1" >"$f"; } 2>/dev/null || return 1
  h="$(od -An -tx1 -N4 "$f" 2>/dev/null | tr -d ' \n' || true)"; [[ "${#h}" -eq 8 ]] || return 1
  printf '0x%s %s' "${h:6:2}${h:4:2}${h:2:2}${h:0:2}" "$((16#${h:6:2}${h:4:2}${h:2:2}${h:0:2}))"
}

# --- Preconditions (exit 2 with an actionable message) -----------------------
[[ "$(id -u)" -eq 0 ]] || die2 "must run as root (monitor_cpu is root-only; the driver smn attr is root-writable)"
[[ -x "$MONITOR" ]] || die2 "monitor_cpu not found at ${MONITOR} (install the ryzen_smu userspace tool, or pass --monitor)"
[[ -n "$CLIENT" ]] || CLIENT="$(command -v ramsleuth-client || true)"
[[ -n "$CLIENT" && -x "$CLIENT" ]] || die2 "ramsleuth-client not found (build/install ramsleuth, or pass --client <path>)"
[[ -S "$SOCKET" ]] || die2 "daemon socket ${SOCKET} not present — start the root daemon (systemd ramsleuth, or ramsleuth-daemon) and re-run"
status_rc=0; "$CLIENT" status --socket "$SOCKET" >"$WORK/probe.txt" 2>&1 || status_rc=$?
[[ "$status_rc" -eq 0 ]] || die2 "daemon unreachable over ${SOCKET} (status probe rc=${status_rc}: $(head -n 1 "$WORK/probe.txt" 2>/dev/null || true))"
# Driver attrs: canonical ryzen_smu_drv kobject first, legacy ryzen_smu path
# second (the verified-kobject-first rule; older module builds).
for d in /sys/kernel/ryzen_smu_drv /sys/kernel/ryzen_smu; do [[ -e "${d}/pm_table" && -e "${d}/smn" ]] && { SYSFS_DIR="$d"; break; }; done
[[ -n "$SYSFS_DIR" ]] || die2 "ryzen_smu driver attrs not found (need pm_table + smn under /sys/kernel/ryzen_smu_drv or /sys/kernel/ryzen_smu — is the module loaded?)"

# --- Driver context (record-only; degrades, never fatal) ---------------------
codename="$(attr codename)"; drv_version="$(attr drv_version)"; smu_fw="$(attr version)"
pm_ver="(unreadable)"
pm_ver_raw="$(od -An -tx1 -N4 "${SYSFS_DIR}/pm_table_version" 2>/dev/null | tr -d ' \n' || true)"  # raw 4-byte LE u32 (P5-15)
[[ "${#pm_ver_raw}" -eq 8 ]] && pm_ver="0x${pm_ver_raw:6:2}${pm_ver_raw:4:2}${pm_ver_raw:2:2}${pm_ver_raw:0:2}"
log "preconditions OK: root, monitor_cpu=${MONITOR}, client=${CLIENT}, daemon=${SOCKET}, sysfs=${SYSFS_DIR}"
log "driver context: codename=${codename} drv_version=${drv_version} smu_fw=${smu_fw} pm_table_version=${pm_ver}"
log "daemon probe: $(grep -m 1 '^AMD:' "$WORK/probe.txt" || echo 'AMD: ok')"

# --- Matched-condition capture (sequential, short idle settle) ---------------
log "settling ${SETTLE}s (idle), then capturing sequentially (monitor_cpu -> dump -> monitor_cpu -m)..."
sleep "$SETTLE"
# (a) one monitor_cpu PM-table frame: `timeout 2` sends SIGTERM; the
# monitor's handler exits cleanly 0. Label values use the LAST occurrence
# (the most recent frame).
mon_rc=0; timeout 2 "$MONITOR" >"$WORK/mon.raw" 2>"$WORK/mon.err" || mon_rc=$?
[[ "$mon_rc" -eq 0 ]] || die2 "monitor_cpu PM-frame capture failed (rc=${mon_rc}: $(head -n 1 "$WORK/mon.err" 2>/dev/null || true))"
# Strip ANSI escapes; map box char U+2502 (E2 94 82) to '|'; drop remaining
# non-ASCII (corners) — the frame rows become `| <label> | <value> |` (GNU sed).
sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\xe2\x94\x82/|/g' <"$WORK/mon.raw" | LC_ALL=C tr -d '\200-\377' >"$WORK/pm.txt"
# (b) the ramsleuth dashboard from the root daemon.
dump_rc=0; "$CLIENT" dump --socket "$SOCKET" >"$WORK/dump.txt" 2>&1 || dump_rc=$?
[[ "$dump_rc" -eq 0 ]] || die2 "ramsleuth-client dump failed (rc=${dump_rc}: $(head -n 1 "$WORK/dump.txt" 2>/dev/null || true))"
# (c) one-shot SMN memory timings (the reference for the timing gate).
smn_rc=0; "$MONITOR" -m >"$WORK/smn.txt" 2>&1 || smn_rc=$?
[[ "$smn_rc" -eq 0 ]] || log "NOTE: monitor_cpu -m failed (rc=${smn_rc}: $(head -n 1 "$WORK/smn.txt" 2>/dev/null || true)) — no SMN reference; any active timing gate becomes no-verdict"

# --- One value gate: dump cell vs monitor_cpu frame label ---------------------
# A gate is active only when the dump cell holds a value; an absent or N/A
# primary cell is a no-verdict (the cross-check cannot run on that gate).
# The AMD section is either fully populated or a single `N/A (<reason>)` line
# (dump.rs) — in the latter case no dump-derived gate can run: one no-verdict
# note instead of one per gate. The SMN/PM-table delta record below still runs.
no_verdict=0; pass_count=0; fail_count=0
amd_na="$(awk '/^=== AMD ===$/{a=1;next} /^=== [A-Z]/{a=0} a&&!s{if($0!~/^[ \t]*$/){s=1; if($0~/N\/A/){print; exit}}}' "$WORK/dump.txt")"
if [[ -n "$amd_na" ]]; then
  log "AMD section degraded: $(trim "$amd_na") — the PM clock / voltage / timing / CAD gates cannot run (no verdict)"
  no_verdict=1
else
  gate_value() { # $1 name, $2 dump key, $3 pm label, $4 tolerance, $5 unit
    local name="$1" key="$2" label="$3" tol="$4" unit="$5" ds mc rc
    ds="$(dump_field "$key")"; mc="$(pm_field "$label")"
    [[ -n "$ds" ]] || { log "  ${name}: NO-VERDICT (no ${key} cell in the dump's AMD section — branch degraded?)"; no_verdict=1; return 0; }
    [[ "$ds" != NA* ]] || { log "  ${name}: NO-VERDICT (dump ${key} = N/A $(trim "${ds#NA}"))"; no_verdict=1; return 0; }
    [[ -n "$mc" ]] || { log "  ${name}: NO-VERDICT (no '${label}' row in the monitor_cpu frame)"; no_verdict=1; return 0; }
    rc=0; fcmp_within "$(first_number "$mc")" "$(first_number "${ds#VAL}")" "$tol" || rc=$?
    [[ "$rc" -ne 2 ]] || { log "  ${name}: NO-VERDICT (unparseable: dump='$(trim "${ds#VAL}")' monitor_cpu='${mc}')"; no_verdict=1; return 0; }
    if [[ "$rc" -eq 0 ]]; then log "  ${name}: PASS  monitor_cpu=${mc}  dump=$(trim "${ds#VAL}")  (tol ±${tol} ${unit})"; pass_count=$((pass_count + 1))
    else log "  ${name}: FAIL  monitor_cpu=${mc}  dump=$(trim "${ds#VAL}")  (tol ±${tol} ${unit})"; fail_count=$((fail_count + 1)); fi
    return 0
  }
  log "--- gates: PM clocks (±${CLOCK_TOL_MHZ} MHz) + VDDCR_SOC (±10 mV) ---"
  gate_value MCLK MCLK "Memory Clock" "$CLOCK_TOL_MHZ" MHz
  gate_value UCLK UCLK "Uncore Clock" "$CLOCK_TOL_MHZ" MHz
  gate_value FCLK FCLK "Fabric Clock" "$CLOCK_TOL_MHZ" MHz
  gate_value VDDCR_SOC VDDCR_SOC "VDDCR_SoC" "$VOLT_TOL_V" V

  # --- The 27 DRAM timings, ±1 tick (dump vs monitor_cpu -m) --------------------
  log "--- gates: DRAM timings (±${TICK_TOL} tick; N/A cells deferred) ---"
  t_active=0; t_deferred=0; t_noref=0
  for pair in "${TIMING_PAIRS[@]}"; do
    key="${pair%%:*}"; ref="${pair#*:}"; ds="$(dump_field "$key")"
    [[ -n "$ds" ]] || { log "  ${key}: NO-VERDICT (absent from the dump's AMD section)"; no_verdict=1; continue; }
    [[ "$ds" != NA* ]] || { t_deferred=$((t_deferred + 1)); continue; }
    t_active=$((t_active + 1)); rv=""; [[ "$smn_rc" -eq 0 ]] && rv="$(smn_field "$ref" || true)"
    [[ -n "$rv" ]] || { t_noref=$((t_noref + 1)); log "  ${key}: NO-REFERENCE (active, but no monitor_cpu -m '${ref}' value)"; continue; }
    rc=0; fcmp_within "$(first_number "${ds#VAL}")" "$(first_number "$rv")" "$TICK_TOL" || rc=$?
    [[ "$rc" -ne 2 ]] || { t_noref=$((t_noref + 1)); log "  ${key}: NO-REFERENCE (unparseable: dump='$(trim "${ds#VAL}")' ref='${rv}')"; continue; }
    if [[ "$rc" -eq 0 ]]; then log "  ${key}: PASS  dump=$(trim "${ds#VAL}")  monitor_cpu -m ${ref}=${rv}"; pass_count=$((pass_count + 1))
    else log "  ${key}: FAIL  dump=$(trim "${ds#VAL}")  monitor_cpu -m ${ref}=${rv}  (tol ±${TICK_TOL} tick)"; fail_count=$((fail_count + 1)); fi
  done
  log "  timings: ${t_active} active, ${t_deferred} deferred (N/A), ${t_noref} active-without-reference"
  [[ "$t_noref" -eq 0 ]] || { log "  -> an active timing gate lacks its monitor_cpu -m reference: no verdict (exit 2)"; no_verdict=1; }

  # --- Record: CAD bus (informational only; the gate is deferred) ---------------
  log "--- record: CAD bus (informational; gate deferred) ---"
  cad_na=0
  for key in "${CAD_KEYS[@]}"; do
    ds="$(dump_field "$key")"
    if [[ -z "$ds" ]]; then log "  ${key}: (absent)"
    elif [[ "$ds" == NA* ]]; then log "  ${key}: N/A — $(trim "${ds#NA}")"; cad_na=1
    else log "  ${key}: $(trim "${ds#VAL}")"; fi
  done
  if [[ "$cad_na" -eq 1 ]]; then log "  CAD gate: DEFERRED — SMN CAD bitfields unconfirmed (lands with P6-02/03); non-fatal"
  else log "  CAD: populated values recorded above (no CAD reference exists in monitor_cpu -m)"; fi

fi
# --- Record: the 1792-vs-1800 MCLK delta (set-point vs measured) --------------
# The driver's `smn` attr is a write-address -> read-value protocol (plan
# §1). Address 0x50200 = LE bytes 00 02 50 00; when the first read returns
# 0x300 the platform offsets UMC registers by +0x100000 (the monitor_cpu
# rule -> 0x10050200 = bytes 00 02 50 10).
log "--- record: 1792-vs-1800 MCLK delta (set-point 0x50200[6:0] vs measured) ---"
setpoint_raw="$(smn_read_word '\x00\x02\x50\x00' || true)"
[[ "$setpoint_raw" == "0x00000300 768" ]] && setpoint_raw="$(smn_read_word '\x00\x02\x50\x10' || true)"
gdm_bit="?"; cr_text=""
if [[ -n "$setpoint_raw" ]]; then
  w="${setpoint_raw#* }"; sp=$(( (w & 127) * 100 / 3 )); gdm_bit=$(( (w >> 11) & 1 ))
  [[ $(( (w & 1024) >> 10 )) -eq 1 ]] && cr_text="2T" || cr_text="1T"
  log "  smn 0x50200 raw=${setpoint_raw%% *}: set-point=${sp} MHz ((raw & 0x7F)/3 x 100), GDM bit=${gdm_bit}, CR=${cr_text}"
  [[ "$sp" -gt 0 ]] || log "  (note: set-point 0 is implausible — likely a stale smn_result after a failed SMU read, plan D3)"
else
  log "  smn 0x50200 set-point: N/A (smn write/read failed — check the driver)"
fi
gdm_m=""; [[ "$smn_rc" -eq 0 ]] && gdm_m="$(smn_field GDM || true)"
dump_gdm="$(dump_field GDM || true)"
if [[ "$dump_gdm" == VAL* ]]; then dump_gdm_disp="$(trim "${dump_gdm#VAL}")"
elif [[ "$dump_gdm" == NA* ]]; then dump_gdm_disp="N/A $(trim "${dump_gdm#NA}")"
else dump_gdm_disp="n/a"; fi
log "  GDM: smn bit=${gdm_bit}  monitor_cpu -m=${gdm_m:-n/a}  dump=${dump_gdm_disp}"
log "  monitor_cpu PM-frame Memory Clock: $(pm_field "Memory Clock" || true)"
dump_mclk="$(dump_field MCLK || true)"
if [[ "$dump_mclk" == VAL* ]]; then dump_mclk_disp="$(trim "${dump_mclk#VAL}")"
elif [[ "$dump_mclk" == NA* ]]; then dump_mclk_disp="N/A $(trim "${dump_mclk#NA}")"
else dump_mclk_disp="n/a"; fi
log "  ramsleuth dump MCLK: ${dump_mclk_disp}"
pm_size=""; [[ -r "${SYSFS_DIR}/pm_table_size" ]] && pm_size="$(od -An -tu8 -N8 "${SYSFS_DIR}/pm_table_size" 2>/dev/null | tr -d ' \n' || true)"
if [[ -n "$pm_size" && "$pm_size" -ge 208 ]]; then
  pm_f32="$(od -An -tf4 -j 204 -N4 "${SYSFS_DIR}/pm_table" 2>/dev/null | awk '{print $1}' || true)"
  log "  PM table f32 @0x0CC (MEMCLK_FREQ): ${pm_f32:-N/A}"
else
  log "  PM table f32 @0x0CC: N/A (pm_table_size=${pm_size:-unreadable} < 208)"
fi
log "  -> classification: set-point register (discrete steps) vs measured PM f32 — plan P6-01 / handover §5.1"

# --- Verdict ------------------------------------------------------------------
if [[ "$no_verdict" -eq 1 ]]; then log "VERDICT: NO-VERDICT — at least one required gate could not be evaluated (see the NO-VERDICT lines)"; exit 2; fi
if [[ "$fail_count" -gt 0 ]]; then log "VERDICT: FAIL — ${fail_count} active gate(s) out of tolerance (${pass_count} in tolerance)"; exit 1; fi
log "VERDICT: PASS — ${pass_count} active gate(s) in tolerance (timing/CAD deferrals noted, non-fatal)"
exit 0
