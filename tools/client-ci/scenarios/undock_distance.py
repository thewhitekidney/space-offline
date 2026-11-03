# -*- coding: utf-8 -*-
"""How far the ego ball is from the station it just undocked from.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  See `docs/lanes/ci-scenarios.md`.

The defect this exists for: the ship undocked at the station's *centre*
instead of at its mouth.  A person had to notice that the station was drawn
around them.  The number that says it outright is
`ballpark.GetSurfaceDist(stationID, shipID)`, and its **sign** is the whole
diagnosis: negative means the ship is inside the hull.

Three layers of assertion, deliberately separated by how well each is
verified:

  1. the surface and centre distances, straight off the native ballpark --
     `[V]`, and enough on its own to catch the defect;
  2. the station is still within docking range -- `[V]`, a different defect
     (flung too far to dock again);
  3. the distance to the nearest docking mouth -- `[I]`, because
     `ballparkCommon.docking.GetUndockVector` is a server-side entry point
     and its body has to be reproduced here.
"""

EOCI_SCENARIO = 'undock_distance'

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

    #: Station item ids, `idCheckers.IsStation`:
    #: `minStation <= itemID <= maxStation` (`eve/common/script/sys/
    #: idCheckers.pyc:36`, bounds at `inventorycommon/const.pyc:63` and `:66`)
    MIN_STATION = 60000000
    MAX_STATION = 63999999

    #: `const.maxDockingDistance` (`eve/common/lib/appConst.pyc:1728`).
    MAX_DOCKING_DISTANCE = 50000.0

    #: How near a docking mouth a ship that has just undocked should be.
    #: [A] -- the server jitters the exit direction and the ship keeps moving
    #: after the undock, so this is a "near the mouth, not at the centre"
    #: threshold rather than a position assertion.  Falsified if the harness
    #: ever lets the ship fly for more than a few seconds before this runs;
    #: the sign check on the surface distance below is the one that does not
    #: depend on timing at all.
    MOUTH_TOLERANCE = 5000.0

    def station_balls(park):
        out = []
        try:
            for ballID in park.balls.keys():
                if MIN_STATION <= ballID <= MAX_STATION:
                    out.append(ballID)
        except Exception:
            pass
        return sorted(out)

    def mouth_positions(park, stationID, station_ball, ship_type_id):
        """Every docking mouth of this station, in world coordinates.

        `ballparkCommon.docking.GetUndockVector(ballpark, ballID, typeID)`
        (`ballparkCommon/docking.pyc:148`) is the client's own code for this
        and it is **not callable from in here**: it needs `GetBallType`,
        `GetSlimItemField` and `CheckBalls`, none of which is in
        `_destiny.dll`'s `Ballpark` method table -- it is a server-side entry
        point.  So its body is reproduced here from its own source, which is
        a translation by the station ball's position plus an optional
        `dunRotation`:

            position, direction = GetUndockDirection(ballTypeID, typeID)
            rotation = ballpark.GetSlimItemField(ballID, 'dunRotation')
            if rotation: ... QuaternionRotationSetYawPitchRoll ...
            position = [ball.x + position[0], ...]

        `[I]`, inferred from that function's own body rather than read back
        from the client -- which is why the checks that rest on it are kept
        separate from the ones that only use `GetSurfaceDist`.

        `GetUndockDirections(typeID)` returns
        `{category: [((px,py,pz), (dx,dy,dz)), ...], None: [all of them]}`
        and `{}` for an unknown type (`docking.pyc:96-117`) [V].  The `None`
        key is used rather than `GetUndockDirection`, because that one picks
        with `random.choice` (`:127`) and a scenario should not be comparing
        against a coin flip.
        """
        import math
        try:
            import ballparkCommon.docking as docking
        except Exception:
            return None, 'ballparkCommon.docking would not import'
        station_type = None
        try:
            station_type = park.GetInvItem(stationID).typeID
        except Exception:
            pass
        if not station_type:
            return None, 'no slimItem/typeID for station ball %s' \
                % text(stationID)
        try:
            directions = docking.GetUndockDirections(station_type)
        except Exception:
            return None, 'GetUndockDirections(%s) raised' % text(station_type)
        locations = []
        try:
            locations = list(directions.get(None, []))
        except Exception:
            locations = []
        if not locations:
            return None, ('GetUndockDirections(%s) knows no mouths for this'
                          ' station type; its graphicID has no directional'
                          ' locators' % text(station_type))

        rotation = None
        try:
            rotation = park.GetInvItem(stationID).dunRotation
        except Exception:
            rotation = None

        quaternion = None
        if rotation:
            try:
                import geo2
                quaternion = geo2.QuaternionRotationSetYawPitchRoll(
                    *[math.radians(a) for a in rotation])
            except Exception:
                quaternion = None

        out = []
        for entry in locations:
            try:
                position = entry[0]
                if quaternion is not None:
                    import geo2
                    position = geo2.QuaternionTransformVector(quaternion,
                                                              position)
                out.append((station_ball.x + position[0],
                            station_ball.y + position[1],
                            station_ball.z + position[2]))
            except Exception:
                continue
        if not out:
            return None, 'none of the %d mouth locators could be placed' \
                % len(locations)
        note = '%d mouth(s) for station type %s, dunRotation %s' \
            % (len(out), text(station_type), text(rotation))
        if ship_type_id:
            try:
                category = docking.GetUndockCategoryByType(ship_type_id)
                note += ', this ship\'s undock category %s' % text(category)
            except Exception:
                pass
        return out, note

    def distance(a, b):
        import math
        return math.sqrt((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2
                         + (a[2] - b[2]) ** 2)

    def eoci_body():
        # **Undock first.** `/autoSelectCharacter:0` lands in the station,
        # and there is no ballpark in a hangar -- the first unattended run
        # failed on exactly that, with five checks green and
        # `session.stationid = 60012526`. The prelude's `undock` has the
        # evidence that this does not prompt.
        # The station we leave, noted before undocking clears it: the park
        # now holds more than one station (a neighbour in the same system),
        # so "the" station is the one we came out of, not the only one.
        home_station = getattr_safe(builtin('session'), 'stationid')
        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)
        session = builtin('session')
        if session is None:
            check('session is reachable', False, 'session = None')
            return
        shipid = getattr_safe(session, 'shipid')

        park, ball = get_ego_ball()
        check('the ego ball is in the park', ball is not None,
              'ballpark.GetBall(ballpark.ego) = %s' % text(ball))
        if park is None or ball is None:
            return

        # **Is the y-only STOP damping on in this build?**
        #
        # Carbon's `Ballpark::EvolveStop` is three lines: it multiplies the
        # *y* component of the new velocity by `(1 - 0.07) / 1.07`, about
        # 0.869, once per evolve step -- and it is gated on
        # `!g_useDynamicalOrientation`, whose compiled-in default is `false`,
        # so the damping runs unless something turns the flag on
        # (`docs/port-tracker.md` 51.2, 51.3a).
        #
        # That matters here and nowhere else, because an undocked ship sits
        # in STOP with real velocity. Our server decays all three axes with
        # one time constant; if the client damps y as well, the two drift
        # apart in the y axis only -- by about 180 m after two seconds and
        # 400 m after five for a frigate, and over a kilometre for a cruiser
        # (51.4). That is not a rounding difference, it is the ship not being
        # where the client draws it.
        #
        # We are not measuring the decay here: the client simply *exposes the
        # setting*, so one read settles it. `destiny._util`'s
        # `is_dynamical_orientation_enabled` is
        # `destiny.settings.Get().useDynamicalOrientation` **[V]**, so that
        # is what is read, with the helper preferred when it exists.
        #
        # **The check passes either way, deliberately.** We do not yet know
        # which value is correct for us -- the point is to learn it. A
        # failure here means the setting could not be read at all, which is
        # the only outcome that would be a defect in this check.
        try:
            import destiny as _destiny
            _util = getattr(_destiny, '_util', None)
            _reader = getattr(_util, 'is_dynamical_orientation_enabled', None)
            if _reader is not None:
                _dyn = _reader()
                _how = 'destiny._util.is_dynamical_orientation_enabled()'
            else:
                _dyn = _destiny.settings.Get().useDynamicalOrientation
                _how = 'destiny.settings.Get().useDynamicalOrientation'
            check('the dynamical-orientation setting is readable', True,
                  '%s = %s.  The y-only STOP damping runs when this is'
                  ' FALSE, so False means our Mode::Stop diverges from the'
                  ' client in the y axis after an undock and True means it'
                  ' does not.  See docs/port-tracker.md 51.2 and 51.3a'
                  % (_how, text(_dyn)))
        except Exception as exc:
            check('the dynamical-orientation setting is readable', False,
                  'could not read it: %s.  Without it, whether the y-only'
                  ' STOP damping applies to this build stays unknown'
                  % text(exc))

        # **The ballpark's own simulation constants, read off the live
        # object.**
        #
        # Carbon maps four of its `Ballpark` members into Python as
        # READWRITE attributes (`Ballpark_Blue.cpp`), so the client will
        # simply tell us what it is running with. Each of these is a number
        # we have so far taken from the C++ constructor
        # (`Ballpark.cpp:86-118`), which is the default and not necessarily
        # the value in play:
        #
        #   tickInterval  `dt = mTickInterval * 0.001`, so this sets the step
        #                 every per-step law is quoted against -- including
        #                 the y damping just above, whose 0.869 is *per
        #                 step*. The constructor says 1000 ms, i.e. dt = 1 s,
        #                 which is ten times longer than the 100 ms this
        #                 project had been assuming informally. If that is
        #                 right the damping's time constant is 7.1 s, not
        #                 0.71 s, and the divergence numbers in 51.4 are an
        #                 order of magnitude out.
        #   currentTime   the counter the **orbital plane** is a function of
        #                 (`docs/port-tracker.md` 54.3). We derived the plane
        #                 from `mCurrentTime` and the ball id but could not
        #                 wire it in, because `mCurrentTime` is incremented
        #                 once per evolve and our own tick is Unix-epoch
        #                 seconds -- same rate only if tickInterval is 1000.
        #                 Reported next to the client's wall clock so the two
        #                 can be compared directly: if they agree, the phase
        #                 question is answered and the plane can be wired in.
        #   friction      1000000.0 in the constructor, and `tau = mass *
        #                 agility / friction` is our whole integrator (51.1).
        #   warpSpeed     2.0 AU/s in the constructor.
        #
        # **All four pass whatever they read**, like the setting above: the
        # only failure is not being able to read them at all. Writing an
        # expected value in here would turn a measurement into an assumption,
        # which is the one thing this directory is not for.
        for _attr, _why in (
                ('tickInterval',
                 'dt = tickInterval/1000 is the step every per-step law is'
                 ' quoted against, the y damping above included'),
                ('currentTime',
                 'the counter the orbital plane is a function of; see'
                 ' docs/port-tracker.md 54.3'),
                ('friction',
                 'tau = mass*agility/friction, our entire integrator'),
                ('warpSpeed', 'AU/s, the warp curve ceiling'),
        ):
            try:
                _val = getattr(park, _attr)
                check('the ballpark reports its %s' % _attr, True,
                      'ballpark.%s = %s -- %s' % (_attr, text(_val), _why))
            except Exception as exc:
                check('the ballpark reports its %s' % _attr, False,
                      'could not read ballpark.%s: %s' % (_attr, text(exc)))

        # The client's wall clock, for comparing against `currentTime` above.
        # Two numbers in one place beat two numbers in two logs.
        try:
            import time as _time
            _now = _time.time()
            _ct = getattr(park, 'currentTime', None)
            _delta = ('n/a' if _ct is None
                      else text(float(_now) - float(_ct)))
            check('the ballpark clock can be compared to the wall clock', True,
                  'time.time() = %s, ballpark.currentTime = %s, difference ='
                  ' %s.  A difference near zero means the ballpark counts'
                  ' Unix seconds and our destiny::tick() is the same clock,'
                  ' which would settle the orbital-plane phase of'
                  ' docs/port-tracker.md 54.3.  A large or drifting'
                  ' difference means it counts evolves from its own origin'
                  ' and the plane cannot be wired in from the tick alone'
                  % (text(_now), text(_ct), _delta))
        except Exception as exc:
            check('the ballpark clock can be compared to the wall clock',
                  False, 'could not compare them: %s' % text(exc))

        # Undocking clears `session.stationid` (`eveCfg.IsDocked` is
        # `bool(session.stationid)`, `eveCfg.pyc:1929-1933`), so the station is
        # found in the park by id range rather than from the session.
        stations = station_balls(park)
        check('the station we undocked from is in the park',
              home_station in stations,
              'home station %s; %d station ball(s) in the park: %s (of %d'
              ' balls total).  The station has to be in the park for the'
              ' client to draw it, and for this scenario to have anything to'
              ' measure against.  Other stations in the system are fine'
              % (text(home_station), len(stations), text(stations),
                 len(getattr_safe(park, 'balls') or {})))
        if home_station not in stations:
            return
        stationID = home_station

        station_ball = None
        try:
            station_ball = park.GetBall(stationID)
        except Exception:
            station_ball = None
        if station_ball is None:
            check('the station ball is readable', False,
                  'ballpark.GetBall(%s) = None' % text(stationID))
            return

        radius = getattr_safe(station_ball, 'radius') or 0.0
        check('the station ball has a positive radius', radius > 0,
              'station %s radius = %s m.  Zero means the entity we sent'
              ' carried no radius, and every distance below is then measured'
              ' against a point' % (text(stationID), fmt(radius, 1)))

        # -- the defect, in one number.  `GetSurfaceDist` is the native
        # ballpark primitive, "Returns distance between ball surfaces (can be
        # negative)"; `Park.DistanceBetween` (`michelle.pyc:1320-1329`) is the
        # same call with the negatives clamped to 0.0 and a RuntimeError on
        # invalid balls -- which is why the raw one is used here: the sign is
        # the whole diagnosis.
        surface = None
        centre = None
        try:
            surface = park.GetSurfaceDist(stationID, shipid)
        except Exception:
            pass
        try:
            centre = park.GetCenterDist(stationID, shipid)
        except Exception:
            pass

        check('the surface distance to the station is readable',
              surface is not None,
              'ballpark.GetSurfaceDist(%s, %s) = %s'
              % (text(stationID), text(shipid), text(surface)))
        check('the ego ball is outside the station hull',
              surface is not None and surface > 0.0,
              'surface distance = %s m, centre distance = %s m, station'
              ' radius = %s m.  A negative surface distance means the ship'
              ' was placed INSIDE the station -- which is what undocking at'
              ' the station centre instead of its mouth looks like, measured'
              % (fmt(surface, 1), fmt(centre, 1), fmt(radius, 1)))
        check('the centre distance is at least the station radius',
              centre is not None and centre >= radius,
              'centre distance = %s m, station radius = %s m'
              % (fmt(centre, 1), fmt(radius, 1)))
        check('the ego ball is still within docking range',
              surface is not None and surface < MAX_DOCKING_DISTANCE,
              'surface distance = %s m, const.maxDockingDistance = %s m.'
              ' Further than this and the ship cannot dock again, which is a'
              ' different defect from undocking too close'
              % (fmt(surface, 1), fmt(MAX_DOCKING_DISTANCE, 0)))

        # -- the mouth.  [I], reproduced from `GetUndockVector`; see
        # `mouth_positions`.
        ship_type_id = None
        try:
            ship_type_id = park.GetInvItem(shipid).typeID
        except Exception:
            pass
        mouths, note = mouth_positions(park, stationID, station_ball,
                                       ship_type_id)
        ego_pos = (getattr_safe(ball, 'x'), getattr_safe(ball, 'y'),
                   getattr_safe(ball, 'z'))
        station_pos = (station_ball.x, station_ball.y, station_ball.z)
        to_centre = None
        try:
            to_centre = distance(ego_pos, station_pos)
        except Exception:
            pass

        if mouths is None:
            check('the station\'s docking mouths could be located', False,
                  '%s.  The surface-distance checks above still stand; this'
                  ' one needs ballparkCommon.docking and the station type\'s'
                  ' graphic locators' % note)
            return

        best = None
        try:
            best = min([distance(ego_pos, m) for m in mouths])
        except Exception:
            best = None
        check('the distance to the nearest docking mouth is computable',
              best is not None, note)
        if best is None:
            return

        check('the ego ball is nearer a docking mouth than the station centre',
              to_centre is not None and best < to_centre,
              'nearest mouth %s m away, station centre %s m away.  The'
              ' undocked-at-the-centre defect reads the other way round'
              % (fmt(best, 1), fmt(to_centre, 1)))
        check('the ego ball is at a docking mouth', best <= MOUTH_TOLERANCE,
              'expected the ego ball within %s m of a station mouth, got'
              ' %s m (ego at %s, %s, %s; station at %s, %s, %s; %s)'
              % (fmt(MOUTH_TOLERANCE, 0), fmt(best, 1),
                 fmt(ego_pos[0]), fmt(ego_pos[1]), fmt(ego_pos[2]),
                 fmt(station_pos[0]), fmt(station_pos[1]), fmt(station_pos[2]),
                 note))

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
