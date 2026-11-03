# -*- coding: utf-8 -*-
"""Dump byte-exact Destiny full-state blobs out of the real EVE client.

    THIS FILE RUNS INSIDE THE CLIENT, under Stackless Python 2.7.
    It is not part of the server build and nothing in the repo imports it.

Why it exists
-------------
`docs/archive/destiny-and-the-ballpark.md` section 4.4: the Destiny state blob is
opaque binary produced and consumed by `_destiny.dll`, and most of the field
layout is inferred from disassembly.  But the client ships the *writer* as well
as the reader -- `destiny.Ballpark.WriteFullStateToStream` -- so one script run
inside the client produces a reference blob from the exact code that will parse
ours.  See `docs/reference/destiny-reference-blob.md` for deployment and triggering.

What it does
------------
1. Probes the three things that were unverified before this file existed:
   how to construct a bare `destiny.Ballpark`, which `AddBall` arity this
   build wants, and how to get the bytes back out of a `blue.MemStream`.
   Each probe is *validated*, not merely attempted -- the MemStream accessor
   has to return a known payload byte for byte before it is believed.
2. Writes a baseline blob holding one ball whose every field is a distinctive
   value, then a series of variants that each change exactly ONE field.
3. Diffs every variant against the baseline and, for each differing window,
   searches the blob for the probe value encoded as f64/f32/i64/i32/u32/u16.
   A hit labels that offset with that type and field name.  **That table is
   the deliverable**: it turns section 4.4's `[I]` rows into measurements.
4. Reads the baseline blob back through `ReadFullStateFromStream` into a fresh
   ballpark and records every `destiny.Ball` attribute, which maps AddBall
   arguments onto attribute names without touching a single byte offset.

Design rules, because a dump we cannot trust is worse than no dump
------------------------------------------------------------------
* It works on a **private** ballpark, created here.  It never adds, mutates or
  removes a ball in the live one; the only thing it does to the live ballpark
  is read it (one `WriteFullStateToStream`, which the client itself does once
  per tick in `Park.StoreState`).
* It never writes inside the client installation.  Output goes to
  `C:\\eve-logs\\destiny-ref\\`, beside the client log.
* Nothing is silent.  Every probe result, every skipped variant and every
  exception text lands in the report, the report is flushed to disk even when
  the run fails, and a failure is re-raised so the client logs the traceback.
* `EO_TOUCH_DESTINY_SETTINGS` is False and should stay False: the dumped
  layout depends on `destiny.settings`, and changing it mutates global state
  the live simulation is using.  The current values are recorded instead.

Every name defined here is prefixed `eo_`/`EO_` on purpose.  `/execfile` runs
this with `globals()` of the module that owns the command
(`eve.devtools.script.svc_console`), so unprefixed names would shadow its own.
"""

import os
import struct
import sys
import traceback

# ---------------------------------------------------------------------------
# configuration
# ---------------------------------------------------------------------------

#: Where the blobs and the report go.  Inside the Proton prefix this is
#: `drive_c/eve-logs/destiny-ref`, i.e. beside the client log that
#: `tools/client-log.sh` already reads.  Deliberately NOT in the client
#: installation.
EO_OUTDIR = 'C:\\eve-logs\\destiny-ref'

#: Leave False.  See the module docstring.
EO_TOUCH_DESTINY_SETTINGS = False

#: The live report object, published module-wide so the failure handler at the
#: bottom can flush whatever was gathered before the run died.  A probe that
#: raises must still leave its verdict lines on disk.
EO_REPORT = None

#: Header value we force into `Ballpark.currentTime` so the four header bytes
#: are unmistakable in a hex dump.
EO_TICK_BASE = 0x11223344
EO_TICK_ALT = 0x55667788

#: Baseline `AddBall` arguments.  Positional names follow the verified format
#: string `Ldffiiiiiddddddff` (and `...ffff` when dynamical orientation is on);
#: only `ballID`, the position triple and the velocity triple have verified
#: meanings, so the rest are named by position and labelled by the diff table.
EO_BASE = {
    'ballID': 0x0102030405060708,
    'a2_d': 1234.5,
    'a3_f': 56.25,
    'a4_f': 78.5,
    'a5_i': 1,
    'a6_i': 1,
    'a7_i': 1,
    'a8_i': 1,
    'a9_i': 1,
    'x': 101.0,
    'y': 202.0,
    'z': 303.0,
    'vx': 11.0,
    'vy': 22.0,
    'vz': 33.0,
    'a16_f': 44.5,
    'a17_f': 55.25,
    # only used when this build wants the 19-argument form
    'a18_f': 66.5,
    'a19_f': 77.25,
}

EO_ARG_ORDER = [
    'ballID', 'a2_d', 'a3_f', 'a4_f',
    'a5_i', 'a6_i', 'a7_i', 'a8_i', 'a9_i',
    'x', 'y', 'z', 'vx', 'vy', 'vz',
    'a16_f', 'a17_f',
]

#: Second ball, for finding the record stride and the end-of-list condition.
EO_BALLID_2 = 0x1112131415161718

#: One variant per AddBall argument: (argument, replacement value).
EO_ARG_VARIANTS = [
    ('ballID', 0x2122232425262728),
    ('a2_d', 4242.5),
    ('a3_f', 1337.25),
    ('a4_f', 2048.5),
    ('a5_i', 0),
    ('a6_i', 0),
    ('a7_i', 0),
    ('a8_i', 0),
    ('a9_i', 0),
    ('x', 1001.0),
    ('y', 2002.0),
    ('z', 3003.0),
    ('vx', 111.0),
    ('vy', 222.0),
    ('vz', 333.0),
    ('a16_f', 144.5),
    ('a17_f', 155.25),
]

#: One variant per `destiny.Ball` attribute, set with `setattr` after the ball
#: exists.  The names are read out of `_destiny.dll`'s attribute table
#: (0xa4fd0..0xa51c8 in this build) and are therefore exact, not guessed.
#: A read-only attribute raises, which is recorded rather than fatal.
EO_ATTR_VARIANTS = [
    ('mass', 4242.5),
    ('radius', 1337.25),
    ('maxVelocity', 2500.5),
    ('maxAngularVelocity', 3.25),
    ('isFree', 0),
    ('isGlobal', 0),
    ('isMassive', 0),
    ('isInteractive', 0),
    ('isCloaked', 1),
    ('isMoribund', 1),
    ('harmonic', 9.5),
    ('corporationID', 98000001),
    ('allianceID', 99000001),
    ('x', 1001.0),
    ('y', 2002.0),
    ('z', 3003.0),
    ('vx', 111.0),
    ('vy', 222.0),
    ('vz', 333.0),
    ('wx', 0.125),
    ('wy', 0.25),
    ('wz', 0.5),
    ('rx', 0.109375),
    ('ry', 0.21875),
    ('rz', 0.4375),
    ('rw', 0.875),
    ('yaw', 0.75),
    ('pitch', 0.3125),
    ('roll', 0.65625),
    ('Agility', 1.5),
    ('RotationalAgility', 2.5),
    ('speedFraction', 0.375),
    ('mode', 4),
    ('gotoX', 4004.0),
    ('gotoY', 5005.0),
    ('gotoZ', 6006.0),
    ('followId', EO_BALLID_2),
    ('followRange', 7007.0),
    ('ownerId', 90000001),
    ('effectStamp', 0x3132333435363738),
    ('newBubbleId', 0x4142434445464748),
    ('oldBubbleId', 0x5152535455565758),
    ('formationID', 11),
]

#: Attribute names to report after a round trip, in `_destiny.dll` table order.
EO_BALL_ATTRS = [
    'id', 'mass', 'radius', 'maxVelocity', 'maxAngularVelocity',
    'isFree', 'isGlobal', 'isMassive', 'isInteractive', 'isCloaked',
    'isMoribund', 'harmonic', 'corporationID', 'allianceID',
    'x', 'y', 'z', 'vx', 'vy', 'vz', 'wx', 'wy', 'wz',
    'rx', 'ry', 'rz', 'rw', 'yaw', 'pitch', 'roll',
    'Agility', 'RotationalAgility', 'speedFraction', 'mode',
    'gotoX', 'gotoY', 'gotoZ', 'followId', 'followRange', 'ownerId',
    'effectStamp', 'newBubbleId', 'oldBubbleId', 'formationID',
]

EO_BALLPARK_ATTRS = [
    'ego', 'probe', 'solarsystemID', 'isMaster', 'time', 'currentTime',
    'lagDamping', 'friction', 'warpSpeed', 'tickInterval', 'isRunning',
    'para1', 'para2', 'int1', 'int2',
]


# ---------------------------------------------------------------------------
# reporting
# ---------------------------------------------------------------------------

class EoReport(object):
    """Accumulates the report and flushes it to disk on demand.

    Flushing eagerly matters: if a later variant kills the tasklet we still
    want the probes and everything dumped so far.
    """

    def __init__(self, path):
        self.path = path
        self.lines = []

    def __call__(self, text=''):
        self.lines.append(text)

    def rule(self, title):
        self('')
        self('-- %s %s' % (title, '-' * max(4, 72 - len(title))))

    def flush(self):
        try:
            body = ('\r\n').join([eo_text(l) for l in self.lines]) + '\r\n'
            f = open(self.path, 'wb')
            try:
                f.write(body)
            finally:
                f.close()
            return True
        except Exception:
            return False


def eo_text(value):
    """Best-effort str() that cannot itself raise and break the report."""
    try:
        if isinstance(value, unicode):  # noqa: F821  (py2)
            return value.encode('ascii', 'replace')
        return str(value)
    except Exception:
        try:
            return repr(value)
        except Exception:
            return '<unprintable>'


def eo_exc():
    """One-line exception summary, for lines that are not worth a traceback."""
    exc_type, exc_value = sys.exc_info()[:2]
    try:
        name = getattr(exc_type, '__name__', eo_text(exc_type))
        return '%s: %s' % (name, eo_text(exc_value))
    except Exception:
        return '<unformattable exception>'


# ---------------------------------------------------------------------------
# bytes helpers
# ---------------------------------------------------------------------------

def eo_hexdump(data, indent='  '):
    out = []
    for base in range(0, len(data), 16):
        chunk = data[base:base + 16]
        hexpart = ' '.join(['%02x' % ord(c) for c in chunk])
        asciipart = ''.join([(c if 32 <= ord(c) < 127 else '.') for c in chunk])
        out.append('%s%04x  %-47s  %s' % (indent, base, hexpart, asciipart))
    if not out:
        out.append('%s<empty>' % indent)
    return out


def eo_diff_windows(a, b, merge_gap=7):
    """Byte ranges where `a` and `b` differ, runs merged across small gaps.

    A float64 whose two values share leading zero bytes produces several short
    runs; merging across up to `merge_gap` identical bytes keeps one field in
    one window, which is what the labelling step wants.
    """
    n = min(len(a), len(b))
    diffs = [i for i in range(n) if a[i] != b[i]]
    windows = []
    for i in diffs:
        if windows and i - windows[-1][1] <= merge_gap:
            windows[-1][1] = i + 1
        else:
            windows.append([i, i + 1])
    return [(s, e) for s, e in windows]


#: Encodings tried when labelling a window.  (name, struct format, width)
EO_ENCODINGS = [
    ('f64', '<d', 8),
    ('f32', '<f', 4),
    ('i64', '<q', 8),
    ('u64', '<Q', 8),
    ('i32', '<l', 4),
    ('u32', '<L', 4),
    ('u16', '<H', 2),
]


def eo_find_value(data, value):
    """Offsets where `value` appears, as each encoding in `EO_ENCODINGS`.

    Returns a list of `(offset, encoding_name, width)`.  Only encodings four
    bytes or wider are searched -- a one-byte 0 or 1 matches almost everywhere
    and would be noise rather than evidence.
    """
    hits = []
    for name, fmt, width in EO_ENCODINGS:
        if width < 4:
            continue
        try:
            needle = struct.pack(fmt, value)
        except Exception:
            continue
        start = 0
        while True:
            at = data.find(needle, start)
            if at < 0:
                break
            hits.append((at, name, width))
            start = at + 1
    hits.sort()
    return hits


def eo_window_label(base, variant, window, value):
    """Describe one differing window, labelled by encoding when possible."""
    start, end = window
    old = base[start:end]
    new = variant[start:end]
    text = ['      0x%04x..0x%04x (%d bytes)' % (start, end, end - start)]
    text.append('        base    %s' % ' '.join(['%02x' % ord(c) for c in old]))
    text.append('        variant %s' % ' '.join(['%02x' % ord(c) for c in new]))
    # 0 and 1 (and -1) occur all over a binary record, so searching for them
    # produces matches that are coincidence rather than evidence.  For those
    # the byte delta is the whole story anyway.
    try:
        small_int = isinstance(value, (int, long)) and -1 <= value <= 1  # noqa: F821
    except Exception:
        small_int = False
    labels = []
    fields = []
    if not small_int:
        for at, name, width in eo_find_value(variant, value):
            # Only interesting if the hit is inside, or adjacent to, this window.
            if at + 8 >= start and at <= end:
                labels.append('%s at offset 0x%04x' % (name, at))
                fields.append((at, width, name))
    if labels:
        text.append('        MATCH: the probe value encodes as %s' % ', '.join(labels))
        # The differing window can be narrower than the field -- two floats
        # often share leading bytes -- so print the whole field as both blobs
        # have it.  This is the line to write a serialiser from.
        for at, width, name in fields:
            text.append('        FIELD %s at 0x%04x..0x%04x:'
                        % (name, at, at + width))
            text.append('          base    %s'
                        % ' '.join(['%02x' % ord(c) for c in base[at:at + width]]))
            text.append('          variant %s'
                        % ' '.join(['%02x' % ord(c) for c in variant[at:at + width]]))
    elif end - start == 1:
        text.append('        1-byte field: 0x%02x -> 0x%02x' % (ord(old), ord(new)))
    elif small_int:
        text.append('        flag probe (0/1): encoding search skipped, read the'
                    ' byte delta above')
    else:
        text.append('        no encoding of the probe value lands here'
                    ' -- derived field, or a different width')
    return text


# ---------------------------------------------------------------------------
# the probes
# ---------------------------------------------------------------------------

def eo_make_ballpark(report):
    """Return a private `destiny.Ballpark`, or raise.

    `Ballpark.__init__` takes one optional bool in this build (format `|p`,
    stored at `Ballpark+0x2ba`), which is the `isMaster` attribute the
    full-state writer's bubble filter is gated on.  We want it False/absent:
    a client-side park writes every ball rather than one bubble's worth.
    """
    import blue
    import destiny

    attempts = [
        ('destiny.Ballpark()', lambda: destiny.Ballpark()),
        ('destiny.Ballpark(False)', lambda: destiny.Ballpark(False)),
        ("blue.classes.CreateInstance('destiny.Ballpark')",
         lambda: blue.classes.CreateInstance('destiny.Ballpark')),
    ]
    for label, make in attempts:
        try:
            park = make()
        except Exception:
            report('  ballpark ctor  %-50s FAILED  %s' % (label, eo_exc()))
            continue
        report('  ballpark ctor  %-50s ok' % label)
        return park, label
    raise RuntimeError('eo: could not construct a private destiny.Ballpark')


def eo_memstream_accessor(report):
    """Decide how to read bytes back out of a `blue.MemStream`, and prove it.

    `docs/archive/destiny-and-the-ballpark.md` section 4.4 guessed `ms.data` with
    `ms.Seek(0); ms.Read(ms.Size)` as a fallback.  Neither is right as
    written: `blue.dll` registers the stream methods `Read`/`read`,
    `Write`/`write`, `Seek`/`seek` and exactly two MemStream attributes,
    `size` and `position` -- lowercase, and no `data` at all.  Rather than
    trust that reading, every candidate below is tried against a known
    payload and the first one that returns it byte for byte wins.
    """
    import blue

    probe = '\x00\x01\x7f\x80\xfeeve-offline'

    def mk():
        ms = blue.MemStream()
        ms.Write(probe)
        return ms

    def by_seek_read_size(ms):
        ms.Seek(0)
        return ms.Read(ms.size)

    def by_seek_read_size_capital(ms):
        ms.Seek(0)
        return ms.Read(ms.Size)

    def by_seek_read_all(ms):
        ms.Seek(0)
        return ms.Read()

    def by_data(ms):
        return ms.data

    candidates = [
        ('Seek(0) + Read(ms.size)', by_seek_read_size),
        ('Seek(0) + Read()', by_seek_read_all),
        ('Seek(0) + Read(ms.Size)', by_seek_read_size_capital),
        ('ms.data', by_data),
    ]

    chosen = None
    for label, fn in candidates:
        try:
            got = fn(mk())
            got = str(got)
        except Exception:
            report('  memstream     %-50s FAILED  %s' % (label, eo_exc()))
            continue
        if got == probe:
            report('  memstream     %-50s ok, verified against a known payload'
                   % label)
            if chosen is None:
                chosen = (label, fn)
        else:
            report('  memstream     %-50s WRONG BYTES (%d of %d)'
                   % (label, len(got), len(probe)))
    if chosen is None:
        raise RuntimeError(
            'eo: no blue.MemStream accessor returned the probe payload; '
            'do not trust any blob this run would have produced')
    return chosen


def eo_addball_arity(report, park):
    """Return (nargs, caller) for this build's `destiny.Ballpark.AddBall`.

    `_destiny.dll` carries two format strings, selected at run time by a
    `destiny.settings` byte: `Ldffiiiiiddddddff` (17) and
    `Ldffiiiiiddddddffff` (19).  The client's own call sites pass 17.  Both are
    tried here so the run reports which one this client actually accepts
    instead of assuming.
    """

    def args17(vals):
        return [vals[k] for k in EO_ARG_ORDER]

    def args19(vals):
        return args17(vals) + [vals['a18_f'], vals['a19_f']]

    for nargs, build in ((17, args17), (19, args19)):
        probe = dict(EO_BASE)
        probe['ballID'] = 0x7f00000000000001
        try:
            park.ClearAll()
        except Exception:
            pass
        try:
            park.AddBall(*build(probe))
        except Exception:
            report('  AddBall       %-50s FAILED  %s'
                   % ('%d positional args' % nargs, eo_exc()))
            continue
        report('  AddBall       %-50s ok' % ('%d positional args' % nargs))
        try:
            park.ClearAll()
        except Exception:
            pass
        return nargs, build
    raise RuntimeError('eo: neither the 17- nor the 19-argument AddBall was accepted')


# ---------------------------------------------------------------------------
# dumping
# ---------------------------------------------------------------------------

class EoDumper(object):

    def __init__(self, report, outdir, park, read_bytes, build_args):
        self.report = report
        self.outdir = outdir
        self.park = park
        self.read_bytes = read_bytes
        self.build_args = build_args
        self.index = 0
        self.manifest = []

    def _write_file(self, name, data):
        path = os.path.join(self.outdir, name)
        f = open(path, 'wb')
        try:
            f.write(data)
        finally:
            f.close()
        return path

    def capture(self, title, prepare, tick=EO_TICK_BASE, ball_ids=None):
        """Build a park state with `prepare`, write it, save it, report it.

        `prepare(park)` returns a list of notes to print.  `ball_ids`, when
        given, selects `WriteBallsToStream` instead of the full-state writer.
        """
        import blue

        self.index += 1
        name = '%02d-%s' % (self.index, title)
        self.report.rule(name)

        try:
            self.park.ClearAll()
        except Exception:
            self.report('  ClearAll failed: %s' % eo_exc())
        try:
            self.park.currentTime = tick
        except Exception:
            self.report('  currentTime is NOT writable (%s) -- the header bytes'
                        ' will hold whatever the park already had' % eo_exc())
        try:
            self.report('  currentTime   %r (0x%08x)'
                        % (self.park.currentTime,
                           int(self.park.currentTime) & 0xFFFFFFFF))
        except Exception:
            self.report('  currentTime   <unreadable>')

        notes = []
        try:
            notes = prepare(self.park) or []
        except Exception:
            self.report('  SKIPPED: prepare raised %s' % eo_exc())
            return None
        for note in notes:
            self.report('  %s' % note)

        ms = blue.MemStream()
        try:
            if ball_ids is None:
                self.park.WriteFullStateToStream(ms)
            else:
                self.park.WriteBallsToStream(ms, ball_ids)
        except Exception:
            self.report('  SKIPPED: writer raised %s' % eo_exc())
            return None

        try:
            data = str(self.read_bytes(ms))
        except Exception:
            self.report('  SKIPPED: read-back raised %s' % eo_exc())
            return None

        try:
            path = self._write_file(name + '.bin', data)
        except Exception:
            self.report('  could not save the blob: %s' % eo_exc())
            path = '<unsaved>'

        self.report('  %d bytes -> %s' % (len(data), path))
        for line in eo_hexdump(data):
            self.report(line)
        self.manifest.append((name, len(data)))
        return data

    def base_ball_args(self, overrides=None, ballID=None):
        vals = dict(EO_BASE)
        if overrides:
            vals.update(overrides)
        if ballID is not None:
            vals['ballID'] = ballID
        return self.build_args(vals)

    def add_base_ball(self, park, overrides=None, ballID=None):
        args = self.base_ball_args(overrides, ballID)
        ball = park.AddBall(*args)
        return ball, args


def eo_describe_args(args):
    names = EO_ARG_ORDER + ['a18_f', 'a19_f']
    out = []
    for i, value in enumerate(args):
        label = names[i] if i < len(names) else 'arg%d' % (i + 1)
        out.append('    [%2d] %-8s %r' % (i + 1, label, value))
    return out


def eo_roundtrip(report, blob, read_bytes, ballID):
    """Feed the blob back through the client's own reader and report the ball.

    This is the part that makes the dump trustworthy: if
    `ReadFullStateFromStream` accepts our bytes and the ball comes back with
    the values we put in, the blob is valid by construction and the AddBall
    argument -> Ball attribute mapping is measured rather than guessed.
    """
    import blue
    import destiny

    report.rule('round trip through ReadFullStateFromStream')
    try:
        park = destiny.Ballpark()
    except Exception:
        try:
            park = blue.classes.CreateInstance('destiny.Ballpark')
        except Exception:
            report('  could not build a second ballpark: %s' % eo_exc())
            return
    ms = blue.MemStream()
    try:
        ms.Write(blob)
    except Exception:
        report('  MemStream.Write failed: %s' % eo_exc())
        return
    try:
        park.ReadFullStateFromStream(ms)
    except Exception:
        report('  REJECTED: ReadFullStateFromStream raised %s' % eo_exc())
        report('  the blob above is NOT acceptable to the client parser')
        return
    report('  accepted.')
    try:
        report('  currentTime after read: %r (wrote 0x%08x)'
               % (park.currentTime, EO_TICK_BASE))
    except Exception:
        report('  currentTime unreadable: %s' % eo_exc())
    try:
        ids = list(park.balls.keys())
    except Exception:
        ids = []
        report('  park.balls unreadable: %s' % eo_exc())
    report('  balls in the park after read: %r' % (ids,))
    ball = None
    try:
        ball = park.balls.get(ballID, None)
    except Exception:
        pass
    if ball is None:
        report('  ball %d is not in the reconstructed park' % ballID)
        return
    report('  ball %d reads back as:' % ballID)
    for attr in EO_BALL_ATTRS:
        try:
            report('    %-20s %r' % (attr, getattr(ball, attr)))
        except Exception:
            report('    %-20s <unreadable: %s>' % (attr, eo_exc()))


# ---------------------------------------------------------------------------
# environment
# ---------------------------------------------------------------------------

def eo_environment(report):
    import blue

    report('environment')
    try:
        import eveprefs
        report('  build         %s / %s' % (eveprefs.boot.version, eveprefs.boot.build))
    except Exception:
        report('  build         <unreadable: %s>' % eo_exc())
    try:
        report('  wallclock     %r' % blue.os.GetWallclockTime())
    except Exception:
        pass
    try:
        sess = sys.modules['__builtin__'].session  # noqa: F821
        report('  session       charid=%r shipid=%r solarsystemid=%r stationid=%r'
               % (sess.charid, sess.shipid, sess.solarsystemid, sess.stationid))
        report('  role          0x%016x' % (sess.role & 0xFFFFFFFFFFFFFFFF))
    except Exception:
        report('  session       <unreadable: %s>' % eo_exc())

    report('')
    report('destiny.settings -- the blob layout depends on these')
    try:
        import destiny
        config = destiny.settings.Get()
        for flag in ('useDynamicalOrientation',
                     'disableDynamicalOrientationForMissiles',
                     'useNewOrbit'):
            report('  %-40s %r' % (flag, getattr(config, flag, '<absent>')))
        report('  (useDynamicalOrientation also selects which AddBall format')
        report('   string _destiny.dll parses with, 17 args vs 19.)')
    except Exception:
        report('  <unreadable: %s>' % eo_exc())


def eo_live_ballpark(report, read_bytes, outdir):
    """Dump the live ballpark too, read-only, if there is one.

    `GetBallpark()` defaults to `doWait=False` and returns None unless
    `bpReady` -- so this costs nothing when the client has no park, and never
    spins the 30-second `WaitForBallpark` loop.
    """
    import blue

    report.rule('live ballpark (read-only)')
    try:
        michelle = sys.modules['__builtin__'].sm.GetService('michelle')  # noqa: F821
        park = michelle.GetBallpark()
    except Exception:
        report('  no michelle service: %s' % eo_exc())
        return
    if park is None:
        report('  michelle.GetBallpark() is None -- bpReady is False, or we are docked.')
        report('  Nothing to dump; the private-ballpark dumps above are unaffected.')
        return
    for attr in EO_BALLPARK_ATTRS:
        try:
            report('  %-20s %r' % (attr, getattr(park, attr)))
        except Exception:
            report('  %-20s <unreadable>' % attr)
    try:
        ids = list(park.balls.keys())
    except Exception:
        ids = []
    report('  balls                %d %r' % (len(ids), ids[:16]))
    ms = blue.MemStream()
    try:
        park.WriteFullStateToStream(ms)
        data = str(read_bytes(ms))
    except Exception:
        report('  WriteFullStateToStream on the live park raised %s' % eo_exc())
        return
    try:
        path = os.path.join(outdir, '99-live-ballpark.bin')
        f = open(path, 'wb')
        try:
            f.write(data)
        finally:
            f.close()
        report('  %d bytes -> %s' % (len(data), path))
    except Exception:
        report('  %d bytes, could not save: %s' % (len(data), eo_exc()))
    for line in eo_hexdump(data):
        report(line)
    if len(data) <= 8:
        report('')
        report('  NOTE: a blob this short means the live ballpark holds no balls.')
        report('  That is the expected state for our server today (see')
        report('  docs/archive/destiny-and-the-ballpark.md section 3) and it is why this')
        report('  script builds its own ballpark instead of dumping this one.')


def eo_notify(message):
    """Best-effort on-screen confirmation.  Never allowed to break the run."""
    try:
        from player_messaging.client.ui_message import prompt_player_fsd_dialog
        prompt_player_fsd_dialog('CustomInfo', {'info': message}, modal=0)
        return
    except Exception:
        pass
    try:
        import log
        log.LogError('eve-offline destiny dump:', message)
    except Exception:
        pass


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def eo_main():
    global EO_REPORT
    try:
        os.makedirs(EO_OUTDIR)
    except Exception:
        pass
    if not os.path.isdir(EO_OUTDIR):
        raise RuntimeError('eo: output directory does not exist: %s' % EO_OUTDIR)

    report = EO_REPORT = EoReport(os.path.join(EO_OUTDIR, '00-REPORT.txt'))
    report('eve-offline -- Destiny reference blob dump')
    report('==========================================')
    report('')
    report('Produced by tools/client-overrides/destiny_dump.py running inside the')
    report('real client.  Every blob beside this file came out of')
    report('destiny.Ballpark.WriteFullStateToStream in _destiny.dll.')
    report('')
    eo_environment(report)
    report.flush()

    report('')
    report('probes')
    park, park_ctor = eo_make_ballpark(report)
    accessor_label, read_bytes = eo_memstream_accessor(report)
    nargs, build_args = eo_addball_arity(report, park)
    report('')
    report('  resolved: ballpark=%s, memstream=%s, AddBall=%d args'
           % (park_ctor, accessor_label, nargs))
    if EO_TOUCH_DESTINY_SETTINGS:
        report('  WARNING: EO_TOUCH_DESTINY_SETTINGS is on; global settings were'
               ' changed by this run')
    report.flush()

    dumper = EoDumper(report, EO_OUTDIR, park, read_bytes, build_args)

    # 1. The header alone.  Proves the leading bytes without any ball noise.
    dumper.capture('empty-park', lambda p: ['no balls added'])
    report('  expected shape, from _destiny.dll 0x180096c70 and 0x1800969d0:')
    report('    byte 0      packet type; the writer emits 0 for a full state')
    report('                and 1 for WriteBallsToStream. The reader rejects')
    report('                anything > 1 with "Unknown packet type".')
    report('    bytes 1..5  Ballpark+0x1c as u32 LE -- compare against the')
    report('                currentTime line above, and against the next dump,')
    report('                which is the same park at tick 0x%08x' % EO_TICK_ALT)
    report('    then        self-delimiting ball records until end of stream')

    # 2. The header again with a different tick, to pin the header bytes.
    dumper.capture('empty-park-alt-tick', lambda p: ['no balls added'],
                   tick=EO_TICK_ALT)

    # 3. The baseline: one ball, every argument distinctive.
    def prepare_base(p):
        ball, args = dumper.add_base_ball(p)
        return ['one ball, baseline arguments:'] + eo_describe_args(args)

    base = dumper.capture('base-one-ball', prepare_base)
    if base is None:
        report('')
        report('FATAL: the baseline dump failed, so no variant can be compared.')
        report.flush()
        raise RuntimeError('eo: baseline dump failed; see the report')

    # 4. Two balls, for the record stride and the list terminator.
    def prepare_two(p):
        ball1, args1 = dumper.add_base_ball(p)
        ball2, args2 = dumper.add_base_ball(p, ballID=EO_BALLID_2)
        return ['two balls: 0x%016x and 0x%016x'
                % (EO_BASE['ballID'], EO_BALLID_2),
                'the second differs from the first only in ballID,',
                'so the record stride is the offset between the two ids.']

    dumper.capture('two-balls', prepare_two)

    # 5. The same single ball through WriteBallsToStream: packet type 1, which
    #    is the form AddBalls/AddBalls2 consume with read mode 2.
    def prepare_selected(p):
        dumper.add_base_ball(p)
        return ['one ball, written with WriteBallsToStream([ballID])']

    dumper.capture('write-balls-to-stream', prepare_selected,
                   ball_ids=[EO_BASE['ballID']])

    # 6. One variant per AddBall argument.
    report.rule('AddBall argument variants')
    report('  Each dump below changes exactly one AddBall argument from the')
    report('  baseline. The diff localises that argument in the record.')
    for argname, value in EO_ARG_VARIANTS:
        def prepare(p, argname=argname, value=value):
            ball, args = dumper.add_base_ball(p, overrides={argname: value})
            return ['%s = %r (baseline %r)' % (argname, value, EO_BASE[argname])]

        data = dumper.capture('arg-%s' % argname, prepare)
        if data is None:
            continue
        eo_report_diff(report, base, data, value, 'AddBall argument %s' % argname)
        report.flush()

    # 7. One variant per destiny.Ball attribute.
    report.rule('destiny.Ball attribute variants')
    report('  Same method, driven through setattr on the Ball object rather')
    report('  than through AddBall, which reaches fields AddBall cannot set.')
    for attr, value in EO_ATTR_VARIANTS:
        def prepare(p, attr=attr, value=value):
            ball, args = dumper.add_base_ball(p)
            before = getattr(ball, attr, '<unreadable>')
            setattr(ball, attr, value)
            after = getattr(ball, attr, '<unreadable>')
            return ['ball.%s: %r -> %r (set %r)' % (attr, before, after, value)]

        data = dumper.capture('attr-%s' % attr, prepare)
        if data is None:
            continue
        eo_report_diff(report, base, data, value, 'Ball.%s' % attr)
        report.flush()

    # 8. Mode and flag changes that only a Ballpark method can make.  Each
    #    mutator carries the exact native signature where it is known; the
    #    ones that raise print the parser's own complaint, which names the
    #    arity or the type it wanted.
    report.rule('Ballpark method variants')
    report('  A failure here is still evidence: _destiny.dll answers a bad')
    report('  call with "Function expects %i arguments" or "Could not match')
    report('  argument %i to expected type: %s", which names the signature.')
    for label, mutate in eo_mutators():
        def prepare(p, label=label, mutate=mutate):
            ball, args = dumper.add_base_ball(p)
            second, _ = dumper.add_base_ball(p, ballID=EO_BALLID_2)
            notes = mutate(p, EO_BASE['ballID'], EO_BALLID_2)
            return ['two balls (%s needs a target), then %s'
                    % (label, label)] + list(notes or [])

        data = dumper.capture('call-%s' % label, prepare)
        if data is None:
            continue
        report('    (compare against 04-two-balls.bin, not the baseline:')
        report('     this variant has two balls)')
        report.flush()

    # 9. Round trip, and the live park.
    eo_roundtrip(report, base, read_bytes, EO_BASE['ballID'])
    report.flush()
    eo_live_ballpark(report, read_bytes, EO_OUTDIR)

    report.rule('manifest')
    for name, size in dumper.manifest:
        report('  %-40s %6d bytes' % (name + '.bin', size))

    report('')
    report('How to use this, from docs/reference/destiny-reference-blob.md:')
    report('  * 03-base-one-ball.bin is the value to put in `bag.state` for a')
    report('    one-ball SetState, with ballID 0x%016x.' % EO_BASE['ballID'])
    report('  * Every MATCH line above is a field offset you can write a Rust')
    report('    serialiser against. Lines without a MATCH are the ones still')
    report('    owed an explanation.')
    report('  * The round-trip section proves the client parser accepts these')
    report('    bytes. If it says REJECTED, nothing else here is trustworthy.')
    report.flush()

    try:
        park.ClearAll()
    except Exception:
        pass

    message = ('Destiny reference dump: %d blobs in %s'
               % (len(dumper.manifest), EO_OUTDIR))
    eo_notify(message)
    try:
        import log
        log.LogError('eve-offline destiny dump:', message)
    except Exception:
        pass
    return message


def eo_report_diff(report, base, variant, value, what):
    if len(variant) != len(base):
        report('    LENGTH CHANGED: %d -> %d bytes. %s changes the record'
               ' size, so the field is optional or length-prefixed.'
               % (len(base), len(variant), what))
    windows = eo_diff_windows(base, variant)
    if not windows:
        report('    no byte differs from the baseline. %s is either not'
               ' serialised, or the write was rejected silently.' % what)
        return
    report('    %d differing window(s) for %s:' % (len(windows), what))
    for window in windows:
        for line in eo_window_label(base, variant, window, value):
            report(line)


def eo_mutators():
    """(label, mutate(park, ballID, otherID) -> notes) pairs.

    Only calls whose native format string was read out of `_destiny.dll` are
    passed with confidence; the rest are deliberate probes whose failure text
    is the result.
    """

    def stop(park, bid, other):
        park.Stop(bid)
        return ['Stop(%d) -- expect mode DSTBALL_STOP (2)' % bid]

    def follow(park, bid, other):
        # verified format 'LL|f': (srcID, dstID, optional range)
        park.FollowBall(bid, other, 500.0)
        return ['FollowBall(%d, %d, 500.0) -- format LL|f, expect mode'
                ' DSTBALL_FOLLOW (1) and followId set' % (bid, other)]

    def orbit(park, bid, other):
        park.Orbit(bid, other, 1000.0)
        return ['Orbit(%d, %d, 1000.0) -- expect mode DSTBALL_ORBIT (4)'
                % (bid, other)]

    def goto_point(park, bid, other):
        park.GotoPoint(bid, 4004.0, 5005.0, 6006.0)
        return ['GotoPoint -- expect mode DSTBALL_GOTO (0) and gotoX/Y/Z']

    def rigid(park, bid, other):
        park.SetBallRigid(bid, 1)
        return ['SetBallRigid -- expect mode DSTBALL_RIGID (11), which the'
                ' reader special-cases at 0x180098b1d']

    def troll(park, bid, other):
        park.SetBallTroll(bid, 1)
        return ['SetBallTroll -- expect mode DSTBALL_TROLL (8)']

    def cloak(park, bid, other):
        park.CloakBall(bid)
        return ['CloakBall']

    def speed_fraction(park, bid, other):
        park.SetSpeedFraction(bid, 0.375)
        return ['SetSpeedFraction(0.375)']

    def max_speed(park, bid, other):
        park.SetMaxSpeed(bid, 2500.5)
        return ['SetMaxSpeed(2500.5)']

    def harmonic(park, bid, other):
        park.SetBallHarmonic(bid, 9.5)
        return ['SetBallHarmonic(9.5)']

    def ball_radius(park, bid, other):
        park.SetBallRadius(bid, 1337.25)
        return ['SetBallRadius(1337.25)']

    def ball_mass(park, bid, other):
        park.SetBallMass(bid, 4242.5)
        return ['SetBallMass(4242.5)']

    def ball_velocity(park, bid, other):
        park.SetBallVelocity(bid, 111.0, 222.0, 333.0)
        return ['SetBallVelocity(111, 222, 333)']

    def ball_position(park, bid, other):
        park.SetBallPosition(bid, 1001.0, 2002.0, 3003.0)
        return ['SetBallPosition(1001, 2002, 3003)']

    def formation(park, bid, other):
        park.SetBallFormation(bid, 11)
        return ['SetBallFormation(11) -- expect mode DSTBALL_FORMATION (12)']

    # --- The four modes no earlier run ever asked for -------------------
    #
    # `docs/reference/destiny-reference-blob.md` §9.4 recorded SetBallFormation as a no-op
    # and left WARP, MISSILE, MUSHROOM and FORMATION as [V2] -- a third
    # party's decode of a different build.  The `hull-numbers` lane then read
    # `_destiny.dll`'s own method table, with CCP's docstrings in it, and the
    # answer was not that the simulation has to be running: the dumper
    # probed fifteen methods and **none of them was a warp, missile or
    # mushroom setter**.  The DLL exports `WarpTo`, `MissileFollow`,
    # `LaunchMissile`, `AddMushroom`, `LoadFormations` ("Load formation
    # static data") and `FormationFollow` ("Joins an existing formation").
    #
    # So these six probes are the whole experiment for the last four mode
    # tails.  Their arities are unknown, which is why each one tries a few
    # shapes: `eo_try_shapes` stops at the first that does not raise, and a
    # run where every shape raises is still the deliverable, because
    # `_destiny.dll` answers a bad call by naming the signature it wanted.

    def eo_try_shapes(park, method, shapes):
        """Call `method` with each candidate argument tuple until one works.

        Returns notes describing every attempt. Nothing is silent: a shape
        that raised is reported with the native error, which is the text that
        names the real arity.
        """
        fn = getattr(park, method, None)
        if fn is None:
            return ['%s -- NOT EXPORTED by this build (getattr returned None)'
                    % method]
        notes = []
        for args in shapes:
            try:
                fn(*args)
            except Exception:
                notes.append('  %s%s raised: %s'
                             % (method, eo_describe_args(args), eo_exc()))
                continue
            notes.append('  %s%s ACCEPTED -- this is the shape the blob below'
                         ' reflects' % (method, eo_describe_args(args)))
            return notes
        notes.append('  %s: every shape raised; the errors above name the'
                     ' arity and types it wanted' % method)
        return notes

    def warp_to(park, bid, other):
        # A warp is a destination plus, probably, a range and/or a speed.
        return ['WarpTo -- expect mode DSTBALL_WARP (3) and the warp tail,'
                ' which is [V2] only today'] + eo_try_shapes(park, 'WarpTo', [
            (bid, 7007.0, 8008.0, 9009.0),
            (bid, 7007.0, 8008.0, 9009.0, 1500.0),
            (bid, 7007.0, 8008.0, 9009.0, 1500.0, 3.0),
            (bid, other),
            (bid, other, 1500.0),
        ])

    def missile_follow(park, bid, other):
        return ['MissileFollow -- expect mode DSTBALL_MISSILE (5)'
                ] + eo_try_shapes(park, 'MissileFollow', [
            (bid, other),
            (bid, other, 500.0),
            (bid, other, 500.0, 1.0),
        ])

    def launch_missile(park, bid, other):
        # Not a mode setter but the natural way to *get* a missile ball, and
        # cheap to ask while we are here.
        return ['LaunchMissile -- a second route to DSTBALL_MISSILE (5)'
                ] + eo_try_shapes(park, 'LaunchMissile', [
            (bid, other),
            (bid, other, 500.0),
            (bid, other, 500.0, 1.0, 1.0),
        ])

    def add_mushroom(park, bid, other):
        return ['AddMushroom -- expect mode DSTBALL_MUSHROOM (6)'
                ] + eo_try_shapes(park, 'AddMushroom', [
            (bid,),
            (bid, 1.0),
            (bid, 1001.0, 2002.0, 3003.0),
            (bid, 1001.0, 2002.0, 3003.0, 1.0),
        ])

    def formation_prereqs(park, bid, other):
        """`LoadFormations` then `FormationFollow`, then the setter again.

        §9.4's open question. `SetBallFormation` returned a blob byte-identical
        to the baseline with the mode unchanged, and the DLL's docstrings say
        why that is not surprising: formations have static data to load and a
        formation to join before a ball can be assigned one.
        """
        notes = ['LoadFormations + FormationFollow + SetBallFormation --'
                 ' §9.4\'s open question, and the three calls in the order'
                 ' the DLL\'s own docstrings imply']
        notes += eo_try_shapes(park, 'LoadFormations', [(), (0,), (1,)])
        notes += eo_try_shapes(park, 'FormationFollow', [
            (bid, other),
            (bid, other, 0),
            (bid, other, 0, 500.0),
        ])
        notes += eo_try_shapes(park, 'SetBallFormation', [(bid, 11), (bid, 0)])
        notes.append('  if the mode byte is still 2 after all three, the'
                     ' prerequisite is something else again -- say so rather'
                     ' than guessing a fourth call')
        return notes

    return [
        ('Stop', stop),
        ('FollowBall', follow),
        ('Orbit', orbit),
        ('GotoPoint', goto_point),
        ('SetBallRigid', rigid),
        ('SetBallTroll', troll),
        ('CloakBall', cloak),
        ('SetSpeedFraction', speed_fraction),
        ('SetMaxSpeed', max_speed),
        ('SetBallHarmonic', harmonic),
        ('SetBallRadius', ball_radius),
        ('SetBallMass', ball_mass),
        ('SetBallVelocity', ball_velocity),
        ('SetBallPosition', ball_position),
        ('SetBallFormation', formation),
        # The four tails that were never asked for. Last, so that a crash in
        # one of them costs nothing that was already working.
        ('WarpTo', warp_to),
        ('MissileFollow', missile_follow),
        ('LaunchMissile', launch_missile),
        ('AddMushroom', add_mushroom),
        ('FormationPrereqs', formation_prereqs),
    ]


# `/execfile` and `execfile(path, globals())` both run this file as a module
# body, so the call below is the entry point.  Any exception is reported to
# disk, logged with its traceback, and then re-raised: a dump that failed
# halfway must never look like a dump that worked.
try:
    EO_RESULT = eo_main()
except Exception:
    EO_TRACEBACK = traceback.format_exc()
    try:
        if EO_REPORT is not None:
            EO_REPORT('')
            EO_REPORT('RUN FAILED -- everything above is what had been gathered'
                      ' when it died:')
            EO_REPORT('')
            EO_REPORT(EO_TRACEBACK)
            EO_REPORT.flush()
    except Exception:
        pass
    try:
        failure = EoReport(os.path.join(EO_OUTDIR, '00-FAILED.txt'))
        failure('eve-offline destiny dump FAILED')
        failure('')
        failure(EO_TRACEBACK)
        failure.flush()
    except Exception:
        pass
    try:
        import log
        log.LogError('eve-offline destiny dump FAILED:', EO_TRACEBACK)
    except Exception:
        pass
    eo_notify('Destiny reference dump FAILED -- see the client log and '
              '%s\\00-FAILED.txt' % EO_OUTDIR)
    raise
