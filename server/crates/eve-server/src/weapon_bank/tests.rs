//! K-OVERHEAT weapon-bank tests: invented ids; the client's group ids, flags
//! and error keys are protocol vocabulary [V].

use super::*;
use crate::model::{Character, SandboxConfig};

const ALICE: i64 = 90_000_601;
const SHIP: i64 = 990_601_001;
const HULL: i64 = 587;
/// `Fixture Afterburner`'s type: a self effect, so a bank of them can run
/// without a target (the group on the *item* is what grouping reads).
const AB: i64 = 439;
const OTHER: i64 = 399;
const A: i64 = 990_601_301;
const B: i64 = 990_601_302;
const C: i64 = 990_601_303;
const D: i64 = 990_601_304;
const E: i64 = 990_601_305;
/// `groupProjectileWeapon` [V].
const WEAPON_GROUP: i64 = 55;
const HI: i64 = 27;
const MED: i64 = 19;
/// Two charge types the fixture knows.
const ROUND_1: i64 = 185;
const ROUND_2: i64 = 184;
const EFFECT: &str = "moduleBonusAfterburner";

fn item(id: i64, type_id: i64, group: i64, flag: i64) -> crate::item::Item {
    crate::item::Item {
        item_id: id,
        type_id,
        owner_id: ALICE,
        location_id: SHIP,
        flag_id: flag,
        quantity: 1,
        group_id: group,
        category_id: 7,
        singleton: true,
    }
}

/// Alice in space: A, B, C in high slots 0-2 and D in 4 (all the same type
/// and the weapon group), a module of another type in 5, one of the same
/// type in a medium slot, and one in a group that is no weapon's in 6.
fn world() -> Registry {
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: SHIP,
        ship_type_id: HULL,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![alice],
        ..SandboxConfig::default()
    });
    let rifter = *crate::item::find_in(reg.content(), HULL).expect("a spawnable hull");
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = rifter.type_id;
        i.group_id = rifter.group_id;
    }
    items.extend([
        item(A, AB, WEAPON_GROUP, HI),
        item(B, AB, WEAPON_GROUP, HI + 1),
        item(C, AB, WEAPON_GROUP, HI + 2),
        item(D, AB, WEAPON_GROUP, HI + 4),
        item(E, OTHER, WEAPON_GROUP, HI + 5),
        item(E + 1, AB, WEAPON_GROUP, MED),
        item(E + 2, AB, 77, HI + 6),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: E + 100,
        ..crate::store::Snapshot::default()
    });
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    let _ = reg.take_pending();
    reg
}

fn now() -> i64 {
    crate::pyutil::filetime_now()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(3_600)
}

type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

fn sent(reg: &mut Registry) -> Vec<Sent> {
    let mut out: Vec<Sent> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| {
            let (n, a) = p.to_push();
            (n, a, p.audience())
        })
        .collect();
    out.extend(reg.take_due(far()).into_iter().map(|p| {
        let (n, a) = p.to_push();
        (n, a, p.audience())
    }));
    out
}

/// An error as text: its `Debug`, with every byte string spelled out.
fn text(e: &ServiceError) -> String {
    let d = format!("{e:?}");
    let mut out = String::new();
    let mut rest = d.as_str();
    while let Some(at) = rest.find("Bytes([") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "Bytes([".len()..];
        let end = after.find("])").expect("a closed byte list");
        let bytes: Vec<u8> = after[..end]
            .split(", ")
            .filter_map(|n| n.parse().ok())
            .collect();
        out.push_str(&String::from_utf8_lossy(&bytes));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn refused(r: Result<PyValue, ServiceError>) -> String {
    text(&r.expect_err("refused"))
}

fn banks(reg: &Registry) -> Banks {
    reg.ext::<WeaponBanks>().of(SHIP)
}

fn rounds(reg: &mut Registry, flag: i64, type_id: i64) {
    reg.apply_items(&[crate::item::Edit::Create {
        type_id,
        quantity: 10,
        to: crate::item::Slot::new(ALICE, SHIP, flag),
    }])
    .expect("loaded");
    let _ = sent(reg);
}

fn by_id() -> impl Fn(i64) -> i64 {
    |m| m
}

fn map(entries: &[(i64, &[i64])]) -> Banks {
    entries.iter().map(|(m, s)| (*m, s.to_vec())).collect()
}

// -- the banks, pure ----------------------------------------------------------

#[test]
fn a_module_joins_the_bank_of_the_one_it_is_dropped_on() {
    let order = by_id();
    let mut b = Banks::new();
    assert!(link(&mut b, 1, 2, &order));
    assert_eq!(b, map(&[(1, &[2])]));
    // Dropped on a slave: the bank's master takes it.
    assert!(link(&mut b, 2, 3, &order));
    assert_eq!(b, map(&[(1, &[2, 3])]));
    // Already grouped, or itself: nothing.
    assert!(!link(&mut b, 1, 3, &order));
    assert!(!link(&mut b, 1, 1, &order));
    assert_eq!(master_of(&b, 3), Some(1));
    assert_eq!(master_of(&b, 1), Some(1));
    assert_eq!(master_of(&b, 9), None);
    assert_eq!(members(&b, 3), [1, 2, 3]);
    assert_eq!(members(&b, 9), [9]);
}

#[test]
fn members_are_kept_in_slot_order() {
    let slot = |m: i64| match m {
        10 => 3,
        11 => 1,
        12 => 2,
        _ => 0,
    };
    let mut b = Banks::new();
    link(&mut b, 9, 10, &slot);
    link(&mut b, 9, 11, &slot);
    link(&mut b, 9, 12, &slot);
    assert_eq!(b, map(&[(9, &[11, 12, 10])]));
}

#[test]
fn a_whole_bank_merges_into_another() {
    let order = by_id();
    let mut b = map(&[(1, &[2]), (5, &[6, 7])]);
    assert!(merge(&mut b, 1, 5, &order));
    assert_eq!(b, map(&[(1, &[2, 5, 6, 7])]));
    assert!(!merge(&mut b, 1, 6, &order), "the same bank");
    // A lone module is a bank of one.
    assert!(merge(&mut b, 2, 9, &order));
    assert_eq!(b, map(&[(1, &[2, 5, 6, 7, 9])]));
}

#[test]
fn peeling_moves_one_slave_and_a_bank_left_alone_is_gone() {
    let order = by_id();
    let mut b = map(&[(1, &[2, 3]), (5, &[6])]);
    assert_eq!(peel_and_link(&mut b, 5, 1, &order), Some(2));
    assert_eq!(b, map(&[(1, &[3]), (5, &[2, 6])]));
    assert_eq!(peel_and_link(&mut b, 5, 1, &order), Some(3));
    assert_eq!(
        b,
        map(&[(5, &[2, 3, 6])]),
        "a bank with no slave is not one"
    );
    assert_eq!(peel_and_link(&mut b, 5, 5, &order), None);
    assert_eq!(peel_and_link(&mut b, 5, 9, &order), None);
}

#[test]
fn unlinking_answers_the_slave_the_client_removes_from_its_copy() {
    let mut b = map(&[(1, &[2, 3])]);
    assert_eq!(unlink(&mut b, 1), Some(2));
    assert_eq!(b, map(&[(1, &[3])]));
    assert_eq!(unlink(&mut b, 1), Some(3));
    assert!(b.is_empty());
    assert_eq!(unlink(&mut b, 1), None);
    let mut b = map(&[(1, &[2, 3])]);
    assert!(destroy(&mut b, 3), "by any member");
    assert!(b.is_empty());
    assert!(!destroy(&mut b, 3));
}

#[test]
fn group_all_makes_one_bank_per_type_and_charge_keeping_a_master() {
    let order = by_id();
    let mut groups: BTreeMap<(i64, i64), Vec<i64>> = BTreeMap::new();
    groups.insert((AB, 0), vec![3, 1, 2]);
    groups.insert((AB, ROUND_1), vec![10, 11]);
    groups.insert((OTHER, 0), vec![20]);
    let none = Banks::new();
    let got = link_all(&none, &groups, &order);
    assert_eq!(got, map(&[(1, &[2, 3]), (10, &[11])]));
    // An existing master stays the master.
    let had = map(&[(3, &[9])]);
    assert_eq!(link_all(&had, &groups, &order).get(&3), Some(&vec![1, 2]));
}

/// `SetWeaponBanks` iterates `data.iteritems()` as `(masterID, slaveIDs)`;
/// marshal dict entries are `(value, key)` here.
#[test]
fn the_dict_the_client_reads_is_master_to_a_list_of_slaves() {
    let v = bank_value(&map(&[(1, &[2, 3])]));
    assert_eq!(
        v,
        PyValue::Dict(vec![(
            PyValue::List(vec![PyValue::Int(2), PyValue::Int(3)]),
            PyValue::Int(1)
        )])
    );
    assert_eq!(bank_value(&Banks::new()), PyValue::Dict(vec![]));
}

// -- against a world ------------------------------------------------------------

#[test]
fn linking_builds_the_bank_and_answers_the_whole_dict() {
    let mut reg = world();
    let t = now();
    let r = link_weapons(&mut reg, Some(ALICE), A, B, t).expect("links");
    assert_eq!(r, bank_value(&map(&[(A, &[B])])));
    assert_eq!(banks(&reg), map(&[(A, &[B])]));
    let r = link_weapons(&mut reg, Some(ALICE), B, C, t).expect("onto a slave");
    assert_eq!(r, bank_value(&map(&[(A, &[B, C])])));
    // Merging, peeling, unlinking and destroying, as the client calls them.
    let r = merge_groups(&mut reg, Some(ALICE), A, D, t).expect("merges");
    assert_eq!(r, bank_value(&map(&[(A, &[B, C, D])])));
    // A peel with nothing to peel to is a no-op that still answers the dict.
    let r = peel_link(&mut reg, Some(ALICE), A, A, t).expect("a no-op");
    assert_eq!(r, bank_value(&map(&[(A, &[B, C, D])])));
}

#[test]
fn unlinking_and_destroying_a_bank_in_a_world() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    assert_eq!(
        unlink_module(&mut reg, Some(ALICE), A).unwrap(),
        PyValue::Int(B)
    );
    assert_eq!(banks(&reg), map(&[(A, &[C])]));
    assert_eq!(
        destroy_bank(&mut reg, Some(ALICE), A).unwrap(),
        PyValue::None
    );
    assert!(banks(&reg).is_empty());
    assert!(unlink_module(&mut reg, Some(ALICE), A).is_err());
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    assert_eq!(
        unlink_all(&mut reg, Some(ALICE)).unwrap(),
        PyValue::Dict(vec![])
    );
    assert!(banks(&reg).is_empty());
}

#[test]
fn what_the_server_sees_the_client_cannot_is_refused_with_the_clients_keys() {
    let mut reg = world();
    let t = now();
    // Offline.
    reg.modules_mut().set_online(B, false);
    assert!(
        refused(link_weapons(&mut reg, Some(ALICE), A, B, t)).contains("CantLinkModuleNotOnline")
    );
    reg.modules_mut().set_online(B, true);
    // Damaged.
    reg.ext_mut::<crate::overheat::Heat>().set_damage(B, 1.0);
    assert!(
        refused(link_weapons(&mut reg, Some(ALICE), A, B, t)).contains("CantLinkModuleDamaged")
    );
    reg.ext_mut::<crate::overheat::Heat>().set_damage(B, 0.0);
    // Different types, a group that is no weapon's, and a medium slot.
    assert!(link_weapons(&mut reg, Some(ALICE), A, E, t).is_err());
    assert!(link_weapons(&mut reg, Some(ALICE), A, E + 1, t).is_err());
    assert!(link_weapons(&mut reg, Some(ALICE), A, E + 2, t).is_err());
    // Charges: the same type or none.
    rounds(&mut reg, HI, ROUND_1);
    assert!(refused(link_weapons(&mut reg, Some(ALICE), A, B, t)).contains("CantLinkAmmoInWeapon"));
    rounds(&mut reg, HI + 1, ROUND_2);
    assert!(refused(link_weapons(&mut reg, Some(ALICE), A, B, t)).contains("CantLinkAmmoInWeapon"));
    rounds(&mut reg, HI + 2, ROUND_1);
    assert!(link_weapons(&mut reg, Some(ALICE), A, C, t).is_ok());
    assert_eq!(banks(&reg), map(&[(A, &[C])]));
    // Running.
    crate::weapon_bank::activate(&mut reg, Some(ALICE), D, EFFECT, None, 1, t).expect("D runs");
    assert!(refused(link_weapons(&mut reg, Some(ALICE), A, D, t)).contains("CantLinkModuleActive"));
}

#[test]
fn group_all_groups_the_idle_online_weapons_by_type_and_charge() {
    let mut reg = world();
    let t = now();
    let r = link_all_weapons(&mut reg, Some(ALICE), t).expect("groups");
    assert_eq!(r, bank_value(&map(&[(A, &[B, C, D])])));
    assert_eq!(banks(&reg), map(&[(A, &[B, C, D])]));
    // A charge in one splits it out.
    rounds(&mut reg, HI + 2, ROUND_1);
    let r = link_all_weapons(&mut reg, Some(ALICE), t).expect("regroups");
    assert_eq!(r, bank_value(&map(&[(A, &[B, D])])));
}

/// **A bank runs as one click**: the master's activation starts every member,
/// each told its own `OnGodmaShipEffect`; the master's stop stops them all.
#[test]
fn activating_the_master_runs_every_member_and_stopping_it_stops_them() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    let _ = sent(&mut reg);
    activate(&mut reg, Some(ALICE), A, EFFECT, None, 1000, t).expect("runs");
    for m in [A, B, C] {
        assert!(reg.modules().running(m).is_some(), "{m}");
    }
    assert!(reg.modules().running(D).is_none(), "not in the bank");
    let started: Vec<i64> = sent(&mut reg)
        .iter()
        .filter(|(n, a, _)| *n == "OnGodmaShipEffect" && a[3] == PyValue::Int(1))
        .map(|(_, a, _)| match &a[0] {
            PyValue::Int(m) => *m,
            _ => 0,
        })
        .collect();
    assert_eq!(
        started,
        [A],
        "one button, one icon: only the master is told"
    );
    // A second click on the master is the module's own refusal.
    assert!(
        refused(activate(&mut reg, Some(ALICE), A, EFFECT, None, 1, t + 1))
            .contains("EffectAlreadyActive2")
    );
    deactivate(&mut reg, Some(ALICE), A, EFFECT, t + 2).expect("stops");
    for m in [A, B, C] {
        assert!(
            reg.modules()
                .running(m)
                .is_some_and(|r| r.stop_at.is_some()),
            "{m} stops at its cycle's end"
        );
    }
}

/// `(start flag, module)` of every `OnGodmaShipEffect` in `out`.
fn effects(out: &[Sent]) -> Vec<(i64, i64)> {
    out.iter()
        .filter(|(n, _, _)| *n == "OnGodmaShipEffect")
        .map(|(_, a, _)| match (&a[0], &a[3]) {
            (PyValue::Int(m), PyValue::Int(s)) => (*m, *s),
            _ => (0, -1),
        })
        .collect()
}

#[test]
fn a_bank_fires_every_member_but_tells_the_pilot_of_the_master_only() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    let _ = sent(&mut reg);
    activate(&mut reg, Some(ALICE), A, EFFECT, None, 1000, t).expect("runs");
    let cycles = reg.modules_mut().take_cycles();
    let fired: Vec<i64> = cycles.iter().map(|c| c.module_id).collect();
    assert_eq!(fired, [A, B, C], "every member's cycle is recorded");
    assert_eq!(effects(&sent(&mut reg)), [(A, 1)]);
    // The master stops at its cycle's end; the slaves' stops are withheld too.
    deactivate(&mut reg, Some(ALICE), A, EFFECT, t + 2).expect("stops");
    let stops = effects(&sent(&mut reg));
    assert_eq!(stops, [(A, 0)], "only the master's stop is scheduled");
}

#[test]
fn slaves_stop_silently_with_a_master_that_stopped_or_left_the_bank() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    activate(&mut reg, Some(ALICE), A, EFFECT, None, 1000, t).expect("runs");
    let _ = sent(&mut reg);
    // The master ends (the clock's own stop, as for an empty magazine).
    assert!(crate::modules::stop(&mut reg, A, t + 1, None));
    tick(&mut reg, far(), t + 2);
    assert!(reg.modules().running(B).is_none(), "the slave followed");
    assert!(reg.modules().quiet().is_empty(), "and is forgotten");
    assert_eq!(effects(&sent(&mut reg)), [(A, 0)], "one stop, the master's");
    // A slave unlinked while running no longer runs on its own.
    link_weapons(&mut reg, Some(ALICE), A, B, t + 3).unwrap();
    activate(&mut reg, Some(ALICE), A, EFFECT, None, 1000, t + 4).expect("runs");
    unlink_module(&mut reg, Some(ALICE), A).unwrap();
    tick(&mut reg, far(), t + 5);
    assert!(reg.modules().running(B).is_none());
    assert!(reg.modules().running(A).is_some());
}

#[test]
fn a_member_that_cannot_start_does_not_stop_the_rest() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    reg.modules_mut().set_online(B, false);
    activate(&mut reg, Some(ALICE), A, EFFECT, None, 1000, t).expect("runs");
    assert!(reg.modules().running(A).is_some() && reg.modules().running(C).is_some());
    assert!(reg.modules().running(B).is_none());
}

#[test]
fn loading_or_unloading_the_master_names_the_whole_bank() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    assert_eq!(expand(&reg, Some(ALICE), &[A]), [A, B]);
    assert_eq!(expand(&reg, Some(ALICE), &[B]), [A, B], "by any member");
    assert_eq!(expand(&reg, Some(ALICE), &[A, B, D]), [A, B, D]);
    assert_eq!(expand(&reg, Some(ALICE), &[D]), [D]);
    assert_eq!(expand(&reg, None, &[A]), [A]);
}

#[test]
fn the_board_answer_carries_the_banks_in_its_third_slot() {
    let mut reg = world();
    let t = now();
    let PyValue::Tuple(plain) = ship_state(&reg, SHIP) else {
        panic!("a tuple")
    };
    assert_eq!(plain.len(), 4);
    assert_eq!(plain[2], PyValue::None, "no bank is None, as before");
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    let PyValue::Tuple(state) = ship_state(&reg, SHIP) else {
        panic!("a tuple")
    };
    assert_eq!(state[2], bank_value(&map(&[(A, &[B])])));
    assert_eq!(state[0], plain[0]);
    assert_eq!(state[1], plain[1]);
    assert_eq!(state[3], plain[3]);
}

#[test]
fn a_bank_loses_a_member_that_leaves_the_fitting_and_the_pilot_is_told() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    let _ = sent(&mut reg);
    // Nothing changed: nothing told.
    tick(&mut reg, Instant::now(), t);
    assert!(sent(&mut reg).is_empty());
    // C is unfitted (moved to the cargo hold).
    reg.apply_items(&[crate::item::Edit::Move {
        item_id: C,
        to: crate::item::Slot::new(ALICE, SHIP, 5),
    }])
    .expect("moves");
    let _ = sent(&mut reg);
    tick(&mut reg, Instant::now(), t + 1);
    assert_eq!(banks(&reg), map(&[(A, &[B])]));
    let s = sent(&mut reg);
    let told: Vec<_> = s
        .iter()
        .filter(|(n, ..)| *n == "OnWeaponBanksChanged")
        .collect();
    assert_eq!(told.len(), 1);
    assert_eq!(
        told[0].1,
        [PyValue::Int(SHIP), bank_value(&map(&[(A, &[B])]))]
    );
    assert_eq!(told[0].2, Some(vec![ALICE]));
    // The master going leaves nothing.
    reg.apply_items(&[crate::item::Edit::Move {
        item_id: A,
        to: crate::item::Slot::new(ALICE, SHIP, 5),
    }])
    .expect("moves");
    tick(&mut reg, Instant::now(), t + 2);
    assert!(banks(&reg).is_empty());
    assert!(
        sent(&mut reg)
            .iter()
            .any(|(n, a, _)| *n == "OnWeaponBanksChanged" && a[1] == PyValue::Dict(vec![]))
    );
}

#[test]
fn the_banks_changed_push_has_the_clients_shape() {
    let (n, a) = BanksChanged {
        to: ALICE,
        ship_id: SHIP,
        banks: map(&[(1, &[2])]),
    }
    .to_push();
    assert_eq!(n, "OnWeaponBanksChanged");
    assert_eq!(a, [PyValue::Int(SHIP), bank_value(&map(&[(1, &[2])]))]);
}

#[test]
fn a_call_from_nobody_changes_nothing_and_the_groups_are_the_clients_nineteen() {
    let mut reg = world();
    assert!(link_weapons(&mut reg, None, A, B, now()).is_err());
    assert!(banks(&reg).is_empty());
    let set: BTreeSet<i64> = GROUPABLE_GROUPS.iter().copied().collect();
    assert_eq!(set.len(), 19, "dgmGroupableGroupIDs has nineteen ids");
    assert!(set.contains(&WEAPON_GROUP));
}

// -- persistence ------------------------------------------------------------------

/// A fresh server on the saved world file.
fn reload(snap: crate::store::Snapshot) -> Registry {
    let mut again = Registry::new(SandboxConfig::default());
    again.apply_snapshot(snap);
    again
}

#[test]
fn banks_survive_a_save_and_a_reload_and_reach_the_client() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    let again = reload(reg.to_snapshot());
    assert_eq!(banks(&again), map(&[(A, &[B, C])]));
    // `ship.Board` / `GetAllInfo` shipState: wbData is the third field.
    let PyValue::Tuple(v) = ship_state(&again, SHIP) else {
        panic!("shipState is a tuple");
    };
    assert_eq!(v[2], bank_value(&map(&[(A, &[B, C])])));
}

#[test]
fn a_member_unfitted_while_down_is_dropped_on_load() {
    let mut reg = world();
    let t = now();
    link_weapons(&mut reg, Some(ALICE), A, B, t).unwrap();
    link_weapons(&mut reg, Some(ALICE), A, C, t).unwrap();
    let mut snap = reg.to_snapshot();
    // C was taken out of the rack; the file still has the bank row.
    snap.items.retain(|i| i.item_id != C);
    let mut again = reload(snap.clone());
    assert_eq!(banks(&again), map(&[(A, &[B])]));
    // The trimmed bank is what the next save carries.
    let back = reload(again.to_snapshot());
    assert_eq!(banks(&back), map(&[(A, &[B])]));
    // The master moved to a medium slot: nothing is left of the bank.
    let mut snap = reg.to_snapshot();
    snap.items
        .iter_mut()
        .filter(|i| i.item_id == A)
        .for_each(|i| i.flag_id = MED);
    snap.items.retain(|i| i.item_id != B);
    let mut gone = reload(snap);
    assert!(banks(&gone).is_empty());
    assert!(gone.deltas().of(KIND).is_empty());
    let _ = (&mut again, &mut gone);
}

#[test]
fn a_destroyed_or_traded_ship_loses_its_banks_on_load() {
    let mut reg = world();
    link_weapons(&mut reg, Some(ALICE), A, B, now()).unwrap();
    let mut snap = reg.to_snapshot();
    snap.items.retain(|i| i.item_id != SHIP);
    let again = reload(snap);
    assert!(banks(&again).is_empty());
    assert!(again.deltas().of(KIND).is_empty());
    // Repackaged: the modules are no longer in the ship.
    let mut snap = reg.to_snapshot();
    snap.items
        .iter_mut()
        .filter(|i| i.item_id == A || i.item_id == B)
        .for_each(|i| i.location_id = 1);
    assert!(banks(&reload(snap)).is_empty());
}

#[test]
fn unlinking_removes_the_stored_row() {
    let mut reg = world();
    link_weapons(&mut reg, Some(ALICE), A, B, now()).unwrap();
    assert_eq!(reg.deltas().of(KIND).len(), 1);
    destroy_bank(&mut reg, Some(ALICE), A).unwrap();
    assert!(reg.deltas().of(KIND).is_empty());
}
