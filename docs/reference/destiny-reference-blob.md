# Getting a reference Destiny blob out of the client

`docs/archive/destiny-and-the-ballpark.md` §4.4 names the longest-lead item in the project:
the Destiny state blob is a C++ serialisation in `_destiny.dll` with no Python
source to port, and most of its field layout is `[I]` — read off the
disassembly and matched to attribute names by eye.

It also names the lever. **The client ships the writer, not just the reader.**
`destiny.Ballpark.WriteFullStateToStream` is right there next to
`ReadFullStateFromStream`, in the same binary that will parse whatever we send.
So the layout does not have to be *derived*; it can be *measured*.

This document is that lever, made usable: a script, where to put it, how to
fire it, and what comes back.

> **Nothing here modifies the client installation.** The script lives outside
> it, in the Proton prefix beside the client's own log, and it is executed by a
> command the client already ships. `/client` stays read-only and
> `tools/client-prepare.sh` / `tools/client-patch-manifest.sh` are not involved.
> Deploying is the owner's step, not an agent's — nothing in this repo copies
> anything into a client.

Contents: 0 verified vs inferred, 1 the four unknowns and what settled them,
2 deploy, 3 trigger, 4 output, 5 why not an import override, 6 the empty
ballpark, 7 corrections to §4.4, 8 failure modes.

---

## 0. What is verified, and what is not

`[V]` is read out of this build's bytecode or out of `_destiny.dll`/`blue.dll`
at a named address. `[I]` is inferred. **Nothing below has been confirmed by a
client run yet** — the script has never been executed, because that needs a
live client. Its *inputs* are verified; its output is the thing that will
verify the rest.

**Verified:**

- `blue.MemStream` exposes the methods `Read`/`read`, `Write`/`write`,
  `Seek`/`seek` and exactly two attributes, **`size`** and **`position`**
  (lowercase). There is no `data` and no `Size` (§1.1).
- A full-state blob begins with a **1-byte packet type**, not the 4-byte
  integer §4.4 records. The writer emits `0`; `WriteBallsToStream` emits `1`;
  the reader rejects anything `> 1` with `"Unknown packet type"` (§1.3).
- `ReadFullStateFromStream(stream[, mode])` and `WriteFullStateToStream(stream
  [, ballID])` both **`Seek(0)` on the stream themselves** before touching it
  (§1.1).
- `destiny.Ballpark.AddBall` has **two** signatures in this binary,
  `Ldffiiiiiddddddff` (17 args) and `Ldffiiiiiddddddffff` (19), selected at
  call time by a `destiny.settings` byte. The client's own call sites pass 17
  (§1.4).
- `Ballpark.__init__` takes one optional bool, stored at `Ballpark+0x2ba`,
  which is the `isMaster` attribute; the full-state writer's bubble filter is
  gated on it, so a client-side park writes **every** ball (§1.4).
- The full-state writer **skips any ball whose id is `< -0x40000000`** — the
  client-side ball range. `Park.AddClientSideBall` balls never serialise (§1.4).
- `svc.console` carries `__slashhook__ = True` and a `cmd_execfile` that runs
  `execfile(filename, globals())` behind a `ROLE_PROGRAMMER` check, and our
  session mask has that bit (§1.2).
- The exact `destiny.Ball` and `destiny.Ballpark` Python attribute names, read
  out of `_destiny.dll`'s own tables rather than guessed (§7.3).

**Inferred:**

- `[I]` That the `destiny.settings` byte at `0x1800d42d1` is specifically
  `useDynamicalOrientation`. What is verified is that it is written by the
  function that raises `"Argument 1 is not a SettingsConfiguration"` — i.e. by
  `destiny.settings.Configure` — and that `SettingsConfiguration` has exactly
  three flags, of which that is the first.
- `[I]` The meaning of `AddBall` arguments 2, 3, 4, 16 and 17 and of the five
  int flags. The *types* are verified from the format string; the names are
  not. **The dump settles these**, which is the point.
- `[I]` That the client will let a `ROLE_PROGRAMMER` slash command through
  while in space. The role test is verified; the UI route to a text field that
  accepts slash commands is read off `insider.py` and `neocomSvc.py` and has
  not been walked by hand.

---

## 1. The four unknowns, and what settled them

### 1.1 `blue.MemStream`'s read-back accessor — settled, and the old guess was wrong

§4.4 guessed `ms.data`, "strings in `blue.dll`: `MemStream/mData`", with
`ms.Seek(0); ms.Read(ms.Size)` as a fallback. Both are wrong as written.

`MemStream/mData` at file offset `0x958458` in `blue.dll` is a **memory
tracking tag**, not an attribute name — it sits in the middle of
`BlueMemStream.cpp`'s allocation strings (`"Couldn't alloc %d bytes."`,
`"Couldn't grow a provided buffer."`, `"Cannot resize locked stream."`). The
real Blue class registration is a few hundred bytes further on, at
`0x180186d46`, and it passes exactly four strings:

```
0x180186d46  lea -> 'A memory stream'     ; the class doc
0x180186e21  lea -> 'size'                ; attribute
0x180186e7f  lea -> 'position'            ; attribute
0x180186ed7  lea -> 'MemStream'           ; the class name
```

The stream *methods* come from the shared stream interface, registered at
`0x180262717`..`0x180262912`, which binds six names — `Read`, `read`, `Write`,
`write`, `Seek`, `seek` — and parses them with the format strings `'|n'`
(`Read([n])`), `'O|n'` plus `"Need a buffer or string as argument"`
(`Write(obj[, n])`) and `'i|i'` (`Seek(pos[, whence])`).

So the accessor is **`ms.Seek(0); ms.Read(ms.size)`**, with `ms.Read()` as a
plausible second form since `n` is optional. `ms.data` does not exist and
`ms.Size` does not exist.

The script does not take that on trust. `eo_memstream_accessor` writes a known
13-byte payload into a scratch `MemStream` and tries all four candidates —
`Seek(0)+Read(size)`, `Seek(0)+Read()`, `Seek(0)+Read(Size)`, `.data` — keeping
the first that returns **those exact bytes**, printing the verdict for each,
and raising if none does. A dump that cannot prove it read its own bytes back
is not written at all.

One consequence worth knowing: both native entry points `Seek(0)` the stream
themselves (`0x180096d55` in the writer, `0x180096ae0` in the reader, both
`call [rax+0x10]` with offset 0 and whence 0). That is why
`Park.SetState`'s `ms.Write(bag.state)` followed immediately by
`_parent_ReadFullStateFromStream(ms)` works without a rewind, and why
`_ticker.synchronize_to_simulation_time` can hand a stored stream straight
back. It also means **after a write the position is at the end**, so the
read-back must seek — which is exactly what the chosen accessor does.

### 1.2 Where `Park.StoreState` can be called from — don't call it; call the writer

`Park.StoreState` (michelle.py 1155) is the wrong entry point:

```python
def StoreState(self, midTick=False):
    if self.dirty or not self.isRunning:
        return
    ms = blue.MemStream()
    self.WriteFullStateToStream(ms)
    self.states.append([ms, self.currentTime, midTick])
```

It returns silently unless the park is running and clean, it keeps the stream
in `self.states` rather than giving it to you, and it is reachable only from
`FlushSimulationHistory` and the tick. Every bit of it is a guard between us
and the two lines we actually want. The script calls
`WriteFullStateToStream` directly on a ballpark of its own.

**The trigger** is the real question, and the answer is a command the client
already ships. `eve/devtools/script/svc_console.py`:

```python
class ConsoleService(Service):
    __guid__ = 'svc.console'
    __servicename__ = 'console'
    __slashhook__ = True

    def cmd_execfile(self, p):
        if not (session and session.role & ROLE_PROGRAMMER):
            raise slashError.Error('You are not authorized to use this command.')
        filename, = p.Parse('s')
        execfile(filename, globals())
        return 'Ok'
```

`SlashService._SlashCmd` (`svc_slash.py` 634) dispatches by scanning `svc.__dict__`
for any class with `__slashhook__` and a matching `cmd_<name>`, then
`sm.GetService(cls.__guid__[4:])` and calls it. `svc.py` carries
`from eve.devtools.script.svc_console import ConsoleService as console`, so the
class is in `svc.__dict__` from startup and `/execfile` resolves. A local
command that returns a truthy value does **not** fall through to the server, so
our server sees nothing.

Three gates, all of which we already pass:

| gate | where | our state |
|---|---|---|
| `session.role & ROLE_PROGRAMMER` | `svc_console.py` 130, `cmd_execfile` | set — `roles.rs` `SANDBOX_DOCKED` includes bit 51 `[V]` |
| `ROLE_SLASH = ROLE_GML \| ROLE_LEGIONEER` | `chat/client/slash_command.py`, for the chat route | both set `[V]` |
| `ROLEMASK_ELEVATEDPLAYER` | Insider window, Slash Console entry | set `[V]` |

And one that is easy to miss: `_SlashCmd` calls `FetchRemoteCommandList()`
before matching anything, which needs `slash.SlashCmd("/")` to **raise** a
`UserError` whose reason parses — `service.rs` already does exactly that, with
a comment explaining why. If that ever regresses, *every* local slash command
breaks, `/execfile` included.

Two smaller sharp edges in the dispatcher, both from reading `_SlashCmd`:

- the argument string is run through `args % InlineFunctionResolver()` before
  dispatch, so **a `%` anywhere in the path will be eaten**. Keep the path
  plain;
- `param.ParamObject.Parse('s')` splits on spaces unless the token is
  double-quoted, so **no spaces in the path** (or quote it).

### 1.3 Whether a ballpark with no balls produces a useful blob — no, and here is the number

**Five bytes.** Not "small": five. The full-state writer at `0x180096c70`, after
the argument parse, does precisely this:

```
0x180096d3e  mov  ecx, [r14+0x1c]       ; ballpark.currentTime
0x180096d42  mov  [rbp+0x38], ecx
0x180096d45  mov  byte [rbp+0x30], 0    ; packet type 0
0x180096d55  call [rax+0x10]            ; stream->Seek(0, 0)
0x180096d6d  call [rax+8]  (len 1)      ; write the type byte
0x180096d8c  call [rax+8]  (len 4)      ; write currentTime
0x180096dd0  ... iterate ballpark.balls, writing one record each
```

With no balls the loop body never runs. The output is `00` followed by the
simulation tick as a `u32` LE, and nothing else. §6 says what that means for
the undocked client we have today, and what minimal state would give a blob
worth having.

Which is why **the script builds its own ballpark and its own balls** rather
than dumping the live one. The live park is dumped too, last, read-only — but
as evidence about the client's state, not as the reference.

### 1.4 How the client is made to load a loose override — it isn't, and that is the good news

The honest answer to "how do we make the client load a loose `.py`" is: for
this job, **we don't need to, and trying is the riskiest option on the table.**
§5 has the full reasoning. The short version is that `/execfile` takes an
absolute filename and runs it, with no import machinery, no `sys.path`, no
package layout, and no `.pyc` anywhere near it.

Two things the script still has to get right, both settled by disassembly:

**Constructing a private ballpark.** `Ballpark::__init__` (`0x180091820`)
parses `'|p'` — one optional bool — and stores it at `[rcx+0x2ba]`. That offset
is the attribute `_destiny.dll` documents as *"True if this is a server"*, i.e.
`isMaster`, and it is the same byte the writer tests at `0x180096e26` before
applying its bubble filter. So `destiny.Ballpark()` with no arguments is both
legal and the right choice: `isMaster` stays false, the bubble filter is off,
and every ball in the park is written. The script tries
`destiny.Ballpark()`, then `destiny.Ballpark(False)`, then
`blue.classes.CreateInstance('destiny.Ballpark')`, and reports which worked.

**`AddBall`'s arity.** `Ballpark::AddBall` (`0x180091ca0`) opens with

```
0x180091cb3  cmp  byte [0x1800d42d1], 0
0x180091cc0  je   0x180091edb
0x180091cef  lea  rdx, 'Ldffiiiiiddddddffff'   ; 19 args
0x180091f07  lea  rdx, 'Ldffiiiiiddddddff'     ; 17 args
```

— two signatures, chosen by a module byte that is written by
`destiny.settings.Configure` (the write at `0x18008e1ac` sits in the function
that raises `"Argument 1 is not a SettingsConfiguration"`). Both of the
client's own call sites pass 17 arguments:

```python
# michelle.py 1711, Park.AddClientSideBall
ball = self.AddBall(self.clientBallNextID, 1.0, 0.0, 0, False, isGlobal,
                    False, False, False, x, y, z, 0, 0, 0, 0, 0)
# parklife/scenarioMgr.py, ReplaceObjectBall
replacementBall = bp.AddBall(-ballID, 1.0, 0.0, 0, 0, 0, 0, 0, 0,
                             targetBall.x, targetBall.y, targetBall.z,
                             0, 0, 0, 0, 1.0)
```

so the short form is live on a stock client. The script tries 17 first, falls
back to 19, and records which one the client accepted — which is also a
read-out of that settings byte, obtained without touching it.

**This matters beyond the constructor.** The same byte gates block 8 of the
ball record in §4.4. A blob produced with dynamical orientation on and parsed
with it off will not line up. The script therefore prints all three
`destiny.settings` flags at the top of its report, and
`EO_TOUCH_DESTINY_SETTINGS` is `False` so a dump run cannot perturb the live
simulation's idea of the format.

Last trap, and it would have cost a whole run: the writer's ball loop begins

```
0x180096e17  cmp  qword [rbx+0x18], -0x40000000
0x180096e1f  jl   <next ball>
```

**Balls with an id below `-0x40000000` are not serialised at all.** That is the
client-side ball range `Park.AddClientSideBall` allocates from
(`clientBallNextID`, seeded at `-8000000000000000000`). Add a probe ball the
convenient way and the dump comes back as a bare header with no error
anywhere. The script uses positive, distinctive ids.

---

## 2. Deploying the script

The file is `tools/client-overrides/destiny_dump.py` in this repo. It goes into
the Proton prefix, **not** into the client installation:

```sh
tools/destiny-ref.sh deploy     # and `collect` afterwards, `remove` to undo
```

which is the two `cp`s below, with the paths it is easy to get wrong:

```sh
STEAM_ROOT="$HOME/.local/share/Steam"
PREFIX="$STEAM_ROOT/steamapps/compatdata/8500/pfx/drive_c"

cp tools/client-overrides/destiny_dump.py "$PREFIX/eve-logs/destiny_dump.py"
```

| host path | the client sees |
|---|---|
| `$PREFIX/eve-logs/destiny_dump.py` | `C:\eve-logs\destiny_dump.py` |
| `$PREFIX/eve-logs/destiny-ref/` | `C:\eve-logs\destiny-ref\` (created by the script) |

`C:\eve-logs` is already the client's own log directory — `run-client.sh` 113
passes `/fileLogDirectory=C:\eve-logs`, creates the host directory and
`chown`s it to the account the client runs as. So the directory exists, the
client can write to it, and `tools/client-log.sh` already points at it. If you
run the client as a different user than the one that copies the file in, check
that that user can still write there; the script's first failure mode is
`os.makedirs` on a read-only directory, and it says so by name.

Nothing else is deployed. No file inside `$EVE_CLIENT` changes, `manifest.dat`
is untouched, and `--restore` on either client tool is not needed afterwards.
The script is removed by deleting that one file.

---

## 3. Triggering the dump

The client must be **in space**, logged in, with our session roles applied.
Docked works too — the private ballpark the script builds does not care — but
in space you also get the live-ballpark section for free.

### The route, in the client

1. **Open Insider.** It opens itself at character select for an elevated
   session (`characterSelection.OnOpenView`, line 158: `if isElevated and
   settings.public.ui.Get('Insider', True)`), and the setting persists, so it
   is probably already on screen. If not: **right-click the Neocom → "Toggle
   Insider"** (`neocomSvc.GetMenu`, line 973, gated on `ROLEMASK_ELEVATEDPLAYER`).
2. **Insider → Tools → "Slash Console"** (`insider.ToolMenu`, line 1990, same gate). That
   window's input field feeds `SlashService.SlashCmd` and echoes the result.
3. Type:

   ```
   /execfile c:\eve-logs\destiny_dump.py
   ```

4. The window answers `slash result: Ok` when `cmd_execfile` returns, which is
   *after* the script has finished. A dialog also pops with the blob count.

**`/execfile` never reaches our dispatcher, and there is nothing to implement
on our side.** `cmd_execfile` lives in `eve/devtools/script/svc_console.py`
and is handled client-side; its only gate is
`session.role & ROLE_PROGRAMMER`, which is bit 51 and is set in
`roles::SANDBOX_DOCKED` (`0x61780052_00E41000`), asserted by that module's own
test. Worth stating because the obvious worry -- that a slash command we do
not serve would be refused by us and the dump would never run -- does not
apply to this one.

### The alternative route, same file

**Insider → Tools → "Python Console"** (the last entry of `insider.ToolMenu`, `ROLE_PROGRAMMER`),
then in the text area:

```python
execfile(r'C:\eve-logs\destiny_dump.py', globals())
```

and press **Execute**. The `globals()` argument is not optional and not
cosmetic. `ConsoleService.DoExecute` runs your text as
`eval(code, globals(), returnDict)` — two different namespaces — so a bare
`execfile(path)` would put the script's `def`s into `returnDict` while their
bodies look names up in `globals()`, and the first helper call would raise
`NameError`. Passing `globals()` explicitly gives the script one namespace, the
way `/execfile` does.

That same split is why the script is written as prefixed module-level
functions plus one call at the bottom, rather than as a flat sequence of
statements, and why every name in it starts with `eo_`/`EO_`: it is executed
with `eve.devtools.script.svc_console`'s own globals, and must not shadow
`sys`, `blue`, `Service`, `SERVICENAME` or anything else that module needs
afterwards.

### A third route, if a chat channel happens to exist

Typing the same `/execfile` line into a chat input reaches
`chat.client.slash_command.handle_slash_command`, which routes anything not in
the `publicQaToolsClient` allow-list through `session.role & ROLE_SLASH` to the
same `SlashService.SlashCmd`. It is listed for completeness; the Slash Console
needs no chat server.

---

## 4. What comes back

Everything lands in `C:\eve-logs\destiny-ref\`
(`$PREFIX/eve-logs/destiny-ref/` on this machine):

| file | what it is |
|---|---|
| `00-REPORT.txt` | the whole run: environment, probes, every hex dump, every diff, the round trip |
| `01-empty-park.bin` | header only, five bytes |
| `02-empty-park-alt-tick.bin` | the same at a different tick — pins the header's four tick bytes |
| `03-base-one-ball.bin` | **the reference**: one ball, every argument a distinctive value |
| `04-two-balls.bin` | two balls differing only in id — gives the record stride and the terminator |
| `05-write-balls-to-stream.bin` | the same ball through `WriteBallsToStream`, i.e. **packet type 1**, the form `AddBalls`/`AddBalls2` consume |
| `NN-arg-*.bin` | one per `AddBall` argument, each changing exactly one |
| `NN-attr-*.bin` | one per `destiny.Ball` attribute, set through `setattr` |
| `NN-call-*.bin` | one per `Ballpark` method — `Stop`, `FollowBall`, `Orbit`, `SetBallRigid`, … — for the mode byte and the follow fixups |
| `99-live-ballpark.bin` | the live park, read-only, if `michelle.GetBallpark()` returns one |
| `00-FAILED.txt` | only if the run died; holds the traceback |

### The part that is actually the deliverable

For each variant the report prints the differing byte windows against the
baseline **and then tries to label them**: it re-encodes the probe value as
f64, f32, i64, u64, i32, u32 and u16 and searches the variant blob for each
encoding. A hit next to the differing window names the field's type and
offset, and the `FIELD` lines then print the whole field as both blobs have it:

```
    1 differing window(s) for Ball.radius:
      0x0012..0x0014 (2 bytes)
        base    20 4c
        variant e5 94
        MATCH: the probe value encodes as f64 at offset 0x000d
        FIELD f64 at 0x000d..0x0015:
          base    00 00 00 00 00 20 4c 40
          variant 00 00 00 00 00 e5 94 40
```

Note what happened there: 56.25 and 1337.25 share five leading bytes, so the
*differing* window was two bytes wide and told you almost nothing on its own.
The encoding search recovered the real field — an f64 at `0x000d` — and that
line is a row of a Rust `struct`, measured. Rows of §4.4's table that come back
without a `MATCH` are the ones still owed an explanation — a derived field, a
packed one, or a width we did not try — and the report says so rather than
staying quiet. Flag probes (0/1) skip the search, because an encoded 0 matches
nearly everywhere; for those the one-byte delta is the whole answer.

Two sections are worth reading before the hex:

- **the probes**, at the top: which ballpark constructor, which `MemStream`
  accessor (and the verdict on each one we tried), which `AddBall` arity, and
  the three `destiny.settings` flags. If the arity is 19, every later byte
  offset is for a *different* layout than the one a 17-argument client
  produces, and you want to know that before writing any Rust;
- **the round trip**, near the end: the baseline blob is fed back through the
  client's own `ReadFullStateFromStream` into a fresh ballpark, and every
  `destiny.Ball` attribute of the reconstructed ball is printed. That does two
  jobs. It proves the bytes are acceptable to the parser we have to satisfy —
  `REJECTED` there invalidates everything above it — and it maps `AddBall`
  arguments onto attribute *names* without touching a byte offset at all.

### Reading a blob back, outside the client

`03-base-one-ball.bin` is byte-for-byte what belongs in `bag.state` for a
one-ball `SetState` (`docs/archive/destiny-and-the-ballpark.md` §4.3). It is a plain file:

```sh
xxd "$PREFIX/eve-logs/destiny-ref/03-base-one-ball.bin"
```

and in Rust it is a fixture — `include_bytes!` it next to a serialiser test, in
the same spirit as `eve-inflate`'s reference vectors, so the first Rust blob we
write is diffed against the client's own output rather than against our reading
of a disassembly. The ball in it has id `0x0102030405060708`; a `SetState`
built from it needs the matching `foo.SlimItem` to carry that same `itemID`, or
`Park.SetState` raises `RuntimeError('BallNotInPark', …)`.

---

## 5. Why not a loose `.py` import override

Because for this job it buys nothing and risks a lot, and because the route
`docs/archive/roadmap.md` M0 records as "resolved in principle" has an unresolved step in
the middle of it.

What is verified: CCP extended zipimport's search-order table (`python27.dll`
`.data`, file offset `0x2dc160`) with `.pyj` *in front of* the stock
`.pyc`/`.pyo`/`.py` entries, so source import is live; `autoexec`'s only import
guard is behind `/jessica` and imports two modules that do not ship; and
`blue.dll` sets `Py_NoSiteFlag` and `Py_IgnoreEnvironmentFlag`, which kills
both `sitecustomize` and `PYTHONPATH`. All of that is `docs/archive/recon-v24.01.md` Rounds
2 and 3 and it still holds.

What that leaves unsaid is the part that bites. Modules in this client are
imported by **full dotted path** out of the archive — the client's own log
prints

```
Adding event OnSessionChanged for <module 'monolithhoneycomb' from
'C:\CCP\EVE\tq-offline\code.ccp\monolithhoneycomb\__init__.pyj'>
```

— and `code.ccp` carries `__init__.pyj` at every level
(`eve/`, `eve/client/`, `eve/client/script/`, `eve/client/script/remote/`, all
present). So shadowing `eve/client/script/remote/michelle.py` with a loose file
is not a matter of getting a directory onto `sys.path` ahead of the archive.
Python 2 resolves a submodule through its **parent package's `__path__`**, and
the parent package is whichever `eve` was found first. A loose `eve/` directory
early on the path captures the name `eve` and its `__path__` becomes the loose
directory — at which point the other 3,586 modules under `eve.` are gone unless
the loose `eve/__init__.py` re-appends the archive's `eve` sub-path, and then
the same trick is needed again at `eve.client`, and again at
`eve.client.script`, and again at `eve.client.script.remote`. That is four
hand-written `__init__.py` files whose only job is to undo the damage of the
first one, and any of them getting it wrong is a client that boots into a
different failure every time.

(The same caveat applies to `docs/archive/session-flow.md`'s plan of a loose
`evecrypto/settings.py`: `evecrypto` is an archive package too, so a loose
`evecrypto/` directory shadows the whole package, not one module in it. Worth
knowing before that path is taken for `cryptoPack`; it is not this document's
problem, because `tools/client-prepare.sh` solved `cryptoPack` another way.)

On top of that, it is unresolved whether the installation root is on `sys.path`
at all. `sys.path` is built inside CCP's `python27.dll` fork via its own
`Py_GetPathWData` export, called from `blue.dll`; no binary in `bin64/`
contains the string `code.ccp`, so the entry is composed at run time and
reading it off the disassembly is a job in itself.

**Which is a question the first dump run can answer in one line.** In the
Python Console:

```python
import sys
MyResult = sys.path
```

and press Execute. The output pane prints it. That is worth doing while the
console is open, because the answer belongs in `docs/archive/recon-v24.01.md` and settles
M0's last owed item for the price of one keystroke.

For completeness, the two mechanical questions that were asked and do have
answers:

- **Does the archive's `.pyj` beat a loose `.py`?** Within one `sys.path`
  entry, yes — `.pyj` is first in the search order table. Between two entries,
  no: the first *entry* that can supply the name wins, whatever its suffix. So
  ordering, not suffix, is the lever — which is exactly the package-`__path__`
  problem above.
- **Does a stale `.pyc` beside a loose `.py` break it?** No. CPython 2.7's
  source loader validates the adjacent `.pyc`'s magic and the source's mtime
  and recompiles on a mismatch. And because `blue.dll` sets
  `Py_DontWriteBytecodeFlag`, the client never writes one in the first place,
  so the stale-`.pyc` hazard needs you to have put it there yourself.

None of that is needed for a reference blob. It will be needed the day we want
to *intercept* something — to log every `DoDestinyUpdate` the client receives,
say — and on that day the cheap version of the same lever is still
`/execfile`: a script that monkey-patches a bound method on a live service
object needs no import shadowing at all.

---

## 6. The empty ballpark, said plainly

**An empty ballpark's full-state blob is five bytes and is worth nothing as a
reference.** `01-empty-park.bin` will be one type byte followed by the tick as
a `u32` LE — `00 44 33 22 11` if `Ballpark.currentTime` turns out to be
writable, and whatever the park already held if it is not — and the undocked client we have today, which reaches `AddBallpark` with
an empty formations tuple and no ego ball (`docs/archive/destiny-and-the-ballpark.md` §3),
has exactly that in it. Dumping the live park and calling it a reference would
produce a file with no ball record in it at all, and the mistake would not
announce itself: there is no error, just a very short file.

So the script does not depend on the live park. It makes its own, adds its own
balls with values it chose, and dumps that. **The whole exercise works while
docked, with no ballpark, on day one** — and it does not need step 1 of §8,
the push path of step 3, or anything else from the server.

The minimal state that *would* make the live park worth dumping, for when the
question comes up again:

- one ball whose id is `session.shipid`, delivered by a server-initiated
  `SetState` — i.e. everything in §8 steps 3 and 4. At that point
  `99-live-ballpark.bin` becomes the *round-trip* check on our own serialiser:
  send our blob, let the client parse it, let the client write it back out, and
  diff. If those two differ, the parser read something other than what we
  meant, even if a ship appeared;
- or any server that puts balls on the grid — which is to say, not yet.

Until then, the live-ballpark section of the report is diagnostic only: it
prints the park's attributes and ball ids, and says in as many words that a
blob of eight bytes or fewer means an empty park.

---

## 7. Corrections to `docs/archive/destiny-and-the-ballpark.md` §4.4

Settled while building this, and not yet folded into that document:

**7.1 The blob does not start with the 4-byte integer.** It starts with a
1-byte **packet type**, and the 4-byte tick follows it. §4.4 reads the balls
reader at `0x1800978c0` and reports its first `Read(4)`; that function is one
level down. Its caller is the real `ReadFullStateFromStream` at `0x1800969d0`:

```
0x180096ae0  call [rax+0x10]          ; stream->Seek(0,0)
0x180096af5  call [rax]    (len 1)    ; read the packet type
0x180096b17  cmp  byte [rsp+0x80], 1
0x180096b1f  ja   -> 'Unknown packet type'
0x180096b4c  call 0x1800978c0         ; <- what §4.4 documents
...          loop: read another type byte until Read returns 0
0x180096b86  lea  rdx, 'Read %d bytes from stream'
```

So a blob is **a sequence of packets**, each `<u8 type><u32 tick><ball
records…>`, read until end of stream. `WriteFullStateToStream` emits type `0`
(`0x180096d45`); `WriteBallsToStream` emits type `1` (`0x180096ffd`) and is
otherwise identical, including the tick. Types above 1 are refused by name.

That also explains the `AddBalls` path: `michelle.AddBalls` writes a blob and
calls `ReadFullStateFromStream(ms, 2)` — mode 2 skips the clear-the-park branch
at `0x180096a9e`, and the type-1 packet it is parsing has the same shape as a
type-0 one. The mode is the *second argument* (`'O|i'`), not part of the blob.

**7.2 `WriteFullStateToStream` takes an optional second argument**, format
`'O|L'`: a ball id, defaulting to `-1`. When it is given *and* the park's
`isMaster` is set, the writer filters balls to one bubble
(`0x180096e17`..`0x180096e4d`). With one argument, from a client-side park,
there is no filtering — which is what we want and what the script does.

**7.3 The attribute names are not a best fit any more.** `_destiny.dll` carries
them as plain strings, in table order, at `0xa4fd0` (`destiny.Ball`) and
`0xa9240` (`destiny.Ballpark`):

```
Ball:      id mass radius maxVelocity maxAngularVelocity isFree isGlobal
           isMassive isInteractive isCloaked isMoribund harmonic
           corporationID allianceID x y z vx vy vz wx wy wz rx ry rz rw
           yaw pitch roll Agility RotationalAgility speedFraction mode
           gotoX gotoY gotoZ followId followRange ownerId effectStamp
           newBubbleId oldBubbleId formationID miniBalls miniCapsules
           miniBoxes ballpark
Ballpark:  ego probe solarsystemID isMaster time currentTime lagDamping
           friction warpSpeed tickInterval isRunning para1 para2 int1 int2
           moribundBallRemovalCount moribundBallRemovalBuffer balls globals
           bubbles bubbleInteractives bubbleKeepAlives bubbleKeepAliveBalls
           bubbleSubs rollSpeedAcceleration rollSpeedDecay rollAcceleration
           rollDecay
```

§7.2 of that document paraphrased these from the doc strings; these are the
names themselves, and they are what the script's `setattr` probes use. Note
`isMaster` — §4.4 had no name for the `+0x2ba` byte.

**7.4 Client-side balls are invisible to the serialiser** (`< -0x40000000` is
skipped, §1.4). Anything built on `Park.AddClientSideBall` cannot appear in a
blob, in either direction.

---

## 8. Failure modes, and what each one means

The script is written so that every one of these is loud. In rough order of
likelihood:

| what you see | what it means |
|---|---|
| `slash error: You are not authorized to use this command.` | `ROLE_PROGRAMMER` is not in the session mask. Check `roles.rs`; the default `SANDBOX_DOCKED` has it. |
| `/execfile` is not recognised, or *every* slash command fails | `FetchRemoteCommandList` could not parse our `slash.SlashCmd("/")` raise. It is the first thing `_SlashCmd` does and it breaks all local commands, not just this one. `service.rs` 3488 onwards. |
| `00-FAILED.txt` appears, plus an `EXCEPTION` block in the client log | the script raised. The traceback is in both places; `tools/client-log.sh --errors` prints the client's copy with locals. |
| the report stops after `probes` with *"no blue.MemStream accessor returned the probe payload"* | §1.1's conclusion is wrong on this build. Nothing was written, deliberately — a blob we cannot read back is not evidence. Report the four verdict lines; they name what each accessor did instead. |
| `AddBall … FAILED` for **both** 17 and 19 arguments | the format strings differ on this build, or the park is unusable. The exception text is `_destiny.dll`'s own — `"Function expects %i arguments"` or `"Could not match argument %i to expected type: %s"` — and it names what it wanted. |
| every `NN-*.bin` is 5 bytes | balls are not reaching the park. Either `AddBall` is silently failing, or the ids are in the client-side range (§1.4) — the script uses positive ids precisely to avoid the second. |
| a variant reports *"no byte differs from the baseline"* | that field is not serialised at all, or the `setattr` did not take. The prepare line above it prints the attribute's value before and after, which distinguishes the two. |
| the round trip says `REJECTED` | `ReadFullStateFromStream` would not accept bytes its own writer produced. That is a much bigger finding than a failed dump and it invalidates everything above it in the report. |
| `Can not add ball during Evolve().` | the park was ticking while we added. Should not happen on a private park, which is never `Start()`ed; if it does, something else got hold of it. |

And one non-failure: `michelle.GetBallpark() is None`. `GetBallpark` defaults to
`doWait=False` and returns `None` unless `bpReady`, so a docked client simply
skips the live section. The script never passes `doWait=True`, which is what
would otherwise cost 30 seconds in `WaitForBallpark`.

---

## See also

- [`docs/archive/destiny-and-the-ballpark.md`](../archive/destiny-and-the-ballpark.md) — §4.4 for the
  layout this measures, §8 step 2 for where it sits in the order of work, and
  §7 for the mode table the `NN-call-*.bin` dumps exercise.
- [`docs/guides/debugging-the-client.md`](../guides/debugging-the-client.md) — the `[V]`/`[I]`
  convention, `tools/client-log.sh --errors` for the traceback if it fails, and
  §4's warning about uncompyle6 rendering const-table indices as integers,
  which is why the attribute names in §7.3 were taken from the DLL instead.
- [`docs/reference/session-roles.md`](session-roles.md) §5 and `eve-server/src/roles.rs` — the
  three role bits this depends on, and the one (`ROLE_PROGRAMMER`) that has a
  measured cost of its own.
- [`docs/archive/recon-v24.01.md`](../archive/recon-v24.01.md) Rounds 2 and 3 — the import-override
  work that §5 narrows rather than replaces.

---

## 9. The dump, as run — 2026-10-21

Run against build **24.01 / 3619879**, docked, from the Slash Console.
**65 files**: `00-REPORT.txt` plus 64 blobs, in `findings/destiny-ref/`.
Round 0.2 is closed.

### 9.1 The result that makes the rest usable

```text
-- round trip through ReadFullStateFromStream ---
  accepted.
  currentTime after read: 287454020 (wrote 0x11223344)
  balls in the park after read: [72623859790382856L]
```

**The client's own parser accepted our bytes and read every field back
unchanged.** That is the whole point of the exercise: the blob is not a
reading of a format any more, it is a reference produced and consumed by the
exact DLL that will parse ours. Everything below is measurement rather than
inference.

All three probes reported *validated*, not merely attempted:

| probe | result |
|---|---|
| ballpark ctor | `destiny.Ballpark()` — ok |
| memstream read-back | `Seek(0) + Read(ms.size)` — ok, **verified against a known payload** |
| `AddBall` arity | **17 positional args** |

Two memstream accessors failed and are worth recording so nobody tries them
again: `ms.Size` and `ms.data` are both `AttributeError`. §1.1 guessed
`ms.data`; it is wrong.

`destiny.settings` at dump time: `useDynamicalOrientation` **False**,
`disableDynamicalOrientationForMissiles` False, `useNewOrbit` False. The
first of those selects the 17-arg `AddBall` format over the 19-arg one, so
the arity above is a *consequence* of the setting and not a property of the
build.

### 9.2 `AddBall`'s 17 arguments, named

Previously `a2_d`..`a17_f` — placeholders, because nothing said what they
were. The round-trip readback names every one of them:

| # | name | | # | name |
|---|---|---|---|---|
| 1 | `ballID` | | 10–12 | `x`, `y`, `z` |
| 2 | `mass` | | 13–15 | `vx`, `vy`, `vz` |
| 3 | `radius` | | 16 | `Agility` |
| 4 | `maxVelocity` | | 17 | `speedFraction` |
| 5–8 | `isFree`, `isGlobal`, `isMassive`, `isInteractive` | | 9 | unidentified |

Argument 9 is the one left: it is an int, the baseline passed `1`, and
`isCloaked` reads back as `0`, so it is **not** `isCloaked`. Its variant dump
(`14-arg-a9_i.bin`) produced no `MATCH`, so it does not appear in the record
as any of the probed widths.

### 9.3 The measured record, and §4.4 was right

The record is **100 bytes**, and the two-ball dump proves it rather than
assuming it: the second ball's id begins at `0x69`, and `0x69 - 0x05` is
`0x64`. Offsets below are **absolute in the blob**, so subtract 5 for
record-relative.

| at | width | field | how |
|---|---|---|---|
| `0x00` | 1 | packet type (`0`) | §7.1, now measured |
| `0x01` | 4 | `currentTime`, u32 LE | two dumps, ticks `0x11223344` and `0x55667788` |
| `0x05` | 8 | `ballID`, i64 | arg |
| `0x0d` | 1 | `mode` | reads `02`; readback says `mode 2` |
| `0x0e` | 4 | `radius`, f32 | arg **and** attribute |
| `0x12` | 24 | `x`, `y`, `z`, 3 × f64 | args **and** attributes |
| `0x2a` | 1 | flags | reads `0x1f` |
| `0x2b` | 8 | `mass`, f64 | arg **and** attribute |
| `0x33` | 1 | cloak | reads `00`; readback `isCloaked 0` |
| `0x34` | 8 | `harmonic`, i64 | reads `-1`; readback `harmonic -1` |
| `0x3c` | 4 | `corporationID`, i32 | attribute |
| `0x40` | 4 | `allianceID`, i32 | attribute |
| `0x44` | 4 | `maxVelocity`, f32 | arg **and** attribute |
| `0x48` | 24 | `vx`, `vy`, `vz`, 3 × f64 | args **and** attributes |
| `0x60` | 4 | `Agility`, f32 | arg **and** attribute |
| `0x64` | 4 | `speedFraction`, f32 | arg **and** attribute |
| `0x68` | 1 | `0xff` | see §9.4 |

**Nine of those offsets were hit twice, independently** — once by changing an
`AddBall` argument and once by assigning the `destiny.Ball` attribute. Two
routes to the same byte is the strongest form this table comes in.

And the headline for `docs/archive/destiny-and-the-ballpark.md` §4.4: **every row of its
table is confirmed**, including the three corrections the cross-check made to
it. Rows 1–7d all land where it says, with the types it says. One naming
correction remains: row **7c**, which §4.4 calls `inertia` with init 1.0, is
the client's `Agility` — `RotationalAgility` is the separate attribute that
reads `0.9`.

### 9.4 The `0xff`, which is **not** settled — and the probe that failed

`docs/re/destiny-ball-crosscheck.md` §6 item 1 asked whether the byte at the
end of the record is a section terminator, an orientation-present sentinel,
or a build difference, because our disassembly gates block 8 on a
`destiny.settings` byte while EvEJS writes a bare `0xff` there
unconditionally.

What the dump settles: the byte is at **record offset 99, inside the
100-byte record** — the two-ball dump shows the next record beginning
immediately after it — so it is a **field, not a terminator**. And with
`useDynamicalOrientation` False there is no orientation data anywhere in the
record, so EvEJS's unconditional `0xff` is right *for the shipping default*
and our gate is right about there being a gate. The two readings reconcile
rather than contradicting.

What it does **not** settle: which field. The readback reports
`formationID 255`, and `255 == 0xff`, which is suggestive and nothing more.
The dump that would have proved it **failed silently**:
`80-call-SetBallFormation.bin` is **byte-identical to `04-two-balls.bin`**,
and `mode` stayed `2` instead of becoming `12` (`DSTBALL_FORMATION`). So
`SetBallFormation` is a no-op on a private ballpark — it needs something the
dumper does not provide, probably a running simulation.

So: `[I]` that `0x68` is `formationID`. To settle it, a dump is needed in
which `formationID` actually differs from 255, which means finding the route
that makes `SetBallFormation` take. **Fifteen other variants were skipped the
same way** and every one is informative: `mode`, `followId`, `followRange`,
`yaw`, `pitch`, `roll`, `isMoribund`, `effectStamp`, `newBubbleId`,
`oldBubbleId` and `formationID` are all **read-only Python properties**, so
they can only be set through `AddBall` or a Destiny call, never by
assignment.

### 9.5 Two more things worth keeping

- **`michelle.GetBallpark()` was `None`** — docked, so there is no live
  ballpark to read. The private-ballpark dumps are unaffected, and the
  read-only pass over the live park simply had nothing to do. Running the
  same script undocked would add it.
- **`WriteBallsToStream` could not be called**: `TypeError: must be list, not
  blue.MemStream`, so its argument order is not the writer's. Unimportant for
  a full state, but it is the call `SetState`'s incremental sibling would
  use.

### 9.6 What is committed, and why that is ours to commit

`findings/destiny-ref/` holds the report and the 64 blobs. The blobs contain
**no CCP content**: every value in them is one we chose (`1234.5`, `101.0`,
`0x0102030405060708`), arranged by CCP's serialiser. They are a format
fingerprint and a test vector, the same category as a recorded protocol
handshake, and the same category §11 of `docs/re/dogma.md` allows for table
*shapes*. Anyone wanting the client still has to obtain it themselves.

### 9.7 The per-mode tails, measured — five of nine modes

§9.3 stops at the end of the common record, which is where the format stops
being common: **after the `0xff` at record offset 99, the writer appends a
tail chosen by the `mode` byte at record offset 8.** §4.1 of
`docs/re/destiny-ball-crosscheck.md` lists a tail for all nine modes from a
second implementation `[V2]`. Five of them are now ours.

Each dump below is two balls: ball 1 is `0x1112131415161718`, ball 2 is
`0x0102030405060708`, and the Destiny call is made against **ball 2**. So
every one of these blobs contains both the mode under test *and* a STOP ball,
and the baseline to compare against is `04-two-balls.bin`, not the one-ball
`03-base-one-ball.bin`.

Tail offsets are **relative to the end of the common record** — add 100 for
record-relative, or `105 + 100` for the absolute offset of ball 2's tail in
these blobs.

| mode | tail | at | w | blob | total |
|---|---|---|---|---|---|
| `STOP` (2) | *nothing* | — | 0 | `66-call-Stop.bin` | 205 |
| `GOTO` (0) | `gotoX`, `gotoY`, `gotoZ` f64 | +0, +8, +16 | 24 | `69-call-GotoPoint.bin` | 229 |
| `FOLLOW` (1) | `followId` i64 | +0 | 8 | `67-call-FollowBall.bin` | 217 |
| | `followRange` f32 | +8 | 4 | | |
| `ORBIT` (4) | `followId` i64 | +0 | 8 | `68-call-Orbit.bin` | 217 |
| | `followRange` f32 | +8 | 4 | | |
| `TROLL` (8) | `effectStamp` u32 | +0 | 4 | `71-call-SetBallTroll.bin` | 209 |

All `[V]`. Every total is `5 + 100 + 100 + len(tail)`, and the bytes read:

```text
66-call-Stop         mode 02   tail (none)
                     -- byte-identical to 04-two-balls.bin, so a STOP call on
                        a ball that is already STOP changes nothing, and the
                        zero-length tail is confirmed by the *first* record of
                        all five blobs as well: ball 1 is mode 02 throughout
                        and ball 2's id still begins at 0x69.
69-call-GotoPoint    mode 00   00 00 00 00 00 48 af 40  = 4004.0
                               00 00 00 00 00 8d b3 40  = 5005.0
                               00 00 00 00 00 76 b7 40  = 6006.0
                     -- GotoPoint(ball2, 4004.0, 5005.0, 6006.0)
67-call-FollowBall   mode 01   18 17 16 15 14 13 12 11  = 0x1112131415161718
                               00 00 fa 43              = 500.0
                     -- FollowBall(ball2, ball1, 500.0); the i64 is the
                        *target's* id, not the subject's
68-call-Orbit        mode 04   18 17 16 15 14 13 12 11  = 0x1112131415161718
                               00 00 7a 44              = 1000.0
71-call-SetBallTroll mode 08   45 33 22 11              = 0x11223345
                     -- the blob header's currentTime is 0x11223344, so
                        effectStamp is exactly `currentTime + 1`
```

**The GOTO triple's field order is settled twice over**, which matters because
one `GotoPoint` call sets all three at once and only the increasing magnitudes
of `4004 < 5005 < 6006` distinguish them. The attribute route sets one at a
time, and `gotoX`/`gotoY`/`gotoZ` are the three `destiny.Ball` attributes
§9.4 lists as *writable* rather than read-only:

| blob | len | mode | tail |
|---|---|---|---|
| `56-attr-gotoX.bin` | 129 | `00` | `4004.0, 0.0, 0.0` |
| `57-attr-gotoY.bin` | 129 | `00` | `0.0, 5005.0, 0.0` |
| `58-attr-gotoZ.bin` | 129 | `00` | `0.0, 0.0, 6006.0` |

Each is a **one-ball** dump, `5 + 100 + 24 = 129`, and each shows a second
thing worth having: `00-REPORT.txt`'s diff against `03-base-one-ball.bin`
reports exactly one changed byte, at `0x0d`, `02 -> 00`. **Assigning `gotoX`
puts the ball into `DSTBALL_GOTO`**, and the 24-byte tail appears as a
consequence of the mode change rather than of the assignment. The tail really
is keyed on `mode` and on nothing else `[V]`.

#### What this does to `destiny-ball-crosscheck.md` §4.1

Five of its nine rows are now `[V]` from our own client, and all five agree
with it exactly — field order, widths and the `followID`-is-the-target
semantics:

| row | was | now |
|---|---|---|
| `STOP` | `[V2]` | **`[V]`** |
| `GOTO` | `[V2]` | **`[V]`** |
| `FOLLOW` | `[V2]` | **`[V]`** |
| `ORBIT` | `[V2]` | **`[V]`** |
| `TROLL` | `[V2]` | **`[V]`** |
| `WARP` | `[V2]` | `[V2]` only |
| `MISSILE` | `[V2]` | `[V2]` only |
| `FORMATION` | `[V2]` | `[V2]` only |
| `MUSHROOM` | `[V2]` | `[V2]` only |

That is the undocked tier covered: STOP for a parked ship, GOTO for moving,
FOLLOW and ORBIT for approach and orbit. The cross-check is now right about
every row of its §1 header (§9.3) *and* five for five on its §4.1 tails,
which is the strongest reason yet to take its remaining four rows seriously —
and still not a reason to call them `[V]`.

**What would settle the other four, and it is cheaper than §9.4 implied.**
§9.4 blames the private ballpark, and for `FORMATION` that is right as far as
it goes. But the dumper probed **fifteen** `Ballpark` methods and *none of
them is a warp, missile or mushroom setter* — so for three of the four rows
the dump did not fail, it never asked. `_destiny.dll`'s own method table says
the calls exist, with CCP's docstrings beside them `[V]`:

| method | what the DLL's own doc string says | mode it should set |
|---|---|---|
| `WarpTo` | *(none)* | `WARP` (3) |
| `MissileFollow` | *(none)* | `MISSILE` (5) |
| `LaunchMissile` | "Launches a missile" | `MISSILE` (5) |
| `AddMushroom` | *(none)* | `MUSHROOM` (6) |
| `FormationFollow` | "Joins an existing formation" | `FORMATION` (12) |
| `LoadFormations` | "Load formation static data" | — |
| `SetBallFormation` | "Sets the formation of a ball" | `FORMATION` (12) |

Read them in order and §9.4's failure explains itself: **`SetBallFormation`
sets the `formationID` of a ball that is already in a formation; the call
that puts it there is `FormationFollow`, and the formations themselves have
to be loaded first with `LoadFormations`.** That is why the call took no
effect and raised nothing — it is not "probably a running simulation", it is
two missing prerequisites. The DLL also carries
`Inconsistent parameters for mushroom. Canceled.` and
`Non existing parent for mushroom. Canceled.` `[V]`, so `AddMushroom`
likewise needs a parent ball, which the two-ball dump already provides.

So the experiment is: add `WarpTo`, `MissileFollow` / `LaunchMissile`,
`AddMushroom`, and `LoadFormations` + `FormationFollow` + `SetBallFormation`
to the variant list in `tools/client-overrides/destiny_dump.py` and re-run
the same script. Arity costs one extra pass and no guessing: a wrong
call raises a `TypeError` that **names the right number of arguments**, which
is what happened to the two methods the dumper did probe and could not call
— `70-call-SetBallRigid` got "function takes exactly 1 argument (2 given)"
and `72-call-CloakBall` got "function takes at least 2 arguments (1 given)"
`[V]`. Both are one-line fixes on a re-run, and both are additional modes
(`RIGID` 11, and the cloak byte at record offset 46) for free.

Two fallbacks if the private ballpark still refuses. A **live ballpark
undocked** (§9.5: `GetBallpark()` was `None` only because we were docked, and
warping with the script loaded would produce a real WARP ball to read); or
feeding a hand-built blob to `ReadFullStateFromStream` and reading the parsed
ball back — the §9.1 round trip run backwards, which tests *our* writer
instead of copying theirs. The backwards round trip generalises: **anything
the dumper cannot make the client write, the client can still be made to
read.**

`MUSHROOM` is the one row where a mistake costs nothing — no ship is ever in
it — so it is the right one to do last.

**One free result while looking.** The same string table lists the thirteen
mode names in declaration order `[V]`:

```text
DSTBALL_GOTO  FOLLOW  STOP  WARP  ORBIT  MISSILE  MUSHROOM  BOID  TROLL
              FIELD  MINIBALL  RIGID  FORMATION
```

That is `GOTO 0 … FORMATION 12` read off the DLL directly, and it is a
**fourth** independent confirmation that `FIELD` is 9 and `MINIBALL` is 10 —
`destiny-ball-crosscheck.md` §2.3 corrected EvEJS on exactly that pair, and
the five mode bytes measured above (`02`, `00`, `01`, `04`, `08`) land where
this list says. Do not re-open it.

### 9.8 The trailing `0xff` is written with `isFree` clear as well

§3.4 of `docs/lanes/round2-spec.md` asserts this because it decides where the
tail goes; it is confirmed, and twice.

`10-arg-a5_i.bin` is the baseline with `AddBall`'s `isFree` argument set to
`0`. It is **69 bytes — a 64-byte record** — and it ends like this (offsets
**absolute in the blob**, as in §9.3; subtract 5 for record-relative):

```text
0x2a  1e                       flags, bit 0 clear  -> isFree = 0
0x2b  00 00 00 00 00 4a 93 40  mass        f64  1234.5
0x33  00                       cloak
0x34  ff ff ff ff ff ff ff ff  harmonic    i64  -1
0x3c  ff ff ff ff              corporationID i32 -1
0x40  ff ff ff ff              allianceID  i32  -1
0x44  ff                       <- last byte of the file
```

The 36 gated bytes (`maxVelocity`, the velocity triple, `Agility`,
`speedFraction`) are absent, `100 - 36 = 64`, the `0xff` lands at **record
offset 63** instead of 99, and it is still there,
written **after** the gated block rather than inside it `[V]`. The second
witness is `27-attr-isFree.bin`, which reaches the same state by assigning
`ball.isFree = 0` instead of passing the argument: it is **byte-identical to
`10-arg-a5_i.bin`**, so the two routes agree, and `00-REPORT.txt` records its
length change as `105 -> 69`.

So the writer is, in order: common header, `if isFree { 36 bytes }`, `0xff`,
`tail(mode)`. A reader that treats the `0xff` as part of the gated block
would mis-parse every rigid ball in the park by one byte, and a writer that
omits it would do the same to the client.

One thing this does *not* settle, and it is the same gap as §9.4: both
witnesses are mode `2`, so we have no blob that is `isFree = 0` **and** has a
tail. The order `0xff` then tail is `[I]` for that combination — safe,
because nothing we write is ever both, and an ego ball must be free.
