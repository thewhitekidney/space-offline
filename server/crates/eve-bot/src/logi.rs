//! **The logi bot: remote repairs for the fleet** (K-LOGI, wave K). Lane
//! note `docs/lanes/k-logi.md`.
//!
//! A `logi` bot flies the escort's hull and rules ([`crate::escort`]) and, in
//! the one free high slot, a **remote armor repairer** it owns -- bought in
//! the closed economy by [`KIT`] (never GM-given there), made by GM only in
//! the open mode like every role's kit. On top of the escort's turn it adds
//! two things, and nothing else:
//!
//! - **Docked** ([`docked`]): fit a remote repairer from the hangar, online
//!   it, or -- closed economy, the Rifter and its guns what the pilot flies --
//!   buy one ([`crate::brain::acquire_if_due`]), as the ratter does for its
//!   salvager ([`crate::salvage::docked`]).
//! - **In space** ([`in_space`]): [`plan`] runs **before** the escort's own
//!   turn and its acts go ahead of it. It keeps the **wards** up, **lowest
//!   health first**: every ward that needs what a fitted repairer gives, and
//!   is within its range, is locked and then helped; a repairer whose ward
//!   is whole again or out of reach is stopped. The wards are the fleet boss
//!   and the other bots in the park (the marker's pilots) **[A]**: the only
//!   fleet members a bot knows (`FleetView::boss`; see `driver::is_human`).
//!
//! **What a bot can see of a ward's health** is its bars: the park's
//! `OnDamageStateChange` for every ship in it ([`Scene::bars`], filled by the
//! driver as it fills `Observation::health` for the bot's own ship). A ward
//! whose ship has no bars was never hit and needs nothing.
//!
//! **The safety is the server's** (`crimewatch::assist_refusal`): a ward who
//! has become a criminal or suspect is refused with
//! `ModuleActivationDeniedSafetyPrevents*`, and is let alone for
//! [`REFUSED_FOR`] ([`Scene::refused`]).
//!
//! Everything here is **[A] policy** except the group, effect and attribute
//! ids, which are [V-data]; no number from here appears in a test as an
//! expected value.

use crate::acquire::Kit;
use crate::brain::{Act, Ball, Health, Memory, Observation, ParkView, RANGE_MARGIN, RoleRef};
use crate::fleet_orders::{HealCall, HealKind};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::{Conn, Error, ItemRow, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// Small Remote Armor Repairer I **[V-data]** (group 325, a high-slot module,
/// requires Remote Armor Repair Systems I).
pub const REMOTE_ARMOR_I: i64 = 11_355;
/// Remote Armor Repair Systems **[V-data]**: the skill the module asks for.
pub const SKILL_REMOTE_ARMOR: i64 = 16_069;
/// Remote repairers a logi fits: the Rifter's third high slot, beside the
/// escort's two guns. **[A]**: falsified by a logi that cannot fit it.
pub const WANT_REPS: usize = 1;

/// `activate`'s `repeat`: until stopped (the client sends 1000, `smb:1565`).
pub const REPEAT: i64 = 1_000;

/// Ships whose bars are kept at once: a park holds a few hundred balls, and
/// a bar for each is a few bytes. **[A]**
pub const MAX_BARS: usize = 512;

/// A ward is let alone this long after the safety refused it. **[A]**
pub const REFUSED_FOR: Duration = Duration::from_secs(5 * 60);

/// A ward whose shield is under this share is worth a booster's cycle; the
/// shield regenerates, so a little missing is not. **[A]**
pub const SHIELD_NEED: f64 = 0.7;
/// Armour or hull above this share is whole. **[A]**
pub const WHOLE: f64 = 0.99;
/// The bar a mate's Heal call assumes when his bars are unknown or look
/// whole **[A]**: low enough that every repairer takes him. Falsified by a
/// logi that wastes cycles on a mate who pressed Heal by mistake.
pub const CALLED_BAR: f64 = 0.5;

/// What a remote module restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Armour,
    Shield,
    Hull,
    Capacitor,
}

/// A group of remote modules, its `(name, id)` effect and what it restores
/// **[V-data]** (`typedata.tsv`: the group of each type and its default
/// effect): the module's effect is the one `dogmaIM.Activate` names.
const GROUPS: [(i64, &str, i64, Kind); 4] = [
    (325, "shipModuleRemoteArmorRepairer", 6188, Kind::Armour),
    (41, "shipModuleRemoteShieldBooster", 6186, Kind::Shield),
    (585, "shipModuleRemoteHullRepairer", 6185, Kind::Hull),
    (
        67,
        "shipModuleRemoteCapacitorTransmitter",
        6184,
        Kind::Capacitor,
    ),
];

/// The row of a module group, `None` for any other.
fn row_of(group_id: i64) -> Option<(&'static str, i64, Kind)> {
    GROUPS
        .iter()
        .find(|g| g.0 == group_id)
        .map(|g| (g.1, g.2, g.3))
}

/// Whether `row` is a remote repair, boost or transfer module.
pub fn is_remote(row: &ItemRow) -> bool {
    row_of(row.group_id).is_some()
}

/// One fitted remote module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rep {
    pub item_id: i64,
    pub type_id: i64,
    pub flag_id: i64,
    pub online: bool,
    pub active: bool,
    pub target: Option<i64>,
    /// `maxRange` (attribute 54), metres, from the module's own dogma row.
    pub range: f64,
    pub kind: Kind,
    /// The effect, as `dogmaIM.Activate` names it.
    pub effect: (&'static str, i64),
}

/// What the logi sees beyond the park: its repairers, the ships whose bars
/// it has been told, and the pilots it may help.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub reps: Vec<Rep>,
    /// Every ship's last `OnDamageStateChange`, by ship id.
    pub bars: BTreeMap<i64, Health>,
    /// The other bots' ships in the park (the marker's pilots).
    pub mates: Vec<Ball>,
    /// Wards the safety refused, until when.
    pub refused: BTreeMap<i64, Instant>,
}

/// One act of the logi's own.
#[derive(Debug, Clone, PartialEq)]
pub enum LogiAct {
    /// Run `module`'s `effect` on the locked `target`.
    Assist {
        module: i64,
        effect: (&'static str, i64),
        target: i64,
    },
    /// Stop `module` at the end of its cycle.
    Stop {
        module: i64,
        effect: (&'static str, i64),
    },
}

impl LogiAct {
    /// The act as a log line and the ids it names.
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            LogiAct::Assist {
                module,
                effect,
                target,
            } => (
                format!("Activate {module} {} -> {target}", effect.0),
                vec![*module, *target],
            ),
            LogiAct::Stop { module, effect } => {
                (format!("Deactivate {module} {}", effect.0), vec![*module])
            }
        }
    }
}

/// **The logi's kit**: the ratter's guns and skill, and the remote armor
/// repairer with its skill.
pub const KIT: Kit = Kit {
    hull: crate::acquire::FIGHTER_KIT.hull,
    modules: crate::acquire::FIGHTER_KIT.modules,
    ammo: crate::acquire::FIGHTER_KIT.ammo,
    skills: &[(crate::escort::SKILL_GUNNERY, 1), (SKILL_REMOTE_ARMOR, 1)],
    more: &[(REMOTE_ARMOR_I, WANT_REPS as i64)],
};

/// The role: the escort's, with the logi's two hooks.
pub static ROLE: RoleSpec = RoleSpec {
    name: "logi",
    greeting: "o7 -- repairing for you, stay close",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// Whether `role` is the logi's row.
pub fn is_logi(role: RoleRef) -> bool {
    std::ptr::eq(role, &ROLE) || std::ptr::eq(role, &crate::wing_fight::LOGI_ROLE)
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// GM setup, **open mode only** (the closed economy acquires [`KIT`]): the
/// escort's, and the remote repairer with its skill.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = (crate::roles::ESCORT.setup)(lacks);
    lines.push(format!("/giveskill {SKILL_REMOTE_ARMOR} 1"));
    if lacks.module {
        lines.push(format!("/createitem {REMOTE_ARMOR_I} {WANT_REPS}"));
    }
    lines
}

// ---------------------------------------------------------------------------
// Seeing
// ---------------------------------------------------------------------------

/// **The fitted remote modules**, read as godma reads a module
/// (`ItemGetInfo`): online, running, `maxRange`. `rows` are the active
/// ship's items; `before` the last read, so a module's target survives while
/// it runs.
pub fn read(conn: &mut Conn, rows: &[ItemRow], before: &[Rep]) -> Result<Vec<Rep>> {
    let mut reps = Vec::new();
    for row in rows.iter().filter(|r| r.is_fitted()) {
        let Some((name, id, kind)) = row_of(row.group_id) else {
            continue;
        };
        let info = conn.module_row(row.item_id)?;
        let active = info.effects.contains(&id);
        let target = before
            .iter()
            .find(|r| r.item_id == row.item_id)
            .and_then(|r| r.target)
            .filter(|_| active);
        reps.push(Rep {
            item_id: row.item_id,
            type_id: row.type_id,
            flag_id: row.flag_id,
            online: info.effects.contains(&crate::EFFECT_ONLINE),
            active,
            target,
            range: info.attribute(54).unwrap_or(0.0),
            kind,
            effect: (name, id),
        });
    }
    Ok(reps)
}

/// **[`LogiAct`]**: the activation or the stop, and what it told the
/// scene. A refusal is the server's key; the safety's keeps the ward off
/// for [`REFUSED_FOR`], a lost lock is dropped from `locked`.
pub fn execute(
    conn: &mut Conn,
    op: &LogiAct,
    scene: &mut Scene,
    locked: &mut Vec<i64>,
    now: Instant,
) -> Result<()> {
    match op {
        LogiAct::Assist {
            module,
            effect,
            target,
        } => match conn.activate(*module, *effect, Some(*target), REPEAT) {
            Ok(_) => {
                if let Some(r) = scene.reps.iter_mut().find(|r| r.item_id == *module) {
                    r.active = true;
                    r.target = Some(*target);
                }
                Ok(())
            }
            Err(Error::Refused(why)) => {
                if why.contains("DeniedActivateTargetNotPresent") {
                    locked.retain(|t| t != target);
                }
                if why.contains("SafetyPrevents") {
                    scene.refused.insert(*target, now + REFUSED_FOR);
                }
                Err(Error::Refused(why))
            }
            Err(e) => Err(e),
        },
        LogiAct::Stop { module, effect } => {
            conn.deactivate(*module, effect.0)?;
            if let Some(r) = scene.reps.iter_mut().find(|r| r.item_id == *module) {
                r.active = false;
                r.target = None;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Deciding
// ---------------------------------------------------------------------------

/// Whether a module of `kind` has something to do for a ward with `h`.
pub fn needs(kind: Kind, h: &Health) -> bool {
    match kind {
        Kind::Shield => h.shield < SHIELD_NEED,
        Kind::Armour => h.armour < WHOLE,
        Kind::Hull => h.hull < WHOLE,
        // Nobody's capacitor is seen: nothing to read it from.
        Kind::Capacitor => false,
    }
}

/// **Lowest health first**: the weaker of armour and hull, then the shield.
/// The key sorts ascending.
pub fn urgency(h: &Health) -> (f64, f64) {
    (h.armour.min(h.hull), h.shield)
}

/// The ships the logi keeps up: the boss and the other bots in the park,
/// each with its last bars (none: never hit). A ship running an Entosis Link
/// is left out (FC-2): EVE gives no remote assistance to a linking ship
/// **[A]**, falsified if the server lets reps land on one (then this only
/// saves the cycles).
fn wards<'a>(
    o: &'a Observation,
    park: &'a ParkView,
    called: Option<HealCall>,
) -> Vec<(Ball, Health)> {
    let mut out: Vec<(Ball, Health)> = Vec::new();
    for b in park.boss.iter().chain(o.logi.mates.iter()) {
        if b.id == park.ego.id
            || o.wing.links.contains_key(&b.id)
            || out.iter().any(|(x, _)| x.id == b.id)
        {
            continue;
        }
        if let Some(h) = o.logi.bars.get(&b.id) {
            out.push((*b, *h));
        }
    }
    out.sort_by(|a, b| {
        let (ka, kb) = (urgency(&a.1), urgency(&b.1));
        ka.0.total_cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(a.0.id.cmp(&b.0.id))
    });
    // BOT-FLEET-ORDERS: a mate who pressed Heal Armor / Heal Shield is
    // served first, whatever his bars say (he may be hit and unheard). A
    // ship whose bars are unknown is taken as hurt in the bar he named.
    if let Some(call) = called
        && let Some(ball) = park
            .boss
            .iter()
            .chain(o.logi.mates.iter())
            .find(|b| b.id == call.ship && b.id != park.ego.id)
    {
        let mut h = o.logi.bars.get(&call.ship).copied().unwrap_or(Health {
            shield: 1.0,
            armour: 1.0,
            hull: 1.0,
        });
        match call.kind {
            HealKind::Shield => h.shield = h.shield.min(CALLED_BAR),
            HealKind::Armor => h.armour = h.armour.min(CALLED_BAR),
            // The sender's ship needs whichever bar is lowest; if none is
            // known to be hurt, armour [A].
            HealKind::Target => {
                if h.shield >= SHIELD_NEED && h.armour >= WHOLE {
                    h.armour = CALLED_BAR;
                }
            }
            // Capacitor transmitters are not fitted by the logi's kit.
            HealKind::Capacitor => return out,
        }
        out.retain(|(b, _)| b.id != call.ship);
        out.insert(0, (*ball, h));
    }
    out
}

/// **The logi's turn in space**, to go ahead of the escort's: for each
/// fitted remote module, in order,
///
/// 1. running: stopped when its ward needs nothing more of it or has left
///    its reach;
/// 2. idle: the **lowest-health** ward that needs what it gives, is in its
///    range and is not already being helped by a module of the same kind
///    -- locked first, then helped.
///
/// Empty when nothing is fitted, nothing is hurt or nobody is in reach.
pub fn plan(o: &Observation, park: &ParkView) -> Vec<Act> {
    plan_called(o, park, None)
}

/// [`plan`] with a mate's call for reps in force: his ship goes first.
pub fn plan_called(o: &Observation, park: &ParkView, called: Option<HealCall>) -> Vec<Act> {
    let reps: Vec<&Rep> = o
        .logi
        .reps
        .iter()
        .filter(|r| r.online && r.kind != Kind::Capacitor)
        .collect();
    if reps.is_empty() {
        return Vec::new();
    }
    let ego = park.ego;
    let wards = wards(o, park, called);
    let mut acts = Vec::new();
    // What is being helped now: `(kind, ward)`.
    let mut served: BTreeSet<(u8, i64)> = reps
        .iter()
        .filter(|r| r.active)
        .filter_map(|r| Some((r.kind as u8, r.target?)))
        .collect();
    for rep in reps {
        let reach = rep.range * RANGE_MARGIN;
        if rep.active {
            let keep = rep.target.is_some_and(|t| {
                wards
                    .iter()
                    .any(|(b, h)| b.id == t && needs(rep.kind, h) && ego.surface_to(b) <= reach)
            });
            if !keep {
                if let Some(t) = rep.target {
                    served.remove(&(rep.kind as u8, t));
                }
                acts.push(Act::Logi(LogiAct::Stop {
                    module: rep.item_id,
                    effect: rep.effect,
                }));
            }
            continue;
        }
        let pick = wards.iter().find(|(b, h)| {
            needs(rep.kind, h)
                && !served.contains(&(rep.kind as u8, b.id))
                && ego.surface_to(b) <= reach
                && o.logi
                    .refused
                    .get(&b.id)
                    .is_none_or(|until| o.now >= *until)
        });
        let Some((b, _)) = pick else { continue };
        if o.locked.contains(&b.id) {
            served.insert((rep.kind as u8, b.id));
            acts.push(Act::Logi(LogiAct::Assist {
                module: rep.item_id,
                effect: rep.effect,
                target: b.id,
            }));
        } else if !o.pending.contains(&b.id)
            && !acts.iter().any(|a| matches!(a, Act::Lock(_)))
            && ego.surface_to(b) <= o.lock_range * RANGE_MARGIN
        {
            acts.push(Act::Lock(b.id));
        }
    }
    acts
}

/// **In space**: [`plan`]'s acts, then the escort's turn. A logi heading home
/// (hurt, out of ammo, recovering) helps nobody on the way.
pub fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let turn = crate::escort::in_space(m, o);
    let Some(park) = &o.park else { return turn };
    if m.homing.is_some() {
        return turn;
    }
    let called = m.heal.filter(|c| o.now < c.until);
    let mut acts = plan_called(o, park, called);
    if acts.is_empty() {
        return turn;
    }
    // The escort's own acts go on: a lock for the ward does not replace a
    // fight, and a wait is what the escort would have done.
    acts.extend(turn);
    acts
}

/// **Docked**: a remote repairer in the hangar and none fitted is fitted;
/// fitted and offline, onlined; none anywhere, the closed economy and the
/// Rifter with its guns what the pilot flies, bought. Then the escort's own
/// docked rules.
pub fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if let Some(acts) = fit(m, o) {
        return acts;
    }
    crate::escort::docked(m, o)
}

/// [`docked`]'s own rules; `None` leaves the turn to the escort.
pub fn fit(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    let flying = o.ship.and_then(|s| s.type_id);
    if !flying.is_some_and(crate::hull_choice::is_combat_hull) {
        return None;
    }
    if let Some(rep) = o.logi.reps.iter().find(|r| !r.online) {
        return Some(vec![Act::Online(rep.item_id)]);
    }
    let fitted = o
        .logi
        .reps
        .iter()
        .filter(|r| r.kind != Kind::Capacitor)
        .count();
    if fitted >= WANT_REPS {
        return None;
    }
    if let Some(row) = o.hangar.iter().find(|r| is_remote(r)) {
        return (o.failed(crate::brain::ActKind::FitAuto) == 0)
            .then(|| vec![Act::FitAuto(row.item_id)]);
    }
    // Nothing to fit: buy one, once the guns are what the pilot flies.
    if m.policy.closed && !o.guns.is_empty() {
        return crate::brain::acquire_if_due(m, false, true, false);
    }
    // Open mode: the role's GM setup made one with the guns.
    None
}

#[cfg(test)]
mod tests {
    //! Invented ids; asserts the kind and order of acts and the ids they
    //! name, never a range, a count or a delay.
    use super::*;
    use crate::brain::{ActKind, Place, Policy, ShipInfo};

    const STATION: i64 = 60_012_526;
    const SHIP: i64 = 100_000_501;
    const REP_A: i64 = 100_000_502;
    const REP_B: i64 = 100_000_503;
    const BOSS: i64 = 100_000_504;
    const MATE: i64 = 100_000_505;
    const FAR: i64 = 100_000_506;
    /// An invented remote repairer type: the rule is by group, not by id.
    const REP_TYPE: i64 = 990_501;
    const RANGE: f64 = 6_000.0;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 20.0,
        }
    }

    fn hurt(armour: f64) -> Health {
        Health {
            shield: 1.0,
            armour,
            hull: 1.0,
        }
    }

    fn rep(item_id: i64, kind: Kind, online: bool, active: bool, target: Option<i64>) -> Rep {
        let (_, id, _) = GROUPS
            .iter()
            .find(|g| g.3 == kind)
            .map(|g| (g.1, g.2, g.3))
            .unwrap();
        let name = GROUPS.iter().find(|g| g.3 == kind).map(|g| g.1).unwrap();
        Rep {
            item_id,
            type_id: REP_TYPE,
            flag_id: 27,
            online,
            active,
            target,
            range: RANGE,
            kind,
            effect: (name, id),
        }
    }

    fn logi() -> Memory {
        Memory::new(
            "Logi1",
            Policy {
                role: &ROLE,
                ..Policy::fast()
            },
        )
    }

    fn obs() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::escort::RIFTER),
        });
        o.lock_range = 50_000.0;
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: Some(ball(BOSS, 1_000.0)),
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.guns = vec![crate::brain::GunInfo {
            item_id: 100_000_509,
            type_id: crate::escort::GUN,
            flag_id: 28,
            online: true,
            active: false,
            target: None,
            rounds: Some(1),
            capacity: 1,
            range: 1.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.logi.mates = vec![ball(MATE, 2_000.0), ball(FAR, 500_000.0)];
        o.logi.reps = vec![rep(REP_A, Kind::Armour, true, false, None)];
        o
    }

    fn assist(o: &Observation) -> Vec<Act> {
        plan(o, o.park.as_ref().expect("a park"))
    }

    fn is_assist(a: &Act, module: i64, target: i64) -> bool {
        matches!(a, Act::Logi(LogiAct::Assist { module: m, target: t, .. }) if *m == module && *t == target)
    }

    /// BOT-FLEET-ORDERS: a mate's Heal Armor call puts his ship first, even
    /// when a worse-hurt ward is in reach, and a Capacitor call is no call
    /// for a repairer.
    #[test]
    fn a_mates_heal_call_puts_that_ship_first() {
        use crate::fleet_orders::{HealCall, HealKind};
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.2));
        o.logi.bars.insert(MATE, hurt(0.9));
        let park = o.park.clone().expect("a park");
        // Without a call the worse-hurt boss is first.
        let acts = plan(&o, &park);
        assert!(matches!(acts[..], [Act::Lock(t)] if t == BOSS), "{acts:?}");
        let call = |kind| {
            Some(HealCall {
                ship: MATE,
                kind,
                until: o.now + Duration::from_secs(30),
            })
        };
        let acts = plan_called(&o, &park, call(HealKind::Armor));
        assert!(matches!(acts[..], [Act::Lock(t)] if t == MATE), "{acts:?}");
        // Locked, the repairer runs on him.
        o.locked = vec![MATE];
        let acts = plan_called(&o, &park, call(HealKind::Armor));
        assert!(is_assist(&acts[0], REP_A, MATE), "{acts:?}");
        // A mate with no bars heard is taken as hurt in the bar he named.
        let mut o2 = obs();
        o2.locked = vec![MATE];
        let acts = plan_called(&o2, &park, call(HealKind::Armor));
        assert!(is_assist(&acts[0], REP_A, MATE), "{acts:?}");
        // Capacitor: this kit has no transmitter, so nothing is reordered.
        let acts = plan_called(&o2, &park, call(HealKind::Capacitor));
        assert!(acts.is_empty(), "{acts:?}");
    }

    /// FC-2: the wards are the FC and the mates, minus any ship that is
    /// linking: a hurt linking mate is left alone, the next hurt ward is not.
    #[test]
    fn a_linking_ship_is_no_ward() {
        let mut o = obs();
        o.logi.bars.insert(MATE, hurt(0.2));
        o.logi.bars.insert(BOSS, hurt(0.6));
        o.wing.links.insert(MATE, 100_000_700);
        o.locked = vec![BOSS, MATE];
        let acts = assist(&o);
        assert!(
            acts.iter().any(|a| is_assist(a, REP_A, BOSS)),
            "the boss is still helped: {acts:?}"
        );
        assert!(!acts.iter().any(|a| matches!(
            a,
            Act::Logi(LogiAct::Assist { target, .. }) if *target == MATE
        )));
        // When he stops linking he is the lowest ward again.
        o.wing.links.clear();
        assert!(assist(&o).iter().any(|a| is_assist(a, REP_A, MATE)));
    }

    #[test]
    fn nobody_hurt_means_nothing_to_do() {
        let mut o = obs();
        assert!(assist(&o).is_empty(), "no bars: nobody was hit");
        o.logi.bars.insert(BOSS, hurt(1.0));
        assert!(assist(&o).is_empty(), "bars at full");
    }

    #[test]
    fn a_hurt_ward_is_locked_and_then_helped() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.5));
        assert_eq!(assist(&o), vec![Act::Lock(BOSS)]);
        o.pending = vec![BOSS];
        assert!(assist(&o).is_empty(), "the lock is on its way");
        o.pending.clear();
        o.locked = vec![BOSS];
        let acts = assist(&o);
        assert_eq!(acts.len(), 1);
        assert!(is_assist(&acts[0], REP_A, BOSS), "{acts:?}");
    }

    #[test]
    fn the_lowest_health_ward_comes_first() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.6));
        o.logi.bars.insert(MATE, hurt(0.2));
        o.locked = vec![BOSS, MATE];
        let acts = assist(&o);
        assert!(is_assist(&acts[0], REP_A, MATE), "{acts:?}");
        // Hull counts with the armour: the weaker of the two is the key.
        o.logi.bars.insert(
            BOSS,
            Health {
                shield: 1.0,
                armour: 1.0,
                hull: 0.1,
            },
        );
        o.logi.reps = vec![
            rep(REP_A, Kind::Armour, true, false, None),
            rep(REP_B, Kind::Hull, true, false, None),
        ];
        let acts = assist(&o);
        assert!(is_assist(&acts[0], REP_A, MATE), "armour: the mate");
        assert!(is_assist(&acts[1], REP_B, BOSS), "hull: the boss");
    }

    #[test]
    fn two_modules_of_a_kind_spread_over_the_wards() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.6));
        o.logi.bars.insert(MATE, hurt(0.2));
        o.locked = vec![BOSS, MATE];
        o.logi.reps = vec![
            rep(REP_A, Kind::Armour, true, false, None),
            rep(REP_B, Kind::Armour, true, false, None),
        ];
        let acts = assist(&o);
        assert!(is_assist(&acts[0], REP_A, MATE), "{acts:?}");
        assert!(is_assist(&acts[1], REP_B, BOSS), "{acts:?}");
    }

    #[test]
    fn a_ward_out_of_reach_is_left_alone() {
        let mut o = obs();
        o.logi.bars.insert(FAR, hurt(0.1));
        o.locked = vec![FAR];
        assert!(assist(&o).is_empty());
    }

    #[test]
    fn a_repairer_stops_when_its_ward_is_whole_or_gone() {
        let mut o = obs();
        o.logi.reps = vec![rep(REP_A, Kind::Armour, true, true, Some(BOSS))];
        o.logi.bars.insert(BOSS, hurt(1.0));
        let acts = assist(&o);
        assert!(matches!(
            acts.as_slice(),
            [Act::Logi(LogiAct::Stop { module, .. })] if *module == REP_A
        ));
        // Still hurt and in reach: it runs on.
        o.logi.bars.insert(BOSS, hurt(0.4));
        assert!(assist(&o).is_empty());
        // Hurt but out of reach: it stops.
        o.park.as_mut().expect("a park").boss = Some(ball(BOSS, 400_000.0));
        assert_eq!(assist(&o).len(), 1);
    }

    #[test]
    fn a_shield_booster_wants_a_thin_shield_not_a_hurt_armour() {
        let mut o = obs();
        o.logi.reps = vec![rep(REP_A, Kind::Shield, true, false, None)];
        o.locked = vec![BOSS];
        o.logi.bars.insert(BOSS, hurt(0.3));
        assert!(assist(&o).is_empty(), "armour is not its business");
        o.logi.bars.insert(
            BOSS,
            Health {
                shield: 0.1,
                armour: 1.0,
                hull: 1.0,
            },
        );
        assert!(is_assist(&assist(&o)[0], REP_A, BOSS));
    }

    #[test]
    fn the_safetys_refusal_keeps_a_ward_off_for_a_while() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.3));
        o.locked = vec![BOSS];
        o.logi.refused.insert(BOSS, o.now + Duration::from_secs(60));
        assert!(assist(&o).is_empty());
        o.logi.refused.insert(BOSS, o.now);
        assert_eq!(assist(&o).len(), 1, "the time is up");
    }

    #[test]
    fn an_offline_or_capacitor_module_is_not_used() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.3));
        o.locked = vec![BOSS];
        o.logi.reps = vec![
            rep(REP_A, Kind::Armour, false, false, None),
            rep(REP_B, Kind::Capacitor, true, false, None),
        ];
        assert!(assist(&o).is_empty());
    }

    #[test]
    fn the_logis_turn_puts_the_help_ahead_of_the_escorts() {
        let mut o = obs();
        o.logi.bars.insert(BOSS, hurt(0.3));
        o.locked = vec![BOSS];
        let mut m = logi();
        let acts = in_space(&mut m, &o);
        assert!(is_assist(&acts[0], REP_A, BOSS), "{acts:?}");
        assert!(acts.len() > 1, "and the escort's turn follows");
        // Heading home: nobody is helped on the way.
        let mut m = logi();
        o.guns.clear();
        let acts = in_space(&mut m, &o);
        assert!(
            !acts.iter().any(|a| matches!(a, Act::Logi(_))),
            "an unarmed escort goes home: {acts:?}"
        );
    }

    fn row(item_id: i64, group_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id: REP_TYPE,
            owner_id: 90_000_001,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id,
            category_id: 7,
            singleton: false,
        }
    }

    fn docked_obs() -> Observation {
        let mut o = obs();
        o.place = Place::Docked { station: STATION };
        o.logi.reps.clear();
        o
    }

    #[test]
    fn a_remote_repairer_in_the_hangar_is_fitted_then_onlined() {
        let mut o = docked_obs();
        o.hangar = vec![row(REP_A, 325)];
        assert_eq!(fit(&mut logi(), &o), Some(vec![Act::FitAuto(REP_A)]));
        o.hangar.clear();
        o.logi.reps = vec![rep(REP_A, Kind::Armour, false, false, None)];
        assert_eq!(fit(&mut logi(), &o), Some(vec![Act::Online(REP_A)]));
        o.logi.reps = vec![rep(REP_A, Kind::Armour, true, false, None)];
        assert_eq!(fit(&mut logi(), &o), None, "fitted: the escort's turn");
    }

    #[test]
    fn a_failed_fit_is_not_retried_and_a_cargo_or_gun_is_no_repairer() {
        let mut o = docked_obs();
        o.hangar = vec![row(REP_A, 325), row(REP_B, 55)];
        o.failures.insert(ActKind::FitAuto, 1);
        assert_eq!(fit(&mut logi(), &o), None);
        o.failures.clear();
        o.hangar = vec![row(REP_B, 55)];
        assert!(!is_remote(&o.hangar[0]));
        assert_ne!(fit(&mut logi(), &o), Some(vec![Act::FitAuto(REP_B)]));
    }

    #[test]
    fn the_closed_economy_buys_one_and_never_asks_gm() {
        let mut o = docked_obs();
        let mut m = Memory::new(
            "Logi2",
            Policy {
                role: &ROLE,
                closed: true,
                ..Policy::fast()
            },
        );
        m.now = Some(o.now);
        let acts = fit(&mut m, &o).expect("a purchase");
        assert!(
            acts.iter().all(|a| !matches!(a, Act::GmSetup { .. })),
            "{acts:?}"
        );
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Acquire { module: true, .. })),
            "{acts:?}"
        );
        // Unarmed (or in a capsule): nothing yet.
        o.guns.clear();
        assert_eq!(fit(&mut m, &o), None);
    }

    #[test]
    fn the_roles_setup_and_kit_name_the_repairer_and_its_skill() {
        let lacks = Lacks {
            ship: false,
            module: true,
            ammo: false,
            isk: false,
        };
        let lines = setup(lacks);
        assert!(lines.contains(&format!("/giveskill {SKILL_REMOTE_ARMOR} 1")));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("/createitem {REMOTE_ARMOR_I}")))
        );
        assert!(KIT.skills.iter().any(|s| s.0 == SKILL_REMOTE_ARMOR));
        assert!(KIT.more.iter().any(|m| m.0 == REMOTE_ARMOR_I));
        assert!(is_logi(&ROLE) && !is_logi(&crate::roles::ESCORT));
    }

    #[test]
    fn every_remote_group_has_its_effect() {
        for (group, name, id, kind) in GROUPS {
            assert_eq!(row_of(group), Some((name, id, kind)));
            assert!(
                eve_server::remote_rep::is_remote(id),
                "{name}: the server runs it"
            );
        }
        assert_eq!(row_of(1), None);
    }
}
