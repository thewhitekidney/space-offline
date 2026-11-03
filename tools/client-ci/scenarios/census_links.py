# -*- coding: utf-8 -*-
"""The census link hand-offs: two browser calls and the video index.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Packet CEN-4 of the census
    (`docs/guides/census.md`, `docs/lanes/cen4.md`); run it only through the census
    orchestrator, `tools/census.sh`.  Outside a census (`EVE_CENSUS=1` absent
    from the client's environment) it refuses: one failing check, nothing
    fired.

Besides the verdict it writes `c:\\eve-logs\\client-ci\\census_links.census.jsonl`
(the census block in the body says what is in it).

Covers R-PATCH N11 (`blue.os.ShellExecute`, and the client's own
`urlUtil.open_in_browser`) and P13 (`videoplayer.index.VideoIndex`, whose
loader is `urllib2`).  Every URL is an invented `.invalid` name.  Control:
two browser execs and a lookup for the video name.  Patched: no exec, and
the video request at the proxy or as a lookup.
"""

EOCI_SCENARIO = 'census_links'

# How long the orchestrator should wait for the verdict, in seconds
# (`client-ci.sh --timeout`); the default 600 is too short for some.
EOCI_CENSUS_TIMEOUT_S = 600

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

    def eoci_body():
        # -- census block: identical in every census_* scenario ------------
        # `tools/test_census_scenarios.py` pins that it is byte-identical in
        # all six; `docs/lanes/cen4.md` says why it is not in the prelude.
        #
        # Besides the verdict, each census scenario appends one JSON line per
        # step to `c:\eve-logs\client-ci\<name>.census.jsonl`, in the shape
        # `tools/census_canary.py` writes `RUN/canary.jsonl` (`event`, `t` in
        # host Unix ms, plus fields).  The orchestrator appends it to
        # `RUN/canary.jsonl` after the run.  It is written line by line and
        # fsynced, because two of these scenarios end with the client dying
        # and the verdict file never appearing.
        import time as census_time

        CENSUS_ENV = 'EVE_CENSUS'
        CENSUS_CONTROL_ENV = 'EVE_CENSUS_CONTROL'
        CENSUS_NOENV_ENV = 'EVE_CENSUS_NOENV'
        CENSUS_LOG = os.path.join(OUTDIR, '%s.census.jsonl' % EOCI_NAME)
        census_fresh = [True]

        def unix_ms():
            """Host wallclock in Unix ms: the clock `census_canary.py mark`
            uses.  Wine's `time.time()` is the host's own clock [I]."""
            try:
                return int(census_time.time() * 1000)
            except Exception:
                return None

        def jval(value):
            if value is None:
                return 'null'
            if value is True:
                return 'true'
            if value is False:
                return 'false'
            if type(value).__name__ in ('int', 'long'):
                return '%d' % value
            return jstr(value)

        def census_event(event, **fields):
            """Append one line; the first call of the run truncates the file,
            so a line from an earlier run can never be read as this one's."""
            fields['event'] = event
            if 't' not in fields:
                fields['t'] = unix_ms()
            line = '{%s}\n' % ', '.join(['%s: %s' % (jstr(k), jval(fields[k]))
                                        for k in sorted(fields)])
            mode = 'wb' if census_fresh[0] else 'ab'
            census_fresh[0] = False
            try:
                os.makedirs(OUTDIR)
            except Exception:
                pass
            try:
                f = open(CENSUS_LOG, mode)
                try:
                    f.write(line)
                    f.flush()
                    try:
                        os.fsync(f.fileno())
                    except Exception:
                        pass
                finally:
                    f.close()
            except Exception:
                say('could not append to %s' % CENSUS_LOG)
            say('census: %s' % line.strip())
            return line

        def census_guard():
            """True in a census run.  Anywhere else: one failing check, and
            nothing fires.  A census scenario fires canaries, hangs or kills
            the client on purpose, so it must not run because a loop over
            the directory (`tools/nightly.sh`) happened to reach it."""
            flag = os.environ.get(CENSUS_ENV, '')
            if flag != '1':
                # Still truncates the log, so no earlier run's lines survive.
                census_event('stage', stage='refused', scenario=EOCI_NAME)
                check('this is a census run', False,
                      '%s=%r in the client environment, not "1": refused, and'
                      ' nothing was fired.  Run it through tools/census.sh.'
                      % (CENSUS_ENV, flag))
                return False
            # `noenv` is what lets the report prove a no-env run really was
            # one: the launcher puts EVE_CENSUS_NOENV=1 in the client's env.
            census_event('stage', stage='begin', scenario=EOCI_NAME,
                         control=os.environ.get(CENSUS_CONTROL_ENV, '') == '1',
                         noenv=os.environ.get(CENSUS_NOENV_ENV, '') == '1')
            return True

        def outcome(fn):
            """Run one network attempt; its outcome as text, never raising.
            A timeout or a refusal is the expected outcome under the census
            sink, so an exception here is a result, not a failure."""
            try:
                value = fn()
                return 'returned %s' % text(value)[:160]
            except Exception as e:
                return 'raised %s: %s' % (e.__class__.__name__, text(e)[:160])

        def hold(ms, step_ms=5000):
            """Yield to the client for `ms` of measured wallclock.  Returns
            the milliseconds that really passed."""
            start = unix_ms()
            if start is None:
                sleep_ms(ms)
                return ms
            while True:
                left = ms - (unix_ms() - start)
                if left <= 0:
                    break
                if not sleep_ms(min(step_ms, left)):
                    break
            return unix_ms() - start
        # -- end of the census block -------------------------------------------

        # Invented names, one per hand-off, so the report can tell them apart
        # and none of them can be owned (`.invalid`).  `docs/lanes/cen4.md`
        # lists them for the path table.
        SHELL_URL = 'https://census-shell.census.invalid/'
        URLUTIL_URL = 'https://census-urlutil.census.invalid/'
        VIDEO_INDEX_URL = 'http://census-video.census.invalid/index.json'
        # After each browser hand-off: Wine runs `winebrowser`, which runs
        # `xdg-open`, after `ShellExecute` has already returned.
        HANDOFF_MS = 15000
        # The index loads on its own tasklet; give it this long.
        VIDEO_LOAD_MS = 30000

        if not census_guard():
            return

        # (1) N11, the native hand-off: `blue.os.ShellExecute(url)`
        # (`blue.dll` ShellExecuteExW; R-PATCH N11 [V]).
        try:
            import blue
            shell = blue.os.ShellExecute
            have_shell = True
        except Exception as e:
            shell = None
            have_shell = False
            census_event('stage', stage='shell-missing', error=text(e))
        check('blue.os.ShellExecute is reachable', have_shell, text(shell))
        if have_shell:
            census_event('stage', stage='shell', url=SHELL_URL)
            result = outcome(lambda: shell(SHELL_URL))
            census_event('stage', stage='shell-done', result=result)
            check('ShellExecute(url) was called', True, result)
            hold(HANDOFF_MS)

        # (2) N11 through the client's own helper:
        # `eve.common.script.util.urlUtil.open_in_browser(url)`, which is
        # `blue.os.ShellExecute(url)` and nothing else (urlUtil.pyc:55 [V]).
        try:
            from eve.common.script.util import urlUtil
            opener = urlUtil.open_in_browser
        except Exception as e:
            opener = None
            census_event('stage', stage='urlutil-missing', error=text(e))
        check('urlUtil.open_in_browser is reachable', opener is not None,
              text(opener))
        if opener is not None:
            census_event('stage', stage='urlutil', url=URLUTIL_URL)
            result = outcome(lambda: opener(URLUTIL_URL))
            census_event('stage', stage='urlutil-done', result=result)
            check('open_in_browser(url) was called', True, result)
            hold(HANDOFF_MS)

        # (3) P13, the video index: `videoplayer.index.VideoIndex(url)`
        # starts a tasklet whose `_load_index` reads an http URL with
        # `urllib2.urlopen` (index.pyc:37 [V]).  R-CENSUS asked for the
        # client's own index URL through the real window.  Neither exists in
        # this build: `VideoIndex.__init__(self, url, language_id=None)` has
        # no default URL, and no module but `videoplayer/index.pyc` names
        # `VideoIndex` [V, a byte search of the whole tree];
        # `VideoPlayerWindow` plays a path it is given and loads no index.
        # So the spec's fallback applies with an invented URL: the sink is
        # the client's own, the target is ours.
        try:
            from videoplayer import index as video_index
            factory = video_index.VideoIndex
        except Exception as e:
            factory = None
            census_event('stage', stage='video-missing', error=text(e))
        check('videoplayer.index.VideoIndex is reachable',
              factory is not None, text(factory))
        if factory is not None:
            census_event('stage', stage='video', url=VIDEO_INDEX_URL)
            made = []

            def build():
                made.append(factory(VIDEO_INDEX_URL))
                return made[-1]
            result = outcome(build)
            check('VideoIndex(url) was built', bool(made), result)
            if made:
                loaded, waited = wait_for(
                    lambda: getattr_safe(made[0], '_data') is not None,
                    timeout_ms=VIDEO_LOAD_MS)
                census_event('stage', stage='video-done',
                             loaded=bool(loaded), waited_ms=waited)
                # Not a pass/fail on the network: under the census nothing
                # answers, so "not loaded" is the expected outcome.
                say('VideoIndex loaded=%s after %d ms' % (bool(loaded), waited))

        check('the census log was written', os.path.exists(CENSUS_LOG),
              CENSUS_LOG)

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
