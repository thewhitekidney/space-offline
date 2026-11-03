<!-- space-offline-pr-template: keep this line; label.yml looks for it -->
Fill in every section and tick every box. A PR with an empty section or an
unticked box is labelled `needs-human` and waits. The procedure is `AGENTS.md`.

Closes #

## What changed, and why

## Owned files
The files or globs the issue's `Owns:` line gave you (your `/claim` comment):

## Tests
new: …   existing: …   (or "none, because ...")

## [A] values
Each assumed value with what would falsify it, or "none":
[A] … — falsifier: …

## Status rows touched (docs/status/progress.tsv)
The rows you changed or added (state, evidence, issue, theme), and that you ran
`python3 -I tools/status-page.py`. Or, when no feature state changed, a line:
Status: none (<why>)

## Human check
Only for player-visible work (label `client-visible`): what a human with the
client should see, and the scenario that shows it. Otherwise "none".

## Gate
The last lines of `tools/land.sh --no-push`: the `out/landed/<sha>` stamp.

## Review
Reviewer model: …   Findings: each answered (fixed in <sha>, or why not).

## Agent and model
The coding agent and model you used, or "none, written by hand":

## Checklist
- [ ] The PR is linked to an issue that was claimed before the work started.
- [ ] Status rows updated in `docs/status/progress.tsv` and the page regenerated, or a `Status: none (<why>)` line above.
- [ ] No extracted client data, in tests or anywhere else (tests use invented ids and rows).
- [ ] No new dependency (`server/deps.allow` stays empty) and no `unsafe`.
- [ ] I touched no guarded path (see `.github/CODEOWNERS`), or I say here why: 
- [ ] I ran the gate (`tools/land.sh --no-push`) and it is green.
- [ ] Everything was built and tested through `tools/dev.sh` (Docker), not on the host.
- [ ] I accept the Contributor Terms (`CONTRIBUTING-terms.md`) for this PR: I wrote it or have the right to submit it, it contains no AGPL, GPL or other incompatible code, and I grant thewhitekidney the rights set out there, including relicensing.
