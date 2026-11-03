# Plan: the docked subsystems, in two rounds

> **Demoted 2026-10-28** — a finished plan: its two rounds (docked subsystems, skills, wallet ledger, market) are built; per-subsystem status is in `docs/port-tracker.md`. Current truth: the code and `docs/archive/plan-subsystems.md`. Kept for history; do not brief a lane from this file.

Written 2026-10-19, from the owner's own test run
(`captures/1792442372-3777106-0002.evecap`, five reported faults, filed as
tracker issues #1–#5) and from the two censuses that landed the same evening:
[`findings/client-calls.tsv`](../../findings/client-calls.tsv) (1,816 call sites
with what the client does with each answer) and
[`findings/push-events.tsv`](../../findings/push-events.tsv) (3,177 push
handlers).

Strategy and milestone order live in [`docs/archive/roadmap.md`](roadmap.md); how parallel
work is divided lives in [`lanes/README.md`](../lanes/README.md). **This document
is the execution plan for one round**: what is being fixed, by whom, in which
files, and what is deliberately left for the round after.

---

## The shape of the problem, and why it splits in two

Every one of the five reported faults is one of exactly two kinds, and the
split is the whole plan:

**Kind 1 — a wrong answer.** A call with no arm, answered `None`, meeting a
client that subscripts, unpacks or iterates it. Four of the five. Each is one
arm, each is confined to one service module, and each is independent of every
other. **These parallelise perfectly and are this round.**

**Kind 2 — a missing world.** The wallet has no ledger, there is no item
store, skills have nowhere to live. Three of the five converge here, and so
does every mutator in the game. It is one piece of work, it touches the
central files, and splitting it across lanes would produce conflicting
diffs. **It is strictly serial and it is the round after.**

The temptation is to start with Kind 2 because it is the real thing. The
reason not to: Kind 1 is currently *masking* Kind 2. Four of the five windows
abort with a `TypeError` before they ever ask for the state that does not
exist, so we cannot see what the world actually has to provide until they stop
breaking. Fixing the cheap arms is not busywork ahead of the real work; it is
how the real work becomes legible.

---

## Round 1 — the arms, six lanes, fully parallel

Each lane owns **files**, not subsystems, per `lanes/README.md`. The ownership
below is disjoint by construction; nothing in this round edits `service.rs`,
`dispatch.rs`, `conn.rs` or `server.rs`.

| lane | owns | work |
|---|---|---|
| `industry` | `service/industry.rs` | issue #4, then the whole industry entry path |
| `market` | `service/market.rs` | issue #2, then the market entry path |
| `character` | `service/character.rs` | issue #5 step 1, the clone live lock, the `charMgr` half of #3, `GetLabels` |
| `inventory` | `service/inventory.rs` | the `invbroker` half of #3, `GetShipFittingInfo`, `GetLicencedSkins` |
| `world-corp` | `service/world.rs`, `service/corp.rs` | two more live locks: `map.GetStationCount`, `corpRegistry.GetTitles` |
| `chat-petitions` | `service/base.rs`, `service/social.rs` | the XMPP traceback, and the `petitioner` subsystem |

### What each lane is actually fixing

**`industry` — issue #4, and it is the most valuable single arm in the round.**
`blueprintManager.GetLimits()` is read as `GetLimits()['maxBlueprintResults']`
inside `blueprintSvc.Run`, under `StartServiceRun` — which stores the
exception and re-raises it from *every* later
`sm.GetService('blueprintSvc')`. So `None` does not empty the industry tab, it
**poisons the service for the rest of the session**. One dictionary with one
key fixes it. The rest of the lane is the enumeration pass.

**`market` — issue #2.** `marketProxy.GetOrders(typeID)` is read as
`orders[0]`, `orders[1]`, with `len()` taken of each: a 2-sequence. Note
`GetPlexOrders`, which `RefreshOrderCache` calls *instead* for the PLEX type.

**`character` — three independent items, and one of them is the find of the
evening.** `skillMgr2`'s handler `.PurchaseSkills` is `extend`ed, so it must
be iterable. `charMgr.ListIncludingContainers` is the assets window's
cross-station query. And **`jumpCloneSvc.GetShipCloneState` is a live lock on
a call we already answer**: `CRowset` subclasses `list`, so our empty answer is
*falsy*, so `if not self.shipJumpClones` never latches and every call
re-issues. No traceback, no `UNANSWERED` line, invisible to every instrument
we own — the census found it before it cost anyone an evening, and that is the
clearest argument for the census there is.

**`inventory` — the other half of assets, plus two shapes that will raise the
moment a window opens.** `ship.GetShipFittingInfo` is consumed by
`UNPACK_SEQUENCE 4`, so our `None` raises immediately; and
`shipCosmeticsMgr.GetLicencedSkins` is a poisoned cache *and* twelve iterating
callers at once, in the Personalization window — which already stalls for an
unrelated gateway reason, so it will be misattributed if left.

**`world-corp` and `chat-petitions`** are the same pattern at lower stakes: two
more guarded caches that never latch, one traceback with no visible symptom
(Local chat, which needs a separate XMPP connection and so can only be made to
*stop throwing* from here), and one entirely self-contained subsystem with 35
call sites and zero arms.

### The one cross-lane contract, settled before the lanes start

Issue #3 spans two files: `charMgr.ListIncludingContainers` and
`invbroker.GetContainerContents`. Two lanes must answer the **same row shape**
or the assets window gets half of one and half of the other. So that shape is
not either lane's decision: `Registry::owned_item_rows()` lands in
`service.rs` as pre-work, both lanes call it, and neither may define its own.
This is rule 2 of `lanes/README.md` — a shared decision is removed before the
round, not negotiated during it.

The same pre-work creates `service/industry.rs` and `service/market.rs` and
moves `marketProxy` into the latter, so that no lane has to touch `service.rs`
to register a new service. `DOMAINS` and the module list are the last shared
edit in this tree and they are spent here, once, by the coordinator.

### Round 1 is done when

- Every one of the six tracebacks in `captures/1792442372-3777106-0002.evecap`
  that is **ours** is gone from a fresh run's `evecap --triage`.
- `tools/census-calls.sh --diff` shows rows moving from `no`/`partial` to
  `yes`, and **no new `native` consumer** — there is exactly one in the client
  today and it is the one that caused the undock black screen, so a second one
  appearing is a new black-screen risk and the diff is how we would see it.
- `tests/replay.rs` still answers every call in all four committed captures.
- The gate is green and each lane has parked a note in `docs/lanes/<name>.md`.

---

## Round 2 — the world, strictly serial

Not a lane split. One piece of work, in this order, each step leaving the gate
green:

1. **Share the world** (spine step A). `Arc<RwLock<World>>`; today
   `serve_one` builds a `Registry` per connection, so two clients are two
   universes and a reconnect discards everything. Design in
   [`docs/reference/world-state.md`](../reference/world-state.md). Everything below is blocked on it,
   which is why it is first.
2. **The item store.** Items with an owner, a location and a flag — the thing
   that issue #3's real half, issue #5's real half, fittings and industry jobs
   all turn out to need. `item_columns()` already pins the row shape and the
   eleven *positional* indices `OnItemChange` requires are written down in
   [`push-events.md`](../reference/push-events.md).
3. **The wallet ledger** (issue #1). Cheap once 1 is done, and meaningless
   before it: a journal in a per-connection `Registry` dies at reconnect, like
   the balance does today.
4. **`OnItemChange` and `OnGodmaItemChange`** (push increment 4, the two-phase
   ticker). Which is what makes a mutator's effect *visible*, and without which
   the item store is invisible: every inventory, fitting, market and industry
   mutator discards its return value, 618 call sites' worth.
5. **Skill state** (issue #5's real half), which needs 1, 2 and 4.

Step 1 forces the push ticker to become two-phase — build under the world read
lock, write holding nothing — which is `docs/reference/world-state.md` §4 rule 4 and already
designed in [`docs/reference/push-path.md`](../reference/push-path.md) §1.5. That coupling is the reason
round 2 is one job and not five.

---

## Deliberately not in either round

- **The protobuf gateway.** PLEX balance, daily goals, campaigns, achievements,
  ship cosmetics licences. Seven of the fourteen tracebacks in the owner's test
  run are this, no answer of ours can affect any of them, and the client
  catches every one. See `docs/guides/debugging-the-client.md` §3c.
- **Local chat for real**, which is a separate XMPP/ejabberd connection. Round
  1 only stops the traceback.
- **Destiny and a visible ship** (spine step E). Blocked on the reference blob,
  which is one `/execfile` away inside a running client — `tools/destiny-ref.sh`.
