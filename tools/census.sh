#!/usr/bin/env bash
# The census: does the patched client phone anybody? (CEN-3; docs/guides/census.md)
#
#   tools/census.sh --all                 every copy, every census scenario;
#                                         unattended; ends with
#                                         out/census/<utc>/SUMMARY.md
#   tools/census.sh --check               client-ci.sh's preflight for each
#                                         (copy, scenario); starts nothing
#   tools/census.sh --copies LIST --scenarios LIST
#
#   --copies LIST     DIR:ROLE,... ; ROLE is control, subject or noenv (a
#                     subject launched without the patcher's env, for the
#                     tier C proof; give it its own copy dir or the same one
#                     with a second run). Needs a control copy too. Default:
#                     tq-3662168-control:control,tq-offline:subject,
#                     tq-3619879:subject. The control's name follows the
#                     PINNED row of tools/builds.tsv (tq-<pinned>-control; the
#                     fallback above is 3662168), so a re-pin needs no edit
#                     here. It is a copy of the pinned build with only B0+P0,
#                     made from the STOCK import before any patch, never a
#                     copy of tq-offline, which is fully patched
#                     (tools/re-pin.sh refuses to swap without it). The control runs first; its
#                     report is what each subject is judged against.
#   --scenarios LIST  default: census_canary,census_idle,census_links,
#                     census_cc,census_hang,census_crash (packet CEN-4), and
#                     census_report_bug,census_urls,census_cdn (issue #40)
#   --noenv-scenarios LIST  the `noenv` role's own list (default
#                     census_canary,census_idle,census_cc): the launcher's env is
#                     what absorbs the other scenarios' names
#   --mode M          the sink's mode for every copy (observe|handshake).
#                     Default: handshake for the control (an IP-literal flow
#                     then shows its TLS SNI or Host), observe for subjects
#                     (the verdict runs; R-CENSUS §1.3)
#   --rootless        tools/jail-rootless.sh --census instead of the helper
#   --out DIR         default out/census/<utc>
#   -- ARGS...        passed to every client-ci.sh run (e.g. --no-build)
#
# Per copy, one RUN directory, every scenario appended to it by
# `client-ci.sh run SCEN --census RUN` (census-up, the canaries, the scenario,
# census-dump, census-down, verify again). Then
# `census_report.py RUN --role control` on the control, and
# `census_report.py RUN --role subject --control CONTROL/report.json` on each
# subject. Exit status, the worst over the copies:
#   0 V1/V2/CONTROL   1 FAIL   2 INVALID   3 INCONCLUSIVE   4 usage/refused
#
# The run needs: the helper reinstalled with the census verbs and the four
# sudoers lines (docs/guides/census.md, "Running it"), or --rootless; the copies in
# the offline prefix; and out/census/paths.json (without it every subject is
# INCONCLUSIVE). Nothing here needs root itself.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# The control is named for the pinned build, read from the registry: hard-coded,
# the 11-09 re-pin would have run against a copy of the wrong build.
PINNED=$(awk -F'\t' '!/^#/ && $3 == "pinned" {print $1; exit}' "$REPO/tools/builds.tsv" 2>/dev/null || true)
COPIES="tq-${PINNED:-3662168}-control:control,tq-offline:subject,tq-3619879:subject"
SCENARIOS='census_canary,census_idle,census_links,census_cc,census_hang,census_crash,census_report_bug,census_urls,census_cdn'
# The no-env subject proves the file patches by themselves: boot, character
# select and the character-creation sink. Every other scenario fires names
# the launcher's proxy absorbs, which a no-env run has no proxy for.
NOENV_SCENARIOS='census_canary,census_idle,census_cc'
MODE=
ROOTLESS=0
CHECK=0
OUT=
PASS=()

usage() { sed -n '2,/^[^#]/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; }
die() { echo "census.sh: $*" >&2; exit 4; }

while [[ $# -gt 0 ]]; do
    case $1 in
        --all) ;;
        --check) CHECK=1 ;;
        --copies) COPIES=${2:?--copies needs a list}; shift ;;
        --scenarios) SCENARIOS=${2:?--scenarios needs a list}; shift ;;
        --noenv-scenarios) NOENV_SCENARIOS=${2:?--noenv-scenarios needs a list}; shift ;;
        --mode) MODE=${2:?--mode needs observe or handshake}; shift ;;
        --rootless) ROOTLESS=1 ;;
        --out) OUT=${2:?--out needs a directory}; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; PASS=("$@"); break ;;
        *) usage >&2; exit 4 ;;
    esac
    shift
done
[[ -z $MODE || $MODE == observe || $MODE == handshake ]] || die "--mode is observe or handshake"
(( EUID != 0 )) || die "run me as yourself; the helper is the only thing that is root"

# Parse and order the copies: the control first, exactly one of it.
declare -a CDIR=() CROLE=()
IFS=',' read -r -a items <<< "$COPIES"
for it in "${items[@]}"; do
    d=${it%%:*} r=${it#*:}
    [[ -n $d && $d != */* && $d != .* && ($r == control || $r == subject || $r == noenv) ]] || die "bad copy '$it' (DIR:control|subject|noenv)"
    if [[ $r == control ]]; then CDIR=("$d" ${CDIR[@]+"${CDIR[@]}"}) CROLE=(control ${CROLE[@]+"${CROLE[@]}"})
    else CDIR+=("$d") CROLE+=("$r"); fi
done
controls=0; for r in "${CROLE[@]}"; do [[ $r == control ]] && controls=$((controls + 1)); done
(( controls <= 1 )) || die "one control per census"
IFS=',' read -r -a SCENS <<< "$SCENARIOS"
(( ${#SCENS[@]} > 0 )) || die "no scenarios"
IFS=',' read -r -a NSCENS <<< "$NOENV_SCENARIOS"
(( ${#NSCENS[@]} > 0 )) || die "no no-env scenarios"
# The scenarios a copy's role runs.
scens_for() { if [[ $1 == noenv ]]; then printf '%s\n' "${NSCENS[@]}"; else printf '%s\n' "${SCENS[@]}"; fi; }

# A scenario's own timeout (CEN-4: EOCI_CENSUS_TIMEOUT_S), else 600 s.
scen_timeout() {
    local f=$REPO/tools/client-ci/scenarios/$1.py t
    t=$(sed -n 's/^EOCI_CENSUS_TIMEOUT_S *= *\([0-9][0-9]*\).*/\1/p' "$f" 2>/dev/null | head -n 1)
    printf '%s' "${t:-600}"
}

client_ci() {   # COPY ROLE RUN SCEN [extra client-ci args...]
    local copy=$1 role=$2 run=$3 scen=$4 mode; shift 4
    mode=${MODE:-$([[ $role == control ]] && echo handshake || echo observe)}
    local -a a=(run "$scen" --census "$run" --census-mode "$mode" --timeout "$(scen_timeout "$scen")")
    [[ $role == control ]] && a+=(--census-control)
    [[ $role == noenv ]] && a+=(--census-noenv)
    (( ROOTLESS )) && a+=(--rootless)
    a+=("$@")
    (( ${#PASS[@]} )) && a+=("${PASS[@]}")
    EVE_CLIENT_DIR=$copy "$REPO/tools/client-ci.sh" "${a[@]}"
}

if (( CHECK )); then
    rc=0
    for i in "${!CDIR[@]}"; do
        for scen in $(scens_for "${CROLE[$i]}"); do
            echo "== check ${CDIR[$i]} (${CROLE[$i]}) $scen"
            client_ci "${CDIR[$i]}" "${CROLE[$i]}" "$REPO/out/census/check" "$scen" --check </dev/null || rc=4
        done
    done
    exit "$rc"
fi

OUT=${OUT:-$REPO/out/census/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$OUT"
echo "census -> $OUT"

# Merge one copy's per-scenario R2 dumps into the nft.json the report reads.
merge_nft() {   # RUN
    python3 -I - "$1" <<'PY'
import glob, json, os, sys
run = sys.argv[1]
items = []
for p in sorted(glob.glob(os.path.join(run, "nft-*.json"))):
    try:
        with open(p) as f:
            items.extend(json.load(f).get("nftables", []))
    except (OSError, ValueError) as err:
        with open(os.path.join(run, "faults.txt"), "a") as f:
            f.write("%s: does not parse: %s\n" % (os.path.basename(p), err))
if glob.glob(os.path.join(run, "nft-*.json")):
    with open(os.path.join(run, "nft.json"), "w") as f:
        json.dump({"nftables": items}, f)
PY
}

# A segment that did not run its scenario to a good end is a harness fault, so
# the copy is INVALID and no path of it can read as passed. Exempt from the
# exit-code rule: census_crash and census_hang, which may end with the client
# dead (client-ci exit 4); not from the no-client rule.
segment_faults() {   # RUN ROLE
    local run=$1 role=$2 scen n c seen
    for scen in $(scens_for "$role"); do
        seen=0
        if [[ -f $run/segments.tsv ]]; then
            while IFS=$'\t' read -r n c; do
                [[ $n == "$scen" ]] || continue
                seen=1
                if [[ $n != census_crash && $n != census_hang && $c != 0 ]]; then
                    printf '%s: client-ci exit %s; the scenario did not run to a passing end\n' "$n" "$c" >> "$run/faults.txt"
                fi
            done < "$run/segments.tsv"
        fi
        (( seen )) || printf '%s: no segment was recorded\n' "$scen" >> "$run/faults.txt"
        if [[ -f $run/noclient.txt ]] && grep -qxF -- "$scen" "$run/noclient.txt"; then
            printf '%s: no client was launched in this segment\n' "$scen" >> "$run/faults.txt"
        fi
    done
}

worst=0
rank() { case $1 in 2) echo 4 ;; 1) echo 3 ;; 3) echo 2 ;; 0) echo 0 ;; *) echo 5 ;; esac; }
note_rc() { (( $(rank "$1") > $(rank "$worst") )) && worst=$1; return 0; }
CONTROL_REPORT=
{
    echo "# Census $(basename "$OUT")"
    echo
    echo "Copies in order; the verdict is census_report.py's (docs/guides/census.md)."
    echo
} > "$OUT/SUMMARY.md"

for i in "${!CDIR[@]}"; do
    copy=${CDIR[$i]} role=${CROLE[$i]}
    run=$OUT/$copy
    [[ $role == noenv ]] && run=$OUT/$copy-noenv
    mkdir -p "$run"
    echo
    echo "======== $copy ($role)"
    mkdir -p "$run"
    printf '%s\n' "$role" > "$run/role.txt"   # what this directory is, for whoever reads it later
    for scen in $(scens_for "$role"); do
        echo "== $scen"
        rc=0
        client_ci "$copy" "$role" "$run" "$scen" </dev/null || rc=$?
        echo "   client-ci exit $rc"
        # 3 means it was refused before (or at) census-up: nothing of this
        # segment was recorded, so the copy cannot be complete.
        (( rc != 3 )) || printf '%s: client-ci refused (exit 3); the segment was not recorded\n' "$scen" >> "$run/faults.txt"
    done
    merge_nft "$run"
    segment_faults "$run" "$role"
    # Segments with no client: client-ci lists them in noclient.txt (and exit 3
    # or 4 say the same). If no segment had a client the rows are the harness's
    # own (the jail's verify lookups, a recorder's start and stop), not a
    # client's: no report is made, and none of it is called a leak.
    nseg=$(wc -l < "$run/segments.tsv" 2>/dev/null || echo 0)
    nocl=0
    [[ ! -f $run/noclient.txt ]] || nocl=$(sort -u "$run/noclient.txt" | wc -l)
    if (( nseg == 0 || nocl >= nseg )); then
        rc=2
        verdict="NO CLIENT RAN: no client was launched in any of ${nseg} segment(s) (see the exit codes below and $run/faults.txt); no report was made, and nothing recorded is a client's"
        echo "$verdict"
        printf '%s\n' "NO CLIENT RAN" > "$run/summary.txt"
        note_rc "$rc"
        {
            echo "## $copy ($role): exit $rc"
            echo
            echo "$verdict"
            echo
            [[ -s $run/faults.txt ]] && { echo '```'; cat "$run/faults.txt"; echo '```'; echo; }
            echo "Segments (scenario, client-ci exit):"
            echo
            sed 's/^/    /' "$run/segments.tsv" 2>/dev/null || echo "    none"
            echo
        } >> "$OUT/SUMMARY.md"
        continue
    fi
    rargs=("$run" --role "$role" --label "$copy")
    [[ $role == noenv ]] && rargs=("$run" --role subject --label "$copy-noenv" --noenv)
    if [[ $role == subject || $role == noenv ]]; then
        if [[ -n $CONTROL_REPORT ]]; then rargs+=(--control "$CONTROL_REPORT"); fi
    fi
    rc=0
    python3 -I "$REPO/tools/census_report.py" "${rargs[@]}" || rc=$?
    verdict=$(head -n 1 "$run/summary.txt" 2>/dev/null || echo "no summary")
    # A segment fault the report cannot see (a recorder restarted, verify
    # failing after census-down) makes the copy INVALID, whatever it says.
    if [[ -s $run/faults.txt ]]; then
        rc=2
        verdict="INVALID (harness faults, $run/faults.txt) -- the report said: $verdict"
    fi
    [[ $role == control && -f $run/report.json ]] && CONTROL_REPORT=$run/report.json
    note_rc "$rc"
    {
        echo "## $copy ($role): exit $rc"
        echo
        echo "$verdict"
        echo
        [[ -s $run/faults.txt ]] && { echo '```'; cat "$run/faults.txt"; echo '```'; echo; }
        echo "Segments (scenario, client-ci exit):"
        echo
        sed 's/^/    /' "$run/segments.tsv" 2>/dev/null || echo "    none"
        echo
        echo "Full summary: \`$run/summary.txt\`; every event: \`$run/report.json\`."
        echo
    } >> "$OUT/SUMMARY.md"
done

echo
echo "SUMMARY: $OUT/SUMMARY.md (exit $worst)"
exit "$worst"
