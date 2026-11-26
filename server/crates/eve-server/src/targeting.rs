//! Who has a lock on whom.
//!
//! **A lock is confirmed by the return value of one remote call, and by
//! nothing else `[V]`.** `targetMgr._LockTarget` (`targetMgr.py:1586`) is a
//! short function:
//!
//! `_LockTarget` unpacks `(flag, targetList)` from `AddTarget(tid)` and
//! calls `OnTargetAdded(tid)` when the flag is falsy.
//!
//! and `OnTargetAdded` (`:674`) *is* the whole visible lock -- the target box,
//! the bracket, the sound stopping, the active target. `targetList` is stored
//! and never read (one `STORE_FAST`, no `LOAD_FAST`), so an empty list is
//! complete. There is no push on the happy path, no destiny action, and no
//! second message. `docs/re/targeting.md` is the whole spec; its §0 is this
//! paragraph with the evidence attached.
//!
//! ## What the server is for, then
//!
//! Three things the client cannot do for itself:
//!
//! 1. **Remember.** `GetTargets`/`GetTargeters` are how a lock survives a
//!    reload, a view reopen or a reconnect: `godma.RefreshTargets` iterates
//!    both and re-scatters `OnTarget` for each `[V]`. Answering `[]` for ever
//!    -- which this server did until now -- means every lock vanishes at the
//!    next session change.
//! 2. **Tell the other player.** Nothing reaches a victim except our push.
//! 3. **End a lock.** Unlock is *not* return-confirmed: `RemoveTarget` is
//!    followed by `RemoveFromTeam`, which opens with `if self.IsObserving():`
//!    and is a no-op for any ordinary pilot (`targetMgr.py:420`) `[V]`. With
//!    no push the target box stays on screen for ever.
//!
//! ## The ghost this module exists to prevent
//!
//! `targetedBy` is **never** cleared by ball removal -- `DoBallRemove`
//! touches only `targetsByID` `[V]`. So if a *locker* docks, disconnects or
//! dies, the victim keeps `state.threatTargetsMe` on a bracket that no longer
//! exists, `IsTargeted()` stays true, and `OnTargetByOther` then logs an
//! error and *skips the indicator* on her next lock -- self-perpetuating, and
//! invisible from the locker's side. Every path that takes a ship out of
//! space has to drop its locks in both directions, which is why
//! [`Locks::drop_ship`] returns both.
//!
//! ## Not destiny, and the park does not change
//!
//! `ballparkCommon/` has no occurrence of `target` `[V]`: there is no lock
//! bit in the ball tail and no observer learns of a lock from the park. So
//! `destiny::Park::encode` is untouched by this feature, and the state lives
//! here beside the `Roster` instead.

use crate::notify::Push;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

/// Every lock in the world, by the **ship** that holds it.
///
/// Keyed by ship id rather than by character because that is what crosses the
/// wire: `AddTarget`'s argument is a ball id, `OnTarget`'s is a ball id, and
/// the client's own `targetsByID` is keyed the same way. Translating to
/// characters once, at the audience boundary, is cheaper than translating
/// back at every comparison.
///
/// A `BTreeSet` per locker so that locking twice is idempotent and the order
/// `GetTargets` answers in is stable -- the client re-scatters that list on
/// every session change, and a set that reordered itself would redraw the
/// target bar in a different order each time.
///
/// **A lock still being acquired is held but not complete (#225).** `held`
/// has a pair from the moment `AddTarget` is accepted, so the range sweep,
/// the unlock paths and the `GetTargets` answer see it; `due` stamps the
/// pairs whose acquisition has not finished, each with the [`Delivered`]
/// flag the queued completion sets **when the clock delivers it**, so
/// "complete" means exactly "the `add` has gone out", with no clock of our
/// own to disagree with the scheduler's. [`Self::targets_of`]
/// answers completed locks only, which is what an effect that needs a lock
/// reads; [`Self::held_of`] answers both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Locks {
    held: BTreeMap<i64, BTreeSet<i64>>,
    due: BTreeMap<(i64, i64), Delivered>,
}

/// Set when a queued lock completion is delivered (#225). Shared between the
/// lock's stamp in [`Locks`] and the scheduled [`Completes`] push.
#[derive(Debug, Clone, Default)]
pub struct Delivered(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Delivered {
    pub fn deliver(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_delivered(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl PartialEq for Delivered {
    fn eq(&self, other: &Self) -> bool {
        self.is_delivered() == other.is_delivered()
    }
}

/// The locker's `OnTarget('add')` for a lock still acquiring: the plain
/// [`TargetChange`], plus the flag that makes the lock complete when the
/// clock drains it. `restamp` is the one call `Registry::take_due` makes on
/// every push it delivers (after `still_stands`, before the grid filter).
#[derive(Debug)]
pub struct Completes {
    pub change: TargetChange,
    pub done: Delivered,
}

impl Push for Completes {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        self.change.to_push()
    }

    /// The wrapped change's own wire form: a completion goes in an
    /// `OnMultiEvent` batch like every other `OnTarget` ([`TargetChange`]).
    fn to_wire(&self) -> (&'static str, Vec<PyValue>) {
        self.change.to_wire()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        self.change.audience()
    }

    fn restamp(&mut self, _stamp: u32) {
        self.done.deliver();
    }

    fn still_stands(&self, w: &Registry) -> bool {
        self.change.still_stands(w)
    }

    fn cancel_tag(&self) -> Option<crate::notify::CancelTag> {
        self.change.cancel_tag()
    }
}

impl Locks {
    /// Every `(locker ship, target ship)` pair currently held.
    ///
    /// For the clock's range sweep, which has no character in hand and has to
    /// look at all of them. Collected rather than borrowed because the caller
    /// mutates the world while acting on the answer.
    pub fn pairs(&self) -> Vec<(i64, i64)> {
        self.held
            .iter()
            .flat_map(|(locker, targets)| targets.iter().map(move |t| (*locker, *t)))
            .collect()
    }

    /// Record a lock. `false` if it was already held.
    pub fn add(&mut self, locker_ship: i64, target_ship: i64) -> bool {
        self.held
            .entry(locker_ship)
            .or_default()
            .insert(target_ship)
    }

    /// Record a lock still being acquired, complete when `done` is set.
    /// `false` if it was already held (complete or not), and then nothing
    /// changes.
    pub fn add_pending(&mut self, locker_ship: i64, target_ship: i64, done: Delivered) -> bool {
        let new = self.add(locker_ship, target_ship);
        if new {
            self.mark_pending(locker_ship, target_ship, done);
        }
        new
    }

    /// Mark a held lock as still being acquired until `done` is set.
    pub fn mark_pending(&mut self, locker_ship: i64, target_ship: i64, done: Delivered) {
        if self
            .held
            .get(&locker_ship)
            .is_some_and(|t| t.contains(&target_ship))
        {
            self.due.insert((locker_ship, target_ship), done);
        }
    }

    /// Is this held lock still being acquired (#234)?
    pub fn is_pending(&self, locker_ship: i64, target_ship: i64) -> bool {
        self.held
            .get(&locker_ship)
            .is_some_and(|t| t.contains(&target_ship))
            && self
                .due
                .get(&(locker_ship, target_ship))
                .is_some_and(|d| !d.is_delivered())
    }

    /// Drop one lock. `false` if it was not held.
    pub fn remove(&mut self, locker_ship: i64, target_ship: i64) -> bool {
        let Some(targets) = self.held.get_mut(&locker_ship) else {
            return false;
        };
        let had = targets.remove(&target_ship);
        if targets.is_empty() {
            self.held.remove(&locker_ship);
        }
        self.due.remove(&(locker_ship, target_ship));
        had
    }

    /// Drop every lock this ship holds, returning what it was holding.
    pub fn clear(&mut self, locker_ship: i64) -> Vec<i64> {
        self.due.retain(|(locker, _), _| *locker != locker_ship);
        self.held
            .remove(&locker_ship)
            .map(|t| t.into_iter().collect())
            .unwrap_or_default()
    }

    /// What this ship has locked **and finished locking**, in a stable
    /// order. A lock still being acquired is not a lock (#225) `[I]`: this is
    /// the reader for anything that acts through a lock.
    pub fn targets_of(&self, locker_ship: i64) -> Vec<i64> {
        self.held
            .get(&locker_ship)
            .map(|t| {
                t.iter()
                    .copied()
                    .filter(|target| {
                        self.due
                            .get(&(locker_ship, *target))
                            .is_none_or(Delivered::is_delivered)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What this ship holds, **including locks still being acquired**, in a
    /// stable order: for the count against the target limit, for the
    /// unlock-everything paths and for what `GetTargets` answers.
    pub fn held_of(&self, locker_ship: i64) -> Vec<i64> {
        self.held
            .get(&locker_ship)
            .map(|t| t.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Who has locked this ship, in a stable order.
    ///
    /// Linear over lockers, for the same reason `Roster` filters linearly: at
    /// the scale this server is for the scan is free, and a second index is a
    /// second thing that can disagree with the first.
    pub fn targeters_of(&self, target_ship: i64) -> Vec<i64> {
        self.held
            .iter()
            .filter(|(_, targets)| targets.contains(&target_ship))
            .map(|(locker, _)| *locker)
            .collect()
    }

    /// Take a ship out of the world entirely.
    ///
    /// Returns `(what it had locked, who had locked it)` -- **both**, because
    /// a ship leaving space breaks locks in two directions and the module
    /// header's ghost is what happens when only one is handled.
    pub fn drop_ship(&mut self, ship: i64) -> (Vec<i64>, Vec<i64>) {
        let was_locking = self.clear(ship);
        let was_locked_by = self.targeters_of(ship);
        for locker in &was_locked_by {
            self.remove(*locker, ship);
        }
        (was_locking, was_locked_by)
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// How many more targets this ship may lock, given what it already holds.
///
/// **The limit is ours to enforce, and that is settled rather than assumed.**
/// `TryLockTarget`'s four client-side guards are "already locked", "no ball",
/// "the group is a planet, moon, asteroid belt or cosmic anomaly" and a
/// self-lock test that applies *only* while piloting a structure `[V]`. None
/// of them is a count or a range check, and `IsInTargetingRange` is defined
/// and never called from the lock path. So a permissive server is wire-safe,
/// and a server that enforces nothing lets a frigate lock forty things.
pub fn room_for_another(held: usize, max_locked_targets: f64) -> bool {
    let cap = if max_locked_targets.is_finite() && max_locked_targets > 0.0 {
        max_locked_targets as usize
    } else {
        0
    };
    held < cap
}

/// One `OnTarget` notification.
///
/// ```text
/// OnTarget(what, tid, reason)      # 1..3 positional; reason may be omitted
/// ```
///
/// `targetMgr.OnTarget` (`targetMgr.py:660`) dispatches on `what` `[V]`:
/// `'add'` completes a lock, `'lost'` ends one of ours, `'clear'` ends all of
/// ours, `'otheradd'` means someone locked *us*, `'otherlost'` means they
/// stopped. Every arm then chains `ProcessTargetChanged`, whose only
/// subscriber discards all three arguments, so **no unknown `what` can
/// raise**.
///
/// `reason` is accepted and never read, with exactly one exception:
/// `'Exploding'` routes to `ClearDeadTarget` (`:1327`) and plays the target
/// box's `anim_disappear()` instead of closing it flat `[V]`. So any string
/// is safe and that one is load-bearing.
///
/// **On the wire it rides in an `OnMultiEvent` batch** ([`Push::to_wire`]),
/// as a live server sends it: every `OnTarget` in a recorded session of a
/// new character, 808 of them of all six kinds, came as a `dogmaMessages`
/// entry `(('OnTarget', what, tid, reason), stamp)` and none on its own
/// **[V-TQ]**. godma's `OnMultiEvent_` gathers those entries and scatters
/// `OnTargets(entries)`, which `targetMgr.OnTargets` turns back into
/// `OnTarget(*entry[1:])` -- the handler above, unchanged -- and which the
/// node graphs' `Target Exploded` event (atom 167) also hears, firing on a
/// `('lost', tid, 'Exploding')` entry **[V]**. Sent as a bare `OnTarget`,
/// the target bar worked and that event never fired: the AIR tutorial's
/// task 2.1.02 waits on it for its mined-out rock. `OnTargets` itself is
/// still not sent; the client builds it.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetChange {
    /// Character ids. A lock is between ships; an audience is always
    /// characters (`crate::notify::Push::audience`).
    pub to: Vec<i64>,
    pub what: What,
    /// The *other* ship in the relationship, from the recipient's point of
    /// view: the target for `add`/`lost`, the locker for `otheradd`/
    /// `otherlost`. `None` only for [`What::Clear`], which names nothing.
    pub ship: Option<i64>,
    pub reason: Option<Reason>,
}

/// `OnTarget`'s first argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// A lock of ours completed. Only needed when `AddTarget` answered
    /// "wait"; our `AddTarget` answers "locked", so this is unused today and
    /// is the one-line change that buys the authentic acquisition delay.
    Add,
    /// A lock of ours ended.
    Lost,
    /// **All** our locks ended.
    Clear,
    /// Somebody locked us.
    OtherAdd,
    /// Somebody stopped locking us.
    OtherLost,
}

impl What {
    fn name(self) -> &'static [u8] {
        match self {
            What::Add => b"add",
            What::Lost => b"lost",
            What::Clear => b"clear",
            What::OtherAdd => b"otheradd",
            What::OtherLost => b"otherlost",
        }
    }
}

/// `OnTarget`'s third argument: never read, except for one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Unlocked,
    OutOfRange,
    /// A ship went into active warp (WARP-UNLOCK). EvEJS names it
    /// `WarpingOut`; the client reads no reason but `Exploding`, so the name
    /// only labels the log. [A: from EvEJS, falsified by a client that logs
    /// an unknown reason].
    WarpingOut,
    /// The client's own lock-abort sentinel, which it already expects.
    ShipChanged,
    /// The locker cloaked (CLOAK-1): its locks are dropped. The client reads
    /// no reason but `Exploding`, so this only labels the log.
    Cloaked,
    /// **Load-bearing**, and the only one that is: it routes to
    /// `ClearDeadTarget` and plays the box's disappear animation rather than
    /// closing it flat `[V]`.
    Exploding,
}

impl Reason {
    fn name(self) -> &'static [u8] {
        match self {
            Reason::Unlocked => b"Unlocked",
            Reason::OutOfRange => b"OutOfRange",
            Reason::WarpingOut => b"WarpingOut",
            Reason::ShipChanged => b"DeniedShipChanged",
            Reason::Exploding => b"Exploding",
            Reason::Cloaked => b"Cloaked",
        }
    }
}

impl Push for TargetChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let mut args = vec![PyValue::Bytes(self.what.name().to_vec())];
        // **Positional and trailing-optional**, so a `None` in the middle is
        // not available: `OnTarget(what, tid, reason)` with `tid` omitted is
        // what `'clear'` sends, and it sends *one* argument rather than three
        // with holes in them.
        if let Some(ship) = self.ship {
            args.push(PyValue::Int(ship));
            if let Some(reason) = self.reason {
                args.push(PyValue::Bytes(reason.name().to_vec()));
            }
        }
        ("OnTarget", args)
    }

    /// `OnMultiEvent([(('OnTarget', *args), stamp)])`: one batch of one
    /// entry, the shape godma's `OnMultiEvent` groups by its last element
    /// **[V]** (`godma.OnMultiEvent`, `itemgetter(-1)`). The stamp is the
    /// blue time now, as a live server's was **[V-TQ]**.
    fn to_wire(&self) -> (&'static str, Vec<PyValue>) {
        let (name, args) = self.to_push();
        let mut event = vec![PyValue::Bytes(name.as_bytes().to_vec())];
        event.extend(args);
        let entry = PyValue::Tuple(vec![
            PyValue::Tuple(event),
            PyValue::Int(crate::pyutil::filetime_now()),
        ]);
        ("OnMultiEvent", vec![PyValue::List(vec![entry])])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// **An `otheradd` stands only while its lock does** (RAT-GHOST): the
    /// locker still holds the victim's ship in the lock graph. A victim who
    /// died (or left space) while the push waited out the acquisition delay
    /// has had its `otherlost` already; delivering the `otheradd` after it
    /// would draw a ghost targeter. Every other change is unconditional.
    fn still_stands(&self, w: &Registry) -> bool {
        let (What::OtherAdd, Some(locker), &[victim]) = (self.what, self.ship, &self.to[..]) else {
            return true;
        };
        w.roster()
            .get(victim)
            .is_some_and(|p| w.locks().held_of(locker).contains(&p.ship_id))
    }

    /// An `add` or `otheradd` naming a ship, to one character: what a lock
    /// still acquiring has queued and `unlock_via` recalls.
    fn cancel_tag(&self) -> Option<crate::notify::CancelTag> {
        use crate::notify::CancelTag;
        let (&[to], Some(ship)) = (&self.to[..], self.ship) else {
            return None;
        };
        match self.what {
            What::Add => Some(CancelTag::TargetAdd { to, ship }),
            What::OtherAdd => Some(CancelTag::TargetOtherAdd { to, ship }),
            _ => None,
        }
    }
}

/// Lock a target, or refuse with a `UserError` the client treats as a clean
/// rejection.
///
/// **Returns `(0, [])` -- a falsy flag and an empty list `[V]`.** The falsy
/// flag is what makes `_LockTarget` call `OnTargetAdded`, and the list is
/// stored and never read. Returning `(1, [])` instead would mean "wait for a
/// push", which is how the authentic acquisition delay would be built
/// (§1.1 of the spec) and which this deliberately does not do: the client's
/// own progress bar is already running from its own clock, so an instant
/// server answer gives the player the bar they expect followed by a lock.
///
/// Every refusal is a raise. `_LockTarget`'s handler catches `UserError`,
/// calls `FailLockTarget` and swallows the re-raise, so **any** key is a
/// clean rejection and the key only decides what the player reads `[V]`.
/// `ship` is in an activated warp now (aligning is ordinary flight) **and
/// has not yet landed**. The one test the lock start and an NPC's lock share.
///
/// **The record is carried to now first** (WARP-LOCK-FIX): a roster entry is
/// true only as of its ship's last command, and `Mode::Warp` becomes
/// `Mode::Stop` only when `Motion::advanced_at` sees the curve finish. A
/// pilot who warped and then sent nothing (a player sitting at the belt he
/// warped to) kept `Mode::Warp` with a due stamp forever, so he could never
/// be locked again. Advanced only when the stale record says "in warp", so a
/// ship in ordinary flight is not touched.
pub fn in_active_warp(w: &mut Registry, ship: i64) -> bool {
    let Some(p) = w.roster().by_ship(ship).copied() else {
        return false;
    };
    if !p.motion.warp_active() {
        return false;
    }
    w.advance_character(p.character_id);
    w.roster()
        .by_ship(ship)
        .is_some_and(|p| p.motion.warp_active())
}

pub fn lock(
    w: &mut Registry,
    actor: i64,
    target_ship: i64,
) -> Result<PyValue, crate::service::ServiceError> {
    let Some(here) = w.roster().get(actor).copied() else {
        // Not in space. `TargetingAttemptCancelled` is the silent key: no
        // dialog, no log line, and the client already treats it as a
        // sentinel. A player who cannot be here does not need telling.
        return Err(silently());
    };
    // **The client only refuses a self-lock while piloting a structure**
    // `[V]`, so an ordinary pilot really can ask, and we really must say no:
    // a ship locked to itself is a weapon that can fire on itself.
    if target_ship == here.ship_id {
        return Err(refused("DeniedTargetSelf", vec![]));
    }
    // CLOAK-2: a lock attempt ends the gate cloak (a device cloak refuses).
    crate::cloak::break_jump(w, actor);
    // CLOAK-1: a cloaked pilot locks nothing, nobody locks a cloaked ship,
    // and the recalibration after a decloak holds the lock back.
    if let Some(key) =
        crate::cloak::lock_refusal(w, here.ship_id, target_ship, crate::pyutil::filetime_now())
    {
        return Err(refused(key, vec![]));
    }
    // Lockable means "another ship in this solar system" **or "a rock in
    // it"**. Stations and celestials are deliberately not: the client's own
    // guards exclude planets, moons, belts and anomalies, and nothing in EVE
    // locks a station. A rock is the one non-ship the client offers "Lock
    // target" on (`OfferLockTarget`, `celestialCheckers.py:971`) `[V]`, and
    // answering the silent cancel for a rock that is right there would be a
    // lie (`docs/lanes/asteroids.md` §4.2).
    //
    // **A hackable can too** (HK2): `OfferLockTarget` does not exclude
    // group 1207, and `const.nonTargetableGroups` -- which does list it --
    // is read only by the click-to-lock helper and the camera [V], so the
    // menu's "Lock target" reaches us. Any other site object still gets the
    // silent cancel.
    // **A ship in ACTIVE warp cannot be locked** (WARP-LOCK #191): the
    // silent key, as for a target that is not there. The sweep ends a lock
    // at activation; this stops one from starting.
    if in_active_warp(w, target_ship) {
        return Err(silently());
    }
    let lockable = w
        .roster()
        .by_ship(target_ship)
        .is_some_and(|p| p.solar_system_id == here.solar_system_id)
        || w.rocks()
            .get(target_ship)
            .is_some_and(|r| r.solar_system_id == here.solar_system_id)
        || crate::hacking::is_lockable(w, target_ship, here.solar_system_id)
        || crate::salvage::is_lockable(w, target_ship, here.solar_system_id)
        // NW-2: an anchored (or anchorable) deployable has hull points.
        || crate::anchor::is_lockable(w, target_ship, here.solar_system_id)
        // A Sovereignty Hub (SOC-D3a): the one thing an Entosis Link needs.
        || crate::entosis::is_lockable(w, target_ship, here.solar_system_id)
        // An Upwell structure in this system (#45): `OfferLockTarget` has no
        // structure exclusion `[V]`, so the client asks and we agree.
        || crate::structure_combat::is_lockable(w, target_ship, here.solar_system_id);
    if !lockable {
        return Err(silently());
    }
    // Already held: the client's own first guard is "already locked", so
    // this is not a state change and not an event, and it owes nothing to
    // the jam or the limit below (a full computer still answers for a pair
    // it holds). **#234: a re-lock of a pair still being acquired is "not
    // yet", `(1, [])`**, the answer the first call got: `(0, [])` made a
    // re-locking bot draw and fire on a lock that does not exist.
    if w.locks().held_of(here.ship_id).contains(&target_ship) {
        if w.locks().is_pending(here.ship_id, target_ship) {
            return Ok(PyValue::Tuple(vec![
                PyValue::Int(1),
                PyValue::List(Vec::new()),
            ]));
        }
        return Ok(locked_now());
    }
    let held = w.locks().held_of(here.ship_id).len();
    let limit = crate::service::max_locked_targets_of(
        w.content(),
        w.pilot(actor).map_or(0, |p| p.ship_type_id),
    );
    // A CONCORD jam caps the locks for its cycle (CONCORD-1a).
    let limit = crate::concord::jam_limit(w, here.ship_id, limit);
    // An ECM cycle that took hold refuses every new lock until it ends
    // (K-EWAR).
    // EWAR-ICONS: the refusal has its own key, naming what was asked for.
    if crate::ewar::jammed(w, here.ship_id) {
        let name = pilot_of_ship(w, target_ship)
            .and_then(|c| w.character(c).map(|c| c.name.clone()))
            .unwrap_or_default();
        return Err(refused(
            "TargetingSystemsBeingJammed",
            vec![("targetname", PyValue::Unicode(name))],
        ));
    }
    let limit = crate::ewar::lock_limit(w, here.ship_id, limit);
    if !room_for_another(held, limit) {
        return Err(refused(
            "TargetingSystemsAlreadyFullyUtilized",
            vec![("limit", PyValue::Int(limit as i64))],
        ));
    }
    if !w.locks_mut().add(here.ship_id, target_ship) {
        // Already locked. The client's own first guard is "already locked",
        // so this is not a state change and not an event. Defensive: the
        // early return above answers a held or pending lock first, so this
        // is not reached today (#262).
        return Ok(locked_now());
    }
    // UNDOCK-INVULN: locking a target gives up the protection.
    crate::invuln::break_for(w, actor);
    let victim = pilot_of_ship(w, target_ship);
    // **The lock takes time, and the time is the client's own formula.**
    let delay = acquisition_delay(w, actor, target_ship);
    // Held from now, complete when the clock delivers the `add` (#225):
    // until then no effect reads it as a lock.
    let done = Delivered::default();
    if delay >= crate::push::TICK {
        w.locks_mut()
            .mark_pending(here.ship_id, target_ship, done.clone());
    }
    if delay < crate::push::TICK {
        // Faster than the clock that would carry the push, so there is
        // nothing to wait for: answer "locked" and let the client draw it.
        if let Some(victim) = victim {
            w.push(TargetChange {
                to: vec![victim],
                what: What::OtherAdd,
                ship: Some(here.ship_id),
                reason: None,
            });
        }
        return Ok(locked_now());
    }
    // **`(1, [])` means "not yet".** `_LockTarget`'s `if not flag` is then
    // false, so it does *not* call `OnTargetAdded`; the progress bar it
    // already started keeps running and the lock completes when our
    // `OnTarget('add')` arrives `[V]` (`docs/re/targeting.md` §3.1). The
    // client's bar is driven by its own clock from the same two numbers, so
    // the two agree without being synchronised.
    //
    // **`schedule`, not `push_later`, and the difference is whether a quiet
    // player ever gets their lock.** `push_later` queues on `pending`, and
    // `Registry::take_pending` is called from exactly one place:
    // `dispatch.rs`, after a payload. So a deferred completion sat there
    // until the client happened to call something else -- a player who locks
    // a target and then sits still would watch the progress bar fill and
    // hang, for ever. The real client polls often enough to hide it; a bot
    // can be genuinely idle, and that is how this was found
    // (`eve-bot`'s `a_lock_is_dropped_when_its_target_leaves_range` saw no
    // completion in 25 s because it made no calls, while the test beside it
    // passed only because it happened to call `GetTargets` first).
    //
    // `scheduled` is drained by the clock, which runs whether anybody is
    // talking or not. That is what the second queue is for, and
    // `Registry::schedule`'s own doc already listed this case.
    w.schedule(
        delay,
        Completes {
            change: TargetChange {
                to: vec![actor],
                what: What::Add,
                ship: Some(target_ship),
                reason: None,
            },
            done,
        },
    );
    // The victim learns when the lock *completes*, not when it is attempted:
    // in the client a target bracket appears on them at the end of the
    // acquisition, and telling them early would show a lock that might still
    // be cancelled.
    if let Some(victim) = victim {
        w.schedule(
            delay,
            TargetChange {
                to: vec![victim],
                what: What::OtherAdd,
                ship: Some(here.ship_id),
                reason: None,
            },
        );
    }
    Ok(PyValue::Tuple(vec![
        PyValue::Int(1),
        PyValue::List(Vec::new()),
    ]))
}

/// Abandon a lock the player cancelled while it was still acquiring.
///
/// **Silent on purpose.** `targetMgr.CancelAddTarget` does the whole client
/// side itself -- clears the state, stops the sound, scatters
/// `OnFailLockTarget` -- and *then* calls us `[V]`, so anything we sent would
/// be telling the client something it already did.
///
/// **The race, stated rather than hidden.** Now that a lock takes time, the
/// completion is a push queued with a delay, and a queued push cannot be
/// recalled -- it is serialised into the connection's outbox at the end of
/// the turn that made it. So a player who cancels *after* that and *before*
/// the delay elapses gets an `OnTarget('add')` for a lock this server has
/// just dropped, and their client draws a target box the server does not
/// believe in. It heals on the next session change, because
/// `godma.RefreshTargets` re-reads `GetTargets` and ours will not list it.
///
/// The honest fix is the same one §30.4 of `docs/port-tracker.md` describes
/// for warp arrival: scheduled work that can be cancelled. Until then this is
/// a rare, self-healing cosmetic disagreement, and it is strictly better than
/// the alternative of locks that complete instantly.
pub fn cancel(w: &mut Registry, actor: i64, target_ship: i64) {
    let Some(ship) = w.roster().get(actor).map(|p| p.ship_id) else {
        return;
    };
    w.locks_mut().remove(ship, target_ship);
    // The completion still queued would draw a lock the player cancelled.
    let victim = pilot_of_ship(w, target_ship);
    let _ = cancel_completion(w, actor, victim, ship, target_ship);
}

/// `(0, [])` -- a falsy flag, which is what makes `_LockTarget` draw the lock
/// at once.
fn locked_now() -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(0), PyValue::List(Vec::new())])
}

/// How long this ship takes to lock that one.
///
/// **The client's own formula**, `bracketMgr.py:1305` `[V]`:
///
/// ```text
/// min(180000, 40e6 / scanResolution / asinh(signatureRadius)^2)  ms
/// ```
///
/// `scanResolution` is our ship's attribute 564 -- ours to serve, so ours to
/// read -- and `signatureRadius` is the **target's type's** attribute 552,
/// which the client reads out of its own FSD and we therefore never send.
/// The *bar* the client draws uses that unpainted type value `[V]`
/// (`bracketMgr.GetScanSpeed` `:1063-1085`, `godma.GetType`, not `GetItem`),
/// so it cannot know a target painter. **Our** lock time reads the signature
/// as flown (`dogma::hull`: fitted, painted, a drone's or squadron's own) `[I]`:
/// a painted target locks sooner than its bar says, and the lock is pushed
/// then (#269). The locker's side (scan resolution, dampeners) is unchanged.
///
/// Computing it rather than picking a number is the whole point: the client's
/// progress bar is already running on its own clock from these same two
/// values, so a server that lands its push at the same moment produces a bar
/// that fills and *then* a lock. A server that guesses produces a bar that
/// finishes early and hangs, or a lock before the bar is full.
///
/// A capsule locking a capsule comes out around five seconds; a Rifter on a
/// Rifter about three and a half.
///
/// `actor` is a roster key, so an NPC (`crate::npc`) times its own locks with
/// this too, and a player locking an NPC reads the entity's signature.
pub(crate) fn acquisition_delay(w: &Registry, actor: i64, target_ship: i64) -> std::time::Duration {
    let scan = w.scan_resolution_of(w.pilot(actor).map_or(0, |p| p.ship_type_id))
        // A sensor dampener on the locker (K-EWAR): 1.0 undamped.
        * w.roster()
            .get(actor)
            .map_or(1.0, |p| crate::ewar::scan_factor(w, p.ship_id));
    // **A rock's signature is its ball radius**, as the client reads it:
    // `bracketMgr.GetScanSpeed` (`:1063-1085`) takes `signatureRadius` only
    // if `godma.GetType(typeID).AttributeExists('signatureRadius')`, and
    // `AttributeExists` (`godma.py:743`) looks at the type's own attribute
    // rows, never the attribute default. No ore type carries 552 `[V-data]`,
    // so the client falls back to `bp.GetBall(id).radius`, the radius we
    // send in the ball (the quantity law's, 90 m and up). An earlier note
    // here took the default, 100 m: it agrees only for a rock near 100 m.
    // **[A]** a hackable can's and a wreck's is the same default: no can type
    // carries 552 that we have checked; falsified by a can whose lock bar
    // finishes before or after our `OnTarget('add')`.
    let signature = if let Some(rock) = w.rocks().get(target_ship) {
        if rock.radius > 0.0 {
            rock.radius
        } else {
            crate::asteroid::SIGNATURE_RADIUS
        }
    } else if crate::hacking::can(w, target_ship).is_some()
        || crate::salvage::place(w, target_ship).is_some()
    {
        crate::asteroid::SIGNATURE_RADIUS
    } else if crate::entosis::target_of(w, target_ship).is_some() {
        // A hub's signature radius is its type's (SOC-D3a).
        crate::entosis::SIGNATURE_RADIUS
    } else if let Some(t) = crate::structure_combat::fixed_target(w, target_ship) {
        // An Upwell structure's own 552, else its ball radius (#45).
        t.signature
    } else {
        // A ship, drone or squadron: the signature **as flown** -- fitted,
        // painted (`ewar::signature_factor`) and, for a drone or squadron,
        // its own hull (`dogma::hull`). Falls back to the type's table row.
        crate::dogma::hull(w, target_ship).map_or_else(
            || {
                w.roster()
                    .by_ship(target_ship)
                    .and_then(|p| w.pilot(p.character_id))
                    .map(|p| p.ship_type_id)
                    .and_then(|t| crate::destiny::hull::find_in(w.content(), t))
                    .map_or(crate::destiny::hull::FALLBACK.signature_radius, |h| {
                        h.signature_radius
                    })
                    // `dogma::hull` paints its own; the table row is painted here.
                    * crate::ewar::signature_factor(w, target_ship)
            },
            |h| h.signature_radius,
        )
    };
    let shaped = signature.asinh().powi(2);
    if scan <= 0.0 || !shaped.is_finite() || shaped <= 0.0 {
        // No usable numbers is "instant", not "never": a lock that cannot be
        // timed is better drawn than withheld.
        return std::time::Duration::ZERO;
    }
    let millis = (40.0e6 / scan / shaped).min(180_000.0);
    if !millis.is_finite() || millis <= 0.0 {
        return std::time::Duration::ZERO;
    }
    std::time::Duration::from_millis(millis as u64)
}

/// Drop one lock, telling both sides.
///
/// **The locker has to be told too**, which is the part that is not obvious:
/// unlock is not return-confirmed, because `RemoveTarget` is followed by
/// `RemoveFromTeam`, whose body opens with `if self.IsObserving():` and is a
/// no-op for any ordinary pilot `[V]`. Without the push the target box stays
/// on screen for ever.
pub fn unlock(w: &mut Registry, actor: i64, target_ship: i64, reason: Reason) {
    unlock_via(w, actor, target_ship, reason, Delivery::WithAnswer);
}

/// Which queue a lock teardown's notifications leave by.
///
/// **Not a detail.** `Registry::push` fills `pending`, drained by `dispatch`
/// after each payload; `Registry::schedule` fills `scheduled`, drained by the
/// clock. Two queues with one owner each, and §30.4 records the attempt that
/// mixed them. An unlock the *clock* decided on must not go into `pending`,
/// where it would sit until the player happened to call something -- a lock
/// silently kept past its range until you clicked, which is worse than not
/// dropping it.
///
/// **The caller is not free to pick.** The question it answers is factual:
/// *is there a payload turn in flight that this teardown rides out on?* An
/// unlock the player asked for has one; a lock dropped by the range sweep and
/// a ship torn out of the graph by a **disconnect** do not. `server.rs`
/// closes a socket without ever entering `dispatch`, so there is no drain of
/// `pending` to join, and the frame would wait on a turn that may never come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// In the same flush as the answer to the call that caused it.
    WithAnswer,
    /// On the clock, because nobody asked.
    OnTheClock,
}

pub(crate) fn unlock_via(
    w: &mut Registry,
    actor: i64,
    target_ship: i64,
    reason: Reason,
    delivery: Delivery,
) {
    let Some(here) = w.roster().get(actor).copied() else {
        return;
    };
    if !w.locks_mut().remove(here.ship_id, target_ship) {
        return;
    }
    let victim = pilot_of_ship(w, target_ship);
    // **A lock still acquiring is torn down differently** (LOCK-WARP). Its
    // completion is a scheduled `OnTarget('add')`; left queued it arrives
    // after the 'lost' and the client draws a lock the server dropped (the
    // owner watched a Merlin warp off and lock anyway). Recalling it also
    // recalls the victim's 'otheradd', so the victim hears nothing: he was
    // never told of the lock.
    let pending = cancel_completion(w, actor, victim, here.ship_id, target_ship);
    // An NPC locker (`crate::npc`) has no target box to close.
    let locker_known = w.character(actor).is_some();
    if locker_known && pending {
        // **The client has no push that aborts an acquiring lock**
        // (`targetMgr.OnTargetLost` clears `pendingTargets` and the box but
        // not `targeting[tid]`, which only `OnTargetAdded`, `FailLockTarget`
        // and `CancelAddTarget` clear, and `TryLockTarget` refuses a target
        // still in it). So the locker gets the completion and the loss
        // back to back: the add resets the countdown and the sound, the
        // lost closes the box. [A: read in targetMgr.py, not run; falsified
        // by a target box flashing for a frame, or a bar that hangs.]
        deliver(
            w,
            delivery,
            TargetChange {
                to: vec![actor],
                what: What::Add,
                ship: Some(target_ship),
                reason: None,
            },
        );
    }
    if locker_known {
        deliver(
            w,
            delivery,
            TargetChange {
                to: vec![actor],
                what: What::Lost,
                ship: Some(target_ship),
                reason: Some(reason),
            },
        );
    }
    if let (Some(victim), false) = (victim, pending) {
        deliver(
            w,
            delivery,
            TargetChange {
                to: vec![victim],
                what: What::OtherLost,
                ship: Some(here.ship_id),
                reason: Some(reason),
            },
        );
    }
}

/// Recall the scheduled completion of a lock still acquiring: the locker's
/// `'add'` and the victim's `'otheradd'`. True when it was still queued,
/// that is when the lock had not completed.
fn cancel_completion(
    w: &mut Registry,
    locker: i64,
    victim: Option<i64>,
    locker_ship: i64,
    target_ship: i64,
) -> bool {
    let n = w.cancel_scheduled(|p| completion_matches(p, locker, victim, locker_ship, target_ship));
    n > 0
}

/// Whether a scheduled push is the completion of the lock `locker` has on
/// `target_ship`: the locker's `'add'` or the victim's `'otheradd'`. Read
/// from the push's [`crate::notify::CancelTag`], not from `to_push`.
fn completion_matches(
    p: &dyn Push,
    locker: i64,
    victim: Option<i64>,
    locker_ship: i64,
    target_ship: i64,
) -> bool {
    use crate::notify::CancelTag;
    match p.cancel_tag() {
        Some(CancelTag::TargetAdd { to, ship }) => to == locker && ship == target_ship,
        Some(CancelTag::TargetOtherAdd { to, ship }) => Some(to) == victim && ship == locker_ship,
        _ => false,
    }
}

/// The old test, on `to_push` and `audience`: the oracle for
/// [`completion_matches`].
#[cfg(test)]
fn completion_matches_slow(
    p: &dyn Push,
    locker: i64,
    victim: Option<i64>,
    locker_ship: i64,
    target_ship: i64,
) -> bool {
    let (name, args) = p.to_push();
    if name != "OnTarget" {
        return false;
    }
    let aud = p.audience().unwrap_or_default();
    let what = args.first().and_then(crate::pyutil::as_bytes);
    let ship = args.get(1);
    (what == Some(b"add".as_slice()) && aud == [locker] && ship == Some(&PyValue::Int(target_ship)))
        || (what == Some(b"otheradd".as_slice())
            && Some(&aud[..]) == victim.as_ref().map(std::slice::from_ref)
            && ship == Some(&PyValue::Int(locker_ship)))
}

/// Drop every lock whose target has left the locker's targeting range.
///
/// **The gap this closes.** `Reason::OutOfRange` has existed since locking was
/// written and was never sent by anything: a lock survived any distance, so a
/// ship that warped off stayed locked on the locker's screen for ever. The
/// missing piece was never the reason, it was somebody to notice -- which is
/// exactly what `Registry::schedule`'s own doc lists as one of the three
/// things the second queue exists for, beside the warp's arrival
/// reconciliation and a bot acting while every human is idle.
///
/// Called from the clock, so the notifications go out `OnTheClock`. Returns
/// how many it dropped, for the caller's log.
///
/// Range is the **locker's** attribute 76 and the comparison is centre to
/// centre. Two refinements deliberately not made: EVE measures to the
/// target's surface, and a lock dropped exactly at the boundary will flap if
/// the ship is drifting. Both want a hysteresis band, and a band wants a
/// number from the client rather than one invented here `[A]`. Falsified by a
/// lock that drops and re-acquires repeatedly while a ship hovers at range.
#[cfg(test)]
pub fn sweep_out_of_range(w: &mut Registry) -> usize {
    sweep_out_of_range_stats(w).dropped
}

/// [`sweep_out_of_range`] with what it cost to find out (lane
/// fight-stall-fix, I-3): the pairs walked, the scheduled queue it began
/// with, and the time and entries `cancel_scheduled` took inside it.
pub fn sweep_out_of_range_stats(w: &mut Registry) -> crate::clock_probe::SweepStats {
    let mut stats = crate::clock_probe::SweepStats {
        scheduled_at_start: w.scheduled_len(),
        ..Default::default()
    };
    let mark = crate::clock_probe::cancel_mark();
    let mut dropped = 0;
    let pairs = w.locks().pairs();
    stats.pairs = pairs.len();
    // **Both ends carried to now first.** A roster entry is true only as
    // of its ship's last command (`Registry::advance_character`), so a
    // ship told to approach a rock and then left alone is still, here,
    // where it was when it was told -- and its lock on the rock it has
    // since reached was dropped as out of range on the next tick. Found
    // by the P8b bots: approach, lock, `OnTarget('lost', rock,
    // 'OutOfRange')`, every time (`docs/lanes/bots.md`, P8b note).
    //
    // **Once per pilot, not once per pair**: a pilot with five locks, or a
    // rock mined by twenty, was advanced (and its system scanned for
    // tethers) every time. Gather the ids, then advance each once -- a
    // system with a tethered pair is stepped as one.
    let mut movers = std::collections::BTreeSet::new();
    for (locker_ship, target_ship) in &pairs {
        for ship in [locker_ship, target_ship] {
            if let Some(p) = w.roster().by_ship(*ship) {
                movers.insert((p.solar_system_id, p.character_id));
            }
        }
    }
    let mut stepped_systems = std::collections::BTreeSet::new();
    let mut paired: std::collections::BTreeMap<i64, bool> = std::collections::BTreeMap::new();
    for (system, id) in movers {
        let pair = *paired
            .entry(system)
            .or_insert_with(|| w.steps_as_system(system));
        if pair {
            if stepped_systems.insert(system) {
                w.advance_system(system);
            }
        } else {
            w.advance_unpaired(id);
        }
    }
    let now = crate::space_state::now_seconds();
    for (locker_ship, target_ship) in pairs {
        let Some(locker) = w.roster().by_ship(locker_ship).copied() else {
            continue;
        };
        // **A ship in ACTIVE warp holds no lock and bears none**
        // (WARP-UNLOCK). Aligning is ordinary flight and keeps them
        // (INWARP-AUDIT); the tick the activation stamp comes due, both
        // directions end here, with the pairs the sweep already walks, so no
        // extra scan. Before this the locks ended only when the distance
        // grew past range, many ticks into the tunnel. EvEJS drops the
        // outgoing locks of a ship in active warp and refuses a lock on one;
        // we drop at activation, not at align [A: falsified by a client
        // whose lock bar outlives the align in a live capture].
        let target_warps = w
            .roster()
            .by_ship(target_ship)
            .is_some_and(|t| t.motion.warp_active_at(now));
        if locker.motion.warp_active_at(now) || target_warps {
            unlock_via(
                w,
                locker.character_id,
                target_ship,
                Reason::WarpingOut,
                Delivery::OnTheClock,
            );
            dropped += 1;
            continue;
        }
        // **A ship, or a rock.** A rock never leaves space, so a lock on one
        // ends only here or by the player's hand; without this branch a rock
        // stayed locked from any distance (`docs/lanes/asteroids.md` §8,
        // risk 6).
        // `surface`: how much of the centre gap is the target itself. Only a
        // sov target gives it, and must: a hub's radius can match a hull's
        // whole range. The rest stay centre to centre (see the doc above).
        let (target_system, target_position, surface) =
            if let Some(target) = w.roster().by_ship(target_ship).copied() {
                (target.solar_system_id, target.motion.position, 0.0)
            } else if let Some(rock) = w.rocks().get(target_ship) {
                (rock.solar_system_id, rock.position, 0.0)
            } else if let Some((can, system)) = crate::hacking::can(w, target_ship) {
                // A hackable can (HK2) never moves either.
                (system, can.position, 0.0)
            } else if let Some((system, at)) = crate::salvage::place(w, target_ship) {
                // A wreck (X-SALV) lies where it is, or where a beam left it.
                (system, at, 0.0)
            } else if let Some(hub) = crate::entosis::lock_position(w, target_ship) {
                // Nor does a Sovereignty Hub (SOC-D3a).
                hub
            } else if let Some(t) = crate::structure_combat::fixed_target(w, target_ship) {
                // An Upwell structure (#45): range is to its surface, as the
                // hub's [A: a lock dropped while the client's range circle
                // still covers the structure].
                (t.system, t.position, t.radius)
            } else {
                // The target left space entirely. Not this function's
                // business: whoever removed them owes the push, and
                // inventing one here would double it.
                continue;
            };
        if locker.solar_system_id != target_system {
            continue;
        }
        // A sensor dampener on the locker cuts it (K-EWAR): 1.0 undamped.
        let range = w.max_target_range_of_ship(
            locker.ship_id,
            w.pilot(locker.character_id).map_or(0, |p| p.ship_type_id),
        ) * crate::ewar::range_factor(w, locker.ship_id);
        if range <= 0.0 {
            continue;
        }
        let gap = crate::space_state::distance(locker.motion.position, target_position) - surface;
        if gap > range {
            unlock_via(
                w,
                locker.character_id,
                target_ship,
                Reason::OutOfRange,
                Delivery::OnTheClock,
            );
            dropped += 1;
        }
    }
    stats.dropped = dropped;
    stats.finish(mark);
    stats
}

/// Drop every lock this pilot holds.
///
/// One `'clear'` to the locker rather than one `'lost'` each -- the client has
/// a handler for exactly this -- and one `'otherlost'` per victim, because
/// nothing else would tell them.
pub fn clear_all(w: &mut Registry, actor: i64, reason: Reason) {
    let Some(here) = w.roster().get(actor).copied() else {
        return;
    };
    // A lock still acquiring has its completion queued; left there it would
    // arrive after the `clear` (#225).
    for t in w.locks().held_of(here.ship_id) {
        let victim = pilot_of_ship(w, t);
        let _ = cancel_completion(w, actor, victim, here.ship_id, t);
    }
    let dropped = w.locks_mut().clear(here.ship_id);
    if dropped.is_empty() {
        return;
    }
    w.push(TargetChange {
        to: vec![actor],
        what: What::Clear,
        ship: None,
        reason: None,
    });
    for target_ship in dropped {
        if let Some(victim) = pilot_of_ship(w, target_ship) {
            w.push(TargetChange {
                to: vec![victim],
                what: What::OtherLost,
                ship: Some(here.ship_id),
                reason: Some(reason),
            });
        }
    }
}

/// Take a pilot's ship out of the lock graph, both ways.
///
/// **Called from every path that takes a ship out of space** -- docking,
/// disconnecting, dying -- and the reason it has to exist separately from
/// [`clear_all`] is the ghost in this module's header: the client never
/// clears `targetedBy` on ball removal, so a victim keeps a bracket on a
/// locker who left, `IsTargeted()` stays true, and her *next* lock logs an
/// error and silently skips the indicator.
///
/// The departing pilot is told nothing: their own `Park.Release` tears their
/// state down, and a push arriving after that is discarded while one arriving
/// before it is pointless.
///
/// **Every frame here is addressed to somebody else**, which is why
/// [`Delivery`] is a parameter and not a constant. Docking is a call and the
/// dock's answer drains `pending`; a disconnect is not a call at all, and a
/// victim whose `'otherlost'` sat in `pending` kept the ghost bracket this
/// function exists to prevent until *any* connection happened to ask us
/// something. That is the same defect `session_closed` already fixes for the
/// station guest list and the ball removal on either side of this call.
pub(crate) fn left_space(w: &mut Registry, actor: i64, reason: Reason, delivery: Delivery) {
    let Some(ship) = w.roster().get(actor).map(|p| p.ship_id) else {
        return;
    };
    // Recall the queued completion of every lock still acquiring, in both
    // directions, before the pairs are dropped (#225).
    for t in w.locks().held_of(ship) {
        let victim = pilot_of_ship(w, t);
        let _ = cancel_completion(w, actor, victim, ship, t);
    }
    for locker_ship in w.locks().targeters_of(ship) {
        if let Some(locker) = pilot_of_ship(w, locker_ship) {
            let _ = cancel_completion(w, locker, Some(actor), locker_ship, ship);
        }
    }
    let (was_locking, was_locked_by) = w.locks_mut().drop_ship(ship);
    for target_ship in was_locking {
        if let Some(victim) = pilot_of_ship(w, target_ship) {
            deliver(
                w,
                delivery,
                TargetChange {
                    to: vec![victim],
                    what: What::OtherLost,
                    ship: Some(ship),
                    reason: Some(reason),
                },
            );
        }
    }
    // And everyone who had *us* locked loses their box. `BallsRemoved` would
    // clear it on its own for a ship leaving the grid, but not for a ship
    // that merely became untargetable, and a redundant `'lost'` costs one
    // frame while a missing one costs a stuck box.
    for locker_ship in was_locked_by {
        if let Some(locker) = pilot_of_ship(w, locker_ship) {
            deliver(
                w,
                delivery,
                TargetChange {
                    to: vec![locker],
                    what: What::Lost,
                    ship: Some(ship),
                    reason: Some(reason),
                },
            );
        }
    }
}

/// [`left_space`] for a pilot who **stays in space in another hull**: a ship
/// death that ejects the capsule (RAT-GHOST).
///
/// `left_space` tells the departing pilot nothing because their own
/// `Park.Release` clears their target state. A podded pilot's park is not
/// released: the `shipid` change runs `ProcessSessionChange`'s in-space
/// branch, which calls `OnTargetClear()` and leaves `targetedBy` alone
/// (`docs/re/targeting.md` §5.1) `[V]`. Without one `'otherlost'` per locker
/// the capsule pilot keeps every locker of the dead ship in `targetedBy`
/// for ever (fci_hunter_bot, 2026-11-03: five belt rats still listed 600 s
/// after they killed the Rifter).
///
/// `[A]` an `'otherlost'` that lands before the `shipid` change clears the
/// same `targetedBy` list as one after it; falsified by a capsule pilot
/// whose `GetTargetedBy()` still lists the dead ship's lockers.
pub(crate) fn lost_ship_in_space(w: &mut Registry, actor: i64, reason: Reason, delivery: Delivery) {
    let Some(ship) = w.roster().get(actor).map(|p| p.ship_id) else {
        return;
    };
    let lockers = w.locks().targeters_of(ship);
    left_space(w, actor, reason, delivery);
    if w.character(actor).is_none() {
        return;
    }
    for locker_ship in lockers {
        deliver(
            w,
            delivery,
            TargetChange {
                to: vec![actor],
                what: What::OtherLost,
                ship: Some(locker_ship),
                reason: Some(reason),
            },
        );
    }
}

/// A pilot who steps out of `ship` into another ship in space (eject, board)
/// is told `'otherlost'` for every locker of the ship he left, so the client's
/// `targetedBy` does not keep them for the new ship. The locks themselves stay
/// in the graph: the hull is still in space and still targeted (which is what
/// makes `CantBoardTargeted` work). Call it before the roster changes.
///
/// `[A]` the client clears `targetedBy` on `'otherlost'` whatever the ship id
/// is now; falsified by a boarded pilot whose `GetTargetedBy()` still lists the
/// old hull's lockers.
pub(crate) fn left_ship_for_another(w: &mut Registry, actor: i64, old_ship: i64, reason: Reason) {
    for locker_ship in w.locks().targeters_of(old_ship) {
        deliver(
            w,
            Delivery::WithAnswer,
            TargetChange {
                to: vec![actor],
                what: What::OtherLost,
                ship: Some(locker_ship),
                reason: Some(reason),
            },
        );
    }
}

/// The **character** flying this ball, for a notification's audience.
///
/// `None` for a ball nobody is in the roster with, and for an NPC's
/// (`crate::npc`): an NPC is in the roster under its own id and has no
/// session, so an `OnTarget` addressed to it is noise on the queue rather
/// than a message to anyone. Its own state reads the lock graph instead.
fn pilot_of_ship(w: &Registry, ship: i64) -> Option<i64> {
    w.roster()
        .by_ship(ship)
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
}

/// One notification, onto the queue [`Delivery`] names.
///
/// `Duration::ZERO` on the clock path and not a delay: "as soon as the world
/// ticker looks" is the earliest a push can leave without a turn, and nothing
/// here is waiting for anything -- the lock is already gone from the graph.
fn deliver(w: &mut Registry, delivery: Delivery, n: TargetChange) {
    match delivery {
        Delivery::WithAnswer => w.push(n),
        Delivery::OnTheClock => w.schedule(std::time::Duration::ZERO, n),
    }
}

/// `GetTargets` and `GetTargeters`, which are how a lock survives a reload.
///
/// `godma.RefreshTargets` iterates both and re-scatters an `OnTarget` for
/// each `[V]`, and `targetMgr.Run` calls it synchronously whenever
/// `session.shipid and not session.stationid`. Answering `[]` for ever --
/// which this server did until now -- loses every lock at the next session
/// change.
///
/// A list, not `None`: both are iterated, so `None` is a `TypeError` on the
/// view-open path, and `target` is one of `SpaceView.__dependencies__`
/// started outside any try block -- a raise here means the space view never
/// appears at all.
pub fn targets_of_actor(w: &Registry, actor: Option<i64>) -> PyValue {
    ship_list(w, actor, |locks, ship| locks.held_of(ship))
}

pub fn targeters_of_actor(w: &Registry, actor: Option<i64>) -> PyValue {
    ship_list(w, actor, |locks, ship| locks.targeters_of(ship))
}

fn ship_list(w: &Registry, actor: Option<i64>, pick: fn(&Locks, i64) -> Vec<i64>) -> PyValue {
    let ships = actor
        .and_then(|a| w.roster().get(a))
        .map(|p| pick(w.locks(), p.ship_id))
        .unwrap_or_default();
    PyValue::List(ships.into_iter().map(PyValue::Int).collect())
}

/// A refusal the player is shown, formatted by the client from FSD.
fn refused(key: &'static str, args: Vec<(&'static str, PyValue)>) -> crate::service::ServiceError {
    crate::service::ServiceError::Raise(eve_proto::user_error(
        key,
        // `(value, key)`: marshal dict entries are inverted on the wire.
        args.into_iter()
            .map(|(k, v)| (v, crate::pyutil::bstr(k)))
            .collect(),
    ))
}

/// A refusal the player is **not** shown.
///
/// `TargetingAttemptCancelled` is caught by name in `_LockTarget`'s own
/// handler and returns without a dialog or a log line `[V]`. It is the right
/// answer for a lock that could not have been asked for legitimately -- a
/// pilot who is not in space, a target that is not a ship or a rock in this
/// system --
/// where a message would only describe a situation the player cannot see.
fn silently() -> crate::service::ServiceError {
    crate::service::ServiceError::Raise(eve_proto::user_error(
        "TargetingAttemptCancelled",
        Vec::new(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target change goes on the wire as a one-entry `OnMultiEvent` whose
    /// entry is `(('OnTarget', what, tid, reason), stamp)`, while `to_push`
    /// stays the bare `OnTarget` the server's own readers match on.
    #[test]
    fn a_target_change_rides_in_a_multi_event_batch() {
        let lost = TargetChange {
            to: vec![7],
            what: What::Lost,
            ship: Some(42),
            reason: Some(Reason::Exploding),
        };
        let (name, args) = lost.to_push();
        assert_eq!(name, "OnTarget");
        let (wire, batch) = lost.to_wire();
        assert_eq!(wire, "OnMultiEvent");
        let [PyValue::List(entries)] = &batch[..] else {
            panic!("one list: {batch:?}")
        };
        let [PyValue::Tuple(entry)] = &entries[..] else {
            panic!("one entry: {entries:?}")
        };
        let [PyValue::Tuple(call), PyValue::Int(stamp)] = &entry[..] else {
            panic!("(call, stamp): {entry:?}")
        };
        assert!(*stamp > 0);
        let mut want = vec![PyValue::Bytes(b"OnTarget".to_vec())];
        want.extend(args);
        assert_eq!(call, &want);
        assert_eq!(
            call[1..],
            [
                PyValue::Bytes(b"lost".to_vec()),
                PyValue::Int(42),
                PyValue::Bytes(b"Exploding".to_vec())
            ]
        );
    }

    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_980_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;

    /// Two pilots in one system, both in space.
    fn two_in_space() -> Registry {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let bob = crate::model::Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: BOB_SHIP,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice, bob],
            ..crate::model::SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        reg
    }

    /// Every `OnTarget` currently queued, as `(audience, what, ship)`.
    fn pushed(reg: &mut Registry) -> Vec<(Vec<i64>, Vec<u8>, Option<i64>)> {
        reg.take_pending()
            .into_iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                if name != "OnTarget" {
                    return None;
                }
                let what = match args.first() {
                    Some(PyValue::Bytes(b)) => b.clone(),
                    _ => return None,
                };
                let ship = match args.get(1) {
                    Some(PyValue::Int(i)) => Some(*i),
                    _ => None,
                };
                Some((p.audience().unwrap_or_default(), what, ship))
            })
            .collect()
    }

    /// **The whole feature, in the order the client sees it.**
    ///
    /// Alice locks Bob. The lock takes time -- the client's own formula on a
    /// capsule's numbers is about five seconds -- so the call answers
    /// `(1, [])`, "not yet", which leaves the progress bar the client already
    /// started running and completes when our delayed `OnTarget('add')`
    /// arrives `[V]`.
    ///
    /// This asserted `(0, [])` and an immediate push until a player reported
    /// that locks were instant. They were: `(0, [])` is the "draw it now"
    /// answer, and this test was pinning it.
    ///
    /// The server-side lock is recorded **immediately** even though the
    /// client's is not, which is what makes `GetTargets` right during the
    /// acquisition and is why `CancelAddTarget` has work to do.
    #[test]
    fn alice_locks_bob_and_the_lock_takes_time() {
        let mut reg = two_in_space();
        let answer = lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        assert_eq!(
            answer,
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::List(vec![])]),
            "a truthy flag is 'not yet'; a falsy one draws the lock at once \
             and is what made locks instant"
        );

        // **Both confirmations are on the CLOCK queue, not the pending one,
        // and the assertion is behavioural because that is the property.**
        //
        // This read `take_pending()` and the delay off the entry. They are
        // scheduled now: `pending` is drained only by `dispatch` after a
        // payload, so a deferred completion sat there until the client
        // happened to call something else -- a player who locks and then sits
        // still watched the bar fill and hang. `eve-bot` found it by being
        // genuinely idle, which the real client never is.
        //
        // So: nothing is due at once, and both are released together once the
        // delay has passed. That is stronger than reading a `Duration` off a
        // queue entry -- it proves they are actually withheld.
        let names = |batch: Vec<Box<dyn crate::notify::Push>>| -> Vec<(Vec<i64>, Vec<u8>)> {
            batch
                .into_iter()
                .filter_map(|p| {
                    let (name, args) = p.to_push();
                    if name != "OnTarget" {
                        return None;
                    }
                    match args.first() {
                        Some(PyValue::Bytes(b)) => {
                            Some((p.audience().unwrap_or_default(), b.clone()))
                        }
                        _ => None,
                    }
                })
                .collect()
        };
        assert!(
            names(reg.take_due(std::time::Instant::now())).is_empty(),
            "nothing may be due at once: an immediate completion is exactly the \
             instant-lock defect this test was written for"
        );
        // Past any capsule's acquisition, which is about five seconds.
        let released =
            names(reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(30)));
        assert!(
            released
                .iter()
                .any(|(to, what)| to == &vec![ALICE] && what == b"add"),
            "the locker's completion should be released by the clock; got {released:?}"
        );
        assert!(
            released
                .iter()
                .any(|(to, what)| to == &vec![BOB] && what == b"otheradd"),
            "and the victim's, in the same batch -- both sides learn at the same \
             moment; got {released:?}"
        );

        // The lock exists on our side straight away, which is what
        // `GetTargets` has to report during the acquisition.
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![PyValue::Int(BOB_SHIP)])
        );
        assert_eq!(
            targeters_of_actor(&reg, Some(BOB)),
            PyValue::List(vec![PyValue::Int(ALICE_SHIP)]),
            "which is Bob's only way to rediscover it after a reload"
        );
        assert_eq!(targets_of_actor(&reg, Some(BOB)), PyValue::List(vec![]));
    }

    /// **The delay is the client's own arithmetic, not a number we picked.**
    ///
    /// The client's progress bar runs on `min(180000, 40e6 / scanResolution /
    /// asinh(sigRadius)^2)` ms from its own clock `[V]`. If the server's push
    /// lands at a different moment the player sees a bar that fills and then
    /// hangs, or a lock before the bar is full -- so the property is that our
    /// delay *is* that formula on the same two numbers.
    ///
    /// Computed here independently from the table rather than compared
    /// against a constant, so a change to either column has to move both.
    #[test]
    fn the_acquisition_delay_is_the_clients_own_formula() {
        let reg = two_in_space();
        let ours = acquisition_delay(&reg, ALICE, BOB_SHIP);

        let ship_type_id = reg.character(ALICE).expect("Alice").ship_type_id;
        let scan = reg.scan_resolution_of(ship_type_id);
        let signature = crate::destiny::hull::find_in(reg.content(), ship_type_id)
            .expect("a known hull")
            .signature_radius;
        let expected = (40.0e6 / scan / signature.asinh().powi(2)).min(180_000.0);
        assert!(
            (ours.as_millis() as f64 - expected).abs() < 2.0,
            "{ours:?} against the client's {expected} ms"
        );
        // And it is a plausible EVE number rather than an accident.
        assert!(
            ours.as_millis() > 500 && ours.as_millis() < 20_000,
            "{ours:?} is not the right order for a lock"
        );
    }

    /// A lock the player abandons mid-acquisition is dropped, silently.
    ///
    /// `CancelAddTarget` was a true no-op while locks completed instantly,
    /// because there was nothing pending to cancel. Now there is, and leaving
    /// it would make `GetTargets` report a target the player gave up on.
    #[test]
    fn a_cancelled_lock_is_forgotten_and_nothing_is_sent() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        let _ = reg.take_pending();
        assert!(!reg.locks().held_of(ALICE_SHIP).is_empty());

        cancel(&mut reg, ALICE, BOB_SHIP);
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![]),
            "the abandoned lock must not survive in GetTargets"
        );
        assert!(
            pushed(&mut reg).is_empty(),
            "the client did its own cleanup before calling us"
        );
    }

    /// Unlocking tells **both** sides, and the locker's half is the one that
    /// is easy to miss.
    ///
    /// `RemoveTarget`'s return value is discarded, and `RemoveFromTeam` --
    /// the client's own cleanup -- opens with `if self.IsObserving():` and is
    /// a no-op for any ordinary pilot `[V]`. So a server that only tells the
    /// victim leaves the locker's own target box on screen for ever.
    #[test]
    fn unlocking_closes_the_box_on_both_screens() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        let _ = reg.take_pending();
        // The lock completes first: unlocking one still acquiring is a
        // different path (LOCK-WARP).
        let _ = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60));

        unlock(&mut reg, ALICE, BOB_SHIP, Reason::Unlocked);
        let frames = pushed(&mut reg);
        assert!(
            frames.contains(&(vec![ALICE], b"lost".to_vec(), Some(BOB_SHIP))),
            "the locker is not told by the return value: {frames:?}"
        );
        assert!(
            frames.contains(&(vec![BOB], b"otherlost".to_vec(), Some(ALICE_SHIP))),
            "and the victim is not told by anything else: {frames:?}"
        );
        assert_eq!(targets_of_actor(&reg, Some(ALICE)), PyValue::List(vec![]));
    }

    /// **Docking breaks the lock in both directions, which is the ghost.**
    ///
    /// The client never clears `targetedBy` on ball removal, so a victim who
    /// is not told keeps `threatTargetsMe` on a bracket that no longer
    /// exists, `IsTargeted()` stays true, and her *next* lock logs an error
    /// and silently skips the indicator -- self-perpetuating, and invisible
    /// from the locker's side.
    ///
    /// Asserted through `Registry::dock_change` rather than by calling
    /// `left_space` directly, because the bug this guards against is the
    /// wiring: the lock teardown has to happen *before* the roster forgets
    /// the ship, or there is no ball id to name.
    #[test]
    fn docking_while_holding_a_lock_does_not_leave_a_ghost_bracket() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        let _ = reg.take_pending();

        reg.dock_change(Some(ALICE));
        let frames = pushed(&mut reg);
        assert!(
            frames.contains(&(vec![BOB], b"otherlost".to_vec(), Some(ALICE_SHIP))),
            "Bob keeps a bracket on a ship that docked: {frames:?}"
        );
        assert!(
            reg.locks().is_empty(),
            "and nothing is left pointing at a ship out of space: {:?}",
            reg.locks()
        );
    }

    /// **A disconnect has no turn to ride out on, so the clock has to carry
    /// the lock teardown.**
    ///
    /// The sibling of `service.rs`'s
    /// `a_departure_is_announced_by_the_clock_not_by_a_turn`, and found by
    /// auditing the other half of the same question: `session_closed` already
    /// put the station guest frame and the `BallsRemoved` on the clock queue
    /// because "a disconnect is not a payload", and the `left_space` call
    /// sitting *between* those two was still using `pending`.
    ///
    /// Nothing drains `pending` on a socket close -- `server.rs` never enters
    /// `dispatch` -- so Bob's `'otherlost'` waited for *any* connection's next
    /// call. Alone in the system, that is for ever, and what he keeps is the
    /// self-perpetuating ghost bracket: `IsTargeted()` stays true and his
    /// next lock silently skips its indicator.
    ///
    /// Asserted through `session_closed` rather than by calling `left_space`
    /// directly, for the same reason the docking test is: the bug is the
    /// wiring, not the function.
    #[test]
    fn a_disconnect_tells_the_victim_by_the_clock_not_by_a_turn() {
        let mut reg = two_in_space();
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 10_000_000_001,
            session_id: 10_000_000_001,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(10_000_000_001, ALICE);
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        // Both queues cleared, so what is left is what the disconnect itself
        // produced: the lock's own deferred completion is on the clock queue
        // and would otherwise be counted as the answer.
        let _ = reg.take_pending();
        let _ = reg.take_due(std::time::Instant::now());

        reg.session_closed(10_000_000_001);

        let on_target = |batch: Vec<Box<dyn crate::notify::Push>>| -> Vec<(Vec<i64>, Vec<u8>)> {
            batch
                .into_iter()
                .filter_map(|p| {
                    let (name, args) = p.to_push();
                    if name != "OnTarget" {
                        return None;
                    }
                    match args.first() {
                        Some(PyValue::Bytes(b)) => {
                            Some((p.audience().unwrap_or_default(), b.clone()))
                        }
                        _ => None,
                    }
                })
                .collect()
        };
        let due = on_target(reg.take_due(std::time::Instant::now()));
        let pending = on_target(
            reg.take_pending()
                .into_iter()
                .map(|(_, n)| n)
                .collect::<Vec<_>>(),
        );
        assert!(
            due.contains(&(vec![BOB], b"otherlost".to_vec())),
            "the clock has to carry it: due={due:?} pending={pending:?}"
        );
        assert!(
            pending.is_empty(),
            "nothing may be left on the turn queue, which a disconnect never \
             drains; pending={pending:?}"
        );
        assert!(reg.locks().is_empty(), "and the graph is clear either way");
    }

    /// A pilot cannot lock their own ship, and the client will let them ask.
    ///
    /// `TryLockTarget`'s self-lock guard applies **only while piloting a
    /// structure** `[V]`, so this refusal is ours alone. Every refusal is a
    /// raise, which `_LockTarget` turns into a clean rejection.
    #[test]
    fn a_pilot_cannot_lock_their_own_ship() {
        let mut reg = two_in_space();
        assert!(
            lock(&mut reg, ALICE, ALICE_SHIP).is_err(),
            "a ship locked to itself is a weapon that can fire on itself"
        );
        assert!(reg.locks().is_empty());
        // And nothing outside this grid, which is the other half of
        // "lockable": a ball id we cannot resolve to a ship in this system.
        assert!(lock(&mut reg, ALICE, 424_242).is_err());
        assert!(reg.locks().is_empty());
    }

    /// Locking is idempotent, and unlocking something unlocked is not an
    /// event.    /// Locking is idempotent, and unlocking something unlocked is not an
    /// event.
    ///
    /// Both matter at the wire: the client's own first guard is "already
    /// locked", so a duplicate `AddTarget` is a client bug rather than a
    /// state change, and a `RemoveTarget` for a lock we do not hold must not
    /// produce a push that closes some *other* target box.
    #[test]
    fn a_second_lock_on_the_same_ship_is_not_a_second_lock() {
        let mut locks = Locks::default();
        assert!(locks.add(1, 2));
        assert!(!locks.add(1, 2), "the same lock twice is one lock");
        assert_eq!(locks.targets_of(1), vec![2]);
        assert!(locks.remove(1, 2));
        assert!(!locks.remove(1, 2), "unlocking twice is not an event");
        assert!(
            locks.is_empty(),
            "and an empty locker leaves no entry behind"
        );
    }

    /// **A ship leaving space breaks locks in both directions.**
    ///
    /// The module header's ghost: `targetedBy` is never cleared by ball
    /// removal, so a victim keeps a bracket on a locker who docked, and her
    /// next lock logs an error and skips the indicator. The property is that
    /// `drop_ship` reports *both* sides and leaves nothing behind -- a fix
    /// that cleared only the departing ship's own locks would pass a
    /// one-sided test and reproduce the ghost exactly.
    #[test]
    fn a_ship_leaving_space_breaks_locks_in_both_directions() {
        let mut locks = Locks::default();
        locks.add(1, 2); // 1 locks 2
        locks.add(3, 2); // 3 locks 2 as well
        locks.add(2, 4); // and 2 locks 4

        let (was_locking, was_locked_by) = locks.drop_ship(2);
        assert_eq!(was_locking, vec![4], "2 had 4 locked");
        assert_eq!(was_locked_by, vec![1, 3], "and 1 and 3 had locked 2");
        assert!(
            locks.is_empty(),
            "nothing may be left pointing at a ship that is gone: {locks:?}"
        );
    }

    /// **#225: a lock being acquired is held but not complete.** `targets_of`
    /// answers completed locks, `held_of` both, and every way out clears the
    /// stamp.
    #[test]
    fn a_pending_lock_is_held_but_not_complete_until_it_is_delivered() {
        let done = Delivered::default();
        let mut locks = Locks::default();
        assert!(locks.add_pending(1, 2, done.clone()));
        assert!(locks.add(1, 3));
        assert!(
            !locks.add_pending(1, 2, Delivered::default()),
            "already held changes nothing"
        );
        assert_eq!(locks.targets_of(1), vec![3], "2 is not a lock yet");
        assert_eq!(locks.held_of(1), vec![2, 3], "but it is held");
        assert_eq!(locks.targeters_of(2), vec![1]);
        done.deliver();
        assert_eq!(locks.targets_of(1), vec![2, 3], "delivered: complete");
        assert!(locks.remove(1, 2));
        assert!(locks.add(1, 2), "a removed lock leaves no stale stamp");
        assert_eq!(locks.targets_of(1), vec![2, 3]);
        assert!(locks.add_pending(4, 5, Delivered::default()));
        assert_eq!(locks.clear(4), vec![5]);
        assert!(locks.add(4, 5));
        assert_eq!(locks.targets_of(4), vec![5]);
    }

    /// **#225, the production path: a lock is complete exactly when the clock
    /// delivers its `add`**, under a clock that is not the wall clock. The
    /// bots fire the moment they see the add; a stamp compared against the
    /// wall clock said "still pending" there.
    #[test]
    fn a_lock_is_complete_the_moment_the_clock_delivers_its_add() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        let delay = acquisition_delay(&reg, ALICE, BOB_SHIP);
        // The test clock: just short of the due time, then just past it,
        // both measured from a fixed origin that the wall clock has not
        // reached the delay beyond.
        let origin = std::time::Instant::now();
        let early = reg.take_due(origin + delay / 2);
        assert!(
            early
                .iter()
                .all(|p| p.to_push().1.first() != Some(&PyValue::Bytes(b"add".to_vec()))),
            "not delivered yet"
        );
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty());
        let delivered = reg.take_due(origin + delay + std::time::Duration::from_secs(5));
        assert!(
            delivered
                .iter()
                .any(|p| p.to_push().1.first() == Some(&PyValue::Bytes(b"add".to_vec()))),
            "the add goes out"
        );
        assert_eq!(
            reg.locks().targets_of(ALICE_SHIP),
            vec![BOB_SHIP],
            "and with it the lock is complete, whatever the wall clock says"
        );
    }

    /// **#234: a re-lock of a pair still being acquired answers `(1, [])`**
    /// ("not yet"), not `(0, [])` ("locked"); once the add is delivered the
    /// same call answers `(0, [])`. No second completion is queued.
    #[test]
    fn a_relock_of_a_pending_pair_answers_not_yet() {
        let not_yet = PyValue::Tuple(vec![PyValue::Int(1), PyValue::List(Vec::new())]);
        let mut reg = two_in_space();
        assert_eq!(lock(&mut reg, ALICE, BOB_SHIP).expect("lock"), not_yet);
        let queued = reg.scheduled_count();
        assert!(queued > 0);
        assert_eq!(lock(&mut reg, ALICE, BOB_SHIP).expect("re-lock"), not_yet);
        assert_eq!(reg.scheduled_count(), queued, "nothing more is queued");
        let delay = acquisition_delay(&reg, ALICE, BOB_SHIP);
        let _ = reg.take_due(std::time::Instant::now() + delay + std::time::Duration::from_secs(5));
        assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);
        assert_eq!(
            lock(&mut reg, ALICE, BOB_SHIP).expect("re-lock"),
            locked_now(),
            "complete: locked"
        );
    }

    /// **#234 (F5): a re-lock of a pair already held is answered by the pair,
    /// not by the limit.** With the computer full, locking a held pair again
    /// is "not yet" while it acquires and "locked" after, never
    /// `TargetingSystemsAlreadyFullyUtilized`.
    #[test]
    fn a_relock_of_a_held_pair_on_a_full_computer_is_not_refused() {
        let not_yet = PyValue::Tuple(vec![PyValue::Int(1), PyValue::List(Vec::new())]);
        let mut reg = two_in_space();
        assert_eq!(lock(&mut reg, ALICE, BOB_SHIP).expect("lock"), not_yet);
        // Fill the computer with invented filler ships up to the capsule limit.
        let limit = crate::service::max_locked_targets_of(reg.content(), 0) as usize;
        for i in 0..limit.saturating_sub(1) {
            assert!(reg.locks_mut().add(ALICE_SHIP, 900_000_000 + i as i64));
        }
        assert!(!room_for_another(
            reg.locks().held_of(ALICE_SHIP).len(),
            limit as f64
        ));
        assert_eq!(
            lock(&mut reg, ALICE, BOB_SHIP).expect("re-lock while full"),
            not_yet
        );
        let delay = acquisition_delay(&reg, ALICE, BOB_SHIP);
        let _ = reg.take_due(std::time::Instant::now() + delay + std::time::Duration::from_secs(5));
        assert_eq!(
            lock(&mut reg, ALICE, BOB_SHIP).expect("re-lock while full"),
            locked_now()
        );
    }

    /// **A lock still acquiring is recalled by a clear or by a ship leaving
    /// space**: the queued `add` must not arrive after them (#225).
    #[test]
    fn clear_all_and_leaving_space_recall_a_lock_still_acquiring() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        assert!(reg.scheduled_count() > 0, "the completion is queued");
        clear_all(&mut reg, ALICE, Reason::Unlocked);
        let sent = due(&mut reg);
        assert!(
            sent.iter()
                .all(|(to, w)| !(to == &vec![ALICE] && w.as_slice() == b"add")),
            "no add to the locker after the clear: {sent:?}"
        );
        assert_eq!(reg.scheduled_count(), 0);

        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        left_space(&mut reg, BOB, Reason::Unlocked, Delivery::OnTheClock);
        let sent = due(&mut reg);
        assert!(
            sent.iter().all(|(_, w)| w.as_slice() != b"add"),
            "the locked ship leaving recalls the locker's add: {sent:?}"
        );
        assert_eq!(reg.scheduled_count(), 0);
    }

    /// **#225 through `lock`**: while the acquisition runs the lock is held
    /// (the limit counts it, `GetTargets` lists it, as before) but no effect
    /// sees it; the fast path, with nothing to wait for, is complete at once.
    #[test]
    fn a_lock_being_acquired_is_not_yet_a_lock_for_effects() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        assert!(
            acquisition_delay(&reg, ALICE, BOB_SHIP) >= std::time::Duration::from_secs(3),
            "the fixture has a lock that takes seconds, so a slow run cannot finish it"
        );
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty());
        assert_eq!(reg.locks().held_of(ALICE_SHIP), vec![BOB_SHIP]);
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![PyValue::Int(BOB_SHIP)]),
            "the wire answer still lists it"
        );
    }

    /// The reverse lookup finds every locker and only the lockers.
    #[test]
    fn targeters_are_the_ships_holding_a_lock_on_you() {
        let mut locks = Locks::default();
        locks.add(1, 9);
        locks.add(2, 9);
        locks.add(3, 8);
        assert_eq!(locks.targeters_of(9), vec![1, 2]);
        assert_eq!(locks.targeters_of(8), vec![3]);
        assert_eq!(locks.targeters_of(7), Vec::<i64>::new());
        assert_eq!(locks.clear(1), vec![9]);
        assert_eq!(locks.targeters_of(9), vec![2], "and clearing one frees one");
    }

    /// The limit is a count of what is held against the hull's own attribute,
    /// and a hull with no attribute may lock nothing.
    ///
    /// Pins the zero case deliberately: `maxLockedTargets` defaults to `0` in
    /// the client's FSD and a capsule's really is zero, so "no attribute" has
    /// to mean "no locks" here rather than "no limit". What a *pod* is served
    /// is a separate decision, made once in `service.rs`.
    #[test]
    fn the_lock_limit_counts_what_is_held_and_zero_means_none() {
        assert!(room_for_another(0, 4.0));
        assert!(room_for_another(3, 4.0));
        assert!(!room_for_another(4, 4.0), "four of four is full");
        assert!(!room_for_another(0, 0.0), "a hull that cannot lock, cannot");
        assert!(!room_for_another(0, f64::NAN));
        assert!(!room_for_another(0, -1.0));
    }

    /// `'clear'` sends one argument, not three with holes in them.
    ///
    /// `OnTarget(what, tid, reason)` is positional with trailing optionals,
    /// so a `None` standing in for `tid` would be read *as* a target id. The
    /// client's `OnTargetClear` takes no arguments at all.
    #[test]
    fn clear_names_nothing_and_a_loss_names_its_reason() {
        let clear = TargetChange {
            to: vec![7],
            what: What::Clear,
            ship: None,
            reason: None,
        };
        let (name, args) = clear.to_push();
        assert_eq!(name, "OnTarget");
        assert_eq!(args, vec![PyValue::Bytes(b"clear".to_vec())]);

        let lost = TargetChange {
            to: vec![7],
            what: What::Lost,
            ship: Some(42),
            reason: Some(Reason::Exploding),
        };
        let (_, args) = lost.to_push();
        assert_eq!(
            args,
            vec![
                PyValue::Bytes(b"lost".to_vec()),
                PyValue::Int(42),
                // The one reason the client reads: it plays the target box's
                // disappear animation instead of closing it flat.
                PyValue::Bytes(b"Exploding".to_vec()),
            ]
        );
        assert_eq!(lost.audience(), Some(vec![7]));
    }

    /// Alice in space, with rocks: five in her system, four in the next.
    /// The counts and law are a fixture; nothing asserts them as the
    /// client's.
    fn alice_at_a_belt() -> Registry {
        belt_world_with(Vec::new())
    }

    fn belt_world_with(extra: Vec<crate::model::Character>) -> Registry {
        let celestials = crate::celestial::Catalogue::parse(
            "belt\t40000003\t15\t0.0\t100000.0\t0.0\t0.0\n\
             belt\t40000020\t15\t0.0\t-100000.0\t0.0\t0.0\n",
        );
        let counts = crate::asteroid::Counts::parse(
            "40000003\t0\t1230\t5\t30000001\t0.5\t90.0\t16255.0\n\
             40000020\t5\t1230\t4\t30990002\t0.5\t90.0\t16255.0\n",
        );
        let rocks = crate::asteroid::Field::generate(
            &counts,
            &celestials,
            &crate::asteroid::Quantities::default(),
        );
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let mut characters = vec![alice];
        characters.extend(extra);
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters,
            celestials,
            rocks,
            ..crate::model::SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        reg
    }

    fn rock_in(reg: &Registry, system: i64) -> crate::asteroid::Rock {
        *reg.rocks().in_system(system).next().expect("a rock")
    }

    /// **A rock in your system can be locked**, and the lock completes for
    /// the locker alone: there is no victim to tell.
    #[test]
    fn a_rock_in_the_system_is_lockable() {
        let mut reg = alice_at_a_belt();
        let rock = rock_in(&reg, 30_000_001);
        let answer = lock(&mut reg, ALICE, rock.id).expect("a rock is lockable");
        assert!(
            answer == locked_now()
                || answer == PyValue::Tuple(vec![PyValue::Int(1), PyValue::List(vec![])]),
            "{answer:?}"
        );
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![PyValue::Int(rock.id)])
        );
        let later = std::time::Instant::now() + std::time::Duration::from_secs(600);
        let mut frames: Vec<(Vec<i64>, Vec<u8>, Option<i64>)> = Vec::new();
        for p in reg.take_due(later).into_iter() {
            let (name, args) = p.to_push();
            if name != "OnTarget" {
                continue;
            }
            let what = match args.first() {
                Some(PyValue::Bytes(b)) => b.clone(),
                _ => continue,
            };
            let ship = match args.get(1) {
                Some(PyValue::Int(i)) => Some(*i),
                _ => None,
            };
            frames.push((p.audience().unwrap_or_default(), what, ship));
        }
        frames.extend(pushed(&mut reg));
        if answer != locked_now() {
            assert!(
                frames.contains(&(vec![ALICE], b"add".to_vec(), Some(rock.id))),
                "a 'not yet' answer owes the completion: {frames:?}"
            );
        }
        assert!(
            frames.iter().all(|(to, _, _)| to == &vec![ALICE]),
            "a rock has nobody to tell: {frames:?}"
        );
    }

    /// A rock in another system, or an id in the asteroid range that is not
    /// a rock, stays the silent refusal.
    #[test]
    fn a_rock_elsewhere_or_no_rock_is_not_lockable() {
        let mut reg = alice_at_a_belt();
        let far = rock_in(&reg, 30_990_002);
        assert!(lock(&mut reg, ALICE, far.id).is_err());
        assert!(lock(&mut reg, ALICE, crate::asteroid::MIN_ROCK_ID + 199).is_err());
        assert!(reg.locks().is_empty());
    }

    /// The delay on a rock is the client's formula with the rock's **ball
    /// radius** (no ore type has attribute 552, so `bracketMgr.GetScanSpeed`
    /// falls back to the ball), and a barge-like scan resolution locks it in
    /// a few seconds. Invented values: scan 500, rocks of 300 m and 3 km.
    #[test]
    fn a_rocks_lock_delay_reads_its_ball_radius_and_is_a_few_seconds() {
        let mut reg = alice_at_a_belt();
        let scan = reg.scan_resolution_of(reg.character(ALICE).unwrap().ship_type_id);
        let rock = rock_in(&reg, 30_000_001);
        let fixed = 40.0e6 / scan / 100.0_f64.asinh().powi(2);
        let mut last = f64::MAX;
        for radius in [300.0_f64, 3_000.0] {
            let mut r = rock;
            r.radius = radius;
            reg.rocks_mut().remove(rock.id);
            reg.rocks_mut().add(r);
            let ours = acquisition_delay(&reg, ALICE, rock.id).as_millis() as f64;
            let expected = (40.0e6 / scan / radius.asinh().powi(2)).min(180_000.0);
            assert!(
                (ours - expected).abs() < 2.0,
                "{radius}: {ours} vs {expected}"
            );
            assert!(ours < last && ours < 10_000.0, "{radius}: {ours} ms");
            last = ours;
        }
        // a 3 km rock is faster than the old fixed 100 m signature
        assert!(last < fixed, "{last} vs {fixed}");
    }

    /// A rock lock drops when the locker leaves range, like a ship lock.
    #[test]
    fn a_rock_lock_drops_out_of_range() {
        let mut reg = alice_at_a_belt();
        let rock = rock_in(&reg, 30_000_001);
        lock(&mut reg, ALICE, rock.id).expect("a lock");
        let _ = reg.take_pending();
        let near = [
            rock.position[0] + rock.radius + 1_000.0,
            rock.position[1],
            rock.position[2],
        ];
        reg.roster_mut().get_mut(ALICE).unwrap().motion.position = near;
        assert_eq!(sweep_out_of_range(&mut reg), 0, "in range is kept");
        reg.roster_mut().get_mut(ALICE).unwrap().motion.position = [1.0e9, 0.0, 0.0];
        assert_eq!(sweep_out_of_range(&mut reg), 1);
        assert!(reg.locks().is_empty());
    }

    // -- an Upwell structure (#45) ----------------------------------------------

    const STRUCTURE: i64 = 5_000_001;
    const STRUCTURE_RADIUS: f64 = 4_000.0;
    const STRUCTURE_SIGNATURE: f64 = 900.0;

    /// Alice and Bob in space, and an Astrahus-typed structure (invented
    /// radius and signature) in Alice's system, `at` metres from the origin.
    fn alice_and_a_structure(at: f64) -> Registry {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let rows = format!(
            "{}type\t{}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t{STRUCTURE_RADIUS}\t0\t\t{}={STRUCTURE_SIGNATURE}\tInvented Hull\n",
            crate::typedata::FIXTURE,
            crate::structure_space::ASTRAHUS,
            crate::structure_combat::ATTR_SIGNATURE_RADIUS,
        );
        let mut content = crate::content::Content::fixture();
        content.insert(
            "typedata",
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(&rows).expect("rows parse"),
            ),
        );
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let e = crate::structure::Env::new(0);
        let mut st = crate::structure::Structure::anchored(
            STRUCTURE,
            crate::structure_space::ASTRAHUS,
            11,
            system,
            [at, 0.0, 0.0],
            0.0,
            "Invented Home",
            1,
            &e,
        );
        st.set_state(crate::structure::State::ShieldVulnerable, &e);
        crate::structure::put(reg.deltas_mut(), &st);
        let _ = reg.take_pending();
        reg
    }

    #[test]
    fn a_structure_in_the_system_is_lockable_and_one_elsewhere_is_not() {
        let mut reg = alice_and_a_structure(10_000.0);
        lock(&mut reg, ALICE, STRUCTURE).expect("a structure is lockable");
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![PyValue::Int(STRUCTURE)])
        );
        // The same row in another system is the silent refusal.
        let mut away = alice_and_a_structure(10_000.0);
        let mut st = crate::structure::get(away.deltas(), STRUCTURE).unwrap();
        st.system += 1;
        crate::structure::put(away.deltas_mut(), &st);
        assert!(lock(&mut away, ALICE, STRUCTURE).is_err());
        assert!(away.locks().is_empty());
    }

    #[test]
    fn the_lock_time_reads_the_structures_signature() {
        let reg = alice_and_a_structure(10_000.0);
        let scan = reg.scan_resolution_of(reg.character(ALICE).unwrap().ship_type_id);
        let expected = (40.0e6 / scan / STRUCTURE_SIGNATURE.asinh().powi(2)).min(180_000.0);
        let ours = acquisition_delay(&reg, ALICE, STRUCTURE).as_millis() as f64;
        assert!((ours - expected).abs() < 2.0, "{ours} vs {expected}");
    }

    #[test]
    fn a_structure_beyond_range_by_surface_is_unlocked_by_the_sweep() {
        let mut reg = alice_and_a_structure(0.0);
        lock(&mut reg, ALICE, STRUCTURE).expect("a lock");
        let _ = reg.take_pending();
        let range = reg
            .max_target_range_of_ship(ALICE_SHIP, reg.pilot(ALICE).map_or(0, |p| p.ship_type_id));
        // The centre is farther than the range, the surface is inside it.
        reg.roster_mut().get_mut(ALICE).unwrap().motion.position =
            [range + STRUCTURE_RADIUS / 2.0, 0.0, 0.0];
        assert_eq!(sweep_out_of_range(&mut reg), 0, "the surface is in range");
        reg.roster_mut().get_mut(ALICE).unwrap().motion.position =
            [range + STRUCTURE_RADIUS * 2.0, 0.0, 0.0];
        assert_eq!(sweep_out_of_range(&mut reg), 1);
        assert!(reg.locks().is_empty());
    }

    /// Every `OnTarget` that leaves on the clock in the next minute, as
    /// `(audience, what)`.
    fn due(reg: &mut Registry) -> Vec<(Vec<i64>, Vec<u8>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .into_iter()
            .filter_map(|p| {
                let (name, args) = p.to_push();
                if name != "OnTarget" {
                    return None;
                }
                match args.first() {
                    Some(PyValue::Bytes(b)) => Some((p.audience().unwrap_or_default(), b.clone())),
                    _ => None,
                }
            })
            .collect()
    }

    /// **RAT-GHOST**: an `otheradd` still queued when its lock ends is dropped
    /// at delivery, so it cannot land after the `otherlost` and draw a ghost
    /// targeter; one whose lock still stands leaves as usual.
    #[test]
    fn a_queued_otheradd_is_dropped_when_its_lock_no_longer_stands() {
        let mut reg = two_in_space();
        let queue = |reg: &mut Registry| {
            reg.schedule(
                std::time::Duration::ZERO,
                TargetChange {
                    to: vec![BOB],
                    what: What::OtherAdd,
                    ship: Some(ALICE_SHIP),
                    reason: None,
                },
            );
        };
        assert!(reg.locks_mut().add(ALICE_SHIP, BOB_SHIP));
        queue(&mut reg);
        assert_eq!(
            due(&mut reg),
            vec![(vec![BOB], b"otheradd".to_vec())],
            "a standing lock is announced"
        );
        // The lock ends (the victim died) while the push is still queued.
        queue(&mut reg);
        assert!(reg.locks_mut().remove(ALICE_SHIP, BOB_SHIP));
        assert!(due(&mut reg).is_empty(), "no ghost after the lock ended");
    }

    /// **A lock still acquiring fails when the target leaves range** (LOCK-WARP):
    /// the completion is recalled, so Bob's warp does not end in a lock, and
    /// Bob, never told of it, hears nothing.
    #[test]
    fn a_lock_in_progress_fails_when_the_target_warps_out_of_range() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        assert_eq!(sweep_out_of_range(&mut reg), 0, "in range is kept");
        reg.roster_mut().get_mut(BOB).unwrap().motion.position = [1.0e12, 0.0, 0.0];
        assert_eq!(sweep_out_of_range(&mut reg), 1);
        assert!(reg.locks().is_empty());
        let sent = due(&mut reg);
        let to_alice: Vec<&[u8]> = sent
            .iter()
            .filter(|(to, _)| to == &vec![ALICE])
            .map(|(_, w)| w.as_slice())
            .collect();
        assert_eq!(
            to_alice,
            [b"add".as_slice(), b"lost".as_slice()],
            "the add resets the client's countdown, the lost closes the box; \
             and no second add may follow: {sent:?}"
        );
        assert!(
            sent.iter().all(|(to, _)| to != &vec![BOB]),
            "Bob never heard of the lock: {sent:?}"
        );
    }

    /// A lock that completed breaks on leaving range: lost for the locker,
    /// otherlost for the target, nothing more.
    #[test]
    fn a_completed_lock_breaks_when_the_target_leaves_range() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        let done = due(&mut reg);
        assert!(done.iter().any(|(_, w)| w == b"add"));
        reg.roster_mut().get_mut(BOB).unwrap().motion.position = [1.0e12, 0.0, 0.0];
        assert_eq!(sweep_out_of_range(&mut reg), 1);
        let sent = due(&mut reg);
        assert_eq!(
            sent,
            vec![
                (vec![ALICE], b"lost".to_vec()),
                (vec![BOB], b"otherlost".to_vec())
            ]
        );
    }

    /// A player's own cancel of an acquiring lock recalls its completion.
    #[test]
    fn a_cancelled_lock_never_completes() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock is allowed");
        cancel(&mut reg, ALICE, BOB_SHIP);
        assert!(due(&mut reg).is_empty());
    }

    /// **Microbenchmark**: 500 pilots in one system, 400 locks (half on
    /// each other, half on rocks), `sweep_out_of_range` timed. Run with
    /// `--ignored --nocapture`; it asserts only that the sweep drops nothing
    /// (everyone sits at one point).
    #[test]
    #[ignore = "benchmark: run by hand with --nocapture"]
    fn bench_sweep_with_500_pilots_and_400_locks() {
        let (n, base) = (500_i64, 7_000_i64);
        let extra = (0..n)
            .map(|i| crate::model::Character {
                character_id: base + i,
                name: format!("Bot{i}"),
                ship_id: base + i + 100_000,
                ..crate::model::Character::default()
            })
            .collect();
        let mut reg = belt_world_with(extra);
        let rock = rock_in(&reg, 30_000_001);
        for i in 0..n {
            let id = base + i;
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    id + 100_000,
                    30_000_001,
                    crate::space_state::Motion::arrived(rock.position, [0.0; 3]),
                ));
        }
        for i in 0..400_i64 {
            let locker = base + i + 100_000;
            let target = if i % 2 == 0 {
                base + (i + 1) % n + 100_000
            } else {
                rock.id
            };
            reg.locks_mut().add(locker, target);
        }
        let mut times = Vec::new();
        for _ in 0..5 {
            let t0 = std::time::Instant::now();
            let dropped = sweep_out_of_range(&mut reg);
            times.push(t0.elapsed());
            assert_eq!(dropped, 0);
        }
        crate::errln!("sweep, 500 pilots / 400 locks: {times:?}");
    }

    /// EVE: a ship that is aligning for a warp is normal sub-warp flight and
    /// can be locked (only the tunnel hides it).
    #[test]
    fn a_ship_aligning_for_warp_can_be_locked() {
        let mut reg = two_in_space();
        reg.roster_mut().get_mut(BOB).expect("in space").motion.mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 + 3_600,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock on an aligning ship is allowed");
        assert_eq!(
            targets_of_actor(&reg, Some(ALICE)),
            PyValue::List(vec![PyValue::Int(BOB_SHIP)])
        );
    }

    /// Put `who` in a warp whose activation stamp is `ahead` ticks from now
    /// (negative: already due).
    fn warp_in(reg: &mut Registry, who: i64, ahead: i32) {
        reg.roster_mut().get_mut(who).expect("in space").motion.mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 + ahead,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
    }

    /// WARP-UNLOCK: the tick a ship's warp turns active, every lock on it
    /// and every lock it held end, each side told once.
    #[test]
    fn a_ship_entering_active_warp_loses_locks_both_ways() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        lock(&mut reg, BOB, ALICE_SHIP).expect("a lock");
        let _ = reg.take_pending();
        let _ = due(&mut reg);
        warp_in(&mut reg, BOB, 3_600);
        assert_eq!(sweep_out_of_range(&mut reg), 0, "aligning keeps both");
        warp_in(&mut reg, BOB, -1);
        assert_eq!(sweep_out_of_range(&mut reg), 2);
        assert!(reg.locks().is_empty());
        let sent = due(&mut reg);
        let lost = |who| {
            sent.iter()
                .filter(|(a, w)| a == &[who] && w == b"lost")
                .count()
        };
        let other = |who| {
            sent.iter()
                .filter(|(a, w)| a == &[who] && w == b"otherlost")
                .count()
        };
        assert_eq!((lost(ALICE), other(BOB)), (1, 1), "Alice's lock on Bob");
        assert_eq!((lost(BOB), other(ALICE)), (1, 1), "Bob's lock on Alice");
    }

    /// WARP-LOCK (#191): a ship in an activated warp cannot be locked, with
    /// the silent key; a lock still acquiring when the target's warp turns
    /// active fails and never completes.
    #[test]
    fn a_ship_in_active_warp_cannot_be_locked() {
        let mut reg = two_in_space();
        warp_in(&mut reg, BOB, -1);
        assert!(lock(&mut reg, ALICE, BOB_SHIP).is_err(), "refused in warp");
        assert!(reg.locks().is_empty());
        warp_in(&mut reg, BOB, 3_600);
        lock(&mut reg, ALICE, BOB_SHIP).expect("aligning is lockable");
        warp_in(&mut reg, BOB, -1);
        assert_eq!(sweep_out_of_range(&mut reg), 1);
        assert!(reg.locks().is_empty());
        let sent = due(&mut reg);
        assert!(
            sent.iter()
                .any(|(to, w)| to == &vec![ALICE] && w == b"lost"),
            "Alice's box is closed: {sent:?}"
        );
        assert!(
            sent.iter().all(|(to, _)| to != &vec![BOB]),
            "Bob never hears of it: {sent:?}"
        );
    }

    /// WARP-LOCK-FIX: a ship that warped and has since landed, and sent
    /// nothing after, can be locked. Its stored record still says
    /// `Mode::Warp` with a due stamp until something carries it to now; the
    /// roam-gang socket test's quarry is exactly this pilot.
    #[test]
    fn a_ship_that_warped_and_landed_can_be_locked() {
        let mut reg = two_in_space();
        let bob = reg.roster_mut().get_mut(BOB).expect("in space");
        // Activated an hour ago on a 1,000 km warp: long since landed.
        bob.motion.mode = crate::destiny::Mode::Warp {
            goto: [1.0e6, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 - 3_600,
            total_distance: 1.0e6,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        bob.since = crate::space_state::now_seconds() - 3_600.0;
        assert!(
            reg.roster()
                .get(BOB)
                .expect("in space")
                .motion
                .warp_active(),
            "the stale record still reads as in warp"
        );
        lock(&mut reg, ALICE, BOB_SHIP).expect("a landed ship is lockable");
        assert_eq!(
            reg.roster().get(BOB).expect("in space").motion.mode,
            crate::destiny::Mode::Stop,
            "carried to now, the warp is over"
        );
    }

    /// An aligning ship, not yet in the tunnel, keeps its locks.
    #[test]
    fn an_aligning_ship_keeps_its_locks() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        lock(&mut reg, BOB, ALICE_SHIP).expect("a lock");
        warp_in(&mut reg, BOB, 3_600);
        assert_eq!(sweep_out_of_range(&mut reg), 0);
        assert_eq!(reg.locks().pairs().len(), 2);
    }

    /// A scram during the align still cancels the warp (INWARP-AUDIT) and,
    /// the warp never going active, the locks stay.
    #[test]
    fn a_scrammed_align_keeps_its_locks() {
        fn refuse_bob(_: &Registry, c: i64) -> Option<crate::movement_guard::Refusal> {
            (c == BOB).then(|| crate::movement_guard::Refusal {
                user_error: "WarpScrambled",
                args: vec![],
            })
        }
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        lock(&mut reg, BOB, ALICE_SHIP).expect("a lock");
        warp_in(&mut reg, BOB, 3_600);
        crate::movement_guard::abort_warps_refused_by(&mut reg, &[refuse_bob]);
        assert_eq!(
            reg.roster().get(BOB).expect("in space").motion.mode,
            crate::destiny::Mode::Stop
        );
        assert_eq!(sweep_out_of_range(&mut reg), 0);
        assert_eq!(reg.locks().pairs().len(), 2);
    }

    /// EWAR-ICONS (c): a lock refused under a jam says so.
    #[test]
    fn a_jammed_pilot_is_refused_with_the_jam_key() {
        let mut reg = two_in_space();
        reg.ext_mut::<crate::ewar::Ewars>()
            .jam_for_test(ALICE_SHIP, crate::pyutil::filetime_now() + 600_000_000);
        let err = lock(&mut reg, ALICE, BOB_SHIP).expect_err("jammed");
        let crate::service::ServiceError::Raise(e) = err else {
            panic!("not a raise")
        };
        assert_eq!(
            e,
            eve_proto::user_error(
                "TargetingSystemsBeingJammed",
                vec![(
                    PyValue::Unicode("Bob".to_owned()),
                    crate::pyutil::bstr("targetname")
                )]
            )
        );
        assert!(reg.locks().held_of(ALICE_SHIP).is_empty());
    }

    /// A push whose arguments cannot be built: any scan that calls `to_push`
    /// on a queued entry to recognise it dies here.
    #[derive(Debug)]
    struct Bomb;

    impl Push for Bomb {
        fn to_push(&self) -> (&'static str, Vec<PyValue>) {
            panic!("the scheduled-queue scan built a push's arguments");
        }

        fn audience(&self) -> Option<Vec<i64>> {
            Some(vec![ALICE])
        }
    }

    /// The tag names the same pushes as the old test on `to_push` and
    /// `audience` (lane fight-stall-fix, step 4): every `TargetChange`
    /// shape, readdressed or not, against every candidate lock.
    #[test]
    fn the_completion_tag_agrees_with_the_slow_test() {
        let mut pushes: Vec<Box<dyn Push>> = Vec::new();
        for what in [
            What::Add,
            What::Lost,
            What::Clear,
            What::OtherAdd,
            What::OtherLost,
        ] {
            for ship in [None, Some(BOB_SHIP), Some(ALICE_SHIP), Some(7)] {
                for to in [vec![], vec![ALICE], vec![BOB], vec![ALICE, BOB]] {
                    let change = TargetChange {
                        to,
                        what,
                        ship,
                        reason: None,
                    };
                    pushes.push(Box::new(change.clone()));
                    // Readdressed as a kill's pushes are: over a change that
                    // already names its one recipient.
                    if change.to.len() == 1 {
                        for readdressed in [None, Some(vec![ALICE]), Some(vec![BOB]), Some(vec![])]
                        {
                            pushes.push(Box::new(crate::notify::Addressed {
                                to: readdressed,
                                inner: Box::new(change.clone()),
                            }));
                        }
                    }
                }
            }
        }
        // Not a target change at all.
        pushes.push(Box::new(
            crate::destiny::ActionUpdate::new(
                vec![ALICE],
                1,
                vec![crate::destiny::Action::Stop { ball: BOB_SHIP }],
            )
            .expect("a batch"),
        ));
        let mut matched = 0;
        for p in &pushes {
            for victim in [None, Some(ALICE), Some(BOB)] {
                for (locker, locker_ship, target_ship) in [
                    (ALICE, ALICE_SHIP, BOB_SHIP),
                    (BOB, BOB_SHIP, ALICE_SHIP),
                    (ALICE, ALICE_SHIP, 7),
                ] {
                    let fast =
                        completion_matches(p.as_ref(), locker, victim, locker_ship, target_ship);
                    let slow = completion_matches_slow(
                        p.as_ref(),
                        locker,
                        victim,
                        locker_ship,
                        target_ship,
                    );
                    assert_eq!(fast, slow, "{p:?} locker {locker} victim {victim:?}");
                    matched += usize::from(fast);
                }
            }
        }
        assert!(matched > 0, "the table reaches both arms of the match");
    }

    /// A scheduled push that cannot build its arguments survives a lock's
    /// teardown, and a lock still acquiring is still recalled beside it.
    #[test]
    fn unlock_does_not_build_the_queued_pushes() {
        let mut reg = two_in_space();
        lock(&mut reg, ALICE, BOB_SHIP).expect("a lock");
        reg.schedule(std::time::Duration::from_secs(60), Bomb);
        reg.schedule(
            std::time::Duration::from_secs(60),
            TargetChange {
                to: vec![ALICE],
                what: What::Add,
                ship: Some(BOB_SHIP),
                reason: None,
            },
        );
        let _ = reg.take_pending();
        unlock(&mut reg, ALICE, BOB_SHIP, Reason::OutOfRange);
        assert_eq!(
            reg.scheduled_count(),
            1,
            "the completion is recalled; the bomb is only skipped"
        );
        assert!(reg.locks().is_empty());
    }
}
