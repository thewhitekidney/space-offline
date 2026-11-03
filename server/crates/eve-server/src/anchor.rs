//! **Anchorable deployables: the Mobile Warp Disruptor** (issue #172, lane
//! nw-2; spec `handback/r-null-war/report.md` §2).
//!
//! A mobile disruptor (group 361) is launched like a Mobile Depot, then
//! *anchored*: the client sends `dogmaLM.Activate(itemID, effectID)` with an
//! **int** effect id and two arguments (`menusvc:2447`), the effects being
//! `anchorDrop` 649 and `anchorLift` 650, whose duration attribute is
//! `anchoringDelay` 556 [V]. The older `Activate(itemID, name, ...)` arm
//! matched only a bytes name, so the call fell through to `BadArguments`.
//!
//! # What the client does [V]
//!
//! - **Launch for Self** (`menusvc.LaunchForSelf`) is `ship.Drop([(id, 1)],
//!   charid, ignoreWarning)`, the Mobile Depot's call ([`crate::mtu`]).
//! - The anchor menu (`OfferAnchorPOSObject`) wants an anchorable group, the
//!   owner or the corporation, a ball that **is free**, in range, and a type
//!   with `anchorDrop`. The call is `godma.GetDogmaLM().Activate(itemID,
//!   const.effectAnchorDrop)`; the client then shows the bar itself, for
//!   `anchoringDelay / 1000.0` seconds, so the attribute is in **milliseconds**
//!   (`menusvc.AnchorObject:2441-2451`) [V]. A lift shows the same
//!   `anchoringDelay`, so it takes the same time.
//! - `MobileWarpDisruptor.Assemble` reads `isFree` once: `anchored = not
//!   isFree`, the radius is `warpScrambleRange`, and the force field is drawn
//!   when anchored. It reads nothing again.
//! - A scoop is offered only for an unanchored unit
//!   (`CanOnlyScoopIfUnanchored{group}`).
//!
//! # What the server does
//!
//! The unit is a [`crate::mtu`] unit in everything but its table row: a store
//! item at flag 0 in the system with a [`crate::combat::Wreck`] record (so it
//! is a ball, it persists through the `spunit` row and a restart brings it
//! back), plus one `anchor` delta row, `unit -> state until hurt`, which is
//! this module's state. The states are `Free`, `Anchoring`, `Anchored` and
//! `Unanchoring`.
//!
//! - **Free**: the ball is free (a zero-velocity physics block), the unit may
//!   be anchored or scooped by its owner or corporation.
//! - **Anchoring**: for `anchoringDelay` after the call; the client draws the
//!   bar itself. Still a free ball.
//! - **Anchored**: not free, and the bubble ([`crate::bubble::Source::Mobile`])
//!   holds. The change reaches a client as `RemoveBalls`, then the grid's
//!   reconcile adds the new ball at its next pass.
//! - **Unanchoring**: after an `anchorLift`, for the same delay. Not free, no
//!   bubble, not scoopable; then `Free` again.
//!
//! Anyone may shoot it (its hull points are attribute 9, with no resists):
//! turrets (range counts, tracking does not), missiles (at once, no flight),
//! drones and smartbombs. When the points are gone it is removed, its locks
//! and the guns on it released. A launch is refused where the type's
//! `disallowInEmpireSpace` (1074) is 1 and the system is high or low
//! security, as a bubble launcher is ([`crate::bubble::forbidden_here`]).
//!
//! # Assumed, with a falsifier each
//!
//! - **[A]** the state change reaches the client as a removal and a re-add
//!   with the ball not free, not as `SetBallFree`. Falsifier:
//!   `fci_mobile_bubble` shows no force field; then try `SetBallFree(id, 0)`.
//! - **[A]** a deployable has no lifetime ([`LIFETIME`] is ten years).
//!   Falsifier: a `deploy/units.tsv` row for 361 with a `decay_s`.
//! - **[A]** anyone can shoot it and it is no crime. Falsifier: the owner.
//! - **[A]** it is launched [`DEPLOY_AT`] metres from the ship, and may be
//!   worked on from [`REACH`]. Falsifier: the client's anchor menu offers it
//!   outside the server's reach, or refuses inside it.
//! - **[A]** the free ball has `agility` 1 and mass [`BALL_MASS`], and the
//!   client tolerates a free ball that never moves. Falsifier: a traceback or
//!   a drifting ball in `fci_anchor_small`.
//! - **[A]** only the owner works on or scoops a unit; a corporation mate
//!   does not (the client's `IsOwnedByMeOrCorp` takes `ownerID` against the
//!   corporation id). Falsifier: the client offers a mate the menu.
//! - **[A]** a kill is a plain removal: no explosion, wreck, killmail,
//!   `OnDamageMessage`, hull bar or aggression and weapons timer. Falsifier:
//!   the live client shows any of them for a destroyed deployable.
//! - **[A]** at most [`MAX_PER_SYSTEM`] units a system, and none in an abyssal
//!   pocket; no spacing, gate or station rule.
//! - **[A]** missiles hit at once with no flight, turrets ignore tracking
//!   (a fixed target), drones and smartbombs deal raw points.

use crate::combat::{self, LOOT_RANGE, LOOT_RANGE_SLACK};
use crate::damage::BlueTime;
use crate::destiny::{Ball, FLAGS_SHIP_BALL, Physics};
use crate::item::{Edit, Slot};
use crate::modules::user_error;
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`): `unit -> state
/// until hurt`.
pub const KIND: &str = "anchor";

/// `groupMobileWarpDisruptor` [V]: the one anchorable group served.
pub const GROUP_MOBILE_DISRUPTOR: i64 = 361;
/// `effectAnchorDrop` and `effectAnchorLift` [V].
pub const EFFECT_ANCHOR_DROP: i64 = 649;
pub const EFFECT_ANCHOR_LIFT: i64 = 650;
/// `anchoringDelay` [V-data]: milliseconds (`menusvc.AnchorObject` divides by 1000).
pub const ATTR_DELAY: i64 = 556;
/// `hp`, the hull points [V-data].
pub const ATTR_HULL_HP: i64 = 9;

/// How far from the launching ship the unit is put, metres **[A]**.
pub const DEPLOY_AT: f64 = 500.0;
/// How near the pilot must be to anchor or lift, surface to surface, metres:
/// the client offers both under `maxConfigureDistance`, 5,000 m
/// (`IsWithinConfigRange`) [V], plus the slack a moving ship needs.
pub const REACH: f64 = 5_000.0 + LOOT_RANGE_SLACK;
/// How near for a scoop, metres **[A]**.
pub const SCOOP_REACH: f64 = LOOT_RANGE + LOOT_RANGE_SLACK;
/// The unit's life on the blue clock: none in the client **[A]**, so ten years.
pub const LIFETIME: BlueTime = 3_650 * 86_400 * SEC;
/// How many units one system holds **[A]**: a bound on the per-tick scans
/// and the bubbles. Falsifier: CCP's own limit (`AnchorProximityUnsuitable`
/// and `AnchorLocationUnsuitable` exist [V]; whether they apply to group 361
/// is unknown), or a friend who wants more.
pub const MAX_PER_SYSTEM: usize = 40;
/// The free ball's mass **[A]**.
pub const BALL_MASS: f64 = 1_000.0;

/// A blue second, in ticks of 100 ns.
const SEC: BlueTime = 10_000_000;
/// A millisecond.
const MSEC: BlueTime = 10_000;

/// Where a unit is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Free,
    /// Until this blue time.
    Anchoring(BlueTime),
    Anchored,
    /// Until this blue time.
    Unanchoring(BlueTime),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    state: State,
    /// Hull points lost.
    hurt: f64,
}

fn key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

fn encode(row: Row) -> Vec<String> {
    let (code, until) = match row.state {
        State::Free => ("free", 0),
        State::Anchoring(t) => ("anchoring", t),
        State::Anchored => ("anchored", 0),
        State::Unanchoring(t) => ("unanchoring", t),
    };
    vec![
        code.to_owned(),
        until.to_string(),
        format!("{:?}", row.hurt),
    ]
}

fn decode(v: &[String]) -> Option<Row> {
    let until: BlueTime = v.get(1)?.parse().ok()?;
    let state = match v.first()?.as_str() {
        "free" => State::Free,
        "anchoring" => State::Anchoring(until),
        "anchored" => State::Anchored,
        "unanchoring" => State::Unanchoring(until),
        _ => return None,
    };
    let hurt = v.get(2)?.parse::<f64>().ok().filter(|h| h.is_finite())?;
    Some(Row { state, hurt })
}

fn row_of(w: &Registry, id: i64) -> Option<Row> {
    w.deltas().of(KIND).get(&key(id)).and_then(|v| decode(v))
}

fn write(w: &mut Registry, id: i64, row: Row) {
    w.deltas_mut().put(KIND, key(id), encode(row));
}

/// Forget a unit's row (it was scooped or is gone).
pub fn forget(w: &mut Registry, id: i64) {
    if row_of(w, id).is_some() {
        w.deltas_mut().del(KIND, &key(id));
    }
}

/// Whether `group_id` is an anchorable group served here.
pub fn is_anchorable(group_id: i64) -> bool {
    group_id == GROUP_MOBILE_DISRUPTOR || group_id == crate::cyno::GROUP_INHIBITOR
}

/// Whether `effect_id` is one this module answers.
pub fn is_anchor_effect(effect_id: i64) -> bool {
    effect_id == EFFECT_ANCHOR_DROP || effect_id == EFFECT_ANCHOR_LIFT
}

/// The unit's state; `None` for anything that is not one in space.
pub fn state_of(w: &Registry, id: i64) -> Option<State> {
    w.combat().wreck(id)?;
    row_of(w, id).map(|r| r.state)
}

/// Whether `id` is an anchorable unit in space.
pub fn is_unit(w: &Registry, id: i64) -> bool {
    state_of(w, id).is_some()
}

fn refuse(text: &str) -> ServiceError {
    crate::mtu::notify(text)
}

fn type_name(w: &Registry, type_id: i64) -> String {
    crate::item::find_in(w.content(), type_id)
        .map_or_else(|| "that".to_owned(), |t| t.name.to_owned())
}

// ---------------------------------------------------------------------------
// Launching: ship.Drop
// ---------------------------------------------------------------------------

/// `ship.Drop([(itemID, qty)], whoseBehalfID, ignoreWarning)` for an
/// anchorable unit; `None` when none of the items is one (the call then
/// carries on to the Sovereignty Hub's). The answer is the depot's:
/// `{itemID: [newItemID]}` or `{itemID: [(label, args)]}`.
pub(crate) fn drop_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let list = match req.args.first() {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v,
        _ => return None,
    };
    let items: Vec<i64> = list
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => match p.first() {
                Some(PyValue::Int(id)) => Some(*id),
                _ => None,
            },
            PyValue::Int(id) => Some(*id),
            _ => None,
        })
        .collect();
    if !items
        .iter()
        .any(|id| reg.item(*id).is_some_and(|i| is_anchorable(i.group_id)))
    {
        return None;
    }
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    for id in items {
        let entry = match req.actor.map(|a| launch_one(reg, a, id)) {
            Some(Ok(new_id)) => PyValue::Int(new_id),
            Some(Err(why)) => crate::mtu::refusal(&why),
            None => crate::mtu::refusal("You must be in space to do that."),
        };
        answer.push((PyValue::List(vec![entry]), PyValue::Int(id)));
    }
    Some(Ok(PyValue::Dict(answer)))
}

/// The checks of a launch and the launch. The new unit's id, or the reason.
pub fn launch_one(reg: &mut Registry, actor: i64, item_id: i64) -> Result<i64, String> {
    let c = reg
        .character(actor)
        .cloned()
        .ok_or_else(|| "You must be in space to do that.".to_owned())?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or_else(|| "You must be in space to do that.".to_owned())?;
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor
                && i.location_id == c.ship_id
                && crate::mtu::LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .ok_or_else(|| "That is not in your ship's hold.".to_owned())?;
    if !is_anchorable(item.group_id) {
        return Err("That cannot be launched.".to_owned());
    }
    if here.motion.warp_active() {
        return Err("You cannot launch that while you are in warp.".to_owned());
    }
    let system = here.solar_system_id;
    if crate::bubble::forbidden_here(reg, item.type_id, system) {
        return Err("That cannot be launched in high or low security space.".to_owned());
    }
    // An abyssal pocket keeps a system taken while a wreck or unit is in it,
    // and a unit lives ten years: none is launched there.
    if crate::abyss::systems_taken(reg).contains(&system) {
        return Err("That cannot be launched in an abyssal pocket.".to_owned());
    }
    if units_in(reg, system) >= MAX_PER_SYSTEM {
        return Err("There are too many of those in this system.".to_owned());
    }
    let at = [
        here.motion.position[0] + DEPLOY_AT,
        here.motion.position[1],
        here.motion.position[2],
    ];
    // NW-4: a cyno inhibitor activates by itself after its component's
    // time (it has no anchor effect), lives that long plus its decay, and
    // keeps its distance from its own kind, gates and stations.
    let inhibitor = crate::cyno::inhibitor_spec(reg, item.group_id);
    if item.group_id == crate::cyno::GROUP_INHIBITOR {
        if inhibitor.is_none() {
            return Err("That cannot be launched.".to_owned());
        }
        if let Some(i) = &inhibitor {
            crate::cyno::placement_refusal(reg, i, system, at)?;
        }
    }
    let (lifetime, activate) = inhibitor.map_or((LIFETIME, 0), |i| {
        (i.activate.saturating_add(i.decay), i.activate)
    });
    let id = crate::mtu::park(
        reg,
        item_id,
        &crate::mtu::Parked {
            owner: actor,
            corp: c.corporation_id,
            ship_id: c.ship_id,
            system,
            at,
        },
        lifetime,
        0,
    )
    .map_err(|_| "The server could not launch that.".to_owned())?;
    write(
        reg,
        id,
        Row {
            state: if inhibitor.is_some() {
                State::Anchoring(crate::pyutil::filetime_now().saturating_add(activate))
            } else {
                State::Free
            },
            hurt: 0.0,
        },
    );
    crate::outln!(
        "[anchor] {actor} launched type {} as {id} in {system}",
        item.type_id
    );
    Ok(id)
}

// ---------------------------------------------------------------------------
// Anchoring: dogmaLM.Activate(itemID, effectID)
// ---------------------------------------------------------------------------

/// `dogmaLM.Activate(itemID, effectID)` with an int effect: the anchor drop
/// or lift of a unit the pilot is beside. Refusals are `CustomNotify`.
pub fn activate(
    reg: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
    effect_id: i64,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let actor = actor.ok_or_else(|| refuse("No character is selected."))?;
    let row = state_of(reg, item_id)
        .and_then(|_| row_of(reg, item_id))
        .ok_or_else(|| refuse("That is gone."))?;
    let unit = reg
        .item(item_id)
        .cloned()
        .ok_or_else(|| refuse("That is gone."))?;
    crate::mtu::beside(reg, actor, item_id, REACH)?;
    if unit.group_id == crate::cyno::GROUP_INHIBITOR {
        // No anchor effect on the type [V]: it activates by itself.
        return Err(refuse("That cannot be anchored or unanchored."));
    }
    if !may_work(actor, &unit) {
        return Err(refuse("That is not yours to anchor."));
    }
    let delay = delay_of(reg.content(), unit.type_id);
    let next = match (effect_id, row.state) {
        (EFFECT_ANCHOR_DROP, State::Free) => {
            let delay = delay.ok_or_else(|| refuse("That cannot be anchored."))?;
            State::Anchoring(now.saturating_add(delay))
        }
        (EFFECT_ANCHOR_LIFT, State::Anchored) => {
            let delay = delay.ok_or_else(|| refuse("That cannot be unanchored."))?;
            State::Unanchoring(now.saturating_add(delay))
        }
        (_, State::Anchoring(_) | State::Unanchoring(_)) => {
            return Err(refuse("That is already changing."));
        }
        (EFFECT_ANCHOR_DROP, _) => return Err(refuse("That is already anchored.")),
        _ => return Err(refuse("That is not anchored.")),
    };
    write(reg, item_id, Row { state: next, ..row });
    Ok(PyValue::None)
}

/// How long an anchor drop or lift of a unit of `type_id` takes, blue ticks:
/// its `anchoringDelay` [V], in milliseconds. `None` for a type without one.
pub fn delay_of(content: &crate::content::Content, type_id: i64) -> Option<BlueTime> {
    crate::typedata::attribute_in(content, type_id, ATTR_DELAY)
        .filter(|d| *d > 0.0)
        .map(|ms| (ms * MSEC as f64) as BlueTime)
}

/// Whether `actor` may anchor, lift or scoop `unit`: its owner. (A corporation
/// mate is out: two strangers in one starter corporation would count, and the
/// client's own menu takes `ownerID` against the corporation id.)
fn may_work(actor: i64, unit: &crate::item::Item) -> bool {
    unit.owner_id == actor
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The unit clock** (`combat::tick`): a finished anchoring or lift moves
/// the state on and tells the system's pilots, and a row whose unit has left
/// space is dropped.
pub fn tick(w: &mut Registry, now: BlueTime) {
    if w.deltas().of(KIND).is_empty() {
        return;
    }
    let ids: Vec<i64> = w
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .collect();
    for id in ids {
        let Some(row) = row_of(w, id) else {
            forget(w, id);
            continue;
        };
        if w.combat().wreck(id).is_none() {
            forget(w, id);
            continue;
        }
        let next = match row.state {
            State::Anchoring(t) if t <= now => State::Anchored,
            State::Unanchoring(t) if t <= now => State::Free,
            _ => continue,
        };
        write(w, id, Row { state: next, ..row });
        withdraw(w, id);
    }
}

/// Who gets the removal: the pilots of the unit's system with a character, but
/// not one who holds an order on the ball.
fn withdraw_to(w: &Registry, id: i64) -> Vec<i64> {
    let Some(wr) = w.combat().wreck(id).copied() else {
        return Vec::new();
    };
    w.roster()
        .in_system(wr.solar_system_id)
        .iter()
        .map(|p| p.character_id)
        .filter(|c| w.character(*c).is_some())
        // A viewer who has the ball locked, or orbits or follows it, keeps it:
        // its client would end that order on the removal and the server
        // would keep it (as a landing, `grid`, #112).
        .filter(|c| {
            w.roster().get(*c).is_none_or(|p| {
                !w.locks().targets_of(p.ship_id).contains(&id)
                    && p.motion.tether().is_none_or(|(t, _)| t != id)
            })
        })
        .collect()
}

/// Take the unit's ball out of every park of its system, so that the grid's
/// next reconcile adds it back in its new shape.
fn withdraw(w: &mut Registry, id: i64) {
    let to = withdraw_to(w, id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::BallsRemoved::new(to, stamp, vec![id]) {
        w.schedule(std::time::Duration::ZERO, p);
    }
}

// ---------------------------------------------------------------------------
// The ball
// ---------------------------------------------------------------------------

/// Shape the wreck ball of `id` to its state (`combat::wreck_parts`): a free
/// unit's ball is free, with a physics block of rest, and any other's is the
/// wreck's own, not free.
pub fn shape_ball(w: &Registry, id: i64, ball: &mut Ball) {
    if w.deltas().of(KIND).is_empty() {
        return;
    }
    if row_of(w, id).is_some_and(|r| matches!(r.state, State::Free | State::Anchoring(_))) {
        ball.flags = FLAGS_SHIP_BALL;
        ball.mass = BALL_MASS;
        ball.physics = Some(Physics {
            max_velocity: 0.0,
            velocity: [0.0; 3],
            agility: 1.0,
            speed_fraction: 0.0,
        });
    }
}

/// An anchored unit, for the bubble.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchored {
    pub ball_id: i64,
    pub type_id: i64,
    pub centre: [f64; 3],
}

/// How many units are in `system`, whatever their state.
pub fn units_in(w: &Registry, system: i64) -> usize {
    w.deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .filter(|id| {
            w.combat()
                .wreck(*id)
                .is_some_and(|wr| wr.solar_system_id == system)
        })
        .count()
}

/// Every unit in `system` that is anchored now.
pub fn anchored(w: &Registry, system: i64) -> Vec<Anchored> {
    units_where(w, system, |r| r.state == State::Anchored)
}

/// Every unit in `system`, whatever its state (NW-4: the spacing rule).
pub fn all_in(w: &Registry, system: i64) -> Vec<Anchored> {
    units_where(w, system, |_| true)
}

fn units_where(w: &Registry, system: i64, keep: impl Fn(&Row) -> bool) -> Vec<Anchored> {
    w.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse::<i64>().ok()?, decode(v)?)))
        .filter(|(_, r)| keep(r))
        .filter_map(|(id, _)| {
            let wr = w.combat().wreck(id)?;
            (wr.solar_system_id == system).then_some(Anchored {
                ball_id: id,
                type_id: wr.type_id,
                centre: wr.position,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shooting it
// ---------------------------------------------------------------------------

/// Whether a pilot in `system` may lock `id`: a unit there with hull points.
pub fn is_lockable(w: &Registry, id: i64, system: i64) -> bool {
    let Some(wr) = w.combat().wreck(id) else {
        return false;
    };
    wr.solar_system_id == system
        && row_of(w, id).is_some()
        && crate::typedata::attribute_in(w.content(), wr.type_id, ATTR_HULL_HP)
            .is_some_and(|hp| hp > 0.0)
}

/// The hull points a unit has left; `None` for anything that is not one.
pub fn hull_left(w: &Registry, id: i64) -> Option<f64> {
    let row = row_of(w, id)?;
    let wr = w.combat().wreck(id)?;
    let hp = crate::typedata::attribute_in(w.content(), wr.type_id, ATTR_HULL_HP)?;
    Some((hp - row.hurt).max(0.0))
}

/// `damage` points on a unit. It is removed at nothing left, and the
/// answer says so. **[A]** no resists, and no crime.
pub fn hit(w: &mut Registry, id: i64, damage: f64, now: BlueTime) -> Option<bool> {
    let left = hull_left(w, id)?;
    let mut row = row_of(w, id)?;
    row.hurt += damage.max(0.0);
    if left - damage <= 0.0 {
        forget(w, id);
        combat::remove_wreck(w, id);
        crate::mtu::forget(w, id);
        release_target(w, id, now);
        return Some(true);
    }
    write(w, id, row);
    Some(false)
}

/// A unit gone from space: stop every module still on it and drop every lock
/// on it, from the clock (`salvage::remove`'s pattern). Without this the
/// guns keep cycling on nothing and the dead lock holds a slot.
fn release_target(w: &mut Registry, id: i64, now: BlueTime) {
    for r in w.modules().running_against(id) {
        crate::modules::stop(w, r.module_id, now, None);
    }
    for locker_ship in w.locks().targeters_of(id) {
        let Some(locker) = w.roster().by_ship(locker_ship).map(|p| p.character_id) else {
            w.locks_mut().remove(locker_ship, id);
            continue;
        };
        crate::targeting::unlock_via(
            w,
            locker,
            id,
            crate::targeting::Reason::Exploding,
            crate::targeting::Delivery::OnTheClock,
        );
    }
}

// ---------------------------------------------------------------------------
// Scooping
// ---------------------------------------------------------------------------

/// `ship.Scoop(unitID)` for a unit: only when unanchored, by its owner or
/// corporation, beside it, into the `flag` hold of the active ship.
pub fn scoop_unit(reg: &mut Registry, actor: i64, id: i64, flag: i64) -> Result<(), ServiceError> {
    let row = row_of(reg, id).ok_or_else(|| refuse("That is gone."))?;
    let unit = reg
        .item(id)
        .cloned()
        .ok_or_else(|| refuse("That is gone."))?;
    if unit.group_id == crate::cyno::GROUP_INHIBITOR {
        return Err(refuse("That cannot be retrieved once deployed."));
    }
    let (c, _) = crate::mtu::beside(reg, actor, id, SCOOP_REACH)?;
    if !may_work(actor, &unit) {
        return Err(refuse("That is not yours to scoop."));
    }
    if row.state != State::Free {
        return Err(user_error(
            "CanOnlyScoopIfUnanchored",
            vec![("group", crate::pyutil::bstr(&type_name(reg, unit.type_id)))],
        ));
    }
    crate::fitting::check_room_in_hold(
        reg.content(),
        reg.store(),
        c.ship_id,
        unit.type_id,
        1,
        flag,
    )
    .map_err(|r| r.to_error())?;
    reg.apply_items(&[Edit::Move {
        item_id: id,
        to: Slot::new(actor, c.ship_id, flag),
    }])
    .map_err(crate::mtu::plan_refused)?;
    combat::release_wreck(reg, id);
    crate::mtu::forget(reg, id);
    forget(reg, id);
    release_target(reg, id, crate::pyutil::filetime_now());
    Ok(())
}

#[cfg(test)]
mod tests;
