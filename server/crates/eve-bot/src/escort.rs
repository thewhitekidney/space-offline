//! The escort's rules: a second role for [`crate::brain`] (P8c).
//!
//! **What an escort is.** A bot in a combat ship -- a Rifter with two
//! autocannons and EMP rounds, given to itself with GM in setup only, as the
//! miner gives itself a Merlin -- that sits on its fleet boss's wing and kills
//! the NPCs that threaten the fleet: one whose gun is on the boss or on the
//! escort, or any NPC near the boss ("in the belt"). It locks, orbits at its
//! guns' range, fires every gun, loots the wreck if its hold allows, and goes
//! back to the boss. With no boss it rats: it goes to the belt (the rocks)
//! and kills what comes. It docks to repair and to re-arm. Docking no
//! longer mends a hull for free (GAP-REPAIR): the undock pays the station's
//! repair shop first (`Conn::repair_ship`, `driver::Act::Undock`), out of the
//! bot's own wallet.
//!
//! **Pure, like the rest of the brain**: [`docked`] and [`in_space`] read
//! the [`Observation`] and the [`Memory`] and return acts; the driver does
//! the I/O. The world is in the observation; the memory holds intentions
//! only -- the NPC being fought, the wrecks already opened, the guns told to
//! stop, a reload's lockout.
//!
//! **The rules, in priority order** -- after the shared ones (transit,
//! invite, greeting, bind; `brain.rs`). Everything here is **[A] policy**,
//! and no number from here appears in a test as an expected value.
//!
//! Docked:
//!
//! 1. Three failures of a kind -> reconnect.
//! 2. Anything in the hold that is not our rounds (loot) -> unload it.
//!    Loot in the hangar, once a visit and at most once per
//!    `Policy::sell_every` -> [`Act::SellLoot`] ([`sell_loot`], E10b: into
//!    NPC bids, listed, or refined, by the pure rule [`fate`]; rounds up to
//!    [`AMMO_KEEP`] and [`WANT_GUNS`] spare guns are kept).
//! 3. Not in a Rifter -> board the one in the hangar, or set one up / say
//!    `MISSING`; but a ship that carries a gun (the rookie ship's civilian
//!    gun, E10b) flies as it is while setup is not due.
//! 4. Fewer than [`WANT_GUNS`] guns fitted -> fit one from the hangar (or set
//!    up / `MISSING` when none is fitted at all); offline -> online.
//! 5. A gun short of a full load and rounds in the hangar -> load them.
//! 6. Fewer than [`SPARE_ROUNDS`] in the hold -> stock it from the hangar.
//! 7. Dwell, then undock.
//!
//! In space:
//!
//! 1. Recovery as the miner's: travel failing -> reconnect; anything else
//!    failing -> home.
//! 2. No gun -> home. Armour or hull hurt -> home to repair. More incoming
//!    fire than [`RETREAT_AT`] by the hostiles' profiles -> home (CP46).
//!    Every gun empty and none in the hold -> home to re-arm (a gun that
//!    takes no rounds, a civilian gun, is never empty: [`takes_rounds`]).
//!    An empty gun
//!    and rounds in the hold -> reload it (and wait out the gun's own
//!    `reloadTime`).
//! 3. Going home: cease fire, then warp, approach, dock (the miner's way).
//! 4. A hostile: keep fighting the one already engaged; else one shooting
//!    the boss or us; else any NPC within [`GUARD_RADIUS`] of the boss (of
//!    us, with no boss). Among those, by [`priority`]: a tackler first, a
//!    hauler last, then the fewest hit points, then the nearest the boss
//!    (CP46; with no profiles, just the nearest). More than a warp
//!    away -> warp to the boss if he is far too, else to the NPC. Then orbit
//!    it at the guns' range, lock it inside the hull's lock range, and fire
//!    every gun that is idle and loaded.
//! 5. Nothing hostile: cease fire; loot the nearest unopened NPC wreck near
//!    the boss (approach it first) unless the hold has said it is full; then
//!    back to the boss -- warp to him if far, else keep at [`ESCORT_RANGE`];
//!    with no boss, warp to the belt.
//!
//! **NPC profiles** (CP46, C6 §5.2, N7): with `eve-bots --content DIR` the
//! policy carries [`crate::profile::Profiles`], the client's own data about
//! each NPC type; without it the escort plays as before. C6 §5.2's second
//! use, the ammo by the faction's resists, is not here: the driver loads
//! [`AMMO`] by name (`docs/lanes/cp46.md`).
//!
//! **What it says** (only with a chat connection, [`Observation::chat`]):
//! "Engaging X" when it picks a target, "X down" when one goes, and why it
//! is heading home. The words are [A].

use crate::brain::{
    self, Act, ActKind, Ball, GIVE_UP_AFTER, GunInfo, GunKind, Homing, Memory, Observation,
    ParkView, RANGE_MARGIN, SHORT_WAIT,
};
use crate::pricing::Quote;
use crate::profile::Profiles;
use crate::verbs::{Book, BookOrder, SellLine};
use crate::{CATEGORY_CHARGE, Conn, Error, LOOT_RANGE, MIN_WARP_DISTANCE, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// A Rifter **[V-data]**: the hull the escort flies.
pub const RIFTER: i64 = 587;
/// 125mm Gatling AutoCannon I **[V-data]**: its guns.
pub const GUN: i64 = 484;
/// EMP S **[V-data]**: its rounds (group 83, size 1 -- fits the 125mm).
pub const AMMO: i64 = 185;
/// Small Projectile Turret **[V-data]** (`docs/lanes/modules.md`, P2.3's
/// human check): the skill the guns ask for.
pub const SKILL_GUNNERY: i64 = 3302;

/// Guns to fit. A Rifter has three high slots and two turret hardpoints
/// [V-data]; two guns and a free slot. **[A]**
pub const WANT_GUNS: usize = 2;
/// Rounds to carry in the hold for a reload in space. **[A]**
pub const SPARE_ROUNDS: i64 = 400;
/// How many rounds GM setup creates: two loads, the spare, and some over.
/// **[A]**
pub const SETUP_ROUNDS: i64 = 1_000;
/// An NPC this close to the boss (or to us, with no boss) is a threat,
/// metres, surface to surface: "in the belt". **[A]**
pub const GUARD_RADIUS: f64 = 60_000.0;
/// How far from the boss the escort keeps when nothing is happening.
/// **[A]**
pub const ESCORT_RANGE: f64 = 2_500.0;
/// Where to land when warping to the boss, to an NPC or to the belt.
/// **[A]**
pub const WARP_STANDOFF: f64 = 10_000.0;
/// Approach a wreck to this range before looting it. **[A]**
pub const WRECK_APPROACH: f64 = 500.0;
/// Dock to repair below this armour fraction... **[A]**
pub const REPAIR_ARMOUR: f64 = 0.5;
/// ...or below this hull fraction. **[A]**
pub const REPAIR_HULL: f64 = 0.95;
/// Go home when the hostiles within [`GUARD_RADIUS`] of us would deal more
/// than this, damage per second, raw, by their profiles (CP46, C6 §5.2).
/// **[A]**: a Rifter's few hundred hit points a layer against a belt
/// frigate's tens a second, so a handful of frigates is a fight and a
/// cruiser squad is not. Falsified by an escort that leaves a belt a person
/// in a Rifter would hold, or that loses its ship without leaving.
pub const RETREAT_AT: f64 = 50.0;
/// How long to wait between combat turns; the driver wakes on the first
/// push, so this is a ceiling. **[A]**
pub const COMBAT_WAIT: Duration = Duration::from_secs(1);
/// How long to sit beside the boss, or in a quiet belt. **[A]**
pub const PATROL_WAIT: Duration = Duration::from_secs(5);

/// **A round the escort fires** (BOT-TUNE): its own [`AMMO`], or any other
/// charge of the projectile round group ([`crate::ammo::GROUP_ROUNDS`]) --
/// what it bought when the market had no EMP S, or a builder's starter
/// round. The size is not on the row; the buyer chose it by the gun
/// ([`crate::ammo::rounds_for`]).
pub fn is_round(r: &crate::ItemRow) -> bool {
    r.category_id == CATEGORY_CHARGE
        && (r.type_id == AMMO || r.group_id == crate::ammo::GROUP_ROUNDS)
}

/// **The rounds of this bot's weapon system** in a listing (PROG-PVE2): the
/// projectile rounds for a projectile bot (the same as [`rounds`]), hybrid
/// charges, crystals or missiles for the others. A round of another system
/// is loot to it.
fn own_rounds(
    rows: &[crate::ItemRow],
    weapon: crate::hull_choice::Weapon,
) -> impl Iterator<Item = &crate::ItemRow> {
    rows.iter()
        .filter(move |r| crate::fit_plan::is_round_of(weapon, r))
}

/// **A gun that takes the escort's rounds** (E10b): one whose capacity
/// holds some ([`GunInfo::capacity`], `modules::rounds_capacity`). A gun
/// that holds none -- a civilian gun, which fires with no charge -- is
/// never empty and never reloaded. **[A]**: falsified by a gun the server
/// refuses with `NoCharges` while its capacity reads 0 (an unknown type
/// with no bundle reads 0 too).
pub fn takes_rounds(g: &GunInfo) -> bool {
    g.capacity > 0
}

/// A gun that takes rounds and has none loaded.
fn empty_gun(g: &GunInfo) -> bool {
    takes_rounds(g) && g.rounds == Some(0)
}

/// **Every gun is dry**: each one takes rounds and has none. A civilian gun
/// among them keeps the ship armed.
pub fn dry(guns: &[GunInfo]) -> bool {
    // BP-EWAR-AIM: a web or a scrambler takes no rounds and keeps no ship armed.
    let weapons = |g: &&GunInfo| g.kind == GunKind::Weapon;
    if guns.iter().any(|g| g.kind == GunKind::Weapon) {
        return guns.iter().filter(weapons).all(empty_gun);
    }
    guns.iter().all(empty_gun)
}

/// Docked: rules 1-7 of the header.
pub fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    crate::drone_use::docked(m, o, docked_core)
}

/// **Setup for a fighter** (ship-pool): the open mode's GM-or-missing
/// ([`brain::gm_or_missing`]); the closed economy never calls GM, it
/// acquires (the hull through the chooser, [`crate::hull_choice`]) or says
/// `MISSING`.
pub(crate) fn fighter_setup(
    m: &mut Memory,
    ship: bool,
    module: bool,
    ammo: bool,
    what: &'static str,
) -> Vec<Act> {
    if !m.policy.closed {
        return brain::gm_or_missing(m, ship, module, ammo, what);
    }
    match brain::acquire_if_due(m, ship, module, ammo) {
        Some(acts) => crate::drone_use::rewrite_hull(m, acts),
        None => vec![Act::Missing(what), Act::Wait(brain::MISSING_WAIT)],
    }
}

/// [`docked`]'s rules, without the drones ([`crate::drone_use`]).
fn docked_core(m: &mut Memory, o: &Observation) -> Vec<Act> {
    // Whatever was meant in space is over.
    m.claim = None;
    m.homing = None;
    m.stop_sent = false;
    m.target = None;
    m.ceasing.clear();
    m.reload_until = None;
    if let Some(stop) = &o.last_stop {
        m.stop_handled = m.stop_handled.max(stop.seq);
    }
    // PROG-PVE2: the bot's race follows the hull it flies or holds, and the
    // weapon follows the race; a Tech III hull steers its stance in a fight.
    if m.policy.closed {
        m.combat.race =
            Some(crate::progression::rung(o.ship.and_then(|s| s.type_id), &o.hangar, m.seed).race);
    }
    m.combat.t3_hull = crate::t3::is_flying(&o.hangar, o.ship.map(|s| s.ship_id));
    if !m.combat.t3_hull {
        m.combat.t3_weapon = None;
    }
    let weapon = crate::drone_use::weapon(m);

    // 1. Recovery, docked.
    if o.failures.values().any(|&n| n >= GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }

    // 2. Loot in the hold.
    let loot: Vec<(i64, i64)> = o
        .cargo
        .iter()
        .filter(|r| !crate::fit_plan::is_round_of(weapon, r))
        // Spare probes are the camper's reload, not loot.
        .filter(|r| !crate::camp::is_probe(r))
        .map(|r| (r.item_id, r.quantity))
        .collect();
    if !loot.is_empty() && o.failed(ActKind::Unload) == 0 {
        return vec![Act::Unload(loot)];
    }

    // 2b. Loot in the hangar: sell it, once a visit and at most once per
    // `sell_every` (E10b, the miner's pace).
    let carried: i64 = own_rounds(&o.cargo, weapon).map(|r| r.quantity).sum();
    let sale_due = m.sold_at.is_none_or(|t| o.now >= t + m.policy.sell_every);
    if !m.sold && sale_due && !lots_keeping(&o.hangar, carried, &plan_weapons(m, o)).is_empty() {
        m.sold = true;
        m.sold_at = Some(o.now);
        return vec![Act::SellLoot];
    }

    let active = o.ship.map(|s| s.ship_id);
    let flying = o.ship.and_then(|s| s.type_id);
    let hangar_rifter = o
        .hangar
        .iter()
        .find(|r| r.type_id == crate::drone_use::hull(m) && Some(r.item_id) != active)
        .map(|r| r.item_id);
    // PROG-PVE: the guns and rounds are the flown class's (a Rifter's are
    // the old ones, [`crate::fit_plan::SMALL`]).
    let fit = if m.policy.closed {
        crate::fit_plan::fit_for_bot(flying, weapon)
    } else {
        crate::fit_plan::SMALL
    };
    let hangar_gun = o
        .hangar
        .iter()
        .find(|r| r.type_id == fit.gun)
        .map(|r| r.item_id);
    let hangar_rounds: Vec<&crate::ItemRow> = o
        .hangar
        .iter()
        .filter(|r| crate::fit_plan::suits(&fit, r))
        .collect();
    let no_rounds =
        hangar_rounds.is_empty() && !o.cargo.iter().any(|r| crate::fit_plan::suits(&fit, r));

    // 3. A hull. With none to be had, a ship that already carries a gun
    // (the rookie ship's civilian gun, E10b) is flown as it is; setup is
    // still asked for whenever it is due.
    let mut makeshift = false;
    if m.policy.closed {
        // The hull pool (ship-pool): whatever combat hull the hangar holds,
        // else one the market sells, else the rookie ship.
        if o.failed(ActKind::Board) > 0
            && let Some(t) = m.combat.boarding.take()
        {
            m.combat.refused.push(t);
        }
        let need = crate::drone_use::need(m, 0.0);
        // T3-H: a Tech III hull the pilot assembled is kept, not swapped for a
        // ranked one.
        let t3_flying = crate::t3::is_flying(&o.hangar, active);
        // A dev preset's role hull (a Sabre) is not in the chooser's ladder
        // and a doctrine would not name it: board the one the preset bought.
        let dev_hull = m
            .policy
            .dev
            .as_ref()
            .and_then(|d| d.hull)
            .filter(|h| crate::hull_choice::ROLE_HULLS.contains(h));
        let dev_board = dev_hull
            .filter(|h| flying != Some(*h) && !m.combat.refused.contains(h))
            .and_then(|h| {
                o.hangar
                    .iter()
                    .find(|r| r.type_id == h && Some(r.item_id) != active)
            })
            .map(|r| (r.item_id, r.type_id));
        if let Some((item, type_id)) = dev_board
            && !t3_flying
        {
            m.combat.boarding = Some(type_id);
            return vec![Act::Board(item)];
        }
        let board = if t3_flying || dev_hull.is_some() && flying == dev_hull {
            None
        } else {
            crate::hull_choice::board_choice(
                crate::hull_choice::HULLS,
                &o.hangar,
                active,
                flying,
                &need,
            )
        };
        if let Some(item) = board {
            m.combat.boarding = o
                .hangar
                .iter()
                .find(|r| r.item_id == item)
                .map(|r| r.type_id);
            return vec![Act::Board(item)];
        }
        if !t3_flying
            && !flying.is_some_and(|f| {
                (crate::hull_choice::in_table(crate::hull_choice::HULLS, f)
                    || m.combat.doctrine.as_ref().is_some_and(|d| d.hull == f)
                    || dev_hull == Some(f))
                    && !m.combat.refused.contains(&f)
            })
        {
            let acts = fighter_setup(
                m,
                true,
                hangar_gun.is_none(),
                no_rounds,
                "a combat hull in the hangar",
            );
            let setup = acts
                .iter()
                .any(|a| matches!(a, Act::Acquire { .. } | Act::Drone(_)));
            if setup || o.guns.is_empty() {
                return acts;
            }
            makeshift = true;
        }
    } else if flying != Some(crate::drone_use::hull(m)) {
        if let Some(rifter) = hangar_rifter {
            return vec![Act::Board(rifter)];
        }
        let acts = brain::gm_or_missing(
            m,
            true,
            hangar_gun.is_none(),
            no_rounds,
            "a Rifter in the hangar",
        );
        let setup = acts
            .iter()
            .any(|a| matches!(a, Act::GmSetup { .. } | Act::Acquire { .. }));
        if setup || o.guns.is_empty() {
            return acts;
        }
        makeshift = true;
    }

    // 4. Guns. A makeshift hull tries a hangar gun once; a refusal (no
    // hardpoint, too little CPU) leaves it with what it has.
    if o.guns.len() < fit.guns {
        // A bigger class's guns are CPU and power grid bound: one refusal
        // and the bot keeps what it fitted (PROG-PVE).
        if let Some(gun) = hangar_gun
            && (!(makeshift || fit.size != crate::fit_plan::Size::Small)
                || o.failed(ActKind::FitAuto) == 0)
        {
            // FAST-KIT: all the missing guns in one MultiAdd; a stack
            // splits one unit per id (inventory.rs `multi_add`).
            let need = fit.guns - o.guns.len();
            let have = o
                .hangar
                .iter()
                .find(|r| r.item_id == gun)
                .map_or(1, |r| r.quantity.max(1) as usize);
            let n = need.min(have);
            return vec![if n > 1 {
                Act::FitAutoN { item: gun, n }
            } else {
                Act::FitAuto(gun)
            }];
        }
        if let Some(acts) = arm_due(m, &fit, flying) {
            return acts;
        }
        if o.guns.is_empty() {
            if fit.size != crate::fit_plan::Size::Small {
                // The bigger class's guns are bought by the arm act above,
                // at its own pace: until then the ship stays docked.
                return vec![
                    Act::Missing("the guns of the hull's class in the hangar"),
                    Act::Wait(brain::MISSING_WAIT),
                ];
            }
            return fighter_setup(m, false, true, no_rounds, gun_text(fit.weapon));
        }
    }
    let offline: Vec<i64> = o
        .guns
        .iter()
        .filter(|g| !g.online)
        .map(|g| g.item_id)
        .collect();
    match offline.len() {
        0 => {}
        1 => return vec![Act::Online(offline[0])],
        _ => return vec![Act::OnlineMany(offline)],
    }

    // 5. Rounds in the guns.
    let short: Vec<(i64, i64)> = o
        .guns
        .iter()
        .filter(|g| g.rounds.is_some_and(|r| r < g.capacity))
        .map(|g| (g.item_id, g.flag_id))
        .collect();
    if !short.is_empty() && o.failed(ActKind::LoadAmmo) == 0 {
        if let (false, Some(station)) = (hangar_rounds.is_empty(), docked_at(o)) {
            return vec![Act::LoadAmmo {
                guns: short,
                charges: hangar_rounds.iter().map(|r| r.item_id).collect(),
                location: station,
            }];
        }
        if dry(&o.guns) && no_rounds {
            if fit.size != crate::fit_plan::Size::Small {
                return arm_due(m, &fit, flying).unwrap_or_else(|| {
                    vec![
                        Act::Missing("rounds of the hull's gun size"),
                        Act::Wait(brain::MISSING_WAIT),
                    ]
                });
            }
            return fighter_setup(m, false, false, true, round_text(fit.weapon));
        }
    }

    // 6. Spare rounds in the hold, for guns that take them.
    if carried < SPARE_ROUNDS
        && o.guns.iter().any(takes_rounds)
        && o.failed(ActKind::Stock) == 0
        && let Some(stack) = hangar_rounds.first()
    {
        return vec![Act::Stock {
            item: stack.item_id,
            quantity: (SPARE_ROUNDS - carried).min(stack.quantity),
        }];
    }

    // 6b. A salvager (X-SALV): fit it, online it, or buy one.
    if let Some(acts) = crate::salvage::docked(m, o) {
        return acts;
    }

    // 6c. A bigger hull (PROG-PVE), when the skills and the wallet allow.
    if let Some(acts) = upgrade_due(m, o, flying) {
        return acts;
    }

    // 6d. The old hulls past the one spare (PROG-PVE2).
    if let Some(acts) = hull_sale_due(m, o, flying) {
        return acts;
    }

    // 6e. A richer system, as the ship has grown (PROG-PVE2).
    if let Some(acts) = relocation_due(m, o, flying) {
        return acts;
    }

    // 7. Dwell, then go.
    brain::dwell_then_undock(m, o)
}

/// What a bot of this weapon system misses with no gun in the hangar: the act
/// log's `MISSING` line, which is a feature request (`docs/lanes/bots.md`).
fn gun_text(weapon: crate::hull_choice::Weapon) -> &'static str {
    use crate::hull_choice::Weapon;
    match weapon {
        Weapon::Projectile => "a 125mm autocannon in the hangar",
        Weapon::Hybrid | Weapon::Drones => "a small hybrid gun in the hangar",
        Weapon::Laser => "a small laser in the hangar",
        Weapon::Missile => "a light missile launcher in the hangar",
    }
}

/// [`gun_text`] for the rounds.
fn round_text(weapon: crate::hull_choice::Weapon) -> &'static str {
    use crate::hull_choice::Weapon;
    match weapon {
        Weapon::Projectile => "EMP S in the hangar",
        Weapon::Hybrid | Weapon::Drones => "small hybrid charges in the hangar",
        Weapon::Laser => "small frequency crystals in the hangar",
        Weapon::Missile => "light missiles in the hangar",
    }
}

/// **Buy the flown class's guns and rounds** (PROG-PVE), closed economy, a
/// class past the small fit only, at most once per
/// [`crate::drone_use::ARM_EVERY`].
fn arm_due(m: &mut Memory, fit: &crate::fit_plan::Fit, flying: Option<i64>) -> Option<Vec<Act>> {
    if !m.policy.closed || fit.size == crate::fit_plan::Size::Small {
        return None;
    }
    let class = flying.and_then(|f| crate::hull_choice::class_of(crate::hull_choice::HULLS, f))?;
    let now = m.now?;
    if m.combat.arm_next.is_some_and(|t| now < t) {
        return None;
    }
    m.combat.arm_next = Some(now + crate::drone_use::ARM_EVERY);
    Some(vec![Act::Drone(crate::drone_use::DroneAct::Arm(class))])
}

/// **Look for a bigger hull** (PROG-PVE): a fighter that climbs
/// ([`crate::progression::climbs`]), in a table hull with a class above it,
/// armed (the guns of its class fitted), at most once per
/// [`crate::progression::UPGRADE_EVERY`].
fn upgrade_due(m: &mut Memory, o: &Observation, flying: Option<i64>) -> Option<Vec<Act>> {
    if !m.policy.closed || !crate::progression::climbs(m.policy.role) {
        return None;
    }
    let class = flying.and_then(|f| crate::hull_choice::class_of(crate::hull_choice::HULLS, f))?;
    crate::progression::next(class)?;
    if o.guns.is_empty() {
        return None;
    }
    let now = m.now?;
    if m.combat.upgrade_next.is_some_and(|t| now < t) {
        return None;
    }
    m.combat.upgrade_next = Some(now + crate::progression::UPGRADE_EVERY);
    Some(vec![Act::Drone(crate::drone_use::DroneAct::Upgrade(class))])
}

/// **Sell the old hulls** (PROG-PVE2): a fighter that climbs
/// ([`crate::progression::climbs`]) holding hulls past its spare
/// ([`crate::hull_sale::surplus`]), at most once per
/// [`crate::hull_sale::SELL_EVERY`] (the clock runs only when there was
/// something to sell).
fn hull_sale_due(m: &mut Memory, o: &Observation, flying: Option<i64>) -> Option<Vec<Act>> {
    if !m.policy.closed || !crate::progression::climbs(m.policy.role) {
        return None;
    }
    let class = flying.and_then(|f| crate::hull_choice::class_of(crate::hull_choice::HULLS, f))?;
    let now = m.now?;
    if m.combat.sale_next.is_some_and(|t| now < t) {
        return None;
    }
    let active = o.ship.map(|s| s.ship_id);
    if crate::hull_sale::surplus(&o.hangar, active, class).is_empty() {
        return None;
    }
    m.combat.sale_next = Some(now + crate::hull_sale::SELL_EVERY);
    Some(vec![Act::Drone(crate::drone_use::DroneAct::SellHulls)])
}

/// **Move to richer ground** (PROG-PVE2): a ratter or site runner (not in a
/// fleet: an escort follows its boss) whose system is safer than its class's
/// band ([`crate::relocate`]) flies to the nearest system inside it, at most
/// once per [`crate::relocate::EVERY`].
fn relocation_due(m: &mut Memory, o: &Observation, flying: Option<i64>) -> Option<Vec<Act>> {
    let role = m.policy.role;
    if !m.policy.closed
        || !(std::ptr::eq(role, &crate::roles::ESCORT)
            || std::ptr::eq(role, &crate::site_runner::ROLE))
        || o.fleet.is_some()
    {
        return None;
    }
    let map = m.policy.travel.clone()?;
    let class = crate::relocate::class_flown(flying)?;
    let system = map.system_of(docked_at(o)?)?;
    let now = m.now?;
    if m.combat.move_next.is_some_and(|t| now < t) {
        return None;
    }
    m.combat.move_next = Some(now + crate::relocate::EVERY);
    let station =
        crate::relocate::target(&map, system, class, m.policy.security_floor, m.seed, &[])?;
    Some(vec![Act::Drone(crate::drone_use::DroneAct::Move(station))])
}

fn docked_at(o: &Observation) -> Option<i64> {
    match o.place {
        brain::Place::Docked { station } => Some(station),
        _ => None,
    }
}

/// In space: rules 1-5 of the header, with the drones steered around them
/// ([`crate::drone_use`]) and, in a Tech III hull, the stance
/// ([`crate::t3::stance_steer`], PROG-PVE2).
pub fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let acts = crate::drone_use::in_space(m, o, in_space_core);
    crate::t3::stance_steer(m, o, acts)
}

/// [`in_space`]'s rules, without the drones ([`crate::drone_use`]).
fn in_space_core(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(SHORT_WAIT)];
    };
    m.dwell_until = None;
    let ego = park.ego;
    m.ceasing
        .retain(|g| o.guns.iter().any(|x| x.item_id == *g && x.active));
    if m.reload_until.is_some_and(|t| o.now >= t) {
        m.reload_until = None;
    }
    let mut said: Vec<Act> = Vec::new();

    // 1. Recovery.
    if o.failures
        .iter()
        .any(|(k, &n)| k.is_travel() && n >= GIVE_UP_AFTER)
    {
        return vec![Act::Reconnect];
    }
    if m.homing.is_none()
        && o.failures
            .iter()
            .any(|(k, &n)| !k.is_travel() && !k.is_incidental() && n >= GIVE_UP_AFTER)
    {
        m.homing = Some(Homing::Recovery);
    }

    // 2. Armed, whole, loaded.
    let neut = crate::cap::answer(m, o, park);
    // BOT-CAP: a neut rat that can be killed is the primary target.
    if let crate::cap::Answer::Kill(id, name) = &neut
        && m.target.as_ref().map(|(t, _)| *t) != Some(*id)
    {
        m.target = Some((*id, name.clone()));
    }
    let cargo_rounds: Vec<i64> = own_rounds(&o.cargo, crate::drone_use::weapon(m))
        .map(|r| r.item_id)
        .collect();
    if m.homing.is_none() {
        if o.guns.is_empty() {
            m.homing = Some(Homing::Unarmed);
        } else if o
            .health
            .is_some_and(|h| h.armour < REPAIR_ARMOUR || h.hull < REPAIR_HULL)
        {
            m.homing = Some(Homing::Repair);
            say(
                o,
                &mut said,
                "Taking a beating -- docking to repair".to_owned(),
            );
        } else if m
            .policy
            .profiles
            .as_deref()
            .is_some_and(|p| incoming_dps(p, park, &ego) > RETREAT_AT)
        {
            // Repairing is a dock and a shop visit, so the repair's way home serves. [A]
            m.homing = Some(Homing::Repair);
            say(
                o,
                &mut said,
                "Too many guns on me -- pulling back".to_owned(),
            );
        } else if let crate::cap::Answer::Flee(why) = neut {
            // BOT-CAP: drained by a neutralizer that cannot be killed.
            m.homing = Some(crate::cap::FLEE);
            m.home_why = Some((crate::cap::FLEE, why));
            say(o, &mut said, "Neuted dry -- pulling out".to_owned());
        } else if dry(&o.guns) && cargo_rounds.is_empty() {
            m.homing = Some(Homing::Rearm);
            say(
                o,
                &mut said,
                "Out of ammo -- going home to re-arm".to_owned(),
            );
        }
    }
    let empty: Vec<(i64, i64)> = o
        .guns
        .iter()
        .filter(|g| empty_gun(g) && !g.active)
        .map(|g| (g.item_id, g.flag_id))
        .collect();
    if m.homing.is_none()
        && !empty.is_empty()
        && !cargo_rounds.is_empty()
        && m.reload_until.is_none()
        && o.failed(ActKind::LoadAmmo) < GIVE_UP_AFTER
        && let Some(ship) = o.ship
    {
        let lockout = o.guns.iter().map(|g| g.reload).max().unwrap_or_default();
        m.reload_until = Some(o.now + lockout);
        return vec![Act::LoadAmmo {
            guns: empty,
            charges: cargo_rounds,
            location: ship.ship_id,
        }];
    }

    // 3. Home.
    if m.homing.is_some() {
        m.target = None;
        let cease = cease_fire(m, o, None);
        if !cease.is_empty() {
            return [said, cease].concat();
        }
        if !m.ceasing.is_empty() {
            return [said, vec![Act::Wait(SHORT_WAIT)]].concat();
        }
        return [said, brain::go_home(m, o, park)].concat();
    }

    let boss = park.boss;
    let anchor = boss.unwrap_or(ego);
    let boss_char = o.fleet.and_then(|f| f.boss);

    // 4. A hostile.
    if let Some((id, name)) = m.target.clone()
        && !park.hostiles.iter().any(|h| h.ball.id == id)
    {
        m.target = None;
        m.kills += 1;
        say(o, &mut said, format!("{name} down"));
    }
    let threat = pick_target(m, park, &ego, boss.as_ref());
    if let Some(h) = threat {
        if m.target.as_ref().map(|(id, _)| *id) != Some(h.ball.id) {
            // REACT-JITTER: a new hostile is noticed before it is engaged.
            if let Some(left) = m.thinking(crate::react::Class::Hostile, h.ball.id, o) {
                return [said, vec![Act::Wait(left.min(SHORT_WAIT))]].concat();
            }
            m.target = Some((h.ball.id, h.name.clone()));
            say(o, &mut said, format!("Engaging {}", h.name));
        }
        let gap = ego.surface_to(&h.ball);
        if !warp_lands_short(gap, WARP_STANDOFF) {
            if let (Some(char_id), Some(b)) = (boss_char, boss)
                && !warp_lands_short(ego.surface_to(&b), WARP_STANDOFF)
            {
                return [
                    said,
                    vec![Act::WarpToMember {
                        char_id,
                        range: WARP_STANDOFF,
                    }],
                ]
                .concat();
            }
            return [
                said,
                vec![Act::WarpToItem {
                    id: h.ball.id,
                    range: WARP_STANDOFF,
                }],
            ]
            .concat();
        }
        let mut acts = said;
        acts.extend(cease_fire(m, o, Some(h.ball.id)));
        // The Orbit is always asked for; a camper does not wait for the tether
        // but falls through to the lock and the fire, and `hold_position`
        // drops the Orbit on the fight path (unfiltered callers keep it).
        if park.ego_tether != Some(h.ball.id) {
            acts.push(Act::Orbit {
                id: h.ball.id,
                range: crate::counter::orbit(m, o, h.ball.id),
            });
            if !crate::camp::camping(m, o) {
                return acts;
            }
        }
        if !o.locked.contains(&h.ball.id) {
            if o.pending.contains(&h.ball.id) || gap > o.lock_range * RANGE_MARGIN {
                acts.push(Act::Wait(SHORT_WAIT));
            } else {
                acts.push(Act::Lock(h.ball.id));
            }
            return acts;
        }
        let reloading = m.reload_until.is_some();
        let ready: Vec<Act> = o
            .guns
            .iter()
            .filter(|g| g.kind == GunKind::Weapon)
            .filter(|g| g.online && !g.active && !empty_gun(g) && !reloading)
            .map(|g| Act::Fire {
                gun: g.item_id,
                target: h.ball.id,
            })
            .collect();
        // BP-EWAR-AIM: tackle and ewar modules aim by their own rules.
        let foes = crate::ewar_aim::foes(m, o, park, &h.ball);
        let ewar = crate::ewar_aim::ewar_acts(&mut m.ceasing, o, &foes, h.ball.id, &ego);
        acts.extend(ewar);
        // BP-COUNTER: a drive spooling is scrammed inside its window.
        acts.extend(crate::counter::mjd_scram(o, &m.policy.counter, &foes, &ego));
        if ready.is_empty() {
            acts.push(Act::Wait(COMBAT_WAIT));
        } else {
            acts.extend(ready);
        }
        return acts;
    }

    // 5. Quiet: cease fire, loot, back to the boss.
    let mut acts = said;
    let cease = cease_fire(m, o, None);
    if !cease.is_empty() {
        acts.extend(cease);
        return acts;
    }
    if !o.hold_full
        && let Some(wreck) = park
            .wrecks
            .iter()
            .filter(|w| !m.looted.contains(&w.id))
            .filter(|w| anchor.surface_to(w) <= GUARD_RADIUS)
            .min_by(|a, b| ego.surface_to(a).total_cmp(&ego.surface_to(b)))
    {
        let gap = ego.surface_to(wreck);
        if gap < LOOT_RANGE * RANGE_MARGIN {
            m.looted.push(wreck.id);
            acts.push(Act::Loot(wreck.id));
            return acts;
        }
        if gap <= MIN_WARP_DISTANCE && park.ego_tether == Some(wreck.id) {
            acts.push(Act::Wait(SHORT_WAIT));
            return acts;
        }
        // An approach that was not taken -- the server may not resolve a
        // wreck as a ball to follow -- is not retried: the wreck is let go.
        // [A]
        if gap <= MIN_WARP_DISTANCE && o.failed(ActKind::Follow) == 0 {
            acts.push(Act::Follow {
                id: wreck.id,
                range: WRECK_APPROACH,
            });
            return acts;
        }
        m.looted.push(wreck.id);
    }
    // Salvage what was looted (X-SALV), before going back to the boss.
    if let Some(more) = crate::salvage::in_space(m, o, park) {
        acts.extend(more);
        return acts;
    }
    if let Some(b) = boss {
        let gap = ego.surface_to(&b);
        if !warp_lands_short(gap, WARP_STANDOFF)
            && let Some(char_id) = boss_char
        {
            acts.push(Act::WarpToMember {
                char_id,
                range: WARP_STANDOFF,
            });
        } else if park.ego_tether != Some(b.id) && gap > ESCORT_RANGE {
            acts.push(Act::Follow {
                id: b.id,
                range: ESCORT_RANGE,
            });
        } else {
            acts.push(Act::Wait(PATROL_WAIT));
        }
        return acts;
    }
    // A boss off this grid: his ship is not in the park (GRID-B).
    if let Some(char_id) = boss_char {
        acts.push(Act::WarpToMember {
            char_id,
            range: WARP_STANDOFF,
        });
        return acts;
    }
    // No boss: ratting. The belt is where the rocks are -- and off a belt's
    // grid the rocks are not in the park, but the belt is (GRID-B).
    // L7b: with a job from the request board, the foreman's belt first.
    let jobbed = m.job.belt().and_then(|id| {
        park.rocks
            .iter()
            .chain(park.belts.iter())
            .find(|b| b.id == id)
    });
    let belt = jobbed.or_else(|| {
        park.rocks
            .iter()
            .chain(park.belts.iter())
            .min_by(|a, b| ego.surface_to(a).total_cmp(&ego.surface_to(b)))
    });
    match belt {
        Some(rock) if !warp_lands_short(ego.surface_to(rock), WARP_STANDOFF) => {
            acts.push(Act::WarpToItem {
                id: rock.id,
                range: WARP_STANDOFF,
            })
        }
        _ => acts.push(Act::Wait(PATROL_WAIT)),
    }
    acts
}

/// **Whether a warp from `gap` (surface to surface) with a standoff of
/// `range` would land inside the minimum warp distance** (WARP-MIN). The
/// server measures the landing point, `range` short of the target, and
/// refuses at or under [`MIN_WARP_DISTANCE`] (`eve_server::space_state::
/// MIN_WARP_DISTANCE`, the same 150 km); the client's menu offers no warp
/// below it either.
pub fn warp_lands_short(gap: f64, range: f64) -> bool {
    gap - range.max(0.0) <= MIN_WARP_DISTANCE
}

/// **Warp to `ball` if the warp would land beyond the minimum, else
/// approach it** to `range`. The one decision every brain site that picks
/// a ball to warp to shares.
pub fn warp_or_approach(ego: &Ball, ball: &Ball, range: f64) -> Act {
    if warp_lands_short(ego.surface_to(ball), range) {
        Act::Follow {
            id: ball.id,
            range: range.max(WRECK_APPROACH),
        }
    } else {
        Act::WarpToItem { id: ball.id, range }
    }
}

/// A line, when there is a chat to say it in.
fn say(o: &Observation, said: &mut Vec<Act>, line: String) {
    if o.chat {
        said.push(Act::Say(line));
    }
}

/// The damage per second the hostiles within [`GUARD_RADIUS`] of `ego`
/// would deal, by their profiles; a type with no profile counts nothing.
/// Raw: every shot lands, no resists. **[A]**
pub fn incoming_dps(profiles: &Profiles, park: &ParkView, ego: &Ball) -> f64 {
    park.hostiles
        .iter()
        .filter(|h| ego.surface_to(&h.ball) <= GUARD_RADIUS)
        .filter_map(|h| profiles.get(h.type_id))
        .map(|p| p.dps)
        .sum()
}

/// C6 §5.2's order among rule 4's candidates, lowest first: a tackler (a
/// scram keeps the fleet from leaving), then anything else, then a hauler;
/// then the fewest hit points (the least time to kill, our guns being the
/// same for each). A type with no profile ranks with "anything else" and
/// after every known hit point count. **[A]**
pub fn priority(profiles: Option<&Profiles>, h: &brain::Hostile) -> (u8, f64) {
    match profiles.and_then(|p| p.get(h.type_id)) {
        Some(p) if p.is_tackler() => (0, p.hp),
        Some(p) if p.is_hauler() => (2, p.hp),
        Some(p) => (1, p.hp),
        None => (1, f64::INFINITY),
    }
}

/// Rule 4's choice: the NPC already engaged while it lives; else one
/// shooting the boss or us; else an NPC within [`GUARD_RADIUS`] of the boss
/// (of us, alone). Within each, the first by [`priority`], then the nearest
/// the boss.
pub fn pick_target<'a>(
    m: &Memory,
    park: &'a ParkView,
    ego: &Ball,
    boss: Option<&Ball>,
) -> Option<&'a brain::Hostile> {
    if let Some((id, _)) = &m.target
        && let Some(h) = park.hostiles.iter().find(|h| h.ball.id == *id)
    {
        return Some(h);
    }
    let anchor = boss.unwrap_or(ego);
    let profiles = m.policy.profiles.as_deref();
    let near = |a: &&brain::Hostile, b: &&brain::Hostile| {
        let (ra, ha) = priority(profiles, a);
        let (rb, hb) = priority(profiles, b);
        ra.cmp(&rb)
            .then(ha.total_cmp(&hb))
            .then(
                anchor
                    .surface_to(&a.ball)
                    .total_cmp(&anchor.surface_to(&b.ball)),
            )
            .then(a.ball.id.cmp(&b.ball.id))
    };
    let friends = [Some(ego.id), boss.map(|b| b.id)];
    park.hostiles
        .iter()
        .filter(|h| h.shooting.is_some() && friends.contains(&h.shooting))
        .min_by(near)
        .or_else(|| {
            park.hostiles
                .iter()
                .filter(|h| anchor.surface_to(&h.ball) <= GUARD_RADIUS)
                .min_by(near)
        })
}

/// Stop every running gun not on `keep`, once each.
fn cease_fire(m: &mut Memory, o: &Observation, keep: Option<i64>) -> Vec<Act> {
    let mut acts = Vec::new();
    for g in &o.guns {
        // BP-EWAR-AIM: a running tackle or ewar module is aimed by
        // [`crate::ewar_aim`] while a fight goes on; it stops with the fight.
        if g.kind != GunKind::Weapon && keep.is_some() {
            continue;
        }
        if g.active && (keep.is_none() || g.target != keep) && !m.ceasing.contains(&g.item_id) {
            m.ceasing.push(g.item_id);
            acts.push(Act::CeaseFire(g.item_id));
        }
    }
    acts
}

/// The range to orbit a target at: the shortest of the guns' fighting
/// ranges, so every gun reaches. **[A]**
pub fn fight_range(guns: &[GunInfo]) -> f64 {
    // BP-EWAR-AIM: only a weapon sets the orbit; a ship with none keeps the
    // old rule (every module), so an ewar-only hull orbits as before.
    let has_weapon = guns.iter().any(|g| g.kind == GunKind::Weapon);
    guns.iter()
        .filter(|g| !has_weapon || g.kind == GunKind::Weapon)
        .map(|g| g.range)
        .filter(|r| *r > 0.0)
        .fold(None, |acc: Option<f64>, r| {
            Some(acc.map_or(r, |a| a.min(r)))
        })
        .unwrap_or(ESCORT_RANGE)
}

// ---------------------------------------------------------------------------
// The loot sale (E10b, R-ECON §2.2)
// ---------------------------------------------------------------------------

/// Rounds of [`AMMO`] the ratter keeps, hangar and hold together: what
/// setup gives it, two loads, the spare and some over. Rounds past this are
/// loot like any other. **[A]**: falsified by a ratter that docks to re-arm
/// with rounds on the market it sold itself.
pub const AMMO_KEEP: i64 = SETUP_ROUNDS;
/// A player's bid under this share of `P_ref` is not sold into, and an ask
/// is never listed under it: the bot price clamp's low end
/// ([`crate::pricing::BOT_PRICE_MIN`], E10).
pub const SELL_FLOOR: f64 = crate::pricing::BOT_PRICE_MIN;
/// A stack worth less than this at its ask is not listed (the broker fee's
/// 100 ISK minimum would eat it): it is refined. **[A]**: falsified by
/// hangars that fill with cheap loot nobody refines or lists.
pub const LIST_MIN_VALUE: f64 = 10_000.0;
/// Open orders past which nothing more is listed: the order cap with no
/// trade skill is 5 (E5), one left for whatever else the pilot does.
/// **[A]**
pub const ORDER_ROOM: usize = 4;
/// How long a listed ask rests, days. **[A]**
pub const LIST_DAYS: i64 = 3;
/// `expectedBrokersFee`: the server computes the fee and does not read this
/// (E5).
pub const LIST_BROKER_FEE: f64 = 0.0;

/// **Loot**, as the ratter sees its hangar: a packaged stack of a module, a
/// charge (`econ_exchange::LOOT_CATEGORIES`) or a tag
/// (`econ_exchange::TAG_GROUPS`) -- never the hull it flies. [A]
pub fn is_loot(r: &crate::ItemRow) -> bool {
    !r.singleton
        && !crate::hull_choice::is_combat_hull(r.type_id)
        // A logi's remote repairer is its kit, not loot (K-LOGI).
        && !crate::logi::is_remote(r)
        // A hunter's PvP kit (smartbomb, drive, tackle, sphere launcher and probe)
        // is fitted after the sale at the same dock, so it is not loot.
        && !is_pvp_kit(r)
        && (eve_server::econ_exchange::LOOT_CATEGORIES.contains(&r.category_id)
            || eve_server::econ_exchange::TAG_GROUPS.contains(&r.group_id)
            || r.group_id == crate::salvage::GROUP_SALVAGE)
}

/// A hunter's PvP kit: the stock [`crate::pvp_kit::stock_to_fit`] and
/// `hunt::setup` fit, which the sale must not take.
pub fn is_pvp_kit(r: &crate::ItemRow) -> bool {
    crate::pvp_kit::STOCK_TYPES.contains(&r.type_id)
        || crate::camp::is_launcher(r)
        || crate::camp::is_probe(r)
        || crate::hunt::is_tackle(r.type_id)
}

/// One stack, or the part of it past what the ratter keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lot {
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    /// The whole stack: only a whole stack can be refined.
    pub whole: bool,
}

/// The key of the one round allowance in [`lots_for`]'s keep table.
const OWN_ROUNDS: i64 = 0;

/// **What is for sale**: every loot stack in `hangar`, less what the ratter
/// keeps -- [`AMMO_KEEP`] rounds counting the `carried` ones in its hold,
/// and [`WANT_GUNS`] spare guns.
pub fn lots(hangar: &[crate::ItemRow], carried: i64) -> Vec<Lot> {
    lots_for(hangar, carried, crate::hull_choice::BOT_WEAPON)
}

/// [`lots`] for a bot firing `weapon` (PROG-PVE2): it keeps that system's
/// guns and shares the one round allowance among that system's rounds; the
/// others' are loot.
pub fn lots_for(
    hangar: &[crate::ItemRow],
    carried: i64,
    weapon: crate::hull_choice::Weapon,
) -> Vec<Lot> {
    lots_keeping(hangar, carried, &[weapon])
}

/// **The weapon systems a bot's fit plan needs**: the one it fires now
/// ([`crate::drone_use::weapon`]), the one of the hull it is about to fly
/// ([`crate::progression::weapon_for`]; a hull bought and not yet boarded
/// can be of another race) and the one its seed draws (the race it falls
/// back to with no table hull). The loot sale keeps the guns of all of them:
/// a bot whose race flips must not have sold the guns of the plan it
/// is about to follow (a ratter refined its own guns, then logged `MISSING`
/// 197 times).
pub fn plan_weapons(
    m: &crate::brain::Memory,
    o: &crate::brain::Observation,
) -> Vec<crate::hull_choice::Weapon> {
    let flying = o.ship.and_then(|s| s.type_id);
    let to_fly = crate::progression::hull_to_fly(flying, &o.hangar);
    plan_weapons_of(crate::drone_use::weapon(m), to_fly, m.seed)
}

/// [`plan_weapons`] from its parts (pure).
pub fn plan_weapons_of(
    current: crate::hull_choice::Weapon,
    to_fly: Option<i64>,
    seed: u64,
) -> Vec<crate::hull_choice::Weapon> {
    let mut out = vec![current];
    for w in [
        crate::progression::weapon_for(to_fly, seed),
        crate::progression::weapon_of(crate::progression::Race::draw(seed), seed),
    ] {
        // Drones and hybrids fit the same guns: one entry per table.
        if !out.iter().any(|k| same_system(*k, w)) {
            out.push(w);
        }
    }
    out
}

fn same_system(a: crate::hull_choice::Weapon, b: crate::hull_choice::Weapon) -> bool {
    use crate::hull_choice::Weapon::{Drones, Hybrid};
    a == b || matches!((a, b), (Hybrid, Drones) | (Drones, Hybrid))
}

/// **Every weapon system**, for a sale that does not know the bot's plan
/// (the contract offer, [`crate::contracts::stock`]): it keeps the guns and
/// rounds of all of them.
pub const ALL_WEAPONS: [crate::hull_choice::Weapon; 4] = [
    crate::hull_choice::Weapon::Projectile,
    crate::hull_choice::Weapon::Hybrid,
    crate::hull_choice::Weapon::Laser,
    crate::hull_choice::Weapon::Missile,
];

/// [`lots_for`] for a bot whose fit plan needs several weapon systems: the
/// guns of each stay (per [`crate::fit_plan::keep_guns`]), and each system
/// has its own round allowance, the first one's less the `carried` rounds.
pub fn lots_keeping(
    hangar: &[crate::ItemRow],
    carried: i64,
    weapons: &[crate::hull_choice::Weapon],
) -> Vec<Lot> {
    let mut keep: BTreeMap<i64, i64> = BTreeMap::new();
    // The round allowances sit under keys no type has (0, -1, ...): a round
    // of another system is its own stack, loot, even when it is EMP S.
    // Each size of a system has its own allowance, so the Rifter's EMP S does
    // not make the Hurricane's EMP M a lot. The carried rounds come off the
    // first system's Small one only: over-keeping is harmless, over-selling
    // is the bug.
    use crate::fit_plan::Size;
    let key_of = |i: usize, z: Size| OWN_ROUNDS - (i as i64 * 3 + z as i64);
    for (i, w) in weapons.iter().enumerate() {
        for z in [Size::Small, Size::Medium, Size::Large] {
            let carried = if i == 0 && z == Size::Small {
                carried
            } else {
                0
            };
            keep.insert(key_of(i, z), (AMMO_KEEP - carried).max(0));
        }
        for (gun, n) in crate::fit_plan::keep_guns(*w) {
            let e = keep.entry(gun).or_insert(0);
            *e = (*e).max(n as i64);
        }
    }
    let mut out = Vec::new();
    for r in hangar.iter().filter(|r| is_loot(r)) {
        // Every kind of round of a kept system shares that system's allowance.
        let key = weapons
            .iter()
            .position(|w| crate::fit_plan::is_round_of(*w, r))
            .map_or(r.type_id, |i| {
                key_of(i, crate::fit_plan::round_size(weapons[i], r))
            });
        let left = keep.entry(key).or_insert(0);
        let kept = (*left).min(r.quantity);
        *left -= kept;
        if r.quantity > kept {
            out.push(Lot {
                item_id: r.item_id,
                type_id: r.type_id,
                quantity: r.quantity - kept,
                // Salvage is listed or sold into a bid, never refined: it
                // feeds the builders' rigs (X-SALV).
                whole: kept == 0 && r.group_id != crate::salvage::GROUP_SALVAGE,
            });
        }
    }
    out
}

/// What becomes of one [`Lot`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fate {
    /// Sold now into a bid at this station, at this price or better.
    Bid(f64),
    /// Listed here as an ask at this price.
    List(f64),
    /// Refined at this station.
    Reprocess,
    /// Left in the hangar.
    Keep,
}

fn cents(x: f64) -> f64 {
    (x * 100.0).floor() / 100.0
}

/// **The rule, pure** (E10b), in order:
/// 1. An NPC bid at this station (loot-only, E5) -- sell into the best bid
///    here. A player's bid here at [`SELL_FLOOR`] × `P_ref` or better is
///    sold into the same way.
/// 2. No `P_ref`: nothing tells what it is worth (a starter type has none
///    to sell, E5), so keep it.
/// 3. Already listed here: keep it (one ask per type and station).
/// 4. Room for an order, and worth [`LIST_MIN_VALUE`] at
///    `min(the seller's ask, the cheapest ask here)` -- the ask is
///    [`crate::pricing::ask`] on `P_mkt` with no cost (E10) -- not under the
///    floor: list it.
/// 5. Else refine a whole stack, and keep a part one.
pub fn fate(
    lot: &Lot,
    book: &Book,
    station: i64,
    quote: Option<Quote>,
    listed_here: bool,
    room: bool,
) -> Fate {
    let p_ref = quote.map(|q| q.p_ref);
    let here = |o: &&BookOrder| o.station_id == station && o.vol_remaining > 0;
    if let Some(best) = book
        .bids
        .iter()
        .filter(here)
        .max_by(|a, b| a.price.total_cmp(&b.price))
    {
        let npc = book.bids.iter().filter(here).any(BookOrder::is_npc);
        if npc || p_ref.is_some_and(|p| best.price >= p * SELL_FLOOR) {
            return Fate::Bid(best.price);
        }
    }
    let Some((p, ask)) = quote
        .filter(|q| q.p_ref > 0.0)
        .and_then(|q| Some((q.p_ref, crate::pricing::ask(&q)?)))
    else {
        return Fate::Keep;
    };
    if listed_here {
        return Fate::Keep;
    }
    let cheapest = book
        .asks
        .iter()
        .filter(here)
        .map(|o| o.price)
        .min_by(f64::total_cmp);
    let price = cheapest.map_or(ask, |a| cents(a).min(ask));
    if room && price >= p * SELL_FLOOR && price * lot.quantity as f64 >= LIST_MIN_VALUE {
        return Fate::List(price);
    }
    if lot.whole {
        Fate::Reprocess
    } else {
        Fate::Keep
    }
}

/// What one sale does, before it does it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Into bids, in one Sell-window call.
    pub bids: Vec<SellLine>,
    /// Asks, one call each.
    pub lists: Vec<SellLine>,
    /// Item ids to refine, in one refinery call.
    pub reprocess: Vec<i64>,
    /// Type ids left in the hangar.
    pub kept: Vec<i64>,
}

/// **[`fate`] over every lot**: `books` by type, `prices` the [`Quote`]s
/// (`P_ref` and `P_mkt`),
/// `mine` the pilot's open orders (the room, and what is listed here
/// already; each listing takes one).
pub fn plan(
    lots: &[Lot],
    books: &BTreeMap<i64, Book>,
    prices: &BTreeMap<i64, Quote>,
    station: i64,
    mine: &[BookOrder],
) -> Plan {
    let mut open = mine.len();
    let mut listed: BTreeSet<i64> = mine
        .iter()
        .filter(|o| !o.bid && o.station_id == station)
        .map(|o| o.type_id)
        .collect();
    let empty = Book::default();
    let mut out = Plan::default();
    for lot in lots {
        let book = books.get(&lot.type_id).unwrap_or(&empty);
        let line = |price| SellLine {
            station_id: station,
            type_id: lot.type_id,
            item_id: lot.item_id,
            price,
            quantity: lot.quantity,
        };
        match fate(
            lot,
            book,
            station,
            prices.get(&lot.type_id).copied(),
            listed.contains(&lot.type_id),
            open < ORDER_ROOM,
        ) {
            Fate::Bid(price) => out.bids.push(line(price)),
            Fate::List(price) => {
                out.lists.push(line(price));
                open += 1;
                listed.insert(lot.type_id);
            }
            Fate::Reprocess => out.reprocess.push(lot.item_id),
            Fate::Keep => out.kept.push(lot.type_id),
        }
    }
    out
}

/// What one [`sell_loot`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootSale {
    pub plan: Plan,
    /// The bid lines went through.
    pub sold: bool,
    /// The asks that were placed.
    pub listed: Vec<SellLine>,
    /// The refinery's outputs, `(typeID, quantity)`.
    pub refined: Vec<(i64, i64)>,
    /// Each step the server refused, and why.
    pub refused: Vec<String>,
    pub before: f64,
    pub after: f64,
}

impl LootSale {
    /// The act log's note.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "sold {} stacks into bids{}, listed {} of {}, refined {} into {} types, kept {}; wallet {:.2} -> {:.2} ISK ({:+.2})",
            self.plan.bids.len(),
            if self.sold || self.plan.bids.is_empty() {
                ""
            } else {
                " (refused)"
            },
            self.listed.len(),
            self.plan.lists.len(),
            self.plan.reprocess.len(),
            self.refined.len(),
            self.plan.kept.len(),
            self.before,
            self.after,
            self.after - self.before
        );
        for why in &self.refused {
            s.push_str("; ");
            s.push_str(why);
        }
        s
    }
}

/// A refusal is noted and the sale goes on; a broken connection is not.
fn soft<T>(r: Result<T>, what: &str, refused: &mut Vec<String>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(Error::Refused(why) | Error::NotSent(why)) => {
            refused.push(format!("{what}: {why}"));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// **[`Act::SellLoot`], docked**: read the hangar, the hold's rounds, the
/// price index, the pilot's open orders and each loot type's book (the
/// market window's own calls), [`plan`], then sell into bids in one
/// Sell-window call, list each ask, and refine what is left to refine. A
/// refusal of one step is noted and the rest go on.
pub fn sell_loot(conn: &mut Conn) -> Result<LootSale> {
    sell_loot_for(conn, crate::hull_choice::BOT_WEAPON)
}

/// [`sell_loot`] for a bot firing `weapon` (PROG-PVE2).
pub fn sell_loot_for(conn: &mut Conn, weapon: crate::hull_choice::Weapon) -> Result<LootSale> {
    sell_loot_keeping(conn, &|_| vec![weapon])
}

/// [`sell_loot`] for a bot whose plan needs `weapons` ([`plan_weapons`]);
/// the first is the one it fires.
pub fn sell_loot_keeping(
    conn: &mut Conn,
    weapons: &dyn Fn(&[crate::ItemRow]) -> Vec<crate::hull_choice::Weapon>,
) -> Result<LootSale> {
    let station = conn
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("sell_loot: not docked".to_owned()))?;
    let mut refused = Vec::new();
    let before = conn.cash_balance()?;
    let hangar = conn.hangar_items()?;
    let weapons = weapons(&hangar);
    let weapon = weapons[0];
    let carried: i64 = soft(conn.cargo(), "hold", &mut refused)?.map_or(0, |rows| {
        own_rounds(&rows, weapon).map(|r| r.quantity).sum()
    });
    refused.clear();
    let lots = lots_keeping(&hangar, carried, &weapons);
    let mut market = crate::pricing::Prices::read(conn)?;
    let mut prices = BTreeMap::new();
    for lot in &lots {
        if let Some(p_ref) = market.p_ref(lot.type_id)
            && !prices.contains_key(&lot.type_id)
        {
            let p_mkt = market.p_mkt(conn, lot.type_id)?.unwrap_or(p_ref);
            prices.insert(
                lot.type_id,
                Quote {
                    p_mkt,
                    pressure: market.pressure(conn, lot.type_id)?,
                    ..Quote::at_ref(p_ref)
                },
            );
        }
    }
    let mine = conn.my_orders()?;
    let mut books = BTreeMap::new();
    for lot in &lots {
        if let std::collections::btree_map::Entry::Vacant(e) = books.entry(lot.type_id) {
            e.insert(conn.market_orders(lot.type_id)?);
        }
    }
    let plan = plan(&lots, &books, &prices, station, &mine);
    let mut sale = LootSale {
        before,
        ..LootSale::default()
    };
    if !plan.bids.is_empty() {
        sale.sold = soft(
            conn.sell(&plan.bids, 0, LIST_BROKER_FEE),
            "sell",
            &mut refused,
        )?
        .is_some();
    }
    for line in &plan.lists {
        let placed = conn.sell(&[*line], LIST_DAYS, LIST_BROKER_FEE);
        if soft(placed, "list", &mut refused)?.is_some() {
            sale.listed.push(*line);
        }
    }
    if !plan.reprocess.is_empty() {
        sale.refined =
            soft(conn.reprocess(&plan.reprocess), "refine", &mut refused)?.unwrap_or_default();
    }
    sale.after = conn.cash_balance()?;
    sale.plan = plan;
    sale.refused = refused;
    Ok(sale)
}

#[cfg(test)]
mod tests {
    //! The escort's rules as tables: each with a minimal observation.
    //! Asserts the **kind and order** of acts and the ids they name, never
    //! a delay, a range or a radius -- those are [A].
    use super::*;
    use crate::ItemRow;
    use crate::brain::{
        FleetView, Health, Hostile, Place, Policy, Role, ShipInfo, Stop, StopReason, decide,
    };
    use crate::profile::{HAULER, NpcProfile};
    use eve_server::npc_data::Role as NpcRole;
    use std::collections::BTreeMap;
    use std::time::Instant;

    const STATION: i64 = 60_012_526;
    const SHIP: i64 = 100_000_101;
    const GUN_A: i64 = 100_000_102;
    const GUN_B: i64 = 100_000_103;
    const NPC: i64 = 7_000_000_001;
    const NPC_2: i64 = 7_000_000_002;
    const WRECK: i64 = 100_000_200;
    const BOSS_SHIP: i64 = 100_000_009;
    const BOSS: i64 = 90_000_009;

    fn row(item_id: i64, type_id: i64, category_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 90_000_001,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity,
            group_id: 0,
            category_id,
            singleton: quantity == 1,
        }
    }

    /// An invented reload lockout.
    const RELOAD: Duration = Duration::from_millis(4_321);

    fn gun(item_id: i64, flag_id: i64, rounds: i64) -> GunInfo {
        GunInfo {
            item_id,
            type_id: GUN,
            flag_id,
            online: true,
            active: false,
            target: None,
            rounds: Some(rounds),
            capacity: 200,
            range: 3_000.0,
            reload: RELOAD,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn hostile(id: i64, x: f64, shooting: Option<i64>) -> Hostile {
        Hostile {
            ball: ball(id, x),
            type_id: 10_013,
            name: "Angel Hijacker".to_owned(),
            shooting,
        }
    }

    fn escort() -> Memory {
        Memory::new(
            "Escort1",
            Policy {
                role: Role::Escort,
                ..Policy::fast()
            },
        )
    }

    /// Docked in a Rifter with two full guns and a stocked hold.
    fn docked_ready() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(RIFTER),
        });
        o.guns = vec![gun(GUN_A, 27, 200), gun(GUN_B, 28, 200)];
        o.cargo = vec![row(1, AMMO, CATEGORY_CHARGE, SPARE_ROUNDS)];
        o.home = Some(STATION);
        o.lock_range = 20_000.0;
        o
    }

    /// In space beside the boss, nothing hostile.
    fn in_space() -> Observation {
        let mut o = docked_ready();
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(BOSS),
        });
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: Some(ball(STATION, -1_000.0)),
            boss: Some(ball(BOSS_SHIP, 1_000.0)),
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: Some(BOSS_SHIP),
        });
        o
    }

    fn park(o: &mut Observation) -> &mut ParkView {
        o.park.as_mut().expect("a park")
    }

    fn kinds(acts: &[Act]) -> Vec<ActKind> {
        acts.iter().map(Act::kind).collect()
    }

    // ---- PROG-PVE: the fit follows the class, and the class climbs ----------

    /// A cruiser of the table, docked with `n` medium guns fitted and full.
    fn docked_cruiser(n: usize) -> Observation {
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(CRUISER),
        });
        o.guns = (0..n)
            .map(|i| GunInfo {
                type_id: medium.gun,
                ..gun(GUN_A + i as i64, 27 + i as i64, 200)
            })
            .collect();
        let mut rounds = row(1, medium.round, CATEGORY_CHARGE, SPARE_ROUNDS);
        rounds.group_id = crate::ammo::GROUP_ROUNDS;
        o.cargo = vec![rounds];
        o
    }

    /// A table cruiser (the Minmatar one the ladder starts at).
    const CRUISER: i64 = 629;

    fn closed_climber() -> Memory {
        let mut m = escort();
        m.policy.closed = true;
        m
    }

    #[test]
    fn a_dev_bot_boards_its_presets_role_hull_and_asks_for_nothing() {
        let role_hull = crate::hull_choice::ROLE_HULLS[1];
        let mut m = closed_climber();
        m.policy.dev = Some(std::sync::Arc::new(crate::dev::DevBot {
            group: "g".into(),
            ticker: "G".into(),
            skills: crate::dev::Skills::Role,
            skill_ids: None,
            hull: Some(role_hull),
            home: None,
            boss: false,
            hall: std::sync::Arc::new(crate::dev::DevHall::default()),
        }));
        // Docked in the rookie hull, the role hull in the hangar.
        let mut o = docked_cruiser(0);
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::brain::CAPSULE),
        });
        o.hangar = vec![row(GUN_B, role_hull, 6, 1)];
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Board(GUN_B)]);
    }

    #[test]
    fn a_cruiser_fits_its_class_guns_not_the_frigates() {
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let mut o = docked_cruiser(1);
        o.hangar = vec![row(GUN_B, medium.gun, 7, 1)];
        let (_, acts) = decide(&closed_climber(), &o);
        assert_eq!(acts, vec![Act::FitAuto(GUN_B)], "the class's gun");
        // A small gun in the hangar is not a cruiser's gun.
        o.hangar = vec![row(GUN_B, GUN, 7, 1)];
        let (_, acts) = decide(&closed_climber(), &o);
        assert_ne!(acts, vec![Act::FitAuto(GUN_B)]);
    }

    #[test]
    fn a_cruiser_short_of_guns_buys_them_once_per_arm_interval() {
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let o = docked_cruiser(1);
        let (m, acts) = decide(&closed_climber(), &o);
        assert_eq!(
            acts,
            vec![Act::Drone(crate::drone_use::DroneAct::Arm(
                crate::hull_choice::Class::Cruiser
            ))]
        );
        assert!(m.combat.arm_next.is_some());
        // Asked again at once, it does not ask again: the old rule's wait.
        let (_, again) = decide(&m, &o);
        assert!(
            !again.contains(&Act::Drone(crate::drone_use::DroneAct::Arm(
                crate::hull_choice::Class::Cruiser
            ))),
            "{again:?}"
        );
        // The open mode never buys.
        let (_, open) = decide(&escort(), &o);
        assert!(open.iter().all(|a| !matches!(a, Act::Drone(_))), "{open:?}");
        assert_eq!(medium.size, crate::fit_plan::Size::Medium);
    }

    #[test]
    fn medium_rounds_are_the_cruisers_and_small_ones_are_not() {
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let mut o = docked_cruiser(medium.guns);
        for g in &mut o.guns {
            g.rounds = Some(0);
        }
        o.cargo.clear();
        let mut small = row(2, AMMO, CATEGORY_CHARGE, 600);
        small.group_id = crate::ammo::GROUP_ROUNDS;
        o.hangar = vec![small];
        let mut m = closed_climber();
        m.sold = true;
        // Small rounds in the hangar do not load a medium gun: it buys.
        let (_, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Drone(crate::drone_use::DroneAct::Arm(
                crate::hull_choice::Class::Cruiser
            ))]
        );
        let mut round = row(3, medium.round, CATEGORY_CHARGE, 600);
        round.group_id = crate::ammo::GROUP_ROUNDS;
        o.hangar.push(round);
        let (_, acts) = decide(&m, &o);
        assert!(
            matches!(&acts[..], [Act::LoadAmmo { charges, .. }] if charges == &vec![3]),
            "{acts:?}"
        );
    }

    #[test]
    fn an_armed_climber_looks_for_a_bigger_hull_once_per_interval() {
        use crate::drone_use::DroneAct;
        use crate::hull_choice::Class;
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let mut o = docked_cruiser(medium.guns);
        o.now = Instant::now();
        let mut m = closed_climber();
        m.now = Some(o.now);
        assert_eq!(
            upgrade_due(&mut m, &o, Some(CRUISER)),
            Some(vec![Act::Drone(DroneAct::Upgrade(Class::Cruiser))])
        );
        assert_eq!(
            upgrade_due(&mut m, &o, Some(CRUISER)),
            None,
            "once per interval"
        );
        m.now = Some(o.now + crate::progression::UPGRADE_EVERY);
        assert!(
            upgrade_due(&mut m, &o, Some(CRUISER)).is_some(),
            "and again after it"
        );
        // Nothing above a battleship; an unarmed ship, the open mode and a
        // miner never look.
        let mut top = closed_climber();
        top.now = Some(o.now);
        assert_eq!(upgrade_due(&mut top, &o, Some(24_692)), None);
        let mut bare = o.clone();
        bare.guns.clear();
        let mut m2 = closed_climber();
        m2.now = Some(o.now);
        assert_eq!(upgrade_due(&mut m2, &bare, Some(CRUISER)), None);
        let mut open = escort();
        open.now = Some(o.now);
        assert_eq!(upgrade_due(&mut open, &o, Some(CRUISER)), None);
        let mut miner = closed_climber();
        miner.policy.role = Role::Miner;
        miner.now = Some(o.now);
        assert_eq!(upgrade_due(&mut miner, &o, Some(CRUISER)), None);
        // A hull the table does not know is not a rung.
        let mut odd = closed_climber();
        odd.now = Some(o.now);
        assert_eq!(upgrade_due(&mut odd, &o, Some(1)), None);
    }

    #[test]
    fn the_loot_sale_keeps_every_class_its_guns_and_sells_the_rest() {
        let medium =
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile);
        let keep = crate::fit_plan::keep_guns(crate::hull_choice::Weapon::Projectile);
        let n = keep.iter().find(|(g, _)| *g == medium.gun).unwrap().1 as i64;
        let hangar = [row(5, medium.gun, 7, n + 2)];
        let sold = lots(&hangar, 0);
        assert_eq!(sold.len(), 1);
        assert_eq!(sold[0].quantity, 2, "all but the most any class fits");
    }

    #[test]
    fn the_sale_keeps_the_guns_of_every_weapon_the_plan_needs() {
        use crate::hull_choice::Weapon;
        let hybrid = crate::fit_plan::keep_guns(Weapon::Hybrid)[0].0;
        let hangar = [row(5, hybrid, 7, 2)];
        // The current weapon is projectile: the hybrid guns would be loot.
        assert_eq!(lots_for(&hangar, 0, Weapon::Projectile).len(), 1);
        // The plan also names hybrids: they stay.
        let both = [Weapon::Projectile, Weapon::Hybrid];
        assert!(lots_keeping(&hangar, 0, &both).is_empty());
        // The contract offer knows no plan and keeps every system's guns.
        assert!(lots_keeping(&hangar, 0, &ALL_WEAPONS).is_empty());
    }

    #[test]
    fn the_plan_names_the_current_and_the_seeds_weapon_once_each() {
        use crate::hull_choice::Weapon;
        let seed = 3;
        let drawn = crate::progression::weapon_of(crate::progression::Race::draw(seed), seed);
        let got = plan_weapons_of(Weapon::Laser, None, seed);
        assert_eq!(got[0], Weapon::Laser);
        assert!(got.iter().any(|w| same_system(*w, drawn)));
        assert_eq!(
            plan_weapons_of(Weapon::Hybrid, None, seed)
                .iter()
                .filter(|w| matches!(w, Weapon::Hybrid | Weapon::Drones))
                .count(),
            1
        );
    }

    #[test]
    fn docked_a_hull_missing_its_guns_gets_one_multiadd_and_one_online() {
        // FAST-KIT: a stack of the class's guns fits in one call, and every
        // offline gun goes online in one turn.
        let mut o = docked_ready();
        o.guns.clear();
        o.hangar = vec![row(GUN_B, GUN, 7, WANT_GUNS as i64)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(
            acts,
            vec![Act::FitAutoN {
                item: GUN_B,
                n: WANT_GUNS
            }]
        );
        // A stack of one fits as the plain act.
        o.hangar = vec![row(GUN_B, GUN, 7, 1)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::FitAuto(GUN_B)]);

        let mut o = docked_ready();
        o.guns[0].online = false;
        o.guns[1].online = false;
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::OnlineMany(vec![GUN_A, GUN_B])]);
    }

    #[test]
    fn a_quick_policy_undocks_at_once() {
        let mut m = escort();
        m.policy.dwell = (Duration::from_secs(30), Duration::from_secs(40));
        let (m2, acts) = decide(&m, &docked_ready());
        assert_ne!(kinds(&acts), vec![ActKind::Undock], "{acts:?}");
        drop(m2);
        m.policy.quick = true;
        let (_, acts) = decide(&m, &docked_ready());
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
    }

    #[test]
    fn docked_a_capsule_boards_the_rifter_or_sets_one_up() {
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(brain::CAPSULE),
        });
        o.guns.clear();
        o.cargo.clear();
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(
            acts,
            vec![Act::GmSetup {
                ship: true,
                module: true,
                ammo: true
            }]
        );
        o.hangar = vec![row(SHIP, RIFTER, 6, 1)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::Board(SHIP)]);

        let mut m = escort();
        m.policy.gm_setup = false;
        o.hangar.clear();
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);
    }

    #[test]
    fn docked_guns_are_fitted_onlined_loaded_and_the_hold_stocked() {
        let mut o = docked_ready();
        o.guns.truncate(1);
        o.hangar = vec![row(GUN_B, GUN, 7, 1), row(2, AMMO, CATEGORY_CHARGE, 600)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::FitAuto(GUN_B)]);

        let mut o = docked_ready();
        o.guns[1].online = false;
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::Online(GUN_B)]);

        let mut o = docked_ready();
        o.guns[0].rounds = Some(0);
        o.hangar = vec![row(2, AMMO, CATEGORY_CHARGE, 600)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(
            acts,
            vec![Act::LoadAmmo {
                guns: vec![(GUN_A, 27)],
                charges: vec![2],
                location: STATION
            }]
        );

        let mut o = docked_ready();
        o.cargo.clear();
        o.hangar = vec![row(2, AMMO, CATEGORY_CHARGE, 600)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Stock]);

        let (_, acts) = decide(&escort(), &docked_ready());
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);
    }

    #[test]
    fn docked_loot_in_the_hold_is_unloaded_rounds_are_not() {
        let mut o = docked_ready();
        o.cargo.push(row(9, 25_589, 7, 3));
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::Unload(vec![(9, 3)])]);
        // Spare probes stay in the hold.
        let mut probe = row(10, 25_589, 7, 4);
        probe.group_id = crate::camp::GROUP_PROBE;
        o.cargo.push(probe);
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::Unload(vec![(9, 3)])]);
    }

    /// fix-w8b B5: a camper does not wait for the tether, so with none it still
    /// locks the hostile in reach instead of asking for an orbit forever
    /// (`hold_position` drops the Orbit it also pushes).
    #[test]
    fn a_camper_without_a_tether_locks_instead_of_orbiting() {
        let mut o = in_space();
        park(&mut o).hostiles = vec![hostile(NPC, 3_000.0, Some(BOSS_SHIP))];
        park(&mut o).ego_tether = None;
        let mut m = escort();
        m.policy.pvp = Some(std::sync::Arc::new(
            crate::hunt::Ground::new().with_security(30_000_001, -0.5),
        ));
        m.combat.role = Some(crate::doctrine::Role::Interdict);
        assert!(crate::camp::camping(&m, &o));
        // The first pass notices the hostile; the next one acts on it.
        let (m, _) = decide(&m, &o);
        let (_, acts) = decide(&m, &o);
        assert!(acts.contains(&Act::Lock(NPC)), "{acts:?}");
        // A non-camper still takes the tether first.
        let plain = escort();
        let (plain, _) = decide(&plain, &o);
        let (_, acts) = decide(&plain, &o);
        assert!(
            acts.iter().any(|a| matches!(a, Act::Orbit { .. })),
            "{acts:?}"
        );
    }

    #[test]
    fn an_npc_shooting_the_boss_is_orbited_locked_and_shot_with_every_gun() {
        let mut o = in_space();
        park(&mut o).hostiles = vec![hostile(NPC, 3_000.0, Some(BOSS_SHIP))];
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Orbit]);
        assert!(matches!(acts[0], Act::Orbit { id: NPC, .. }));
        assert_eq!(m.target.as_ref().map(|t| t.0), Some(NPC));

        park(&mut o).ego_tether = Some(NPC);
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Lock(NPC)]);

        o.locked = vec![NPC];
        let (_, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![
                Act::Fire {
                    gun: GUN_A,
                    target: NPC
                },
                Act::Fire {
                    gun: GUN_B,
                    target: NPC
                }
            ]
        );
    }

    #[test]
    fn the_npc_on_the_boss_beats_a_nearer_idle_one() {
        let mut o = in_space();
        park(&mut o).hostiles = vec![
            hostile(NPC_2, 1_500.0, None),
            hostile(NPC, 30_000.0, Some(BOSS_SHIP)),
        ];
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.target.map(|t| t.0), Some(NPC));
    }

    #[test]
    fn an_npc_far_from_the_boss_and_idle_is_left_alone() {
        let mut o = in_space();
        park(&mut o).hostiles = vec![hostile(NPC, 1_000_000.0, None)];
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(m.target, None);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
    }

    #[test]
    fn a_boss_more_than_a_warp_away_is_warped_to_by_character_id() {
        let mut o = in_space();
        park(&mut o).boss = Some(ball(BOSS_SHIP, 1_000_000.0));
        let (_, acts) = decide(&escort(), &o);
        assert!(
            matches!(acts.as_slice(), [Act::WarpToMember { char_id: BOSS, .. }]),
            "{acts:?}"
        );

        // A hostile beside the far boss: still the boss first.
        park(&mut o).hostiles = vec![hostile(NPC, 1_003_000.0, Some(BOSS_SHIP))];
        let (_, acts) = decide(&escort(), &o);
        assert!(
            matches!(acts.as_slice(), [Act::WarpToMember { char_id: BOSS, .. }]),
            "{acts:?}"
        );
    }

    /// WARP-MIN: a boss 5 km off, or 160 km off (a 10 km standoff would
    /// land inside the 150 km minimum), is followed, never warped to.
    #[test]
    fn a_boss_inside_the_minimum_warp_is_followed_not_warped_to() {
        for x in [5_000.0, MIN_WARP_DISTANCE + WARP_STANDOFF] {
            let mut o = in_space();
            park(&mut o).boss = Some(ball(BOSS_SHIP, x));
            let (_, acts) = decide(&escort(), &o);
            assert!(
                !acts
                    .iter()
                    .any(|a| matches!(a, Act::WarpToMember { .. } | Act::WarpToItem { .. })),
                "{x}: {acts:?}"
            );
        }
        let mut o = in_space();
        park(&mut o).ego_tether = None;
        park(&mut o).boss = Some(ball(BOSS_SHIP, MIN_WARP_DISTANCE + WARP_STANDOFF));
        let (_, acts) = decide(&escort(), &o);
        assert!(
            matches!(acts.as_slice(), [Act::Follow { id: BOSS_SHIP, .. }]),
            "{acts:?}"
        );
    }

    #[test]
    fn warp_or_approach_picks_by_the_landing_point() {
        let ego = ball(1, 0.0);
        let far = ball(2, MIN_WARP_DISTANCE + 3.0 * WARP_STANDOFF);
        assert!(matches!(
            warp_or_approach(&ego, &far, WARP_STANDOFF),
            Act::WarpToItem { id: 2, .. }
        ));
        let near = ball(3, MIN_WARP_DISTANCE);
        assert!(matches!(
            warp_or_approach(&ego, &near, WARP_STANDOFF),
            Act::Follow { id: 3, .. }
        ));
    }

    #[test]
    fn the_pvp_kit_is_not_loot_and_is_not_sold() {
        use crate::{camp, hunt, pvp_kit};
        let mut kit = vec![
            row(1, pvp_kit::STOCK_TYPES[0], 7, 3),
            row(2, hunt::SCRAMBLER, 7, 2),
            row(3, hunt::STASIS_WEBIFIER, 7, 2),
            row(4, 5_001, 7, 2),
            row(5, 5_002, 7, 2),
        ];
        kit[3].group_id = camp::GROUP_LAUNCHER;
        kit[4].group_id = camp::GROUP_PROBE;
        for r in &kit {
            assert!(is_pvp_kit(r) && !is_loot(r), "{r:?}");
        }
        assert!(lots(&kit, 0).is_empty());
        // A plain module stack still sells.
        let plain = row(6, 5_003, 7, 2);
        assert!(!is_pvp_kit(&plain));
    }

    #[test]
    fn when_the_target_goes_the_guns_stop_the_wreck_is_looted_and_it_returns() {
        let mut o = in_space();
        o.chat = true;
        park(&mut o).hostiles = vec![hostile(NPC, 3_000.0, Some(BOSS_SHIP))];
        park(&mut o).ego_tether = Some(NPC);
        o.locked = vec![NPC];
        let mut m = escort();
        m.greeted = Some(7);
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::Say, ActKind::Fire, ActKind::Fire]
        );

        // Gone: the guns still running are stopped first.
        park(&mut o).hostiles.clear();
        o.locked.clear();
        for g in &mut o.guns {
            g.active = true;
            g.target = Some(NPC);
        }
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::Say, ActKind::CeaseFire, ActKind::CeaseFire]
        );
        assert_eq!(m.kills, 1);
        assert_eq!(m.target, None);

        // Stopped; a wreck 10 km off: approach, then loot, once.
        for g in &mut o.guns {
            g.active = false;
            g.target = None;
        }
        park(&mut o).wrecks = vec![ball(WRECK, 10_000.0)];
        let (m, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Follow {
                id: WRECK,
                range: WRECK_APPROACH
            }]
        );
        park(&mut o).ego = ball(SHIP, 9_500.0);
        park(&mut o).ego_tether = Some(WRECK);
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Loot(WRECK)]);

        // Looted: back to the boss.
        let (_, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Follow {
                id: BOSS_SHIP,
                range: ESCORT_RANGE
            }]
        );
    }

    #[test]
    fn a_full_hold_skips_the_wreck() {
        let mut o = in_space();
        o.hold_full = true;
        park(&mut o).wrecks = vec![ball(WRECK, 1_000.0)];
        park(&mut o).ego_tether = None;
        park(&mut o).boss = Some(ball(BOSS_SHIP, 10_000.0));
        let (_, acts) = decide(&escort(), &o);
        assert!(matches!(
            acts.as_slice(),
            [Act::Follow { id: BOSS_SHIP, .. }]
        ));
    }

    #[test]
    fn hurt_goes_home_to_repair_and_ceases_fire_first() {
        let mut o = in_space();
        o.health = Some(Health {
            shield: 0.0,
            armour: 0.2,
            hull: 1.0,
        });
        o.guns[0].active = true;
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(acts, vec![Act::CeaseFire(GUN_A)]);
        assert_eq!(m.homing, Some(Homing::Repair));
        o.guns[0].active = false;
        park(&mut o).station = Some(ball(STATION, 500.0));
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::Dock(STATION)]);
    }

    #[test]
    fn an_empty_gun_reloads_from_the_hold_and_empty_everywhere_goes_home() {
        let mut o = in_space();
        o.guns[0].rounds = Some(0);
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(
            acts,
            vec![Act::LoadAmmo {
                guns: vec![(GUN_A, 27)],
                charges: vec![1],
                location: SHIP
            }]
        );
        // The lockout is the gun's own `reload` (FX11), not a table's.
        assert_eq!(m.reload_until, Some(o.now + RELOAD));

        let mut o = in_space();
        o.cargo.clear();
        for g in &mut o.guns {
            g.rounds = Some(0);
        }
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.homing, Some(Homing::Rearm));
    }

    #[test]
    fn alone_it_rats_in_the_belt() {
        let mut o = in_space();
        o.fleet = None;
        park(&mut o).boss = None;
        park(&mut o).rocks = vec![ball(eve_server::asteroid::MIN_ROCK_ID, 400_000.0)];
        let (_, acts) = decide(&escort(), &o);
        assert!(matches!(acts.as_slice(), [Act::WarpToItem { .. }]));

        // In the belt, an NPC near it is a target; one a system away is not.
        park(&mut o).ego = ball(SHIP, 395_000.0);
        park(&mut o).ego_tether = None;
        park(&mut o).hostiles = vec![hostile(NPC, 405_000.0, None), hostile(NPC_2, 0.0, None)];
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.target.map(|t| t.0), Some(NPC));
    }

    #[test]
    fn three_failed_shots_take_it_home_a_failed_line_does_not() {
        let mut o = in_space();
        o.failures.insert(ActKind::Say, 5);
        o.failures.insert(ActKind::Loot, 5);
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.homing, None);
        o.failures.insert(ActKind::Fire, GIVE_UP_AFTER);
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.homing, Some(Homing::Recovery));
    }

    #[test]
    fn a_stop_push_does_not_confuse_the_escort() {
        let mut o = in_space();
        o.last_stop = Some(Stop {
            seq: 1,
            reason: StopReason::Depleted,
        });
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
    }

    #[test]
    fn a_new_fleet_is_greeted_once_with_chat_and_never_without() {
        let mut o = in_space();
        o.chat = true;
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Say]);
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
        o.chat = false;
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(kinds(&acts), vec![ActKind::Wait]);
    }

    /// Invented NPC types (CP46): a damage dealer, a tackler, a hauler.
    const DPS_RAT: i64 = 990_300_001;
    const TACKLER_RAT: i64 = 990_300_002;
    const HAULER_RAT: i64 = 990_300_004;
    /// A damage dealer with fewer hit points than [`DPS_RAT`].
    const WEAK_RAT: i64 = 990_300_005;

    /// An escort whose policy carries `rows` as its NPC profiles.
    fn with_profiles(rows: &[(i64, NpcRole, &str, f64, f64)]) -> Memory {
        let mut m = escort();
        m.policy.profiles = Some(std::sync::Arc::new(
            rows.iter()
                .map(|&(id, role, class, hp, dps)| {
                    (
                        id,
                        NpcProfile {
                            role,
                            class: class.to_owned(),
                            hp,
                            dps,
                        },
                    )
                })
                .collect(),
        ));
        m
    }

    fn rat(id: i64, x: f64, type_id: i64, shooting: Option<i64>) -> Hostile {
        Hostile {
            type_id,
            ..hostile(id, x, shooting)
        }
    }

    /// BOT-CAP: drained, with a neut rat on grid it can reach, the ratter
    /// makes the neut its target (and fires no gun the gate holds); when the
    /// neut is out of reach it goes home, drained.
    #[test]
    fn a_drained_ratter_targets_the_neut_rat_or_leaves() {
        let neut = 10_099;
        let mut o = in_space();
        o.fleet = None;
        park(&mut o).boss = None;
        park(&mut o).ego_tether = Some(neut);
        park(&mut o).hostiles = vec![
            hostile(10_050, 500.0, Some(SHIP)),
            hostile(neut, 4_000.0, None),
        ];
        o.locked = vec![neut];
        o.cap = crate::cap::Scene {
            fraction: Some(0.02),
            drained: true,
            hold: true,
            neuts: vec![neut],
        };
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(m.target.as_ref().map(|t| t.0), Some(neut));
        assert!(m.homing.is_none());
        assert!(
            !acts.iter().any(|a| matches!(a, Act::Fire { .. })),
            "the gate holds the guns: {acts:?}"
        );
        // Out of lock range: leave.
        o.lock_range = 100.0;
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.homing, Some(Homing::Drained));
        assert!(m.dock_reason().contains("drained"));
        // Not drained: an ordinary fight, the nearest shooter first.
        o.lock_range = 80_000.0;
        o.cap = crate::cap::Scene::default();
        let (m, _) = decide(&escort(), &o);
        assert_ne!(m.homing, Some(Homing::Drained));
        assert_eq!(m.target.as_ref().map(|t| t.0), Some(10_050));
    }

    /// C6 §5.2: a tackler beats a nearer damage dealer -- idle or both on
    /// the boss -- and with no profiles the nearer one is still chosen.
    #[test]
    fn a_tackler_is_chosen_over_a_nearer_dps_ship() {
        let m = with_profiles(&[
            (DPS_RAT, NpcRole::Dps, "Frigate", 100.0, 0.0),
            (TACKLER_RAT, NpcRole::Tackler, "Frigate", 100.0, 0.0),
        ]);
        for shooting in [None, Some(BOSS_SHIP)] {
            let mut o = in_space();
            park(&mut o).hostiles = vec![
                rat(NPC_2, 3_000.0, DPS_RAT, shooting),
                rat(NPC, 20_000.0, TACKLER_RAT, shooting),
            ];
            let (after, _) = decide(&m, &o);
            assert_eq!(after.target.map(|t| t.0), Some(NPC), "{shooting:?}");
            let (after, _) = decide(&escort(), &o);
            assert_eq!(after.target.map(|t| t.0), Some(NPC_2), "{shooting:?}");
        }
    }

    /// C6 §5.2: a hauler only when nothing else is left; among the rest the
    /// fewest hit points first.
    #[test]
    fn a_hauler_is_left_for_last_and_the_weakest_goes_first() {
        let m = with_profiles(&[
            (DPS_RAT, NpcRole::Dps, "Frigate", 100.0, 0.0),
            (WEAK_RAT, NpcRole::Dps, "Frigate", 50.0, 0.0),
            (HAULER_RAT, NpcRole::Dps, HAULER, 1.0, 0.0),
        ]);
        let mut o = in_space();
        park(&mut o).hostiles = vec![
            rat(NPC_2, 3_000.0, HAULER_RAT, None),
            rat(NPC, 9_000.0, DPS_RAT, None),
            rat(NPC + 2, 15_000.0, WEAK_RAT, None),
        ];
        let (after, _) = decide(&m, &o);
        assert_eq!(after.target.map(|t| t.0), Some(NPC + 2));
        park(&mut o).hostiles.retain(|h| h.type_id == HAULER_RAT);
        let (after, _) = decide(&m, &o);
        assert_eq!(after.target.map(|t| t.0), Some(NPC_2));
    }

    /// C6 §5.2: home when the hostiles near us add up to more than
    /// `RETREAT_AT`; at it, or with no profiles, it fights.
    #[test]
    fn it_retreats_above_retreat_at_and_not_at_it() {
        let half = RETREAT_AT / 2.0;
        let mut o = in_space();
        park(&mut o).hostiles = vec![
            rat(NPC, 3_000.0, DPS_RAT, Some(SHIP)),
            rat(NPC_2, 4_000.0, DPS_RAT, Some(SHIP)),
        ];
        let at = with_profiles(&[(DPS_RAT, NpcRole::Dps, "Frigate", 100.0, half)]);
        let (m, acts) = decide(&at, &o);
        assert_eq!(m.homing, None);
        assert!(matches!(acts.as_slice(), [Act::Orbit { .. }]), "{acts:?}");
        let (m, _) = decide(&escort(), &o);
        assert_eq!(m.homing, None);

        let above = with_profiles(&[(DPS_RAT, NpcRole::Dps, "Frigate", 100.0, half + 1.0)]);
        let (m, acts) = decide(&above, &o);
        assert_eq!(m.homing, Some(Homing::Repair));
        assert!(
            !acts.iter().any(|a| matches!(a, Act::Orbit { .. })),
            "{acts:?}"
        );

        // One of them far off the boss and us: only the near one counts.
        park(&mut o).hostiles[1].ball = ball(NPC_2, 1_000_000.0);
        let (m, _) = decide(&above, &o);
        assert_eq!(m.homing, None);
    }

    #[test]
    fn a_scram_does_not_set_the_orbit() {
        let mut weapon = gun(1, 27, 200);
        weapon.range = 9_000.0;
        let mut scram = gun(2, 28, 0);
        scram.range = 500.0;
        scram.kind = GunKind::Scram;
        assert_eq!(fight_range(&[weapon, scram]), 9_000.0);
        // No weapon: the old rule, so an ewar-only hull orbits as before.
        assert_eq!(fight_range(&[scram]), 500.0);
        // A dry weapon with a scrambler beside it is still dry.
        let mut dry_weapon = weapon;
        dry_weapon.rounds = Some(0);
        dry_weapon.capacity = 200;
        assert!(dry(&[dry_weapon, scram]));
    }

    #[test]
    fn the_fight_range_is_the_shortest_gun() {
        let mut a = gun(1, 27, 1);
        a.range = 4_000.0;
        let b = gun(2, 28, 1);
        assert_eq!(fight_range(&[a, b]), b.range);
        assert_eq!(fight_range(&[]), ESCORT_RANGE);
    }

    // ---- E10b: the loot sale and the makeshift hull --------------------

    /// An invented rookie hull and civilian gun: no client value.
    const ROOKIE: i64 = 990_870_001;
    const CIVILIAN: i64 = 990_870_002;
    /// Invented loot types.
    const LOOT_ONLY: i64 = 990_870_003;
    const T1: i64 = 990_870_004;

    fn civilian(item_id: i64) -> GunInfo {
        GunInfo {
            type_id: CIVILIAN,
            capacity: 0,
            ..gun(item_id, 27, 0)
        }
    }

    fn order(id: i64, type_id: i64, station: i64, price: f64, bid: bool) -> BookOrder {
        BookOrder {
            order_id: id,
            type_id,
            station_id: station,
            price,
            vol_remaining: 10,
            bid,
        }
    }

    fn at(p_ref: f64) -> Option<Quote> {
        Some(Quote::at_ref(p_ref))
    }

    /// What a ratter lists a type at with no history and nothing cheaper
    /// here: the seller's ask on `P_ref` (E10).
    fn list_at(p_ref: f64) -> f64 {
        crate::pricing::ask(&Quote::at_ref(p_ref)).expect("an ask")
    }

    fn lot(type_id: i64, quantity: i64, whole: bool) -> Lot {
        Lot {
            item_id: 1,
            type_id,
            quantity,
            whole,
        }
    }

    /// **E10b, the pure rule**: an NPC bid here takes loot-only stacks; a
    /// T1 stack worth listing is listed at `P_ref` (or the cheapest ask
    /// here, if lower); one worth less, or with no room for an order, is
    /// refined whole and kept in part; no `P_ref` keeps it.
    #[test]
    fn the_loot_rule_sells_to_npc_bids_lists_t1_and_refines_the_rest() {
        let npc = eve_server::econ_exchange::NPC_ORDER_BASE;
        let p_ref = LIST_MIN_VALUE;
        let bid_price = p_ref * SELL_FLOOR / 2.0;
        let mut book = Book::default();
        book.bids
            .push(order(npc, LOOT_ONLY, STATION, bid_price, true));
        // An NPC bid sells even under the floor; elsewhere it does not.
        let l = lot(LOOT_ONLY, 3, true);
        assert_eq!(
            fate(&l, &book, STATION, at(p_ref), false, true),
            Fate::Bid(bid_price)
        );
        assert_ne!(
            fate(&l, &book, STATION + 1, at(p_ref), false, true),
            Fate::Bid(bid_price)
        );
        // A player's bid under the floor is not taken; at it, it is.
        let mut player = Book::default();
        player.bids.push(order(1, T1, STATION, bid_price, true));
        let t1 = lot(T1, 2, true);
        assert_eq!(
            fate(&t1, &player, STATION, at(p_ref), false, true),
            Fate::List(list_at(p_ref))
        );
        player.bids[0].price = p_ref * SELL_FLOOR;
        assert_eq!(
            fate(&t1, &player, STATION, at(p_ref), false, true),
            Fate::Bid(p_ref * SELL_FLOOR)
        );

        let none = Book::default();
        // The cheapest ask here caps the listing, never under the floor.
        let mut asks = Book::default();
        asks.asks
            .push(order(2, T1, STATION, list_at(p_ref) * 0.9, false));
        assert_eq!(
            fate(&t1, &asks, STATION, at(p_ref), false, true),
            Fate::List(cents(list_at(p_ref) * 0.9))
        );
        asks.asks[0].price = p_ref * SELL_FLOOR / 2.0;
        assert_eq!(
            fate(&t1, &asks, STATION, at(p_ref), false, true),
            Fate::Reprocess
        );
        // Too little to list, no room, or listed already.
        let small = lot(T1, 1, true);
        let tiny = LIST_MIN_VALUE / 2.0;
        assert_eq!(
            fate(&small, &none, STATION, at(tiny), false, true),
            Fate::Reprocess
        );
        assert_eq!(
            fate(&t1, &none, STATION, at(p_ref), false, false),
            Fate::Reprocess
        );
        assert_eq!(fate(&t1, &none, STATION, at(p_ref), true, true), Fate::Keep);
        // A part stack is never refined; no price, nothing done.
        assert_eq!(
            fate(&lot(T1, 1, false), &none, STATION, at(tiny), false, true),
            Fate::Keep
        );
        assert_eq!(fate(&t1, &none, STATION, None, false, true), Fate::Keep);
    }

    /// **E10b**: the plan lists one ask per type and station and stops at
    /// [`ORDER_ROOM`] open orders.
    #[test]
    fn the_plan_lists_once_per_type_and_inside_the_order_room() {
        // Worth listing one unit at the seller's ask.
        let p_ref = 2.0 * LIST_MIN_VALUE;
        let prices: BTreeMap<i64, Quote> =
            (0..10).map(|i| (T1 + i, Quote::at_ref(p_ref))).collect();
        let lots: Vec<Lot> = (0..10)
            .map(|i| Lot {
                item_id: 100 + i,
                type_id: T1 + i,
                quantity: 1,
                whole: true,
            })
            .chain(std::iter::once(Lot {
                item_id: 200,
                type_id: T1,
                quantity: 1,
                whole: true,
            }))
            .collect();
        let p = plan(&lots, &BTreeMap::new(), &prices, STATION, &[]);
        assert_eq!(p.lists.len(), ORDER_ROOM);
        assert_eq!(p.reprocess.len(), lots.len() - ORDER_ROOM - 1);
        assert_eq!(p.kept, vec![T1], "the second stack of a listed type");
        let mine = vec![order(9, T1 + 9, STATION, 1.0, false); ORDER_ROOM];
        let p = plan(&lots, &BTreeMap::new(), &prices, STATION, &mine);
        assert!(p.lists.is_empty());
    }

    /// **BOT-TUNE**: a round of any type of the projectile round group is a
    /// round -- it is not unloaded as loot, and it shares the one keep
    /// allowance with EMP S -- and a charge of another group is not.
    #[test]
    fn any_projectile_round_is_a_round_and_shares_the_keep() {
        const OTHER_ROUND: i64 = 9_501;
        const NOT_ROUND_GROUP: i64 = 9_502;
        let mut other = row(3, OTHER_ROUND, CATEGORY_CHARGE, 600);
        other.group_id = crate::ammo::GROUP_ROUNDS;
        let mut missile = row(4, OTHER_ROUND, CATEGORY_CHARGE, 50);
        missile.group_id = NOT_ROUND_GROUP;
        assert!(is_round(&other));
        assert!(is_round(&row(1, AMMO, CATEGORY_CHARGE, 1)));
        assert!(!is_round(&missile));
        // 600 of the other round fill 600 of the allowance; EMP S fills the
        // rest and only the surplus is for sale.
        let lots = lots(&[other, row(2, AMMO, CATEGORY_CHARGE, AMMO_KEEP)], 0);
        let sold: i64 = lots.iter().map(|l| l.quantity).sum();
        assert_eq!(sold, 600);
    }

    /// A Small and a Medium stack each at the allowance are both kept: the
    /// sizes do not share it (fix-w8b B4).
    #[test]
    fn each_round_size_has_its_own_allowance() {
        let mut small = row(1, 178, CATEGORY_CHARGE, AMMO_KEEP);
        small.group_id = crate::fit_plan::GROUP_PROJECTILE;
        let mut medium = row(2, 186, CATEGORY_CHARGE, AMMO_KEEP);
        medium.group_id = crate::fit_plan::GROUP_PROJECTILE;
        let w = [crate::hull_choice::Weapon::Projectile];
        assert!(lots_keeping(&[small, medium], 0, &w).is_empty());
        // Rounds in the hold come off the Small allowance only.
        let got = lots_keeping(&[small, medium], 100, &w);
        let sold: i64 = got.iter().map(|l| l.quantity).sum();
        assert_eq!((got.len(), sold), (1, 100));
        assert_eq!(got[0].item_id, 1);
    }

    /// **E10b**: rounds are kept up to [`AMMO_KEEP`], the hold's counted,
    /// and [`WANT_GUNS`] spare guns; the hull and singletons are never loot.
    #[test]
    fn the_ratter_keeps_its_rounds_up_to_its_load_and_its_spare_guns() {
        let hangar = vec![
            row(1, AMMO, CATEGORY_CHARGE, AMMO_KEEP),
            row(2, AMMO, CATEGORY_CHARGE, 300),
            row(3, GUN, 7, WANT_GUNS as i64 + 1),
            row(4, RIFTER, 6, 1),
            row(5, LOOT_ONLY, 7, 1),
            row(6, LOOT_ONLY, 7, 2),
        ];
        let carried = 100;
        let got = lots(&hangar, carried);
        assert_eq!(
            got,
            vec![
                Lot {
                    item_id: 1,
                    type_id: AMMO,
                    quantity: carried,
                    whole: false
                },
                Lot {
                    item_id: 2,
                    type_id: AMMO,
                    quantity: 300,
                    whole: true
                },
                Lot {
                    item_id: 3,
                    type_id: GUN,
                    quantity: 1,
                    whole: false
                },
                Lot {
                    item_id: 6,
                    type_id: LOOT_ONLY,
                    quantity: 2,
                    whole: true
                },
            ]
        );
        let kept: i64 = hangar
            .iter()
            .filter(|r| r.type_id == AMMO)
            .map(|r| r.quantity)
            .sum::<i64>()
            - got
                .iter()
                .filter(|l| l.type_id == AMMO)
                .map(|l| l.quantity)
                .sum::<i64>();
        assert_eq!(kept + carried, AMMO_KEEP);
    }

    /// **E10b**: docked with loot in the hangar, the ratter sells once a
    /// visit and at most once per `sell_every`; rounds it keeps are not loot.
    #[test]
    fn docked_hangar_loot_is_offered_once_per_sell_every() {
        let mut o = docked_ready();
        o.hangar = vec![row(2, AMMO, CATEGORY_CHARGE, AMMO_KEEP - SPARE_ROUNDS)];
        let (_, acts) = decide(&escort(), &o);
        assert_eq!(
            kinds(&acts),
            vec![ActKind::Undock],
            "kept rounds are not loot"
        );

        o.hangar.push(row(9, LOOT_ONLY, 7, 3));
        let mut m = escort();
        m.policy.sell_every = brain::SELL_EVERY;
        let start = o.now;
        let (m, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::SellLoot]);
        let (m, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock], "once a visit");
        let mut m = m;
        m.sold = false;
        o.now = start + brain::SELL_EVERY / 2;
        let (m, acts) = decide(&m, &o);
        assert_ne!(acts, vec![Act::SellLoot], "not inside sell_every");
        let mut m = m;
        m.sold = false;
        o.now = start + brain::SELL_EVERY;
        let (_, acts) = decide(&m, &o);
        assert_eq!(acts, vec![Act::SellLoot]);
    }

    /// **E10b (E8's Not done)**: a closed fighter in its rookie ship with
    /// no Rifter to be had asks for one when due, and otherwise flies and
    /// fights with the civilian gun, which needs no rounds.
    #[test]
    fn a_rookie_ship_with_a_civilian_gun_flies_and_fights_when_no_rifter_is_had() {
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(ROOKIE),
        });
        o.guns = vec![civilian(GUN_A)];
        o.cargo.clear();
        let mut m = escort();
        m.policy.gm_setup = false;
        m.policy.closed = true;
        let (m, acts) = decide(&m, &o);
        // The chooser's hull first (ship-pool), then the rest of the request.
        assert_eq!(kinds(&acts), vec![ActKind::Drone, ActKind::Acquire]);
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Undock]);

        // No gun at all: it says what it lacks, as before.
        o.guns.clear();
        let (_, acts) = decide(&m, &o);
        assert_eq!(kinds(&acts), vec![ActKind::Missing, ActKind::Wait]);

        // In space: never "out of ammo", and the civilian gun fires.
        let mut o = in_space();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(ROOKIE),
        });
        o.guns = vec![civilian(GUN_A)];
        o.cargo.clear();
        park(&mut o).hostiles = vec![hostile(NPC, 3_000.0, Some(BOSS_SHIP))];
        park(&mut o).ego_tether = Some(NPC);
        o.locked = vec![NPC];
        let (m, acts) = decide(&escort(), &o);
        assert_eq!(m.homing, None);
        assert_eq!(
            acts,
            vec![Act::Fire {
                gun: GUN_A,
                target: NPC
            }]
        );
        // A real gun beside it still runs dry and is reloaded as before.
        assert!(!dry(&[civilian(GUN_A), gun(GUN_B, 28, 0)]));
        assert!(dry(&[gun(GUN_B, 28, 0)]));
    }

    // ---- PROG-PVE2: the weapon systems, the old hulls, richer ground ---------

    const PUNISHER: i64 = 597;

    fn charge(item_id: i64, type_id: i64, group: i64, quantity: i64) -> ItemRow {
        let mut r = row(item_id, type_id, CATEGORY_CHARGE, quantity);
        r.group_id = group;
        r
    }

    #[test]
    fn a_laser_bot_fits_a_laser_and_loads_crystals_not_projectile_rounds() {
        use crate::hull_choice::{Class, Weapon};
        let fit = crate::fit_plan::fit_for(Class::Frigate, Weapon::Laser);
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(PUNISHER),
        });
        o.guns = Vec::new();
        o.cargo.clear();
        o.hangar = vec![row(GUN_B, fit.gun, 7, 1)];
        let (m, acts) = decide(&closed_climber(), &o);
        assert_eq!(acts, vec![Act::FitAuto(GUN_B)], "the small laser");
        assert_eq!(crate::drone_use::weapon(&m), Weapon::Laser);
        // The 125mm gun is not a laser bot's: it is not fitted.
        o.hangar = vec![row(GUN_B, GUN, 7, 1)];
        let (_, acts) = decide(&closed_climber(), &o);
        assert_ne!(acts, vec![Act::FitAuto(GUN_B)]);

        // Two lasers fitted and empty: the crystals load, EMP S does not.
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(PUNISHER),
        });
        o.guns = (0..fit.guns)
            .map(|i| GunInfo {
                type_id: fit.gun,
                rounds: Some(0),
                ..gun(GUN_A + i as i64, 27 + i as i64, 0)
            })
            .collect();
        o.cargo.clear();
        o.hangar = vec![
            charge(6, AMMO, crate::ammo::GROUP_ROUNDS, 600),
            charge(5, fit.round, crate::fit_plan::GROUP_LASER, 600),
        ];
        let mut m = closed_climber();
        m.sold = true;
        let (_, acts) = decide(&m, &o);
        match &acts[..] {
            [Act::LoadAmmo { charges, guns, .. }] => {
                assert_eq!(charges, &vec![5], "the crystals");
                assert_eq!(guns.len(), fit.guns);
            }
            other => panic!("expected a load, got {other:?}"),
        }
    }

    #[test]
    fn a_laser_bot_with_only_projectile_rounds_asks_for_crystals() {
        use crate::hull_choice::{Class, Weapon};
        let fit = crate::fit_plan::fit_for(Class::Frigate, Weapon::Laser);
        let mut o = docked_ready();
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(PUNISHER),
        });
        o.guns = (0..fit.guns)
            .map(|i| GunInfo {
                type_id: fit.gun,
                rounds: Some(0),
                ..gun(GUN_A + i as i64, 27 + i as i64, 0)
            })
            .collect();
        o.cargo.clear();
        o.hangar = vec![charge(6, AMMO, crate::ammo::GROUP_ROUNDS, 600)];
        let mut m = closed_climber();
        m.sold = true;
        let (_, acts) = decide(&m, &o);
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Acquire { ammo: true, .. })),
            "{acts:?}"
        );
    }

    #[test]
    fn another_systems_rounds_and_guns_are_loot_and_the_bots_own_are_kept() {
        use crate::hull_choice::{Class, Weapon};
        let hybrid = crate::fit_plan::fit_for(Class::Frigate, Weapon::Hybrid);
        let laser = crate::fit_plan::fit_for(Class::Frigate, Weapon::Laser);
        let emp = charge(1, AMMO, crate::ammo::GROUP_ROUNDS, 100);
        let anti = charge(2, hybrid.round, crate::fit_plan::GROUP_HYBRID, 100);
        let ids = |lots: Vec<Lot>| lots.iter().map(|l| l.type_id).collect::<Vec<_>>();
        // A hybrid bot sells the projectile rounds and keeps its charges.
        let got = ids(lots_for(&[emp, anti], 0, Weapon::Hybrid));
        assert_eq!(got, vec![AMMO]);
        // A projectile bot is the reverse.
        let got = ids(lots_for(&[emp, anti], 0, Weapon::Projectile));
        assert_eq!(got, vec![hybrid.round]);
        // Guns: a laser bot keeps its lasers (up to the most any class fits)
        // and sells the 125mm guns it no longer fires.
        let lasers = row(3, laser.gun, 7, 9);
        let cannons = row(4, GUN, 7, 9);
        let got = lots_for(&[lasers, cannons], 0, Weapon::Laser);
        let kept = crate::fit_plan::keep_guns(Weapon::Laser)[0].1 as i64;
        assert_eq!(
            got.iter()
                .find(|l| l.type_id == laser.gun)
                .map(|l| l.quantity),
            Some(9 - kept)
        );
        assert_eq!(
            got.iter().find(|l| l.type_id == GUN).map(|l| l.quantity),
            Some(9)
        );
    }

    fn ship_row(item_id: i64, type_id: i64) -> ItemRow {
        row(item_id, type_id, crate::acquire::CATEGORY_SHIP, 1)
    }

    #[test]
    fn a_climber_holding_two_old_hulls_sells_one_and_waits_before_looking_again() {
        use crate::drone_use::DroneAct;
        let mut o = docked_cruiser(
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile)
                .guns,
        );
        o.hangar = vec![ship_row(21, 587), ship_row(22, 585)];
        let mut m = closed_climber();
        m.now = Some(o.now);
        let sell = Some(vec![Act::Drone(DroneAct::SellHulls)]);
        assert_eq!(hull_sale_due(&mut m, &o, Some(CRUISER)), sell);
        assert_eq!(
            hull_sale_due(&mut m, &o, Some(CRUISER)),
            None,
            "once per interval"
        );
        m.now = Some(o.now + crate::hull_sale::SELL_EVERY);
        assert_eq!(
            hull_sale_due(&mut m, &o, Some(CRUISER)),
            sell,
            "and again after it"
        );
        // One old hull is the spare: nothing to sell, and the clock is not
        // started by a look that found nothing.
        let mut one = o.clone();
        one.hangar = vec![ship_row(21, 587)];
        let mut m2 = closed_climber();
        m2.now = Some(o.now);
        assert_eq!(hull_sale_due(&mut m2, &one, Some(CRUISER)), None);
        assert_eq!(m2.combat.sale_next, None);
        // The open mode, a miner and an unknown hull never sell.
        let mut open = escort();
        open.now = Some(o.now);
        assert_eq!(hull_sale_due(&mut open, &o, Some(CRUISER)), None);
        let mut miner = closed_climber();
        miner.policy.role = Role::Miner;
        miner.now = Some(o.now);
        assert_eq!(hull_sale_due(&mut miner, &o, Some(CRUISER)), None);
        let mut odd = closed_climber();
        odd.now = Some(o.now);
        assert_eq!(hull_sale_due(&mut odd, &o, Some(1)), None);
    }

    #[test]
    fn the_sale_comes_after_the_arming_and_the_upgrade_look_in_a_whole_turn() {
        use crate::drone_use::DroneAct;
        let mut o = docked_cruiser(
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile)
                .guns,
        );
        o.hangar = vec![ship_row(21, 587), ship_row(22, 585)];
        // A salvager already fitted, so rule 6b leaves the turn alone.
        o.salvager = Some(crate::brain::ModuleInfo {
            item_id: 5,
            flag_id: 28,
            online: true,
            active: false,
            target: None,
            range: 1_000.0,
        });
        let mut m = closed_climber();
        m.sold = true;
        // The first turn looks for a bigger hull, as before.
        let (m1, acts) = decide(&m, &o);
        assert_eq!(
            acts,
            vec![Act::Drone(DroneAct::Upgrade(
                crate::hull_choice::Class::Cruiser
            ))]
        );
        // The next turn sells the old hull.
        let (_, acts) = decide(&m1, &o);
        assert_eq!(acts, vec![Act::Drone(DroneAct::SellHulls)]);
    }

    /// Two invented systems a gate apart: a safe one with `STATION` and a
    /// richer one with a station of its own.
    const RICH_STATION: i64 = 60_000_002;

    fn two_systems() -> std::sync::Arc<crate::travel::Map> {
        std::sync::Arc::new(
            crate::travel::Map::new()
                .with_security(1, 0.95)
                .with_security(2, 0.7)
                .with_station(STATION, 1)
                .with_station(RICH_STATION, 2)
                .with_gate(1, 101, 2, 102)
                .with_gate(2, 102, 1, 101),
        )
    }

    #[test]
    fn a_cruiser_in_the_safest_system_moves_to_its_band_and_asks_once_an_hour() {
        use crate::drone_use::DroneAct;
        let mut m = closed_climber();
        m.policy.travel = Some(two_systems());
        m.policy.security_floor = crate::travel::ANY_SECURITY;
        let mut o = docked_cruiser(
            crate::fit_plan::fit_for_bot(Some(CRUISER), crate::hull_choice::Weapon::Projectile)
                .guns,
        );
        m.now = Some(o.now);
        let go = Some(vec![Act::Drone(DroneAct::Move(RICH_STATION))]);
        assert_eq!(relocation_due(&mut m, &o, Some(CRUISER)), go);
        assert_eq!(
            relocation_due(&mut m, &o, Some(CRUISER)),
            None,
            "once per interval"
        );
        m.now = Some(o.now + crate::relocate::EVERY);
        assert_eq!(relocation_due(&mut m, &o, Some(CRUISER)), go, "and again");
        // A frigate is at home at 0.95: it stays (and asks again in an hour).
        let mut f = closed_climber();
        f.policy.travel = Some(two_systems());
        f.policy.security_floor = crate::travel::ANY_SECURITY;
        f.now = Some(o.now);
        assert_eq!(relocation_due(&mut f, &o, Some(RIFTER)), None);
        // The site runner moves too; a miner, the open mode, a fleet escort,
        // a bot with no map and a hull the table lacks do not.
        let mut site = closed_climber();
        site.policy.role = &crate::site_runner::ROLE;
        site.policy.travel = Some(two_systems());
        site.policy.security_floor = crate::travel::ANY_SECURITY;
        site.now = Some(o.now);
        assert_eq!(relocation_due(&mut site, &o, Some(CRUISER)), go);
        let mut miner = closed_climber();
        miner.policy.role = Role::Miner;
        miner.policy.travel = Some(two_systems());
        miner.now = Some(o.now);
        assert_eq!(relocation_due(&mut miner, &o, Some(CRUISER)), None);
        let mut open = escort();
        open.policy.travel = Some(two_systems());
        open.now = Some(o.now);
        assert_eq!(relocation_due(&mut open, &o, Some(CRUISER)), None);
        let mut nomap = closed_climber();
        nomap.now = Some(o.now);
        assert_eq!(relocation_due(&mut nomap, &o, Some(CRUISER)), None);
        let mut odd = closed_climber();
        odd.policy.travel = Some(two_systems());
        odd.policy.security_floor = crate::travel::ANY_SECURITY;
        odd.now = Some(o.now);
        assert_eq!(relocation_due(&mut odd, &o, Some(1)), None);
        let mut fleet = closed_climber();
        fleet.policy.travel = Some(two_systems());
        fleet.policy.security_floor = crate::travel::ANY_SECURITY;
        fleet.now = Some(o.now);
        o.fleet = Some(FleetView {
            fleet_id: 7,
            boss: Some(BOSS),
        });
        assert_eq!(relocation_due(&mut fleet, &o, Some(CRUISER)), None);
    }
}

/// **E10b's harness**: one in-process server over invented furniture -- the
/// typedata fixture's shield extender as loot-only (an NPC bid at the home
/// station, which is a hub), its autocannon as T1 loot worth listing and its
/// rig as T1 loot worth refining -- and a ratter docked with all three and
/// more rounds than it keeps. The GM slashes are the test's setup, never
/// the bot's.
#[cfg(test)]
mod harness {
    use super::*;
    use crate::driver::{BotConfig, Pace};
    use eve_marshal::PyValue;
    use eve_server::econ_exchange as exchange;
    use eve_server::model::Character;
    use std::net::SocketAddr;
    use std::sync::Arc;

    /// The typedata fixture's types (invented ids).
    const SHIELD: i64 = 990_000_003;
    const CANNON: i64 = 990_000_002;
    const RIG: i64 = 990_000_004;
    const RATTER: i64 = 90_000_201;
    /// Invented prices: the shield's `P_ref` for its NPC bid; the cannon's
    /// worth listing; the rig's not.
    const SHIELD_REF: f64 = 40_000.0;
    const CANNON_REF: f64 = 2.0 * LIST_MIN_VALUE;
    const RIG_REF: f64 = LIST_MIN_VALUE / 100.0;
    const ROUND_REF: f64 = LIST_MIN_VALUE / 10.0;
    const WALLET: f64 = 5_000_000.0;
    const SURPLUS: i64 = 300;

    fn content() -> eve_server::content::Content {
        // The escort's rounds, on the fixture's invented ammo numbers.
        let rounds = format!(
            "type\t{AMMO}\t83\t{CATEGORY_CHARGE}\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t\
             114=7.0;117=3.0;128=1.0\tFixture Rounds\n"
        );
        let text = format!("{}{rounds}", eve_server::typedata::FIXTURE);
        let td = eve_server::typedata::Table::parse(&text).expect("the fixture parses");
        let price = |id: i64, p: f64, source: &str, cat: i64, group: i64, meta: &str| {
            format!("{id}\t{p}\t{source}\t{p}\t{cat}\t{group}\t{meta}\t1")
        };
        let prices = eve_server::econ_prices::Table::parse(&[
            price(SHIELD, SHIELD_REF, "base", 7, 38, "4"),
            price(CANNON, CANNON_REF, "recipe", 7, 55, "1"),
            price(RIG, RIG_REF, "recipe", 7, 773, "1"),
            price(AMMO, ROUND_REF, "recipe", CATEGORY_CHARGE, 83, "1"),
        ])
        .expect("the rows parse");
        let home = Character::default();
        let station = home.station_id.expect("the default pilot is docked");
        let types = [(
            SHIELD,
            exchange::TypeRow {
                group_id: 38,
                category_id: 7,
                base_price: SHIELD_REF,
                meta_group_id: Some(4),
                tech_level: Some(1),
            },
        )]
        .into_iter()
        .collect();
        let table = exchange::Table::new(
            vec![exchange::Station {
                station_id: station,
                system_id: home.solar_system_id.expect("in a system"),
                region_id: home.region_id,
                owner_id: 1_999_601,
            }],
            &[],
            types,
            &[station],
        )
        .expect("the exchange builds");
        let mut c = eve_server::content::Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(td));
        c.insert("econ_prices", prices);
        c.insert("econ_exchange", table);
        c
    }

    fn start() -> SocketAddr {
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![Character {
            character_id: RATTER,
            name: "Ratter1".to_owned(),
            ship_id: 100_000_201,
            balance: WALLET,
            ..Character::default()
        }];
        config.content = Some(Arc::new(content()));
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    fn login(addr: SocketAddr, name: &str) -> Conn {
        let config = BotConfig::new(name, addr, Pace::Fast);
        let mut c = Conn::connect(addr, std::time::Duration::from_secs(10)).expect("connects");
        c.handshake_as(&config.token).expect("handshake");
        c.select_character_named(name).expect("selects by name");
        c
    }

    /// Setup only: a GM slash, named at the call site.
    fn gm(c: &mut Conn, line: &str) {
        c.call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    }

    fn count(c: &mut Conn, type_id: i64) -> i64 {
        c.hangar_items()
            .expect("the hangar lists")
            .iter()
            .filter(|r| r.type_id == type_id)
            .map(|r| r.quantity)
            .sum()
    }

    /// **E10b, the row's test**: a ratter docked with loot sells the
    /// loot-only stack into the fixture NPC bid, keeps its rounds up to
    /// [`AMMO_KEEP`] and lists the rest, lists the T1 stack worth listing
    /// and refines (or is told why not) the one that is not.
    #[test]
    fn a_ratter_docked_with_loot_sells_loot_only_keeps_its_rounds_and_lists_or_refines_t1() {
        let addr = start();
        let mut c = login(addr, "Ratter1");
        let station = c.session().station_id().expect("docked");
        gm(&mut c, &format!("/createitem {SHIELD} 3"));
        gm(&mut c, &format!("/createitem {CANNON} 2"));
        gm(&mut c, &format!("/createitem {RIG} 1"));
        // The slash makes at most a thousand at once.
        gm(&mut c, &format!("/createitem {AMMO} {AMMO_KEEP}"));
        gm(&mut c, &format!("/createitem {AMMO} {SURPLUS}"));
        assert_eq!(
            count(&mut c, AMMO),
            AMMO_KEEP + SURPLUS,
            "the setup made rounds"
        );
        let bid = c.market_orders(SHIELD).expect("the book reads");
        assert!(
            bid.bids
                .iter()
                .any(|o| o.is_npc() && o.station_id == station),
            "the fixture NPC bid: {bid:?}"
        );

        let sale = sell_loot(&mut c).expect("the sale runs");
        let note = sale.summary();
        assert!(sale.sold, "{note}");
        assert_eq!(
            sale.plan.bids.iter().map(|l| l.type_id).collect::<Vec<_>>(),
            vec![SHIELD],
            "{note}"
        );
        assert_eq!(count(&mut c, SHIELD), 0, "{note}");
        assert!(sale.after > sale.before, "{note}");

        assert_eq!(count(&mut c, AMMO), AMMO_KEEP, "{note}");
        let mine = c.my_orders().expect("my orders read");
        let ask = |t: i64| mine.iter().find(|o| o.type_id == t && !o.bid).copied();
        assert_eq!(ask(AMMO).map(|o| o.vol_remaining), Some(SURPLUS), "{note}");
        assert_eq!(ask(CANNON).map(|o| o.vol_remaining), Some(2), "{note}");
        assert_eq!(
            ask(CANNON).map(|o| o.price),
            crate::pricing::ask(&Quote::at_ref(CANNON_REF)),
            "{note}"
        );
        assert_eq!(sale.listed.len(), 2, "{note}");

        assert!(ask(RIG).is_none(), "{note}");
        assert_eq!(sale.plan.reprocess.len(), 1, "{note}");
        assert!(
            !sale.refined.is_empty() || sale.refused.iter().any(|r| r.starts_with("refine")),
            "{note}"
        );

        // A second sale finds nothing new to do: the asks stand.
        let again = sell_loot(&mut c).expect("the sale runs");
        assert!(
            again.plan.bids.is_empty() && again.plan.lists.is_empty(),
            "{}",
            again.summary()
        );
    }
}
