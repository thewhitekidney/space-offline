# -*- coding: utf-8 -*-
"""Throttle, then warp, and read back `speedFraction` and `mode` off the ball.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  See `docs/lanes/ci-scenarios.md`.

This is the scenario `docs/port-tracker.md` section 34 is asking for.  That
defect -- a warp that aligned forever -- was one number: the ball's
`speedFraction` was 0, and the client's own align gate needs

    |1 - dot(normalize(gotoTarget - position), normalize(velocity))| < 0.01
    speed^2 > (0.75 * maxVelocity)^2

so a commanded speed of zero makes **both** gates permanently unpassable
(`_destiny.dll` `AlignForWarp` `0x180037357`, the throttle-scaled goto
integrator at `0x180038870`).  What the player saw instead was three minutes
of "Aligning" for every hull equally, ended only by the client's own 180-tick
watchdog.  Nothing about the `WarpTo` arguments was wrong, which is exactly
why every existing test passed: the defect was in what the batch did *not*
also contain.

Reading `ball.speedFraction` and `ball.mode` after a warp command is a
two-second check.  So is confirming the ship moved.
"""

EOCI_SCENARIO = 'throttle_and_warp'

# ---------------------------------------------------------------------------
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

    #: `0.75 * maxVelocity`, squared, is the client's own align gate:
    #: the f64 `0.5625` at `_destiny.dll 0x1800ad848`, compared against the
    #: squared speed from `ball+0x178..0x188` at `0x1800374ed`
    #: (`docs/port-tracker.md` s34.1).
    ALIGN_GATE = 0.75

    #: `const.minWarpDistance` (`eve/common/lib/appConst.pyc:1747`).
    MIN_WARP_DISTANCE = 150000.0

    #: How long to let the warp run before deciding it never started.  The
    #: client's own watchdog is 180 ticks (`_destiny.dll 0x180037522`), i.e.
    #: three minutes, so anything well under that distinguishes "aligning
    #: forever" from "aligning normally": a correct align is sub-second for a
    #: frigate and a couple of seconds for the heaviest hulls.
    WARP_SETTLE_MS = 25000

    def modes():
        """`{value: name}` for `destiny.DSTBALL_*`.

        Values from `_destiny.dll`'s own mode docstrings
        (`docs/archive/destiny-and-the-ballpark.md` s7.1); the names are read back off
        the `destiny` module so a mismatch shows up as a missing entry rather
        than as a wrong label.
        """
        out = {}
        try:
            import destiny
        except Exception:
            return out
        for name in ('DSTBALL_GOTO', 'DSTBALL_FOLLOW', 'DSTBALL_STOP',
                     'DSTBALL_WARP', 'DSTBALL_ORBIT', 'DSTBALL_MISSILE',
                     'DSTBALL_MUSHROOM', 'DSTBALL_BOID', 'DSTBALL_TROLL',
                     'DSTBALL_FIELD', 'DSTBALL_MINIBALL', 'DSTBALL_RIGID',
                     'DSTBALL_FORMATION'):
            value = getattr_safe(destiny, name)
            if value is not None:
                out[value] = name
        return out

    def mode_name(value, table):
        return '%s (%s)' % (text(value), table.get(value, 'unknown mode'))

    def speed_of(ball):
        """The ship's speed, the way the HUD reads it.

        `ActiveShipController.GetSpeed` is
        `GetBall().GetVectorDotAt(blue.os.GetSimTime()).Length()`
        (`activeShipController.pyc:167-170`) [V]:

            LOAD_ATTR GetVectorDotAt / CALL_FUNCTION 1
            LOAD_ATTR Length        / CALL_FUNCTION 0

        **The `.Length()` is not optional**, and this docstring claimed [V]
        without it. `GetVectorDotAt` returns a native vector, so `speed_of`
        handed every caller a vector where they expected a float -- and a
        native vector compared against a float is not greater than it, so a
        ship at exactly maxVelocity failed two checks:

            the ship accelerated under its own throttle: FAIL
            ball.GetVectorDotAt(simTime) = (0.000, 0.000, -150.000) m/s,
            maxVelocity 150.0 m/s, speedFraction 1.0

        The server was right and the scenario was wrong, which is the one
        failure mode a test must not have. An incompletely verified [V] is
        worse than an honest [I].

        `ball.vx/vy/vz` is the last fallback: those are native
        `destiny.Ball` members [V] but they are the last integrated value
        rather than the interpolated one.
        """
        import blue
        import math
        vector = None
        try:
            vector = ball.GetVectorDotAt(blue.os.GetSimTime())
        except Exception:
            vector = None
        if vector is not None:
            try:
                return vector.Length()
            except Exception:
                pass
            # A sequence, if this build ever returns one instead.
            try:
                return math.sqrt(vector[0] ** 2 + vector[1] ** 2
                                 + vector[2] ** 2)
            except Exception:
                pass
        try:
            return math.sqrt(ball.vx ** 2 + ball.vy ** 2 + ball.vz ** 2)
        except Exception:
            return None

    def forward_of(ball):
        """The ship's own forward vector, exactly as `SetSpeed` builds it.

        `trinity.TriVector(0.0, 0.0, 1.0)` transformed by
        `ball.GetQuaternionAt(blue.os.GetSimTime())`
        (`activeShipController.pyc:224-226`) [V].  `(0, 0, 1)` is the
        fallback if trinity is not importable, which only costs us a goto in
        an arbitrary direction -- the throttle is what is under test.
        """
        import blue
        try:
            import trinity
            direction = trinity.TriVector(0.0, 0.0, 1.0)
            direction.TransformQuaternion(
                ball.GetQuaternionAt(blue.os.GetSimTime()))
            return (direction.x, direction.y, direction.z)
        except Exception:
            return (0.0, 0.0, 1.0)

    def warp_target(park, shipid):
        """A celestial far enough away to be worth warping to.

        `Park.GetWarpinPoints()` (`michelle.pyc:1920`) iterates
        `self.globals` and keeps what `ballparkCommon.parkhelpers`
        `HasWarpInPoint` accepts -- `radius > 90000 and categoryID ==
        categoryCelestial and groupID != groupWreck` [V] -- which is the
        client's own definition of a warpable body.  Returns
        `[(ball, slimItem, warpinPoint), ...]`.
        """
        rows = []
        try:
            rows = park.GetWarpinPoints() or []
        except Exception:
            rows = []
        best = None
        notes = []
        for row in rows:
            try:
                target = row[0]
                slim = row[1]
                targetID = target.id
            except Exception:
                continue
            dist = None
            try:
                dist = park.GetSurfaceDist(shipid, targetID)
            except Exception:
                dist = None
            notes.append('%s(type=%s dist=%s)'
                         % (text(targetID), text(getattr_safe(slim, 'typeID')),
                            fmt(dist, 0)))
            if dist is None or dist < MIN_WARP_DISTANCE:
                continue
            if best is None or dist > best[1]:
                best = (targetID, dist)
        return best, '%d warpable celestial(s): %s' % (len(rows),
                                                       text(notes))

    def eoci_body():
        # **Undock first.** `/autoSelectCharacter:0` lands in the station,
        # and there is no ballpark in a hangar -- the first unattended run
        # failed on exactly that, with five checks green and
        # `session.stationid = 60012526`. The prelude's `undock` has the
        # evidence that this does not prompt.
        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)
        import math

        session = builtin('session')
        if session is None:
            check('session is reachable', False, 'session = None')
            return
        shipid = getattr_safe(session, 'shipid')

        table = modes()
        check('the destiny mode constants are readable', len(table) >= 5,
              'destiny.DSTBALL_* resolved to %d values: %s'
              % (len(table), text(sorted(table.items()))))

        park, ball = get_ego_ball()
        check('the ego ball is in the park', ball is not None,
              'ballpark.GetBall(ballpark.ego) = %s' % text(ball))
        if park is None or ball is None:
            return

        mich = get_michelle()
        rbp = None
        try:
            rbp = mich.GetRemotePark()
        except Exception:
            rbp = None
        check('the remote ballpark is bound', rbp is not None,
              'michelle.GetRemotePark() = %s -- this is ballpark.remoteBallpark'
              ', the bound beyonce object every movement command goes to'
              ' (michelle.pyc:516)' % text(rbp))
        if rbp is None:
            return

        max_velocity = getattr_safe(ball, 'maxVelocity') or 0.0
        check('the ego ball has a positive maxVelocity', max_velocity > 0,
              'ball.maxVelocity = %s m/s.  Every number below is scaled by'
              ' this, and the align gate is 0.75 of it' % fmt(max_velocity))

        start_mode = getattr_safe(ball, 'mode')
        start_fraction = getattr_safe(ball, 'speedFraction')
        say('before the throttle: mode=%s speedFraction=%s maxVelocity=%s'
            % (mode_name(start_mode, table), text(start_fraction),
               fmt(max_velocity)))

        # The premise `docs/port-tracker.md` s34.5 insists on asserting first,
        # so that this scenario fails loudly if it ever stops being true
        # rather than quietly testing nothing.
        check('a freshly undocked ship starts with the throttle shut',
              start_fraction == 0 or start_fraction is None
              or float(start_fraction) == 0.0,
              'ball.speedFraction = %s before any command.  If this is'
              ' already non-zero the two throttle checks below prove nothing'
              % text(start_fraction))

        # -- phase 1: the throttle --------------------------------------
        # `activeShipController.SetSpeed` sends BOTH, `CmdGotoDirection`
        # first and `CmdSetSpeedFraction` immediately after: the
        # `mode == DSTBALL_STOP` branch at `:222` falls through to the
        # `CmdSetSpeedFraction` at `:230` [V].  Sending only the fraction is
        # the mistake `docs/lanes/movement.md` s1.5 documents, and it leaves
        # the ship in STOP forever.
        direction = forward_of(ball)
        goto_sent = False
        try:
            rbp.CmdGotoDirection(direction[0], direction[1], direction[2])
            goto_sent = True
        except Exception:
            goto_sent = False
        check('CmdGotoDirection was accepted', goto_sent,
              'rbp.CmdGotoDirection(%s, %s, %s) -- the ship\'s own forward'
              ' vector (activeShipController.pyc:227)'
              % (fmt(direction[0], 4), fmt(direction[1], 4),
                 fmt(direction[2], 4)))

        throttle_sent = False
        try:
            rbp.CmdSetSpeedFraction(1.0)
            throttle_sent = True
        except Exception:
            throttle_sent = False
        check('CmdSetSpeedFraction was accepted', throttle_sent,
              'rbp.CmdSetSpeedFraction(1.0) (activeShipController.pyc:230)')

        moved, waited = wait_for(
            lambda: (getattr_safe(ball, 'speedFraction') or 0) > 0,
            timeout_ms=15000)
        now_fraction = getattr_safe(ball, 'speedFraction')
        now_mode = getattr_safe(ball, 'mode')
        check('the throttle reached the ball', bool(moved),
              'ball.speedFraction = %s after %d ms.  Zero means the server'
              ' answered CmdSetSpeedFraction without pushing a'
              ' SetSpeedFraction action, and nothing will ever accelerate'
              % (text(now_fraction), waited))
        check('the ball left DSTBALL_STOP',
              now_mode is not None and now_mode != 2,
              'ball.mode = %s, was %s.  A goto from a standing start should'
              ' land in 0 (DSTBALL_GOTO); still 2 (DSTBALL_STOP) means the'
              ' server never pushed a GotoDirection action'
              % (mode_name(now_mode, table), mode_name(start_mode, table)))

        sleep_ms(4000)
        sub_warp_speed = speed_of(ball)
        check('the ship accelerated under its own throttle',
              (sub_warp_speed or 0) > 0,
              'ball.GetVectorDotAt(simTime) = %s m/s four seconds after the'
              ' throttle, maxVelocity %s m/s, speedFraction %s'
              % (fmt(sub_warp_speed), fmt(max_velocity),
                 text(getattr_safe(ball, 'speedFraction'))))

        # -- phase 2: the warp ------------------------------------------
        target, note = warp_target(park, shipid)
        check('a warpable celestial is in the park', target is not None,
              '%s; need one at least %s m away (const.minWarpDistance)'
              % (note, fmt(MIN_WARP_DISTANCE, 0)))
        if target is None:
            return
        targetID, target_distance = target

        before = (getattr_safe(ball, 'x'), getattr_safe(ball, 'y'),
                  getattr_safe(ball, 'z'))

        # `michelle.CmdWarpToStuff(subject, subjectID, **kwargs)`
        # (`michelle.pyc:945-949`) is the path every "Warp To" in the UI
        # takes; `SetFleetFormationSettings` in front of it is a no-op unless
        # `kwargs['fleet']` is truthy (`:969`) [V], so this is the UI's own
        # call with nothing added.
        warp_sent = False
        try:
            mich.CmdWarpToStuff('item', targetID, minRange=0.0)
            warp_sent = True
        except Exception:
            warp_sent = False
        check('CmdWarpToStuff was accepted', warp_sent,
              'michelle.CmdWarpToStuff("item", %s, minRange=0.0), target %s m'
              ' away' % (text(targetID), fmt(target_distance, 0)))

        warp_mode = table.get(3)
        in_warp, waited = wait_for(
            lambda: getattr_safe(ball, 'mode') == 3, timeout_ms=15000)
        check('the ego ball entered DSTBALL_WARP', bool(in_warp),
              'ball.mode = %s after %d ms, expected 3 (DSTBALL_WARP%s)'
              % (mode_name(getattr_safe(ball, 'mode'), table), waited,
                 '' if warp_mode == 'DSTBALL_WARP' else
                 ', which this build names %s' % text(warp_mode)))

        # ** The check `docs/port-tracker.md` section 34 exists for. **
        warp_fraction = getattr_safe(ball, 'speedFraction')
        check('the warp did not shut the throttle',
              (warp_fraction or 0) > 0,
              'ball.speedFraction = %s while ball.mode = %s.  ZERO HERE IS'
              ' THE ALIGNING-FOREVER DEFECT: the sub-warp goto integrator'
              ' multiplies maxVelocity by speedFraction'
              ' (_destiny.dll 0x180038907), so a zero throttle commands a'
              ' speed of zero, the speed gate speed^2 > (0.75 *'
              ' maxVelocity)^2 can never pass, a zero velocity does not'
              ' normalise so the direction gate cannot pass either, and the'
              ' ship aligns until the 180-tick watchdog forces the warp'
              % (text(warp_fraction),
                 mode_name(getattr_safe(ball, 'mode'), table)))
        check('the warp throttle clears the client\'s align gate',
              (warp_fraction or 0) > ALIGN_GATE,
              'ball.speedFraction = %s, the gate needs strictly more than'
              ' %s of maxVelocity and a goto only approaches its commanded'
              ' speed from below -- so %s is the only value that converges'
              ' past it (docs/port-tracker.md s34.3)'
              % (text(warp_fraction), fmt(ALIGN_GATE, 2), fmt(1.0, 1)))
        check('the warp throttle is wide open',
              warp_fraction is not None and abs(float(warp_fraction) - 1.0)
              < 0.001,
              'ball.speedFraction = %s, expected exactly 1.0.  In the'
              ' client\'s arithmetic warping IS full throttle; "warp at half'
              ' speed" is not expressible' % text(warp_fraction))

        # **`effectStamp < 0`, and the sign is the whole point.** This check
        # asserted `> 0` and cited `michelle.IsPreparingWarp` as [V] for it.
        # The disassembly says otherwise -- `COMPARE_OP 0` is `<`, not `>`:
        #
        #     LOAD_ATTR effectStamp / LOAD_CONST 0 / COMPARE_OP 0  ('<')
        #
        # so `IsPreparingWarp` is `ball and mode == DSTBALL_WARP and
        # effectStamp < 0` (`michelle.pyc:575-580`) [V]. A NEGATIVE stamp is
        # what "preparing to warp" looks like, and `spaceMgr._GetWarpProgress`
        # (`spaceMgr.pyc:839`) says what the magnitude means [V]:
        #
        #     MAX_ALIGN_TICKS = 180.0
        #     autoWarpProgress = min(-ball.effectStamp / MAX_ALIGN_TICKS, 1.0)
        #
        # `-effectStamp` is align ticks elapsed, out of 180 -- the same
        # watchdog as `cmp eax, 0xb4` in `_destiny.dll`'s AlignForWarp,
        # reached from a different artefact entirely. So the observed -2 was
        # two ticks into a healthy align, and this check failed a correct
        # server on a comparison we had written down backwards.
        stamp = getattr_safe(ball, 'effectStamp')
        check('the warp is in preparation (effectStamp < 0)',
              stamp is not None and stamp < 0,
              'ball.effectStamp = %s, and -effectStamp/180.0 is the align'
              ' progress the client draws. michelle.IsPreparingWarp is'
              ' `mode == DSTBALL_WARP and effectStamp < 0`'
              ' (michelle.pyc:575-580) [V], so 0 here means the client does'
              ' not consider a warp to be in preparation at all'
              % text(stamp))

        # -- and then: did it actually move?  This is the end-to-end answer,
        # and it is the one that does not depend on reading any one field
        # correctly.  A ship aligning forever sits still.
        peak = sub_warp_speed or 0.0
        travelled = 0.0
        elapsed = 0
        while elapsed < WARP_SETTLE_MS:
            sleep_ms(1000)
            elapsed += 1000
            current = speed_of(ball)
            if current is not None and current > peak:
                peak = current
            try:
                travelled = math.sqrt(
                    (ball.x - before[0]) ** 2 + (ball.y - before[1]) ** 2
                    + (ball.z - before[2]) ** 2)
            except Exception:
                pass
            if travelled >= MIN_WARP_DISTANCE:
                break

        check('the ship moved under the warp',
              travelled >= MIN_WARP_DISTANCE,
              'travelled %s m in %d ms after the warp command, expected at'
              ' least %s m; peak speed seen %s m/s against a maxVelocity of'
              ' %s m/s; ball.mode is %s and ball.speedFraction is %s.  A ship'
              ' that aligns forever travels ~0 m here'
              % (fmt(travelled, 1), elapsed, fmt(MIN_WARP_DISTANCE, 0),
                 fmt(peak, 1), fmt(max_velocity),
                 mode_name(getattr_safe(ball, 'mode'), table),
                 text(getattr_safe(ball, 'speedFraction'))))
        check('the peak speed cleared the align gate',
              peak > ALIGN_GATE * max_velocity,
              'peak speed %s m/s, gate 0.75 * maxVelocity = %s m/s.  This is'
              ' the client\'s own second align condition, evaluated on the'
              ' client\'s own numbers'
              % (fmt(peak, 1), fmt(ALIGN_GATE * max_velocity, 1)))

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
