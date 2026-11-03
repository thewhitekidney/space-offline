# Handing the client executable code

This is the one category of answer where our reply is not *data* the client
reads but *instructions* the client runs. There are exactly two such answers
in the server today, and this document is about what makes them different
from every other arm in `service/`.

Read `docs/archive/marshal-tags.md` first and then put it down: **none of it applies
here.** The format in this document is CPython's own `marshal`, and it shares
nothing with `blue.marshal` — not a type byte, not a length convention, not
the dict pair order. The two meet at exactly one point: a code object
produced here travels inside a wire-marshal `Bytes` (Python 2 `str`), which
`blue.marshal` treats as an opaque blob. `server/crates/eve-server/src/pycode.rs`
is the only place the CPython format is written, and it imports nothing from
`eve-marshal`.

## The two sites

| call | consumer | what it does with our bytes |
|---|---|---|
| handshake H8 `signedFunc` | `GPS.__Execute` | `marshal.loads` then `eval` |
| `charMgr.GetSettingsInfo` | `settingsSvc.UpdateSettingsStatistics` | `marshal.loads` then **installs as `SettingsInfo.__code__`** and calls it |

The second is the stronger form. The first is an expression evaluated in a
frame whose exceptions `GPS.__Execute` swallows into a capture buffer. The
second replaces the body of a module-level function in the live client and
then calls it, with nothing catching anything closer than
`MollycoddledUthread`.

Both are gated by `evecrypto` — and in this build `cryptoPack` is **Placebo**,
so both gates are identities. `evecrypto/placebo.py(22)`:

```text
def verify(signedData):
    return (signedData[0], signedData[1] == 0)
```

There is no signature, no key, no hash. `placebo.sign` is `return (data, 0)`,
which is why the answer shape is the 2-tuple `(blob, 0)`: the client's own
signer produced that pair and its own verifier unpacks it. We are not
bypassing a check; the check does not exist in a Placebo build.

## Why this is safe here and would not be elsewhere

The client is jailed (`docs/guides/jail.md`) and the only server it can reach is ours.
Nothing in this path touches the network, and the blobs are compile-time
constants in Rust with no input from the wire — there is no path by which a
client-supplied value reaches the bytecode we emit. That matters, because the
mechanism is otherwise exactly an RCE primitive pointed at the player's
machine.

The practical rule: **a code push is a constant, never a template.** If a
future arm needs to vary the code it sends, that is the point to stop and
write down why, because a code object assembled from anything the client said
is a different kind of thing from these two.

## What `GetSettingsInfo` has to match

`SettingsInfo.__code__ = ...` is an attribute assignment on a function
object, and CPython 2.7's `func_set_code` validates exactly one thing:

```text
nfree    = PyCode_GetNumFree(value)          # len(co_freevars)
nclosure = len(func.__closure__ or ())
if nclosure != nfree:
    ValueError: SettingsInfo() requires a code object with N free vars, not M
```

So **`co_freevars` must be empty**, because `SettingsInfo` is a module-level
`def` with no closure. `co_argcount` is *not* checked by the assignment; it is
checked by the call on the next line, `ret = SettingsInfo()`, which needs 0.
Everything else is unvalidated and will simply be believed — including
`co_stacksize`, which the interpreter trusts when it sizes the frame, so it
is not a field to guess low.

The original, read out of the real `.pyc` with `tools/pyc27.py`
(`carbonui/services/settingsSvc.py`, `firstlineno` 231):

```text
argcount 0  nlocals 0  stacksize 1  flags 0x43
code     'd\x00\x00S'        LOAD_CONST 0 ; RETURN_VALUE
consts   (None,)
names () varnames () freevars () cellvars ()
name     'SettingsInfo'
```

`def SettingsInfo(): pass`. The real body lives on CCP's cluster, is pushed at
runtime, and is therefore **not recoverable from anything we have** — the
client ships the socket, not the payload.

Ours differs in three fields and nothing else:

```text
code     'i\x00\x00S'        BUILD_MAP 0 ; RETURN_VALUE
consts   ()                  nothing to load, so nothing to resolve
filename '<eve-offline settings stub>'
```

`BUILD_MAP` takes a two-byte argument that is only a presizing hint for
`_PyDict_NewPresized` and pops nothing, so `0` is both a valid argument and
the honest one. `flags` stays `0x43`
(`CO_OPTIMIZED | CO_NEWLOCALS | CO_NOFREE`), byte for byte the original's.

Why an empty dict and not `None`: the client's next line is
`if len(ret) > 0`, so `None` is a `TypeError` at `len` and anything non-empty
invents telemetry and sends the client back with `LogSettings`. `{}` is the
one answer that is both truthful ("this server collects no settings
statistics") and terminal.

## How a code object gets checked when there is no Python 2.7

There is none anywhere in this project — not on the host, where nothing is
ever installed, and not in the build container — and Python 3 cannot
`marshal.loads` a 2.7 code object, so `tools/analysis.sh` cannot help by
running it either. What it *can*
do is **parse** it: `tools/pyc27.py` is a read-only 2.7 marshal reader that
already parses this exact format out of the client's own `.pyc` files, which
makes feeding it our blob a real test rather than a mirror of our own
assumptions.

```sh
tools/analysis.sh python3 -I -c "
import sys; sys.path.insert(0,'/work/tools')
from pyc27 import Unmarshal, Code, dis
b = bytes.fromhex('<the hex from the failing test>')
u = Unmarshal(b, 0); c = u.load()
assert u.p == len(b), (u.p, len(b))      # no trailing junk, no short read
print([(f, getattr(c, f)) for f in Code.__slots__])
print(list(dis(c)))
"
```

The `u.p == len(b)` assertion is the part worth keeping: a marshal reader
stops when the object is complete and never notices trailing bytes, so a
stream that is one byte too long parses perfectly and is still wrong.

The result is pinned as golden hex in `pycode.rs`'s tests, next to a
field-by-field reader written separately from the encoder — deliberately not
sharing code with it, because an encoder checked by its own inverse agrees
with itself about any mistake they share.

**What this does not prove**, and cannot from here: that the real
`marshal.loads` accepts the bytes, that `PyCode_New`'s internal checks pass,
and that the interpreter runs the opcodes as expected. A live client settles
all three at once. Until then, a failure here looks like
`ValueError: bad marshal data` or a `TypeError` from `PyCode_New`, raised at
`settingsSvc.py(208)` and arriving by mail.

## The log entry this fix is based on

The arm answered `None` for several runs. The comment on it predicted the
cost — "exactly one isolated log record" — and was half right in a way worth
recording, because it is a general shape of mistake.

```text
TypeError: 'NoneType' object is not subscriptable
  /packages/carbonui/services/settingsSvc.py(215) OnCharacterSessionChanged
  /packages/carbonui/services/settingsSvc.py(205) UpdateSettingsStatistics
  /packages/evecrypto/placebo.py(22) verify     signedData = None
```

Right: 215 is `OnCharacterSessionChanged`, which runs once per session when
`oldCharacterID is None`, under `MollycoddledUthread`. One traceback, not a
live lock.

Wrong: there was nothing graceful about it. The prediction reasoned about
`UpdateSettingsStatistics`, which does test `verified` — but `placebo.verify`
subscripts `signedData[0]` *before* returning, so the throw happens two
frames lower, before any of the client's own handling is reached. **When
judging what a bad answer costs, read the frame that touches the value
first, not the frame that looks like it checks it.**

And the only reason we saw it at all is that the client mails us its
tracebacks (`docs/guides/debugging-the-client.md` §3.c2, `evecap --alerts`): a call we
answer wrongly leaves no trace in any server-side instrument, which is §3a.
Had this arm been left unregistered instead, `UNANSWERED` would have named it
in one line.
