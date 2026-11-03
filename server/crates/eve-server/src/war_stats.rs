//! The war report's numbers: who killed how many ships of what class, for
//! how much (C8-b; R-WAR §3 `warStatisticMgr`; lane note
//! `docs/lanes/c8-a.md`).
//!
//! A kill is filed under a war when it happens (`war::on_kill`, the `warkill`
//! rows); here the filed reports are read back and added up. A kill belongs
//! to a **side**: the war's aggressor, or the defender's camp (the defender
//! and every ally of it, `war.allies`), by the owner the final blow or the
//! victim fought as -- their alliance if it is in the war, else their corp.
//!
//! # The client's two keys for a ship class [V]
//!
//! `GetKillsByGroup` answers `{inventory groupID: {attackerShipLoss, ...}}`
//! and the report maps each key with `cfg.GetShipClassTypeByGroupID`, so its
//! keys are real inventory groups (here: the victim hull's group, read from
//! the type table at run time). `GetKills(entityID, shipGroup)` is asked
//! with the **class** (`appConst.GROUP_*`, 0 to 9) of the bar the pilot
//! clicked (`warReportKillGroups`, setting `killGroupDisplayed`), so the
//! server needs the same group-to-class table the client has
//! (`eveCfg.GetShipGroupByClassType`): [`CLASSES`], transcribed as the
//! protocol constants they are (the client's own UI table, like
//! `notifications::GROUPS`), not read from static data.

use crate::killmail::Report;
use crate::war::War;
use std::collections::BTreeMap;

/// `appConst.GROUP_CAPSULES` .. `GROUP_STRUCTURES` **[V]**.
pub const CLASS_CAPSULES: i64 = 0;
pub const CLASS_FRIGATES: i64 = 1;
pub const CLASS_DESTROYERS: i64 = 2;
pub const CLASS_CRUISERS: i64 = 3;
pub const CLASS_BATTLESHIPS: i64 = 4;
pub const CLASS_BATTLECRUISERS: i64 = 5;
pub const CLASS_CAPITALS: i64 = 6;
pub const CLASS_INDUSTRIALS: i64 = 7;
pub const CLASS_POS: i64 = 8;
pub const CLASS_STRUCTURES: i64 = 9;

/// `eveCfg.GetShipGroupByClassType` **[V]**: the inventory groups of each
/// ship class. `GROUP_POS` and `GROUP_STRUCTURES` are by category
/// ([`CATEGORY_STARBASE`], [`CATEGORY_STRUCTURE`]), not by group.
pub const CLASSES: &[(i64, &[i64])] = &[
    (CLASS_CAPSULES, &[29]),
    (
        CLASS_FRIGATES,
        &[31, 237, 25, 324, 830, 893, 1022, 831, 834, 1283, 1527],
    ),
    (CLASS_DESTROYERS, &[420, 541, 1305, 1534]),
    (
        CLASS_CRUISERS,
        &[26, 963, 906, 833, 358, 894, 832, 1972, 5087],
    ),
    (CLASS_BATTLECRUISERS, &[419, 1201, 540, 4902]),
    (CLASS_BATTLESHIPS, &[27, 898, 900]),
    (CLASS_CAPITALS, &[547, 5120, 883, 485, 30, 659, 1538, 4594]),
    (CLASS_INDUSTRIALS, &[1202, 543, 513, 28, 941, 902, 463, 380]),
];

/// `inventorycommon.const.categoryStarbase` and `categoryStructure` **[V]**.
pub const CATEGORY_STARBASE: i64 = 23;
pub const CATEGORY_STRUCTURE: i64 = 65;

/// The class of a hull by its inventory group and category
/// (`GetShipClassTypeByGroupID`), `None` for a type the client has no bar
/// for.
pub fn class_of(group: i64, category: i64) -> Option<i64> {
    if let Some((class, _)) = CLASSES.iter().find(|(_, groups)| groups.contains(&group)) {
        return Some(*class);
    }
    match category {
        CATEGORY_STARBASE => Some(CLASS_POS),
        CATEGORY_STRUCTURE => Some(CLASS_STRUCTURES),
        _ => None,
    }
}

/// Which side of `war` a pilot of `(corp, alliance)` fights on: its principal
/// or ally id (alliance first), `None` when neither is in the war.
pub fn owner_in_war(w: &War, corp: Option<i64>, alliance: Option<i64>) -> Option<i64> {
    [alliance, corp]
        .into_iter()
        .flatten()
        .find(|o| w.involves(*o))
}

/// Whether `owner` fights for the aggressor.
fn is_aggressor_camp(w: &War, owner: Option<i64>) -> bool {
    owner == Some(w.declared_by)
}

/// The side a report's final blow fought for, as a principal: the
/// aggressor, or the defender (for the defender or any ally).
fn killer_side(w: &War, r: &Report) -> Option<i64> {
    let o = owner_in_war(w, r.final_blow.corporation, r.final_blow.alliance)?;
    Some(if o == w.declared_by {
        w.declared_by
    } else {
        w.against
    })
}

/// The side the victim fought for.
fn victim_side(w: &War, r: &Report) -> Option<i64> {
    let o = owner_in_war(w, Some(r.victim_corporation), r.victim_alliance)?;
    Some(if o == w.declared_by {
        w.declared_by
    } else {
        w.against
    })
}

/// What the report window's header shows: ships and ISK killed by each
/// principal's camp, `(shipsKilled, iskKilled)` by principal id.
pub fn base_info(w: &War, reports: &[Report]) -> (BTreeMap<i64, i64>, BTreeMap<i64, f64>) {
    let mut ships: BTreeMap<i64, i64> = BTreeMap::new();
    let mut isk: BTreeMap<i64, f64> = BTreeMap::new();
    ships.insert(w.declared_by, 0);
    ships.insert(w.against, 0);
    isk.insert(w.declared_by, 0.0);
    isk.insert(w.against, 0.0);
    for r in reports {
        let Some(side) = killer_side(w, r) else {
            continue;
        };
        *ships.entry(side).or_default() += 1;
        *isk.entry(side).or_default() += r.isk_lost.unwrap_or(0.0);
    }
    (ships, isk)
}

/// `GetKills(entityID, shipGroup)`: the war's reports, newest first, those
/// the camp of `entity` made (`None`: both camps) and, if `class` is given,
/// of ships of that class. `group_of` is the type table: a hull's `(group,
/// category)`.
pub fn kills<'a>(
    w: &War,
    reports: &'a [Report],
    entity: Option<i64>,
    class: Option<i64>,
    group_of: impl Fn(i64) -> Option<(i64, i64)>,
) -> Vec<&'a Report> {
    let mut v: Vec<&Report> = reports
        .iter()
        .filter(|r| match entity {
            None => true,
            Some(e) if e == w.declared_by => is_aggressor_camp(w, killer_side(w, r)),
            Some(_) => killer_side(w, r).is_some_and(|s| !is_aggressor_camp(w, Some(s))),
        })
        .filter(|r| match class {
            None => true,
            Some(c) => group_of(r.ship_type).and_then(|(g, cat)| class_of(g, cat)) == Some(c),
        })
        .collect();
    v.sort_by(|a, b| b.time.cmp(&a.time).then(b.kill_id.cmp(&a.kill_id)));
    v
}

/// The four numbers of one hull group in `GetKillsByGroup`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GroupTotals {
    pub attacker_ship_loss: i64,
    pub attacker_isk_loss: f64,
    pub defender_ship_loss: i64,
    pub defender_isk_loss: f64,
}

/// `GetKillsByGroup`: ships (and their ISK) each camp lost, by the victim
/// hull's **inventory group**. A report whose hull the type table does not
/// know is left out.
pub fn kills_by_group(
    w: &War,
    reports: &[Report],
    group_of: impl Fn(i64) -> Option<(i64, i64)>,
) -> BTreeMap<i64, GroupTotals> {
    let mut out: BTreeMap<i64, GroupTotals> = BTreeMap::new();
    for r in reports {
        let (Some(side), Some((group, _))) = (victim_side(w, r), group_of(r.ship_type)) else {
            continue;
        };
        let t = out.entry(group).or_default();
        let isk = r.isk_lost.unwrap_or(0.0);
        if side == w.declared_by {
            t.attacker_ship_loss += 1;
            t.attacker_isk_loss += isk;
        } else {
            t.defender_ship_loss += 1;
            t.defender_isk_loss += isk;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::killmail::Attacker;

    const AGGRESSOR: i64 = 98_000_001;
    const DEFENDER: i64 = 98_000_002;
    const ALLY: i64 = 98_000_003;
    const OUTSIDER: i64 = 98_000_004;
    /// Invented hulls; the group table below is the test's own.
    const FRIGATE_HULL: i64 = 990_001;
    const CRUISER_HULL: i64 = 990_002;

    fn war() -> War {
        let mut w = War::new(1, AGGRESSOR, DEFENDER, 100, 0, None);
        w.allies.insert(
            ALLY,
            crate::war::Ally {
                ally_id: ALLY,
                time_started: 0,
                time_finished: i64::MAX,
                told: true,
            },
        );
        w
    }

    /// The test's type table: the client's real classes are looked up by
    /// group, so use groups the class table has (25 frigates, 26 cruisers).
    fn group_of(ship: i64) -> Option<(i64, i64)> {
        match ship {
            FRIGATE_HULL => Some((25, 6)),
            CRUISER_HULL => Some((26, 6)),
            _ => None,
        }
    }

    fn attacker(corp: i64) -> Attacker {
        Attacker {
            character: Some(1),
            corporation: Some(corp),
            alliance: None,
            faction: None,
            ship_type: None,
            weapon_type: None,
            damage: 10,
            security: None,
        }
    }

    fn report(id: i64, killer_corp: i64, victim_corp: i64, hull: i64, isk: f64) -> Report {
        Report {
            kill_id: id,
            time: 1_000 + id,
            system: 30_000_001,
            victim: 2,
            victim_corporation: victim_corp,
            victim_alliance: None,
            ship_type: hull,
            damage_taken: 10,
            isk_lost: Some(isk),
            isk_destroyed: Some(isk),
            final_blow: attacker(killer_corp),
            attackers: vec![],
            items: vec![],
            bounty_claimed: None,
            kill_right_supplied: None,
        }
    }

    fn reports() -> Vec<Report> {
        vec![
            report(1, AGGRESSOR, DEFENDER, FRIGATE_HULL, 10.0),
            report(2, ALLY, AGGRESSOR, CRUISER_HULL, 40.0),
            report(3, DEFENDER, AGGRESSOR, FRIGATE_HULL, 20.0),
            report(4, OUTSIDER, AGGRESSOR, FRIGATE_HULL, 99.0),
        ]
    }

    #[test]
    fn the_client_groups_map_to_its_classes() {
        assert_eq!(class_of(25, 6), Some(CLASS_FRIGATES));
        assert_eq!(class_of(26, 6), Some(CLASS_CRUISERS));
        assert_eq!(class_of(29, 6), Some(CLASS_CAPSULES));
        assert_eq!(class_of(5, CATEGORY_STARBASE), Some(CLASS_POS));
        assert_eq!(class_of(5, CATEGORY_STRUCTURE), Some(CLASS_STRUCTURES));
        assert_eq!(class_of(5, 7), None);
        // No group is in two classes: the client's lookup returns the first.
        let mut seen = std::collections::BTreeSet::new();
        for (_, groups) in CLASSES {
            for g in *groups {
                assert!(seen.insert(*g), "group {g} is in two classes");
            }
        }
    }

    #[test]
    fn a_kill_is_the_aggressors_or_the_defender_camps() {
        let w = war();
        let rs = reports();
        let (ships, isk) = base_info(&w, &rs);
        assert_eq!(ships[&AGGRESSOR], 1, "one kill by the aggressor");
        assert_eq!(ships[&DEFENDER], 2, "the defender's and its ally's");
        assert_eq!(isk[&AGGRESSOR], 10.0);
        assert_eq!(isk[&DEFENDER], 60.0);
        // The outsider's kill counts for neither.
        assert_eq!(ships.values().sum::<i64>(), 3);
    }

    #[test]
    fn kills_filter_by_camp_and_class_newest_first() {
        let w = war();
        let rs = reports();
        let ids = |v: Vec<&Report>| v.iter().map(|r| r.kill_id).collect::<Vec<_>>();
        assert_eq!(ids(kills(&w, &rs, None, None, group_of)), vec![4, 3, 2, 1]);
        assert_eq!(
            ids(kills(&w, &rs, Some(AGGRESSOR), None, group_of)),
            vec![1]
        );
        assert_eq!(
            ids(kills(&w, &rs, Some(DEFENDER), None, group_of)),
            vec![3, 2]
        );
        assert_eq!(
            ids(kills(&w, &rs, None, Some(CLASS_CRUISERS), group_of)),
            vec![2]
        );
        assert_eq!(
            ids(kills(
                &w,
                &rs,
                Some(DEFENDER),
                Some(CLASS_FRIGATES),
                group_of
            )),
            vec![3]
        );
    }

    #[test]
    fn kills_by_group_are_keyed_by_the_hulls_inventory_group() {
        let w = war();
        let rs = reports();
        let by = kills_by_group(&w, &rs, group_of);
        assert_eq!(by.keys().copied().collect::<Vec<_>>(), vec![25, 26]);
        // The aggressor lost the cruiser (40) and two frigates (20 + 99);
        // the defender lost one frigate (10).
        assert_eq!(by[&25].attacker_ship_loss, 2);
        assert_eq!(by[&25].attacker_isk_loss, 119.0);
        assert_eq!(by[&25].defender_ship_loss, 1);
        assert_eq!(by[&25].defender_isk_loss, 10.0);
        assert_eq!(by[&26].attacker_ship_loss, 1);
        assert_eq!(by[&26].defender_ship_loss, 0);
    }
}
