#!/usr/bin/env bash
# Run the client-CI regression list, several clients at once, and carry on past
# failures. A red result means the client disagreed, not that the runner
# forgot a flag: the flags each scenario needs are in the list itself.
#
#   tools/client-ci-regress.sh                    # the whole list, 3 at a time
#   tools/client-ci-regress.sh fci_doc_0 fci_sov  # just these (by name or name@label)
#   tools/client-ci-regress.sh --fails            # only the ones the last run lost
#
#   --jobs N        CLIENTS at once (default 3, or EVE_CI_JOBS): a client budget, not
#                   a row count. A row costs 1, a `--with-second` row costs 2 and
#                   waits until 2 are free; the budget is never exceeded, and a row
#                   that costs more than N runs alone. --jobs 1 is the old behaviour
#                   exactly: one row after another in the owner's prefix, list
#                   order, `--serial` ignored. With N > 1 every row gets a slot
#                   with a prefix, jail, run directory, world and ports of its own
#                   (tools/client-ci/slot-lib.sh), the rows marked `--serial` wait
#                   and run alone at the end, and rows finish out of order.
#   --clean         delete every slot prefix (slot-prefix.sh remove all) and stop;
#                   refused while another runner holds the slots
#   --list FILE     another list (default tools/client-ci/regress.list)
#   --out DIR       logs and results (default var/regress)
#   --client DIR    the client directory under CCP/EVE (EVE_CLIENT_DIR)
#   --no-build      do not build even for the first scenario (the rest never do)
#   --no-rootless   use the jail helper (sudo) instead of the rootless namespace
#                   (one jail, so --jobs 1 only)
#   --dry-run       print the plan and the client-ci.sh lines and stop
#
# The list format and what each flag is for: the header of
# tools/client-ci/regress.list. Every --dev run starts from a fresh dev world
# (tools/client-ci.sh does the reset). Each scenario runs with its own
# EOCI_FCI_TIMEOUT_S (tools/client-ci.sh reads it), not the 600 s default that
# made the long ones read as "never ran".
#
# Results: OUT/regress.tsv, one `name<TAB>exit status` row per scenario, written
# as each one finishes (the statuses are client-ci.sh's: 0 pass, 1 client
# disagreed, 3 refused, 4 never ran, 5 malformed, 6 the scenario raised; 143 a row
# that an interrupted run stopped or never started), and
# OUT/<name>.log. OUT/clock.tsv has, per row, the slot, the seconds, the world
# clock's worst lateness and the box's load, for judging whether --jobs can go up
# (docs/guides/client-ci.md, "Several clients at once").
#
# Run it on a QUIET machine: the scenarios fly, and a busy box makes the
# world clock late.
set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
LIST=$REPO/tools/client-ci/regress.list
OUT=$REPO/var/regress
BUILD=1 ROOTLESS=1 DRY=0 FAILS=0 CLEAN=0
JOBS=${EVE_CI_JOBS:-3}
ONLY=()
# Overridable so the scheduling can be tested with stubs (tools/test_client_ci_parallel.py).
CLIENT_CI=${EVE_CI_CLIENT_CI:-$REPO/tools/client-ci.sh}
SLOT_PREFIX_TOOL=${EVE_CI_SLOT_PREFIX:-$REPO/tools/client-ci/slot-prefix.sh}
HEADLESS=${EVE_CI_HEADLESS:-$REPO/tools/client-headless.sh}
SCEN_DIR=${EVE_CI_SCENARIOS:-$REPO/tools/client-ci/scenarios}
QUIET=${EVE_CI_QUIET:-$REPO/tools/client-ci/quiet-settings.py}
# shellcheck source=tools/client-ci/slot-lib.sh
source "$REPO/tools/client-ci/slot-lib.sh"
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

die() { echo "client-ci-regress: $*" >&2; exit 2; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --jobs)        JOBS=${2:?--jobs needs a number}; shift ;;
        --list)        LIST=${2:?--list needs a file}; shift ;;
        --out)         OUT=${2:?--out needs a directory}; shift ;;
        --client)      export EVE_CLIENT_DIR=${2:?--client needs a directory name}; shift ;;
        --no-build)    BUILD=0 ;;
        --no-rootless) ROOTLESS=0 ;;
        --dry-run)     DRY=1 ;;
        --fails)       FAILS=1 ;;
        --clean)       CLEAN=1 ;;
        -h|--help)     sed -n '2,/^set -uo/{/^set -uo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        -*)            die "unknown argument $1 (see --help)" ;;
        *)             ONLY+=("$1") ;;
    esac
    shift
done
[[ $JOBS =~ ^[1-9][0-9]{0,2}$ ]] || die "--jobs takes a number from 1 to 999, not '$JOBS'"
(( JOBS == 1 || ROOTLESS )) || die "--jobs $JOBS needs the rootless jail: the helper's jail is one namespace for one client (use --jobs 1 with --no-rootless)"
if (( CLEAN )); then
    exec "$SLOT_PREFIX_TOOL" remove all
fi
[[ -f $LIST ]] || die "no list at $LIST"
mkdir -p "$OUT"
TSV=$OUT/regress.tsv
CLOCK=$OUT/clock.tsv

# --fails: the names the previous run's table lists with a non-zero status.
if (( FAILS )); then
    [[ -f $TSV ]] || die "--fails needs a previous run: $TSV does not exist"
    while IFS=$'\t' read -r n rc; do
        [[ -n $n && $rc != 0 && $rc != finished ]] && ONLY+=("$n")
    done < "$TSV"
    (( ${#ONLY[@]} )) || { echo "nothing failed in the last run"; exit 0; }
fi

# @SCHOOL@: the first school in the content bundle (the table is extracted
# data, so it is read here at run time and never written into the repository).
school_id() {
    local f=${EVE_CI_CONTENT:-$REPO/out/content}/agents/schools.tsv
    [[ -f $f ]] || return 1
    awk -F'\t' '!/^#/ && $1 ~ /^[0-9]+$/ {print $1; exit}' "$f"
}

# @FWSTATION@: a station owned by a corporation of one of the four empire
# factions (500001-500004, the militia factions), read from the bundle at run time
# for fci_fw_enlist (exported as EVE_CI_FWSTATION; client-ci.sh deploys it).
fw_station_id() {
    local d=${EVE_CI_CONTENT:-$REPO/out/content}
    [[ -f $d/stations/stations.tsv && -f $d/npc/corporations.tsv ]] || return 1
    awk -F'\t' 'FNR==NR { if ($1 ~ /^[0-9]+$/ && $2 ~ /^50000[1-4]$/) ok[$1]=1; next }
        $1 ~ /^[0-9]+$/ && ($4 in ok) { print $1; exit }' "$d/npc/corporations.tsv" "$d/stations/stations.tsv"
}

wanted() {  # is "$1" (a list key) selected?
    (( ${#ONLY[@]} )) || return 0
    local o
    for o in "${ONLY[@]}"; do [[ $o == "$1" || ${1%%@*} == "$o" ]] && return 0; done
    return 1
}

# ---------------------------------------------------------------- the list

# One entry per selected line, in list order: KEYS, NAMES, FLAGS and SRVS (the
# words, space-joined: no word of either holds a space), SERIAL (1: runs alone).
KEYS=() NAMES=() FLAGS=() SRVS=() SERIAL=()
while IFS= read -r line || [[ -n $line ]]; do
    line=${line%%#*}
    # shellcheck disable=SC2206
    words=($line)
    (( ${#words[@]} )) || continue
    key=${words[0]}
    wanted "$key" || continue
    flags=() srv=() in_srv=0 serial=0
    for w in "${words[@]:1}"; do
        if (( in_srv )); then srv+=("$w")
        elif [[ $w == -- ]]; then in_srv=1
        elif [[ $w == --serial ]]; then serial=1     # the runner's own flag; client-ci.sh never sees it
        else flags+=("$w"); fi
    done
    KEYS+=("$key") NAMES+=("${key%%@*}") FLAGS+=("${flags[*]-}") SRVS+=("${srv[*]-}") SERIAL+=("$serial")
done < "$LIST"

# The client-ci.sh arguments of entry I, in ARGS; $2 is 1 when this run must not
# build. What @SCHOOL@ becomes is read from the bundle, so it is looked up only
# when a selected row names it.
entry_args() {
    local i=$1 nobuild=$2 flags srv sid s=${SRVS[$1]}
    if [[ " $s " == *@SCHOOL@* ]]; then
        sid=$(school_id) || { (( DRY )) && sid=N || die "${KEYS[$i]}: no school table in the content bundle (agents/schools.tsv)"; }
        s=${s//@SCHOOL@/$sid}
    fi
    # shellcheck disable=SC2206
    flags=(${FLAGS[$i]})
    # shellcheck disable=SC2206
    srv=($s)
    args=(run "${NAMES[$i]}")
    (( ROOTLESS )) && args+=(--rootless)
    (( nobuild )) && args+=(--no-build)
    args+=(${flags[@]+"${flags[@]}"} -- --gateway ${srv[@]+"${srv[@]}"})
}

fw_for() {   # KEY NAME: the militia station fci_fw_enlist needs, else empty (in FW)
    FW=
    if [[ $2 == fci_fw_enlist ]]; then
        FW=$(fw_station_id) || FW=
        (( DRY )) || [[ -n $FW ]] || echo "   note: no militia station in the bundle; $1 starts where the character does" >&2
    fi
}

cost_of() {   # I: clients the row runs, 1 or 2
    [[ " ${FLAGS[$1]} " == *" --with-second "* ]] && echo 2 || echo 1
}

wants_second() {   # does any parallel row run a second client?
    local i
    for i in "${PAR[@]}"; do
        [[ " ${FLAGS[$i]} " == *" --with-second "* ]] && return 0
    done
    return 1
}

# ---------------------------------------------------------------- the clock

# The world clock's own account of a finished row: [world] tick health lines in
# the row's server log. clock_row KEY SLOT NAME SECONDS RC.
clock_row() {
    local key=$1 slot=$2 name=$3 secs=$4 rc=$5 dir log lines n worst over load
    if (( slot > 0 )); then ci_slot_vars "$REPO" "$HOME" "$slot" && dir=$SLOT_RUNS/$key; else dir=$REPO/out/client-ci/$key; fi
    log=$dir/eve-server.log
    n=0 worst=- over=-
    if [[ -s $log ]]; then
        lines=$(/usr/bin/grep '\[world\] tick health:' "$log" || true)
        if [[ -n $lines ]]; then
            n=$(wc -l <<<"$lines")
            worst=$(sed -n 's/.*max \([0-9.]*\) ms, over budget.*/\1/p' <<<"$lines" | sort -rn | head -1)
            over=$(sed -n 's/.*over budget \([0-9]*\).*/\1/p' <<<"$lines" | sort -rn | head -1)
        fi
    fi
    read -r load _ < /proc/loadavg
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$key" "$slot" "$rc" "$secs" "$n" "${worst:--}" "${over:--}" "$load" >> "$CLOCK"
}

clock_summary() {
    [[ -s $CLOCK ]] || return 0
    awk -F'\t' '
        NR > 1 && $6 != "-" { if ($6 + 0 > w) { w = $6 + 0; wk = $1 } }
        NR > 1 && $7 != "-" && $7 + 0 > 0 { o++; ol = ol " " $1 }
        NR > 1 && $8 + 0 > l { l = $8 + 0 }
        NR > 1 { n++ }
        END { printf "client-ci-regress: clock over %d rows: worst tick %.1f ms late (%s), %d row(s) with ticks over budget%s, highest load %.1f\n",
                     n, w, wk == "" ? "-" : wk, o, ol, l }' "$CLOCK"
}

# ---------------------------------------------------------------- bookkeeping

ran=0 failed=0 FINISHED=0 TSV_OPEN=0
declare -A PID_SLOT=() PID_KEY=() PID_NAME=() PID_T0=() PID_COST=() SLOT_BUSY=() RECORDED=()
SWEEP=()
RUNNING_COST=0

record() {   # KEY RC
    printf '%s\t%s\n' "$1" "$2" >> "$TSV"
    RECORDED[$1]=1
    ran=$((ran + 1))
    (( $2 == 0 )) || { failed=$((failed + 1)); echo "   -> $2 (see $OUT/$1.log)"; }
}

missing_scenario() {   # NAME: is the scenario file absent? (recorded as a failure)
    [[ -f $SCEN_DIR/$1.py ]] && return 1
    return 0
}

record_missing() {   # KEY
    printf '%s\tmissing\n' "$1" >> "$TSV"
    RECORDED[$1]=1
    failed=$((failed + 1))
}

# The owner's settings file, and every copy's, back where it was: client-ci.sh does it
# for its own run, and this is the net under it for a run that was killed. Slots
# forced their own copies, never the owner's, so on the owner's prefix this is a
# no-op unless a --jobs 1 or serial run died. An entry is skipped while a client of
# its install name still runs: its file is in use, and putting it back under that
# client is the harm the sweep exists to avoid.
live_client() {   # DIR: does a client from install directory DIR run?
    local pid comm
    for pid in $(pgrep -f "$1.bin64.exefile" 2>/dev/null || true); do
        comm=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        case $comm in pgrep | pkill | '') continue ;; esac
        return 0
    done
    return 1
}

settings_sweep() {
    local entry
    for entry in "${SWEEP[@]}"; do
        live_client "${entry##*|}" && continue
        python3 -I "$QUIET" restore "${entry%|*}" "${entry##*|}" >/dev/null 2>&1 || true
    done
}

sweep_add() {   # the entry "DRIVE_C|CLIENT_DIR", once
    local e
    for e in "${SWEEP[@]}"; do [[ $e == "$1" ]] && return 0; done
    SWEEP+=("$1")
}

reap() {   # PID RC
    local pid=$1 rc=$2 s=${PID_SLOT[$1]}
    record "${PID_KEY[$pid]}" "$rc"
    clock_row "${PID_KEY[$pid]}" "$s" "${PID_NAME[$pid]}" $((SECONDS - PID_T0[$pid])) "$rc"
    RUNNING_COST=$((RUNNING_COST - PID_COST[$pid]))
    unset "PID_SLOT[$pid]" "PID_KEY[$pid]" "PID_NAME[$pid]" "PID_T0[$pid]" "PID_COST[$pid]" "SLOT_BUSY[$s]"
}

# Rows still running are stopped (each client-ci.sh cleans up its own stack on TERM) and
# recorded as 143, so --fails after an interrupted run reruns them.
stop_all() {
    local pid rc
    for pid in "${!PID_SLOT[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
    for pid in "${!PID_SLOT[@]}"; do
        wait "$pid" 2>/dev/null; rc=$?
        (( rc == 0 )) || rc=143
        reap "$pid" "$rc"
    done
}

finish() {
    local rc=$? k
    trap - EXIT INT TERM
    stop_all
    # A run that did not reach its end: the rows that never started are 143 too.
    if (( TSV_OPEN && ! FINISHED )); then
        for k in "${KEYS[@]}"; do
            [[ -n ${RECORDED[$k]:-} ]] || printf '%s\t143\n' "$k" >> "$TSV"
        done
    fi
    settings_sweep
    [[ ${STARTED_DISPLAY:-0} == 1 ]] && "$HEADLESS" stop >/dev/null 2>&1
    exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# One row of client-ci.sh, in the background so the runner can stop it on a signal.
# SLOT 0: the owner's prefix, no slot. Returns 1 when the scenario is missing.
start_row() {   # I SLOT
    local i=$1 s=$2 key=${KEYS[$1]} name=${NAMES[$1]} pid c
    fw_for "$key" "$name"
    entry_args "$i" $(( s > 0 || ! (BUILD && FIRST) ))
    if missing_scenario "$name"; then record_missing "$key"; return 1; fi
    c=$(cost_of "$i")
    if (( s > 0 )); then
        echo "== $key [slot $s]: tools/client-ci.sh ${args[*]}"
        # Stale runs of this scenario in the other, idle slots are gone, so none can be read
        # for this one. A busy slot may be running a row of the same name (name@on/@off).
        local d n
        for d in "$REPO"/out/client-ci/slots/[1-9]*/runs/"$key"; do
            [[ -d $d ]] || continue
            n=${d#"$REPO"/out/client-ci/slots/}; n=${n%%/*}
            [[ -z ${SLOT_BUSY[$n]:-} && $n != "$s" ]] && rm -rf -- "$d"
        done
        EOCI_RUN_KEY=$key EVE_CI_SLOT=$s EVE_CI_FWSTATION=$FW timeout 3600 "$CLIENT_CI" "${args[@]}" </dev/null > "$OUT/$key.log" 2>&1 8>&- &
    else
        echo "== $key: tools/client-ci.sh ${args[*]}"
        EOCI_RUN_KEY=$key EVE_CI_FWSTATION=$FW timeout 3600 "$CLIENT_CI" "${args[@]}" </dev/null > "$OUT/$key.log" 2>&1 8>&- &
    fi
    pid=$!
    PID_SLOT[$pid]=$s PID_KEY[$pid]=$key PID_NAME[$pid]=$name PID_T0[$pid]=$SECONDS PID_COST[$pid]=$c
    RUNNING_COST=$((RUNNING_COST + c))
    (( s == 0 )) || SLOT_BUSY[$s]=1
    LAST_PID=$pid
}

# ---------------------------------------------------------------- one at a time

run_serial_entry() {   # I: sets RAN_ROW=1 when the row ran (or, with --dry-run, was printed)
    local i=$1 rc
    RAN_ROW=0
    if (( DRY )); then
        fw_for "${KEYS[$i]}" "${NAMES[$i]}"
        entry_args "$i" $(( ! (BUILD && FIRST) ))
        echo "tools/client-ci.sh ${args[*]}"; RAN_ROW=1; return 0
    fi
    # The owner's prefix is in play from here: put its settings back if this run dies.
    sweep_add "$(eve_offline_prefix "$HOME")/pfx/drive_c|${EVE_CLIENT_DIR:-tq-offline}"
    start_row "$i" 0 || return 0
    RAN_ROW=1
    wait "$LAST_PID"; rc=$?
    # A signal to the runner interrupts the wait: finish stops and records the row.
    reap "$LAST_PID" "$rc"
}

# ---------------------------------------------------------------- jobs 1: as before

FIRST=1
if (( JOBS == 1 )); then
    [[ $DRY -eq 1 ]] || { : > "$TSV"; TSV_OPEN=1; printf 'key\tslot\trc\tseconds\tsamples\tworst_late_ms\tover_budget\tload1\n' > "$CLOCK"; }
    for i in "${!KEYS[@]}"; do
        run_serial_entry "$i"
        (( RAN_ROW )) && FIRST=0
    done
    (( DRY )) && exit 0
    echo finished >> "$TSV"; FINISHED=1
    echo "client-ci-regress: $ran ran, $failed not green; table in $TSV"
    clock_summary
    (( failed == 0 ))
    exit
fi

# ---------------------------------------------------------------- jobs N: the pool

FIRST=0   # the pool builds once, before the slots; no row builds
PAR=() SER=()
for i in "${!KEYS[@]}"; do
    if [[ ${SERIAL[$i]} == 1 ]]; then SER+=("$i"); else PAR+=("$i"); fi
done
NSLOTS=$JOBS
(( ${#PAR[@]} >= NSLOTS )) || NSLOTS=${#PAR[@]}

eff_cost() {   # I: the row's cost, clipped to the budget (a row that needs more runs alone)
    local c; c=$(cost_of "$1")
    (( c <= JOBS )) || c=$JOBS
    echo "$c"
}

if (( DRY )); then
    echo "plan: --jobs $JOBS: a budget of $JOBS client(s), a --with-second row costs 2; ${#PAR[@]} row(s) in parallel on at most $NSLOTS slot(s) (a row takes the lowest free slot; round-robin shown), then ${#SER[@]} serial row(s) alone"
    SRC_DIR=${EVE_CLIENT_DIR:-tq-offline}
    for ((s = 1; s <= NSLOTS; s++)); do
        ci_slot_dump "$REPO" "$HOME" "$s" "$SRC_DIR" | sed "s/^/slot $s: /"
    done
    n=0
    for i in "${PAR[@]}"; do
        s=$(( n % NSLOTS + 1 )); n=$((n + 1))
        fw_for "${KEYS[$i]}" "${NAMES[$i]}"
        entry_args "$i" 1
        echo "[slot $s cost $(eff_cost "$i")] EVE_CI_SLOT=$s tools/client-ci.sh ${args[*]}"
    done
    for i in "${SER[@]}"; do
        fw_for "${KEYS[$i]}" "${NAMES[$i]}"
        entry_args "$i" 1
        echo "[serial] tools/client-ci.sh ${args[*]}"
    done
    exit 0
fi

: > "$TSV"; TSV_OPEN=1
printf 'key\tslot\trc\tseconds\tsamples\tworst_late_ms\tover_budget\tload1\n' > "$CLOCK"

# One runner at a time: a second one would rebuild a slot's client install and delete its
# prefix under a running client. Non-blocking; the rows do not inherit the lock, so a
# client that outlives the runner cannot wedge it. slot-prefix.sh is told it is held.
LOCK=$(ci_runner_lock "$REPO")
mkdir -p "$(dirname "$LOCK")"
exec 8>"$LOCK"
flock -n 8 || die "another client-ci-regress run holds the slots ($LOCK)"
export EVE_CI_RUNNER_LOCK_HELD=1

if (( BUILD )); then
    echo "== build (once, before the slots start)"
    "$REPO/tools/dev.sh" cargo build --locked --workspace --bins >&2 || die "the build failed"
fi

SRC_DIR=${EVE_CLIENT_DIR:-tq-offline}
for ((s = 1; s <= NSLOTS; s++)); do
    echo "== slot $s: prefix"
    "$SLOT_PREFIX_TOOL" ensure "$s" --client "$SRC_DIR" || die "slot $s: no prefix (tools/client-ci/slot-prefix.sh ensure $s)"
    ci_slot_vars "$REPO" "$HOME" "$s" "$SRC_DIR"
    sweep_add "$SLOT_PREFIX/pfx/drive_c|$SLOT_CLIENT_DIR"
    if wants_second; then
        "$SLOT_PREFIX_TOOL" ensure "$s" --second --client "$SRC_DIR" || die "slot $s: no second prefix"
        sweep_add "$SLOT_PREFIX2/pfx/drive_c|$SLOT_CLIENT_DIR2"
    fi
done
# The Xvfb the slots' `desktop on` talks to: started once here and stopped once at the end,
# but only when this run started it. One that was already up belongs to another run.
if "$HEADLESS" status >/dev/null 2>&1; then
    echo "== the headless display is already up; leaving it to whoever started it"
else
    "$HEADLESS" start >&2 || die "the headless display did not start"
    STARTED_DISPLAY=1
fi

next=0
while (( next < ${#PAR[@]} || ${#PID_SLOT[@]} > 0 )); do
    # Start rows in list order while the head of the list fits the budget and a slot is free.
    while (( next < ${#PAR[@]} )); do
        i=${PAR[$next]}
        c=$(eff_cost "$i")
        (( RUNNING_COST + c <= JOBS )) || break
        s=0
        for ((t = 1; t <= NSLOTS; t++)); do
            [[ -z ${SLOT_BUSY[$t]:-} ]] && { s=$t; break; }
        done
        (( s > 0 )) || break
        next=$((next + 1))
        start_row "$i" "$s" || true
    done
    (( ${#PID_SLOT[@]} > 0 )) || continue
    done_pid=''
    wait -n -p done_pid "${!PID_SLOT[@]}"
    rc=$?
    [[ -n $done_pid && -n ${PID_SLOT[$done_pid]:-} ]] && reap "$done_pid" "$rc"
done

# The rows that must run alone, in the owner's prefix as before, once nothing else runs.
for i in "${SER[@]}"; do
    run_serial_entry "$i"
done

echo finished >> "$TSV"; FINISHED=1
echo "client-ci-regress: $ran ran, $failed not green; table in $TSV"
clock_summary
(( failed == 0 ))
