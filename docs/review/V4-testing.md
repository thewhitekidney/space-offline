# V4: testing, tooling and the gate

Review lane V4, 2026-10-25, at `3f2741d`. Read-only. Brief: `docs/review/README.md` §2 and §3 (V4).

Measurements were taken in an exported copy at `~/code/eve-offline-lanes/V4/`
with `EVE_MAX_JOBS=4`, which also sets `RUST_TEST_THREADS=4`. The copy was
deleted afterwards. The rootless-jail prototype commands ran on the host as
`youruser`, with no sudo, and created nothing that outlives the process.

Tags: **[V]** verified (command run, or code read at the cited line);
**[I]** inferred; **unknown** where it is neither.

---

## 0. Summary for the synthesis lane

1. **Unattended client CI is not blocked on `sudo`.** §1's "blocked on
   `sudo`" is out of date. The root-owned helper and its sudoers entry have
   been installed since 2026-10-22/24, and six unattended runs passed on
   2026-10-23 (`docs/lanes/ci-harness.md` §9). **The real defects:**
   - the *installed* helper is a stale snapshot;
   - nothing detects that it is stale;
   - the jail lets the client reach the host's **unix sockets**, which are a
     way out that the network rules do not cover;
   - nothing *schedules* a client run.

   Each has a small fix (P1-P4).
2. **A rootless jail works on this host today** [V prototype]. It is a
   user+net+mount namespace whose only interface is `lo`. The server, the
   resolver and the client all run inside it. It needs no sudo, makes no
   firewall change, and has no veth. Its no-phone-home guarantee is
   structural (there is no interface to send through), not a matter of
   nft rules. Recommended as the **CI** path (P5). The helper stays for
   human play with friends, because a friend's client cannot reach a server
   inside a private namespace.
3. **The gate takes about 3.7 min, and 95% of that is 39 bot tests**
   [V]. The 1,760 other tests together take about 6 s. The six `eve-bot`
   integration binaries run **one after another** and wait in real game
   time. Merging them into one binary (P7) and removing an unconditional
   30 s sleep (P8) should bring the test step close to its longest single
   test [I].
4. **The scripts that guard every push have never had their failure paths
   tested.** That class of bug happened four times: tracker §48.5, §54.6,
   §64 and §65. A stub-driven `tools/test_gate.py` (P9) pins all four.
5. **The soak cannot catch the bug it found (§67) unless that bug kills the
   server.** It never checks the server's exit status and never measures
   memory [V]. See P11.

---

## 1. The test pyramid as it stands

| layer | where | count | gate wall (4 threads) | catches |
|---|---|---|---|---|
| unit | `src/**` `#[cfg(test)]`, 9 crates | 1,701 | about 3.4 s | logic, encoders, registry mutators |
| wire / loopback | `eve-server/tests/loopback.rs`, `replay.rs`, `eve-dns/tests/sockets.rs` | 59 | about 2 s | framing, handshake, dispatch, persistence |
| bot harness | `eve-bot/tests/*.rs` (6 binaries) | 39 (+1 `#[ignore]`) | **167 s** | multi-session behaviour in real time |
| soak | `tools/soak.sh` (manual) | 1 run | 20 min, not in the gate | leaks, panics under 50 bots |
| client CI | `tools/client-ci.sh` + `tools/client-ci/scenarios/` (8 scenarios) | 8 | 43 s each, not in the gate | what the real client does with our bytes |
| human | `docs/guides/human-checks.md` | 6 numbered + about 40 batched | a morning | what nobody can script yet |

Counts are [V]. The total is **1,799 passed, 2 ignored**
(`cargo test --workspace`, `test-run1`). Per-binary times are [V] from
the same run:

```
59.84s   7 tests  eve-bot/tests/mining_bots.rs
36.41s   2 tests  eve-bot/tests/escort_bots.rs
31.24s  19 tests  eve-bot/tests/handshake.rs
23.81s   2 tests  eve-bot/tests/jump.rs
 9.50s   2 tests  eve-bot/tests/npc.rs
 6.30s   7 tests  eve-bot/tests/verbs.rs
 3.35s 1333 tests eve-server lib
 1.76s  46 tests  eve-server/tests/loopback.rs
 (everything else < 0.3 s)
```

**The shape is sound.** The pyramid is wide at the base. Every
integration test binds `127.0.0.1:0` and reads the port back
(`eve-bot/tests/handshake.rs:15`, `eve-dns/tests/sockets.rs:3`), so no
two tests can collide on a fixed port [V]. The client-CI layer has an
honest flake record (6/6, with zero variance in wall time;
`docs/lanes/ci-harness.md:390-401`).

**What is wrong is cost and wiring, not coverage:**
- the bot layer dominates the gate (§3);
- the soak and client CI are not on any loop (§5, §6);
- the bot and wire fixtures are copied file by file (§4).

## 2. The gate and landing

**Measured gate steps** [V], in a cold copy with 4 jobs:

| step | time |
|---|---|
| python unit tests (41 tests) | < 1 s |
| `cargo fmt --check` | 2 s |
| clippy, cold | 14 s |
| test build, cold | 23 s |
| test run | 175 s |
| test run, warm repeat | 174 s (stable to 1 s, so it is wait-bound, not compile-bound) |
| clippy, warm | 1 s |

So the gate is about **3.6-3.7 min**, and the test *run* is the entire
problem. Compiling is not.

**G1 — the pre-push hook contradicts `land.sh --head`** [V].
- `tools/hooks/pre-push` is `exec tools/gate.sh`: the bare gate, in the
  **main** tree.
- `land.sh --head` exists precisely because the main tree is dirty
  during a lane round (`tools/land.sh:162-168`).
- With the hook installed, every `--head` landing would gate twice, and the
  second run would fail on the dirty tree.
- It is harmless today only because the hook is **not installed** (no
  `core.hooksPath`, no `.git/hooks/pre-push`) [V].
- Cost: the first person who runs `tools/install-hooks.sh` breaks
  landing.
- **Change:** make the hook accept a sha that `land.sh` has already gated.
  `land.sh` writes `out/landed/<sha>` on a pass, and the hook checks it and
  otherwise runs the gate. Or delete the hook and `install-hooks.sh`. **S**,
  behaviour-neutral for `land.sh`.

**G2 — the guard scripts' failure paths are untested** [V].
- `gate.sh`'s own comments record three failure-path bugs:
  - `;` vs `&&` (§48.5);
  - a pipeline's status hidden by `tail` (§54.6);
  - `grep -q` + SIGPIPE + `pipefail` failing a *green* gate
    (`tools/gate.sh:113-122`).
- The tracker adds §64 and §65: the red path printed nothing.
- Each was found in production. The gate's python stage runs
  `tools/test_*.py` (`tools/gate.sh:84-91`), but none of those tests
  exercises `gate.sh` or `land.sh`.
- **Change:** add `tools/test_gate.py`:
  - copy `tools/gate.sh` and `tools/land.sh` into a temp tree with a stub
    `tools/dev.sh` and a stub `check_deps.py`;
  - assert that:
    1. a test failure (stub exits 101) prints the tail and `land.sh`
       pushes nothing;
    2. "0 passed" is refused;
    3. a pass with output over 64 KiB is still a pass (the SIGPIPE case);
    4. `--dirty`, `--only` and `--without` never print `== gate passed`;
    5. `land.sh` refuses an exit-0 log with no pass line.
  - Use a bare local git remote as the push target.
- Stdlib `unittest`, so it rides the existing python stage. **S**,
  behaviour-neutral.

**G3 — no syntax check over `tools/*.sh`** [V].
- A typo in `client-ci.sh` or `up.sh` is found at the next unattended
  run, minutes in.
- **Change:** in the gate's python stage, run `bash -n` over every
  `tools/*.sh`. Do **not** add shellcheck: it is a new image dependency and
  the owner's supply-chain rule applies. **S**.

**G4 — "plans cited tests that did not exist"** (`README.md` §1).
- Nothing checks a citation.
- **Change:** make a citation convention mandatory in packet tables only:
  a `Pins:` field listing test function names in backticks.
- Add `tools/check_pins.py`, which fails if any `Pins:` name has no
  `fn <name>` under `server/crates`.
- Scope it to `docs/review/`, `docs/content/plan.md` and `docs/lanes/*.md`.
  Free text stays free, so the check cannot cry wolf.
- **S**. Coordinate the field name with V5.

## 3. Slow and flaky tests

**S1 — the bot binaries run in series, and their waits are real time** [V].
- `cargo test` runs test binaries one after another and parallelises only
  *within* a binary.
- The six `eve-bot/tests/*.rs` files are six binaries whose tests mostly
  wait on game clocks:
  - mining cycles (`mining_bots.rs:177`);
  - `JUMP_DELAY` = 4 s (`eve-server/src/jump.rs:67`);
  - warp.
- They are CPU-idle, so running them concurrently costs almost nothing.
- **Change:** move them into one integration binary,
  `eve-bot/tests/bots/main.rs`, with `mod mining; mod escort; …`, and the
  shared fixture in `bots/common.rs`.
- Expected wall time: about the longest single test, not the sum [I].
  The longest is probably in `mining_bots` (it holds a 30 s sleep, S2), so
  roughly 40-60 s instead of 167 s.
- **S**, behaviour-neutral. Lane-ownership note: these six files belong
  to whichever lane last owned bots, so this is a coordinator packet.

**S2 — an unconditional 30 s sleep in a gated test** [V].
- `mining_bots.rs:561` sleeps 30 s and then checks that no more than 3
  refusals happen in a row.
- **Change:** wait with `until(…)` for the observable that matters: *k*
  further `Mine` attempts after the first refusal, with *k* = 4, which is
  what "a fourth refusal in a row" needs. Assert on that log.
- Same assertion, without the fixed floor. **S**.

**S3 — a known cross-run flake on a fixed path** [V].
- `loopback.rs:377` writes to `CARGO_TARGET_TMPDIR/captures`, runs
  `remove_dir_all` on it, and asserts exactly one file in it
  (`:389`).
- Any two `cargo test` runs sharing a target directory race on that path.
  That happens whenever several lanes run tests in the shared tree, or
  while `soak.sh`/the gate runs.
- It already flaked once (`docs/lanes/character.md:412`).
- **Change:** add the pid and a per-test counter to the path, as the other
  11 temp-dir tests do (`store.rs:1070`, `corp.rs:3300`, …). **S**.

**S4 — no game-time scale** [V absent; `grep` finds no clock abstraction
outside `crimewatch.rs:187`].
- A server-wide `time_scale` for test servers would cut the bot tier
  further.
- It touches tick, destiny and every timer, and it overlaps V1 and V3.
- **Listed under "Do not do" for now:** after S1 and S2 the payoff is
  seconds.

**Flake inventory.** Recorded flakes:
- the S3 capture-path race;
- the two-in-space bare `recv()` hazard (`docs/lanes/two-in-space.md:472`),
  already designed around with bounded read loops;
- the client-CI `pgrep` self-match (`tools/client-ci.sh:517-537`), fixed.

There is no retry-on-flake anywhere, and that is correct
(`docs/lanes/ci-scenarios.md:550`).

## 4. Duplicated fixtures

- **The bot harness:**
  - each of the six binaries boots its own server, with
    `Config::new("127.0.0.1:0") … Server::bind … thread::spawn`. There are
    11 such sites [V];
  - there are 2 copies of `until(within, done)`: `escort_bots.rs:81` and
    `mining_bots.rs:99`.
  - S1 folds this into `bots/common.rs` for free.
- **The registry:**
  - three private `fn registry()` builders, in `service/tests.rs:30`,
    `service/fleet.rs:880` and `service/character.rs:1942`;
  - **206** `Registry::new`-style constructions outside `registry` [V
    grep].
  - **Change:** add a new file, `eve-server/src/testkit.rs`
    (`#[cfg(test)] pub mod`). It holds `registry()`, `registry_with(…)`
    builders and `scratch_dir(tag)`, the S3 fix in one place, removed on
    `Drop`.
  - Migrate callers opportunistically, never as a sweep: a sweep touches
    every hot file and makes lanes queue (`README.md` §1, "hot files").
  - **S** for the module. Migration is free when a lane is in the file
    anyway.
- **Temp dirs:** 12 hand-rolled `temp_dir().join(format!(…))` sites [V],
  none cleaned on panic. `scratch_dir` above covers them.

## 5. The soak

`tools/soak.sh` found the most expensive bug of the session (§67). It
asserts less than it reports [V]:

| what | reported | fails the run |
|---|---|---|
| panics in either log | yes | **yes** |
| `ERROR`/`error:` lines | yes | no |
| `UNANSWERED` calls | yes | no |
| bots' exit status | yes | **yes** (`soak.sh:99`) |
| **server's exit status** (an OOM SIGKILL is 137) | written to `server.exit` | **no** |
| **memory** | not measured | no |

- §67's OOM was caught only because the kill was obvious in the log.
- A 5× per-bot growth that stayed under the 16 GB container cap would
  pass today.
- **Change (P11):**
  1. fail on any non-zero `server.exit` other than the expected SIGTERM
     exit;
  2. sample the server's `VmRSS` from `/proc/$spid/status` every 10 s into
     `rss.tsv`, inside the container where the pid is known;
  3. fail if the peak exceeds `--rss-budget MB`. The default is an [A]: the
     post-§67 measurement of about 1.46 GB at 50 bots, plus 50%.
  4. print `UNANSWERED` names, top 10, so a soak's output drives the next
     packet.

  **S**. Do not put the budget in a Rust test (owner rule: no [A] in a
  test). It lives in the script's default, where it is visibly a knob.

## 6. Client CI unattended: what is actually missing

### 6.1 The state of the machine, verified today

| fact | evidence |
|---|---|
| helper installed, root:root 0755 | `ls -la /usr/local/sbin` → `eve-offline-jail`, 2026-10-22 23:52 [V] |
| sudoers grants the seven verbs, NOPASSWD | `sudo -n -l` lists up, down, verify, dns, client, status, preflight [V] |
| **installed helper is stale** | `tools/install-jail-helper.sh --diff` shows the installed copy **lacks the `preflight` verb and port 5222** [V] |
| …so the grant for `preflight` is dead | `sudo -n /usr/local/sbin/eve-offline-jail preflight` → `usage: … up\|down\|verify\|dns\|client\|status`, rc 1 [V] |
| unattended runs already worked | 5/5 + 1 `ballpark_smoke` runs on 2026-10-23 (`docs/lanes/ci-harness.md:385-401`) |

**Consequences of the stale helper** [V by code]:
- `up.sh` falls back to host-side checks that "cannot see the namespace's
  routing" (`tools/up.sh:486-513`), so the in-namespace preflight is
  skipped on every run.
- The installed jail drops **5222**, so in-game chat cannot connect from
  a jailed client. Any scenario or human check that touches chat fails
  for a reason unrelated to the code under test [I for the scenario
  effect].
- Tracker §43.4 named the fix (`sudo tools/install-jail-helper.sh`). It
  has not been applied, and nothing reports that.

### 6.2 Findings

**C1 — helper drift goes undetected** [V].
- The helper is a deliberate snapshot (`tools/eve-offline-jail.in:110-114`),
  and that design is right: a human reads the diff.
- But no tool says when the snapshot is behind. `up.sh` notices only the
  `preflight` case, and only as a "note".
- **Change (P2):**
  - `install-jail-helper.sh --check` exits 0 if up to date, 3 if stale, 4
    if absent. It needs no root, because the installed file is
    world-readable.
  - `client-ci.sh` preflight calls it and **refuses** with exit 3 when it
    is stale. The message is the two commands to fix it.
  - `up.sh` warns.
- **S**.

**C2 — the helper is pinned to one checkout** [V].
- `REPO` is substituted at install (`tools/install-jail-helper.sh:32`).
  `DNS_BIN` and `CLIENT_SHIM` derive from it
  (`tools/eve-offline-jail.in:53-54`).
- `run-client.sh` writes the launch file to **its own**
  `$REPO/out/client-ci/launch-env` (`tools/run-client.sh:392`). The shim
  the helper runs reads the **main** checkout's file
  (`tools/jail-client.sh:24`).
- So a client-CI run from a lane copy or a `land.sh --head` worktree
  launches with the main tree's **previous** launch file, or with none. It
  also runs the main tree's `eve-dns`, not the one under test.
- **Change (P3):** `run-client.sh` and `client-ci.sh` refuse when `$REPO`
  differs from the helper's `REPO=` line. The rootless path (P5) removes
  the restriction entirely. **S**.

**C3 — unix sockets cross the network jail** [V reachability; I exploit].
- A network namespace isolates the *network*. It does not isolate the
  filesystem, and unix sockets are files.
- `XDG_RUNTIME_DIR` is forwarded into the jail on both launch paths
  (`tools/run-client.sh:398`, `:424`; allow-listed at
  `tools/jail-client.sh:29`). So the client can reach the session D-Bus
  (`/run/user/1000/bus`).
- Also reachable: `/run/systemd/resolve/io.systemd.Resolve`, which is
  mode **0666** [V], and the system bus.
- None of these is used by NSS here (`hosts: files mdns4_minimal
  [NOTFOUND=return] dns …` [V]), so name lookups are not leaking.
- **The concrete path** [I]: Wine resolves a ShellExecute of an `http(s)`
  URL through `winebrowser` → `xdg-open`. With a browser already running
  in the owner's session, `xdg-open` hands the URL over D-Bus or remoting
  to **that** browser, which is outside the jail and online. The EVE client
  opens URLs (support, help and store links).
- That is phoning home through a door the nft rules cannot see. Neither
  `verify` nor `jail-verify.sh` checks for it: both probe TCP only
  (`tools/eve-offline-jail.in:275-301`).
- **Change (P4):**
  1. Stop forwarding the host `XDG_RUNTIME_DIR`. Point it at a private,
     empty `0700` dir under `out/client-ci/runtime/`.
  2. In `jail-client.sh` (which runs as the user), wrap the launch in
     `unshare --user --map-current-user --mount`, with tmpfs over
     `/run/user/$UID`, `/run/dbus` and `/run/systemd/resolve`. That needs
     no root. The netns is inherited, so the guarantee only grows.
  3. Add a `verify` probe that fails if `/run/user/$UID/bus` or
     `/run/dbus/system_bus_socket` is reachable from the client's
     context.

  **S/M**. **Not behaviour-neutral for humans:** client audio
  (PulseAudio/PipeWire live in `XDG_RUNTIME_DIR`) goes quiet unless the
  pulse socket alone is bind-mounted back in. That is a named exception
  the owner should approve (**unknown**: whether Proton's audio needs
  anything beyond the pulse socket).

**C4 — `verify` probes stale addresses** [V].
- The helper's `verify` probes the hardcoded `192.0.2.1` and
  `192.0.2.151` (`tools/eve-offline-jail.in:286-288`).
- `192.0.2.151` is a DHCP **dynamic** lease on `eno1` [V `ip addr`].
- When it changes, the probe targets an address that does not exist and
  "passes" trivially.
- The real guarantee is the namespace's `policy drop` plus no default
  route. `verify` checks the route but **never checks that the nft table
  exists**.
- **Change:**
  - `verify` asserts `ip netns exec NS nft list chain inet
    eve_offline_out output` contains `policy drop`;
  - derive the LAN address at runtime, as `tools/jail-verify.sh:48`
    already does.

  Needs a reinstall, so bundle it with C1/C3. **S**.

**C5 — nothing schedules a run.**
- `client-ci.sh` works unattended, but only when somebody types it.
- The memory note "autonomous push queue" is where it belongs.
- **Change (P6):** `tools/nightly.sh SHA`, which:
  1. exports the sha;
  2. builds;
  3. runs `soak.sh --secs 300` and then every scenario through
     `client-ci.sh` (rootless, P5);
  4. writes `out/nightly/<sha>/summary.tsv` and a one-screen report.

  It **never pushes and never edits**: verification only. The coordinator,
  or a `/loop`, reads the summary. **M**.

### 6.3 The narrowest privilege: two options, ranked

**Option A — rootless jail for CI (recommended)** [V prototype, I for
the full client run].

```
unshare --user --map-root-user --net --mount --propagation private
  ip link set lo up
  ip addr add 10.137.0.1/32 dev lo             # same address the real jail uses
  mount --bind out/client-ci/hosts /etc/hosts  # tranquility… -> 10.137.0.1
  mount -t tmpfs none /run/user/$UID           # + /run/dbus, /run/systemd/resolve
  eve-dns --bind 10.137.0.1:53 &               # optional: hosts file already answers
  eve-server --bind 10.137.0.1:26000 … &
  unshare --user --map-user=$UID --map-group=$GID   # drop back to "you"
      jail-client.sh                           # Proton, DISPLAY=:99 via /tmp/.X11-unix
```

**Verified on this host, as `youruser`, with no sudo:**
- creating the namespace works even with
  `kernel.apparmor_restrict_unprivileged_userns = 1` (Ubuntu 26.04.1,
  util-linux 2.41.3);
- inside it:
  - `lo` comes up;
  - `10.137.0.1/32` can be added;
  - binding UDP `:53` works;
  - the `/etc/hosts` bind-mount resolves `tranquility.servers.eveonline.com`
    → `10.137.0.1`;
  - tmpfs hides `/run/user/1000` (0 entries);
  - `ip route` is empty;
- after dropping to uid 1000 through the nested userns:
  - `ip addr add` → `Operation not permitted`;
  - `connect(1.1.1.1:443)` → `ENETUNREACH`.
- Separately, `ci-display` already ran the real client under Proton in
  `unshare --map-current-user --net` to its login screen
  (`tools/client-headless.sh:288-330`, `docs/lanes/ci-display.md:97`).

**Why it is the narrowest option:**
- **Zero added privilege.** No sudoers line, no root-owned file, no
  firewall change, nothing left behind if it crashes: the namespace dies
  with its last process. `v_down` and the reaper become unnecessary on
  this path.
- **The guarantee is structural.** There is no interface but `lo`, so
  there is nothing to route through. The current jail's guarantee rests on
  one nft table whose presence `verify` does not even check (C4).
- **The client cannot weaken it.** It runs as uid 1000 in a child userns
  with no capabilities over the netns.
- **It closes the C3 sockets** in the same mount namespace.
- **It is checkout-independent (C2):** lanes, worktrees and `nightly.sh`
  can each run their own sha.
- Two clients can share the namespace, so `--with-second` still works.

**Limits:**
- CI only. A friend on the LAN cannot reach a server in a private netns,
  so human play keeps the helper.
- It depends on Ubuntu continuing to let `/usr/bin/unshare` create userns
  under the AppArmor restriction. If that changes, `client-ci.sh` falls
  back to the helper, the same "either is enough" structure as
  `client-ci.sh:467-479`.
- **unknown:** whether Proton, Wine and `wineserver` behave under the
  *nested* userns. The smoke used a single level, and Wine's prefix
  ownership check should pass because the uid maps back to 1000.
- **unknown:** whether a client logs in end to end against a server
  inside the namespace. That is the first thing P5 runs.

**Option B — the existing helper, kept minimal** [V design;
`docs/reference/unattended-root.md` §4].
- This is already the narrowest *sudo* design:
  - no arguments;
  - root never executes user-writable code (`setpriv` drops before the
    shim, `tools/eve-offline-jail.in:393-399`);
  - the trust chain is checked on every call (`:145-157`).
- If the owner wants *less* sudo, the §7 "one verb" variant still holds:
  set up the jail once per boot interactively, and grant only `client`.
- Do not widen it to `ip netns exec`, `jail-*.sh` or the `eve-dns` binary
  (`docs/reference/unattended-root.md` §3: each is root with extra steps).
- With A in place, B's unattended use drops to zero, and the sudoers
  entry could be reduced to `status` + `client` for human sessions, or
  removed (owner decision).

---

## Proposals

1. **P1 — Reinstall the helper now (human, 1 min).** Commands:
   `tools/install-jail-helper.sh --diff`, then
   `sudo tools/install-jail-helper.sh`. This restores the in-namespace
   preflight and chat on 5222 [V stale]. Put it at the top of
   `docs/guides/human-checks.md`.
2. **P2 — Drift detection.** Add `install-jail-helper.sh --check`.
   `client-ci.sh` refuses on stale; `up.sh` warns (C1).
3. **P3 — Refuse a foreign checkout on the helper path** (C2).
4. **P4 — Close the unix-socket door.** Use a private `XDG_RUNTIME_DIR`
   and a user-level mount namespace in `jail-client.sh`, and probe the
   socket in `verify`. Also make `verify` assert the nft policy and derive
   the LAN IP (C3, C4). One helper reinstall carries all of it.
5. **P5 — Rootless CI jail.** Add `tools/jail-rootless.sh`, and give
   `client-ci.sh` a `--rootless` flag that becomes the default once it has
   5 consecutive green runs of `ballpark_smoke`, matching the 2026-10-23
   bar (§6.3 A).
6. **P6 — `tools/nightly.sh SHA`.** Soak 300 s, then every scenario, a
   summary file, no push (C5).
7. **P7 — One `eve-bot` integration binary** with a shared `common.rs`
   (S1, §4).
8. **P8 — Remove the 30 s sleep** at `mining_bots.rs:561` (S2).
9. **P9 — `tools/test_gate.py`**, stub-driven failure-path tests for
   `gate.sh` and `land.sh` (G2), plus `bash -n` over `tools/*.sh` (G3).
10. **P10 — Reconcile the pre-push hook with `land.sh --head`** (G1).
11. **P11 — Soak asserts:** server exit, a peak-RSS budget, and the top
    `UNANSWERED` calls (§5).
12. **P12 — `testkit.rs`**: shared registry builders and `scratch_dir`.
    Fix the capture-path race first (S3, §4).
13. **P13 — `Pins:` citation check** for packet tables (G4).

## Packets

| id | files | tests / check that pins it | size | neutral? |
|---|---|---|---|---|
| T1 (P1) | none (human) | `install-jail-helper.sh --diff` prints "up to date"; `sudo -n …/eve-offline-jail preflight` prints `PREFLIGHT OK` with the stack up | S | yes |
| T2 (P2+P3) | `tools/install-jail-helper.sh`, `tools/client-ci.sh` (preflight block), `tools/up.sh` (note block), `tools/run-client.sh` (helper branch) | `client-ci.sh --check` with a stale fake helper via an `EVE_JAIL_HELPER` override → exit 3 naming the fix; foreign `REPO` → exit 3 | S | yes on a current install |
| T3 (P4) | `tools/eve-offline-jail.in` (`verify`), `tools/jail-client.sh`, `tools/run-client.sh` (launch-env), `tools/jail-verify.sh` | `verify` FAILs with the nft table deleted; a probe inside the shim's mount ns finds no `/run/user/$UID/bus`; one `ballpark_smoke` green | S/M | **no**: humans lose audio unless the pulse socket is re-bound (owner call) |
| T4 (P5) | new `tools/jail-rootless.sh`; `tools/client-ci.sh` (`--rootless`, jail selection) | the egress probes inside the ns (`ENETUNREACH`, no route, `ip addr add` refused as uid 1000); `ballpark_smoke` 5/5; `--with-second` 1/1 | M | yes (new path, opt-in) |
| T5 (P6) | new `tools/nightly.sh` | a run against `HEAD` writes `out/nightly/<sha>/summary.tsv` with one row per step; exits non-zero on any red step | M | yes |
| T6 (P7+P8) | `server/crates/eve-bot/tests/*.rs` → `tests/bots/{main,common,mining,escort,handshake,jump,npc,verbs}.rs` | same 39 tests by name; gate test step wall time recorded before and after in the commit message | S | yes |
| T7 (P9+P10) | new `tools/test_gate.py`; `tools/gate.sh` (one `bash -n` loop); `tools/land.sh` (stamp); `tools/hooks/pre-push` | `test_gate.py`'s five cases (§2 G2) | S | yes |
| T8 (P11) | `tools/soak.sh` | `soak.sh --miners 1 --escorts 0 --secs 30 --rss-budget 1` exits 1 naming RSS; with no budget it exits 0 | S | yes (stricter) |
| T9 (P12) | new `server/crates/eve-server/src/testkit.rs`, one `mod` line in `lib.rs`, `tests/loopback.rs:377` | two concurrent `cargo test -p eve-server --test loopback every_frame` in one target dir both pass | S | yes |
| T10 (P13) | new `tools/check_pins.py`; one gate line | `tools/test_check_pins.py` with synthetic docs: a missing fn fails, a present fn passes | S | yes |

**Order:**
1. T1 today: one human command, no code.
2. T7, T6 and T9: the gate gets cheaper and trustworthy, and these files
   collide with no content wave.
3. T2 and T3, with one reinstall.
4. T4, then T5.
5. T8 and T10 fit anywhere.

**Files that collide:** T2, T3 and T4 all touch `client-ci.sh` and
`run-client.sh`, so run them in one lane or in series.

## Do not do

- **Do not add a `NOPASSWD` entry for `ip netns exec`, `jail-setup.sh` or
  the `eve-dns` binary.** Each one is root for whoever can write the repo
  (`docs/reference/unattended-root.md` §3). Option A makes them unnecessary.
- **Do not have the helper read or exec the repo's `jail-*.sh` "to stop the
  drift".** The snapshot is the review step. Fix drift with *detection*
  (P2), not by removing the snapshot.
- **Do not add retry-on-flake** to the gate or to client CI. A rerun that
  goes green teaches everyone to rerun (`docs/lanes/two-in-space.md:714`).
- **Do not add a game-time scale (S4) to speed tests** until after T6. It
  touches every timer for seconds of gain, and it overlaps V1 and V3.
- **Do not put client CI or the soak in `gate.sh`.** The gate must run with
  no client and no Proton prefix, inside `--network none`. Client CI and
  the soak belong in `nightly.sh`, which reports and never blocks a push.
- **Do not add shellcheck, nextest or any other test runner.** Each is a
  new supply-chain dependency. `bash -n` plus one binary for the bot tests
  gets most of the value.
- **Do not sweep the 206 registry constructions into `testkit` at once.**
  It touches every hot file. Migrate only while a lane is already in the
  file.
- **Do not frame-diff screenshots as a CI oracle.** Numbers the client
  computed are not flaky; a 3D frame comparison is
  (`docs/lanes/ci-harness.md:330-333`).

## unknown

- Whether Proton runs under the **nested** userns of Option A. The
  single-level case is verified (ci-display). The nested case was checked
  only up to `id`.
- Whether a full login → in-space scenario passes against a server bound
  on `lo` inside the rootless namespace. It needs no sudo, so it can be
  run unattended. It was not run here, because the client and its prefix
  are shared state and this lane is read-only.
- Whether Proton's audio needs more than the pulse socket once
  `XDG_RUNTIME_DIR` is private (T3's human-facing cost).
- Whether the EVE client actually calls ShellExecute with URLs on any path
  a scenario reaches. C3's door is verified open; its use is [I].
- The gate's test-step wall time at the default `EVE_MAX_JOBS=16`. It was
  measured at 4. The bot tests are wait-bound, so 16 threads helps within
  a binary but not across the six serial binaries.
- Whether future Ubuntu AppArmor updates restrict `/usr/bin/unshare`
  userns creation. It is allowed today with the restriction sysctl on.
