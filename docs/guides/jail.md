# The jail

The modified client never gets a path to CCP. Not "is configured not to phone
home" — has no route to anywhere.

## What the jail protects, in one paragraph

This is the paragraph to read before you play, and the one the friend page
quotes word for word:

> **The jail stops the client from reaching anything but this server. It does
> not protect your files from this server.** The server can run any Python it
> likes inside the client. That code runs as you, inside the jail: it can read
> and write everything the jail lets the client see, which includes the Wine
> prefix the client lives in, and it can send keystrokes and clicks to other
> windows on your X display. Only connect to a server whose owner you trust,
> and only over the tunnel.

The rest of this document is the detail behind those sentences:

- what blocks the network: [Design](#design);
- what the client can see on disk: [The client's view is an
  allowlist](#the-clients-view-is-an-allowlist);
- how the server runs code in the client: [The server can run code in the
  client](#the-server-can-run-code-in-the-client).

## Design

The client runs in its own network namespace (`eveoffline`) joined to the host
by a veth pair. The namespace has **no default route**. The only address that
exists on the other end is the host side of the veth, and nftables inside the
namespace restricts even that to the ports our own server listens on.

```
  namespace eveoffline                    host
  ┌──────────────────────────┐
  │ exefile.exe (via Proton) │
  │ eve-ns   10.137.0.2/30 ──┼── eve-host  10.137.0.1/30
  │                          │      ├─ 26000/tcp  machoNet
  │ no default route         │      ├─ 5222/tcp   in-game chat (XMPP)
  │ no global IPv6           │      └─ 53/udp+tcp stub resolver
  │ output policy: drop      │
  └──────────────────────────┘      forwarding for 10.137.0.0/30: dropped
```

Three independent layers, so a mistake in one does not open a path:

1. **No route.** Nothing outside `10.137.0.0/30` is routable. This alone is
   sufficient; the rest is belt and braces.
2. **Default-deny egress** inside the namespace — `policy drop` on output, with
   an allow-list of `10.137.0.1` on the game ports, DNS, and ICMP echo.
3. **Forwarding refused on the host.** This one matters: the host almost
   certainly has `ip_forward=1` because Docker and libvirt are installed, so
   without an explicit drop the namespace could route out *through* the host.
   `jail-setup.sh` installs an `inet eve_offline_jail` table at forward
   priority −300 — ahead of Docker's rules — that drops anything touching the
   jail subnet or the veth.

Graphics, audio and input are unix sockets and file descriptors, so a network
namespace does not touch them. That is why X11/Wayland, PipeWire and the DRI
device work inside the jail. It is also a hole: every *other* pathname socket
crosses the namespace too. Before IP14, the client could reach the session
D-Bus (and through it a running host browser), systemd-resolved and avahi
(V6 F1). The network layers above do not close that. The mount namespace
does: see [The client's view is an allowlist](#the-clients-view-is-an-allowlist).

## Server redirection without touching a client file

`manifest.dat` is signed and covers `start.ini`, so editing the server address
there trips the integrity check (enforcement unknown, but why find out). Instead
the jail's `resolv.conf` points at a stub resolver on `10.137.0.1` which answers
only for the EVE hostnames — `*.servers.eveonline.com` → our server — and
NXDOMAINs everything else.

`serverInfo._GetServerIP` builds `"{}.servers.eveonline.com".format(serverName)`
and resolves it, so a stub answer is all it takes. No client file is modified,
and DNS is simultaneously proof of isolation: if a name outside our stub ever
resolves, the jail is broken.

### The resolver exists now, and the hostname is confirmed

`eve-dns` (`server/crates/eve-dns`) is the stub: std-only, zero dependencies,
UDP and TCP, answering `*.servers.eveonline.com` and NXDOMAIN for everything
else. `docs/reference/server.md` has none of it; it is small enough to read.

The hostname was confirmed from the client's own bytecode rather than assumed.
`serverInfo._GetServerIP` formats `'{}.servers.eveonline.com'` with the server
name for every region except China's `optic`/`serenity`, which get
`{}.evepc.163.com` and `servers-{}.evepc.163.com`. `GetServerIP` looks the name
up in `SERVERS` lowercased, and a stock `start.ini` says `server = Tranquility`.
So the one name that has to resolve is:

```
tranquility.servers.eveonline.com  ->  10.137.0.1
```

Verified end to end against `dig` — a client implementation we did not write —
with the resolver running on a high port:

| query | result |
|---|---|
| `tranquility.servers.eveonline.com A` | `10.137.0.1`, over UDP and over TCP |
| `servers.eveonline.com A` (the apex) | `10.137.0.1` |
| `tranquility.servers.eveonline.com AAAA` | NOERROR, 0 answers |
| `tranquility.servers.eveonline.com MX` | NOERROR, 0 answers |
| `www.google.com A` | NXDOMAIN |
| `notservers.eveonline.com A` | NXDOMAIN |

That last row is the one worth having: matching is label-wise, so a name that
merely *ends* in the suffix as text does not match.

### On naming the server "Singularity"

It does nothing. `IsLiveServer` is referenced by no module but `serverInfo`
itself, and no client module references `IsTestServer` or `singularity` at all.
The Singularity sandbox was **server-side** — the dev and GM tools came from
the role mask the server granted, not from a flag the client reads. So there is
nothing to unlock here; it has to come from us.

## One client file does have to change

`cryptoPack`. A stock install says `CryptoAPI`, under which every packet after
the handshake is encrypted against CCP's public key and `GPS.__Execute` demands
a signature we cannot produce. `Placebo` makes `EncryptedRead`/`EncryptedWrite`
the identity (`docs/reference/login-path.md` §3). There is no DNS-style trick for this
one: the alternative is patching `blue.dll`.

**This sits awkwardly with the section above**, which avoids editing `start.ini`
because `manifest.dat` is signed and covers it. Both things are true, and the
honest statement is: we avoid editing it where there is an alternative, and for
`cryptoPack` there is not. Enforcement of the manifest check is still unknown —
if it bites, the first symptom will be at launch, long before any traffic.

**Do not edit `start.ini` by hand** — it is covered by the manifest (below), so
a hand edit makes the client refuse to start. Use `tools/client-prepare.sh`,
which sets `cryptoPack = Placebo` *and* updates the manifest hash to match, on
the copy, reversibly.

### The manifest does hash `start.ini`, and we now know exactly how

`manifest.dat` (19,089 bytes, at the install root) is a flat table:

```text
u32 version = 4
u32 count   = 220
u32 0
repeated:  u32 nameLen, name, u32 hashLen = 32, 32-byte hash, u32 flag
...then a trailer, which is the signature
```

Entry 1 is `app:\start.ini` with hash
`d30eaa39…6bf439e6`, and that is the **SHA-256 of the stock file** — confirmed
by hashing it. Our `Placebo` copy hashes to `a1d7a658…66761166`, so it does not
match and cannot be made to match without resigning the manifest.

**But there may be nothing that checks it.** The only manifest strings in
`blue.dll` are four branches of `VerifyManifestFile`, and **every one of them is
an I/O error**:

```text
VerifyManifestFile: Couldn't open manifest file <path>
VerifyManifestFile: Could not create IResFilePtr instance
VerifyManifestFile: Read problem
VerifyManifestFile: fileSize < 0
```

There is **no "hash mismatch" string anywhere in the binary** — the only
hash-related strings are OpenSSL's own. So either a mismatch is reported
through some generic path, or the manifest is consulted lazily per resource
and `start.ini`, which is read very early, never goes through it. Unresolved,
and cheap to settle by running the client.

It **is** enforced: the controlled experiment settled it. A copy with stock
`start.ini` launches; the same copy with `cryptoPack` hand-edited shows the
dialog. So the manifest hash of `start.ini` is checked.

`tools/client-prepare.sh` is the answer that touches the least. It does not
disable the check or modify any binary — it recomputes the SHA-256 of the
edited `start.ini` and rewrites the one manifest entry for `app:\start.ini`
(exactly 32 bytes, verified contiguous), so the manifest tells the truth about
the owner's own change. `--check` reports state, `--restore` undoes it byte for
byte, and it refuses the live install.

One question remains, and only a launch answers it: **is `manifest.dat` itself
signature-checked?** The manifest ends in a ~256-byte trailer that may be an
RSA signature over the table. If it is not verified, the re-hashed manifest is
accepted and this is the whole fix. If it is, the client still refuses, and the
next step is to set `cryptoPack` another way (the Python source-override route,
`docs/archive/recon-v24.01.md` Round 3, which touches no manifest-covered file) rather than
to defeat the signature. The tool says this plainly rather than claiming the
question is closed.

We did look for a way to override it from the command line instead. `boot` is
native, and neither `blue.dll` nor `exefile.exe` contains the strings
`start.ini` or `cryptoPack` in ASCII or UTF-16, so where the file is parsed was
not found and **whether an argv override exists is unresolved**. Editing the
file is the known-good path.

## Use

```sh
sudo -v && tools/up.sh         # build, jail, resolver, server, client
sudo -v && tools/up.sh --no-client   # services only
```

`up.sh` tears the whole stack down on exit, including the jail, unless `--keep`.
It refuses to continue if a service dies on startup, and — when `dig` is
present — refuses to launch a client until it has seen the resolver answer
correctly.

The pieces, if you would rather drive them yourself:

```sh
sudo tools/jail-setup.sh      # once per boot
sudo tools/jail-verify.sh     # assert no egress; non-zero exit if leaking
tools/run-client.sh           # launches only if verify passes
sudo tools/jail-teardown.sh   # remove
```

Services run on the **host** end of the veth pair, not inside the namespace:
the namespace has no default route, so the host end is the only address the
client can reach, and nftables narrows that to ports 26000, 5222 and 53.

`run-client.sh` runs `jail-verify.sh` itself and refuses to launch on failure,
so the client cannot be started with a live network path by accident. It passes
`/ssoToken=` to skip the launcher.

## `proton run` returns before the client exits

A failure worth naming, because it reads as a server bug and is not one:

> unable to connect to server

`run-client.sh` used to end with `exec ... "$PROTON" run exefile.exe`. But
`proton run` returns as soon as it has handed the executable to `wineserver`
— the game is still running when that command finishes. `up.sh` treats
`run-client.sh` returning as "the session is over", so its `cleanup` trap
killed `eve-server` and `eve-dns` and ran `jail-teardown.sh`, while the client
was still playing. The client then had no resolver, no server, and no veth,
and said it could not connect.

The symptom is distinctive once you know it: `/var/run/netns/` is empty, no
`eve-server` process is running, and yet an `exefile.exe` is still alive and
still appending to its log. The server log stops after the first couple of
connections because there was nothing left to connect to.

Two changes:

- `run-client.sh` no longer `exec`s. It launches, then polls for the client's
  own process and only returns once that is gone. The pattern it matches is
  built from the client directory name (`tq-offline`), so it waits for *our*
  copy and ignores a real EVE the owner may have open.
- `run-client.sh` also refuses to start if such a client is already running,
  and `up.sh`'s cleanup warns if one somehow outlives it rather than quietly
  pulling the network out from under it.

If you hit this before the fix, the client left behind is unrecoverable —
closing it is the only option, since the namespace it was launched into no
longer exists.

## The host firewall has to let the jail reach our services

This is the subtlety that is easy to miss and bites per-machine. The services
(eve-dns, eve-server) run on the **host** end of the veth, at `10.137.0.1`.
When the jailed client sends a DNS query or a connection there, it arrives as
*input to the host* — and a default-deny host firewall drops it before our
services see it. The client then reports "could not connect" or fails to
resolve, while the services sit there working. On the machine this was first
run on, `ufw` was active and did exactly that.

`jail-setup.sh` detects an active `ufw` (or a raw-nftables input hook) and opens
exactly one hole: input **on the veth interface only** (`ufw allow in on
eve-host`, or an early-priority nft accept for that interface).
`jail-teardown.sh` removes it. If no host firewall filters input, nothing is
changed.

This does **not** weaken the jail. Egress control lives on the *namespace* side
— the output chain's allow-list plus the absence of any default route — and
that is untouched. The host accepting input on the veth only lets the jail
reach `10.137.0.1`; the namespace still cannot address anything else, which
`jail-verify.sh` confirms by failing to reach the host's real LAN address.

(If you would rather not have the setup touch the host firewall at all, the
alternative is to run both the services and the client *inside* the namespace
on loopback, with no veth — then only `lo` exists, egress is impossible by
construction, and no host firewall is involved. That is a cleaner end state and
is noted here as a future option; the current design keeps the services on the
host so their logs and captures land in the repo without a namespace hop.)

## Getting Placebo past the manifest check

The client refuses to start unless `start.ini` and `manifest.dat` match their
recorded hashes, and setting `cryptoPack = Placebo` changes `start.ini`. This
was established the hard way, by three launches of the copy:

| start.ini | manifest.dat | result |
|---|---|---|
| stock | stock | launches |
| Placebo | re-hashed so its entry is correct | fails |
| Placebo | stock | fails |

The second row is the decisive one: the manifest entry for `start.ini` was
corrected, and the client *still* refused — so editing the manifest breaks a
second, whole-manifest integrity check on top of the per-file hashes.
Re-hashing alone cannot work, and the manifest cannot be re-signed without
CCP's key.

So the owner-authorised client modification is to neutralise that self-check on
the **copy's** `blue.dll`. `tools/client-patch-manifest.sh` does it, surgically:
it locates the check by the stable `root:/manifest.dat` string reference (not a
fixed offset), confirms the exact instruction, and rewrites one 4-byte
instruction right after the verify call —

```text
call   VerifyManifestFile        ; verify root:/manifest.dat
mov    rbx, [rax+0x10]           ; rbx = error length   48 8b 58 10
                                 ;   ->  xor rbx,rbx;nop 48 31 db 90
...
test   rbx, rbx
je     success                   ; now always taken
```

— so the error length is forced to zero and the success branch is always taken.
Nothing else in the binary changes. It backs `blue.dll` up first, verifies the
result, has `--check` and `--restore`, refuses the live install, and **refuses
rather than guesses** if the surrounding instructions are not exactly what this
build has — a different build fails safe instead of being corrupted.

Then set the crypto pack with the manifest left stock:

```sh
tools/client-patch-manifest.sh      # neutralise the check (copy's blue.dll)
tools/client-prepare.sh --no-rehash # cryptoPack = Placebo, manifest untouched
```

Both are reversible with `--restore`, and the live install is never touched.
Whether a further per-resource check catches `start.ini` later is not yet
known; if it does, it is the next thing to neutralise, and the first client
launch after this says so.

## The build is pinned, and now it is checked

`tools/pin-client.sh` records a sha256 of every file in the jailed install and
verifies it. **Why it matters more than it looks:** every `[V]` mark in this
repository is measured against the pinned build — **3662168** since the re-pin
of 2026-10-31 (issue #19; it was 3619879 before, and 3619879 stays on disk as
`tq-3619879`). Every constant read out of CCP's bytecode, every wire layout,
every formula is anchored to it. Steam moved its own copy once already (to
3628513), and until 2026-10-24 nothing noticed that the two had diverged. The
real build of 2026-11-09 will repeat the re-pin.

```sh
tools/pin-client.sh            # verify (232 files, ~1.2 s)
tools/pin-client.sh --quick    # 10 load-bearing files only
tools/pin-client.sh --write    # re-record, after a deliberate change
```

Falsified both ways when it was written: pointed at `tq` it reports
`BUILD MOVED -- manifest pins 3619879, install says 3628513` and exits 1 (the
numbers are those of the day it was written; the manifest pins 3662168 now);
pointed at the second prefix it passes, because **`tq-offline-2` is
byte-identical to `tq-offline` across all 232 files**, so one manifest covers
both clients (after a re-pin, rebuild the second client with `client2-setup.sh`).

Two things a failure here means, and the second is easy to miss. The obvious
one is that the RE corpus no longer describes what we run. The other is that
our own patching is gone: `start.ini` and `manifest.dat` are modified in
place with `.eveoffline-orig` backups beside them, so whatever replaced the
build also reverted the `cryptoPack` work of the section above.

## We run a copy, never the install you play on

`EVE_CLIENT` defaults to `.../CCP/EVE/tq-offline` inside the offline
client's own prefix (next section), a copy of `tq` that sits beside that
prefix's `ResFiles` — so `../ResFiles` resolves exactly as the client
expects and no drive mapping changes. 392 MB. `run-client.sh` **refuses** to launch if `EVE_CLIENT`
points at `.../tq`, unless `EVE_OFFLINE_ALLOW_REAL_CLIENT=1`.

Two reasons, and the second is the one that matters:

1. We modify the client — `cryptoPack` now, possibly loose `.py` overrides or a
   patched `blue.dll` later. Doing that to a live install means Steam can
   revalidate and silently revert it in the middle of a debugging session.
2. **If the jail ever leaked, whatever phoned home would be the client sitting
   next to your real EVE state** — settings, cache, and whatever the prefix
   holds. A copy keeps the game directories apart.

Only the copy has `cryptoPack = Placebo`. The real install is left stock.

Making the copy by hand is `cp -a .../CCP/EVE/tq .../CCP/EVE/tq-offline` inside
the online prefix, with EVE closed, before `tools/offline-prefix.sh`. A friend
does not do that by hand: `tools/friend-setup.sh copy` does it and then runs
`offline-prefix.sh --link-resfiles`. The prefix copy leaves out the launcher's
login state (`AppData/Roaming/EVE Online`, `AppData/Local/CCP/EVE/Launcher`):
the prefix is read-write for a client the server can drive.

## The client has its own Wine prefix (O19)

**A copy of the game directory is not a separate prefix.** Until O19,
`tq-offline` sat inside prefix 8500, the Wine prefix the owner's online,
Steam-launched EVE uses, and the jail bound that prefix in read-write. So a
compromised jailed client, or a hostile server driving it (the server can run
Python in the client: [below](#the-server-can-run-code-in-the-client)), could
plant a DLL in the prefix that the online client loads later, outside the
jail (IP14 lane note; ruling 14). Owner decision **O19** (tracker §81): the
offline client gets a prefix of its own.

**Where.** `~/.local/share/eve-offline/prefix`, decided in one place,
`tools/prefix-lib.sh`. Outside Steam's `compatdata` on purpose: Steam's own
prefix management (cleanup on uninstall, "delete compatibility data", a
Proton upgrade) never reaches it. On the same filesystem as the Steam library,
so ResFiles can be hard-linked (below).

**Made once**, by `tools/offline-prefix.sh`, as a copy of 8500:

```sh
tools/offline-prefix.sh                  # a full copy
tools/offline-prefix.sh --link-resfiles  # the same, ResFiles hard-linked
tools/offline-prefix.sh --check          # what is there; changes nothing
```

- It copies everything in `compatdata/8500` (bottle, registry, Wine user
  profile, `tq-offline` with its `cryptoPack` and `blue.dll` patches, the
  client's settings) **except** the online installs `CCP/EVE/tq` and
  `CCP/EVE/sisi`, which the offline client never runs.
- It only reads 8500. Every write goes to `<dest>.partial`, which is renamed
  into place only once the copy is complete. It refuses while a Wine or Proton
  process is using 8500, refuses an existing copy unless given `--force`
  (and then only a copy it made itself, which it recognises by its
  `eve-offline-prefix.txt`), and refuses a copy with a symlink that leads
  back into 8500.
- **Size.** On one test machine (ext4, 2026-10-26) prefix 8500 is 57G, and
  48G of that is `ResFiles`. ext4 has no reflinks, so a full copy costs about
  56G. `--link-resfiles` hard-links the store instead and costs about 8G. On
  btrfs or xfs, `cp --reflink=auto` makes the full copy almost free.
- **Hard-linked ResFiles are the online store's own inodes**, so the jail
  must not be able to write them. It cannot: both jails bind
  `<client>/../ResFiles` **read-only on top of** the read-write prefix (a
  read-only entry inside a read-write one is no longer skipped as covered),
  and `jail-client.sh` refuses to launch if ResFiles is writable from inside.
  The second client has always run with a read-only ResFiles.

**Why an imported build is a real copy and its blobs are hard links.**
`tools/import-build.sh` (`docs/lanes/import-tool.md`) brings a later online
build into the offline prefix. The install directory is a real copy: the
client writes `cache:` inside it and the patcher rewrites its files, and a
hard-linked install would write through to the online one. The ResFiles blobs
are hard links, because the store is 48G on a disk that cannot hold two. Only
names matching `^[0-9a-f]{2}/[0-9a-f]{16}_[0-9a-f]{32}$` are linked, never over
an existing blob, each checked against the index (size, md5) first. The cost of
sharing an inode is that a launcher that rewrites a blob in place changes ours
too, so `--sweep` re-checks the md5 monthly. The tool never names the copy
`tq` or `sisi`, refuses while a Wine or Proton process uses 8500, and reads
8500 only.

**Every jailed launch path uses it.** `run-client.sh`, `up.sh`,
`client-ci.sh` (sudo and `--rootless`) and `client-headless.sh` default to
it (`EVE_PREFIX_ID=offline`); `--second` is still prefix 8501, which
`client2-setup.sh` now fills from the offline prefix and whose `ResFiles`
symlink it repoints there. The tools that read the client (`pin-client.sh`,
`client-prepare.sh`, `client-patch-manifest.sh`, `analysis.sh`,
`check-resources.sh`, `client-log.sh`, `evidence.sh`, `destiny-ref.sh`) point
at the copy too.

**The online prefix is invisible in the jail**, not even read-only. Four
places refuse it, each on its own:

| where | refuses |
|---|---|
| `run-client.sh`, `client-ci.sh`, `client-headless.sh` (`tools/prefix-lib.sh`) | a prefix, client or ResFiles under `*/steamapps/compatdata/8500` |
| the root helper's `bind_back` (`eve-offline-jail.in`) | any view entry that is, is inside, or **holds** the online prefix (a prefix of `~/.local/share/Steam` would bind it), checked after the path is resolved and before anything is mounted |
| `jail-rootless.sh`'s `bind_back` | the same, for the rootless CI jail |
| `jail-client.sh`, inside the jail | the same paths again, the online prefix being visible at all, and a writable ResFiles |

The root helper is the one that matters, since the launch-env it reads is
yours to write. It changed, so **reinstall it**: `sudo
tools/install-jail-helper.sh` (until then `install-jail-helper.sh --check`
says STALE, `run-client.sh` falls back to sudo with the current template, and
`client-ci.sh` refuses).

**Limits.** The "holds" check knows where your own Steam library is
(`~/.local/share/Steam`); a library elsewhere is caught by its
`steamapps/compatdata/8500` path only. `client-headless.sh smoke` runs a
client with no network but no mount jail either: it sees your whole home,
online prefix included, and is a display test with no server to drive it.
The old `tq-offline` inside 8500 is no longer used; removing it is safe and
yours to do.

## Why the egress probes aim at your own LAN

`jail-verify.sh` makes **real connection attempts** from inside the namespace —
you cannot prove a jail is sealed without trying to escape it. That makes the
choice of target a decision rather than a detail, because a probe that leaks is
a packet genuinely sent.

So every target is on this machine or this LAN: the default gateway, `docker0`,
the libvirt bridge, and the host's *other* addresses (the allowlist names
exactly one, so reaching any other means it is not holding). Plus a UDP probe
to `10.137.0.1:123`, which tests the **port allowlist** rather than the routing
table — a different failure mode.

This is also the stronger test. The host has `ip_forward=1` (docker, libvirt),
so the realistic leak is the namespace being routed out *through* the host onto
the LAN, which is exactly what these probe. The namespace has no default route
and no LAN route, so a public address and a LAN address fail in the same place.

An earlier version probed `1.1.1.1`, `8.8.8.8`, `example.com` and **Sentry's
address — which is CCP's crash-reporting endpoint**, so a leak there was a leak
to CCP. Replaced. The DNS checks are safe as they stand: they resolve through
our own stub, which has no upstream at all, so a query never leaves the machine
even when it names a CCP host.

## The client's view is an allowlist

Built by IP14 (`docs/lanes/ip14.md`) and put on both launch paths by IP15
(`docs/lanes/ip15.md`), under rulings 3, 4 and 11. Neither has yet run as
root; the human checks in those notes are the proof still owed.

**How it is built.** The helper's `client` verb runs `ip netns exec NS
unshare --mount --propagation private`, then `build_view`:

1. Each allowlist entry is vetted (absolute path, resolves, right type, owned
   by the caller or root) and staged before anything is hidden.
2. An empty tmpfs goes over `/run`, `/var/run`, `/tmp`, the caller's `$HOME`
   and the runtime dir.
3. Each entry is bound back at its own host path, `nosuid,nodev`, read-only
   where the table says so. `/dev/shm` is replaced with an empty one.

| entry | mode | why |
|---|---|---|
| `/run/udev` | ro | input devices (controllers) |
| the jail's `resolv.conf` | ro | DNS goes only to our stub |
| `$XDG_RUNTIME_DIR/pulse/native`, `pipewire-0` | rw | audio |
| `$XDG_RUNTIME_DIR/wayland-N` | rw | display |
| `$XAUTHORITY` | ro | X cookie |
| `/tmp/.X11-unix/X<N>` | rw | display |
| Proton's directory | ro | Proton |
| `STEAM_COMPAT_DATA_PATH` (the prefix) | rw | the offline client's own prefix, never 8500 (O19) |
| `EVE_CLIENT`, `../ResFiles` | rw, ro | the game; ResFiles read-only on top of the prefix |
| `tools/jail-client.sh`, `out/client-ci/launch-env` | ro | the shim and its settings |

**It refuses to launch** when:

- `scan_view` finds any socket in `/run`, `/tmp`, `/dev/shm`, `$HOME` or the
  runtime dir that is not on the list, or any of docker, the system bus,
  resolved, avahi, snapd, cups or libvirt;
- the client user, after `setpriv --clear-groups --no-new-privs`, still
  holds `docker`, `libvirt`, `sudo` or `lxd`, or `/run/docker.sock` exists.
  `jail-client.sh` repeats this check, so a stale helper is caught too.

`WINEDLLOVERRIDES=winebrowser.exe=d` stops the browser chain before it
starts.

**Both launch paths get the same view.** The non-helper branch of
`run-client.sh` generates the helper for this checkout and runs its own
`v_in_jail_client`, so there is no second allowlist to drift.
`install-jail-helper.sh --check` reports an installed helper that is stale
or belongs to another checkout, and `client-ci.sh` then refuses to run.

**What the allowlist does not stop:**

- **X11 input injection.** The client talks to your X server, so it can send
  keystrokes and clicks to any other X11 window, and remote-control an
  already-running X11 browser. Nothing here prevents that. Wayland-only
  sessions narrow it; this repo does not enforce one.
- **Files outside the masked trees.** The client can still read anything you
  can read outside `/run`, `/tmp`, `/dev/shm`, `$HOME` and the runtime dir:
  other disks, `/etc`, `/srv`.
- **A world-writable socket outside those trees** is not scanned. A one-off
  `sudo find / -xdev -type s -perm -o=w` shows whether any exists.
- ~~The prefix is shared with the online client~~ Fixed by O19: the client
  has [its own prefix](#the-client-has-its-own-wine-prefix-o19), and every
  jail refuses the online one.

## The server can run code in the client

*Corrected 2026-10-25 (V6 F4).* This section used to say "There is no code
execution", citing `debug.Eval` and `debug.Exec`. **That was wrong.** Those
two do disarm themselves on a client (`debugSvc.py:115`, `:153`), but the
server has two other ways to hand the client a code object that it runs,
and our own server uses both:

- **`charMgr.GetSettingsInfo`.** The client asks for it after login. It
  answers with a marshalled CPython code object, which the client installs
  as `SettingsInfo.__code__` and calls (`pycode.rs:173-190`,
  `service/character.rs`). Ours returns an empty dict. The client-CI
  trigger sends the same blob with `execfile(...)` added, which runs any
  script we choose (`pycode.rs:77-82`, `:302`).
- **Handshake H8 `signedFunc`.** The client runs it through `Crypto.Verify`,
  then `marshal.loads`, then `eval` in `GPS.__Execute`
  (`pycode.rs:1-12`, `docs/reference/login-path.md` §4). Under `cryptoPack = Placebo`
  the verify passes for anything.

So **a server, or anyone who can rewrite its traffic, can run arbitrary
Python in the client.** It runs as the client user, inside the jail. Under
Placebo the traffic is plaintext, so an on-path attacker can do this too
(V6 F5). That is why a friend reaches a server only through the SSH tunnel,
never over a raw internet path.

Besides that, `machoNet.HandleMessage` on a client dispatches any inbound
`CallReq` to the exported set: 103 classes declare `__exportedcalls__`, 81
non-empty, 608 methods (`findings/client-callable.tsv`). The client makes no
role check on calls from the server. That channel is bounded, but with
`GetSettingsInfo` available the bound no longer matters.

**What bounds a compromised server is therefore the jail, not the client.**
The code it runs can:

- reach nothing on the network but this server;
- read and write what [the allowlist](#the-clients-view-is-an-allowlist)
  shows it: the prefix, the game directory, audio and display;
- read what the allowlist does not mask;
- inject input through X11.

That is the plain statement at the top of this document.

## What the jail can and cannot record, and the census

The jail stops a phone-home; it does not see most of them. With no route,
`connect()` to anything outside `10.137.0.0/30`, and to any IPv6 address,
fails with ENETUNREACH **before** every netfilter hook, so an attempt at an
IP literal leaves no trace at all. The `counter` at the end of each jail
chain sees only traffic to `10.137.0.1` on a port we did not allow. And
there is no kernel log: nf_log is suppressed in every network namespace but
the init one (`net.netfilter.nf_log_all_netns=0`), so the rate-limited
`log` rules the chains used to carry could never write a line. They are
gone; `tools/jail-drops.sh` reads the counters and the resolver's log.
"Nothing in `eve-dns.log`" therefore means "no name was looked up", not
"nothing was attempted".

The census (`docs/guides/census.md`) is what records the rest: for the length of a
run it gives the namespace default routes (v4 and v6) into `census0`, a TUN
device read by a sink that opens no socket. A TUN device has no wire, so
the jail still reaches nothing; every attempt becomes a recorded packet.
The helper's verbs are `census-up`, `census-down`, `census-dump` and
`census-verify` (rootless: `jail-rootless.sh --census`). While a census is
up, `verify` fails by design (it asserts the /30 is the only route); it
runs before `census-up` and again after `census-down`, and must pass both
times. In census mode the client's view also gets the browser stand-ins
and the canary script (read-only) and the two census logs the client's side
appends to; the sink's and the proxy's own logs stay outside it.

## What is verified, and what is not

**Verified end to end, 2026-10-19.** `jail-setup.sh` and `jail-verify.sh` have
now actually run, as root, on a test machine. Every assertion passed.
This section used to describe a design; it now reports a result.

```text
== egress must be impossible (targets are all local to this machine/LAN)
  ok    blocked the LAN gateway (192.0.2.1:53)
  ok    blocked the LAN gateway (192.0.2.1:80)
  ok    blocked docker0 on this host (172.17.0.1:53)
  ok    blocked libvirt bridge (192.168.122.1:53)
== the host's OTHER addresses must be unreachable
  ok    blocked the host's LAN address (192.0.2.151:22)
  ok    blocked the host's LAN address (192.0.2.151:26000)
== udp egress must be impossible too
  ok    blocked udp 10.137.0.1:123 (only 53 is allowed)
== dns must not resolve anything outside our stub
  ok    cannot resolve sentry.io
  ok    cannot resolve clientresources.eveonline.com
== there must be no default route
  ok    no default route
== no global IPv6 address
  ok    no global IPv6

JAIL OK - no egress path found.
```

The host-address checks are the sharpest of these: the allowlist names exactly
one address, and the host has several, so `192.0.2.151:22` being unreachable
from inside while the namespace can still reach `10.137.0.1:26000` is the
allowlist doing precisely what it claims. The UDP check matters separately —
every TCP probe above fails at *routing*, while UDP to `10.137.0.1:123` is a
routable address on a non-allowlisted port, so it tests the nftables policy
instead.

What that run did **not** exercise: the jail with a client actually inside it
generating traffic. Egress was proven impossible with the namespace idle.

**Not covered:** the client cannot download resources it is missing, because
`clientresources.eveonline.com` is unreachable by design. The install must be
complete before it goes in the jail. Crash reporting (Sentry via `exefile.exe`,
and the Crashpad handler `eve_crashmon.exe`) is native and always on, and is
stopped by the jail rather than by patching — it will try, and fail, silently.
