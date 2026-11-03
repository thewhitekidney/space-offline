# The session object and the role mask — what unlocks the dev/GM tools

Client build V24.01, 3619879. Recovered by decompiling the client's own
unobfuscated Python 2.7 (`out/pyc/`, 12,774 modules) with `uncompyle6` in the
analysis container, cross-checked against raw bytecode via `tools/pyc27.py`
wherever a literal mattered.

Confidence markers as in `docs/reference/dbrow.md` / `docs/reference/object-wrapper.md`: **[V]** read from
this build's code, **[I]** inferred, **[?]** open.

Nothing here was taken from published emulator lore. Where a well-known role
name turned out to have **no reader at all in this build**, that is said
explicitly (§4.3) rather than quietly dropped.

---

## 1. The answer, up front

**The lever is `session.role`, a 64-bit mask the server sends in the session
notification.** The client reads it in **216 modules, 368 times** (§2.3). There
is no other switch: no client-side flag, no `start.ini` key.

### 1.1 Recommended mask

```text
role = 7059392768105517056          # 0x61F8005200E41000
```

That is exactly:

```python
ROLE_LOGIN | ROLE_PLAYER                      # required, see §2.4
| ROLE_GML | ROLE_GMH | ROLE_GMS               # GM tooling
| ROLE_WORLDMOD | ROLE_SPAWN                   # world modification / spawning
| ROLE_HEALSELF | ROLE_HEALOTHERS              # repair / resurrect tooling
| ROLE_QA | ROLE_PROGRAMMER | ROLE_ADMIN       # dev windows, Python console
| ROLE_CONTENT                                 # dungeon editor
| ROLE_TOURNAMENT | ROLE_LEGIONEER             # two extra menu branches
| ROLE_CHTADMINISTRATOR                        # chat admin
```

Every bit in it has at least one reader in this build. It fits in a **signed
int64**, so it marshals as tag `0x03` with the 8 bytes
`00 10 e4 00 52 00 f8 61`. **[V]**

**Until the server answers character creation and the localization service,
use the reduced form instead** (drops `ROLE_QA` and `ROLE_CONTENT`, the only
two bits with a measured cost — §5):

```text
role = 7018860371459182592          # 0x6168005200E41000
```

### 1.2 Do **not** use `ROLE_ANY`

`docs/archive/session-flow.md` and `docs/reference/login-path.md` both recommend
`ROLE_ANY & ~ROLE_SERVICE`. **That mask does not contain `ROLE_GML` or
`ROLE_GMH`** — the two roles the client tests most often. See §7; this is the
single most important correction in this document.

```python
ROLE_ANY = 0xFFFFFFFFFFFFFFFF & ~(ROLE_IGB | ROLE_EXT_GM1_PLUS | ROLE_EXT_GM2_PLUS | ROLE_EXT_GM3_PLUS)
#                                              ^ and ROLE_EXT_GM*_PLUS each include ROLE_GML | ROLE_GMH
```

so `ROLE_ANY` = `0xFF9C3FFF7FFFFFFF` = **18418666942635180031**, with bits 53
(`ROLE_GMH`), 54 (`ROLE_GML`), 31 (`ROLE_IGB`) and 46–49 (`ROLE_EXT_GM1..4`)
**cleared**. A session with `ROLE_ANY & ~ROLE_SERVICE` gets the Insider window
and the QA/PROGRAMMER tools but **no "GM / WM Extras" submenu, no GM slash
menus, no Inventory Tools, no UI debugger** — i.e. it misses most of what the
Singularity experience actually consisted of. **[V]**

### 1.3 Never set

| role | why |
|---|---|
| `ROLE_SERVICE` | **fatal.** Two independent guards raise `RuntimeError` on any session change (§5.1) |
| `ROLE_REMOTESERVICE` | inert client-side, pairs with SERVICE semantics; no upside |
| `ROLE_CHTINVISIBLE` | classifies you `INVISIBLE` and filters you out of member lists |
| `ROLE_PINKCHAT` | classifies you as an **NPC** in chat |
| `ROLE_NEWBIE` | starts a rookie-help-channel join tasklet; also excluded from `ROLEMASK_ELEVATEDPLAYER` by CCP |
| `ROLE_IGB`, `ROLE_EXT_GM1..4` | zero readers in this build, and CCP's own `ROLE_ANY` clears them |

---

## 2. The session object

### 2.1 Class layout

`carbon/common/script/sys/basesession.pyc` defines `CoreSession` and
`Session(CoreSession)`. There is **no EVE-specific session subclass in this
build** — the EVE attributes are added to `Session` inside Carbon itself.
`eve/common/script/sys/eveSessions.pyc` only subclasses the *service manager*
(`SessionMgr`), not the session. **[V]**

The client's session is created once at boot:

```python
# autoexec_client_core.pyc
session = basesession.CreateSession(None, SESSION_TYPE_GAME)
__builtin__.session = session
__builtin__.charsession = session
```

`CreateSession(sid=None, sessionType=5, role=ROLE_LOGIN)` — so before any
server packet, `session.role == ROLE_LOGIN` and `session.sessionType == 5`.
`SESSION_TYPE_GAME = 5`, and `VALID_SESSION_TYPES = (1, 2, 4, 5)`
(`carbon/common/lib/session.pyc`); anything else raises `ValueError` in
`CoreSession.__init__`. **[V]**

> **Decompiler caveat.** `uncompyle6` renders several of these class-level
> string lists as bare integers — they are `LOAD_CONST` *indices*, not values.
> Every attribute list below was read from the raw bytecode with
> `tools/pyc27.py`, not from the decompiler output.

### 2.2 The attribute lists

**`Session.__persistvars__`** — read-only via `__setattr__`, and the basis of
`GetDistributedProps()`, i.e. **exactly what travels in a session packet**
(34 names; the 9 from `CoreSession` first): **[V]**

```text
userid  languageID  role  charid  address  userType  maxSessionTime
sessionType  countryCode
regionid  constellationid  allianceid  warfactionid  corpid  fleetid
fleetrole  wingid  squadid  shipid  stationid  structureid  solarsystemid
solarsystemid2  hqID  baseID  rolesAtAll  rolesAtHQ  rolesAtBase
rolesAtOther  genderID  bloodlineID  raceID  corpAccountKey  inDetention
```

**`Session.additionalDistributedProps`** = `['locationid', 'corprole']`, so the
full distributed set is those 34 **plus** `locationid` and `corprole`
(36 names). Both are also in `additionalNoSetAttributes` and
`additionalAttributesToPrint`. **[V]**

Other lists set in `Session.__init__`: **[V]**

| list | value |
|---|---|
| `__nonpersistvars__` | `CoreSession`'s (`sid c2ooid connectedObjects connectedServices clientID localSID contextOnly receivedVersion irrelevanceTime`) + `locationid`, `corprole` |
| `__attributesWithDefaultValueOfZero__` | `corprole rolesAtAll rolesAtHQ rolesAtBase rolesAtOther` |
| `additionalNonIntegralAttributes` | `fleetid fleetrole wingid squadid` |
| `longAttributes` | `corprole` *(note: **not** `role`)* |
| `additionalHexAttributes` | `corprole rolesAtAll rolesAtHQ rolesAtBase rolesAtOther` |

`CoreSession.__characterDependantAttributes__` (cleared on character logout):
`objectID wingid constellationid regionid locationid shipid squadid
solarsystemid charid corpid structureid stationid fleetid warfactionid
allianceid solarsystemid2 inDetention`. **[V]**

`CoreSession.__userDependantAttributes__` (kept across character changes):
`userid languageID address userType maxSessionTime countryCode role`. **[V]**

**Set at initialisation vs. only on change.** Nothing except the constructor
arguments is set at init: `CoreSession.__init__` installs
`role`, `sessionType`, `sid`, `localSID` and the bookkeeping dicts, then runs
every name in `__persistvars__` through `GetDefaultValueOfAttribute`. The
defaults are: `role → ROLE_LOGIN`; `sessionType → itself`; the five
zero-default names → `0`; `languageID` on the client →
`strx(prefs.GetValue('languageID', 'EN'))`; **everything else → `None`**.
So a server that omits an attribute from `initialstate` leaves it at the
default — and crucially, **omitting `role` leaves the client at bare
`ROLE_LOGIN`**, no player rights. **[V]**

Dependent attributes are **recomputed locally**, never sent:
`Session.RecalculateDependantAttributes` derives `locationid` from
`stationid or solarsystemid`, and `corprole` from
`rolesAtAll | rolesAt{HQ,Base,Other}` keyed on `stationid == hqID / baseID`.
Which is why `hqID`, `baseID` and `rolesAt*` have **zero direct readers**
in the whole tree (§2.3) — they exist only to feed `corprole`. **[V]**

### 2.3 Which attributes the client actually reads

Census over **all 12,774 modules**: every `LOAD_GLOBAL/LOAD_NAME session`
immediately followed by `LOAD_ATTR <name>`. **[V]**

| attribute | reads | modules | |
|---|---:|---:|---|
| `charid` | 1127 | 436 | the hot one; routes the entire post-login UI |
| `shipid` | 790 | 270 | |
| `solarsystemid2` | 586 | 224 | |
| `corpid` | 567 | 208 | |
| `stationid` | 424 | 144 | |
| **`role`** | **368** | **216** | **the subject of this document** |
| `structureid` | 307 | 112 | |
| `solarsystemid` | 296 | 119 | |
| `allianceid` | 290 | 98 | |
| `corprole` | 172 | 93 | derived locally |
| `warfactionid` | 160 | 59 | |
| `userid` | 81 | 49 | |
| `fleetid` | 79 | 27 | |
| `languageID` | 73 | 33 | |
| `locationid` | 62 | 33 | derived locally |
| `regionid` | 59 | 34 | |
| `corpAccountKey` | 39 | 20 | |
| `constellationid` | 28 | 19 | |
| `fleetrole` | 24 | 6 | |
| `raceID` | 20 | 17 | |
| `sid` | 10 | 4 | |
| `wingid` | 7 | 2 | |
| `genderID` | 6 | 3 | |
| `bloodlineID` / `squadid` | 5 | 3 / 2 | |
| `clientID` | 3 | 2 | |
| `maxSessionTime` | 3 | 1 | |
| `countryCode` | 2 | 1 | |
| `address`, `userType`, `sessionType`, `inDetention` | 1 each | 1 | |
| `hqID`, `baseID`, `rolesAtAll`, `rolesAtHQ`, `rolesAtBase`, `rolesAtOther` | **0** | **0** | never read directly |

**Getting these wrong breaks the UI**, in rough order of blast radius:
`charid` (absent ⇒ character select; present ⇒ station/space), then
`shipid`/`stationid`/`solarsystemid2`, then `corpid`/`allianceid`, then `role`.
Only three modules in the tree read `session.role` without naming a `ROLE_*`
constant (`chat/client/chat_service.pyc` via
`chat.common.util.get_classification_from_role`, plus two server-side HTML
writers), so **the `ROLE_*` constant census in §4 is complete** — no module
tests the mask against a bare numeric literal. **[V]**

### 2.4 Guards that fire on every session notification

`CoreSession.ApplyRemoteAttributeChanges(changes, initialState)` — the single
entry point for both packet types: **[V]**

| condition (evaluated against the **pre-change** role) | effect |
|---|---|
| `self.role & ROLE_SERVICE` | `RuntimeError` — "A service session may not change via …" |
| `not self.role & (ROLE_LOGIN \| ROLE_PLAYER)` | `LogSessionError` (not fatal) — "a distributed session's role should probably always have login and player rights" |
| `'userid' in notify` and both old and new truthy | `RuntimeError("A session's userID may not change")` |
| not `initialState`, `'role' in notify`, `self.charid` set, and the new role is neither `old \| ROLE_PLAYER` nor `old & ~ROLE_PLAYER` | `LogSessionError` — "a session's role should probably not change for active characters" |

That last one is worth planning around: **once a character is selected, change
the role only by toggling `ROLE_PLAYER`**, or accept a logged session error on
every other role edit. **[V]**

Note also that `ApplyRemoteAttributeChanges` does **not** filter unknown
attribute names (unlike `SetAttributes`, which deletes anything not in
`__persistvars__` and `int()`/`long()`-coerces the rest). Whatever the server
puts in `initialstate` is set verbatim, with no type coercion. **[V]**

---

## 3. The role mask

### 3.1 Where it is defined

`carbon/common/script/sys/serviceConst.pyc` — the only definition in the
tree. Re-exported by `carbon/common/script/sys/service.pyc` (so client code
says either `serviceConst.ROLE_X` or `service.ROLE_X`). `eveservices/roles.pyc`
provides the helpers:

```python
def has_any_role(session, role):      return bool(session.role & role)
def has_specific_role(session, role): return bool(session.role & role == role)
def remove_role(role, role_to_remove): return role & (SERVICE_ALL ^ role_to_remove)
def add_role(role, role_to_add):       return role | role_to_add
```

`SERVICE_ALL = 18446744073709551615` (`cluster/__init__.pyc`). **[V]**

There is **no `IsRole` function anywhere in this build** — `grep` over all
12,774 modules finds none. Role tests are written out longhand as
`session.role & ROLE_X`, occasionally `== ROLE_X` for an all-bits test, and
once as `sessionChecker.IsElevatedPlayer()` / `HasGMHRole()`
(`menucheckers/sessionChecker.pyc`). **[V]**

### 3.2 Numeric type — this matters for marshalling

Every constant from bit 31 upward is a **Python long** in the source
(`ROLE_IGB = 2147483648L`). On Win64 a CPython 2.7 `int` is **32-bit**, so any
mask above `2**31 - 1` is a `long` on this client regardless.

The relevant marshal tags (`docs/archive/marshal-tags.md`):

| tag | meaning | note |
|---|---|---|
| `0x03` | int64, 8 bytes LE two's complement → `PyLong_FromLongLong` | **use this** for a mask < 2**63 |
| `0x2F` | arbitrary-precision int: escaped length + bytes, `_PyLong_FromByteArray(…, little_endian=1, is_signed=1)` | needed above 2**63 |

Because `0x2F` is **signed**, a mask with bit 63 set needs a trailing `0x00`
sign byte — 9 payload bytes for a 64-bit value. **Bit 63 is unassigned: no
`ROLE_*` uses it.** So keep bit 63 clear and the whole mask fits tag `0x03`
with no special handling. `ROLE_ANY` *does* set bit 63 (it is all-ones minus
exclusions, and the 12 unassigned bits 0, 13, 15, 16, 25, 26, 28, 30, 42, 43,
44, 63 are all set in it) — a second, smaller reason not to use it. **[V]**

The client never coerces the received value: `__ChangeAttribute` writes it into
`__dict__` as-is, and all readers use `&`, which auto-promotes. A `long` is
therefore correct and safe. **[V]**

### 3.3 The constants, from this build

Single bits, with the number of **client-side** test sites (whole tree,
excluding the `serviceConst` definitions themselves):

| bit | hex | decimal | name | refs |
|---:|---|---:|---|---:|
| 1 | `0x…0002` | 2 | `ROLE_BANNING` | 0 |
| 2 | `0x…0004` | 4 | `ROLE_VMSADMIN` | 0 |
| 3 | `0x…0008` | 8 | `ROLE_MARKETH` | 0 |
| 4 | `0x…0010` | 16 | `ROLE_CSMADMIN` | 0 |
| 5 | `0x…0020` | 32 | `ROLE_CSMDELEGATE` | 0 |
| 6 | `0x…0040` | 64 | `ROLE_PINKCHAT` | 1 |
| 7 | `0x…0080` | 128 | `ROLE_VGSADMIN` | 0 |
| 8 | `0x…0100` | 256 | `ROLE_PETITIONEE` | 1 (reads *other* chars' roles) |
| 9 | `0x…0200` | 512 | `ROLE_CDKEYS` | 0 |
| 10 | `0x…0400` | 1024 | `ROLE_VGSMANAGER` | 0 |
| 11 | `0x…0800` | 2048 | `ROLE_CENTURION` | 5 |
| 12 | `0x…1000` | **4096** | **`ROLE_WORLDMOD`** | **62** |
| 14 | `0x…4000` | 16384 | `ROLE_DBA` | 0 |
| 17 | `0x…0002_0000` | 131072 | `ROLE_REMOTESERVICE` | 1 (server) |
| 18 | `0x…0004_0000` | 262144 | `ROLE_LEGIONEER` | 13 |
| 19 | `0x…0008_0000` | 524288 | `ROLE_TRANSLATION` | 0 |
| 20 | `0x…0010_0000` | 1048576 | `ROLE_CHTINVISIBLE` | 7 |
| 21 | `0x…0020_0000` | 2097152 | `ROLE_CHTADMINISTRATOR` | 6 |
| 22 | `0x…0040_0000` | 4194304 | `ROLE_HEALSELF` | 4 |
| 23 | `0x…0080_0000` | 8388608 | `ROLE_HEALOTHERS` | 3 |
| 24 | `0x…0100_0000` | 16777216 | `ROLE_NEWSREPORTER` | 0 |
| 27 | `0x…0800_0000` | 134217728 | `ROLE_TRANSLATIONADMIN` | 0 |
| 29 | `0x…2000_0000` | 536870912 | `ROLE_ACCOUNTMANAGEMENT` | 0 |
| 31 | `0x…8000_0000` | 2147483648 | `ROLE_IGB` | 0 |
| 32 | `0x1_0000_0000` | 4294967296 | `ROLE_TRANSLATIONEDITOR` | 0 |
| 33 | `0x2_0000_0000` | 8589934592 | **`ROLE_SPAWN`** | 4 |
| 34 | `0x4_0000_0000` | 17179869184 | `ROLE_BATTLESERVER` | 0 |
| 35 | `0x8_0000_0000` | 34359738368 | `ROLE_TRANSLATIONTESTER` | 0 |
| 36 | `0x10_0000_0000` | 68719476736 | `ROLE_TOURNAMENT` | 1 |
| 37 | `0x20_0000_0000` | 137438953472 | `ROLE_TRANSFER` | 0 |
| 38 | `0x40_0000_0000` | 274877906944 | `ROLE_GMS` | 2 |
| 39 | `0x80_0000_0000` | 549755813888 | `ROLE_CL` | 0 |
| 40 | `0x100_0000_0000` | 1099511627776 | `ROLE_CR` | 0 |
| 41 | `0x200_0000_0000` | 2199023255552 | `ROLE_CM` | 0 |
| 45 | `0x2000_0000_0000` | 35184372088832 | `ROLE_BSDADMIN` | 0 |
| 46 | `0x4000_0000_0000` | 70368744177664 | `ROLE_EXT_GM1` | 0 |
| 47 | `0x8000_0000_0000` | 140737488355328 | `ROLE_EXT_GM2` | 0 |
| 48 | `0x1_0000_0000_0000` | 281474976710656 | `ROLE_EXT_GM3` | 0 |
| 49 | `0x2_0000_0000_0000` | 562949953421312 | `ROLE_EXT_GM4` | 0 |
| 50 | `0x4_0000_0000_0000` | 1125899906842624 | `ROLE_SECURITY` | 0 |
| 51 | `0x8_0000_0000_0000` | 2251799813685248 | **`ROLE_PROGRAMMER`** | **93** |
| 52 | `0x10_0000_0000_0000` | 4503599627370496 | **`ROLE_QA`** | **89** |
| 53 | `0x20_0000_0000_0000` | 9007199254740992 | **`ROLE_GMH`** | 6 |
| 54 | `0x40_0000_0000_0000` | 18014398509481984 | **`ROLE_GML`** | **153** |
| 55 | `0x80_0000_0000_0000` | 36028797018963968 | **`ROLE_CONTENT`** | 19 |
| 56 | `0x100_0000_0000_0000` | 72057594037927936 | `ROLE_ADMIN` | 16 |
| 57 | `0x200_0000_0000_0000` | 144115188075855872 | `ROLE_VIPLOGIN` | 0 (server only) |
| 58 | `0x400_0000_0000_0000` | 288230376151711744 | `ROLE_ROLEADMIN` | 0 |
| 59 | `0x800_0000_0000_0000` | 576460752303423488 | `ROLE_NEWBIE` | 2 |
| 60 | `0x1000_0000_0000_0000` | 1152921504606846976 | **`ROLE_SERVICE`** | 82 (never set it) |
| 61 | `0x2000_0000_0000_0000` | 2305843009213693952 | `ROLE_PLAYER` | 5 |
| 62 | `0x4000_0000_0000_0000` | 4611686018427387904 | `ROLE_LOGIN` | 1 |

**Unassigned bits: 0, 13, 15, 16, 25, 26, 28, 30, 42, 43, 44, 63.** **[V]**

Composite masks: **[V]**

| name | expression | value |
|---|---|---|
| `ROLE_EXT_GM4_PLUS` | `EXT_GM4 \| GML \| GMH` | 27584547717644288 (`0x0062…`) |
| `ROLE_EXT_GM3_PLUS` | `EXT_GM3 \| EXT_GM4_PLUS` | 27866022694354944 |
| `ROLE_EXT_GM2_PLUS` | `EXT_GM2 \| EXT_GM3_PLUS` | 28006760182710272 |
| `ROLE_EXT_GM1_PLUS` | `EXT_GM1 \| EXT_GM2_PLUS` | 28077128926887936 (`0x0063C00000000000`) |
| `ROLE_ANY` | `2**64-1 & ~(IGB \| EXT_GM1_PLUS \| EXT_GM2_PLUS \| EXT_GM3_PLUS)` | 18418666942635180031 (`0xFF9C3FFF7FFFFFFF`) |
| `ROLEMASK_ELEVATEDPLAYER` | `ROLE_ANY & ~(LOGIN \| PLAYER \| NEWBIE \| VIPLOGIN \| NEWSREPORTER \| CSMDELEGATE)` | 10780561974598041567 (`0x959C3FFF7EFFFFDF`) |
| `ROLEMASK_VIEW` | `ADMIN \| CONTENT \| GML \| GMH \| QA \| EXT_GM1_PLUS` | 140667119611150336 (`0x01F3C00000000000`) |

Two non-obvious consequences, both **[V]**:

1. `ROLEMASK_ELEVATEDPLAYER` is an **any-of** test mask and it excludes
   `ROLE_GML`/`ROLE_GMH` (inherited from `ROLE_ANY`). A session holding *only*
   `ROLE_GML` is **not** an "elevated player" and **will not get the Insider
   window**. You need at least one of `WORLDMOD / QA / PROGRAMMER / ADMIN /
   CONTENT / …` as well.
2. `ROLEMASK_VIEW` *does* contain `GML | GMH`, so `ROLE_GML` alone passes it.

`PRE_NONE/PRE_AUTH/PRE_HASCHAR/PRE_HASSHIP/PRE_INSTATION/PRE_INFLIGHT`
(0/1/2/4/8/16) also live in `serviceConst` but are unrelated to the role mask.

---

## 4. What each role unlocks, client-side

### 4.1 The headline surfaces

| surface | gate | where |
|---|---|---|
| **Insider window** (the dev tool bar) opens at all | `ROLEMASK_ELEVATEDPLAYER` | `eve/devtools/script/insider.pyc Insider.Show` |
| Insider **auto-opens** at character select | `ROLEMASK_ELEVATEDPLAYER` and setting `ui.Insider` (default `True`) | `…/login/charSelection/characterSelection.pyc OnOpenView` |
| Insider auto-opens when the Neocom is built | same | `…/neocom/neocom/neocomContainer.pyc` |
| "Reload Insider" / "Toggle Insider" in the Neocom right-click menu | `ROLEMASK_ELEVATEDPLAYER` | `…/neocom/neocom/neocomSvc.pyc:837` |
| **"GM / WM Extras"** submenu, inserted at index 0 of *every* right-click menu | `ROLE_GML \| ROLE_WORLDMOD` (char/type menus also accept `ROLE_LEGIONEER`) | `…/ui/services/menusvc.pyc` lines 463, 677, 975, 1524, 2042, 2337 |
| the whole GM type submenu body | `ROLE_GML \| ROLE_WORLDMOD` | `menusvc.GetGMTypeMenu` |
| **Python console** | `ROLE_PROGRAMMER` | `insider.ToolMenu`; `eve/devtools/script/svc_console.pyc:128,135` |
| **developer menu on every window's "more options"** | `ROLE_PROGRAMMER` | `carbonui/control/window.pyc GetMenuMoreOptions` |
| **UI debugger** (`UITree`) | `ROLE_PROGRAMMER` to register the command, `ROLE_GML` to actually open it | `eveCommands.py:753` registers; `eveCommands.OpenUIDebugger` re-checks `ROLE_GML` |
| privileged info logging (`blue.LogControl.LogtypeInfoIsPrivilegedOnly = False`) | `ROLEMASK_ELEVATEDPLAYER`, re-evaluated on every session change containing `role` | `logcontrol/logcontrolsvc.pyc OnSessionChanged` |
| FPS monitor enabled on session change | `ROLE_GML` | `carbonui/uilib.pyc OnSessionChanged` (fires when `userid` is in the change) |
| chat classification / name colour | see §4.4 | `chat/common/util.pyc get_classification_from_role` |

### 4.2 Role → what appears

| role | what it unlocks (client-side, confirmed) |
|---|---|
| **`ROLE_GML`** (153 refs — the single most important bit) | "GM / WM Extras" on every right-click menu; Insider menu bar entries *Tools, Macro, Ship, Charges, Drones, Implants*; Insider tools *Fighter debugger, WWise Sound Player, Video Player, Ballpark exporter, Destroyable Items, Grid Utilities, Starbase Tools*; `UITree` UI debugger; `/tutorial`, `/advantage`, `/bypassdestination`, `/stationintro`, `/resetstationintro`, `/promptexitstarterspace`, `/resetstarterspaceexitdialog` accepted in chat input; "GM: /load me this type"; `GetGMGiveSkillMenu` on skills; engine-tools monitors *GPU Memory, Texture LOD, LiveCount, Python objects, Method Calls*; raw IDs and unpublished data shown throughout (info windows, market, planets, industry, map view, skill plans, chat channel IDs, certificates, contracts, fittings, war entries, bookmarks); `/online me` from Copycat; alt-ctrl-shift movement override (`menuSvcExtras/movementFunctions.py:497`) |
| **`ROLE_WORLDMOD`** (62) | the destructive half of "GM / WM Extras": `WM: /Spawn this type`, `WM: /Unspawn this item`, `WM: /Entity deploy this type`, `WM: /create this type`, `/chowner`, Copycat *Clone Perfect*, `gridutils` NukeStructures / poser Starbase tools, bookmarks and wallet panels for items you do not own, pseudo-localization QA options in the system menu, `menu_implant` tools |
| **`ROLE_QA`** (89) | Insider menu bar entries *QA* and *UI*; Insider tools *Module Test, Skill Tool, Image Cache Demo*; engine-tools monitors *Background download queue, Threads, Tasklets, Blue statistics graphs, Telemetry panel, Resource Manager, GPU Frame Profiler*; unpublished types in cosmetics/skins/market; QA tool menus across ~30 feature packages (`*/qa/*.pyc`, `*/qa_tools.pyc`); localization QA options; QA starter-system picker at character creation (**see §5.2**) |
| **`ROLE_PROGRAMMER`** (93) | Python console; per-window developer menu; `ToggleTelemetryRecord`, `ToggleUILinearColor`, `ToggleGammaCorrectText`, `OpenSkillTreeWindow`, `OpenImageCacheDemo` commands; `PROG: Modify Attributes` in the GM type menu; attribute editing in `svc_dgmattr`; debug entries in ~25 feature windows; unknown-rich-text-tag warnings in `carbonui/control/label.pyc` |
| **`ROLE_GMH`** (6) | Insider tools *Inventory Tools*, *Settings Manager*; `svc_dgmattr` input becomes writable (`self.input.readonly = not session.role & ROLE_GMH`); `sessionChecker.HasGMHRole`; market quote behaviour (`market/quote.py:70`) |
| **`ROLE_ADMIN`** (16) | agent-dialog reloaders in `devtools/windowManager`; `invCache.trace = 1` on session change; admin colour key in the chat window; `EndAllGameSessions` / `TerminateClientConnections` on the server side |
| **`ROLE_CONTENT`** (19) | dungeon editor: `Ctrl+Shift+D` command registration and `OpenDungeonEditor`; `scenario` service started on entering space; Add/Remove from Selection in the GM menu; scene-cursor dragging in `baseCameraController`; `eveImagePicker` content options; mission-giver tools |
| **`ROLE_SPAWN`** (4) | `WM: /Spawn this type` and `/Entity deploy` without `WORLDMOD`; Copycat "Recover Ship" / T3 subsystems; `dna.py` ship spawning |
| **`ROLE_HEALSELF`** / **`ROLE_HEALOTHERS`** (4 / 3) | Copycat "Online Modules" out of station; `gridutils` RepairShip / RepairAll / NukeStructures |
| **`ROLE_TOURNAMENT`** (1) | Insider menu bar entry *Tournament* |
| **`ROLE_LEGIONEER`** (13) | "Gag ISK Spammer" on character menus even without GML/WORLDMOD; extra entries in character/type menus; chat moderation; `ROLE_SLASH = ROLE_GML \| ROLE_LEGIONEER` in `xmppchatwindow` |
| **`ROLE_CHTADMINISTRATOR`** (6) | chat window stack admin, channel field editing, player-channel moderation |
| **`ROLE_GMS`** (2) | chat GAME_MASTER classification; chat "poke" roles |
| **`ROLE_CENTURION`** (5) | chat VOLUNTEER classification; "special chat highlighting" checkbox (also satisfied by `ROLEMASK_ELEVATEDPLAYER`) |
| **`ROLE_PLAYER`** | required (§2.4); also gates service locking in `ObjectConnection`, call throttling (`ThrottlePer*` decorators), and three Copycat branches |
| **`ROLE_LOGIN`** | required (§2.4); otherwise only `/whoami` mentions it |
| **`ROLE_NEWBIE`** | joins the rookie help channel (`xmppchatsvc`) |
| **`ROLE_CHTINVISIBLE`** | you are filtered out of chat member lists and classified `INVISIBLE` |
| **`ROLE_PINKCHAT`** | you are classified as an **NPC** in chat |
| **`ROLE_PETITIONEE`** | only ever tested against *other* characters' roles in the petition list |
| **`ROLE_VIPLOGIN`** | server-side only: `machoNet` login-slot bypass and a `zuser.Users_SelectByRole` query. No client reader. |
| **`ROLE_SERVICE`** | **do not set** — §5.1 |

### 4.3 Roles with **zero** readers anywhere in this build

`ROLE_BANNING`, `ROLE_VMSADMIN`, `ROLE_MARKETH`, `ROLE_CSMADMIN`,
`ROLE_CSMDELEGATE`, `ROLE_VGSADMIN`, `ROLE_CDKEYS`, `ROLE_VGSMANAGER`,
`ROLE_DBA`, `ROLE_TRANSLATION`, `ROLE_TRANSLATIONADMIN`,
`ROLE_TRANSLATIONEDITOR`, `ROLE_TRANSLATIONTESTER`, `ROLE_NEWSREPORTER`,
`ROLE_ACCOUNTMANAGEMENT`, `ROLE_IGB`, `ROLE_BATTLESERVER`, `ROLE_TRANSFER`,
`ROLE_CL`, `ROLE_CR`, `ROLE_CM`, `ROLE_BSDADMIN`, `ROLE_EXT_GM1..4`,
`ROLE_SECURITY`, `ROLE_ROLEADMIN`.

They are defined in `serviceConst` and never tested. Setting them is harmless
and pointless. **[V]** (census over all 12,774 modules.)

### 4.4 The slash-command surface — and why it is *not* role-gated client-side

This is the part most likely to be misremembered. In this build:

- `eve/devtools/script/svc_slash.pyc SlashService._SlashCmd` dispatches by
  scanning `svc.__dict__` for any service class with
  `__slashhook__ == True` and a method named `cmd_<name>`. **There is no role
  check in the dispatcher at all.** **[V]**
- If no local handler matches, it falls through to
  `sm.RemoteSvc('slash').SlashCmd(commandLine)`. **The entire GM slash
  vocabulary lives on the server**, so our server owns that gate, not the role
  mask.
- `FetchRemoteCommandList()` runs before the first command and calls
  `sm.RemoteSvc('slash').SlashCmd('/')`, expecting a `UserError` whose
  `reason` is `"...: <python list literal>"`. On failure it pops
  *"Unable to acquire server slash command list"* once and continues. A server
  that wants autocompletion must answer `/` that way. **[V]**

Only **five** client services carry `__slashhook__` (whole-tree grep): **[V]**

| service | `cmd_*` methods |
|---|---|
| `svc.slash` (54) | `alias ammo asteroid avatar avatarcommand avatarscene bp buyorders cancelallorders chtban chtgag chtunban chtungag clearbountycache createitem dgoto dogmaupdate dplay entity fit giveskills givestarterskills highlight hop intro killright loadcontainer loop macromenu massfleetinvite massstanding masstransport networth newscanner noob online openhacking pos randomizeskills recustomize reimbursebounties removeskills run sellorders sessionchangetimer sethackingdifficulty sethackingvirusstats skillscopy spawn spawnn super uiblink unload whoami` |
| `svc.copycat` | `cc`, `getshipsetup` |
| `svc.console` | `execfile`, `reportdesync` (both `ROLE_PROGRAMMER`) |
| `svc.trainer` | `applyskills` |
| `svc.podguide` | `podguide_show` |

Most of those client handlers immediately re-issue a *server* slash command
(`/spawn`, `/create`, `/load me`, `/tr`, `/giveskill`, `/heal`, `/unspawn`,
`/entity deploy`, `/online`, …). So the role mask decides whether the **menu
entry** exists; the server decides whether the **command works**.

`cmd_whoami` is a free diagnostic worth knowing: it prints
`role: 0x%08X` plus the names of every `ROLE_*` global in `svc_slash`'s
namespace that the session holds (skipping `ROLE_ANY`, `ROLE_PLAYER`,
`ROLE_LOGIN`). Typing `/whoami` is the cheapest confirmation that our mask
arrived intact. **[V]**

### 4.5 Keyboard / command registrations that depend on the role

`eve/client/script/ui/eveCommands.pyc` builds the command map at runtime: **[V]**

| gate | commands added |
|---|---|
| `ROLE_CONTENT` | `OpenDungeonEditor` bound to `Ctrl+Shift+D` |
| `ROLEMASK_ELEVATEDPLAYER` | `CmdToggleCombatView` bound to `Ctrl+Alt+T` |
| `ROLE_PROGRAMMER` | `OpenUIDebugger`, `ToggleTelemetryRecord`, `ToggleUILinearColor`, `ToggleGammaCorrectText`, `OpenSkillTreeWindow`, `OpenImageCacheDemo` (no default keys) |

The Insider window itself has **no keybinding** — it is opened from the Neocom
right-click menu or auto-opened at character select / Neocom construction.

### 4.6 The Insider menu bar, dumped

`Insider.Show` gates the window on `ROLEMASK_ELEVATEDPLAYER`, then builds the
bar from this table, appending each entry whose role the session holds: **[V]**

```python
menus = MenuList([['Tools',      self.ToolMenu,       serviceConst.ROLE_GML],
                  ['Macro',      self.MacroMenu,      serviceConst.ROLE_GML],
                  ['Ship',       self.ShipMenu,       serviceConst.ROLE_GML],
                  ['Charges',    self.ChargeMenu,     serviceConst.ROLE_GML],
                  ['Drones',     self.DroneMenu,      serviceConst.ROLE_GML],
                  ['Implants',   self.ImplantsMenu,   serviceConst.ROLE_GML],
                  ['QA',         self.QAMenu,         serviceConst.ROLE_QA],
                  ['UI',         self.UIMenu,         serviceConst.ROLE_QA],
                  ['Tournament', self.TournamentMenu, serviceConst.ROLE_TOURNAMENT]])
```

So with `ROLEMASK_ELEVATEDPLAYER` satisfied but **no `ROLE_GML`**, the Insider
window opens with an **empty menu bar** unless `ROLE_QA` or `ROLE_TOURNAMENT`
is also set. This is the concrete failure mode of the `ROLE_ANY` recommendation.

`eve/devtools/script/enginetools.pyc TOOLS` (the Engine Tools launcher list):
**[V]**

| role | tools |
|---|---|
| `0` (always) | System Memory, Log viewer, Outstanding Calls, Network, FPS Monitor, Carbon Info |
| `ROLE_GML` | GPU Memory, Texture LOD, LiveCount, Python objects, Method Calls |
| `ROLE_QA` | Background download queue monitor, Threads, Tasklets, Blue statistics graphs, Telemetry panel, Resource Manager, GPU Frame Profiler |

`eve/devtools/script/svc_gridutils.pyc RoleCheck(*roleSets)` is the one place a
role failure is surfaced to the user rather than hidden: it pops *"You require
the following role(s) to use this function"*. Note it is an **all-of** test
(`session.role & flags == flags`) per set.

### 4.7 Chat classification (first match wins)

`chat/common/util.pyc get_classification_from_role`: **[V]**

```text
ROLE_CHTINVISIBLE                              -> INVISIBLE
ROLE_PINKCHAT                                  -> NPC
ROLE_QA                                        -> DEVELOPER
ROLE_GML | ROLE_GMH | ROLE_GMS | ROLE_ADMIN    -> GAME_MASTER
ROLE_CENTURION | ROLE_LEGIONEER                -> VOLUNTEER
otherwise                                      -> UNSPECIFIED
```

Called from `chat/client/chat_service.pyc _update_self_from_session`, i.e. on
your own session — which is why `ROLE_CHTINVISIBLE` and `ROLE_PINKCHAT` are on
the "never set" list: they win over DEVELOPER and GAME_MASTER.

---

## 5. Risk analysis — what "set every bit" actually costs

### 5.1 `ROLE_SERVICE` — fatal, two independent guards

1. `machoNetTransport.LockSession`, *before* the packet is applied:
   on a session-init or session-change notification it takes the write lock,
   then

   ```python
   if sess.role & ROLE_SERVICE:
       sess.rwlock.Unlock()
       sess.LogSessionError('SESSIONFUXUP', ...)
       raise RuntimeError('Session map failure, attempting to run a session init or change in a service session context')
   ```

2. `CoreSession.ApplyRemoteAttributeChanges`: same condition, `RuntimeError`.

Both test the **pre-change** role, so the *first* notification carrying
`ROLE_SERVICE` succeeds — and then **every subsequent session change blows
up**, including the character-selection change. The session is poisoned, not
merely degraded. **[V]**

(`ROLE_SERVICE` also appears in the `__exportedcalls__` role requirement of
~30 client-side services. Those are inbound-call gates in
`ObjectCallGPCS.CallUp` / `service.PreCall_RoleCheck`; whether a real CCP
proxy satisfies them some other way is **[?]** and outside this document. It is
not a reason to set the bit: the session guards above are unconditional.)

### 5.2 `ROLE_QA` — two automatic server calls

| call | when | severity |
|---|---|---|
| `charUnboundMgr.GetQAStarterSystemIDs()` | **every character creation**, from `ccSvc.CreateCharacterWithDoll` → `_QAPickStarterSystem`. **No try/except.** | **hard** — an unimplemented method aborts character creation. Even when implemented it pops a modal radio-button dialog ("QA - Pick Starter System"). Returning `[]` makes `_QAPickStarterSystem` return `None` immediately and skips the dialog. |
| `localizationServer.UpdateLocalizationQAWrap(bool)` | `localizationClient.OnCharacterSessionChanged` when `oldCharacterID is None`, i.e. **every character login** | **soft** — dispatched via `sm.ScatterEvent`, which runs each handler in its own `MollycoddledUthread`, so the exception is logged and isolated. |

The sibling call in the same handler, `localizationServer.GetAllTextChanges`,
is guarded by `not blue.pyos.packaged` — our client **is** packaged, so it is
skipped. **[V]**

**Recommendation:** leave `ROLE_QA` out of the mask until
`charUnboundMgr.GetQAStarterSystemIDs` returns `[]`. It costs only the Insider
*QA*/*UI* menus and the QA engine-tools monitors.

### 5.3 `ROLE_CONTENT` — one lazy remote bind and one input hijack

- `spaceView.LoadView` calls `sm.StartService('scenario')` whenever you enter
  space. `ScenarioMgr.Run` makes no server call, so starting it is free. **[V]**
- `ScenarioMgr.GetLevelEditor()` calls
  `sm.RemoteSvc('keeper').GetLevelEditor()` and then `.Bind()` — but only when
  the dungeon editor is actually used (`Ctrl+Shift+D`, or the editor menus).
  Unimplemented `keeper` ⇒ that one action fails, nothing else. **[V]**
- `baseCameraController.CheckMoveSceneCursor` routes left-drag to
  `scenario.MoveCursor` and **returns `True`** when `ROLE_CONTENT` is set and
  `isMovingSceneCursor` is active — i.e. only after a scene cursor has been
  picked up, not on ordinary camera drags. **[V]**

Low risk overall; it is the second bit to drop if space behaves oddly.

### 5.4 Everything else

| role | cost of setting it |
|---|---|
| `ROLE_REMOTESERVICE` | inert on the client (`role & (SERVICE\|REMOTESERVICE) == SERVICE` is false without SERVICE). No upside; leave it out. |
| `ROLE_CHTINVISIBLE` | makes you invisible in chat member lists; overrides GM/DEVELOPER classification |
| `ROLE_PINKCHAT` | classifies you as an NPC |
| `ROLE_NEWBIE` | starts a `JoinRookieHelpChannel` tasklet against a chat server we do not have |
| `ROLE_ADMIN` | `invCache.trace = 1` ⇒ verbose inventory logging. Cosmetic. |
| `ROLE_PROGRAMMER` | **has an automatic cost after all — see 5.5.** |
| `ROLE_GML`, `ROLE_GMH`, `ROLE_WORLDMOD`, `ROLE_SPAWN`, `ROLE_HEAL*`, `ROLE_TOURNAMENT`, `ROLE_LEGIONEER`, `ROLE_CHTADMINISTRATOR`, `ROLE_GMS` | no automatic cost found. Every effect is a menu entry, a window, or a value shown — nothing fires without a user action. |
| the 12 unassigned bits | no readers; but keeping **bit 63** clear is what lets the mask marshal as a plain int64 (§3.2) |

### 5.5 Correction: `ROLE_PROGRAMMER` *does* fire something by itself **[V]**

The row above used to say that none of the GM-ish roles cost anything without
a user action. That was wrong, and a real client run is what caught it.

`EvePhoto.GetPortrait` draws the character-select slots. When the image-server
portrait is missing — which it always is here, we serve no image server — it
checks the role before falling back:

```text
127 POP_JUMP_IF_FALSE 299
136 LOAD_GLOBAL       ROLE_PROGRAMMER
139 BINARY_AND
140 POP_JUMP_IF_FALSE 295        -> skips _QueueScenePortrait entirely
...
273 LOAD_ATTR         _QueueScenePortrait
```

With the bit set, the client starts a `photo::ProcessScenePortraits` worker
that calls `paperDollServer.GetPaperDollDataBatch`, then
`charMgr.GetPublicInfo` and `paperDollServer.GetPaperDollPortraitDataFor` for
every id in the batch. Without the bit, it shows the default portrait icon and
makes none of those calls.

No user action is involved: drawing the character-select screen is enough.

This is not a reason to drop the role — we want the sandbox — but it is worth
knowing for two reasons. First, it is three service methods on the critical
path to M1 that exist *because of a choice we made*, and an unanswered one is
a black screen. Second, it is a counterexample to the method of §5: reading
every `role &` test tells you what a role *unlocks*, not what it *triggers*,
and the two are not the same question. Expect more of these.

**Does any role change the login path?** The login path proper, no. The only
role test on it is `characterSelection.OnOpenView` opening Insider, and
`gameui.SetLanguage` widening the accepted language set under
`ROLEMASK_ELEVATEDPLAYER`. `ROLE_VIPLOGIN`, which *does* affect login, is read
only by the **server** half of `machoNet` — irrelevant to a server we write
ourselves. **[V]**

---

## 6. Where the mask goes on the wire

### 6.1 Command numbers

`cluster/__init__.pyc`: **[V]**

```text
MACHONETMSG_TYPE_SESSIONCHANGENOTIFICATION       = 16
MACHONETMSG_TYPE_SESSIONINITIALSTATENOTIFICATION = 18
```

Both are in `MachoPacket.__bizzarrerouters__`, together with `PING_REQ`/`RSP`.

### 6.2 Packet shape

`MachoPacket.__getstate__` is a **14-tuple**, identical for every command: **[V]**

```text
(command, source, destination, userID, body, oob, contextKey,
 journeyID, trace_id, parent_span_id, sampled, ingress_id,
 sample_rate, cut_parent)
```

`body` is a tuple built from the subclass's `__machodesc__['params']`, in order
(a trailing `?` marks optional, and truncates the body if absent):

| class | command | `params` |
|---|---|---|
| `macho.SessionInitialStateNotification` | 18 | `('sid', 'sessionType', 'initialstate')` |
| `macho.SessionChangeNotification` | 16 | `('sid', 'change', 'nodesOfInterest?')` |

So **the role mask is `body[2]['role']` for command 18**, and
`body[1][1]['role'] == (old, new)` for command 16.

### 6.3 Payload shapes, as the client consumes them

`carbon/common/script/net/SessionChangeGPCS.pyc SessionChange.NotifyUp`: **[V]**

```python
if packet.command == MACHONETMSG_TYPE_SESSIONCHANGENOTIFICATION:
    session.ApplyRemoteAttributeChanges(packet.change[1], initialState=False)
elif packet.command == MACHONETMSG_TYPE_SESSIONINITIALSTATENOTIFICATION:
    session.ApplyRemoteAttributeChanges(packet.initialstate, initialState=True)
```

- **`initialstate` is a flat `{attr: value}` dict.** The proxy builds it as
  `{v: getattr(session, v) for v in session.GetDistributedProps(0)}` — only
  attributes **differing from their default**. So a minimal map is legal.
- **`change` is `(flag, {attr: (old, new)})`.** The flag is `0` when a proxy
  originates the change and `1` when it rebroadcasts; the client ignores it and
  only ever reads `change[1]`.
- Example from the proxy's own rebroadcast path, which is the best available
  template for what a server should emit:

```python
machoNetPacket.SessionInitialStateNotification(
    source      = MachoAddress(clientID=session.clientID),
    destination = MachoAddress(nodeID=each),
    sid         = session.sid,
    sessionType = session.sessionType,
    initialstate= sessionprops)
```

### 6.4 The `role` value itself

Send it as a Python long. With the recommended mask
(`7059392768105517056`, bit 63 clear) that is marshal tag **`0x03`** and the
8 little-endian bytes `00 10 e4 00 52 00 f8 61`. If you insist on a mask with
bit 63 set you must use tag `0x2F` with a trailing `0x00` sign byte, because
`_PyLong_FromByteArray` is called with `is_signed=1`. **[V]**

`role` is in `__persistvars__`, so `session.__setattr__('role', …)` raises
`RuntimeError('ReadOnly', 'role')`; the value only ever lands via
`__ChangeAttribute`, which writes `__dict__` directly. No local code path can
alter it — which is exactly why the server is the only lever. **[V]**

---

## 7. Contradictions with the existing docs

Reported, not edited — those files are other tracks' lanes.

1. **`docs/archive/session-flow.md`, "Session establishment" → "Role mask":**
   *"`ROLE_ANY & ~ROLE_SERVICE` is the recommended mask"*, and
   **`docs/reference/login-path.md` §7**, *"Practical choice: `role = ROLE_ANY & ~ROLE_SERVICE`
   gives login + player + every elevated bit."*

   **Wrong in the way that matters.** `ROLE_ANY` is defined as
   `2**64-1 & ~(ROLE_IGB | ROLE_EXT_GM1_PLUS | ROLE_EXT_GM2_PLUS | ROLE_EXT_GM3_PLUS)`,
   and `ROLE_EXT_GM4_PLUS = ROLE_EXT_GM4 | ROLE_GML | ROLE_GMH` propagates into
   all three `_PLUS` masks. **`ROLE_ANY` therefore clears `ROLE_GML` (bit 54)
   and `ROLE_GMH` (bit 53)** — 159 of the client's role test sites, including
   the "GM / WM Extras" submenu and six of the nine Insider menus. It is "every
   elevated bit *except the GM bits*". **[V]**

2. **`docs/archive/session-flow.md`:** *"The dev/sandbox UI gates on
   `ROLEMASK_ELEVATEDPLAYER` and `ROLE_QA`."*

   Half right, and incomplete. `ROLEMASK_ELEVATEDPLAYER` gates whether the
   Insider *window* opens; `ROLE_GML` gates six of its nine menus and almost
   every GM menu in the game. And `ROLEMASK_ELEVATEDPLAYER` itself inherits
   `ROLE_ANY`'s exclusions, so `ROLE_GML` alone does **not** satisfy it — the
   two must be set together. **[V]**

3. **`docs/reference/login-path.md` §7:** *"Setting `ROLE_VIPLOGIN` also skips the proxy's
   login-slot check."*

   True of CCP's proxy, irrelevant to us: `ROLE_VIPLOGIN` has **zero client
   readers**; its only two references are in the server half of
   `carbon/common/script/net/machoNet.pyc` (a `zuser.Users_SelectByRole` query
   and a VIP-mode check). It does nothing in the client. **[V]**

4. **`docs/reference/login-path.md` §7** lists `ROLE_GML = 18014398509481984` and
   `ROLE_GMH = 9007199254740992` correctly, then recommends a mask that
   excludes both. Internally inconsistent rather than wrong.

Everything else I checked in both documents **holds**: command 16/18 numbers,
`initialstate` being flat, `change = (flag, {attr: (old,new)})`,
`sessionType = 5`, the `ROLE_SERVICE` prohibition, the `ROLE_LOGIN|ROLE_PLAYER`
requirement, `charid` absent for character select, the `__persistvars__` list,
`additionalDistributedProps = ['locationid','corprole']`, the
`GetDefaultValueOfAttribute` behaviour, the client-side `_AssociateWithSession`
branch (`nodeID = 0`, `sess = <global session>`, `clientID = 0`, `packet=None`
sets nothing), and the `sessionVersion` / `LockSession` trap.

---

## 8. Open questions

1. **[?] How a real proxy satisfies `ROLE_SERVICE`-gated inbound calls to
   client services.** ~30 client services declare `__exportedcalls__` entries
   requiring `ROLE_SERVICE` (`usure`, `accountsvc`, `uilog`, `jumpQueue`,
   `agents`, `xmppchatsvc`, `scenarioMgr`, `nodegraph`, …), and both
   `ObjectCallGPCS.CallUp` and `service.PreCall_RoleCheck` test them against
   the *client's own* `session.role`, which cannot contain `ROLE_SERVICE`
   (§5.1). Either those services are reached by a path that skips the check, or
   the check is masqueraded around somewhere I did not find.
   **What would answer it:** a capture, or a trace of one server→client
   `CALL_REQ` to e.g. `usure`. It does not block M1/M2 — nothing on the login
   path needs it.

2. **[?] Whether `change[0]` (the flag in `(flag, dict)`) is ever load-bearing.**
   The client ignores it; the proxy emits `0` for originated and `1` for
   rebroadcast changes. We can send either. Unproven that nothing downstream
   inspects it.

3. **[?] What the real `session_init` dict contains**, and therefore whether a
   real Singularity session set bits we would not think to set. `docs/reference/login-path.md`
   already flags this; the role mask is the field it matters most for, since
   `machoNet.HandleAccept` does
   `llvresponse['session_init']['role'] |= s.role` — an **or** with the
   proxy's own session role, which we cannot observe without a capture.

4. **[I] `ROLE_PETITIONEE`, `ROLE_CENTURION` and the 25 zero-reader roles** are
   listed as "harmless to set" on the basis of the reference census. That is
   strong evidence of absence within the Python tree, but it does not cover
   native code in `blue.dll` / `trinity`. No native reader of a role mask was
   looked for.

5. **[?] Whether the Insider window's `settings.public.ui.Get('Insider', True)`
   default survives a fresh profile.** It reads `True` by default in the code,
   so Insider should auto-open on first login with a satisfying mask — a very
   cheap confirmation of M1, if it happens.

---

## 9. Cheat sheet

```python
# serviceConst values, this build
ROLE_LOGIN            = 4611686018427387904   # 1 << 62
ROLE_PLAYER           = 2305843009213693952   # 1 << 61
ROLE_SERVICE          = 1152921504606846976   # 1 << 60   NEVER SET
ROLE_ADMIN            =   72057594037927936   # 1 << 56
ROLE_CONTENT          =   36028797018963968   # 1 << 55
ROLE_GML              =   18014398509481984   # 1 << 54
ROLE_GMH              =    9007199254740992   # 1 << 53
ROLE_QA               =    4503599627370496   # 1 << 52
ROLE_PROGRAMMER       =    2251799813685248   # 1 << 51
ROLE_GMS              =        274877906944   # 1 << 38
ROLE_TOURNAMENT       =         68719476736   # 1 << 36
ROLE_SPAWN            =          8589934592   # 1 << 33
ROLE_HEALOTHERS       =             8388608   # 1 << 23
ROLE_HEALSELF         =             4194304   # 1 << 22
ROLE_CHTADMINISTRATOR =             2097152   # 1 << 21
ROLE_LEGIONEER        =              262144   # 1 << 18
ROLE_WORLDMOD         =                4096   # 1 << 12

SANDBOX      = 7059392768105517056   # 0x61F8005200E41000  all of the above
SANDBOX_M1M2 = 7018860371459182592   # 0x6168005200E41000  minus QA and CONTENT
```

`initialstate` for the character-select screen, with the mask in place:

```python
{'userid':      <int>,
 'role':        7018860371459182592,   # long, marshal tag 0x03
 'sessionType': 5,
 'languageID':  'EN',
 # 'charid' deliberately absent
}
```

Then type `/whoami` once a character is in: it prints the mask in hex and names
every role the session holds.
