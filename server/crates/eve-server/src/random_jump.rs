//! **Needlejack filaments** (issue #88): a filament in the pilot's cargo is
//! spent to carry the pilot, and the fleet members beside them, to a random
//! system of another null-sec region.
//!
//! ## The calls **[V]** (`randomJump/client`)
//!
//! - The item menu offers the activation window for the key groups
//!   (`itemCheckers.OfferRandomJumpKey`); the window's controller calls
//!   `sessionMgr.PerformSessionChange('RandomJump', svc.ActivateRandomJumpFilament,
//!   itemID)`, i.e. `randomJumpMgr.ActivateRandomJumpFilament(itemID)`, whose
//!   answer is discarded ([`activate`]).
//! - A refusal is a `UserError` named `RandomJumpError` whose dict holds
//!   `errors`, a **list** of `(code, args)` pairs (the controller adds it to a
//!   list and puts the sum in a `set`): the codes are `randomJump/error.py`'s
//!   ([`code`]), the args a tuple whose first item is the pilot the error is
//!   about (`get_error_label` reads `charID` from it), or the fleet cap for
//!   [`code::TOO_MANY_IN_FLEET`].
//!
//! ## The type **[V-data]**
//!
//! `microJumpPortalMaxShipCandidates` (2832) is how many pilots go;
//! `microJumpPortalRadius` (2067, 6,000 m on every filament) is how far from
//! the activator a fleet member may be and still be carried;
//! `activeSystemJump` (3025) is 1 for Signal and 0 for Noise;
//! `filamentSpoolupTimeSeconds` (5783) is the spool: 30, 45 or 60 s. The text
//! of the type says the pilots are carried "to a random solar system in
//! another region of Null-Security space". Ids are never written here: a type
//! is a filament when its **group** is [`GROUP_RANDOM_JUMP_KEYS`] and it
//! carries 2832; `eve-mod-econ` finds the same set in the bundle.
//!
//! ## The rules, in the order they are checked
//!
//! The client's `ClientRandomJumpValidator` runs for the activator, so the
//! server runs it for the activator **and every fleet member it would carry**
//! (the errors name a pilot for that reason), plus what the client cannot see:
//!
//! 1. the key: the item exists ([`code::KEY_NOT_FOUND`]), is a filament
//!    group ([`code::INVALID_KEY_TYPE`]), is the pilot's
//!    ([`code::INVALID_OWNER`]), in the ship's cargo ([`code::ITEM_NOT_REACHABLE`]),
//!    at least one ([`code::INVALID_KEY_AMOUNT`]), and a Needlejack type
//!    ([`code::NOT_ACTIVE`]: the other key groups, and a type with no pilot
//!    count, are not served);
//! 2. per pilot: in space, not in an activated warp, not invulnerable, not
//!    counting down a self-destruct, no PvP timer, the safety is not green,
//!    the hull is a sub-capital (`isCapitalSize` 1785; the type's text:
//!    "can only be used by subcapital ships"), not already waiting on a jump;
//! 3. per system: not abyssal or void space ([`code::IN_ABYSS_SYSTEM`]), not
//!    wormhole space ([`code::WORMHOLE_SPACE_DISALLOWED`]), not the hazard
//!    system the client bans ([`code::IN_BANNED_SYSTEM`]);
//! 4. the fleet: the activator and the fleet members in the system within
//!    the type's radius must fit the pilot count ([`code::TOO_MANY_IN_FLEET`]).
//!    A fleet member out of range, or in another system, is simply not
//!    carried **[A]**.
//!
//! The filament is then consumed (once; nothing is consumed on a refusal) and
//! the pilots wait the type's spool ([`SPOOL_SCALE`]) **where they are**; the
//! call returns at once so `PerformSessionChange` is not held for a minute
//! **[A]**. At the end of the spool each pilot who is still in the activator's
//! system, in range and free to jump leaves by `jump_drive::depart` (the
//! departure effect of the jump drive, **[A]**) for the same destination.
//!
//! ## The destination ([`pick_destination`])
//!
//! Null-sec systems (`celestial::Band::Null`: so never high or low sec,
//! wormhole or void space) in a region other than the origin's, never one the
//! bundle flags `disallow_scanning` (the only one is the hazard system the
//! client's `is_random_jump_filament_restricted` names **[A]**: falsified by a
//! second flagged system), and served by the catalogue when there is one.
//! **Noise** picks any of them. **Signal** picks one with a pilot or a bot in
//! it now ("The system has potentially high activity" [V text]); with none,
//! it picks like Noise **[A]**. The draw is a pure function of the world seed,
//! the activator and the clock second.
//!
//! The landing is in deep space: [`DEEP_SPACE_M`] from a celestial of the
//! system **[A]**, the fleet within [`SPREAD_M`] of one another.
//!
//! ## The [A]s (the lane note has each falsifier)
//!
//! - no trace ball in space during the spool (falsified by a client that hangs
//!   in `PerformSessionChange`: then set [`SPOOL_SCALE`] to 0 for the departure
//!   delay alone);
//! - `JumpDriveOut` for the departure FX (a witness to say otherwise);
//! - **no PvP timer is set** on activation: the window's timer text
//!   ("Capsuleer Log-Off Timer... Cause: Combat with another capsuleer") names
//!   the timer that *blocks* the activation, not one it starts;
//! - no jump fatigue (a filament has no drive);
//! - cloaked and tethered ships are not refused (the server has neither);
//! - the pilot is not in a dungeon: not checked.

use crate::celestial::{Band, Universe};
use crate::damage::BlueTime;
use crate::item::{Edit, Item};
use crate::modules::user_error;
use crate::service::{Registry, Request, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The services this module answers (`service::DOMAINS`, coordinator-only).
pub const SERVICES: &[&str] = &["randomJumpMgr"];

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "random_jump",
    make,
};

/// `groupRandomJumpKeys` **[V]** (`inventorycommon/const`): the Needlejack
/// filaments' group. The Triglavian group (4087) is another feature.
pub const GROUP_RANDOM_JUMP_KEYS: i64 = 4041;
/// `groupTriglavianJumpKeys` **[V]**: the client lets the window open for it; the
/// server serves no type of it ([`code::NOT_ACTIVE`]).
pub const GROUP_TRIGLAVIAN_JUMP_KEYS: i64 = 4087;
/// `microJumpPortalMaxShipCandidates` **[V]**: the pilots a filament takes.
pub const ATTR_MAX_SHIPS: i64 = 2832;
/// `microJumpPortalRadius` **[V-data]** (6,000 on every filament): how far
/// from the activator a fleet member is carried **[I]**.
pub const ATTR_RADIUS: i64 = 2067;
/// `activeSystemJump` **[V]**: 1 for Signal, 0 for Noise.
pub const ATTR_ACTIVE_SYSTEM: i64 = 3025;
/// `filamentSpoolupTimeSeconds` **[V]**.
pub const ATTR_SPOOLUP: i64 = 5783;
/// `isCapitalSize` **[V-data]**: set on every capital hull.
pub const ATTR_IS_CAPITAL: i64 = 1785;

/// The radius used when a type names none, metres **[A]** (6,000 on every
/// filament in the bundle).
pub const DEFAULT_RADIUS_M: f64 = 6_000.0;

/// **[A] The spool is the type's seconds times this.** `1.0` is the type's own
/// number. Falsifier: a client that hangs, or shows an error, while the call is
/// outstanding or the ship waits: then `0.0` makes it the departure delay alone.
pub const SPOOL_SCALE: f64 = 1.0;

/// **[A] How far from a celestial the group lands, metres** (a system is
/// 10^11 m across; the type says "a random solar system", not where in it).
/// Falsified by a pilot who lands where the client draws nothing at all, or
/// on top of a site.
pub const DEEP_SPACE_M: f64 = 5.0e8;
/// **[A]** How far apart the group lands, metres.
pub const SPREAD_M: f64 = 1_500.0;

/// The `RandomJumpError` codes **[V]** (`randomJump/error.py`).
pub mod code {
    pub const IN_ABYSS_SYSTEM: i64 = 1;
    pub const INVALID_KEY_TYPE: i64 = 2;
    pub const INVALID_OWNER: i64 = 3;
    pub const INVALID_SHIP_TYPE: i64 = 4;
    pub const ITEM_NOT_REACHABLE: i64 = 5;
    pub const KEY_NOT_FOUND: i64 = 6;
    pub const PVP_TIMER_ACTIVE: i64 = 8;
    pub const SAFETY_ENGAGED: i64 = 9;
    pub const SESSION_CHANGE_IN_PROGRESS: i64 = 10;
    pub const INVALID_KEY_AMOUNT: i64 = 11;
    pub const SHIP_IN_WARP: i64 = 14;
    pub const SHIP_IS_DOCKED: i64 = 16;
    pub const SHIP_IS_INVULNERABLE: i64 = 17;
    pub const SHIP_IS_SELF_DESTRUCTING: i64 = 18;
    pub const TOO_MANY_IN_FLEET: i64 = 24;
    pub const WORMHOLE_SPACE_DISALLOWED: i64 = 34;
    pub const IN_BANNED_SYSTEM: i64 = 35;
    pub const NOT_ACTIVE: i64 = 38;
}

/// The error the client reads: `UserError('RandomJumpError', {'errors': [...]})`.
pub const ERROR: &str = "RandomJumpError";

/// One group waiting out its spool.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The pilot who spent the filament.
    pub anchor: i64,
    /// Everyone carried, the anchor first.
    pub pilots: Vec<i64>,
    /// Where they were, and where they are going.
    pub origin: i64,
    pub dest: i64,
    pub due: Instant,
    /// The type spent (for the log line).
    pub key_type: i64,
}

/// The slot: groups in their spool, by activator.
#[derive(Debug, Default)]
pub struct RandomJumps {
    pending: BTreeMap<i64, Pending>,
}

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(RandomJumps::default())
}

/// Every group in its spool.
pub fn pending(w: &Registry) -> impl Iterator<Item = &Pending> {
    w.ext::<RandomJumps>().pending.values()
}

/// Is this pilot in a group that is waiting to leave?
pub fn is_waiting(w: &Registry, character_id: i64) -> bool {
    pending(w).any(|p| p.pilots.contains(&character_id))
}

/// The service's methods.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "ActivateRandomJumpFilament" => {
            activate(reg, req, Instant::now(), crate::pyutil::filetime_now())
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

// ---------------------------------------------------------------------------
// The type
// ---------------------------------------------------------------------------

/// A Needlejack filament type, read from its attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filament {
    pub type_id: i64,
    /// Pilots it takes, the activator included.
    pub pilots: i64,
    /// Signal: prefers a busy system.
    pub signal: bool,
    pub radius_m: f64,
    pub spool: Duration,
}

/// The filament `type_id` is, if it is one: the group is the key group and
/// the type carries a pilot count.
pub fn filament_type(content: &crate::content::Content, type_id: i64) -> Option<Filament> {
    let row = crate::typedata::installed_in(content)
        .table()
        .get(type_id)?;
    if row.group_id != GROUP_RANDOM_JUMP_KEYS {
        return None;
    }
    let pilots = row.attribute(ATTR_MAX_SHIPS)?;
    if pilots < 1.0 {
        return None;
    }
    let secs = row.attribute(ATTR_SPOOLUP).unwrap_or(0.0).max(0.0);
    Some(Filament {
        type_id,
        pilots: pilots as i64,
        signal: row.attribute(ATTR_ACTIVE_SYSTEM).is_some_and(|v| v > 0.0),
        radius_m: row
            .attribute(ATTR_RADIUS)
            .filter(|r| *r > 0.0)
            .unwrap_or(DEFAULT_RADIUS_M),
        spool: Duration::from_secs_f64(secs * SPOOL_SCALE.max(0.0)),
    })
}

/// Every filament type in the bundle, by type id.
pub fn filament_types(content: &crate::content::Content) -> Vec<Filament> {
    let mut out: Vec<Filament> = crate::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.group_id == GROUP_RANDOM_JUMP_KEYS && t.published)
        .filter_map(|t| filament_type(content, t.type_id))
        .collect();
    out.sort_by_key(|f| f.type_id);
    out
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One error: its code and the arguments the client's label reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: i64,
    pub args: Vec<i64>,
}

impl Fault {
    fn about(code: i64, who: i64) -> Fault {
        Fault {
            code,
            args: vec![who],
        }
    }

    fn bare(code: i64) -> Fault {
        Fault { code, args: vec![] }
    }

    fn value(&self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.code),
            PyValue::Tuple(self.args.iter().map(|a| PyValue::Int(*a)).collect()),
        ])
    }
}

/// The `UserError` for these faults.
pub fn refusal(faults: &[Fault]) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        ERROR,
        crate::pyutil::pypairs(vec![(
            "errors",
            PyValue::List(faults.iter().map(Fault::value).collect()),
        )]),
    ))
}

/// The faults of `who` flying alone, in the client's order
/// (`perform_base_validation_checks`, then the activation checks).
fn pilot_faults(reg: &Registry, who: i64, now: BlueTime) -> Vec<Fault> {
    let mut out = Vec::new();
    let Some(here) = reg.roster().get(who).copied() else {
        out.push(Fault::about(code::SHIP_IS_DOCKED, who));
        return out;
    };
    if crate::invuln::character_protected(reg, who, now) {
        out.push(Fault::about(code::SHIP_IS_INVULNERABLE, who));
    }
    if crate::self_destruct::is_running(reg, here.ship_id) {
        out.push(Fault::about(code::SHIP_IS_SELF_DESTRUCTING, who));
    }
    if crate::dungeon_tracking::in_warp(&here.motion) {
        out.push(Fault::about(code::SHIP_IN_WARP, who));
    }
    let universe = reg.content().try_get::<Universe>();
    match universe.and_then(|u| u.band(here.solar_system_id)) {
        Some(Band::Void) => out.push(Fault::about(code::IN_ABYSS_SYSTEM, who)),
        Some(Band::Wormhole) => out.push(Fault::about(code::WORMHOLE_SPACE_DISALLOWED, who)),
        _ => {}
    }
    if universe
        .and_then(|u| u.system(here.solar_system_id))
        .is_some_and(|s| s.disallow_scanning)
    {
        out.push(Fault::bare(code::IN_BANNED_SYSTEM));
    }
    if crate::crimewatch::safety(reg, who) == crate::crimewatch::rules::Safety::Full {
        out.push(Fault::about(code::SAFETY_ENGAGED, who));
    }
    if reg
        .combat()
        .crimewatch()
        .expiry(who, crate::crimewatch::Timer::Pvp, now)
        .is_some()
    {
        out.push(Fault::about(code::PVP_TIMER_ACTIVE, who));
    }
    let ship_type = crate::jump_drive::ship_type_of(reg, here.ship_id, who);
    if crate::typedata::attribute_in(reg.content(), ship_type, ATTR_IS_CAPITAL)
        .is_some_and(|v| v > 0.0)
    {
        out.push(Fault::about(code::INVALID_SHIP_TYPE, who));
    }
    if reg.jumps().contains(who) || is_waiting(reg, who) {
        out.push(Fault::about(code::SESSION_CHANGE_IN_PROGRESS, who));
    }
    out
}

/// The filament `item_id` as a key `actor` may spend, else the faults.
fn key_of(reg: &Registry, actor: i64, item_id: i64) -> Result<(Item, Filament), Vec<Fault>> {
    let Some(item) = reg.store().get(item_id).cloned() else {
        return Err(vec![Fault::bare(code::KEY_NOT_FOUND)]);
    };
    let mut faults = Vec::new();
    if item.group_id != GROUP_RANDOM_JUMP_KEYS && item.group_id != GROUP_TRIGLAVIAN_JUMP_KEYS {
        faults.push(Fault::about(code::INVALID_KEY_TYPE, actor));
    }
    if item.owner_id != actor {
        faults.push(Fault::about(code::INVALID_OWNER, actor));
    }
    let ship = reg
        .roster()
        .get(actor)
        .map(|p| p.ship_id)
        .or_else(|| reg.character(actor).map(|c| c.ship_id));
    if ship != Some(item.location_id) && !faults.iter().any(|f| f.code == code::INVALID_OWNER) {
        faults.push(Fault::about(code::ITEM_NOT_REACHABLE, actor));
    }
    if item.quantity < 1 {
        faults.push(Fault::bare(code::INVALID_KEY_AMOUNT));
    }
    if !faults.is_empty() {
        return Err(faults);
    }
    match filament_type(reg.content(), item.type_id) {
        Some(f) => Ok((item, f)),
        None => Err(vec![Fault::bare(code::NOT_ACTIVE)]),
    }
}

/// Who a filament activated by `actor` carries: the activator, then the
/// fleet members in the same system within `radius_m`, nearest first. Fleet
/// members elsewhere are not carried **[A]**.
pub fn carried(reg: &Registry, actor: i64, radius_m: f64) -> Vec<i64> {
    let Some(me) = reg.roster().get(actor).copied() else {
        return Vec::new();
    };
    let mut near: Vec<(f64, i64)> = Vec::new();
    if let Some(fleet) = reg.fleets().fleet_of(actor) {
        for who in fleet.members.keys().copied().filter(|w| *w != actor) {
            let Some(p) = reg.roster().get(who) else {
                continue;
            };
            let piloted = reg.character(who).is_some();
            if p.solar_system_id != me.solar_system_id || !piloted {
                continue;
            }
            let gap = crate::space_state::distance(me.motion.position, p.motion.position);
            if gap <= radius_m {
                near.push((gap, who));
            }
        }
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    std::iter::once(actor)
        .chain(near.into_iter().map(|(_, who)| who))
        .collect()
}

// ---------------------------------------------------------------------------
// The destination
// ---------------------------------------------------------------------------

/// The wormhole class of Pochven's systems.
const POCHVEN_CLASS: i64 = 25;

/// The systems a filament may land in, ascending: null-sec, not Pochven, another region
/// than `origin_region`, not flagged `disallow_scanning`, and `served`.
pub fn destinations(
    universe: &Universe,
    origin_region: Option<i64>,
    served: &dyn Fn(i64) -> bool,
) -> Vec<i64> {
    let mut out: Vec<i64> = universe
        .systems()
        .filter(|s| s.band() == Band::Null)
        // Pochven's gates all lead inside Pochven, so a pilot landed there
        // could not leave (class 25 [V-data], R-WH report §6).
        .filter(|s| universe.wormhole_class(s.id) != Some(POCHVEN_CLASS))
        .filter(|s| Some(s.region_id) != origin_region)
        .filter(|s| !s.disallow_scanning)
        .filter(|s| served(s.id))
        .map(|s| s.id)
        .collect();
    out.sort_unstable();
    out
}

/// **Where a filament goes**: Noise any of [`destinations`], Signal one that
/// `busy` says holds a pilot, else as Noise. `None` when there is nowhere.
pub fn pick_destination(
    universe: &Universe,
    origin_region: Option<i64>,
    served: &dyn Fn(i64) -> bool,
    busy: &dyn Fn(i64) -> bool,
    signal: bool,
    seed: u64,
) -> Option<i64> {
    let all = destinations(universe, origin_region, served);
    let chosen: Vec<i64> = if signal {
        let active: Vec<i64> = all.iter().copied().filter(|s| busy(*s)).collect();
        if active.is_empty() { all } else { active }
    } else {
        all
    };
    if chosen.is_empty() {
        return None;
    }
    let mut s = crate::seed::Stream::new(seed);
    chosen
        .get(s.range(0, chosen.len() as u64) as usize)
        .copied()
}

/// Where a group lands in `system`: [`DEEP_SPACE_M`] from one of its
/// celestials (not the star when it has others), in a direction drawn from
/// `seed`.
pub fn landing_centre(reg: &Registry, system: i64, seed: u64) -> [f64; 3] {
    let mut s = crate::seed::Stream::new(seed);
    let bodies: Vec<_> = reg.celestials().served_in(system).collect();
    let others: Vec<_> = bodies
        .iter()
        .filter(|c| c.kind != crate::celestial::Kind::Star)
        .copied()
        .collect();
    let pool = if others.is_empty() { &bodies } else { &others };
    let (base, radius) = match pool.get(s.range(0, pool.len().max(1) as u64) as usize) {
        Some(c) => (c.position, c.radius),
        None => ([0.0; 3], 0.0),
    };
    let dir = random_direction(&mut s);
    let out = radius + DEEP_SPACE_M;
    [
        base[0] + dir[0] * out,
        base[1] + dir[1] * out,
        base[2] + dir[2] * out,
    ]
}

/// A direction, uniform on the sphere.
fn random_direction(s: &mut crate::seed::Stream) -> [f64; 3] {
    for _ in 0..32 {
        let p = [
            s.unit() * 2.0 - 1.0,
            s.unit() * 2.0 - 1.0,
            s.unit() * 2.0 - 1.0,
        ];
        if let Some(u) = crate::space_state::unit(p).filter(|_| {
            let n = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
            n <= 1.0
        }) {
            return u;
        }
    }
    [1.0, 0.0, 0.0]
}

/// Where the `index`-th pilot of a group lands: within [`SPREAD_M`] of
/// `centre`; the first is on it.
pub fn landing_spot(centre: [f64; 3], index: usize, seed: u64) -> [f64; 3] {
    if index == 0 {
        return centre;
    }
    let mut s = crate::seed::Stream::new(crate::seed::mix(seed, &[index as u64]));
    let d = random_direction(&mut s);
    let r = SPREAD_M * (0.25 + 0.75 * s.unit());
    [
        centre[0] + d[0] * r,
        centre[1] + d[1] * r,
        centre[2] + d[2] * r,
    ]
}

fn is_served(reg: &Registry, system: i64) -> bool {
    let cat = reg.celestials();
    cat.is_empty() || cat.system(system).is_some() || cat.served_in(system).next().is_some()
}

/// The systems that hold a pilot or a bot right now.
fn busy_systems(reg: &Registry) -> BTreeSet<i64> {
    reg.roster()
        .systems()
        .filter(|(_, ids)| ids.iter().any(|id| reg.character(*id).is_some()))
        .map(|(system, _)| system)
        .collect()
}

// ---------------------------------------------------------------------------
// The call
// ---------------------------------------------------------------------------

/// `randomJumpMgr.ActivateRandomJumpFilament(itemID)`: refuses with
/// `RandomJumpError` and changes nothing, or spends the filament and starts the
/// spool; answers `None`.
pub fn activate(
    reg: &mut Registry,
    req: &Request<'_>,
    now: Instant,
    ft: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(PyValue::Int(item_id)) = req.args.first() else {
        return Err(notify("ActivateRandomJumpFilament needs (itemID,)."));
    };
    let (key, filament) = key_of(reg, actor, *item_id).map_err(|f| refusal(&f))?;
    let pilots = carried(reg, actor, filament.radius_m);
    let mut faults: Vec<Fault> = Vec::new();
    for who in &pilots {
        faults.extend(pilot_faults(reg, *who, ft));
    }
    if faults.is_empty() && pilots.is_empty() {
        faults.push(Fault::about(code::SHIP_IS_DOCKED, actor));
    }
    if faults.is_empty() && pilots.len() as i64 > filament.pilots {
        faults.push(Fault {
            code: code::TOO_MANY_IN_FLEET,
            args: vec![filament.pilots],
        });
    }
    if !faults.is_empty() {
        return Err(refusal(&faults));
    }
    let origin = reg.roster().get(actor).map_or(0, |p| p.solar_system_id);
    let seed = crate::seed::mix(
        reg.world().seed,
        &[actor as u64, key.type_id as u64, (ft / 10_000_000) as u64],
    );
    let dest = {
        let universe = reg.content().try_get::<Universe>();
        let busy = busy_systems(reg);
        universe.and_then(|u| {
            pick_destination(
                u,
                u.system(origin).map(|s| s.region_id),
                &|s| is_served(reg, s),
                &|s| busy.contains(&s),
                filament.signal,
                seed,
            )
        })
    };
    let Some(dest) = dest else {
        // Nowhere to go: as for a type that is not served, and nothing spent.
        return Err(refusal(&[Fault::bare(code::NOT_ACTIVE)]));
    };
    let spend = if key.quantity > 1 {
        Edit::Consume {
            item_id: key.item_id,
            quantity: 1,
        }
    } else {
        Edit::Destroy {
            item_id: key.item_id,
        }
    };
    reg.apply_items(&[spend])
        .map_err(|e| notify(&format!("The filament could not be spent: {e:?}")))?;
    crate::outln!(
        "[random-jump] {actor} spends type {} in {origin}: {} pilot(s) for {dest} in {:?}",
        key.type_id,
        pilots.len(),
        filament.spool
    );
    reg.ext_mut::<RandomJumps>().pending.insert(
        actor,
        Pending {
            anchor: actor,
            pilots,
            origin,
            dest,
            due: now + filament.spool,
            key_type: key.type_id,
        },
    );
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The stage** (`tick::STAGES`, coordinator-only): every group whose spool
/// ended leaves.
pub fn tick(reg: &mut Registry, now: Instant, ft: BlueTime) {
    if reg.ext::<RandomJumps>().pending.is_empty() {
        return;
    }
    let due: Vec<i64> = pending(reg)
        .filter(|p| p.due <= now)
        .map(|p| p.anchor)
        .collect();
    for anchor in due {
        if let Some(p) = reg.ext_mut::<RandomJumps>().pending.remove(&anchor) {
            launch(reg, &p, ft);
        }
    }
}

/// Send a group on its way. The anchor must still be where they spent the
/// filament; a passenger who left the system, moved out of range, entered a
/// warp or is already in a jump is left behind. Returns who went.
pub fn launch(reg: &mut Registry, p: &Pending, ft: BlueTime) -> Vec<i64> {
    let radius = filament_type(reg.content(), p.key_type).map_or(DEFAULT_RADIUS_M, |f| f.radius_m);
    let anchor_ok = reg
        .roster()
        .get(p.anchor)
        .is_some_and(|a| a.solar_system_id == p.origin)
        && !reg.jumps().contains(p.anchor);
    if !anchor_ok {
        crate::outln!(
            "[random-jump] {} is no longer there: the group stays",
            p.anchor
        );
        return Vec::new();
    }
    let here = carried(reg, p.anchor, radius);
    let going: Vec<i64> = p
        .pilots
        .iter()
        .copied()
        .filter(|w| here.contains(w))
        .filter(|w| !reg.jumps().contains(*w))
        .filter(|w| {
            reg.roster()
                .get(*w)
                .is_some_and(|r| !crate::dungeon_tracking::in_warp(&r.motion))
        })
        .collect();
    let seed = crate::seed::mix(
        reg.world().seed,
        &[p.anchor as u64, p.dest as u64, (ft / 10_000_000) as u64],
    );
    let centre = landing_centre(reg, p.dest, seed);
    for (i, who) in going.iter().enumerate() {
        let Some(there) = reg.roster().get(*who).copied() else {
            continue;
        };
        crate::jump_drive::depart(
            reg,
            *who,
            there.ship_id,
            p.origin,
            p.dest,
            landing_spot(centre, i, seed),
        );
    }
    crate::outln!(
        "[random-jump] {} -> {}: {} of {} pilot(s) left",
        p.origin,
        p.dest,
        going.len(),
        p.pilots.len()
    );
    going
}

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

#[cfg(test)]
mod tests;
