# Bots: a design, derived from EvEJS and adapted to what we actually have

> **Demoted 2026-10-24** — line 3's "nothing here is built yet" is out of date; most milestones are now built or partly built. Current truth: its own milestone notes in §6. Kept for history; do not brief a lane from this file.

Written 2026-10-24. **A plan, not a description** — nothing here is built yet.
Read `docs/lanes/README.md` first if you are going to implement any of it.

The end goal this serves is co-op and PvP with friends *plus player-like
bots*. The bots are not a test harness that happens to look like players; they
are players that happen to be useful as a test harness. That ordering decides
most of the design questions below.

## 0. Where the ideas come from, and what we may take

`$EVEJS` is a fork of EvEJS that has already
built this. Its `bots/README.md` and `doc/BOT_TEST_PLATFORM_PLAN.md` are the
most useful prior art we have.

**It is AGPL-3.0.** Facts about CCP's client and protocol are not theirs and we
use them freely; their code and prose are theirs and we copy neither. Every
borrowing below is an *idea* re-derived in our own terms and marked `[V2]`,
which in this repo means "a third party's decode of a different build" — their
inventory is EvEJS against build 3396210, ours is 3619879, and the two differ.
Nothing from there is `[V]` until measured here.

## 1. The decision that shapes everything: speak the wire, not an HTTP gateway

Their bots talk to a web gateway (`/_evejs-web/v1`) which exposes service calls
over HTTP and drains notifications onto each response. **That gateway drops
`DoDestinyUpdate`** (`evejsWebGatewayRuntime.js:4024,4344`) `[V2]`, so their
bots never receive ball state at all. They compensate with a `/space/snapshot`
endpoint and with notification types that do survive (`OnItemsChanged`,
`OnDamageMessage`, `OnGodmaShipEffect`).

A bot that cannot see the ballpark cannot fly. It cannot know it has arrived,
cannot tell an align from a warp, cannot measure a standoff, and cannot
witness another player's ship. Every hard-won fact in `docs/port-tracker.md`
§34–§36 — the align gate, the 180-tick watchdog, the throttle, the arrival
reconciliation — is invisible from behind such a gateway.

So **our bot is a client of our own `tcp/26000`**, speaking the same MachoNet
framing the real client speaks, and it receives `DoDestinyUpdate` like anybody
else. We already have every piece: `eve-proto` for framing and packets,
`eve-marshal` for `PyValue`, `eve-rowset` for rowsets, and a server that
implements the thirteen services a player's client actually calls
(`charMgr`, `beyonce`, `dogmaIM`, `invbroker`, `ship`, `station`, `map`,
`fleetObjectHandler`, `scanMgr`, `marketProxy`, `account`, `corpRegistry`,
`slash`).

### 1.1 The limitation this creates, stated up front

**A bot built on our own codec cannot validate our codec.** If
`eve-marshal` encodes a tuple wrongly, a bot using `eve-marshal` to decode it
will agree perfectly and report success. The bot and the server would be
wrong together, fluently.

That is not a reason to avoid the design; it is a reason to be clear about what
each tool is for:

| tool | what it is the oracle for |
|---|---|
| `tools/client-ci.sh` + scenarios | **wire and client correctness.** The real client is the only thing that can tell us our bytes are right. Keep it. |
| bots | **behaviour, interaction and scale.** Many actors, long runs, economies, fleets — things one Wine client per player cannot reach. |

A bot suite that is green while the client suite is red means the bot is
sharing our mistake. The client suite stays the gate.

## 2. The principle worth copying exactly: the player way, and the GM way

Their `goDock` skill is documented as "the player way to a station: undock when
docked elsewhere, `travelRoute`, `dock`; **never GM, whatever `allowGm` says**"
`[V2]`, and their GM verbs live in one module (`gm.mts`) that is refused unless
a scenario opts in.

Take this wholesale, because it is the difference between a bot and a puppet:

1. **Every skill is the sequence a player's client would send.** If a player
   must undock, align, warp, approach and dock, the bot sends exactly those
   commands in that order and waits for the same evidence a player's client
   waits for.
2. **GM shortcuts live apart and are off by default.** We have `/tp` and the
   other slash commands, and they are indispensable for *setting up* a
   scenario. They must never be reachable from a skill that claims to be a
   player action.
3. **A bot that cannot do something the player way has found a missing
   feature, not a reason to use GM.** This is the rule that keeps the bots
   honest: see §4.

Their own note that fixtures depending on GM slash commands are something "a
production-shaped deployment must not ship" `[V2]` is the same observation from
the other side.

## 3. The skills, mapped to what we have

Their taxonomy, filtered to what our server can answer today. **Marks here are
about OUR server**, read from `dispatch.rs` and `service/`:

| skill | our calls | state |
|---|---|---|
| `login` | the handshake, `charMgr` character selection | **exists** `[V]` — the client CI does it every run |
| `undock` | `station.UndockAttempt` → `ship.Undock` | **exists** `[V]` — `undock_distance` asserts it |
| `watchBallpark` | receive `DoDestinyUpdate`, keep a `Park` | **server side exists** `[V]`; the bot's half is the first thing to write |
| `setSpeed` / `align` | bound `beyonce.CmdSetSpeedFraction`, `CmdGotoDirection`, `CmdAlignTo` | **exists** `[V]` — `throttle_and_warp` drives all three |
| `warpTo` | bound `beyonce.CmdWarpToStuff` | **exists** `[V]`, including the arrival batch (§36) |
| `approach` / `orbit` | bound `beyonce.CmdFollowBall`, `CmdOrbit` | **exists**, `[A]` — human steps 4/5 never confirmed |
| `lock` / `activate` | `dogmaIM.AddTarget`, `Activate` | **exists**, lock delay `[A]` (step 8) |
| `dock` | bound `beyonce.CmdDock` | `[I]` — not exercised by any scenario yet |
| `readCargo` / `hangar` | `invbroker.List`, `ListByFlags`, `Add` | **exists** `[V]` — the industry work uses it |
| `fleet` | `fleetObjectHandler.*` | **exists**, `[A]` — human step 17 |
| `jumpThroughGate` | bound `beyonce.CmdStargateJump` | **blocked**: we have no stargates (§4) |
| `travelRoute` | a jump graph over stargates | **blocked**: same |
| `goDock` | undock → travel → dock between two stations | **blocked**: one station in the sandbox |
| `market` | `marketProxy.*` | partial; the seeder question is ours, not theirs |
| `mine` | asteroid entities | **blocked**: no asteroid fields |

## 4. What the bots will immediately prove we are missing

This is the most valuable column in the table above, and the reason to build
the bots before the features rather than after. Each of these is a feature the
*player way* needs and we have not built:

1. **Stargates.** No gates means no `travelRoute`, no `jumpThroughGate`, and
   therefore no multi-system anything. Their jump graph is a BFS over a
   stargate table `[V2]`; ours would be the same shape over our celestials.
2. **A second station.** `goDock` between two stations is the canonical
   player-path journey, and the sandbox has one station.
3. **Asteroid fields.** `mine` is their most developed role and we cannot
   host it. The data is catalogued (§35) and the slim-item fields are unknown.
4. **The sun.** Not needed by any skill, but its absence is the same class of
   gap and it is cheap.

## 5. Shape of the thing

A new crate, `server/crates/eve-bot`, std-only like everything else and listed
in no `deps.allow` because it needs nothing.

```
eve-bot/
  src/
    conn.rs      # connect, handshake, the call/response correlation
    notify.rs    # the inbound notification pump; keeps a Park
    park.rs      # our own client-side ballpark: balls, modes, positions
    skills/      # one module per verb group, player-path only
    gm.rs        # the slash commands, behind a flag that defaults off
    bin/eve-bot.rs
```

Two pieces deserve their design settled before any code:

- **The call/response correlation.** A player's client fires a `CallReq` with a
  `callID` and matches the `CallRsp`. Our bot needs the same, plus a timeout
  that is *ours* rather than the client's 3600 s `[V]`.
- **`park.rs` is a second implementation of `michelle.Park`.** That is the
  point — it is how the bot knows where it is — but it is also the place where
  a bot can quietly diverge from the real client's understanding. It should be
  written against `docs/reference/client-architecture.md` §4 and
  `docs/reference/destiny-reference-blob.md`, and the first scenario to compare them is
  worth more than the next ten skills.

## 6. Milestones, each one a thing that works

1. **Login and stand still.** Connect, handshake, select a character, hold the
   session open. Done when `park_census` run from a real client sees the bot's
   character in the same station.

   **Half built, 2026-10-24.** `server/crates/eve-bot` exists: `Conn::connect`,
   `Conn::handshake` (H1–H10 with both of the real client's H8 checks — the
   `challenge_responsehash` match and the queue-position refusal), and a
   `eve-bot --connect HOST:PORT --hold SECONDS` binary. Two tests: the
   handshake against a real in-process server, and a silent server failing as
   a timeout rather than hanging, which is the failure mode that matters for
   something unattended.

   **Still missing: character selection**, and therefore the `park_census`
   cross-check that would actually close this milestone. A handshake gets a
   session; it does not get a character, and until the bot has one no other
   player can see it. That is the next piece.
2. **See the world.** Receive `DoDestinyUpdate`, build a `Park`, report the ego
   ball. The headless equivalent of `ballpark_smoke`, and the moment the bot
   becomes useful: **it gives us a second actor without a second Wine client.**

   **First half done, 2026-10-24.** `undock()`, `bind_ballpark()`,
   `collect_pushes()` and a tolerant `notification()` reader; the bot logs in,
   becomes a character, undocks, binds and receives `DoDestinyUpdate`. The
   chain has one surprising step and the test names it: **the bind is the
   trigger, not the undock.** The server holds the bootstrap until
   `beyonce.MachoBindObject(ssid, None)` asks, because the real client has no
   ballpark for ~3 s after the undock and `michelle.DoDestinyUpdate` silently
   returns while that is true. A bot that undocks and waits waits for ever.

   **Done, 2026-10-24.** `read_set_state` unwraps
   `[[(stamp, ('SetState', (bag,)))], waitForBubble]` and the `KeyVal` bag,
   and `enter_space` is bind-and-decode in one call. `WorldState` carries the
   stamp, the ego and the park; `ego_ball()` is an `Option` because
   `Park.SetState` assigns the ego without checking the ball exists.

   **It decodes with the server's own `Park::decode` rather than a second blob
   reader**, which is deliberate and has a cost: the bot round-trips our own
   bytes and so cannot catch our encoder being wrong. Same limitation as §1.1,
   same answer — that is the client suite's job.

   The bot is now a second actor: it logs in, undocks, and can say what is in
   the scene and which ball is its own.
3. **Move.** Throttle, align, warp, arrive. Compare the bot's own arrival
   against the server's scheduled batch (§36) — the first assertion only a bot
   can make, because it can read its own position at a known instant.

   **Commands done, 2026-10-24.** `bound_call`, `set_speed_fraction`,
   `warp_to_item`, and `read_actions` for the incremental pushes. The bot warps
   and reads the whole batch back: `SetSpeedFraction`, `WarpTo`,
   `SetSpeedFraction`, `OnSpecialFX`. **That makes it a headless regression
   test for both defects the player reported** — a `WarpTo` with no throttle
   beside it is aligning-forever (§34), and a missing `OnSpecialFX` was the
   absent warp tunnel (§35).

   Two protocol lessons paid for here, both worth knowing before writing the
   next verb:

   - **A reply is identified by its `callID` and nothing else.** Taking the
     first non-notification frame is wrong: `ship.Undock` leaves a reply in the
     stream that the next call read as its own, so a bind came back as the
     undock's `None` and the ballpark was never bound. It surfaced as "nothing
     is bound yet" against a server that had answered correctly.
   - **A `CallRsp` payload is a pickle, not a value.** It must be loaded before
     anything inside can be read, and getting that wrong is not a parse error:
     the `Bytes` looks like a leaf, so a search finds nothing and reports an
     empty result rather than a failure.

   **Arrival done, 2026-10-24**, and it is the first end-to-end test over a
   real socket of the *scheduled* push queue -- the newest machinery in the
   server, which everything else exercises in-process. The bot times its own
   warp against `destiny::warp_flight_seconds`, which it can call itself:
   travel 581 m, model 8.1 s, **observed 8.73 s**. The clock honours its
   schedule to within 0.63 s, which is the 250 ms tick granularity plus the
   push's trip out.

   **And it nearly reported a defect that was not there.** Measuring
   ego-to-target gave a model of 11.4 s against an observed 8.7 s -- which
   looks precisely like the server scheduling an arrival earlier than its own
   flight time. It was not: the server warps to `min_range.max(radius +
   WARP_STANDOFF)` short of the target and computes the flight over *that*
   distance, and a station has a large radius. The same lesson as §34.8 of the
   port tracker, in the opposite direction: **compute what the server
   computes, or the disagreement is yours.** Twice in one day is a pattern
   worth naming rather than a coincidence.
4. **Two bots.** The `lobby_guests` rendezvous pattern, headless, which makes
   `[A]` human steps 4, 5, 7, 13 and 14 automatable at a cost of nothing.

   **Done, 2026-10-24**, and it arrived earlier than this ordering expected
   because milestones 2 and 4 share everything but a second socket.
   `two_bots_in_one_system_each_see_the_other_s_ship`: Alice undocks and reads
   the world, then Bob connects, undocks and reads his — and Bob's park holds
   **both** ships, **739.6 m apart**. `docs/guides/human-checks.md` §7 step 1 asks a
   human to eyeball "about 750 m"; the bot measures it.

   The assertion is deliberately the *second* bot's view of the *first*.
   Asserting that a bot sees its own ship proves nothing about multi-actor
   state — our own ego ball is in every state we are sent. The defect this
   shape catches is the one where a joiner is told about itself and nothing
   else, which is what §29 was about.

   Not covered: whether either ship is drawn *inside* the station model. That
   is geometry a park cannot answer and stays with the human batch.
5. **A role.** One long-running loop that does something a player does, chosen
   once stargates exist.

Milestone 2 is the one to aim at. Everything before it is plumbing; everything
after it is cheap.

## 7. What we take from their operational experience, free

Their README's "known server behaviour the assertions have to live with" is a
list we would otherwise have had to discover `[V2]`:

- Entering a solar system with no scene yet stalls their main thread 600–700
  ms, which is why their lateness ceiling is 1000 ms. Ours is the same number
  for the same reason (`tools/client-ci.sh clock_health`), chosen before we had
  our own measurement — our measured baseline is 0.1 ms, so we have room.
- Their `approach` **re-sends a `CmdOrbit` the server dropped during a pending
  landing**, and their `dock` re-sends a `CmdDock` the server answers silently
  while a warp landing is pending. Those are server bugs they worked around in
  the client. **We should test for both rather than inherit the workaround** —
  a command dropped during a pending landing is exactly the kind of defect our
  arrival reconciliation could have.
- A shared backend means one person's restart kills everyone's run. Ours is
  per-run and in-memory, which we should keep: restart *is* reset, and their
  biggest documented gap is the absence of exactly that.
