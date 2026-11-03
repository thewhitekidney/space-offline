# Passwordless root for unattended client testing — a plan to review

> **Status note, 2026-10-28.** The "Status: proposal" line below is stale: the helper this document argues for is built (`tools/install-jail-helper.sh`, `tools/eve-offline-jail.in`) and cites §3-§5 as its threat model, so keep reading it as that. Whether the exact option the document recommends is the one installed is `[unverified]` here; the installed helper's own header is the authority on what it grants.

**Status: proposal. Nothing here is installed, and I have changed nothing on
the system.** This document exists to be argued with before anything happens.

**Audience: the project owner, reviewing a privilege change.** It leads with
the objection rather than the design, because the objection is the useful part.

---

## 0. The headline

> **The obvious version of this — a `NOPASSWD` list of the seven commands we
> actually run — is equivalent to giving the agent your root password.** Not
> approximately: exactly. Five of the six are either a script I can edit, a
> binary I rebuild on every run, or a command whose argument is "anything".
>
> There is a version that is genuinely narrow. It needs one root-owned helper
> that the unprivileged user cannot modify, and it is maybe 150 lines. This
> document describes it and states what it still leaves exposed.

---

## 1. What it would buy

Today every unattended run needs `sudo -v` typed by a human within the last
few minutes. That is the single remaining reason the client-CI loop is not
actually unattended — it is why the overnight round could not verify its own
work, and why four lanes' worth of harness, trigger, scenarios and display
ended the evening waiting on one password prompt.

With it: `tools/client-ci.sh run <scenario>` works from a timer, from a hook,
or from an agent at 3am, and a change that breaks the client gets caught by
the thing that broke it rather than by you, the next morning.

## 2. What actually needs root, exhaustively

Six operations, from `tools/up.sh`, `tools/run-client.sh` and
`tools/client-ci.sh`:

| # | operation | why root |
|---|---|---|
| 1 | `tools/jail-setup.sh` | creates a network namespace, a veth pair, `nft` rules, and one `ufw` rule on the host |
| 2 | `tools/jail-teardown.sh` | removes all of the above |
| 3 | `tools/jail-verify.sh` | runs probes *inside* the namespace (`ip netns exec`) |
| 4 | `eve-dns --bind 10.137.0.1:53` | binding a port below 1024 |
| 5 | `ip netns exec eveoffline …` to launch the client | entering a network namespace needs `CAP_SYS_ADMIN` |
| 6 | `kill <pid>` of a stranded `eve-dns` | it was started as root, so only root can stop it |

## 3. Why the obvious rule is not narrow at all

A `NOPASSWD` entry is only as narrow as the thing it points at. Checked on
this machine, today:

```
drwxrwxr-x  youruser  youruser   tools/
-rwxrwxr-x  youruser  youruser   tools/jail-setup.sh
-rwxr-xr-x  youruser  youruser   server/target/debug/eve-dns
```

- **#1, #2, #3 are shell scripts owned by `youruser` in a directory writable
  by `youruser`.** A rule that lets root run `tools/jail-setup.sh` without a
  password lets anything running as you append one line to that file and have
  root execute it. That includes me, and it includes any subagent I spawn.
  **This is full root with extra steps.**
- **#4 is a binary rebuilt from this repo on every `up.sh` run.** Root
  executing an artefact the unprivileged user regenerates at will is the same
  hole wearing a different hat.
- **#5 is literally "run an arbitrary command as root".** `sudo ip netns exec
  eveoffline /bin/sh` is a root shell. `sudo` *can* pin arguments, but
  `run-client.sh` builds a long, varying command line, so a pinned rule would
  either be brittle or wide enough to drive through.
- **#6, `sudo kill <pid>`, is the mildest** — it is denial of service rather
  than escalation — but an unconstrained pid still means any process on the
  machine.

None of this requires anybody to be malicious. It means the blast radius of a
**mistake** — mine, a subagent's, a bad `sed` — stops being "the repo" and
becomes "the machine". I have made two file-mangling mistakes in this session
alone; both were recoverable because everything I touched was in git.

## 4. The design that is actually narrow

**One root-owned helper, a fixed verb set, no arguments, and root never
executes anything the unprivileged user can write.**

```
/usr/local/sbin/eve-offline-jail      root:root  0755
```

`/usr/local/sbin` is already `root:root 0755` on this machine, so the path is
sound without creating anything new.

Verbs, all taking **no arguments at all** — which is what makes the sudoers
match exact:

| verb | what it does |
|---|---|
| `up` | creates the namespace, veth, `nft` rules, the one `ufw` rule |
| `down` | removes them, and stops anything it started |
| `verify` | the egress probes, inside the namespace |
| `preflight` | the mirror of `verify`: proves the one permitted path *in* works, from inside the namespace |
| `dns` | starts the stub resolver on `10.137.0.1:53` |
| `client` | enters the namespace and a private mount namespace, builds the allowlisted view (`docs/guides/jail.md`), then drops to the invoking user with `setpriv --clear-groups --no-new-privs` and runs one fixed command |
| `status` | prints what is up; useful and harmless |
| `census-up` | the census (`docs/guides/census.md`): `census0`, a TUN device owned by the invoking user, default routes (v4, v6) into it, the R2 counter table, one accept rule for `census0`, ports from 9 unprivileged and ping allowed in the namespace; then starts `tools/census_sink.py` and `tools/census_proxy.py` **as the invoking user** (`setpriv --clear-groups --no-new-privs`) |
| `census-down` | SIGTERM to those two, then undoes `census-up` exactly; `verify` passes again |
| `census-dump` | prints the R2 table as JSON |
| `census-verify` | asserts the census shape: links, routes, addresses, the recorders running as the user |

Four properties do the actual work:

1. **The jail logic lives in the helper, copied at install time.** It does
   **not** source, exec or read anything from the repo. The repo's
   `jail-setup.sh` stays as the human-facing copy; the helper is a snapshot,
   and changing the jail means reinstalling the helper deliberately — which is
   the point, because that step is where a human looks at the diff.
2. **`dns` does not run our binary as root.** The resolver needs exactly one
   capability — `CAP_NET_BIND_SERVICE` — so the helper starts it *as the
   invoking user* with that capability alone:
   `setpriv --reuid=… --regid=… --clear-groups
   --inh-caps=+net_bind_service --ambient-caps=+net_bind_service <path>`.
   Root never executes a user-writable file. If the binary is malicious it can
   bind a low port, and nothing else.
3. **`client` drops privileges in the same breath as it gains them.** Root is
   needed only to *enter* the namespace; the command then runs as the
   invoking user, inside a network with no route anywhere. The command is
   built by the helper from a fixed template, not taken from the caller.
4. **The helper refuses to run if its own trust chain is broken** — if it or
   any parent directory is writable by anyone but root, it exits non-zero with
   a message. Cheap, and it catches the install being quietly undone later.

### Why `preflight` is a verb and not a line in `up.sh`

`verify` proves nothing gets *out*. Something also has to prove the one
permitted path gets *in* — that the client will resolve
`tranquility.servers.eveonline.com` to us and reach `26000` — and that is only
meaningful asked from inside the namespace, which needs `CAP_SYS_ADMIN`.

`up.sh` used to ask with `sudo ip netns exec`. That works interactively and
fails in exactly the case the helper exists for: no cached timestamp, no tty.
The probe then returns nothing and `up.sh` reports *"resolved to nothing"* and
tears the stack down — blaming the resolver for its own failure. It cost two
unattended CI runs. The resolver was healthy both times and answered
`10.137.0.1` correctly when queried directly.

Two smaller lessons from the same incident, both now fixed in `up.sh`:

- A diagnostic must not need a privilege the failing path has already shown it
  does not have. `sudo -n cat … | sed` under `set -o pipefail` **exits the
  script** when sudo refuses, deleting the rest of the diagnosis — the useful
  part. The namespace's `resolv.conf` is world-readable while the jail is up,
  so it is read as the user, with sudo only as a fallback.
- `sudo -n -l <cmd> <args>` cannot be used to ask "is this verb granted?". A
  user with a blanket `(ALL : ALL) ALL` rule is reported as permitted for every
  argument, password requirement and all. `up.sh` classifies on the helper's
  own `PREFLIGHT OK` / `PREFLIGHT FAILED` sentinels instead.

### The sudoers entry

```sudoers
# /etc/sudoers.d/eve-offline   (mode 0440, root:root, validate with visudo -c)
youruser ALL=(root) NOPASSWD: /usr/local/sbin/eve-offline-jail up, \
                              /usr/local/sbin/eve-offline-jail down, \
                              /usr/local/sbin/eve-offline-jail verify, \
                              /usr/local/sbin/eve-offline-jail preflight, \
                              /usr/local/sbin/eve-offline-jail dns, \
                              /usr/local/sbin/eve-offline-jail client, \
                              /usr/local/sbin/eve-offline-jail status, \
                              /usr/local/sbin/eve-offline-jail census-up, \
                              /usr/local/sbin/eve-offline-jail census-down, \
                              /usr/local/sbin/eve-offline-jail census-dump, \
                              /usr/local/sbin/eve-offline-jail census-verify
```

Each line matches that command **with exactly that argument and no others**,
which is why the verbs take none. `sudo`'s `env_reset` is on by default and
the helper sets its own `PATH`.

### What changes in the repo

`up.sh`, `run-client.sh` and `client-ci.sh` call `sudo
/usr/local/sbin/eve-offline-jail <verb>` when the helper exists, and fall back
to today's `sudo tools/jail-setup.sh` when it does not — so a fresh clone on a
machine without the helper behaves exactly as it does now, and nothing in this
repo depends on a privilege that may not be there.

## 5. What this still grants, stated plainly

After the design above, anything running as `youruser` — including me, without
asking — can:

- **bring the jail up and down at any time**, which adds and removes one `ufw`
  rule and a set of `nft` rules on the host. Bounded and reversible, but it is
  a change to your firewall made without a prompt;
- **bind `10.137.0.1:53`**, shadowing DNS on that one address;
- **launch the EVE client inside the namespace**, with the allowlisted view
  of the filesystem and no supplementary groups (`docs/lanes/ip14.md`) —
  something you can already do without root;
- **stop the resolver** it started;
- **run a census** (CEN-2): give the jail's namespace default routes into a
  TUN device it owns, run two repo scripts there **as `youruser`** (the sink
  and the dead proxy; root only creates the device and starts `setpriv`),
  make ports from 9 up bindable and ping allowed inside that namespace, and
  take it all down again. A TUN device has no wire, so the namespace still
  reaches nothing; it is recorded instead of refused.

And the honest one, which is not technical:

> **It removes a prompt that currently makes me stop and ask.** Today, an
> action needing root is a point where you see what is about to happen. After
> this, six of them are not. The technical blast radius is small; the change
> in posture is not nothing, and it is the part worth thinking about rather
> than the `nft` rules.

What it does **not** grant: writing outside the jail's own config, installing
packages, touching other users, running any binary of mine as root, or
surviving a reboot (the jail is not persistent and the helper starts nothing
at boot).

**But the account already has root by another door** (V6 F13). The owner's
user is in the `docker` and `libvirt` groups, and both are root-equivalent:
`docker run -v /:/h ...` mounts the whole host into a container as root. So
for an agent running as `youruser`, this helper's narrowness is real but
does not bound what it can do; the groups already grant more. Whether to
leave those groups is the owner's call.

The same groups were a **client escape** on any launch path that kept them:
a client holding `docker` could reach `/run/docker.sock` and get root on the
host. Every launch path now drops supplementary groups with `setpriv
--clear-groups`, never `sudo -u` (ruling 3), and the client refuses to start
if it still holds `docker`, `libvirt`, `sudo` or `lxd`, or can see the docker
socket (IP14, IP15). The rootless jail cannot drop groups at all, so there
the sockets are masked instead and must fail to connect (ruling 11).

## 6. How to check the install is sound

After installing, these should all hold:

```sh
stat -c '%U %G %a' /usr/local/sbin /usr/local/sbin/eve-offline-jail   # root root 755
sudo -l | grep eve-offline-jail            # exactly the seven verbs, no others
sudo -n /usr/local/sbin/eve-offline-jail status    # works, no password
sudo -n /usr/local/sbin/eve-offline-jail up extra  # MUST refuse: unlisted command
sudo -n ip netns exec eveoffline /bin/sh           # MUST still ask for a password
sudo -n tools/jail-setup.sh                        # MUST still ask for a password
```

The last two are the ones that matter: they prove the old wide paths were not
left open beside the new narrow one.

## 7. The cheaper alternative, if you would rather not

**Set the jail up once per boot, interactively, and let runs reuse it.** You
type a password once a day; operations 1–4 and 6 disappear from the unattended
path entirely, because the namespace and the resolver are already there.

That leaves only #5 — entering the namespace to launch the client — which is
one `client` verb, one sudoers line, and the narrowest useful version of this
whole document. It is strictly less convenient and strictly safer, and if you
are at all unsure, it is the one to start with.

## 8. What I would like you to decide

1. **Whether to do this at all**, given §5's last paragraph.
2. **Full helper (§4) or the one-verb version (§7).**
3. If yes: I write `tools/eve-offline-jail.in` plus an installer that you run
   and read first. **I will not install it** — the standing rule is that I
   build images and never touch the host, and a privilege escalation is the
   last place to make an exception to it.
