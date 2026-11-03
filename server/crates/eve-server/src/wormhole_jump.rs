//! **Wormhole jumps, mass and decay**: `wormholeMgr.WormholeJump`, the mass
//! a jump takes, and the collapse of a hole that is spent or too old.
//!
//! W17 packet CP54b (`docs/content/plan.md` §6.2; spec
//! `docs/content/R-WH.md` §2 and §4). Lane note: `docs/lanes/cp54b.md`. Built
//! on W16 CP54a (`crate::wormhole`): a hole is found from its ball with
//! `site::Sites::of_item` and [`wormhole::hole`], and mass is taken only by
//! [`wormhole::debit`], the one writer of the `wh` delta.
//!
//! # The jump (R-WH §2.1-§2.2, §4.3)
//!
//! `menusvc.EnterWormhole` approaches to 5,000 m, then
//! `movementFunctions.RealEnterWormhole:644` calls
//! `sm.RemoteSvc('wormholeMgr').WormholeJump(itemID)` inside
//! `PerformSessionChange`, which discards the answer [V]. So:
//!
//! 1. the hole is resolved, or the call raises `WormholeHasCollapsed` [V key];
//! 2. `movement_guard::JUMP_GUARDS` run with the hole's per-jump mass in
//!    view, so [`too_heavy`] (`WormholeTooSmall` [V key]) refuses a hull
//!    heavier than `wormholeMaxJumpMass` beside CP57's scramble guard
//!    (option (b) of R-WH §4.3's seam note: the target is set in this
//!    module's slot for the length of the check);
//! 3. the hull's `mass` [V attribute 4] is debited from the pair at arm time;
//! 4. the stargate jump's sequence follows (`crate::jump`): an `OnSpecialFX`
//!    of `effects.JumpOutWormhole` whose `otherTypeID` is the destination's
//!    class, the jumper's `RemoveBalls` after `jump::REMOVE_AFTER`, the
//!    session change after `jump::JUMP_DELAY` (with `constellationid` and
//!    `regionid` when they move), and the far grid's `SetState` at the bind,
//!    landed by `jump::take_arrival` at the far hole ([`arrival_position`]).
//!
//! # Collapse (R-WH §3 "Removal", §4.2)
//!
//! [`tick`] (a stage before `wormhole`) finds every placed hole whose pair is
//! spent (`mass_left <= 0`) or whose generation has ended, sends its
//! terminal `RemoveBall` (the explosion) to the system, vacates the
//! instance (which removes the Deadspace Signature too) and has CP54a
//! re-read the system, which drops the tracker records. A jump that empties
//! the pair still passes [A] (R-WH §4.3 step 4).

use crate::damage::BlueTime;
use crate::grid::GridRole;
use crate::movement_guard::Refusal;
use crate::service::{Registry, Request, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use crate::wormhole::{self, Pair, Sig};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Instant;

/// The services this module answers (`service::DOMAINS`, coordinator-only).
pub const SERVICES: &[&str] = &["wormholeMgr"];

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "wormhole_jump",
    make,
};

/// The departure effect's guid (R-WH §2.2, `Jump.pyc:758-773`) [V].
pub const FX_JUMP_OUT_WORMHOLE: &str = "effects.JumpOutWormhole";
/// `WormholeTooSmall`: the hull is heavier than the hole's per-jump mass
/// (`dialogs.static`) [V key].
pub const TOO_SMALL: &str = "WormholeTooSmall";
/// `WormholeHasCollapsed`: the hole is gone, a stale click [V key].
pub const COLLAPSED: &str = "WormholeHasCollapsed";
/// `WormholeLoading`: a jump of this pilot's is already being armed [V key].
pub const LOADING: &str = "WormholeLoading";

/// **[A] A hole's radius, metres, when neither its dungeon object nor
/// typedata gives one** (R-WH §4.2's 3,000 m). Falsified by an arrival
/// inside the hole's model.
pub const HOLE_RADIUS: f64 = 3000.0;
/// **[A] How far outside the far hole's surface a ship arrives, metres**
/// (R-WH §4.2: 2,000-5,000 m; one value here, then stepped per arriver as at a
/// gate). Falsified by a human who lands inside the hole or out of reach of
/// jumping straight back (the client approaches to 5,000 m first, so far
/// is only slower).
pub const ARRIVAL_CLEARANCE: f64 = 2500.0;

/// `NotCloseEnoughToEnterWormhole`: the client's own refusal (message 2782,
/// a log line, kwarg `maxdist`) for a hole farther than [`ENTER_RANGE`]
/// [V key].
pub const TOO_FAR: &str = "NotCloseEnoughToEnterWormhole";
/// The client sends `WormholeJump` only inside this surface distance,
/// metres: `menusvc.EnterWormhole` -> `GetCloseAndTryCommand(...,
/// interactionRange=const.maxWormholeEnterDistance)`, 5,000 in `appConst`
/// **[V]**.
pub const ENTER_RANGE: f64 = 5_000.0;
/// What the server adds to [`ENTER_RANGE`] before it refuses, metres: the
/// same lag allowance as `dock_policy::DOCK_RANGE_SLACK`. `[A]`; falsified
/// by an honest entry refused with `NotCloseEnoughToEnterWormhole` (the
/// `[wormhole] out of range` log line gives the distance).
pub const ENTER_RANGE_SLACK: f64 = 1_500.0;

/// `Err(NotCloseEnoughToEnterWormhole)` when `actor`'s ship is farther from
/// the hole's surface than the client would have sent the jump from. A
/// pilot with no ball is let through (the roster check came first).
fn refuse_out_of_range(reg: &Registry, actor: i64, hole: &Sig) -> Result<(), ServiceError> {
    let Some(me) = reg
        .roster()
        .get(actor)
        .and_then(|p| reg.ball_target(p.ship_id))
    else {
        return Ok(());
    };
    let (at, radius) = hole_spot(reg, hole);
    let d = crate::space_state::distance(me.position, at) - me.radius - radius;
    if d <= ENTER_RANGE + ENTER_RANGE_SLACK {
        return Ok(());
    }
    crate::outln!(
        "[wormhole] out of range: {actor} is {d:.0} m from hole {}",
        hole.site_id
    );
    Err(ServiceError::Raise(eve_proto::user_error(
        TOO_FAR,
        crate::pyutil::pypairs(vec![("maxdist", PyValue::Int(ENTER_RANGE as i64))]),
    )))
}

/// The slot: the jump being guarded, and when collapses are next checked.
#[derive(Debug, Default)]
pub struct WormholeJumps {
    /// `(character, the hole's per-jump mass)`, set only for the length of
    /// one `movement_guard::check`, so [`too_heavy`] sees the hole being
    /// entered and nothing else (a stargate jump leaves it `None`).
    entering: Option<(i64, f64)>,
    /// The next blue time [`tick`] looks for collapses.
    next_check: BlueTime,
    /// The `(wormholeSize, wormholeAge)` buckets each placed hole's slim
    /// last carried, by site id, so [`tell_slim`] sends only a change.
    told: BTreeMap<i64, (u64, i64)>,
}

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(WormholeJumps::default())
}

/// The service's methods.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "WormholeJump" => wormhole_jump(reg, req, crate::pyutil::filetime_now()),
        // `None` -> `ClearPolarization()` (`jumpPolarization.pyc:17-19`):
        // no polarization in v1 [V-safe] (R-WH §2.1).
        "GetWormholePolarization" => Ok(PyValue::None),
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

/// The `mass` [V attribute 4] of the ship this character flies, **as fitted**:
/// the dogma sheet's modified value (a propulsion module's mass add is in
/// it), else `destiny::hull::of_in` on `reg.content()` (the hand table, then
/// typedata, else the `_destiny.dll` default) for a hull the sheet does not
/// hold. [A] The fitted mass is the one a hole takes (rolling a hole with a
/// microwarpdrive depends on it); falsified by a capture of a hull with a
/// propulsion module refused or passed where its bare mass says otherwise.
pub fn hull_mass(reg: &Registry, character_id: i64) -> Option<f64> {
    let type_id = reg.character(character_id)?.ship_type_id;
    let bare = crate::destiny::hull::of_in(reg.content(), type_id).mass;
    let fitted = reg
        .roster()
        .get(character_id)
        .and_then(|p| crate::jump_drive::attribute(reg, p.ship_id, type_id, 4));
    Some(fitted.filter(|m| *m > 0.0).unwrap_or(bare))
}

/// Is `character_id` entering a wormhole in the guard check under way (CLOAK-1
/// picks its refusal's key by it)?
pub fn entering(reg: &Registry, character_id: i64) -> bool {
    reg.ext::<WormholeJumps>()
        .entering
        .is_some_and(|(who, _)| who == character_id)
}

/// **The `JUMP_GUARDS` row** (seam S12): `WormholeTooSmall` [V key] while
/// this character is entering a hole whose `wormholeMaxJumpMass` is below
/// their hull's mass. `None` for every other jump.
pub fn too_heavy(reg: &Registry, character_id: i64) -> Option<Refusal> {
    let (who, jump_mass) = reg.ext::<WormholeJumps>().entering?;
    if who != character_id {
        return None;
    }
    (hull_mass(reg, character_id)? > jump_mass).then(|| Refusal {
        user_error: TOO_SMALL,
        args: Vec::new(),
    })
}

/// One resolved jump: the hole entered, its pair now, and the far end.
#[derive(Debug, Clone)]
struct Target {
    hole: Sig,
    pair: Pair,
    far: Sig,
}

impl Target {
    /// Where the jump goes: the static leads to its pair's far system, a
    /// K162 back to the static's home (R-WH §4.2).
    fn far_system(&self) -> i64 {
        self.far.system
    }
}

/// The hole `ball` names in `system`, still live at `now`, and its far end.
fn target(reg: &Registry, system: i64, ball: i64, now: BlueTime) -> Option<Target> {
    let inst = reg.ext::<crate::site::Sites>().of_item(ball)?;
    if inst.system != system || !wormhole::is_wormhole_site(inst.site_id) {
        return None;
    }
    let hole = wormhole::hole(reg, inst.site_id)?.clone();
    // The hole object itself, not the Deadspace Signature beside it.
    inst.objects
        .iter()
        .find(|p| p.item_id == ball && p.type_id == hole.type_id)?;
    let pair = wormhole::pair(reg, hole.pair.home, hole.pair.k, now)
        .filter(|p| p.generation == hole.pair.generation && p.mass_left > 0.0)?;
    // The far end's record, as it will be once this jump has taken mass:
    // its place depends on the pair's slot and generation, not on the mass.
    let mut opened = pair;
    opened.mass_left = 0.0;
    let far = reg
        .ext::<wormhole::Wormholes>()
        .prepared()
        .sigs(reg.world().seed, reg.celestials(), &opened, now)
        .into_iter()
        .find(|s| s.k162 != hole.k162)?;
    Some(Target { hole, pair, far })
}

/// **Where a hole is and how big**: its placed object when the room is
/// laid out, else the anchor plus its dungeon object's offset (as
/// `site::layout` places it); its radius from the object, typedata, or
/// [`HOLE_RADIUS`] [A].
pub fn hole_spot(reg: &Registry, hole: &Sig) -> ([f64; 3], f64) {
    if let Some(p) = reg
        .ext::<crate::site::Sites>()
        .get(hole.site_id)
        .and_then(|i| i.objects.iter().find(|p| p.type_id == hole.type_id))
    {
        return (p.position, p.radius);
    }
    let object = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&hole.dungeon_id))
        .and_then(|d| d.objects.iter().find(|o| o.type_id == hole.type_id));
    let a = hole.anchor;
    let at = object.map_or(a, |o| [a[0] + o.pos[0], a[1] + o.pos[1], a[2] + o.pos[2]]);
    let usable = |r: &f64| r.is_finite() && *r > 0.0;
    let radius = object
        .and_then(|o| o.radius)
        .filter(usable)
        .or_else(|| {
            crate::typedata::installed_in(reg.content())
                .table()
                .get(hole.type_id)
                .map(|r| r.radius)
                .filter(usable)
        })
        .unwrap_or(HOLE_RADIUS);
    (at, radius)
}

/// **Where the `index`-th arriver lands at a hole** at `at` of `radius`:
/// [`ARRIVAL_CLEARANCE`] outside its surface on the side of the system's
/// star, stepped per arriver as `jump::arrival_position` steps them at a
/// gate [A].
pub fn arrival_position(at: [f64; 3], radius: f64, index: usize) -> [f64; 3] {
    let inward = crate::space_state::unit([-at[0], -at[1], -at[2]]).unwrap_or([1.0, 0.0, 0.0]);
    let out = radius + ARRIVAL_CLEARANCE;
    let spot = [
        at[0] + inward[0] * out,
        at[1] + inward[1] * out,
        at[2] + inward[2] * out,
    ];
    crate::destiny::berth(spot, at, index)
}

/// `wormholeMgr.WormholeJump(itemID)` (R-WH §2.1): one positional int, the
/// hole's ball. Answered `None`, which `PerformSessionChange` discards.
/// Refused with nothing changed when the caller is not in space, is already
/// jumping (`WormholeLoading`), names no live hole in their system
/// (`WormholeHasCollapsed`), or a `JUMP_GUARDS` row says no (among them
/// [`too_heavy`]).
fn wormhole_jump(
    reg: &mut Registry,
    req: &Request<'_>,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(PyValue::Int(ball)) = req.args.first() else {
        return Err(notify("WormholeJump needs (itemID,)."));
    };
    let ball = *ball;
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    if reg.jumps().contains(actor) {
        return Err(user_error(LOADING));
    }
    // CONCORD-1b: a criminal does not enter a wormhole.
    crate::concord::refuse_wormhole(reg, Some(actor), now)?;
    let origin = here.solar_system_id;
    let Some(t) = target(reg, origin, ball, now) else {
        return Err(user_error(COLLAPSED));
    };
    // The client's 5,000 m gate, repeated: a modified client could name
    // any hole in the system.
    refuse_out_of_range(reg, actor, &t.hole)?;
    reg.ext_mut::<WormholeJumps>().entering = Some((actor, t.pair.hole.jump_mass));
    let refusal = crate::movement_guard::check(&crate::module::plan().jump_guards, reg, actor);
    reg.ext_mut::<WormholeJumps>().entering = None;
    if let Some(r) = refusal {
        return Err(r.into_error());
    }
    let Some(mass) = hull_mass(reg, actor) else {
        return Err(notify("No ship to jump."));
    };
    let dest_system = t.far_system();
    // **Mass at arm time** (R-WH §4.3 step 4); the first debit opens the
    // K162. At or below zero the pair is spent: this jump still passes [A]
    // and [`tick`] collapses both ends.
    let left = wormhole::debit(reg, t.pair.home, t.pair.k, mass, now);
    tell_slim(reg, now);

    let (at, radius) = hole_spot(reg, &t.far);
    let index = reg
        .roster()
        .others_in_system(dest_system, actor)
        .into_iter()
        .filter(|id| reg.character(*id).is_some())
        .count();
    let arrival = arrival_position(at, radius, index);

    // From here on, `jump::cmd_stargate_jump`'s sequence, verbatim in its
    // order: the FX to the jumper and everyone whose park holds the ship,
    // locks before the roster, the park forgotten, the removal on the clock.
    let mut audience = vec![actor];
    audience.extend(
        reg.roster()
            .others_in_system(origin, actor)
            .into_iter()
            .filter(|id| reg.character(*id).is_some()),
    );
    let stamp = reg.next_destiny_stamp();
    reg.push(WormholeJumpFx {
        to: audience.clone(),
        stamp,
        ship: here.ship_id,
        hole: ball,
        dest_class: t.hole.other_class,
        dest_system,
    });
    crate::targeting::left_space(
        reg,
        actor,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::WithAnswer,
    );
    let held = crate::jump_drive::park_held(reg, actor, here.ship_id);
    reg.roster_mut().leave(actor);
    crate::grid::forget(reg, actor);
    crate::jump_drive::schedule_departure_removal(reg, stamp, here.ship_id, held, audience);
    crate::jump::arm_at(reg, actor, origin, dest_system, arrival);
    crate::outln!(
        "[wormhole] character {actor} ship {}: hole {ball} (site {}) in {origin} -> {dest_system}, \
         {mass} kg, {} kg left",
        here.ship_id,
        t.hole.site_id,
        left.unwrap_or(0.0)
    );
    Ok(PyValue::None)
}

/// **The stage** (before `wormhole`, so CP54a re-reads a collapsed system in
/// the same tick): every `site::CHECK_EVERY`, each placed hole whose pair is
/// spent or whose generation has ended collapses.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    let s = reg.ext_mut::<WormholeJumps>();
    if ft < s.next_check {
        return;
    }
    s.next_check = ft + crate::site::CHECK_EVERY;
    for (site_id, system, ball) in collapsed(reg, ft) {
        collapse(reg, site_id, system, ball);
    }
    tell_slim(reg, ft);
}

/// **`/whmass <percent 1-100>`**: a GM tool. Every live wormhole in the
/// pilot's system has its mass left cut to at most that percent of its
/// maximum (a hole is never refilled), and its slim follows at once
/// ([`tell_slim`]), so a client scenario can see the size buckets without
/// pushing ships through. Not a rule of the game.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |t: &str| Ok(crate::pyutil::bstr(t));
    let Some(actor) = actor else {
        return say("no character");
    };
    let Some(pct) = args
        .first()
        .and_then(|a| a.parse::<f64>().ok())
        .filter(|p| (1.0..=100.0).contains(p))
    else {
        return say("usage: /whmass <percent 1-100>");
    };
    let Some(here) = reg.roster().get(actor).copied() else {
        return say("you are not in space: undock first");
    };
    let now = crate::pyutil::filetime_now();
    let mut cut = 0;
    let mut seen = std::collections::BTreeSet::new();
    for sig in wormhole::sigs(reg, here.solar_system_id, now) {
        let p = sig.pair;
        if !seen.insert((p.home, p.k)) {
            continue;
        }
        let want = p.hole.max_mass * pct / 100.0;
        if p.mass_left > want {
            wormhole::debit(reg, p.home, p.k, p.mass_left - want, now);
            cut += 1;
        }
    }
    tell_slim(reg, now);
    say(&format!("{cut} wormhole(s) cut to {pct}% of their mass"))
}

/// The slim buckets of `sig`: `wormholeSize`'s bits and `wormholeAge`.
fn buckets(sig: &Sig) -> (u64, i64) {
    (
        wormhole::size_bucket(sig.pair.mass_left / sig.pair.hole.max_mass).to_bits(),
        wormhole::age_bucket(sig.pair.expires_at - sig.at),
    )
}

/// Record `now` as what `site_id`'s slim carries; `true` when that differs
/// from what was recorded before. A hole not yet recorded was sent as the
/// last sync saw it, `seen`, so that is what `now` is compared with.
fn note_bucket(
    told: &mut BTreeMap<i64, (u64, i64)>,
    site_id: i64,
    seen: (u64, i64),
    now: (u64, i64),
) -> bool {
    told.insert(site_id, now).unwrap_or(seen) != now
}

/// **Hole slim truth** (R-WH §3 "Updates"): every placed hole whose size or
/// age bucket has changed since its slim was last sent gets an
/// `OnSlimItemChange` to the system, so its model and info window follow the
/// mass a jump took and the hours that passed. The client reruns the
/// controller setup on it (`Wormhole.OnSlimItemUpdated`) [V]. Called after a
/// jump's debit and by [`tick`]; holes that are gone are forgotten.
pub fn tell_slim(reg: &mut Registry, now: BlueTime) {
    // (site id, system, ball, buckets last sent, the hole as of now)
    type Placed = (i64, i64, i64, (u64, i64), Sig);
    let placed: Vec<Placed> = reg
        .ext::<crate::site::Sites>()
        .iter()
        .filter(|i| wormhole::is_wormhole_site(i.site_id))
        .filter_map(|inst| {
            let sig = wormhole::hole(reg, inst.site_id)?;
            let ball = inst
                .objects
                .iter()
                .find(|p| p.type_id == sig.type_id)?
                .item_id;
            // The pair as it stands now, not as the last sync saw it.
            let pair = wormhole::pair(reg, sig.pair.home, sig.pair.k, now)
                .filter(|p| p.generation == sig.pair.generation)?;
            let seen = buckets(sig);
            let mut fresh = sig.clone();
            fresh.pair = pair;
            fresh.at = now;
            Some((inst.site_id, inst.system, ball, seen, fresh))
        })
        .collect();
    let live: std::collections::BTreeSet<i64> = placed.iter().map(|p| p.0).collect();
    reg.ext_mut::<WormholeJumps>()
        .told
        .retain(|id, _| live.contains(id));
    for (site_id, system, ball, seen, sig) in placed {
        let told = &mut reg.ext_mut::<WormholeJumps>().told;
        let changed = note_bucket(told, site_id, seen, buckets(&sig));
        if !changed {
            continue;
        }
        let to = crate::dungeon::audience(reg, system);
        let Some(object) = reg
            .ext::<crate::site::Sites>()
            .of_item(ball)
            .and_then(|i| i.objects.iter().find(|p| p.item_id == ball))
            .map(crate::site::slim)
        else {
            continue;
        };
        if to.is_empty() {
            continue;
        }
        let stamp = reg.next_destiny_stamp();
        reg.schedule(
            std::time::Duration::ZERO,
            crate::hacking::CanSlimChange {
                to,
                stamp,
                slim: object,
                extras: sig.slim_fields(),
            },
        );
    }
}

/// `(site id, system, the hole's ball)` of every placed hole that no longer
/// holds at `now`.
fn collapsed(reg: &Registry, now: BlueTime) -> Vec<(i64, i64, Option<i64>)> {
    reg.ext::<crate::site::Sites>()
        .iter()
        .filter(|i| wormhole::is_wormhole_site(i.site_id))
        .filter_map(|inst| {
            let hole = wormhole::hole(reg, inst.site_id)?;
            let holds = wormhole::pair(reg, hole.pair.home, hole.pair.k, now)
                .is_some_and(|p| p.generation == hole.pair.generation && p.mass_left > 0.0);
            if holds {
                return None;
            }
            let ball = inst
                .objects
                .iter()
                .find(|p| p.type_id == hole.type_id)
                .map(|p| p.item_id);
            Some((inst.site_id, inst.system, ball))
        })
        .collect()
}

/// One hole goes: its explosion to the system's pilots, then its room
/// (`site::vacate`, whose `RemoveBalls` takes the signature object), then
/// CP54a re-reads the system so the tracker drops it and nothing lays the
/// room out again.
fn collapse(reg: &mut Registry, site_id: i64, system: i64, ball: Option<i64>) {
    if let Some(hole) = ball {
        let to = crate::dungeon::audience(reg, system);
        if !to.is_empty() {
            let stamp = reg.next_destiny_stamp();
            reg.schedule(std::time::Duration::ZERO, HoleCollapse { to, stamp, hole });
        }
    }
    crate::site::vacate(reg, site_id);
    wormhole::resync(reg, system);
    crate::outln!("[wormhole] site {site_id} in {system} collapsed");
}

/// The departure: `jump::JumpFx`'s fourteen positionals with R-WH §2.2's
/// changes: `targetID` the hole (the effect plays `'Activate'` on it and
/// reads its nebula), `otherTypeID` the destination's wormhole class (the
/// HUD's "into <class>" [I: destination]), guid
/// [`FX_JUMP_OUT_WORMHOLE`], and `graphicInfo[0]` the destination system
/// (`JumpTransitionWormhole._PlayMidCurves:475` reads it) [V].
#[derive(Debug, Clone, PartialEq)]
pub struct WormholeJumpFx {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub ship: i64,
    pub hole: i64,
    pub dest_class: i64,
    pub dest_system: i64,
}

impl crate::notify::Push for WormholeJumpFx {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let args = vec![
            PyValue::Int(self.ship),
            PyValue::None,
            PyValue::None,
            PyValue::Int(self.hole),
            PyValue::Int(self.dest_class),
            crate::pyutil::bstr(FX_JUMP_OUT_WORMHOLE),
            // isOffensive, start, active: R-WH §2.2's row.
            PyValue::Bool(false),
            PyValue::Bool(true),
            PyValue::Bool(true),
            PyValue::Int(-1),
            PyValue::None,
            PyValue::None,
            PyValue::Int(0),
            PyValue::Tuple(vec![PyValue::Int(self.dest_system)]),
        ];
        destiny_entry(self.stamp, "OnSpecialFX", args)
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// The jumping ship, which `Park.OnSpecialFX` looks up first.
    fn grid(&self) -> GridRole {
        GridRole::Names(vec![self.ship])
    }
}

/// A collapse: `RemoveBall(holeID, terminal=True)`, the destroyed-ball form
/// (`destiny::Kill`'s, one entry with the explosion; `Wormhole.Explode`
/// plays it, `wormhole.pyc:41-52`) [I] (R-WH §3 "Removal").
#[derive(Debug, Clone, PartialEq)]
pub struct HoleCollapse {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub hole: i64,
}

impl crate::notify::Push for HoleCollapse {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let args = vec![PyValue::Int(self.hole), PyValue::Bool(true)];
        destiny_entry(self.stamp, "RemoveBall", args)
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> GridRole {
        GridRole::Removes(vec![self.hole])
    }
}

/// One `DoDestinyUpdate` of one named entry.
fn destiny_entry(stamp: u32, name: &str, args: Vec<PyValue>) -> (&'static str, Vec<PyValue>) {
    let entry = PyValue::Tuple(vec![
        PyValue::Int(i64::from(stamp)),
        PyValue::Tuple(vec![crate::pyutil::bstr(name), PyValue::Tuple(args)]),
    ]);
    (
        "DoDestinyUpdate",
        vec![PyValue::List(vec![entry]), PyValue::Int(0)],
    )
}

fn user_error(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, Vec::new()))
}

/// Our own words, for refusals about this server (as `jump.rs`'s).
fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::{Catalogue, Universe};
    use crate::content::Content;
    use crate::dungeon_data::{self, Dungeon, Object, Room};
    use crate::model::{Character, SandboxConfig};
    use crate::notify::Push;
    use crate::space_state::{Motion, Presence};
    use crate::wormhole::{Facts, Prepared, Table, Wormholes};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    const SEED: u64 = 0x5eed_c54b;
    /// The universe fixture's null-sec system: one static, to the invented
    /// wormhole class.
    const NULL: i64 = 39_000_003;
    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;

    fn per(c: i64) -> f64 {
        if c == wormhole::CLASS_NULL { 1.0 } else { 0.0 }
    }

    /// One invented archetype-38 dungeon per fixture type and the K162: the
    /// hole and a Deadspace Signature, at invented offsets.
    fn dungeons() -> dungeon_data::Table {
        let types = [990_400, 990_401, 990_402, wormhole::K162];
        let list = types.iter().enumerate().map(|(i, t)| {
            let object = |id, type_id, pos| Object {
                room: 1,
                id,
                type_id,
                pos,
                ..Object::default()
            };
            Dungeon {
                id: 999_480 + i as i64,
                archetype: wormhole::ARCHETYPE,
                entry_type: Some(dungeon_data::ENTRY_SIGNATURE),
                rooms: vec![Room {
                    id: 1,
                    ..Room::default()
                }],
                objects: vec![
                    object(1, *t, [700.0, 0.0, -300.0]),
                    object(2, dungeon_data::ENTRY_SIGNATURE, [0.0; 3]),
                ],
                ..Dungeon::default()
            }
        });
        dungeon_data::Table {
            dungeons: list.map(|d| (d.id, d)).collect(),
            ..dungeon_data::Table::default()
        }
    }

    /// A planet per fixture system, and a `#system` line each with an
    /// invented constellation and region of its own, so every jump moves
    /// both.
    fn cat() -> Catalogue {
        let mut text = String::new();
        for (n, s) in Universe::fixture().systems().map(|s| s.id).enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!("#system\t{s}\t{}\t{}\n", 29_100_000 + n, 19_100_000 + n);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\t0\t0\n",
                40_000_000 + n
            );
        }
        Catalogue::parse(&text)
    }

    /// Every fixture type a wormhole of the row's target class, with the
    /// given total and per-jump masses (invented, relative to the hull's).
    fn prep(max_mass: f64, jump_mass: f64) -> Prepared {
        let facts = move |t: i64| {
            let r = *Table::fixture().rows().iter().find(|r| r.type_id == t)?;
            Some(Facts {
                group_id: wormhole::GROUP_WORMHOLE,
                target_class: Some(r.dest_class as f64),
                life_min: None,
                max_mass: Some(max_mass),
                jump_mass: Some(jump_mass),
            })
        };
        Prepared::new(
            &Table::fixture(),
            &Universe::fixture(),
            &dungeons(),
            &facts,
            &per,
        )
    }

    /// The hull mass of the default character's ship [V hand table].
    fn mass() -> f64 {
        crate::destiny::hull::of_in(&Content::fixture(), Character::default().ship_type_id).mass
    }

    /// Alice and Bob, both connected; a world born now, whose holes carry
    /// `max_mass` and `jump_mass` as multiples of the hull's mass.
    fn world(max: f64, jump: f64) -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeons", dungeons());
        c.insert("wormhole", Table::fixture());
        let ch = |id, ship, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(ALICE, ALICE_SHIP, "Alice"), ch(BOB, BOB_SHIP, "Bob")],
            celestials: cat(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        *reg.ext_mut::<Wormholes>() = Wormholes::new(prep(max * mass(), jump * mass()));
        reg.deltas_mut().set_world(crate::delta::World {
            seed: SEED,
            gen_version: crate::population::GEN_VERSION,
            birth: crate::pyutil::filetime_now(),
            ..crate::delta::World::FIXTURE
        });
        for (client, who) in [(1, ALICE), (2, BOB)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: client,
                session_id: client,
                user_id: client,
                character_id: Some(who),
            });
        }
        reg
    }

    fn put(reg: &mut Registry, who: i64, ship: i64, system: i64, at: [f64; 3], now: BlueTime) {
        reg.roster_mut().leave(who);
        let p = Presence::arriving(who, ship, system, Motion::arrived(at, [0.0; 3]));
        reg.roster_mut().arrive(p);
        crate::liveness::refresh(reg, Instant::now(), now);
    }

    /// Alice (and Bob, when asked) at NULL's static, its room laid out;
    /// returns the static and its ball.
    fn at_the_static(reg: &mut Registry, now: BlueTime, bob_too: bool) -> (Sig, i64) {
        let s = wormhole::sigs(reg, NULL, now).remove(0);
        put(reg, ALICE, ALICE_SHIP, NULL, s.anchor, now);
        if bob_too {
            put(reg, BOB, BOB_SHIP, NULL, s.anchor, now);
        }
        wormhole::tick(reg, Instant::now(), now);
        let ball = reg
            .ext::<crate::site::Sites>()
            .get(s.site_id)
            .and_then(|i| i.objects.iter().find(|p| p.type_id == s.type_id))
            .expect("the hole is laid out")
            .item_id;
        let _ = reg.take_pending();
        (s, ball)
    }

    fn call(
        reg: &mut Registry,
        method: &str,
        args: &[PyValue],
        now: BlueTime,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "wormholeMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        match method {
            "WormholeJump" => wormhole_jump(reg, &req, now),
            _ => dispatch(reg, &req),
        }
    }

    /// The slim follows the mass: one `OnSlimItemChange` per bucket change,
    /// none while the bucket holds, and the size is always a client key.
    #[test]
    fn the_hole_slim_is_pushed_once_per_bucket_change() {
        let mut reg = world(10.0, 2.0);
        let now = reg.world().birth;
        let (s, ball) = at_the_static(&mut reg, now, false);
        let size_pushed = |reg: &mut Registry| -> Vec<f64> {
            let late = Instant::now() + Duration::from_millis(50);
            reg.take_due(late)
                .iter()
                .filter_map(|p| {
                    let (n, ops) = action(p.as_ref());
                    if n != "OnSlimItemChange" {
                        return None;
                    }
                    assert_eq!(ops[0], PyValue::Int(ball));
                    let PyValue::Instance { state, .. } = &ops[1] else {
                        panic!("not an instance: {:?}", ops[1])
                    };
                    let PyValue::Dict(pairs) = state.as_ref() else {
                        panic!("not a dict")
                    };
                    pairs.iter().find_map(|(v, k)| match (k, v) {
                        (PyValue::Bytes(k), PyValue::Real(v)) if k == b"wormholeSize" => {
                            assert!([1.0, 0.7, 0.4].contains(v), "{v}");
                            Some(*v)
                        }
                        _ => None,
                    })
                })
                .collect()
        };
        tell_slim(&mut reg, now);
        assert!(size_pushed(&mut reg).is_empty(), "first sight is no change");
        let max = s.pair.hole.max_mass;
        // Down to 40%: 1.0 -> 0.7.
        wormhole::debit(&mut reg, s.pair.home, s.pair.k, max * 0.6, now);
        tell_slim(&mut reg, now);
        assert_eq!(size_pushed(&mut reg), vec![0.7]);
        tell_slim(&mut reg, now);
        assert!(size_pushed(&mut reg).is_empty(), "the bucket holds");
        // Down to 5%: 0.7 -> 0.4.
        wormhole::debit(&mut reg, s.pair.home, s.pair.k, max * 0.35, now);
        tell_slim(&mut reg, now);
        assert_eq!(size_pushed(&mut reg), vec![0.4]);
    }

    /// The first entry of a `DoDestinyUpdate`: `(name, operands)`; another
    /// push's event name and no operands.
    fn action(p: &dyn Push) -> (String, Vec<PyValue>) {
        let (event, args) = p.to_push();
        if event != "DoDestinyUpdate" {
            return (event.to_owned(), Vec::new());
        }
        let Some(PyValue::List(entries)) = args.first() else {
            panic!("no entry list")
        };
        let PyValue::Tuple(entry) = &entries[0] else {
            panic!("not an entry")
        };
        let PyValue::Tuple(named) = &entry[1] else {
            panic!("not a named entry")
        };
        let (Some(PyValue::Bytes(n)), Some(PyValue::Tuple(ops))) = (named.first(), named.get(1))
        else {
            panic!("not (name, args)")
        };
        (String::from_utf8_lossy(n).into_owned(), ops.clone())
    }

    /// The session change as `{key: (old, new)}`.
    fn change_map(change: &[(PyValue, PyValue)]) -> BTreeMap<String, (i64, i64)> {
        change
            .iter()
            .filter_map(|(v, k)| {
                let (PyValue::Bytes(k), PyValue::Tuple(p)) = (k, v) else {
                    return None;
                };
                let (PyValue::Int(a), PyValue::Int(b)) = (&p[0], &p[1]) else {
                    return None;
                };
                Some((String::from_utf8_lossy(k).into_owned(), (*a, *b)))
            })
            .collect()
    }

    /// **The plan's test**: a jump through the static answers `None`, sends
    /// the wormhole tunnel (14 operands, the hole as `targetID`, the far
    /// class, the far system last), debits the pair by the hull's mass [V
    /// attribute], changes the session to the far system (constellation and
    /// region with it, never `shipid`), and the bind lands the ship at the
    /// far end's K162, outside its radius, with the far grid's state.
    #[test]
    fn a_jump_moves_the_ship_to_the_far_end_and_debits_the_hull_mass() {
        let mut reg = world(10.0, 2.0);
        let now = reg.world().birth;
        let (s, ball) = at_the_static(&mut reg, now, false);
        let before = wormhole::pair(&reg, NULL, 0, now).expect("a pair");
        assert!(!before.has_k162());
        let dest = before.dest;

        let got = call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now);
        assert_eq!(got, Ok(PyValue::None));

        let after = wormhole::pair(&reg, NULL, 0, now).expect("still a pair");
        assert_eq!(after.mass_left, before.mass_left - mass());
        let far = wormhole::sigs(&reg, dest, now)
            .into_iter()
            .find(|x| x.k162)
            .expect("the K162 is open");
        assert_eq!(far.pair.mass_left, after.mass_left, "one pair, one mass");

        let pushed = reg.take_pending();
        let fx: Vec<Vec<PyValue>> = pushed
            .iter()
            .map(|(_, p)| action(p.as_ref()))
            .filter(|(n, _)| n == "OnSpecialFX")
            .map(|(_, ops)| ops)
            .collect();
        assert_eq!(fx.len(), 1);
        let ops = &fx[0];
        assert_eq!(ops.len(), 14);
        assert_eq!(ops[0], PyValue::Int(ALICE_SHIP));
        assert_eq!(ops[3], PyValue::Int(ball), "targetID is the hole");
        assert_eq!(ops[4], PyValue::Int(s.other_class), "the far class");
        assert_eq!(ops[5], crate::pyutil::bstr(FX_JUMP_OUT_WORMHOLE));
        assert_eq!(ops[13], PyValue::Tuple(vec![PyValue::Int(dest)]));
        assert!(!reg.roster().is_in_space(ALICE), "in the tunnel");
        assert!(reg.take_session_changes().is_empty(), "not with the answer");

        let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
        let removed: Vec<String> = reg
            .take_due(late)
            .iter()
            .map(|p| action(p.as_ref()).0)
            .collect();
        assert!(removed.contains(&"RemoveBalls".to_owned()), "{removed:?}");
        crate::jump::tick(&mut reg, late);
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        let map = change_map(&changes[0].1);
        assert_eq!(map.get("solarsystemid"), Some(&(NULL, dest)));
        assert_eq!(map.get("solarsystemid2"), Some(&(NULL, dest)));
        assert!(map.contains_key("constellationid") && map.contains_key("regionid"));
        assert!(!map.contains_key("shipid"));

        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let p = *reg.roster().get(ALICE).expect("in space again");
        assert_eq!(p.solar_system_id, dest);
        let (at, radius) = hole_spot(&reg, &far);
        let d = crate::space_state::distance(p.motion.position, at);
        assert!(d > radius, "outside the far hole: {d} m");
        assert!(
            d < radius + 2.0 * ARRIVAL_CLEARANCE,
            "at the far hole: {d} m"
        );
        let state: Vec<String> = reg
            .take_pending()
            .iter()
            .filter(|(_, p)| p.audience() == Some(vec![ALICE]))
            .map(|(_, p)| p.to_push().0.to_owned())
            .collect();
        assert!(
            !state.is_empty(),
            "the far grid's state goes to the arriver"
        );
        assert!(
            reg.ext::<crate::grid::Parks>()
                .of(ALICE)
                .is_some_and(|h| h.contains(&ALICE_SHIP))
        );
    }

    /// **The plan's test**: a hull heavier than the hole's per-jump mass is
    /// refused with `WormholeTooSmall` [V key] **through the guard**
    /// (`movement_guard::JUMP_GUARDS`); nothing moves, nothing is pushed, no
    /// mass is taken. The guard says nothing outside a wormhole jump.
    #[test]
    fn a_ship_heavier_than_the_holes_jump_mass_is_refused_through_the_guard() {
        let mut reg = world(10.0, 0.5);
        let now = reg.world().birth;
        let (_, ball) = at_the_static(&mut reg, now, false);
        let roster = reg.roster().clone();
        let got = call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now);
        assert_eq!(got, Err(user_error(TOO_SMALL)));
        assert_eq!(reg.roster(), &roster);
        assert!(reg.jumps().is_empty());
        assert!(reg.take_pending().is_empty());
        assert!(reg.deltas().of(wormhole::KIND).is_empty(), "no mass moved");
        assert_eq!(too_heavy(&reg, ALICE), None, "only while entering");
        reg.ext_mut::<WormholeJumps>().entering = Some((ALICE, mass() / 2.0));
        assert!(too_heavy(&reg, ALICE).is_some());
        assert_eq!(too_heavy(&reg, BOB), None, "only the one entering");
    }

    /// **The plan's test, mass**: the jump that spends the pair passes [A];
    /// the next tick explodes the hole for the pilot still there, removes
    /// its room, and neither end is in space or the tracker afterwards.
    #[test]
    fn a_hole_out_of_mass_collapses_and_is_removed() {
        let mut reg = world(1.0, 2.0);
        let now = reg.world().birth;
        let (s, ball) = at_the_static(&mut reg, now, true);
        let dest = wormhole::pair(&reg, NULL, 0, now).unwrap().dest;
        let got = call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now);
        assert_eq!(got, Ok(PyValue::None), "the last ship gets through");
        assert!(wormhole::pair(&reg, NULL, 0, now).unwrap().mass_left <= 0.0);
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now());

        tick(&mut reg, Instant::now(), now);
        let gone: Vec<(String, Vec<PyValue>, Vec<i64>)> = reg
            .take_due(Instant::now())
            .iter()
            .map(|p| {
                let (n, ops) = action(p.as_ref());
                (n, ops, p.audience().unwrap_or_default())
            })
            .collect();
        assert!(
            gone.contains(&(
                "RemoveBall".to_owned(),
                vec![PyValue::Int(ball), PyValue::Bool(true)],
                vec![BOB]
            )),
            "{gone:?}"
        );
        assert!(gone.iter().any(|(n, _, _)| n == "RemoveBalls"), "the room");
        assert!(reg.ext::<crate::site::Sites>().get(s.site_id).is_none());

        wormhole::tick(&mut reg, Instant::now(), now);
        assert!(wormhole::sigs(&reg, NULL, now).is_empty(), "no static");
        assert!(
            wormhole::sigs(&reg, dest, now).iter().all(|x| !x.k162),
            "no K162"
        );
        assert!(
            reg.ext::<crate::site::Sites>().get(s.site_id).is_none(),
            "not laid out again"
        );
    }

    /// **The plan's test, lifetime**: at the generation's `expires_at` the
    /// placed hole collapses and is removed, and the next generation's
    /// static is a different site.
    #[test]
    fn a_hole_past_its_lifetime_collapses_and_is_removed() {
        let mut reg = world(10.0, 2.0);
        let now = reg.world().birth;
        let (s, ball) = at_the_static(&mut reg, now, false);
        tick(&mut reg, Instant::now(), now);
        assert!(
            reg.ext::<crate::site::Sites>().get(s.site_id).is_some(),
            "alive"
        );
        let end = s.pair.expires_at;
        let _ = reg.take_due(Instant::now());
        tick(&mut reg, Instant::now(), end);
        let gone: Vec<(String, Vec<PyValue>)> = reg
            .take_due(Instant::now())
            .iter()
            .map(|p| action(p.as_ref()))
            .collect();
        assert!(
            gone.contains(&(
                "RemoveBall".to_owned(),
                vec![PyValue::Int(ball), PyValue::Bool(true)]
            )),
            "{gone:?}"
        );
        assert!(reg.ext::<crate::site::Sites>().get(s.site_id).is_none());
        wormhole::tick(&mut reg, Instant::now(), end);
        let next = wormhole::sigs(&reg, NULL, end);
        assert_eq!(next.len(), 1);
        assert_ne!(next[0].site_id, s.site_id);
        assert!(reg.ext::<crate::site::Sites>().get(s.site_id).is_none());
    }

    /// A ball that is no hole is `WormholeHasCollapsed` [V key];
    /// `GetWormholePolarization` answers `None` [V-safe]; a second jump in
    /// flight is `WormholeLoading` [V key]; none of them changes anything.
    #[test]
    fn stale_holes_polarization_and_a_second_arm() {
        let mut reg = world(10.0, 2.0);
        let now = reg.world().birth;
        let (_, ball) = at_the_static(&mut reg, now, false);
        assert_eq!(
            call(&mut reg, "WormholeJump", &[PyValue::Int(ball + 1_000)], now),
            Err(user_error(COLLAPSED))
        );
        assert_eq!(
            call(
                &mut reg,
                "GetWormholePolarization",
                &[PyValue::Int(ball)],
                now
            ),
            Ok(PyValue::None)
        );
        assert!(reg.deltas().of(wormhole::KIND).is_empty());
        call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now).expect("a jump");
        let rows = reg.deltas().of(wormhole::KIND).clone();
        // Back in space, mid-jump, as a confused client would be.
        put(&mut reg, ALICE, ALICE_SHIP, NULL, [0.0; 3], now);
        assert_eq!(
            call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now),
            Err(user_error(LOADING))
        );
        assert_eq!(reg.deltas().of(wormhole::KIND), &rows);
    }

    /// Every arrival is outside the hole and the arrivers do not share a
    /// spot.
    #[test]
    fn arrivals_are_outside_the_hole_and_apart() {
        let at = [3.0e11, -2.0e10, 5.0e9];
        let spots: Vec<[f64; 3]> = (0..5)
            .map(|i| arrival_position(at, HOLE_RADIUS, i))
            .collect();
        for s in &spots {
            assert!(crate::space_state::distance(*s, at) > HOLE_RADIUS);
        }
        for i in 1..spots.len() {
            assert!(crate::space_state::distance(spots[i], spots[0]) > 1.0);
        }
    }

    /// The client's 5,000 m gate, repeated: a hole named from across the
    /// system is refused with the client's own `NotCloseEnoughToEnterWormhole`
    /// and `maxdist`, and nothing is debited; from inside the range plus the
    /// slack it passes.
    #[test]
    fn a_hole_named_from_out_of_range_is_refused_and_one_in_range_passes() {
        let mut reg = world(10.0, 2.0);
        let now = reg.world().birth;
        let (s, ball) = at_the_static(&mut reg, now, false);
        let (spot, radius) = hole_spot(&reg, &s);
        let before = wormhole::pair(&reg, NULL, 0, now).expect("a pair");
        let ship_radius = reg.ball_target(ALICE_SHIP).map_or(0.0, |b| b.radius);
        let at_surface = |d: f64| [spot[0] + radius + ship_radius + d, spot[1], spot[2]];

        put(
            &mut reg,
            ALICE,
            ALICE_SHIP,
            NULL,
            at_surface(ENTER_RANGE + ENTER_RANGE_SLACK + 1_000.0),
            now,
        );
        let got = call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now);
        assert_eq!(
            got,
            Err(ServiceError::Raise(eve_proto::user_error(
                TOO_FAR,
                crate::pyutil::pypairs(vec![("maxdist", PyValue::Int(5_000))]),
            )))
        );
        assert_eq!(
            wormhole::pair(&reg, NULL, 0, now).unwrap().mass_left,
            before.mass_left,
            "a refused jump takes no mass"
        );
        assert!(reg.roster().get(ALICE).is_some(), "still in space");

        put(
            &mut reg,
            ALICE,
            ALICE_SHIP,
            NULL,
            at_surface(ENTER_RANGE),
            now,
        );
        let got = call(&mut reg, "WormholeJump", &[PyValue::Int(ball)], now);
        assert_eq!(got, Ok(PyValue::None));
    }
}
