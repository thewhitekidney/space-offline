# Running EvEJS in a container, and pointing our client at it

A go/no-go for **executing** the archive, which
`docs/archive/evejs-261018-reference.md` §0.1 deliberately refused to assess. That
refusal was reasoned as "nothing is gained by running it". Something is gained
now — see §4 — so the question is live and this file answers it.

**Verdict: safe to run in a container with four changes. The blocker is not
security, it is the build delta.**

Contents: 1 what was checked, 2 the four changes, 3 the two trust questions,
4 why bother, 5 the plan, 6 what will probably break.

---

## 1. What was checked for execution specifically

The earlier audit covered the source. This pass covers **what happens when you
type `docker compose up`**, which is a different question.

| check | result |
|---|---|
| **npm lifecycle scripts** (`preinstall`/`postinstall`/`prepare`) — the classic install-time RCE | **none, in either `package.json`.** This is the single most important result on the page |
| runtime dependency count | **7**, all mainstream: `express`, `ws`, `better-sqlite3`, `protobufjs`, `node-forge`, `picocolors`, `yaml`. 3 at the root |
| install mode | `npm ci --omit=dev` — lockfile-pinned, no dev tree |
| rust build | `cargo build --locked` — lockfile enforced |
| container user | `USER node`, uid 1000. **Not root** |
| PID 1 | `tini -g` — correct signal and process-group handling |
| privilege | no `privileged`, no `cap_add`, no `/var/run/docker.sock`, no `network_mode: host`, no `pid:`/`ipc: host` |
| port publication | **every port bound to `127.0.0.1:` on the host.** Nothing reachable off-box |
| multi-stage hygiene | compilers (`g++`, `make`, `python3`) exist only in the dependency stage, not in the runtime image |
| first-run download | the **SDE from `developers.eveonline.com`** over HTTPS with `curl --fail --location`. CCP's own official static-data endpoint, legitimately obtainable. No checksum, but it is HTTPS to the vendor |
| web gateway (audit finding S7) | **closed by the container config, not by code.** `EVEJS_WEB_GATEWAY_TOKEN` defaults empty, and their own comment is right: the token-less path requires a peer socket address of `127.0.0.1`, which a container peer never is. Credit where due |

Combined with the earlier finding of **no backdoor and no server-side RCE**,
the execution risk is ordinary third-party-software risk, not elevated.

## 2. The four changes to make before `up`

1. **Pin the base images by digest.** `rust:1.95-bookworm` and
   `node:24-bookworm-slim` are mutable tags. Resolve once, pin, record.
2. **Accept or remove `better-sqlite3`'s prebuilt binary.** `npm ci` pulls
   `prebuild-install`, which fetches a prebuilt `.node` from GitHub Releases
   **outside lockfile integrity** — the one real supply-chain hole, and it
   executes at build time. Either accept it knowingly or force a source build
   (`npm_config_build_from_source=true`), which is why `g++`/`make`/`python3`
   are in that stage anyway.
3. **Drop the writable host bind mounts that are not needed — which is one of
   three, not all three.** An earlier draft of this list said all of them, and
   that was a generic hardening reflex rather than an answer. What matters
   about a bind mount is *which host path it exposes*, not that it is
   writable, and all three point inside the archive's own extraction
   directory. Per mount:
   - `./config` — **keep.** The server seeds config on first run and we need
     to edit it; in the image, every config change means a rebuild.
   - `./_local/certs` — **keep, writable.** The server *creates* its CA there,
     and we need to read it back out to trust it in the Proton prefix if the
     HTTPS endpoints are ever wanted.
   - `.../image/generated/Alliance` — **dropped.** ConfigEditor alliance-logo
     import, which we do not use.
4. **Do not attach anything else to `evejs-net`.** Auth is off by default
   (`config/server.json`: `devAutoCreateAccounts: true`,
   `devSkipPasswordValidation: true`) and every listener binds `0.0.0.0`
   inside the container, so **any container sharing that bridge is a GM**.
   The loopback-only publishing is what contains this; the bridge is not.

Also note: `127.0.0.1:443:26003` takes host port **443**. And the image is
built with `COPY . .`, so it contains the archive's ~505 MB of CCP-derived
static data — **never push that image anywhere.**

## 3. The two trust questions are different

Worth separating, because the answers differ.

**Is their server safe to run on this machine?** Yes, per §1–2. Non-root,
unprivileged, loopback-only, no RCE found, no install hooks.

**Is it safe to point our client at their server?** Different question, and
the honest answer is *only because of the jail*. Audit finding **S3**: the
server pushes **unsigned Python to the client** at handshake, with the
verification flag `false`, and the client `eval`s it. That is inherent to the
`signedFunc` design — our own server does the same thing
(`eve-server::pycode`) — and it means **a server owns every client that
connects to it**.

So this is not a reason not to test. It is the reason the test happens inside
`docs/guides/jail.md`: a network namespace with no default route, default-deny egress,
and a host forward-drop. Their code can run in our client process and still
reach nothing. **Do not run this test outside the jail**, and do not point a
client at their server from a machine you care about.

## 4. Why bother — the part that makes this worth doing

Not curiosity. `docs/re/destiny-ball-crosscheck.md` §6 leaves three things
unknown about the ball record, and names the experiment that settles them: get
a **byte-exact reference blob** from a real serialiser.

A running EvEJS that undocks a ship produces exactly that, **on the wire, in
plaintext, on port 26000**. Our jail's veth is ours, so `tcpdump` on it plus
our own `eve-marshal` decodes the whole conversation. That is:

- the reference blob, obtained **black-box** — observation of a protocol, with
  zero contact with their source and no licence question of any kind; and
- a wire-level reference for subsystems we have not built, which is a far
  better artifact than their code, because it is *what our client accepts*
  rather than what their server guessed.

This is the cleanest possible route to the thing we are blocked on, and it is
cleaner than reading their encoder was.

## 5. The plan

**Phase 0 — build.** Apply §2, then `tools/evejs-up.sh`, which builds nothing
itself — `DOCKER_BUILDKIT=0 docker build -t evejs-local .` from the extraction
directory does that — and then runs init, market and server.

**Three host quirks worth knowing, all found the hard way:**

- `docker compose` **does not exist here** and `docker-compose` is the v2.11.2
  standalone, which speaks Docker API **1.42** while the daemon requires
  **≥ 1.44**. Installing the compose plugin would break the project's
  no-local-installs rule, so `tools/evejs-up.sh` is their `compose.yaml`
  translated to plain `docker run`. Keep the two in sync by hand.
- There is **no `buildx` plugin**, so builds use the classic builder
  (`DOCKER_BUILDKIT=0`). Their Dockerfile declares `# syntax=docker/dockerfile:1.7`
  but uses no BuildKit-only feature, so the classic builder handles it.
- `~/.docker/buildx` is **root-owned** (a 2022 `sudo docker` accident) and
  breaks anything that wants to write there. Point `DOCKER_CONFIG` somewhere
  writable rather than chowning the user's home.

Also: their `compose.yaml` uses nested `${VAR:-${OTHER:-default}}` in
`EVEJS_IMAGE_SERVER_URL` and `EVEJS_MICROSERVICES_PUBLIC_URL`, which older
compose cannot parse. Flattened to literals in the extracted copy.

**Phase 1 — reachability.** Their ports publish to `127.0.0.1`, which is
unreachable from inside our netns — `127.0.0.1` there is the namespace's own
loopback. Republish on the jail's host-side address instead:

```
ports: ["10.137.0.1:26000:26000"]      # was 127.0.0.1:26000:26000
```

and widen the jail's allow-list, which is already parameterised:

```sh
EVE_JAIL_PORTS=26000,26001,26002,26400 tools/jail-setup.sh
```

**Phase 1a — two things that bite here, both found by a failed launch.**

*`getaddrinfo failed` in the client.* The jail's client resolves names through
**our own `eve-dns`** on `10.137.0.1:53`, and that is part of our stack, not
theirs. Running EvEJS instead of our server leaves nothing on :53, so the
client never gets as far as TCP. `eve-dns` is independent of the game server
and can run alone — it needs root only because 53 is privileged:

```sh
sudo server/target/debug/eve-dns --bind 10.137.0.1:53 --target 10.137.0.1
```

*A container cannot be reached at `10.137.0.1` at all.* This one is structural
and worth understanding rather than working around blindly. Publishing a
container port means **DNAT to the bridge subnet** (here `172.23.0.0/16`),
which is a routing decision, which is **FORWARD** — and the jail's third layer
drops exactly that:

```
chain forward { iifname "eve-host" drop; oifname "eve-host" drop; ... }
```

Our own server never meets that chain, because a host process listening on
`10.137.0.1` terminates its traffic in INPUT. A container does meet it, and
the connection dies silently.

**Do not open the forward chain for this.** It is the layer that stops the
namespace routing out *through* the host, which matters precisely because
`ip_forward=1` is already on for Docker and libvirt. Weakening it permanently
to run one test is the wrong trade.

`tools/jail-forward.py` does it the other way: terminate in userspace on the
host (INPUT, exactly like our server), then make a **fresh loopback
connection** to the container's published port. The forward chain is never
involved, the jail is untouched, and no root is needed because every port is
above 1024.

```sh
EVEJS_BIND=127.0.0.1 tools/evejs-up.sh          # container on loopback
tools/jail-forward.py 26000 26001 26002         # jail address -> loopback
```

**Phase 2 — machoNet only, no TLS.** Point the client at their **26000** and
nothing else. Skip 26003/443, 26002 and 5222 on the first attempt: their
server rebuilds its CA against a fixed SAN allowlist plus `127.0.0.1` every
boot, and getting our Proton prefix to trust that is work with no payoff until
the game protocol itself is shown to work. We already know from our own
captures that an absent gateway costs 6 tracebacks and blocks nothing.

**Phase 3 — capture.** `tcpdump -i eve-host -w` on the veth, then decode with
`eve-marshal`. Target: a `DoDestinyUpdate` carrying a `SetState` blob.

**Do not use their client patcher.** `Set-EvEJSClientNetworkPatches.ps1` gates
on an **exact SHA-256** of *their* client build and byte-patches three
endpoints at **fixed string length** (≤9 chars of host after `https://`). Our
client is a different build, so the hash check fails by construction — and
`10.137.0.1` is ten characters, one over their limit, so it could not be
expressed even if it passed. Our DNS jail already does this job properly.

## 6. What will probably break, in order of likelihood

**1. The build delta — and it is not the handshake.** Their SDE build is
**3396210**; ours is **3619879**. The obvious failure would be a version
gate, and *there isn't one*: `handshake.js` reads
`[birthday, machoVer, _, versionNum, buildVer, projectVer]`, compares birthday
and machoVersion, and on mismatch calls **`log.warn` and continues** —
"Version accepted". `buildVer` is only logged, at debug level. **Our client
will get through their front door.** What breaks will be downstream, in
service payloads shaped for a client three revisions older, and it will break
as wrong-shaped data rather than as a clean refusal.

**2. Static-data divergence.** Their universe is built from the 3396210 SDE.
Type, station and system ids our client knows may not match theirs.

**3. TLS and the image/CDN endpoints** — skipped in phase 2 by design, and the
reason phase 2 exists.

**4. Nothing, for a while.** Auth is off and accounts auto-create as GM, so
there is no credential step to get wrong. Login is the *easy* part here.

A useful outcome does not require the client to undock. A capture of their
session-change wave against our build is worth having on its own.
