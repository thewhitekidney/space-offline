#!/usr/bin/env bash
# Run one client-CI scenario end to end, with nobody at the keyboard.
#
#   tools/client-ci.sh list                        # what scenarios exist
#   sudo -v && tools/client-ci.sh run undock       # the whole thing
#
#   --timeout SEC      how long to wait for the verdict (default 600, or the
#                      scenario's own EOCI_FCI_TIMEOUT_S when that is longer)
#   --ready SEC        how long to wait for the server to come up (default 600;
#                      it includes the container build, so --no-build is faster)
#   --no-build         pass --no-build to up.sh (use the binaries as they are)
#   --no-scenario-flag do not pass --scenario to eve-server. The scenario will
#                      not be triggered, so this can only ever report "never
#                      ran" -- it is for testing the harness itself.
#   --no-auto-select   do not pass /autoSelectCharacter:0 to the client
#   --with-second [N]  ALSO run the second client (prefix 8501, tq-offline-2)
#                      against the same server, selecting character slot N
#                      (default 1), and wait for its verdict too. The server
#                      needs a roster that deep: given none, the script adds
#                      `--character Alice --character Bob ...` itself (or pass
#                      them after `--`). Both clients run the SAME scenario --
#                      the scenario is chosen by the settings blob the server
#                      serves, so a two-player scenario branches on its own
#                      `session.charid`. Exit status is the worse of the two.
#   --with-bots        run one `eve-bots --control` beside the server, so the GM's
#                      `/bots` works (fci_gang_summon, the wing and fleet-mission
#                      halves that SKIP without it). Needs --rootless and a
#                      `-- --dev` server on the run's own dev world. The server
#                      then runs with --world-accounts (a catch-all gives the
#                      bots and the client one account): a gm account `cipilot`
#                      is made first, its password kept in
#                      ~/.config/eve-offline/client-ci-pilot.tsv (mode 600,
#                      EVE_CI_PILOT_TOKENS to move it), and the client logs in
#                      as it. The bots start inside the jail's namespace (the
#                      control link is loopback there), log to $RUNDIR/bots.log
#                      and are stopped by their recorded pid. Not with
#                      --with-second or --character (accounts mode refuses it).
#   --bots-kit WHAT    with --with-bots: give the open-mode hunter bots a part they
#                      cannot get otherwise, `smartbomb` or `mjd` (BP-KIT,
#                      fci_bot_smartbomb / fci_bot_mjd): sets
#                      EVE_BOTS_KIT_DRILL for eve-bots, which grants the skill
#                      and one part per session and fits it from the hangar.
#   --frame            also grab the root window and report mean saturation
#   --check            run every preflight check and stop; starts nothing. The
#                      only mode that works without root, so it is also how
#                      this script is tested.
#   --inherit-display  use the caller's $DISPLAY instead of bringing up a
#                      headless one. For watching a run on your own screen;
#                      an unattended run wants the default.
#   --software         render with lavapipe on Xvfb instead of the GPU (the
#                      default is RADV on a headless gamescope display);
#                      --frame implies it, the frame oracle reads Xvfb
#   --keep             leave the server, the jail, the client and the deployed
#                      scenario alone on exit (for poking at a failure)
#   --reuse-server     use a server that is already running even if its command
#                      line does not name this scenario
#   --rootless         no sudo and no helper: the server, eve-dns and the client
#                      all run in tools/jail-rootless.sh's lo-only namespace
#                      (IP29). Opt-in until 5 green ballpark_smoke runs. The
#                      client starts only if the in-jail gate fails to connect
#                      to every host socket (ruling 11). Needs the content
#                      bundle already made (tools/up.sh makes it: out/content,
#                      or out/content-<build> for an unverified client build)
#   --slot N           a PARALLEL slot (lane ci-parallel, #139; also EVE_CI_SLOT=N):
#                      the run owns a Wine prefix of its own (tools/client-ci/
#                      slot-prefix.sh ensure N), a client install named
#                      <client>-sN, its own rootless jail state, launch-env, run
#                      directory, dev world, pilot login and ports, so several runs
#                      can go at once (tools/client-ci-regress.sh --jobs). Needs
#                      --rootless; the jail check and the gate run for every slot
#                      exactly as for one run. The owner's prefix is not touched.
#                      Not with --census. tools/client-ci/slot-lib.sh lists what a
#                      slot owns.
#   --census RUN       a census run (CEN-3, docs/guides/census.md): census-up before the
#                      client (a default route into census0, the recorders),
#                      EVE_CENSUS=1 in the client's launch-env, and after it
#                      census-dump, census-down and `verify` again. This run's
#                      recorder files are APPENDED to RUN (tools/census.sh
#                      gives every scenario of one copy the same RUN). Needs
#                      the helper with the census verbs, or --rootless
#   --census-control   the control copy: EVE_CENSUS_CONTROL=1 (no patcher env
#                      or args, no winebrowser override, crash upload on)
#   --census-noenv     the no-env subject: patched files, the patcher's args, and
#                      NONE of its env (EVE_CENSUS_NOENV=1). Proves the file
#                      patches (tier C) close their paths without the launcher
#   --census-mode M    the sink's mode, observe (default) or handshake
#   -- ARGS...         everything after -- goes to eve-server (e.g. --character)
#
# Exit status, because the difference between these is the whole point:
#
#   0  the scenario ran and every check passed
#   1  the scenario ran and the CLIENT DISAGREED (ok:false, or a failed check)
#   2  usage
#   3  refused: the run could not be set up at all -- no jail, no display, no
#      scenario, a foreign server in the way, or the stack would not start
#   4  the scenario NEVER RAN (no verdict file: timeout, or the client died)
#   5  the verdict file is there but malformed
#   6  the scenario itself raised -- its own traceback is in `error`
#
# The pieces this composes are all pre-existing: `up.sh --no-client` for the
# jail, the stub resolver and the server; `run-client.sh` for the client under
# Proton; and the two `cp`s that `destiny-ref.sh` does, for getting a script
# into the Proton prefix and a result back out.
#
# The scenario itself is NOT ours (lane `ci-scenarios`): we read
# tools/client-ci/scenarios/<name>.py, deploy it to
# c:\eve-logs\client-ci\<name>.py in the prefix, and wait for the client to
# write c:\eve-logs\client-ci\<name>.json beside it.
#
# The DISPLAY is ours as of 2026-10-22: we bring up `tools/client-headless.sh`
# (lane `ci-display`) and point the client at it, with lavapipe forced and a
# wine virtual desktop set. It was not, and the first real run failed exactly
# there -- see the comment on HEADLESS below.
#
# See docs/archive/plan-client-ci.md for why, and docs/lanes/ci-harness.md for what is
# verified and what is still assumed.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
# Overridable so that this script can be exercised against a throwaway
# scenario without writing into tools/client-ci/scenarios, which belongs to
# another lane. The default is the real thing and nothing in CI sets it.
: "${EVE_CI_SCENARIOS:=$REPO/tools/client-ci/scenarios}"
SCEN_DIR=$EVE_CI_SCENARIOS

EX_FAIL=1 EX_USAGE=2 EX_REFUSED=3 EX_NEVER_RAN=4 EX_MALFORMED=5 EX_RAISED=6

usage() { sed -n '2,/^[^#]/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; }
say()   { printf '\n== %s\n' "$*"; }
# Every refusal happens before anything has been started, so it can just exit:
# there is nothing to clean up yet, which is why the trap is installed later.
refuse() { echo "client-ci: $*" >&2; exit $EX_REFUSED; }

# ---------------------------------------------------------------- scenarios

scenario_desc() {
    # First human line of the scenario, for `list`. Scenarios belong to another
    # lane, so this copes with a `#` comment, a docstring, or neither -- and
    # skips the two lines that are never a description: a shebang and the
    # `# -*- coding: utf-8 -*-` cookie that every one of them starts with.
    sed -n '1,12{
        /^#!/d
        /coding[:=]/d
        s/\r$//
        s/^[[:space:]]*#[[:space:]]*//
        s/^[[:space:]]*"""//
        s/"""[[:space:]]*$//
        /[^[:space:]]/{p;q}
    }' "$1" | cut -c1-72
}

list_scenarios() {
    if [[ ! -d $SCEN_DIR ]]; then
        echo "no scenarios: $SCEN_DIR does not exist yet (lane ci-scenarios owns it)"
        return 0
    fi
    local f n found=0
    for f in "$SCEN_DIR"/*.py; do
        [[ -e $f ]] || continue
        found=1
        n=$(basename "$f" .py)
        printf '  %-20s %s\n' "$n" "$(scenario_desc "$f")"
    done
    [[ $found -eq 1 ]] || echo "no scenarios in $SCEN_DIR yet (lane ci-scenarios owns it)"
}

# ---------------------------------------------------------------- arguments

CMD=${1:-}
case "$CMD" in
    list) list_scenarios; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    run) shift ;;
    *) usage >&2; exit $EX_USAGE ;;
esac

NAME=${1:-}
[[ -n $NAME ]] || { echo "client-ci: run needs a scenario name. available:" >&2
                    list_scenarios >&2; exit $EX_USAGE; }
shift

# Generous, and here is the reasoning. Client startup to the login screen is
# tens of seconds; this server deliberately leaves content services
# unimplemented and the client waits out a call timeout on each one; and a
# scenario may itself wait for its ballpark or for a ship to finish moving.
# 600s is roughly ten times the only end-to-end number we have (a hand-driven
# login to in-space is about a minute) and the cost of it being too generous is
# that a hung run wastes ten minutes once, while the cost of it being too tight
# is a false "never ran" that sends somebody debugging the wrong thing.
TIMEOUT=600
TIMEOUT_GIVEN=0
READY_TIMEOUT=600
NO_BUILD=0
PASS_SCENARIO=1
AUTO_SELECT=1
SECOND=0
SECOND_SLOT=1
WITH_BOTS=0
BOTS_KIT=
BOTS_PID=
FRAME=0
CHECK_ONLY=0
KEEP=0
# **Headless by default, and that is the whole point of an unattended run.**
# The first real run of this script used the owner's own session, because this
# script was written to take $DISPLAY from the environment and the display was
# another lane's job. Neither lane owned the seam, so nothing joined them: the
# client launched onto an XWayland screen through `sudo -u`, Xalia died with
# `Video driver not supported`, and the client exited **without writing a log
# at all** -- three reachability probes in the server log and no handshake.
# That failure is indistinguishable from a dozen others, which is exactly why
# the seam is now here instead of in a README.
HEADLESS=1
GPU=1
GS_PID=
# Set once the display and the virtual desktop are ours to take down again.
DISPLAY_UP=0
SECOND_DESKTOP=0
REUSE_SERVER=0
ROOTLESS=0
CENSUS_RUN=
CENSUS_CONTROL=0
CENSUS_NOENV=0
CENSUS_MODE=observe
SLOT=${EVE_CI_SLOT:-0}
SERVER_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --timeout) TIMEOUT=${2:?--timeout needs a value}; TIMEOUT_GIVEN=1; shift ;;
        --ready) READY_TIMEOUT=${2:?--ready needs a value}; shift ;;
        --no-build) NO_BUILD=1 ;;
        --no-scenario-flag) PASS_SCENARIO=0 ;;
        --no-auto-select) AUTO_SELECT=0 ;;
        --with-second)
            SECOND=1
            # An optional numeric argument: --with-second, or --with-second 2.
            if [[ ${2:-} =~ ^[0-9]+$ ]]; then SECOND_SLOT=$2; shift; fi ;;
        --with-bots) WITH_BOTS=1 ;;
        --bots-kit) BOTS_KIT=${2:?--bots-kit needs smartbomb or mjd}; shift ;;
        --frame) FRAME=1 ;;
        --check) CHECK_ONLY=1 ;;
        --keep) KEEP=1 ;;
        --inherit-display) HEADLESS=0 ;;
        --software) GPU=0 ;;
        --reuse-server) REUSE_SERVER=1 ;;
        --rootless) ROOTLESS=1 ;;
        --slot) SLOT=${2:?--slot needs a number}; shift ;;
        --census) CENSUS_RUN=${2:?--census needs a run directory}; shift ;;
        --census-control) CENSUS_CONTROL=1 ;;
        --census-noenv) CENSUS_NOENV=1 ;;
        --census-mode) CENSUS_MODE=${2:?--census-mode needs observe or handshake}; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; SERVER_ARGS=("$@"); break ;;
        *) echo "client-ci: unknown argument $1" >&2; exit $EX_USAGE ;;
    esac
    shift
done

# ---------------------------------------------------------------- preflight

# Same reason up.sh refuses: run-client.sh resolves the launching user and
# their HOME, and under sudo that becomes root with no Wine prefix and no X
# authority. We call sudo ourselves where root is actually needed. This one is
# a hard exit, because every check below it would be answering about root.
[[ $EUID -ne 0 ]] || refuse "do not run this under sudo. Run it as yourself: sudo -v && tools/client-ci.sh run $NAME"

# Every preflight problem is COLLECTED and they are all reported together.
#
# An unattended run gets one chance to tell you what is wrong. Refusing on the
# first problem means a stale sudo timestamp hides a missing scenario hides a
# client left running, and somebody fixes them one overnight round at a time.
PROBLEMS=()
problem() { PROBLEMS+=("$1"); }

# A parallel slot (ci-parallel): its own prefix, client name, jail state and run
# directories, from tools/client-ci/slot-lib.sh. Slot 0 is the run that is alone, and
# every path below stays what it always was.
RL_STATE=$REPO/out/client-ci/rootless
LAUNCH_ENV=$REPO/out/client-ci/launch-env
DEV_WORLD=$REPO/out/client-ci/dev-world
RUN_BASE=$REPO/out/client-ci
PILOT_TOKENS_DEFAULT=$HOME/.config/eve-offline/client-ci-pilot.tsv
ACCOUNT_PORT=${EVE_CI_ACCOUNT_PORT:-26093}
if [[ $SLOT != 0 ]]; then
    # shellcheck source=tools/client-ci/slot-lib.sh
    source "$REPO/tools/client-ci/slot-lib.sh"
    if ci_slot_vars "$REPO" "$HOME" "$SLOT" "${EVE_CLIENT_DIR:-tq-offline}"; then
        (( ROOTLESS )) || problem "--slot $SLOT needs --rootless: parallel slots are rootless jails, one namespace each"
        [[ -z $CENSUS_RUN ]] || problem "--slot and --census: the census has one live directory and one recorder set"
        [[ -z ${EVE_PREFIX:-} ]] || problem "--slot $SLOT makes its own prefix; EVE_PREFIX is set ($EVE_PREFIX)"
        # EVE_CLIENT would win over the slot's client name below (`: "${EVE_CLIENT:=...}"`),
        # and the slot would run, and stop, a client that is not its own.
        [[ -z ${EVE_CLIENT:-} ]] || problem "--slot $SLOT names its own client; EVE_CLIENT is set ($EVE_CLIENT)"
        EVE_PREFIX=$SLOT_PREFIX EVE_CLIENT_DIR=$SLOT_CLIENT_DIR EVE_PREFIX_ID=slot-$SLOT
        RL_STATE=$SLOT_RL_STATE LAUNCH_ENV=$SLOT_LAUNCH_ENV DEV_WORLD=$SLOT_WORLD RUN_BASE=$SLOT_RUNS
        PILOT_TOKENS_DEFAULT=$SLOT_PILOT_TOKENS ACCOUNT_PORT=$SLOT_ACCOUNT_PORT
        # Every jail call below (check, client, the bots' nsenter) is this slot's.
        export EVE_RL_SLOT=$SLOT
    else
        problem "--slot takes a number from 1 to 999, not '$SLOT'"
        SLOT=0
    fi
fi
# A lone run's jail is the one at out/client-ci/rootless: a stray EVE_RL_SLOT from the
# caller's environment must not send its jail calls into a slot's namespace.
[[ $SLOT != 0 ]] || unset EVE_RL_SLOT

SCENARIO=$SCEN_DIR/$NAME.py
# A scenario that flies declares how long it needs (EOCI_FCI_TIMEOUT_S, in
# seconds). The 600 s default is too short for those: a 1800 s scenario read
# as "never ran" at 600 s. An explicit --timeout still wins.
if (( ! TIMEOUT_GIVEN )) && [[ -f $SCENARIO ]]; then
    declared=$(sed -n 's/^EOCI_FCI_TIMEOUT_S[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$SCENARIO" | head -1)
    [[ -n $declared ]] && (( declared > TIMEOUT )) && TIMEOUT=$declared
fi
[[ -f $SCENARIO ]] || problem "no scenario at $SCENARIO
    available: $(list_scenarios | tr -s " \n" " ")"

command -v python3 >/dev/null || problem "python3 is needed to read the verdict file"

# The server's own arguments. A scenario that needs some declares them in its
# header, `# SERVER-ARGS: --no-concord`, and they apply when none follow `--`.
# Two clients need a roster two deep: before this, `--with-second` without
# `-- --character Alice --character Bob` gave the server ONE character, and the
# second client died in `EnterGameWithCharacter` (KeyError: None, the slot it
# was told to pick did not exist) while the first waited out its whole
# timeout for a pilot that never came (fci_skin_viewer, 2026-10-31, 480 s).
if [[ -f $SCENARIO && ${#SERVER_ARGS[@]} -eq 0 ]]; then
    decl=$(sed -n 's/^# SERVER-ARGS:[[:space:]]*//p' "$SCENARIO" | head -n 1)
    [[ -z $decl ]] || read -r -a SERVER_ARGS <<<"$decl"
fi
if (( SECOND )) && [[ " ${SERVER_ARGS[*]:-} " != *" --character "* ]]; then
    roster_names=(Alice Bob Carol Dave Erin Frank)
    roster_args=()
    for ((i = 0; i <= SECOND_SLOT && i < ${#roster_names[@]}; i++)); do
        roster_args+=(--character "${roster_names[i]}")
    done
    SERVER_ARGS=("${roster_args[@]}" ${SERVER_ARGS[@]+"${SERVER_ARGS[@]}"})
fi

# --bots-kit: BP-KIT's drill, for the bots only.
case "$BOTS_KIT" in
    ""|smartbomb|mjd) ;;
    *) problem "--bots-kit takes smartbomb or mjd, not $BOTS_KIT" ;;
esac
[[ -z $BOTS_KIT ]] || (( WITH_BOTS )) || problem "--bots-kit needs --with-bots"

# --with-bots: the checks that need no root, so --check says them too.
if (( WITH_BOTS )); then
    (( ROOTLESS )) || problem "--with-bots needs --rootless: the bots dial the server inside the jail's namespace"
    (( ! SECOND )) || problem "--with-bots and --with-second: accounts mode has no --character roster"
    wb_dev=0
    for a in ${SERVER_ARGS[@]+"${SERVER_ARGS[@]}"}; do
        case $a in
            --dev) wb_dev=1 ;;
            --character|--world|--accounts) problem "--with-bots: the server args may not hold $a (the run makes its own dev world, in accounts mode)" ;;
        esac
    done
    (( wb_dev )) || problem "--with-bots needs a dev server: put -- --dev after the flags"
fi

# The display. Ours now, via `tools/client-headless.sh`, which owns the
# container and the Vulkan and virtual-desktop arrangements that make the
# client draw without a monitor. With `--inherit-display` the caller supplies
# one instead and we only assert that it exists, because a client launched
# with no display dies deep in DXVK with an error that looks nothing like
# "there is no screen".
if [[ $HEADLESS -eq 1 ]]; then
    command -v docker >/dev/null || problem \
        "docker is needed for the headless display. Pass --inherit-display to
    use a screen you have already brought up, and export DISPLAY=:N for it."
else
    [[ -n ${DISPLAY:-} || -n ${WAYLAND_DISPLAY:-} ]] || problem \
        "--inherit-display given but there is no DISPLAY and no
    WAYLAND_DISPLAY. Export DISPLAY=:N for a screen that already exists, or
    drop the flag and let this script bring up a headless one."
fi

# An unattended run cannot answer a password prompt, and the things that need
# root (the jail, binding port 53, entering the namespace) are minutes into the
# run. Say so now rather than blocking on a prompt nobody is watching.
#
# **Either is enough.** The root-owned helper grants exactly the seven verbs
# this run needs with no password at all, which is the whole point of it; a
# cached timestamp is the older path and still works. Checking for the helper
# first means a machine that has it never asks for a password and never
# depends on one expiring mid-run -- a 600 s timeout outliving a 5-minute
# sudo timestamp is a real way for this to fail two thirds of the way in.
#
# **A stale or foreign helper refuses the run** (V4 C1, C2; IP15). Stale: it
# predates the template (today, the allowlist), so the client would launch
# with the host's sockets in view. Foreign: it was installed for another
# checkout, so its `client` verb would launch THAT tree's shim with THAT
# tree's launch-env, and its `dns` verb that tree's eve-dns -- a lane copy or
# worktree would test the main tree's previous run. Neither falls back to
# cached sudo: a helper that is there but wrong is a fact to fix, not to route
# around unattended. `install-jail-helper.sh --check` needs no root and
# honours EVE_JAIL_HELPER (a fake, in tests); 4 means "not installed".
JAIL_HELPER=/usr/local/sbin/eve-offline-jail
HAVE_HELPER=0
if (( ROOTLESS )); then
    # **Rootless (IP29): no root anywhere**, so none of the checks below
    # apply. The namespace must be makeable, and the bundle must exist,
    # because this path does not run up.sh, which is what makes it.
    "$REPO/tools/jail-rootless.sh" probe >/dev/null 2>&1 || problem \
        "--rootless: this host will not make an unprivileged user namespace
    (tools/jail-rootless.sh probe). Drop --rootless to use the helper."
else
helper_rc=0
helper_state=$("$REPO/tools/install-jail-helper.sh" --check 2>&1) || helper_rc=$?
if (( helper_rc != 0 && helper_rc != 4 )); then
    problem "the installed jail helper is not the one this checkout would install
    (exit $helper_rc from tools/install-jail-helper.sh --check):
$(printf '%s\n' "$helper_state" | sed 's/^/    /')"
fi
if (( helper_rc == 0 )) && [[ -x $JAIL_HELPER ]] && sudo -n "$JAIL_HELPER" status >/dev/null 2>&1; then
    HAVE_ROOT=1
    HAVE_HELPER=1
else
    HAVE_HELPER=0
    sudo -n true 2>/dev/null && HAVE_ROOT=1 || HAVE_ROOT=0
fi
# (A helper that is there but wrong is already a problem above; this one is
# for a machine with neither.)
(( HAVE_ROOT || helper_rc != 4 )) || problem \
    "no cached sudo credentials, and no jail helper. Either run 'sudo -v'
    first, or install the root-owned helper so this needs no password at all:
        tools/install-jail-helper.sh --diff     # read it
        sudo tools/install-jail-helper.sh
    The jail, the stub resolver and entering the namespace all need root, and
    an unattended run cannot answer a password prompt.
    docs/reference/unattended-root.md has what the helper does and does not grant."
fi

# The census (CEN-3). Everything it needs is checked here, with the rest.
CENSUS_LIVE=$REPO/out/census/live
CENSUS_SINK=$REPO/tools/census_sink.py
CENSUS_CANARY=$REPO/tools/census_canary.py
if [[ -n $CENSUS_RUN ]]; then
    CENSUS_RUN=$(realpath -m -- "$CENSUS_RUN")
    [[ $CENSUS_MODE == observe || $CENSUS_MODE == handshake ]] \
        || problem "--census-mode is observe or handshake, not '$CENSUS_MODE'"
    (( ! KEEP )) || problem "--census and --keep: a census collects its files on the way out, so it cannot leave them running"
    (( ! SECOND )) || problem "--census and --with-second: one client per census, so every event is that client's"
    for f in census_sink.py census_proxy.py census_canary.py census_wire.py census_report.py; do
        [[ -f $REPO/tools/$f ]] || problem "--census: no tools/$f (packet CEN-1)"
    done
    [[ -x $REPO/tools/census/bin/xdg-open ]] || problem "--census: no browser stand-ins at tools/census/bin (CEN-1)"
    [[ -z $(pgrep -f "^python3 -I $CENSUS_SINK " 2>/dev/null || true) ]] \
        || problem "--census: a census sink from this checkout is already running; census-down first"
    if (( ! ROOTLESS && ! HAVE_HELPER )); then
        problem "--census needs the jail helper (its census-up/-down/-dump/-verify verbs), or --rootless.
    Cached sudo is not enough: the census exists only as helper verbs."
    fi
    [[ -s $REPO/out/census/paths.json ]] || echo "client-ci: note: no out/census/paths.json; a subject's report will be INCONCLUSIVE" >&2
elif (( CENSUS_CONTROL )); then
    problem "--census-control without --census: the control's weakened launch exists only inside a census"
elif (( CENSUS_NOENV )); then
    problem "--census-noenv without --census: the no-env launch exists only inside a census"
fi
(( ! (CENSUS_NOENV && CENSUS_CONTROL) )) || problem "--census-noenv and --census-control are different copies' roles"

# The prefix, named exactly as run-client.sh names it (tools/prefix-lib.sh),
# so that EVE_PREFIX_ID=8501 steers both this script and the client launch at
# the second player's prefix. The default, `offline`, is the offline client's
# own prefix (O19: tools/offline-prefix.sh); the online prefix 8500 is refused.
#
# NOTE a trap here: `EVE_PREFIX` means the prefix directory (the one holding
# `pfx/`) to run-client.sh and ".../pfx/drive_c" to destiny-ref.sh. We follow
# run-client.sh, and check the result exists rather than trusting it.
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_PREFIX_ID:=offline}"
: "${EVE_CLIENT_DIR:=tq-offline}"
if [[ -z ${EVE_PREFIX:-} ]]; then
    EVE_PREFIX=$(eve_prefix_of "$EVE_PREFIX_ID" "$HOME" "$STEAM_ROOT") \
        || problem "EVE_PREFIX_ID must be 'offline' or a number, not '$EVE_PREFIX_ID'"
fi
: "${EVE_CLIENT:=$EVE_PREFIX/pfx/drive_c/CCP/EVE/$EVE_CLIENT_DIR}"
eve_refuse_online client-ci.sh "EVE_PREFIX=$EVE_PREFIX" "EVE_CLIENT=$EVE_CLIENT" \
    "the client's ResFiles=$EVE_CLIENT/../ResFiles"
# The content bundle this client's build may use (tools/content-lib.sh): the
# pinned build's out/content, or out/content-<build> for an unverified one,
# which also needs `--client-build` on the server.
# shellcheck source=tools/content-lib.sh
source "$REPO/tools/content-lib.sh"
eve_content_select "$EVE_CLIENT" "$REPO" || {
    problem "no content bundle can be chosen for $EVE_CLIENT (see above)"
    CONTENT_REL=out/content-invalid CONTENT=$REPO/out/content-invalid EVE_SERVER_BUILD_ARGS=()
}
if (( ROOTLESS )); then
    # This path does not run up.sh, which is what makes the bundle.
    [[ -s $CONTENT/MANIFEST.tsv ]] || problem \
        "--rootless: no content bundle at $CONTENT_REL. Make it once with
    EVE_CLIENT_DIR=$(basename "$EVE_CLIENT") tools/up.sh --no-client, or tools/analysis.sh python3 -I /work/tools/content.py ..."
fi
[[ -d $EVE_PREFIX/pfx || $EVE_PREFIX_ID != offline ]] || problem "no offline prefix at
    $EVE_PREFIX
    The jailed client has a Wine prefix of its own (O19). Make it once with:
        tools/offline-prefix.sh"
if [[ $SLOT != 0 ]]; then
    [[ -f $EVE_CLIENT/bin64/exefile.exe ]] || problem "slot $SLOT has no client at $EVE_CLIENT. Make it with:
        tools/client-ci/slot-prefix.sh ensure $SLOT"
fi
DRIVE_C=$EVE_PREFIX/pfx/drive_c
LOGDIR=$DRIVE_C/eve-logs
CI_DIR=$LOGDIR/client-ci

[[ -d $DRIVE_C ]] || problem "$DRIVE_C does not exist. If EVE_PREFIX is set it
    must be the compatdata directory and not its drive_c -- that is how
    run-client.sh reads it."
[[ ! -d $DRIVE_C || -d $LOGDIR ]] || problem "$LOGDIR does not exist. Launch the
    client once (tools/up.sh) so /fileLogDirectory creates it."

# Same pattern run-client.sh uses: the client's Windows command line carries
# the install directory name, so this matches OUR copy and not the owner's real
# install and not somebody else's EVE.
CLIENT_PAT="$(basename "$EVE_CLIENT")"'.bin64.exefile'


# **`pgrep -f PATTERN` matches any command line containing the pattern --
# including another `pgrep -f PATTERN`.** Not hypothetical, and it cost two
# two-player runs on 2026-10-24: the stagger loop below polls for the first
# client with this exact pattern while `run-client.sh` is checking that no
# client is already running, so the two pgreps saw each other and the launch
# was refused, naming `pgrep -f tq-offline.bin64.exefile` as the client it had
# found. Every `pgrep -f`/`pkill -f` on a client pattern in this script goes
# through these two functions instead, and they decide by the process's own
# `comm`: a pgrep is not a client.
#
# Reading `/proc` rather than piping keeps the whole thing immune to the other
# half of that failure -- a diagnostic that exits non-zero and takes the
# script with it under `set -o pipefail`. See `run-client.sh`'s note.
client_pids() {
    local pid comm
    for pid in $(pgrep -f "$1" 2>/dev/null || true); do
        comm=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        case $comm in pgrep | pkill | '') continue ;; esac
        printf '%s\n' "$pid"
    done
}

# The same, as one line of evidence per process, for a refusal to print.
client_lines() {
    local pid out=""
    for pid in $(client_pids "$1"); do
        out+="$pid $(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)|"
    done
    printf '%s' "$out"
}

# The second player, if asked for. `client2-setup.sh` builds prefix 8501 with
# the client copied to `tq-offline-2` -- a DIFFERENT directory name, which is
# what makes two clients distinguishable here at all: `CLIENT_PAT` is built
# from that name, so neither client's pattern matches the other's process and
# `stop_client` cannot take both down. Verified 2026-10-23 by running the whole
# single-client flow against 8501 (18/18, 41s).
if [[ $SECOND -eq 1 ]]; then
    SECOND_PREFIX_ID=${EVE_SECOND_PREFIX_ID:-8501}
    SECOND_CLIENT_DIR=${EVE_SECOND_CLIENT_DIR:-tq-offline-2}
    SECOND_PREFIX=$STEAM_ROOT/steamapps/compatdata/$SECOND_PREFIX_ID
    if [[ $SLOT != 0 ]]; then
        # The slot's own second prefix; prefix 8501 is one for the whole machine.
        SECOND_PREFIX_ID=slot-$SLOT-2 SECOND_CLIENT_DIR=$SLOT_CLIENT_DIR2 SECOND_PREFIX=$SLOT_PREFIX2
    fi
    SECOND_DRIVE_C=$SECOND_PREFIX/pfx/drive_c
    SECOND_CI_DIR=$SECOND_DRIVE_C/eve-logs/client-ci
    SECOND_CLIENT=$SECOND_DRIVE_C/CCP/EVE/$SECOND_CLIENT_DIR
    eve_refuse_online client-ci.sh "the second prefix=$SECOND_PREFIX" \
        "the second client's ResFiles=$SECOND_CLIENT/../ResFiles"
    SECOND_PAT="$SECOND_CLIENT_DIR"'.bin64.exefile'
    [[ -d $SECOND_CLIENT ]] || problem "--with-second needs the second client at
    $SECOND_CLIENT
    Build it once with: $( [[ $SLOT != 0 ]] && echo "tools/client-ci/slot-prefix.sh ensure $SLOT --second" || echo tools/client2-setup.sh)"
    [[ ! -d $SECOND_DRIVE_C/eve-logs ]] && problem "$SECOND_DRIVE_C/eve-logs does
    not exist. Launch the second client once so /fileLogDirectory creates it."
    second_running=$(client_lines "$SECOND_PAT")
    if [[ -n $second_running ]]; then
        problem "a second client from $SECOND_CLIENT_DIR is already running:
    $second_running"
    fi
    [[ $SECOND_PREFIX_ID == "$EVE_PREFIX_ID" ]] && problem "the second client's
    prefix ($SECOND_PREFIX_ID) is the first client's. Two clients cannot share
    one prefix -- they share a wineserver, a registry and a cache directory."
fi
first_running=$(client_lines "$CLIENT_PAT")
if [[ -n $first_running ]]; then
    problem "a client from $(basename "$EVE_CLIENT") is already running:
    $first_running
    Close it first. We will not kill a client somebody else started, and two
    clients cannot share one jail."
fi

# Is there a server up, and is it ours to stop?
#
# This is the `up.sh --keep` question in reverse. A server running with
# `--scenario <name>` on its command line was started for exactly this run, so
# adopt it and leave it alone afterwards. A server running with anything else is
# somebody's session: refuse, rather than tearing it down to start our own
# (which would also tear the jail out from under their client).
SERVER_BIN=$REPO/server/target/debug/$SRV
RUNNING_SERVER=$(pgrep -af "^$SERVER_BIN " 2>/dev/null | head -1 || true)
OWN_SERVER=1
ADOPTED=
# Rootless starts its own server inside its own namespace, so a server on the
# host (somebody's session) is neither in the way nor reachable.
(( ROOTLESS )) && RUNNING_SERVER=
if [[ -n $RUNNING_SERVER ]]; then
    if [[ $PASS_SCENARIO -eq 1 && $RUNNING_SERVER == *"--scenario $NAME"* ]]; then
        OWN_SERVER=0; ADOPTED="started with --scenario $NAME"
    elif [[ $REUSE_SERVER -eq 1 ]]; then
        OWN_SERVER=0; ADOPTED="--reuse-server; the scenario trigger may not be armed"
    else
        problem "an eve-server is already running and it is not ours:
    ${RUNNING_SERVER#* }
    Stop it, or pass --reuse-server if it really was started for this scenario.
    Starting our own would tear down the jail under whatever is using it."
    fi
fi

# Does the server we are about to run understand --scenario at all? The flag
# belongs to the coordinator, and until it lands eve-server exits with
# "unknown argument --scenario" -- which up.sh reports as "a service died on
# startup", three steps from the cause. One --help is cheaper than that.
if [[ $PASS_SCENARIO -eq 1 && $OWN_SERVER -eq 1 && -x $SERVER_BIN ]]; then
    "$SERVER_BIN" --help 2>&1 | grep -q -- '--scenario' || problem \
        "$(basename "$SERVER_BIN") has no --scenario flag yet (the coordinator
    wires it). Rebuild once it has landed, or run with --no-scenario-flag to
    exercise everything up to the trigger."
fi

if [[ ${#PROBLEMS[@]} -gt 0 ]]; then
    echo "client-ci: refusing to run $NAME. ${#PROBLEMS[@]} problem(s):" >&2
    for p in "${PROBLEMS[@]}"; do echo "  - $p" >&2; done
    exit $EX_REFUSED
fi

say "preflight ok"
echo "   scenario:  $SCENARIO"
echo "   prefix:    $EVE_PREFIX"
[[ $SLOT == 0 ]] || echo "   slot:      $SLOT (own prefix, client $(basename "$EVE_CLIENT"), jail state ${RL_STATE#"$REPO"/}, runs ${RUN_BASE#"$REPO"/})"
[[ $SECOND -eq 1 ]] && echo "   second:    $SECOND_PREFIX (slot $SECOND_SLOT, $SECOND_CLIENT_DIR)"
echo "   display:   DISPLAY=${DISPLAY:-<unset>} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-<unset>}"
[[ -n $CENSUS_RUN ]] && echo "   census:    $CENSUS_RUN ($( (( CENSUS_CONTROL )) && echo control || echo subject), sink $CENSUS_MODE)"
if [[ $OWN_SERVER -eq 0 ]]; then
    echo "   server:    adopting the one already running ($ADOPTED); it will be left alone"
else
    echo "   server:    ours to start and ours to stop"
fi
# --check exists so that the whole host side can be exercised, including every
# refusal above, without starting a server or a client -- which is also the only
# part of this script that works without root.
if [[ $CHECK_ONLY -eq 1 ]]; then
    echo
    echo "--check: stopping here, nothing started."
    exit 0
fi

# A regression row (name@on, name@off) names its own directory, so two rows of one scenario never share one.
RUNDIR=$RUN_BASE/${EOCI_RUN_KEY:-$NAME}
rm -rf "$RUNDIR"; mkdir -p "$RUNDIR"   # one run's worth; out/ is gitignored
UP_LOG=$RUNDIR/up.log
SERVER_LOG=$REPO/logs/eve-server.log
(( ROOTLESS )) && SERVER_LOG=$RUNDIR/eve-server.log
LAUNCH_LOG=$RUNDIR/client-launch.log
RESULT_HOST=$CI_DIR/$NAME.json
[[ $SECOND -eq 1 ]] && SECOND_RESULT=$SECOND_CI_DIR/$NAME.json

# ---------------------------------------------------------------- cleanup

UP_PID=
LAUNCHER_PID=
LAUNCHER_PID2=
DEPLOYED=0
DEPLOYED2=0

# Kill a process and everything it spawned, leaf first.
#
# Needed because the launch chain is
#   sudo ip netns exec NS  ->  unshare  ->  setpriv (to the user)  ->  proton  ->  exefile.exe
# and its first links run as ROOT, so a plain kill from our own user bounces
# off them. That is the same `kill || sudo -n kill` dance up.sh does in its
# trap, and for the same reason: an orphaned link holds the Proton prefix and
# breaks the NEXT run, which is exactly the class of bug up.sh's reaper exists
# to clean up after.
kill_tree() {
    local pid=$1 child
    for child in $(pgrep -P "$pid" 2>/dev/null || true); do kill_tree "$child"; done
    kill -0 "$pid" 2>/dev/null || return 0
    kill "$pid" 2>/dev/null && return 0
    sudo -n kill "$pid" 2>/dev/null && return 0
    kill -0 "$pid" 2>/dev/null || return 0
    echo "   WARNING: could not stop pid $pid ($(ps -o comm= -p "$pid" 2>/dev/null))" >&2
}

# `stop_client [pattern] [label]` -- the pattern defaults to the first
# client's. The two clients are distinguishable precisely because their
# install directories are named differently (`tq-offline` vs `tq-offline-2`),
# so neither pkill can reach the other's process.
stop_client() {
    local pat=${1:-$CLIENT_PAT} what=${2:-client}
    local pids
    pids=$(client_pids "$pat")
    [[ -z $pids ]] && return 0
    echo "   stopping the $what"
    # By pid and not by pattern: `pkill -f` on a client pattern would also
    # signal anything scanning for that pattern, which includes our own
    # pgreps.
    kill $pids 2>/dev/null || true
    local i
    for i in $(seq 1 15); do
        [[ -z $(client_pids "$pat") ]] && return 0
        sleep 1
    done
    # A wedged client must not be able to hold the whole run open: this script
    # is the thing that is supposed to always terminate.
    echo "   $what did not exit; SIGKILL" >&2
    kill -9 $(client_pids "$pat") 2>/dev/null || true
    sleep 2
}

# ---------------------------------------------------------------- the census
#
# CEN-3 (docs/guides/census.md). The recorders write to out/census/live/ (the sink,
# the proxy, the shim's browser stand-ins and the canaries); this run's
# share is APPENDED to $CENSUS_RUN on the way out, so every scenario of one
# copy builds one run. A fault in a segment (a recorder with no clean
# start/stop pair, a verify that fails after census-down) goes to
# $CENSUS_RUN/faults.txt, which tools/census.sh turns into INVALID.
CENSUS_UP=0
census_fault() { printf '%s: %s\n' "$NAME" "$*" >> "$CENSUS_RUN/faults.txt"; echo "   CENSUS FAULT: $*" >&2; }

# A fresh live directory, the caller's: the files the client's side appends
# to must exist before the view binds them back, and the mode is a file
# because the helper's verbs take no arguments.
census_prepare_live() {
    rm -rf "$CENSUS_LIVE"
    mkdir -p -m 0700 "$CENSUS_LIVE" "$CENSUS_RUN"
    printf '%s\n' "$CENSUS_MODE" > "$CENSUS_LIVE/mode"
    : > "$CENSUS_LIVE/exec.log"
    : > "$CENSUS_LIVE/canary.jsonl"
}

census_dns_log() { if (( ROOTLESS )); then echo "$RUNDIR/eve-dns.log"; else echo "$REPO/logs/eve-dns.log"; fi; }

# After the jail is verified, before anything is deployed.
census_begin() {
    say "census ($( (( CENSUS_CONTROL )) && echo control || echo subject), sink $CENSUS_MODE)"
    CENSUS_DNS_OFF=$(stat -c %s "$(census_dns_log)" 2>/dev/null || echo 0)
    touch "$RUNDIR/.census-mark"
    rm -f "$CI_DIR/$NAME.census.jsonl"
    if (( ! ROOTLESS )); then
        census_prepare_live
        CENSUS_UP=1
        if ! sudo -n "$JAIL_HELPER" census-up >"$RUNDIR/census-up.log" 2>&1; then
            sed 's/^/   /' "$RUNDIR/census-up.log" >&2
            census_fault "census-up failed"
            echo "client-ci: census-up failed -- refusing to launch the client." >&2
            exit $EX_REFUSED
        fi
        census_check=(sudo -n "$JAIL_HELPER" census-verify)
    else
        CENSUS_UP=1
        census_check=("$REPO/tools/jail-rootless.sh" check)   # the gate expects census0 now
    fi
    if ! timeout 120 "${census_check[@]}" >"$RUNDIR/census-verify.log" 2>&1; then
        sed 's/^/   /' "$RUNDIR/census-verify.log" >&2
        census_fault "census-verify failed"
        echo "client-ci: the census is not in place -- refusing to launch the client." >&2
        exit $EX_REFUSED
    fi
    echo "   in place (census-verify: $RUNDIR/census-verify.log)"
    # run-client.sh and rootless_client write these into the launch-env;
    # jail-client.sh honours them only if census0 really is there.
    export EVE_CENSUS=1
    if (( CENSUS_CONTROL )); then export EVE_CENSUS_CONTROL=1; else export EVE_CENSUS_CONTROL=; fi
    if (( CENSUS_NOENV )); then export EVE_CENSUS_NOENV=1; else export EVE_CENSUS_NOENV=; fi
}

# One recorder log: exactly one start and one clean stop, then appended.
census_take_jsonl() {   # FILE
    local f=$CENSUS_LIVE/$1 starts stops
    if [[ ! -f $f ]]; then census_fault "no $1 from this segment"; return 0; fi
    starts=$(grep -c '"event": "start"' "$f" || true)
    stops=$(grep -c '"event": "stop"' "$f" || true)
    if [[ $starts != 1 || $stops != 1 ]] || grep -q '"reason": "error"' "$f"; then
        census_fault "$1 has $starts start and $stops stop lines (a recorder died or restarted)"
    fi
    cat "$f" >> "$CENSUS_RUN/$1"
}

# After the client has gone, before the stack goes: dump R2, take the census
# down (SIGTERM first, so each recorder writes its stop line), prove the jail
# is as it was, and append this segment to the run.
census_end() {
    local code=$1 off end dns dns_end crash nft
    (( CENSUS_UP )) || return 0
    CENSUS_UP=0
    say "census: collecting into $CENSUS_RUN"
    # R3's end is taken NOW, before census-down and the `verify` after it: that
    # verify looks up sentry.io, clientresources.eveonline.com and the server
    # name from inside the jail, and used to land in this segment's slice as
    # "client" lookups (the 2026-10-30 report's sentry.io LEAK lines, in
    # segments where no client had started).
    dns_end=$(stat -c %s "$(census_dns_log)" 2>/dev/null || echo 0)
    # No client launched in this segment (refused before the launch): say so,
    # and census.sh will not present the run's rows as a client's.
    [[ -n $LAUNCHER_PID ]] || printf '%s\n' "$NAME" >> "$CENSUS_RUN/noclient.txt"
    nft=$CENSUS_RUN/nft-$NAME.json
    if (( ROOTLESS )); then
        "$REPO/tools/jail-rootless.sh" census-dump >"$nft" 2>"$RUNDIR/census-dump.err" || census_fault "census-dump failed"
        "$REPO/tools/jail-rootless.sh" census-stop >>"$RUNDIR/census-down.log" 2>&1 || census_fault "census-stop failed"
    else
        sudo -n "$JAIL_HELPER" census-dump >"$nft" 2>"$RUNDIR/census-dump.err" || census_fault "census-dump failed"
        sudo -n "$JAIL_HELPER" census-down >"$RUNDIR/census-down.log" 2>&1 || census_fault "census-down failed"
        # R-CENSUS §1.4: afterwards `verify` must pass unchanged.
        timeout 120 sudo -n "$JAIL_HELPER" verify >"$RUNDIR/census-after-verify.log" 2>&1 \
            || census_fault "verify fails after census-down ($RUNDIR/census-after-verify.log)"
    fi
    census_take_jsonl tun.jsonl
    census_take_jsonl proxy.jsonl
    cat "$CENSUS_LIVE/exec.log" >> "$CENSUS_RUN/exec.log" 2>/dev/null || true
    # What Wine's own canaries did (output, exit status, timing), for a canary that went missing.
    cat "$CENSUS_LIVE/wine-canary.log" >> "$CENSUS_RUN/wine-canary.log" 2>/dev/null || true
    for w in "$CENSUS_LIVE"/wine-*.out; do
        [[ -f $w ]] && { printf '== %s (%s)\n' "$(basename "$w")" "$NAME"; head -c 4000 "$w"; } >> "$CENSUS_RUN/wine-canary.out" 2>/dev/null
    done
    rm -f "$CENSUS_LIVE"/wine-*.out "$CENSUS_LIVE/wine-canary.log" 2>/dev/null || true
    cat "$CENSUS_LIVE/canary.jsonl" >> "$CENSUS_RUN/canary.jsonl" 2>/dev/null || true
    # The client window and its canary rows (CEN-4's scenarios write them).
    if [[ -f $CI_DIR/$NAME.census.jsonl ]]; then
        tr -d '\r' < "$CI_DIR/$NAME.census.jsonl" >> "$CENSUS_RUN/canary.jsonl"
    fi
    # R3: this segment's bytes of the resolver's log, census-up to now.
    dns=$(census_dns_log)
    touch "$CENSUS_RUN/dns.log"
    if [[ -f $dns ]]; then
        end=$dns_end
        off=${CENSUS_DNS_OFF:-0}
        (( end >= off )) || off=0   # truncated meanwhile: take it all
        # head first: it reads exactly what it prints, so no SIGPIPE under pipefail.
        head -c "$end" "$dns" | tail -c +"$((off + 1))" >> "$CENSUS_RUN/dns.log"
    else
        census_fault "no resolver log at $dns"
    fi
    # New crash-DB files since census-up (Crashpad's *.dmp and reports/).
    crash=$(find "$EVE_PREFIX/pfx" -newer "$RUNDIR/.census-mark" -type f \
                \( -iname '*.dmp' -o -path '*/reports/*' \) 2>/dev/null || true)
    [[ -n $crash ]] && printf '%s\n' "$crash" >> "$CENSUS_RUN/crash.txt"
    printf '%s\t%s\n' "$NAME" "$code" >> "$CENSUS_RUN/segments.tsv"
    echo "   segment $NAME appended (client-ci exit $code)"
}

# `cleanup <code>` so that an interrupted run does not exit 0. Called bare
# from the EXIT trap, where $? is the real status.
# The client's audio and graphics, forced for this run on each client it starts
# (tools/client-ci/quiet-settings.py). `restore` takes no client: it covers them all.
QUIET_CLIENTS=()
quiet_settings() {   # apply DRIVE_C CLIENT_DIR | restore
    local entry
    if [[ $1 == apply ]]; then
        QUIET_CLIENTS+=("$2|$3")
        python3 -I "$REPO/tools/client-ci/quiet-settings.py" apply "$2" "$3"
        return
    fi
    for entry in ${QUIET_CLIENTS[@]+"${QUIET_CLIENTS[@]}"}; do
        python3 -I "$REPO/tools/client-ci/quiet-settings.py" restore "${entry%|*}" "${entry##*|}" || true
    done
}

cleanup() {
    local code=${1:-$?}
    trap - EXIT INT TERM           # never re-enter this
    if [[ $KEEP -eq 1 ]]; then
        say "left running (--keep)"
        [[ -n $UP_PID ]] && echo "   up.sh pid $UP_PID (jail + services); kill it to tear down"
        [[ $DEPLOYED -eq 1 ]] && echo "   scenario still deployed: $CI_DIR/$NAME.py"
        [[ $DEPLOYED2 -eq 1 ]] && echo "   scenario still deployed: $SECOND_CI_DIR/$NAME.py"
        [[ $DISPLAY_UP -eq 1 ]] && echo "   headless display still up, and the
   prefix still has a wine virtual desktop set. Put both back with:
       tools/client-headless.sh desktop off && tools/client-headless.sh stop"
        [[ -n $(client_pids "$CLIENT_PAT") ]] && echo "   client still running"
        [[ ${#QUIET_CLIENTS[@]} -eq 0 ]] || echo "   the client still has CI's quiet settings; the next run puts the owner's back first"
        exit "$code"
    fi
    say "cleaning up"
    # The client first: run-client.sh's wait loop ends when the client goes, so
    # killing the client is what lets the launcher return.
    if [[ -n $LAUNCHER_PID2 ]]; then
        stop_client "$SECOND_PAT" "second client"
        for _ in $(seq 1 10); do kill -0 "$LAUNCHER_PID2" 2>/dev/null || break; sleep 1; done
        kill -0 "$LAUNCHER_PID2" 2>/dev/null && kill_tree "$LAUNCHER_PID2"
    fi
    if [[ -n $LAUNCHER_PID ]]; then
        stop_client
        # run-client.sh's own loop ends when the client goes, so give it a
        # moment to return by itself before taking the chain apart.
        for _ in $(seq 1 10); do kill -0 "$LAUNCHER_PID" 2>/dev/null || break; sleep 1; done
        kill -0 "$LAUNCHER_PID" 2>/dev/null && kill_tree "$LAUNCHER_PID"
    fi
    # The bots (ours, by pid), before the server they are connected to.
    if [[ -n $BOTS_PID ]]; then
        kill "$BOTS_PID" 2>/dev/null || true
        for _ in $(seq 1 10); do kill -0 "$BOTS_PID" 2>/dev/null || break; sleep 1; done
        kill -0 "$BOTS_PID" 2>/dev/null && kill -9 "$BOTS_PID" 2>/dev/null
        wait "$BOTS_PID" 2>/dev/null || true
    fi
    # The owner's settings file, now that the clients that write it are gone.
    quiet_settings restore
    # The census, once the client has gone and while the stack is still up.
    census_end "$code"
    # The display, once the client that was using it has gone. The virtual
    # desktop is a registry key in the owner's prefix, so putting it back is
    # not optional tidiness -- a left-behind `Desktop=Default` would windowbox
    # their next ordinary launch.
    if [[ $SECOND_DESKTOP -eq 1 ]]; then
        EVE_PREFIX_ID=$SECOND_PREFIX_ID EVE_PREFIX=$SECOND_PREFIX "$REPO/tools/client-headless.sh" desktop off >&2 2>/dev/null || true
    fi
    if [[ -n $GS_PID ]]; then
        kill "$GS_PID" 2>/dev/null || true
        wait "$GS_PID" 2>/dev/null || true
    fi
    if [[ $DISPLAY_UP -eq 1 ]]; then
        EVE_PREFIX=$EVE_PREFIX "$REPO/tools/client-headless.sh" desktop off >&2 2>/dev/null || true
        # The Xvfb that `desktop` talks to is one for all slots: the regression runner
        # (or whoever started it) stops it, not the first slot to finish.
        [[ $SLOT != 0 ]] || "$REPO/tools/client-headless.sh" stop >&2 2>/dev/null || true
    fi
    # The deployed scenario is ours, and leaving it behind would make the next
    # run's "did it write a verdict" question ambiguous.
    if [[ $DEPLOYED2 -eq 1 ]]; then
        rm -f "$SECOND_CI_DIR/$NAME.py"
        echo "   removed $SECOND_CI_DIR/$NAME.py"
    fi
    if [[ $DEPLOYED -eq 1 ]]; then
        rm -f "$CI_DIR/$NAME.py"
        echo "   removed $CI_DIR/$NAME.py"
    fi
    # Only ever stop a server WE started. SIGTERM reaches up.sh's own exit
    # trap, which kills eve-server and eve-dns and tears the jail down -- so
    # this must be a polite signal and not a kill -9.
    if [[ -n $UP_PID ]] && kill -0 "$UP_PID" 2>/dev/null; then
        echo "   stopping the stack (up.sh pid $UP_PID)"
        kill -TERM "$UP_PID" 2>/dev/null || true
        local i
        for i in $(seq 1 30); do kill -0 "$UP_PID" 2>/dev/null || break; sleep 1; done
        if kill -0 "$UP_PID" 2>/dev/null; then
            kill -9 "$UP_PID" 2>/dev/null || true
            echo "   up.sh would not stop; the jail may still be up." >&2
            echo "   tear it down with: sudo tools/jail-teardown.sh" >&2
        fi
    elif [[ $OWN_SERVER -eq 0 ]]; then
        echo "   left the adopted server and its jail alone"
    fi
    [[ -n ${CI_RUNTIME_DIR:-} ]] && rm -rf "$CI_RUNTIME_DIR"
    echo "   artefacts: $RUNDIR"
    exit "$code"
}
trap cleanup EXIT
trap 'cleanup 130' INT
trap 'cleanup 143' TERM

# ---------------------------------------------------------------- the server

if [[ $OWN_SERVER -eq 1 ]]; then
    # A --dev server keeps its world between runs, so a scenario would inherit
    # the pilot the last one left in space (fci_dev_mode and fci_gang_summon
    # failed on exactly that). out/dev/world.dat is the owner's dev world
    # (tools/dev-world.sh) and is never touched: unless the caller names a
    # world (-- --dev --world FILE), a dev run gets its own, fresh each time.
    dev_run=0; dev_world_named=0
    for a in ${SERVER_ARGS[@]+"${SERVER_ARGS[@]}"}; do
        [[ $a == --dev ]] && dev_run=1
        [[ $a == --world ]] && dev_world_named=1
    done
    if (( dev_run && ! dev_world_named )); then
        say "dev world reset (${DEV_WORLD#"$REPO"/})"
        rm -rf "$DEV_WORLD"
        mkdir -p "$DEV_WORLD"
        SERVER_ARGS+=(--world "${DEV_WORLD#"$REPO"/}/world.dat")
        # A slot's server must not depend on the cwd either.
        [[ $SLOT == 0 ]] || SERVER_ARGS[${#SERVER_ARGS[@]}-1]=$DEV_WORLD/world.dat
        # The pilot's account and the bots' control link both name this
        # directory, so it must not depend on the server's cwd.
        (( ! WITH_BOTS )) || SERVER_ARGS[${#SERVER_ARGS[@]}-1]=$DEV_WORLD/world.dat
    fi
    if (( WITH_BOTS )); then
        # Accounts mode: the client is one account and each bot another; the
        # catch-all gives every login user 1.
        SERVER_ARGS+=(--world-accounts --bot-control "$DEV_WORLD")
    fi
    # --catch-all: the scenarios bring their own roster (--character) and
    # log in as `offline`, so the CI server is the catch-all one, loopback
    # and jail only, never world accounts (ACC2; docs/lanes/acc2tools.md).
    up_args=(--no-client --catch-all)
    [[ $NO_BUILD -eq 1 ]] && up_args+=(--no-build)
    srv=()
    [[ $PASS_SCENARIO -eq 1 ]] && srv+=(--scenario "$NAME")
    srv+=("${SERVER_ARGS[@]:-}")
    # Drop the empty element ${SERVER_ARGS[@]:-} leaves when there are no
    # trailing args; eve-server rejects an empty argument.
    pruned=(); for a in ${srv[@]+"${srv[@]}"}; do [[ -n $a ]] && pruned+=("$a"); done
    [[ ${#pruned[@]} -gt 0 ]] && up_args+=(--) && up_args+=("${pruned[@]}")

    if (( ROOTLESS )); then
        # up.sh's server line, inside the rootless namespace. Its build, and
        # the bundle it reads; captures go to this run's directory.
        if (( ! NO_BUILD )); then
            say "build"
            "$REPO/tools/dev.sh" cargo build --locked --workspace --bins >&2
        fi
        if (( WITH_BOTS )); then
            # The pilot: a gm account on the fresh dev world, made before the
            # server by a one-shot (tools/bot-accounts.sh), then unmarked as
            # a bot: it is the human's.
            BOTS_BIN=$REPO/server/target/debug/eve-bots
            [[ -x $SERVER_BIN && -x $BOTS_BIN ]] || { echo "client-ci: --with-bots: no $SERVER_BIN or $BOTS_BIN (run without --no-build)" >&2; exit $EX_REFUSED; }
            PILOT_NAME=cipilot
            PILOT_TOKENS=${EVE_CI_PILOT_TOKENS:-$PILOT_TOKENS_DEFAULT}
            say "pilot account $PILOT_NAME"
            wb_world=$DEV_WORLD/world.dat
            bash "$REPO/tools/bot-accounts.sh" --bin "$REPO/server/target/debug" --world "$wb_world" \
                --tokens "$PILOT_TOKENS" --logs "$RUNDIR" --port "$ACCOUNT_PORT" --wait 300 \
                "$PILOT_NAME" -- --dev --content "$CONTENT" --authored "$REPO/content" \
                ${EVE_SERVER_BUILD_ARGS[@]+"${EVE_SERVER_BUILD_ARGS[@]}"} >"$RUNDIR/pilot-account.log" 2>&1 \
                || { tail -n 20 "$RUNDIR/pilot-account.log" >&2; echo "client-ci: --with-bots: the pilot account failed ($RUNDIR/pilot-account.log)" >&2; exit $EX_REFUSED; }
            "$SERVER_BIN" --world "$wb_world" --mark-bot "$PILOT_NAME" off >>"$RUNDIR/pilot-account.log" 2>&1 \
                || { echo "client-ci: --with-bots: could not unmark $PILOT_NAME as a bot" >&2; exit $EX_REFUSED; }
            # The token is the line's last field; it stays in this variable.
            EVE_SSO_TOKEN=$(awk -v n="$PILOT_NAME" '!/^#/ && $1 == n { t = $NF } END { print t }' "$PILOT_TOKENS")
            [[ $EVE_SSO_TOKEN == "$PILOT_NAME":* ]] || { echo "client-ci: --with-bots: no login for $PILOT_NAME in $PILOT_TOKENS" >&2; exit $EX_REFUSED; }
            export EVE_SSO_TOKEN
        fi
        rl_srv=("$SERVER_BIN" --bind 10.137.0.1:26000 --capture "$RUNDIR/captures"
                --content "$CONTENT" --authored "$REPO/content" --chat
                ${EVE_SERVER_BUILD_ARGS[@]+"${EVE_SERVER_BUILD_ARGS[@]}"})
        [[ ${#pruned[@]} -gt 0 ]] && rl_srv+=("${pruned[@]}")
        say "server (rootless: ${rl_srv[*]})"
        rl_flags=()
        # The census starts with the namespace, so its live directory first.
        if [[ -n $CENSUS_RUN ]]; then census_prepare_live; rl_flags=(--census); fi
        "$REPO/tools/jail-rootless.sh" ${rl_flags[@]+"${rl_flags[@]}"} up "$RUNDIR" -- "${rl_srv[@]}" </dev/null >"$UP_LOG" 2>&1 &
    else
    say "server (up.sh ${up_args[*]})"
    # stdin from /dev/null on purpose: up.sh calls `sudo -v`, and with no
    # terminal it fails immediately instead of waiting on a prompt that an
    # unattended run will never answer. The sudo check above means it should
    # not come to that.
    EVE_CLIENT="$EVE_CLIENT" "$REPO/tools/up.sh" "${up_args[@]}" </dev/null >"$UP_LOG" 2>&1 &
    fi
    UP_PID=$!
    echo "   waiting for the stack (up to ${READY_TIMEOUT}s; includes the container build)"
    ready=0
    deadline=$((SECONDS + READY_TIMEOUT))
    while ((SECONDS < deadline)); do
        if grep -q '^stack is up\.' "$UP_LOG" 2>/dev/null; then ready=1; break; fi
        if ! kill -0 "$UP_PID" 2>/dev/null; then break; fi
        sleep 2
    done
    if [[ $ready -eq 0 ]]; then
        echo "client-ci: the stack did not come up." >&2
        echo "   --- up.log (tail) ---" >&2
        tail -n 30 "$UP_LOG" | sed 's/^/   /' >&2
        exit $EX_REFUSED
    fi
    echo "   stack is up"
    if (( WITH_BOTS )); then
        # Inside the jail's namespace: the control link and the listener are
        # loopback and the jail host there. A plain nsenter (no mount or pid
        # namespace): the bots only need the network and the repo's files.
        wb_holder=$(cat "$RL_STATE/holder.pid" 2>/dev/null || true)
        [[ $wb_holder =~ ^[0-9]+$ ]] && kill -0 "$wb_holder" 2>/dev/null \
            || { echo "client-ci: --with-bots: no rootless jail holder to enter" >&2; exit $EX_REFUSED; }
        say "bots (eve-bots --control, in the jail's namespace)"
        nsenter -t "$wb_holder" -U -n --preserve-credentials --keep-caps -- \
            env EVE_BOTS_KIT_DRILL="$BOTS_KIT" nice -n 10 "$BOTS_BIN" --connect 10.137.0.1:26000 --content "$CONTENT" \
            --control "$DEV_WORLD" --society-pvp \
            </dev/null >"$RUNDIR/bots.log" 2>&1 &
        BOTS_PID=$!
        wb_up=0
        for _ in $(seq 1 60); do
            grep -q -- '--control .*GM /bots commands are answered here' "$RUNDIR/bots.log" 2>/dev/null && { wb_up=1; break; }
            kill -0 "$BOTS_PID" 2>/dev/null || break
            sleep 1
        done
        if (( ! wb_up )); then
            echo "client-ci: --with-bots: eve-bots did not come up." >&2
            tail -n 15 "$RUNDIR/bots.log" | sed 's/^/   /' >&2
            exit $EX_REFUSED
        fi
        echo "   bots up (pid $BOTS_PID), log $RUNDIR/bots.log"
    fi
fi

# Non-negotiable, and checked whether we brought the jail up or adopted it: the
# client does not launch unless the namespace is proven sealed. run-client.sh
# checks this too; doing it here means a leaking jail is reported before we
# deploy anything, and `sudo -n` keeps it unattended.
say "jail"
# `timeout` because this is the one step with no deadline of its own, and the
# promise of this script is that it always terminates. jail-verify.sh bounds
# each of its own probes at 3s, so 120 is slack and not a real limit.
# Through the helper when it is there: its `verify` verb is a snapshot of the
# same probes, and on a machine with the helper but no cached timestamp the
# old line could not run at all -- the one check this script calls
# non-negotiable would have failed for want of a password rather than for
# want of a sealed jail.
if (( ROOTLESS )); then
    # The gate itself, on the client's own view, launching nothing. It runs
    # again, from scratch, as the precondition of each client launch.
    jail_check=(timeout 120 "$REPO/tools/jail-rootless.sh" check)
elif (( HAVE_HELPER )); then
    jail_check=(timeout 120 sudo -n "$JAIL_HELPER" verify)
else
    jail_check=(timeout 120 sudo -n "$REPO/tools/jail-verify.sh")
fi
if ! "${jail_check[@]}" >"$RUNDIR/jail-verify.log" 2>&1; then
    echo "client-ci: jail verification FAILED -- refusing to launch the client." >&2
    sed 's/^/   /' "$RUNDIR/jail-verify.log" >&2
    exit $EX_REFUSED
fi
echo "   sealed (full output in $RUNDIR/jail-verify.log)"
[[ -z $CENSUS_RUN ]] || census_begin

# ---------------------------------------------------------------- deploy

# The same two `cp`s as destiny-ref.sh, for the same reason: the scenario goes
# into the Proton prefix's log directory and never into the client
# installation, so manifest.dat stays honest and nothing needs restoring.
# `docs/lanes/ci-scenarios.md` §5: a scenario cannot know who else logged in,
# so the harness rewrites the one `EXPECT_CHARID = 0` assignment as it
# deploys. We edit the DEPLOYED COPY and never the source, which belongs to
# the `ci-scenarios` lane.
#
# The ids are derivable rather than guessed: `roster()` gives character N
# `Character::default().character_id + N` (`bin/eve-server.rs:226`), which is
# 90000001 + N. Override with EVE_CHARID_BASE if that default ever moves.
CHARID_BASE=${EVE_CHARID_BASE:-90000001}

# `expect_charid <deployed file> <charid>` -- a no-op for the scenarios that
# have no such knob, which is most of them.
expect_charid() {
    local file=$1 who=$2
    grep -qE '^ *EXPECT_CHARID *= *0 *$' "$file" || return 0
    sed -i -E "s/^( *)EXPECT_CHARID *= *0 *\$/\1EXPECT_CHARID = $who/" "$file"
    grep -qE "^ *EXPECT_CHARID = $who\$" "$file" || {
        echo "client-ci: could not set EXPECT_CHARID=$who in $file" >&2
        exit $EX_REFUSED
    }
    echo "   EXPECT_CHARID = $who in $(basename "$(dirname "$file")")/$(basename "$file")"
}

# FW_STATION = 0 -> a militia station from the bundle (EVE_CI_FWSTATION, set by
# tools/client-ci-regress.sh), so fci_fw_enlist can move its pilot there.
fw_station() {
    local file=$1
    [[ -n ${EVE_CI_FWSTATION-} && $EVE_CI_FWSTATION =~ ^[0-9]+$ ]] || return 0
    sed -i -E "s/^FW_STATION *= *0 *\$/FW_STATION = $EVE_CI_FWSTATION/" "$file"
}

say "deploying $NAME"
mkdir -p "$CI_DIR"
# A stale verdict from an earlier run is the one failure mode that could report
# a PASS for a scenario that never ran. Delete it, and prove it is gone.
rm -f "$RESULT_HOST"
[[ ! -e $RESULT_HOST ]] || { echo "client-ci: could not remove the old verdict $RESULT_HOST" >&2; exit $EX_REFUSED; }
cp "$SCENARIO" "$CI_DIR/$NAME.py"
DEPLOYED=1
echo "   $CI_DIR/$NAME.py"
fw_station "$CI_DIR/$NAME.py"
echo "   the client reads it as c:\\eve-logs\\client-ci\\$NAME.py"
if [[ $SECOND -eq 1 ]]; then
    # Each client is told to expect the OTHER one. That is the whole point:
    # a two-player assertion is "I can see them", from both sides.
    expect_charid "$CI_DIR/$NAME.py" "$((CHARID_BASE + SECOND_SLOT))"
    # The same scenario file, in the second prefix. It is the same scenario on
    # purpose: the server chooses it once, in the settings blob both clients
    # read at startup, so a two-player scenario tells itself apart by its own
    # `session.charid` rather than by being a different file.
    mkdir -p "$SECOND_CI_DIR"
    rm -f "$SECOND_RESULT"
    [[ ! -e $SECOND_RESULT ]] || { echo "client-ci: could not remove the old second verdict $SECOND_RESULT" >&2; exit $EX_REFUSED; }
    cp "$SCENARIO" "$SECOND_CI_DIR/$NAME.py"
    # The second client's own slot is the primary's, i.e. 0 unless
    # --no-auto-select changed what the first client picked.
    expect_charid "$SECOND_CI_DIR/$NAME.py" "$CHARID_BASE"
    DEPLOYED2=1
    echo "   $SECOND_CI_DIR/$NAME.py  (second client)"
fi

# ---------------------------------------------------------------- the client

# A marker to tell this run's client log from the hundreds already there. The
# client starts a new file per process, so anything newer than this is ours.
MARKER=$RUNDIR/.launch-marker
touch "$MARKER"

client_args=()
# Without this, charsel sits waiting for a click that nobody is there to make,
# and the GetSettingsInfo trigger (which fires after character selection)
# never comes -- so every unattended run would report "never ran".
#
# `_CheckAutoSelectCharacter` (characterSelection.pyc:208-222) does
# `int(arg.split(':')[1])`, tries `slotsByCharID` first and falls back to
# `slotsByIdx[n]`, then `EnterGameWithCharacter(slot)`. So 0 is the first
# character in the roster. It also returns False unless `isInitialLogin`,
# which a fresh launch is.
[[ $AUTO_SELECT -eq 1 ]] && client_args+=("/autoSelectCharacter:0")

if [[ $HEADLESS -eq 1 ]]; then
    say "display"
    # `start` is idempotent and builds the image on first use. The Vulkan
    # variables are not optional: Xvfb offers no DRI3, so RADV cannot present
    # -- and the client does **not** fall back by itself, it leaves processes
    # alive and never makes a window. `run-client.sh` forwards all of these
    # through its sudo (and the launch-env allow-list), which drop everything not named there.
    # Under the display lock: two slots starting at once would both see "not running".
    mkdir -p "$REPO/out/client-ci"
    exec {display_fd}>"$REPO/out/client-ci/display.lock"
    flock "$display_fd"
    "$REPO/tools/client-headless.sh" start >&2
    # Per-prefix, reversible, and the cure for a client that otherwise exits
    # in under six seconds with no log at all, right after DXVK fails to read
    # the monitor EDID. `docs/lanes/ci-display.md` has the diagnosis.
    EVE_PREFIX=$EVE_PREFIX "$REPO/tools/client-headless.sh" desktop on >&2
    DISPLAY_UP=1
    # **Per prefix, and that is the whole point of this line.** The desktop is
    # a registry key inside a Proton prefix, so turning it on for 8500 does
    # nothing for 8501 -- and a client with no virtual desktop exits a few
    # seconds after handoff with no error of its own, which is exactly how the
    # first two-client run failed: first=file, second=client_gone, and a
    # launch log ending "client exited" with nothing above it.
    if [[ $SECOND -eq 1 ]]; then
        EVE_PREFIX_ID=$SECOND_PREFIX_ID EVE_PREFIX=$SECOND_PREFIX "$REPO/tools/client-headless.sh" desktop on >&2
        SECOND_DESKTOP=1
    fi
    export DISPLAY=":${EVE_HEADLESS_DISPLAY:-99}"
    export WAYLAND_DISPLAY=
    export XAUTHORITY=
    export PROTON_USE_XALIA=0
    # **A private, empty runtime dir: no session D-Bus, no desktop sockets.**
    # The desktop's own XDG_RUNTIME_DIR holds the session bus, and through
    # it Wine's URL hand-off (winebrowser -> xdg-open) could ask a browser
    # OUTSIDE the jail's namespace to open a page: a phone-home path that
    # bypasses the network jail (docs/review/V4-testing.md and
    # V6-security-ops.md). An unattended run needs no audio and no desktop,
    # so it gets none. Human play is unchanged: that path keeps audio and
    # is the helper's to fix (review packet K/P4).
    CI_RUNTIME_DIR=$(mktemp -d -t eve-ci-runtime-XXXXXX)
    chmod 700 "$CI_RUNTIME_DIR"
    export XDG_RUNTIME_DIR="$CI_RUNTIME_DIR"
    unset DBUS_SESSION_BUS_ADDRESS
    icd=${EVE_LAVAPIPE_ICD:-/usr/share/vulkan/icd.d/lvp_icd.json}
    export VK_DRIVER_FILES="$icd" VK_ICD_FILENAMES="$icd"
    # **lavapipe rasterises in software and takes a thread per core.** Two
    # clients at once on a 32-thread box is then 64 render threads competing
    # with the owner's desktop, which is what this cap is for; `run-client.sh`
    # forwards it through its launch-env. Half the machine by default, one
    # knob shared with tools/dev.sh.
    export LP_NUM_THREADS="${EVE_MAX_JOBS:-16}"
    echo "   lavapipe threads capped at $LP_NUM_THREADS (EVE_MAX_JOBS)"
    echo "   headless display $DISPLAY, lavapipe, wine virtual desktop on"
    [[ $FRAME -eq 0 ]] || GPU=0
    if [[ $GPU -eq 1 ]]; then
        # **The GPU, on a display that has DRI3.** Xvfb stays up because
        # `desktop` runs its wine there; the client goes to gamescope's
        # Xwayland, which shows no window anywhere. Its X socket is the only
        # new one, and the jail binds whichever display it is given.
        before=$(ls /tmp/.X11-unix 2>/dev/null)
        env -u WAYLAND_DISPLAY -u VK_DRIVER_FILES -u VK_ICD_FILENAMES gamescope --backend headless -W 1280 -H 720 -w 1280 -h 720 -- sleep 86400 \
            >"$RUNDIR/gamescope.log" 2>&1 &
        GS_PID=$!
        gs_display=
        for _ in {1..30}; do
            gs_display=$(comm -13 <(printf '%s\n' "$before" | sort) <(ls /tmp/.X11-unix 2>/dev/null | sort) | head -1)
            [[ -z $gs_display ]] || break
            kill -0 "$GS_PID" 2>/dev/null || break
            sleep 0.5
        done
        [[ -n $gs_display ]] || refuse "gamescope gave no display (see $RUNDIR/gamescope.log, or pass --software)"
        export DISPLAY=":${gs_display#X}"
        icd=${EVE_RADV_ICD:-/usr/share/vulkan/icd.d/radeon_icd.json}
        export VK_DRIVER_FILES="$icd" VK_ICD_FILENAMES="$icd"
        unset LP_NUM_THREADS
        echo "   GPU display $DISPLAY (gamescope pid $GS_PID), RADV"
    fi
    # The new socket is ours now; the next slot may look for its own.
    flock -u "$display_fd"; exec {display_fd}>&-
fi

# **Rootless launch.** run-client.sh is the sudo path; here this script writes
# the same launch-env (its keys, run-client.sh:412-433) and asks
# jail-rootless.sh, whose launcher exits only when the client's pid
# namespace is empty. Not done here: run-client.sh's cryptoPack edit, and the
# first client's prefs.ini (chat TLS) edit; a prefix that has run the client
# once already has both. The SECOND client's profile (tq-offline-2) can be new
# after a re-pin and then lacks chatserver_disable_tls, so
# `ensure_second_chat_prefs` adds it (fci_skin_viewer died of that).
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"
rootless_client() {   # PREFIX CLIENT_DIR CLIENT_ARGS...
    local prefix=$1 client=$2; shift 2
    local -a a=("/ssoToken=${EVE_SSO_TOKEN:-offline}" '/fileLogDirectory=C:\eve-logs'
                /noCrashReportUpload "$@")
    mkdir -p "$(dirname -- "$LAUNCH_ENV")"
    {
        printf 'HOME=%s\n' "$HOME"
        printf 'DISPLAY=%s\n' "${DISPLAY:-}"
        printf 'WAYLAND_DISPLAY=%s\n' "${WAYLAND_DISPLAY:-}"
        printf 'XDG_RUNTIME_DIR=%s\n' "${XDG_RUNTIME_DIR:-}"
        printf 'XAUTHORITY=%s\n' "${XAUTHORITY:-}"
        printf 'PROTON_USE_XALIA=%s\n' "${PROTON_USE_XALIA:-}"
        printf 'VK_DRIVER_FILES=%s\n' "${VK_DRIVER_FILES:-}"
        printf 'VK_ICD_FILENAMES=%s\n' "${VK_ICD_FILENAMES:-}"
        printf 'LP_NUM_THREADS=%s\n' "${LP_NUM_THREADS:-}"
        printf 'PROTON_LOG=%s\n' "${PROTON_LOG:-}"
        printf 'PROTON_LOG_DIR=%s\n' "${PROTON_LOG_DIR:-}"
        printf 'WINEDEBUG=%s\n' "${WINEDEBUG:-}"
        printf 'STEAM_COMPAT_DATA_PATH=%s\n' "$prefix"
        printf 'STEAM_COMPAT_CLIENT_INSTALL_PATH=%s\n' "$STEAM_ROOT"
        printf 'EVE_CLIENT=%s\n' "$client"
        printf 'PROTON=%s\n' "$PROTON"
        printf 'EVE_CLIENT_ARGS=%s\n' "${a[*]}"
        printf 'EVE_CENSUS=%s\n' "${EVE_CENSUS:-}"
        printf 'EVE_CENSUS_CONTROL=%s\n' "${EVE_CENSUS_CONTROL:-}"
        printf 'EVE_CENSUS_NOENV=%s\n' "${EVE_CENSUS_NOENV:-}"
    } > "$LAUNCH_ENV"
    exec "$REPO/tools/jail-rootless.sh" client
}

# Mirror of run-client.sh's ensure_chat_prefs/set_pref for the second prefix,
# without sudo: the line is added when absent (CRLF, the client writes no final
# newline), and the run fails when no prefs.ini exists to carry it.
ensure_second_chat_prefs() {
    local rel folder base f found=0
    rel=${SECOND_CLIENT#"$SECOND_DRIVE_C/"}
    folder="c_$(printf '%s' "$rel" | tr '[:upper:]/ ' '[:lower:]__' | tr -d ':')"
    base="$SECOND_DRIVE_C/users"
    for f in "$base"/*/AppData/Local/CCP/EVE/"$folder"_*/settings*/prefs.ini; do
        [[ -f $f ]] || continue
        found=1
        if ! grep -qs '^chatserver_disable_tls=' "$f"; then
            if [[ -s $f && -n $(tail -c1 "$f" | tr -d '\n') ]]; then printf '\r\n' >>"$f"; fi
            printf 'chatserver_disable_tls=1\r\n' >>"$f"
            echo "   chat: chatserver_disable_tls=1 added to the second client's ${f#"$base"/}"
        fi
    done
    (( found )) || { echo "client-ci: no prefs.ini under $base for the second client ($folder)" >&2; exit "$EX_REFUSED"; }
}

say "client"
quiet_settings apply "$EVE_PREFIX/pfx/drive_c" "$(basename "$EVE_CLIENT")"
# The catch-all token; run-client.sh would otherwise read a login file.
export EVE_SSO_TOKEN=${EVE_SSO_TOKEN:-offline}
if (( ROOTLESS )); then
    ( rootless_client "$EVE_PREFIX" "$EVE_CLIENT" ${client_args[@]+"${client_args[@]}"} ) \
        </dev/null >"$LAUNCH_LOG" 2>&1 &
else
"$REPO/tools/run-client.sh" ${client_args[@]+"${client_args[@]}"} \
    </dev/null >"$LAUNCH_LOG" 2>&1 &
fi
LAUNCHER_PID=$!
echo "   launched (run-client.sh pid $LAUNCHER_PID, DISPLAY=${DISPLAY:-}${WAYLAND_DISPLAY:+ WAYLAND_DISPLAY=$WAYLAND_DISPLAY})"

if [[ $SECOND -eq 1 ]]; then
    # Second, and deliberately not in parallel: both clients want the same
    # display and the same jail, and a client that is still deciding whether
    # it has a window is the one thing in this stack that has been observed to
    # die quietly. Letting the first get its login in before the second starts
    # costs a few seconds and removes a whole class of flake.
    #
    # `--second` is run-client.sh's own flag for prefix 8501; the slot is the
    # roster index, so the two clients must be told DIFFERENT ones or they
    # both pick character 0 and the test proves nothing.
    # The stagger this comment used to only claim. Wait for the first
    # client's process to actually exist before starting the second: they
    # share a display and a jail, and the first two-client run launched them
    # one pid apart, which is not what "let the first get its login in" means.
    for _ in $(seq 1 30); do
        [[ -n $(client_pids "$CLIENT_PAT") ]] && break
        sleep 1
    done
    sleep "${EVE_SECOND_DELAY:-5}"

    second_args=()
    [[ $AUTO_SELECT -eq 1 ]] && second_args+=("/autoSelectCharacter:$SECOND_SLOT")
    LAUNCH_LOG2=$RUNDIR/client2-launch.log
    quiet_settings apply "$SECOND_DRIVE_C" "$SECOND_CLIENT_DIR"
    if (( ROOTLESS )); then
        ensure_second_chat_prefs
        ( rootless_client "$SECOND_PREFIX" "$SECOND_CLIENT" ${second_args[@]+"${second_args[@]}"} ) \
            </dev/null >"$LAUNCH_LOG2" 2>&1 &
    else
    "$REPO/tools/run-client.sh" --second ${second_args[@]+"${second_args[@]}"} \
        </dev/null >"$LAUNCH_LOG2" 2>&1 &
    fi
    LAUNCHER_PID2=$!
    echo "   second client launched (pid $LAUNCHER_PID2, prefix $SECOND_PREFIX_ID, slot $SECOND_SLOT)"
    echo "   waiting for BOTH verdicts"
fi
echo "   waiting up to ${TIMEOUT}s for c:\\eve-logs\\client-ci\\$NAME.json"

client_log() {
    find "$LOGDIR" -maxdepth 1 -name 'client-*.log' -newer "$MARKER" \
        -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -1 | cut -d' ' -f2-
}

# A client that died at boot: Wine gave it console code page 0, so
# `LogStarting` raises `LookupError: unknown encoding: cp0` inside the
# Startup() tasklet and Python stays up with no startup (1 run of 108, the
# d-reg4 report; the cause is [I], falsifier: `cp0` in a run with a console
# proven attached). Without this the run waits out the whole timeout (40
# minutes on fci_pvp_kill). It asserts nothing about the server: the run is
# NEVER RAN, to be repeated. $1 is the directory that client logs into.
boot_died() {
    local f
    f=$(find "$1" -maxdepth 1 -name 'client-*.log' -newer "$MARKER" \
        -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -1 | cut -d' ' -f2-)
    [[ -n $f ]] && grep -q 'unknown encoding: cp0' "$f" 2>/dev/null
}

# The wait. Two exits besides success, and they are different diagnoses:
# the deadline (something is wedged) and the launcher returning (the client is
# gone, so no verdict is ever coming).
# A verdict file that exists, is non-empty and parses. Half-written is the
# normal transient state, not an error.
verdict_valid() {
    [[ -s $1 ]] && python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$1" 2>/dev/null
}

VERDICT=none
VERDICT2=none
[[ $SECOND -eq 0 ]] && VERDICT2=skipped
wait_from=$SECONDS
deadline=$((SECONDS + TIMEOUT))
while ((SECONDS < deadline)); do
    # The file first, every time. A scenario writes its verdict and then calls
    # blue.os.Quit(), so the client being gone and the verdict existing is the
    # NORMAL ending, and checking the process first would race it into a
    # spurious "never ran".
    [[ $VERDICT == none ]] && verdict_valid "$RESULT_HOST" && VERDICT=file
    if [[ $SECOND -eq 1 && $VERDICT2 == none ]] && verdict_valid "$SECOND_RESULT"; then
        VERDICT2=file
    fi
    # Both settled (or the only one, single-client) is the normal ending.
    [[ $VERDICT != none && $VERDICT2 != none ]] && break

    # A launcher that has returned without a verdict is terminal for THAT
    # client only: with two clients, one dying must not stop us collecting the
    # other's verdict, because which one failed is the diagnosis.
    if [[ $VERDICT == none ]] && ! kill -0 "$LAUNCHER_PID" 2>/dev/null; then
        sleep 2                     # last chance for the write to land
        if verdict_valid "$RESULT_HOST"; then VERDICT=file; else VERDICT=client_gone; fi
    fi
    if [[ $SECOND -eq 1 && $VERDICT2 == none && -n $LAUNCHER_PID2 ]] \
        && ! kill -0 "$LAUNCHER_PID2" 2>/dev/null; then
        sleep 2
        if verdict_valid "$SECOND_RESULT"; then VERDICT2=file; else VERDICT2=client_gone; fi
    fi
    [[ $VERDICT != none && $VERDICT2 != none ]] && break

    # After ~30 s a client with no verdict may have died at boot (see
    # `boot_died`); check every 10th pass (about 20 s) so the grep stays cheap.
    if (( SECONDS - wait_from >= 30 )) && (( (SECONDS - wait_from) % 20 < 2 )); then
        if [[ $VERDICT == none ]] && boot_died "$LOGDIR"; then VERDICT=boot_died; fi
        if [[ $SECOND -eq 1 && $VERDICT2 == none ]] && boot_died "$SECOND_DRIVE_C/eve-logs"; then
            VERDICT2=boot_died
        fi
    fi
    [[ $VERDICT != none && $VERDICT2 != none ]] && break
    sleep 2
done
[[ $VERDICT == none ]] && VERDICT=timeout
[[ $VERDICT2 == none ]] && VERDICT2=timeout
if [[ $SECOND -eq 1 ]]; then
    echo "   first=$VERDICT second=$VERDICT2 after $((SECONDS - wait_from))s of waiting"
else
    echo "   $VERDICT after $((SECONDS - wait_from))s of waiting"
fi

CLIENT_LOG=$(client_log)
[[ -n $CLIENT_LOG ]] && cp "$CLIENT_LOG" "$RUNDIR/client.log" 2>/dev/null || true
if [[ $SECOND -eq 1 ]]; then
    CLIENT_LOG2=$(find "$SECOND_DRIVE_C/eve-logs" -maxdepth 1 -name 'client-*.log' -newer "$MARKER" \
        -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -1 | cut -d' ' -f2-)
    [[ -n $CLIENT_LOG2 ]] && cp "$CLIENT_LOG2" "$RUNDIR/client2.log" 2>/dev/null || true
fi
[[ -s $RESULT_HOST ]] && cp "$RESULT_HOST" "$RUNDIR/result.json" 2>/dev/null || true
(( ROOTLESS )) || tail -n 2000 "$SERVER_LOG" >"$RUNDIR/eve-server.log" 2>/dev/null || true

# An optional frame metric, because a greyscale scene is the one defect no
# in-client assertion can see (docs/archive/plan-client-ci.md section 3). A *scalar*,
# never an image diff: mean saturation reads 0.00 for a greyscale render and
# 0.74 for a flat red, so one number with a threshold catches the whole class
# where comparing frames of a 3D scene would only be flaky. [V] measured on
# synthetic images.
#
# Off by default for two reasons. On the owner's own :0 this photographs their
# desktop, which is both a privacy matter and a useless measurement. And on
# THIS session it cannot work at all: DISPLAY=:0 is XWayland, whose root window
# is not a real drawable, so X_GetImage returns BadMatch. It is expected to work
# on a plain X server -- which is one more reason for the ci-display lane to
# provide an actual Xvfb rather than a nested or XWayland display. [V] for the
# BadMatch, [I] for Xvfb working, from how xwd behaves on a normal X root.
if [[ $FRAME -eq 1 ]]; then
    say "frame"
    if [[ -z ${DISPLAY:-} ]]; then
        echo "   skipped: no DISPLAY (there is no X root window to grab)"
    elif ! command -v xwd >/dev/null || ! command -v magick >/dev/null; then
        echo "   skipped: xwd or magick missing, and nothing may be installed on this host"
    elif ! xwd -root -silent >"$RUNDIR/frame.xwd" 2>"$RUNDIR/frame.err"; then
        echo "   could not grab the root window of $DISPLAY:"
        sed 's/^/     /' "$RUNDIR/frame.err"
        echo "   BadMatch on X_GetImage means an XWayland root, which cannot be"
        echo "   read this way. This needs a real X server from the ci-display lane."
        rm -f "$RUNDIR/frame.xwd"
    elif ! magick "$RUNDIR/frame.xwd" "$RUNDIR/frame.png" 2>>"$RUNDIR/frame.err"; then
        echo "   grabbed $RUNDIR/frame.xwd but could not convert it; see $RUNDIR/frame.err"
    else
        rm -f "$RUNDIR/frame.xwd"
        sat=$(magick "$RUNDIR/frame.png" -colorspace HSL -channel G -separate \
                  -format '%[fx:mean]' info: 2>/dev/null || echo "?")
        echo "   $RUNDIR/frame.png  mean saturation $sat"
        echo "   (greyscale reads ~0, a coloured scene well above it; there is no"
        echo "    threshold wired in yet -- record a known-good number first)"
    fi
fi

# ---------------------------------------------------------------- diagnosis

# What a person debugging this would read anyway, so print it rather than
# pointing at a directory.
#
# The exception blocks are extracted here rather than with
# `tools/client-log.sh --errors`, which does the same awk but only ever on the
# newest log in a hardcoded prefix-8500 directory. A CI run knows exactly which
# file is its own (see client_log above) and must not depend on it still being
# the newest by the time the diagnosis prints.
# **Did it crash, or did it hang -- and are those the same thing?**
#
# For this client they very nearly are, which is why a "timeout" here is a
# misleading word. `blue.dll` carries the rule in plain text **[V]**, read out
# of build 3619879:
#
#     "time out value the process is assumed to be hanging and will be
#      treated as a crash."
#
# and `autoexec_client_core` sets `blue.os.frameTimeTimeout = 30000`. Stackless
# runs its watchdog in SOFT mode, so a tasklet is never interrupted mid
# bytecode: a scenario that busy-loops without yielding is not preempted, it
# holds the frame, and at 30 s the process is *killed as a crash*. From out
# here that looks exactly like "no verdict appeared", and we would call it a
# timeout and go looking for the trigger.
#
# `exefile.exe` links Crashpad in-process -- there is no separate handler
# binary in `bin64` -- and its report database is created on first use, so
# there is no path to register in advance. Hence `find -newer "$LAUNCH_LOG"`
# over the prefix rather than a known location: anything dump-shaped that
# appeared while we were watching is evidence, wherever it landed.
#
# The idea of treating a crash artefact as a hard failure rather than a
# footnote is EvEJS's: its bot harness fails a run outright when a new file
# shows up in `node-reports/` **[V2]**. Ours could not even tell.
crash_evidence() {
    local newer=$1 found=0 root
    for root in "$DRIVE_C" "${SECOND_DRIVE_C:-}"; do
        [[ -n $root && -d $root ]] || continue
        # `-newer` against the launch log: the launch log is written at launch,
        # so anything newer than it belongs to this run.
        while IFS= read -r hit; do
            [[ -n $hit ]] || continue
            if (( found == 0 )); then
                echo
                echo "  *** CRASH ARTEFACTS, newer than the launch ***"
                found=1
            fi
            printf '    %s\n' "$hit"
        done < <(find "$root" -xdev -newer "$newer" \
                     \( -iname '*.dmp' -o -ipath '*crashpad*' -o -iname '*.mdmp' \) \
                     -type f -printf '%TY-%Tm-%Td %TH:%TM  %s bytes  %p\n' \
                     2>/dev/null | head -20)
    done
    if (( found == 1 )); then
        echo
        echo "    A minidump means the process died rather than merely failing to"
        echo "    answer. If the scenario busy-loops without calling sleep_ms, the"
        echo "    30 s frameTimeTimeout kills the client AS a crash -- the bug is"
        echo "    then ours and in the scenario, not in the trigger."
        echo "    Nothing was uploaded: the client is jailed, and we pass"
        echo "    /noCrashReportUpload. exefile.exe has a Sentry minidump URL"
        echo "    compiled into it, so both of those matter."
    fi
    # The hang has its own signature even when no dump survives.
    if [[ -n $CLIENT_LOG && -s $CLIENT_LOG ]]; then
        local hang
        hang=$(/usr/bin/grep -niE 'is hanging|treated as a crash|frameTimeTimeout|watchdog' \
                   "$CLIENT_LOG" 2>/dev/null | tail -5)
        if [[ -n $hang ]]; then
            echo
            echo "  *** the client log mentions hanging or the watchdog ***"
            printf '%s\n' "$hang" | sed 's/^/    /'
        fi
    fi
}

diagnose() {
    say "diagnosis"
    echo "$1"
    crash_evidence "$LAUNCH_LOG"
    if [[ -s $SERVER_LOG ]]; then
        echo
        echo "--- $SERVER_LOG (tail) ---"
        tail -n 40 "$SERVER_LOG" | sed 's/^/  /'
    fi
    if [[ -n $CLIENT_LOG ]]; then
        echo
        echo "--- $CLIENT_LOG (exceptions) ---"
        awk '/EXCEPTION #|STACKTRACE #/{p=1} p{print} /EXCEPTION END|STACKTRACE END/{p=0}' \
            "$CLIENT_LOG" | tail -n 120 | sed 's/^/  /'
        echo
        echo "--- $CLIENT_LOG (tail) ---"
        tail -n 40 "$CLIENT_LOG" | sed 's/^/  /'
    else
        echo
        echo "  the client wrote no log newer than the launch. It probably never"
        echo "  started -- see $LAUNCH_LOG:"
        tail -n 20 "$LAUNCH_LOG" 2>/dev/null | sed 's/^/  /'
    fi
    echo
    echo "  artefacts kept in $RUNDIR"
}

case "$VERDICT" in
boot_died)
    diagnose "THE SCENARIO NEVER RAN: the client died at boot (LookupError: unknown
  encoding: cp0 in LogStarting, the Wine console code page read 0 [I]). Nothing
  was asserted about the server. Re-run; --fails covers it."
    exit $EX_NEVER_RAN
    ;;
client_gone)
    diagnose "THE SCENARIO NEVER RAN: the client exited without writing a verdict.
  Either it never reached the trigger (character selection, then
  GetSettingsInfo), or it died first. This is not the client disagreeing with
  us -- nothing was asserted."
    exit $EX_NEVER_RAN
    ;;
timeout)
    diagnose "THE SCENARIO NEVER RAN: no verdict after ${TIMEOUT}s, and the client is
  still up. Either the trigger never fired or the scenario is blocked waiting
  for something. Re-run with --keep --timeout N to keep the client alive and
  look at it."
    exit $EX_NEVER_RAN
    ;;
esac

# ---------------------------------------------------------------- the verdict

say "verdict"
set +e
# A function, so that two clients get the same reader rather than a second
# copy of it. argv is (verdict file, expected scenario name).
read_verdict() {
python3 - "$1" "$2" <<'PY'
import json, sys

path, want = sys.argv[1], sys.argv[2]
try:
    with open(path) as fh:
        d = json.load(fh)
except Exception as e:
    print("  the verdict file is not JSON: %s" % e)
    sys.exit(5)
if not isinstance(d, dict):
    print("  the verdict file is not an object: %r" % type(d).__name__)
    sys.exit(5)

got = d.get("scenario")
if got != want:
    # Not fatal on its own, but it means somebody's name is wrong and the
    # result may belong to a different run entirely.
    print("  WARNING: verdict says scenario=%r, we ran %r" % (got, want))

checks = d.get("checks")
if checks is None:
    checks = []
if not isinstance(checks, list):
    print("  'checks' is not a list: %r" % type(checks).__name__)
    sys.exit(5)

bad = []
for c in checks:
    if not isinstance(c, dict):
        print("  a check is not an object: %r" % (c,))
        sys.exit(5)
    ok = c.get("ok")
    name = c.get("name", "<unnamed>")
    detail = c.get("detail") or ""
    print("  %-4s %-44s %s" % ("ok" if ok else "FAIL", name, detail))
    if not ok:
        bad.append((name, detail))

err = d.get("error")
if err:
    print()
    print("  THE SCENARIO ITSELF RAISED -- this is a bug in the scenario or in")
    print("  what it assumed about the client, not a failed assertion:")
    for line in str(err).rstrip().splitlines():
        print("    " + line)
    sys.exit(6)

overall = d.get("ok")
if not checks:
    print("  no checks at all -- a scenario that asserts nothing cannot pass")
    sys.exit(5)
if bad:
    print()
    print("  THE CLIENT DISAGREED WITH US. %d of %d checks failed:" % (len(bad), len(checks)))
    for name, detail in bad:
        print("    %s: %s" % (name, detail))
    if overall:
        print("  (the scenario reported ok:true anyway, which is itself a bug in it)")
    sys.exit(1)
if not overall:
    print()
    print("  THE CLIENT DISAGREED WITH US: every check passed but the scenario")
    print("  reported ok:false, so it failed somewhere it did not record.")
    sys.exit(1)
print()
print("  all %d checks passed" % len(checks))
PY
}

read_verdict "$RESULT_HOST" "$NAME"
rc=$?
if [[ $SECOND -eq 1 ]]; then
    printf '\n  --- second client (prefix %s, character slot %s) ---\n' \
        "$SECOND_PREFIX_ID" "$SECOND_SLOT"
    if [[ $VERDICT2 != file ]]; then
        echo "  no verdict from the second client ($VERDICT2)"
        rc2=$EX_NEVER_RAN
    else
        read_verdict "$SECOND_RESULT" "$NAME"
        rc2=$?
        [[ -s $SECOND_RESULT ]] && cp "$SECOND_RESULT" "$RUNDIR/result2.json" 2>/dev/null || true
    fi
    # The worse of the two wins, so one green client cannot hide a red one.
    ((rc2 > rc)) && rc=$rc2
fi
set -e

# **The clock's own verdict, which no scenario can give.**
#
# A scenario asserts what the client saw. Nothing in it can notice that our
# world clock was half a second late delivering a batch -- the ship still
# arrives, the checks still pass, and the only trace is a stamp drifting away
# from the wall. EvEJS's bot harness asserts on tick lateness for exactly this
# reason and defaults its ceiling to 1000 ms **[V2]**; this is the same idea
# over our own `[world] tick health:` line.
#
# Two levels on purpose:
#
#   - **over budget > 0** (a tick more than LATE_BUDGET late) is reported
#     loudly but does NOT fail the run. This machine builds containers while
#     the client flies, so an occasional late tick says more about the host
#     than the server, and a check that cries wolf gets switched off.
#   - **max late past CLOCK_CEILING_MS** fails the run outright. At that point
#     the clock is not keeping time and a passing scenario is luck.
#
# **Baseline under load**, which is not the number this comment first carried.
# It said "late mean 0.1 ms, max 0.1 ms, over budget 0, measured on a real
# single-client run" -- and those were the idle figures, because the gauge was
# then sampling only the server's first ten seconds, before the client had
# connected. See the note on REPORT_EVERY in push.rs.
#
# Measured properly, three samples across one `capacitor` run:
#
#   world lock max 6.2 ms   (the bootstrap, built while the client binds)
#   late max 1.8 ms
#   over budget 0
#
# So the 100 ms budget has ~50x headroom on the worst real sample and the
# 1000 ms ceiling has ~500x. Loose on purpose, and now loose against a number
# that was actually observed with a client attached.
CLOCK_CEILING_MS=${CLOCK_CEILING_MS:-1000}
clock_health() {
    local log=$SERVER_LOG line worst
    [[ -s $log ]] || return 0
    local lines
    lines=$(/usr/bin/grep '\[world\] tick health:' "$log")
    if [[ -z $lines ]]; then
        # Not a failure: a run can be shorter than one report interval.
        echo "   clock: no health line (server up for under 10 s)"
        return 0
    fi
    # **Every sample, not the last one.** The server reports each ten seconds,
    # and the interesting window is while the client is connected -- which is
    # neither the first sample nor necessarily the last. Judging one line was
    # how this came to report an idle server six runs in a row.
    local count
    count=$(wc -l <<<"$lines")
    line=$(tail -1 <<<"$lines")
    echo "   clock: $count sample(s), last: ${line#*tick health: }"
    local over
    over=$(sed -n 's/.*over budget \([0-9]*\).*/\1/p' <<<"$lines" | sort -rn | head -1)
    worst=$(sed -n 's/.*max \([0-9.]*\) ms, over budget.*/\1/p' <<<"$lines" | sort -rn | head -1)
    if [[ -n $over && $over != 0 ]]; then
        echo "   clock: *** $over tick(s) over the lateness budget (worst sample) ***"
        echo "          not a failure by itself -- this host builds containers while"
        echo "          the client flies -- but a batch scheduled for t went out late."
    fi
    # Integer compare without bc: strip the decimal.
    if [[ -n $worst ]] && (( ${worst%%.*} > CLOCK_CEILING_MS )); then
        echo "   clock: *** worst tick ${worst} ms late across $count sample(s), \
ceiling ${CLOCK_CEILING_MS} ms ***" >&2
        return 1
    fi
    return 0
}

case $rc in
0)  if clock_health; then
        say "PASS  $NAME"
        echo "   result: $RUNDIR/result.json"
        exit 0
    fi
    say "FAIL  $NAME"
    diagnose "THE SCENARIO PASSED BUT THE CLOCK DID NOT KEEP TIME. Every check the
  client makes agreed with us, and the world clock was still past its ceiling --
  so a scheduled batch went out late and the green verdict is luck. This is the
  one failure a scenario cannot report on itself."
    exit $EX_FAIL ;;
5)  diagnose "THE VERDICT FILE IS MALFORMED. The scenario wrote something, so it ran,
  but we cannot read a verdict out of it. $RESULT_HOST"
    exit $EX_MALFORMED ;;
6)  diagnose "THE SCENARIO RAISED. Its own traceback is above; the client log below is
  the other half of the story."
    exit $EX_RAISED ;;
*)  diagnose "FAIL  $NAME -- the scenario ran and the client disagreed with us.
  The failing checks are listed above; this is the diagnosis worth having."
    exit $EX_FAIL ;;
esac
