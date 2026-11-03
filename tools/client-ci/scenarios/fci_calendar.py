# -*- coding: utf-8 -*-
"""The calendar: a personal event with an invitee, a corporation event, listing, responses, edit, delete.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Lane `fci-services` (master plan 3.1);
    the server half is `service/calendar.rs` and `calendar.rs`.

Two clients: `tools/client-ci.sh run fci_calendar --with-second`.  The pilot
with the lower character id is A (founder and CEO), the other B (invited and
joined as a plain member).  Everything goes through the client's own
`svc.calendar` (`eveCalendarsvc.py`: `CreateNewEvent`, `GetEventsByMonthYear`,
`GetEventDetails`, `RespondToEvent`, `GetResponsesToEvent`, `DeleteEvent`'s
server call) and `sm.RemoteSvc('calendarMgr')` for what the window code
refuses before the server sees it:

0. A founds a corporation and invites B;
1. the server refuses an empty title, a past date, a date more than 12 months
   out and an alliance event from a pilot with no alliance, and none of the
   refusals adds an event to the month;
2. A creates a personal event inviting B, and a corporation event: A's month
   list holds both (title, owner, `flag` personal / corp), the details carry the
   description and A as creator, A's own response to the personal event is
   accepted;
3. B lists both (the corporation's by membership, the personal one by invitation),
   its own response to the personal event is undecided; B COUNTS the client's own
   `OnNewCalendarEvent` pushes, accepts the personal event and declines the
   corporation one, and the server refuses B an edit, a delete and a corporation
   event (a plain member holds no events role);
4. A reads both responses (`GetResponsesToEvent`), edits the corporation event's
   title (B counts `OnEditCalendarEvent` and lists the new title), then deletes the
   personal event (B counts `OnRemoveCalendarEvent`; the row is flagged deleted).

Nothing here asserts a duration: every wait is a bound.  No client log
traceback may appear.  A fresh world is a PRECONDITION: A starts in an NPC
corporation.

NOT here: the calendar window and the New Event form on screen (a human
check), alliance events with an alliance, the event-start notification
(`CheckForNewEvents_thread`), a server restart.
[A]s: `blue.os.GetWallclockTime()` in the client and the server's clock agree
to within a day (falsifier: the personal event is refused as past or the month
list misses it); the server and the client read the month in UTC (falsifier:
the month list misses an event made for a day that is the 1st or the last
of a month: the scenario picks a day that is not).
"""

EOCI_SCENARIO = 'fci_calendar'

# Seconds to pass to `tools/client-ci.sh run fci_calendar --timeout`.
EOCI_FCI_TIMEOUT_S = 1200

# SERVER-ARGS: --character Alice --character Bob
# SERVER-EXPECT: absent  UNANSWERED (calendarProxy|calendarMgr|corpRegistry)
# SERVER-EXPECT: absent  panicked at

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


    # The harness sets this to the OTHER client's character id
    # (`tools/client-ci.sh`: `^ *EXPECT_CHARID *= *0 *$`).
    EXPECT_CHARID = 0
    PEER_WAIT_MS = 300000           # a bound on the other client's boot, never a claim
    STEP_WAIT_MS = 180000           # a bound on one step crossing between the clients
    LINGER_MS = 20000
    TICKS_PER_DAY = 864000000000    # a FILETIME day, 100 ns ticks

    # -- the client's calendar, corporation and clock ----------------------

    def cal_svc():
        """`svc.calendar` (`eveCalendarsvc.py`) [V]."""
        return get_svc('calendar')

    def corp_svc():
        return get_svc('corp')

    def corp_of(charid):
        ok, info = rcall('charMgr', 'GetPublicInfo', charid)
        return getattr_safe(info, 'corporationID') if ok else None

    def wall_filetime():
        import blue
        return int(blue.os.GetWallclockTime())

    def month_of(filetime):
        """`(month, year)` UTC of a FILETIME, the pair `GetEventList` takes."""
        import datetime
        d = datetime.datetime(1601, 1, 1) + datetime.timedelta(microseconds=int(filetime) // 10)
        return d.month, d.year

    def pick_day(base):
        """A FILETIME 3 to 6 days from `base` whose UTC day is neither the 1st nor
        the last two of a month, so a UTC/local difference cannot move it."""
        import datetime
        for extra in range(3, 10):
            ft = base + extra * TICKS_PER_DAY
            d = datetime.datetime(1601, 1, 1) + datetime.timedelta(microseconds=ft // 10)
            if 2 <= d.day <= 26:
                return ft
        return base + 3 * TICKS_PER_DAY

    def month_rows(ft):
        """The client's own view of the month `ft` falls in, fetched fresh
        (`FlushCache` drops its per-month cache), or None."""
        svc = cal_svc()
        if svc is None:
            return None
        safe(lambda: svc.FlushCache())
        month, year = month_of(ft)
        ok, rows = safe(lambda: svc.GetEventsByMonthYear(month, year))
        return list(rows) if ok else None

    def find_event(ft, title):
        rows = month_rows(ft) or []
        for r in rows:
            if getattr_safe(r, 'eventTitle') == title:
                return r
        return None

    def raw_count(ft):
        return len(month_rows(ft) or [])

    class Tally(object):
        """Counts the client's own calendar pushes, so a push the server never
        sends (or sends under another name) is a failed check."""
        __notifyevents__ = ['OnNewCalendarEvent', 'OnEditCalendarEvent', 'OnRemoveCalendarEvent']

        def __init__(self):
            self.new = 0
            self.edit = 0
            self.remove = 0

        def OnNewCalendarEvent(self, *args, **kw):
            self.new += 1

        def OnEditCalendarEvent(self, *args, **kw):
            self.edit += 1

        def OnRemoveCalendarEvent(self, *args, **kw):
            self.remove += 1

    def start_tally():
        tally = Tally()
        ok, v = safe(lambda: builtin('sm').RegisterNotify(tally))
        return tally, ('ok' if ok else 'RegisterNotify: ' + text(v))

    def found_corp():
        """`(corp_id, detail)`: a corporation A is CEO of."""
        gm('/giveisk 3000000000')
        gm('/giveskill Corporation Management 1')
        sfx = uniq(4)
        old = getattr_safe(builtin('session'), 'corpid')
        ok, v = rcall('corpRegistry', 'AddCorporation', 'Fci Calendar ' + sfx, 'C' + sfx,
                      'f-ci', '', 0.0, 1, 1, 1, 1, 1, 1, True, False, 0.0)
        if not ok:
            return None, 'AddCorporation: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') != old,
                           timeout_ms=30000)
        if not got:
            return None, 'session.corpid never changed from %s' % text(old)
        return getattr_safe(builtin('session'), 'corpid'), 'founded'

    def join_corp_of_issuer(me):
        """B: take the invitation A sent (status 8) and accept it (status 2)."""
        def invited():
            ok, apps = rcall('corpRegistry', 'GetMyApplications')
            if not ok:
                return None
            for corpid in list(apps.keys()):
                for a in apps[corpid]:
                    if a.status == 8:
                        return a
            return None
        app, waited = wait_for(invited, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        if not app:
            return None, 'no invitation after %s ms' % text(waited)
        ok, v = rcall('corpRegistry', 'UpdateApplicationOffer', app.applicationID, me,
                      app.corporationID, '', 2)
        if not ok:
            return None, 'UpdateApplicationOffer: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') == app.corporationID,
                           timeout_ms=30000)
        return (app.corporationID, 'joined') if got else (None, 'session.corpid never became %s' % text(app.corporationID))

    def statuses_to(event_id, owner):
        """`{characterID: status}` from `GetResponsesToEvent`, or None."""
        ok, rows = rcall('calendarMgr', 'GetResponsesToEvent', event_id, owner)
        if not ok:
            return None
        got = {}
        for r in list(rows):
            got[r.characterID] = r.status
        return got

    def my_status(event_id):
        ok, rows = rcall('calendarMgr', 'GetResponsesForCharacter')
        if not ok:
            return None
        for r in list(rows):
            if r.eventID == event_id:
                return r.status
        return 0

    # -- the two phases ----------------------------------------------------

    def phase_a(me, other):
        svc = cal_svc()
        precondition('the client calendar service runs', svc is not None, text(svc))
        if svc is None:
            return
        tag_p = const_of('calendarTagPersonal', 1)
        tag_c = const_of('calendarTagCorp', 2)
        st_accept = const_of('eventResponseAccepted', 4)
        st_decline = const_of('eventResponseDeclined', 2)
        a_corp = getattr_safe(builtin('session'), 'corpid')
        precondition('A is in an NPC corporation (a fresh world)',
                     a_corp is not None and a_corp < 98000000, text(a_corp))
        corp, why = found_corp()
        check('A founds a corporation', corp is not None, why)
        if corp is None:
            return
        ok, v = rcall('corpRegistry', 'InsertInvitation', other)
        check('A invites B to the corporation (InsertInvitation)', ok, text(v)[:200])
        joined, waited = wait_for(lambda: corp_of(other) == corp, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('B is a member of A\'s corporation', joined is True,
              'B in %s, A in %s, after %s ms' % (text(corp_of(other)), text(corp), text(waited)))
        if joined is not True:
            return
        now = wall_filetime()
        when = pick_day(now)
        before = raw_count(when)
        precondition('the month starts with no event of this run', find_event(when, 'Fci personal') is None)
        # 1. what the server must refuse
        ok, v = rcall('calendarMgr', 'CreatePersonalEvent', when, 60, '', 'x', 0, [])
        check('an empty title is refused', not ok, 'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'CreatePersonalEvent', now - TICKS_PER_DAY, 60, 'Fci past', 'x', 0, [])
        check('an event in the past is refused', not ok, 'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'CreatePersonalEvent', now + 400 * TICKS_PER_DAY, 60,
                      'Fci far', 'x', 0, [])
        check('an event more than 12 months ahead is refused', not ok, 'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'CreateAllianceEvent', when, 60, 'Fci alliance', 'x', 0)
        check('an alliance event from a pilot with no alliance is refused', not ok,
              'answered %s' % text(v)[:200])
        check('the refusals added no event to the month', raw_count(when) == before,
              '%d -> %d' % (before, raw_count(when)))
        # 2. create
        sfx = uniq(4)
        t_personal = 'Fci personal ' + sfx
        t_corp = 'Fci corp ' + sfx
        ok, v = safe(lambda: svc.CreateNewEvent(when, 60, t_personal, 'personal text ' + sfx,
                                                tag_p, 1, [other]))
        check('A creates a personal event inviting B (CreateNewEvent)', ok, text(v)[:200])
        ok, v = safe(lambda: svc.CreateNewEvent(when + TICKS_PER_DAY // 24, 30, t_corp,
                                                'corp text ' + sfx, tag_c, 0, []))
        check('A creates a corporation event (CreateNewEvent)', ok, text(v)[:200])
        ev_p, _w = wait_for(lambda: find_event(when, t_personal), timeout_ms=30000, step_ms=1000)
        ev_c, _w = wait_for(lambda: find_event(when, t_corp), timeout_ms=30000, step_ms=1000)
        check('A\'s month list holds the personal event, owned by A, flagged personal, important',
              ev_p is not None and ev_p.ownerID == me and ev_p.flag == tag_p
              and ev_p.importance == 1 and not ev_p.isDeleted, text(ev_p)[:300])
        check('A\'s month list holds the corporation event, owned by the corporation, flagged corp',
              ev_c is not None and ev_c.ownerID == corp and ev_c.flag == tag_c,
              text(ev_c)[:300])
        if ev_p is None or ev_c is None:
            return
        ok, det = safe(lambda: svc.GetEventDetails(ev_p.eventID, ev_p.ownerID))
        check('the details carry the description and A as the creator',
              ok and getattr_safe(det, 'eventText') == 'personal text ' + sfx
              and getattr_safe(det, 'creatorID') == me, text(det)[:300])
        check('A\'s own response to its personal event is accepted',
              my_status(ev_p.eventID) == st_accept, text(my_status(ev_p.eventID)))
        st = statuses_to(ev_p.eventID, me)
        check('the responses list A as accepted (B may have answered already)',
              st is not None and st.get(me) == st_accept, text(st))
        # 3. B responds; wait for both answers
        got, waited = wait_for(lambda: (statuses_to(ev_p.eventID, me) or {}).get(other) == st_accept,
                               timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('B\'s acceptance reaches A (GetResponsesToEvent)', bool(got),
              '%s after %s ms' % (text(statuses_to(ev_p.eventID, me)), text(waited)))
        got, waited = wait_for(lambda: (statuses_to(ev_c.eventID, corp) or {}).get(other) == st_decline,
                               timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('B\'s refusal of the corporation event reaches A', bool(got),
              '%s after %s ms' % (text(statuses_to(ev_c.eventID, corp)), text(waited)))
        # 4. edit the corporation event, delete the personal one
        new_title = t_corp + ' v2'
        ok, v = safe(lambda: svc.EditEvent(ev_c.eventID, ev_c.eventDateTime, ev_c.eventDateTime,
                                           30, new_title, 'corp text ' + sfx, tag_c, 0))
        check('A edits the corporation event\'s title (EditEvent)', ok, text(v)[:200])
        got, _w = wait_for(lambda: find_event(when, new_title), timeout_ms=30000, step_ms=1000)
        check('A\'s list shows the new title and the old one is gone',
              got is not None and find_event(when, t_corp) is None, text(got)[:200])
        ok, v = rcall('calendarMgr', 'DeleteEvent', ev_p.eventID, ev_p.ownerID)
        check('A deletes the personal event (calendarMgr.DeleteEvent)', ok, text(v)[:200])
        row = find_event(when, t_personal)
        check('the deleted event is flagged deleted or gone from A\'s list',
              row is None or bool(row.isDeleted), text(row)[:200])
        ok, v = rcall('calendarMgr', 'EditPersonalEvent', ev_p.eventID, ev_p.eventDateTime,
                      ev_p.eventDateTime, 60, 'Fci after delete', 'x', 0)
        check('a deleted event cannot be edited', not ok, 'answered %s' % text(v)[:200])

    def phase_b(me, other):
        svc = cal_svc()
        precondition('the client calendar service runs', svc is not None, text(svc))
        if svc is None:
            return
        tag_p = const_of('calendarTagPersonal', 1)
        tag_c = const_of('calendarTagCorp', 2)
        st_accept = const_of('eventResponseAccepted', 4)
        st_undecided = const_of('eventResponseUndecided', 3)
        st_decline = const_of('eventResponseDeclined', 2)
        tally, tally_why = start_tally()
        precondition('the client calendar pushes can be counted', tally_why == 'ok', tally_why)
        corp, why = join_corp_of_issuer(me)
        check('B joins A\'s corporation on A\'s invitation', corp is not None, why)
        if corp is None:
            return
        when = pick_day(wall_filetime())

        def pair():
            rows = month_rows(when) or []
            p = c = None
            for r in rows:
                t = text(getattr_safe(r, 'eventTitle'))
                if t.startswith('Fci personal ') and r.flag == tag_p:
                    p = r
                if t.startswith('Fci corp ') and r.flag == tag_c:
                    c = r
            return (p, c) if (p is not None and c is not None) else None
        got, waited = wait_for(pair, timeout_ms=PEER_WAIT_MS, step_ms=3000)
        check('B\'s month list holds A\'s personal event (invited) and the corporation event',
              got is not None, 'none after %s ms; %d rows' % (text(waited), raw_count(when)))
        if got is None:
            return
        ev_p, ev_c = got
        check('the personal event keeps A as owner (B is only an invitee)', ev_p.ownerID != me,
              text(ev_p)[:200])
        check('B hears OnNewCalendarEvent for both (the personal one by invitation)',
              tally.new >= 2, '%d pushes' % tally.new)
        check('B\'s own response to the personal event is undecided until it answers',
              my_status(ev_p.eventID) == st_undecided, text(my_status(ev_p.eventID)))
        # refusals: a plain member
        ok, v = rcall('calendarMgr', 'DeleteEvent', ev_p.eventID, ev_p.ownerID)
        check('the server refuses B deleting A\'s personal event', not ok, 'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'EditPersonalEvent', ev_p.eventID, ev_p.eventDateTime,
                      ev_p.eventDateTime, 60, 'Fci hijack', 'x', 0)
        check('the server refuses B editing A\'s personal event', not ok, 'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'CreateCorporationEvent', ev_p.eventDateTime, 30, 'Fci member corp', 'x', 0)
        check('the server refuses a corporation event from a member with no events role', not ok,
              'answered %s' % text(v)[:200])
        ok, v = rcall('calendarMgr', 'EditCorporationEvent', ev_c.eventID, ev_c.eventDateTime,
                      ev_c.eventDateTime, 30, 'Fci hijack', 'x', 0)
        check('the server refuses B editing the corporation event', not ok, 'answered %s' % text(v)[:200])
        check('none of that changed what B sees', find_event(when, ev_p.eventTitle) is not None
              and find_event(when, ev_c.eventTitle) is not None, '')
        # respond
        ok, v = safe(lambda: svc.RespondToEvent(ev_p.eventID, ev_p, st_accept))
        check('B accepts the personal event (RespondToEvent)', ok, text(v)[:200])
        ok, v = safe(lambda: svc.RespondToEvent(ev_c.eventID, ev_c, st_decline))
        check('B declines the corporation event (RespondToEvent)', ok, text(v)[:200])
        check('B\'s responses read back as accepted and declined',
              my_status(ev_p.eventID) == st_accept and my_status(ev_c.eventID) == st_decline,
              '%s %s' % (text(my_status(ev_p.eventID)), text(my_status(ev_c.eventID))))
        # A edits, then deletes
        got, waited = wait_for(lambda: tally.edit >= 1, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('B hears OnEditCalendarEvent for A\'s edit of the corporation event', bool(got),
              '%d pushes after %s ms' % (tally.edit, text(waited)))
        got, waited = wait_for(lambda: find_event(when, ev_c.eventTitle + ' v2') is not None,
                               timeout_ms=30000, step_ms=2000)
        check('B lists the new corporation title', bool(got), text(ev_c.eventTitle))
        got, waited = wait_for(lambda: tally.remove >= 1, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('B hears OnRemoveCalendarEvent for the deleted personal event', bool(got),
              '%d pushes after %s ms' % (tally.remove, text(waited)))
        row = find_event(when, ev_p.eventTitle)
        check('the deleted event is flagged deleted or gone from B\'s list',
              row is None or bool(row.isDeleted), text(row)[:200])

    def linger(ms):
        waited = 0
        while waited < ms:
            sleep_ms(2000)
            waited += 2000

    def eoci_body():
        mark = log_mark()
        sess = builtin('session')
        me = getattr_safe(sess, 'charid')
        precondition('the harness named the other pilot (run with --with-second)',
                     bool(EXPECT_CHARID), 'EXPECT_CHARID %s' % text(EXPECT_CHARID))
        if not EXPECT_CHARID:
            return
        is_a = me < EXPECT_CHARID
        say('calendar role: %s (me %s, other %s)' % ('A' if is_a else 'B', text(me), text(EXPECT_CHARID)))
        if is_a:
            phase_a(me, EXPECT_CHARID)
        else:
            phase_b(me, EXPECT_CHARID)
        linger(LINGER_MS if not is_a else 5000)
        no_new_tracebacks('calendar', mark)

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
