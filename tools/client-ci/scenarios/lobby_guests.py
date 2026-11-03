# -*- coding: utf-8 -*-
"""Two players in one station can see each other in the lobby's Guests panel.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  See `docs/lanes/ci-scenarios.md`.

**Step C's observable, from inside the client.**  `docs/lanes/roster.md` calls
`station.GetGuests` "the cheapest thing two players can see each other
through", and `server/crates/eve-server/src/service/world.rs:226` says the
same from the other side.  This scenario is the oracle for it: run it with
`tools/client-ci.sh run lobby_guests --with-second -- --character Alice
--character Bob` and each client asserts that the *other* character is
standing in its own station lobby.

**It does not undock, and must not.**  `Registry::characters_at_station`
filters `sessions.online_characters()` by `!roster.is_in_space(id)`
(`service.rs:1725-1734`), so a pilot in space is correctly absent from the
guest list -- the opposite precondition from `park_census`, which undocks on
its first line.  Nothing here calls `UndockAttempt`.

The call chain this scenario walks, all `[V]` from this build's `co_lnotab`:

    eve/client/script/ui/shared/dockedUI/guests.py:116
        ServiceManager.Instance().GetService('station').GetGuests()
    eve/client/script/ui/station/base.py:133   StationSvc.GetGuests
      :134  currentStationID = session.stationid
      :135  if not self.guestListReceived or
                self.guestListReceived != currentStationID:
      :136      self.guests.clear()
      :137      guests = sm.RemoteSvc('station').GetGuests()
      :138      for charID, corpID, allianceID, warFactionID in guests:
      :139          self.guests[charID] = (corpID, allianceID, warFactionID)
      :140      self.guestListReceived = currentStationID
      :141  return self.guests

So the wire row is a **4-sequence** `(charID, corpID, allianceID,
warFactionID)` and what the panel iterates is a **dict**
`{charID: (corpID, allianceID, warFactionID)}` -- two different shapes, one
of them cached.

**The cache is bustable, and this scenario busts it.**  `guestListReceived`
holds a *station id*, not a flag, and it has exactly four references in the
whole file: read twice at `:135`, written at `:140`, and written `None` at
`:202` inside `CleanUp(clearGuestList=True)` [V]
(`tools/call_lines.py out/pyc/.../station/base.pyc guestListReceived`).
Setting it to `None` is therefore line 202 on its own, and the next
`GetGuests()` goes to the server.  **`CleanUp` itself is not usable for this**:
it also runs `uicore.registry.UnregisterWindow(self.underlay)` (`:174`) and
`uix.Close(self, ['closeBtn', 'hint', 'underlay', 'lobby'])` (`:181`) and then
nulls `self.lobby` (`:183`) -- it would tear down the very lobby we are
observing.

That the cache is bustable is what makes this scenario honest: the scenario
runs from the first session change (`docs/lanes/ci-trigger.md`), which is
earlier than the second client has finished logging in, so it has to re-ask.
Each poll clears the stamp first, so every poll is a real
`sm.RemoteSvc('station').GetGuests()` and a green verdict cannot have come out
of a cache.

**Two clients need a rendezvous, and that is what the second half of this
file is about.**  A scenario writes its verdict and then terminates the
client, which is right for one client and was wrong for two: on 2026-10-23
both two-player verdicts were invalid because the clients' presence barely
overlapped (`docs/lanes/ci-harness.md` section 11).  The client that reached
this assertion first found the other already standing there, wrote `0 ms` and
quit; the other then polled an empty-but-for-itself lobby for the full 90 s
and reported a server bug that does not exist -- the wire log shows ~45 real
`station::GetGuests` calls, every one answered `[[90000001, ...]]`, so the
cache-bust worked and there was simply nobody to see.

So with `EXPECT_CHARID` set this scenario does two things it did not do:

1. **The poll for the other character is the rendezvous, not the assertion.**
   Its failure says "the other client never appeared, so this run measured
   nothing", which is a different sentence from "the server omitted them", and
   only one of the two is a server bug.
2. **It then lingers (`LINGER_MS`) before writing its verdict and quitting**,
   so whichever client gets here first is still standing in the lobby when the
   other one looks.  Without that half, a rendezvous alone just moves the race.

With `EXPECT_CHARID = 0` neither happens: a single-client run is exactly what
it was -- no rendezvous, no linger, the census reported and the ego-side facts
asserted.

Set `EXPECT_CHARID` below to the other character's id to turn the census into
an assertion about them; the harness rewrites that one line on deploy --
`docs/lanes/ci-harness.md` section 10, `docs/lanes/ci-scenarios.md` section 5.
"""

EOCI_SCENARIO = 'lobby_guests'

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

    #: Set this to the OTHER character's id, or leave it 0.  With 0 the
    #: scenario reports the whole guest list and asserts only the ego-side
    #: facts, so a single-client run is still meaningful and still green; with
    #: a charID it additionally asserts that that character is standing in
    #: this station's lobby, which is step C's observable.
    #:
    #: The harness rewrites this line as it deploys the file -- the regex is
    #: `^ *EXPECT_CHARID *= *0 *$` (`tools/client-ci.sh:582`), so it must stay
    #: a single assignment, spaces-only indentation, nothing after the `0`.
    #: Same spelling and same position as `park_census.py`, on purpose.
    EXPECT_CHARID = 0

    #: How long to wait for the other client to finish logging in and docking.
    #: The harness staggers the second client (`EVE_SECOND_DELAY`, default 5 s,
    #: `docs/lanes/ci-harness.md` section 10) and that client then spends ~35 s
    #: booting, so 90 s is generous and still well inside the harness's own
    #: 600 s verdict timeout (`tools/client-ci.sh:137`).  It only elapses in
    #: the failing case.
    OTHER_TIMEOUT_MS = 90000

    #: The rendezvous poll interval.  Every poll is a real wire call, because
    #: `fresh_guests` clears the cache stamp first, so this doubles as the rate
    #: at which `station.GetGuests` is asked: 45 calls across the full 90 s,
    #: which is the number the 2026-10-23 client log shows.
    OTHER_POLL_MS = 2000

    #: **How long to stay in the lobby after the rendezvous has succeeded,
    #: before writing the verdict and quitting.  This constant is the fix for
    #: the overlap bug; the rendezvous on its own is only half of it.**
    #:
    #: It has to exceed the *skew* between the two clients reaching the
    #: rendezvous, because the earlier one quits at `its rendezvous + LINGER_MS`
    #: and the later one needs it present at `its own rendezvous`.
    #:
    #: Why 30 s.  The skew is the difference between two ~40 s startup paths
    #: (wine start, login, character select, dock, then this scenario's own
    #: `session.stationid` and `svc.station` waits) offset by the harness's 5 s
    #: stagger, and on 2026-10-23 it measured at a few seconds -- small, but
    #: small as the difference of two large numbers, with its spread never
    #: measured, and in the wrong direction from the obvious guess: the client
    #: launched *second* reached this point *first*, because the first client's
    #: launch is the cold one.  30 s therefore dominates the one observed skew
    #: by about an order of magnitude and covers the 5 s stagger plus a 25 s
    #: divergence in boot time, which is most of a whole boot.  It is not a
    #: measurement, and the verdict reports what the linger actually saw so
    #: that the next run turns it into one (see `linger_report`).
    #:
    #: The budget it has to fit in.  Worst case for the client that waits
    #: longest: 30 s (`session.stationid`) + 30 s (`svc.station`) + 90 s
    #: (`OTHER_TIMEOUT_MS`) + 30 s (this) = 180 s after the trigger fires, plus
    #: ~35 s of boot before it -- ~215 s against the harness's 600 s, which
    #: leaves room for this number to be tripled before anything has to change.
    #: In the passing case it costs the run one linger, not two, because both
    #: clients linger at overlapping times.
    #:
    #: **It is unconditional -- both clients linger, not just the later one.**
    #: A client can see whether its *first* poll already contains the other
    #: character, and that does tell it who logged in last.  It is the wrong
    #: question.  What the earlier client needs to know is whether the other
    #: has *already made its observation*, and a character is in the guest list
    #: from its login, which is tens of seconds before its own copy of this
    #: scenario reaches the rendezvous -- "they were already here" does not
    #: imply "they have already seen me".  The 2026-10-23 run is exactly that
    #: case: the first client's poll found nobody, so it would have branched to
    #: "I am the early one, no linger needed", and it was the one whose check
    #: failed.  Branching would also put a correctness risk on a signal nobody
    #: has measured, to save 30 s on a run that takes ~55 s; the whole point of
    #: this constant is that a two-player red should mean the server.
    LINGER_MS = 30000

    #: The linger's own poll interval.  Deliberately the same rate as the
    #: rendezvous, so "still there on N of M polls" and "appeared after K ms"
    #: are numbers on one scale.  These polls are diagnostics only: nothing is
    #: asserted about them, because the other client disappearing during our
    #: linger is the *expected* ending once its own linger has run out.
    LINGER_POLL_MS = 2000

    def station_svc(timeout_ms=30000):
        """`(svc.station, detail)` -- preferring the instance already running.

        **Waited for rather than demanded.**  `sm.GetService(name)` *starts* a
        service that is not up, and `StationSvc.__dependencies__` is
        `['journal', 'insurance', 'crimewatchSvc']` (`station/base.py:62-64`)
        [V], so forcing the start from inside the settings tasklet the trigger
        runs us in would drag three more services up a path no player ever
        takes.  `park_census` learned the general shape of this the expensive
        way: its first unattended run died because the dogma location did not
        hold the ship yet.

        Docking loads the lobby, which starts `svc.station` on its own, so the
        patient version is also the faithful one -- we end up reading the same
        service instance the panel reads.  `GetServiceIfRunning(name)` returns
        `self.services[name]` when `IsServiceRunning(name)`, else `None`
        (`carbon/common/script/sys/serviceManager.py:334-342`) [V], so it
        never starts anything.  `sm.GetService` is only the fallback, and the
        detail says which route answered.
        """
        sm = builtin('sm')
        if sm is None:
            return None, 'no sm to reach svc.station through'
        svc, waited = wait_for(lambda: sm.GetServiceIfRunning('station'),
                              timeout_ms=timeout_ms, step_ms=500)
        if svc is not None:
            return svc, ('svc.station was already running (%s ms)'
                         % text(waited))
        try:
            svc = sm.GetService('station')
        except Exception, e:
            return None, ("svc.station was not running after %s ms and"
                          " sm.GetService('station') raised %s: %s"
                          % (text(timeout_ms), e.__class__.__name__, text(e)))
        return svc, ("svc.station was not running after %s ms;"
                     " sm.GetService('station') started it, so three"
                     " dependencies came up with it" % text(timeout_ms))

    def fresh_guests(svc):
        """`(guest dict or None, detail)` from a **real** remote call.

        Clears `guestListReceived` first, which is exactly what
        `CleanUp(clearGuestList=True)` does at `station/base.py:202` and
        nothing else `CleanUp` does.  With the stamp gone, `:135`'s
        `not self.guestListReceived` is true, so `:136` clears the dict and
        `:137` calls `sm.RemoteSvc('station').GetGuests()` [V].

        **The write lands, and that was worth checking.**  A service is a
        `CoreService`, which defines its own `__setattr__`
        (`carbon/common/script/sys/service.py:985-993`) [V]:

            986  super(CoreService, self).__setattr__(key, value)
            988  if key in self.__configvalues__:
            989      value = self.OnSetConfigValue(key, value)
            990      prefs.SetValue('%s.%s' % (self.__logname__, key), value)
            991      return
            993  self.__dict__[key] = value

        `CoreService.__configvalues__` is `{}` (`:661`) and `station/base.pyc`
        never mentions `__configvalues__` at all, so `guestListReceived` takes
        the plain `__dict__` write at `:993` -- no `prefs` side effect, no
        interception.  `GetGuests` writes the same attribute through the same
        `__setattr__` at `:140`, so this is the client's own path and not a
        trick.  Had that hook swallowed the write, this scenario would have
        been testing the cache while claiming to test the server, which is the
        one outcome worth more than the round it would have cost.

        **Writing it first also stops `GetGuests` raising on a very early
        run.** `guestListReceived` is never initialised in `__init__`, which
        sets only `self.guests = {}` (`station/base.py:84`), and it has no
        class-level default -- it appears nowhere in the `StationSvc` class
        body [V].  So before `CleanUp(clearGuestList=True)` has run from
        `OnCharacterSelected` (`:94`) or `OnSessionReset` (`:98`), `:135`'s
        read falls through to `CoreService.__getattr__` and
        `raise_(AttributeError, key)` (`service.py:982`) [V].  Both of those
        handlers are `__notifyevents__` entries (`:73`, `:78`) on the same
        session change that triggers this scenario, and nothing orders them
        against us -- so this scenario might otherwise be the first caller.
        Assigning before calling makes that ordering irrelevant.

        If the write itself fails the call is still made -- a cached answer is
        better than no answer -- but the detail says so, because "we tested
        the cache" and "we tested the server" are different claims and only
        the detail can tell them apart afterwards.
        """
        how = 'cache stamp cleared, so this is a real station.GetGuests call'
        try:
            svc.guestListReceived = None
        except Exception, e:
            how = ('COULD NOT clear svc.guestListReceived (%s: %s), so this'
                   ' answer may be the client-side cache and not the server'
                   % (e.__class__.__name__, text(e)))
        try:
            return svc.GetGuests(), how
        except Exception, e:
            return None, ('svc.station.GetGuests() raised %s: %s -- %s.  A'
                          ' ValueError here is the wire row arity:'
                          ' station/base.py:138 unpacks each row into exactly'
                          ' four fields (charID, corpID, allianceID,'
                          ' warFactionID)'
                          % (e.__class__.__name__, text(e), how))

    def census(guests):
        """The guest list as one line a human can read."""
        rows = []
        try:
            for charID in sorted(guests.keys()):
                rows.append('%s->%s' % (text(charID), text(guests[charID])))
        except Exception:
            return text(guests)
        return '{%s}' % ', '.join(rows)

    def rendezvous(svc):
        """Wait until the other character is really in our guest list.

        `(found, last, waited_ms, polls, how)` -- `found` is the guest dict
        that contained `EXPECT_CHARID`, or `None` on timeout; `last` is the
        most recent dict seen either way, so the failing case still has
        something to report.

        **This is the rendezvous, and its success is not the assertion.**  The
        caller records the two separately on purpose: a timeout here means the
        two clients were never both present, which is a statement about the
        harness and the clients' startup skew and says *nothing* about the
        server -- see the module docstring and `docs/lanes/ci-harness.md`
        section 11.  Conflating the two is what made 2026-10-23's red read as
        a missing server feature.

        Written as an explicit loop rather than through `wait_for` because the
        poll *count* is part of the diagnosis: 46 polls over 90 s is 46 real
        `sm.RemoteSvc('station').GetGuests()` calls, and that number is how the
        cache-bust was cleared of suspicion when this bug was found.

        `waited_ms` is the sum of the sleeps and therefore a *lower bound* on
        the wallclock: each poll also makes a blocking wire call, which is not
        counted.  It is not read from a clock because this scenario has no
        verified wallclock source it can call -- `sleep_ms` is the only timing
        primitive here that is `[V]`.
        """
        polls = 0
        waited = 0
        last = None
        how = 'no poll completed'
        while True:
            answer, how = fresh_guests(svc)
            polls += 1
            if isinstance(answer, dict):
                last = answer
                if EXPECT_CHARID in answer:
                    return answer, last, waited, polls, how
            if waited >= OTHER_TIMEOUT_MS:
                return None, last, waited, polls, how
            sleep_ms(OTHER_POLL_MS)
            waited += OTHER_POLL_MS

    def linger_report(svc):
        """Stay in the lobby for `LINGER_MS`, so the other client can see us.

        `(waited_ms, polls, together, lost_at)`.  `together` is how many polls
        still found `EXPECT_CHARID`, and `lost_at` is the elapsed time at the
        first poll that did not (or `None` if it never stopped seeing them).

        **The sleeping is the point; the polling is only the measurement.**
        What the other client needs from us is that this process keeps its
        session open, and the only way a scenario provides that is by not
        terminating yet.  The polls cost one wire call each and buy the number
        nobody had on 2026-10-23: how long the two clients' presence actually
        overlaps.  If a future verdict says the other character vanished on the
        *first* linger poll, the two lingers barely overlap and `LINGER_MS` is
        too small; if it says they stayed for all of them, it is comfortable
        and could come down.

        Nothing here is asserted.  The other client vanishing partway through
        is the normal ending -- its own linger started earlier than ours and
        runs out earlier -- and a check that went red on it would be the
        original bug wearing a different hat.
        """
        polls = 0
        together = 0
        waited = 0
        lost_at = None
        while waited < LINGER_MS:
            sleep_ms(LINGER_POLL_MS)
            waited += LINGER_POLL_MS
            answer, _how = fresh_guests(svc)
            polls += 1
            if isinstance(answer, dict) and EXPECT_CHARID in answer:
                together += 1
            elif lost_at is None:
                lost_at = waited
        return waited, polls, together, lost_at

    def panel_guests():
        """`(list of Guest or None, detail)` -- the panel's own data path, run.

        This is the one check that separates "the data is there" from "the
        panel would draw it", and it is not a reconstruction: it calls the
        real method on a real instance of the real class.

        `GuestsPanelController._get_station_guests`
        (`eve/client/script/ui/shared/dockedUI/guests.py:114-123`) [V] is:

            116  guests_raw = ServiceManager.Instance().GetService(
                                  'station').GetGuests()
            123  return [Guest(character_id, corporation_id, alliance_id,
                               war_faction_id)
                         for character_id, (corporation_id, alliance_id,
                                            war_faction_id)
                         in guests_raw.items()]

        -- an `UNPACK_SEQUENCE 2` then an `UNPACK_SEQUENCE 3` then
        `CALL_FUNCTION 4` into `Guest.__init__(self, character_id,
        corporation_id, alliance_id, war_faction_id)` (`:488-492`) [V].

        **Why running it is worth a check of its own.** Its only caller,
        `_load_guest_list` (`:138`), wraps the whole load in one
        `SETUP_EXCEPT` and ends it with

            179  except Exception:
            180  logger.exception(
            181      'Unhandled exception while attempting to load a guest list')

        [V], so anything this function raises is a **silently empty Guests
        tab** -- a defect that no amount of correct data in `svc.guests` would
        reveal.

        Instantiating the controller is safe and deliberate: `__init__`
        (`:53-64`) sets `_load_state`, a dict, a set and three
        `signals.Signal`s and nothing else [V] -- no UI, no service call, no
        `RegisterNotify` (that is `load`/`_subscribe`, `:86` and `:422`).
        The fallback route exists because `_get_station_guests` never reads
        `self` at all -- there is no `LOAD_FAST self` in its disassembly -- so
        its underlying function runs correctly on any argument.

        Note that this reaches `GetGuests` through the *cached* client service,
        as the panel does.  That is the point: by the time this runs the cache
        has just been filled from the wire, and what is under test here is the
        transform, not the fetch.
        """
        try:
            mod = __import__('eve.client.script.ui.shared.dockedUI.guests',
                             {}, {}, ['GuestsPanelController'])
        except Exception, e:
            return None, ('could not import'
                          ' eve.client.script.ui.shared.dockedUI.guests'
                          ' (%s: %s).  This is the panel\'s own module; a'
                          ' failure here is a client/harness problem and not'
                          ' a server one'
                          % (e.__class__.__name__, text(e)))
        cls = getattr_safe(mod, 'GuestsPanelController')
        if cls is None:
            return None, 'the panel module has no GuestsPanelController'
        try:
            return (cls()._get_station_guests(),
                    'GuestsPanelController()._get_station_guests()')
        except Exception, e:
            first = '%s: %s' % (e.__class__.__name__, text(e))
        method = getattr_safe(cls, '_get_station_guests')
        raw = getattr_safe(method, 'im_func', getattr_safe(method, '__func__'))
        if raw is None:
            return None, ('GuestsPanelController()._get_station_guests()'
                          ' raised %s and the underlying function could not'
                          ' be reached' % first)
        try:
            return (raw(None),
                    'GuestsPanelController._get_station_guests.im_func(None)'
                    ' -- the instance route raised %s first' % first)
        except Exception, e:
            return None, ('the panel\'s own row transform raised. Instance'
                          ' route: %s.  Unbound route: %s: %s.  A ValueError'
                          ' is the dict value arity -- guests.py:123 unpacks'
                          ' each value into exactly three fields (corpID,'
                          ' allianceID, warFactionID) -- and whatever it is,'
                          ' _load_guest_list swallows it into one log line'
                          ' (guests.py:179-181) and draws an empty tab'
                          % (first, e.__class__.__name__, text(e)))

    def eoci_body():
        # -- 1. are we docked, and where? ---------------------------------
        #
        # Waited for, not read: the trigger fires from the *first* session
        # change (`docs/lanes/ci-trigger.md`), so this runs earlier than any
        # player's lobby has finished drawing, and `stationid` arriving with
        # that very change is not something this scenario should assume.
        def docked():
            s = builtin('session')
            if s is None:
                return None
            return getattr_safe(s, 'stationid')

        stationid, waited = wait_for(docked, timeout_ms=30000, step_ms=500)
        session = builtin('session')
        if session is None:
            check('the session is reachable', False,
                  'session = None after %s ms' % text(waited))
            return
        charid = getattr_safe(session, 'charid')
        check('the session reports a station and a character',
              bool(stationid) and bool(charid),
              'session.stationid = %s, session.charid = %s,'
              ' session.solarsystemid = %s, after %s ms.  This scenario reads'
              ' the station lobby and deliberately does NOT undock:'
              ' Registry::characters_at_station excludes pilots in space'
              ' (service.rs:1725-1734), so undocking would empty the very'
              ' list under test'
              % (text(stationid), text(charid),
                 text(getattr_safe(session, 'solarsystemid')), text(waited)))
        if not stationid or not charid:
            return

        # -- 2. can the guest list be obtained at all? --------------------
        svc, svc_detail = station_svc()
        check('svc.station is reachable', svc is not None, svc_detail)
        if svc is None:
            return

        guests, how = fresh_guests(svc)
        check('svc.station.GetGuests() answered with a guest list',
              isinstance(guests, dict),
              'svc.station.GetGuests() = %s (%s).  %s.  StationSvc.GetGuests'
              ' returns self.guests, a dict keyed by charID'
              ' (station/base.py:139, :141)'
              % (census(guests) if isinstance(guests, dict) else text(guests),
                 type(guests).__name__, how))
        if not isinstance(guests, dict):
            return
        say('guests at station %s: %s' % (text(stationid), census(guests)))

        # -- 3. is the other character there? -----------------------------
        #
        # Polled with the cache busted on every pass, because this scenario
        # starts before the second client has selected a character: the
        # server only learns of them in `session_selected_character`
        # (`service.rs:1655`), and `characters_at_station` reads
        # `sessions.online_characters()` (`service.rs:1726`).  Re-asking is
        # the whole reason `fresh_guests` exists.
        # The rendezvous comes first and is recorded as its own check, because
        # "they were never both here" and "the server left them out" are
        # different findings with different owners, and on 2026-10-23 the
        # second was reported when the truth was the first.
        rendezvous_ok = False
        if EXPECT_CHARID:
            found, last, waited, polls, how = rendezvous(svc)
            rendezvous_ok = found is not None
            if isinstance(found, dict):
                guests = found
            elif isinstance(last, dict):
                # The last answer is the one worth reporting, not the first.
                guests = last

            if rendezvous_ok:
                check('both clients were in this station at the same time',
                      True,
                      'charID %s appeared among the guests of station %s after'
                      ' %s ms of re-asking (%s poll(s), each one clearing'
                      ' svc.guestListReceived first, so every one of them was'
                      ' a real sm.RemoteSvc(\'station\').GetGuests() and this'
                      ' is the server\'s answer rather than a cache).  %s ms'
                      ' is itself a measurement worth reading: 0 ms means this'
                      ' client got here last and the other was already'
                      ' standing in the lobby, a large number means it arrived'
                      ' while we watched.  This client now stays put for'
                      ' %s ms (LINGER_MS) before writing its verdict, so it'
                      ' cannot quit before the other one looks.  %s'
                      % (text(EXPECT_CHARID), text(stationid), text(waited),
                         text(polls), text(waited), text(LINGER_MS), how))
            else:
                check('both clients were in this station at the same time',
                      False,
                      'charID %s never appeared among the guests of station'
                      ' %s: %s poll(s) over %s ms, each one cache-busted, and'
                      ' the last answer was %s.  THE OTHER CLIENT NEVER'
                      ' APPEARED, SO THIS RUN MEASURED NOTHING -- this is not'
                      ' the same finding as "the server omitted them", and'
                      ' from inside one client the two cannot be told apart.'
                      '  Read the other client\'s verdict to decide which it'
                      ' was: if it failed this same check, the two clients'
                      ' simply never overlapped (the second one did not boot,'
                      ' or booted so late that %s ms of OTHER_TIMEOUT_MS ran'
                      ' out, or LINGER_MS is shorter than the skew between'
                      ' them) and nothing has been learned about the server;'
                      ' if it PASSED -- it saw us while we could not see it --'
                      ' then the server really did omit a character who was'
                      ' standing here, and that is a server bug.  The'
                      ' guest-list assertion below was not made, because there'
                      ' was nothing to assert it about.  %s'
                      % (text(EXPECT_CHARID), text(stationid), text(polls),
                         text(waited), census(guests),
                         text(OTHER_TIMEOUT_MS), how))

            # Only now, and only if the rendezvous held, is there anything to
            # assert about the server.  Membership is not shape, so the other
            # character's row is checked for the panel's arity too -- it is
            # built per character (`Registry::station_guest_frame`,
            # `service.rs:1689-1696`), so a row that is right for us is not
            # automatically right for them.
            if rendezvous_ok:
                other_row = guests.get(EXPECT_CHARID)
                other_len = None
                try:
                    other_len = len(other_row)
                except Exception:
                    pass
                check('the other character is in this station\'s guest list',
                      EXPECT_CHARID in guests and other_len == 3,
                      'guests[%s] = %s (len %s) among the guests of station %s'
                      ' after %s ms.  station/base.py:139 stores exactly'
                      ' (corpID, allianceID, warFactionID) per character and'
                      ' guests.py:123 unpacks exactly those three, so a row of'
                      ' any other width is a silently empty Guests tab.  Full'
                      ' census: %s'
                      % (text(EXPECT_CHARID), text(other_row),
                         text(other_len), text(stationid), text(waited),
                         census(guests)))

        # -- 4. are *we* there? -------------------------------------------
        #
        # A list that omits you is a different bug from one that omits them:
        # `world.rs:226`'s `GetGuests` includes the actor on purpose ("the
        # real lobby lists you among the guests"), and the push path
        # deliberately excludes them (`Registry::announce_station_guest`,
        # `service.rs:1659-1672`) -- so your own row can only have come from
        # the call, and theirs can have come from either.
        check('this character is in the guest list too', charid in guests,
              'session.charid = %s, guests = %s.  world.rs:226 includes the'
              ' actor deliberately; announce_station_guest excludes them'
              ' (service.rs:1659-1672), so the ego row proves the call and'
              ' not the push'
              % (text(charid), census(guests)))

        # -- 5. are the fields in the order the panel unpacks them? -------
        #
        # Membership is not shape.  A row of the right arity in the wrong
        # order passes every check above and draws a corp ticker where the
        # alliance belongs.  `session.corpid` is the cross-check the client
        # already has: 567 reads in 208 modules
        # (`findings/session-keys.tsv`), and the server builds the guest row
        # from `c.corporation_id` in field 1 (`service/world.rs`, and
        # identically in `Registry::station_guest_frame`,
        # `service.rs:1689-1696`).
        #
        # `warFactionID` is reported and not asserted: `world.rs` sends
        # `PyValue::None` for it unconditionally because we have no wars, so
        # a mismatch against the session would say nothing about this panel.
        row = guests.get(charid)
        corpid = getattr_safe(session, 'corpid')
        row_corp = None
        row_len = None
        try:
            row_len = len(row)
            row_corp = row[0]
        except Exception:
            pass
        check('the ego guest row\'s fields are in the panel\'s order',
              row_len == 3 and row_corp == corpid and corpid is not None,
              'guests[%s] = %s (len %s); field 0 = %s and session.corpid ='
              ' %s.  station/base.py:139 stores (corpID, allianceID,'
              ' warFactionID) and guests.py:123 unpacks exactly those three,'
              ' in that order.  session.allianceid = %s,'
              ' session.warfactionid = %s, both reported and not asserted'
              % (text(charid), text(row), text(row_len), text(row_corp),
                 text(corpid), text(getattr_safe(session, 'allianceid')),
                 text(getattr_safe(session, 'warfactionid'))))

        # -- 6. would the panel actually draw it? -------------------------
        rows, panel_detail = panel_guests()
        panel_ids = None
        if rows is not None:
            try:
                panel_ids = sorted([getattr_safe(g, 'character_id')
                                    for g in rows])
            except Exception:
                panel_ids = None
        # The other character is expected here only if the rendezvous held.
        # Otherwise this check would go red for the rendezvous's reason and a
        # run that measured nothing would show two failures with one cause --
        # the thing that made 2026-10-23's output read as a server bug.  A
        # failed rendezvous now produces exactly one red check, and it is the
        # one whose text says the run measured nothing.
        want = [charid]
        if EXPECT_CHARID and rendezvous_ok:
            want.append(EXPECT_CHARID)
        missing = []
        for who in want:
            if panel_ids is None or who not in panel_ids:
                missing.append(who)
        want_note = ''
        if EXPECT_CHARID and not rendezvous_ok:
            want_note = ('  charID %s is NOT expected here: the rendezvous'
                         ' above timed out, so this check is ego-only and the'
                         ' one red check in this verdict is the rendezvous.'
                         % text(EXPECT_CHARID))
        check('the lobby panel\'s own row transform builds a Guest for each'
              ' of us',
              rows is not None and not missing,
              'GuestsPanelController._get_station_guests() -> %s Guest(s)'
              ' with character_id %s; expected %s, missing %s.  Route: %s%s'
              % (text(len(rows)) if rows is not None else 'no',
                 text(panel_ids), text(want), text(missing), panel_detail,
                 want_note))

        if not EXPECT_CHARID:
            check('the guest census was taken', True,
                  'guests of station %s: %s.  EXPECT_CHARID is 0, so no other'
                  ' character is asserted and only the ego-side facts above'
                  ' are -- see docs/lanes/ci-scenarios.md section 5. Run'
                  ' `tools/client-ci.sh run lobby_guests --with-second --'
                  ' --character Alice --character Bob` for the two-player'
                  ' assertion' % (text(stationid), census(guests)))

        # -- 7. stay put, so the other client can see us ------------------
        #
        # **The second half of the rendezvous, and the half that was missing.**
        # Everything above is decided; this only keeps the session open.  It is
        # last on purpose: the linger's polls refill the client-side cache, and
        # the panel check above reads that cache, so lingering any earlier
        # would let a legitimately departed other character fail check 6.
        #
        # A single-client run never gets here -- `EXPECT_CHARID` is 0 -- and
        # neither does a run that failed its rendezvous: if the other client
        # was never seen in 90 s there is nobody to wait for, our verdict
        # already says the run measured nothing, and waiting longer would only
        # risk turning one honest red into a mismatched pair of verdicts.
        if EXPECT_CHARID and rendezvous_ok:
            lingered, polls, together, lost_at = linger_report(svc)
            check('this client stayed in the lobby for the other one to see'
                  ' it',
                  True,
                  'stayed in station %s for %s ms after the rendezvous'
                  ' (LINGER_MS), re-asking every %s ms: charID %s was still in'
                  ' the guest list on %s of %s poll(s)%s.  REPORT ONLY,'
                  ' nothing is asserted -- the other character disappearing'
                  ' partway through is the expected ending, because its own'
                  ' linger started before ours and runs out before ours.  The'
                  ' numbers are here to size LINGER_MS from a real run instead'
                  ' of from the estimate in its comment: gone on the first'
                  ' poll means the two lingers barely overlap and it should go'
                  ' up; present on all of them means it is comfortable.'
                  '  Without this wait, whichever client reaches the'
                  ' rendezvous first quits before the other looks, which is'
                  ' what invalidated every two-player verdict of 2026-10-23'
                  ' (docs/lanes/ci-harness.md section 11)'
                  % (text(stationid), text(lingered), text(LINGER_POLL_MS),
                     text(EXPECT_CHARID), text(together), text(polls),
                     ('' if lost_at is None
                      else ', first missing %s ms into the linger'
                           % text(lost_at))))

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
