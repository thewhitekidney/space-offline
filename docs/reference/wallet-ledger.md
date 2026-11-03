# The wallet ledger

Issue #1: *"isk: works, but shows no transaction history."* Step 3 of round 2
in [`docs/archive/plan-docked-subsystems.md`](../archive/plan-docked-subsystems.md). This document is
the specification the implementation is built from; it contains no Rust and
changes no code.

`/giveisk` moves `config.characters[0].balance` and pushes `OnAccountChange`,
so the neocom and the ISK tab header update. There is no ledger at all, so the
Journal and the Market Transactions tabs are empty — correctly empty, by every
test the client applies. The work is to make a row exist, and the hard part is
not the row: it is that **four of the eleven things the client does with that
row are only safe for particular values**, and three of those fail without a
line in any log of ours.

Claims are marked **[V]** (read out of the client's bytecode or our own code)
or **[I]** (inferred, with the reasoning given). **Every client line number
here came from `tools/call_lines.py` or `tools/pyc27.py`**, i.e. from
`co_lnotab`; none was read off uncompyle6's output, which numbers its own
reconstruction (`docs/guides/debugging-the-client.md` §4, "Tooling traps").

---

## 0. What we answer today

`server/crates/eve-server/src/service/wallet.rs` and the helpers it uses in
`service.rs`:

| call | our answer today | verdict |
| --- | --- | --- |
| `account.GetCashBalance` | `Real(balance)` / `Real(0.0)` for corp | right |
| `account.GetTransactions` | empty `CRowset`, 9 columns (`service.rs:2013`) | **right shape, right columns, ignores its four arguments** |
| `account.GetEntryTypes` | list of 2 `KeyVal`s (`service.rs:1743`) | right family, **`entryTypeNameID: None` blanks the Type column — see §5.2** |
| `account.GetWalletDivisionsInfo` | `[]` | right, and unreachable on a personal wallet (§1.4) |
| `marketProxy.CharGetTransactions` | empty `CRowset`, 12 columns (`service.rs:1982`) | right family, **three column names are wrong — see §2.3** |
| `LPSvc.GetAllMyCharacterWalletLPBalances` | `[]` | right |
| `account.GiveCash` | **no arm** | §5.4 |

Nothing in the server records a transaction, so there is nothing to serve.

---

## 1. Which calls each tab makes

### 1.1 The window is three tab groups deep, and "Journal" is not called Journal

`walletWnd.py` → `personalWalletPanel.py` → `personalISKPanel.py`:

```text
WalletWindow                      header tab group
├── My Wallet        PANEL_PERSONAL    'personal'
│   └── PersonalWalletPanel       WalletTabGroup (tab headers carry balances)
│       ├── ISK      PANEL_PERSONALISK 'mywallet'
│       │   └── PersonalISKPanel  TabGroup
│       │       ├── Overview            PANEL_OVERVIEW           'overview'
│       │       ├── Transactions        PANEL_TRANSACTIONS       'transactions'   <- the JOURNAL
│       │       ├── Market Transactions PANEL_MARKETTRANSACTIONS 'marketTransactions'
│       │       ├── Shares              PANEL_SHARES             'shares'
│       │       └── Give ISK            PANEL_GIVEISK            'personalGiveISK'
│       ├── PLEX     PANEL_PLEX, LP PANEL_LP, EverMarks PANEL_EVERMARKS
├── Corporation Wallet  PANEL_CORP   (built only if HaveAccessToCorpWallet/EM/LP)
└── Settings            PANEL_SETTINGS
```

**[V]** The constants are `walletConst.py` (`PANEL_TRANSACTIONS` at line 90,
`NUM_TRANSACTIONS_PER_PAGE = 100` at line 129). The sub-tab list is
`personalISKPanel.GetMainTabs`; the ISK/PLEX/LP/EverMark list is
`personalWalletPanel.Load`.

**The tab the UI calls "Transactions" is the wallet journal**, served by
`account.GetTransactions`. The tab called "Market Transactions" is the separate
market table, served by `marketProxy.CharGetTransactions`. There is no
`GetJournal` and no Market *Orders* tab in this window — orders live in the
Market window (`marketProxy.GetCharOrders`).

### 1.2 What fires, in order, when the window opens

For the pilot we actually have (NPC corp, no corp roles), on a cold open:

| # | what runs | call | args | answered today |
| --- | --- | --- | --- | --- |
| 1 | `WalletWindow.ApplyAttributes` → `ConstructLayout` | — (`HaveAccessToCorpWallet()` is pure session arithmetic, `walletUtil.py:51`) | | — |
| 2 | each `WalletTabGroup` tab header's `BaseWalletTab.ConstructLayout` → `PopulateBottomContainer` (`baseWalletTab.py:36`) | `account.GetCashBalance(0)` **[I] from cache**, `LPSvc.GetAllMyCharacterWalletLPBalances()`, EverMark + PLEX balances | | yes / yes / gateway |
| 3 | `PersonalISKPanel.OnTabSelect` → `ISKPanel.ConstructLayout` → `ConstructPanels` → `GiveIskPanel.ApplyAttributes` | `wallet.GetWealth()` — client cache | | — |
| 4 | `ISKPanel.ConstructLayout` → `uthread.new(CheckSetDefaultWalletDivision)` | none (`walletUtil.py:111`, local) | | — |
| 5 | `tabGroup.AutoSelect()` → **Overview** → `OverviewPanel.OnTabSelect` → `Update` → `TransactionOverviewController` | **`account.GetTransactions(1000, None, None, False)`** (`transactionOverviewController.py:108` → `accountsvc.py:128`) | `(accountKey, year, month, isCorp)` | yes, empty |
| 6 | same controller, `_UpdateMarketTransactions` | **`marketProxy.CharGetTransactions(None)`** (`transactionOverviewController.py:93` → `marketSvc.py:21`) | `(fromDate)` | yes, empty |
| 7 | *if* either answer has rows | **`account.GetEntryTypes()`** (`accountsvc.py:114` → `97`) | none | yes, latches |

**`GetCashBalance` is not on the wire here.** `WalletSvc.Run` calls it once at
login (`walletSvc.py:44`) and stores `_wealth`; the property's guard is
`if self._wealth is None` (`walletSvc.py:50`), so a real number latches and
opening the wallet never re-asks. That is also why the owner's balance already
works. **[V]**

**Step 7 is the one to notice: `GetEntryTypes` is not fetched while the
journal is empty.** The only caller is `Account.GetRefTypeKeyByID`
(`accountsvc.py:114`), which is reached per row from
`transactionsPanel.Transaction.__init__` (line 44),
`TransactionsPanel.GetScrollEntry` (line 196),
`GetReferenceTypeComboOptions` (line 291) — which loops over the *set of
entryTypeIDs present in the answer* — and
`TransactionOverviewController.GetTypeName` (line 126). With no rows, every
one of those loops is empty. So the latch fires at the instant the first
journal row arrives, and whatever `GetEntryTypes` answers at that moment is
what the session has forever.

`GetEntryTypes` does not appear in `findings/client-calls.tsv` as an `account`
row because `GetStaticData` reaches it through
`getattr(account, itemName)(*args)` (`accountsvc.py:99`), which the census
cannot resolve. It is a real remote call all the same.

### 1.3 What each sub-tab adds when clicked

| sub-tab | entry point | calls |
| --- | --- | --- |
| Overview | `OverviewPanel.OnTabSelect`, and also `OnWindowEndScale`, `on_window_stack_changed`, `on_window_display_changed`, the Income/Expenses radio, a donut segment click, Back | `account.GetTransactions(1000, None, None, False)` + `marketProxy.CharGetTransactions(None)` — both behind client caches |
| **Transactions (Journal)** | `BaseTransactionsPanel.OnTabSelect` (line 182) → `UpdateTransactionsAndPopulateScroll` (line 187 → 120) | `account.GetTransactions(1000, year, month, False)` via `personalTransactionsPanel.py:7`; **`(year, month)` is `(None, None)` by default**, the same cache key the Overview already filled, so usually **no new wire call** |
| Market Transactions | same `OnTabSelect` | `marketProxy.CharGetTransactions(None)` (`personalMarketTransactionsPanel._GetTransactions`) |
| Shares | `SharesPanel.OnTabSelect` → `ShowMyShares` → `GetScrollList` | `corp.GetSharesByShareholder(False)` — not an `account` call |
| Give ISK | nothing on select; the Transfer button | `account.GiveCash(toID, amount)` — 2 args (`giveIskPanel.py:149`); `transferMoneyWnd.py:187` is the 3-arg form |

### 1.4 The division / account selector

**There is none on the personal wallet. [V]** `account.GetWalletDivisionsInfo`
has exactly one caller in the whole client,
`panels/corp/corpDivisionsPanel.pyc` (grep over `out/pyc` returns that file
and `accountsvc.pyc` only), and the corp tab is not constructed at all unless
`HaveAccessToCorpWallet() or HaveAccessToCorpEMWallet() or
HaveAccessToCorpLPWallet()` (`walletWnd.ConstructLayout`). The corp journal's
own key selector is `transactionsPanel._GetAccountKeyList`, built from
`corp.GetDivisionNames()` and `session.corprole`, not from us.

Our `[]` answer stays right and stays unreachable. Keep it: it costs nothing
and the first corp role we grant makes it reachable.

---

## 2. Row shapes and container families

### 2.1 `account.GetTransactions` — the journal

**Container family: `CRowset`. [V]** And the thing that forces it is not a
rowset API — nothing calls `.Index()`, reads `.columns`, or subscripts the
answer by id. It is `utillib.KeyVal`:

```python
def GetTransactionKeyVal(transaction):          # baseTransactionsPanel.py:17
    keyVal = utillib.KeyVal(transaction)        # 18
    keyVal.currency = appConst.creditsISK       # 19
    keyVal.sortValue = transaction.transactionID  # 20
```

and `utillib.KeyVal.__init__` accepts a `dict`, a `blue.DBRow`
(`utillib/__init__.py:18`) or another `KeyVal`, and **raises `TypeError` on
anything else** (line 24). A `util.Row` out of an old-style `Rowset` is
rejected on the spot. `CRowset`'s elements are `blue.DBRow`, so `CRowset` is
the family. A plain `list` of `blue.DBRow`, or a list of dicts, would also
survive every consumer — `accountsvc._GetPersonalTransactions` copies the
answer into a fresh Python `list` anyway (`accountsvc.py:129-133`) — but
`CRowset` is what the real server sends and what `service.rs:2013` already
builds. Keep it.

Two more consumers to keep honest:

- `accountsvc.py:128-133` iterates it once and calls
  `transactionsUtil.GetDerivedTransactions(transaction)` per row, which reads
  `transaction.description` — so the attribute must exist.
- `transactionOverviewController._UpdateWalletTransactions` (line 108) reads
  `.entryTypeID` and `.amount` off the **raw row**, before any KeyVal wrapping.

**Falsiness is not a hazard here.** `CRowset` subclasses `list`, so an empty
one is falsy, but no guard on this path tests truthiness — see §6.

**Columns, in order.** These are exactly what the client reads, and they match
`service.rs:2013` today:

| # | column | `DbType` | read at | notes |
| --- | --- | --- | --- | --- |
| 1 | `transactionID` | `I8` (20) | `baseTransactionsPanel.py:20` | becomes `sortValue`; derived rows do `±0.25`/`+0.5` on it (`transactionsUtil._GetDerivedSecurityTaxTransaction`, `_GetDerivedCorporationTaxTransaction`), so it must be a number and should increase with time |
| 2 | `transactionDate` | `FileTime` (64) | `transactionsPanel.py:46`, and `:232` as a sort key | **the date trap — §3.1** |
| 3 | `entryTypeID` | `I4` (3) | `transactionsPanel.py:44`, `:173`, `:187`, `:196`, `:203`, `:245`, `:293`; `transactionOverviewController.py:126` | must resolve through `GetEntryTypes` — §5 |
| 4 | `ownerID1` | `I4` | `transactionsPanel.py:53`, `:161`, `:248` | the "from" side |
| 5 | `ownerID2` | `I4` | `transactionsPanel.py:54`, `:162`, `:249` | the "to" side |
| 6 | `referenceID` | `I8` | `transactionsPanel.py:55` | reaches `FmtWalletEntry`'s `arg1` |
| 7 | `amount` | `Cy` (6) | `transactionsPanel.py:41`, `:48`, `:207`, `:225` | **`Cy` is divided by 10000 on read — §3.2** |
| 8 | `balance` | `Cy` | `transactionsPanel.py:42`, `:49`, `:210`, `:229` | running balance *after* this entry |
| 9 | `description` | `WStr` (130) | `transactionsPanel.py:57`, `:62`, `:212`; `transactionsUtil.GetDerivedTransactions`, `GetTransactionHint` | a **YAML mapping or the empty string** — §3.5 |

`currency` and `sortValue` are added by the client
(`baseTransactionsPanel.py:19-20`); do not send them.

Nothing else is read. `accountKey` is **not** a column — the client passes the
key *in* and never reads it back.

### 2.2 `account.GetEntryTypes` — the reference-type table

**Container family: a plain `list` of `utillib.KeyVal`. [V]** Not a rowset of
either family, and again the reason has nothing to do with reading:

```python
def GetStaticData(self, itemName, trCol=None, trID=None, *args):   # accountsvc.py:97
    if itemName not in self.data:
        data = getattr(account, itemName)(*args)                   # 99
        for rec in data:
            messageID = getattr(rec, trID, None)
            translated = localization.GetByMessageID(messageID)
            setattr(rec, trCol, translated)                        # <- writes to every row
        self.data[itemName] = data
    return self.data[itemName]
```

`Row.__setattr__` goes through `header.index(key)` and raises
`AttributeError` for an undeclared column; `blue.DBRow` is a fixed native
schema. `KeyVal` takes arbitrary attributes. `service.rs:1743` already builds
this correctly.

Columns read: `entryTypeID` (`accountsvc.py:115`,
`transactionsPanel.py:198`/`:293`/`:294`), `entryTypeNameID` (read by name
through `trID`), `entryTypeName` (written then read at
`transactionsPanel.py:45`/`:293`/`:294` and
`transactionOverviewController.py:127`/`:129`).

### 2.3 `marketProxy.CharGetTransactions` — the market table

**Container family: `CRowset`, for the same `KeyVal` reason** —
`marketSvc._GetPersonalTransactionKeyVal` (line 50) wraps each row.

**Three of the twelve columns in `service.rs:1982` are named wrong. [V]** The
rowset is empty today so nothing breaks, but it will the moment a market
transaction exists. What the client actually reads off the server row:

| the client reads | at | our column today |
| --- | --- | --- |
| `transactionID` | `marketTransactionsPanel.py:292` | `transactionID` ✓ |
| **`transactionDate`** | `marketTransactionsPanel.py:47` | **`transactionDateTime`** ✗ |
| `typeID` | `:49`, `:77`, `:287`, `:332` | ✓ |
| `quantity` | `:43`, `:48`, `:239`, `:290`; `marketSvc.py:57` | ✓ |
| `price` | `:44`, `:50`, `:291`; `marketSvc.py:57` | ✓ |
| `stationID` | `:53`, `:335` | ✓ |
| `buyerID` | `marketSvc.py:51`, `:56`, `:57` | ✓ |
| `sellerID` | `marketSvc.py:56`, `:57` | ✓ |
| `accountID` | `marketSvc.py:52`, `:54` | ✓ |
| **`buyerAccountID`** | `marketSvc.py:52` | **`buyerAccountKey`** ✗ |
| **`sellerAccountID`** | `marketSvc.py:54` | **`sellerAccountKey`** ✗ |

`clientID`, `totalPrice`, `isBuyTransaction` and `isForCorp` are **derived
client-side** (`marketSvc.py:55-59`) and overwrite anything we send;
`characterID` and `keyID` are probed with `hasattr`
(`marketTransactionsPanel.py:56`, `:62`) and are the corp form's. So our
`clientID` column is harmless, but the three misnamed ones are an
`AttributeError` inside `_GetPersonalTransactionKeyVal`, which has no handler.

`service/market.rs` is not this lane's file. Flagged for whoever owns it.

---

## 3. Every field the UI formats, and how

`transactionsPanel.Transaction.__init__` is the whole per-row formatter
(lines 40-62):

```python
self.amount = self.data.amount                                       # 41
self.balance = self.data.balance                                     # 42
refType = sm.GetService('account').GetRefTypeKeyByID(
                                      self.data.entryTypeID)         # 44
self.dateString = GetByLabel('UI/Wallet/WalletWindow/FmtWalletDate',
                             dt=self.data.transactionDate)           # 46
self.typeString = refType.entryTypeName                              # 45/47
self.amountString  = FmtWalletCurrency(self.data.amount,  self.data.currency)   # 48
self.balanceString = FmtWalletCurrency(self.data.balance, self.data.currency)   # 49
self.descriptionString = FmtWalletEntry(self.data.entryTypeID,       # 51-57
            self.data.ownerID1, self.data.ownerID2, self.data.referenceID,
            amount=self.data.amount, reason=self.data.description)
self.hint = GetTransactionHint(self.data)                            # 58
```

and the caller wraps it in exactly one handler:

```python
for t in transactionKeyVals:                              # transactionsPanel.py:166
    try:
        transactions.append(Transaction(t))
    except AttributeError as exc:                         # 170
        log.LogWarn('Wallet Journal: failed to load data for entryType {}, '
                    'error: {}'.format(t.entryTypeID, exc))   # 171-173
        continue
```

**That handler catches `AttributeError` and nothing else.** Every failure mode
below is classified by whether it lands inside it.

### 3.1 The date — and why this one is *not* the usual FILETIME trap

`GetByLabel('UI/Wallet/WalletWindow/FmtWalletDate', dt=...)` ends in
`localization.formatters.dateTimeFormatters.FormatDateTime`, whose first test
is `isinstance(value, long)`; the `else` branch is
`logger.LogTraceback('datetime only accepts blue time or Python time ...')`
and `return` — a **blank field, no exception, and nothing in our log** (the
traceback lands in the *client's* log as a `STACKTRACE #n` block, which
`tools/client-log.sh --errors` extracts and `evecap --alerts` does not, because
`LogTraceback` is not an exception). `dateTimeFormatters.py:54` is the
`LogTraceback`; line 41 is the `blue.os.GetTimeParts` on the happy path. **[V]**

Our encoder picks the integer tag by magnitude —
`eve-marshal/src/value.rs:631` emits `INT8`/`INT16`/`INT32` for anything that
fits `i32`, `INT64` only above it — so a loose `PyValue::Int` under 2^31
arrives as a Python `int` and takes that `else` branch
(`docs/archive/architecture-sweep.md`, "A `PyValue::Long`").

**Inside a packed `DBRow` this cannot happen. [V]** `docs/reference/dbrow.md`'s type table
has `FILETIME` (64) at size class 4, 8 bytes, read "as UI8", i.e. through
`PyLong_FromUnsignedLongLong` — a Python **`long` whatever the value**. So the
journal's `transactionDate`, travelling in a `FileTime` column of a
`CRowset`, is always a `long` and always takes the first branch. The magnitude
trap does not reach it.

**What does reach it. [I]** The first branch then does
`blue.os.GetTimeParts(value)` and hands the result to
`time.strftime('%Y.%m.%d %H:%M', ...)`. A zero or near-zero FILETIME is
year 1601, and CPython 2's `time.strftime` refuses years before 1900. That
raises **`ValueError`**, which the `except AttributeError` above does **not**
catch — so one bad date takes down the whole Journal tab, not one row. Marked
**[I]**: the 1900 floor is CPython 2 behaviour I did not read out of this
build, and `blue.os.GetTimeParts` is native. The safe rule costs nothing:

> **Every `transactionDate` is a real FILETIME from `filetime_now()`.** Never
> 0, never a placeholder, never Unix seconds.

The same applies to `marketTransactionsPanel.py:47`.

The three other places a date would hit the *magnitude* trap, i.e. where a
time field would travel as a loose value rather than in a `FileTime` column:

1. If the journal is ever answered as a **list of `KeyVal`s or dicts** instead
   of a `CRowset` — a shape every consumer in §2.1 would accept — then
   `transactionDate` is a loose `PyValue::Int` and the magnitude rule applies
   again. **Answering the journal as a list is a correctness regression even
   though nothing reads `.columns`.** This is the one trap in this subsystem
   that an implementer could walk into while making a change that looks
   harmless.
2. `account.GetEntryTypes` has no time field, and must keep none.
3. `account.GetWalletDivisionsInfo`'s answer is wrapped by the client in its
   own `KeyVal(info=..., expires=now + 5*MIN)` (`accountsvc.py:146`) — the
   expiry is the *client's* clock, not ours, so `[]` is safe.

### 3.2 The amount and the balance

`FmtWalletCurrency(amt, currency, showFractions=None)` (`walletUtil.py:24`):

```python
if showFractions is None:
    showFractions = GetSettingValue('walletShowCents')     # default False
    if not showFractions and 1 < abs(amt) and abs(amt) < sys.maxint:
        amt = int(amt)
return FmtCurrency(amt, showFractionsAlways=showFractions, currency=currency)
```

so `amount` and `balance` must be **numbers**: `abs()` and `int()` on `None`
raise `TypeError`, which the `except AttributeError` does not catch. The
colour of the row is `transaction.data.amount < 0`
(`transactionsPanel.py:207`) — Python 2 orders `None` below every number, so a
`None` amount that somehow survived would silently paint the row red rather
than throw, which is the `GetMySecurityStatus` failure mode again.

**`Cy` (6) is divided by 10000.0 on read. [V]** `docs/reference/dbrow.md`'s type table,
verified from the native getter. `service.rs:2013` declares `amount` and
`balance` as `Cy`, so **the stored integer is ISK × 10000**: one million ISK is
`10_000_000_000`, not `1_000_000`. Getting this wrong is a silent factor-of-ten-thousand
and the single most likely bug in this work. The alternative is `R5` (f64),
which carries the float straight through and needs no scaling; `Cy` is what
EVE uses and it keeps the balance column exact at two decimals. Either is
defensible — **pick one and pin it with a test that asserts the client-visible
number, not the stored one.**

`currency` is not ours: `baseTransactionsPanel.py:19` sets
`appConst.creditsISK = 0` on every row unconditionally.

### 3.3 The type name

`refType.entryTypeName` at `transactionsPanel.py:45`. `GetRefTypeKeyByID`
returns `None` for an id not in `GetEntryTypes`, and `None.entryTypeName`
is an `AttributeError` — **inside** the handler, so the row is dropped and
`LogWarn('Wallet Journal: failed to load data for entryType ...')` appears in
the client log. See §5.3 for the full unknown-id story.

### 3.4 The description, and an unresolvable reference id

`FmtWalletEntry(entryTypeID, o1, o2, arg1, pretty=1, amount, reason)`
(`eve/common/script/util/walletEntryFormat.py`):

```python
if entryTypeID not in MANUAL_WALLET_REFERENCES:
    messageID = get_entry_format_localization_id(entryTypeID)
    if messageID is not None:
        return _FmtWalletEntryNormalized(messageID, GetName(o1), GetName(o2), arg1)
return _FmtWalletEntryManual(entryTypeID, o1, o2, arg1, pretty, amount, reason)
```

Three things follow, and all three are good news:

- **`get_entry_format_localization_id` reads the client's own FSD**
  (`eveaccounting/data.py`, `res:/staticdata/accountingEntryTypes.fsdbinary`).
  We send no message id and cannot influence this.
- **An owner id we cannot resolve to a name does not break anything. [V]**
  `eveFormat.GetName(ownerID)` is
  `try: ... cfg.eveowners.Get(ownerID).name except: return 'id:%s (no name)'`
  — a bare `except`. The row renders with a literal `id:90000001 (no name)`
  in place of the name.
- **An entry type with nothing to say renders as a dash.**
  `_FmtWalletEntryManual`'s fallback, with `pretty=1` (the default), is
  `return PrepareLocalizationSafeString('-')`.

`cfg.eveowners.Prime({ownerID1, ownerID2})` at `transactionsPanel.py:163` sits
**outside** the `try`, so it must not raise — and it does not.
`Recordset._Prime` (`carbon/common/script/sys/cfg.py`) drops keys already in
`self.data` and keys in `knownLuzers` before fetching, and an id the server
returns no row for is remembered rather than retried. It reaches us as
`config.GetMultiOwnersEx([ids])`, which `service.rs:1457` already answers for
our characters and our corporation and returns nothing for anything else.

**[V] and useful: owner id `1` is resolved entirely client-side.**
`eveCfg.py:565` seeds `cfg.eveowners.data[1]` with the label
`UI/Chat/ChatEngine/EveSystem` ("EVE System", typeID 0), so `GetName(1)`
never touches the wire and never needs a `GetMultiOwnersEx` arm. That makes
`ownerID1 = 1` the cheapest honest "from" side for a GM grant.

### 3.5 The hint, and why `description` must be YAML or empty

`transactionsUtil.GetTransactionHint(transaction)` and
`GetDerivedTransactions(transaction)` both open with
`if not transaction.description: return` and then
`yaml.load(description, Loader=yaml.CSafeLoader)`, catching only
`yaml.scanner.ScannerError`. A non-YAML string that happens to scan cleanly
and is not a mapping is handled (`if type(description) != types.DictType:
return`). **So the only two safe values are the empty string and a YAML
mapping.** The empty string is right for a GM grant.

One consequence worth recording for later: if we ever emit a description
mapping carrying one of `appConst.derivedTransactionParentMapping`'s keys,
`_GetDerivedCorporationTaxTransaction` does `transaction.amount += taxAmount`
on the **raw row**. `blue.DBRow` is a fixed native schema and may not accept
the write. Corp tax only; not in scope.

---

## 4. The container-family summary

| call | family | what forces it | empty value |
| --- | --- | --- | --- |
| `account.GetTransactions` | **`CRowset`** | `utillib.KeyVal.__init__` rejects anything that is not dict / `blue.DBRow` / `KeyVal` (`utillib/__init__.py:18`, raise at `:24`) | an empty `CRowset` — falsy, and safe, because no guard here is truthiness (§6) |
| `account.GetEntryTypes` | **`list` of `KeyVal`** | `GetStaticData` does `setattr(rec, 'entryTypeName', ...)` on every row (`accountsvc.py:97-104`) | never empty — see §5 |
| `marketProxy.CharGetTransactions` | **`CRowset`** | `marketSvc.py:50` `KeyVal(transaction)` | empty `CRowset` |
| `account.GetWalletDivisionsInfo` | **iterable** | `enumerate` in `corpDivisionsPanel`; wrapped in the client's own `KeyVal` | `[]` |
| `account.GetCashBalance` | **float** | arithmetic (`newBalance - self.wealth`, `walletSvc.py:93`) | `0.0`, never `None` |

No `CIndexedRowset`, no `CFilterRowset`, no old-style `Rowset` and no
`(header, data)` tuple anywhere on the wallet path. Nothing reads `.columns`,
nothing calls `.Index()`, nothing subscripts a wallet answer by id, and
nothing unpacks one.

---

## 5. Entry types and reference ids

### 5.1 The numbers

**[V]** from `eve/common/lib/appConst.pyc`, read with `tools/pyc27.py`
(module-level `LOAD_CONST` → `STORE_NAME` pairs — *not* from uncompyle6, which
renders class-body constant lists as const-table indices):

| constant | id | constant | id |
| --- | --- | --- | --- |
| `refUndefined` | 0 | `refInsurance` | 19 |
| `refPlayerTrading` | 1 | `refMarketEscrow` | 42 |
| `refMarketTransaction` | 2 | `refBrokerfee` | 46 |
| **`refGMCashTransfer`** | **3** | `refTransactionTax` | 54 |
| `refPlayerDonation` | 10 | `refBountyPrize` | 17 |
| `refCorporationPayment` | 11 | `refBountyPrizes` | 85 |
| `refMaxEve` | 10000 | `refSkipLog` | −1 |

`service.rs:96` already has `REF_GM_CASH_TRANSFER = 3` and
`service.rs:98` `REF_PLAYER_DONATION = 10`. Both confirmed.

`refGMCashTransfer` is in the `refGroupTransfer` (6) bucket: `appConst`'s
`refGroups` literal builds `(refCorporationAccountWithdrawal,
refGMCashTransfer, refPlayerDonation): refGroupTransfer`, read off the
disassembly at module offsets 5392-5407, and `refGroupByRefType` is inverted
from it. That matters because the Overview donut does
`GetByLabel(NAME_BY_REFGROUPID[refGroupID])`
(`transactionOverviewController.py:149`) with **no `.get` and no default** —
a ref group missing from that dict is a `KeyError`. Group 6 is present in both
`NAME_BY_REFGROUPID` and `COLOR_BY_REFGROUPID`, so a GM grant is safe on the
Overview tab too.

`appConst.accountingKeyCash = 1000` **[V]** (`inventorycommon/const.pyc`;
`accountingKeyCash2..7` are 1001-1006).

### 5.2 `entryTypeNameID` — the bug in what we ship today

`GetStaticData` **overwrites** `entryTypeName` with
`localization.GetByMessageID(rec.entryTypeNameID)` on every row
(`accountsvc.py:97-104`), and
`localizationBase.GetByMessageID` returns `''` for `None`
(`localizationBase.py:209-210`) and `u'<marker> %s]'` for an id it cannot
resolve (line 214).

`service.rs:1743` sends `entryTypeNameID: None` and a human-readable
`entryTypeName`. **The name is discarded and the Type column renders empty.**
A guessed id is worse: it renders a visible unresolved-localisation marker.

**The real message ids are recoverable, and they are in the shipped client.**
`res:/localizationfsd/localization_fsd_en-us.pickle` is a protocol-0 pickle of
`{messageID: (text, None, None)}` (40 MB, ASCII, on disk under
`.../CCP/EVE/ResFiles/2c/2c3038b3c38e91a1_...`). The accounting entry-type
names are one contiguous run **[V]**:

```text
sI59792  (VUndefined            refUndefined          0
sI59793  (VPlayer Trading       refPlayerTrading      1
sI59794  (VMarket Transaction   refMarketTransaction  2
sI59795  (VGM Cash Transfer     refGMCashTransfer     3     <--
sI59796  (VATM Withdraw         refATMWithdraw        4
...
sI59802  (VPlayer Donation      refPlayerDonation    10
sI59809  (VBounty Prize         refBountyPrize       17
sI59811  (VInsurance            refInsurance         19
sI59834  (VMarket Escrow        refMarketEscrow      42
sI59838  (VBrokers Fee          refBrokerfee         46
sI59846  (VTransaction Tax      refTransactionTax    54
sI59877  (VBounty Prizes        refBountyPrizes      85
```

so **`entryTypeNameID = 59792 + entryTypeID`** for the whole range
0..103 (spot-checked by name against the `appConst` constant at 0, 1, 2, 3, 4,
5, 6, 7, 8, 9, 10, 17, 19, 33, 42, 46, 54, 85, 99, 101, 102, 103 and 106; the
two gaps at 100 and 104/105 are ids with no ref type). **The formula breaks
above ~112** — the run ends at 59912 and the later ref types (170+, 190+) got
their message ids elsewhere, so do not extrapolate. For any ref type we
actually book, grep the pickle for the name and read the `sI` before it.

Marked **[V]** for the id-to-text mapping (read out of the shipped table) and
**[I]** for the rendered result, which depends on `eveLocalization`'s loaded
language data and can only be confirmed with a client in front of you.

### 5.3 What the client does with a `refTypeID` it does not know

Not blank, not a raise that reaches the player, and not a fallback string:
**the row is silently dropped and a warning goes to the client's log.** **[V]**

The order matters. `GetRefTypeKeyByID` returns `None` (`accountsvc.py:116`),
and the *first* dereference is `refType.entryTypeName` at
`transactionsPanel.py:45`, inside `Transaction.__init__`, inside
`UpdateTransactions`'s `except AttributeError` (line 170) — so the row never
becomes a `Transaction`, `continue` runs, and the log gets
`Wallet Journal: failed to load data for entryType <id>, error: ...`
(line 171). `GetScrollEntry`'s own `if not refType: log.LogWarn('Wallet
Journal: No entryType found for entryTypeID', ...)` at `transactionsPanel.py:203`
is therefore **unreachable from this path** — and note it is placed *after* a
dereference of the same `refType` at line 198
(`if originalType and refType.entryTypeID != originalType`), so it would raise
before its own guard if a filter were selected. Dead code with a latent bug in
it; not ours to fix.

`TransactionOverviewController.GetTypeName` is the one forgiving reader:
`if not hasattr(refType, 'entryTypeName'): return None` → `continue`
(`transactionOverviewController.py:126-129`), so the donut just omits it.

**The consequence for us is the whole of §5.5:** because `GetEntryTypes`
latches on a presence check (`if itemName not in self.data`,
`accountsvc.py:98`) and is only fetched once the first journal row exists,
every ref type we will *ever* book must be in that one answer. A ref type
added later is invisible until `ProcessUIRefresh` or a character change clears
`self.data` (`accountsvc.py:93`, `:62`).

### 5.4 Which ref types a sandbox realistically needs

| ref type | id | `entryTypeNameID` | booked by |
| --- | --- | --- | --- |
| `refGMCashTransfer` | 3 | 59795 | `/giveisk` — **the one this issue needs** |
| `refPlayerDonation` | 10 | 59802 | `account.GiveCash` (the Give ISK tab, and `/giveisk <char>` once there are two characters) |
| `refMarketTransaction` | 2 | 59794 | a market buy or sell — the journal half of it (§7) |
| `refBrokerfee` | 46 | 59838 | placing an order |
| `refTransactionTax` | 54 | 59846 | a sale |
| `refInsurance` | 19 | 59811 | a ship loss payout |
| `refBountyPrize` | 17 | 59809 | a single NPC kill |
| `refBountyPrizes` | 85 | 59877 | the aggregated form, and the one the real server actually uses |
| `refPlayerTrading` | 1 | 59793 | a player-to-player trade |
| `refUndefined` | 0 | 59792 | a last resort; `FmtWalletEntry` renders it as `''` with `pretty=1` |

Two of these are filtered out of the *reference-type combo* by
`walletConst.SKIP_REF_TYPES` (line 65) — `refTransactionTax` is in that list,
and `refMarketTransaction` is in `transactionOverviewController`'s
`REFTYPEID_BLACKLIST` so it is excluded from the donut (the Market
Transactions tab covers it instead). They still render as journal rows. The
combo also drops anything `>= refMaxEve` (`transactionsPanel.py:293`).

Shipping all ten costs ten `KeyVal`s and removes the "add them later means
relog" problem permanently. **Do that rather than growing the list.**

### 5.5 What a GM grant's fields should be

| field | value | why |
| --- | --- | --- |
| `transactionID` | a monotonically increasing i64 | `sortValue`; the derived-row arithmetic needs a number |
| `transactionDate` | `filetime_now()` | §3.1 — never 0 |
| `entryTypeID` | `3` (`refGMCashTransfer`) | §5.1 |
| `ownerID1` | `1` | "EVE System", resolved client-side at `eveCfg.py:565`, no `GetMultiOwnersEx` needed. **[I]** — the real server puts the GM's character id here; `1` is the honest sandbox substitute and the only owner id that is free |
| `ownerID2` | the character id | the receiver |
| `referenceID` | `0` | **[I]** — the journal message for entry type 3 appears to be message `234508`, `"{name1} deposited cash into {name2}'s account"` (the only "deposited cash" string in the en-us table, and entry 3's name is "GM Cash Transfer"), which uses neither `arg1` nor `arg`. If that message id is wrong, `_FmtWalletEntryManual` has no arm for 3 and the description renders as `-`. Neither outcome raises |
| `amount` | the grant, scaled (§3.2) | positive |
| `balance` | the balance *after* the grant, scaled | |
| `description` | `""` | §3.5 |

---

## 6. Paging and range semantics — and the live-lock question

### 6.1 The range is `(year, month)`, and the paging is client-side

**[V]** `account.GetTransactions(accountKey, year, month, isCorp)`, four
positional arguments in that order, from the disassembly of
`accountsvc._GetPersonalTransactions` (`accountsvc.py:128`):

```text
LOAD_ATTR GetTransactions
LOAD_GLOBAL appConst / LOAD_ATTR accountingKeyCash     -> 1000
LOAD_FAST year
LOAD_FAST month
LOAD_GLOBAL False
CALL_FUNCTION 4
```

- **No row count, no `fromDate`, no id cursor.** The journal's only range
  argument is a calendar month.
- `(None, None)` means **the last 30 days**:
  `transactionsPanel.py:33` `DATE_FILTER_LAST30DAYS = (None, None)`, inserted
  as the first combo option at line 142 under the label
  `UI/Wallet/WalletWindow/Last30Days`.
- The other options are the five previous whole months,
  `transactionsUtil.GetLastNYearMonthTuples(6)[1:]` with the value
  `(year, month + 1)` — so **`month` is 1-based** when it is not `None`
  (`transactionsPanel.py:137-143`).
- **[I]** the default selection is the first option, `(None, None)`: the
  `Combo` is constructed with `options=` and no `select=`
  (`transactionsPanel.py:133`). A `Combo` named `dateCombo` may restore a
  remembered value, so answer both forms.
- `NUM_TRANSACTIONS_PER_PAGE = 100` (`walletConst.py:129`) is **client-side
  only**: `BaseTransactionsList.GetTransactionsRange(fromIdx, count)` slices
  the list the client already holds (`baseTransactionsPanel.py:52`, used at
  `:138`), and the Previous/View-more buttons only re-run `PopulateScroll`.
  **The server returns every row for the period in one answer.**

The corp form is `account.GetTransactions(accountKey, year, month, True)`
(`accountsvc.py:139`), with `accountKey` one of 1000..1006.

`marketProxy.CharGetTransactions(fromDate)` takes one argument and the client
always passes `None` (`marketTransactionsPanel.UpdateTransactions` sets
`fromDate = None` unconditionally before the call, line ~218).

### 6.2 What the window does with an empty answer

**It caches it and stops asking. The Journal is not a live lock.** **[V]**

`docs/guides/debugging-the-client.md` §4's rule is to look at the **guard**, not at the
store. Both guards on this path are *presence* checks on a dict, not
truthiness and not `is None`:

```python
def GetPersonalTransactions(self, year=None, month=None):   # accountsvc.py:120
    key = (year, month)
    if key in self.transactionCache:                        # 122
        return self.transactionCache[key]
    self.transactionCache[key] = self._GetPersonalTransactions(year, month)   # 124
    return self.transactionCache[key]                       # 125
```

```python
def GetStaticData(self, itemName, trCol=None, trID=None, *args):   # accountsvc.py:97
    if itemName not in self.data:                                  # 98
```

An empty `list` is a perfectly good dict value, so the store succeeds and the
guard is satisfied. This is **not** the `jumpCloneSvc.GetShipCloneState`
variant (variant 4, "a correct answer that is merely falsy"), even though a
`CRowset` with no rows *is* falsy — nothing on this path tests the answer's
truth. It is not variant 1 (`LPSvc`, the comprehension raises so the
assignment never happens): `_GetPersonalTransactions` iterates the answer and
builds a fresh list, which an empty `CRowset` satisfies. It is not variant 2
(`NumOutstandingContracts`, the stored value fails its own guard) and not
variant 3 (`GetScanMan`, poisoned behind a debounce).

`marketSvc.GetPersonalMarketTransactions` has the same shape —
`if fromDate in self.personalMarketTransactionCache` (`marketSvc.py:20`).

For completeness, the cadence of every caller, since §4's rule is that
severity is set by the caller's cadence:

| caller | cadence | behind the cache? |
| --- | --- | --- |
| `BaseTransactionsPanel.OnTabSelect` (`:182`) | a tab click | yes |
| `TransactionsPanel.OnDateCombo` / `OnCombo` (`:146`, `:153`) | a combo change | yes (new key for a new month) |
| `OverviewPanel.OnTabSelect`, `OnIncomeExpensesRadioButton`, `OnBack`, `OnEntrySelected` | a click | yes |
| `OverviewPanel.OnWindowEndScale`, `on_window_stack_changed`, `on_window_display_changed` | end of a drag / show / stack change — **not** per frame | yes |

None sits behind a render handler, a `uthread2.debounce`, a `BufferedCall` or
a `timerstuff.AutoTimer`. One extra wire call per month-selection is the
worst case, and the empty answer *displays* correctly: `Scroll.Load` is given
`noContentHint=GetByLabel('UI/Wallet/WalletWindow/HintNoTransactionsFound')`
(`baseTransactionsPanel.py:143`).

**So the honest answer is already the right answer, and nothing about paging
needs a guard.** Record the emptiness in a test so nobody invents a row to
silence something that was never loud.

---

## 7. Refresh

### 7.1 `OnAccountChange` clears the journal cache but does not redraw the tab

We push `OnAccountChange('cash', charid, balance)` (`dispatch.rs:944`). Five
handlers exist for it; `findings/push-events.tsv` lists all of them:

| handler | what it does |
| --- | --- |
| `accountsvc.py:75` `Account.OnAccountChange` | `self.transactionCache = {}` and `self.corpTransactionCache = {}` (line 76) — **the journal cache is invalidated** |
| `walletSvc.py:78` `WalletSvc.OnAccountChange` | → `_OnCharAccountChange` (line 83) |
| `industrySvc.py:382`, `lpStoreSvc.py:452`, `securityOfficeWindow.py:233` | unrelated |

and `_OnCharAccountChange` (`walletSvc.py:89`):

```python
if GetSettingValue('notifyAccountChange'):                      # 90
    sm.GetService('neocom').Blink('wallet', ...)                 # 91
    self.BlinkPersonalWallet(walletConst.PANEL_TRANSACTIONS)     # 92
difference = newBalance - self.wealth
sm.ScatterEvent('OnPersonalAccountChangedClient', newBalance, difference)   # 94
self._wealth = newBalance
walletSignals.on_personal_isk_balance_changed(newBalance, difference)
```

Two things to know:

- **`notifyAccountChange` defaults to `False`** (`walletConst.SETTING_DEFAULTS`),
  so by default there is no neocom blink and no tab blink. The owner can tick
  it on the wallet's Settings tab. The neocom *balance* still updates, because
  that goes through `OnPersonalAccountChangedClient`.
- **No handler of `OnAccountChange` or `OnPersonalAccountChangedClient`
  repopulates the Journal scroll. [V]** The
  `OnPersonalAccountChangedClient` handlers in `push-events.tsv` are
  `neocomContainer.py:87`, `giveIskPanel.py:88` (updates the max-ISK input),
  `iskTab.py:32` (animates the tab-header balance), `transferMoneyWnd.py:162`,
  `nodegraph`, `playerprogression/totalnetworthcontainer.py:128` and
  `raffles`. `TransactionsPanel` declares no `__notifyevents__` at all, and
  `ISKPanel.__notifyevents__ = []`.
- `BlinkPanelByID` flashes a tab header; it does not reload a panel.

**So the Journal tab refreshes on exactly one thing: being (re-)selected.**
`BaseTransactionsPanel.OnTabSelect` (line 182) → `UpdateTransactionsAndPopulateScroll`
(line 187 → 120) → `UpdateTransactions` → `_GetTransactions`. The
`OnAccountChange` push is still load-bearing, because without it the cleared
cache would not be cleared and the re-select would be answered from the
client's own dict.

The events that matter, named: **`OnAccountChange`** (ours, already pushed) and
nothing else. `OnShareChange`, `OnBillReceived`, `OnCorpWalletBillsChanged`,
`OnOwnOrdersChanged` and `OnCorpAccountChangedClient` are all for other tabs
or the corp wallet. `ProcessUIRefresh` (`accountsvc.py:93`) and a `charid`
session change (`accountsvc.py:62`) are the only things that clear the
`GetEntryTypes` latch.

### 7.2 The minimum path to the owner's test

> `/giveisk 1000000`, open the wallet, see a journal line for it.

1. A world that holds a `Vec` of journal rows that outlives the connection —
   round 2 step 1. (A journal in a per-connection `Registry` dies at
   reconnect, exactly like the balance does today.)
2. `/giveisk` appends one row: `refGMCashTransfer`, `filetime_now()`, owner
   `1` → the character, amount and the new balance, empty description.
3. `account.GetTransactions(accountKey, year, month, isCorp)` returns the rows
   that fall in the requested period, as the `CRowset` `service.rs:2013`
   already builds. **`(None, None)` must mean "the last 30 days", not "all
   rows"** — a grant made today falls in both, so the owner's test passes
   either way, but the month options would not.
4. `account.GetEntryTypes` gains `entryTypeNameID = 59792 + id` so the Type
   column is not blank, with all ten ref types from §5.4 present because the
   call latches.
5. The owner opens the wallet → **My Wallet → ISK → Transactions**. The
   Overview tab opens first and already fetches the same `(None, None)` key,
   so the row is on the wire before the Journal tab is clicked.

Steps 1-3 are the test. Step 4 is what makes it *read* correctly rather than
showing a row with an empty Type column.

**If the wallet is already open on the Transactions tab**, the push clears the
cache but does not redraw: switch tabs away and back, or change the date
combo. Worth saying in the `/giveisk` console line, which today says
*"Gone after a server restart"* and could also say *"reopen the Transactions
tab to see the journal line"*.

---

## 8. Transactions vs Journal

**Confirmed for this build: they are two different tables, served by two
different services. [V]**

| | the Journal | Market Transactions |
| --- | --- | --- |
| tab | `PANEL_TRANSACTIONS` | `PANEL_MARKETTRANSACTIONS` |
| call | `account.GetTransactions(1000, year, month, False)` | `marketProxy.CharGetTransactions(None)` |
| client service | `accountsvc.Account` | `marketSvc.Market` |
| client cache | `transactionCache[(year, month)]`, cleared on `OnAccountChange` (`accountsvc.py:76`) | `personalMarketTransactionCache[fromDate]`, cleared on `OnOwnOrdersChanged` and `OnSessionReset` (`marketSvc.py:33`, `:36`) |
| row | §2.1 | §2.3 |
| columns in common | none — different names, different types | |

The Overview tab is the only place they meet, and it reads them separately:
`_UpdateWalletTransactions` (line 108) and `_UpdateMarketTransactions`
(line 93), with `refMarketTransaction` in `REFTYPEID_BLACKLIST` so the
journal's market rows are not double-counted against the market table's own.
That blacklist is the proof that a market trade is expected to appear in
**both**: the client would otherwise count it twice.

**A GM grant belongs in the Journal only.** It is not a market trade, it has
no `typeID`, no quantity and no station, and the market row has no column that
could carry it. `/giveisk` books one journal row and nothing else.

For later: a market buy or sell should book **both** — a
`refMarketTransaction` journal row *and* a market transaction row — plus a
`refBrokerfee` journal row on order placement and a `refTransactionTax` one on
a sale. That is the market lane's work, not this one's; this document's job is
to leave the journal API able to accept it.

---

## 9. Implementation order

Each step leaves the gate green and ends with one thing the owner can do.

1. **A journal in the shared world.** A `Vec<JournalEntry>` on `World` behind
   the `Arc<RwLock<_>>` from round 2 step 1, plus a monotonic
   `next_transaction_id`. One entry is the nine fields of §2.1 in server
   terms: `i64` id, `i64` filetime, `i64` entry type, two owner ids, `i64`
   reference id, two amounts, a `String` description.
   *Closes: nothing on the wire.*
   **Observable: nothing yet — this step exists so the next one survives a
   reconnect.**

2. **`/giveisk` books a row.** Append `refGMCashTransfer` with
   `ownerID1 = 1`, `ownerID2 = character_id`, `filetime_now()`, the amount and
   the post-grant balance, `description = ""`, then push `OnAccountChange` as
   it already does. Pin the FILETIME in a test: assert the date is
   `> 94_354_848_000_000_000` (year 1900) rather than merely non-zero, because
   zero is the value that takes the Journal tab down with a `ValueError`
   (§3.1).
   *Closes: nothing new.*
   **Observable: `evecap --calls` shows one journal row in the next
   `account.GetTransactions` answer.**

3. **`account.GetTransactions` reads its arguments.** `(accountKey, year,
   month, isCorp)`: `isCorp == True` or `accountKey != 1000` → an empty
   `CRowset`; `(None, None)` → the last 30 days by `transactionDate`;
   `(year, 1..12)` → that calendar month. Return every matching row in one
   answer — the client pages at 100 locally (§6.1). Keep the `Cy` scaling in
   one helper with one test that asserts the client-visible ISK figure.
   *Closes: `account.GetTransactions` properly, for the personal and the corp
   form.*
   **Observable: `/giveisk 1000000`, open the wallet, go to My Wallet → ISK →
   Transactions, and the line is there — with a blank Type column.**

4. **`account.GetEntryTypes` gains real `entryTypeNameID`s and all ten ref
   types.** `entryTypeNameID = 59792 + entryTypeID` for the ids in §5.4; keep
   `entryTypeName` as documentation even though the client overwrites it.
   Note in the arm that the call latches on a presence check
   (`accountsvc.py:98`) and is first fetched only when a row exists
   (§1.2 step 7), so the list must be complete on the first answer.
   *Closes: `account.GetEntryTypes` for every ref type we will book.*
   **Observable: the journal row's Type column reads "GM Cash Transfer", and
   the reference-type filter combo above the list has that entry in it.**

5. **`account.GiveCash` books two rows.** Two positional args from the Give
   ISK tab (`giveIskPanel.py:149`), three from `transferMoneyWnd.py:187`.
   `refPlayerDonation` (10), `ownerID1` = the giver, `ownerID2` = the
   receiver, one row per side once there are two characters; one row and a
   balance move while there is one.
   *Closes: `account.GiveCash`.*
   **Observable: the Give ISK tab's Transfer button works and the journal
   shows a Player Donation line.**

6. **Fix the three market-transaction column names** (`transactionDate`,
   `buyerAccountID`, `sellerAccountID` — §2.3) and give `market.rs` a hook to
   append both a market row and a `refMarketTransaction` journal row.
   *Closes: `marketProxy.CharGetTransactions` for non-empty answers.*
   **Observable: nothing until the market can trade — this step is so that
   the first trade does not throw in `_GetPersonalTransactionKeyVal`.**
   Not this lane's file.

7. **Mention the refresh in the `/giveisk` reply.** The push clears the
   client's journal cache but does not redraw an already-open Transactions
   tab (§7.1), so the console line should say to re-select it. One line, and
   it saves the next person the same confusion the *"reopen the wallet"*
   apology caused before.
   **Observable: the player is told what to do instead of guessing.**

---

## 10. Left unknown

- **Whether `accountingEntryTypes[3].entryJournalMessageID` is set**, and so
  whether the GM-grant description renders as *"EVE System deposited cash into
  <name>'s account"* (message `234508`) or as the `-` fallback. The FSD is
  `res:/staticdata/accountingEntryTypes.fsdbinary`; its loader is a native
  `.pyd` and the binary was not decoded. The entry *exists* — the string
  `"GM Cash Transfer"` is in that file — and **neither outcome raises**, so
  this is cosmetic.
- **Whether `59792 + entryTypeID` is literally what the real server sends as
  `entryTypeNameID`.** What is verified is that those ids resolve to the entry
  types' names in the shipped en-us table, which is all the Type column needs.
- **Whether `time.strftime` in this client's Python 2 really refuses year
  < 1900.** Marked **[I]** in §3.1. The mitigation (always a real FILETIME)
  is free, so this was not worth a client run to settle.
- **The `dateCombo`'s restored default.** `Combo(name='dateCombo')` may
  persist a selection; answering both `(None, None)` and `(year, month)`
  makes it moot.
- **`refGMCashTransfer`'s conventional `referenceID` on the real server.** `0`
  is used here; the message appears not to read it.
- **The corp journal end to end.** `account.GetTransactions(accountKey, year,
  month, True)` and `corp.GetDivisionNames` are specified above only as far as
  the argument shape. Unreachable for an NPC-corp pilot, so untested against
  the client.

---

## 11. Reference types added by later lanes (Q-DOCS)

The journal now carries more than GM grants and the market. Every ref type the
server writes has a row in `econ_ledger::REFS` (a test keeps the list total,
so a lane that adds a `const REF_*` must add the row). The ones a newcomer is
most likely to meet:

| ref | name | written by | lane note |
|---|---|---|---|
| 13 | office rental fee | the Master Wallet at rent, and at each renewal once `paid_until` has passed; a system sink | `corp-o` |
| 19 | insurance | the premium when a ship is insured and the payout on a loss; a CONCORD kill pays nothing | `x-insure`, `concord-1a` |
| 117 | security office processing fee | the tag office (`security_office.rs`); counted as a sink | `sec-1b` |
| 135 | reaction fee | `cost_lines` of a reaction job (EIV of the inputs times runs); a sink | `mn-a` |

Earlier rows (corp wallet tax 92 to 94, alliance fee 48, war fees 49, 113 and
114, kill right fee 116, planetary taxes 96 to 98, LP store 26) are in their own
lane notes. Bots read their own journals the same way a human does, which is
how `docs/guides/human-checks.md` verifies the insurance and reaction cards.
