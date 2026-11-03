# The architecture sweep

> **Demoted 2026-11-01** — last updated 2026-10-19. Landed since: the shared world (`docs/reference/threads-and-locks.md`) and the game clock (`server/crates/eve-server/src/game_clock.rs`). Current truth: `docs/plans/master-plan.md`, which carries the two items still open.

A reading of our own code rather than the client's, looking for the things
that will cost us later: shared state that should not be shared, a seam we
will want and do not have, a diagnostic that lies, a shortcut that has
started to carry weight.

It is recorded here because it was recorded nowhere: the first sweep's
findings lived in one conversation, three of them landed, and the rest were
invisible to the human and would have been invisible to the next session.
That is the failure mode this file exists to prevent. Items move out of it by
being done (with the commit named) or by being dropped (with the reason).

Companion documents: [`docs/archive/roadmap.md`](../archive/roadmap.md) is what we are building and
in what order; this is what we should change about how it is built.
[`lanes/README.md`](../lanes/README.md) is how the work is divided, and two of
its entries came out of this sweep.

## Landed

**One file per subsystem** (`efbfcf5`). `service.rs` was 7,374 lines, one
`match`, one `mod tests`. Every lane queued behind it. Now eleven modules
with their own tests and their own `SERVICES` list. Exercised immediately by
a change touching three modules with nothing to merge.

**The handshake is a constructor argument** (`c31fb0c`). `--user-id` and
`--proxy-node` were silently broken: `Config::new(bind)` derived
`sandbox.node_id` from the *default* handshake and the later
`config.handshake = hs` replaced only the handshake, leaving four other
copies of the identity disagreeing. The test written to prevent this passed
the whole time, because it exercised `Config::new`. The field is now only
reachable through `with_handshake`, and the test asserts all five copies
agree for a non-default identity.

**The service list derives from the table that routes it** (`79ac175`).
Removed the last file every lane had to touch. Both of its drift tests had
become tautologies; the per-module one now probes its own dispatcher with a
method that cannot exist, which is the bug the old test was aiming at.

**Three from the list below, in one commit** (`the push path, and three
things the sweep found`): `unanswered` is a `BTreeMap<String, u64>` and the
end-of-connection report prints `name x3` rather than three copies of `name`
— the multiplicity is the interesting part, since one occurrence is a gap and
a hundred is a live lock. `conn::serve` has an idle read timeout of 600 s,
comfortably past the client's own 240 s keepalive, which matters more now
that the push ticker holds a handle to every live connection's write side. And
a capture's filename carries the pid, so two runs started in the same second
no longer collide and silently lose the second one's recording.

**`Registry::config()` is no longer dead** — it was listed below as having
zero callers, and deleting it was the right call at the time. It has one now,
from the `/giveisk` push test, which needs the character id the notification
is addressed to. Left in place, and the item is struck from the list rather
than quietly dropped: the reason it was listed (it is an accessor to the field
we are about to stop owning per-connection) still stands, and the world split
is where it gets revisited.

**Capture replay, tier 1** (`aa8c354`). The four committed captures are a
test: 753 calls replayed through `Registry::dispatch`, asserting each is
**answered** and deliberately not comparing values — a changed value is
usually the work, a missing answer never is. It did not need the clock seam
after all, which is why it landed before it. Tier 2 still does.

**The rowset whitelist is asserted** (`ffa4b15`). `debug_assert!` in
`pyutil::instance`, because an unlisted class name makes the client abort the
whole read with `HACKER WARNING!` rather than ignore the value. The suite
passes with it on, so every class name we emit today is in the list.

**`evecap --calls` attributes every bound call** (`ffa4b15`). Two bugs: one
bind can hand out two objects (the fused `GetInventoryFromId` reply carries
both), and an object does not have to come from a bind at all
(`skillMgr2.GetMySkillHandler` returns a reference from an ordinary call). On
the richest capture, `?*` rows went from eleven to zero.

## This week

Empty, for the first time since this file was written. Everything that was
here has landed; what remains is below, and the next item is the clock seam.

## Next, in order

**The clock seam.** `filetime_now()` has four call sites
(`dispatch.rs:522`, `dispatch.rs:876`, `service/inventory.rs:198`,
`service.rs:654`) and reads the wall clock directly. Everything time-shaped
is therefore untestable except by waiting, and a push ticker makes that
worse, not better. One trait object, or one `Clock` field, and the replay
tier below becomes possible.

**The identity/world split, properly.** `server.rs` builds every connection
its own `Registry::new(config.sandbox.clone())`, so **two clients are two
universes** that agree only because they were cloned from the same
`Default`. The fix is one `Arc<Mutex<World>>`; the design is in
[`docs/reference/world-state.md`](../reference/world-state.md). The push path's ticker forces the same
question (the clock is world state, not connection state), so these two want
doing together.

**Diagnostics that escape the connection.** Everything we know about a
session is printed when that connection ends, which is also the moment the
human has stopped watching. Nothing is queryable while a client is up.

**A `PyValue::Long` that forces tag `0x2F`.** Our encoder picks the integer
tag by magnitude, so any value under 2^31 arrives in the client as a Python
`int` and never a `long`. `FormatDateTime` opens with
`isinstance(value, long)` and its else branch logs and returns `None` — a
blank field, with nothing in our log. We work around it by making every
"never" `FILETIME_2100` (157,469,184,000,000,000), where the magnitude is the
contract rather than the date. That works and it is a trap: the first small
timestamp anyone answers honestly will be silently blank. A variant that
pins the tag regardless of magnitude removes the trap.

**An `Args<'a>` layer.** Every arm reads its arguments by indexing a slice
and matching on `PyValue`, so the same four-line shape is written out
hundreds of times and each copy decides for itself what to do when the
argument is missing or the wrong type. One borrowing reader with named
accessors would make the arms shorter and the failure uniform.

## Dropped

Nothing yet. When something is dropped, it stays in this section with the
reason, because "we considered it and decided not to" is worth as much to the
next session as "we did it".
