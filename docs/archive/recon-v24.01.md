# Recon: EVE client V24.01 (build 3619879)

> **Demoted 2026-10-24** — it pins a build (3619879) the live Steam copy has since left; our jailed `tq-offline` copies still run it. Current truth: [docs/archive/plan-subsystems.md](../archive/plan-subsystems.md) §2a. Kept for history; do not brief a lane from this file.

Read-only analysis of a local, legitimately installed client. No client code or
assets are redistributed here — only our own tooling and our own notes. Anyone
reproducing this needs their own installation.

Install analysed: `.../CCP/EVE/tq`, `start.ini` reporting
`version = 24.01`, `build = 3619879`, `region = ccp`, `branch = //eve/branches/release/V24.01`.

## Headline findings

1. **Still Stackless Python 2.7.** `bin64/python27.dll`. The entire script layer
   is Python; the 2009-era analysis techniques still apply in 2026.
2. **Bytecode is not obfuscated.** `code.ccp` is a *stored* (uncompressed) zip of
   12,794 `.pyj` entries. Each `.pyj` is zlib-compressed, and underneath is a
   standard CPython 2.7 `.pyc` (magic `03 f3 0d 0a`), with original build paths
   intact (`C:\BuildAgent\work\...\eve\release\V24.01\...`). `uncompyle6` class
   tooling applies directly. See `tools/extract_code.py`.
3. **Carbon ships server-side code.** `carbon/common/script/net/` contains the
   shared client/server framework: `machoNet`, `machoNetPacket`,
   `machoNetTransport`, `machobase`, `ServiceCallGPCS`, `SessionChangeGPCS`,
   `serviceManager`, `basesession`, `sessions`. `machoNetTransport` includes
   paths that only ever run on a server — `InstallSessionIfRequired called on %s
   (should only be called on server)`, `RemoveSessionFromServer should only be
   called on a proxy for a proxy/server transport`, proxy/node transport
   bookkeeping, `ServerBroadcast`, and a `zcluster.Attacks_Insert` DB call.
   We are not reverse-engineering the server's interface so much as reading a
   partially redacted copy of it.
4. **Static data is entirely client-side.** 149 `res:/staticdata/*.fsdbinary`
   files (`types`, `groups`, `categories`, `dogmaattributes`, `dogmaeffects`,
   `typedogma`, ...) with 149 matching native `*Loader.pyd` modules in `bin64`.
   The server does not need to serve static data at all.
5. **Two payload encodings, not one.**
   - *Legacy*: machoNet marshalled-Python calls. `machoNetPacket` defines
     `CallReq`, `CallRsp`, `Notification`, `SessionChangeNotification`,
     `SessionInitialStateNotification`, `PingReq`/`PingRsp`,
     `IdentificationReq`/`Rsp`, `TransportClosed`, `ErrorResponse`.
   - *Modern*: protobuf over a request/event gateway (`publicGateway/`), used by
     newer subsystems (store, achievements, skill plans, industry jobs, PLEX,
     sovereignty mercenary dens, community tracks). Directories are literally
     named `grpc`.
6. **The protobuf half of the protocol is fully recoverable.** 1,455 `_pb2`
   modules; every embedded `FileDescriptorProto` parses, giving **5,786 message
   types** and 5 files declaring RPC services. The envelopes are small and clean:
   `eve.Request{correlation_uuid, payload: Any, language, tenant, ...}` →
   `eve.Response{correlation_uuid, status_code, status_message, payload: Any,
   ...}`, plus `eve.Event` for pushes. `Any` means type-URL dispatch, and we have
   every type. This half is mechanical to implement.
7. **Auth is SSO/JWT.** `connectionService` exposes `ConnectSso`, `LoginSso`,
   `GetLoginJWT`, `GetAccessToken`, `access_token`, `EVE-SSO-CONNECTION`,
   and errors with `Trying to do an SSO log in without an SSO token`. Our server
   validates nothing, so the token only has to exist.
8. **Endpoint selection is soft.** `carbon/common/lib/serverInfo.pyc` resolves
   names (`tranquility`, `singularity`, `tranquilityesp`, ...) to
   `{}.servers.eveonline.com` / `{}.evepc.163.com`, but also has
   `checkServerIP` / `checkServerName` paths and `GetServerIP`. `start.ini`
   already carries `server = Tranquility` and `port = 26000`. Redirecting looks
   like a config change, not a patch.
9. **No TLS on the machoNet path (in Python).** `machoNetTransport` does its own
   zlib compression and reads/writes pickles directly; the only `ssl` references
   in the tree are stdlib/`httplib2`/`cherrypy`/`certifi`. Compression is
   negotiable and disableable (`/disablePacketCompression`,
   `machoNet.minimumBytesToCompress`). The gRPC gateway has an explicit
   secure/insecure switch — `connecting to (secure: %s): %s`.
10. **The sandbox we want is still in the client.** `eve/devtools/script/` ships
    (UI debugger, type browser, warp vector, window manager, ~hundreds of
    modules), there are QA-tools clients throughout, a `/localdevtools` slash
    command, and a full role mask in `carbon/common/script/sys/serviceConst`:
    `ROLE_ADMIN`, `ROLE_PROGRAMMER`, `ROLE_QA`, `ROLE_SPAWN`, `ROLE_WORLDMOD`,
    `ROLE_GML/GMH/GMS`, `ROLE_HEALSELF`, ... These are gated server-side, i.e.
    by us. Remote services include `slash.SlashCmd` and
    `publicQaToolsServer.{MoveMeTo, SlashCmd, CanGiveItemForMultifit}`.

## The surface to implement

Statically recovered: **175 remote (server-side) services** referenced by client
code, with method names — see `findings/remote-services.txt`. Highlights on the
critical path:

| service | why it matters |
|---|---|
| `authentication` | `AccruedTime`, `AmUnderage`, `SetLanguageID` |
| `charUnboundMgr` | `GetCharacterInfo`, `GetNumCharacters`, `SelectCharacterID`, `CreateCharacterWithDoll` |
| `charMgr` | 27 methods: contacts, notes, `GetHomeStationRow`, description |
| `stationSvc` | `GetStation`, `GetStationItemBits`, `GetSolarSystem` |
| `dogmaIM` | `CreateNewbieShip`, `GetRequiredSkillLevels` |
| `beyonce` | movement/formations (`GetFormations`, `AdminGetBubbleID`, cyno jammer state) |
| `machoNet` | `GetTime`, `SetGlobalConfigValue`, cluster stats |
| `slash` | `SlashCmd` — 97 references; the dev-sandbox entry point |
| `keeper` | dungeons, `ActivateAccelerationGate`, level editor |
| `invCache`/`invtools` | inventory |
| `skills`, `skillqueue`, `wallet`, `market`, `industryManager` | progression and economy |

This is an undercount: bound-object calls (inventory, dogma, ship control) do
not go through `sm.RemoteSvc` and so do not appear in this scan. Enumerating
those needs either bytecode analysis of the bind sites or in-process observation.

Note how much of the 175 is *not* on the critical path: petitions, bounties,
ESS, insurgencies, Project Discovery, SKINs, PLEX, store, tournaments,
recruitment ads, mutual wars. Those can return empty or be stubbed.

`findings/client-services.json` additionally lists the 303 client-side service
classes with their public methods, and `findings/proto-messages.txt` the 5,786
protobuf message types.

## What is now the hard part

Not the protocol — the protocol is unusually legible. The hard parts, in order:

1. **Destiny (space physics).** `bin64/_destiny.dll`, `carbon_destiny v2.1.1`.
   It is native on the client, so there is no Python to read and no Python to
   port. State blobs are packed binary, not marshal. This is the single biggest
   unknown and the thing standing between "docked" and "undocked".
2. **Bound objects.** The dispatch pattern for inventory/dogma/ship control,
   which static scanning of `RemoteSvc` call sites misses entirely.
3. **Rowset schemas.** Field *names* are readable from attribute access; column
   order and types are not, and the packed-row encoding is sensitive to both.
   Needs observed examples.
4. **Sequencing.** Which notifications must precede which during session
   establishment. Only observation answers this.
5. **Breadth.** A 2026 client touches far more services at login than a 2011 one.

## Open questions

- **Script override.** Unresolved, and it is the most important open question,
  because it decides whether we can *delete* subsystems instead of implementing
  them. `start.ini` has `useScriptIndexFiles = 1`, but `resfileindex.txt`
  contains no `.pyj` or script entries, and `blue.dll` yields only `PYTHONPATH`
  on a strings pass. Needs proper work on `blue.dll`'s loader.
- Whether the machoNet transport is encrypted below the Python layer (inside
  `blue.dll` / carbonio), and what the native framing is.
- The FSD binary format, to read `*.fsdbinary` server-side (or substitute the
  public SDE).
- Whether in-process injection into the embedded interpreter is practical under
  Proton/Wine, which is how this install runs.
- ~~How much the gRPC gateway can simply be switched off before the UI
  breaks.~~ **Answered.** It cannot be switched off, and it does not need to
  be. There is no disabled path -- `ResponseChannel.receive()` waits out its
  timeout however the connection is doing -- but each request is a separate
  tasklet, so every gateway subsystem failing costs one logged exception
  apiece and blocks nothing. A session that drew the hangar had six of them
  outstanding. And when we do want to serve it, the endpoint is redirected by
  the `EVE_PUBLIC_GATEWAY_ADDRESS` environment variable, whose branch also
  returns `secure=False`: no client patch, no TLS, straight at the tcp 8443
  the jail already allows. See `docs/guides/debugging-the-client.md` section 3c.

## Tooling

In `tools/`, all read-only and dependency-light (`protos.py` wants `protobuf`):

| tool | purpose |
|---|---|
| `extract_code.py` | `code.ccp` → tree of 2.7 `.pyc` |
| `pyc27.py` | Python 2.7 marshal parser + bytecode disassembler, usable from Python 3 |
| `build_index.py` | flatten every string const and name into a greppable `index.txt` (~1.25M lines) |
| `strings_of.py` | dump one module's strings, as a cheap stand-in for reading it |
| `scan_services.py` | find service classes via `__servicename__`/`__guid__` |
| `scan_remote.py` | find `sm.RemoteSvc('x').Method()` call sites → server interface |
| `protos.py` | recover `FileDescriptorProto`s from `_pb2` modules |

A real decompiler (`uncompyle6` or `pycdc`) is still worth setting up; these
tools were written to avoid needing one for the first pass.

---

# Round 2: override, anti-tamper, phone-home

## Override: answered, and the answer is yes

The question was whether we can make the client load our Python instead of
CCP's. Every ingredient ships.

**1. The importer accepts plain `.py` source.** The zipimport search-order table
lives at file offset `0x2dc160` in `python27.dll` (RVA `0x2de160`, section
`.data` — writable), stride 20 bytes, and reads:

| suffix | flags | meaning |
|---|---|---|
| `/__init__.pyj` | `0x07` | package \| bytecode \| CCP-zlib |
| `/__init__.pyc` | `0x03` | package \| bytecode |
| `/__init__.pyo` | `0x03` | package \| bytecode |
| `/__init__.py`  | `0x02` | package \| source |
| `.pyj` | `0x05` | bytecode \| CCP-zlib |
| `.pyc` | `0x01` | bytecode |
| `.pyo` | `0x01` | bytecode |
| `.py`  | `0x00` | source |

CCP added `.pyj` (= zlib over standard 2.7 bytecode) at the front of the
standard CPython table and left `.pyc`/`.pyo`/`.py` in place. Source import is
not a leftover we have to re-enable; it is live.

Note also `python27.dll` is CCP's own Stackless fork — the embedded build path
is `...\carbon\src\stackless\python\import.c`.

**2. There is no import guard in the retail client.** `autoexec` does install
one, but only here:

```python
allArgs = blue.pyos.GetArg()
containsToolParam = any(a.startswith('/tools=') for a in allArgs)
if '/jessica' in allArgs and not containsToolParam:
    import devenv
    sys.path.append(os.path.join(devenv.SHARED_TOOLS_PYTHONDIR, 'lib27xccp'))
    import packageaddwatcher
    packageaddwatcher.guard_metapath(boot.role)
```

`/jessica` is CCP's internal dev environment, and neither `devenv` nor
`packageaddwatcher` ships in `code.ccp` — so this branch cannot even execute on
our install. `sys.path` is unguarded.

**3. CCP's own hot-reload ships.** `carbon/common/lib/codereloading.pyc` —
`InstallSakeAutocompiler`, `InitializeCodeReloadSpy`, `InitializeFSDReloadSpy`,
`FileSystemObserver` — built on `watchdog` (which also ships, `watchdog/observers`)
plus `xreload`/`codereload`. It watches the filesystem and hot-swaps modules.
There is a `/disableSake` argument, and the install is gated on `boot.packaged`.

**4. A remote debugger attach path ships.** `autoexec` installs
`GOPYCHARM5`, `GOPYCHARM2017`, `GOPYCHARM2019` and `NOPYCHARM` into
`__builtin__` from a `pydevdebugging` module.

**5. The launcher can be bypassed.** `/ssoToken=<token>` and `/refreshToken=`
are recognised command-line arguments, parsed in `gameui.pyc` and
`loginII.pyc` (which carry the error string `Invalid format of SSO token, should
be /ssoToken=<token>`), and filtered out of the argument string the client
echoes. `/path` is recognised and stripped alongside them; its exact semantics
are not yet pinned down, but a `sys.path`-related meaning is the obvious read
and worth 20 minutes to confirm.

Other arguments seen: `/localdevtools` (imports `localdevtools.startup`, which
does *not* ship, and is gated on `not boot.packaged`), `/thinclient`,
`/skiprun`, `/disableSake`, `/enableTelemetry`, `/disablePacketCompression`.

**What we must not touch:** `manifest.dat` is an asymmetrically signed list of
file hashes (`blue.dll`: `UnpackManifest: Invalid manifest signature`,
`VerifyManifestEntry: Invalid manifest entry hash`, `Gets a shared asymmetric
cipher which should be initialized with the manifest key`) and it **does** cover
`root:\code.ccp`, `app:\start.ini`, `app:\resfileindex.txt`, `python27.dll` and
every DLL/pyd in `bin64`. We cannot re-sign it. We also don't need to: the whole
point of source override is that no shipped file changes. Whether verification
blocks startup or merely reports is still unknown and is now a low-priority
question, since nothing we plan trips it.

So the plan is: a sibling directory of `.py` files, ahead of `code.ccp` on
`sys.path`, with `codereloading` for iteration. Remaining work is confirming the
path-ordering mechanism, in this order: `/path`, then `PYTHONPATH`, then
patching the 20-byte search-order table in `.data` as a fallback.

## Anti-tamper: there isn't any (for us)

`kiringMgr.ShouldKickCheater` and `GetEveGuardServerAddress` led me to expect an
anti-cheat problem. It isn't one:

- **EVE Guard is NetEase's.** `serenity/eveguard/client/eveguard_service.pyc` —
  package `serenity` (the Chinese server), endpoint `https://ac.evepc.163.com/`,
  a handshake/heartbeat/terminate pipe protocol to an external agent.
- **`kiring` is the NetEase account SDK**, and its constants are Chinese
  regulatory compliance: `ANTI_ADDICTION_CHECK_FAIL_JUVENILES_PLAY_TIME_OVERFLOW`,
  `CNYPurchaseFailUnder16MonthlyTooHigh`. Not integrity checking.

On `region = ccp` neither is in the path, and both are server-addressed by
services we implement ourselves. The only integrity machinery that applies to us
is the signed manifest above, which we route around rather than defeat.

## Phone-home

Full list with provenance in `findings/outbound-endpoints.txt`. Summary:

- **Telemetry is opt-in.** `autoexec` gates it on `/enableTelemetry`. Honeycomb
  and Datadog are therefore quiet by default.
- **Crash reporting is not.** `exefile.exe` embeds
  `https://sentry.io/api/1434648/minidump/` and ships Crashpad
  (`eve_crashmon.exe`). Native, always on.
- **LaunchDarkly is native and fails soft** — `launchdarkly/loader.pyc` catches
  client-creation failure, so blackholing it is safe.
- **Required:** `servers.eveonline.com` (which we replace) and
  `clientresources.eveonline.com` (resource CDN).

Approach: a local jail — deny outbound to everything except our own server, at
the firewall, rather than patching call sites. It needs no override, so it can be
built now; it cannot be defeated by a code path we missed; and it is isolation
rather than anything that would help a modified client talk to Tranquility.
Module-level disabling comes later as tidiness, not as the mechanism.

---

# Round 3: how override actually attaches (`/path` resolved)

Decompiled `autoexec.py` and disassembled `blue.dll`/`python27.dll` in the
analysis container to pin the injection mechanism. Result: the two *cheap*
mechanisms are dead, source override itself is alive, and the attach point is
now a narrow, known choice.

## `/path` is a dead end

`/path` has no consumer anywhere — not in `code.ccp`, not in any binary. Its
only appearance is in `autoexec`, which strips `/path`, `/ssoToken` and
`/refreshToken` out of the argument string *before* storing it as the Sentry
`startupArgs` crash key:

```python
for each in orgArgs:
    if not each.startswith('/path') and not each.startswith('/ssoToken') \
       and not each.startswith('/refreshToken'):
        args += each + ' '
blue.SetCrashKeyValues('startupArgs', args)
```

That is privacy redaction of the crash report, not argument handling. `/path`
does nothing on this build.

## The two cheap hooks are both dead (confirmed at the instruction level)

`blue.dll`'s Python init writes the CPython flag globals before any user code:

```
mov rax,[Py_NoSiteFlag]            ; inc dword [rax]
mov rax,[Py_IgnoreEnvironmentFlag] ; inc dword [rax]
mov rax,[Py_DontWriteBytecodeFlag] ; inc dword [rax]
mov rax,[Py_OptimizeFlag]          ; inc dword [rax]
```

Consequences:

- **`Py_NoSiteFlag` = 1** → `site.py` never runs at startup. And
  `sitecustomize.pyj`/`usercustomize.pyj` are not in the archive, so even the
  manual `execsitecustomize()` path (stock `site.py` ships and would import a
  loose `sitecustomize`) is never reached. The `site` hook is out.
- **`Py_IgnoreEnvironmentFlag` = 1** → `PYTHONPATH` (and all `PYTHON*` env) is
  ignored. Env-based injection is out — which also means the jail/Proton
  environment cannot accidentally alter the client's import path.

Neither finding is a setback for isolation; both simply remove injection
avenues we would otherwise have reached for.

## What remains, and the recommendation

`sys.path` is built inside CCP's `python27.dll` fork via a custom
`Py_GetPathWData` export (not stock `PySys_SetPath`; `blue.dll` does not call
the standard path API). The archive is mounted under CCP's own `root:/ app:/
script:/` path-alias scheme, the same `ResFile` layer that already does
loose-file-beats-archive resolution for `res:/` assets.

Two viable attach points, in order of preference:

1. **Earliest-Python `sys.path` insert.** The first user Python to run is
   `autoexec`. We cannot edit it (manifest-signed via `code.ccp`), but we do not
   need to: any module imported *before* the one we want to override can do
   `sys.path.insert(0, OUR_DIR)`, after which our loose `.py` wins over the
   archive copy (source import is live — see Round 2). The clean way to get in
   that early is a tiny bootstrap loaded ahead of `autoexec`; candidates to
   confirm empirically are the `blue`/`eveprefs` init order and whether the
   `ResFile` loose-file rule already covers `script:`-namespace imports.
2. **A proxy/shim DLL** next to `exefile.exe` that runs before `Py_Initialize`
   and inserts our directory, then forwards to the real DLL. Heavier, Proton-
   specific, but mechanism-independent of CCP's path code. Fallback only.

The suffix-table patch floated in Round 2 is downgraded to a last resort: it
requires writing to a manifest-covered DLL in memory and buys nothing that
option 1 doesn't, since `.py` import already works without it.

## Status of the override question

Resolved in principle: source override works, no guard runs, and the attach is
one of the two options above. What is left is a single empirical test — drop one
loose `.py` shadowing a late-loaded module and confirm it wins — which needs the
client actually running inside the jail against at least a stub server. That
makes it the *first* milestone once a minimal listener exists, not a separate
investigation.
