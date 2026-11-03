# Inventory: social, chat, session and infrastructure (EvEJS)

<!-- Produced by a read-only inventory lane, 2026-10-20, against
     $EVEJS_RUN/server/src. Nothing in that tree was executed.
     Line citations are as of this read. All LOC figures are measured with
     `wc -l`, not estimated. EvEJS ships ZERO test files (`find` over the
     release root returns no test directory and no *.test.js), so "the file
     exists" is nowhere treated as evidence that the feature works; each
     status below says what the evidence actually is. -->

This is the plumbing half of EvEJS: the macho protocol service that tells the client
what services exist, the GPS handshake and framing, the per-connection session object
and its push helpers, the live session registry, the chat stack (an in-process XMPP MUC
stub plus a TLS edge process on 5222, *not* the legacy LSC push), the ~176-name GM slash
console, the four social services (mail, mailing lists, notifications, calendar,
bookmarks), the object cache, the SQLite-backed `gameStore` with its persistence worker
and owner lease, the content-pack loader, and five separate worker-thread "edge"
processes. Measured: **chat 24 187 LOC across 58 files**, `network/` 9 889 across 25,
`gameStore/` 9 085 across 17, `services/account/` 6 626, `edge/` 4 229 across 24,
`contentPacks/` 1 485, mail 3 308, bookmark 3 313, calendar 2 292, notifications 1 482,
machoNet 992, cache 880, login 618, user 1 179 — roughly **66 000 LOC** in this lane.
The RPC base class every service extends is **158 lines** (`services/baseService.js`),
and services are auto-loaded by filename convention: `bootstrap.js:215-247` walks
`src/services/**` and registers every `*Service.js` export, of which there are **206**.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---|---|---|
| MachoNet protocol service | 5: `machoNetService.js`, `globalConfig.js`, `externalQueueMgrService.js`, `sessionMgrService.js`, `tcpRawProxyService.js` | 992 | functional | `services/machoNet/machoNetService.js:326` `Handle_GetInitVals`; the service directory it returns is a hand-written 160-entry dict at `machoNetService.js:175-321`, with access levels (`"station"`, `"solarsystem2"`, `"character"`) spelled per service |
| GPS handshake + transport | 8 under `network/tcp/`: `handshake.js` (942), `index.js` (644), `directGameSocketTransport.js`, `socketLifecycle.js`, `accountLoginClose.js`, `socketTuning.js`, `gameRuntimeLifecycle.js`, `utils/crypto.js` | 2 180 | functional | `network/tcp/handshake.js:716-725` binds account identity; `:807-854` sends `CryptoHandshakeAck`. Carries an **eve-offline patch dated 2026-10-20** at `handshake.js:838-847` adding `access_token`/`computer_hash` for build 3619879 — so this tree is already ours-modified |
| Marshal codec + framing | `network/tcp/utils/marshal.js` (1 910), `inboundMarshalDecoder.js`, `encode/`, `decode/` | 2 100 | functional | `marshal.js` is the single encode/decode; `clientSession.js:446` calls `marshalEncode` inline |
| Packet dispatch | `network/packetDispatcher.js` | 1 163 | functional | routes by `MACHONETMSG_TYPE`; `:697-721` invokes an optional `service.afterCallResponse(method, session)` hook **after** the CallRsp is written |
| RPC base class | `services/baseService.js` | 158 | functional | `:69-155` `callMethod` tries `Handle_<method>` then `<method>`, handles promises, and falls through to `log.warn("Unhandled method")` returning `null` at `:152-154` |
| Client session object | `network/clientSession.js` | 1 316 | functional | class at `:291`; `sendSessionChange` `:727-844`; `sendNotification` `:872`; `sendServiceNotification` `:986`; `sendClientCallRequest` `:1069`; `sendObjectNotification` `:1215` |
| Session registry (presence) | `services/chat/sessionRegistry.js` | 239 | functional | `Set` + `characterID → Set<session>` index at `:20-22`; `findSessionByCharacterID` `:194-226` with an O(N) fallback scan deliberately retained |
| Canonical identity resolver | `services/_shared/sessionIdentity.js` | 39 | functional | `:24-35` returns **0** rather than a default character, with a header comment naming the exact bug eve-offline has today (`:6-15`) |
| Character control lease | `services/online/characterControlRuntime.js` + 4 | 2 056 | partial | `:8-20` three mutually exclusive control states (`offline` / `retail_client` / `browser_pilot`) with a 60 s browser lease TTL; consumed at `charService.js:1570` as a login preflight. No tests; lease expiry path unverified |
| Account / login / auth | 3 + 20: `authenticationService.js` (333), `multiLoginBlockerService.js` (221), `accountStore.js` (64); `services/account/` walletState, billRuntime, accountService | 7 244 | functional (login) / partial (wallet authority) | `accountStore.js:24` single-writer repo for the `accounts` table; `walletAuthorityProcess.worker.js` (335) is a separate worker thread — an untested concurrency path |
| User service | `services/user/userService.js` | 1 179 | unknown | one file, `userSvc`; not exercised by any command I found |
| Chat channels (LSC RPC) | `lscService.js` (118), `chatHub.js` (370), `channelRules.js` (239), `localChannelMove.js` | 760 | partial-by-design | `chatHub.js:24-34` states plainly that the **V24 client has no live `OnLSC` consumer** and that LSC is advertised in `GetInitVals` for parity but never called; all real delivery moved to XMPP + a protobuf gateway |
| Chat transport (XMPP MUC) | `xmppStubServer.js` (3 387), `xmppChatMgrService.js` (412), `xmppConfig.js`, `xmppSocketLifecycle.js`, `sessionChatSync.js` | 4 150 | functional | `xmppStubServer.js` holds rooms/rosters but **no listening socket**; the TLS socket is in a separate process at `edge/chat/chatEdgeProcess.worker.js:113-121` on port 5222 (`network/protocolProfile.js:17`) |
| Chat auth token | `chatAuthenticationServiceService.js` | 114 | stub | `:96-105` `Handle_GetAuthenticationToken` returns `null` whenever nothing injected a token; the fallback is "local" |
| GM slash console | `chatCommands.js` (2 461) + 33 under `commands/` + `capitalNpc/`, `wormhole/`, `trigDrifter/`; driven by `services/admin/slashService.js` and `lscService.js` | ~12 000 | functional | **176 advertised names** (`chatCommands.js:292-458`: 161 literals + 15 via four spreads); `executeChatCommand` at `:1076` dispatches 162 of them explicitly plus 11 fitting-preset aliases via `resolveSingleRackCommandPreset` at `:2289` |
| Slash RPC surface | `services/admin/slashService.js` | 300 | functional | `Handle_SlashCmd` `:214`; `_throwCommandListError` `:197` raises a `UserError` carrying the command list — the same protocol trap eve-offline's `gm.rs:42-82` already documents |
| Mail | 4: `mailState.js` (2 251), `mailMgrService.js` (584), `mailNotifications.js` | 3 308 | functional | **31** `Handle_*` methods in `mailMgrService.js:198-576` covering headers, bodies, labels, trash, read/unread by label and by list, and `SendMail` at `:548` |
| Mailing lists | `mailingListsMgrService.js` | 317 | functional | **19** handlers `:138-308`: create/join/leave/delete, members, muting, operators, welcome mail |
| Notifications | 4: `notificationState.js` (758), `notificationYaml.js` (378), `notificationMgrService.js` (185), `notificationConstants.js` | 1 482 | functional | **10** handlers `:63-175`; the YAML module implies retail notification-type templates are modelled, not faked |
| Calendar | 10: `calendarRuntimeState.js` (837), `calendarAccess.js` (295), `calendarStore.js`, `calendarMgrService.js` (165), `calendarSchedulerRuntime.js`, `calendarChatCommands.js` (222) | 2 292 | functional | **13** handlers `:24-158` incl. corporation and alliance events; driven end-to-end by `/calauto` (`chatCommands.js:1205`) |
| Bookmarks | 9: `bookmarkRuntimeState.js` (1 548), `bookmarkRuntimeAccess.js` (427), `bookmarkChatCommands.js` (362), `bookmarkService.js` | 3 313 | partial | only **4** handlers in `bookmarkService.js:29-50` (`GetBookmarks`, `CreateFolder`, `GetFolders`, `GetBookmarksInFolder`) against 1 548 lines of runtime state — most of the capability is reachable only through `/bookmarkauto` |
| Object cache | 3: `objectCacheRuntime.js` (731), `objCacheService.js` (116), `cacheService.js` (33) | 880 | functional | 8 handlers `objCacheService.js:41-110`; `objectCacheRuntime.js:25-31` caps at 4 versions per object and compresses above 170 bytes; `packetDispatcher.js:46-48` short-circuits a cached method call to a `CacheOK` macho error |
| Persistence (gameStore) | 17: `index.js` (2 376), `persistenceWorker.js` (2 127), `sqliteStore.js` (1 674), `tableOwnership.js` (430), `persistenceRoles.js`, `persistenceLeasePolicy.js`, 4 migrators | 9 085 | functional | see **Persistence** below; `gameStore/README.md` is the only README in this lane and is accurate against the code |
| Content packs | 2: `contentPackManager.js` (1 234), `autoInstallContentPacks.js` (251) | 1 485 | functional | schema-versioned (`CONTENT_PACK_SCHEMA = "evejs.content-pack.v1"`, `:13`), 128 MB cap, declared-capability allowlist `:26-80` that **refuses** a pack naming an unknown capability; `autoInstallContentPacks.js:20-32` fingerprints the drop folder because re-import measured 56 s on 232 MB of packs |
| Edge processes | 24 under `edge/`: `edgeProcessHost.js` (272), gateway (6), chat (5), game (4), image (2), redshift (2) | 4 229 | partial | each is a `worker_threads` child with its own ordered request protocol and admission control (`edgeProcessHost.js:26-46` argues at length that a queue limit is not admission control). No tests; restart policy is 3 startup attempts, 250 ms delay |
| Marshal worker pool | `marshalWorkerPool.js` (518) + `.worker.js` (99) + 2 task modules | 737 | functional | offload thresholds at `:143-147`: 256 KiB or 5 000 nodes; pool size `os.cpus().length` at `:35`. Pure Node-event-loop mitigation |
| Observability (sync ledger) | `network/syncLedger.js` (379), `spatialTrace.js` (1 452), `nativeTimeDilation.js`, `tickAlignedSends.js` | 2 220 | functional | `clientSession.js:363` tracks socket lifecycle; `:377-389` records `packet.dropped` / `packet.begin` per send |
| `sessionMgr` service | `services/machoNet/sessionMgrService.js` | 88 | **stub** | `Handle_RemoveSessionsFromServer` `:51-60` records an audit event and `return null` with `removedCount: 0`; `Handle_GetSessionStatistics` `:62-72` only counts. Nothing here manages sessions — the name is misleading |
| `tcpRawProxyService` | `services/machoNet/tcpRawProxyService.js` | 96 | **stub** | `Handle_GetTunnelsByType` `:65-71` returns `[]` unconditionally; the tunnel map is only ever populated by `_testing.setTunnelsForTests` `:86` |
| Overview presets | `overviewPresetMgrService.js` (542), `overviewMotdBootstrap.js` | 689 | partial | only 2 handlers (`:512`, `:526`) — store-a-link and fetch-it-back |
| Shared settings | `sharedSettingsMgrService.js` | 304 | partial | 2 handlers (`:263`, `:278`), same store-a-link shape |
| Achievements | 4: `achievementGatewayProto.js` (936), `achievementState.js` (766), `achievementRuntime.js`, `achievementAuthority.js` | 2 297 | partial | delivered over the **protobuf gateway**, not macho RPC (`achievementGatewayProto.js`) — i.e. not reachable by the client path eve-offline implements |
| Activity (filaments etc.) | 14: `pvpFilamentMgrService.js`, `randomJumpMgrService.js`, `warpVectorMgrService.js`, `rwManagerService.js`, `encodedItems.js` | 1 880 | partial | many small services, each 150-230 LOC; `pvpFilamentQueueRuntime.js:216` hooks session disconnect |
| Rewards / login campaigns | `rewardMgrService.js` | 68 | stub | audit-event recorder with a 100-entry ring; no reward state |
| Seasons / campaign trail | 27 small files, largest `campaignTrailRuntime.js` (293) | 2 045 | partial | persisted tables `campaignTrailProgress`, `eventLifecycle` classified `INFERRED` in `tableOwnership.js:162-165` — i.e. not confidently attributed even by their own audit |
| Daily goals | 3: `dailyGoalsState.js` (1 296), `dailyGoalsProto.js` (486), `dailyGoalsCatalog.js` | 2 151 | partial | again protobuf-gateway-delivered |
| NPE / tutorial (AIR) | 14: `tutorialRuntime.js` (505), `airEncounterRuntime.js`, `airNpeRuntime.js`, `airCombatRuntime.js`, `airMiningRuntime.js` | 2 042 | partial | hooked into character selection at `charService.js:1722-1725` inside a `try/catch` that only warns |

## The session model

### What a session *is*

One object per TCP connection, constructed once the handshake reaches `DONE` and kept
for the life of the socket: `network/clientSession.js:291-364`, constructed at
`network/tcp/index.js:282-309`. It is a **plain mutable object**, not a handle into a
table, and it carries three distinct layers that eve-offline currently conflates:

1. **Account identity**, from the handshake and never changed afterwards:
   `userid`, `userName`, `clientID`, `accountRole`, `chatRole`, composed `role`,
   `languageID`, `countryCode`, `sid` (`clientSession.js:299-315`).
2. **Character identity and location**, all zero/empty at construction
   (`:317-344`) and filled only by character selection.
3. **Transport state**: socket, crypto functions, `connectTime`, `lastActivity`,
   `_notificationSequenceNumber`, `_outboundTail` (`:346-363`).

A session also accretes `_`-prefixed per-feature scratch state in place —
`_space`, `_safeLogoff`, `_characterControlDisconnected`, `_pendingLocalChannelSync`,
`_abyssalOfflineSession`, `_localChatDeparted`. `sessionRegistry.unregister`
(`sessionRegistry.js:135-153`) has to know about eight of these to tear a session down.
**This is the part to not copy.** It is why their own disconnect path is a hardcoded list
of eleven `require(...).handleSessionDisconnected(session)` calls.

### How it is keyed

Three different identifiers, and they do different jobs:

- **`clientID`** — the transport key. `nextClientID()` at
  `network/tcp/handshake.js:179-185` returns `offset * 10_000_000_000 + PROXY_NODE_ID`
  (`PROXY_NODE_ID = 0xffaa`, `network/protocolProfile.js:16`), a monotonic counter
  encoding the node. The comment at `handshake.js:175-178` is the load-bearing one:
  *"Two connections of one account must not share one: whatever is kept by client ID (a
  solar system's sessions, for one) then keeps only the later of the two."* This is the
  address a push is routed to — `clientSession.js:789-794` puts `clientID` in the
  destination `MachoAddress` of every session change.
- **`sid` / `sessionID`** — `createSessionID(clientId)` at `handshake.js:187-190`:
  `BigInt(Date.now()) * 15n + BigInt(clientId % 15)`. Echoed in the handshake ack
  (`:831`) and in the first element of every `SessionChangeNotification` payload
  (`clientSession.js:776`). It identifies the *session instance*, not the connection.
- **`characterID`** — the gameplay key, and the one everything else indexes by.
  `sessionRegistry.js:21-22` keeps `characterID → Set<session>` eagerly, maintained by
  `indexCharacterSession` / `deindexCharacterSession` and called from
  `applyCharacterToSession` (`characterState.js:4408-4410`).

There is **no account → session map**. `closeOtherSessionsOfUser`
(`network/tcp/accountLoginClose.js:70-85`) scans every live session comparing `userid`.
At their scale that is fine; it is a design choice worth copying, not a defect.

### `clientSession.js` vs `sessionMgr`

They are unrelated, and the naming is a trap. `services/machoNet/sessionMgrService.js`
is an **88-line stub**: `Handle_RemoveSessionsFromServer` (`:51-60`) records an audit
event and returns `null` with `removedCount: 0`; `Handle_GetSessionStatistics`
(`:62-72`) counts the registry. Real session management is split across three places:

- `network/clientSession.js` — the object and its **push helpers**.
- `services/chat/sessionRegistry.js` — the live set, the presence index, and
  **registration/teardown**. It lives under `services/chat/` for historical reasons and
  is required by 30+ modules outside chat.
- `services/character/characterState.js:4287-4598` — `applyCharacterToSession`, the
  **one function** that binds a character to a session.

### Multiple characters per account

An account row owns characters by a back-reference on the character, not a list on the
account: `charService.js:465-471` `characterBelongsToAccount(character, accountID)`
compares `character.accountId` to `session.userid`.
`Handle_GetCharacterSelectionData` (`charService.js:857-900`) scans the whole
`characters` table filtering on that predicate, and hardcodes
`characterSlots: 3` / `maxCharacterSlots: 3` (`:878-882`).

Selecting a character calls `applyCharacterToSession(session, charId, …)`
(`charService.js:1708`). That function is **430 lines** and its shape is the specification
eve-offline needs:

1. Snapshot ~35 *old* values off the session (`characterState.js:4328-4364`) — not for
   rollback, but because a `SessionChangeNotification` is a dict of
   `key → [old, new]` pairs and the client's `ApplyRemoteAttributeChanges` wants both.
2. Write ~60 fields onto the session, **each in two spellings** — `characterID` *and*
   `charid`, `corporationID` *and* `corpid`, `solarsystemid` *and* `solarsystemid2`
   (`:4399-4504`). The duplication is not cosmetic: the client reads the lowercase
   names, server code reads the camelCase ones, and `sessionIdentity.js:18` has to try
   four spellings of the character id.
3. Re-index the registry (`:4408-4410`), skipping it for a headless Abyssal owner so a
   non-player session does not leak presence or block relog — a precise statement of
   "a session that is not a player's client must not count as presence".
4. Build a *notification plan* (`:4540-4581`) and flush it only if
   `options.emitNotifications !== false` (`:4583`). Callers that re-apply a character to
   refresh affiliations pass `selectionEvent: false, emitNotifications: true`
   (`chatCommands.js:891-895`); callers inside a larger transaction pass
   `emitNotifications: false` and flush later (`charService.js:1708-1709`).

**Re-selection reuses the same session object**, which they document as a hazard:
`characterState.js:4303-4310` clears deferred docked-ship state on every fresh
selection because *"any deferred docked-ship restore still hanging off a previous
character selection can flush into the new login and restore the wrong shipid"*.

### How a session change reaches the client

`clientSession.sendSessionChange(changes, options)` — `clientSession.js:727-844`:

- **Flush first.** `flushTickAlignedSends(this)` at `:730`, with the comment that the
  place being left must get its last updates *before* the change.
- **Filter against an allowlist.** `SESSION_CHANGE_ALLOWED_KEYS` is a 34-name `Set` at
  `:58-91`; anything else is dropped and logged (`:742-747`). An empty change set is
  skipped unless `allowEmpty` (`:762-767`).
- **Payload shape** (`:775-779`, citing `General.xmlp`):
  `(sessionID: long, (clueless: int = 0, changes: dict), nodesOfInterest: listInt)`,
  wrapped in a 14-element macho tuple at `:781-806` with
  `MACHONETMSG_TYPE.SESSIONCHANGENOTIFICATION = 16`, source `node` address, destination
  `client` address, `userid`, and **seven trailing `null`s** — elements 8 and 9 are
  commented as *required* (`:799-800`).
- **Then two follow-ups**, after the packet: tale data for the new system (`:828`) and
  `synchronizeSessionChatState` in a `try/catch` (`:830-843`) so chat membership
  follows the location change.

Notifications get a monotonic `sn` sequence number per session
(`clientSession.js:111-121`) — a detail eve-offline's push path does not currently have.

Side effects that must land **after** the CallRsp is on the wire are deferred, not
inlined: a command sets `session._pendingLocalChannelSync` /
`_pendingCommandInitialBallpark`, and `packetDispatcher.js:697-721` calls
`service.afterCallResponse(method, session)` once the response is written, which runs
`flushPendingCommandSessionEffects` (`commandSessionEffects.js:81-103`). Compare
eve-offline's `dispatch.rs:556-577` `with_session_change`, which puts the notification
**before** the response. Both orders are defensible for different packets; the point is
that they needed *both* and have a hook for the second.

### Two simultaneous clients

Two independent guards, at two layers:

- **Per account, at the transport.** On every successful handshake,
  `closeOtherSessionsOfUser(clientSession)` (`network/tcp/index.js:317`,
  implemented `accountLoginClose.js:64-87`) sends the earlier session a
  `GPSTransportClosed` with code `CLIENTDISCONNECTED`, then `socket.end()` and a 5 s
  grace before `socket.destroy()` (`:30-47`). The header comment (`:1-14`) cites CCP's
  own `sessionMgr.CloseUserSessions` and `machoNet.DisconnectUsers`, and honestly admits
  *"neither the client's code nor a recording says which close Tranquility sends"*.
- **Per character, at selection.** `charService.js:1586-1620`:
  `findSessionByCharacterID(charId, { excludeSession: session, includeClosing: true })`.
  If one exists, either evict it (`config.loginTakeoverEnabled`) or raise
  `"<name> is already online."`. `includeClosing: true` is there because a destroyed
  socket is still the controller until teardown deindexes it — i.e. **a closing session
  must still block a new one** or the two race.

On top of that, `characterControlRuntime.js:8-20` gives a character exactly one
controller across *two* transports (`retail_client` over TCP, `browser_pilot` over web,
60 s lease), checked as a login preflight at `charService.js:1570`. For eve-offline's
stated end goal — friends plus player-like bots — **this is the model to take**: a bot is
a controller holding a lease on a character, not a fake session in the registry.
`sessionRegistry.js:3-6` says exactly that: *"Character control and online presence are
derived through online/characterControlRuntime so browser-pilot leases never need fake
sessions in this registry."*

### What eve-offline's identity split must support

Today: `Session` is 5 fields with `user_id: 1` hardcoded in three places
(`session.rs:39`, `handshake.rs:75`, `service.rs:218`) and **25** `characters.first()`
call sites. Prescriptively, the split must deliver:

1. **Three separate types, not one struct.** `Account` (id, name, role mask) ·
   `Character` (id, name, corp, owned-by account id) · `Connection`/`Session` (the
   socket, the client id, the *currently selected* character id as an `Option`).
   `charid` absent must stay distinguishable from `charid == 0` — `session.rs:50-53`
   already depends on that for character select.
2. **Three keys, kept distinct.** A monotonic `client_id` encoding the node (copy
   `handshake.js:179-185`; our push destination already needs it —
   `dispatch.rs:568` currently passes `self.session.user_id` as the client address,
   which is the bug the split must fix). A `session_id`. And `character_id` as the
   gameplay key.
3. **A registry keyed by character id, with an authoritative "who controls this
   character" answer.** One lookup, `find_by_character(id) -> Option<&Connection>`, that
   a bot controller and a real client both resolve through.
4. **One `apply_character_to_session` function** that is the *only* writer of character
   fields onto a session, returns the `(old, new)` pairs for the session change, and
   takes flags for `emit_notifications` and `selection_event` — because three callers
   already need the three combinations (select, refresh-affiliation, in-transaction).
   Replace all 25 `characters.first()` sites with a resolver that returns
   `Option<CharacterId>` and **has no default** — `sessionIdentity.js:6-15` is a written
   post-mortem of taking the default, and it names the exact hazard
   ("a session with no identity acts as a specific real player (a cross-account
   hazard)").
5. **A session-change allowlist.** `SESSION_CHANGE_ALLOWED_KEYS`
   (`clientSession.js:58-91`) is 34 names; it is cheap, it is a const, and it turns a
   wrong key from a client-side mystery into a logged drop.
6. **Both orderings for pushes.** Notification-before-response (we have it) *and* a
   post-response hook (we do not). `commandSessionEffects.js` exists because the
   initial-ballpark send must not happen before the client has the CallRsp.
7. **Duplicate-login handling at both layers**, with the closing-session subtlety:
   a socket being torn down still owns its character.

## Persistence

### Their model

`gameStore` is an **in-memory cache with a write-behind SQLite backend**, behind a
three-call synchronous API: `read(table, path)`, `write(table, path, value)`,
`remove(table, path)` (`gameStore/README.md`, verified against `index.js:2126`,
`:2202`, `:2275`). Data is in three tiers (README table, confirmed by
`tableOwnership.js:41-46`): SDE-generated JSON (regenerable, read-only in memory),
hand-curated static JSON (in git), and **runtime** tables, which are the only ones
that persist.

**Schema.** One SQL table per runtime table, `"<table>"(key TEXT PRIMARY KEY, json TEXT)` —
a key/JSON-blob store, not a relational model. SQLite is used for ACID and per-row
upserts, **not** for queries: the README's own query example has to reach for
`json_extract`. A few tables nest one level and are split into per-entity rows via a
`ROW_GROUPS` map (`sqliteStore.js`), so that touching one notification box does not
rewrite every box.

**When a write happens.** Never at the moment of the mutation. `write()` mutates the
cache, marks the touched row dirty, and calls `scheduleFlush(table)`
(`index.js:2262-2266`). `scheduleFlush` (`:1731-1741`) is a **per-table debounce**:
`clearTimeout` then `setTimeout(flushTable, FLUSH_DELAY_MS)` with
`FLUSH_DELAY_MS = 2000` (`:185`). So a burst of writes to one table collapses to one
flush 2 s after the last one. `write()` also **drops no-op writes**: `isSameValue`
comparison at `:2225` and `:2259` returns success without dirtying.

**How writes are batched.** Per table, per flush, diffed against a per-row baseline:
`flushSqliteTable` (`:1504-1606`) collects only the dirty row keys
(`dirtyRowKeys[table]`, `:760`) and emits an upsert/delete set. That set is handed to
`persistenceWorker`, a `worker_threads` child, so the synchronous `better-sqlite3`
transaction does not stall the simulation loop (`persistenceWorker.js:1-12`).
`flushAllSync` (`index.js:1874`) is the shutdown path and flushes every dirty table
inline.

**Two-phase durability.** A `_persistence_outbox` table
(`sqliteStore.js:341-360`) holds the operation *before* it is applied:
`operation_id` autoincrement, `table_name` UNIQUE, the owner's role/epoch/instance,
`upserts_json`, `deletes_json`, and `state IN ('pending','applied')`. The outbox row
survives until the worker's success is acknowledged, so a crash between submit and
apply is recoverable rather than lost.

**What the lease is for.** `_persistence_owners` (`sqliteStore.js:369-380`):
`owner_role` PK, `epoch > 0`, `instance_id`, `active`, `exclusive`,
`lease_expires_at`. `acquirePersistenceOwner` (`:509-620`) runs in one transaction:
if a live lease exists for a different instance it **throws**
`PERSISTENCE_OWNER_ACTIVE`; otherwise it bumps the epoch (`:588-592`) and upserts with
`lease_expires_at = now + leaseMs`. Default lease 30 s, minimum configured 3 s
(`persistenceLeasePolicy.js:5-10`) — which is the `leaseMs=30000` in the boot log.
Roles are `world | wallet | scheduler | maintenance | reader | standalone`
(`persistenceRoles.js:3-10`); each table has exactly one owning role
(`:12-15`, default `world`), `reader` may mutate nothing, and `maintenance` is
exclusive and evicts other leases. Every `write`/`remove`/`flush` asserts the role
(`index.js:2208`, `:2276`, `:1752`) *and* `assertPersistenceOwnerHealthy()`, which
throws once `Date.now() >= leaseExpiresAt` (`persistenceWorker.js:513-525`).

So the lease answers **two** questions, and they are worth separating:

- *Mutual exclusion*: only one process may be the `world` writer of one database file.
  The epoch is the fence — a stale process that resumes after its lease lapses finds a
  higher epoch and fails closed.
- *Liveness*: a crashed owner's lease simply expires, and the next process can take over
  after `leaseMs + 5 s` (`persistenceLeasePolicy.js:31-33`), which is why the
  supervisor's startup deadline has to encompass it (`:42-48`).

**What happens on a crash.** Three independent mechanisms: SQLite WAL means a torn
write cannot corrupt a table; the outbox means an un-acknowledged operation is replayed
(`index.js:672-700` `recoverPersistenceOwnerOperations`, opt-in via
`acquirePersistenceOwnerLease({ recover: true })`); and the lease means the dead owner
stops being the owner without anyone cleaning up. **But up to 2 s of mutations
(`FLUSH_DELAY_MS`) are in RAM only and are lost.** That is a deliberate trade, not a
bug, and it is the number eve-offline should compare against.

### What eve-offline should learn, without SQLite

Our `store.rs` is a whole-world snapshot: tmp + fsync + rename, a `SCHEMA: u32`
refused rather than guessed (`store.rs:49-51`), saved from `dispatch.rs:1168` inside
the write lock whenever `take_dirty()` is true. Five things to take, none of which
needs a crate:

1. **Debounce, don't save per mutation — but keep the "only when dirty" rule.**
   We already have `take_dirty()`. Add their 2 s coalescing window so a GM typing five
   `/giveitem`s is one fsync, not five. Their comment at `dispatch.rs:1151-1156` already
   worries about an fsync on the draw path; a debounce is the fix, and it is a timestamp
   plus a comparison.
2. **Partition the file by owner, so a flush is not a whole-world rewrite.**
   Their per-row diff is the one thing SQLite genuinely buys them, but the *idea* is
   backend-independent: one append/replace unit per logical table. A line-oriented
   format can do this as one file per table in a directory, each written tmp+fsync+rename
   — same atomicity, and a save costs the tables that changed instead of the world.
   `tableOwnership.js` is the map to copy: **one owner per table, no shared writers**,
   and `persistenceRoles.js:58-96` turns that map into a runtime assertion. Do the
   assertion; it is 20 lines and it catches the class of bug where two subsystems both
   think they own `items`.
3. **Keep the single-writer lock, drop the distributed lease.** The lease exists because
   they run multiple processes against one file. We are one process, so the
   mutual-exclusion half is answered by `RwLock`. The **liveness** half is still real and
   we have nothing for it: a second `eve-offline` started against the same world
   directory would silently interleave writes. A **PID + boot-time lockfile**, written
   with `O_EXCL` and refused on a live PID, is the std-only form of the same guard, and
   it is strictly better than a lease because we do not need the renew timer at all.
   Flag as a design proposal, not a port.
4. **Write the intent before the apply.** Their outbox is the mechanism that makes a
   crash *mid-apply* recoverable rather than merely non-corrupting. tmp+fsync+rename
   already gives us "old or new, never half" for one file; it does **not** give us
   "all five tables or none" once (2) splits the file. If we split, we need either a
   single rename of a directory-level manifest, or an intent line written and fsynced
   before the per-table renames and cleared after. Cheap, and it is the difference
   between a consistent world and a world where the item moved but the wallet did not.
5. **Keep the fatal schema check and make it louder.** Theirs is weaker than ours —
   they auto-seed from a legacy `data.json` once and record a `_migrations` marker
   (README, "First load / auto-seed"), with the explicit property that *a later delete is
   never resurrected from the stale JSON*. That second property is the one we lack: we
   should refuse to silently fall back to an empty world when a file is unreadable.
   `store.rs:49-51` is already right about refusing; the gap is the "don't resurrect"
   rule.

Do **not** take: the `worker_threads` persistence worker (its entire reason is that
`better-sqlite3` is synchronous and Node has one loop — see "Architecture we should NOT
copy"), the epoch/instance fencing, or the JSON-blob-in-SQL schema, which gets ACID at
the cost of being unqueryable without `json_extract`. Our grep-able text file is the
better answer to the one thing they said they valued most.

## GM command surface

**Measured:** `AVAILABLE_SLASH_COMMANDS` (`chatCommands.js:292-458`) advertises **176**
names — 161 literals plus four spreads contributing 15 (`CAPITAL_NPC` 10,
`WORMHOLE` 2, `TRIG_DRIFTER` 2, `GATE_SKIN` 1). `executeChatCommand` (`:1076`)
dispatches 162 of them by explicit comparison plus 11 fitting-preset aliases resolved at
`:2289`. The coordinator's "~144" is the right order of magnitude; the real number is
higher.

**How a lane drives one.** Two entry points, both reaching the same
`executeChatCommand(session, line, chatHub, options)`:

- `slash.SlashCmd` — `services/admin/slashService.js:214`. This is the path eve-offline
  already has (`gm.rs:32`). Note `_throwCommandListError` at `:197`: `SlashCmd("/")`
  must **raise** a `UserError` whose reason is `Commands: [...]`, which is the same trap
  `gm.rs:42-82` documents.
- `LSC.SendMessage` — `services/chat/lscService.js:82-107`. Any chat line starting with
  `/` **or `.`** (`chatCommands.js:1078`) is tried as a command first, and broadcast to
  local chat only if unhandled (`lscService.js:99-101`).

**Access control is one bit, checked in two places.** `hasStaffDebugRole(session)`
(`services/chat/commands/commandAccess.js:11-21`) tests `ROLEMASK_VIEW` against
`accountRole ?? role ?? rolesAtAll`. It gates the *help text* (`chatCommands.js:1070`)
and four commands (`/lco` subcommands `:1327`, `/events` `:1800`, and the abyssal staff
block). **Everything else is ungated.** For eve-offline this is a feature: the GM
console is the test harness and should not need a role dance.

Unknown commands get a Levenshtein suggestion list (`:634-659`) — nice, not essential.

Grouped by the subsystem each command exercises:

### Session / identity / presence — *the lane this inventory serves*
| command | subsystem it exercises | what it does |
|---|---|---|
| `/session` | client session | prints char, ship, corp, docked location, system, wallet off the session object (`getSessionSummary` `:998-1011`) — the one-line "what does the server think I am" |
| `/who` | session registry | lists every connected character and its location, deduped by preferred session (`getConnectedCharacterSummary` `:962-996`) |
| `/where` | session location fields | docked-vs-in-space from `getDockedLocationID` (`getLocationSummary` `:919-935`) |
| `/backintime [me\|charID\|name]` | character state rollback | `handleBackInTimeCommand` `:1255` |
| `/tele <name\|charID>` | session + space teleport | `handleTeleCommand` `:1762` — moves one session to another character's location |
| `/announce <msg>` | chat broadcast to all sessions | `sendAnnouncement` `:1057-1067` iterates the registry |
| `/motd` | chat | returns `DEFAULT_MOTD_MESSAGE` (`:143`) |
| `/help`, `/commands`, `/commandlist` | slash console | role-filtered help text (`:1069-1074`) |

### Chat
| command | subsystem it exercises | what it does |
|---|---|---|
| `/blue`, `/red`, `/teal`, `/yellow` | chat colour / role presence | `handleChatColorCommand` `:1117-1122`; sets the `refreshChatRolePresence` flag that `lscService.js:96-98` acts on |
| `/mailme` | mail delivery | sends the caller a mail (`handleMailMeCommand` `:818-871`) — the cheapest end-to-end mail test |

### Wallet / economy
| command | subsystem it exercises | what it does |
|---|---|---|
| `/wallet`, `/isk` | wallet | balance, PLEX, EverMarks, last delta (`getWalletSummary` `:899-917`) |
| `/addisk <n>` / `/setisk <n>` | wallet mutation | `:1821` / `:2053` |
| `/addplex <n>` / `/setplex <n>` | PLEX balance | `:2009` / `:2241` |
| `/evermarks`, `/addevermarks`, `/setevermarks` | EverMark (LP) wallet | `:1555` / `:1922` / `:2154` |
| `/corpevermarks`, `/addcorpevermarks`, `/setcorpevermarks` | corp LP wallet | `:1577` / `:1965` / `:2197` |
| `/lp [corp]`, `/addlp`, `/setlp` | loyalty points per NPC corp | `:1546` / `:1869` / `:2101` |

### Inventory / items / fitting
| command | subsystem it exercises | what it does |
|---|---|---|
| `/giveitem <name\|typeID> [n]`, `/item` | item store | `handleGiveItemCommand` `:2365` |
| `/create`, `/createitem` | item creation | `handleCreateItemCommand` `:2357` |
| `/minerals` | item store bulk seed | `handleMineralsCommand` `:2353` |
| `/load <charge>` | ammo loading | `handleLoadCommand` `:2361` |
| `/fit me <module>` | fitting / dogma | `handleFitCommand` `:2369` |
| `/unfit me <module\|all>` | fitting / dogma | `handleUnfitCommand` `:2373` |
| `/hangar` | station hangar | active ship + hangar ship list (`getHangarSummary` `:1013-1033`) |
| `/iteminfo <itemID>` | item store read | type, owner, location, flag, singleton, quantity (`getItemSummary` `:1035-1055`) |
| `/typeinfo <name\|typeID>` | type registry | `:2389` |
| `/container`, `/container1`, `/spawncontainer` | containers in space | `handleDebrisFieldCommand(..., "container")` `:1231`, `:2349`, `:1288` |
| `/jetcan <item> [n]` | jettison can | `handleJetcanCommand` `:1227` |
| `/wreck`, `/spawnwreck` | wrecks + loot | `handleDebrisFieldCommand(..., "wreck")` `:1296`, `:1292` |
| `/bp <sub>`, `/bpauto <sub>` | blueprints / industry | `:1195`, `:1190` (async) |
| `/reprocesssmoke <sub>` | reprocessing | `:1210` (async) |

### Ships
| command | subsystem it exercises | what it does |
|---|---|---|
| `/ship <name\|typeID>`, `/giveme` | ship spawn + board | `handleShipSpawn` `:2285` |
| `/gmships` | ship catalogue | `handleGmShipsCommand` `:1235` |
| `/gmweapons` | weapon catalogue | `:1239` |
| `/laser`, `/lasers`, `/hybrids`, `/railgun`, `/projectiles`, `/autocannon`, `/rocket`, `/light`, `/heavy`, `/torp` | fitting presets (single rack) | `resolveSingleRackCommandPreset` `:2289` → `commands/fittingPresets.js:100-343` |
| `/lesmis` | mixed heavy-missile + pulse fit | `:2300` |
| `/prop` | propulsion module | `:1243` |
| `/dirt <0-1> [shipID]` | ship appearance | `handleShipDirtCommand` `:1368` |
| `/killmarks <n>`, `/killmark` | ship appearance | `:1373` |
| `/allskins [target]` | SKIN grants | `:1251` |
| `/grantshipemblem`, `/grantcorplogo`, `/grantalliancelogo` | ship logos | `handleGrantShipLogoCommand` `:1601-1605` |
| `/dmg [light\|medium\|heavy]` | ship damage model | `handleDamageCommand` `:1153` |
| `/heal` | repair | `:1157` |
| `/invu [on\|off]` | invulnerability | `handleInvuCommand` `:1480` |
| `/rr` | remote-repair fleet | `executeRemoteRepairFleetCommand` `:1510` |

### Skills
| command | subsystem it exercises | what it does |
|---|---|---|
| `/allskills` | skill store | grants everything (`:1247`) |
| `/gmskills` | skill catalogue | `:1263` |
| `/giveskill <target> <skill\|all\|super> [lvl]`, `/giveskills` | skill grant | `:1267` |
| `/removeskill <target> <skill\|all>`, `/removeskills` | skill removal | `:1271` |
| `/expertsystem <list\|inspect\|status\|add\|remove\|clear\|giveitem\|consume>`, `/expertsystems` | expert systems | `:1259` |

### Space / movement / scene
| command | subsystem it exercises | what it does |
|---|---|---|
| `/dock` | docking | docks at home station (`handleHomeDock` `:1149`) |
| `/solar <system>` | system teleport | `handleSolarTeleport` `:1758` |
| `/tr <me\|charID\|entityID> <dest\|bookmark=\|pos=x,y,z\|offset=>` | transport / positioning | `handleTransportCommand` `:1817` → `commands/transport.js` (1 533 LOC, the largest command module) |
| `/loadsys`, `/loadallsys` | solar-system scene load | `:1275`, `:1279` |
| `/celestial <typeID>` | celestial spawn | `:1320` |
| `/lco <typeID>` / `/lco list\|inspect\|move\|offset\|rotate\|save` | large collidable objects | `:1324-1338`; the subcommands are the one **staff-gated** branch (`:1327`) |
| `/spawnsite <templateID> [offset]` | dungeon / content-pack templates | `handleSpawnSiteCommand` `:1316` — the way to drive the content-pack loader |
| `/sysjunkclear`, `/testclear` | scene cleanup | `:1523`, `:1519` |
| `/tidi [0.1-1.0]` | time dilation | `handleTimeDilationCommand` `:1283` → `commands/timeDilation.js` |
| `/effect <name>` | special FX registry | `:1161` → `specialFxRegistry.js` (589 LOC) |
| `/fireshow [n\|stop]`, `/fire [ship\|typeID]`, `/fire2 [n]` | native formation / FX | `:1165`, `:1488`, `:1492` |
| `/gate <type> <dest>`, `/gateshowcase [dest\|stop]` | transient stargates | `:1169` |
| `/gateskin [start\|stop\|list]` | gate skins | `GATE_SKIN_CHAT_COMMANDS`, `space/gateSkinCommand.js:33` |
| `/deer_hunter` | microjump effect | `:1112` |
| `/overlayrefresh` | sensor overlay | `:1796` |
| `/sigs`, `/sigscan` | scanning signatures | `:1783`, `:1779` |
| `/orbitals [planetID]` | customs offices / skyhooks | `:1128`, works while docked |

### NPCs / combat
| command | subsystem it exercises | what it does |
|---|---|---|
| `/npc [n] [faction\|profile\|pool]` | NPC spawn | `handleNpcCommand` `:1341` |
| `/mnpc [n]` | missile NPCs | `:1345` |
| `/npcw [n]`, `/wnpc` | warping NPCs | `handleNpcWarpCommand` `:1436` |
| `/npcclear <system\|radius <m>> [npc\|concord\|all]` | NPC cleanup | `:1440` |
| `/npctest`, `/npctest2` | NPC test packs (player / FFA) | `:1378`, `:1382` |
| `/concord [n]` | CONCORD response | `handleConcordCommand` `:1444` |
| `/gateconcord [on\|off]`, `/gaterats [on\|off]` | gate operators | `handleGateOperatorCommand` `:1460`, `:1470` |
| `/capnpc`, `/capnpcinfo`, `/capnpcstatus`, `/capnpcclear`, `/capnpctarget`, `/capnpchome`, `/capnpcfighters`, `/capnpcsuper`, `/capnpcsignoff`, `/capnpcperf` | capital NPCs + fighters + superweapons | `capitalNpc/index.js:7-18`; handlers in `capitalNpc/capitalNpcCommandHandlers.js` (870 LOC) |
| `/trigspawn [n] [status\|clear\|hull\|role]`, `/drifter [n] [...]` | Triglavian / Drifter NPCs | `trigDrifter/index.js:34-37`; `trigDrifter/index.js` is 1 358 LOC |
| `/supertitan`, `/supertitanshow [n]`, `/titansupershow [n]` | superweapons | `:1496`, `:1501` |
| `/cburst`, `/guardian`, `/basilisk`, `/ewar`, `/trig [hull\|family]` | NPC behaviour packs | `:2329`, `:2334`, `:2339`, `:2344`, `:2324` |
| `/miner`, `/orca`, `/probe`, `/probe2` | NPC role ships | `:2304`, `:2309`, `:2314`, `:2319` |
| `/npcminer [n]`, `/npcmineraggro`, `/npcminerpanic`, `/npcminerretreat`, `/npcminerresume`, `/npcminerhaul`, `/npcminerclear`, `/npcminerstatus` | NPC mining fleet state machine | `:1386-1423`, all via `mining/miningCommandService` |
| `/miningreset`, `/miningstatus` | mining state | `:1426`, `:1431` |

### Death / destruction
| command | subsystem it exercises | what it does |
|---|---|---|
| `/suicide` | ship + pod destruction, clone jump | `handleSuicideCommand` `:1300` |
| `/deathtest [ship\|typeID] [n]` | destruction pipeline | `:1304` → `commands/deathTests.js` (427 LOC) |
| `/deathstructure <sovhub\|skyhook\|…\|typeID> [n] [delay]` | structure destruction | `:1308` |
| `/deadwarp` | warp-while-dead edge case | `:1312` |
| `/keepstar` | keepstar spawn | `:1484` |

### Crimewatch / security / standings
| command | subsystem it exercises | what it does |
|---|---|---|
| `/cwatch [status\|clear\|safety <full\|partial\|none>\|weapon\|pvp\|npc\|criminal\|suspect\|disapproval <off\|secs>]` | Crimewatch timers and flags | `handleCrimewatchCommand` `:1448` → `commands/crimewatch.js` (536 LOC) — the richest single command |
| `/naughty` | criminal flag | `:1452` |
| `/secstatus [status]` | security status | `:1456` |
| `/setstanding <value> <owner> [target]` | standings | `:1766` |
| `/maxagentstandings [target]`, `/fullstandings` | agent standings | `:1770` |

### Corporation / alliance / sovereignty / structures
| command | subsystem it exercises | what it does |
|---|---|---|
| `/corpcreate <name>` | corporation creation | `:1614` |
| `/setalliance <name>`, `/joinalliance <name>` | alliance creation / join | `:1651`, `:1704` |
| `/upwell <sub>`, `/upwellauto <type\|structureID>` | Upwell structures | `:1173`, `:1184`; both log via `structureLog.logCommand` |
| `/pos <sub>` | starbases (POS) | `:1179` |
| `/sov <sub>`, `/sovauto <sub>`, `/autosov` | sovereignty | `:1215`, `:1221`; log via `sovLog.logCommand` |

### Social services (the lane's own)
| command | subsystem it exercises | what it does |
|---|---|---|
| `/bookmarkauto <sub>` | bookmarks | `executeBookmarkAutoCommand` `:1200` → `bookmark/bookmarkChatCommands.js` (362 LOC). **The only way to drive the 1 548-line bookmark runtime**, since `bookmarkService.js` exposes 4 handlers |
| `/calauto <sub>` | calendar | `executeCalendarAutoCommand` `:1205` → `calendar/calendarChatCommands.js` (222 LOC) |
| `/mailme` | mail | see Chat above |
| `/events` | seasons / event lifecycle | `:1800`, **staff-gated** |
| `/missioncomplete [agentID\|all]` | agents / missions | `:1787` |

### Abyssal / wormholes
| command | subsystem it exercises | what it does |
|---|---|---|
| `/abyssaldebug suppressor <on\|off\|status>` | abyssal debug | `:1133`, staff |
| `/abyssallayout <list\|status\|next <id>\|clear>` | abyssal room layout | `:1137`, staff |
| `/abyssalstatus [status\|weather\|fx\|scenery\|help]` | abyssal run state | `:1141`, staff — their own help text (`:632`) admits group ownership, admission, survivor and recovery semantics are *"EveJS-authored policy rather than a retail-exact claim"* |
| `/abyssalrescue` | abyssal relog recovery | `:1145`, staff |
| `/wormhole <status\|ensure\|random\|clear>`, `/wormholes [here\|all\|system]` | wormholes | `wormhole/index.js:7-19` |

eve-offline's side today: `Registry::SLASH_COMMANDS` is **8** names
(`service.rs:940-952`) — `createitem`, `giveisk`, `giveskill`, `removeskill`,
`givestarterskills`, `setstanding`, `whoami`, `pushtest` — dispatched by a
`match cmd` in `service.rs:1199-1205`. The `slash` service is registered
(`gm.rs:16`) and the `/`-raises trap is already handled (`gm.rs:42-82`). The gap is
breadth, not mechanism: **the dispatcher is right and the command set is 5% of theirs.**

## Dependency order

```
                    marshal codec
                          |
                  GPS handshake + framing
                          |
              +-----------+-----------+
              |                       |
        packet dispatch         clientSession (push helpers)
              |                       |
        baseService (RPC)       session change + notification
              |                       |
              +-----------+-----------+
                          |
                  machoNet.GetInitVals   <-- gates EVERY other lane
                          |
              +-----------+-----------+-----------+
              |           |           |           |
        sessionRegistry  objectCache  persistence  contentPacks
              |                           |
      applyCharacterToSession ------------+
              |
      +-------+-------+-------+-------+
      |       |       |       |       |
   chat   GM slash  mail  calendar bookmarks
      |       |
   XMPP   (drives every other lane)
```

Each edge, and why:

- **marshal → everything.** Nothing can be said to the client without it. Already done
  on our side.
- **handshake → clientSession.** The session's account identity, `clientID` and `sid`
  all come out of the handshake (`clientSession.js:299-315` reads `handshakeData`).
  `clientID` in particular is minted there (`handshake.js:718`) and is the push address.
- **clientSession → session change.** Session changes are a *method on the session*
  (`:727`), not a free function, because they need the socket, the `sid` and the
  notification sequence number.
- **packet dispatch + baseService → every service.** `baseService.callMethod`'s
  `Handle_<method>` convention is what makes 206 services addressable without a
  registration table.
- **`machoNet.GetInitVals` → every other lane. This is the hard prerequisite.** The
  160-entry service dict at `machoNetService.js:175-321` is what the client consults to
  decide *where to route a call* (the comment at `:166-168` cites
  `ServiceCallGPCS.py:197`). A service absent from that dict is unreachable no matter how
  complete its implementation. **Flag for the coordinator: any lane adding a service must
  also add its name and access level to this dict**, and the access level
  (`null` / `"station"` / `"solarsystem2"` / `"character"`) is not cosmetic — it decides
  whether the client binds or calls directly.
- **sessionRegistry → chat, presence, online status, GM targeting.** `/who`, local chat
  membership, the duplicate-login guard and notice routing all go through it. It must
  exist before chat can have more than one participant. **Prerequisite for the chat lane
  and for any multiplayer work at all.**
- **`applyCharacterToSession` → chat, space, inventory, wallet, everything
  character-scoped.** It is the only writer of character identity onto a session, and
  `sendSessionChange` is how the client learns. **Prerequisite for every other lane**:
  until a session can hold a character that is not character #1, no lane can be tested
  with two players.
- **persistence → wallet, items, skills, mail, calendar, bookmarks, achievements.**
  Everything with a runtime table. `tableOwnership.js` lists which. **Prerequisite for
  the economy and skills lanes**, which cannot claim a feature works until it survives a
  restart.
- **objectCache → market, dogma, SDE-ish bulk responses.** `packetDispatcher.js:46-48`
  can answer a cached method call with a `CacheOK` error instead of the payload; a lane
  returning large static rowsets wants this before it wants speed work.
- **contentPacks → dungeons, exploration, homefront, mining missions.** The pack
  capability allowlist (`contentPackManager.js:26-80`) is the contract. Driven by
  `/spawnsite`.
- **chat → GM slash console.** `/`-commands arrive over `LSC.SendMessage` as well as
  `slash.SlashCmd` (`lscService.js:92`), and *every* command's reply goes back through
  `chatHub.sendSystemMessage`. **So chat output is a prerequisite for the GM console
  being usable as a test harness** — a command whose answer nobody can read is not a
  test. On our side `slash.SlashCmd`'s return string is already the channel
  (`gm.rs:83-89`), which is enough to start, but `/who` and `/announce` need real chat.
- **GM slash console → every other lane.** This is the inverted edge and the reason
  question (3) matters: once the console exists, every other lane gets its test harness
  for free. **Flag: build the console breadth early, not last.**
- **XMPP depends on chat but nothing depends on XMPP.** `chatHub.js:24-34` establishes
  that the V24 client has no `OnLSC` consumer, so real delivery is XMPP MUC or the
  protobuf gateway. That is a large, separable chunk and it blocks nothing else.

## Effort to port, per subsystem

| subsystem | effort | why | risk |
|---|---|---|---|
| Session identity split (3 types, 3 keys, one `apply_character`) | **L** | 25 `characters.first()` sites plus 3 hardcoded `user_id` plus a registry plus the `(old,new)` session-change plumbing. Their `applyCharacterToSession` is 430 lines and most of it is the field table | **High.** Touches every service. Their own code documents re-selection hazards (`characterState.js:4303-4310`) and the closing-session race (`charService.js:1588-1592`). Both are bugs we will reproduce if we skip them |
| Session registry + presence index | **S** | 239 lines, one `Set` and one map; the O(N) fallback means the index is an optimisation we can omit at first | Low |
| Duplicate login (per account + per character) | **S** | 87 + 35 lines (`accountLoginClose.js`, `charService.js:1586-1620`) | Medium — the `GPSTransportClosed`/`CLIENTDISCONNECTED` close is a protocol detail they admit is unverified (`accountLoginClose.js:11-14`), so client behaviour is a guess |
| Session-change allowlist + `sn` sequence | **S** | a 34-name const and a counter | Low. Pure win |
| Post-response push hook (`afterCallResponse`) | **S** | one hook in the dispatcher plus a per-session pending-effects slot | Medium — ordering bugs here are client hangs, not errors |
| GM console breadth (to ~60 useful commands) | **M** | the dispatcher exists (`gm.rs`); each command is 10-40 lines of parse + call. The 162-arm `if` chain is tedious but mechanical, and **each command is only as portable as the subsystem it drives** | Low in itself. The risk is scheduling: a command for a subsystem we have not built is dead weight |
| GM console full parity (176) | **XL** | most of the 176 drive space, NPC, abyssal, sovereignty and structure subsystems that do not exist on our side | n/a — not a goal |
| Persistence: debounce + per-table files + intent record | **M** | `store.rs` already has the trait, the format and tmp+fsync+rename. This is partitioning and a timer | Medium — splitting one atomic rename into N breaks cross-table atomicity unless (4) above is done at the same time |
| Persistence: single-instance lockfile | **S** | `O_EXCL` + PID, std-only | Low |
| Table-ownership assertion | **S** | a const map plus one assert in the write path; `tableOwnership.js` is the data | Low. Catches a real bug class |
| Mail | **M** | 31 handlers, but `mailMgrService.js` is only 584 lines over a 2 251-line state module; the shapes are mechanical | Low — `social.rs:84-116` already stubs the read side |
| Mailing lists | **M** | 19 handlers, 317 lines | Low |
| Notifications | **M** | 10 handlers; the real work is the notification-type templates (`notificationYaml.js`, 378 lines) | Medium — getting a type's payload shape wrong is a client-side exception |
| Calendar | **M** | 13 handlers, and `/calauto` gives a ready-made driver | Low |
| Bookmarks | **L** | 4 RPC handlers over 1 548 lines of runtime state; the client surface is small but the semantics (folders, access groups, coordinate vs object bookmarks) are not | Medium — most of their logic is reachable only via `/bookmarkauto`, so the RPC surface is under-specified by their own code |
| Object cache | **M** | 8 handlers + the `CacheOK` short-circuit; version/compression policy is 3 constants | Medium — a wrong cache key means the client serves stale data forever and we will blame the wrong subsystem |
| `machoNet` service dict | **S** | a const table; we already answer `GetInitVals` | Low, but **it gates everything**, so get the access levels right |
| Chat channels (LSC shapes) | **M** | `chatHub.js` is 370 lines and the rowset headers are listed at `:36-60` | Low — and `chatHub.js:24-34` says the client never calls LSC, so this is parity-only |
| Chat delivery (XMPP MUC + TLS) | **XL** | 3 387 lines of stub server plus a TLS listener plus roster reconciliation with batching limits (`xmppStubServer.js:63-70`). A second protocol, a second port, and TLS with no crates | **High.** TLS with zero external crates is a project in itself. Needs a decision before any estimate is credible |
| Content packs | **L** | 1 485 lines, but the valuable part (schema version, capability allowlist, size cap, fingerprint skip) is maybe 300 | Low — genuinely good design, and data-only packs are a natural fit for std-only Rust |
| Edge processes | **n/a** | do not port; see below | n/a |
| Marshal worker pool | **n/a** | do not port; see below | n/a |
| Overview presets / shared settings | **S** each | 2 handlers each, store-a-link-get-it-back | Low |
| Achievements / daily goals | **L** each | both delivered over the protobuf gateway, not macho RPC — a different transport we do not have | High — unclear these are reachable at all on our path |
| NPE / AIR tutorial | **L** | 2 042 lines across 14 runtimes, hooked into selection | Medium — their own hook is in a `try/catch` that only warns, so it is not load-bearing for login |

## Architecture we should NOT copy

**Node-specific accidents — skip entirely:**

- **The marshal worker pool** (`network/marshalWorkerPool.js`, 518 LOC + a worker +
  two task modules). Its entire purpose is that Node has one event loop and a 256 KiB
  marshal blocks it (`:143-147` sets the offload threshold). We are thread-per-connection:
  each connection already marshals on its own thread. Copying this would add a thread
  pool to solve a problem we do not have.
- **The persistence worker** (`gameStore/persistenceWorker.js`, 2 127 LOC). The header
  says it outright: *"so the synchronous better-sqlite3 transaction does not stall the
  simulation loop"* (`:1-12`). A synchronous `write` + `fsync` on the owning thread is
  the correct answer for us, and `dispatch.rs:1157-1163` already reasons about the cost
  and accepts it.
- **The five edge processes** (`edge/`, 4 229 LOC). Separate `worker_threads` for game,
  chat, image, redshift and an HTTP gateway, each with its own ordered request protocol,
  admission control, restart policy and authority bridge. The reason they exist is Node:
  a TLS XMPP listener, an image resizer and a gRPC gateway cannot share one event loop
  with a simulation tick. In std-only Rust each of those is a thread, and
  `edge/edgeProcessHost.js:26-46`'s long essay on admission control is a problem we only
  get if we introduce the queue first.
- **`worker_threads` generally** — also `services/account/walletAuthorityProcess.worker.js`.
  Same analysis.
- **The epoch/instance persistence lease.** It solves multi-process contention over one
  database file. One process, one `RwLock`.
- **JSON blobs in SQL** (`"<table>"(key TEXT, json TEXT)`). This buys WAL durability at
  the price of being unqueryable without `json_extract` — and their own README reaches for
  it in the one example query. Our text file is strictly better at the thing they said
  they valued (`gameStore/README.md`, "Inspecting / querying").
- **Dual field spellings on the session** (`characterID` *and* `charid`,
  `solarsystemid` *and* `solarsystemid2`, 60-odd fields at
  `characterState.js:4399-4504`). This is an artefact of duck-typed JS accreting two
  naming conventions. Rust should have one field and one serialisation name, and
  `sessionIdentity.js:18` having to try four spellings of the character id is the
  cautionary tale.
- **`_`-prefixed scratch state on the session object.** `_space`, `_safeLogoff`,
  `_pendingLocalChannelSync`, `_abyssalOfflineSession` and friends, with
  `sessionRegistry.unregister` (`:135-153`) hardcoding eleven teardown calls. In Rust
  this is a per-subsystem map keyed by connection id, or a teardown trait — not fields
  bolted onto a shared struct.
- **Filename-convention service autoloading** (`bootstrap.js:215-247` registering every
  `*Service.js`). Rust has no equivalent and needs none; our explicit `SERVICES` consts
  (`gm.rs:16`, `social.rs:16`) are better because a missing registration is a compile
  error.

**Real constraints we will also hit, whatever the language:**

- **The service directory gates everything** (`machoNetService.js:175-321`). The client
  will not route a call to a service it was not told about. This is the protocol, not
  Node.
- **Session change is an allowlisted `(old, new)` dict with a fixed 14-element envelope**
  and two *required* trailing nulls (`clientSession.js:775-806`). Protocol.
- **Ordering around a session change is load-bearing**, in both directions: flush the old
  location's pending sends *before* the change (`:730`), and defer some effects until
  *after* the CallRsp (`commandSessionEffects.js`). We will hit this exact problem;
  `dispatch.rs:556-577` only solves the first half.
- **One controller per character, and a closing connection still counts as the
  controller** (`charService.js:1588-1592`). A real distributed-systems constraint that
  arrives the moment two clients exist. Our bots will hit it too, and
  `characterControlRuntime.js` already has the right shape for it.
- **Write-behind coalescing is necessary.** Saving on every mutation puts an fsync on the
  draw path. Their answer is a 2 s per-table debounce; ours is `take_dirty()`, which is
  the same idea without the timer. Both are responses to the same physics.
- **Per-entity write granularity matters once the world is big.** Their `ROW_GROUPS`
  split exists because one notification box changing was rewriting every box. A
  whole-world snapshot has the same problem at scale, and partitioning is the fix in any
  language.
- **Content must be data, not code** (`contentPacks/`, schema-versioned, capability-gated,
  size-capped). Nothing Node about it, and it is the single best idea in this lane.
- **Presence and identity must be derivable without fabricating a session**
  (`sessionRegistry.js:3-6`). Directly on the path to player-like bots.

## Notes for the coordinator

**Surprises:**

1. **`sessionMgr` is a stub and `tcpRawProxyService` is a stub.** If a plan says "port
   their session manager", there is nothing there — 88 lines of audit logging. The real
   session model is `network/clientSession.js` + `services/chat/sessionRegistry.js` +
   `characterState.applyCharacterToSession`, and the third of those lives in another
   lane's tree.
2. **`services/_shared/sessionIdentity.js` is a 39-line written post-mortem of the exact
   bug eve-offline has.** Its header (`:6-15`) names the `|| 140000001` fallback pattern,
   explains that it conflates account id with character id, and states the rule: return
   0 and let the caller fail. Read it before writing the resolver.
3. **Local chat does not use LSC.** `chatHub.js:24-34` says the V24 client has no live
   `OnLSC` consumer — no `def OnLSC`, no service registration, no `__notifyevents__`
   entry, and zero OnLSC traffic in their golden login capture. LSC is advertised in
   `GetInitVals` for parity only. Any plan that ports "chat" by implementing `OnLSC`
   pushes is porting a dead path. The live paths are XMPP MUC (port 5222, TLS, a
   separate process) and a protobuf gateway.
4. **Achievements and daily goals are protobuf-gateway features**, not macho RPC
   (`achievementGatewayProto.js` 936 LOC, `dailyGoalsProto.js` 486). They are not
   reachable over the path eve-offline implements. Deprioritise.
5. **This tree is already patched by us.** `network/tcp/handshake.js:838-847` carries a
   comment dated **2026-10-20** adding `access_token`/`computer_hash` for build 3619879
   and citing `eve-offline docs/reference/login-path.md section H10`. So `$EVEJS_RUN`
   is not pristine upstream. Anyone diffing against a release tarball should know.
6. **`characterSlots` is hardcoded to 3** (`charService.js:878-882`). Their multi-character
   support is real but the slot count is not configurable.
7. **The GM console is almost entirely ungated.** One role bit (`ROLEMASK_VIEW`) gates
   the help text and four commands; the other ~172 are open to any session. For a test
   harness that is the right call, and it means we do not need a role system first.
8. **`/bookmarkauto` and `/calauto` are the only drivers for most of bookmarks and
   calendar.** 1 548 lines of bookmark runtime behind 4 RPC handlers. Their own RPC
   surface under-specifies the feature, so porting bookmarks means reading
   `bookmarkRuntimeState.js`, not the service.

**Unknowns I could not settle read-only:**

- Whether the 2 s flush debounce has ever lost data in practice, and whether the outbox
  recovery path has been exercised. `acquirePersistenceOwnerLease({ recover: true })` is
  called from exactly two places, both migration tools (`importPlayers.js:198`,
  `migrateJsonToSqlite.js:195`) — **the normal boot path at `index.js:728` calls
  `acquirePersistenceOwner()` without recovery.** Flagged as a guess, but it reads like
  crash recovery is opt-in and the server does not opt in.
- Whether the browser-pilot lease (`characterControlRuntime.js`) actually works. The
  expiry path has no caller I could trace, and there are no tests.
- `edge/image` and `edge/redshift` purposes. I read their runtimes but not enough to say
  what they serve; neither looks load-bearing for a client session.
- Whether `chatAuthenticationService` returning `null` (`:104`) is sufficient for the
  client, or whether the XMPP path needs a real token. Their own log message calls it a
  `"local-fallback"` (`:101`), which suggests they know it is a stub.

**What to look at next, in order:**

1. `services/_shared/sessionIdentity.js` (39 lines) and `characterState.js:4287-4598`
   (`applyCharacterToSession`). Together they are the specification for our identity
   split, and the second one is the field table we will otherwise have to invent.
2. `network/clientSession.js:58-91` and `:727-844`. The allowlist and the exact session
   change envelope — directly comparable to our `dispatch.rs:556-577`.
3. `services/online/characterControlRuntime.js:1-60`. The controller-lease model, which
   is the shape the bot end goal needs and which our current one-session design cannot
   express.
4. `gameStore/tableOwnership.js`. The complete runtime-table inventory with an owner per
   table. It is the only place the whole persisted world is enumerated, and it is the
   input to partitioning `store.rs`.
5. `services/machoNet/machoNetService.js:175-321`. The service directory. Diff it against
   whatever eve-offline advertises; every gap is a service the client will never call.
