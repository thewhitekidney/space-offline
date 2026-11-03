# Login path — exact call sequence (client V24.01, reference build 3662168)

> Re-pinned 2026-10-31 (issue #19): the reference is 3662168; 3619879 and 3628513 share
> every constant below (same line V24.01, `eveMachoNetVersion` and `GPS` identical past
> the header). The sequence was read on 3619879 and `GPS`, `machoNet` and
> `eveMachoNetVersion` did not change. The 2026-11-09 build will repeat the check.

What the client sends, in what order, and the shape of every value it reads
back, from TCP connect to the character-select screen. Everything below was
read out of the decompiled `.pyc` of **this** install; each row names the module
it came from. Items that are **native** (in `blue.dll` / `carbonio`) or
server-only are marked **GAP** and not guessed.

Companion docs: `docs/archive/wire-format.md` (envelopes, command ids, compression),
`docs/archive/session-flow.md` (high-level sketch — **two of its claims are wrong, see
§9**).

---

## 0. Build constants the server must match

| name | value | source |
|---|---|---|
| `machoVersion` / `macho.version` | **496** | `eve/common/script/net/eveMachoNetVersion.pyc` (`machoVersion = 496`); re-exported as `version` by `carbon/common/script/net/machobase.pyc` |
| `boot.version` | **24.01** (float) | `/client/start.ini` `[main] version` |
| `boot.build` | **3662168** (the pinned build; 3619879 and 3628513 are `unverified` in `tools/builds.tsv`, so the server refuses them unless `--client-build` names them) | `start.ini` `build` |
| `boot.codename` | **`V24.01`** | `start.ini` `codename` |
| `boot.region` | **`ccp`** | `start.ini` `region` |
| combined codename field | **`V24.01@ccp`** | `GPS.py` builds `str(boot.codename) + '@' + str(boot.region)` |
| default port | **26000** | `start.ini` `port` |
| default server name | `Tranquility` → `tranquility.servers.eveonline.com` | `start.ini` `server` + `carbon/common/lib/serverInfo.pyc` |
| `GPSTransport.__HANDSHAKE_SECRET_KEY` | **170472** | `carbon/common/script/net/GPS.pyc` |
| `handShakePaddingLength` | **64** | `GPS.pyc`: `boot.GetValue('handShakePaddingLength', 64)`; key absent from `start.ini` |
| `publicKeyVersion` sent by client | literal string **`'placebo'`** | `GPS.pyc` uses `getattr(macho, 'publicKeyVersion', 'placebo')` and `machobase` defines no such name |
| `cryptoPack` | **`CryptoAPI`** | `start.ini` `cryptoPack` — see §1, this is the one hard blocker |
| `socketIO` | `iocp` | `start.ini`; so `iocp.UsingIOCP()` is true but `UsingSSL()` is false (needs `/ssl` arg or `ssl=1` pref), so the transport is `SecureSocketPacketTransportFactory`, not SSL (`machoNet.get_gps_map`, `carbon/common/lib/iocp.pyc`) |
| `const.session.SESSION_TYPE_GAME` | **5** | `carbon/common/lib/session.pyc` |
| `const.responseUnknown` | `'Unknown'` | `carbon/common/lib/const.pyc` |

### Useful command-line overrides (all confirmed)

| arg | effect | source |
|---|---|---|
| `/server:<host>` | overrides `boot`/`prefs` server name | `utillib/__init__.pyc GetServerName` |
| `/port:<n>` | overrides port | `utillib GetServerPort` |
| `/login:<user>:<password>` | enables `connection.TryAutomaticLogin()` | `utillib GetLoginCredentials`, `connectionService.TryAutomaticLogin` |
| `/ssoToken=<jwt>` | skips the login form entirely, see §3 | `gameui.Startup`, `loginII._Connect` |
| `/autoSelectCharacter:<slotOrCharID>` | auto-confirms a character at charsel | `charSelection/characterSelection.pyc _CheckAutoSelectCharacter` |
| `/character=<id>` (startup arg value) | `blue.os.GetStartupArgValue('character')` → immediate `ConfirmWithCharID` | `characterSelection.OnOpenView` |

---

## 1. Crypto: the one real blocker (and its fix)

`evecrypto/settings.pyc`: `cryptoPack = boot.GetValue('cryptoPack', 'Placebo')`.
This install's `start.ini` says **`CryptoAPI`**, so `evecrypto.crypto` binds to
`evecrypto/cryptoapi.pyc`:

- `CryptoHash(*args) = sha256(blue.marshal.Save(args)).digest()`
- `GetRandomBytes(n) = blue.crypto.GenerateRandomBytes(n)`
- `Verify(signedData)` → `asymmetricKey.VerifySignature(sha256(...), sig)` where
  `asymmetricKey = blue.crypto.GetSharedAsymmetricCipher()`
- `CryptoApiCryptoContext.Initialize()` generates a random 32-byte AES key +
  16-byte IV, encrypts **both with CCP's public key**, and returns
  `{'crypting_sessionkey': <blob>, 'crypting_sessioniv': <blob>}`.

Two consequences, and they are fatal for a third-party server as shipped:

1. We cannot recover the AES session key from `crypting_sessionkey` without
   CCP's private key, and **every MachoPacket after the handshake is encrypted
   with it**: `SocketGPS.pyc` sets, for the non-SSL packet transport,
   `Read = SocketPacketTransport.EncryptedRead` /
   `Write = SocketPacketTransport.EncryptedWrite`, with
   `UnEncryptedRead/Write` kept as separate aliases used only during the
   handshake.
2. The handshake requires the server to hand the client a **signed marshalled
   code object** (`Crypto.Sign(marshal.dumps(handShake.GetHandshakeFunc()))`),
   which the client runs through `Crypto.Verify` and `eval`s
   (`GPS.__Execute`). Under CryptoAPI that signature must validate against
   CCP's public key, or `__Execute` raises
   `RuntimeError('Failed to verify initial function blobs')`.

**Fix: set `cryptoPack = Placebo` in `/client/start.ini`.** `evecrypto/placebo.pyc`
then makes the whole thing trivially reproducible and plaintext:

- `create_context()` → `PlaceboCryptoContext`, whose `Initialize(*a, **kw)`
  returns `{}` and whose `SymmetricEncryption`/`SymmetricDecryption` are the
  identity. **So the post-handshake stream is plaintext.**
- `crypto_hash(*args) = str(binascii.crc_hqx(blue.marshal.Save(args), 0))`
  — a *decimal string* of a CRC-16/XMODEM over the marshalled argument tuple.
- `get_random_bytes(n) = '\x00' * n` — so `clientChallenge` and
  `serverChallenge` are 64 NUL bytes.
- `sign(data) = (data, 0)`; `verify(signedData) = (signedData[0], signedData[1] == 0)`
  — i.e. the server signs by sending `(marshal.dumps(code), 0)`.
- `asymmetricKey = None`.

`cryptoPack` is read only by `evecrypto/settings.py` and is **not** part of the
version handshake, so changing it does not trip any check (§4). Note that
`cryptoapi.asymmetricKey = blue.crypto.GetSharedAsymmetricCipher()` still runs
at import time (`crypto.py` imports both modules unconditionally) but is unused.

**Secondary lead (unverified):** `evecrypto/publickey.pyc` `get_keystr()`
returns a CryptoAPI `RSA1` PUBLICKEYBLOB with **bitlen 0x200 = 512 bits**,
exponent 0x00010001, 64-byte modulus
`bbe8146a10c5008bd9ea2876d581cedeb9c2d6032f468e2f58a8da4a6be1b152d9c8ed411cbceaeb89370eaea054650e51c93d9c2ef0dc2527418d6ba5f6e3e1`.
A 512-bit modulus is factorable with commodity tooling, which would let a server
speak CryptoAPI for real. **GAP:** nothing in the Python tree references
`get_keystr()`, so whether `blue.crypto.GetSharedAsymmetricCipher()` actually
uses this blob is unconfirmed — it may be dead or superseded by a key compiled
into `blue.dll`. The `Placebo` switch is strictly cheaper; treat factoring as a
later nicety.

---

## 2. Framing — still a GAP

`SocketGPS.pyc` `SocketPacketTransport.Write`/`Read` call
`self.socket.sendpacket(packet[, header])` and `self.socket.recvpacketoob()`.
Those are native. The on-wire length prefix width/endianness remains a **GAP**
exactly as `docs/archive/wire-format.md` says; this document adds nothing to it.

---

## 3. Getting the client to dial out

### SSO path (recommended for M1)

`eve/client/script/parklife/gameui.pyc Startup`:

```
token = None
for arg in blue.pyos.GetArg()[1:]:
    if arg.startswith('/ssoToken'):
        try:    argName, token = arg.split('=')
        except: raise RuntimeError('Invalid format of SSO token, should be /ssoToken=<token>')
if token is not None:
    self.usingSingleSignOn = True
    self.DoLogin(token)
else:
    self.viewState.ActivateView('login')
```

- The split is a bare `arg.split('=')` into exactly two names, so a token
  containing an `=` raises. Standard base64url JWTs have no `=`, so this is fine.
- `gameui.UsingSingleSignOn()` → `getattr(self, 'usingSingleSignOn', False)`.
- `gameui.DoLogin(token)`: `sm.ScatterEvent('OnClientReady', 'login')` then
  `sm.GetService('connection').LoginSso(token)`; on **any** exception it calls
  `loading.CleanUp()`, `viewState.ActivateView('login')` and re-raises. That is
  the whole "auth rejection" behaviour at this layer — back to the login screen.
- `loginII._Connect` has the same `/ssoToken` loop but **does not** pre-initialise
  `token`, so clicking Connect in SSO mode with no `/ssoToken` arg raises
  `NameError` rather than a clean error. Only reachable if `usingSingleSignOn`
  were set without an arg, which cannot happen; noted for completeness.
- `loginII._Connect` in SSO mode calls `gameui.DoLogin(token)` and returns
  immediately — it skips the server-status probe and the build comparison.

`connectionService.LoginSso(token)`:

```
if token is None: raise RuntimeError('Trying to do an SSO log in without an SSO token')
if is_jwt(token): self.loginJWT = token            # is_jwt == token.count('.') == 2
response = self.ConnectSso(serverInfo.GetServerIP(GetServerName()), GetServerPort(), token, 'tcp')
self.accessToken  = response['access_token']       # KeyError if absent
self.computerHash = response['computer_hash']      # KeyError if absent
```

`eve/common/lib/jwt/__init__.pyc` also ships
`fabricate_local_dev_user_token(tenant, user_id)` → `encode({'sub': 'USER:EVE:<user_id>',
'tenant': tenant, 'tier': 'dev', 'region': 'local'}, 'dev')`, i.e. **HS256 with
the key `'dev'`**. Handy for generating a token our server can parse.

`connectionService.ConnectSso` calls
`machoNet.ConnectToServer(address, 'EVE-SSO-CONNECTION', 'fakePW', ct, token)` —
so on the wire `user_name` is the literal **`EVE-SSO-CONNECTION`**, the password
is **`fakePW`** (hashed as usual), and the token rides in `user_sso_token`.

### Username/password path

`loginII._Connect` → `connection.Login([user, password, serverIP, serverPort, 0])`
→ `connection.Connect(host, port, userid, password, ct)` →
`machoNet.ConnectToServer('%s:%d' % (host, port), userid, password, ct)`
(no token). `loginparam[4]` truthy would select `'udp'`; the UI passes `0`.

`connectionService.TryAutomaticLogin()` fires only if `/login:` was given and
`session.userid is None`, and is only invoked from
`loginII.__SetServerStatusText` when the resolved status label is exactly
`/Carbon/MachoNet/ServerStatus/OK` — i.e. the status probe (§5) must succeed
first.

On error anywhere in `Connect`/`ConnectSso`, `GPSException` is converted to
`UserError('LoginConnectFailed', {'what': what})`; `loginII` catches `UserError`,
shows the FSD dialog named by `e.msg`, and calls `CancelLogin()`
(`loading.CleanUp()`). `machoNet.ConnectToServer` additionally
`sm.ScatterEvent('OnConnectionRefused')` on every failure branch.

---

## 4. The low-level version check + authentication handshake

All of this is **Python, not native** — `carbon/common/script/net/GPS.pyc`.
Server side is `GPSTransport.HandleClientAuthentication` (+ `...Step1/2/3`),
client side is `GPSTransport.Authenticate`. Each line is one
`macho.Dumps`/`Loads` payload (`blue.marshal`) over one `sendpacket`.
"E" = sent through `EncryptedWrite`/`EncryptedRead` (identity under Placebo).

| # | dir | enc | payload | client-side checks on receipt |
|---|---|---|---|---|
| H1 | S→C | – | 7-tuple `(170472, 496, loggedOnUserCount:int, 24.01:float, 3662168, 'V24.01@ccp', updateInfo)` | see below |
| H2 | C→S | – | 6-tuple `(170472, 496, 0, 24.01, 3662168, 'V24.01@ccp')` | — |
| H3 | C→S | – | `(None, 'VK', CryptoHash(CaseFold(user_name)))` — **only if `user_name` is truthy** | — |
| H3' | C→S | – | `(None, 'QC')` — **only if `user_name is None`** (status probe); then reads one value = logon queue position and returns | — |
| H4 | C→S | – | `('placebo', cryptoContext.Initialize())`; Placebo → `('placebo', {})`; CryptoAPI → `('placebo', {'crypting_sessionkey':…, 'crypting_sessioniv':…})` | server compares element 0 against its `publicKeyVersion`/`'placebo'` |
| H5 | S→C | – | the string `'OK CC'` | loaded and discarded |
| H6 | C→S | **E** | `(clientChallenge:str len 64, requestDict)` — see dict below | — |
| H7 | S→C | **E** | `passwordVersion:int` (server hardcodes **2**) | if `== 1`, client resends H6 with `user_password` set and `user_password_hash=None` |
| H8 | S→C | **E** | 4-tuple `(serverChallenge:str len 64, signedFunc, {}, responseDict)` | see below |
| H9 | C→S | **E** | `(CryptoHash(serverChallenge), funcOutputBuffer:str, funcResult:dict)` | — |
| H10 | S→C | **E** | `authenticationResult:dict` (must be truthy) | see below |

### H1 — what makes "Low-level version check failure"

Client (`GPS.Authenticate`) builds
`insecresp = {'macho_version': r[1], 'cluster_usercount': r[2], 'boot_version': r[3],
'boot_build': r[4], 'update_info': r[6]}` and splits `r[5]` on `'@'` into
`boot_codename`/`boot_region` (no `'@'` → region `''`). It then aborts, in this
order, raising `GPSTransportClosed(label, reasonCode, machoVersion=r[1],
version=r[3], build=r[4], codename=…, region=…, loggedOnUserCount=r[2])`:

| condition | reasonCode |
|---|---|
| `str(boot.region) != insecresp['boot_region']` | `HANDSHAKE_INCOMPATIBLEREGION` |
| `str(boot.codename) != insecresp['boot_codename']` | `HANDSHAKE_INCOMPATIBLERELEASE` |
| `boot.version != r[3]` | `HANDSHAKE_INCOMPATIBLEVERSION` |
| `macho.version != r[1]` | `HANDSHAKE_INCOMPATIBLEPROTOCOL` |
| `boot.build < r[4]` | `HANDSHAKE_INCOMPATIBLEBUILD` |

So: region and codename are **string-equal**, `boot.version` is **float-equal**
(24.01), `machoVersion` is **int-equal** (496), and build is only rejected when
the *server* build is **newer** than the client's — equal or lower is accepted.
`r[0]` (the secret key) and `r[2]` are not validated by the client; `r[6]` is
stored as `update_info` only.

The symmetric server-side checks in `HandleClientAuthenticationStep1` are type-
strict and are what produce the `'Low-level version check failure while
accepting from …'` log line in `machoNet.HandleAccept` when
`HandleClientAuthentication` returns a non-`'OK'` string. Expected types: `r[0]`
`int`, `r[1]` `int`, `r[2]` `int`, `r[3]` **`float`**, `r[4]` `int|unicode|str`,
`r[5]` `str|unicode`. Reproduce the client's H2 element types exactly
(`24.01` must marshal as a real, not a string).

### H6 — `requestDict` the client sends

From `GPS.Authenticate`:

```
{'macho_version': 496,
 'boot_version': 24.01,
 'boot_build': 3662168,
 'boot_codename': 'V24.01',
 'boot_region': 'ccp',
 'user_name': <username or 'EVE-SSO-CONNECTION'>,
 'user_password': None,
 'user_password_hash': machobase.PasswordHash(username, password),
 'user_languageid': prefs.GetValue('languageID', 'EN'),
 'user_affiliateid': 0,
 'user_sso_token': <token or None>}
```

`machobase.PasswordHash(userName, password)`: utf-16-le (BOM stripped) of the
stripped username, lowercased, as salt; `sha1(utf16le(password) + salt)` then
**1000 further iterations** of `sha1(digest + salt)`; returns the raw 20-byte
digest. (A server that accepts anything need not compute it.)

The server's `Step3` re-checks `boot_region`, `boot_codename`, `boot_version`,
`macho_version` for equality and `boot.build > request['boot_build']` for
staleness, and requires `len(clientChallenge) == handShakePaddingLength` (64).

### First real client: the crypto hash must use blue.marshal's byte encoding **[V]**

The first real client connection (2026-10-19) got through H1-H8 and rejected
H8 with `GPSTransportClosed`: *"SLServer response signature incorrect, the
server's crypto hash wasn't correct."* The capture showed why, and it is a
byte-exactness trap worth stating loudly:

`challenge_responsehash` is `str(crc_hqx(blue.marshal.Save((clientChallenge,)),
0))` — a CRC over the **raw marshalled bytes**. The client marshals every
string with tag **`0x13`** (seen in its own H2/H4/H6 frames: `13 02 "VK"`,
`13 07 "placebo"`, `13 40 <64-byte challenge>`). Our general encoder used
`0x10` (STRING_SHORT), which decodes identically but CRCs differently — so our
hash (`654`) did not match the client's and the handshake was refused.

Under Placebo the client challenge is **64 zero bytes**
(`get_random_bytes` returns `\x00`*n), so the expected value is deterministic:
the client CRCs `7E 00 00 00 00  25  13 40  <64 zeros>` — stream header, 1-tuple
tag `0x25`, string tag `0x13`, length `0x40`, then the zeros. `eve-server` now
has a `blue_marshal` encoder used only for the crypto hash that reproduces this
exactly (strings `0x13`; tuples `0x24`/`0x25`/`0x2C`/`0x14`), separate from the
general codec, which may keep using `0x10` since the client only ever *decodes*
our payloads.

### H8 — `responseDict` and `signedFunc`

```
{'challenge_responsehash': CryptoHash(clientChallenge),
 'macho_version': 496,
 'boot_version': 24.01,
 'boot_build': 3662168,
 'boot_codename': 'V24.01',
 'boot_region': 'ccp',
 'cluster_usercount': <int>,
 'proxy_nodeid': <int>,
 'user_logonqueueposition': <int>,
 'config_vals': <dict>}
```

Client checks, in order:

1. `response['challenge_responsehash'] != Crypto.CryptoHash(clientChallenge)`
   → closes with "Server response signature incorrect…". Under Placebo this is
   `str(crc_hqx(marshal.Save((clientChallenge,)), 0))` — note `CryptoHash` is
   varargs, so it hashes the **1-tuple** `(clientChallenge,)`.
2. `response.get('user_logonqueueposition', 0) >= 2` → `SetLogonQueuePosition`
   and `raise UserError('AutLogonFailureTotalUsers', {'position': …})`. Send
   **1** (or omit) to proceed.
3. `signedFunc` goes through `Crypto.Verify` → `marshal.loads` →
   `eval(func, globals(), context)` inside `GPS.__Execute`, with stdout/stderr
   captured into a `WriteBuffer`. Under Placebo: send
   `(marshal.dumps(<a py2.7 code object>), 0)`. Element 2 of H8 is the `eval`
   locals dict — the real server sends `{}`.
   **GAP:** `carbon/server/script/net/handShake.py` (`GetHandshakeFunc`,
   `Verify`) is server-only and not in the client tree, so the real expression's
   content is unknown. The client only needs *something* that marshals as a code
   object and evaluates; its `funcResult` is whatever the expression returns
   (the client does not inspect it) and `funcOutput` is the captured text. If
   the `eval` raises, `__Execute` swallows it, prints the traceback into the
   buffer and returns `funcResult = {}` — so **a deliberately failing
   expression is an acceptable stub**, as long as `Crypto.Verify` passes.

### H10 — `authenticationResult`, the keys the client actually reads

| key | type | read by |
|---|---|---|
| `session_init` | dict | `machoNet.ConnectToServer`: `transport.userID = response['session_init']['userid']` — **required**, `['userid']` must exist |
| `proxy_nodeid` | int | `machoNet.ConnectToServer`: `self.myProxyNodeID = response['proxy_nodeid']` — **required** (also present in H8; H10 is merged over H8 via `response.update(crAck)`) |
| `access_token` | str | `connectionService.LoginSso` — **required on the SSO path only**, plain `[…]` index |
| `computer_hash` | str | `connectionService.LoginSso` — same; also later `charsel._CanLogin()` → `multiLoginBlocker.Login(computerhash)` if non-`None` |

> **Both of those rows were confirmed the hard way on 2026-10-20, by an
> independent server that omits them.** EvEJS's `CryptoHandshakeAck` carries
> `live_updates`, `session_init`, `sessionID`, `client_hash` and
> `user_clientid` — and neither `access_token` nor `computer_hash`. Our client
> reached character selection against it and then died with
> **`KeyError: 'access_token'`** at `connectionService.py(175) LoginSso`, with
> `userName = 'EVE-SSO-CONNECTION'` and `ssoToken = u'offline'` in the frame:
> the SSO path, exactly as this table says. Their server targets build
> **3396210**, which evidently did not require it; ours is **3662168**, which
> does.
>
> This is the most useful kind of confirmation available for a required-key
> claim — a **negative control**. "The client reads this key" is normally
> argued from a `LOAD_ATTR`/index in the disassembly and cannot be falsified by
> a server that happens to send it. A server that omits it, and a client that
> dies on exactly the predicted key at exactly the predicted line, tests the
> claim from the other side. Two keys in this table moved from read-off-the-
> bytecode to read-off-the-bytecode-and-observed-failing-without-them.
| `config_vals` | dict | `machoNet.ConnectToServer`: `response.get('config_vals', {})` → `InitClientGlobalConfig` → `sm.ScatterEvent('OnGlobalConfigChanged', …)`. Optional; `{}` is fine |
| `live_updates` | list | optional; if present, each element's `.code` is scattered as `OnLiveClientUpdate` and the key is deleted. **Omit it.** |
| `client_hash` | any | set by the real server; **never read by client Python** |
| `user_name` | str | server-side logging only |
| `sessionID` | int | server-side only |
| `user_clientid` | int | server-side only |

Shape confirmed from `machoNet.Authenticate` (which assembles
`{'sessionID', 'user_clientid', 'session_init', 'access_token',
'computer_hash'}` from the `authentication` service) and
`GPS.HandleClientAuthentication` (which adds `live_updates`, `client_hash`).
The `authentication` service itself is server-only — **GAP**, but irrelevant:
we author `session_init` directly (§7).

Non-fatal server shortcut: `HandleClientAuthentication` returns early with
`'OK'` (no H8/H9/H10) when `not request['user_name']` or neither password field
is set or the queue position is > 1. Both real client paths always send
`user_password_hash`, so this branch is not taken.

---

## 5. Server-status probe (`GetServerStatus`) — only on the password path

`loginII.InternalUpdateServerStatus` → `machoNet.GetServerStatus(addr, forceQueueCheck)`
→ opens a transport and calls `rawTransport.Authenticate(None, None, None)`,
which runs H1, H2, H3' and then reads **one** value: the logon queue position.
It returns `(statusMessageOrCallable, insecresp)`. `loginII` then reads:

```
serverStatus.get('boot_version')   serverStatus.get('boot_build')
serverStatus.get('boot_codename', 'Unknown')   serverStatus.get('update_info', 'Unknown')
```

For the label to come out as `/Carbon/MachoNet/ServerStatus/OK` (which is what
gates `TryAutomaticLogin`), the position returned at H3' must be **≤ 1**
(`machoNet.GetOKMessage`: `position > 1` → InQueue; `position == 1 and
reportedQueued` → HeadOfQueue; otherwise OK). The server-side
`GetLogonQueuePosition` clamps with `max(1, position)`, so **send `1`**.
The transport is then cached in `clientHalfBakedTransports[address]` and reused
by the next `ConnectToServer` to the same address — so after a status probe the
real login continues on the *same* socket, resuming at H3/H4.

**The SSO path never calls this.** For M1 via `/ssoToken=`, the status probe can
be ignored entirely.

---

## 6. `ConnectToServer` — order of operations and progress events

`carbon/common/script/net/machoNet.pyc ConnectToServer(address, userName, password, sockettype='tcp', ssoToken=None)`:

| step | action | notes |
|---|---|---|
| 1 | reject if `self.authenticating` or already queueing on this address | `UserError('AlreadyConnecting')` |
| 2 | verify every `session.__persistvars__` equals its default | else `UserError('DirtySession')` |
| 3 | `ScatterEvent('OnProcessLoginProgress', 'loginprogress::connecting', '', 1, 3)` | **step 1 of 3** |
| 4 | reuse `clientHalfBakedTransports[address]` or `factory.Connect(address)` | TCP connect |
| 5 | `ScatterEvent('OnProcessLoginProgress', 'loginprogress::authenticating', '', 2, 3)` | **step 2 of 3** |
| 6 | `response = rawTransport.Authenticate(userName, password, ssoToken)` | §4 (H1…H10) |
| 7 | `objectCaching.LoadCache(rawTransport.address)` | **purely local** — `mkdir cache:/MachoNet/<ip>/496/{CachedMethodCalls,MethodCallCachingDetails,CachedObjects}` (`objectCaching.pyc`). No traffic. Note the path embeds `macho.version` = 496 |
| 8 | wrap in `MachoTransport`, start `TransportReader` tasklet | |
| 9 | `transport.userID = response['session_init']['userid']`; `myProxyNodeID = response['proxy_nodeid']` | |
| 10 | `transport._AssociateWithSession()` | client branch: `nodeID = 0`, `sess = <global session>`, installs `session.__dict__['clientID'] = 0`, creates `session.rwlock`. **`packet is None` ⇒ no attributes are set here.** The session is still empty at this point |
| 11 | `blue.net.EnumerateTransport(...)`, `blue.net.AddProxyNode(transportID, myProxyNodeID)` | native bookkeeping |
| 12 | **`RemoteServiceCall(session, myProxyNodeID, 'machoNet', 'GetServiceInfo')`** | first real service call — see §8 |
| 13 | `sm.ChainEvent('ProcessLoginProgressDone')` | only listener is `cfg.DataConfig` → `cfg.LoadCfgData()` (§8) |
| 14 | `ScatterEvent('OnProcessLoginProgress', 'loginprogress::done', '', 3, 3, response)` | **step 3 of 3** |
| 15 | `InitClientGlobalConfig(response.get('config_vals', {}))` | |

Back in `connectionService.Connect`/`ConnectSso`, after `ConnectToServer` returns:

| step | action |
|---|---|
| 16 | `SynchronizeClock()` — up to 5 × `sm.ProxySvc('machoNet').GetTime()` (§8) |
| 17 | `uthread.new(self.ClockSyncDaemon)` — every 1–3 min, 5 × `GetTime()` |
| 18 | `uthread2.start_tasklet(self.PingLoop)` — `sm.ProxySvc('pingService').Ping()` when idle (§8) |

`OnProcessLoginProgress` handlers: `connectionService.OnProcessLoginProgress`
(draws the progress window; rejects any `what` not in its label dict) and
`eve/client/script/sys/clientStatsSvc.pyc` (local stats only).

---

## 7. The session: `SessionInitialStateNotification` / `SessionChangeNotification`

The client's session is **not** populated by the handshake. After step 10 above,
`session.userid` is still `None`. The real proxy populates its own copy from
`llvresponse['session_init']` (`machoNet.HandleAccept`:
`llvresponse['session_init']['role'] |= s.role; s.SetAttributes(llvresponse['session_init'])`),
and the resulting session change is pushed to the client as a packet.

**The `|=` is measured now, not only read.** The `evejs-probe` lane drove a
real login against a third-party server with our own codec and watched the
role bits the proxy granted arrive broadened rather than replaced
(`docs/lanes/evejs-probe.md`). That does not make it CCP's proxy, but it does
move the claim from "this is what the bytecode says the proxy does" to "a
server doing this produces a session the client accepts" -- which is the half
we actually depend on.

`carbon/common/script/net/SessionChangeGPCS.pyc NotifyUp`, client branch:

| command | field | client call |
|---|---|---|
| 18 `SESSIONINITIALSTATENOTIFICATION` | `initialstate` = **flat** `{attr: value}` | `session.ApplyRemoteAttributeChanges(packet.initialstate, initialState=True)` |
| 16 `SESSIONCHANGENOTIFICATION` | `change` = `(flag, {attr: (old, new)})` | `session.ApplyRemoteAttributeChanges(packet.change[1], initialState=False)` |

Either works for login. **Prefer command 18 with a flat dict** — it is one
packet, needs no old values, and takes the `RecalculateDependantAttributes`
(non-pairs) branch. `sessionType` on the packet must be **5**
(`SESSION_TYPE_GAME`); `_AssociateWithSession` would otherwise use it to
`base.CreateSession`, and `CoreSession.__init__` raises `ValueError` for a type
outside `VALID_SESSION_TYPES = (1, 2, 4, 5)`.

Hard requirements inside `basesession.CoreSession.ApplyRemoteAttributeChanges`:

- `self.role & ROLE_SERVICE` ⇒ `RuntimeError`. **Never set ROLE_SERVICE.**
- `not self.role & (ROLE_LOGIN | ROLE_PLAYER)` ⇒ logged session error (not
  fatal, but it dumps the session). So include both.
- `'userid' in notify and both old and new truthy` ⇒ `RuntimeError("A session's
  userID may not change")`. Send `userid` exactly once, from `None`.
- Then: `DoSessionChanging` → `DisconnectFilteredObjects` → per-attribute
  `__ChangeAttribute` → `ValidateSession('post-change')` →
  `ProcessSessionChange` → `CallProcessChangeOnObjects` → **`OnSessionChanged`**.

### Session field list (authoritative; recovered by disassembly)

`uncompyle6` mangles these literal lists into integer indices — the names below
come from disassembling the `CoreSession` / `Session` class bodies of
`carbon/common/script/sys/basesession.pyc` directly.

`CoreSession.__persistvars__` (9) — these are the only ones a Carbon-level
session has:

`userid`, `languageID`, `role`, `charid`, `address`, `userType`,
`maxSessionTime`, `sessionType`, `countryCode`

`Session.__persistvars__` = the above **+ 25 EVE fields**:

`regionid`, `constellationid`, `allianceid`, `warfactionid`, `corpid`,
`fleetid`, `fleetrole`, `wingid`, `squadid`, `shipid`, `stationid`,
`structureid`, `solarsystemid`, `solarsystemid2`, `hqID`, `baseID`,
`rolesAtAll`, `rolesAtHQ`, `rolesAtBase`, `rolesAtOther`, `genderID`,
`bloodlineID`, `raceID`, `corpAccountKey`, `inDetention`

**`__persistvars__` is exactly the set that can appear in `initialstate`**:
`GetDistributedProps(getAll)` iterates `self.__persistvars__ +
self.additionalDistributedProps` and, for `getAll == 0`, emits only attributes
whose value differs from `GetDefaultValueOfAttribute`.
`additionalDistributedProps` for `Session` is `['locationid', 'corprole']`.

Non-distributed (never in `initialstate`): `CoreSession.__nonpersistvars__` =
`sid`, `c2ooid`, `connectedObjects`, `connectedServices`, `clientID`,
`localSID`, `contextOnly`, `receivedVersion`, `irrelevanceTime`; `Session` adds
`locationid`, `corprole`.

`GetDefaultValueOfAttribute`: `role` → `ROLE_LOGIN`; `sessionType` → current
value; anything in `__attributesWithDefaultValueOfZero__` → `0`
(`Session`: `corprole`, `rolesAtAll`, `rolesAtHQ`, `rolesAtBase`,
`rolesAtOther`); `languageID` on the client → `strx(prefs.GetValue('languageID', 'EN'))`;
everything else → `None`.

### Minimum `initialstate` to reach character select

| key | type | why |
|---|---|---|
| `userid` | int | gates everything; `gameui.OnSessionChanged` treats the change as relevant only if one of `userid, charid, solarsystemid, stationid, structureid, shipid` is present |
| `role` | long bitmask | must include `ROLE_LOGIN \| ROLE_PLAYER`; see below for the dev mask |
| `sessionType` | int `5` | |
| `languageID` | str, e.g. `'EN'` | compared against `prefs` by `gameui._CheckSessionLanguage`; a mismatch rewrites `session.__dict__['languageID']` locally (no server call at this point) |
| `address` | str | informational; `userType`, `maxSessionTime`, `countryCode` likewise |
| `charid` | **must be absent/None** | `gameui.OnSessionChanged` routes on `session.charid is None` → `_HandleUserLogonSessionChange` → character selection. Any non-`None` `charid` sends it to station/space instead |

### Role mask — what unlocks the dev/sandbox UI

`carbon/common/script/sys/serviceConst.pyc` (values are exact):

`ROLE_LOGIN = 4611686018427387904`, `ROLE_PLAYER = 2305843009213693952`,
`ROLE_SERVICE = 1152921504606846976` (**do not set**),
`ROLE_ADMIN = 72057594037927936`, `ROLE_PROGRAMMER = 2251799813685248`,
`ROLE_QA = 4503599627370496`, `ROLE_GML = 18014398509481984`,
`ROLE_GMH = 9007199254740992`, `ROLE_CONTENT = 36028797018963968`,
`ROLE_ROLEADMIN = 288230376151711744`, `ROLE_VIPLOGIN = 144115188075855872`,
`ROLE_NEWBIE = 576460752303423488`, `ROLE_SPAWN = 8589934592`,
`ROLE_WORLDMOD = 4096`, `ROLE_SECURITY = 1125899906842624`,
`ROLE_IGB = 2147483648`.

```
ROLE_ANY              = 0xFFFFFFFFFFFFFFFF & ~(ROLE_IGB | ROLE_EXT_GM1_PLUS | ROLE_EXT_GM2_PLUS | ROLE_EXT_GM3_PLUS)
ROLEMASK_ELEVATEDPLAYER = ROLE_ANY & ~(ROLE_LOGIN | ROLE_PLAYER | ROLE_NEWBIE | ROLE_VIPLOGIN | ROLE_NEWSREPORTER | ROLE_CSMDELEGATE)
ROLEMASK_VIEW           = ROLE_ADMIN | ROLE_CONTENT | ROLE_GML | ROLE_GMH | ROLE_QA | ROLE_EXT_GM1_PLUS
```

Confirmed gates on the login path:

- `characterSelection.OnOpenView`: `isElevated = session.role & ROLEMASK_ELEVATEDPLAYER`
  and the `'Insider'` setting ⇒ opens the **insider** dev window.
- `gameui.SetLanguage`: `session.role & ROLEMASK_ELEVATEDPLAYER` allows any
  language instead of the client-valid set.
- `ccSvc.CreateCharacterWithDoll`: `session.role & ROLE_QA` ⇒ QA starter-system
  picker, and calls `charUnboundMgr.GetQAStarterSystemIDs()`.

**Correction — do not use `ROLE_ANY & ~ROLE_SERVICE`.** This section
recommended it while listing the correct `ROLE_GML`/`ROLE_GMH` values three
lines above, which should have been the tell: `ROLE_ANY` **clears both GM
bits**, because `ROLE_EXT_GM4_PLUS = ROLE_EXT_GM4 | ROLE_GML | ROLE_GMH`
propagates into every `_PLUS` mask that `ROLE_ANY` excludes. Verified against
the constants in `serviceConst.py`:
`ROLE_ANY = 0xFF9C3FFF7FFFFFFF`, and `ROLE_ANY & (ROLE_GML | ROLE_GMH) == 0`.
It also sets bit 63, so it will not marshal as an `int64`.

Use `0x6168005200E41000` (`eve_server::roles::SANDBOX_SAFE`), which has both GM
bits and omits the two roles with a measured cost. Reasoning and the full role
table: `docs/reference/session-roles.md`; the correction is also written up in
`docs/archive/session-flow.md`.

`ROLE_VIPLOGIN` skipping the proxy's login-slot accounting is true of CCP's
proxy and irrelevant to a server we write — it has **zero** client-side
readers, its only two references being in the server half of `machoNet.py`.

Also worth knowing: `machoNetTransport.Read` sets `message.oob['sn']` itself for
NODE→BROADCAST packets, and `LockSession` calls
`WaitForSequenceNumber(packet.source, packet.oob.get('sn', 0))` for every
even-command packet. Send session notifications as
`source = MachoAddress(nodeID=<proxy_nodeid>)`,
`destination = MachoAddress(clientID=<client id>)` and leave `oob` empty/None
(`MachoPacket.__setstate__` restores `{}` from `None`), so `sn` is `0` and
`WaitForSequenceNumber` returns immediately. Do **not** set a `sessionVersion`
attribute on the packet, or `LockSession` will spin waiting for the session
version to catch up.

---

## 8. Service calls, in order

Client call mechanics (`carbon/common/script/net/ServiceCallGPCS.pyc`
`RemoteServiceCallWithoutTheStars`): `payload = (1, method, args, kw2)` where
`kw2` is the keyword dict with `machoVersion` forced to `1` if absent; wrapped
in `CallReq` (command 6) with
`destination = MachoAddress(nodeID=…, service=…)` when a node is known, else
`MachoAddress(service=…)` (ANY). `sm.RemoteSvc(n)` →
`session.ConnectToRemoteService(n)` (no node ⇒ ANY address);
`sm.ProxySvc(n)` → same with `nodeID = machoNet.myProxyNodeID`
(`carbon/common/script/sys/serviceManager.pyc`). The reply is `CallRsp`
(command 7) whose `payload` is the return value; if that value is an
`objectCaching.CachedMethodCallResult` the client caches it and unwraps
`.GetResult()`.

### Clicking a character: the first bound object **[V]**

Selecting a character stops being a service call and becomes the **bind
protocol**. `characterSelection.TryEnterGame` (line 692) does:

```python
sm.GetService('jumpQueue').PrepareQueueForCharID(charID)
populationCap = moniker.Moniker('populationCap', (charID, appConst.groupCharacter))
locations = populationCap.GetCharacterLoadSlot(charID)
if len(locations) > 1:
    selectedSolarSystemID = self.AskForAlternativeSystems(locations.keys())
    ...
    populationCap.MoveCharacterToNewSystem(charID, selectedSolarSystemID, slotKey)
...
self.EnterAsCharacter(charID, secondChoiceID, skipTutorial)
```

which is **two round trips**, and the capture shows the client hanging on the
first of them:

```text
[ 71] 12748.262ms OK   multiLoginBlocker Login("eve-offline")  -> true
[ 73] 12755.836ms HANG populationCap     MachoResolveObject((90000001, 1))  <- NO ANSWER
```

`(90000001, 1)` is `(charID, groupCharacter)` — the `bindParams`.

1. **Resolve.** `MachoResolveObject(bindParams)` returns a **node id**. `None`
   is an error the client reports as `'Resolution failure: ResolveObject
   returned None'`. We are one node, so the answer is our own id.
2. **Bind, with the first call fused in.** `MachoBindObject(bindParams, call)`
   where `call` is `(methodName, args, keywords)` — so the bind *and*
   `GetCharacterLoadSlot(charID)` are one message. The reply is the 2-tuple
   `(boundObject, retval)`; `Moniker.Bind` unpacks exactly two values.

`GetCharacterLoadSlot` returns `{solarSystemID: slotKey}`. It is only
`len()`-ed, and a length above one prompts the player to choose a system. `{}`
means "no alternatives, load where you are", which is both true here and the
answer that keeps the client out of `MoveCharacterToNewSystem`.

#### The bound object on the wire, and three ways to get it wrong

The reference travels as marshal tag **`0x19`** — the Load-callback form —
wrapping a byte string that is itself a complete marshal stream of
`(objectID, refID)`. The client passes `ObjectCall.ParseObjectID` as the
callback when loading a `CallRsp` payload, so `0x19` hands it that byte string
and it loads the nested stream (`ObjectCallGPCS.pyc:277`).

Three constraints, all from `ParseObjectID` and
`MachoObjectConnection.__init__`, and each of them a silent failure if missed:

| constraint | source | what goes wrong |
|---|---|---|
| the id contains **exactly one** colon | `nid, oid = objectID.split(':')` | `ValueError` on unpack |
| `nid` must **differ** from the connection's own `identifier`, which is the literal `'C=0'` on a client | `if nid != self.identifier: ... else: raise` | `RuntimeError('The specified object does not exist within this session')` |
| the node in the id must be a node we answer as | `nid.split('=')[1]`, `MachoAddress(nodeID=long(nid))` | every later call on the object is addressed to a node that is not us — a hang with no missing method behind it |

So `'N=<nodeID>:<n>'` is the form, and the `<nodeID>` has to match the
`proxy_nodeid` the handshake advertised. Those were two separate constants
until now, equal by coincidence; `Config::new` derives one from the other and
a test asserts they agree.

`refID` is a reference-count token the client only echoes back when releasing
the object. The real server uses a wallclock reading, and so do we.

**Still inferred:** that CCP's own writer uses `0x19` for this. The tag's
*mechanism* is settled from `blue.dll` — the Save callback is consulted at one
site, the `0x17` branch, and a non-`None` return makes the writer emit `0x19`
followed by the returned object (`docs/archive/marshal-tags.md` Gap 9) — and the client's
load path is confirmed from bytecode. But we have never seen a real one on the
wire, because references only travel server→client and we are the server. If
this is wrong the symptom will be loud: either "Unmarshal stream contains
custom data but I have no callback method", or a `RuntimeError` out of
`ParseObjectID`.

#### Bind is also where our shortcut lives

`dispatch::handle_bind` dispatches the fused call against the same service
name, so a bound method is an ordinary arm in `service.rs` and gets the same
`UNANSWERED` diagnostics as everything else. That is only legitimate because
we are a single node with one object per service; a real cluster binds a
distinct object per `bindParams`. When that stops being true, `handle_bind` is
what has to grow a registry keyed by `(service, bindParams)`.

We also do not send the `OID+` bookkeeping in `oob`. The real server reports
new references there, but the client only ever *writes* that key — inbound
registration happens in `ParseObjectID` — so omitting it is safe until we need
release tracking. **[I]**

### The screen draws, then freezes: a short character row **[V]**

The first run to reach the character-select screen froze on it — the character
was drawn but could not be clicked. **Every call was answered** (29 of 29, no
`UNANSWERED` lines, no unread type codes), so this is a different failure mode
from everything above, and it is the one to expect from here on.

The client was raising on *our data*:

```text
AttributeError: KeyVal instance has no attribute 'skillTypeID'
  ... characterSelection.py(669) LoadCharacterSlots
  ... characterSlots.py(748) LoadSlot
  ... charselData.py(218) GetSkillInTrainingInfo

AttributeError: KeyVal instance has no attribute 'deletePrepareDateTime'
  ... characterSelection.py(792) OnSlotOnMouseEnter
  ... charselData.py(256) GetDeletePrepareTime
```

The first threw inside `LoadSlot`, so the slot was left half-built. The second
is the reason it was unclickable: it is reached from the **mouse-over**
handler, so it threw again on every hover, eventually tripping the client's
own `Exception in Error Logging, preventing infinite recursion` guard.

**Why this was invisible from the server.** We answered the call, the reply was
well-formed, and the client then fell over reading a field off it. There is no
server-side signal for that at all — no hang, no refusal, nothing in
`GpsSession.unanswered`. It shows up only in the client's own traceback log
(`tools/client-log.sh --errors`). An answered call is not a satisfied one.

#### The authoritative row

Recovered by decompiling the only two modules that touch a character row,
`loginCharacterselection/charselData.py` and
`charSelection/characterSlots.py`. Every name below is a bare attribute access
except `factionID`, which goes through `getattr(.., None)`:

| field | value we send | note |
|---|---|---|
| `characterID`, `characterName`, `logoffDate` | real | `logoffDate` is the sort key for `GetChars()` |
| `corporationID`, `allianceID`, `stationID`, `solarSystemID`, `factionID` | real / None | `factionID` truthy pulls in faction war data |
| `balance`, `balanceChange`, `skillPoints`, `unreadMailCount` | 0 | |
| `shipTypeID` | real | |
| `paperdollState` | `0` | a `force_recustomize` / `no_existing_customization` value diverts into character creation |
| `finishedSkills` | `[]` | |
| `skillTypeID` | `None` | **the branch point** — see below |
| `toLevel`, `trainingStartTime`, `trainingEndTime`, `queueEndTime`, `finishSP`, `trainedSP` | `None` | only read when `skillTypeID` is not None |
| `deletePrepareDateTime` | `None` | `bool()`-tested; read from the mouse-over handler |
| `lockTypeID` | `None` | `bool()`-tested by `IsUnavailable` (transferring, on sale) |

`GetSkillInTrainingInfo` branches on `skillTypeID is None` and returns an
all-None dict when it is, which the slot code then filters with
`if currentSkill is not None`. So "no skill in training" is expressed by one
`None`, and the six fields behind it are never touched. They are sent anyway:
the names are confirmed from the other branch, and a row that names its whole
column set is a truer stand-in for the real server's.

#### Two fields we had invented

`petitionMessage` and `deleteCharacterTime` were in our row and appear
**nowhere in the client** — zero hits across the whole `.pyc` tree.
`deleteCharacterTime` is the instructive one: it is close enough to the real
`deletePrepareDateTime` to survive a review unchallenged.

Two tests now guard both directions — every name the client reads is present,
and no name is sent that the client never reads. A missing field is far
cheaper to assert than to diagnose, and the diagnosis here needed the client's
log, a decompile and a hover.

### The character-select call set: `evecap --calls`, and four more methods **[V]**

The run that produced `captures/1792409964-0002.evecap` is the first where the
client got all the way through the character data and stopped on something
other than a protocol fault. It is also the run that produced a usable
instrument, so start there.

#### `evecap --calls`

`tools/dev.sh cargo run --locked -q --bin evecap -- /repo/captures/<f>.evecap --calls`
prints one line per service call, pairing requests to responses **by callID**
and decoding both halves:

```text
[ 39]  2370.586ms OK   charUnboundMgr   GetCharacterSelectionData()  -> ([<utillib.KeyVal ...>], (None, None), [...], [])  (0.1ms)
[ 41]  2372.597ms OK   config           GetMultiOwnersEx([90000001])  -> (["ownerID", "ownerName", "typeID", ...], [[90000001, u"Capsuleer", 1373, None, None]])  (0.1ms)
[ 45]  2873.482ms HANG seasonalLoginCampaignManager get_active_campaign()  <- NO ANSWER; the client is still waiting
```

Two details in there are worth stating because they cost time to learn:

- **Pair on callID, never on order.** The client has several calls outstanding
  at once, one per tasklet, and they do not return in order.
- **The client does not stop at the first hang.** This capture shows calls 18
  through 23 issued while four of them were unanswered, with pings still
  being served throughout. "The client is still talking" is not evidence that
  nothing is stuck, and the stuck call is not necessarily the last one.

The owner prime from the previous round is confirmed working here: frame 41
returns the `(header, data)` tuple and the client accepts it.

#### The four that hung

| call | answer | why |
|---|---|---|
| `seasonalLoginCampaignManager.get_active_campaign()` | `(None, None, None, None)` | **not `None`** — see below |
| `map.GetSecurityModifiedSystems()` | empty `dbutil.CRowset`, columns named | `.Index()` is called on it |
| `userSvc.GetRedeemTokens()` | `[]` | only iterated and `len()`d |
| `paperDollServer.GetPaperDollDataBatch([ids])` | `{}` | a real dict; `.iteritems()` is called with no guard |

#### `get_active_campaign` is the counterexample to "empty means None"

Most of this login path is forgiving: a falsy return short-circuits whatever
block consumes it, which is why so many arms in `service.rs` are `None`. This
one is not, and it is worth knowing as a shape of mistake rather than as a
fact about one method.

`SeasonalLoginCampaignService.prime_campaign_data` unpacks **before** it tests:

```text
 45 CALL_FUNCTION     0
 48 UNPACK_SEQUENCE   4          <- unconditional
 51..60 STORE_FAST    campaign_id, end_date_ordinal,
                      turnover_time_str, num_remaining_rewards
 63 LOAD_FAST         campaign_id
 66 LOAD_CONST        None
 69 COMPARE_OP        9 (is not)
 72 POP_JUMP_IF_FALSE 168        -> straight to `return`
```

So `None` raises `TypeError: 'NoneType' object is not iterable` inside the
tasklet — the same black screen as no answer at all, but with a traceback.
A 4-tuple whose first element is `None` skips lines 58–63 entirely and leaves
`end_date`, `time_keeper` and `number_of_reward_days_remaining` at `None`;
every downstream path is guarded by `is_login_campaign_active()`, which is
exactly `active_login_reward_campaign_id is not None`. The remaining three
slots are a `date.fromordinal` ordinal, a `'%H:%M:%S'` string for `strptime`,
and a reward count, none of them read while the id is `None`.

The sibling `loginCampaignManager.get_client_campaign_state` *does* take a
plain `None`. Two services with near-identical names and different contracts:
check the caller, every time.

#### The paperdoll calls are self-inflicted, and there are three of them

`paperDollServer.GetPaperDollDataBatch` is only ever called because **we grant
ourselves `ROLE_PROGRAMMER`.** `EvePhoto.GetPortrait` falls back to a locally
rendered scene portrait only when the image-server portrait is missing *and*
that bit is set (`BINARY_AND` at offset 139, `POP_JUMP_IF_FALSE` at 140
skipping `_QueueScenePortrait`). We serve no image server, so the first
condition always holds. See `docs/reference/session-roles.md` §5.5, which this finding
corrects.

Answering the batch alone is not enough: `_FetchBatchData` then runs a second
loop over the same batch and calls, for every non-NPC id,

- `charMgr.GetPublicInfo(charID)` — `COMPARE_OP 7 (not in)` on the cache at
  320, then `LOAD_ATTR GetPublicInfo` at 372, inside `try/except UserError`;
- `paperDollServer.GetPaperDollPortraitDataFor(charID)` via
  `GetPortraitSettings`, which unpacks two values and swallows any exception.

An exception is tolerated by both; **no answer is not**, because the worker
blocks. This is the general case of the hang rule: stubbing one call often
reveals the next two behind it, and the cheapest way to find them is to read
the caller to the end of its loop rather than to the end of its first
statement.

One thing to avoid: `GetPaperDollDataBatch` must not raise `UserError`. That
is the rate-limit signal, and the caller answers it by pushing the batch back
onto the queue and sleeping 5 seconds — forever.

#### `map.GetSecurityModifiedSystems` needs a real rowset, not an empty one

This is the other end of the range from `get_active_campaign`: not a tuple
that must be the right length, but an **object that must have a method.**
`CharacterDetailsLocation._GetSecurityModifierText` calls and then
dereferences, with nothing in between:

```text
 21 CALL_FUNCTION 0
 30 LOAD_ATTR     Index
 33 LOAD_CONST    'solarSystemID'
 48 COMPARE_OP    6 (in)
 60 BINARY_SUBSCR
 70 LOAD_ATTR     security
 76 LOAD_ATTR     modifiedSecurity
```

So `None`, `{}` and `[]` are all `AttributeError`, and the `(header, data)`
tuple that the owner primes use is wrong here too — this call does **not** go
through `cfg.Recordset._Prime`. The answer is an empty `dbutil.CRowset`
(`eve_rowset::CRowset::new(descriptor)`) with columns `solarSystemID` (I4),
`security` and `modifiedSecurity` (both R5 — which is CCP's own name for the
8-byte double, see `eveCfg.pyc`, not a typo for R8).

**Declare the columns even with no rows.** `Rowset.Index` does
`header.index(idName)` and raises `ValueError` on a missing column whatever
the row count; the native `CRowset` path hands the name to
`blue.pyos.XUtil_Index`, which is C and unread. Naming them is free.

`@Memoize(1)` on the caller memoises per `locationID`, not per session, so
this is one call per distinct system and more after `clear_memoized()` on
close. It has to stay cheap.

A related call to pre-empt: once in space, `SecurityService.Initialize` asks
`securityMgr.get_modified_systems()` for the same data and builds a dict
comprehension over `.iteritems()` — so that one wants a **dict**, the opposite
of this one. `{}` is safe. **[I]**

#### `userSvc.GetRedeemTokens` — `[]`, and why not `None`

Only iterated and `len()`d; no rowset method is ever called on it, so a plain
empty list is the whole answer.

`None` is the trap, and it is instructive because it *half* works: it survives
`redeemsvc.GetRedeemTokenCount`, which guards with an explicit `is None`, and
then dies on `GET_ITER` in `RedeemItemsContainer.Controller.load`, whose
tasklet never reaches `on_load()`. The panel never becomes ready — the same
hang, one frame later.

`[]` caches correctly, which is luck rather than design:
`redeemsvc.GetRedeemTokens` re-fetches on `self.tokens is None`, not on
falsiness. Had that been `if not self.tokens:`, an empty list would have
re-called us on every access. Worth remembering before answering some other
`Get...` with an empty container and assuming it will be asked once.

### First real client: what the first post-handshake frame taught us **[V]**

With the crypto hash fixed, a real client completed the handshake, took the
session, and sent one frame — 174 bytes — which the server could not read at
all. Four separate things were wrong, and none of them were visible from the
Python alone. Capture: `captures/1792404377-0002.evecap`, frame 11.

**1. The payload is zlib-compressed.** It begins `78 01`, and the *whole*
marshal stream including its `0x7E` header is inside the compressed data. This
is **not** the frame-level bit-29 compression in `docs/reference/framing.md` — that bit was
clear, and that form carries a `u32 rawLen` prefix this one does not have. It
is the client's native marshal layer compressing a stream once it grows (275
bytes raw here). `wire::loads` therefore sniffs for a zlib header before
parsing, using RFC 1950's own validity rules; `0x7E`/`0x7D` cannot be a zlib
CMF byte, so the two forms are distinguishable exactly, in both directions.
Inflating is `eve-inflate`, written from scratch because the workspace takes no
external crates.

**2. The pickle is carried as a substream (`0x2B`), not a byte string.**
`ToPickle` is `blue.marshal.Save(...)`, which returns a `str` in Python, so a
string tag is what the source suggests. The writer uses the substream tag
instead. Both now decode; we emit the substream form.

**3. `callID` can be `None`.** The destination was
`MachoAddress(NODE, nodeID=1, service='machoNet', callID=None)`. Our model
typed `callID` as a plain integer, so the packet failed to parse. It is
`Option<i64>` now — `None` and `0` are different callIDs and a reply has to
echo back whichever arrived.

**4. The string table was the wrong table entirely.** The kwargs key arrived as
tag `0x11` byte `0x87` — string-table index 135. See `docs/archive/marshal-tags.md`:
`marshalstrings.py` is a no-op and the live table is a 195-entry native array in
`blue.dll`. Index 135 there is `machoVersion`, which is exactly the key
`RemoteServiceCallWithoutTheStars` sets — so one byte of real traffic confirmed
the table, the 1-based convention, and the fix.

The frame decodes as:

```text
CallReq(
  command     = 6,
  source      = MachoAddress(CLIENT, clientID=0, callID=1, service=None),
  destination = MachoAddress(NODE, nodeID=1, service='machoNet', callID=None),
  userID      = 1,
  body        = ((0, <substream: (1, 'GetServiceInfo', (), {machoVersion: 1})>),),
  ...
)
```

and `ServiceCallGPCS.CallUp` reads it as

```text
if not packet.payload[0]:               # outer 0 => a service call
    packet.payload = packet.payload[1]  # materialise the substream
    return self.ForwardCallUp(packet)   # re-enter with (1, method, args, kw)
```

so the outer `0` and the inner leading `1` are two different discriminators one
level apart.

### Reaching character selection: the owner/location primes **[V]**

Answering `invbroker.GetItemDescriptor` let `ApplyInitialState` finish, and
the client went all the way through:

```text
OK   invbroker        GetItemDescriptor
OK   subscriptionMgr  GetCloneGrade
OK   charUnboundMgr   GetCharacterSelectionData     <-- row 8, answered
>>>  config           GetMultiOwnersEx              <-- hangs here
```

So the client asked for its characters, got them, and then hung **one call
short of drawing them** — the same black screen, the same cause, one step
further on. Row 9 of the table above: `cfg.eveowners.Prime` fetches the names
behind the ids in the character rows, and blocks like any other call.

`cfg.Recordset._Prime` accepts either form:

```python
fetch = conf.<dbMultiFetcher>(keysToGet)
if type(fetch) is TupleType:
    self.header, data = fetch
elif hasattr(fetch, '__columns__'):
    self.header = fetch.__columns__
```

The `(header, data)` tuple is the cheaper one — an exact `type ==` test, no
whitelisted class, no packed blob — and it is what `eve_rowset::cfg`'s
`*_header_data` builders produce, in the column order the client pre-sets on
itself before ever calling.

**Only the ids we can name come back.** A key the reply drops is remembered in
`knownLuzers` and never asked for again, which is the right outcome for an id
this sandbox has never heard of — better than inventing a name for it.

### The black screen: one unanswered call hangs `ApplyInitialState` **[V]**

Everything answered, no dialog, an empty Gamelog — and a black window. The
client's log named it:

```text
EXCEPTION: AttributeError: 'NoneType' object has no attribute 'view'
  viewStateSvc.py(961) OnShowUI
  viewStateSvc.py(900) UpdateOverlays
  viewStateSvc.py(927) _GetOverlaysToEnable
  session was <Session: (... changing:ApplyInitialState, userid:1,
                         role:0x6168005200e41000L, sessionType:5 ...)>
```

`_GetOverlaysToEnable` begins `self.primaryInfo.view.__overlays__`, so
`primaryInfo is None` means **no view was ever activated** — there is nothing
to draw, hence black.

The session line is the real evidence: twenty-six seconds in, the session is
still `changing:ApplyInitialState`. The initial state never finished applying,
so the login never reached the step that activates `charsel`, and
`charUnboundMgr.GetCharacterSelectionData` was never called — it appears in no
capture.

The cause was the one call left unanswered: `invbroker.GetItemDescriptor`.
`invCache.GetItemHeader` blocks on it, a service primes `invCache` while the
initial state is being applied, and `RemoteServiceCallWithoutTheStars` blocks
its tasklet until a response arrives. One unanswered call therefore stalls the
whole session change — quietly, with no error, because nothing failed; it is
still waiting.

**The lesson is general and worth stating on its own: on this protocol an
unanswered `CallReq` is not a no-op, it is a hang.** It has now cost two
debugging rounds — `invbroker.GetItemDescriptor` here, and
`config.GetMultiOwnersEx` one call short of the character select. Both times
the answer was already in `GpsSession`'s `unanswered` list and unread, because
the connection was still open and `state()` had not printed yet. The server
now prints `[svc] UNANSWERED <service>.<method>` the moment it happens. A service we have not
implemented is better off returning something wrong than returning nothing,
and the `unanswered` list in `GpsSession::state` is the thing to read first
when the client goes quiet.

`GetItemDescriptor` must return a real `blue.DBRowDescriptor`, because
`godma.sublocrd` iterates it as `(key, dbtype, size)` triples. The column set
is reconstructed — see `service::item_descriptor` for the provenance and for
why it is **[I]** rather than **[V]**.

### A ping must carry the proxy/server hop chain **[V]**

With responses pickled correctly the client ran 36 frames and 21 seconds of
session, and the one remaining exception was:

```text
ValueError: 'proxy::writing' is not in list
  clientStatsSvc.py(346) GetMachoPingTime
  machoNet.py(3886) Ping
```

`MachoNetService.Ping` ends, **unconditionally**, with

```python
server1 = [p[0].strip() for p in prepared].index('proxy::writing')
server2 = [p[0].strip() for p in prepared].index('proxy::handle_message', server1)
```

so `proxy::writing` must be present and `proxy::handle_message` must follow
it. We were appending a single `proxy::turnaround`.

The required order makes sense once you see where the labels come from:

| appended by | label |
|---|---|
| `HandleMessage`, on every ping request or response received | `<mode>::handle_message` |
| `FragileWrite`, on every ping forwarded | `<mode>::writing` |
| the `PING_REQ` branch of `HandleMessage`, before replying | `<mode>::turnaround` |

On a real cluster a client ping is routed **through** the proxy to a server
node, so `proxy::writing` is the proxy forwarding it onward and the later
`proxy::handle_message` is the proxy taking the server's answer — that pair
measures the server leg, which is why the client calls them
`server1`/`server2`. **A proxy that answers a ping by itself never emits
`proxy::writing` at all**, because that reply goes out through
`srcTransport.Write` rather than `FragileWrite`. That was exactly our list.

Being one process standing in for both nodes, we reproduce the chain both
would have written:

```text
client::start            (the client's own, kept)
proxy::handle_message    the proxy takes the request
proxy::writing           ...and forwards it to the server
server::handle_message   the server takes it
server::turnaround       ...answers it
server::writing          ...and writes the answer back
proxy::handle_message    the proxy takes the answer
proxy::writing           ...and writes it to the client
```

All at the same timestamp, which is the truth rather than a placeholder:
there is no network between our "proxy" and our "server".

### A `CallRsp` payload is a pickle, not the value **[V]**

The first thing file logging bought us was the cause of the two exceptions
that had been aborting every login:

```text
EXCEPTION #4 : Unhandled exception in <TaskletExt ...>
TypeError: unhashable type
  machoNet.py(2028) ConnectToServer
  ServiceCallGPCS.py(315) RemoteServiceCallWithoutTheStars
  ObjectCallGPCS.py(239) CallDown
  ObjectCallGPCS.py(495) FromPickle
    otherDudesAddress = Address::Node(nodeID="1", service="machoNet", callID="None")
    thePickle = {}
```

`thePickle = {}` is **our** `GetServiceInfo` answer. `ObjectCall.CallDown` does

```python
retval = self.ForwardCallDown(packet)
retval.payload = self.FromPickle(retval.payload, packet.destination)
```

and `FromPickle` is `blue.marshal.Load(thePickle, self.ParseObjectID)`. The
client hands a response payload **straight to `marshal.Load`**, so it must be a
marshalled string. We were sending the live value, and `marshal.Load` raised on
each one — `{}` gave *"unhashable type"*, and `alert.BeanCount`'s
`(None, None)` gave *"argument must be a string or buffer"* in exception #6,
from the identical stack.

**A request is the other way round.** Its payload is `(0, substream)` and
`ServiceCallGPCS.CallUp` uses `payload[1]` as the call tuple directly, without
loading it. The asymmetry has a reason: a response is loaded explicitly so it
can pass `ParseObjectID`, the callback that turns bound-object references in a
return value back into live objects. A request carries none.

So the rule is:

| direction | payload | who decodes it |
|---|---|---|
| `CallReq` | `(0, <substream of (1, method, args, kwargs)>)` | the marshal decoder, inline |
| `CallRsp` | a **marshalled byte string** of the return value | `ObjectCall.FromPickle`, explicitly |

A plain byte string, not a substream: the substream tag materialises into an
object when the stream is decoded, which lands straight back on
`marshal.Load(<an object>)`.

`PingRsp` is **not** affected — `machoNet.HandleMessage` handles pings itself,
without going through `ObjectCall.CallDown`, so its `times` stay a live list.

This also closes the `DirtySession` loop: the failing call was inside
`ConnectToServer`, so the login aborted with the transport still open, which
is precisely the "failed cleanup during a disconnect" the next attempt
complained about.

### Seeing what the client actually thinks: `/fileLogDirectory` **[V]**

The client reports its exceptions to the server's `alert` service as an opaque
stack hash, and the Gamelog under `Documents/EVE/logs` carries only
user-facing message-box text. So a client-side traceback was invisible to us,
and several rounds of debugging were inference from what the client *stopped*
doing.

It does not have to be. `blue` has a native file-logging sink, and
`carbon/common/lib/autoexec_common` enables it from the command line very
early in startup, before login:

```python
fileLoggingThreshold = 0
for arg in blue.pyos.GetArg():
    if arg.startswith('/fileLogDirectory'):
        try:    fileLoggingDirectory = arg.split('=')[1]
        except IndexError: fileLoggingDirectory = None
if not fileLoggingDirectory:
    fileLoggingDirectory = prefs.ini.GetValue('fileLogDirectory', None)
    fileLoggingThreshold = int(prefs.ini.GetValue('fileLoggingThreshold', 1))
if fileLoggingDirectory:
    blue.EnableFileLogging(os.path.normpath(fileLoggingDirectory),
                           boot.role, fileLoggingThreshold)
```

Two details make the **command-line** form the right one. The threshold is
severity-indexed (`0=info, 1=notice, 2=warn, 3=err`), and the command-line
path leaves it at `0` — everything — while the `prefs.ini` path defaults it to
`1` and silently drops info. And a switch touches nothing on disk, which suits
a client we are otherwise keeping stock.

`tools/run-client.sh` passes `/fileLogDirectory=C:\eve-logs`, which lands in
the Proton prefix as an ordinary directory on this machine. Read it with
`tools/client-log.sh` (`--errors` prints just the exception blocks).

Tracebacks arrive whole. `logmodule.LogException` and `LogTraceback` both
default `toLogServer=1`, and the stream they write to is buffered until
`flush()`, so one exception is one record containing the formatted traceback
**with locals** — not something to reassemble. The native record holds 12800
bytes and appends `" -- MESSAGE TRUNCATED"` beyond that.

There is also a UDP sink — `blue.EnableNetworkLogging(host, port, role,
threshold)`, configured by `networkLogging`/`networkLoggingPort` in
`prefs.ini`, emitting zlib-compressed GELF 1.0 JSON (default port 12201, with
CCP's own 38-byte chunk header rather than standard GELF chunking when a
message exceeds 1420 bytes). The host must be a dotted quad, because the
native code calls `inet_addr` rather than `getaddrinfo`. Not used: file
logging is lossless, needs no receiver, and starts earlier. Noted here so the
option is on the record, and because pointing it at `10.137.0.1` stays inside
the jail.

### "Local Session Corrupt" is `UserError('DirtySession')` **[V]**

A client that has already connected once in the same process shows this dialog
on the *next* login attempt:

> **Local Session Corrupt** — The client's local session information is
> corrupt, probably as a result of failed cleanup during a disconnect. …
> Please restart the client prior to logging in.

It is a **client-side pre-connect check**, not a reaction to anything the
server sends. `MachoNetService.ConnectToServer` (`machoNet.pyc`, offsets
147-289) runs this before opening a socket:

```python
dirty = 0
for each in session.__persistvars__:
    if getattr(session, each) != session.GetDefaultValueOfAttribute(each):
        self.LogError('Connecting with a dirty session.  ', each, ' was ',
                      getattr(session, each), ' but should have been ',
                      session.GetDefaultValueOfAttribute(each))
        dirty = 1
if dirty:
    log.LogTraceback('Connecting with a dirty session')
    raise UserError('DirtySession')
```

`__persistvars__` (`basesession.CoreSession`) is `['userid', 'languageID',
'role', 'charid', 'address', 'userType', 'maxSessionTime', 'sessionType',
'countryCode']`. Defaults are `None` except `role` → `ROLE_LOGIN`
(`4611686018427387904`), `languageID` → `str(prefs['languageID'])`, and
`sessionType`, which reads back its own value and so can never be dirty.

Why the text is not in `code.ccp`: the key is **`DirtySession`**, and the
words come from localization — `UI/Messages/800/DirtySessionTitle` (259590)
and `.../DirtySessionBody` (259591), in the resfile cache under
`.../CCP/EVE/ResFiles/`, not in the client directory. Grepping for the English
text was never going to find it.

**Why cleanup fails.** The only thing that clears the session is
`CoreSession.ClearAttributes`, reached from
`machoNetTransport.MachoTransport.SessionClosed`. It sets
`self.__dict__['clearing_session'] = 1` and **never resets it**, and it
early-outs when that flag is already set. So a client can fully clear its
session **once per process**: the first full disconnect clears it, the second
silently does nothing, and the login after that raises `DirtySession`. That is
exactly why the dialog says to restart the client — in-process recovery is not
possible.

**What this means for the server.** We cannot cause or prevent it directly; we
can only cause it *indirectly* by making the client abort a login after a
session was established and then retry. Two consequences:

1. **Testing:** restart the client between login attempts. After one full
   disconnect the client is one disconnect away from this dialog no matter
   what the server does, so a second attempt in the same process tells us
   nothing.
2. **Server behaviour:** do not drop or refuse a connection after sending
   `session_init` / the session notification, and close through a normal
   transport close so `SessionClosed` → `ClearAttributes` actually runs. The
   client keys that cleanup off `transport.sessions[nodeID]`, so a session
   associated with a clientID the client did not register means `SessionClosed`
   finds nothing and skips cleanup entirely.

The dialog is therefore a **symptom of an earlier abort**, and the abort is
what to chase. Diagnostically, the check logs one
`Connecting with a dirty session.  <attr> was <value> but should have been
<default>` line per offending attribute — which names the field, if we can
ever see the client's log.

### Second real client run: PingReq, tag 0x2F, and `alert.BeanCount` **[V]**

Answering `GetServiceInfo` let the client go further, and the next capture
(`1792405830-0002.evecap`, 19 frames) showed three more things.

**`PingReq` (command 20) must be answered, and it is annotated rather than
acknowledged.** `machoNet.HandleMessage`:

```text
theMessage.times.append((blue.os.GetWallclockTime(),
                         blue.os.GetWallclockTimeNow(),
                         macho.mode + '::turnaround'))
theResponse = theMessage.Response(times=theMessage.times[:])
srcTransport.Write(theResponse)
```

Every hop appends one `(wallclock, wallclock-now, label)` triple to the list it
received and passes the **whole list** on; the client seeds it with
`client::start` and reads the differences to measure the cluster round trip. A
fresh list, or a bare acknowledgement, throws away the measurement it asked
for. We are the proxy, so our label is `proxy::turnaround`. The label is
unicode (tag `0x2E`), matching the entry the client sends.

**Those timings are Windows FILETIMEs under tag `0x2F`**, which was specified
in `docs/archive/marshal-tags.md` but never implemented, so the server could not even
decode the ping. The two values in that capture decode to unix 1792405830 —
the capture file's own timestamp, which makes them self-checking.

**`MachoPacket.Response` does not invent a source address.** Read off its
bytecode, a reply sets `source = self.destination` and
`destination = self.source`, copies `userID`, `trace_id`, `ingress_id`,
`sampled` and `sample_rate`, and clears `contextKey`, `parent_span_id` and
`cut_parent`. `journeyID` is not copied. We were building replies with a node
address of our own choosing; `eve_proto::MachoPacket::respond_to` now mirrors
`Response` exactly, which matters because `machoNetTransport.LockSession`
calls `WaitForSequenceNumber(packet.source, ...)` on even-numbered commands.

**`alert.BeanCount` is the client's error reporter.** `Alert.BeanCount(
stackIDHash)` returns `(errorID, logMode)`, and `(None, None)` when the hash is
unknown — which on this server is always. The client calls it from its own
exception handler, so an unanswered `BeanCount` turns one client-side error
into a hung call on top. Four of them in that capture mean the client raised
four exceptions; what they were is still unknown, since the Gamelog carries
only user-facing messages.

| # | when | service.method | args | return shape the client consumes | read from |
|---|---|---|---|---|---|
| 1 | `ConnectToServer` step 12 | `machoNet.GetServiceInfo` | () | `dict {serviceLogName: machoResolveString}`. Used by `RemoteServiceCallWithoutTheStars` only as a *hint* for guessing node ids; `{}` is acceptable (calls then go to the ANY address). The real proxy wraps it in a `CachedObject`; a bare dict is what the code stores in `self.machoNet.serviceInfo` | `machoNet.pyc GetServiceInfo`, `ServiceCallGPCS` |
| 2 | `ProcessLoginProgressDone` → `cfg.LoadCfgData` | `config.GetBlackListedPlanets` | () | stored verbatim as `cfg.planetBlacklist`; a container is enough. **Synchronous, blocks login** | `eve/common/script/sys/eveCfg.pyc LoadPlanetBlacklist` |
| 3 | 2 s after `LoadCfgData`, async tasklet | `config.GetAverageMarketPrices` | () | stored as `cfg._averageMarketPrice`; `{}` fine | `eveCfg.GetAveragePricesThread` |
| 4 | `connection.SynchronizeClock`, ×1–5 | `machoNet.GetTime` (via `ProxySvc`, so `nodeID = proxy_nodeid`) | () | **int/long**: Windows FILETIME, 100-ns ticks since 1601-01-01 UTC (`blue.os.GetWallclockTimeNow()`). Client computes `diff = now - (serverTime + elapsed/2)` and `blue.pyos.synchro.ResetClock(serverTime)`. Loop ends after 3 "good" samples or 5 tries | `connectionService.SynchronizeClock`, `machoNet.GetTime` |
| 5 | every 1–3 min | `machoNet.GetTime` ×5 | () | same | `connectionService.ClockSyncDaemon` |
| 6 | when transport idle ≥ `maxConnectionIdleTimeSeconds` | `pingService.Ping` (via `ProxySvc`) | () | return ignored | `connectionService.PingLoop` |
| 7 | session change with `'userid'` | `subscriptionMgr.GetCloneGrade` | () | int: `0` = `CLONE_STATE_ALPHA`, `1` = `CLONE_STATE_OMEGA` (`clonegrade/const.pyc`) | `gameui.OnSessionChanged` → `omega/client/clone_grade_svc.pyc prime_clone_state_with_grace_period` |
| 8 | `_HandleUserLogonSessionChange` | **`charUnboundMgr.GetCharacterSelectionData`** | () | **4-tuple** — the critical one, detailed below | `ccSvc.GetCharacterSelectionData` → `loginCharacterselection/charselData.pyc` |
| 9 | after #8, local | `cfg.eveowners.Prime` / `corptickernames.Prime` / `evelocations.Prime` ⇒ on a miss: `config.GetMultiOwnersEx(ids)`, `config.GetMultiCorpTickerNamesEx(ids)`, `config.GetMultiLocationsEx(ids)` | `[ids]` | a rowset with `.columns` (or a `(header, data)` tuple, or an object with `__columns__`). Required columns — owners: `ownerID:I4, ownerName:WSTR, typeID:I4, gender:I2, ownerNameID:I4`; locations: `locationID:I8, locationName:WSTR, solarSystemID:I8, x:R5, y:R5, z:R5, locationNameID:I4`. Keys not returned are remembered as `knownLuzers` and never re-asked | `carbon/common/script/sys/cfg.pyc Recordset._Prime`, `eveCfg.GetOwnersRowDescriptor` / `GetLocationRowDescriptor` |
| 10 | `characterSelection.OnOpenView` | `survey.PerformSurveyChecks`, then `survey.GetActiveSurveyID` | () | **conditional** — both are skipped unless the shipped FSD survey data has an active/grace-period survey (`surveySvc.PerformSurveyChecks` returns early) | `eve/client/script/ui/services/surveySvc.pyc` |
| 11 | `characterSelection.InitUI`, async | `charUnboundMgr.GetCharOmegaDowngradeStatus` | () | truthy ⇒ draws the "lapsed game time" warning bar. `False`/`None` is the quiet answer. Wrapped in a try/except | `characterSelection._HandleSubscriptionDowngradeStatus` |
| 12 | `characterSelection._CheckOpenRewardsWindow` | `loginCampaignManager.get_client_campaign_state` | () | object with `.static_data` and `.item_progress`; a falsy return (`None`) short-circuits every reward check. `loginCampaignService.Run` also binds `sm.RemoteSvc('loginCampaignManager')` at service start | `loginrewards/client/loginCampaignService.pyc _get_campaign_state` |
| 13 | charsel, on hover/entry only | `seasonalLoginCampaignService.is_login_campaign_active` (local svc, may go remote) | — | not on the critical path | `loginCampaignService.should_reward_btn_be_visible` |
| 14 | clicking a character | `multiLoginBlocker.Login(computerHash)` — only if `connection.GetComputerHash()` is not `None` | `(str,)` | **bool**; falsy ⇒ `MultiLoginBlockedWindow` and the login is refused. Returning `computer_hash = None` at H10 skips the call entirely | `characterSelection._CanLogin` |
| 15 | clicking a character | `charUnboundMgr.GetCharacterLockType(charID)` | `(int,)` | `None` = unlocked. Non-`None` raises `UserError` (`CharacterTransferring` / `CharacterOnSale` / `CharacterLocked`) | `characterSelection.CheckCharacterLockStatus` |
| 16 | clicking a character | `moniker.Moniker('populationCap', (charID, groupCharacter)).GetCharacterLoadSlot(charID)` | `(int,)` | bound-object call (`MachoBindObject`) — **beyond M1's charsel milestone** | `characterSelection.TryEnterGame` |
| 17 | clicking a character | **`charUnboundMgr.SelectCharacterID(charID, secondChoiceID, skipTutorial)`** wrapped in `sessionMgr.PerformSessionChange('charsel', …)` | `(int, int\|None, bool)` | return ignored; the *real* effect must be a `SessionChangeNotification` adding `charid` (and station/system) — this is the M2 boundary | `characterSelection.EnterAsCharacter`, `carbon/common/script/sys/sessions.pyc PerformSessionChange` |

### #8 in detail — `charUnboundMgr.GetCharacterSelectionData()`

`charselData.CharacterSelectionData.FetchCharacterInfoForUserID`:

```
userDetails, self.trainingDetails, characterDetails, wars = charRemoteSvc.GetCharacterSelectionData()
self.userDetails = userDetails[0]
warsByCharacterID[eachWar.characterID].append(eachWar)   for each row in wars
```

So the return is `(userDetails, trainingDetails, characterDetails, wars)`:

- **`userDetails`** — indexable; only `[0]` is used. Attributes read:
  `characterSlots` (→ `GetNumCharacterSlots`), `userName`, `creationDate`,
  `subscriptionEndTime`, `maxCharacterSlots` (→ `int()`, drives how many slots
  the screen draws).
- **`trainingDetails`** — opaque; stored and returned as
  `Bundle(subscriptionEndTime=…, trainingEndTimes=trainingDetails)`. Initialised
  to `(None, None)` before the call, so a 2-tuple of `None` is a safe stub.
- **`characterDetails`** — iterable of rows. Every attribute read off a row,
  across `charselData.py`, `characterSlots.py` and `characterSelection.py`:

  | attribute | used for |
  |---|---|
  | `characterID` | key; slot identity |
  | `logoffDate` | **sort key** for `GetChars()` (descending) — must be comparable |
  | `corporationID`, `allianceID` | `GetCorporationInfo()`; primed into `cfg.eveowners` / `cfg.corptickernames`; `allianceID` may be `None`/0 |
  | `stationID`, `solarSystemID` | location; `stationID` primed into `cfg.evelocations`. If `stationID` set and `solarSystemID` not, it is looked up via `cfg.evelocations.Get(stationID).solarSystemID` |
  | `factionID` | optional (`getattr(…, 'factionID', None)`); non-zero ⇒ synthetic faction wars |
  | `balance` | `GetWalletBalance()` |
  | `balanceChange` | `GetWalletChanged()` |
  | `skillPoints` | `GetSkillInfo()` (`or 0`) |
  | `unreadMailCount` | `GetUnreaddMailCount()`; `> 0` ⇒ `mailSvc.CheckNewMessages_thread` after select |
  | `shipTypeID` | `GetCurrentShip()` |
  | `paperdollState` | `GetPaperDollState()`; compared against `paperdoll.State.force_recustomize` / `no_existing_customization` — either value diverts to **character creation** instead of entering the game |
  | `finishedSkills` | `GetFinishedSkills()` |
  | `deletePrepareDateTime` | `GetDeletePrepareTime()` / `IsPreparingForDeletion()`; non-`None` blocks entering |
  | `lockTypeID` | `IsUnavailable()` / `GetCharacterLockType()` |
  | `skillTypeID` | `None` ⇒ "no skill in training" branch, skipping all of the next five |
  | `toLevel`, `trainingStartTime`, `trainingEndTime`, `queueEndTime`, `finishSP`, `trainedSP` | skill-in-training panel (only when `skillTypeID` is not `None`) |
  | `gender`, `raceID`, `bloodlineID` | only on the re-customization path (`CreateNewAvatar`) |

  **Safest stub: set `skillTypeID = None`, `paperdollState` to something that is
  neither `force_recustomize` nor `no_existing_customization`,
  `deletePrepareDateTime = None`, `lockTypeID = None`.**
- **`wars`** — iterable of rows; each needs `.characterID`. Rows are also read
  for `warID, declaredByID, againstID, mutual, ally` in the at-war tooltip
  (`characterSlots.LoadAtWarTooltipPanel`). An empty list is fine.

Non-remote data the slot UI still needs: `cfg.mapSystemCache[solarSystemID].securityStatus`
(local FSD `res:/staticdata/systems.static`), `cfg.evelocations`,
`cfg.stations.Get(stationID).orbitID`, `photo.GetPortrait(charID, 512, …)` and
`photo.GetAllianceLogo` (these go to `paperDollServer`, and are async /
failure-tolerant).

### Calls the current `docs/archive/session-flow.md` lists but that this build does **not** make

- `authentication.AccruedTime` and `authentication.AmUnderage` exist only in
  `eve/client/script/ui/login/antiaddiction.pyc`, and `OnLogin()` returns
  immediately unless `boot.region == 'optic'`. This build's region is `ccp`, so
  **neither is ever called**.
- `authentication.SetLanguageID(mlsLanguageID)` is called only from
  `gameui.SetLanguage`, i.e. when the user actively changes language *and*
  `session.userid` is set. Not on the login path.
- There is no `charUnboundMgr.GetNumCharacters` or `GetCharacterInfo` call
  anywhere in this tree. The one call is `GetCharacterSelectionData()`.

---

## 9. Corrections to `docs/archive/session-flow.md`

1. **Login-progress order.** There are three steps, not four, and the order is
   `connecting` (1/3) → `authenticating` (2/3) → `done` (3/3).
   `loginprogress::lowlevelversioncheck` exists **only** as a key in
   `connectionService.LoginProgressLabels`; nothing in this build ever scatters
   it (verified across the whole tree). The version check happens *inside*
   step 2.
2. **The first service calls** are not `authentication.*` /
   `charUnboundMgr.GetNumCharacters`. They are `machoNet.GetServiceInfo`,
   `config.GetBlackListedPlanets`, `machoNet.GetTime` ×N, and then
   `charUnboundMgr.GetCharacterSelectionData` — see §8.

---

## 10. Remaining GAPs (not guessed here)

1. **On-wire framing** — `socket.sendpacket` / `recvpacketoob` are native
   (`SocketGPS.pyc`); length-prefix width/endianness still unknown. Unchanged
   from `docs/archive/wire-format.md`.
2. **Marshal numeric opcodes / DBRow packed layout / the `0x7D` vs `0x7E`
   marker** — unchanged from `docs/archive/wire-format.md`.
3. **`carbon/server/script/net/handShake.py`** (`GetHandshakeFunc`, `Verify`) is
   not in the client tree, so the real handshake expression is unknown. Only its
   *shape* matters to the client (§4, H8 note 3).
4. **`blue.crypto.GetSharedAsymmetricCipher()`** — whether it uses the 512-bit
   blob in `evecrypto/publickey.pyc` is unconfirmed; `get_keystr()` has no
   Python caller.
5. **The `authentication` service** (`Login`, `Logout`) is server-side; the
   exact contents of the real `session_init` dict are unknown. We author it
   ourselves, constrained by `Session.__persistvars__` (§7).
6. **`populationCap.GetCharacterLoadSlot`** is a `MachoBindObject` bound-object
   call; the bound object's interface was not traced (post-charsel, M2).
7. **`loginCampaignManager.get_client_campaign_state`** return type is a
   server-side object with `.static_data` / `.item_progress`; only the falsy
   short-circuit was verified.
