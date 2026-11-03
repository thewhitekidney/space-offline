//! **Combat drones for the fighters** (K-DRONE): a ratter, a site runner, a
//! mission runner or a hunter whose hull has a drone bay carries light combat
//! drones, launches them when it picks a fight, sends them at its target,
//! recalls them before it warps, docks or goes quiet, and replaces the ones
//! it lost.
//!
//! **It wraps, it does not fork.** Every fighter's docked and in-space rules
//! are [`crate::escort`]'s (the hostile, the site runner and the mission
//! runner call them). [`docked`] and [`in_space`] run those rules and add to
//! what they decided, so one edit covers them all:
//!
//! - **Docked.** Once a visit, before the undock, [`DroneAct::Stock`]: if
//!   the hull has a drone bay (`droneCapacity`, attribute 283 [V]) it is
//!   topped up to [`WANT_DRONES`] light drones -- from the hangar, else
//!   bought ([`crate::acquire::buy_more`], closed economy) or given (GM
//!   `/createitem`, open) -- with the skills they need ([`DRONE_SKILLS`]).
//!   The escort role, when [`Policy::combat_drones`] is on, flies
//!   [`DRONE_HULL`] instead of a Rifter ([`rewrite_hull`], [`DroneAct::Hull`]),
//!   because a Rifter has no bay: that is the one knob, and setting
//!   [`DRONE_HULL`] to the Rifter turns the whole thing back into "drones
//!   only where the hull happens to have a bay".
//! - **In space.** When the escort has picked a target ([`Memory::target`])
//!   and is not leaving: [`DroneAct::Engage`] launches what the bay holds
//!   (`ship.LaunchDrones` [V]) and sends every drone out at the target
//!   (`entity.CmdEngage`, `droneFunctions.EngageTarget:109` [V]), once per
//!   target, and again every [`TOP_UP_EVERY`] while fewer than
//!   [`WANT_DRONES`] are out (a lost drone is replaced from the bay). Before
//!   any warp or dock, and when nothing hostile is left, [`DroneAct::Recall`]
//!   (`entity.CmdReturnBay` [V], then the server's scoop). The server's
//!   drones are **aggressive** by default, so they also defend the ship
//!   between orders (`eve_server::drones`, `ChangeDroneSettings`).
//!
//! Every rule is **[A] policy** except the client's calls, cited [V].
//! The failure of a drone act is never a reason to go home
//! ([`crate::brain::ActKind::is_incidental`]): the guns go on.

use crate::brain::{Act, Memory, Observation};
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// Hobgoblin I **[V-data]**: the drone the kit buys (light, thermal, 5 m3).
pub const DRONE_I: i64 = 2_454;
/// The light combat drones the bot will carry **[V-data]**: Hobgoblin,
/// Acolyte, Hornet and Warrior, each in I and II (group 100, 5 m3, bandwidth
/// 5, default effect 10). The bot ships no type table, so a row is a combat
/// drone by its type id.
pub const COMBAT_DRONES: &[i64] = &[
    2_454, 2_456, 2_203, 2_205, 2_464, 2_466, 2_486, 2_488,
    // MINER-DEFENSE: Hammerhead I and II, the medium drones a barge's bay
    // takes [V-data] (group 100, 10 m3, bandwidth 10).
    2_183, 2_185,
];
/// One light drone's volume, m3 **[V-data]**.
pub const DRONE_VOLUME: f64 = 5.0;
/// A Rupture **[V-data]** (Minmatar cruiser, 4 turret hardpoints, 40 m3 bay
/// and 40 Mbit/s): the escort's hull when it flies drones. **[A]** the hull;
/// set it to [`crate::escort::RIFTER`] to put the escort back in the Rifter.
pub const DRONE_HULL: i64 = 629;
/// How many drones the bot carries and keeps out. **[A]**: three, which is
/// Drones III; falsified by bots that sit with an empty bay or one that
/// cannot fly the third.
pub const WANT_DRONES: i64 = 3;
/// Skills `(type, level)` the drone hull and the drones need **[V-data]**:
/// Gunnery's small projectile turret skill (the guns), Minmatar Cruiser
/// (3333, the Rupture), Drones (3436, [`WANT_DRONES`] out at once) and
/// Light Drone Operation (24241, the light drones).
pub const DRONE_SKILLS: &[(i64, i64)] = &[
    (crate::escort::SKILL_GUNNERY, 1),
    (3_333, 1),
    (3_436, WANT_DRONES),
    (24_241, 1),
];
/// What the escort buys when it flies drones: the Rupture, the guns, the
/// rounds, the skills and the drones.
pub const DRONE_KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(DRONE_HULL),
    modules: crate::acquire::FIGHTER_KIT.modules,
    ammo: crate::acquire::FIGHTER_KIT.ammo,
    skills: DRONE_SKILLS,
    more: &[(DRONE_I, WANT_DRONES)],
};
/// `const.flagDroneBay` **[V]**.
pub const FLAG_DRONE_BAY: i64 = 87;
/// `droneCapacity` (283) **[V]**.
pub const ATTR_DRONE_CAPACITY: i64 = 283;
/// How long a recall waits for the drones to come aboard. **[A]** the
/// miner's ([`crate::ore::RECALL_WAIT`]).
pub const RECALL_WAIT: Duration = crate::ore::RECALL_WAIT;
/// While fewer than [`WANT_DRONES`] are out and the bay has more, how often
/// the fight sends for them. **[A]**
pub const TOP_UP_EVERY: Duration = Duration::from_secs(30);

/// Whether `type_id` is one of [`COMBAT_DRONES`].
pub fn is_combat_drone(type_id: i64) -> bool {
    COMBAT_DRONES.contains(&type_id)
}

/// One drone step, docked or in space: the driver's arm is one [`act`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroneAct {
    /// Docked: top the drone bay up to [`WANT_DRONES`].
    Stock,
    /// Docked: bring the drone hull ([`DRONE_HULL`]) into the hangar.
    Hull,
    /// In space: launch what the bay holds if drones are missing, and send
    /// every drone out at this ball.
    Engage(i64),
    /// In space: recall every drone out and wait for them to come aboard.
    Recall,
    /// Docked (MINER-DEFENSE, [`crate::defend`]): stock the drone bay with
    /// what the hull's bay and bandwidth take beside the mining drones.
    Guard,
    /// PROG-PVE, docked: look for a hull above this class that the pilot's
    /// skills and wallet allow, and buy it with the guns and rounds of its
    /// class ([`crate::progression`], [`crate::fit_plan`]).
    Upgrade(crate::hull_choice::Class),
    /// PROG-PVE, docked: buy the guns and rounds the flown class fits.
    Arm(crate::hull_choice::Class),
    /// PROG-PVE2, docked: repackage the old hulls past the spare the bot
    /// keeps and list them on the market ([`crate::hull_sale`]).
    SellHulls,
    /// PROG-PVE2, docked: fly to this station and make it the base, for a
    /// ratter or site runner that has outgrown its band ([`crate::relocate`]).
    Move(i64),
}

impl DroneAct {
    /// The act line.
    pub fn describe(self) -> (String, Vec<i64>) {
        match self {
            DroneAct::Stock => ("combat drones: hangar -> drone bay".to_owned(), vec![]),
            DroneAct::Hull => ("drone hull: acquire".to_owned(), vec![]),
            DroneAct::Engage(t) => (format!("LaunchDrones + CmdEngage -> {t}"), vec![t]),
            DroneAct::Recall => ("CmdReturnBay".to_owned(), vec![]),
            DroneAct::Guard => ("defence drones: hangar -> drone bay".to_owned(), vec![]),
            DroneAct::Upgrade(c) => (format!("hull upgrade: look above {c:?}"), vec![]),
            DroneAct::Arm(c) => (format!("arm: guns and rounds for a {c:?}"), vec![]),
            DroneAct::SellHulls => ("sell the old hulls past the spare".to_owned(), vec![]),
            DroneAct::Move(s) => (format!("relocate to station {s}"), vec![s]),
        }
    }
}

/// What the bot remembers about its drones: intentions only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The drones in space, as the last launch left them.
    pub out: Vec<i64>,
    /// The ball they were last sent at.
    pub engaged: Option<i64>,
    /// The bay held no more drones at the last launch: no more tries this
    /// trip.
    pub dry: bool,
    /// The bay was stocked this visit.
    pub stocked: bool,
    /// MINER-DEFENSE: how many drones the defending miner keeps out; `0`
    /// means [`WANT_DRONES`] ([`Mind::want_out`]).
    pub want: i64,
    /// GM gave drones already (once a run; the open mode only).
    pub gm_given: bool,
    /// Hull types a boarding was refused for (ship-pool): never chosen again.
    pub refused: Vec<i64>,
    /// SKILLGATE: hull types the pilot's skills do not board yet (a boarding
    /// the bot did not send); skipped by the chooser, cleared when the skill
    /// plan is filled again ([`crate::skill_plan::fill`]).
    pub untrained: Vec<i64>,
    /// DOC-5: the fit the leader's doctrine gives this bot, set from the
    /// hall each docked turn ([`crate::doctrine_assign::MemberPlan`]); `None`
    /// without a doctrine, and the old path runs unchanged.
    pub doctrine: Option<crate::doctrine::RoleFit>,
    /// BP-CAMP: the role that fit is for (the same sync); an Interdict bot
    /// camps ([`crate::camp`]).
    pub role: Option<crate::doctrine::Role>,
    /// The hull type the last `Board` was for (ship-pool).
    pub boarding: Option<i64>,
    /// PROG-PVE: not before this, the next look for a bigger hull
    /// ([`crate::progression::UPGRADE_EVERY`]).
    pub upgrade_next: Option<Instant>,
    /// PROG-PVE: not before this, the next purchase of the class's guns
    /// ([`ARM_EVERY`]).
    pub arm_next: Option<Instant>,
    /// When the last launch was tried.
    pub launched_at: Option<Instant>,
    /// PROG-PVE2: the race of the hull the bot flies or holds
    /// ([`crate::progression::rung`]), set at each docked turn; the weapon
    /// follows it ([`weapon`]).
    pub race: Option<crate::progression::Race>,
    /// PROG-PVE2: the flown hull is a Tech III one ([`crate::t3::is_flying`]),
    /// as of the last docked turn: the fight steers its stance.
    pub t3_hull: bool,
    /// The weapon system of the Tech III kit the bot flies
    /// ([`crate::t3::kit_weapon`]); [`weapon`] answers it first, so fit,
    /// ammo and skills agree with the hardpoints the kit has.
    pub t3_weapon: Option<crate::hull_choice::Weapon>,
    /// PROG-PVE2: not before this, the next stance request
    /// ([`crate::t3::STANCE_EVERY`]).
    pub stance_next: Option<Instant>,
    /// PROG-PVE2: not before this, the next look at the spare hulls
    /// ([`crate::hull_sale::SELL_EVERY`]).
    pub sale_next: Option<Instant>,
    /// PROG-PVE2: not before this, the next look for a better band
    /// ([`crate::relocate::EVERY`]).
    pub move_next: Option<Instant>,
    /// PROG-PVE2: what the bot remembers about its tank modules
    /// ([`crate::tank::outfit`]).
    pub tank: crate::tank::State,
    /// PROG-PVE2: the stance last set ([`crate::t3::set_stance`]).
    pub stance: Option<i64>,
}

impl Mind {
    /// How many drones to keep out: the defender's own count, else
    /// [`WANT_DRONES`].
    pub fn want_out(&self) -> i64 {
        if self.want > 0 {
            self.want
        } else {
            WANT_DRONES
        }
    }
}

// ---- the brain's rules (pure) ----------------------------------------------

/// Whether this bot is the one that flies [`DRONE_HULL`]: the escort role
/// with the policy on, in the open mode (closed play chooses its hull,
/// [`crate::hull_choice`]).
pub fn drone_role(m: &Memory) -> bool {
    m.policy.combat_drones && !m.policy.closed && std::ptr::eq(m.policy.role, &crate::roles::ESCORT)
}

/// **The chooser's need** (ship-pool) for this bot with `wallet` ISK: the
/// closed economy's fighters fly what [`crate::hull_choice`] picks, the open
/// mode the fixed [`hull`].
pub fn need(m: &Memory, wallet: f64) -> crate::hull_choice::Need<'_> {
    crate::hull_choice::Need {
        weapon: weapon(m),
        drones: m.policy.combat_drones || race_of(m).is_some_and(crate::progression::uses_drones),
        wallet,
        risk: crate::plexer::PLANS.risk(&m.name),
        bias: crate::hull_choice::bias_of(m.policy.role),
        taste: m.seed,
        avoid: &m.combat.refused,
        untrained: &m.combat.untrained,
        strict: false,
        doctrine: m.combat.doctrine.as_ref(),
    }
}

/// The race the bot's weapon follows (PROG-PVE2): the race of the hull it
/// flies or holds ([`Mind::race`]), else the one drawn from its seed; `None`
/// in the open mode, whose GM kit gives every bot the same small guns.
pub fn race_of(m: &Memory) -> Option<crate::progression::Race> {
    m.policy.closed.then(|| {
        m.combat
            .race
            .unwrap_or_else(|| crate::progression::Race::draw(m.seed))
    })
}

/// **The weapon system this bot fires** (PROG-PVE2): its race's
/// ([`crate::progression::weapon_of`]) in the closed economy, projectile in
/// the open mode ([`crate::hull_choice::BOT_WEAPON`]).
pub fn weapon(m: &Memory) -> crate::hull_choice::Weapon {
    if let Some(w) = m.combat.t3_weapon {
        return w;
    }
    race_of(m).map_or(crate::hull_choice::BOT_WEAPON, |r| {
        crate::progression::weapon_of(r, m.seed)
    })
}

/// The hull the escort rules want: [`DRONE_HULL`] for the drone role, else
/// a Rifter.
pub fn hull(m: &Memory) -> i64 {
    if drone_role(m) {
        DRONE_HULL
    } else {
        crate::escort::RIFTER
    }
}

/// Whether an act takes the ship away from where its drones are.
fn leaves(a: &Act) -> bool {
    matches!(
        a,
        Act::WarpToItem { .. } | Act::WarpToMember { .. } | Act::Dock(_) | Act::Reconnect
    )
}

/// **The hull rewrite**: the escort rules ask for a hull through GM or the
/// market ([`Act::GmSetup`] / [`Act::Acquire`] with `ship`); the drone role
/// gets its own hull first ([`DroneAct::Hull`]) and the rest of the request
/// without the hull. Every closed-economy fighter gets the chooser's hull
/// ([`crate::hull_choice`]) the same way.
pub fn rewrite_hull(m: &Memory, acts: Vec<Act>) -> Vec<Act> {
    if !drone_role(m) && !m.policy.closed {
        return acts;
    }
    let mut out = Vec::new();
    for a in acts {
        match a {
            Act::GmSetup {
                ship: true,
                module,
                ammo,
            } => {
                out.push(Act::Drone(DroneAct::Hull));
                out.push(Act::GmSetup {
                    ship: false,
                    module,
                    ammo,
                });
            }
            Act::Acquire {
                ship: true,
                module,
                ammo,
            } => {
                out.push(Act::Drone(DroneAct::Hull));
                out.push(Act::Acquire {
                    ship: false,
                    module,
                    ammo,
                });
            }
            other => out.push(other),
        }
    }
    out
}

/// **Docked**: `core` (the escort's rules), with the hull rewritten and,
/// once a visit, the drone bay stocked just before the undock.
pub fn docked(
    m: &mut Memory,
    o: &Observation,
    core: fn(&mut Memory, &Observation) -> Vec<Act>,
) -> Vec<Act> {
    if !m.policy.combat_drones {
        return core(m, o);
    }
    // Whatever was out is a matter for the server's grace now.
    m.combat.out.clear();
    m.combat.engaged = None;
    m.combat.dry = false;
    let asked = core(m, o);
    let acts = rewrite_hull(m, asked);
    if !m.combat.stocked && acts.contains(&Act::Undock) {
        m.combat.stocked = true;
        let mut out = vec![Act::Drone(DroneAct::Stock)];
        out.extend(acts);
        return out;
    }
    acts
}

/// **In space**: `core`'s acts, with the drones steered around them
/// ([`steer`]).
pub fn in_space(
    m: &mut Memory,
    o: &Observation,
    core: fn(&mut Memory, &Observation) -> Vec<Act>,
) -> Vec<Act> {
    if !m.policy.combat_drones {
        return core(m, o);
    }
    m.combat.stocked = false;
    let acts = core(m, o);
    steer(m, o.now, acts)
}

/// **The steering** (pure): recall before the ship leaves or when nothing
/// hostile is left; engage when it fights.
pub fn steer(m: &mut Memory, now: Instant, acts: Vec<Act>) -> Vec<Act> {
    if acts.iter().any(leaves) {
        if m.combat.out.is_empty() {
            return acts;
        }
        let mut out = vec![Act::Drone(DroneAct::Recall)];
        out.extend(acts);
        return out;
    }
    match m.target.as_ref().map(|(id, _)| *id) {
        None => {
            if m.combat.out.is_empty() {
                return acts;
            }
            let mut out = vec![Act::Drone(DroneAct::Recall)];
            out.extend(acts);
            out
        }
        Some(target) => {
            let c = &m.combat;
            let missing = (c.out.len() as i64) < WANT_DRONES && !c.dry;
            let due = c
                .launched_at
                .is_none_or(|t| now.saturating_duration_since(t) >= TOP_UP_EVERY);
            let new_target = c.engaged != Some(target);
            if !(new_target && !(c.dry && c.out.is_empty()) || missing && due) {
                return acts;
            }
            let mut out = vec![Act::Drone(DroneAct::Engage(target))];
            out.extend(acts);
            out
        }
    }
}

/// **The stocking plan** (pure): of a bay of `capacity` m3 holding `in_bay`
/// drones, with `in_hangar` more in the hangar, how many to buy and how many
/// to move into the bay to reach [`WANT_DRONES`] (or what the bay holds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockPlan {
    pub buy: i64,
    pub moves: i64,
}

/// See [`StockPlan`].
pub fn plan_stock(capacity: f64, in_bay: i64, in_hangar: i64) -> StockPlan {
    let room = if capacity.is_finite() && capacity > 0.0 {
        (capacity / DRONE_VOLUME + 1e-9).floor() as i64
    } else {
        0
    };
    let want = room.min(WANT_DRONES);
    let buy = (want - in_bay - in_hangar).max(0);
    let moves = (want - in_bay).max(0).min(in_hangar + buy);
    StockPlan { buy, moves }
}

// ---- the arm (I/O) ----------------------------------------------------------

/// What the driver gives the arm: how the bot gets what it lacks.
pub struct Ctx<'a> {
    /// The closed economy: buy, never GM.
    pub closed: bool,
    /// The pilot sits in a capsule.
    pub in_capsule: bool,
    pub trips: Option<crate::travel::Trips<'a>>,
    /// The bundle, for the Tech III step (T3-H); `None` without `--content`.
    pub content: Option<&'a eve_server::content::Content>,
}

/// What an arm did, for the act log: a note, and the GM lines it sent with
/// their refusals, which the driver records as `slash` entries.
#[derive(Debug, Default)]
pub struct Done {
    pub note: Option<String>,
    pub slashes: Vec<(String, Option<String>)>,
    /// The hangar changed.
    pub docked_dirty: bool,
    /// PROG-PVE: the ISK an arm laid out on assets (a hull, guns, books),
    /// which the income ledger does not count as a loss
    /// ([`crate::earnings::Tracker::invest`]).
    pub spent: f64,
    /// PROG-PVE2: the station the bot now calls home after a
    /// [`DroneAct::Move`]; the driver records it.
    pub moved_to: Option<i64>,
}

/// **One [`DroneAct`]**, the driver's arm. Soft: a bay with no room, a
/// refused launch or a missing drone is a note, not an error.
pub fn act(c: &mut Conn, m: &mut Memory, op: DroneAct, ctx: Ctx<'_>) -> Result<Done> {
    // A bot that flies a Tech III hull after a restart has no kit weapon yet:
    // read it from the fitted subsystems.
    if ctx.closed
        && m.combat.t3_hull
        && m.combat.t3_weapon.is_none()
        && let Some(content) = ctx.content
    {
        m.combat.t3_weapon = crate::t3::flown_weapon(c, content, m.seed)?;
    }
    match op {
        DroneAct::Stock => stock(c, m, ctx),
        DroneAct::Guard => crate::defend::stock(c, m, ctx),
        DroneAct::Hull => hull_in(c, m, ctx),
        DroneAct::Upgrade(held) => upgrade_in(c, m, held, ctx),
        DroneAct::Arm(class) => arm_in(c, m, class, ctx),
        DroneAct::SellHulls => sell_hulls(c, m),
        DroneAct::Move(station) => relocate(c, station, ctx),
        DroneAct::Engage(target) => {
            engage(c, m, target)?;
            Ok(Done::default())
        }
        DroneAct::Recall => {
            let mut out = std::mem::take(&mut m.combat.out);
            // Drones that died are not coming back: waiting for them would
            // spend the whole recall wait.
            out.retain(|id| c.world().get(*id).is_some());
            m.combat.engaged = None;
            crate::ore::recall(c, &out)?;
            Ok(Done::default())
        }
    }
}

fn bay_drones(c: &mut Conn, ship: i64) -> Result<Vec<ItemRow>> {
    Ok(c.ship_items(ship, Some(FLAG_DRONE_BAY))?
        .into_iter()
        .filter(|r| is_combat_drone(r.type_id))
        .collect())
}

fn count(rows: &[ItemRow]) -> i64 {
    rows.iter().map(|r| r.quantity.max(1)).sum()
}

/// A Tech III kit was assembled: its weapon steers the next buy (the
/// launcher a missile-only hull fires, not the seed's turret), and the buy
/// timer and miss streak reset so that buy is not put off.
fn adopt_t3_weapon(m: &mut Memory, weapon: Option<crate::hull_choice::Weapon>) {
    m.combat.t3_weapon = weapon;
    m.acquire_next = None;
    m.missing_streak = 0;
}

/// Docked: bring a combat hull in. Closed economy: the chooser
/// ([`crate::hull_choice::obtain`]), never GM. Open mode: the fixed drone
/// hull, GM-made.
fn hull_in(c: &mut Conn, m: &mut Memory, ctx: Ctx<'_>) -> Result<Done> {
    let mut done = Done {
        docked_dirty: true,
        ..Done::default()
    };
    if ctx.closed {
        // T3-H: a Tech III kit the pilot holds or can buy and use comes first.
        if let Some(content) = ctx.content {
            let t3 = crate::t3::pilot_step(c, content, m.seed)?;
            if t3.flying {
                adopt_t3_weapon(m, t3.weapon);
                done.note = Some(t3.note);
                return Ok(done);
            }
            if !t3.note.is_empty() {
                done.note = Some(t3.note);
            }
        }
        let wallet = c.cash_balance()?;
        let got = crate::hull_choice::obtain(c, &need(m, wallet), ctx.trips, ctx.in_capsule)?;
        done.note = Some(got.note);
        return Ok(done);
    }
    if c.hangar_items()?.iter().any(|r| r.type_id == DRONE_HULL) {
        done.note = Some("the drone hull is in the hangar".to_owned());
        return Ok(done);
    }
    let line = format!("/createitem {DRONE_HULL}");
    let outcome = crate::gm::slash(c, &line)?;
    done.slashes.push((line, outcome.refusal()));
    Ok(done)
}

/// How often a docked bot buys the guns of the class it flies **[A]**: a
/// station may not sell them, and a fail should not be repeated at once.
pub const ARM_EVERY: Duration = Duration::from_secs(10 * 60);

/// **The guns and rounds of a class**, bought (PROG-PVE): its turret skill,
/// [`crate::fit_plan::Fit::guns`] guns and [`crate::escort::SETUP_ROUNDS`]
/// rounds of the class's size, up to what the pilot already holds
/// ([`crate::acquire::buy_more`]). No GM. `spent` is the ISK it laid out.
fn arm_in(
    c: &mut Conn,
    m: &mut Memory,
    class: crate::hull_choice::Class,
    ctx: Ctx<'_>,
) -> Result<Done> {
    let fit = crate::fit_plan::fit_for(class, weapon(m));
    let mut order = crate::acquire::Order::default();
    order
        .want_skill(fit.skill, 1)
        .want_module(fit.gun, fit.guns as i64)
        .want_charge(fit.round, crate::escort::SETUP_ROUNDS);
    let got = crate::acquire::buy_order(c, &order, ctx.trips)?;
    Ok(Done {
        note: Some(format!("arm for {class:?}: {}", got.summary())),
        docked_dirty: true,
        spent: (got.before - got.after).max(0.0),
        ..Done::default()
    })
}

/// **The old hulls, sold** (PROG-PVE2): [`crate::hull_sale::sell_spares`].
fn sell_hulls(c: &mut Conn, _m: &mut Memory) -> Result<Done> {
    let sold = crate::hull_sale::sell_spares(c)?;
    Ok(Done {
        note: Some(sold.summary()),
        docked_dirty: true,
        ..Done::default()
    })
}

/// **Fly to a richer system and live there** (PROG-PVE2,
/// [`crate::relocate`]): [`crate::travel::go`] to `station`; the base moves
/// with the bot once it is docked there. No map is a note.
fn relocate(c: &mut Conn, station: i64, ctx: Ctx<'_>) -> Result<Done> {
    let Some(trips) = ctx.trips else {
        return Ok(Done {
            note: Some("relocate: no map to fly over".to_owned()),
            ..Done::default()
        });
    };
    let trip = crate::travel::go(c, trips, station)?;
    Ok(Done {
        note: Some(format!(
            "relocated to station {station}: {} gates",
            trip.jumps
        )),
        docked_dirty: true,
        moved_to: (c.session().station_id() == Some(station)).then_some(station),
        ..Done::default()
    })
}

/// **A bigger hull** (PROG-PVE): the hulls above `held` whose racial hull
/// skill and gun skill the pilot has trained ([`crate::progression::wanted`]),
/// the best the wallet covers ([`crate::hull_choice::obtain_from`], less the
/// fit's reserve), and the class's guns and rounds with it. Nothing to buy is
/// a note, not a fault: the bot looks again in
/// [`crate::progression::UPGRADE_EVERY`].
fn upgrade_in(
    c: &mut Conn,
    m: &mut Memory,
    held: crate::hull_choice::Class,
    ctx: Ctx<'_>,
) -> Result<Done> {
    let mut done = Done::default();
    if !ctx.closed {
        return Ok(done);
    }
    let levels = crate::acquire::skill_levels(c)?;
    let table =
        crate::progression::wanted_with(held, &levels, weapon(m), m.combat.doctrine.as_ref());
    if table.is_empty() {
        done.note = Some(format!(
            "hull upgrade: no hull above {held:?} the skills allow"
        ));
        return Ok(done);
    }
    let wallet = c.cash_balance()?;
    let mut need = need(m, crate::progression::upgrade_wallet(wallet));
    need.strict = true;
    let got = crate::hull_choice::obtain_from(c, &table, &need, ctx.trips, false, false)?;
    done.docked_dirty = true;
    match got.hull {
        Some(h) => {
            let class = crate::hull_choice::class_of(crate::hull_choice::HULLS, h).unwrap_or(held);
            let armed = arm_in(c, m, class, ctx)?;
            done.spent = (wallet - c.cash_balance()?).max(0.0);
            done.note = Some(format!(
                "hull upgrade: bought {h} ({class:?}); {}; {}",
                got.note,
                armed.note.unwrap_or_default()
            ));
        }
        None => {
            done.note = Some(format!(
                "hull upgrade: nothing bought above {held:?} ({})",
                got.note
            ));
        }
    }
    Ok(done)
}

/// Docked: top the drone bay up ([`plan_stock`]).
fn stock(c: &mut Conn, m: &mut Memory, ctx: Ctx<'_>) -> Result<Done> {
    let mut done = Done {
        docked_dirty: true,
        ..Done::default()
    };
    let (Some(ship), Some(station)) = (c.session().ship_id(), c.session().station_id()) else {
        return Err(Error::NotSent("drones: stock: not docked in a ship".into()));
    };
    // The served dogma row may not carry the bay (it lists what the client
    // reads); an unanswered ask means "try, and let the server's refusal say".
    let capacity = c
        .ship_attribute(ship, ATTR_DRONE_CAPACITY)?
        .unwrap_or(WANT_DRONES as f64 * DRONE_VOLUME);
    if capacity < DRONE_VOLUME {
        done.note = Some("no drone bay".to_owned());
        return Ok(done);
    }
    let in_bay = count(&bay_drones(c, ship)?);
    let hangar: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| is_combat_drone(r.type_id))
        .collect();
    let plan = plan_stock(capacity, in_bay, count(&hangar));
    if plan.buy > 0 {
        if ctx.closed {
            let mut order = crate::acquire::Order::default();
            for &(s, l) in DRONE_SKILLS {
                order.want_skill(s, l);
            }
            order.want_module(DRONE_I, plan.buy + count(&hangar) + in_bay);
            let got = crate::acquire::buy_order(c, &order, ctx.trips)?;
            let bought: i64 = got
                .bought
                .iter()
                .filter(|b| is_combat_drone(b.0))
                .map(|b| b.1)
                .sum();
            let mut note = got.summary();
            // No drones on offer: rest a bid, which is the demand the
            // builders read (ship-pool).
            if bought < plan.buy {
                let wallet = c.cash_balance()?;
                if crate::hull_choice::bid_for(c, DRONE_I, plan.buy - bought, wallet)? {
                    note.push_str("; bid for drones");
                }
            }
            done.note = Some(note);
        } else if !m.combat.gm_given {
            m.combat.gm_given = true;
            for line in [
                format!("/giveskill 3436 {WANT_DRONES}"),
                "/giveskill 24241 1".to_owned(),
                format!("/createitem {DRONE_I} {}", plan.buy),
            ] {
                let outcome = crate::gm::slash(c, &line)?;
                done.slashes.push((line, outcome.refusal()));
            }
        }
    }
    if let Some(note) = move_to_bay(c, ship, station, plan.moves, &is_combat_drone)? {
        done.note = Some(note);
    }
    Ok(done)
}

/// Docked: move up to `left` of the hangar's drones that `wanted` accepts
/// into the ship's drone bay. A refusal (no room) is the note returned.
pub(crate) fn move_to_bay(
    c: &mut Conn,
    ship: i64,
    station: i64,
    mut left: i64,
    wanted: &dyn Fn(i64) -> bool,
) -> Result<Option<String>> {
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| wanted(r.type_id))
        .collect();
    for s in stacks {
        if left <= 0 {
            break;
        }
        let n = left.min(s.quantity.max(1));
        let inventory = c.open_ship(ship)?;
        let outcome = c.call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(s.item_id), PyValue::Int(station)],
            vec![
                (PyValue::Int(n), crate::bstr("qty")),
                (PyValue::Int(FLAG_DRONE_BAY), crate::bstr("flag")),
            ],
        )?;
        if let Some(key) = outcome.refusal() {
            // The hull has no room for them (a bay the ask could not see).
            return Ok(Some(format!(
                "Add({}, flag={FLAG_DRONE_BAY}): {key}",
                s.item_id
            )));
        }
        left -= n;
    }
    Ok(None)
}

/// In space: launch what the bay holds when drones are missing, then send
/// every drone out at `target`.
fn engage(c: &mut Conn, m: &mut Memory, target: i64) -> Result<()> {
    // Drones that died are not in the park any more.
    m.combat.out.retain(|id| c.world().get(*id).is_some());
    let want = m.combat.want_out();
    if (m.combat.out.len() as i64) < want && !m.combat.dry {
        m.combat.launched_at = Some(Instant::now());
        let fresh = launch(c, want - m.combat.out.len() as i64)?;
        if fresh.is_empty() {
            m.combat.dry = true;
        }
        m.combat.out.extend(fresh);
    }
    if m.combat.out.is_empty() {
        m.combat.engaged = Some(target);
        return Ok(());
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("drones: engage: not in space".into()))?;
    let answer = c.moniker_call(
        crate::ore::ENTITY,
        PyValue::Int(system),
        "CmdEngage",
        vec![
            PyValue::List(m.combat.out.iter().map(|i| PyValue::Int(*i)).collect()),
            PyValue::Int(target),
        ],
        vec![],
    )?;
    if let Some(key) = answer.refusal() {
        return Err(Error::Refused(format!("drones: CmdEngage: {key}")));
    }
    // `{droneID: (label, args)}` for each refused (`HandleMultipleCallError`
    // [V]); all of them refused is a target the drones cannot take (out of
    // range): ask again next turn.
    let refused = answer
        .value
        .as_ref()
        .and_then(PyValue::dict_as_kv)
        .map_or(0, |kv| kv.len());
    m.combat.engaged = (refused < m.combat.out.len()).then_some(target);
    Ok(())
}

/// `ship.LaunchDrones` [V] for up to `n` of the bay's combat drones; the
/// ids launched.
fn launch(c: &mut Conn, n: i64) -> Result<Vec<i64>> {
    let Some(ship) = c.session().ship_id() else {
        return Err(Error::NotSent("drones: launch: no ship".into()));
    };
    let mut left = n;
    let mut items = Vec::new();
    let mut stacks = Vec::new();
    for r in bay_drones(c, ship)? {
        if left <= 0 {
            break;
        }
        let q = left.min(r.quantity.max(1));
        items.push(PyValue::Tuple(vec![
            PyValue::Int(r.item_id),
            PyValue::Int(q),
        ]));
        stacks.push(r.item_id);
        left -= q;
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let params = c
        .session()
        .location_moniker()
        .ok_or_else(|| Error::NotSent("drones: launch: no location".into()))?;
    let answer = c.moniker_call(
        "ship",
        params,
        "LaunchDrones",
        vec![PyValue::List(items), PyValue::None, PyValue::Bool(false)],
        vec![],
    )?;
    if let Some(key) = answer.refusal() {
        return Err(Error::Refused(format!("drones: LaunchDrones: {key}")));
    }
    let value = answer.value.unwrap_or(PyValue::None);
    Ok(stacks
        .iter()
        .flat_map(|s| crate::ore::launched(&value, *s))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Policy, Role};
    use std::time::Duration;

    const TARGET: i64 = 8_100_000_001;
    const OTHER: i64 = 8_100_000_002;
    const DRONE: i64 = 100_000_901;

    fn memory(on: bool) -> Memory {
        let p = Policy {
            role: Role::Escort,
            combat_drones: on,
            ..Policy::fast()
        };
        Memory::new("drone-test", p)
    }

    fn fighting(m: &mut Memory, t: i64) {
        m.target = Some((t, "Rat".to_owned()));
    }

    fn engage_acts(acts: &[Act]) -> Vec<i64> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Drone(DroneAct::Engage(t)) => Some(*t),
                _ => None,
            })
            .collect()
    }

    fn recalls(acts: &[Act]) -> usize {
        acts.iter()
            .filter(|a| matches!(a, Act::Drone(DroneAct::Recall)))
            .count()
    }

    #[test]
    fn a_light_combat_drone_is_recognised_by_its_id() {
        assert!(is_combat_drone(DRONE_I));
        assert!(!is_combat_drone(crate::ore::MINING_DRONE_I));
        assert!(!is_combat_drone(0));
    }

    /// A new target sends the drones once; the same target does not send
    /// them again; another target does.
    #[test]
    fn a_target_sends_the_drones_once() {
        let mut m = memory(true);
        let now = Instant::now();
        fighting(&mut m, TARGET);
        let acts = steer(&mut m, now, vec![Act::Wait(Duration::from_secs(1))]);
        assert_eq!(engage_acts(&acts), vec![TARGET]);
        // The arm has run: three drones out, sent.
        m.combat.out = vec![1, 2, 3];
        m.combat.engaged = Some(TARGET);
        m.combat.launched_at = Some(now);
        let again = steer(&mut m, now, vec![Act::Wait(Duration::from_secs(1))]);
        assert!(engage_acts(&again).is_empty());
        fighting(&mut m, OTHER);
        let next = steer(&mut m, now, vec![Act::Wait(Duration::from_secs(1))]);
        assert_eq!(engage_acts(&next), vec![OTHER]);
    }

    /// Lost drones are sent for again, but not more often than the pace,
    /// and not once the bay has said it is dry.
    #[test]
    fn lost_drones_are_replaced_from_the_bay_at_a_pace() {
        let mut m = memory(true);
        let t0 = Instant::now();
        fighting(&mut m, TARGET);
        m.combat.out = vec![1];
        m.combat.engaged = Some(TARGET);
        m.combat.launched_at = Some(t0);
        let wait = vec![Act::Wait(Duration::from_secs(1))];
        assert!(engage_acts(&steer(&mut m, t0 + Duration::from_secs(1), wait.clone())).is_empty());
        assert_eq!(
            engage_acts(&steer(&mut m, t0 + TOP_UP_EVERY, wait.clone())),
            vec![TARGET]
        );
        m.combat.dry = true;
        assert!(engage_acts(&steer(&mut m, t0 + TOP_UP_EVERY * 4, wait)).is_empty());
    }

    /// An empty bay that has said so is not asked again for the same
    /// target.
    #[test]
    fn an_empty_bay_is_asked_once_a_target() {
        let mut m = memory(true);
        let now = Instant::now();
        fighting(&mut m, TARGET);
        m.combat.dry = true;
        m.combat.engaged = Some(TARGET);
        let wait = vec![Act::Wait(Duration::from_secs(1))];
        assert!(engage_acts(&steer(&mut m, now, wait.clone())).is_empty());
        fighting(&mut m, OTHER);
        assert!(
            engage_acts(&steer(&mut m, now, wait)).is_empty(),
            "nothing to send"
        );
    }

    /// **Recall before any warp or dock**, and when the fight is over.
    #[test]
    fn drones_come_home_before_the_ship_leaves_and_when_it_is_quiet() {
        let mut m = memory(true);
        let now = Instant::now();
        m.combat.out = vec![1, 2];
        fighting(&mut m, TARGET);
        for leaving in [
            Act::WarpToItem {
                id: TARGET,
                range: 1.0,
            },
            Act::WarpToMember {
                char_id: 9,
                range: 1.0,
            },
            Act::Dock(5),
        ] {
            let acts = steer(&mut m, now, vec![leaving.clone()]);
            assert_eq!(acts[0], Act::Drone(DroneAct::Recall), "{leaving:?}");
            assert_eq!(acts.last(), Some(&leaving));
            assert!(engage_acts(&acts).is_empty(), "no launch on the way out");
        }
        m.target = None;
        let quiet = steer(&mut m, now, vec![Act::Wait(Duration::from_secs(5))]);
        assert_eq!(recalls(&quiet), 1);
        m.combat.out.clear();
        let quiet = steer(&mut m, now, vec![Act::Wait(Duration::from_secs(5))]);
        assert_eq!(recalls(&quiet), 0, "nothing out, nothing to recall");
    }

    #[test]
    fn with_the_policy_off_the_acts_are_the_cores() {
        fn core(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Undock]
        }
        let mut m = memory(false);
        let o = Observation::empty(Instant::now());
        assert_eq!(docked(&mut m, &o, core), vec![Act::Undock]);
        assert_eq!(in_space(&mut m, &o, core), vec![Act::Undock]);
    }

    /// Docked: the bay is stocked once, right before the undock.
    #[test]
    fn the_bay_is_stocked_once_a_visit_just_before_the_undock() {
        fn core(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Undock]
        }
        fn dwelling(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Wait(Duration::from_secs(1))]
        }
        let mut m = memory(true);
        let o = Observation::empty(Instant::now());
        assert_eq!(
            docked(&mut m, &o, dwelling),
            vec![Act::Wait(Duration::from_secs(1))]
        );
        assert_eq!(
            docked(&mut m, &o, core),
            vec![Act::Drone(DroneAct::Stock), Act::Undock]
        );
        assert_eq!(docked(&mut m, &o, core), vec![Act::Undock], "once");
        // Space resets the visit.
        let _ = in_space(&mut m, &o, dwelling);
        assert_eq!(docked(&mut m, &o, core).len(), 2);
    }

    /// The drone role asks for its own hull wherever the escort rules ask
    /// for one; others' requests are not touched.
    #[test]
    fn the_hull_request_becomes_the_drone_hulls() {
        let m = memory(true);
        assert!(drone_role(&m));
        assert_eq!(hull(&m), DRONE_HULL);
        let asked = vec![
            Act::GmSetup {
                ship: true,
                module: true,
                ammo: false,
            },
            Act::Acquire {
                ship: true,
                module: false,
                ammo: true,
            },
            Act::Undock,
        ];
        let got = rewrite_hull(&m, asked.clone());
        assert_eq!(
            got,
            vec![
                Act::Drone(DroneAct::Hull),
                Act::GmSetup {
                    ship: false,
                    module: true,
                    ammo: false
                },
                Act::Drone(DroneAct::Hull),
                Act::Acquire {
                    ship: false,
                    module: false,
                    ammo: true
                },
                Act::Undock,
            ]
        );
        let off = memory(false);
        assert_eq!(hull(&off), crate::escort::RIFTER);
        assert_eq!(rewrite_hull(&off, asked.clone()), asked);
    }

    /// The plan: buy what is short, move what fits.
    #[test]
    fn the_stock_plan_buys_the_shortfall_and_moves_what_fits() {
        // 40 m3 holds eight; the want is the lesser.
        let p = plan_stock(40.0, 0, 0);
        assert_eq!((p.buy, p.moves), (WANT_DRONES, WANT_DRONES));
        // Some aboard, some in the hangar: nothing to buy.
        let p = plan_stock(40.0, 1, WANT_DRONES);
        assert_eq!((p.buy, p.moves), (0, WANT_DRONES - 1));
        // Full already.
        assert_eq!(
            plan_stock(40.0, WANT_DRONES, 5),
            StockPlan { buy: 0, moves: 0 }
        );
        // A rookie bay holds one.
        let p = plan_stock(5.0, 0, 0);
        assert_eq!((p.buy, p.moves), (1, 1));
        // No bay: nothing.
        assert_eq!(plan_stock(0.0, 0, 3), StockPlan { buy: 0, moves: 0 });
        assert_eq!(plan_stock(4.9, 0, 3), StockPlan { buy: 0, moves: 0 });
    }

    #[test]
    fn the_kit_flies_the_drone_hull_and_buys_drones() {
        assert_eq!(DRONE_KIT.hull, Some(DRONE_HULL));
        assert!(DRONE_KIT.more.contains(&(DRONE_I, WANT_DRONES)));
        assert!(DRONE_KIT.skills.contains(&(3_436, WANT_DRONES)));
    }

    #[test]
    fn the_act_lines_say_what_they_do() {
        assert!(DroneAct::Engage(TARGET).describe().0.contains("CmdEngage"));
        assert_eq!(DroneAct::Engage(TARGET).describe().1, vec![TARGET]);
        assert!(DroneAct::Recall.describe().0.contains("CmdReturnBay"));
        let _ = DRONE;
    }

    #[test]
    fn the_weapon_follows_the_race_of_the_hull_and_the_open_mode_stays_projectile() {
        use crate::hull_choice::Weapon;
        use crate::progression::Race;
        let mut m = memory(false);
        // The open mode: the GM kit's small projectile guns, whatever the hull.
        assert!(!m.policy.closed);
        assert_eq!(race_of(&m), None);
        assert_eq!(weapon(&m), Weapon::Projectile);
        m.combat.race = Some(Race::Amarr);
        assert_eq!(weapon(&m), Weapon::Projectile);
        // Closed: the race of the hull flown or held.
        m.policy.closed = true;
        assert_eq!(weapon(&m), Weapon::Laser);
        m.combat.race = Some(Race::Gallente);
        assert_eq!(weapon(&m), Weapon::Hybrid);
        m.combat.race = Some(Race::Minmatar);
        assert_eq!(weapon(&m), Weapon::Projectile);
        // No hull yet: the race drawn from the bot's seed.
        m.combat.race = None;
        assert_eq!(race_of(&m), Some(Race::draw(m.seed)));
        assert_eq!(
            weapon(&m),
            crate::progression::weapon_of(Race::draw(m.seed), m.seed)
        );
    }

    #[test]
    fn a_tech_iii_kit_weapon_beats_the_races() {
        use crate::hull_choice::Weapon;
        use crate::progression::Race;
        let mut m = memory(false);
        m.policy.closed = true;
        m.combat.race = Some(Race::Amarr);
        m.combat.t3_weapon = Some(Weapon::Missile);
        assert_eq!(weapon(&m), Weapon::Missile);
        m.combat.t3_weapon = None;
        assert_eq!(weapon(&m), Weapon::Laser);
    }
    #[test]
    fn a_new_tech_iii_kit_clears_the_buy_timer() {
        use crate::hull_choice::Weapon;
        let mut m = memory(false);
        m.acquire_next = Some(std::time::Instant::now() + std::time::Duration::from_secs(600));
        m.missing_streak = 5;
        adopt_t3_weapon(&mut m, Some(Weapon::Missile));
        assert_eq!(m.combat.t3_weapon, Some(Weapon::Missile));
        assert!(m.acquire_next.is_none());
        assert_eq!(m.missing_streak, 0);
    }

    #[test]
    fn the_chooser_needs_what_the_race_fires_and_a_drone_race_wants_a_bay() {
        use crate::hull_choice::Weapon;
        use crate::progression::Race;
        let mut m = memory(false);
        m.policy.closed = true;
        m.combat.race = Some(Race::Amarr);
        let n = need(&m, 1_000.0);
        assert_eq!(n.weapon, Weapon::Laser);
        assert!(!n.drones && !n.strict);
        m.combat.race = Some(Race::Gallente);
        let n = need(&m, 1_000.0);
        assert_eq!(n.weapon, Weapon::Hybrid);
        assert!(n.drones, "the drone race looks for a bay");
    }
}
