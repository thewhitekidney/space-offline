# -*- coding: utf-8 -*-
"""PA-10: two pilots and rats on one grid, with Destiny coalescing (`--coalesce-destiny`).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Lane pa10-coalesce.

What it proves: that a client fed MERGED same-stamp `DoDestinyUpdate` frames
(`push.rs` outbox tail merge) stays in step -- no fatal or recoverable desync,
no `RequestReset`, no multi-stamp batch, a short history, `validState` true.
It is run twice as an A/B pair; the off run is the control, a desync there is
not PA-10's:

    sudo -v && tools/client-ci.sh run fci_pa10_coalesce --with-second -- --coalesce-destiny \
        --character Alice --character Bob
    sudo -v && tools/client-ci.sh run fci_pa10_coalesce --with-second -- \
        --character Alice --character Bob

(`--with-second` needs a roster two deep.  To make the merge row a hard check
on the on run, drop `coalesce=1` into `c:\\eve-logs\\client-ci\\fci.params`;
without it that row is informational.)  Both clients run this file.  The pilot
with the lower character id is A.

Instruments, installed right after login: the client's own desync counters
(`clientStatsSvc.entries`, keys read by name from `clientStatsCommon`), a
counting shim on `michelle.DoDestinyUpdate` (calls, a histogram of `len(state)`,
batches with more than one stamp) and a counter on `park.RequestReset`.  Every
client route named here that is not [V] is [A]; a route that does not exist
fails ITS OWN check with the route's name in the detail.

Steps, after both pilots warp to the nearest belt:
1. orbit 60 s (B orbits A at 5 km, A orbits B at 7.5 km): orbit mode held,
   distance inside wide bounds;
2. fire 60 s at rats: a rat's damage state changes at least 5 times;
3. warp 60 s+: B warps to the farthest belt and back while A watches the ball
   leave and return, then A does the same while B watches.

NOT here: the look of anything (rubber-banding, bars, tunnels) -- that is the
owner's ten minutes in a busy belt with the flag on (`docs/guides/human-checks.md`).
"""

EOCI_SCENARIO = 'fci_pa10_coalesce'

# Seconds to pass to `tools/client-ci.sh run fci_pa10_coalesce --timeout`: login,
# the peer's arrival, three 60 s phases and the longest warps, for two clients.
EOCI_FCI_TIMEOUT_S = 1800

# The tick-health line names the counters (both runs; the on run's `merged`
# is checked by the scenario's own row and by `grep 'merged [1-9]'`).
# SERVER-EXPECT: present destiny [0-9]+ frames [0-9]+ bytes parts [0-9]+ merged [0-9]+
# SERVER-EXPECT: absent  UNANSWERED (dogmaIM|ship|invbroker|entity|beyonce)
# SERVER-EXPECT: absent  panicked at
# SERVER-EXPECT: absent  would not encode

# PRELUDE -- byte-identical in every scenario in this directory, on purpose.
#
# The harness contract deploys exactly ONE file, `<name>.py`, into
# `c:\eve-logs\client-ci\`.  Nothing guarantees a shared library lands beside
# it, so a scenario that imported one could fail for a reason that has
# nothing to do with the thing it tests.  Every scenario is therefore
# self-contained and this block is copied verbatim.
# `tools/client-ci/scenarios/_prelude.py` is the canonical copy; if you change
# it, change it in all of them -- `docs/lanes/ci-scenarios.md` section 6.
#
# Why the whole scenario lives inside one function:
#   `execfile(path)` called from inside a *function* frame -- which is what
#   the planned trigger is (`crate::pycode` emits a code object whose body
#   execfiles the scenario) -- runs the file with `locals() is not globals()`.
#   Module-level `def`s then land in that frame's locals, and a sibling
#   function looking the other up through *globals* gets a NameError.  Putting
#   every helper and the body itself inside one function turns those into
#   closure lookups, which work under `/execfile`, under `exec` with a
#   separate locals mapping, and at true module level alike.
# ---------------------------------------------------------------------------

def eoci_main(EOCI_NAME):
    import os
    import sys
    import traceback

    OUTDIR = 'C:\\eve-logs\\client-ci'

    checks = []

    # -- output ------------------------------------------------------------

    def text(value):
        """`str()` that cannot itself raise and break the verdict file."""
        try:
            if isinstance(value, unicode):  # noqa: F821  (py2)
                return value.encode('ascii', 'replace')
            return str(value)
        except Exception:
            try:
                return repr(value)
            except Exception:
                return '<unprintable>'

    def jstr(value):
        """A JSON string literal.  Hand-rolled so nothing has to import."""
        src = value if isinstance(value, unicode) else text(value)  # noqa: F821
        out = ['"']
        for ch in src:
            o = ord(ch)
            if ch == '"':
                out.append('\\"')
            elif ch == '\\':
                out.append('\\\\')
            elif ch == '\n':
                out.append('\\n')
            elif ch == '\r':
                out.append('\\r')
            elif ch == '\t':
                out.append('\\t')
            elif o < 0x20 or o > 0x7E:
                out.append('\\u%04x' % o)
            else:
                out.append(ch)
        out.append('"')
        return ''.join(out)

    def check(name, ok, detail=''):
        """Record one assertion.  Never raises; a bad `detail` is survivable."""
        try:
            flag = bool(ok)
        except Exception:
            flag = False
        checks.append((text(name), flag, text(detail)))
        try:
            # live, so a scenario that never finishes still shows how far it got
            import log
            log.LogInfo('eve-offline client-ci [%s] live:' % EOCI_NAME,
                        '[%s] %s -- %s' % ('ok' if flag else 'FAIL', text(name), text(detail)[:200]))
        except Exception:
            pass
        return flag

    def say(message):
        """Best-effort line into the client's own log, for the failure case."""
        try:
            import log
            log.LogInfo('eve-offline client-ci [%s]:' % EOCI_NAME,
                        text(message))
        except Exception:
            pass

    def write_result(error):
        """Write the verdict.  Called from `finally`; must always succeed."""
        ok = error is None
        rows = []
        for (cname, cok, cdetail) in checks:
            ok = ok and cok
            rows.append('  {"name": %s, "ok": %s, "detail": %s}'
                        % (jstr(cname), 'true' if cok else 'false',
                           jstr(cdetail)))
        body = ('{"scenario": %s, "ok": %s,\n "checks": [\n%s\n ],\n'
                ' "error": %s}\n'
                % (jstr(EOCI_NAME),
                   'true' if ok else 'false',
                   (',\n').join(rows),
                   jstr(error) if error is not None else 'null'))
        try:
            os.makedirs(OUTDIR)
        except Exception:
            pass
        path = os.path.join(OUTDIR, '%s.json' % EOCI_NAME)
        try:
            f = open(path, 'wb')
            try:
                f.write(body)
                f.flush()
                try:
                    os.fsync(f.fileno())
                except Exception:
                    pass
            finally:
                f.close()
        except Exception:
            # The harness treats a missing file as its own diagnosis, so make
            # sure the client log carries the verdict as well.
            say('COULD NOT WRITE %s' % path)
        say('ok=%s checks=%d error=%s'
            % (ok, len(checks), 'yes' if error is not None else 'no'))
        for (cname, cok, cdetail) in checks:
            say('  [%s] %s -- %s' % ('ok' if cok else 'FAIL', cname, cdetail))
        if error is not None:
            say(error)
        return ok

    def quit_client(reason):
        """Exit the client, the way the client's own quit button does.

        **`blue.os.Quit()` -- which `docs/archive/plan-client-ci.md` section 3 names --
        does not exist in this build.**  The native call is
        `blue.os.Terminate(exitCode)` ("Terminates the process forcefully.",
        `|i:Terminate`), and the Python wrapper every caller uses is
        `bluepy.Terminate(exit_code, reason='')` (`bluepy/__init__.pyc:502`,
        reaching `blue.os.Terminate(exit_code)` at `:528`) [V].  That is what
        `carbonui/services/command.pyc:659` `_AppQuitGame` and
        `eve/client/script/ui/eveCommands.pyc:1254` `DoQuitGame` both call
        [V], and it fires `ProcessShutdown` and the atexit hooks on the way
        out, so a scenario exits the way a player quitting would.

        `log.Quit` (`logmodule/__init__.pyc:613`) reaches the same
        `bluepy.Terminate` at `:624` but dumps every tasklet's stack into the
        log first (`:616`), which is noise in the log the harness attaches to
        a failure -- so it is a fallback, not the first choice.
        """
        try:
            import bluepy
            bluepy.Terminate(0, text(reason))
            return 'bluepy.Terminate'
        except Exception:
            pass
        try:
            import blue
            blue.os.Terminate(0)
            return 'blue.os.Terminate'
        except Exception:
            pass
        try:
            import log
            log.Quit(text(reason))
            return 'log.Quit'
        except Exception:
            pass
        try:
            sys.exit(0)
        except SystemExit:
            raise
        except Exception:
            pass
        return 'none'

    # -- client access -----------------------------------------------------

    def builtin(name):
        """`session` and `sm` are installed as builtins by the client.

        `destiny_dump.py` reaches them this way for the same reason: the
        globals the trigger happens to run us under are not ours to assume.
        """
        try:
            return getattr(sys.modules['__builtin__'], name)
        except Exception:
            return None

    def sleep_ms(ms):
        """Yield to the client for `ms` wallclock milliseconds.

        Wallclock, not sim time: a scenario that has to survive a stalled
        simulation must not wait on the simulation's own clock.

        `blue.pyos.synchro.SleepWallclock(ms)` is the client's own name for it
        (`uthread/__init__.pyc:995`); `blue.synchro.SleepWallclock(ms)` is the
        same call reached through the shorter alias
        (`stationintrocinematic/client/cinematic_banner.pyc:123`); `SleepSim`
        is last because it is the wrong clock and is only better than not
        yielding at all (`michelle.pyc:508`).  All three take one argument,
        milliseconds [V].
        """
        import blue
        try:
            blue.pyos.synchro.SleepWallclock(int(ms))
            return True
        except Exception:
            pass
        try:
            blue.synchro.SleepWallclock(int(ms))
            return True
        except Exception:
            pass
        try:
            blue.pyos.synchro.SleepSim(int(ms))
            return True
        except Exception:
            return False

    def now_ms():
        """Wallclock milliseconds, or None if blue will not tell us.

        Blue keeps time in 100 ns units, so a difference divided by 10000 is
        milliseconds.  `blue.os.GetWallclockTimeNow` and
        `blue.os.GetWallclockTime` both exist in this build [V] (present in
        the decompiled tree); which one a given build prefers is [I], so both
        are tried.
        """
        import blue
        for getter in ('GetWallclockTimeNow', 'GetWallclockTime'):
            try:
                return getattr(blue.os, getter)() / 10000.0
            except Exception:
                continue
        return None

    def wait_for(predicate, timeout_ms=20000, step_ms=250):
        """Poll `predicate` until truthy.  Returns `(value, waited_ms)`.

        **`waited_ms` is measured, not counted.**  This used to return the sum
        of the `step_ms` values it had asked for, which is not the time that
        passed: `sleep_ms` yields to the client's own scheduler, and when the
        client is busy -- loading the destination scene during a warp, most
        obviously -- a requested 250 ms can take very much longer.  Every
        "after N ms" the suite printed was therefore a number the scenario had
        asked for rather than one it had observed.

        That is not cosmetic.  `throttle_and_warp` reported a ship travelling
        8,886,218 m with a peak of 3.9e9 m/s "in 3000 ms", and those three
        figures cannot all be true at once: our own warp model (a capsule at
        4.0 AU/s) puts a ship ~163 km along and at ~651 km/s three seconds in.
        The distance and the speed were real; the 3000 ms was not, so the
        check passed while carrying a number that could not be right.  A test
        that reports a quantity it did not measure can agree with a broken
        server as easily as a working one.

        The timeout is now honest too, which is a behaviour change: it is real
        elapsed time, so a client that stalls for longer than `timeout_ms`
        gives up instead of polling on past it.  The poll count comes back in
        the detail so a stall is visible rather than merely slow.

        A predicate that raises counts as "not yet", which is what every
        "that service is not up" case looks like from in here.
        """
        started = now_ms()
        counted = 0
        polls = 0
        while True:
            try:
                value = predicate()
            except Exception:
                value = None
            polls += 1
            if started is None:
                elapsed = counted
            else:
                here = now_ms()
                elapsed = counted if here is None else (here - started)
            if value:
                return value, int(elapsed)
            if elapsed >= timeout_ms:
                return None, int(elapsed)
            sleep_ms(step_ms)
            counted += step_ms

    def get_michelle():
        """`sm.GetService('michelle')` -- michelle.pyc:87 `GetMichelle` [V]."""
        sm = builtin('sm')
        if sm is None:
            return None
        return sm.GetService('michelle')

    def undock(timeout_ms=60000):
        """Leave the station, if we are in one.  `(ok, detail)`.

        **Nothing else undocks us.**  The harness logs in with
        `/autoSelectCharacter:0`, which lands the character in the station it
        was last in -- and every scenario that reads a ballpark needs to be in
        space, because there is no ballpark in a hangar.  The first real
        unattended run failed on exactly this: five checks green, and
        `session.solarsystemid = None, session.stationid = 60012526`.

        `sm.GetService('station')` is `StationSvc`, `__guid__ = 'svc.station'`
        (`eve/client/script/ui/station/base.pyc:43`) [V], and `UndockAttempt`
        (`:563`) is the client's own entry point -- the one a player's click
        reaches.  It is `self._DoUndockAttempt(False, True, shipID)` [V].

        **It does not prompt.**  With `observedSuppressed` false the body runs
        straight through `GetOnlineModules`, `space.PrioritizeLoadingForIDs`
        and then `sessionMgr.PerformSessionChange('undock', shipsvc.Undock,
        shipID)` (`:580-586`) [V].  The contraband question at `:597` is in
        the *exception* path, raised by the server, and ours answers `Undock`
        with `None`.  That matters here more than it would in a UI: a dialog
        nobody can click is an unattended run that hangs until its timeout.

        The return value of `Undock` is not the answer either -- the call site
        `POP_TOP`s it.  What takes the client out of the station is the
        session change that travels with it, so this waits for
        `session.solarsystemid` to appear rather than for the call to return.
        """
        sess = builtin('session')
        if sess is None:
            return False, 'no session to undock'
        if not getattr_safe(sess, 'stationid'):
            return True, 'already in space (session.stationid is unset)'
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm to reach svc.station through'
        ship_id = getattr_safe(sess, 'shipid')
        if not ship_id:
            return False, 'session.shipid is unset; nothing to undock'
        # **Retried, because this scenario runs earlier than any human ever
        # clicks.**  The trigger fires from `GetSettingsInfo`, and the first
        # real run raised `KeyError: 100000001` out of
        # `clientDogmaIM.GetDogmaLocation().GetOnlineModules(shipID)` --
        # `_DoUndockAttempt`'s first statement (`base.pyc:580`) -- because the
        # client logs `BaseDogmaLocation::_InstantiateDogmaItem 100000001`
        # about 3,300 log lines *after* the scenario starts.  Nothing is
        # wrong with the server: a player clicks seconds later, by which time
        # the dogma location holds their ship.
        #
        # Retrying rather than waiting on that one precondition is deliberate.
        # The precise thing that is not ready yet is an implementation detail
        # of a path we do not own, and a scenario that waits on the wrong one
        # hangs for its whole timeout and blames the wrong thing.
        last = None
        deadline_steps = max(1, int(timeout_ms / 500))
        for _attempt in range(deadline_steps):
            try:
                sm.GetService('station').UndockAttempt(ship_id)
                last = None
                break
            except Exception, e:
                # The type matters more than the message here: the first run
                # reported `raised: 100000001`, which is a `KeyError`'s key
                # and reads like nonsense on its own.
                last = '%s: %s' % (e.__class__.__name__, text(e))
                sleep_ms(500)
        if last is not None:
            return False, ('UndockAttempt(%s) still raising after %s ms -- %s'
                           % (text(ship_id), text(timeout_ms), last))
        # The session object is replaced on a session change, so re-read it
        # rather than closing over the one we started with.
        def in_space():
            s = builtin('session')
            return s is not None and getattr_safe(s, 'solarsystemid')
        ssid, waited = wait_for(in_space, timeout_ms=timeout_ms)
        if not ssid:
            return False, ('still docked %s ms after UndockAttempt(%s)'
                           % (text(waited), text(ship_id)))
        # The SERVER puts the ship in its roster when the ballpark binds, and
        # a GM command or a module activation sent before that answers 'not in
        # space' ('Modules can only be activated in space', '/whjump: you are
        # not in space') [V: run 2026-10-28].  So undock() returns when the
        # ego ball is in the park, not when the session says so.
        park, ego = get_ego_ball(timeout_ms=timeout_ms)
        if ego is None:
            return False, ('solarsystemid %s after %s ms but no ego ball in the park'
                           % (text(ssid), text(waited)))
        return True, 'undocked into solarsystemid %s after %s ms (ego ball in the park)' % (
            text(ssid), text(waited))

    def get_ballpark(timeout_ms=120000):
        """The live client-side ballpark, polled rather than waited on.

        `michelle.GetBallpark()` returns `self.__bp` only once
        `michelle.bpReady` is true (`michelle.pyc:484`) [V].  The blocking
        form, `GetBallpark(doWait=True)`, spins `WaitForBallpark` -- 30 x
        `blue.pyos.synchro.SleepSim(1000)` (`michelle.pyc:502-508`) [V] -- on
        the *simulation* clock, so it is polled on the wallclock here instead:
        if the simulation is not advancing, the client's own loop is the thing
        that hangs.
        """
        mich = get_michelle()
        if mich is None:
            return None
        park, _waited = wait_for(lambda: mich.GetBallpark(),
                                 timeout_ms=timeout_ms)
        return park

    def get_ego_ball(timeout_ms=120000):
        """`(park, ball)` once the ego ball is really in the park.

        **`bpReady` is not "there are balls".**  `Michelle.AddBallpark` sets
        `bpReady = False` at `michelle.pyc:444` and `True` at `:460`,
        immediately after `self.__bp.Start()` at `:459` and before any ball
        has been added -- and `Park.__init__` starts with `slimItems = {}`
        (`:1019`) [V].  So every scenario that reads the ego ball has to wait
        for the ball, not for the park.

        `ballpark.ego` is the ego ball's id and `ballpark.GetBall(id)` is
        `self.balls.get(id, None)` (`michelle.pyc:1888`) [V]; this is the
        same pair `Michelle.GetMyPosition` uses (`:591-598`) [V].
        """
        park = get_ballpark(timeout_ms=timeout_ms)
        if park is None:
            return None, None

        def probe():
            ego = park.ego
            if not ego:
                return None
            return park.GetBall(ego)

        ball, _waited = wait_for(probe, timeout_ms=timeout_ms)
        return park, ball

    def fmt(value, places=1):
        """A number for a human to read inside a `detail` string."""
        try:
            return ('%%.%df' % places) % float(value)
        except Exception:
            return text(value)

    def getattr_safe(obj, name, default=None):
        try:
            return getattr(obj, name, default)
        except Exception:
            return default

    # -- the scenario ------------------------------------------------------

    # -- fci block: shared helpers of the f-ci scenarios --------------------
    # THIS BLOCK IS BYTE-IDENTICAL IN EVERY f-ci SCENARIO.  The canonical copy
    # is `tools/client-ci/fci-block.txt`; edit that and run
    # `python3 -I tools/client-ci/sync-fci.py` (`--check` for the gate).  It is
    # copied, not imported, for the reason the prelude is: the harness deploys
    # exactly one file per scenario.  What it adds to the prelude: GM slash
    # commands from inside the client, calls on server services, the client's
    # own log read for tracebacks, a window census, and system picking from
    # the client's own map data.  Every client API named here is [I] or [A]
    # unless a line says [V]; a route that does not exist fails ITS OWN check
    # with the route's name in the detail, never the whole scenario.

    LOG_DIR = 'C:\\eve-logs'
    LOG_CAP = 6 * 1024 * 1024
    PARAMS_FILE = 'C:\\eve-logs\\client-ci\\fci.params'

    def safe(fn, *args, **kw):
        """`(ok, value)`.  Never raises; a failure's value is `Type: message`."""
        try:
            return True, fn(*args, **kw)
        except Exception as e:
            return False, '%s: %s' % (e.__class__.__name__, text(e))

    def get_svc(name):
        """A client service by name, preferring the running instance."""
        sm = builtin('sm')
        if sm is None:
            return None
        try:
            s = sm.GetServiceIfRunning(name)
            if s is not None:
                return s
        except Exception:
            pass
        try:
            return sm.GetService(name)
        except Exception:
            return None

    def rcall(service, method, *args):
        """`(ok, value)`: one call on a SERVER service, `sm.RemoteSvc(name)`.

        The same route the client's own services use (every row of
        `findings/client-calls.tsv` with kind `remotesvc`).  The reply is
        decoded by the client's own marshal, so a reply shape the client
        cannot read is a failure here exactly as it would be on screen.
        """
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm'
        return safe(lambda: getattr(sm.RemoteSvc(service), method)(*args))

    def rcall_kw(service, method, args, kwargs):
        """`rcall` with keyword arguments (`GetCashBalance(1, accountKey=...)`)."""
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm'
        return safe(lambda: getattr(sm.RemoteSvc(service), method)(*args, **kwargs))

    def uniq(n=4):
        """A short base-36 suffix, so a re-run in a persisted world does not
        collide with a name or ticker the last run took."""
        import random
        import time
        digits = '0123456789abcdefghijklmnopqrstuvwxyz'
        rng = random.Random(int(time.time() * 1000) % 2147483647)
        return ''.join([digits[rng.randint(0, 35)] for _i in range(n)])

    def gm(line):
        """`(ok, reply_text)`: a GM slash command, sent to the SERVER.

        `sm.RemoteSvc('slash').SlashCmd(line)` [V] (`docs/reference/session-roles.md`
        :445); the client's own `/createitem` and friends reach the server the
        same way, and the reply is the string the console would print.  The
        session needs the GM role, which the sandbox grants.
        """
        ok, value = rcall('slash', 'SlashCmd', line)
        # `/bots` and `/world` answer as a raised CustomInfo on purpose
        # [V service/gm.rs visible_reply]: the client shows the reply as an
        # info box.  That is a success, not a failure.  A real refusal is
        # still caught by the `forbid` lists ('no eve-bots', 'dev server').
        if not ok and 'msg=CustomInfo' in text(value) \
                and line.split(' ')[0].lower() in ('/bots', '/world'):
            ok = True
        return ok, text(value)

    def gm_check(name, line, expect=None, forbid=None):
        """`gm(line)` as a check.  `expect` and `forbid` are lists of
        lower-case substrings of the reply.  Returns `(ok, reply)`."""
        ok, reply = gm(line)
        low = reply.lower()
        good = ok
        why = ''
        if good and expect:
            for e in expect:
                if e.lower() not in low:
                    good = False
                    why = ' [missing %r]' % e
                    break
        if good and forbid:
            for f in forbid:
                if f.lower() in low:
                    good = False
                    why = ' [found %r]' % f
                    break
        check(name, good, '%s -> %s%s' % (line, reply[:300], why))
        return good, reply

    def precondition(name, ok, detail=''):
        """A check that says the scenario could not even set up.  Same as
        `check`, named so a verdict reads 'could not set up' and not
        'the server is wrong'."""
        return check('PRECONDITION: ' + name, ok, detail)

    def shape(value, depth=0):
        """A short description of a reply: type, length, first keys."""
        try:
            if value is None:
                return 'None'
            kind = value.__class__.__name__
            if isinstance(value, (list, tuple)):
                return '%s[%d]' % (kind, len(value))
            if isinstance(value, dict):
                return 'dict{%d: %s}' % (len(value), text(sorted(value.keys())[:4]))
            for attr in ('header', 'lines'):
                if hasattr(value, attr):
                    try:
                        return '%s(%s=%s)' % (kind, attr, text(getattr(value, attr))[:100])
                    except Exception:
                        pass
            return '%s %s' % (kind, text(value)[:100])
        except Exception:
            return '<unprintable>'

    def rows_of(value):
        """The rows of a reply that may be a list, a dict, a Rowset (`.lines`)
        or a CRowset.  Returns a list (possibly empty), or None if unknown."""
        if value is None:
            return None
        if isinstance(value, (list, tuple)):
            return list(value)
        if isinstance(value, dict):
            return list(value.values())
        if getattr_safe(value, 'header') is not None:
            # a Rowset iterates as Row objects (rowset.py), which is how the
            # journal and the job board read it; `.lines` are bare lists
            try:
                return list(value)
            except Exception:
                pass
        lines = getattr_safe(value, 'lines')
        if lines is not None:
            return list(lines)
        try:
            return list(value)
        except Exception:
            return None

    # -- the client's own log: tracebacks and lines, since a mark ----------

    def client_log_path():
        try:
            import glob
            best = None
            best_t = -1
            for f in glob.glob(os.path.join(LOG_DIR, 'client-*.log')):
                try:
                    t = os.path.getmtime(f)
                except Exception:
                    continue
                if t > best_t:
                    best = f
                    best_t = t
            return best
        except Exception:
            return None

    def log_mark():
        """Where the newest client log ends now."""
        p = client_log_path()
        if p is None:
            return (None, 0)
        try:
            return (p, os.path.getsize(p))
        except Exception:
            return (p, 0)

    def log_since(mark):
        """`(text or None, detail)`: everything the client logged since `mark`.
        Waits a moment first: the client's log is buffered."""
        sleep_ms(1500)
        p, off = mark
        now = client_log_path()
        if p is None or now is None:
            return None, 'no client-*.log under %s' % LOG_DIR
        spans = [(p, off)] if p == now else [(p, off), (now, 0)]
        chunks = []
        for path, start in spans:
            try:
                f = open(path, 'rb')
                try:
                    f.seek(start)
                    chunks.append(f.read(LOG_CAP))
                finally:
                    f.close()
            except Exception as e:
                return None, 'cannot read %s: %s' % (path, text(e))
        blob = ''.join(chunks)
        return blob, '%d bytes of %s' % (len(blob), os.path.basename(now))

    def log_exceptions(blob, ignore=()):
        """`(count, first_three)`: the client brackets a Python traceback with
        `EXCEPTION #n` or `STACKTRACE #n` (`tools/client-log.sh --errors`)."""
        # the client's own watchdog warning while a scenario runs inside a
        # service call (xmpphandler/settingsSvc): not a fault of ours
        ignore = tuple(ignore) + (
            'Possible service deadlock detected',
            # the jail gives the client's CCP-side gateway nothing to answer
            # (docs/guides/jail.md): its own timeouts, not ours
            'GetEquippedForCharacterRequest', 'get goal definitions',
            'fetch definitions', 'SkillPlansErrorTimeout',
            'BluepyTasklet::_build_profile', 'SendClientStackTraceAlert',
            'Formatted exception info: | TimeoutException',
            # board_ship retries TryActivateShip until the client's ~9 s
            # session-change lockout has passed: each early call throws this
            # inside the client's MakeShipActive tasklet [V: client.log,
            # sessions.py:394 -- a refusal, nothing was changed]
            'PerformSessionChangeInProgress',
            # the jail has no route to a public-IP service (docs/guides/jail.md)
            'api.ipify.org',
            # more of the CCP-side gateway's timeouts (same cause, same jail)
            'TimeoutException', 'GoalMessengerError', 'unable to cache',
            'Exception in Error Logging', 'achievement_system', 'dailygoals',
            # a CCP UI localisation property with no handler, at login
            # (client.log _GetByMessageID 'min_value'): nothing we send
            "Failed to find the value for 'min_value'",
            # the station view reloading the PREVIOUS ship's model (the pod)
            # right after the board's session change: hangarView.py:505 ->
            # baseHangarBehaviours.py:63/148 -> clientDogmaLocation.py:442.
            # Seen on every board; the boarded ship works.  [A] a client race
            # -- falsifier: the pod missing from the hangar listing.
            'Dogma was unable to get invItem to load dogmaItem',
            # a loaded charge's sublocation prime builds blue.DBRow(sublocrd,
            # nine values) against our ELEVEN-column item descriptor
            # (service.rs item_columns: `singleton`/`stacksize` are physical
            # here, virtual in CCP's; docs/reference/dbrow.md Gap 4).  A warning, the
            # row is still made -- 801 of them in one fci_doc_3 run.  Known
            # server deviation, not a new fault: the run is not failed by it.
            'sequence is too short',
            # Build 3662168 gave the Neocom's Aura Guidance button this label
            # and shipped no such label: the client's own fault, once per login.
            "No label with name 'AuraGuidanceWnd'",
            # The gateway answers a content checkpoint 404 on purpose: there is
            # no CDN, and the client keeps its empty default data (gateway.rs).
            'fetchRequest for flatbuffer data raises Not Found(404)',
            # A session change (a /tr, a jump) swaps the ballpark under a
            # service that still holds the old one, which then asks the new
            # park for a ball that is not in it yet.  [A] a race at the swap --
            # falsifier: the ball still missing from the new park after the
            # scenario's wait for the ego ball.
            "'NoneType' object has no attribute 'SendMessageToItem'",
            # The camera reset on arrival, run before the new scene has a
            # camera.  [A] a fast client does it on a jump -- falsifier: a
            # camera stuck on the old system after the arrival.
            'Failed to reset camera',
            # The client's own Disconnect kills its receive tasklet while it
            # is in recv (xmppConnection.py 242, 445): 10058, then a
            # SystemError from that tasklet.  Chat logs in again.  [A] a race
            # in blue's socket -- falsifier: chat not connected after the
            # next login.
            'recv tasklet block failed',
            'xmppConnection.py(445) _receive_t')
        lines = blob.split('\n')
        heads = []
        n = 0
        for i in range(len(lines)):
            ln = lines[i]
            if 'EXCEPTION #' in ln or 'STACKTRACE #' in ln:
                # the whole record, up to its END line (the frames that name
                # the culprit can sit a dozen lines down)
                end = i + 1
                while end < len(lines) and end < i + 60 and \
                        'EXCEPTION END' not in lines[end] and 'STACKTRACE END' not in lines[end]:
                    end += 1
                block = ' | '.join([x.strip() for x in lines[i:end]])
                skip = False
                for ig in ignore:
                    if ig in block:
                        skip = True
                if skip:
                    continue
                n += 1
                if len(heads) < 3:
                    heads.append(block[:320])
        return n, heads

    def no_new_tracebacks(label, mark, ignore=()):
        """The check a human makes with `tools/client-log.sh --errors`.  It
        fails on an unreadable log too: 'could not look' is not 'all clear'."""
        blob, detail = log_since(mark)
        if blob is None:
            return check('client log has no new traceback: ' + label, False,
                         'COULD NOT READ THE LOG (%s)' % detail)
        n, heads = log_exceptions(blob, ignore)
        return check('client log has no new traceback: ' + label, n == 0,
                     '%d traceback(s) in %s%s'
                     % (n, detail, (' -- first: ' + ' ## '.join(heads)) if heads else ''))

    def log_lines_with(blob, needle, limit=3):
        out = []
        for ln in blob.split('\n'):
            if needle in ln:
                out.append(ln.strip()[:200])
                if len(out) >= limit:
                    break
        return out

    # -- windows ------------------------------------------------------------

    def window_names():
        """Class names (and window ids) of the open windows.  [A]: three routes,
        the first that answers wins."""
        names = []
        try:
            from carbonui.uicore import uicore
        except Exception:
            return names
        reg = getattr_safe(uicore, 'registry')
        wins = None
        for route in (lambda: reg.GetWindows(), lambda: reg.windows,
                      lambda: uicore.layer.main.children):
            ok, wins = safe(route)
            if ok and wins:
                break
            wins = None
        try:
            if isinstance(wins, dict):
                wins = list(wins.values())
            for w in (wins or []):
                names.append('%s:%s' % (w.__class__.__name__,
                                        text(getattr_safe(w, 'windowID'))))
        except Exception:
            pass
        return names

    def window_like(sub):
        sub = sub.lower()
        return [n for n in window_names() if sub in n.lower()]

    # -- the client's own map data, and the optional params file -----------

    def params():
        """`key=value` lines from PARAMS_FILE (the host may drop one beside
        the scenario to pin an id).  Empty when absent."""
        out = {}
        try:
            f = open(PARAMS_FILE, 'rb')
            try:
                for ln in f.read().split('\n'):
                    if '=' in ln and not ln.strip().startswith('#'):
                        k, v = ln.split('=', 1)
                        out[k.strip()] = v.strip()
            finally:
                f.close()
        except Exception:
            pass
        return out

    def map_systems(pred, limit=60):
        """Solar system ids from the CLIENT's static map data whose row passes
        `pred(row)`.  [A] `cfg.mapSystemCache` (id -> row with
        `securityStatus` and `factionID`); no id is written in this file."""
        cfg = builtin('cfg')
        out = []
        for route in (lambda: cfg.mapSystemCache,):
            ok, table = safe(route)
            if not ok or table is None:
                continue
            try:
                items = table.items() if hasattr(table, 'items') else list(table)
                for key, row in items:
                    ok2, good = safe(pred, row)
                    if ok2 and good:
                        out.append(int(key))
                        if len(out) >= limit:
                            return out
            except Exception:
                continue
        return out

    def type_ids_named(sub, limit=10, exact=False):
        """Type ids whose English name contains `sub` (equals it when `exact`),
        from the client's own `evetypes` (`cfg.invtypes` does not exist in this
        build: docs/reference/item-spawn.md 4.1 [V]).  Names come from the client.  The
        id -> lower-case name table is read once (about 50 k localisation
        lookups) and kept; `type_ids_named.note` says how that went."""
        out = []
        sub = sub.lower()
        import time
        table = getattr(type_ids_named, 'table', None)
        if table is None:
            table = []
            type_ids_named.note = 'no types'
            t0 = time.time()
            # the table is kept in a file beside the params, so only the first
            # scenario of a prefix pays for ~50 k localisation lookups
            cache = os.path.join(os.path.dirname(PARAMS_FILE), 'typenames.tsv')
            try:
                f = open(cache, 'rb')
                try:
                    for ln in f.read().split('\n'):
                        if '\t' in ln:
                            a, b = ln.split('\t', 1)
                            table.append((int(a), b))
                finally:
                    f.close()
                type_ids_named.note = '%d names from %s' % (len(table), cache)
            except Exception:
                table = []
            if not table:
                try:
                    evetypes = __import__('evetypes')
                    for tid in sorted(evetypes.GetAllTypeIDs()):
                        try:
                            table.append((int(tid), text(evetypes.GetEnglishName(tid)).lower()))
                        except Exception as e:
                            type_ids_named.note = 'GetEnglishName(%s): %s' % (tid, text(e)[:100])
                    type_ids_named.note = '%d names read in %.1f s' % (len(table), time.time() - t0)
                    try:
                        f = open(cache, 'wb')
                        try:
                            f.write('\n'.join(['%d\t%s' % (a, b.encode('utf-8') if isinstance(b, unicode) else b)
                                               for a, b in table]))
                        finally:
                            f.close()
                    except Exception:
                        pass
                except Exception as e:
                    type_ids_named.note = 'evetypes: ' + text(e)[:160]
            type_ids_named.table = table
        for tid, nm in table:
            if (nm == sub) if exact else (sub in nm):
                out.append(tid)
                if len(out) >= limit:
                    break
        return out

    def ship_items(flag=None):
        """`(ok, rows)`: the active ship's inventory (client `invCache`), the
        route `eve/devtools/script/svc_slash.py` itself uses.  [A]."""
        sess = builtin('session')
        inv = get_svc('invCache')
        if inv is None or sess is None or not getattr_safe(sess, 'shipid'):
            return False, 'no invCache or no ship'
        if flag is None:
            return safe(lambda: inv.GetInventoryFromId(sess.shipid).List())
        return safe(lambda: inv.GetInventoryFromId(sess.shipid).List(flag))

    def hangar_items():
        """`(ok, rows)`: the item hangar of the station we are docked in. [A]"""
        inv = get_svc('invCache')
        if inv is None:
            return False, 'no invCache'
        const = builtin('const')
        flag = getattr_safe(const, 'containerHangar', 10004)
        return safe(lambda: inv.GetInventory(flag).List())

    def count_type(rows, type_id):
        n = 0
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                n += 1
        return n

    # -- items, ships, modules (all [A] routes; each says which one answered) --

    FLAG_HI0 = 27     # flagHiSlot0; med 19, low 11, rig 92 (slot flags)
    FLAG_DRONE_BAY = 87
    FLAG_MINING_HOLD = 134

    def dogma_loc():
        """`clientDogmaIM.GetDogmaLocation()` [V] (the prelude's `undock`
        cites it)."""
        d = get_svc('clientDogmaIM')
        if d is None:
            return None
        ok, loc = safe(lambda: d.GetDogmaLocation())
        return loc if ok else None

    def find_hangar_item(type_id, timeout_ms=15000):
        # A GM-created item reaches the client's hangar list a moment after
        # the slash answer (OnItemChange + OnInvalidateCacheForLocations are
        # handled on the client's own tasklets): poll, do not read once
        # [V: run 2026-10-28, fci_fighters -- rows grew 2,2,3,3,4,5 as the
        # scenario went through its types].
        def one():
            ok, rows = hangar_items()
            if not ok:
                return None
            for r in (rows or []):
                if getattr_safe(r, 'typeID') == type_id:
                    return r
            return None
        found, _w = wait_for(one, timeout_ms=timeout_ms, step_ms=500)
        ok, rows = hangar_items()
        if not ok:
            return None, 'hangar unreadable: %s' % text(rows)
        if found is not None:
            return found, '%d row(s) in the hangar' % len(rows or [])
        return None, 'type %s is not among the %d hangar row(s) after %d ms' % (
            text(type_id), len(rows or []), timeout_ms)

    def missing_skills_for(type_id):
        """`[(skillTypeID, level)]` the client says `type_id` still needs
        (`dogmaLocation.GetMissingSkills`, clientDogmaLocation.py:432 [V])."""
        d = dogma_loc()
        ok2, m = safe(lambda: d.GetMissingSkills(type_id))
        if not ok2 or not m:
            return []
        try:
            return list(m.items())
        except Exception:
            return [(k, 1) for k in m]

    def grant_skills_for(type_id):
        """GM-grant what `missing_skills_for` lists, then wait for the client
        to agree.  Fitting a module without them is allowed; ONLINING it is
        not (`OnlineHasSkillPrerequisites`) [V: run 2026-10-28].  Returns the
        list still missing."""
        for sk, lvl in missing_skills_for(type_id):
            ok3, nm = safe(lambda: __import__('evetypes').GetEnglishName(sk))
            if ok3 and nm:
                gm('/giveskill %s %s' % (text(nm), text(lvl)))
        wait_for(lambda: not missing_skills_for(type_id), timeout_ms=10000)
        return missing_skills_for(type_id)

    def fit_module(type_id, flag):
        """Fit a hangar module into the active ship at slot `flag` by the
        client's own inventory `Add` (the drag-and-drop route).  `(ok, detail)`"""
        item, why = find_hangar_item(type_id)
        if item is None:
            return False, why
        sess = builtin('session')
        inv = get_svc('invCache')
        if inv is None or sess is None or not getattr_safe(sess, 'shipid'):
            return False, 'no invCache or no active ship'
        grant_skills_for(type_id)
        # the new ship's inventory moniker is not always bound the moment the
        # board returns (invCacheContainer.Add: 'NoneType' has no attribute
        # 'Add' on the first fit only) [V: run 2026-10-28]: retry a few times
        ok, v = False, None
        for _attempt in range(4):
            try:
                inv.GetInventoryFromId(sess.shipid).Add(
                    item.itemID, item.locationID, qty=1, flag=flag)
                ok, v = True, None
                break
            except Exception:
                ok, v = False, ' | '.join(traceback.format_exc().strip().split('\n')[-7:])
                sleep_ms(3000)
        if not ok:
            return False, 'inventory.Add raised %s' % text(v)[:700]
        fitted, waited = wait_for(
            lambda: count_type(ship_items(flag)[1] if ship_items(flag)[0] else [],
                               type_id) > 0, timeout_ms=10000)
        return bool(fitted), ('fitted item %s at flag %s (%s ms)' % (
            text(item.itemID), text(flag), text(waited))
            if fitted else 'Add returned but the ship shows nothing at flag %s'
            % text(flag))

    def module_item(type_id, flag):
        ok, rows = ship_items(flag)
        if not ok:
            return None
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                return r
        return None

    def online_module(item_id):
        """`(ok, detail)`: `clientDogmaLocation.OnlineModule` [V], then wait
        for the godma item to read online."""
        loc = dogma_loc()
        if loc is None:
            return False, 'no dogma location'
        # a freshly fitted module reaches the client's dogma location a moment
        # after the inventory shows it (KeyError otherwise) [V: run 2026-10-28]
        wait_for(lambda: item_id in getattr_safe(loc, 'dogmaItems', {}), timeout_ms=10000)
        ok, v = safe(lambda: loc.OnlineModule(item_id))
        if not ok and 'EffectAlreadyActive2' not in text(v):
            return False, 'OnlineModule raised %s' % text(v)
        # EffectAlreadyActive2: the online effect already runs in the client
        # (the server onlines a module it fits); fall through and check godma

        def on():
            g = get_svc('godma')
            it = g.GetItem(item_id) if g is not None else None
            return bool(getattr_safe(it, 'isOnline'))
        up, waited = wait_for(on, timeout_ms=10000)
        return bool(up), 'godma isOnline after %s ms: %s' % (text(waited), text(bool(up)))

    def module_effect_name(item_id):
        g = get_svc('godma')
        ok, it = safe(lambda: g.GetItem(item_id))
        if not ok or it is None:
            return None
        ok, name = safe(lambda: it.defaultEffect.effectName)
        return name if ok else None

    def activate_module(item_id, target_id=None):
        """`(ok, detail)`.  The module button's route: `godma.GetStateManager().
        Activate(itemID, effectName, target, repeat)` [V: godma.py:1984, called
        by the module wrapper at :947].  NOT `dogmaLocation.Activate`, which
        takes an effectID and only starts the effect locally (clientDogmaLocation
        .py:392) -- the server never hears of it [V: run 2026-10-28, no heat]."""
        g = get_svc('godma')
        ok0, stm = safe(lambda: g.GetStateManager())
        eff = module_effect_name(item_id)
        if not ok0 or stm is None or eff is None:
            return False, 'no state manager (%s) or no default effect (%s)' % (
                text(stm), text(eff))
        okc, cat = safe(lambda: g.GetItem(item_id).defaultEffect.effectCategory)
        if target_id is None and okc and cat == 2:
            # a targeted effect (dgmEffTarget, category 2) needs its target
            # (DeniedActivateTargetNotPresent): the module button passes the
            # active target [V: shipmodulebutton.py:1260]
            tsvc = get_svc('target')
            ok5, tid = safe(lambda: tsvc.GetActiveTargetID())
            target_id = tid if ok5 and tid else None
        ok, v = safe(lambda: stm.Activate(item_id, eff, target_id, 1000))  # the module button's repeat (smb:1565): 0 is a ONE-SHOT
        if ok:
            return True, 'statemanager.Activate(itemID, effectName, target, repeat) took (effect %s)' % eff
        return False, 'statemanager.Activate raised %s' % text(v)[:160]

    def deactivate_module(item_id):
        g = get_svc('godma')
        ok0, stm = safe(lambda: g.GetStateManager())
        eff = module_effect_name(item_id)
        if not ok0 or stm is None or eff is None:
            return False, 'no state manager or default effect'
        ok, v = safe(lambda: stm.Deactivate(item_id, eff))
        return ok, 'statemanager.Deactivate(itemID, effectName): %s' % text(v)[:120]

    first_board_done = []

    def board_ship(type_id, timeout_ms=30000):
        """Make the hangar ship of `type_id` active, as a double-click does:
        `svc.station.TryActivateShip(invItem)` [V: svc_fittingspawner calls it].
        `(ok, detail)`; waits for `session.shipid` to become that ship."""
        say('board_ship: looking for type %s in the hangar' % text(type_id))
        item, why = find_hangar_item(type_id)
        say('board_ship: hangar says %s' % text(why))
        if item is None:
            return False, why
        st = get_svc('station')
        if st is None:
            return False, 'no station service'
        # TryActivateShip with missing skills OPENS A MODAL DIALOG
        # (prompt_missing_skill_requirements, station/base.py:210) and never
        # returns: the scenario thread hangs for ever [V: client.log
        # skill_requirement_dialog].  So give the skills first (GM), and
        # refuse to call when the client still sees any missing.
        def missing_skills():
            """None when the client cannot say (an exception, e.g. while its
            skill table reloads after a /giveskill): never read as 'none'."""
            d = dogma_loc()
            ok2, m = safe(lambda: d.GetMissingSkills(type_id))
            if not ok2:
                return None
            if not m:
                return []
            try:
                return list(m.items())
            except Exception:
                return [(k, 1) for k in m]

        def settle_skills():
            for sk, lvl in (missing_skills() or []):
                ok3, nm = safe(lambda: __import__('evetypes').GetEnglishName(sk))
                if ok3 and nm:
                    gm('/giveskill %s %s' % (text(nm), text(lvl)))
            wait_for(lambda: missing_skills() == [], timeout_ms=10000)
            return missing_skills()
        check('board_ship: found the ship in the hangar', True, text(why))
        left = settle_skills()
        if left != []:
            return False, 'the client still sees missing skills %s' % text(left)
        # The client refuses a session change within ~9 s of the last one
        # (login's 'charsel'): eveSessionMgr "Too frequent session change
        # attempts", PerformSessionChangeInProgress thrown inside the
        # MakeShipActive tasklet [V: client.log, sessions.py PerformSelectiveSessionChange].
        # TryActivateShip itself returns normally, so one call is silently
        # lost when boarding comes straight after login.  Retry until it takes.
        import time as _t
        t0 = _t.time()
        n = 0
        if not first_board_done:
            # the first change after login's charsel: let the lockout pass
            # before the first call (later boards are 10 s+ apart anyway)
            first_board_done.append(1)
            sleep_ms(9000)

        def changed():
            s = builtin('session')
            return getattr_safe(s, 'shipid') == item.itemID
        while True:
            n += 1
            # every call, not just the first: a call with a skill missing
            # opens the modal dialog and the thread never returns
            left = settle_skills()
            if left != []:
                return False, 'the client sees missing skills before call %d: %s' % (n, text(left))
            say('board_ship: TryActivateShip call %d' % n)
            check('board_ship: TryActivateShip call %d (skills settled: %s)' % (n, text(left)), True,
                  'missing now: %s' % text(missing_skills()))
            ok, v = safe(lambda: st.TryActivateShip(item))
            say('board_ship: call %d returned %s' % (n, text(ok)))
            if not ok:
                return False, 'TryActivateShip raised %s' % text(v)
            up, waited = wait_for(changed, timeout_ms=min(4000, timeout_ms))
            if up or (_t.time() - t0) * 1000 > timeout_ms:
                break
        return bool(up), 'session.shipid == %s after %s s, %d call(s): %s' % (
            text(item.itemID), text(int(_t.time() - t0)), n, text(bool(up)))

    def wait_ship_item(type_id, flag, timeout_ms=15000):
        got, waited = wait_for(lambda: module_item(type_id, flag), timeout_ms=timeout_ms)
        return got, waited

    # -- flying: belts, rocks, approach, lock (the ballpark API the older
    #    scenarios already use is [V]; the targeting calls are [A]) ----------

    GROUP_ASTEROID_BELT = 9
    CATEGORY_ASTEROID = 25
    MODE_WARP = 3          # DSTBALL_WARP, as `modes()` reads it in the prelude users

    def const_of(name, default):
        return getattr_safe(builtin('const'), name, default)

    def slims_where(park, pred):
        """`[(ballID, slim)]` of the park's slim items passing `pred(slim)`."""
        out = []
        try:
            for ballid, slim in park.slimItems.items():
                ok, good = safe(pred, slim)
                if ok and good:
                    out.append((ballid, slim))
        except Exception:
            pass
        return out

    def dist_to(park, ball_id):
        ok, d = safe(lambda: park.GetSurfaceDist(park.ego, ball_id))
        return d if ok else None

    def warp_to(park, ball_id, timeout_ms=120000, min_range=0.0):
        """Warp the ego ship to a ball and wait until the warp is over.
        `(ok, detail)`.  `michelle.CmdWarpToStuff` is the route
        `throttle_and_warp` and `warp_blob` run for real [V]."""
        mich = get_michelle()
        ego = park.GetBall(park.ego)
        ok, v = safe(lambda: mich.CmdWarpToStuff('item', ball_id, minRange=min_range))
        if not ok:
            return False, 'CmdWarpToStuff raised %s' % text(v)
        went, _w = wait_for(lambda: getattr_safe(ego, 'mode') == MODE_WARP, timeout_ms=30000)
        if not went:
            return False, 'never entered DSTBALL_WARP (mode %s)' % text(getattr_safe(ego, 'mode'))
        landed, waited = wait_for(lambda: getattr_safe(ego, 'mode') != MODE_WARP,
                                  timeout_ms=timeout_ms, step_ms=500)
        return bool(landed), 'warp over after %s ms, %s m from the target' % (
            text(waited), fmt(dist_to(park, ball_id), 0))

    def approach(park, ball_id, within_m, timeout_ms=200000):
        """`CmdFollowBall(ball, range)` [A] then wait to be inside `within_m`."""
        rbp = get_michelle().GetRemotePark()
        ok, v = safe(lambda: rbp.CmdFollowBall(ball_id, max(50, int(within_m / 2))))
        if not ok:
            return False, 'CmdFollowBall raised %s' % text(v)
        # a ship leaves the station with its speed fraction at 0 and Follow
        # does not raise it: the throttle is the pilot's [V: run 2026-10-28,
        # 25112 m after 200 s with the follow accepted; throttle_and_warp
        # sends CmdSetSpeedFraction(1.0) the same way]
        safe(lambda: rbp.CmdSetSpeedFraction(1.0))
        trail = []

        def close_enough():
            d = dist_to(park, ball_id)
            if len(trail) < 400 and (not trail or len(trail) % 40 == 0):
                eb = park.GetBall(park.ego)
                sp = (getattr_safe(eb, 'vx', 0.0) ** 2 + getattr_safe(eb, 'vy', 0.0) ** 2
                      + getattr_safe(eb, 'vz', 0.0) ** 2) ** 0.5
                trail.append('%s m @ %s m/s' % (fmt(d, 0), fmt(sp, 0)))
            trail.append('.')
            return (d or 1e12) < within_m
        near, waited = wait_for(close_enough, timeout_ms=timeout_ms, step_ms=500)
        return bool(near), '%s m from the target after %s ms; trail %s' % (
            fmt(dist_to(park, ball_id), 0), text(waited),
            text([t for t in trail if t != '.'][:8]))

    def lock_target(ball_id, timeout_ms=30000):
        """`svc.target.TryLockTarget` [A] and wait for it in `GetTargets()`."""
        tgt = get_svc('target')
        if tgt is None:
            return False, 'no svc.target'
        ok, v = safe(lambda: tgt.TryLockTarget(ball_id))
        if not ok:
            return False, 'TryLockTarget raised %s' % text(v)
        got, waited = wait_for(lambda: ball_id in (tgt.GetTargets() or {}),
                               timeout_ms=timeout_ms)
        return bool(got), 'in svc.target.GetTargets() after %s ms: %s' % (
            text(waited), text(bool(got)))

    def go_to_rock(park, within_m=3000):
        """Warp to the nearest belt, find a rock, approach it.
        `(rock_ball_id or None, detail)`."""
        belts = slims_where(park, lambda s: getattr_safe(s, 'groupID')
                            == const_of('groupAsteroidBelt', GROUP_ASTEROID_BELT))
        if not belts:
            return None, 'no asteroid belt in the park'
        best = sorted([(dist_to(park, b) or 1e18, b) for b, _s in belts])[0][1]
        ok, detail = warp_to(park, best)
        if not ok:
            return None, 'warp to belt %s: %s' % (text(best), detail)
        rocks, waited = wait_for(
            lambda: slims_where(park, lambda s: getattr_safe(s, 'categoryID')
                                == const_of('categoryAsteroid', CATEGORY_ASTEROID)),
            timeout_ms=20000)
        if not rocks:
            return None, 'no asteroid appeared within %s ms of landing' % text(waited)
        rock = sorted([(dist_to(park, r) or 1e18, r) for r, _s in rocks])[0][1]
        ok, d2 = approach(park, rock, within_m)
        return (rock if ok else None), 'belt %s; rock %s of %d; %s' % (
            text(best), text(rock), len(rocks), d2)

    def module_active(item_id):
        """True/False if the client can tell whether a module's default effect
        is running, else None.  [A] `godmaItem.effects[name].isActive`."""
        eff = module_effect_name(item_id)
        g = get_svc('godma')
        ok, it = safe(lambda: g.GetItem(item_id))
        if ok and it is not None and eff:
            ok2, v = safe(lambda: bool(it.effects[eff].isActive))
            if ok2:
                return v
        return None

    def load_ammo(module_ids, charge_type_id, charge_item):
        """Load a hangar/cargo charge stack into modules.  [A]
        `dogmaLocation.LoadAmmoToModules(shipID, moduleIDs, chargeItemIDs,
        ammoLocationID)` [V: clientDogmaLocation.py:957, 4 args + self].
        `charge_type_id` is kept for the callers and unused.  `(ok, detail)`."""
        loc = dogma_loc()
        sess = builtin('session')
        if loc is None:
            return False, 'no dogma location'
        ok, v = safe(lambda: loc.LoadAmmoToModules(
            sess.shipid, list(module_ids), [charge_item.itemID],
            charge_item.locationID))
        return ok, 'LoadAmmoToModules: %s' % (text(v)[:160] if not ok else 'accepted')

    # -- an alliance to hold things with (used by the sov scenarios) --------

    def ensure_alliance():
        """`(corp_id, alliance_id, detail)`.  The character's alliance, founded
        if it has none: GM ISK and skill, `AddCorporation`, a Master Wallet
        deposit, `CreateAlliance` (the calls `fci_corp_alliance` asserts one by
        one).  [A] `AddCorporation`'s positional shape; the server reads it by
        argument name."""
        sess = builtin('session')
        aid = getattr_safe(sess, 'allianceid')
        if aid:
            return getattr_safe(sess, 'corpid'), aid, 'already in alliance %s' % text(aid)
        gm('/giveisk 3000000000')
        gm('/giveskill Corporation Management 1')
        sfx = uniq(4)
        old = getattr_safe(sess, 'corpid')
        ok, v = rcall('corpRegistry', 'AddCorporation', 'Fci Corp ' + sfx, 'F' + sfx,
                      'f-ci', '', 0.0, 1, 1, 1, 1, 1, 1, True, False, 0.0)
        if not ok:
            return None, None, 'AddCorporation: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') != old,
                           timeout_ms=30000)
        corp = getattr_safe(builtin('session'), 'corpid')
        if not got:
            return None, None, 'session.corpid never changed from %s' % text(old)
        ok, v = rcall_kw('account', 'GiveCash', (corp, 1000000000.0, 'fee'),
                         {'toAccountKey': 1000})
        if not ok:
            return corp, None, 'Master Wallet deposit: ' + text(v)
        ok, v = rcall('corpRegistry', 'CreateAlliance', 'Fci Alliance ' + sfx, 'A' + sfx,
                      'f-ci', '')
        if not ok:
            return corp, None, 'CreateAlliance: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'allianceid'),
                           timeout_ms=30000)
        return corp, got, ('founded alliance %s' % text(got)) if got else 'allianceid never arrived'

    # -- end of the fci block ------------------------------------------------

    GUN = '200mm autocannon i'
    AMMO = 'emp s'
    RIFTER = 'rifter'
    CATEGORY_ENTITY = 11
    RAT_WAIT_MS = 180000
    PEER_WAIT_MS = 600000
    PHASE_MS = 60000
    WARP_AWAY_MS = 240000
    # The distances the orbit step asks for, and the wide bounds on what it
    # must hold: "no rubber-band", not a model of the orbit.
    ORBIT_A_M = 7500
    ORBIT_B_M = 5000
    ORBIT_MIN_M = 2500.0
    # Both pilots land ~950 m from the same belt, so the first seconds of a
    # mutual orbit start ~2 km apart: the bounds apply after this settle.
    ORBIT_SETTLE_MS = 20000
    ORBIT_MAX_M = 12000.0
    MIN_WARP_M = 150000.0
    BIG_BATCH_MIN = 20

    # What the client logs when its history merge misbehaves
    # (`docs/lanes/pa10-research`, report 1.1-1.4); none may appear.
    LOG_NEEDLES = ('Found update batch', 'Failed to synchronize',
                   'discarded a state that was too old', 'Events ignored',
                   'Requesting new state')

    # -- instruments ---------------------------------------------------------

    shim = {'calls': 0, 'sizes': {}, 'multi_entry': 0, 'multi_stamp': 0,
            'resets': 0, 'installed': False, 'reset_wrapped': False,
            'rat_ids': set(), 'rat_damage': 0}

    def size_bucket(n):
        if n <= 2:
            return str(n)
        if n <= 4:
            return '3-4'
        if n <= 8:
            return '5-8'
        return '9+'

    def install_batch_shim(mich):
        """Wrap `michelle.DoDestinyUpdate` with a counter.  [A] the client's
        notify loop calls through `getattr(srv, 'DoDestinyUpdate')`, so an
        instance attribute shadows the method.  Falsifier: `calls` stays 0
        while the ego ball moves (reported as a PRECONDITION)."""
        orig = getattr_safe(mich, 'DoDestinyUpdate')
        if orig is None:
            return False, 'michelle has no DoDestinyUpdate'

        def counting(*args, **kw):
            try:
                state = args[0] if args else kw.get('state')
                n = len(state)
                shim['calls'] += 1
                key = size_bucket(n)
                shim['sizes'][key] = shim['sizes'].get(key, 0) + 1
                if n >= 2:
                    shim['multi_entry'] += 1
                if len(set([a[0] for a in state])) > 1:
                    shim['multi_stamp'] += 1
            except Exception:
                pass
            try:
                # Count the events, not 500 ms samples of the state: a batch may
                # arrive as a PackagedAction (a marshalled list of entries).
                entries = []
                for a in (args[0] if args else kw.get('state')):
                    if a[1][0] == 'PackagedAction':
                        import blue
                        entries.extend(blue.marshal.Load(a[1][1]))
                    else:
                        entries.append(a)
                for a in entries:
                    if a[1][0] == 'OnDamageStateChange' and a[1][1][0] in shim['rat_ids']:
                        shim['rat_damage'] += 1
            except Exception:
                pass
            return orig(*args, **kw)
        ok, v = safe(lambda: setattr(mich, 'DoDestinyUpdate', counting))
        shim['installed'] = ok
        return ok, 'shadowed michelle.DoDestinyUpdate' if ok else text(v)

    def install_reset_counter(park):
        """Count `park.RequestReset` calls (a desync recovery).  [A] the name
        and that the park takes an instance attribute."""
        orig = getattr_safe(park, 'RequestReset')
        if orig is None:
            return False, 'the park has no RequestReset'

        def counting(*args, **kw):
            shim['resets'] += 1
            return orig(*args, **kw)
        ok, v = safe(lambda: setattr(park, 'RequestReset', counting))
        shim['reset_wrapped'] = ok
        return ok, 'wrapped park.RequestReset' if ok else text(v)

    def stat_total(entries, key):
        """The sum of stat `key` over every state in `clientStatsSvc.entries`.
        [A] the shape: a mapping of state to a mapping of stat to a number
        (or a list, counted by length)."""
        total = 0
        try:
            values = entries.values()
        except Exception:
            return None
        for per in values:
            try:
                v = per.get(key)
            except Exception:
                v = None
            if v is None:
                continue
            try:
                total += v
            except Exception:
                try:
                    total += len(v)
                except Exception:
                    pass
        return total

    def desync_counts():
        """`(fatal, recoverable, detail)`; the constants are read by name from
        the client's module, never as literals."""
        try:
            from eve.common.script.util import clientStatsCommon
        except Exception as e:
            return None, None, 'no clientStatsCommon: %s' % text(e)
        fkey = getattr_safe(clientStatsCommon, 'STAT_FATAL_DESYNCS')
        rkey = getattr_safe(clientStatsCommon, 'STAT_RECOVERABLE_DESYNCS')
        svc = get_svc('clientStatsSvc')
        entries = getattr_safe(svc, 'entries')
        if fkey is None or rkey is None or entries is None:
            return None, None, ('keys %s/%s, entries %s'
                                % (text(fkey), text(rkey), shape(entries)))
        return (stat_total(entries, fkey), stat_total(entries, rkey),
                'keys %s/%s' % (text(fkey), text(rkey)))

    def history_len(park):
        for name in ('history', 'historyTimeEntries'):
            h = getattr_safe(park, name)
            if h is not None:
                try:
                    return len(h), name
                except Exception:
                    pass
        return None, 'no history attribute'

    # -- the scenario's pilots ------------------------------------------------

    def rats(park):
        return slims_where(park, lambda s: getattr_safe(s, 'categoryID')
                           == const_of('categoryEntity', CATEGORY_ENTITY))

    def other_pilot(park):
        """`(ball_id, charid)` of another player's ship in the park, or None."""
        me = getattr_safe(builtin('session'), 'charid')
        found = slims_where(park, lambda s: getattr_safe(s, 'charID')
                            and getattr_safe(s, 'charID') != me)
        if not found:
            return None
        return found[0][0], getattr_safe(found[0][1], 'charID')

    def ball_of(park, ball_id):
        return getattr_safe(park, 'GetBall') and park.GetBall(ball_id)

    def orbit(park, ball_id, metres):
        """`CmdOrbit` through the client's remote ballpark, then full speed.
        [A] the route and signature."""
        rbp = get_michelle().GetRemotePark()
        ok, v = safe(lambda: rbp.CmdOrbit(ball_id, int(metres)))
        if not ok:
            return False, 'CmdOrbit raised %s' % text(v)
        safe(lambda: rbp.CmdSetSpeedFraction(1.0))
        return True, 'CmdOrbit(%s, %d)' % (text(ball_id), metres)

    def position(ball):
        return (getattr_safe(ball, 'x', 0.0), getattr_safe(ball, 'y', 0.0),
                getattr_safe(ball, 'z', 0.0))

    def moved(a, b):
        return ((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2) ** 0.5

    def damage_of(park, ball_id):
        """`GetDamageState` is on the park, not on michelle, and it recomputes
        the shield over time, so the figures are rounded: a regenerating shield
        must not look like a change every sample.  Armour and hull are kept."""
        ok, v = safe(lambda: park.GetDamageState(ball_id))
        if not ok or not v:
            return None
        ok, r = safe(lambda: [round(float(x), 2) for x in v[:3]])
        return r if ok else None

    def watch_the_other_leave_and_return(park, other, label):
        """Another pilot warps off and back; this client only watches its
        ball: gone from the park (RemoveBalls), then back (AddBalls2)."""
        gone, w1 = wait_for(lambda: ball_of(park, other) is None,
                            timeout_ms=WARP_AWAY_MS, step_ms=500)
        check('%s: the other ship leaves the park' % label, bool(gone),
              'after %s ms' % text(w1))
        if not gone:
            return False
        back, w2 = wait_for(lambda: ball_of(park, other) is not None,
                            timeout_ms=WARP_AWAY_MS, step_ms=500)
        check('%s: the other ship returns to the park' % label, bool(back),
              'after %s ms' % text(w2))
        return bool(back)

    def warp_out_and_back(park, here, there, label):
        """This client warps to `there` and back to `here`, with a dwell so an
        observer sees the absence.  Checks the distance covered."""
        start = position(ball_of(park, park.ego))
        ok, detail = warp_to(park, there)
        check('%s: warped to the far celestial' % label, ok, detail)
        if not ok:
            return False
        far_pos = position(ball_of(park, park.ego))
        d = moved(start, far_pos)
        check('%s: the ego position moved by %.0f m or more' % (label, MIN_WARP_M),
              d >= MIN_WARP_M, 'moved %s m' % fmt(d, 0))
        sleep_ms(10000)
        ok, detail = warp_to(park, here)
        check('%s: warped back' % label, ok, detail)
        ego = ball_of(park, park.ego)
        check('%s: the ego ball is in the park at the end' % label, ego is not None,
              'ego %s' % text(park.ego))
        return ok

    def eoci_body():
        mark = log_mark()
        gun = type_ids_named(GUN, exact=True)
        ammo = type_ids_named(AMMO, exact=True)
        rifter = type_ids_named(RIFTER, exact=True)
        precondition('the client knows the gun, ammo and Rifter',
                     bool(gun and ammo and rifter),
                     'gun=%s ammo=%s rifter=%s' % (text(gun), text(ammo), text(rifter)))
        if not (gun and ammo and rifter):
            return

        # Instruments first, before anything is sent to the client's park.
        mich = get_michelle()
        ok, detail = install_batch_shim(mich)
        precondition('the DoDestinyUpdate counting shim is installed', ok, detail)
        f0, r0, why = desync_counts()
        precondition('the client\'s desync counters can be read', f0 is not None
                     and r0 is not None, '%s; fatal %s recoverable %s'
                     % (why, text(f0), text(r0)))

        for line in ('/giveskill Gunnery 3', '/giveskill Small Projectile Turret 3',
                     '/createitem rifter', '/createitem "200mm AutoCannon I" 2',
                     '/createitem "EMP S" 500'):
            gm_check('GM: ' + line, line)
        ok, detail = board_ship(rifter[0])
        check('boarded the Rifter', ok, detail)
        if not ok:
            return
        guns = []
        for slot in (FLAG_HI0, FLAG_HI0 + 1):
            ok, detail = fit_module(gun[0], slot)
            check('a gun fits at flag %d' % slot, ok, detail)
            m = module_item(gun[0], slot)
            if m is not None:
                guns.append(m)
        check('both guns are fitted', len(guns) >= 2, '%d of 2 fitted' % len(guns))
        if len(guns) < 2:
            return
        for m in guns:
            ok, detail = online_module(m.itemID)
            check('module %s onlines' % text(m.itemID), ok, detail)
        charge, why = find_hangar_item(ammo[0])
        check('the EMP S stack is in the hangar', charge is not None, why)
        if charge is None:
            return
        ok, detail = load_ammo([m.itemID for m in guns], ammo[0], charge)
        check('the guns take the EMP S', ok, detail)
        if not ok:
            return

        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)
        if not _undocked:
            return
        park, ball = get_ego_ball()
        if park is None or ball is None:
            check('the ego ball is in the park', False, 'no park/ball')
            return
        ok, detail = install_reset_counter(park)
        precondition('the RequestReset counter is installed', ok, detail)

        belts = slims_where(park, lambda s: getattr_safe(s, 'groupID')
                            == const_of('groupAsteroidBelt', GROUP_ASTEROID_BELT))
        precondition('two asteroid belts are in the park (one to fight in, one to warp to)',
                     len(belts) >= 2, '%d belt(s)' % len(belts))
        if len(belts) < 2:
            return
        by_dist = sorted([(dist_to(park, b) or 1e18, b) for b, _s in belts])
        here = by_dist[0][1]
        there = by_dist[-1][1]
        ok, detail = warp_to(park, here)
        check('warped to the belt', ok, detail)
        if not ok:
            return

        # Both clients run this file; the pilot with the lower character id
        # is A (observer first), the other is B.
        peer, waited = wait_for(lambda: other_pilot(park), timeout_ms=PEER_WAIT_MS,
                                step_ms=1000)
        precondition('the other pilot arrives on the grid', bool(peer),
                     'after %s ms' % text(waited))
        if not peer:
            return
        other, other_char = peer
        me = getattr_safe(builtin('session'), 'charid')
        is_a = me < other_char
        say('PA-10 role: %s (me %s, other %s)' % ('A' if is_a else 'B', text(me),
                                                  text(other_char)))
        check('this client took a role', True, 'role %s' % ('A' if is_a else 'B'))

        # The peer's ball is added while it is still in warp, and its landing is
        # a fresh remove-and-add (grid.rs) that ends an orbit ordered before it:
        # wait until it has landed near us and the reconcile has passed.
        def peer_landed():
            b = ball_of(park, other)
            d = dist_to(park, other)
            return (b is not None and d is not None and d < 100000.0
                    and getattr_safe(b, 'mode') != MODE_WARP)
        landed, waited = wait_for(peer_landed, timeout_ms=PEER_WAIT_MS, step_ms=1000)
        precondition('the other pilot has landed on the grid', bool(landed),
                     'after %s ms' % text(waited))
        if not landed:
            return
        sleep_ms(3000)

        # -- 1. orbit, 60 s -----------------------------------------------------
        mode_orbit = getattr_safe(__import__('destiny'), 'DSTBALL_ORBIT')
        ok, detail = orbit(park, other, ORBIT_A_M if is_a else ORBIT_B_M)
        check('step 1: orbit ordered', ok, detail)
        samples = 0
        in_orbit = 0
        lo = 1e18
        hi = 0.0
        t0 = now_ms()
        # The first ORBIT_SETTLE_MS are not sampled for the bounds nor the
        # orbit count, so the 60 s of samples that remain are the same 60 s
        # check as before; the bounds do not move.
        while t0 is not None and now_ms() - t0 < PHASE_MS + ORBIT_SETTLE_MS:
            sleep_ms(1000)
            if now_ms() - t0 < ORBIT_SETTLE_MS:
                continue
            samples += 1
            ego = ball_of(park, park.ego)
            if getattr_safe(ego, 'mode') == mode_orbit:
                in_orbit += 1
            d = dist_to(park, other)
            if d is not None:
                lo = min(lo, d)
                hi = max(hi, d)
        check('step 1: the ego ball is in orbit mode for 55 of 60 s',
              in_orbit >= 55 and samples >= 55,
              '%d of %d samples, mode value %s' % (in_orbit, samples, text(mode_orbit)))
        check('step 1: the distance stays within the wide bounds (no rubber-band)',
              lo >= ORBIT_MIN_M and hi <= ORBIT_MAX_M,
              'min %s m max %s m (bounds %d..%d)' % (fmt(lo, 0), fmt(hi, 0),
                                                     ORBIT_MIN_M, ORBIT_MAX_M))

        # -- 2. fire, 60 s ------------------------------------------------------
        found, waited = wait_for(lambda: rats(park), timeout_ms=RAT_WAIT_MS, step_ms=1000)
        precondition('belt rats appear', bool(found),
                     '%d rat slim(s) after %s ms' % (len(found or []), text(waited)))
        if found:
            changes = 0
            own_changes = 0
            last = {}
            own_last = None
            locked = None
            t0 = now_ms()
            events0 = shim['rat_damage']
            while t0 is not None and now_ms() - t0 < PHASE_MS:
                live = rats(park)
                shim['rat_ids'].update([r for r, _s in live])
                if locked is None or locked not in [r for r, _s in live]:
                    locked = None
                    if live:
                        near = sorted([(dist_to(park, r) or 1e18, r) for r, _s in live])
                        target = near[0][1]
                        ok, _d = lock_target(target, timeout_ms=15000)
                        if ok:
                            locked = target
                            for m in guns:
                                safe(lambda: activate_module(m.itemID))
                # Count every live rat, not only the one this pilot locked:
                # both pilots shoot the same few rats, so a locked-only count
                # is a share of a fixed pool (4+7 and 6+5 in the regress run),
                # not a coalescing effect.  `OnDamageStateChange` reaches both
                # clients for every rat, so the event under test is the same.
                for r, _s in live:
                    now_state = damage_of(park, r)
                    if r in last and now_state != last[r]:
                        changes += 1
                    last[r] = now_state
                own = damage_of(park, park.ego)
                if own_last is not None and own != own_last:
                    own_changes += 1
                own_last = own
                sleep_ms(500)
            for m in guns:
                deactivate_module(m.itemID)
            events = shim['rat_damage'] - events0
            check('step 2: rats\' damage state changes at least 5 times '
                  '(OnDamageStateChange events seen in the batch shim)', events >= 5,
                  '%d event(s) over %d ms; %d change(s) sampled' % (events, PHASE_MS, changes))
            check('step 2 (informational): our own damage state while the rats shoot back',
                  True, '%d change(s)' % own_changes)

        # -- 3. warp: B away and back (A watches), then A (B watches) -----------
        if is_a:
            watch_the_other_leave_and_return(park, other, 'step 3 (B warps)')
            warp_out_and_back(park, here, there, 'step 3 (A warps)')
            sleep_ms(15000)
        else:
            warp_out_and_back(park, here, there, 'step 3 (B warps)')
            watch_the_other_leave_and_return(park, other, 'step 3 (A warps)')

        # -- the verdict rows ---------------------------------------------------
        sleep_ms(5000)
        f1, r1, _why = desync_counts()
        if f0 is not None and f1 is not None:
            check('GATE: fatal desyncs delta == 0', f1 - f0 == 0,
                  'fatal %s -> %s' % (text(f0), text(f1)))
        if r0 is not None and r1 is not None:
            check('GATE: recoverable desyncs delta == 0', r1 - r0 == 0,
                  'recoverable %s -> %s' % (text(r0), text(r1)))
        precondition('the batch shim was reached (DoDestinyUpdate calls seen)',
                     shim['calls'] > 0, '%d call(s); shim not reached if 0' % shim['calls'])
        check('RequestReset was never called', shim['resets'] == 0,
              '%d call(s); counter wrapped: %s' % (shim['resets'], text(shim['reset_wrapped'])))
        check('no batch carries more than one stamp (the OnFatalDesync path)',
              shim['multi_stamp'] == 0, '%d batch(es)' % shim['multi_stamp'])
        hlen, hname = history_len(park)
        check('the park history is short (no waitForBubble stall, nothing piling up)',
              hlen is not None and hlen <= 2, 'len(park.%s) = %s' % (hname, text(hlen)))
        check('park.validState is true', bool(getattr_safe(park, 'validState')),
              'validState = %s' % text(getattr_safe(park, 'validState')))
        # The host drops `coalesce=1` into fci.params for the --coalesce-destiny
        # run; without it this row is informational (the off run is the control).
        on_run = params().get('coalesce') == '1'
        enough = shim['multi_entry'] >= BIG_BATCH_MIN
        check(('merging happened: at least %d calls with len(state) >= 2' % BIG_BATCH_MIN)
              if on_run else 'INFO (off-run control): calls with len(state) >= 2',
              enough or not on_run,
              '%d of %d call(s); sizes %s' % (shim['multi_entry'], shim['calls'],
                                              text(sorted(shim['sizes'].items()))))
        blob, detail = log_since(mark)
        if blob is None:
            check('the client log can be read for the history-merge lines', False, detail)
        else:
            for needle in LOG_NEEDLES:
                hits = log_lines_with(blob, needle)
                check('client log has no %r' % needle, not hits, text(hits))
        no_new_tracebacks('PA-10 coalescing', mark)

    # -- end of the scenario body ------------------------------------------
    # PRELUDE (part 2) -- identical in every scenario.  Always writes the
    # verdict, even on catastrophe: a missing file and a failing check are
    # different diagnoses and the harness tells them apart.

    error = None
    try:
        eoci_body()
    except Exception:
        error = traceback.format_exc()
    finally:
        if not checks and error is None:
            check('scenario recorded at least one check', False,
                  'the body returned without asserting anything')
        try:
            write_result(error)
        except Exception:
            say('write_result itself raised:\n%s' % traceback.format_exc())
        quit_client('eve-offline client-ci: %s finished' % EOCI_NAME)


eoci_main(EOCI_SCENARIO)
