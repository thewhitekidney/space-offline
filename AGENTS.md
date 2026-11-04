# eve-offline: start here

A std-only Rust private server for the EVE Online client (the pinned client build is
the `pinned` row of `tools/builds.tsv`). The goal: co-op and PvP with friends, plus
about 50 bots that play like people.

**Agent: this file is a procedure, not a reference. Run it.** Start at step 0, do what
each step says, follow its jump, and stop where it says stop. Read nothing else unless a
step or your issue links it. Your human answers for what you submit; ask them only where
a step says so. Many people's agents work on this tree at once; the claim in step 3 is
what keeps you from colliding with them or doing their work twice.

## Never read these whole

`wc -c` a file before you read it; over 60 KB, grep it or read a range.

| file | instead |
|---|---|
| `findings/*.tsv`, `findings/*.txt`, `findings/*.json` (up to ~670 KB) | grep for your key |
| `docs/reference/client-architecture.md` (~210 KB), `docs/evejs/review/features.md` (~160 KB) | their headings, then a section |
| `docs/status/README.md` (generated) | `grep -n '<feature words>' docs/status/progress.tsv` |
| `server/crates/eve-server/src/service.rs`, `service/tests.rs` (thousands of lines each) | grep for the handler |

## Hard rules, and why

- **No installs on the host; third-party code runs only in Docker** (`tools/dev.sh`,
  `tools/analysis.sh`). Why: supply chain. Your human's machine is not ours to risk.
- **No extracted client data in git, tests included.** Tests use invented ids and rows;
  real values go in `out/` (gitignored). Why: CCP's data is not ours to publish.
- **No dependencies (`server/deps.allow` stays empty) and no `unsafe`.** Why: supply
  chain, and an audit surface one person can read.
- **The client is always jailed.** It never gets a route to CCP (`docs/guides/jail.md`).
- **`[A]` marks an assumption, with what would falsify it; an `[A]` value never goes
  into a test.** Why: a test that pins a guess makes the guess look proved.
- **The gate decides, by exit code: `tools/land.sh --no-push`.** Never pipe a test into
  `&&`. Why: a pipeline returns its last command's status, and red reached master that
  way.
- **No push to master, no merge.** You open a PR; a maintainer merges. Why: one
  reviewed path.
- **No work without a claim, and no edit outside your claim's `Owns:` paths.** Why: two
  agents on one file lose an edit, and two agents on one issue waste a day.
- **Guarded paths need a maintainer** (label `needs-human`): `AGENTS.md`, `CLAUDE.md`,
  `tools/land.sh`, `tools/gate.sh`, `tools/public-export.py`, `server/deps.allow`,
  `.github/`, and the hot files in `docs/guides/contributing.md` §10. Why: they hold
  the rules and the gate.
- **Feature state is kept current in the same PR as the code** (step 6). Why: the
  status page is how everyone else knows what is done. A change to `server/`,
  `content/` or `tools/client-ci/scenarios/` that leaves `docs/status/progress.tsv`
  alone fails the gate (`tools/status-guard.py`).
- **EvEJS is AGPL: facts only, verified against the client.** Never copy its code or
  prose (`docs/evejs/`). Why: licence, and it targets another client build.
- **No money, ever.** Nobody receives money or anything worth money in connection
  with this project: no donations, sponsorship, ads, paid access or shared hosting
  costs (`LICENSE.md` §3). Why: it is the project licence's condition, and a breach
  ends the licence.
- **No secrets in the repo, an issue, a PR, a log or a prompt; never print a token.**
  Why: issues and PRs are read by others.
- **Never drive a human's mouse or keyboard.** Why: the client runs on someone's
  desktop.

## The procedure

### Step 0. Where am I?

    tools/agent-status.sh

It prints `key=value` lines and ends with `next=...`. Go where it says.
- `next=step1` (`docker=no` or `image=no`) → step 1.
- `next=step4` (`claim=<N>`: your branch is `<handle>/<N>-...` and you hold N) → step 4;
  resume: read the issue and your draft PR first.
- `next=stop-dirty` (uncommitted work and no claim) → **stop**: tell your human. Do not
  touch that work.
- `tracker=no-config` → ask your human which tracker repo to use, then set
  `EVE_TRACKER=github`, `EVE_TRACKER_API=https://api.github.com` and
  `EVE_TRACKER_REPO=<owner>/<name>` (or clone so that `origin` is that GitHub repo).
  Without a tracker you cannot pick a task: stop after asking.
- `tracker=no-token` → your human sets `EVE_TRACKER_TOKEN` (or `EVE_TRACKER_TOKEN_FILE`)
  in your environment; never ask them to paste it to you. Without it, listing works
  only on a public repo, and step 3 needs your human.
- Otherwise → step 2.

Never: trust your memory of an earlier session over this output.

### Step 1. Set up

    tools/dev.sh --image                                   # builds the Rust image once (network)
    EVE_MAX_JOBS=4 tools/dev.sh cargo test --locked -p eve-marshal

Success: `test result: ok`. Without an EVE client you can still take server-only tasks
(step 2 filters them with `--no-client`). Setting up a client is for your human
(`docs/guides/contributing.md`).
Failure: no Docker → **stop**, ask your human to install it. Killed for memory → lower
`EVE_MAX_JOBS`; `tools/dev.sh` waits for free RAM.
Never: `pip`, `npm`, `cargo install` or `curl | sh` on the host; `sudo` anything
yourself.

### Step 2. Pick a task

    tools/next-task.sh             # add --no-client if you have no client

The tracker is the shared board: GitHub Issues on the public repo. `next-task.sh` lists
open, unclaimed, unblocked issues labelled `status/ready`, best first, with each one's
area, its `Owns:` hint and its `Read:` docs. Take the first one you can finish. Propose
it to your human in this form, and wait for a yes:

    Task #N: <title>. I would own: <paths>. Plan: <three lines>. Needs a client: yes/no. OK?

No rows → **stop** and say so. Do not invent work.
Never: take an issue that is assigned, `status/claimed`, `needs-owner`, `parked` or
blocked; start without an issue. A bug or idea with no issue → draft one for your human
(title, steps, files) and stop.

### Step 3. Claim it

    tools/claims.py check --owns '<path>,<glob>,...'       # exit 0: nobody else owns them
    tools/claims.py claim N --owns '<path>,<glob>,...'
    tools/claims.py show N                                 # confirms you hold it

Success: `show` prints your login as the holder. The claim is a `/claim` comment with
the `Owns:` line, plus the assignee and `status/claimed` where your token may set them;
without those rights the comment alone is the claim.
Failure:
- exit 3 (overlap) → narrow your paths, or go back to step 2;
- exit 4 (someone claimed first) → step 2;
- exit 5 (a hot file) → ask your human to get `hot-ok` from a maintainer, or pick
  another;
- no token (`EVE_TRACKER_TOKEN`) → give your human the exact `claim` command to run, or
  have them post the `/claim` comment.

Never: hold two claims; claim on someone else's behalf; widen `Owns:` without running
`tools/claims.py claim N --owns <new>` again (it re-checks).

### Step 4. Work

    git fetch origin && git switch -c <handle>/<N>-<slug> origin/master

- Read the issue's `Read:` docs and its area's row in `docs/guides/contributing.md`
  §10. Nothing else unless those link it. `docs/guides/working-together.md` explains the
  rules of sharing the tree.
- Within the first hour, push the branch and open a **draft** PR that says `Closes #N`,
  with the template filled as far as you can.
- Code: `service/x.rs` holds the wire arm only; state and logic live in the crate-root
  `x.rs`. A new test goes in the module it tests. Every claim about the client carries
  `[V]` (verified, with evidence), `[I]` (inferred) or `[A]` (assumed, with its
  falsifier).
- Tests while you work:
  `EVE_MAX_JOBS=4 tools/dev.sh cargo test --locked -p eve-server --lib -- <filter>`.
- A bug outside your claim → draft an issue for your human; do not fix it here.

Never: edit outside `Owns:`; put a client value in a test; copy EvEJS; leave a build
copy in `/tmp` (it may be RAM).

### Step 5. Gate

    git fetch origin && git rebase origin/master
    tools/land.sh --no-push

Success: exit 0 and a stamp line naming `out/landed/<sha>`.
Failure: read the log it names (grep `error\|FAILED\|panicked`), fix, run it again.
Three reds on the same cause → step 7, stuck.
Never: edit the gate, `#[ignore]` a test, delete a failing test, or report green from
part of a run.

### Step 6. Pull request, with the progress documents

1. **Update the progress documents. This is required, and the gate checks it.**
   - Edit your feature's row in `docs/status/progress.tsv` (or add one): `state`
     (new code is `built-unproved`; only a client run makes it `client-proved`),
     `evidence` (code, a commit or a scenario, never a plan), `issue` (`#N`) and
     `theme`. Never reorder the table; on a conflict keep both rows.
   - Run `python3 -I tools/status-page.py` and commit what it regenerates. Never
     hand-edit or hand-merge a generated file.
   - If your change finishes or moves a theme's open item, say so in the PR so a
     maintainer updates the theme list.
   - No feature state changed (a refactor, a doc fix, a test)? Put a line
     `Status: none (<why>)` in the PR body and the commit message. Without one of the
     two, `tools/status-guard.py` fails the gate and the PR check.
2. Mark the PR ready. Fill every template field: what and why, `Closes #N`, the owned
   files, the tests, the `[A]`s with falsifiers, the status rows touched, the gate
   stamp, and the agent and model you are. Your human reads `CONTRIBUTING-terms.md`
   and ticks its sign-off box; a PR with that box unticked is not imported.
3. Review: an Opus-class model gets the diff and the PR text, not your conclusions. Run
   it if you can start one; otherwise ask for it in the PR. Answer every finding in the
   PR: fixed (with the commit) or why not.
4. `tools/claims.py status N review`.

Success: green checks, every finding answered. A maintainer merges.
Never: merge, push to master, or settle a finding by weakening a test.

### Step 7. Done, or stuck

Done (merged): `tools/claims.py release N` (the merge also closes the issue). Delete
your build copy, `server/target` first. Return to step 2 only if your human says so.

Stuck means any of these:
- three reds on the same cause;
- a file you need is claimed or guarded;
- a decision only the owner can make;
- an `[A]` you cannot settle without a client.

Then:

    tools/claims.py status N needs-human --why '<what you tried; what failed; the one question>'

and **stop**. Keep your branch and draft PR; do not release the claim.

## Commands

    EVE_MAX_JOBS=4 tools/dev.sh cargo test --locked --workspace
    EVE_MAX_JOBS=4 tools/dev.sh cargo clippy --locked --workspace --all-targets -- -D warnings
    docker run --rm --network none -u $(id -u):$(id -g) -v $PWD/server:/s:rw -w /s eve-offline-build cargo fmt --all
    python3 -I tools/test_docs.py      # the doc lint
    python3 -I tools/env-ref.py --check  # every flag and EVE_* variable has its row
    python3 -I tools/status-page.py    # regenerate the status page from progress.tsv
    tools/land.sh --no-push            # the full gate
    tools/soak.sh                      # bots for 20 min; only on a machine nobody plays on

## Where each answer lives

This table, `docs/README.md` (every doc) and `docs/status/progress.tsv` (every feature)
are the whole index; each points to the others.

| question | the one place |
|---|---|
| is a feature built or proved | `docs/status/progress.tsv`, one row per feature (page: `docs/status/README.md`) |
| what to do next, in what order; who has what | the issue tracker: `tools/next-task.sh`, `tools/claims.py list` |
| what is left overall, theme by theme | `docs/status/README.md` "What is left, by theme"; each theme's issues carry the label `theme/3.N` |
| how sharing the tree works | `docs/guides/working-together.md` |
| labels, areas, hot files, setup with a client | `docs/guides/contributing.md` |
| build, content, run the server and the client | `docs/guides/getting-started.md` |
| every launch flag and `EVE_*` variable | `docs/reference/launch-and-env.md` |
| GM commands | `docs/guides/gm-commands.md` |
| every other doc | `docs/README.md` |
