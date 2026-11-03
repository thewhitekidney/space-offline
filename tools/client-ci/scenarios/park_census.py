# -*- coding: utf-8 -*-
"""Which balls are in the park, and whose ships they are.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  See `docs/lanes/ci-scenarios.md`.

The two-in-space oracle, and the cheapest possible answer to "did the other
player's ship actually arrive".  It also catches a quieter defect class: a
ball with no `slimItem`.  `Park.GetBallsAndItems` silently skips those
(`michelle.pyc:1879`), so such a ball exists in the simulation, moves, and is
invisible to the overview and the brackets -- which is a very expensive thing
to debug by eye and a one-line set difference to detect.

**It undocks on its first line, and must.**  There is no ballpark in a hangar:
`michelle.DoDestinyUpdate` returns at `michelle.py:632-633` while
`self.__bp is None` [V], so a docked client cannot even *receive* another
ship's ball, let alone read one.  This is the opposite precondition from
`lobby_guests`, which must NOT undock because
`Registry::characters_at_station` filters pilots in space out of the very list
it reads (`service.rs:1725-1734`).  The two cannot be merged --
`docs/lanes/ci-scenarios.md` section 10.6.

**Two clients need a rendezvous, and that is what the second half of this file
is about.**  A scenario writes its verdict and then terminates the client,
which is right for one client and was wrong for two: on 2026-10-23 both
two-player verdicts of *this* scenario were invalid because the clients'
presence barely overlapped (`docs/lanes/ci-harness.md` section 11).  The first
client undocked, censused an empty sky and quit before the second had finished
booting; both failed symmetrically, and that symmetric red was written into
`docs/archive/roadmap.md` as "arrivals are not broadcast" and had to be withdrawn.
The server was fine.  `lobby_guests` got the fix first (section 11 of
`ci-scenarios.md`) and a real two-client run came back 9/9 from both sides;
this is the same fix in the harder place.

**Harder, because a ballpark is a subscription and not a call.**  There is no
`GetGuests` here and no cache stamp to clear: the park arrives as pushed
`DoDestinyUpdate` batches into `michelle`'s own state and this scenario
*reads* `ballpark`.  A fresh view therefore cannot be fetched, and waiting is
not a weaker option but the only sound one:

1. **Nothing the observer can call returns the park.**  The 30 methods the
   whole client corpus ever calls on the ballpark bound object
   (`findings/client-calls.tsv`, `service == beyonce`) are `Cmd*`, the
   bookmark and component calls, eight unrelated `Get*`s -- and
   `UpdateStateRequest`, which has exactly one call site in the corpus,
   `michelle.py:1072` [V].
2. **`UpdateStateRequest` is a desync recovery, not a refresh**, and calling
   it to bust a view would be the `CleanUp` mistake of `lobby_guests` with a
   worse blast radius.  Its only caller is `Park.RequestReset`
   (`michelle.py:1074`, reached only from `RealFlushState:1569`'s failed
   `SynchroniseToSimulationTime`) [V], and it *first* sets
   `self.validState = False` (`:1076`) -- after which `RealFlushState:1527`
   drops every batch that is not a `SetState` with one log line,
   `'Events ignored'` (`:1594`) [V].  It answers asynchronously, by push, from
   another tasklet (`uthread.pool`, `:1079`) [V], so it does not even save the
   waiting; and a `SetState` rebuilds the whole park, which would mean
   measuring `bootstrap_for` instead of the arrival we care about.
3. **An arriving ball reaches an already-bound ballpark with no action by the
   observer.**  `'DoDestinyUpdate'` is a `Michelle.__notifyevents__` entry
   (`michelle.py:151`) and the handler ends in
   `self.__bp.FlushState(state, waitForBubble)` (`:668`) -- the park the
   scenario is already holding.  `FlushState` appends to `Park.history` and
   `DoPreTick` applies it through `RealFlushState` (`:1121-1133`) on the
   client's own tick [V], so the observer's only duty is to yield, which
   `sleep_ms` does.

So the rendezvous **waits and re-reads**, and what it re-reads is the
slimItem, not the ball.  Those are two events, not one: `AddBalls2` creates
the ball natively (`:1680`) and then queues the slimItem onto
`broker.ballQueue` (`QueueBallData`, `:1292-1298`), and it is
`Michelle.ProcessDispatchOrders` -- a different tasklet -- that writes
`bp.slimItems[ballID]` at `:808` and fires `DoBallsAdded` at `:818` [V].  A
ball is therefore in `ballpark.balls` *before* it is in `ballpark.slimItems`,
which is both why the rendezvous polls `GetInvItem(ballID).charID` (the later
of the two) and why the census snapshot is taken after the rendezvous and not
before: "every ball has a slimItem" would otherwise be able to red on a ball
that was merely half-arrived.

With `EXPECT_CHARID = 0` none of it happens: a single-client run is exactly
what it was -- no rendezvous, no linger, the census reported and the ego-side
facts asserted.

Set `EXPECT_CHARID` below to the second character's id to turn the census into
an assertion about them; see `docs/lanes/ci-scenarios.md` sections 5 and 12.
"""

EOCI_SCENARIO = 'park_census'

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

    #: `inventorycommon/const.pyc`: `categoryShip = 6` at `:355`,
    #: `categoryStation = 3` at `:359`, `groupStation = 15` at `:971` [V].
    CATEGORY_SHIP = 6

    #: Set this to the second character's id, or leave it 0.  With 0 the
    #: scenario reports every other ship in the park and asserts only that
    #: the ego ship is there; with a charID it additionally asserts that that
    #: character's ship is in the park, which is the two-in-space oracle.
    #: The harness is expected to rewrite this line when it deploys the file
    #: -- `docs/lanes/ci-scenarios.md` section 5 -- because a scenario cannot
    #: know who else logged in.  The regex is `^ *EXPECT_CHARID *= *0 *$`
    #: (`tools/client-ci.sh:582`), so it must stay a single assignment,
    #: spaces-only indentation, nothing after the `0`.  Same spelling and same
    #: position as `lobby_guests.py`, on purpose.
    EXPECT_CHARID = 0

    # -----------------------------------------------------------------------
    # THE RENDEZVOUS -- the same shape as `lobby_guests.py`, deliberately.
    #
    # These four constants, `rendezvous`, `linger_report` and the two checks
    # they feed are a near-copy of that file's version, down to the names and
    # the numbers.  **That duplication is on purpose and is not finished
    # work**: two scenarios wearing the same mechanism must be recognisably
    # the same thing, and inventing a second spelling of a barrier is how two
    # two-player gates come to disagree about what a red means.  The right
    # home is the prelude -- it is already copied verbatim into every scenario
    # and already has a drift check (`docs/lanes/ci-scenarios.md` section 6)
    # -- and moving them there touches all eight files, which is a round of
    # its own.  Section 12 records it as the debt it is.  Until then: change
    # these two functions in both files or in neither.
    #
    # What is NOT shared, because the observable is not the same:
    #   * `lobby_guests` clears a cache stamp and makes a real wire call per
    #     poll; every poll here is a *local* read of `michelle`'s own state,
    #     so the polls are free and prove nothing about the wire by
    #     themselves.  See the module docstring for why that is the only
    #     sound option, and the linger check's detail for what it costs.
    #   * the presence test is a ball whose slimItem names the other
    #     character, not a key in a dict.
    # -----------------------------------------------------------------------

    #: How long to wait for the other client to finish logging in, undocking
    #: and having its ball reach us.  The harness staggers the second client
    #: (`EVE_SECOND_DELAY`, default 5 s, `docs/lanes/ci-harness.md` section
    #: 10) and that client then spends ~35 s booting and a few more undocking,
    #: so 90 s is generous and still well inside the harness's own 600 s
    #: verdict timeout (`tools/client-ci.sh:137`).  It only elapses in the
    #: failing case.  Same number as `lobby_guests`.
    OTHER_TIMEOUT_MS = 90000

    #: The rendezvous poll interval.  Unlike `lobby_guests`, a poll here is a
    #: dict read and not a wire call, so this rate is not also a request rate
    #: -- it is kept at 2000 ms anyway so that "appeared after K ms" is a
    #: number on the same scale in both scenarios' verdicts.
    OTHER_POLL_MS = 2000

    #: **How long to stay in space after the rendezvous has succeeded, before
    #: writing the verdict and quitting.  This constant is the fix for the
    #: overlap bug; the rendezvous on its own is only half of it.**
    #:
    #: It has to exceed the *skew* between the two clients reaching the
    #: rendezvous, because the earlier one quits at `its rendezvous +
    #: LINGER_MS` and the later one needs it present at `its own rendezvous`.
    #:
    #: Why 30 s, and it is now a measurement rather than an estimate.  The
    #: 2026-10-23 `lobby_guests --with-second` run that this shape comes from
    #: reported `rendezvous after 6000 ms (4 polls)` and `after 0 ms (1 poll)`
    #: with the other client still present on 15 of 15 linger polls on both
    #: sides (`docs/lanes/ci-harness.md` section 11, the "rule lifted" note).
    #: So the observed skew was ~6 s against a 30 s linger, neither client
    #: came near the 90 s cap, and 30 s is comfortable rather than marginal.
    #: It is carried over unchanged here on purpose -- but note that this
    #: scenario adds an undock to each client's path *before* the rendezvous,
    #: and an undock is retried (`undock`, above) because the dogma location
    #: does not hold the ship yet on an unattended run.  Two undocks that
    #: retry different numbers of times is a new source of skew that the lobby
    #: pair does not have, and the first real two-client run of this scenario
    #: is what sizes it: read the two rendezvous elapsed times.
    #:
    #: **It is unconditional -- both clients linger, not just the later one.**
    #: A client can see whether its *first* poll already contains the other
    #: ship, and that does tell it who undocked last.  It is the wrong
    #: question: what the earlier client needs to know is whether the other
    #: has *already made its observation*, and a ball is in the park from the
    #: moment that pilot undocks, which is tens of seconds before its own copy
    #: of this scenario reaches the rendezvous.  "They were already here" does
    #: not imply "they have already seen me".  Section 11.3 of
    #: `ci-scenarios.md` has the 2026-10-23 counterexample that settles it.
    LINGER_MS = 30000

    #: **One side holds longer, so that one side actually watches the other
    #: leave.**  With both sides lingering the same 30 s and their rendezvous
    #: at the same wall-clock moment, the two lingers end together and neither
    #: client is still looking when the other quits -- so the removal of a
    #: departed ball was never once observed end to end, on any run.
    #: `ci-scenarios.md` section 14.2 is that gap, written down after the
    #: first green two-player pair.
    #:
    #: The discriminator is `charid < EXPECT_CHARID`: deterministic, and
    #: computed from two numbers *both* sides know, so no slot or launch order
    #: has to be plumbed in.  The lower character id holds for
    #: `LINGER_MS + LINGER_EXTRA_MS` and is the watcher; the higher holds for
    #: `LINGER_MS` and quits while the watcher is still polling.
    #:
    #: 30 s of margin against ~10 s of measured startup skew (section 14.1).
    #: The watcher's poll interval is 2 s, so it sees the removal within a
    #: couple of polls of the other client's socket closing -- the server
    #: schedules it on the clock queue and the world ticker sends it within
    #: one 250 ms tick, with nobody calling anything.
    LINGER_EXTRA_MS = 30000

    #: The linger's own poll interval, same rate as the rendezvous so that
    #: "still there on N of M polls" and "appeared after K ms" are numbers on
    #: one scale.  These polls are diagnostics only: nothing is asserted about
    #: them, because the other client's ball disappearing during our linger is
    #: the *expected* ending once its own linger has run out.
    LINGER_POLL_MS = 2000

    def park_now(fallback):
        """The live ballpark, re-read cheaply, or `fallback`.

        Re-read rather than closed over for the same reason the prelude's
        `undock` re-reads `session`: `Michelle.AddBallpark` replaces
        `self.__bp` wholesale (`michelle.py:442-460`) [V], so a scenario that
        held one across a view change would be polling a dead object and
        reporting an empty sky.  Nothing in this scenario docks, so this is
        belt and braces -- and it is one attribute read per poll.
        """
        try:
            mich = get_michelle()
            park = mich.GetBallpark() if mich is not None else None
        except Exception:
            park = None
        return park if park is not None else fallback

    def other_ball(park):
        """`(ballID or None, ball count)` for a ball whose pilot is them.

        **The slimItem is the observable, not the ball**, and the two are
        separate events: `AddBalls2` makes the ball natively at
        `michelle.py:1680` and queues the slimItem onto `broker.ballQueue`
        (`:1292-1298`), and `Michelle.ProcessDispatchOrders` -- another
        tasklet -- is what writes `bp.slimItems[ballID]` (`:808`) and fires
        `DoBallsAdded` (`:818`) [V].  So `ballID in park.balls` is true
        strictly earlier than `park.GetInvItem(ballID)` answers, and a
        rendezvous that stopped at the ball would release the census while a
        ball with no slimItem was in the park -- which is a check this
        scenario deliberately fails on (`Park.GetBallsAndItems` skips such
        balls at `:1879`).

        `Park.GetInvItem(id)` is `self.slimItems.get(id, None)` (`:2121`) and
        `SlimItem.__getattr__` returns `None` for anything that was not sent
        rather than raising (`slimItem.py:44`) [V], so no read here can throw
        -- the `try` is for a park that is being torn down underneath us.

        **No category filter here, on purpose.**  "Is their pilot in this
        park at all" is the barrier; "and the thing they are flying is a
        ship" is an assertion the body makes afterwards, out of the census.
        Membership is not shape, and conflating the two is how one cause
        produces two reds.
        """
        try:
            ids = list(park.balls.keys())
        except Exception:
            return None, 0
        for ballID in ids:
            try:
                slim = park.GetInvItem(ballID)
            except Exception:
                slim = None
            if slim is None:
                continue
            if getattr_safe(slim, 'charID') == EXPECT_CHARID:
                return ballID, len(ids)
        return None, len(ids)

    def rendezvous(park):
        """Wait until the other pilot's ship is really in our park.

        `(ballID, waited_ms, polls, ball count)` -- `ballID` is `None` on
        timeout, and the ball count is reported either way so the failing case
        still says what the sky did contain.

        **This is the rendezvous, and its success is not the assertion.**  The
        caller records the two separately on purpose: a timeout here means the
        two clients were never both in space, which is a statement about the
        harness and the clients' startup skew and says *nothing* about the
        server -- see the module docstring and `docs/lanes/ci-harness.md`
        section 11.  Conflating the two is what made 2026-10-23's symmetric
        red read as a missing server feature and put it in `docs/archive/roadmap.md`.

        Written as an explicit loop rather than through `wait_for` because the
        poll count and the ball count are part of the diagnosis.  `waited_ms`
        is the sum of the sleeps and therefore a lower bound on the wallclock;
        it is not read from a clock because `sleep_ms` is the only timing
        primitive in this file that is [V].
        """
        polls = 0
        waited = 0
        count = 0
        while True:
            ballID, count = other_ball(park_now(park))
            polls += 1
            if ballID is not None:
                return ballID, waited, polls, count
            if waited >= OTHER_TIMEOUT_MS:
                return None, waited, polls, count
            sleep_ms(OTHER_POLL_MS)
            waited += OTHER_POLL_MS

    def linger_report(park, otherID, hold_ms):
        """Stay in space for `hold_ms`, so the other client can see us.

        `(waited_ms, polls, together, lost_at)`.  `together` is how many polls
        still found `otherID` in the park and `lost_at` is the elapsed time at
        the first poll that did not (`None` if it never stopped seeing it).

        **The sleeping is the point; the polling is only the measurement.**
        What the other client needs from us is that this process keeps its
        session open and its ball in the park, and the only way a scenario
        provides that is by not terminating yet.

        **This docstring used to say the measurement was weak, and that
        stopped being true before it was rewritten.**  It read: *"when the
        other client quits, the server removes its presence immediately but
        can only queue the `BallsRemoved`, and nothing drains that queue
        except some connection taking a turn -- and this client, lingering, is
        by construction making no calls.  So a departed ball can sit in our
        park for the rest of the linger and `together == polls` is the
        expected reading either way."*

        A disconnect now **schedules** the removal on the clock queue, which
        the world ticker drains with no turn involved -- `port-tracker.md`
        sections 36 and 37.1, pinned by
        `a_quitters_ball_is_removed_by_the_clock_not_by_a_turn`.  So
        `Park.GetBall(id)` being a local read (`self.balls.get(id, None)`,
        `michelle.py:1888`) [V] is no longer a reason to distrust the number:
        the ball leaves our park because the server told us, while we were
        calling nothing at all.  `together == polls` now means the other
        client was up for the whole hold, and a `lost_at` means it quit and
        the removal arrived.

        That is why one side holds longer -- see `LINGER_EXTRA_MS`.  The
        watcher's `lost_at` is the only end-to-end observation of a
        clock-delivered push there is, and it is the one thing in this
        scenario that *is* asserted about the linger.
        """
        polls = 0
        together = 0
        waited = 0
        lost_at = None
        while waited < hold_ms:
            sleep_ms(LINGER_POLL_MS)
            waited += LINGER_POLL_MS
            still = None
            try:
                still = park_now(park).GetBall(otherID)
            except Exception:
                still = None
            polls += 1
            if still is not None:
                together += 1
            elif lost_at is None:
                lost_at = waited
        return waited, polls, together, lost_at

    def eoci_body():
        # **Undock first.** `/autoSelectCharacter:0` lands in the station,
        # and there is no ballpark in a hangar -- the first unattended run
        # failed on exactly that, with five checks green and
        # `session.stationid = 60012526`. The prelude's `undock` has the
        # evidence that this does not prompt.
        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)
        session = builtin('session')
        if session is None:
            check('session is reachable', False, 'session = None')
            return
        shipid = getattr_safe(session, 'shipid')
        charid = getattr_safe(session, 'charid')

        park, ball = get_ego_ball()
        check('the ego ball is in the park', ball is not None,
              'ballpark.GetBall(ballpark.ego) = %s' % text(ball))
        if park is None:
            return

        # -- the rendezvous ------------------------------------------------
        #
        # **Before the census, not after it, and that ordering is a fix in
        # its own right.**  The census below snapshots `park.balls`,
        # `park.globals` and `park.slimItems`, and one of its checks is that
        # every ball has a slimItem -- which a ball arriving *while* the
        # snapshot is taken does not yet, because the ball is native and the
        # slimItem comes a queue hop later (`michelle.py:1680`, `:1292-1298`,
        # `:808`) [V].  In a two-player run the other ship arrives exactly
        # then.  Waiting for the other pilot's slimItem first means the
        # census sees a settled park, and it is also the only ordering under
        # which the census's `other_ships` line can be the assertion.
        #
        # **Eligibility is already established by the check above.**  To
        # receive another ship's ball this client must be undocked *and* have
        # bound its ballpark: before the bind `DoDestinyUpdate` returns at
        # `michelle.py:632-633` [V], and on the server the bind is also what
        # puts us in the roster (`destiny::on_ballpark_bind` calls
        # `roster_mut().arrive(...)` and only then pushes). Our own bootstrap
        # state is pushed from that same function, so "the ego ball is in the
        # park" *is* the proof that the bind landed and that we are in the
        # audience for anyone who undocks after us. Nothing further is
        # required of the observer.
        #
        # **Order between the two clients does not matter**, because the
        # server implements both halves: whoever undocks later gets the one
        # already in space in its own bootstrap (`destiny::bootstrap_for`
        # loops the roster's `in_system`), and whoever undocked earlier is
        # sent an `AddBalls2` (`destiny::on_ballpark_bind`'s `BallsAdded`).
        # Both of those are drained by the *arriving* client's own turn, so
        # an idle observer is written to within a tick of the push
        # (`docs/lanes/two-in-space.md` section 1.2) and needs to do nothing
        # but yield. If only one of the two halves is broken, the two
        # verdicts disagree -- see the failing check's text.
        rendezvous_ok = False
        otherID = None
        if EXPECT_CHARID:
            otherID, waited, polls, count = rendezvous(park)
            rendezvous_ok = otherID is not None
            if rendezvous_ok:
                check('both clients were in space at the same time', True,
                      'a ball whose slimItem.charID is %s appeared in this'
                      ' park after %s ms of re-reading (%s poll(s)): ball %s,'
                      ' in a park of %s ball(s).  %s ms is itself a'
                      ' measurement worth reading: 0 ms means this client got'
                      ' into space last and the other was already there (so'
                      ' their ball came in our own bootstrap state), a large'
                      ' number means they arrived while we watched (so it was'
                      ' a pushed AddBalls2).  This client now stays in space'
                      ' for %s ms before writing its verdict, so it cannot'
                      ' quit before the other one looks -- LINGER_MS, plus'
                      ' LINGER_EXTRA_MS if this is the watcher (the lower'
                      ' character id), which is the side that outlasts the'
                      ' other and asserts that its ball left.  Every'
                      ' poll is a local read of michelle\'s own park, not a'
                      ' wire call -- nothing the client can call returns the'
                      ' park, see this file\'s docstring'
                      % (text(EXPECT_CHARID), text(waited), text(polls),
                         text(otherID), text(count), text(waited),
                         text(LINGER_MS
                              + (LINGER_EXTRA_MS
                                 if charid < EXPECT_CHARID else 0))))
            else:
                check('both clients were in space at the same time', False,
                      'no ball whose slimItem.charID is %s ever appeared in'
                      ' this park: %s poll(s) over %s ms, and the park held'
                      ' %s ball(s) at the last one (our own ego ball: %s).'
                      '  THE OTHER CLIENT NEVER APPEARED, SO THIS RUN'
                      ' MEASURED NOTHING -- this is not the same finding as'
                      ' "the server did not broadcast the arrival", and from'
                      ' inside one client the two cannot be told apart.  That'
                      ' exact confusion is what put "arrivals are not'
                      ' broadcast" into docs/archive/roadmap.md on 2026-10-23 and got'
                      ' it withdrawn: both clients failed symmetrically'
                      ' because the first had undocked, censused and quit'
                      ' before the second finished booting.  Read the other'
                      ' client\'s verdict to decide which it was: if it failed'
                      ' this same check, the two clients simply never'
                      ' overlapped in space (the second one did not boot, or'
                      ' its undock retried so long that %s ms of'
                      ' OTHER_TIMEOUT_MS ran out, or LINGER_MS is shorter than'
                      ' the skew between them) and nothing has been learned'
                      ' about the server; if it PASSED -- it saw our ship'
                      ' while we could not see its -- then one of the two'
                      ' halves of an arrival really is missing, and which one'
                      ' follows from who undocked first: the client that'
                      ' undocked second should have been given the other in'
                      ' its bootstrap state (destiny::bootstrap_for), and the'
                      ' client that undocked first should have been pushed an'
                      ' AddBalls2 (destiny::on_ballpark_bind).  The client log'
                      ' separates those two: a delivered batch logs "Handling'
                      ' Server Update with N event(s)" then "Action:'
                      ' AddBalls2" (michelle.py:1508, :1548), a batch dropped'
                      ' for a desync logs "Events ignored" (:1594), and a'
                      ' batch that arrived before this park existed logs'
                      ' nothing at all (:632-633).  No assertion about the'
                      ' other ship was made below, because there was nothing'
                      ' to assert it about'
                      % (text(EXPECT_CHARID), text(polls), text(waited),
                         text(count),
                         'present' if ball is not None else 'MISSING TOO,'
                         ' which makes this red downstream of the ego-ball'
                         ' check above rather than about the other client',
                         text(OTHER_TIMEOUT_MS)))

        ids = []
        try:
            ids = sorted(park.balls.keys())
        except Exception:
            pass
        globals_ids = []
        try:
            globals_ids = sorted(park.globals.keys())
        except Exception:
            pass
        slim_ids = []
        try:
            slim_ids = sorted(park.slimItems.keys())
        except Exception:
            pass

        check('the park holds at least one ball', len(ids) >= 1,
              '%d ball(s): %s' % (len(ids), text(ids)))
        check('the ego ship is among the park\'s balls', shipid in ids,
              'session.shipid = %s, ball ids = %s' % (text(shipid), text(ids)))

        # Every ball the client draws needs a slimItem: `Park.GetBallsAndItems`
        # (`michelle.pyc:1874-1881`) silently skips any ball whose id is not in
        # `self.slimItems` [V], so a ball with no slimItem is a ball that
        # exists in the simulation and is invisible in the UI.  That is a
        # defect class worth its own check.
        #
        # The client mints its own balls (the sensor overlay's site markers,
        # map markers) with ids counting down from michelle's
        # `clientBallNextID = -8000000000000000000L` (michelle.py:797,
        # AddClientSideBall :1251) and puts them in ballpark.globals.  A
        # server never sends those; they have no slimItem by design.  Only
        # that range is excluded: any other negative id must still fail.
        CLIENT_BALL_FIRST = -8000000000000000000
        client_balls = [b for b in ids
                        if b <= CLIENT_BALL_FIRST and b not in slim_ids]
        orphans = [b for b in ids if b > CLIENT_BALL_FIRST
                   and b not in slim_ids]
        check('every ball has a slimItem', not orphans,
              '%d ball(s) with no slimItem: %s.  Park.GetBallsAndItems skips'
              ' these (michelle.pyc:1879), so the overview and the brackets'
              ' never see them.  Excluded %d client-side ball(s) minted from'
              ' clientBallNextID: %s'
              % (len(orphans), text(orphans), len(client_balls),
                 text(client_balls)))
        check('the park reports some global balls', len(globals_ids) >= 1,
              '%d global ball(s): %s.  ballpark.globals is the isGlobal'
              ' subset -- celestials.  Empty means nothing in the system is'
              ' marked global, which is a server-side question worth asking'
              ' separately from this scenario\'s verdict'
              % (len(globals_ids), text(globals_ids[:16])))

        # A readable census, in one detail line.  `Park.GetInvItem(id)` is
        # `self.slimItems.get(id, None)` (`michelle.pyc:2121`) [V] and
        # `SlimItem.__getattr__` returns `None` for anything that was not
        # sent rather than raising (`eve/common/script/util/slimItem.pyc:44`)
        # [V], so none of these reads can throw.
        rows = []
        other_ships = []
        for ballID in ids:
            slim = None
            try:
                slim = park.GetInvItem(ballID)
            except Exception:
                slim = None
            type_id = getattr_safe(slim, 'typeID')
            category = getattr_safe(slim, 'categoryID')
            group = getattr_safe(slim, 'groupID')
            char_id = getattr_safe(slim, 'charID')
            owner = getattr_safe(slim, 'ownerID')
            rows.append('%s(type=%s cat=%s grp=%s char=%s owner=%s)'
                        % (text(ballID), text(type_id), text(category),
                           text(group), text(char_id), text(owner)))
            if category == CATEGORY_SHIP and ballID != shipid:
                other_ships.append((ballID, char_id))
        say('census: %s' % ('  '.join(rows)))

        check('the ego ship\'s slimItem names this character',
              getattr_safe(
                  (park.GetInvItem(shipid) if shipid else None),
                  'charID') == charid,
              'GetInvItem(%s).charID = %s, session.charid = %s'
              % (text(shipid),
                 text(getattr_safe(
                     (park.GetInvItem(shipid) if shipid else None), 'charID')),
                 text(charid)))

        detail = '%d other ship ball(s): %s.  Full census in the client log' \
            % (len(other_ships), text(other_ships))
        if not EXPECT_CHARID:
            check('the census was taken', True,
                  '%s.  EXPECT_CHARID is 0, so no second character is'
                  ' asserted; see docs/lanes/ci-scenarios.md section 5'
                  % detail)
            return

        # **A failed rendezvous asserts nothing, and that is the whole point.**
        # Every check below would go red for the rendezvous's reason rather
        # than for its own, and two reds with one cause is exactly how a run
        # that measured nothing read as a server bug on 2026-10-23.  The one
        # red in this verdict is the rendezvous, and its text is the
        # diagnosis.  (The one exception is an ego ball that never arrived:
        # then the ego-ball check above is red too, for its own, earlier
        # reason, and the rendezvous detail says so in those words.)
        if not rendezvous_ok:
            say('rendezvous timed out: no assertion made about charID %s.'
                ' This run measured nothing; see the failing check'
                % text(EXPECT_CHARID))
            return

        # `Michelle.GetCharIDFromShipID(shipID)` is
        # `self.__bp.slimItems[shipID].charID` (`michelle.pyc:683`) [V] -- the
        # client's own answer to "whose ship is that".
        #
        # **This is not a tautology of the rendezvous.**  The barrier asked
        # only whether a ball carrying their charID exists; `other_ships` is
        # built with two conditions the barrier deliberately does not apply --
        # `slim.categoryID == CATEGORY_SHIP` and `ballID != session.shipid`.
        # So a green rendezvous with a red check here is a real finding and
        # not a timing artefact: their pilot's ball is in the park but the
        # thing it is attached to is not categorised as a ship, which means a
        # `typeID` whose category resolves to something else -- and an
        # overview that will not list them.
        found = [b for (b, c) in other_ships if c == EXPECT_CHARID]
        check('the second character\'s ship is in the park', bool(found),
              'expected a ball whose slimItem.charID is %s AND whose'
              ' slimItem.categoryID is %s (categoryShip), found %s.  The'
              ' rendezvous saw ball %s for that character %s ms earlier, so if'
              ' this is red while the rendezvous is green the ball is present'
              ' and its category is wrong, not missing.  %s'
              % (text(EXPECT_CHARID), text(CATEGORY_SHIP), text(found),
                 text(otherID), text(waited), detail))
        if not found:
            return
        otherID = found[0]
        mich = get_michelle()
        reported = None
        try:
            reported = mich.GetCharIDFromShipID(otherID)
        except Exception:
            pass
        check('michelle maps the second ship back to its character',
              reported == EXPECT_CHARID,
              'michelle.GetCharIDFromShipID(%s) = %s, expected %s'
              % (text(otherID), text(reported), text(EXPECT_CHARID)))

        dist = None
        try:
            dist = park.GetSurfaceDist(shipid, otherID)
        except Exception:
            pass
        check('the two ships are a readable distance apart', dist is not None,
              'ballpark.GetSurfaceDist(%s, %s) = %s m'
              % (text(shipid), text(otherID), fmt(dist, 1)))

        # -- stay put, so the other client can see us ----------------------
        #
        # **The second half of the rendezvous, and the half 2026-10-23 was
        # missing.**  Everything above is decided; this only keeps the session
        # open and this ship's ball in the other park.  It is last on purpose:
        # it costs `LINGER_MS` and nothing above it should pay that.
        #
        # A single-client run never reaches it (`EXPECT_CHARID` is 0 returns
        # above) and neither does a failed rendezvous: if the other client was
        # never seen in `OTHER_TIMEOUT_MS` there is nobody to wait for, this
        # verdict already says the run measured nothing, and waiting longer
        # would only risk turning one honest red into a mismatched pair of
        # verdicts.
        # **The watcher is the lower character id**, computed from two
        # numbers both sides know so that no launch order has to be plumbed
        # through the harness. See `LINGER_EXTRA_MS`.
        watcher = charid < EXPECT_CHARID
        hold_ms = LINGER_MS + (LINGER_EXTRA_MS if watcher else 0)
        lingered, lpolls, together, lost_at = linger_report(park, otherID,
                                                           hold_ms)
        check('this client stayed in space for the other one to see it', True,
              'stayed in space for %s ms after the rendezvous (hold %s ms,'
              ' %s),'
              ' re-reading the park every %s ms: ball %s (charID %s) was still'
              ' in it on %s of %s poll(s)%s.  REPORT ONLY, nothing is'
              ' asserted.  It MEANS what it looks like as of 2026-10-24:'
              ' a disconnect now schedules its BallsRemoved on the clock'
              ' queue, which the world ticker drains with no turn involved'
              ' (port-tracker section 36 and 37.1, pinned by'
              ' a_quitters_ball_is_removed_by_the_clock_not_by_a_turn), so'
              ' "still there on all of them" says the other client was up for'
              ' the whole linger and a lost_at says it quit and the removal'
              ' arrived while nobody called anything.  This text used to say'
              ' the opposite -- that the server could only queue the removal'
              ' until some connection took a turn, so all-present proved'
              ' nothing -- and that stopped being true before it was'
              ' rewritten.  It stays REPORT ONLY for a reason that has not'
              ' changed: which client finishes first is a race, so a run'
              ' where we quit first would have nothing to see.  The numbers'
              ' that'
              ' size LINGER_MS are the two rendezvous elapsed times, one per'
              ' verdict; the lobby pair measured ~6 s of skew (6000 ms and'
              ' 0 ms) against this same'
              ' 30 s on 2026-10-23 (docs/lanes/ci-harness.md section 11).'
              '  Without this wait, whichever client reaches the rendezvous'
              ' first quits before the other looks, which is what invalidated'
              ' every two-player verdict of that day'
              % (text(lingered), text(hold_ms),
                 'the watcher: LINGER_MS + LINGER_EXTRA_MS, so the other'
                 ' client quits while we are still looking' if watcher
                 else 'LINGER_MS, so we quit first and the watcher sees it',
                 text(LINGER_POLL_MS), text(otherID),
                 text(EXPECT_CHARID), text(together), text(lpolls),
                 ('' if lost_at is None
                  else ', first missing %s ms into the linger -- so the'
                       ' removal WAS delivered, which is a finding in its own'
                       ' right' % text(lost_at))))

        # **The one thing asserted about the linger, and only on the watcher.**
        #
        # The watcher holds 30 s longer than the other client, so the other
        # client's socket closes while this one is still polling -- and this
        # one is calling nothing, which is the whole point. A disconnect
        # schedules the `BallsRemoved` on the clock queue and the world ticker
        # sends it within a tick (`port-tracker.md` 36, 37.1), so the ball has
        # to leave our park on its own.
        #
        # **Red here means a real regression with one of two shapes:** the
        # removal went back on the turn queue -- the exact defect
        # `two-in-space.md` section 1.3 was written about, where a departed
        # ship hangs in space to be targeted and shot by the one player who is
        # doing nothing -- or the other client outlived our hold, which the
        # elapsed numbers in this same detail distinguish.
        #
        # The other side asserts nothing: it quits first and has nothing to
        # see. That asymmetry is deliberate and is why `LINGER_EXTRA_MS`
        # exists.
        #
        # No bound is asserted on `lost_at`. The arrival is one 250 ms tick
        # after a socket close and the polls are 2 s apart, so a bound would
        # be a guess about process teardown; the measured value is printed
        # instead and `ci-scenarios.md` section 14 collects it.
        if watcher:
            check('the other client\'s ball left our park when it quit,'
                  ' with us calling nothing',
                  lost_at is not None,
                  'ball %s (charID %s) was still in the park on %s of %s'
                  ' poll(s) across a %s ms hold, and we outlasted the other'
                  ' client by %s ms by construction.  A clock-delivered'
                  ' RemoveBalls should have taken it out of our park without'
                  ' this client calling anything; %s'
                  % (text(otherID), text(EXPECT_CHARID), text(together),
                     text(lpolls), text(hold_ms), text(LINGER_EXTRA_MS),
                     'it never left' if lost_at is None
                     else 'it left %s ms in' % text(lost_at)))

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
