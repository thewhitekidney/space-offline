# Public export: one private repo, one public mirror

The private repo is canonical and keeps every commit and every doc; the team's
issues live beside it on the maintainers' private tracker. The public repo is a mirror of the exported tree,
and its GitHub Issues are the shared claim board for outside contributors. It has its own fresh
history and never shares a commit with private. Nothing reaches it except through
the export.

## The mirror publishes after each green landing

After a landing passes the gate and is pushed, the mirror step
(`tools/mirror-github.sh`, maintainer-only) runs by itself: `tools/land.sh`
calls a maintainer-only after-push hook, which syncs that same gated sha. The
steps:

1. `tools/public-export.py --check` must pass: no client data, no personal data,
   no private pointers. A leak stops the sync.
2. The export goes into a separate checkout, never inside the main tree.
3. One commit per sync, authored and committed by the project's public alias.
   Its message is the date and the exported files that changed, never a private
   commit subject, and is scanned like a file.
4. The commit is pushed to the public repo. The last synced private sha is kept
   outside the mirror.

The sync refuses when the mirror's git identity is not the alias, or when any
commit in the mirror has another author or committer. `--dry-run` does everything
but the push and prints the diffstat. Work under private-only paths is never
exported, and half-done work waits for its landing.

## The message and PR text

They describe the change for an outside reader: what, why, how it was verified. They
name no lanes, issue numbers, private docs, hosts or people.

## Coming back: pull requests are imported, never merged on the mirror

Outside contributors work on the public repo and open pull requests there. A PR
is never merged on the public repo. Instead the maintainer imports it
(`tools/import-pr.sh <n>`, maintainer-only):

1. The PR head is fetched into a scratch clone with no checkout, so nothing from
   the PR is ever executed or written as a working file.
2. The diff against the merge base is refused, and the PR is labelled
   `needs-human`, when it touches a guarded path (the hard-rule docs, `AGENTS.md`,
   `tools/land.sh`, the export and its lists, `deploy/`, `.github/`, CI configs,
   `server/deps.allow`, a Cargo.toml dependency section, a `build.rs`), a binary
   file, or a file over 1 MB. `tools/pr_guard.py` holds the list and `.github/CODEOWNERS`
   mirrors it.
3. A review pass reads the diff and a brief; the PR's code builds and tests only
   through `tools/dev.sh` in Docker with `--network none`.
4. After approval the diff is applied to the private repo with `git apply --3way`,
   committed with the PR author as author, and lands through `tools/land.sh`.
   `apply` reads the PR body (saved as `pr-body.md` beside the diff, or
   `--body FILE`), refuses it unless the Contributor Terms sign-off box is ticked,
   and adds the trailer `Contributor-Terms: 1.0 accepted in PR #<n>`.
5. The next mirror sync publishes it. The public PR is closed as merged, with a
   comment naming the public commit.

The public repo's `check` workflow runs the gate's checks on every PR (the
`pull_request` trigger only, never `pull_request_target`, so a PR gets no
secrets). The `label` workflow only adds `needs-human` or `ready-to-import`; it
never merges. Public `main` is never edited by hand, so the two can't drift.

## The private folder

`private/` holds one maintainer's own setup: their machines, deploy, board,
secrets layout and how the two repos sync. It is never exported. Paths outside
it that are private too are listed in `private/export-private-paths.txt`.
`tools/public-export.py --check` lists every exported line that names a home
path, a home LAN address or anything in `private/export-scan.txt`, and fails
on one; `--out DIR` refuses to copy while any is left. New owner-specific
detail goes in `private/`, never in `AGENTS.md`, `docs/` or a tool comment.

## The licence

Both repos are under the No Money Share-Alike Licence 1.0 or any later version
published by the maintainer (`LICENSE.md`), with `CONTRIBUTING-terms.md` for
contributions and `NOTICE` for the MIT and zlib portions. The licence switch was
done on 2026-11-03. `tools/public-export.py --check` fails when the export lacks
any of those three files, when a `FUNDING.yml` exists anywhere, or when an exported
file calls the project "open source" or "OSI" outside quotes.

Owner actions on GitHub, which no file can do:
- leave the repository's licence picker unset (GitHub would show a licence the project does not use);
- keep GitHub Sponsors off for the repository and the account;
- add no `FUNDING.yml` (the check refuses one);
- add no `open-source` topic.

## Hard rules

- Never push, merge, cherry-pick or rebase private commits into public.
- Never add a public remote to the private repo. The mirror step pushes from its own
  separate checkout.
- Only the export writes to public. Every commit on public uses the alias identity.
- Never merge a PR on the public repo.
- A new file is classified public or private when it is added. PR-EXPORT adds a gate
  check that fails on an unclassified file.
