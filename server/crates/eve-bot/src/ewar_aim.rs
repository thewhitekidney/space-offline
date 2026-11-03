//! **Aim for the ewar and tackle modules** (BP-EWAR-AIM, issue #121, L4 of
//! `handback/r-bot-pvp/report.md`). Until now every fitted module with a
//! target effect was a "gun" and the escort fired all of them at the one
//! primary, in or out of reach, and a scrambler shortened the orbit. Now the
//! driver reads each module's kind from its effect id
//! ([`GunKind::of_effect`]) and its reach ([`GunInfo::reach`]); the escort
//! fires only [`GunKind::Weapon`]s itself and asks this module for the rest.
//!
//! - **Default**: the primary, so a ship with no role data fights as before.
//! - **Point, scram, web**: a foe nobody holds yet; the primary first, then
//!   the nearest unheld foe in reach; the primary again when every foe is
//!   held. "Held" counts this ship's own running modules of the same class
//!   (a point and a scram are one class) and the ones assigned this turn.
//! - **ECM, neut**: enemy logistics by hull group
//!   ([`crate::hunt::Ground::is_logi`]), else the primary.
//! - **Tracking disruptor**: a foe whose hardpoints are turrets
//!   ([`crate::hunt::Ground::arm_of`]); **guidance disruptor**: missiles; a
//!   foe of the other arm is never chosen, an unknown one is the fallback.
//! - **Reach**: a module is never fired at a foe past its reach; a running
//!   one is stopped when its target is past [`STOP_SHARE`] of it.
//! - **Locks**: an extra target is locked only while the locks held and
//!   pending are under `o.lock_max`, one lock a turn, and only in lock range.
//!
//! Every number here is **[A]** policy and none is in a test by value.
//! Falsifiers: a web fired at a foe it cannot reach (the reach reads short:
//! lower [`STOP_SHARE`]); a bot that flips a module between two foes (the
//! hold test is unstable: widen [`STOP_SHARE`]); hybrid or drone hulls read
//! as the wrong arm (hardpoints say nothing of drones).

use crate::brain::{
    Act, Ball, EwarKind, GunInfo, GunKind, Memory, Observation, ParkView, RANGE_MARGIN,
};
use crate::hunt::Arm;

/// A running module is stopped once its target is past this share of its
/// reach **[A]**: the foe moves, and a cycle at the edge still works.
pub const STOP_SHARE: f64 = 1.25;
/// The lock slots assumed when the hull's are not read **[A]**.
pub const LOCK_MAX_DEFAULT: usize = 3;

/// One enemy the modules may be aimed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Foe {
    pub ball: Ball,
    /// A logistics hull.
    pub logi: bool,
    pub arm: Arm,
}

/// The hold class of a kind: a point and a scrambler hold the same thing.
fn class(k: GunKind) -> GunKind {
    match k {
        GunKind::Scram => GunKind::Point,
        k => k,
    }
}

/// What to do with the non-weapon modules this turn (pure): `(gun, target)`
/// for the modules to start, and the guns to stop.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub fire: Vec<(i64, i64)>,
    pub stop: Vec<i64>,
}

/// **The aim** (pure): which target each ready non-weapon module takes and
/// which running ones to stop. `foes` holds the primary.
pub fn ewar_shots(guns: &[GunInfo], foes: &[Foe], primary: i64, ego: &Ball) -> Plan {
    let by_id = |id: i64| foes.iter().find(|f| f.ball.id == id);
    let gap = |f: &Foe| ego.surface_to(&f.ball);
    let mut plan = Plan::default();
    // What each class already holds: running modules whose target stays.
    let mut held: Vec<(GunKind, i64)> = Vec::new();
    for g in guns
        .iter()
        .filter(|g| g.kind != GunKind::Weapon && g.online && g.active)
    {
        match g.target.and_then(by_id) {
            Some(f) if g.reach <= 0.0 || gap(f) <= g.reach * STOP_SHARE => {
                held.push((class(g.kind), f.ball.id));
            }
            _ => plan.stop.push(g.item_id),
        }
    }
    for g in guns
        .iter()
        .filter(|g| g.kind != GunKind::Weapon && g.online && !g.active)
    {
        let reaches = |f: &&Foe| g.reach <= 0.0 || gap(f) <= g.reach;
        let in_reach: Vec<&Foe> = {
            let mut v: Vec<&Foe> = foes.iter().filter(reaches).collect();
            v.sort_by(|a, b| gap(a).total_cmp(&gap(b)).then(a.ball.id.cmp(&b.ball.id)));
            v
        };
        let primary_in = in_reach.iter().find(|f| f.ball.id == primary).copied();
        let is_held = |id: i64| held.iter().any(|(c, t)| *c == class(g.kind) && *t == id);
        let pick: Option<&Foe> = match g.kind {
            GunKind::Weapon => None,
            GunKind::Point | GunKind::Scram | GunKind::Web => primary_in
                .filter(|f| !is_held(f.ball.id))
                .or_else(|| in_reach.iter().find(|f| !is_held(f.ball.id)).copied())
                .or(primary_in),
            GunKind::Neut | GunKind::Ewar(EwarKind::Ecm) => in_reach
                .iter()
                .filter(|f| f.logi)
                .find(|f| !is_held(f.ball.id))
                .or_else(|| in_reach.iter().find(|f| f.logi))
                .copied()
                .or(primary_in),
            GunKind::Ewar(k @ (EwarKind::Track | EwarKind::Guide)) => {
                let (want, not) = if k == EwarKind::Track {
                    (Arm::Turrets, Arm::Missiles)
                } else {
                    (Arm::Missiles, Arm::Turrets)
                };
                let wanted = |f: &&&Foe| f.arm == want;
                primary_in
                    .filter(|f| f.arm == want && !is_held(f.ball.id))
                    .or_else(|| {
                        in_reach
                            .iter()
                            .filter(wanted)
                            .find(|f| !is_held(f.ball.id))
                            .copied()
                    })
                    .or_else(|| in_reach.iter().find(|f| f.arm == want).copied())
                    .or(primary_in.filter(|f| f.arm != not))
            }
            GunKind::Ewar(EwarKind::Damp | EwarKind::Paint) => primary_in,
        };
        if let Some(f) = pick {
            held.push((class(g.kind), f.ball.id));
            plan.fire.push((g.item_id, f.ball.id));
        }
    }
    plan
}

/// **The acts** for the non-weapon modules (pure but for `ceasing`): stops
/// first, a start at a target that is locked, otherwise at most one lock
/// toward the targets the modules want.
pub fn ewar_acts(
    ceasing: &mut Vec<i64>,
    o: &Observation,
    foes: &[Foe],
    primary: i64,
    ego: &Ball,
) -> Vec<Act> {
    let plan = ewar_shots(&o.guns, foes, primary, ego);
    let mut acts = Vec::new();
    for gun in plan.stop {
        if !ceasing.contains(&gun) {
            ceasing.push(gun);
            acts.push(Act::CeaseFire(gun));
        }
    }
    let mut wanted: Vec<i64> = Vec::new();
    for (gun, target) in plan.fire {
        if o.locked.contains(&target) {
            acts.push(Act::Fire { gun, target });
        } else if !wanted.contains(&target) {
            wanted.push(target);
        }
    }
    let slots = if o.lock_max == 0 {
        LOCK_MAX_DEFAULT
    } else {
        o.lock_max
    };
    if o.locked.len() + o.pending.len() < slots
        && let Some(t) = wanted.into_iter().find(|t| !o.pending.contains(t))
        && foes
            .iter()
            .find(|f| f.ball.id == t)
            .is_some_and(|f| ego.surface_to(&f.ball) <= o.lock_range * RANGE_MARGIN)
    {
        acts.push(Act::Lock(t));
    }
    acts
}

/// **The enemies** of this turn (pure): for a hunter the hostile pilots on
/// the grid that the bot may lawfully shoot (hunting ground, else a war
/// target), with hull group and hardpoints from its doctrine; for the rest
/// the park's hostiles. `primary` is always among them.
pub fn foes(m: &Memory, o: &Observation, park: &ParkView, primary: &Ball) -> Vec<Foe> {
    let mut out: Vec<Foe> = match &m.policy.pvp {
        Some(doc) => {
            let ground = matches!(o.place, crate::brain::Place::InSpace { system, .. }
                if doc.is_ground(system));
            o.hunt
                .pilots
                .iter()
                .filter(|p| p.hostile && (ground || p.war))
                .map(|p| Foe {
                    ball: p.ball,
                    logi: doc.is_logi(p.type_id),
                    arm: doc.arm_of(p.type_id),
                })
                .collect()
        }
        None => park
            .hostiles
            .iter()
            .map(|h| Foe {
                ball: h.ball,
                logi: false,
                arm: Arm::Unknown,
            })
            .collect(),
    };
    if !out.iter().any(|f| f.ball.id == primary.id) {
        out.push(Foe {
            ball: *primary,
            logi: false,
            arm: Arm::Unknown,
        });
    }
    out
}

/// The ids the hunter's last line of defence lets an ewar module touch:
/// [`foes`] without the primary (which `guard` knows).
pub fn legal_ids(m: &Memory, o: &Observation) -> Vec<i64> {
    let Some(doc) = &m.policy.pvp else {
        return Vec::new();
    };
    let ground = matches!(o.place, crate::brain::Place::InSpace { system, .. }
        if doc.is_ground(system));
    o.hunt
        .pilots
        .iter()
        .filter(|p| p.hostile && (ground || p.war))
        .map(|p| p.ball.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Policy;
    use crate::hunt::Ground;
    use std::time::{Duration, Instant};

    const EGO: i64 = 1_000_001;
    const PRIMARY: i64 = 1_000_011;
    const LOGI: i64 = 1_000_012;
    const OTHER: i64 = 1_000_013;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 0.0,
        }
    }

    fn gun(item_id: i64, kind: GunKind, reach: f64) -> GunInfo {
        GunInfo {
            item_id,
            type_id: 1_000_900 + item_id,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: None,
            capacity: 0,
            range: reach,
            reload: Duration::ZERO,
            kind,
            reach,
        }
    }

    fn foe(id: i64, x: f64, logi: bool, arm: Arm) -> Foe {
        Foe {
            ball: ball(id, x),
            logi,
            arm,
        }
    }

    fn ego() -> Ball {
        ball(EGO, 0.0)
    }

    #[test]
    fn the_effect_names_the_kind() {
        use eve_server::ewar;
        assert_eq!(GunKind::of_effect(ewar::WEB), GunKind::Web);
        assert_eq!(GunKind::of_effect(ewar::NEUT), GunKind::Neut);
        assert_eq!(
            GunKind::of_effect(ewar::TRACKING),
            GunKind::Ewar(EwarKind::Track)
        );
        assert_eq!(
            GunKind::of_effect(ewar::GUIDANCE),
            GunKind::Ewar(EwarKind::Guide)
        );
        // Anything else is a weapon, as before.
        assert_eq!(GunKind::of_effect(1_000_000), GunKind::Weapon);
        assert_eq!(GunKind::of_jamming("webify"), Some(GunKind::Web));
        assert_eq!(GunKind::of_jamming("unheard"), None);
    }

    #[test]
    fn ewar_defaults_to_the_primary() {
        let guns = [
            gun(1, GunKind::Ewar(EwarKind::Damp), 30_000.0),
            gun(2, GunKind::Ewar(EwarKind::Paint), 30_000.0),
            gun(3, GunKind::Web, 30_000.0),
        ];
        let foes = [foe(PRIMARY, 5_000.0, false, Arm::Unknown)];
        let plan = ewar_shots(&guns, &foes, PRIMARY, &ego());
        assert_eq!(plan.fire, vec![(1, PRIMARY), (2, PRIMARY), (3, PRIMARY)]);
        assert!(plan.stop.is_empty());
    }

    #[test]
    fn an_out_of_range_web_is_not_fired() {
        let guns = [gun(3, GunKind::Web, 10_000.0)];
        let foes = [foe(PRIMARY, 50_000.0, false, Arm::Unknown)];
        assert!(ewar_shots(&guns, &foes, PRIMARY, &ego()).fire.is_empty());
        let near = [foe(PRIMARY, 4_000.0, false, Arm::Unknown)];
        assert_eq!(ewar_shots(&guns, &near, PRIMARY, &ego()).fire.len(), 1);
    }

    #[test]
    fn a_running_module_is_stopped_when_the_target_is_far_past_its_reach() {
        let mut g = gun(3, GunKind::Web, 10_000.0);
        g.active = true;
        g.target = Some(PRIMARY);
        let far = [foe(PRIMARY, 500_000.0, false, Arm::Unknown)];
        assert_eq!(ewar_shots(&[g], &far, PRIMARY, &ego()).stop, vec![3]);
        let near = [foe(PRIMARY, 4_000.0, false, Arm::Unknown)];
        assert!(ewar_shots(&[g], &near, PRIMARY, &ego()).stop.is_empty());
    }

    #[test]
    fn a_second_point_takes_a_foe_not_yet_pointed() {
        let guns = [
            gun(1, GunKind::Scram, 20_000.0),
            gun(2, GunKind::Point, 20_000.0),
        ];
        let foes = [
            foe(PRIMARY, 5_000.0, false, Arm::Unknown),
            foe(OTHER, 8_000.0, false, Arm::Unknown),
        ];
        let plan = ewar_shots(&guns, &foes, PRIMARY, &ego());
        assert_eq!(plan.fire, vec![(1, PRIMARY), (2, OTHER)]);
    }

    #[test]
    fn ecm_and_neut_pick_the_logi_ship() {
        let guns = [
            gun(1, GunKind::Ewar(EwarKind::Ecm), 30_000.0),
            gun(2, GunKind::Neut, 30_000.0),
        ];
        let foes = [
            foe(PRIMARY, 5_000.0, false, Arm::Turrets),
            foe(LOGI, 9_000.0, true, Arm::Unknown),
        ];
        let plan = ewar_shots(&guns, &foes, PRIMARY, &ego());
        assert_eq!(plan.fire, vec![(1, LOGI), (2, LOGI)]);
        let no_logi = [foe(PRIMARY, 5_000.0, false, Arm::Turrets)];
        let plan = ewar_shots(&guns, &no_logi, PRIMARY, &ego());
        assert_eq!(plan.fire, vec![(1, PRIMARY), (2, PRIMARY)]);
    }

    #[test]
    fn a_tracking_disruptor_picks_a_turret_ship() {
        let guns = [
            gun(1, GunKind::Ewar(EwarKind::Track), 30_000.0),
            gun(2, GunKind::Ewar(EwarKind::Guide), 30_000.0),
        ];
        let foes = [
            foe(PRIMARY, 5_000.0, false, Arm::Missiles),
            foe(OTHER, 9_000.0, false, Arm::Turrets),
        ];
        let plan = ewar_shots(&guns, &foes, PRIMARY, &ego());
        assert_eq!(plan.fire, vec![(1, OTHER), (2, PRIMARY)]);
        // Only a missile ship in reach: the tracking disruptor stays dark.
        let only = [foe(PRIMARY, 5_000.0, false, Arm::Missiles)];
        let plan = ewar_shots(&guns[..1], &only, PRIMARY, &ego());
        assert!(plan.fire.is_empty());
        // An unknown arm is the fallback.
        let unknown = [foe(PRIMARY, 5_000.0, false, Arm::Unknown)];
        assert_eq!(
            ewar_shots(&guns[..1], &unknown, PRIMARY, &ego()).fire,
            vec![(1, PRIMARY)]
        );
    }

    fn obs(t: Instant) -> Observation {
        let mut o = Observation::empty(t);
        o.lock_range = 100_000.0;
        o
    }

    #[test]
    fn an_extra_target_is_locked_first_and_only_under_the_lock_slots() {
        let t = Instant::now();
        let mut o = obs(t);
        o.guns = vec![gun(1, GunKind::Ewar(EwarKind::Ecm), 30_000.0)];
        o.locked = vec![PRIMARY];
        o.lock_max = 3;
        let foes = [
            foe(PRIMARY, 5_000.0, false, Arm::Unknown),
            foe(LOGI, 9_000.0, true, Arm::Unknown),
        ];
        let mut ceasing = Vec::new();
        assert_eq!(
            ewar_acts(&mut ceasing, &o, &foes, PRIMARY, &ego()),
            vec![Act::Lock(LOGI)]
        );
        // Locked: the shot goes.
        o.locked.push(LOGI);
        assert_eq!(
            ewar_acts(&mut ceasing, &o, &foes, PRIMARY, &ego()),
            vec![Act::Fire {
                gun: 1,
                target: LOGI
            }]
        );
        // No free slot: nothing is locked.
        o.locked = vec![PRIMARY];
        o.lock_max = 1;
        assert!(ewar_acts(&mut ceasing, &o, &foes, PRIMARY, &ego()).is_empty());
    }

    #[test]
    fn the_foes_of_a_hunter_are_its_lawful_hostile_pilots() {
        let doc = std::sync::Arc::new(
            Ground::new()
                .with_security(7, 0.1)
                .with_fit(10, 832, 0, 0)
                .with_fit(11, 25, 1, 4),
        );
        assert!(doc.is_logi(Some(10)));
        assert!(!doc.is_logi(Some(11)));
        assert_eq!(doc.arm_of(Some(11)), Arm::Turrets);
        assert_eq!(doc.arm_of(Some(10)), Arm::Unknown);
        let mut policy = Policy::fast();
        policy.pvp = Some(doc);
        let m = Memory::new("Hunter1", policy);
        let t = Instant::now();
        let mut o = obs(t);
        o.place = crate::brain::Place::InSpace {
            system: 7,
            bound: true,
        };
        let pilot = |id: i64, ty: i64, hostile: bool| crate::hunt::Pilot {
            ball: ball(id, 1_000.0),
            char_id: id,
            type_id: Some(ty),
            who: eve_server::group_contacts::Affiliation {
                character_id: id,
                corporation_id: 1_000_700,
                alliance_id: None,
            },
            hostile,
            relation: 0.0,
            war: false,
        };
        o.hunt.pilots = vec![
            pilot(PRIMARY, 11, true),
            pilot(LOGI, 10, true),
            pilot(OTHER, 11, false),
        ];
        let park = ParkView {
            ego: ego(),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: Default::default(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        };
        let f = foes(&m, &o, &park, &ball(PRIMARY, 1_000.0));
        let ids: Vec<i64> = f.iter().map(|f| f.ball.id).collect();
        assert_eq!(ids, vec![PRIMARY, LOGI]);
        assert!(f[1].logi);
        assert_eq!(legal_ids(&m, &o), vec![PRIMARY, LOGI]);
    }
}
