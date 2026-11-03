//! SYS-4: the module-attribute sweep. A module's number must reach the code
//! that uses it through the ship's dogma sheet, not the type row, or a
//! beacon (or a skill, a hull bonus, a rig) never moves it.
//!
//! The attributes swept (ids are the client's own vocabulary [V], named in
//! `typedata`): `shieldBonus` 68, `armorDamageAmount` 84,
//! `powerTransferAmount` 90, `energyNeutralizerAmount` 97, `empFieldRange` 99,
//! plus the heat pair that `overheat` reads: `heatDamage` 1211 and the
//! per-module heat added to its rack, attribute 1180 (the client's
//! `attributeHeatAbsorbtionRateModifier`). Every id of a type, effect and
//! system below is invented, and so is every number.
//!
//! Where each consumer is covered:
//! - the sheet itself, all seven: [`a_beacon_moves_every_swept_attribute`];
//! - `tackle::Live` (the one door `remote_rep` and `ewar` read through) and
//!   `remote_rep::strength`, in a world: [`the_live_door_and_the_reps_read_the_beacon`];
//! - `smartbomb`: `smartbomb/tests.rs`; `overheat`: `overheat/tests.rs`.

use super::*;

const GROUP: i64 = 77;
const MODULE: i64 = 990_504_001;
const BEACON: i64 = 990_504_101;
const FX: i64 = 990_504_102;
const SYSTEM: i64 = 990_504_201;
const SHIP: i64 = 990_504_301;
const ITEM: i64 = 990_504_302;
const ALICE: i64 = 90_504_001;
/// The multiplier every modifier of the invented beacon carries.
const X: f64 = 2.0;
/// `(attribute, the type's own value)`.
const SWEPT: [(i64, f64); 7] = [
    (68, 10.0),
    (84, 20.0),
    (90, 30.0),
    (97, 40.0),
    (99, 50.0),
    (1211, 4.0),
    (1180, 0.5),
];

fn rows() -> String {
    let attrs: Vec<String> = SWEPT.iter().map(|(a, v)| format!("{a}={v:?}")).collect();
    let mods: String = SWEPT
        .iter()
        .map(|(a, _)| {
            format!("modifier\t{FX}\tshipID\tLocationGroupModifier\t{a}\t99401\t4\t\t{GROUP}\n")
        })
        .collect();
    format!(
        "{}type\t{MODULE}\t{GROUP}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t{}\tInvented Module\n\
         type\t{BEACON}\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{FX}\t99401={X:?}\tInvented Beacon\n\
         effect\t{FX}\t7\t0\t0\t0\t0\t0\t0\tinventedSweepBeacon\t\n{mods}",
        crate::typedata::FIXTURE,
        attrs.join(";"),
    )
}

fn table() -> Table {
    Table::parse(&rows()).expect("the sweep rows parse")
}

fn module(state: State) -> ModuleInput {
    ModuleInput {
        item_id: ITEM,
        type_id: MODULE,
        state,
        charge_type: None,
    }
}

fn sheet_in(beacon: Option<i64>) -> Sheet {
    evaluate(
        &table(),
        &Inputs {
            ship_type: 990_000_001,
            modules: vec![module(State::Online)],
            beacon,
            ..Inputs::default()
        },
    )
    .expect("the invented hull is in the fixture")
}

/// The sheet-level sweep: with the beacon every swept attribute is the type's
/// value times the beacon's, without it the type's own.
#[test]
fn a_beacon_moves_every_swept_attribute() {
    let plain = sheet_in(None);
    let hot = sheet_in(Some(BEACON));
    for (a, v) in SWEPT {
        assert_eq!(plain.module(ITEM, a), Some(v), "{a} unmodified");
        assert_eq!(hot.module(ITEM, a), Some(v * X), "{a} under the beacon");
    }
}

/// A world with Alice flying a hull with one swept module fitted and running;
/// in `system`.
fn world_in(system: i64) -> (Registry, crate::modules::Running) {
    let alice = crate::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: SHIP,
        ship_type_id: 990_000_001,
        ..crate::model::Character::default()
    };
    let mut content = crate::content::Content::fixture();
    content.insert(
        crate::typedata::TABLE.name,
        crate::typedata::Installed::build(table()),
    );
    content.insert(
        crate::system_effect::TABLE.name,
        crate::system_effect::Effects::parse(&[format!("{SYSTEM}\t{BEACON}\t\t\t0.0\t0.0\t0.0")])
            .expect("the beacon row parses"),
    );
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![alice],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(content));
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = 990_000_001;
        i.group_id = 25;
    }
    items.push(crate::item::Item {
        item_id: ITEM,
        type_id: MODULE,
        owner_id: ALICE,
        location_id: SHIP,
        flag_id: 27,
        quantity: 1,
        group_id: GROUP,
        category_id: 7,
        singleton: true,
    });
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: ITEM + 100,
        ..crate::store::Snapshot::default()
    });
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    reg.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            ALICE,
            SHIP,
            system,
            crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    let running = crate::modules::Running {
        character_id: ALICE,
        ship_id: SHIP,
        module_id: ITEM,
        type_id: MODULE,
        effect: crate::modules::Effect {
            effect_id: 990_504_900,
            name: "inventedActive",
            category: 1,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        },
        target_id: None,
        started: 0,
        period: 1,
        repeat: 1,
        cycles: 1,
        stop_at: None,
    };
    (reg, running)
}

/// The world-level sweep: what the consumers read in a beacon system and out
/// of it. The control (out of it) is what makes the check able to fail.
#[test]
fn the_live_door_and_the_reps_read_the_beacon() {
    use crate::tackle::{Data, Live};
    for (system, factor) in [(SYSTEM, X), (990_504_999, 1.0)] {
        let (reg, r) = world_in(system);
        // The door `remote_rep` and `ewar` read through.
        for (a, v) in &SWEPT[..5] {
            assert_eq!(Live.module_attr(&reg, &r, *a), Some(v * factor), "{a}");
        }
        // remote_rep's own amounts: the local boosters and a remote cap.
        let me = *reg.roster().get(ALICE).expect("in space");
        for (effect, a) in [
            (crate::remote_rep::SHIELD_BOOSTING, 68),
            (crate::remote_rep::ARMOR_REPAIR, 84),
            (crate::remote_rep::REMOTE_CAPACITOR, 90),
        ] {
            let spec = crate::remote_rep::spec_of(effect).expect("a rep");
            let want = SWEPT.iter().find(|(x, _)| *x == a).unwrap().1 * factor;
            assert_eq!(
                crate::remote_rep::strength(&reg, &r, spec, &me, &Live, None),
                Some(want),
                "{effect} reads {a}"
            );
        }
    }
}

/// **The cost the review asked for** (`cargo test -p eve-server --release
/// dogma::sweep::cost -- --ignored --nocapture`): `dogma::hull` for a ship
/// with no pilot (an NPC), for a piloted one, the SYS-2 `beacon_inputs` that
/// every sheet read now makes, and the SYS-4 reads of `overheat` (a module's
/// attribute through the sheet against the type row it read before).
#[test]
#[ignore = "a measurement, not a check"]
fn cost() {
    use std::time::Instant;
    const N: u32 = 200_000;
    let (mut reg, _) = world_in(990_504_999);
    // A second hull of the same type with no pilot: an NPC's.
    let npc = SHIP + 50;
    let mut items = reg.store().items().to_vec();
    items.push(crate::item::Item {
        item_id: npc,
        type_id: 990_000_001,
        owner_id: 1,
        location_id: 30_000_001,
        flag_id: 0,
        quantity: 1,
        group_id: 25,
        category_id: crate::typedata::CATEGORY_SHIP,
        singleton: true,
    });
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: npc + 1,
        ..crate::store::Snapshot::default()
    });
    let per = |label: &str, f: &dyn Fn()| {
        f();
        let t = Instant::now();
        for _ in 0..N {
            f();
        }
        eprintln!(
            "{label}: {:.0} ns/call",
            t.elapsed().as_nanos() as f64 / f64::from(N)
        );
    };
    per("dogma::hull, NPC (no pilot)", &|| {
        std::hint::black_box(hull(&reg, npc));
    });
    per("dogma::hull, piloted ship", &|| {
        std::hint::black_box(hull(&reg, SHIP));
    });
    per("beacon_inputs, NPC", &|| {
        std::hint::black_box(beacon_inputs(&reg, npc));
    });
    per("beacon_inputs, piloted ship", &|| {
        std::hint::black_box(beacon_inputs(&reg, SHIP));
    });
    per("overheat: module attr through the sheet (SYS-4)", &|| {
        std::hint::black_box(crate::overheat::module_attr_for_bench(
            &reg, SHIP, ITEM, 1211,
        ));
    });
    per("overheat: the type row it read before", &|| {
        std::hint::black_box(crate::typedata::attribute_in(reg.content(), MODULE, 1211));
    });
}
