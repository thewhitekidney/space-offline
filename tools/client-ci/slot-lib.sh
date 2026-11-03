# What one parallel client-CI slot owns (lane ci-parallel, issue #139). SOURCED,
# never run: tools/client-ci.sh, tools/client-ci-regress.sh, tools/client-ci/
# slot-prefix.sh and tools/test_client_ci_parallel.py all read it, so the list
# of per-slot resources is written once.
#
# A run that is alone (slot 0, today's behaviour) uses the paths the scripts have
# always used. Slots 1..N each get ALL of the following of their own, and two
# slots share none of them:
#
#   SLOT_ROOT          out/client-ci/slots/N: everything below that lives in out/
#   SLOT_RUNS          per-scenario run directories (up.log, captures, logs)
#   SLOT_WORLD         the --dev world (and the bots' control directory)
#   SLOT_RL_STATE      the rootless jail's holder.pid, socket inventory, resolv.conf
#   SLOT_LAUNCH_ENV    the launch-env the jailed client is started from
#   SLOT_PILOT_TOKENS  the --with-bots pilot's login file
#   SLOT_ACCOUNT_PORT  the host loopback port of the pilot-account one-shot
#   SLOT_PREFIX        the slot's Wine prefix, a copy (tools/client-ci/slot-prefix.sh)
#                      and with it the settings file, the deployed scenario, the
#                      verdict file and the client's logs, which live in the prefix
#   SLOT_CLIENT_DIR    the client's install directory name in it: DIFFERENT per slot,
#                      because client-ci.sh finds and stops its client by that name
#   SLOT_PREFIX2, SLOT_CLIENT_DIR2   the same for the --with-second client
#
# What is shared on purpose, read-only or serialised: the content bundle, the
# authored content, the server binaries, ResFiles (symlinked, bound read-only),
# Proton, the Xvfb that `desktop on` talks to, and a lock around gamescope's
# start (SLOT_DISPLAY_LOCK), which picks "the one new X socket".
#
# One runner at a time: tools/client-ci-regress.sh holds a non-blocking flock on
# out/client-ci/slots/runner.lock (ci_runner_lock REPO) for as long as it has slots, and
# slot-prefix.sh ensure/remove refuse while someone else holds it, so nothing deletes
# a slot's client install or prefix under a running client.
#
#   ci_slot_vars REPO HOME SLOT [SRC_CLIENT_DIR]    sets the SLOT_* variables; 1 if
#                                                   SLOT is not a number 1..999
#   ci_slot_dump REPO HOME SLOT [SRC_CLIENT_DIR]    the same as key=value lines

ci_slot_vars() {
    local repo=$1 home=$2 n=$3 src=${4:-tq-offline}
    [[ $n =~ ^[1-9][0-9]{0,2}$ ]] || return 1
    SLOT_N=$n
    SLOT_ROOT=$repo/out/client-ci/slots/$n
    SLOT_RUNS=$SLOT_ROOT/runs
    SLOT_WORLD=$SLOT_ROOT/dev-world
    SLOT_RL_STATE=$SLOT_ROOT/rootless
    SLOT_LAUNCH_ENV=$SLOT_ROOT/launch-env
    SLOT_PILOT_TOKENS=$home/.config/eve-offline/client-ci-pilot-s$n.tsv
    SLOT_ACCOUNT_PORT=$(( ${EVE_CI_ACCOUNT_PORT:-26093} + n ))
    SLOT_BASE=$home/.local/share/eve-offline/slots/$n
    SLOT_PREFIX=$SLOT_BASE/prefix
    SLOT_CLIENT_DIR=$src-s$n
    SLOT_PREFIX2=$SLOT_BASE/prefix2
    SLOT_CLIENT_DIR2=$src-s$n-2
    SLOT_DISPLAY_LOCK=$repo/out/client-ci/display.lock
}

ci_runner_lock() { printf '%s\n' "${EVE_CI_RUNNER_LOCK:-$1/out/client-ci/slots/runner.lock}"; }

ci_slot_dump() {
    ci_slot_vars "$@" || return 1
    local v
    for v in SLOT_ROOT SLOT_RUNS SLOT_WORLD SLOT_RL_STATE SLOT_LAUNCH_ENV SLOT_PILOT_TOKENS \
             SLOT_ACCOUNT_PORT SLOT_PREFIX SLOT_CLIENT_DIR SLOT_PREFIX2 SLOT_CLIENT_DIR2; do
        printf '%s=%s\n' "$v" "${!v}"
    done
}
