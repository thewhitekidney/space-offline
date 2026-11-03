//! **Dictors and heavy interdictors camp** (BP-CAMP, issue #125, L8 of
//! `handback/r-bot-pvp/report.md`). A doctrine's Interdict role used to be
//! fitted, bought and left idle: the escort never loads the sphere launcher
//! (`weapon_effect` skips it, BP-HOLD) and nothing ran the field generator.
//! This module is the behaviour. It runs inside the fight (`hunt::fight`, so
//! every gang fight that goes through it too) for a bot whose doctrine role
//! is [`Role::Interdict`] (`Mind::role`, set with the fit each docked turn).
//!
//! - **Dictor** (sphere launcher, group 589): docked, [`load_probes`] puts
//!   the hangar's warp disrupt probes (group 548) in the launcher and
//!   [`SPARE_PROBES`] in the hold; in space [`reload_now`] refills an empty
//!   launcher from the hold, and [`launch_now`] launches one probe when a
//!   hostile ship is within [`LAUNCH_REACH`] and no known sphere covers it,
//!   at most once per [`BUBBLE_EVERY`].
//! - **Heavy interdictor** (field generator, effect 3380): [`field_now`]
//!   switches the field on when the prey is inside the field's radius less
//!   [`FIELD_MARGIN`] **and** a friend is on the grid (a lone HIC is a
//!   feed); [`field_off`] switches it off when the prey is out of reach or
//!   the bot goes home (a retreat).
//! - **Band**: the bubbles exist in null sec and wormhole space only
//!   ([`crate::hunt::Ground::allows_bubbles`]); the server refuses elsewhere
//!   and the bot never asks. A system the bot knows nothing of counts as no.
//! - **Its own probe**: the launcher's ship sits in its own sphere (**[A]**:
//!   the server holds the launcher, `bubble::inside` exempts only a field's
//!   own ship), so [`Mem::own_probe_alive`] keeps `warp_bubble::decide` from
//!   burning it out of its own bubble while the probe lives.
//!
//! The Scout role has no behaviour here and its seat stays closed (see
//! `doctrine_assign::seat_open`): the gang's scouts already look, report and
//! cloak (`roam_gang`, CLOAK-2), and a camp scout would add a second copy.
//!
//! Every number here is **[A]** policy and none is in a test by value.
//! Falsifiers are at each constant.

use crate::ItemRow;
use crate::brain::{Act, Ball, Memory, Observation, ParkView, Place};
use crate::doctrine::Role;
use crate::pvp_kit::{KitAct, KitModule, Launcher};
use crate::{Conn, Result};
use std::time::{Duration, Instant};

/// Interdiction sphere launchers and warp disrupt probes **[V]**
/// (`bubble.rs`).
pub const GROUP_LAUNCHER: i64 = eve_server::bubble::GROUP_LAUNCHER;
pub const GROUP_PROBE: i64 = eve_server::bubble::GROUP_PROBE;

/// One probe per this long **[A]**: a probe lives 120 s (`explosionDelay`),
/// so a fresh one goes out just before the last expires. Falsified by a
/// gate that is free for seconds between probes (shorten) or a hold of
/// probes spent on a sphere already up (lengthen).
pub const BUBBLE_EVERY: Duration = Duration::from_secs(110);
/// A hostile ship within this many metres of the dictor is worth a probe
/// **[A]** (the probe's own radius is 20 km at the default,
/// `warp_bubble::DEFAULT_RADIUS`). Falsified by probes dropped on ships that
/// leave the sphere at once (shorten).
pub const LAUNCH_REACH: f64 = 20_000.0;
/// The prey must be this far inside the field's radius before the HIC runs
/// it, metres **[A]**: the prey moves and the server measures centre to
/// centre. Falsified by fields switched on and off around a prey at the edge
/// (widen).
pub const FIELD_MARGIN: f64 = 2_000.0;
/// Spare probes kept in the hold for a reload in space **[A]**. Falsified by
/// a dictor out of probes before it can dock.
pub const SPARE_PROBES: i64 = 3;
/// A refused launch (high sec, no skill) puts the launcher off this long
/// **[A]**.
pub const BLOCKED_BACKOFF: Duration = Duration::from_secs(300);
/// A reload is not sent again before this **[A]**.
pub const RELOAD_LOCKOUT: Duration = Duration::from_secs(10);
/// The dock-side work (grant, load) is looked at this often **[A]**.
pub const DOCK_EVERY: Duration = Duration::from_secs(60);
/// Probes an open-mode dictor is given: the launcher's load and the spare.
/// **[A]**: falsified by a dictor that runs dry before it docks.
pub const GRANT_PROBES: i64 = 2 + SPARE_PROBES;
/// A camper warps to its gate and lands this far off it, metres **[A]**: clear
/// of the jump-in point, inside a sphere's radius. Falsified by a camper
/// that lands on the gate (shrink) or whose probe never covers the gate
/// (the sphere is smaller than the standoff: shorten).
pub const GATE_STANDOFF: f64 = 10_000.0;
/// A camper that is not at its gate is sent no oftener than this **[A]**.
pub const POST_EVERY: Duration = Duration::from_secs(20);
/// A dev-spawned dictor waits this long docked for its granted launcher to be
/// fitted and loaded before it undocks without one **[A]**. Falsified by a
/// dictor that undocks with an empty launcher (lengthen) or sits docked for
/// minutes (the bundle names no launcher: shorten).
pub const KIT_HOLD: Duration = Duration::from_secs(120);
/// The names the open-mode grant looks up in the bundle (a name is no
/// extracted value; the ids are resolved at run time).
pub const LAUNCHER_NAME: &str = "Interdiction Sphere Launcher I";
pub const PROBE_NAME: &str = "Warp Disrupt Probe";

/// A mobile warp disruptor (group 361) **[V]**.
pub const GROUP_UNIT: i64 = eve_server::anchor::GROUP_MOBILE_DISRUPTOR;
/// The name the open-mode grant looks up in the bundle.
pub const UNIT_NAME: &str = "Mobile Warp Disruptor I";
/// The server puts a launched unit this far from the ship (`anchor::DEPLOY_AT`).
pub const DEPLOY_AT: f64 = eve_server::anchor::DEPLOY_AT;
/// The sphere's radius the bot assumes when it launches **[A]**: the type's
/// `warpScrambleRange` is not in the observation, so the bubble default
/// stands. Falsified by a unit launched whose sphere does not hold the gate.
pub const UNIT_RADIUS: f64 = crate::warp_bubble::DEFAULT_RADIUS;
/// The sphere must hold the gate with this much to spare, metres **[A]**.
pub const UNIT_MARGIN: f64 = 2_000.0;
/// A camper is "on the warp-in line" when it is within this of its standoff
/// from the gate, metres **[A]**: it warped in at [`GATE_STANDOFF`] along the
/// line the gate's arrivals take. Falsified by a unit that lands off the
/// line the foes warp in on (aim it, not the camper).
pub const LINE_SLACK: f64 = 3_000.0;
/// Anchoring is taken as done this long after the call **[A]**
/// (`anchoringDelay` is in the type; the client does not say when it ends).
/// Falsified by a lift refused as "already changing".
pub const ANCHOR_WAIT: Duration = Duration::from_secs(60);
/// Lifting is taken as done this long after the call **[A]**, then the scoop
/// is tried; a refusal tries again after [`UNIT_BACKOFF`].
pub const LIFT_WAIT: Duration = Duration::from_secs(60);
/// A refused unit call rests the unit's work this long **[A]**.
pub const UNIT_BACKOFF: Duration = Duration::from_secs(30);
/// A hostile ship inside this of the camper makes a scoop unsafe, metres
/// **[A]**.
pub const SAFE_RANGE: f64 = 50_000.0;
/// The camper must be within this of the unit to lift or scoop it, metres
/// **[A]**: well inside the server's reach (`anchor::REACH`, 5 km plus slack).
pub const WORK_RANGE: f64 = 3_000.0;
/// A unit not seen as a sphere within this of its spot is gone (shot), metres
/// **[A]**.
pub const SPOT_SLACK: f64 = 3_000.0;

/// What the dictor remembers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mem {
    /// When the last probe went out.
    pub launched_at: Option<Instant>,
    /// The launcher is not tried before this (a refusal).
    pub blocked_until: Option<Instant>,
    /// Not before this, the next reload.
    pub reload_until: Option<Instant>,
    /// Not before this, the next dock-side look ([`DOCK_EVERY`]).
    pub dock_next: Option<Instant>,
    /// The open-mode grant was made this session.
    pub granted: bool,
    /// The gate the camper last warped to, and the system it was in.
    pub posted: Option<(i64, i64)>,
    /// Not before this, the next warp to the gate ([`POST_EVERY`]).
    pub post_next: Option<Instant>,
    /// When the dock-side wait for the launcher began ([`KIT_HOLD`]).
    pub kit_wait_from: Option<Instant>,
    /// NW-11b: the mobile warp disruptor this camper launched, if one is out.
    pub unit: Option<Unit>,
    /// A refused launch, anchor, lift or scoop rests the unit's work until
    /// then ([`UNIT_BACKOFF`]).
    pub unit_blocked_until: Option<Instant>,
}

/// Where a launched mobile warp disruptor is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// In space, free, not yet anchored.
    Launched,
    /// The anchor call was sent at `at`.
    Anchoring,
    /// Lifting (unanchoring) from `at`.
    Lifting,
}

/// A mobile warp disruptor the camper put out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub id: i64,
    pub stage: Stage,
    /// When the stage began.
    pub at: Instant,
    /// Where the camper stood when it launched (the unit is [`DEPLOY_AT`]
    /// from there).
    pub spot: [f64; 3],
}

impl Unit {
    /// Anchored: the anchor call was sent and [`ANCHOR_WAIT`] has passed.
    pub fn anchored(&self, now: Instant) -> bool {
        self.stage == Stage::Anchoring && now.saturating_duration_since(self.at) >= ANCHOR_WAIT
    }
}

impl Mem {
    /// Whether the bot's own probe is still up (so it is held by its own
    /// sphere, **[A]**).
    pub fn own_probe_alive(&self, now: Instant) -> bool {
        self.launched_at
            .is_some_and(|at| now.saturating_duration_since(at) < BUBBLE_EVERY)
    }

    /// A launch was refused: the launcher rests.
    pub fn blocked(&mut self, now: Instant) {
        self.blocked_until = Some(now + BLOCKED_BACKOFF);
    }
}

/// **Hold the undock for the kit** (BP-CAMP): a dev-spawned dictor has its
/// launcher granted, fitted and loaded at the dock, a turn or two after its
/// role is known; undocking before that sent it out with no launcher, so it
/// never launched a bubble. True while the Interdict role has no loaded
/// launcher and [`KIT_HOLD`] has not run out; the next dock-side look is
/// brought forward.
pub fn kit_pending(m: &mut Memory, o: &Observation) -> bool {
    if m.combat.role != Some(Role::Interdict) || m.policy.dev.is_none() {
        return false;
    }
    if o.kit
        .launcher
        .is_some_and(|l| l.loaded.is_some_and(|n| n > 0))
    {
        return false;
    }
    let from = *m.camp.kit_wait_from.get_or_insert(o.now);
    if o.now.saturating_duration_since(from) >= KIT_HOLD {
        return false;
    }
    m.camp.dock_next = None;
    true
}

/// A mobile warp disruptor among item rows.
pub fn is_unit(row: &ItemRow) -> bool {
    row.group_id == GROUP_UNIT
}

/// **The unit's sphere would hold the gate** (pure): the camper's standoff
/// plus the launch offset, inside the sphere with [`UNIT_MARGIN`] to spare.
pub fn unit_covers_gate(ego: [f64; 3], gate: [f64; 3]) -> bool {
    dist(ego, gate) + DEPLOY_AT + UNIT_MARGIN < UNIT_RADIUS
}

/// **Whether the camper stands on the gate's warp-in line** (pure): at its
/// standoff from the gate, give or take [`LINE_SLACK`].
pub fn on_line(ego: [f64; 3], gate: [f64; 3]) -> bool {
    (dist(ego, gate) - GATE_STANDOFF).abs() <= LINE_SLACK
}

/// **Launch or anchor the camp's unit** (pure): the camp is on and the
/// camper stands at its gate.
/// - Nothing is out, the hold has a unit, the camper is on the warp-in line,
///   its sphere would hold the gate and no sphere holds it yet (a unit or a
///   probe already there): launch.
/// - A unit is out and free: anchor it.
pub fn unit_deploy(m: &Memory, o: &Observation) -> Option<Act> {
    if !camping(m, o) || m.homing.is_some() {
        return None;
    }
    let park = o.park.as_ref()?;
    if park.ego_warping || m.camp.unit_blocked_until.is_some_and(|t| o.now < t) {
        return None;
    }
    if let Some(u) = m.camp.unit {
        return (u.stage == Stage::Launched).then_some(Act::Kit(KitAct::UnitAnchor(u.id)));
    }
    let Place::InSpace { system, .. } = o.place else {
        return None;
    };
    let item = o
        .cargo
        .iter()
        .filter(|r| is_unit(r))
        .min_by_key(|r| r.item_id)?;
    let (_, gate) = m.camp.posted.filter(|(s, _)| *s == system)?;
    let gate = o.warp_bubble.gates.iter().find(|g| g.id == gate)?;
    if park.station.is_some()
        || !on_line(park.ego.position, gate.position)
        || !unit_covers_gate(park.ego.position, gate.position)
        || o.warp_bubble
            .zones
            .iter()
            .any(|(c, r)| dist(*c, gate.position) < *r)
    {
        return None;
    }
    Some(Act::Kit(KitAct::UnitLaunch(item.item_id)))
}

/// **Take the unit back** (pure but for forgetting a lost unit): the camp is
/// over (`camp_over`), nothing hostile is near, and the unit is out.
/// - Free (never anchored): scoop it.
/// - Anchored: fly to it, lift it, and scoop it [`LIFT_WAIT`] later.
///
/// A unit not seen as a sphere at its spot, with the bot on that grid, is
/// gone and forgotten.
pub fn unit_retrieve(m: &mut Memory, o: &Observation, camp_over: bool) -> Option<Act> {
    let u = m.camp.unit?;
    let park = o.park.as_ref()?;
    let near = dist(park.ego.position, u.spot) <= WORK_RANGE + SPOT_SLACK;
    if near
        && !park.ego_warping
        && !o
            .warp_bubble
            .zones
            .iter()
            .any(|(c, _)| dist(*c, u.spot) <= SPOT_SLACK)
    {
        m.camp.unit = None;
        return None;
    }
    if !camp_over
        || park.ego_warping
        || m.camp.unit_blocked_until.is_some_and(|t| o.now < t)
        || o.hunt
            .pilots
            .iter()
            .any(|p| p.hostile && dist(park.ego.position, p.ball.position) <= SAFE_RANGE)
    {
        return None;
    }
    if !near || dist(park.ego.position, u.spot) > WORK_RANGE {
        return Some(Act::WarpToItem {
            id: u.id,
            range: DEPLOY_AT,
        });
    }
    match u.stage {
        Stage::Launched => Some(Act::Kit(KitAct::UnitScoop(u.id))),
        Stage::Anchoring if u.anchored(o.now) => Some(Act::Kit(KitAct::UnitLift(u.id))),
        Stage::Anchoring => None,
        Stage::Lifting if o.now.saturating_duration_since(u.at) >= LIFT_WAIT => {
            Some(Act::Kit(KitAct::UnitScoop(u.id)))
        }
        Stage::Lifting => None,
    }
}

/// **Put a granted unit in the hold, docked** (soft): the hangar's mobile
/// warp disruptor goes to the cargo when the hold has none.
pub fn load_unit(conn: &mut Conn) -> Result<String> {
    if conn.cargo()?.iter().any(is_unit) {
        return Ok(String::new());
    }
    let Some(row) = conn
        .hangar_items()?
        .into_iter()
        .filter(is_unit)
        .min_by_key(|r| r.item_id)
    else {
        return Ok(String::new());
    };
    match conn.load_cargo(row.item_id, Some(1)) {
        Ok(_) => Ok("camp: a mobile warp disruptor in the hold".to_owned()),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => Err(e),
        Err(e) => Ok(format!("camp unit: {e}")),
    }
}

/// A sphere launcher among item rows.
pub fn is_launcher(row: &ItemRow) -> bool {
    row.group_id == GROUP_LAUNCHER
}

/// A warp disrupt probe among item rows.
pub fn is_probe(row: &ItemRow) -> bool {
    row.group_id == GROUP_PROBE
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Whether this bot camps now (pure): a doctrine Interdict role, in space,
/// in a system where bubbles may be made.
pub fn camping(m: &Memory, o: &Observation) -> bool {
    if m.combat.role != Some(Role::Interdict) {
        return false;
    }
    let Place::InSpace { system, .. } = o.place else {
        return false;
    };
    m.policy
        .pvp
        .as_ref()
        .is_some_and(|g| g.allows_bubbles(system))
}

/// **The gate a camper holds in `system`** (pure): the lowest-numbered
/// neighbour's gate on the map, or `None` without a map or a gate.
pub fn gate_of(m: &Memory, system: i64) -> Option<i64> {
    let map = m.policy.travel.as_ref()?;
    let mut next = map.neighbours(system);
    next.sort_unstable();
    next.into_iter()
        .find_map(|n| map.gate(system, n).map(|(g, _)| g))
}

/// **Take the gate** (pure but for the timers): warp to `system`'s gate unless
/// the bot is there already (it warped there and the station is not on its
/// grid), not while in warp and not oftener than [`POST_EVERY`]. In a system
/// where no bubble may be made there is nothing to camp.
pub fn post(m: &mut Memory, o: &Observation, park: &ParkView, system: i64) -> Option<Act> {
    if !m
        .policy
        .pvp
        .as_ref()
        .is_some_and(|g| g.allows_bubbles(system))
    {
        return None;
    }
    let gate = gate_of(m, system)?;
    // At the station's grid it is not at the gate, whatever it posted.
    let there = m.camp.posted == Some((system, gate)) && park.station.is_none();
    if there
        || park.ego_warping
        || m.camp.post_next.is_some_and(|t| o.now < t)
        || o.failed(crate::brain::ActKind::WarpToItem) >= crate::brain::GIVE_UP_AFTER
    {
        return None;
    }
    m.camp.posted = Some((system, gate));
    m.camp.post_next = Some(o.now + POST_EVERY);
    Some(Act::WarpToItem {
        id: gate,
        range: GATE_STANDOFF,
    })
}

/// **Hold the spot** (pure): a camper's moves are dropped from a fight's acts
/// (the orbit, the follow, any warp), so it never chases off the gate. Shots,
/// locks, ewar and the camp's own acts stay.
pub fn hold_position(acts: Vec<Act>) -> Vec<Act> {
    acts.into_iter()
        .filter(|a| {
            !matches!(
                a,
                Act::Orbit { .. }
                    | Act::Follow { .. }
                    | Act::WarpToItem { .. }
                    | Act::WarpToMember { .. }
            )
        })
        .collect()
}

/// **Refill an empty launcher from the hold** (pure). Not while a probe is
/// out of it being launched, not before the lockout, not after repeated
/// failures.
pub fn reload_now(m: &mut Memory, o: &Observation) -> Option<Act> {
    let l = o
        .kit
        .launcher
        .filter(|l| l.loaded == Some(0) && !l.active)?;
    if m.combat.role != Some(Role::Interdict)
        || m.camp.reload_until.is_some_and(|t| o.now < t)
        || o.failed(crate::brain::ActKind::LoadAmmo) >= crate::brain::GIVE_UP_AFTER
    {
        return None;
    }
    let ship = o.ship?;
    let probes: Vec<i64> = o
        .cargo
        .iter()
        .filter(|r| is_probe(r))
        .map(|r| r.item_id)
        .collect();
    if probes.is_empty() {
        return None;
    }
    m.camp.reload_until = Some(o.now + RELOAD_LOCKOUT);
    Some(Act::LoadAmmo {
        guns: vec![(l.item_id, l.flag_id)],
        charges: probes,
        location: ship.ship_id,
    })
}

/// **Launch a probe** (pure): a camper with a loaded launcher, a hostile
/// ship within [`LAUNCH_REACH`] that no known sphere covers, and none sent in
/// the last [`BUBBLE_EVERY`].
pub fn launch_now(m: &Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    if !camping(m, o) {
        return None;
    }
    let l = o
        .kit
        .launcher
        .filter(|l| l.loaded.is_some_and(|n| n > 0) && !l.active)?;
    let park = o.park.as_ref()?;
    if park.ego_warping
        || m.camp.blocked_until.is_some_and(|t| o.now < t)
        || m.camp
            .launched_at
            .is_some_and(|at| o.now.saturating_duration_since(at) < BUBBLE_EVERY)
    {
        return None;
    }
    if dist(park.ego.position, prey.position) > LAUNCH_REACH {
        return None;
    }
    // A sphere that holds the prey already needs no second one.
    if o.warp_bubble
        .zones
        .iter()
        .any(|(c, r)| dist(*c, prey.position) < *r)
    {
        return None;
    }
    Some(Act::Kit(KitAct::Launch(l)))
}

/// **Run the field** (pure): a camper with an idle field generator, the prey
/// inside its radius less [`FIELD_MARGIN`], a friend on the grid, and the
/// capacitor not held.
pub fn field_now(m: &Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    if !camping(m, o) || m.homing.is_some() {
        return None;
    }
    let f = o.kit.field.filter(|f| !f.active && f.range > 0.0)?;
    let park = o.park.as_ref()?;
    if park.ego_warping || o.cap.hold {
        return None;
    }
    if dist(park.ego.position, prey.position) > f.range - FIELD_MARGIN {
        return None;
    }
    // Friends committed: someone else is on the grid on our side.
    if !o.hunt.pilots.iter().any(|p| !p.hostile) {
        return None;
    }
    Some(Act::Kit(KitAct::FieldOn(f)))
}

/// **Stop the field** (pure): it runs, and the prey is out of reach (or
/// there is none, `prey: None`), or the bot goes home, or no friend is left.
pub fn field_off(m: &Memory, o: &Observation, prey: Option<&Ball>) -> Option<Act> {
    let f: KitModule = o.kit.field.filter(|f| f.active)?;
    let park = o.park.as_ref()?;
    let out = prey.is_none_or(|p| dist(park.ego.position, p.position) > f.range + FIELD_MARGIN);
    let alone = !o.hunt.pilots.iter().any(|p| !p.hostile);
    (out || m.homing.is_some() || alone).then_some(Act::Kit(KitAct::FieldOff(f)))
}

/// **The field check at the top of every in-space turn** (pure): the roles'
/// own paths (a gang's orders, the hunter's early returns) never reach
/// [`quiet`], so a field left running is stopped here when the nearest
/// hostile pilot is out of reach, or none is, or the bot goes home.
pub fn field_check(m: &Memory, o: &Observation) -> Option<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return None;
    }
    let park = o.park.as_ref()?;
    let near = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .min_by(|a, b| {
            dist(park.ego.position, a.ball.position)
                .total_cmp(&dist(park.ego.position, b.ball.position))
        })
        .map(|p| p.ball);
    field_off(m, o, near.as_ref())
}

/// **The camp's acts for one fight turn** (pure but for the reload timer):
/// reload, then the probe, then the field.
pub fn acts(m: &mut Memory, o: &Observation, prey: &Ball) -> Vec<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return Vec::new();
    }
    if let Some(a) = reload_now(m, o) {
        return vec![a];
    }
    let mut out = Vec::new();
    if let Some(a) = field_off(m, o, Some(prey)) {
        out.push(a);
    }
    out.extend(launch_now(m, o, prey));
    out.extend(field_now(m, o, prey));
    out
}

/// **The camp with nobody to fight** (pure but for the reload timer): the
/// launcher is refilled and a running field is switched off.
pub fn quiet(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return Vec::new();
    }
    reload_now(m, o)
        .into_iter()
        .chain(field_off(m, o, None))
        .collect()
}

/// **The GM lines of the open-mode dictor** (open mode only, once a session;
/// the closed economy buys or builds the part): the launcher and the probes,
/// ids looked up by name in the bundle. `/createitem` gives the skills each
/// asks for (`gm::slash`). Empty when the bundle names neither.
pub fn gm_lines(content: &eve_server::content::Content) -> Vec<String> {
    let id = |name: &str| eve_server::item::find_type_in(content, name).map(|t| t.type_id);
    let mut out = Vec::new();
    if let Some(l) = id(LAUNCHER_NAME) {
        out.push(format!("/createitem {l}"));
    }
    if let Some(p) = id(PROBE_NAME) {
        out.push(format!("/createitem {p} {GRANT_PROBES}"));
    }
    // NW-11b: one mobile warp disruptor, for the gate (the way the launcher
    // is granted; a closed-economy bot builds or buys its own).
    if let Some(u) = id(UNIT_NAME) {
        out.push(format!("/createitem {u}"));
    }
    out
}

/// How many more probes the hold wants when it already has `in_hold`.
pub fn spare_wanted(in_hold: i64) -> i64 {
    (SPARE_PROBES - in_hold).max(0)
}

/// **Load the launcher, docked** (the doctrine fit is on): the hangar's
/// probes go into every empty sphere launcher, and up to [`SPARE_PROBES`]
/// more into the hold. The note says what happened; empty when there was
/// nothing to do. Soft but for a dead connection.
pub fn load_probes(conn: &mut Conn) -> Result<String> {
    let (Some(ship), Some(station)) = (conn.session().ship_id(), conn.session().station_id())
    else {
        return Ok(String::new());
    };
    let launchers: Vec<ItemRow> = conn.fitted()?.into_iter().filter(is_launcher).collect();
    if launchers.is_empty() {
        return Ok(String::new());
    }
    let hangar = conn.hangar_items()?;
    let mut probes: Vec<ItemRow> = hangar.into_iter().filter(is_probe).collect();
    probes.sort_by_key(|r| r.item_id);
    let Some(first) = probes.first().copied() else {
        return Ok(String::new());
    };
    let kind = first.type_id;
    let mut note = Vec::new();
    let mut empty = Vec::new();
    for l in &launchers {
        if conn.loaded_rounds(ship, l.flag_id, kind)? == 0 {
            empty.push((l.item_id, l.flag_id));
        }
    }
    if !empty.is_empty() {
        let ids: Vec<i64> = probes
            .iter()
            .filter(|r| r.type_id == kind)
            .map(|r| r.item_id)
            .collect();
        match conn.load_ammo(&empty, &ids, kind, station) {
            Ok(n) => note.push(format!("loaded {n} probes")),
            Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
            Err(e) => note.push(format!("probe load: {e}")),
        }
    }
    // What the launcher left goes in the hold, for a reload in space: up to
    // SPARE_PROBES in all, what the hold has already counted.
    let in_hold = match conn.cargo() {
        Ok(rows) => rows
            .iter()
            .filter(|r| is_probe(r))
            .map(|r| r.quantity)
            .sum(),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
        Err(_) => 0,
    };
    let want = spare_wanted(in_hold);
    let held = conn
        .hangar_items()?
        .into_iter()
        .filter(|r| is_probe(r) && r.type_id == kind)
        .min_by_key(|r| r.item_id);
    if let (Some(row), true) = (held, want > 0) {
        match conn.load_cargo(row.item_id, Some(row.quantity.min(want))) {
            Ok(_) => note.push("spare probes in the hold".to_owned()),
            Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
            Err(e) => note.push(format!("spare probes: {e}")),
        }
    }
    Ok(if note.is_empty() {
        String::new()
    } else {
        format!("camp: {}", note.join("; "))
    })
}

/// A launcher as the driver reads it: the fitted row, its default effect and
/// the probes in it.
pub fn launcher_of(
    row: &ItemRow,
    effect: (&'static str, i64),
    loaded: Option<(i64, i64)>,
    remembered: Option<i64>,
    active: bool,
) -> Launcher {
    Launcher {
        item_id: row.item_id,
        type_id: row.type_id,
        flag_id: row.flag_id,
        effect,
        loaded: loaded.map(|(_, n)| n),
        probe: loaded.map(|(t, _)| t).or(remembered),
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{ParkView, Policy, ShipInfo};
    use crate::hunt::{Ground, Pilot};
    use eve_server::group_contacts::Affiliation;
    use std::sync::Arc;

    const SHIP: i64 = 1_000_101;
    const SYSTEM: i64 = 1_000_001;
    const LAUNCHER: i64 = 1_000_201;
    const PROBE_ITEM: i64 = 1_000_301;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn row(item: i64, group: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id: 1_000_900 + item,
            owner_id: 0,
            location_id: SHIP,
            flag_id: 27,
            quantity: 5,
            group_id: group,
            category_id: 7,
            singleton: false,
        }
    }

    fn launcher(loaded: i64) -> Launcher {
        launcher_of(
            &row(LAUNCHER, GROUP_LAUNCHER),
            ("useMissiles", 101),
            Some((1_000_950, loaded)),
            None,
            false,
        )
    }

    fn field() -> KitModule {
        KitModule {
            item_id: 1_000_202,
            type_id: 1_000_902,
            effect: ("warpDisruptSphere", eve_server::bubble::EFFECT_FIELD),
            active: false,
            online: true,
            range: 20_000.0,
            cycle: Duration::from_secs(10),
            reactivation: Duration::ZERO,
        }
    }

    fn friend(id: i64) -> Pilot {
        Pilot {
            ball: ball(id, 1_000.0),
            char_id: id + 5,
            type_id: None,
            who: Affiliation {
                character_id: id + 5,
                corporation_id: 1_000_700,
                alliance_id: None,
            },
            hostile: false,
            relation: 0.0,
            war: false,
        }
    }

    /// A dictor in a null system with a loaded launcher.
    fn setup(now: Instant) -> (Memory, Observation) {
        let mut policy = Policy::fast();
        policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, -0.5)));
        let mut m = Memory::new("Dictor1", policy);
        m.combat.role = Some(Role::Interdict);
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: None,
        });
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: Default::default(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.kit.launcher = Some(launcher(2));
        (m, o)
    }

    #[test]
    fn a_dev_dictor_stays_docked_until_its_launcher_is_loaded() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.dev = Some(Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: Arc::default(),
        }));
        o.kit.launcher = None;
        assert!(kit_pending(&mut m, &o), "no launcher yet");
        o.kit.launcher = Some(launcher(0));
        assert!(kit_pending(&mut m, &o), "fitted but empty");
        assert!(m.camp.dock_next.is_none(), "the dock-side look is due");
        o.kit.launcher = Some(launcher(2));
        assert!(!kit_pending(&mut m, &o), "loaded: go");
        // No launcher ever comes: the wait ends.
        o.kit.launcher = None;
        o.now = t + KIT_HOLD;
        assert!(!kit_pending(&mut m, &o));
        // A bot that is no dictor, or no dev bot, is never held.
        let (mut plain, o2) = setup(t);
        let mut o3 = o2.clone();
        o3.kit.launcher = None;
        assert!(!kit_pending(&mut plain, &o3), "market bots buy their part");
    }

    #[test]
    fn a_dictor_launches_on_a_hostile_in_reach_once_per_bubble() {
        let t = Instant::now();
        let (mut m, o) = setup(t);
        let prey = ball(9, LAUNCH_REACH / 2.0);
        assert!(matches!(
            launch_now(&m, &o, &prey),
            Some(Act::Kit(KitAct::Launch(_)))
        ));
        // Out of reach: nothing.
        assert_eq!(launch_now(&m, &o, &ball(9, LAUNCH_REACH * 2.0)), None);
        // One was just launched: nothing until the next is due.
        m.camp.launched_at = Some(t);
        assert!(m.camp.own_probe_alive(t));
        assert_eq!(launch_now(&m, &o, &prey), None);
        let mut later = o.clone();
        later.now = t + BUBBLE_EVERY;
        assert!(!m.camp.own_probe_alive(later.now));
        assert!(launch_now(&m, &later, &prey).is_some());
    }

    #[test]
    fn a_covered_prey_gets_no_second_probe() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        let prey = ball(9, 5_000.0);
        o.warp_bubble.zones = vec![([4_000.0, 0.0, 0.0], 10_000.0)];
        assert_eq!(launch_now(&m, &o, &prey), None);
        o.warp_bubble.zones = vec![([-90_000.0, 0.0, 0.0], 10_000.0)];
        assert!(launch_now(&m, &o, &prey).is_some());
    }

    #[test]
    fn nothing_is_launched_without_probes_a_role_or_a_band() {
        let t = Instant::now();
        let prey = ball(9, 1_000.0);
        let (m, mut o) = setup(t);
        o.kit.launcher = Some(launcher(0));
        assert_eq!(launch_now(&m, &o, &prey), None, "an empty launcher");
        let (mut m, o) = setup(t);
        m.combat.role = Some(Role::Dps);
        assert_eq!(launch_now(&m, &o, &prey), None, "not an interdictor");
        // Low sec: the server refuses, so the bot never asks.
        let (mut m, o) = setup(t);
        let mut p = Policy::fast();
        p.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.3)));
        m.policy = p;
        assert_eq!(launch_now(&m, &o, &prey), None, "low sec");
        // No map of the system at all: also no.
        m.policy.pvp = None;
        assert_eq!(launch_now(&m, &o, &prey), None, "unknown band");
        // A refused launch rests the launcher.
        let (mut m, o) = setup(t);
        m.camp.blocked(t);
        assert_eq!(launch_now(&m, &o, &prey), None);
    }

    #[test]
    fn an_empty_launcher_is_refilled_from_the_hold() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.kit.launcher = Some(launcher(0));
        assert_eq!(reload_now(&mut m, &o), None, "no probe in the hold");
        o.cargo = vec![row(PROBE_ITEM, GROUP_PROBE), row(PROBE_ITEM + 1, 83)];
        let Some(Act::LoadAmmo {
            guns,
            charges,
            location,
        }) = reload_now(&mut m, &o)
        else {
            panic!("no reload");
        };
        assert_eq!(guns, vec![(LAUNCHER, 27)]);
        assert_eq!(charges, vec![PROBE_ITEM], "only probes");
        assert_eq!(location, SHIP);
        assert_eq!(reload_now(&mut m, &o), None, "locked out");
    }

    #[test]
    fn a_hic_runs_its_field_while_the_prey_is_inside_and_friends_are_there() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        o.kit.launcher = None;
        o.kit.field = Some(field());
        let range = o.kit.field.unwrap().range;
        let inside = ball(9, range / 2.0);
        assert_eq!(field_now(&m, &o, &inside), None, "alone");
        o.hunt.pilots = vec![friend(20)];
        assert!(matches!(
            field_now(&m, &o, &inside),
            Some(Act::Kit(KitAct::FieldOn(_)))
        ));
        // At the edge, inside the margin: no.
        assert_eq!(
            field_now(&m, &o, &ball(9, range - FIELD_MARGIN / 2.0)),
            None
        );
        // Running: stops when the prey is gone or out of reach, or on the way home.
        let mut on = o.clone();
        on.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
        assert_eq!(field_off(&m, &on, Some(&inside)), None);
        assert!(field_off(&m, &on, None).is_some());
        assert!(field_off(&m, &on, Some(&ball(9, range * 2.0))).is_some());
        let mut homing = m.clone();
        homing.homing = Some(crate::brain::Homing::Repair);
        assert!(field_off(&homing, &on, Some(&inside)).is_some());
        assert_eq!(field_now(&homing, &o, &inside), None);
    }

    /// The fight is where it runs: a hostile pilot in reach makes the real
    /// `hunt::fight` carry the launch, once.
    #[test]
    fn the_hunt_fight_carries_the_launch() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        let prey = Pilot {
            hostile: true,
            ..friend(30)
        };
        let park = o.park.clone().unwrap();
        o.hunt.pilots = vec![Pilot {
            ball: ball(30, 6_000.0),
            ..prey.clone()
        }];
        let target = o.hunt.pilots[0].clone();
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Kit(KitAct::Launch(_)))),
            "{acts:?}"
        );
    }

    #[test]
    fn a_running_field_is_stopped_at_the_top_of_the_turn() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        o.kit.launcher = None;
        o.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
        o.hunt.pilots = vec![friend(20)];
        assert!(field_check(&m, &o).is_some(), "no hostile left");
        o.hunt.pilots.push(Pilot {
            hostile: true,
            ..friend(30)
        });
        assert_eq!(field_check(&m, &o), None, "prey inside, a friend on grid");
        o.hunt.pilots[1].ball = ball(30, 1.0e6);
        assert!(field_check(&m, &o).is_some(), "prey out of reach");
        let mut other = m.clone();
        other.combat.role = Some(Role::Dps);
        assert_eq!(field_check(&other, &o), None);
    }

    #[test]
    fn a_camper_takes_the_lowest_gate_once_and_holds_it() {
        use crate::travel::Map;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        // No map: no gate to take.
        let park = o.park.clone().unwrap();
        assert_eq!(gate_of(&m, SYSTEM), None);
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // Two neighbours: the lower-numbered system's gate.
        let map = Map::new()
            .with_gate(SYSTEM, 2_000_501, 1_000_099, 2_000_502)
            .with_gate(SYSTEM, 2_000_503, 1_000_098, 2_000_504);
        m.policy.travel = Some(Arc::new(map));
        assert_eq!(gate_of(&m, SYSTEM), Some(2_000_503));
        let Some(Act::WarpToItem { id, range }) = post(&mut m, &o, &park, SYSTEM) else {
            panic!("no warp");
        };
        assert_eq!((id, range), (2_000_503, GATE_STANDOFF));
        // Posted: it stays, however long it waits.
        o.now = t + POST_EVERY * 3;
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // Back at the station's grid (docked and undocked again): sent again,
        // but not before the pause.
        let mut home = park.clone();
        home.station = Some(ball(7, 100.0));
        o.now = t + Duration::from_secs(1);
        assert_eq!(post(&mut m, &o, &home, SYSTEM), None, "paused");
        o.now = t + POST_EVERY * 3;
        assert!(post(&mut m, &o, &home, SYSTEM).is_some());
        // Not in warp, and not where no bubble may be made.
        let mut low = m.clone();
        low.camp = Mem::default();
        low.policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.3)));
        assert_eq!(post(&mut low, &o, &park, SYSTEM), None, "low sec");
    }

    #[test]
    fn a_held_position_keeps_the_shots_and_drops_the_moves() {
        let acts = vec![
            Act::Orbit { id: 1, range: 1.0 },
            Act::Follow { id: 1, range: 1.0 },
            Act::WarpToItem { id: 1, range: 1.0 },
            Act::WarpToMember {
                char_id: 1,
                range: 1.0,
            },
            Act::Fire { gun: 2, target: 3 },
            Act::Lock(3),
        ];
        assert_eq!(
            hold_position(acts),
            vec![Act::Fire { gun: 2, target: 3 }, Act::Lock(3)]
        );
    }

    #[test]
    fn the_hunt_fight_of_a_camper_never_orbits() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        let park = o.park.clone().unwrap();
        o.guns = vec![crate::brain::GunInfo {
            item_id: 1_000_401,
            type_id: 1_000_901,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.hunt.pilots = vec![Pilot {
            ball: ball(30, 6_000.0),
            hostile: true,
            ..friend(30)
        }];
        let target = o.hunt.pilots[0].clone();
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Orbit { .. } | Act::Follow { .. })),
            "{acts:?}"
        );
        // Anyone else still closes in.
        m.combat.role = Some(Role::Dps);
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Orbit { .. } | Act::Follow { .. })),
            "{acts:?}"
        );
    }

    #[test]
    fn the_hold_is_topped_up_to_the_spare_not_loaded_again() {
        assert_eq!(spare_wanted(0), SPARE_PROBES);
        assert_eq!(spare_wanted(SPARE_PROBES - 1), 1);
        assert_eq!(spare_wanted(SPARE_PROBES), 0);
        assert_eq!(spare_wanted(SPARE_PROBES + 4), 0);
    }

    #[test]
    fn only_an_interdictor_camps() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.hunt.pilots = vec![friend(20)];
        o.cargo = vec![row(PROBE_ITEM, GROUP_PROBE)];
        o.kit.launcher = Some(launcher(0));
        assert!(!acts(&mut m, &o, &ball(9, 1_000.0)).is_empty());
        let (mut m, _) = setup(t);
        m.combat.role = Some(Role::Logi);
        assert!(acts(&mut m, &o, &ball(9, 1_000.0)).is_empty());
        assert!(quiet(&mut m, &o).is_empty());
    }

    // ---- NW-11b: the camp's own mobile warp disruptor ----

    const GATE: i64 = 1_000_401;
    const UNIT_ITEM: i64 = 1_000_501;
    const UNIT_BALL: i64 = 1_000_601;

    /// A camper standing on the gate's line with a unit in the hold.
    fn at_gate(now: Instant) -> (Memory, Observation) {
        let (mut m, mut o) = setup(now);
        // The gate at the origin, the camper at its standoff.
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF);
        o.warp_bubble.gates = vec![ball(GATE, 0.0)];
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        m.camp.posted = Some((SYSTEM, GATE));
        (m, o)
    }

    fn out(stage: Stage, at: Instant) -> Unit {
        Unit {
            id: UNIT_BALL,
            stage,
            at,
            spot: [GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0],
        }
    }

    #[test]
    fn a_camper_at_its_gate_with_a_unit_launches_it() {
        let (m, o) = at_gate(Instant::now());
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitLaunch(UNIT_ITEM)))
        );
    }

    #[test]
    fn no_unit_is_launched_without_one_a_band_a_line_or_a_gap() {
        let t = Instant::now();
        // None in the hold: nothing from nowhere.
        let (m, mut o) = at_gate(t);
        o.cargo.clear();
        assert_eq!(unit_deploy(&m, &o), None);
        // Not a camper.
        let (mut m, o) = at_gate(t);
        m.combat.role = Some(Role::Logi);
        assert_eq!(unit_deploy(&m, &o), None);
        // High sec: no bubbles.
        let (mut m, o) = at_gate(t);
        m.policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.9)));
        assert_eq!(unit_deploy(&m, &o), None);
        // Far off the line (still at the station side of the system).
        let (m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF + LINE_SLACK * 3.0);
        assert_eq!(unit_deploy(&m, &o), None);
        // A sphere already holds the gate (a unit anchored there).
        let (m, mut o) = at_gate(t);
        o.warp_bubble.zones = vec![([0.0, 0.0, 0.0], UNIT_RADIUS)];
        assert_eq!(unit_deploy(&m, &o), None);
        // Not posted at this gate.
        let (mut m, o) = at_gate(t);
        m.camp.posted = None;
        assert_eq!(unit_deploy(&m, &o), None);
        // In warp.
        let (m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego_warping = true;
        assert_eq!(unit_deploy(&m, &o), None);
    }

    #[test]
    fn a_sphere_that_cannot_hold_the_gate_is_not_worth_launching() {
        assert!(unit_covers_gate([GATE_STANDOFF, 0.0, 0.0], [0.0; 3]));
        assert!(!unit_covers_gate([UNIT_RADIUS, 0.0, 0.0], [0.0; 3]));
    }

    #[test]
    fn a_launched_unit_is_anchored_once_and_a_refusal_rests_it() {
        let t = Instant::now();
        let (mut m, o) = at_gate(t);
        m.camp.unit = Some(out(Stage::Launched, t));
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitAnchor(UNIT_BALL)))
        );
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_deploy(&m, &o), None, "anchoring: nothing to do");
        m.camp.unit = Some(out(Stage::Launched, t));
        m.camp.unit_blocked_until = Some(t + UNIT_BACKOFF);
        assert_eq!(unit_deploy(&m, &o), None);
    }

    #[test]
    fn the_unit_stays_while_the_camp_lasts_and_comes_back_when_it_ends() {
        let t = Instant::now();
        let later = t + ANCHOR_WAIT + Duration::from_secs(1);
        let (mut m, mut o) = at_gate(t);
        o.now = later;
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None, "camp on: stays");
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitLift(UNIT_BALL)))
        );
        // Not before its anchor has settled.
        o.now = t + Duration::from_secs(1);
        assert_eq!(unit_retrieve(&mut m, &o, true), None);
        // Lifting: scooped after the wait only.
        m.camp.unit = Some(out(Stage::Lifting, t));
        assert_eq!(unit_retrieve(&mut m, &o, true), None);
        o.now = t + LIFT_WAIT;
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
        // A free unit (never anchored) is scooped at once.
        m.camp.unit = Some(out(Stage::Launched, t));
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
    }

    #[test]
    fn the_unit_is_not_taken_back_under_the_enemys_guns_or_from_afar() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        m.camp.unit = Some(out(Stage::Launched, t));
        let mut foe = friend(40);
        foe.hostile = true;
        foe.ball = ball(40, GATE_STANDOFF + 1_000.0);
        o.hunt.pilots = vec![foe];
        assert_eq!(unit_retrieve(&mut m, &o, true), None, "hostile near");
        o.hunt.pilots.clear();
        // Far away: it flies there first.
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF + 40.0 * WORK_RANGE);
        assert!(matches!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::WarpToItem { id: UNIT_BALL, .. })
        ));
    }

    #[test]
    fn a_unit_that_is_gone_is_forgotten() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        o.warp_bubble.zones.clear();
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None);
        assert_eq!(m.camp.unit, None, "shot: no sphere at its spot");
        // The same, but the bot is elsewhere: it cannot tell, so it keeps it.
        let (mut m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego = ball(SHIP, 100.0 * WORK_RANGE);
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None);
        assert!(m.camp.unit.is_some());
    }
}
