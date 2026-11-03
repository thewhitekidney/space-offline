# -*- coding: utf-8 -*-
"""The HUD's shield / armour / structure portions, as godma computed them.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  See `docs/lanes/ci-scenarios.md`.

Two of the five most expensive defects this project shipped to a human were
this number: the gauges were *hidden* once (the `damageState` we sent was
empty) and *red* once (shield health landed in a slot nothing reads).  Both
times a person spent a session finding out what
`ActiveShipController.GetShieldHPPortion()` would have said in one line.

It reads the HUD's own accessors, not a reimplementation of them, so the
arithmetic -- the division, the `round(..., 2)`, the clamp and the
`ZeroDivisionError -> 0.0` -- is the client's and not ours.  It then
cross-checks against `baseDogmaLocation.GetDamageStateEx`, which is the
*other*, independent client-side producer of the same three fractions.
"""

EOCI_SCENARIO = 'hud_health'

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

    #: A freshly undocked, undamaged ship.  The suite runs on a clean login,
    #: so every hull layer should read full.  [A] -- falsified by any scenario
    #: that deliberately damages the ship before this one runs, or by a server
    #: that persists damage across sessions; drop the "at full" checks then and
    #: keep the "not zero" ones, which hold regardless.
    FULL = 0.99

    def get_controller():
        """The HUD's own `ActiveShipController`, live instance preferred.

        `ShipUI.ApplyAttributes` stores one as `self.controller`
        (`eve/client/script/ui/inflight/shipHud/shipUI.pyc:100`) and the HUD
        layer is `uicore.layer.shipui`
        (`eve/client/script/ui/inflight/shipAlert.pyc:349`).  That instance is
        the one the gauges actually read, so it is preferred.

        Constructing a fresh one is the documented fallback because the client
        itself does exactly that in two unrelated subsystems --
        `nodegraph/client/conditions/ship.pyc:176-177` and
        `dotWeapons/client/dotClientUtil.pyc:131-132`, both
        `from eve.client.script.ui.inflight.shipHud import
        ActiveShipController` then `ActiveShipController()`.  The only side
        effect is `sm.RegisterNotify(self)` in `__init__`
        (`activeShipController.pyc:28`).
        """
        try:
            from carbonui.uicore import uicore
        except Exception:
            uicore = None
        if uicore is not None:
            try:
                live = uicore.layer.shipui.controller
                if live is not None:
                    return live, 'uicore.layer.shipui.controller (live HUD)'
            except Exception:
                pass
        try:
            from eve.client.script.ui.inflight.shipHud \
                import ActiveShipController
            return ActiveShipController(), 'ActiveShipController() (fresh)'
        except Exception:
            pass
        return None, 'neither the live HUD controller nor a fresh one'

    def eoci_body():
        # **Undock first.** `/autoSelectCharacter:0` lands in the station,
        # and there is no ballpark in a hangar -- the first unattended run
        # failed on exactly that, with five checks green and
        # `session.stationid = 60012526`. The prelude's `undock` has the
        # evidence that this does not prompt.
        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)
        sm = builtin('sm')
        session = builtin('session')
        if sm is None or session is None:
            check('sm and session are reachable', False,
                  'sm = %s, session = %s' % (text(sm), text(session)))
            return
        shipid = getattr_safe(session, 'shipid')
        check('session.shipid is set', bool(shipid),
              'session.shipid = %s' % text(shipid))
        if not shipid:
            return

        # The ballpark has to exist before `IsLoaded` can ever be true:
        # `ActiveShipController.IsLoaded` requires `GetBall()`
        # (`activeShipController.pyc:85`).
        get_ballpark()

        controller, route = get_controller()
        check('the HUD ship controller is reachable', controller is not None,
              route)
        if controller is None:
            return

        godma = None
        try:
            godma = sm.GetService('godma')
        except Exception:
            godma = None
        check('the godma service resolves', godma is not None,
              'sm.GetService("godma") = %s' % text(godma))
        if godma is None:
            return

        ready, waited = wait_for(lambda: godma.IsItemReady(shipid),
                                 timeout_ms=30000)
        check('godma reports the ship item ready', bool(ready),
              'godma.IsItemReady(%s) after %d ms -- False means the server'
              ' never delivered the ship\'s dogma item at all'
              % (text(shipid), waited))

        loaded, waited = wait_for(lambda: controller.IsLoaded(),
                                  timeout_ms=30000)
        check('the HUD controller reports the ship loaded', bool(loaded),
              'controller.IsLoaded() after %d ms -- this is the exact gate'
              ' both HUD update loops sit behind (shipUI.pyc:1346,'
              ' capacitorContainer.pyc:168), so False here means the gauges'
              ' never update at all' % waited)

        item = None
        try:
            item = godma.GetItem(shipid)
        except Exception:
            item = None
        check('godma has a dogma item for the ship', item is not None,
              'godma.GetItem(%s) = %s' % (text(shipid), text(item)))

        # The six raw attributes the three gauges are computed from.
        raw = {}
        for attr in ('shieldCharge', 'shieldCapacity',
                     'armorHP', 'armorDamage', 'hp', 'damage'):
            raw[attr] = getattr_safe(item, attr)
        missing = [a for a in sorted(raw) if raw[a] is None]
        check('all six hull dogma attributes are readable', not missing,
              'shieldCharge=%s shieldCapacity=%s armorHP=%s armorDamage=%s'
              ' hp=%s damage=%s%s'
              % (text(raw['shieldCharge']), text(raw['shieldCapacity']),
                 text(raw['armorHP']), text(raw['armorDamage']),
                 text(raw['hp']), text(raw['damage']),
                 '' if not missing else '  -- MISSING: %s' % text(missing)))

        # -- the maxima.  Zero here is the "gauges hidden" defect: every
        # portion accessor divides by its max and swallows ZeroDivisionError
        # into 0.0 (activeShipController.pyc:105/117/129).
        layers = (
            ('shield', 'GetShieldHP', 'GetShieldHPMax', 'GetShieldHPPortion',
             'shieldCapacity (attributeID 263)'),
            ('armour', 'GetArmorHP', 'GetArmorHPMax', 'GetArmorHPPortion',
             'armorHP (attributeID 265)'),
            ('structure', 'GetStructureHP', 'GetStructureHPMax',
             'GetStructureHPPortion', 'hp (attributeID 9)'),
        )
        for (label, hp_name, max_name, portion_name, source) in layers:
            hp = None
            hpmax = None
            portion = None
            try:
                hp = getattr(controller, hp_name)()
            except Exception:
                pass
            try:
                hpmax = getattr(controller, max_name)()
            except Exception:
                pass
            try:
                portion = getattr(controller, portion_name)()
            except Exception:
                pass

            check('%s max HP is positive' % label,
                  (hpmax or 0) > 0,
                  'controller.%s() = %s, from %s.  Zero makes the portion'
                  ' accessor divide by zero and return 0.0, which is the'
                  ' "gauges hidden" defect exactly'
                  % (max_name, text(hpmax), source))
            check('%s portion is readable' % label, portion is not None,
                  'controller.%s() = %s' % (portion_name, text(portion)))
            if portion is None:
                continue
            check('%s portion is not zero' % label, portion > 0.0,
                  'controller.%s() = %s (%s / %s).  Zero is what the player'
                  ' sees as an empty or red gauge'
                  % (portion_name, fmt(portion, 3), text(hp), text(hpmax)))
            check('%s portion reads full on a freshly undocked ship' % label,
                  portion >= FULL,
                  'controller.%s() = %s, expected >= %s (%s / %s)'
                  % (portion_name, fmt(portion, 3), fmt(FULL, 2),
                     text(hp), text(hpmax)))

        # -- the independent cross-check.  `GetDamageStateEx` is the shared
        # client/server dogma implementation of the same three fractions
        # (`eve/common/script/dogma/baseDogmaLocation.pyc:1726`), reached via
        # `clientDogmaIM.GetDogmaLocation()` (`clientDogmaIM.pyc:27`).  It
        # divides in a completely different place, so agreement between the
        # two is strong evidence the numbers are real and not an artefact.
        state = None
        try:
            location = sm.GetService('clientDogmaIM').GetDogmaLocation()
            state = location.GetDamageStateEx(shipid)
        except Exception:
            state = None
        check('the dogma location reports a damage state',
              state is not None,
              'clientDogmaIM.GetDogmaLocation().GetDamageStateEx(%s) = %s --'
              ' None means the item is in failedLoadingItems or absent from'
              ' dogmaItems (baseDogmaLocation.pyc:1744/1748)'
              % (text(shipid), text(state)))
        if state is not None:
            # `ret[0]` is a `(fraction, tau, simTime)` triple when the shield
            # capacity is positive (`baseDogmaLocation.pyc:1764`) and the bare
            # charge when it is zero (`:1766`).  A scalar here is therefore a
            # direct report that `shieldCapacity` never arrived.
            shield_is_triple = isinstance(state[0], (list, tuple))
            check('the dogma shield entry is a (fraction, tau, time) triple',
                  shield_is_triple,
                  'GetDamageStateEx()[0] = %s.  A bare number is what'
                  ' baseDogmaLocation.pyc:1766 appends when shieldCapacity is'
                  ' 0 -- which is the hidden-gauge defect, reported by the'
                  ' dogma layer itself' % text(state[0]))
            try:
                shield_ex = state[0][0] if shield_is_triple else None
                armor_ex = state[1]
                struct_ex = state[2]
            except Exception:
                shield_ex = armor_ex = struct_ex = None
            pairs = (
                ('shield', shield_ex, 'GetShieldHPPortion'),
                ('armour', armor_ex, 'GetArmorHPPortion'),
                ('structure', struct_ex, 'GetStructureHPPortion'),
            )
            for (label, ex_value, portion_name) in pairs:
                hud_value = None
                try:
                    hud_value = getattr(controller, portion_name)()
                except Exception:
                    pass
                agree = False
                try:
                    agree = abs(float(ex_value) - float(hud_value)) <= 0.02
                except Exception:
                    agree = False
                check('%s agrees between the HUD and the dogma location'
                      % label, agree,
                      'GetDamageStateEx -> %s, controller.%s() -> %s'
                      ' (tolerance 0.02, the HUD rounds to 2 decimals)'
                      % (text(ex_value), portion_name, text(hud_value)))

        # -- what was actually drawn.  `HPGauge.value` returns `_lastValue`
        # (`shipHud/hpGauges.pyc:148`), set in `UpdateValue`
        # (`:162`) and `None` until the first update.  This is the only check
        # in the suite that needs the in-flight UI to exist; see
        # `docs/lanes/ci-scenarios.md` section 4.
        # **Waited for, not read once.**  `UpdateValue` runs on a frame, so
        # whether the gauges have drawn by the time this line executes is a
        # race -- and a bare read is a check that reports the outcome of that
        # race as if it were the server's answer.  This check read them once
        # and passed 28/28 in the morning, then failed all three gauges in the
        # afternoon on a build whose only relevant change was that every
        # timing number in the suite shifted (`wait_for` began measuring
        # elapsed time instead of counting its own sleeps, 5ebb63e).  Nothing
        # in the server's damage-state path had changed; the check simply lost
        # a race it had been winning.
        #
        # So: poll until all three are non-None, with the elapsed time in the
        # detail.  A timeout now means the gauges really never drew, which is
        # the defect this check was written for ("the gauges were *hidden*
        # once", see the file header), and the wait makes that distinguishable
        # from having asked too early.
        def read_gauges():
            try:
                from carbonui.uicore import uicore
                gauges = uicore.layer.shipui.hpGauges
            except Exception:
                return None
            try:
                found = {
                    'shield': gauges.shieldGauge.value,
                    'armour': gauges.armorGauge.value,
                    'structure': gauges.structureGauge.value,
                }
            except Exception:
                return None
            # Truthy only once every gauge has a value, so `wait_for` keeps
            # polling while any of them is still blank.
            if any(v is None for v in found.values()):
                return None
            return found

        drawn, drawn_ms = wait_for(read_gauges, timeout_ms=30000, step_ms=500)
        if drawn:
            say('hpGauges drew all three after %s ms' % text(drawn_ms))
        if not drawn:
            # Distinguish "unreachable" from "reachable but blank", because
            # they are different diagnoses and only one of them is ours.
            try:
                from carbonui.uicore import uicore
                gauges = uicore.layer.shipui.hpGauges
                drawn = {
                    'shield': gauges.shieldGauge.value,
                    'armour': gauges.armorGauge.value,
                    'structure': gauges.structureGauge.value,
                }
            except Exception:
                drawn = {}
        if not drawn:
            check('the HUD gauges have drawn a value', False,
                  'uicore.layer.shipui.hpGauges is not reachable -- this'
                  ' scenario cannot judge what was drawn.  The godma checks'
                  ' above still stand; drop this check if the harness runs'
                  ' the client without the in-flight UI')
        else:
            for label in ('shield', 'armour', 'structure'):
                value = drawn.get(label)
                check('the %s gauge has drawn a value' % label,
                      value is not None,
                      'hpGauges.%s.value = %s after waiting %s ms -- None'
                      ' means UpdateValue was never called in that time, i.e.'
                      ' the gauge is blank on screen'
                      % (label, text(value), text(drawn_ms)))

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
