# -*- coding: utf-8 -*-
"""Make the client's own writer tell us the DSTBALL_WARP state-blob tail.

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.

`destiny::Mode` encodes five of the client's nine ball modes and deliberately
refuses the other four.  `WARP`, `MISSILE`, `FORMATION` and `MUSHROOM` have
tails that are `[V2]` only -- a third party's decode of a different build --
and writing one on a guessed layout corrupts every record after it, because
the native parser consumes each tail by mode.  The private-ballpark dumper
could not settle them: `mode` is a read-only property and `destiny.Ballpark`
exposes no native entry point that puts a ball into warp.

`docs/lanes/round2-spec.md` section 8 item 3 named the way out and the
precondition: *"once the push path works, the client becomes the oracle --
push a `WarpTo` action, then dump the live park with `WriteFullStateToStream`.
The client's own writer then tells us the layout its own reader expects."*
The push path works (port-tracker section 36), and `throttle_and_warp` already
drives a real ship into `mode == 3`.  So this scenario is that plan, run.

**The measurement is a size difference, which is how every other tail in
`destiny::Mode` was measured.**  Two dumps of the same park, the same ball
count, differing only in the ego ball's mode: STOP has no tail, so

    len(warp dump) - len(parked dump) == the WARP tail, in bytes

and the saved bytes give the tail's contents.  `GOTO` was measured exactly
this way from `69-call-GotoPoint.bin` and came out 24.

Artefacts, read off the host after the run:

    c:\\eve-logs\\client-ci\\warp_blob-parked.bin
    c:\\eve-logs\\client-ci\\warp_blob-warping.bin

Decoding them is a desk job and deliberately not done here: a scenario that
asserted a layout would be asserting the thing it is supposed to discover.
"""

EOCI_SCENARIO = 'warp_blob'

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

    #: `const.minWarpDistance` (`eve/common/lib/appConst.pyc:1747`).  A target
    #: nearer than this is not offered "Warp To" by the client at all.
    MIN_WARP_DISTANCE = 150000.0

    #: `DSTBALL_WARP`.  Asserted against the `destiny` module rather than
    #: trusted, below.
    WARP_MODE = 3

    #: `DSTBALL_STOP`, the baseline that has no tail.
    STOP_MODE = 2

    #: The client's own align watchdog is 180 ticks (three minutes,
    #: port-tracker section 34.7).  Well under it distinguishes "aligning
    #: forever" from "aligning normally", which is sub-second to a few seconds.
    WARP_SETTLE_MS = 25000

    def modes():
        """`{value: name}` for `destiny.DSTBALL_*`, read off the module."""
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

    def why():
        """The last line of the current exception, for a `detail` string."""
        try:
            return traceback.format_exc().strip().split('\n')[-1]
        except Exception:
            return '<unknown>'

    def memstream_reader():
        """`(label, fn)` that reads a `blue.MemStream` back, proved first.

        `blue.dll` registers `size` and `position` **lowercase** and no `data`
        attribute at all, so the obvious spellings are not all real.  Rather
        than trust one, every candidate is run against a known payload and the
        first that returns it byte for byte wins -- the same discipline
        `tools/client-overrides/destiny_dump.py` arrived at.
        """
        import blue
        probe = '\x00\x01\x7f\x80\xfeeve-offline'

        def mk():
            ms = blue.MemStream()
            ms.Write(probe)
            return ms

        def by_size(ms):
            ms.Seek(0)
            return ms.Read(ms.size)

        def by_read_all(ms):
            ms.Seek(0)
            return ms.Read()

        def by_size_capital(ms):
            ms.Seek(0)
            return ms.Read(ms.Size)

        def by_data(ms):
            return ms.data

        for label, fn in (('Seek(0) + Read(ms.size)', by_size),
                          ('Seek(0) + Read()', by_read_all),
                          ('Seek(0) + Read(ms.Size)', by_size_capital),
                          ('ms.data', by_data)):
            try:
                if str(fn(mk())) == probe:
                    return label, fn
            except Exception:
                continue
        return None, None

    def dump_park(park, reader, tag):
        """`(bytes, note)` -- the park through the client's own writer."""
        import blue
        if reader is None:
            return None, 'no verified MemStream reader'
        ms = blue.MemStream()
        try:
            park.WriteFullStateToStream(ms)
        except Exception:
            return None, 'WriteFullStateToStream raised: %s' % why()
        try:
            data = str(reader(ms))
        except Exception:
            return None, 'reading the stream back raised: %s' % why()
        try:
            path = os.path.join(OUTDIR, 'warp_blob-%s.bin' % tag)
            fh = open(path, 'wb')
            try:
                fh.write(data)
            finally:
                fh.close()
            note = '%d bytes -> %s' % (len(data), path)
        except Exception:
            note = '%d bytes, could not save: %s' % (len(data), why())
        return data, note

    def hexdump(data, limit=4096):
        """Lines of `offset  16 hex bytes  |ascii|`, for the log."""
        out = []
        n = min(len(data), limit)
        for base in range(0, n, 16):
            chunk = data[base:base + 16]
            hexes = ' '.join('%02x' % ord(c) for c in chunk)
            ascii_ = ''.join((c if 32 <= ord(c) < 127 else '.') for c in chunk)
            out.append('  %08x  %-47s  |%s|' % (base, hexes, ascii_))
        if len(data) > n:
            out.append('  ... %d more bytes, see the saved .bin'
                       % (len(data) - n))
        return out

    def le(data, off, n):
        """`n` bytes at `off` as an unsigned little-endian integer."""
        v = 0
        for k in range(n):
            v += ord(data[off + k]) << (8 * k)
        return v

    def i32(data, off):
        v = le(data, off, 4)
        return v - (1 << 32) if v >= (1 << 31) else v

    def i64(data, off):
        v = le(data, off, 8)
        return v - (1 << 64) if v >= (1 << 63) else v

    def f64(data, off):
        """IEEE754 binary64, by hand.

        `struct` is importable here, but doing it by hand costs ten lines and
        removes a module from the list of things that can be missing inside
        the client -- the same reason the prelude hand-rolls its JSON.
        """
        bits = le(data, off, 8)
        sign = -1.0 if bits >> 63 else 1.0
        exp = (bits >> 52) & 0x7ff
        frac = bits & ((1 << 52) - 1)
        if exp == 0x7ff:
            return sign * (1e308 * 10 if frac == 0 else 0.0)
        if exp == 0:
            return sign * frac * 2.0 ** (-1074)
        return sign * (1.0 + frac * 2.0 ** -52) * 2.0 ** (exp - 1023)

    def warpin_of(ball_id, centre):
        """The client's own warp-in point for `ball_id`, else `centre`.

        the ballpark's `GetWarpinPoint` (`michelle.py:1904`-`1917` [V]) dispatches
        on the group to `eveCfg`'s planet, sun and generic functions -- the
        same rule `warp_in.rs` ports.
        """
        try:
            p = builtin('sm').GetService('michelle').GetBallpark().GetWarpinPoint(
                ball_id)
            if p is not None and len(p) == 3:
                return (float(p[0]), float(p[1]), float(p[2]))
            say('GetWarpinPoint(%s) returned %s; using the centre'
                % (text(ball_id), text(p)))
        except Exception:
            say('GetWarpinPoint(%s) raised; using the centre: %s'
                % (text(ball_id), traceback.format_exc().splitlines()[-1]))
        return centre

    def ego_record_tail(data, ego, tail_len):
        """The ego ball's record tail, wherever the writer put it.

        This used to read record 0 and assume it was the ego. That held while
        the park was a handful of balls; with the content bundle (every belt
        rock in the system) the client's writer emits other balls first, and
        record 0 was a rock. So: find the ego's `ball_id` followed by the
        WARP mode byte, and read the record from there. The layout is the
        one below; only the starting offset is searched.
        """
        key = ''.join(chr((ego >> (8 * k)) & 0xff) for k in range(8))
        at = data.find(key + chr(WARP_MODE), 5)
        if at < 0:
            at = data.find(key, 5)
        return first_record_tail(data, tail_len, at if at >= 0 else 5)

    def first_record_tail(data, tail_len, off=5):
        """A ball record's tail, by `destiny::Park::encode`'s layout.

        5-byte header (packet kind, then the u32 park clock), then per ball:
        `ball_id` i64, `mode` u8, `radius` f32, position 3 x f64, `flags` u8,
        `mass` f64, `cloaked` u8, `harmonic` i64, `corporationID` i32,
        `allianceID` i32, then -- only when flag bit 0 is set -- `maxVelocity`
        f32, velocity 3 x f64, `Agility` f32, `speedFraction` f32, then the
        `formationID` byte, **then** the mode's tail.  Formation before tail
        is not a detail: reading them the other way round makes the tail start
        with `0xff` and look one byte longer than it is.
        """
        ball_id = i64(data, off)
        mode = ord(data[off + 8])
        flags = ord(data[off + 9 + 4 + 24])
        body = 8 + 1 + 4 + 24 + 1 + 8 + 1 + 8 + 4 + 4
        if flags & 1:
            body += 4 + 24 + 4 + 4
        tail_at = off + body + 1
        return ball_id, mode, flags, data[tail_at:tail_at + tail_len]

    def ball_fields(ball):
        """Everything native `WarpTo` is known to write, as the oracle.

        `space_state.rs`'s `Action::WarpTo` records that native `WarpTo`
        writes `effectStamp`, `followId`, `ownerId`, `followRange` and the
        goto target [V].  Those five plus the mode are what a WARP tail can
        possibly be carrying, so they are reported beside the bytes: the
        decode is then matching known values against offsets rather than
        guessing at field meanings.
        """
        names = ('mode', 'effectStamp', 'followId', 'ownerId', 'followRange',
                 'gotoX', 'gotoY', 'gotoZ', 'x', 'y', 'z', 'vx', 'vy', 'vz',
                 'speedFraction', 'maxVelocity', 'radius', 'mass')
        return dict((n, getattr_safe(ball, n)) for n in names)

    def eoci_body():
        # **Undock first**, for the reason `throttle_and_warp` records: there
        # is no ballpark in a hangar, and `/autoSelectCharacter:0` lands in
        # the station.
        _undocked, _detail = undock()
        check('undocked into space', _undocked, _detail)

        table = modes()
        check('the destiny mode constants are readable', len(table) >= 5,
              'destiny.DSTBALL_* resolved to %d values: %s'
              % (len(table), text(sorted(table.items()))))
        # The two mode numbers this scenario hard-codes, checked against the
        # client's own module so a renumbering fails here instead of silently
        # measuring the wrong delta.
        check('mode 3 is DSTBALL_WARP and mode 2 is DSTBALL_STOP',
              table.get(WARP_MODE) == 'DSTBALL_WARP'
              and table.get(STOP_MODE) == 'DSTBALL_STOP',
              'destiny says 3=%s and 2=%s'
              % (text(table.get(WARP_MODE)), text(table.get(STOP_MODE))))

        session = builtin('session')
        shipid = getattr_safe(session, 'shipid') if session else None
        check('the session names a ship', shipid is not None,
              'session.shipid = %s' % text(shipid))

        park, ball = get_ego_ball()
        check('the ego ball is in the park', ball is not None,
              'ballpark.GetBall(ballpark.ego) = %s' % text(ball))
        if park is None or ball is None:
            return

        label, reader = memstream_reader()
        check('a MemStream reader was proved against a known payload',
              reader is not None,
              'chose %s.  Without one there is nothing to read the writer'
              " 's output back with" % text(label))
        if reader is None:
            return

        def ball_count():
            try:
                return len(park.balls)
            except Exception:
                return None

        # -- the baseline: the same park, ego parked ----------------------
        base_mode = getattr_safe(ball, 'mode')
        check('the baseline park has the ego ball in DSTBALL_STOP',
              base_mode == STOP_MODE,
              'ball.mode = %s straight after undocking.  The whole'
              ' measurement below is a size difference against a mode with NO'
              ' tail, so a baseline in any other mode makes the delta the'
              ' difference of two tails and not the WARP one'
              % mode_name(base_mode, table))

        # Let the park settle: ball_count() unchanged for 2 s, 15 s cap.
        seen = {'n': ball_count(), 'polls': 0}

        def park_settled():
            n = ball_count()
            if n != seen['n']:
                seen['n'], seen['polls'] = n, 0
            else:
                seen['polls'] += 1
            return seen['polls'] >= 8      # 8 polls x 250 ms

        wait_for(park_settled, timeout_ms=15000, step_ms=250)
        before_count = ball_count()
        parked, parked_note = dump_park(park, reader, 'parked')
        check('the client wrote the parked state', parked is not None,
              parked_note)
        say('parked park: %s balls, %s' % (text(before_count), parked_note))
        if parked is not None:
            for line in hexdump(parked, 512):
                say(line)

        # -- the warp -----------------------------------------------------
        rows = []
        try:
            rows = park.GetWarpinPoints() or []
        except Exception:
            rows = []
        best = None
        seen = []
        seen_ids = []
        by_id = {}
        for row in rows:
            try:
                targetID = row[0].id
                by_id[targetID] = row[0]
            except Exception:
                continue
            dist = None
            try:
                dist = park.GetSurfaceDist(shipid, targetID)
            except Exception:
                dist = None
            seen.append('%s(%s m)' % (text(targetID), fmt(dist, 0)))
            seen_ids.append(targetID)
            if dist is None or dist < MIN_WARP_DISTANCE:
                continue
            if best is None or dist > best[1]:
                best = (targetID, dist)
        check('a warpable celestial is in the park', best is not None,
              '%d warpable celestial(s): %s; need one at least %s m away'
              % (len(rows), text(seen), fmt(MIN_WARP_DISTANCE, 0)))
        if best is None:
            return
        targetID, target_distance = best

        mich = get_michelle()
        if mich is None:
            check('michelle is reachable', False, 'get_michelle() = None')
            return
        warp_sent = False
        try:
            mich.CmdWarpToStuff('item', targetID, minRange=0.0)
            warp_sent = True
        except Exception:
            warp_sent = False
        check('CmdWarpToStuff was accepted', warp_sent,
              'michelle.CmdWarpToStuff("item", %s, minRange=0.0), %s m away'
              % (text(targetID), fmt(target_distance, 0)))

        in_warp, waited = wait_for(
            lambda: getattr_safe(ball, 'mode') == WARP_MODE,
            timeout_ms=WARP_SETTLE_MS)
        check('the ego ball entered DSTBALL_WARP', bool(in_warp),
              'ball.mode = %s after %d ms'
              % (mode_name(getattr_safe(ball, 'mode'), table), waited))
        if not in_warp:
            return

        # -- the dump that is the point -----------------------------------
        fields = ball_fields(ball)
        warping, warping_note = dump_park(park, reader, 'warping')
        after_count = ball_count()
        still_warping = getattr_safe(ball, 'mode') == WARP_MODE
        check('the client wrote the warping state', warping is not None,
              warping_note)
        check('the ship was still in warp when the writer ran', still_warping,
              'ball.mode = %s immediately after WriteFullStateToStream.  A'
              ' warp that ended mid-dump would write a parked record and the'
              ' delta would be zero'
              % mode_name(getattr_safe(ball, 'mode'), table))
        say('warping park: %s balls, %s' % (text(after_count), warping_note))
        say('ego ball fields in warp: %s' % text(sorted(fields.items())))
        if warping is not None:
            for line in hexdump(warping, 1024):
                say(line)

        if parked is None or warping is None:
            return

        # **The ball count has to be equal or the delta is not a tail.** The
        # park gains and loses balls on its own -- another pilot, a drone, a
        # celestial streaming in -- and each record is about 100 bytes, which
        # would swamp a tail of a few dozen.
        check('the two dumps hold the same number of balls',
              before_count is not None and before_count == after_count,
              'parked %s balls, warping %s balls.  Unequal counts make the'
              ' size difference a record count difference, not a tail'
              % (text(before_count), text(after_count)))

        delta = len(warping) - len(parked)
        check('the warping state is larger than the parked one', delta > 0,
              'warping %d bytes - parked %d bytes = %+d.  STOP writes no tail'
              ' and WARP writes one, so this is the tail length in bytes --'
              ' the same measurement that gave GOTO 24 and ORBIT 12'
              % (len(warping), len(parked), delta))
        say('WARP TAIL = %+d bytes (warping %d - parked %d)'
            % (delta, len(warping), len(parked)))

        # -- and what the 52 bytes are ------------------------------------
        #
        # Decoded once by hand off the saved blobs and pinned here, so the
        # layout `destiny::Mode::Warp` encodes is checked against the client's
        # own writer on every run rather than once:
        #
        #   +0   f64  destination X   the target's own centre
        #   +8   f64  destination Y
        #   +16  f64  destination Z
        #   +24  i32  effectStamp     negative while aligning
        #   +28  f64  -1.0            [I]
        #   +36  f64  stop distance from the centre
        #   +44  i64  warp factor     warp speed in AU/s x 1000
        #
        # Every one of those except +28 is checked against a value this
        # scenario can obtain independently, which is what makes it a
        # measurement rather than a transcription.
        ball_id, mode_byte, flags, tail = ego_record_tail(warping, shipid, delta)
        check('the ego ball\'s record is in the blob, in warp',
              ball_id == shipid and mode_byte == WARP_MODE,
              'ego record: ball_id=%s mode=%s flags=0x%02x (session.shipid=%s)'
              % (text(ball_id), text(mode_byte), flags, text(shipid)))
        check('the tail is where the layout says it is', len(tail) == delta,
              'read %d of %d tail bytes after the formationID byte'
              % (len(tail), delta))
        if len(tail) != delta:
            return
        say('WARP tail bytes: %s'
            % ' '.join('%02x' % ord(c) for c in tail))

        target_ball = by_id.get(targetID)
        tx = getattr_safe(target_ball, 'x')
        ty = getattr_safe(target_ball, 'y')
        tz = getattr_safe(target_ball, 'z')
        dest = (f64(tail, 0), f64(tail, 8), f64(tail, 16))
        # **Since 1bd0a75 the server warps to the warp-in point, not the
        # centre** (`warp_in.rs`, after a warp to a planet landed the owner
        # on it). The oracle is the client's own computation of that point,
        # `michelle.GetWarpinPoint(ballID)` (`michelle.py:1904`-`1917` [V]),
        # with the centre as the fallback for a target that has none.
        aim = warpin_of(targetID, (tx, ty, tz))
        check('the tail opens with the destination: the client\'s own'
              ' warp-in point for the target',
              aim[0] is not None and abs(dest[0] - aim[0]) < 1.0
              and abs(dest[1] - aim[1]) < 1.0 and abs(dest[2] - aim[2]) < 1.0,
              'tail[0:24] = %s, warp-in point of %s per the client = %s,'
              ' centre (%s, %s, %s)'
              % (text(dest), text(targetID), text(aim), fmt(tx, 0),
                 fmt(ty, 0), fmt(tz, 0)))

        stamp_in_tail = i32(tail, 24)
        check('the i32 at +24 is the effectStamp the ball reports',
              stamp_in_tail == fields.get('effectStamp'),
              'tail[24:28] = %s, ball.effectStamp = %s.  Negative while'
              ' aligning: -stamp/180 is the progress bar'
              ' (spaceMgr._GetWarpProgress) [V]'
              % (text(stamp_in_tail), text(fields.get('effectStamp'))))

        unknown = f64(tail, 28)
        say('tail[28:36] = %s  -- [I], no independent oracle for it yet'
            % text(unknown))

        stop_at = f64(tail, 36)
        radius = getattr_safe(target_ball, 'radius')
        check('the f64 at +36 is the standoff from the warp-in point:'
              ' this warp\'s minRange, 0',
              abs(stop_at) < 1.0,
              'tail[36:44] = %s (target radius %s)'
              % (fmt(stop_at, 1), fmt(radius, 1)))

        factor = i64(tail, 44)
        park_warp = getattr_safe(park, 'warpSpeed')
        check('the i64 at +44 is the warp factor, in AU/s x 1000',
              factor > 0 and factor % 10 == 0,
              'tail[44:52] = %s (%s AU/s); ballpark.warpSpeed = %s'
              % (text(factor), fmt(factor / 1000.0, 3), text(park_warp)))

        # -- the second warp, which is what settles +36 -------------------
        #
        # The same ship, a DIFFERENT celestial, and a non-zero `minRange`.
        # If +36 is "radius + a constant" it moves with the new target's
        # radius alone; if it is "radius + constant + minRange" it moves with
        # both.  One run, two equations, and `minRange` is the only input a
        # player controls -- which makes it the field our own standoff has to
        # agree with.
        SECOND_MIN_RANGE = 50000.0
        other = None
        for candidate in seen_ids:
            if candidate != targetID:
                other = candidate
                break
        if other is None:
            check('a second warpable celestial exists', False,
                  'only %s is warpable, so +36 cannot be cross-checked'
                  % text(targetID))
            return
        sent2 = False
        try:
            mich.CmdWarpToStuff('item', other, minRange=SECOND_MIN_RANGE)
            sent2 = True
        except Exception:
            sent2 = False
        check('a second CmdWarpToStuff with a non-zero minRange was accepted',
              sent2, 'michelle.CmdWarpToStuff("item", %s, minRange=%s)'
              % (text(other), fmt(SECOND_MIN_RANGE, 0)))
        if not sent2:
            return

        other_ball = by_id.get(other)
        other_x = getattr_safe(other_ball, 'x')
        # Three seconds for the redirect to reach the ball. There is nothing
        # to poll for: the mode is already WARP and stays WARP, so a
        # `wait_for` would have no predicate that changes. The destination
        # check below is what proves the redirect actually landed.
        sleep_ms(3000)
        again, again_note = dump_park(park, reader, 'warping-2')
        check('the client wrote the redirected warp state', again is not None,
              again_note)
        if again is None:
            return
        _id2, mode2, _flags2, tail2 = ego_record_tail(
            again, shipid, len(again) - len(parked))
        check('the ship is still in warp after the redirect', mode2 == WARP_MODE
              and len(tail2) == delta,
              'ego record mode = %s, tail %d bytes (expected %d)'
              % (text(mode2), len(tail2), delta))
        if len(tail2) != delta:
            return
        say('WARP tail bytes (redirected): %s'
            % ' '.join('%02x' % ord(c) for c in tail2))
        dest2 = (f64(tail2, 0), f64(tail2, 8), f64(tail2, 16))
        stamp2 = i32(tail2, 24)
        stop_at2 = f64(tail2, 36)
        radius2 = getattr_safe(other_ball, 'radius')
        other_y = getattr_safe(other_ball, 'y')
        other_z = getattr_safe(other_ball, 'z')

        # **The destination field is not always the point we sent, and the
        # effectStamp says which it is.**  Our server sends the warp-in point
        # (since 1bd0a75; the centre before it) with the standoff, minRange,
        # in a separate field, so anything else in here is the client's own
        # doing.  The
        # first dump -- taken while still aligning, `effectStamp < 0` -- held
        # the centre to the metre.  This one, three seconds after a redirect,
        # held the centre pulled in by exactly `radius + 1000`, which is the
        # stop distance at +36: the arrival point.  So destiny rewrites the
        # goto target when the warp *activates*, and the stamp is the
        # discriminator rather than "first versus second command".
        gap = None
        aim2 = warpin_of(other, (other_x, other_y, other_z))
        if aim2[0] is not None:
            gap = ((dest2[0] - aim2[0]) ** 2 + (dest2[1] - aim2[1]) ** 2
                   + (dest2[2] - aim2[2]) ** 2) ** 0.5
        say('redirected tail: effectStamp %s, destination %s m from the'
            ' target centre, stop distance %s'
            % (text(stamp2), fmt(gap, 1), fmt(stop_at2, 1)))
        if stamp2 < 0:
            check('while aligning, the destination is the warp-in point we'
                  ' sent',
                  gap is not None and gap < 1.0,
                  'effectStamp %s (<0, still aligning) and the destination is'
                  ' %s m off the client\'s warp-in point -- it should be'
                  ' that point, which is what our WarpTo action carries'
                  % (text(stamp2), fmt(gap, 1)))
        else:
            check('once the warp activates, the destination is the arrival'
                  ' point', gap is not None and stop_at2 > 0
                  and abs(gap - stop_at2) < 1.0,
                  'effectStamp %s (>=0, activated) and the destination is %s m'
                  ' from the centre against a stop distance of %s m. Destiny'
                  ' has folded the standoff into the goto target, so a ship'
                  ' mid-warp reports where it will STOP and not what it was'
                  ' aimed at' % (text(stamp2), fmt(gap, 1), fmt(stop_at2, 1)))
        say('+36 FIRST  %s with radius %s minRange 0'
            % (fmt(stop_at, 1), fmt(radius, 1)))
        say('+36 SECOND %s with radius %s minRange %s'
            % (fmt(stop_at2, 1), fmt(radius2, 1), fmt(SECOND_MIN_RANGE, 1)))
        if radius is None or radius2 is None:
            check('both targets reported a radius', False,
                  'radii: %s and %s' % (text(radius), text(radius2)))
            return
        over1 = stop_at - radius
        over2 = stop_at2 - radius2
        say('+36 minus each radius: first %s, second %s'
            % (fmt(over1, 1), fmt(over2, 1)))
        # **Two readings, two hypotheses, and they cannot both hold.** If the
        # field is `radius + constant` the two overheads are equal; if it is
        # `radius + constant + minRange` they differ by exactly the minRange
        # asked for. Anything else means the field is not what this says, and
        # that is the failure worth having.
        constant_only = abs(over2 - over1) < 1.0
        with_min_range = abs(over2 - over1 - SECOND_MIN_RANGE) < 1.0
        check('+36 is each warp\'s minRange (the standoff from the warp-in'
              ' point)', abs(stop_at) < 1.0
              and abs(stop_at2 - SECOND_MIN_RANGE) < 1.0,
              'overhead above the radius: %s m at minRange 0, %s m at'
              ' minRange %s. Equal means the field excludes minRange;'
              ' differing by exactly the minRange means it includes it;'
              ' neither means it is not a standoff at all'
              % (fmt(over1, 1), fmt(over2, 1), fmt(SECOND_MIN_RANGE, 0)))
        say('+36 VERDICT: %s'
            % ('radius + %s m, minRange NOT included' % fmt(over1, 1)
               if constant_only else
               ('radius + %s m + minRange' % fmt(over1, 1)
                if with_min_range else 'neither hypothesis holds')))

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
