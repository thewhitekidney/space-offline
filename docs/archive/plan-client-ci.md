# Automated testing against the real client

> **Demoted 2026-10-24** — it calls itself "a plan, not a built thing", but `tools/client-ci.sh` exists. Current truth: [lanes/ci-harness.md](../lanes/ci-harness.md). Kept for history; do not brief a lane from this file.

**The goal: remove the human from the verification loop**, so that work done
overnight is *verified* rather than merely written. Today the loopback suite
proves what goes on the wire and a person at a keyboard is the only oracle for
what the client does with it — which is why five of this project's most
expensive defects were found by a player and not by a test.

This is a plan, not a built thing. Its one genuine unknown is named in §4 and
is worth a timeboxed spike before anything else is built.

## 1. Why it is worth doing, scored rather than asserted

The bugs that reached a human in the round-2 work, and whether a harness would
have caught them:

| defect | caught by | how |
|---|---|---|
| health gauges hidden (empty `damageState`) | **inside the client** | read the three HUD values godma computed |
| health gauges red (shield health in a slot nothing reads) | **inside the client** | same three values |
| capacitor rings empty (no dogma attributes sent) | **inside the client** | same |
| undocked at the wrong point (station centre, not the mouth) | **inside the client** | distance from the ego ball to the station ball |
| the scene rendered greyscale | **a frame metric** | mean saturation of a captured frame |

Five for five. Four need no pixels at all — they are numbers the client has
already computed and that our script can read directly.

## 2. The three pieces, two of which already exist

### 2.1 Code execution inside the client — **exists** `[V]`

`tools/client-overrides/destiny_dump.py` runs inside the client under
Stackless Python 2.7 and produced `findings/destiny-ref/*.bin`, the reference
blobs our codec is tested against. So this is not a hope: it has already
produced bytes we rely on.

The trigger today is a human typing, in the client's Slash Console:

```
/execfile c:\eve-logs\destiny_dump.py
```

`cmd_execfile` is **client-side** (`eve/devtools/script/svc_console.py`), gated
on `session.role & ROLE_PROGRAMMER` — a bit **our own server sets**
(`roles::SANDBOX_DOCKED`, bit 51) `[V]`. Nothing has to be unlocked or
patched; the permission is ours to grant and we already grant it.

### 2.2 A channel out — **exists** `[V]`

The in-client script writes to `C:\eve-logs\`, which `run-client.sh` passes as
`/fileLogDirectory` and which is an ordinary directory on the host inside the
Proton prefix. `tools/destiny-ref.sh collect` already copies results back into
the repo. A scenario writes `result.json` the same way.

### 2.3 A launcher — **exists**, needs a virtual display

`tools/run-client.sh` already starts the client under Proton with
`/ssoToken` and the log directory, and `tools/up.sh` already starts the server
beside it. What is missing is running it without a monitor: `Xvfb` (or a
headless Wayland compositor) with the real GPU still underneath, since DX11 →
DXVK → Vulkan → RADV needs a device, not a screen.

## 3. The shape of the harness

```
tools/client-ci.sh run <scenario>
  1. start the server (no client)
  2. Xvfb :99                      # real GPU, virtual screen
  3. deploy tools/client-ci/<scenario>.py into C:\eve-logs\
  4. launch the client with the startup hook (§4) and DISPLAY=:99
  5. the scenario, inside the client:
       wait for its ballpark, drive the sequence, assert,
       write result.json, bluepy.Terminate(0)   # NOT blue.os.Quit -- see below
  6. host: wait with a timeout, read result.json, capture a frame,
       compute the frame metrics
  7. exit non-zero on failure, with the client log attached
```

**`blue.os.Quit()` does not exist in this build**, and an earlier revision of
the line above said it did. Nothing in the client's bytecode names it. The
call is `blue.os.Terminate(exitCode)`, wrapped as
`bluepy.Terminate(exit_code, reason='')` -- `bluepy/__init__.py:502`, reaching
`blue.os.Terminate` at `:528` **[V]** -- which is what the client's own quit
button uses (`carbonui/services/command.py:659`,
`eveCommands.py:1254`) **[V]**. The scenarios try four routes and log which
one worked, so they do not depend on this being right; the plan's line was
wrong and is corrected rather than quietly dropped.

**And `michelle.bpReady` does not mean "there are balls"** **[V]**:
`AddBallpark` sets it `True` at `michelle.py:460`, immediately after
`__bp.Start()` at `:459`, while `Park.__init__` begins with
`slimItems = {}` at `:1019`. So `GetBallpark(doWait=True)` can cost 30
seconds of `SleepSim` and hand back an empty park. A scenario waits for the
*ball*, not for the flag.

**Two kinds of assertion, and the first is the valuable one:**

- **From inside**: values the client itself computed — the HUD's shield,
  armour and structure portions; the ego ball's distance from the station;
  the ball's velocity after a throttle command; the set of ball ids in the
  park. This is exactly what a person is being asked to look at today.
- **From a frame**: *scalar metrics*, not image diffs — mean saturation, a
  luminance histogram. Image comparison is flaky for a 3D scene; a number
  with a threshold is not. Saturation alone would have caught the greyscale
  scene on the first run.

## 4. The one unknown, and the spike that settles it

**How does the scenario run without a human typing `/execfile`?**

Two candidate hooks, both read out of `out/pyc/autoexec.pyc`'s own strings and
neither yet confirmed:

- **A startup argument.** The autoexec chain calls `GetArg` and holds
  `allArgs` / `startupArgs` / `start_from_autoexec` `[I]`, and we already pass
  the client arguments (`/ssoToken`, `/fileLogDirectory`). If one of them
  names a script, that is the whole hook.
- **A loose-file override.** It also calls `RegisterFileSystemBeforeLocal` and
  `RegisterFileSystemAfterLocal` `[I]` — a virtual filesystem root registered
  *ahead* of the packaged one, which is the documented shape of a loose `.py`
  override and exactly what `run-client.sh`'s own comment anticipated
  ("later possibly loose .py overrides").

`autoexec.pyc` is 6,960 bytes and loads `autoexec_<role>` modules
(`autoexec_client`, `autoexec_common`, `autoexec_evecore`,
`autoexec_client_core` all ship). Disassembling it settles both questions in
one sitting.

### 4.1 Spike result: `/tools=` is dev-only, and we already have a better hook

**Settled by disassembly, 2026-10-21.**

`autoexec.pyc` really does read a `/tools=` argument (`:111`), and
`autoexec.run()` really is `inittools.run_(tool)` (`:230`). But the selection
above it is **[V]**:

```python
202  if blue.pyos.packaged:
203      tool = None
204  else:
205      import inittools
206      tool = inittools.gettool()
208  if tool is None:
209      __import__('autoexec_%s' % boot.role)
```

A **packaged** client never consults `/tools=` at all -- it imports
`autoexec_client` and starts. Ours is packaged (the same `blue.pyos.packaged`
appears in `IsFullLogging`, where our full-logging comes from the *role mask*
precisely because `packaged` is true). So the startup-argument route is
**closed** without modifying the client, which this project does not do beyond
the one honest `start.ini` change.

`RegisterFileSystemBeforeLocal`/`AfterLocal` at `:94`/`:96` are called with
`'Remote'` and are part of the remote-filesystem debugging path, not a general
loose-`.py` override. Also closed, on current reading.

**The better hook was already ours.** The server hands the client CPython
code objects that the client *executes*, twice, and we build both by hand:

- **H8's `signedFunc`** (`docs/reference/login-path.md` §4): the client does
  `Crypto.Verify` -> `marshal.loads` -> `eval(func, globals(), context)` inside
  `GPS.__Execute` **[V]**. `crate::pycode` emits it today. `__Execute`
  captures stdout/stderr and **swallows any exception**, printing the traceback
  into a buffer -- so a scenario that throws cannot break login.
- **`charMgr.GetSettingsInfo`'s blob**, run later by `settingsSvc` off
  `OnCharacterSessionChanged` **[V]**. `crate::pycode::settings_info_func`
  emits it today.

The second is the better trigger *moment*: H8 runs early, before services and
the session exist, so code evaluated there cannot act -- it could only arm
something. `GetSettingsInfo` runs **after character selection**, when `sm`,
`session` and the service layer are up, which is exactly when a scenario wants
to start.

So the trigger becomes: when the server is started with a scenario selected,
emit a code object whose body `execfile`s
`c:\eve-logs\<scenario>.py` and then returns what the caller expects. That
is a handful of extra opcodes in a module that already hand-assembles code
objects, it needs **no change to the client at all**, and it keeps the normal
path trivial when no scenario is selected.

Open, and to be settled when it is built: whether the scenario should run
inline on that call or spawn a tasklet (`uthread.new`) so the settings call
returns promptly. Inline risks blocking a service the client is waiting on;
a tasklet is how the client does everything else.

### 4.2 Still open: the display

No `Xvfb` and no `weston` on this machine, and the project's standing rule is
**never install anything locally -- build a Docker image**. `Xephyr` is
present but nests inside the user's own session, which defeats the point.

Two honest options:

- **A container with `Xvfb` and the GPU passed through** (`/dev/dri/card1`,
  `renderD128` both exist). Consistent with how everything else here runs, and
  keeps all five oracles including the frame metric. The work is getting
  Proton and its libraries into an image.
- **Software rendering, headless.** Cheaper, and enough for the four
  *from-inside* assertions, which are the valuable ones. The frame metric --
  the one that catches a greyscale scene -- is lost, because what is rendered
  is no longer what the player's GPU would render.

Recommendation: start with whichever gets a scenario running end to end, then
add the GPU path for the frame metric. The oracle that matters most does not
need pixels.

## 5. Honest limits

- **Not hermetic.** Real GPU, a Proton prefix, this machine, and a client the
  owner legally has. It is a smoke test on the developer's box, not something
  a cloud runner can reproduce.
- **Slow.** Client startup is tens of seconds, and this server deliberately
  leaves a number of content services unimplemented, each costing a timeout.
  So: a handful of high-value scenarios, not a broad suite.
- **Complements the gate, does not replace it.** `tools/gate.sh` stays fast
  and runs on every change; the client smoke test runs before a human test or
  overnight, and its failures are diagnosed with the same client log a person
  would read.
- **Never the live install.** The same rule the rest of the tooling follows: a
  copy, a separate prefix, the jail on, and `manifest.dat` kept honest rather
  than bypassed.

## 6. What it unlocks

Unattended overnight work stops being "write code and hope". The loop becomes:
a lane settles what the client requires, the change lands behind the gate, and
a client scenario proves the client accepts it — all without waking anybody.
The human check moves from *every* change to the ones that are genuinely about
how something looks or feels.

It also pairs with documenting the client as we go: every scenario is a second
consumer of the same knowledge the lane docs hold, and a scenario that fails
is a sharper question for the next lane than any amount of reading.
