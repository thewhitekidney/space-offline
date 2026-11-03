//! The smartbomb's tests. **Every id and number is invented**: types
//! 990,025,xxx, systems 30,025,xxx, a field of five kilometres and a volley of
//! round numbers. The effect id appears by name only.

use super::*;
use crate::celestial::Universe;
use crate::modules::MSEC;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
const SYSTEM: i64 = 30_025_001;
const ALICE: i64 = 90_025_001;
const BOB: i64 = 90_025_002;
const CAROL: i64 = 90_025_003;
const CORP: i64 = 98_025_001;

const BOMB: i64 = 990_025_001;
const DUD: i64 = 990_025_002;
const FRIGATE: i64 = 990_000_001;
/// SYS-4: a system with an invented effect beacon that multiplies the field
/// of every group 72 module on the ship, and the multiplier.
const BEACON_SYSTEM: i64 = 30_025_002;
const BEACON: i64 = 990_025_101;
const BEACON_FX: i64 = 990_025_102;
const BEACON_X: f64 = 5.0;
const ALICE_BOMB: i64 = 100_025_101;

/// Invented: the field's reach, metres, and the module's cycle, ms.
const RANGE: f64 = 5_000.0;
const CYCLE_MS: i64 = 10_000;
const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_025_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let row = format!(
        "{SYSTEM}\t19000001\t29000001\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {SYSTEM}"
    );
    c.insert(
        "universe",
        Universe::parse(
            &[row],
            &["19000001\t\t\t\t\tFixture region".to_owned()],
            &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
        )
        .expect("the invented rows parse"),
    );
    let rows = format!(
        "{}type\t{BOMB}\t72\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t38\t12,16,38\t\
         73={CYCLE_MS}.0;{ATTR_RANGE}={RANGE};114=0.0;116=0.0;117=250.0;118=0.0\tInvented Smartbomb\n\
         type\t{DUD}\t72\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t38\t12,16,38\t\
         73={CYCLE_MS}.0;{ATTR_RANGE}={RANGE};114=0.0;116=0.0;117=0.0;118=0.0\tInvented Smartbomb Without Damage\n\
         type\t{BEACON}\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{BEACON_FX}\t99401={BEACON_X:?}\tInvented Range Beacon\n\
         effect\t{BEACON_FX}\t7\t0\t0\t0\t0\t0\t0\tinventedBeaconEffect\t\n\
         modifier\t{BEACON_FX}\tshipID\tLocationGroupModifier\t{ATTR_RANGE}\t99401\t4\t\t72\n\
         effect\t38\t1\t1\t73\t0\t99\t0\t0\tempWave\teffects.EMPWave\t0\n\
         effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c.insert(
        crate::system_effect::TABLE.name,
        crate::system_effect::Effects::parse(&[format!(
            "{BEACON_SYSTEM}\t{BEACON}\t\t\t0.0\t0.0\t0.0"
        )])
        .expect("the beacon row parses"),
    );
    c
}

fn place(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        SYSTEM,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// Alice with a smartbomb, Bob 3 km away, Carol 20 km away; all in a null
/// system, all at rest.
fn world() -> Registry {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        corporation_id: CORP,
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if [ALICE, BOB, CAROL].iter().any(|p| i.item_id == ship(*p)) {
            i.type_id = FRIGATE;
            i.group_id = 25;
        }
    }
    let t = crate::item::find_in(&content, BOMB).expect("a test type");
    items.push(crate::item::Item {
        item_id: ALICE_BOMB,
        type_id: BOMB,
        owner_id: ALICE,
        location_id: ship(ALICE),
        flag_id: HI_SLOT_0,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    });
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_026_000,
        ..crate::store::Snapshot::default()
    });
    for p in [ALICE, BOB, CAROL] {
        reg.character_mut(p).expect("a pilot").ship_type_id = FRIGATE;
    }
    place(&mut reg, ALICE, [0.0; 3]);
    place(&mut reg, BOB, [3_000.0, 0.0, 0.0]);
    place(&mut reg, CAROL, [20_000.0, 0.0, 0.0]);
    let _ = reg.take_pending();
    reg
}

fn cycle(type_id: i64, index: i64) -> Cycle {
    Cycle {
        character_id: ALICE,
        ship_id: ship(ALICE),
        module_id: ALICE_BOMB,
        type_id,
        effect_id: EFFECT,
        target_id: None,
        at: T0,
        index,
        charge_type_id: None,
    }
}

fn hurt(reg: &Registry, who: i64) -> bool {
    reg.combat().ship(ship(who)).is_some()
}

#[test]
fn a_cycle_hits_every_ship_in_the_field_but_not_the_bomber_or_the_far() {
    let mut reg = world();
    let hit = on_cycle(&mut reg, &cycle(BOMB, 0), T0);
    assert_eq!(hit, 1);
    assert!(hurt(&reg, BOB), "Bob is 3 km out");
    assert!(!hurt(&reg, ALICE), "never the bomber's own ship");
    assert!(!hurt(&reg, CAROL), "Carol is 20 km out");
}

#[test]
fn the_field_reaches_exactly_its_range_and_a_ship_in_warp_is_spared() {
    let mut reg = world();
    place(&mut reg, CAROL, [RANGE, 0.0, 0.0]);
    let who: Vec<i64> = victims(&reg, &cycle(BOMB, 0))
        .iter()
        .map(|p| p.character_id)
        .collect();
    assert!(who.contains(&CAROL) && who.contains(&BOB), "{who:?}");
    reg.roster_mut().get_mut(BOB).expect("in space").motion.mode = crate::destiny::Mode::Warp {
        goto: [1.0e9, 0.0, 0.0],
        effect_stamp: crate::destiny::tick() as i32 - 5,
        total_distance: 1.0e9,
        stop_distance: 0.0,
        warp_factor: 4_000,
    };
    let who: Vec<i64> = victims(&reg, &cycle(BOMB, 0))
        .iter()
        .map(|p| p.character_id)
        .collect();
    assert_eq!(who, vec![CAROL], "Bob is in the tunnel");
}

#[test]
fn a_module_that_does_no_damage_hits_nobody() {
    let mut reg = world();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ALICE_BOMB) {
        i.type_id = DUD;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_026_000,
        ..crate::store::Snapshot::default()
    });
    assert_eq!(on_cycle(&mut reg, &cycle(DUD, 0), T0), 0);
    assert!(!hurt(&reg, BOB));
}

#[test]
fn a_volley_is_aggression_against_the_victim() {
    let mut reg = world();
    on_cycle(&mut reg, &cycle(BOMB, 0), T0);
    let cw = reg.combat().crimewatch();
    assert!(
        cw.expiry(ALICE, crate::crimewatch::Timer::Weapons, T0)
            .is_some()
    );
    assert!(cw.expiry(BOB, crate::crimewatch::Timer::Pvp, T0).is_some());
    assert!(
        cw.expiry(CAROL, crate::crimewatch::Timer::Pvp, T0)
            .is_none()
    );
}

#[test]
fn the_module_bombs_once_a_cycle_through_the_clock() {
    let mut reg = world();
    crate::modules::activate(&mut reg, Some(ALICE), ALICE_BOMB, "empWave", None, 3, T0)
        .expect("a smartbomb needs no target");
    crate::modules::tick(&mut reg, T0);
    crate::combat::tick(&mut reg, T0);
    assert!(hurt(&reg, BOB), "the first cycle lands at activation");
    let first = reg.combat().ship(ship(BOB)).map(|r| r.bars(T0));
    crate::modules::tick(&mut reg, T0 + CYCLE_MS * MSEC);
    crate::combat::tick(&mut reg, T0 + CYCLE_MS * MSEC);
    let second = reg
        .combat()
        .ship(ship(BOB))
        .map(|r| r.bars(T0 + CYCLE_MS * MSEC));
    assert_ne!(first, second, "and again on the next cycle");
    assert!(!hurt(&reg, CAROL));
}

const DRONE_BOB: i64 = 100_025_201;
const DRONE_FAR: i64 = 100_025_203;

/// A combat drone of `owner` flying at `at`, rostered under its own item id
/// as `drones::launch` does.
fn drone(reg: &mut Registry, id: i64, owner: i64, at: [f64; 3]) {
    reg.roster_mut().arrive(Presence::arriving(
        id,
        id,
        SYSTEM,
        Motion::arrived(at, [0.0; 3]),
    ));
    let d =
        crate::drones::Drone::launched(id, crate::drones::HOBGOBLIN_I, owner, ship(owner), SYSTEM);
    reg.modules_mut().drones_mut().insert(d);
}

/// Smartbombs hit drones is the owner's decision (#127); whether the bomber's
/// own drones are hit, and what a hit drone does to its owner's timers, are
/// [A] in `smartbomb.rs` and asserted nowhere.
#[test]
fn a_cycle_hits_the_drones_in_the_field() {
    let mut reg = world();
    drone(&mut reg, DRONE_BOB, BOB, [2_000.0, 0.0, 0.0]);
    drone(&mut reg, DRONE_FAR, BOB, [30_000.0, 0.0, 0.0]);
    let hit = on_cycle(&mut reg, &cycle(BOMB, 0), T0);
    assert_eq!(hit, 2, "Bob and his drone in the field");
    assert!(reg.combat().ship(DRONE_BOB).is_some(), "Bob's drone");
    assert!(reg.combat().ship(DRONE_FAR).is_none(), "out of the field");
}

#[test]
fn a_drone_the_bomb_kills_leaves_space() {
    let mut reg = world();
    drone(&mut reg, DRONE_BOB, BOB, [2_000.0, 0.0, 0.0]);
    for i in 0..40 {
        on_cycle(&mut reg, &cycle(BOMB, i), T0 + i * 10_000 * MSEC);
        if reg.roster().get(DRONE_BOB).is_none() {
            break;
        }
    }
    assert!(reg.roster().get(DRONE_BOB).is_none(), "shot down");
    assert!(reg.modules().drones().get(DRONE_BOB).is_none());
}

/// SYS-4: the field's reach is the module's modified range, so a beacon that
/// multiplies it takes in a ship the type's own range leaves out.
#[test]
fn a_beacon_that_widens_the_field_takes_in_the_far_ship() {
    let mut reg = world();
    // Carol at 20 km is out of the 5 km field; in the beacon's system the
    // same field is five times as wide.
    let far_out = |reg: &mut Registry, system: i64| {
        for (who, x) in [(ALICE, 0.0), (BOB, 3_000.0), (CAROL, 20_000.0)] {
            let _ = reg.roster_mut().leave(who);
            reg.roster_mut().arrive(Presence::arriving(
                who,
                ship(who),
                system,
                Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
            ));
        }
        let mut who: Vec<i64> = victims(reg, &cycle(BOMB, 0))
            .iter()
            .map(|p| p.character_id)
            .collect();
        who.sort();
        who
    };
    assert_eq!(far_out(&mut reg, SYSTEM), vec![BOB], "the type's range");
    assert_eq!(
        far_out(&mut reg, BEACON_SYSTEM),
        vec![BOB, CAROL],
        "five times the range under the beacon"
    );
}
