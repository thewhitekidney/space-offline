# Working together: several people, each with their own agents

How several people, each running their own coding agents, share one tree without
colliding or doing the same work twice. The step-by-step procedure an agent runs is
`AGENTS.md`; this page is the reasoning behind it.

## The shared board

- **The issue tracker is the claim board.** Outside contributors use GitHub Issues
  on the public mirror; the team's own issues live on the maintainers' private tracker.
  `tools/claims.py` speaks both, and reads its tracker from `git remote get-url
  origin` or from `EVE_TRACKER`, `EVE_TRACKER_API` and `EVE_TRACKER_REPO`.
- **An issue is free** when it is open, labelled `status/ready`, has no assignee,
  no live `/claim` comment, no `needs-owner` or `parked` label, and every issue on
  its `Blocked by:` line is closed. `tools/next-task.sh` lists exactly those.
- **A claim** is a comment whose first line is `/claim` and which carries an
  `Owns: path, glob/*` line. Where the token may, the tool also assigns the claimer
  and swaps `status/ready` for `status/claimed`. The earliest live `/claim` wins; a
  `/release` by the holder ends it.
- **One claim per person at a time.** A claim that sees no PR activity for a week is
  shown as stale by `tools/claims.py list`; a maintainer may release it.

## Owned files

- **Declare owned files before you start**, in the `Owns:` line, and edit nothing
  outside them. `tools/claims.py check --owns ...` compares your paths with every
  other open claim and with the files of open PRs; overlap means talk first, or
  narrow your paths.
- **Hot files** are the ones many changes touch. They are listed in
  `docs/guides/contributing.md` §10; a claim on one needs the `hot-ok` label from a
  maintainer.
- **Append-only files** do not count as overlap: `docs/status/progress.tsv` and its
  generated page and pictures, and `tools/client-ci/regress.list`.

## Branches, PRs and the gate

- Each claim is one branch, `<handle>/<issue>-<slug>`, and one PR against main
  that says `Closes #N`. Open it as a draft early, so others see your files.
- Everyone runs the same gate on their branch, `tools/land.sh --no-push`. It decides
  by exit code; never pipe a test into `&&`.
- An Opus-class review reads the diff and the PR text (not the author's
  conclusions); every finding is answered in the PR. A maintainer merges.
- An agent that cannot commit hands back a unified diff (`git diff -U5`) against its
  starting commit, with a short note. Its human applies it on their branch.

## Generated files are never hand-merged

On a conflict in `docs/status/progress.tsv`, keep both sides' rows, then run
`python3 -I tools/status-page.py` to regenerate the page and the pictures.

## The layering rule

`server/crates/eve-server/src/service/x.rs` holds the wire arm only: parse the call,
call the model, build the reply. State and logic live in the crate-root module
`x.rs`, where they can be tested without a socket. A new test goes in the module it
tests. Keeping the arms thin is what lets two people work on two systems without
both editing the same service file.

## Machines

One build copy per claim, on disk (not in `/tmp`, which may be RAM), outside the
main checkout; delete it when done, `server/target` first (it takes gigabytes).
Keep `EVE_MAX_JOBS` to what the machine can spare; `tools/dev.sh` waits for free
RAM. Never run soaks or benches on a machine someone is playing on.
