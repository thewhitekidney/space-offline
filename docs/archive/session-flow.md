# Connection and session establishment — overview

> **Demoted 2026-10-24** — self-declared superseded, with three of its own claims found wrong and two still wrong. Current truth: [docs/reference/login-path.md](../reference/login-path.md). Kept for history; do not brief a lane from this file.

**Superseded in detail by [docs/reference/login-path.md](../reference/login-path.md)**, which specifies the
handshake payload by payload and lists the exact call sequence and return
shapes. This file is the short version plus the decisions a server author needs
up front.

Three earlier claims in this file were **wrong** and are corrected below; they
had been inferred from service-name scanning rather than read from the client.

## The encryption problem, and the fix

`start.ini` in this installation carries `cryptoPack = CryptoAPI`. Under that
pack, **every MachoPacket after the handshake is symmetrically encrypted**
(`SocketGPS`: `Read`/`Write` are `EncryptedRead`/`EncryptedWrite`), and the AES
session key is RSA-sealed to CCP's public key. A server that cannot unseal it
cannot talk to the client at all. This is the single biggest blocker found so
far, and it was invisible to every earlier pass.

The fix is clean, because of how the pack is chosen
(`packages/evecrypto/settings.py`):

```python
cryptoPack = boot.GetValue('cryptoPack', 'Placebo')
```

**`Placebo` is the default**; this install opts into `CryptoAPI` explicitly.
And `evecrypto/placebo.py` is a genuine no-op pack:

```python
def get_random_bytes(n):  return '\x00' * n
def crypto_hash(*args):   return str(binascii.crc_hqx(blue.marshal.Save(args), 0))
def sign(data):           return (data, 0)
def verify(signed):       return (signed[0], signed[1] == 0)
class PlaceboCryptoContext:
    def Initialize(self, *a, **kw):            return {}
    def SymmetricEncryption(self, packet):     return packet   # identity
    def SymmetricDecryption(self, packet):     return packet   # identity
```

So selecting Placebo yields a plaintext stream, a CRC-based `crypto_hash`, and
signatures that verify on a literal `0`. `cryptoPack` is not part of the
handshake, so changing it trips no check.

**How to select it.** Editing `start.ini` would work, but `start.ini` is
covered by the signed `manifest.dat` (see `docs/archive/recon-v24.01.md`). Prefer the
**source override** instead: a loose `evecrypto/settings.py` shadowing the
archive copy, forcing `cryptoPack = 'Placebo'`. That keeps every shipped file
untouched and is the first load-bearing use of the override capability. It also
means the override mechanism is now on M1's critical path, not merely a
convenience.

## Handshake constants (confirmed, encoded in `eve-marshal::constants::handshake`)

| constant | value |
|---|---|
| `machoVersion` | **496** |
| `__HANDSHAKE_SECRET_KEY` | **170472** |
| `handShakePaddingLength` | 64 |
| `boot.version` | 24.01 — must marshal as a **real** (server check is type-strict) |
| `boot.build` | 3619879 |
| `codename@region` | `V24.01@ccp` |
| `sessionType` | 5 |

The handshake tuple the client exchanges is
`(SECRET_KEY, macho.version, loggedOnUserCount, boot.version, boot.build,
codename + "@" + region, updateInfo)`.

**Version-check failure order** (client-side): region string, codename string,
`boot.version` float, `machoVersion` int, then `boot.build < serverBuild`. A
server reporting an *equal or lower* build is accepted.

## Corrections to this file's earlier content

1. **Login progress has three steps, not four.** `connecting` (1/3) →
   `authenticating` (2/3) → `done` (3/3). `loginprogress::lowlevelversioncheck`
   exists only as a label key and is never scattered in this build.
2. **`charUnboundMgr.GetNumCharacters` and `GetCharacterInfo` do not exist**
   anywhere in the tree. The character-select screen is driven by
   **`charUnboundMgr.GetCharacterSelectionData()`**, which returns a 4-tuple
   `(userDetails, trainingDetails, characterDetails, wars)`.
3. **`authentication.AccruedTime` / `AmUnderage` are not called** on this
   region — they are gated on `boot.region == 'optic'` (China). Ours is `ccp`.

## Session establishment

**The handshake does not populate the session.** The client's
`_AssociateWithSession()` runs with `packet=None`, installing only `clientID=0`
and a lock. The server must then push either a
**SessionInitialStateNotification** (command 18; `initialstate` is a *flat*
`{attr: value}` map; `sessionType` must be 5) or a SessionChangeNotification
(command 16; `change = (flag, {attr: (old, new)})`). Prefer 18.

**Minimum session to reach character select:** `userid`, `role`,
`sessionType = 5`, `languageID`, and **`charid` absent** — `gameui`'s session
handler routes on `session.charid is None`.

**Role mask:** must include `ROLE_LOGIN | ROLE_PLAYER`; must **never** include
`ROLE_SERVICE` (the client raises, and the session is poisoned rather than
degraded — every *later* session change fails, including character selection).

> **Correction. This section recommended `ROLE_ANY & ~ROLE_SERVICE`, and that
> mask switches off the exact feature this project exists for.**
>
> `ROLE_ANY` is not "every role". It is all-ones minus `ROLE_IGB` and the
> `ROLE_EXT_GM*_PLUS` masks, and
> `ROLE_EXT_GM4_PLUS = ROLE_EXT_GM4 | ROLE_GML | ROLE_GMH` propagates into all
> three others. So `ROLE_ANY = 0xFF9C3FFF7FFFFFFF` has **both GM bits clear**,
> verified arithmetically against the constants read out of
> `carbon/common/script/sys/serviceConst.py`:
>
> ```text
> ROLE_GML = 0x0040000000000000   ROLE_ANY & ROLE_GML == 0
> ROLE_GMH = 0x0020000000000000   ROLE_ANY & ROLE_GMH == 0
> ```
>
> `ROLE_GML` is the client's *dominant* GM gate — 153 test sites, more than
> any other role. The tempting mask costs the "GM / WM Extras" submenu on
> every right-click menu and six of the Insider window's nine menus. It also
> sets bit 63, so it cannot be marshalled as an `int64` at all.
>
> The claim that the sandbox UI "gates on `ROLEMASK_ELEVATEDPLAYER` and
> `ROLE_QA`" was also incomplete in a way that matters: `ROLEMASK_ELEVATEDPLAYER`
> inherits `ROLE_ANY`'s exclusions, so it does not contain the GM bits either.
> The Insider **window** opens on an any-of test against it, while its **menu
> bar** is gated on `ROLE_GML`/`ROLE_WORLDMOD`. Set only the GM bits and
> Insider never opens; set only the elevated bits and it opens empty. Both are
> needed.

**Use this instead**, implemented as `eve_server::roles::SANDBOX_SAFE`:

```text
role = 0x6168005200E41000  = 7018860371459182592
     = ROLE_LOGIN | ROLE_PLAYER
     | ROLE_GML | ROLE_GMH | ROLE_GMS | ROLE_WORLDMOD
     | ROLE_SPAWN | ROLE_HEALSELF | ROLE_HEALOTHERS
     | ROLE_PROGRAMMER | ROLE_ADMIN
     | ROLE_TOURNAMENT | ROLE_LEGIONEER | ROLE_CHTADMINISTRATOR
```

Bit 63 clear, so it marshals as tag `0x03`: `00 10 e4 00 52 00 f8 61`.

`roles::SANDBOX` additionally sets `ROLE_QA | ROLE_CONTENT`. Hold those back
for now: **`ROLE_QA` is the one role that makes the client call something we
have not implemented** — `ccSvc.CreateCharacterWithDoll` →
`_QAPickStarterSystem` calls `charUnboundMgr.GetQAStarterSystemIDs()` with no
`try`/`except`, on every character creation.

Full role table, per-role reader counts, and what each one unlocks:
`docs/reference/session-roles.md`.

## Protocol traps

- Leave `oob` empty or `None`, and never set a `sessionVersion` attribute on a
  session packet, or the client's `LockSession` spins.
- Send notifications with `source = MachoAddress(nodeID=proxy_nodeid)` and
  `destination = MachoAddress(clientID=…)`.
- `config.GetBlackListedPlanets()` is **synchronous and blocks login** — it
  must be answered.
- Idle pings start at 60 s (`maxConnectionIdleTimeSeconds`); send
  `0` in `config_vals` to disable.

## Milestones

- **M0** — done: recon, override, jail, toolchain, marshal tag space.
- **M1** — character-select screen: the GPS handshake under Placebo, session
  notification 18, and the call sequence in `docs/reference/login-path.md`. Produces the first
  capture, which closes the remaining native gaps in `docs/archive/wire-format.md`.
- **M2** — docked in station. **M3** — undock (destiny).
