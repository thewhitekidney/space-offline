# The client's error vocabulary (build V24.01, 3619879)

Lane: `error-vocabulary` (churn Wave 1, cross-cutting). Park note:
[`docs/lanes/error-vocabulary.md`](../lanes/error-vocabulary.md).
Catalogue: [`findings/user-errors.tsv`](../../findings/user-errors.tsv), 4,280 rows.

Claims are marked **[V]** (read from the client's bytecode, from its own data,
or from our source) or **[I]** (inferred). `unknown` is used where it is
honest. **Every client line number here came from `co_lnotab`** — via
`tools/call_lines.py`, `tools/pyc27.py`, or a disassembly pass written for this
lane — never from uncompyle6 output. Decompiled text is quoted for logic only.

**The gap this closes:** our server could not tell the client that something
failed. It can — the mechanism exists in `dispatch.rs` and has been on the
wire — but we had no idea *what* to say, which errors the client acts on, or
whether saying anything helps. §1 is the wire shape, §3 is the subset worth
implementing, §4 is the answer to "does an error break a live lock".

---

## 1. The mechanisms

### 1.1 The only one that matters: a wrapped exception in an `ErrorResponse`

A service method fails by **not sending a `CallRsp` at all**. It sends a
`MachoNet` `ErrorResponse` whose code is `MACHONETERR_WRAPPEDEXCEPTION` and
whose payload is a one-element tuple holding a *nested marshal stream* of the
exception object. **[V]**

```text
ErrorResponse(originalCommand=CallReq, code=2, payload=(<pickled exception>,))
```

| piece | value | citation |
| --- | --- | --- |
| `MACHONETERR_UNMACHODESTINATION` | `0` | `cluster/__init__.py:120` |
| `MACHONETERR_UNMACHOCHANNEL` | `1` | `cluster/__init__.py:121` |
| `MACHONETERR_WRAPPEDEXCEPTION` | **`2`** | `cluster/__init__.py:122` |
| server side builds it | `packet.ErrorResponse(MACHONETERR_WRAPPEDEXCEPTION, (machobase.DumpsSanitized(e),))` | `ExceptionWrapperGPCS.py:63` (in `CallUp`, `:52`) |
| client turns the code into an exception | `raise MachoWrappedException(retval.payload)` | `machoNet.py:4313` (in `_BlockingCall`, `:3518`) |
| client unwraps and re-raises it | `newException = machobase.LoadsSanitized(e.payload[0]); raise newException` | `ExceptionWrapperGPCS.py:178` (in `CallDown`, `:128`) |

`LoadsSanitized` is `blue.marshal.Load(what, SanitizedParseObjectID)`
(`machobase.py:57`), i.e. the **ordinary marshal reader with the ordinary
`globalsWhitelist`**. There is no second, narrower whitelist for exceptions.
**[V]**

**Keep the payload to exactly one element.** With `len(payload) > 1` the client
reads elements 1..6 as a server-side stack report, formats a cross-the-wire
traceback and fires `SendStackTraceAlert` — the shape CCP's own generic
handler emits at `ExceptionWrapperGPCS.py:121`. We have nothing to put in it.
**[V]** Already enforced in `eve_proto::wrapped_exception_payload`.

### 1.2 The exception object: `eveexceptions.UserError`, marshal tag `0x22`

```text
0x22                                     ObjectEx, "call" form
  <header tuple>
    0x02 "eveexceptions.UserError"       global, 24 bytes
    <tuple>
      <str msg>                          the message key
      <dict>                             the arguments, value-then-key on the wire
  0x2D                                   end of the (empty) append stream
  0x2D                                   end of the (empty) setitem stream
```

Built by `eve_proto::user_error` (`server/crates/eve-proto/src/call.rs:174`);
the byte layout is `eve_marshal`'s `ObjectForm::Call` arm
(`eve-marshal/src/value.rs:731`). Both `0x2D` terminators are unconditional
even when the streams are empty. **[V]**

Four things make this exact and not a choice:

1. **`0x22`, not `0x17`.** `class UserError(Exception)`
   (`eveexceptions/__init__.py:14`) and `Exception` is new-style in Python
   2.7, so `PyInstance_NewRaw` does not apply. `0x22` reconstructs as
   `header[0](*header[1])` — it *calls* the class. **[V]**
   (`docs/archive/marshal-tags.md` §0x22.)
2. **The class name must be `eveexceptions.UserError`.** It is in the
   client's `globalsWhitelist` (`carbon/common/lib/whitelist.py`,
   `InitWhitelist` at `:359`; mirrored with tests in
   `eve-rowset/src/whitelist.rs`). **The class's own `__guid__` is
   `'exceptions.UserError'` (`eveexceptions/__init__.py:16`) and that spelling
   will NOT work** — the whitelist resolves dotted names with
   `__import__` + `getattr`, and stdlib `exceptions` has no `UserError`. An
   unlisted name does not degrade: the client aborts the whole read with
   `HACKER WARNING! object %s.%s is not in whitelist`. **[V]**
3. **We send no state.** `UserError.__init__(self, msg=None, *args)`
   (`eveexceptions/__init__.py:19`) does
   `if len(args) and type(args[0]) == type({}): self.dict = args[0];
   self.args = (self.msg, self.dict)`, so calling `UserError(msg, dict)`
   populates `.msg`, `.dict` and `.args` itself. **[V]**
4. **The message key is a `str`, not `unicode`.** `__init__` raises
   `RuntimeError('Invalid argument, msg must be a string', msg)` for anything
   that is not `str`/`unicode`/`None` — both pass, but
   `_handle_player_fsd_dialog` re-checks with `types.StringTypes`
   (`dialog_handler.py:54`). `PyValue::Bytes` is the safe choice and is what
   `user_error` emits. **[V]**

`DumpsSanitized` (`machobase.py:53`) passes `SanitizedGetObjectID`
(`machobase.py:41`) as the writer's object-id callback, which returns `None`
for anything deriving from `Exception` — so a real server marshals the
exception as a plain object, exactly as we do. **[V]**

### 1.3 What the client does with it, end to end

```text
our ErrorResponse(code=2)
  -> machoNet._BlockingCall           raise MachoWrappedException      machoNet.py:4313
  -> ExceptionWrapper.CallDown        LoadsSanitized -> raise          ExceptionWrapperGPCS.py:178
  -> ServiceCallGPCS                  except (…, UserError): raise     ServiceCallGPCS.py:322
  -> the calling UI code              its own `except UserError`, if it has one
  -> otherwise blue.pyos.exceptionHandler                              autoexec_client_core.py:146
  -> eve_exception_handler.handle_exception                            eve_exception_handler.py:39
       if _IsUserError(exc): prompt_player_fsd_dialog(exc.msg, exc.dict); return
                                                                       eve_exception_handler.py:47-48
  -> dialog_handler._handle_player_fsd_dialog                          dialog_handler.py:54
       dialog_data = cfg.GetMessage(dialog_key, params, onNotFound='raise')
  -> eveCfg.Config.GetMessage                                          eveCfg.py:307
       localization.GetByMessageID(bodyID, **params)
```

Three consequences worth carrying: **[V]**

- **`ServiceCallGPCS` re-raises a `UserError` immediately. There is no
  transport-level retry.** Its `while 1:` loop
  (`RemoteServiceCallWithoutTheStars`, `ServiceCallGPCS.py:230`) only re-dials
  for `WrongMachoNode` (`:342`). `UserError` is in the inner re-raise set at
  `:322` and the outer handler at `:353` re-raises it too. So the exception
  lands at the caller exactly where a return value would have. This is the
  fact §4 turns on.
- **`UserError('UnMachoDestination')` and `UserError('GPSTransportClosed')`
  have a transport side effect**: `ServiceCallGPCS.py:354-355` does
  `self.services.clear()` — the client forgets which node every service lives
  on. Never emit those two keys.
- **A key that is not in `dialogs.static` is not silent.** `GetMessage`'s
  `KeyError` path (`eveCfg.py:361`) substitutes
  `ErrMessageNotFound {'msgid': key, 'args': repr(params)}`, and that row's own
  `dialogType` is **`fatal`** **[V]** (read from the table, §2.4). A typo in our
  key is a fatal modal naming our typo, not a no-op — **and we are shipping one
  today**, see §5.

**Whether the player sees a dialog depends on the path, not on the error.** If
nothing catches it the global hook pops a dialog. If the call was made under a
service notify / `ScatterEvent`, `ServiceManager.MollycoddledUthread`
(`serviceManager.py:1011`) catches *everything* and only calls
`log.LogException` — no dialog, and the only evidence is `evecap --alerts`.
**[V]**

### 1.4 `ThrowUserError` does not exist in this build

**[V]** There is no `ThrowUserError` anywhere in `out/pyc/` (12,774 files; a
disassembly-level scan for any name beginning `Throw` finds only
`ThrowErrorIfBelowZero`, `ThrowAway` and a handful of substring accidents). It
is a server-side CCP helper, not part of the client surface. Do not look for
it.

### 1.5 The other exception types that travel, and which are safe to emit

All of these resolve through the same `globalsWhitelist`, so all are
*encodable*. Only the first two are *useful*.

| class | wire form | use it? |
| --- | --- | --- |
| `eveexceptions.UserError` | `0x22`, `(cls, (msg, dict))` | **yes** — the whole vocabulary |
| `eveexceptions.CancelSessionChange` | same, subclass of `UserError` | **yes, narrowly** — see §1.6 |
| `exceptions.RuntimeError` | `0x22`, `(cls, (arg0, …))` | yes — a few handlers match on `args[0]`, e.g. `CharacterNotAtStation` at `invWindow.py:1868` |
| `machoNetExceptions.WrongMachoNode` | wrapped | **no** — `ServiceCallGPCS.py:342` re-dials, unbounded |
| `machoNetExceptions.UnMachoDestination` / `UnMachoChannel` | `ErrorResponse` codes 0/1, or wrapped | **no** — clears the service map |
| `GPSExceptions.GPSTransportClosed` | wrapped; also raised by `machobase.Loads` if it is the *whole* loaded value (`machobase.py:32`) | **no** |
| `machoNetExceptions.ProxyRedirect`, `UberMachoException`, `MachoException` | wrapped | no — unexamined, and nothing wants them |
| `eveexceptions.SQLError`, `ConnectionError`, `UnmarshalError`, `ServiceNotFound` | `0x22` | no |
| `stackless_response_router.exceptions.TimeoutException` | `0x22` | no |

### 1.6 Two adjacent mechanisms that are not errors but belong in the vocabulary

**`CancelSessionChange` — the clean "no" for a session change.** **[V]**
`sessions.PerformSessionChange` (`sessions.py:451`) wraps the mutating call:
on *any* exception it reverts `sess.nextSessionChange` (the session-change
cooldown) and then re-raises — **unless** the exception is a
`CancelSessionChange`, in which case it swallows it (`sessions.py:533`). So
`CancelSessionChange` cancels a dock/undock/clonejump/charsel with no dialog
and no traceback and no cooldown penalty.
**Caveat, and it matters: the caller is not told.** `station/base.py`'s
`_DoUndockAttempt` (`:567`) would go on to `CloseStationWindows()` and return
`True`, so the station UI would be torn down while the player is still docked.
For undock, a real `UserError` key is correct and `CancelSessionChange` is
not. **[I]** — the control flow is **[V]**, the "therefore do not use it for
undock" is the inference.

**`IgnoreToTop` — a `UserError` that shows nothing.** **[V]**
`player_messaging.IGNORE_TO_TOP == 'IgnoreToTop'`, and
`handle_player_fsd_dialog` returns immediately when the key equals it
(`dialog_handler.py:43`). The client raises `UserError(IGNORE_TO_TOP)` itself
to unwind a stack silently (`invControllers.py:493`). This is the only way to
say "no" and render *nothing* — no dialog, and no `ErrMessageNotFound` either.
**[V]** `IgnoreToTop` has no `dialogs.static` row, which is exactly why the
short-circuit at `:43` has to come *before* `cfg.GetMessage`.

**Provisional responses — "not yet, wait longer".** **[V]** A response packet
carrying `oob['provisional'] = (seconds, eventName, args)` does not complete
the call: `_BlockingCall` extends its own timeout by `seconds`, scatters
`eventName`/`args` (prefix `Process` -> `ChainEvent`, `Do` -> `SendEvent`,
otherwise `ScatterEvent`) and **loops back to receive again**
(`machoNet.py:4267-4275`). That is the real server's answer to a slow call. We
do not use it; it is the right tool for a long operation and is noted here so
the next person does not invent a worse one.

**`GenericStopSpamming` — the server's own throttle key.** **[V]**
`sessions.ThrottlePerSeconds(max=1, seconds=10, message='GenericStopSpamming')`
(`sessions.py:66`) is a decorator CCP puts on server methods; the five
instantiations at `sessions.py:66,76,86,96,106` are the stock rates. Client
callers swallow this key specifically (six sites, `findings/user-errors.tsv`).
If we ever rate-limit, this is the key to use.

---

## 2. The catalogue

`findings/user-errors.tsv`, **4,280 rows**, one per key. Three readers, none of
them hand transcription; the `source` column says which contributed each row.

| source tag | rows | reader |
| --- | --- | --- |
| `fsd` | 1,110 | `tools/fsd.py` -> `res:/staticdata/dialogs.static` only |
| `fsd+loc` | 2,625 | …and a `UI/Messages` label |
| `fsd+loc+pyc` | 334 | …and named in the client's bytecode |
| `fsd+pyc` | 154 | in the table and in the bytecode, no label |
| `pyc` | 42 | named in the bytecode only (12 of them free text, not keys) |
| `loc` | 15 | a stale label with no row in the table |

### 2.1 `dialogs.static` is the authoritative key table, and it reads

**[V]** `eveCfg._LoadMessagesFromFSD` (`eveCfg.py:1757-1758`) loads
`res:/staticdata/dialogs.static` with the sidecar `dialogs.schema`.
`tools/fsd.py` (the `fsd-reader` lane's reader) decodes it as a
`classic-sidecar` FSD table: **4,223 keys**, keyed by the message-key string,
each row carrying `dialogID`, `dialogType`, `titleID`, `bodyID`,
`suppressable`, `closable`, `urlIcon`, `urlAudio`.

```sh
EVE_CLIENT=~/.local/share/.../CCP/EVE tools/analysis.sh python3 -I -c "
import sys; sys.path.insert(0,'/work/tools'); import fsd
d = fsd.Client('/client').load('res:/staticdata/dialogs.static')
print(len(d)); print(d['ShipNotInHangar'].dialogType)"
```

**The two readers cross-validate exactly.** For the 2,959 keys present in both,
the `UI/Messages/<num>/…` numeric id equals `dialogID` **2,959 / 2,959** times;
`bodyID` agrees 2,958 and `titleID` 2,951 (a handful of stale labels). Two
independently written readers over two unrelated files agreeing to three
decimal places is the strongest confidence available in this project.

So the earlier worry is settled and the answer is a correction *in favour of*
the corpus: the `UI/Messages` labels are a **denormalised view over part of the
table**, not the table. 154 keys the client names in code have a `dialogs.static`
row but no label, which is why probing the label index for
`CantFitTooManyByType` returned nothing.

### 2.2 `dialogType` — the column that decides whether we pop a window

**[V]** `_handle_player_fsd_dialog` branches on it (`dialog_handler.py:54`):
`info`, `infomodal`, `warning`, `question`, `error`, `fatal` and `windowhelp`
go to `_prompt_player` (`:147`) and **block the tasklet on a message box**;
`notify`, `hint` and `event` go to `message_player` and are a **non-blocking
log line**; `audio` renders nothing at all. The shipped table uses nine of
those:

| `dialogType` | keys | presentation |
| --- | --- | --- |
| `notify` | 1,767 | log line |
| `info` | 1,307 | **modal** |
| `question` | 507 | **modal**, with buttons |
| `audio` | 280 | nothing |
| `hint` | 141 | log line |
| `warning` | 98 | **modal** |
| `error` | 87 | **modal** |
| `windowhelp` | 27 | **modal** |
| `fatal` | 9 | **modal** |

2,035 modal, 1,908 log line, 280 silent. The TSV's `presentation` column
carries this per key. **Pick a `notify` or `hint` key for anything a UI might
ask more than once; pick `info`/`error` only for a user-initiated action.**
There is no `infomodal` or `event` row in this build.

### 2.3 The generic escape hatches — now with their presentation

**[V]** Confirmed from three sides: the raise sites pass these dict keys, the
message body interpolates the same token, and the table gives the type.

| key | dict | `dialogType` | presentation | raise sites |
| --- | --- | --- | --- | --- |
| `CustomNotify` | `{'notify': <text>}` | `notify` | **log line** | 31 |
| `CustomInfo` | `{'info': <text>}` | `info` | modal | 40 |
| `CustomError` | `{'error': <text>}` | `error` | modal | 3 |

These take free text, so **the server can always say something true**, even for
a subsystem with no proper key — and `CustomNotify` can say it without a modal.
`service/social.rs:29` already records
`UserError('CustomInfo', {'info': 'Failed to load mailbox.'})` as the right
answer for a mailbox we cannot load; for a call a window might repeat,
`CustomNotify` is the better of the two.

### 2.4 Keys with no row: `ErrMessageNotFound`, and the handler-only errors

**[V]** `ErrMessageNotFound` is itself a real row — `dialogID` 259709/259710,
`dialogType` **`fatal`**. So a key we emit that is not in the table produces a
*fatal* modal naming our key. That is loud, and it is the failure mode of a
typo.

**14 keys that the client catches by name have no `dialogs.static` row at
all.** These are **handler-only errors**: the handler is expected to consume
them entirely and render something of its own out of the dict, and the key
itself is never meant to reach `cfg.GetMessage`.

```text
AbyssError  AuthenticationFailure  CCInvalidClothing  CharacterNotAtStation
DeniedShipChanged  KillRightExpired  PasswordProtectedCacheInvalid
PVPJumpError  RandomJumpError  SkinAlreadyApplied  TargetingAttemptCancelled
TaxChanged  tokenMissing  VoidSpaceJumpError
```

The consequence is sharp: **emitting one of these to a call path whose handler
is not listening gives the player a fatal "message not found" dialog.** They
are safe only at the exact call sites listed in `caught_by_key_at`.
(`CharacterNotAtStation` is consistent with being a plain `RuntimeError`
rather than a `UserError` — `invWindow.py:1868`.)

### 2.5 The columns read from the bytecode by this lane

| column | meaning |
| --- | --- |
| `raise_sites`, `raised_at` | how many places the *client* raises it, and the first |
| `args_at_raise` | dict keys seen at the raise site. **Approximate** — the scanner walks forward from the `UserError` load and stops at the first store or jump, so a dict spread over branches is under-reported. 71 of the 106 keys where both are known agree exactly with `loc_args`; where they differ, trust `loc_args`. |
| `args_read_by_handler` | dict keys the *handler* reads (`e.dict.get('x')` / `e.dict['x']`) — for a server-raised key this is what we must supply |
| `caught_by_key_at`, `caught_in`, `caught_via` | where the client branches on this specific key, the enclosing function, and whether it compares `.msg` or `.args[0]` |
| `handler_fn_refs` | notable calls present in the handling *function* (function scope, not branch scope — a hint, not a proof) |
| `ui_behaviour` | prose, filled for 64 keys that were read properly; `unknown` on the 55 rows where it was not |

`loc_args` is from `tools/loctable.py`: the `{token}` names the body text
interpolates, and **the authoritative argument list** — 1,226 keys have at
least one.

**No message text is in the TSV.** The prose is CCP's content and
`docs/archive/plan-the-churn.md` §6 forbids committing extracted data; the keys, ids,
presentation and argument *names* are the interface and are what we need. Read
the text with `tools/loctable.py errors <regex>`.

## 3. The errors the client catches *by name* and acts on

**116 keys** are compared against `.msg` or `.args[0]` somewhere in the
client, at 130 sites across 101 functions. The `presentation` column says what
each one draws if it reaches the generic handler instead — and 14 of them draw
a *fatal* dialog, because they have no table row at all (§2.4). **94 of them the client never raises
itself** — they can only come from a server, which makes them a direct
specification of what we are supposed to send. The full set is the
`caught_by_key_at` column; below are the ones whose handler was read, grouped
by what the handler *does*.

### 3.1 Reverts optimistic UI state

| key(s) | call | what the handler does | citation |
| --- | --- | --- | --- |
| anything **except** `EffectAlreadyActive2` | `dogmaIM.SetModuleOnline` | `OnlineModule` activates `effectOnline` locally *first*, then calls us; on any other error it does `StopEffect(effectOnline, moduleID)` and re-raises — the module goes visually offline again | `clientDogmaLocation.py:857` |
| same, softer | — | `_OnlineModuleIfApplicable` shows the dialog on a pool tasklet instead of re-raising | `clientDogmaLocation.py:771` |
| `BookmarkFolderNoLongerThere`, `FolderAccessDenied` | `accessGroupBookmarkMgr.UpdateKnownFolderState` | `self.foldersNew.pop(folderID, None)`, `RefreshUIOnUpdate()`, then re-raise — the folder disappears from the UI | `bookmarkSvc.py:452` |
| `ShipNotInHangar` | `ship.Undock` | reads `dict['capsuleID']`, `MakeShipActive(capsuleID)`, re-raises; the outer `except Exception` then calls `undocking.AbortUndock()` and the station is usable again | `base.py:591` |
| `DeniedShipChanged`, `TargetingAttemptCancelled` | target locking | the lock attempt is abandoned without a dialog | `targetMgr.py:1602`, `:1638`, `playerOwnedTargetMgr.py:104` |

### 3.2 Cancels a retry loop

| key(s) | what the handler does | citation |
| --- | --- | --- |
| `SystemCheck_JumpFailed_Stuck`, `CantJumpEmanationLockedToAnotherGate` | `autopilot.SetOff()` then re-raise — **autopilot stops** | `autopilot.py:526` |
| `SystemCheck_JumpFailed_StandingsTooLow` | dialog, then `SetOff()` and re-raise | `autopilot.py:532` |
| any other `SystemCheck_JumpFailed_*` | dialog only; autopilot keeps trying | `autopilot.py:543` |
| `NotCloseEnoughToJump` | `park.CmdSetSpeedFraction(1.0)` — resumes the approach instead of stopping | `autopilot.py:536` |
| `SystemCheck_SelectFailed_Loading` | **the opposite**: retries 10x at 6 s (a 60 s loop), then `EnableScreen()` and re-raise | `characterSelection.py:912` |
| `JumpNotEnoughCharge3`, `StructureServiceUnavailable`, `CannotUseJumpGateShipTooHeavy`, `NoLinkToDestination` | handled in `_HandleJumpUserErrors`; branch not read | `autopilot.py:543` |

**The contrast at `autopilot.py` is the single best piece of evidence in this
document.** The same call site has two handlers: `except UserError` ->
`_HandleJumpUserErrors`, which can turn the autopilot off; and a bare
`except:` -> `sys.exc_clear(); 'Will try again'; self.ignoreTimerCycles = 5`.
**A named error stops the loop. Any other exception feeds it.** **[V]**

### 3.3 Closes, or declines to open, a window

| key(s) | what the handler does | citation |
| --- | --- | --- |
| `CantViewShipContainer`, `CantViewShipContainerSpace` | `TreeDataFleetHangar.HasChildren` shows the dialog and returns `False` — the tree node collapses instead of the exception escaping | `treeData.py:191` |
| `FolderAccessDenied` | `ShowBookmarkFolderInfo` swallows it, shows `CustomInfo` with a localised label, and **does not open the window** | `bookmarkSvc.py:512` |
| `CharacterNotAtStation` (a plain `RuntimeError`, `args[0]`) | `invWindow.ConstructTree` (`:1859`) returns early — the inventory window survives, though **[I]** the tree loading wheel is left up | `invWindow.py:1868` |
| `SystemCheck_SelectFailed_Queued` | `EnableScreen()` and swallow — the select screen becomes usable and the character stays queued | `characterSelection.py:965` |
| `PetitionClosed` | four sites in the petition window | `petition.py:1057,1065,1097,1112` |

### 3.4 Falls back to a different call — the client re-asks with a flag

This is the most interesting class: the error is a **question**, and answering
it correctly makes a two-phase confirmation work.

| key(s) | the fallback | citation |
| --- | --- | --- |
| `ShipContrabandWarningUndock` | OK/CANCEL prompt; OK re-calls `Undock` with `ignoreContraband=True` | `base.py:597`, `structureDocking.py:45` |
| `JumpCheckWillLoseExistingClone`, `JumpCheckIntoShip`, `JumpCheckIntoStructure` | YES/NO prompt; YES calls `ResetSessionChangeTimer` then re-runs `CloneJump` with the confirm flag set | `clonejumpsvc.py:267` |
| `ConfirmRepackageSomethingWithUpgrades`, `ConfirmRepackageMultipleItemsWithUpgrades` | reads `dict['itemIDs']`, prompts, retries | `invItemFunctions.py:585,588` |
| `ConfirmCreatingMedal` (`args[0]`) | prompts, retries with the confirmation flag | `medals.py:67` |
| `FacWarDonationTaxChanged` / `TaxChanged` | reads `dict['taxNow']`, re-prompts at the new tax, retries | `infrastructureHub.py:324,333`, `importExportUI.py:788` |
| `PermissionDenied` (`args[0]`) | pops a password dialog and retries `StackAll(securityCode=…)` | `invControllers.py:484` |
| `SCGeneralPasswordRequired`, `SCConfigPasswordRequired` | password prompt and retry | `invCache.py:1577,1578` |
| `ContactCostNotApproved`, `ContactCostNotApprovedForMany` | CSPA-charge confirmation, then retry | `eveMisc.py:147,117` |
| `LaunchCPWarning`, `LaunchUpgradePlatformWarning` | confirm, then retry | `eveMisc.py:83` |
| `NotEnoughCargoSpace` | `UnloadAmmoToContainer` substitutes `NotEnoughCargoSpaceToUnloadBank` for a linked bank; `MultiMerge` recomputes the quantity that fits and re-offers the move | `clientDogmaLocation.py:1369`, `invControllers.py:421` |

### 3.5 Keeps partial progress

The handler reads how far the server got and does not undo it. These need the
dict to be right or the client's bookkeeping drifts.

| key | dict key the handler reads | citation |
| --- | --- | --- |
| `ConMultiContractAbortedDueToTidi` | `createdContractIDs` | `createContractWnd.py:1179` |
| `MultipleCrateOpenAbortedDueToTidiCratesOpened`, `FailedOpeningMultiCrate` | `numCratesOpened` | `crate/controller.py:114,115,118` |
| `FleetErrorMany` and four siblings | `notInvited` | `fleetSvc.py:642` |
| `EffectStillActive` | `timeLeft` | `shipmodulebutton.py:1597` |
| `AuthenticationFailure`, `AutLogonFailureTotalUsers` | `position` (login-queue position) | `machoNet.py:1950,1961` |
| `AbyssError`, `PVPJumpError`, `RandomJumpError`, `VoidSpaceJumpError` | `errors` | four activation controllers |

### 3.6 Treated as success

| key | citation |
| --- | --- |
| `EffectAlreadyActive2` — module is already online, keep the optimistic state | `clientDogmaLocation.py:771,857` |
| `SkinAlreadyApplied` | `shipSkinApplicationSvc.py:149`, `skins/controller.py:91` |
| `GenericStopSpamming`, `InventoryStopSpamming` — rate-limited, caller gives up quietly | 7 sites |
| `MonikerSessionCheckFailure` — "the session moved under us", abandon quietly | `autopilot.py:652`, `cameraUtil.py:62` |
| `tokenMissing` — offer lookup gives up with no dialog | `cosmeticsSvc.py:242` and two others |
| `UnMachoDestination` — `sensorSuite.OnEnterSpace` returns quietly | `sensorSuiteService.py:216` |

---

## 4. Does raising an error break a live lock, or feed it?

### **It feeds it. Straight answer: no, an error does not break a live lock.**

**[V]** The mechanism is §1.3's first bullet: `ServiceCallGPCS` re-raises a
`UserError` out of the remote call rather than retrying
(`ServiceCallGPCS.py:322`, `:353`), so the exception arrives at the caller in
exactly the place a value would have. At a guarded cache **the guard is the
assignment**, and an exception means the assignment does not happen — which is
precisely the first live-lock variant in `docs/guides/debugging-the-client.md` §4, where
`None` raised inside a dict comprehension and the cache stayed `None`.

All four documented variants were checked, and **not one of them catches
`UserError`**: **[V]**

| case | guard | what an error does |
| --- | --- | --- |
| `LPSvc.GetAllMyCharacterWalletLPBalances` (`loyaltyPointsWalletSvc.py:35`) | `is None`, class attribute | exception propagates out of `_LoadCharacterLPData`; `_characterLPBalanceByIssuerCorpID` stays `None`; next read re-asks. No handler anywhere in the module. |
| `contractProxy.NumOutstandingContracts` (`createContractWnd.py:1545`) | `is None` + `return` inside `finally` | the `finally: return` discards the exception exactly as it discards the `AttributeError` today; `n` stays 0; cache unset; re-ask, **and now with no traceback either**. |
| `scanSvc.GetScanMan` (`sensorSuiteService.py:897`) | `is None` | `FullyUpdateSignalTrackerDebounced` has no `except`; `remoteObject` stays `None`; the `@uthread2.debounce(1.0)` re-asks forever. |
| `jumpCloneSvc.GetShipCloneState` | truthiness | the store never runs, so the same re-ask as the correct-but-falsy answer. |

So an error is **not a third option** at a guarded cache. It is the same loop,
with one of two things added:

- **nothing catches it** -> `blue.pyos.exceptionHandler` ->
  `prompt_player_fsd_dialog` -> **one rendering every iteration.** How bad that
  is depends entirely on the key's `dialogType` (§2.2): an `info`/`error` key
  is a blocking modal per iteration, which at a render or debounce cadence is
  *worse* than a falsy answer — the XMPP modal-loop shape. A `notify`/`hint`
  key is a log line per iteration, which is merely noise. **So if you must send
  an error into an unguarded path, send a `notify` one.** **[V]** on the
  per-iteration rendering and the type split; **[I]** on "this is what the XMPP
  symptom was".
- **`MollycoddledUthread` catches it** (`serviceManager.py:1011`) -> a
  `log.LogException` per iteration and no dialog. Same invisibility as a falsy
  answer, plus alert noise.

### The rule that follows

**An error is worth sending only where the caller has a handler for it.** The
handler is what latches, cancels or reverts — not the error. §3 is the list of
places a handler exists; everywhere else, `docs/guides/debugging-the-client.md` §4's
original rule still governs and the shape of the *value* is the whole answer.

Two corollaries:

1. **Severity is still set by the caller's cadence, not by the mechanism.**
   `docs/guides/debugging-the-client.md` §4's rule survives this lane unchanged. An error
   does not change the cadence; it changes how loud each iteration is.
2. **The one error that genuinely breaks a loop does it from the handler
   side**, and the proof is `autopilot.py` §3.2: a named key reaches
   `SetOff()`, anything else reaches `ignoreTimerCycles = 5` and tries again.
   Same call, same packet shape, opposite outcome — decided entirely by
   whether the key is one the handler knows.

**Unknown:** whether `log.LogException` in the Mollycoddle path can itself
surface a message window for a `UserError` (it takes a `toMsgWindow`
argument). `log` did not decompile for this lane. It does not change the
answer above — the guard stays unlatched either way.

---

## 5. The ten calls where an error is the correct answer today

Every one of these has an `except UserError` already written in the client and
is currently answered **`None`** — either by an explicit arm or by
`Dispatcher::unhandled` (`dispatch.rs:430`, which logs `UNANSWERED` and
answers `None` so the client keeps going). `None` is the one answer none of
these handlers has a branch for. Ordered by what the owner would notice on a
docked character today.

1. **`dogmaIM.SetModuleOnline`** — unserved -> `None`. The client turns the
   module on *locally first*, then calls us; on any error but
   `EffectAlreadyActive2` it calls `StopEffect(effectOnline, moduleID)` and the
   module goes back to offline (`clientDogmaLocation.py:857`). With `None` the
   fitting window and the module button show a module that is online with
   nothing behind it, for the rest of the session. **The cheapest correct
   error in the list: the key only has to not be `EffectAlreadyActive2`.**
2. **`accessGroupBookmarkMgr.AddFolder`** — `service/world.rs:358` ->
   `None`. `CreateBookmarkFolder` guards `if folder:`
   (`bookmarkSvc.py:398` / call site `:399`), so the dialog closes, no folder appears, and
   nothing is said. Already recorded as a silent lie in `docs/reference/world-state.md`.
   Send `CustomInfo {'info': …}` until folders exist.
3. **`repackagingSvc.RepackageItems`** — unserved -> `None`. Right-click
   Repackage in the hangar does nothing, silently. The handler reads
   `dict['itemIDs']` for the two `Confirm*` keys and prompts-and-retries;
   anything else propagates (`invItemFunctions.py:583`).
4. **`invbroker.StackAll`** — unserved -> `None`. "Stack All" is a silent
   no-op. The handler compares `args[0] == 'PermissionDenied'` and pops a
   password prompt, retrying with `securityCode` (`invControllers.py:484`).
   Note the census records this call site's guard as
   `if_x_is_none+try_except:UserError` — a cache *and* a handler.
5. **`accessGroupBookmarkMgr.UpdateKnownFolderState`** — unserved -> `None`,
   and the consumer is `UNPACK_SEQUENCE 3` (`bookmarkSvc.py:450`), so `None`
   is a `TypeError` today. Send `FolderAccessDenied` and the client evicts the
   folder and refreshes cleanly (`bookmarkSvc.py:452`).
6. **`contractProxy.CreateContract` / `AcceptContract` / `DeleteContract` /
   `PlaceBid` / `CompleteContract` / `FinishAuction`** — all unserved ->
   `None`, in a window that has an `except UserError` on every one of them
   (`contracts.py:525,547,594,667,803,812`). `PlaceBid` branches on
   `args[0] == 'ConBidTooLow'` (`contracts.py:742`).
7. **`insuranceSvc.InsureShip`** — unserved -> `None`; "insure" appears to
   succeed. Handler catches `InsureShipFailedSingleContract`
   (`base_insurance.py:674`), under `try_finally`.
8. **`dogmaIM.UnloadAmmo`** — unserved -> `None`; unloading charges silently
   does nothing. Handler catches `NotEnoughCargoSpace` and substitutes a
   bank-specific message (`clientDogmaLocation.py:1369`).
9. **`structureDocking.Dock`** — unserved -> `None`; a dock request on a
   structure does nothing at all. Handler reads `dict['reason']` and scatters
   `OnDockingRequestWasDenied` unless the reason is `ShipTooLarge`
   (`structureDocking.py:70`).
10. **`crateService.ClaimLootFromCrateStack`** — unserved -> `None`, consumer
    `UNPACK_SEQUENCE 2` under `try_finally` + `except UserError`
    (`crate/controller.py:112,294`). This is the `contractProxy` shape: the
    `TypeError` from unpacking `None` is eaten by the `finally`, so today
    there is **no evidence of it anywhere**. The handler reads
    `dict['numCratesOpened']`.

Close behind, same argument: `killRightMgr.BuyKillRight` /
`ActivateKillRight` (`bountySvc.py:346,358`), `corporationSvc.CreateMedal` /
`GiveMedalToCharacters` (`medals.py:65,145`),
`jumpCloneSvc.OfferShipCloneInstallation` (`clonejumpsvc.py:129`),
`userSvc.ClaimRedeemTokens` (`redeemsvc.py:133`),
`posMgr.AssumeStructureControl` / `RelinquishStructureControl`
(`playerOwned.py:568,595`), `invbroker.ImportExportWithPlanet`
(`importExportUI.py:784`). The complete list is the census: 66 rows of
`findings/client-calls.tsv` have a guard naming `UserError`.

### The bug this lane found on the way past

**`service/store.rs:295` emits `UserError('OfferNotAvailable')`, and there is
no such key.** **[V]** `dialogs.static` has `OfferNotAvailableNow`
(`dialogID` 3592, `info`) and nothing called `OfferNotAvailable`. So
`FastCheckoutService.BuyOffer` currently answers with a **fatal** modal reading
"Could not find message with key OfferNotAvailable" — a worse outcome than the
`None` it replaced. One-character class of fix; `fastcheckout`'s own handler is
at `fastCheckoutClientService.py:271`.

The other four keys we emit are fine, now checked rather than hoped: **[V]**

| our key | arm | `dialogType` | presentation |
| --- | --- | --- | --- |
| `IndustryBlueprintNotFound` | `service/industry.rs:206` | `notify` | log line |
| `IndustryFacilityAccessDenied` | `:364` | `info` | modal |
| `IndustryJobAccessDenied` | `:454` | `info` | modal |
| `SlashError` | `service/gm.rs:73` | `notify` | log line |

`SlashError` being `notify` is a small piece of luck: the `slash.SlashCmd('/')`
trick puts the whole command list through a non-blocking log line rather than a
modal at every login.

### And five places not to send one

- **`ship.Undock`** (`service/inventory.rs:298`) is `None` **on purpose** and
  undocking works (`docs/archive/roadmap.md` M3). Raising there aborts it.
- **`UnMachoDestination`, `GPSTransportClosed`** — clears the client's
  service-to-node map (§1.3).
- **`WrongMachoNode`** — unbounded re-dial (`ServiceCallGPCS.py:342`).
- **`SystemCheck_SelectFailed_Loading`** — a deliberate 10x6 s retry on the
  character-select screen (`characterSelection.py:912`).
- **Any call behind an `is None` cache with no handler** — §4. The error is a
  louder live lock, not a fix.

---

## 6. What this lane did not establish

- **`ui_behaviour` for 55 rows.** Marked `unknown` rather than guessed;
  `caught_by_key_at` points at each one and reading it is minutes of
  `uncompyle6` per key. Pure grind, and the obvious next increment.
- **What the 42 `pyc`-only keys mean.** 12 are free text (CCP devtools calling
  `UserError('Please enter a valid id!')`, which renders as the
  `ErrMessageNotFound` fatal). 14 are the handler-only errors of §2.4. The
  remaining 16 — `AttributeDoesNotExist`, `BlueprintDoesNotExist`,
  `CannotDisbandFleetIfNotBoss`, `ConAuctionBuyoutTooSmall`,
  `LinkDoesNotExist`, `RouteDoesNotExist`, `MaxCorpPlansReached`,
  `RepackageItemNotSingleton`, the four `FailedToOpen*URL` keys and a few
  more — are raised by client code and have no table row, so **[I]** they are
  either dead code or handled by a caller this lane did not find. Not
  investigated.
- **The 15 `loc`-only keys**: labels with no table row. Stale, **[I]**.
- **`log.LogException`'s `toMsgWindow` behaviour for a `UserError`** in the
  `MollycoddledUthread` path. `log` did not decompile for this lane. It does
  not change §4's answer — the guard stays unlatched either way.
- **Live confirmation that the player sees the right dialog.** The wire shape
  is accepted by a real client — `logs/eve-server.log` has
  `facilityManager.GetFacility raises UserError (by design)` x4 and
  `slash.SlashCmd` x1, with no `HACKER WARNING` — so the client *accepted and
  unwrapped* it. What it then drew is unrecorded. **[V]** on acceptance,
  **unknown** on the rendering. §5's item 1 is the cheapest way to find out.
- **`urlIcon` / `urlAudio`.** Read into the TSV's source data but not carried
  as columns and not examined; `audio`-typed keys (280 of them) render nothing
  but a sound.
