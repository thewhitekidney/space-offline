# Sourced, not run: stop an eve-server cleanly through its stop file.
#
#   source tools/stop-server.sh
#   stop_server PID STOPFILE [SECONDS]
#
# eve-server polls its stop file (--stop-file PATH, default <world>.stop)
# from the clock; when it appears the server stops accepting, hangs up every
# session, writes the world through the saver and exits 0
# (server/crates/eve-server/src/shutdown.rs). SIGTERM cannot do that: std
# has no signal handling and the workspace takes no libc crate, so a
# SIGTERM'd server loses up to SAVE_EVERY of play and exits 143.
#
# stop_server touches STOPFILE and waits up to SECONDS (default
# $EVE_STOP_TIMEOUT, else 30) for PID to exit. Returns 0 when it did, 1 when
# it had to fall back to SIGTERM. A PID already gone is 0. The caller reaps
# its own child with `wait PID` afterwards for the exit status.

stop_server() {
    local pid=$1 file=$2 secs=${3:-${EVE_STOP_TIMEOUT:-30}} i
    kill -0 "$pid" 2>/dev/null || return 0
    if ! : >"$file" 2>/dev/null; then
        echo "stop_server: cannot write $file; SIGTERM to $pid" >&2
        kill -TERM "$pid" 2>/dev/null || true
        return 1
    fi
    for ((i = 0; i < secs * 10; i++)); do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 0.1
    done
    echo "stop_server: $pid ignored $file for ${secs}s; SIGTERM" >&2
    kill -TERM "$pid" 2>/dev/null || true
    return 1
}
