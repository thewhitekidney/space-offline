//! **The ratter salvages** (X-SALV, `docs/lanes/x-salv.md`): a Salvager I
//! in the third high slot of the escort's Rifter, run on the wrecks it has
//! looted, and the salvage sold to the builders.
//!
//! The server side is `eve_server::salvage`: a `salvaging` cycle (2757
//! [V-data]) on a locked, emptied wreck within the module's `maxRange` rolls
//! a chance; on a success the wreck goes and its salvage lands in the hold.
//! A failure, or a refusal, ends the run with one of the client's own keys
//! (`SalvagingFailure`, `SalvagingPointlessLoot`, ...), so a bot reads its
//! outcome off the park: the wreck is gone or it is not.
//!
//! **Pure rules**, hooked into [`crate::escort`]:
//!
//! - [`docked`] (the escort's rule 6b, after guns and rounds): a Salvager in
//!   the hangar and none fitted -> fit it; fitted and offline -> online it;
//!   none anywhere, a closed economy and the pilot in the Rifter -> one
//!   [`Act::Acquire`] for [`RATTER_KIT`]'s salvager, at most once per
//!   [`crate::acquire::ACQUIRE_EVERY`] (`brain::acquire_if_due`).
//! - [`in_space`] (the escort's rule 5, after its loot rule has nothing
//!   left): the nearest wreck the ratter has looted and not given up on, if
//!   a Salvager is fitted, online and idle. Out of the module's range it
//!   closes in; in range it locks, then [`Act::Salvage`]s. An attempt is
//!   counted when it is sent (a refusal is incidental, `ActKind::is_incidental`,
//!   so only the count ends it); a wreck is let go after [`MAX_TRIES`]; a
//!   full hold salvages nothing.
//!
//! The sale needs no act of its own: [`crate::escort::is_loot`] counts
//! salvage (group [`GROUP_SALVAGE`]) as loot, so the ratter's `SellLoot`
//! lists it as an ask (and sells it into a bid when one is there). Salvage is
//! never refined (`escort::lots`: `whole` is false for it): it feeds rigs, by
//! the builders buying it (R-ECON O5-d), not by the refinery.
//!
//! **The rig rung is not added** (the packet left it to this lane): the
//! builder's ladder (`builder::BLUEPRINT_RUNGS`) climbs to blueprints a
//! station in reach sells, and every published rig blueprint takes salvage
//! (`content_salvage.py --check`), so a rig rung needs salvage on the book
//! first -- which this lane is what puts there. X-RIG, which needs X-SALV,
//! adds it once a soak shows asks for salvage and a rig blueprint in reach.
//!
//! Everything here is **[A] policy**; no number from here appears in a test
//! as an expected value.

use crate::acquire::Kit;
use crate::brain::{Act, Memory, ModuleInfo, Observation, ParkView, RANGE_MARGIN, SHORT_WAIT};
use crate::{Conn, ItemRow, MIN_WARP_DISTANCE, Result};
use std::collections::{BTreeMap, BTreeSet};

/// `groupSalvager` **[V-data]**: Salvager I, II and the Civilian Salvager.
pub const GROUP_SALVAGER: i64 = 1122;
/// `groupSalvagedMaterials` **[V]**: what comes out.
pub const GROUP_SALVAGE: i64 = 754;
/// Salvager I **[V-data]** (group 1122, requires Salvaging I).
pub const SALVAGER_I: i64 = 25_861;
/// Salvaging **[V-data]**: the skill the module asks for.
pub const SKILL_SALVAGING: i64 = 25_863;
/// The module's effect as `dogmaIM.Activate` names it, `(name, id)`
/// **[V-data]** (`dogmaeffects[2757]`).
pub const EFFECT: (&str, i64) = ("salvaging", 2757);
/// `Activate`'s `repeat`: one cycle, one attempt **[A]**. The server ends a
/// run at its outcome either way; one cycle makes an attempt a single act.
pub const REPEAT: i64 = 1;

/// Attempts on one wreck before it is let go. **[A]**: a Salvager I at
/// Salvaging I succeeds about every other try, so five misses in a row are
/// a refusal in disguise. Falsified by wrecks left whole in a quiet belt.
pub const MAX_TRIES: u32 = 5;

/// The ratter's kit: [`crate::acquire::FIGHTER_KIT`] and a Salvager I
/// beside the guns, the Salvaging skill beside Gunnery. **[A]**: one
/// salvager, for the one free high slot.
pub const RATTER_KIT: Kit = Kit {
    hull: crate::acquire::FIGHTER_KIT.hull,
    modules: crate::acquire::FIGHTER_KIT.modules,
    ammo: crate::acquire::FIGHTER_KIT.ammo,
    skills: &[(crate::escort::SKILL_GUNNERY, 1), (SKILL_SALVAGING, 1)],
    more: &[(SALVAGER_I, 1)],
};

/// What the ratter remembers about salvaging: [`Memory::salvage`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Attempts sent, by wreck.
    pub tries: BTreeMap<i64, u32>,
    /// Wrecks let go.
    pub done: BTreeSet<i64>,
}

impl State {
    fn forget(&mut self) {
        self.tries.clear();
        self.done.clear();
    }
}

/// Is `row` a salvager?
pub fn is_salvager(row: &ItemRow) -> bool {
    row.group_id == GROUP_SALVAGER
}

/// **The fitted salvager**, read as godma reads a module (`ItemGetInfo`):
/// online, running, its target and `maxRange`. `None` when none is fitted.
/// `rows` are the active ship's items.
pub fn read(
    conn: &mut Conn,
    rows: &[ItemRow],
    content: Option<&eve_server::content::Content>,
) -> Result<Option<ModuleInfo>> {
    let Some(row) = rows.iter().find(|r| r.is_fitted() && is_salvager(r)) else {
        return Ok(None);
    };
    let info = conn.module_row(row.item_id)?;
    let (range, _) = crate::modules::laser_range(|a| info.attribute(a), content, row.type_id);
    Ok(Some(ModuleInfo {
        item_id: row.item_id,
        flag_id: row.flag_id,
        online: info.effects.contains(&crate::EFFECT_ONLINE),
        active: info.effects.contains(&EFFECT.1),
        target: None,
        range,
    }))
}

/// **Docked** (the escort's rule 6b): fit, online, or buy a salvager.
/// `None` leaves the turn to the escort's dwell and undock.
pub fn docked(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    m.salvage.forget();
    if let Some(s) = o.salvager {
        return (!s.online).then(|| vec![Act::Online(s.item_id)]);
    }
    if let Some(row) = o.hangar.iter().find(|r| is_salvager(r)) {
        return (o.failed(crate::brain::ActKind::FitAuto) == 0)
            .then(|| vec![Act::FitAuto(row.item_id)]);
    }
    // Nothing to fit: buy one, in the closed economy, once the Rifter and
    // its guns are what the pilot flies.
    let flying = o.ship.and_then(|s| s.type_id);
    if m.policy.closed
        && flying.is_some_and(crate::hull_choice::is_combat_hull)
        && !o.guns.is_empty()
    {
        return crate::brain::acquire_if_due(m, false, true, false);
    }
    None
}

/// **In space**, quiet (the escort's rule 5, once there is nothing left to
/// loot): salvage the nearest looted wreck. `None` when there is nothing to
/// salvage or no salvager to do it.
pub fn in_space(m: &mut Memory, o: &Observation, park: &ParkView) -> Option<Vec<Act>> {
    let s = o.salvager.filter(|s| s.online)?;
    if o.hold_full {
        return None;
    }
    let ego = park.ego;
    // A run in progress: let the cycle end.
    if s.active {
        return Some(vec![Act::Wait(SHORT_WAIT)]);
    }
    let wreck = park
        .wrecks
        .iter()
        .filter(|w| m.looted.contains(&w.id) && !m.salvage.done.contains(&w.id))
        .min_by(|a, b| ego.surface_to(a).total_cmp(&ego.surface_to(b)))?;
    let tries = m.salvage.tries.get(&wreck.id).copied().unwrap_or(0);
    if tries >= MAX_TRIES {
        m.salvage.done.insert(wreck.id);
        return None;
    }
    let gap = ego.surface_to(wreck);
    if gap > s.range * RANGE_MARGIN {
        // Too far to bother with a warp; an approach that was not taken is
        // not retried.
        if gap > MIN_WARP_DISTANCE || o.failed(crate::brain::ActKind::Follow) > 0 {
            m.salvage.done.insert(wreck.id);
            return None;
        }
        if park.ego_tether == Some(wreck.id) {
            return Some(vec![Act::Wait(SHORT_WAIT)]);
        }
        return Some(vec![Act::Follow {
            id: wreck.id,
            range: (s.range * RANGE_MARGIN / 2.0).max(1.0),
        }]);
    }
    if !o.locked.contains(&wreck.id) {
        if o.pending.contains(&wreck.id) || gap > o.lock_range * RANGE_MARGIN {
            return Some(vec![Act::Wait(SHORT_WAIT)]);
        }
        return Some(vec![Act::Lock(wreck.id)]);
    }
    *m.salvage.tries.entry(wreck.id).or_insert(0) += 1;
    Some(vec![Act::Salvage {
        module: s.item_id,
        wreck: wreck.id,
    }])
}

/// **[`Act::Salvage`]**: one attempt, `dogmaIM.Activate` of the `salvaging`
/// effect on the locked wreck for [`REPEAT`] cycle. A refusal is the
/// server's key (`TargetTooFar`, `SalvagingPointlessLoot`,
/// `SalvagingPointlessOverload`, `DeniedActivateTargetNotPresent`), in the
/// error.
pub fn run(conn: &mut Conn, module: i64, wreck: i64) -> Result<crate::GodmaEffect> {
    conn.activate(module, EFFECT, Some(wreck), REPEAT)
}

#[cfg(test)]
mod tests {
    //! Invented ids; asserts the kind and order of acts and the ids they
    //! name, never a range, a count or a delay.
    use super::*;
    use crate::brain::{ActKind, Ball, Place, Policy, Role, ShipInfo};
    use std::time::Instant;

    const STATION: i64 = 60_012_526;
    const SHIP: i64 = 100_000_301;
    const MODULE: i64 = 100_000_302;
    const WRECK: i64 = 100_000_303;
    const WRECK_2: i64 = 100_000_304;
    const GUN_A: i64 = 100_000_305;
    /// An invented salvager type: the rule is by group, not by id.
    const SALVAGER_TYPE: i64 = 990_601;
    /// An invented salvager range; the planner reads it from the module.
    const RANGE: f64 = 5_000.0;

    fn row(item_id: i64, group_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id: SALVAGER_TYPE,
            owner_id: 90_000_001,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity,
            group_id,
            category_id: 7,
            singleton: quantity == 1,
        }
    }

    fn ratter(closed: bool) -> Memory {
        Memory::new(
            "Ratter1",
            Policy {
                role: Role::Escort,
                closed,
                ..Policy::fast()
            },
        )
    }

    fn module(online: bool, active: bool) -> ModuleInfo {
        ModuleInfo {
            item_id: MODULE,
            flag_id: 29,
            online,
            active,
            target: None,
            range: RANGE,
        }
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn docked_obs() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::escort::RIFTER),
        });
        o.guns = vec![crate::brain::GunInfo {
            item_id: GUN_A,
            type_id: crate::escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(1),
            capacity: 1,
            range: 1.0,
            reload: std::time::Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o
    }

    fn space_obs(wreck_x: f64) -> Observation {
        let mut o = docked_obs();
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.salvager = Some(module(true, false));
        o.lock_range = 50_000.0;
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: vec![ball(WRECK, wreck_x)],
            ego_tether: None,
        });
        o
    }

    fn looted(m: &mut Memory, ids: &[i64]) {
        m.looted.extend_from_slice(ids);
    }

    // --- docked ---------------------------------------------------------

    #[test]
    fn a_salvager_in_the_hangar_is_fitted_then_onlined() {
        let mut o = docked_obs();
        o.hangar = vec![row(MODULE, GROUP_SALVAGER, 1)];
        assert_eq!(
            docked(&mut ratter(false), &o),
            Some(vec![Act::FitAuto(MODULE)])
        );
        o.hangar.clear();
        o.salvager = Some(module(false, false));
        assert_eq!(
            docked(&mut ratter(false), &o),
            Some(vec![Act::Online(MODULE)])
        );
        o.salvager = Some(module(true, false));
        assert_eq!(docked(&mut ratter(false), &o), None, "all set");
    }

    #[test]
    fn a_refused_fit_is_not_tried_again() {
        let mut o = docked_obs();
        o.hangar = vec![row(MODULE, GROUP_SALVAGER, 1)];
        o.failures.insert(ActKind::FitAuto, 1);
        assert_eq!(docked(&mut ratter(false), &o), None);
    }

    #[test]
    fn with_none_to_fit_a_closed_economy_buys_one_and_not_again_at_once() {
        let o = docked_obs();
        let mut m = ratter(true);
        m.now = Some(o.now);
        assert_eq!(
            docked(&mut m, &o),
            Some(vec![Act::Acquire {
                ship: false,
                module: true,
                ammo: false
            }])
        );
        assert_eq!(
            docked(&mut m, &o),
            None,
            "the ask is not repeated every turn"
        );
    }

    #[test]
    fn nothing_is_bought_in_the_open_economy_or_out_of_the_rifter_or_unarmed() {
        let o = docked_obs();
        let mut m = ratter(false);
        m.now = Some(o.now);
        assert_eq!(docked(&mut m, &o), None);
        let mut m = ratter(true);
        m.now = Some(o.now);
        let mut capsule = docked_obs();
        capsule.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::brain::CAPSULE),
        });
        assert_eq!(docked(&mut m, &capsule), None);
        let mut unarmed = docked_obs();
        unarmed.guns.clear();
        assert_eq!(docked(&mut m, &unarmed), None);
    }

    #[test]
    fn docking_clears_what_was_remembered_of_the_last_wrecks() {
        let mut m = ratter(false);
        m.salvage.tries.insert(WRECK, 3);
        m.salvage.done.insert(WRECK_2);
        docked(&mut m, &docked_obs());
        assert_eq!(m.salvage, State::default());
    }

    // --- in space ---------------------------------------------------------

    #[test]
    fn a_looted_wreck_in_range_is_locked_and_then_salvaged() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let mut o = space_obs(1_000.0);
        assert_eq!(
            in_space(&mut m, &o, o.park.as_ref().unwrap()),
            Some(vec![Act::Lock(WRECK)])
        );
        o.locked = vec![WRECK];
        let acts = in_space(&mut m, &o, o.park.as_ref().unwrap());
        assert_eq!(
            acts,
            Some(vec![Act::Salvage {
                module: MODULE,
                wreck: WRECK
            }])
        );
        assert_eq!(
            m.salvage.tries[&WRECK], 1,
            "the attempt is counted when sent"
        );
    }

    #[test]
    fn a_lock_in_progress_is_waited_for() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let mut o = space_obs(1_000.0);
        o.pending = vec![WRECK];
        let acts = in_space(&mut m, &o, o.park.as_ref().unwrap()).expect("acts");
        assert_eq!(
            acts.iter().map(Act::kind).collect::<Vec<_>>(),
            vec![ActKind::Wait]
        );
    }

    #[test]
    fn a_wreck_out_of_range_but_near_is_approached_and_a_far_one_let_go() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let o = space_obs(RANGE + 20_000.0);
        let acts = in_space(&mut m, &o, o.park.as_ref().unwrap()).expect("closes in");
        assert!(matches!(acts[..], [Act::Follow { id: WRECK, .. }]));

        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let o = space_obs(MIN_WARP_DISTANCE + 100_000.0);
        assert_eq!(in_space(&mut m, &o, o.park.as_ref().unwrap()), None);
        assert!(
            m.salvage.done.contains(&WRECK),
            "too far to be worth a warp"
        );
    }

    #[test]
    fn a_running_cycle_is_waited_out_and_nothing_starts_without_an_online_salvager() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let mut o = space_obs(1_000.0);
        o.locked = vec![WRECK];
        o.salvager = Some(module(true, true));
        let acts = in_space(&mut m, &o, o.park.as_ref().unwrap()).expect("acts");
        assert_eq!(
            acts.iter().map(Act::kind).collect::<Vec<_>>(),
            vec![ActKind::Wait]
        );
        o.salvager = Some(module(false, false));
        assert_eq!(in_space(&mut m, &o, o.park.as_ref().unwrap()), None);
        o.salvager = None;
        assert_eq!(in_space(&mut m, &o, o.park.as_ref().unwrap()), None);
    }

    #[test]
    fn only_looted_wrecks_are_salvaged_and_the_nearest_first() {
        let mut m = ratter(false);
        let mut o = space_obs(1_000.0);
        o.park.as_mut().unwrap().wrecks = vec![ball(WRECK, 3_000.0), ball(WRECK_2, 1_000.0)];
        assert_eq!(
            in_space(&mut m, &o, o.park.as_ref().unwrap()),
            None,
            "none looted yet"
        );
        looted(&mut m, &[WRECK, WRECK_2]);
        assert_eq!(
            in_space(&mut m, &o, o.park.as_ref().unwrap()),
            Some(vec![Act::Lock(WRECK_2)])
        );
    }

    #[test]
    fn a_wreck_is_let_go_after_its_tries_and_the_next_one_is_taken() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK, WRECK_2]);
        let mut o = space_obs(1_000.0);
        o.park.as_mut().unwrap().wrecks = vec![ball(WRECK, 1_000.0), ball(WRECK_2, 2_000.0)];
        o.locked = vec![WRECK, WRECK_2];
        for _ in 0..MAX_TRIES {
            let acts = in_space(&mut m, &o, o.park.as_ref().unwrap());
            assert_eq!(
                acts,
                Some(vec![Act::Salvage {
                    module: MODULE,
                    wreck: WRECK
                }])
            );
        }
        // The sixth turn gives it up and the caller's turn goes on; the one
        // after takes the other wreck.
        assert_eq!(in_space(&mut m, &o, o.park.as_ref().unwrap()), None);
        assert!(m.salvage.done.contains(&WRECK));
        assert_eq!(
            in_space(&mut m, &o, o.park.as_ref().unwrap()),
            Some(vec![Act::Salvage {
                module: MODULE,
                wreck: WRECK_2
            }])
        );
    }

    #[test]
    fn a_full_hold_salvages_nothing() {
        let mut m = ratter(false);
        looted(&mut m, &[WRECK]);
        let mut o = space_obs(1_000.0);
        o.locked = vec![WRECK];
        o.hold_full = true;
        assert_eq!(in_space(&mut m, &o, o.park.as_ref().unwrap()), None);
    }

    // --- the sale -------------------------------------------------------------

    fn salvage_stack(quantity: i64) -> ItemRow {
        ItemRow {
            category_id: 17,
            singleton: false,
            ..row(7, GROUP_SALVAGE, quantity)
        }
    }

    #[test]
    fn salvage_is_loot_that_is_sold_or_listed_and_never_refined() {
        use crate::escort::{Fate, is_loot, lots};
        use crate::pricing::Quote;
        use crate::verbs::Book;
        let stack = salvage_stack(400);
        assert!(is_loot(&stack));
        let got = lots(std::slice::from_ref(&stack), 0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].quantity, 400);
        assert!(!got[0].whole, "only a whole stack can be refined");
        // With a price and room it is listed; without room it stays.
        let quote = Quote::at_ref(1.0e6);
        assert!(matches!(
            crate::escort::fate(&got[0], &Book::default(), STATION, Some(quote), false, true),
            Fate::List(_)
        ));
        assert_eq!(
            crate::escort::fate(
                &got[0],
                &Book::default(),
                STATION,
                Some(quote),
                false,
                false
            ),
            Fate::Keep
        );
        assert_eq!(
            crate::escort::fate(&got[0], &Book::default(), STATION, None, false, true),
            Fate::Keep
        );
    }

    #[test]
    fn a_module_in_the_hangar_is_not_taken_for_salvage() {
        let module = ItemRow {
            singleton: false,
            ..row(8, GROUP_SALVAGER, 3)
        };
        // A stack of three packaged salvagers is loot by its category (the
        // ratter sells spare modules), not by the salvage rule.
        assert!(!is_salvager(&salvage_stack(1)));
        assert!(is_salvager(&module));
    }

    // --- the kit -------------------------------------------------------------

    #[test]
    fn the_ratters_kit_is_the_fighters_with_a_salvager_and_its_skill() {
        let f = crate::acquire::FIGHTER_KIT;
        assert_eq!(RATTER_KIT.hull, f.hull);
        assert_eq!(RATTER_KIT.modules, f.modules);
        assert_eq!(RATTER_KIT.ammo, f.ammo);
        assert!(RATTER_KIT.more.iter().any(|(t, _)| *t == SALVAGER_I));
        for skill in f.skills {
            assert!(RATTER_KIT.skills.contains(skill));
        }
        assert!(RATTER_KIT.skills.iter().any(|(s, _)| *s == SKILL_SALVAGING));
    }

    #[test]
    fn a_hangar_row_is_a_salvager_by_its_group() {
        assert!(is_salvager(&row(1, GROUP_SALVAGER, 1)));
        assert!(!is_salvager(&row(1, GROUP_SALVAGE, 1)));
    }
}
