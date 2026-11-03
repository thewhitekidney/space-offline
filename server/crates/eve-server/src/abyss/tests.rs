//! The Abyss's tests. **Every id here is invented**: dungeons 990,9xx,
//! types 990,9xx, abyssal systems 32,880,xxx, pilots 90,880,xxx. The client's
//! constants appear only by name ([`ARCHETYPE`], [`EXIT_LOCATOR`], the gate
//! types, the error codes); no `[A]` value is asserted.

use super::*;
use crate::celestial::Universe;
use crate::content::Content;
use crate::dungeon_data::{Object, Room, TypeList};
use crate::item::Item;
use crate::model::{Character, SandboxConfig};
use crate::notify::Push;
use crate::space_state::{Motion, Presence};
use std::sync::Arc;

const HOME: i64 = 30_880_001;
const HIGH: i64 = 30_880_002;
const ABY: [i64; 6] = [
    32_880_001, 32_880_002, 32_880_003, 32_880_004, 32_880_005, 32_880_006,
];
const ALICE: i64 = 90_880_001;
const BOB: i64 = 90_880_002;
const CAROL: i64 = 90_880_003;
const ROOM_A: i64 = 990_901;
const ROOM_B: i64 = 990_902;
const ENTRY_TYPE: i64 = 990_910;
const OTHER_TYPE: i64 = 990_911;
const CRUISER: i64 = 990_921;
const FRIGATE: i64 = 990_922;
const G_CRUISER: i64 = 990_931;
const G_FRIGATE: i64 = 990_932;
const KEY_TYPE: i64 = 990_941;
const KEY_TIER: i64 = 3;
const NOT_KEY_TYPE: i64 = 990_942;
const KEY_ITEM: i64 = 100_880_901;
const NOT_KEY_ITEM: i64 = 100_880_902;

fn ship(who: i64) -> i64 {
    100_000_000 + who
}

fn object(room: i64, id: i64, type_id: i64, pos: [f64; 3]) -> Object {
    Object {
        room,
        id,
        type_id,
        pos,
        ..Object::default()
    }
}

/// A valid room: entry at (0, 0, -35), exit locators at ids 12 and 11.
fn room(id: i64) -> Dungeon {
    Dungeon {
        id,
        archetype: ARCHETYPE,
        entry_object: Some(10),
        entry_type: Some(ENTRY_TYPE),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![
            object(1, 10, ENTRY_TYPE, [0.0, 0.0, -35.0]),
            object(1, 12, EXIT_LOCATOR, [9.0, 9.0, 9.0]),
            object(1, 11, EXIT_LOCATOR, [0.0, 0.0, 24.0]),
            object(1, 13, OTHER_TYPE, [1.0, 2.0, 3.0]),
            object(1, 14, *AOE_LOCATORS.start(), [5.0, 5.0, 5.0]),
        ],
        ..Dungeon::default()
    }
}

fn table(list: Vec<Dungeon>) -> Table {
    let mut t = Table {
        dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
        ..Table::default()
    };
    let by_group = |g: &[i64]| TypeList {
        inc_groups: g.to_vec(),
        ..TypeList::default()
    };
    for (list, groups) in [
        (57, vec![G_CRUISER]),
        (92, vec![G_FRIGATE]),
        (233, vec![G_FRIGATE, G_CRUISER]),
        (TARGETS_LIST, vec![crate::npc::FIXTURE_GROUP_FRIGATE]),
    ] {
        t.typelists.insert(list, by_group(&groups));
    }
    t
}

fn universe() -> Universe {
    let sys = |id: i64, region: i64, sec: &str, name: &str| {
        format!("{id}\t{region}\t22880001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let mut rows = vec![
        sys(HOME, 10_880_001, "-0.45", "Invented Home"),
        sys(HIGH, 10_880_001, "0.9", "Invented High"),
    ];
    for (n, id) in ABY.iter().enumerate().rev() {
        rows.push(sys(*id, 12_880_001, "-0.99", &format!("Invented AD-{n}")));
    }
    Universe::parse(
        &rows,
        &[
            "10880001\t\t\t\t\tInvented Home".to_owned(),
            "12880001\t\t\t\t\tInvented Depths".to_owned(),
        ],
        &["22880001\t12880001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses")
}

fn typedata() -> crate::typedata::Installed {
    let rows = format!(
        "{}type\t{KEY_TYPE}\t{KEY_GROUP}\t4\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\
         {ATTR_TIER}={KEY_TIER}.0;{ATTR_WEATHER}=990950.0\tInvented Filament\n",
        crate::typedata::FIXTURE
    );
    crate::typedata::Installed::build(
        crate::typedata::Table::parse(&rows).expect("the test rows parse"),
    )
}

fn content(dungeons: Table) -> Arc<Content> {
    let mut c = Content::fixture();
    c.insert("universe", universe());
    c.insert("dungeons", dungeons);
    c.insert("typedata", typedata());
    Arc::new(c)
}

/// A world of three pilots in HOME, each with a ship of group `group`; Alice
/// has `keys` filaments in the cargo, and a thing that is not a key.
fn world_with(group: i64, keys: i64) -> Registry {
    let mut reg = Registry::new(SandboxConfig {
        characters: [ALICE, BOB, CAROL]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: ship(id),
                solar_system_id: Some(HOME),
                ..Character::default()
            })
            .collect(),
        ..SandboxConfig::default()
    })
    .with_content(content(table(vec![room(ROOM_A), room(ROOM_B)])));
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id >= ship(ALICE)) {
        i.group_id = group;
        i.type_id = if group == G_FRIGATE { FRIGATE } else { CRUISER };
    }
    let cargo = |item_id: i64, type_id: i64, group_id: i64, quantity: i64| Item {
        item_id,
        type_id,
        owner_id: ALICE,
        location_id: ship(ALICE),
        flag_id: 5,
        quantity,
        group_id,
        category_id: 4,
        singleton: false,
    };
    items.push(cargo(KEY_ITEM, KEY_TYPE, KEY_GROUP, keys));
    items.push(cargo(NOT_KEY_ITEM, NOT_KEY_TYPE, 990_943, 1));
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_000,
        ..crate::store::Snapshot::default()
    });
    for (client, who) in [(1, ALICE), (2, BOB), (3, CAROL)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
    }
    reg
}

fn world() -> Registry {
    world_with(G_CRUISER, 2)
}

fn stand(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// The bind: the pilot's arrival is taken and the roster has them there.
fn seat(reg: &mut Registry, who: i64) {
    let p = pocket(reg, who);
    crate::jump::arm_arrival(reg, who, p.system, p.entry);
    let _ = crate::jump::take_arrival(reg, who);
    stand(reg, who, p.system, p.entry);
}

/// A jump that has landed: the session change gone, the bind taken.
fn land(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    crate::jump::arm_arrival(reg, who, system, at);
    let _ = crate::jump::take_arrival(reg, who);
    stand(reg, who, system, at);
}

type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

fn sent(reg: &mut Registry) -> Vec<Sent> {
    reg.take_pending()
        .into_iter()
        .map(|(_, p)| {
            let (event, args) = p.to_push();
            (event, args, p.audience())
        })
        .collect()
}

const NOW: BlueTime = 133_000_000_000_000_000;
const T: i64 = FILETIME_PER_S;

fn call(
    reg: &mut Registry,
    who: Option<i64>,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    dispatch(
        reg,
        &Request {
            service: "abyssalMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: who,
        },
    )
}

fn gate_call(
    reg: &mut Registry,
    who: i64,
    method: &str,
    gate: i64,
) -> Result<PyValue, ServiceError> {
    call(reg, Some(who), method, &[PyValue::Int(gate)])
}

/// The `AbyssError` codes a refusal carries, in order.
fn codes(e: &ServiceError) -> Vec<i64> {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("a raised user error, got {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("header")
    };
    let Some(PyValue::Tuple(a)) = h.get(1) else {
        panic!("args")
    };
    assert_eq!(a[0], PyValue::Bytes(b"AbyssError".to_vec()), "{a:?}");
    let PyValue::Dict(d) = &a[1] else {
        panic!("dict")
    };
    let Some((PyValue::List(list), _)) = d.first() else {
        panic!("errors")
    };
    list.iter()
        .map(|t| match t {
            PyValue::Tuple(p) => match p[0] {
                PyValue::Int(c) => c,
                _ => panic!("a code"),
            },
            _ => panic!("a pair"),
        })
        .collect()
}

fn deploy_call(reg: &mut Registry, who: i64, mode: Mode) -> Result<PyValue, ServiceError> {
    call(
        reg,
        Some(who),
        "AbyssalEntranceDeployment",
        &[PyValue::Int(KEY_ITEM), PyValue::Int(mode.wire())],
    )
}

fn cargo_of_key(reg: &Registry) -> Option<i64> {
    reg.store().get(KEY_ITEM).map(|i| i.quantity)
}

fn pocket(reg: &Registry, who: i64) -> Pocket {
    let run = run_of(reg, who).expect("a run");
    run.pocket_of(run.pilot(who).expect("a pilot"))
        .expect("a pocket")
        .clone()
}

// ---- the pure rules ---------------------------------------------------------

#[test]
fn a_room_needs_its_archetype_one_room_an_entry_and_an_exit() {
    let sp = spots(&room(ROOM_A)).expect("a valid room");
    assert_eq!(sp.entry, [0.0, 0.0, -35.0]);
    assert_eq!(sp.exit, [0.0, 0.0, 24.0], "the lowest locator id");
    let mut d = room(ROOM_A);
    d.archetype = 1;
    assert!(spots(&d).is_none());
    let mut d = room(ROOM_A);
    d.objects.retain(|o| o.type_id != EXIT_LOCATOR);
    assert!(spots(&d).is_none(), "no exit");
    let mut d = room(ROOM_A);
    d.objects.push(object(2, 20, OTHER_TYPE, [0.0; 3]));
    assert!(spots(&d).is_none(), "two rooms");
}

#[test]
fn the_rooms_are_the_valid_ones_in_dungeon_order_and_the_pick_is_stable() {
    let mut bad = room(990_900);
    bad.archetype = 2;
    let t = table(vec![room(ROOM_B), bad, room(ROOM_A)]);
    let list = rooms(&t);
    assert_eq!(
        list.iter().map(|r| r.0).collect::<Vec<_>>(),
        [ROOM_A, ROOM_B]
    );
    assert_eq!(pick_room(&list, 7), pick_room(&list, 7));
    assert!(pick_room(&[], 7).is_none());
}

#[test]
fn only_pve_rooms_are_drawn() {
    let mut faction = room(990_903);
    faction.faction = Some(500_001);
    let mut no_aoe = room(990_904);
    no_aoe
        .objects
        .retain(|o| !AOE_LOCATORS.contains(&o.type_id));
    let t = table(vec![room(ROOM_A), faction, no_aoe]);
    assert_eq!(rooms(&t).iter().map(|r| r.0).collect::<Vec<_>>(), [ROOM_A]);
}

#[test]
fn stray_objects_are_the_pvp_gate_and_nodes() {
    let c = Content::fixture();
    assert!(is_stray(&c, PVP_GATE));
    assert!(!is_stray(&c, ENCOUNTER_GATE));
}

#[test]
fn a_run_end_clears_the_abyss_timer_not_the_weapons_one() {
    let f = Finished {
        to: ALICE,
        content_id: 5,
    };
    let (_, args) = f.to_push();
    assert_eq!(args[1], PyValue::Int(TIMER_ABYSSAL_CONTENT_EXPIRATION));
    assert_ne!(args[1], PyValue::Int(0));
}

#[test]
fn a_free_system_is_the_lowest_abyssal_one_nobody_holds() {
    let u = universe();
    let mut taken = BTreeSet::new();
    assert_eq!(free_system(&u, &taken), Some(ABY[0]));
    taken.insert(ABY[0]);
    assert_eq!(free_system(&u, &taken), Some(ABY[1]));
    taken.extend(ABY);
    assert_eq!(free_system(&u, &taken), None);
}

#[test]
fn tiers_are_zero_to_six_the_special_keys_play_as_five_and_zero() {
    assert_eq!(tier_arg("0"), Some(0));
    assert_eq!(tier_arg("6"), Some(6));
    assert_eq!(tier_arg("7"), None);
    assert_eq!(tier_arg("x"), None);
    assert_eq!((play_tier(TIER_5_SHIPYARD), play_tier(TIER_0_DEMO)), (5, 0));
    assert_eq!(play_tier(3), 3);
    for t in -2..12 {
        assert!(nebula(t) > 0);
    }
}

#[test]
fn a_run_site_is_in_its_own_band() {
    assert!(is_abyss_site(SITE_BASE));
    assert!(is_abyss_site(SITE_END - 1));
    assert!(!is_abyss_site(SITE_END));
    assert!(!is_abyss_site(SITE_BASE - 1));
}

#[test]
fn the_modes_are_the_clients() {
    assert_eq!(
        [Mode::Solo, Mode::CoOp, Mode::TwoPlayer].map(|m| (
            m.wire(),
            m.ship_list(),
            m.key_stack(),
            m.max_players(),
            m.jumps_on_deploy(),
            m.fleet_required()
        )),
        [
            (1, 57, 1, 1, true, false),
            (2, 92, 3, 3, false, true),
            (3, 233, 2, 2, false, true)
        ]
    );
    assert_eq!(Mode::from_wire(2), Some(Mode::CoOp));
    assert_eq!(Mode::from_wire(-1), None);
    assert_eq!(Mode::CoOp.ship_error(), err::INVALID_SHIP_TYPE_THREE_PLAYER);
}

#[test]
fn the_security_limit_of_a_tier_is_the_clients() {
    assert_eq!(max_tier(1.0), 0);
    assert_eq!(max_tier(0.9), 0);
    assert_eq!(max_tier(0.8), 3);
    assert_eq!(max_tier(0.7), 4);
    assert_eq!(max_tier(0.6), 5);
    assert_eq!(max_tier(0.5), 6);
    assert_eq!(max_tier(0.4), 200);
    assert_eq!(max_tier(-0.99), 200);
}

#[test]
fn room_after_and_the_gate_a_room_ends_in() {
    assert_eq!(room_after(1, 3), Some(2));
    assert_eq!(room_after(2, 3), Some(3));
    assert_eq!(room_after(3, 3), None);
    assert_eq!(room_after(1, 1), None);
    assert_eq!(gate_type(1, 3), ENCOUNTER_GATE);
    assert_eq!(gate_type(2, 3), ENCOUNTER_GATE);
    assert_eq!(gate_type(3, 3), EXIT_GATE);
    assert_eq!(gate_type(1, 1), EXIT_GATE);
}

#[test]
fn a_pocket_is_out_of_time_at_its_end_and_never_before_it_starts() {
    assert!(!expired(0, NOW), "not started");
    assert!(!expired(NOW + 1, NOW));
    assert!(expired(NOW, NOW));
    assert!(expired(NOW - 1, NOW));
}

fn clean() -> Facts {
    Facts {
        security: -0.45,
        key_tier: 3,
        ship_allowed: true,
        ..Facts::default()
    }
}

fn key(stack: i64) -> Option<KeyFacts> {
    Some(KeyFacts {
        type_id: KEY_TYPE,
        is_key: true,
        owned: true,
        reachable: true,
        stack,
    })
}

fn deploy_ok() -> DeployFacts {
    DeployFacts {
        in_fleet: true,
        key: key(3),
        ..DeployFacts::default()
    }
}

#[test]
fn a_clean_deploy_passes_in_every_mode() {
    for m in [Mode::Solo, Mode::CoOp, Mode::TwoPlayer] {
        assert!(deploy_errors(m, &clean(), &deploy_ok()).is_empty(), "{m:?}");
    }
}

#[test]
fn every_base_check_raises_its_code_in_the_clients_order() {
    let f = Facts {
        ship_allowed: false,
        docked: true,
        cloaked: true,
        invulnerable: true,
        in_encounter: true,
        pvp_timer: true,
        self_destructing: true,
        tethered: true,
        warping: true,
        scrambled: true,
        in_abyss_system: true,
        session_change: true,
        security: 0.9,
        ..clean()
    };
    let got: Vec<i64> = deploy_errors(Mode::Solo, &f, &deploy_ok())
        .iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(
        got,
        [
            err::VERY_SECURE_SYSTEM_DISALLOWED,
            err::INVALID_SHIP_TYPE,
            err::SHIP_IS_DOCKED,
            err::SHIP_IS_CLOAKED,
            err::SHIP_IS_INVULNERABLE,
            err::ABYSS_CONTENT_ACTIVE,
            err::PVP_TIMER_ACTIVE,
            err::SHIP_IS_SELF_DESTRUCTING,
            err::SHIP_IS_TETHERED,
            err::SHIP_IN_WARP,
            err::SHIP_IS_WARP_SCRAMBLED,
            err::IN_ABYSS_SYSTEM,
            err::SESSION_CHANGE_IN_PROGRESS
        ]
    );
}

#[test]
fn a_wrong_ship_names_its_modes_code() {
    let f = Facts {
        ship_allowed: false,
        ..clean()
    };
    let code = |m| deploy_errors(m, &f, &deploy_ok())[0].0;
    assert_eq!(code(Mode::Solo), err::INVALID_SHIP_TYPE);
    assert_eq!(code(Mode::CoOp), err::INVALID_SHIP_TYPE_THREE_PLAYER);
    assert_eq!(code(Mode::TwoPlayer), err::INVALID_SHIP_TYPE_TWO_PLAYER);
}

#[test]
fn the_deploy_checks_read_the_fleet_the_key_the_range_and_the_safety() {
    let e = |m: Mode, f: &Facts, d: &DeployFacts| -> Vec<Refusal> { deploy_errors(m, f, d) };
    // A fleet is required for a fleet mode only.
    let lone = DeployFacts {
        in_fleet: false,
        ..deploy_ok()
    };
    assert!(e(Mode::Solo, &clean(), &lone).is_empty());
    assert_eq!(
        e(Mode::CoOp, &clean(), &lone),
        [(err::SHIP_NOT_IN_FLEET, vec![])]
    );
    // The key.
    let none = DeployFacts {
        key: None,
        ..deploy_ok()
    };
    assert_eq!(
        e(Mode::Solo, &clean(), &none),
        [(err::KEY_NOT_FOUND, vec![])]
    );
    let bad = DeployFacts {
        key: Some(KeyFacts {
            type_id: NOT_KEY_TYPE,
            is_key: false,
            owned: false,
            reachable: false,
            stack: 1,
        }),
        ..deploy_ok()
    };
    assert_eq!(
        e(Mode::Solo, &clean(), &bad),
        [
            (err::INVALID_KEY_TYPE, vec![]),
            (err::INVALID_OWNER, vec![]),
            (err::ITEM_NOT_REACHABLE, vec![NOT_KEY_TYPE])
        ]
    );
    assert_eq!(
        e(
            Mode::CoOp,
            &clean(),
            &DeployFacts {
                key: key(2),
                ..deploy_ok()
            }
        ),
        [(err::INVALID_KEY_AMOUNT, vec![3])]
    );
    // A restricted thing in range names its type; a demo key does not mind.
    let near = DeployFacts {
        closest_restricted: Some(990_960),
        in_dungeon: true,
        ..deploy_ok()
    };
    assert_eq!(
        e(Mode::Solo, &clean(), &near),
        [
            (err::PROXIMITY_RESTRICTION, vec![990_960]),
            (err::SHIP_IN_DUNGEON, vec![])
        ]
    );
    let demo = Facts {
        key_tier: TIER_0_DEMO,
        ..clean()
    };
    assert_eq!(
        e(Mode::Solo, &demo, &near),
        [(err::SHIP_IN_DUNGEON, vec![])]
    );
    // Safety blocks a key above the system's limit, only on green.
    let sus = Facts {
        security: 0.5,
        key_tier: 7,
        safety_green: true,
        ..clean()
    };
    assert_eq!(
        e(Mode::Solo, &sus, &deploy_ok()),
        [(err::SAFETY_ENGAGED, vec![])]
    );
    let yellow = Facts {
        safety_green: false,
        ..sus
    };
    assert!(e(Mode::Solo, &yellow, &deploy_ok()).is_empty());
}

#[test]
fn the_jump_checks_read_the_gate() {
    let open = JumpFacts {
        distance: Some(100.0),
        reach: 3_000.0,
        same_fleet: true,
        open: true,
        has_entered: false,
        full: false,
    };
    let e = |j: &JumpFacts| jump_errors(Mode::CoOp, &clean(), j);
    assert!(e(&open).is_empty());
    assert_eq!(
        e(&JumpFacts {
            distance: None,
            ..open.clone()
        }),
        [(err::ENTRANCE_MISSING, vec![])]
    );
    assert_eq!(
        e(&JumpFacts {
            distance: Some(9_000.0),
            ..open.clone()
        })[0]
            .0,
        err::PROXIMITY_ENTRANCE
    );
    let all = JumpFacts {
        same_fleet: false,
        open: false,
        has_entered: true,
        full: true,
        ..open
    };
    assert_eq!(
        e(&all).iter().map(|x| x.0).collect::<Vec<_>>(),
        [
            err::SHIP_NOT_IN_SAME_FLEET,
            err::ENTRANCE_CLOSED,
            err::SHIP_HAS_ENTERED,
            err::MAX_ENTERED
        ]
    );
}

#[test]
fn a_refusal_is_an_abyss_error_with_pairs_of_code_and_arguments() {
    let e = abyss_error(&[
        (err::ITEM_NOT_REACHABLE, vec![990_941]),
        (err::SHIP_IS_DOCKED, vec![]),
    ]);
    assert_eq!(codes(&e), [err::ITEM_NOT_REACHABLE, err::SHIP_IS_DOCKED]);
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = &e else {
        panic!()
    };
    assert!(format!("{header:?}").contains("990941"));
}

#[test]
fn the_pushes_have_the_clients_names_and_shapes() {
    let (name, args) = Entered {
        to: ALICE,
        content_id: 5,
        nebula_graphic_id: 6,
        tier: 3,
        room: 2,
    }
    .to_push();
    assert_eq!(name, "OnAbyssalLocationEntered");
    let d = &args[0];
    for (k, v) in [
        (&b"content_id"[..], 5),
        (b"nebula_graphic_id", 6),
        (b"difficulty_tier", 3),
        (b"location_id", 2),
    ] {
        assert_eq!(crate::pyutil::dict_get(d, k), Some(&PyValue::Int(v)));
    }
    let (name, args) = Expiry {
        to: ALICE,
        end_time: 9,
    }
    .to_push();
    assert_eq!(
        (name, args),
        (
            "OnAddExpiryDurationForAbyssalContent",
            vec![PyValue::Int(9)]
        )
    );
    let f = Finished {
        to: ALICE,
        content_id: 5,
    };
    assert_eq!(f.to_push().0, "OnAbyssalContentFinished");
    assert_eq!(f.audience(), Some(vec![ALICE]));
}

// ---- the deploy -------------------------------------------------------------

#[test]
fn a_deploy_is_refused_with_the_clients_codes_and_changes_nothing() {
    let mut reg = world();
    // Docked.
    let e = deploy_call(&mut reg, ALICE, Mode::Solo).unwrap_err();
    assert_eq!(codes(&e), [err::SHIP_IS_DOCKED]);
    stand(&mut reg, ALICE, HOME, [1.0, 2.0, 3.0]);
    // Not a key, and no such item.
    let e = call(
        &mut reg,
        Some(ALICE),
        "AbyssalEntranceDeployment",
        &[PyValue::Int(NOT_KEY_ITEM), PyValue::Int(1)],
    )
    .unwrap_err();
    assert_eq!(codes(&e), [err::INVALID_KEY_TYPE]);
    let e = call(
        &mut reg,
        Some(ALICE),
        "AbyssalEntranceDeployment",
        &[PyValue::Int(100_880_999), PyValue::Int(1)],
    )
    .unwrap_err();
    assert_eq!(codes(&e), [err::KEY_NOT_FOUND]);
    // An unknown mode.
    let e = call(
        &mut reg,
        Some(ALICE),
        "AbyssalEntranceDeployment",
        &[PyValue::Int(KEY_ITEM), PyValue::Int(-1)],
    )
    .unwrap_err();
    assert_eq!(codes(&e), [err::UNKNOWN_ERROR]);
    // Co-op wants a fleet, three keys and a frigate.
    let e = deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap_err();
    assert_eq!(
        codes(&e),
        [
            err::INVALID_SHIP_TYPE_THREE_PLAYER,
            err::SHIP_NOT_IN_FLEET,
            err::INVALID_KEY_AMOUNT
        ]
    );
    // Someone else's key.
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    let e = deploy_call(&mut reg, BOB, Mode::Solo).unwrap_err();
    assert!(codes(&e).contains(&err::INVALID_OWNER));
    assert!(codes(&e).contains(&err::ITEM_NOT_REACHABLE));
    // A high-security system admits no tier but 0.
    stand(&mut reg, ALICE, HIGH, [0.0; 3]);
    let e = deploy_call(&mut reg, ALICE, Mode::Solo).unwrap_err();
    assert_eq!(
        codes(&e),
        [err::VERY_SECURE_SYSTEM_DISALLOWED, err::SAFETY_ENGAGED]
    );
    // Nothing happened.
    assert_eq!(cargo_of_key(&reg), Some(2));
    assert_eq!(runs(&reg).count(), 0);
    assert!(!reg.jumps().contains(ALICE));
}

#[test]
fn a_ship_the_list_does_not_admit_is_refused_and_a_missing_list_admits_any() {
    let mut reg = world_with(G_FRIGATE, 2);
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    let e = deploy_call(&mut reg, ALICE, Mode::Solo).unwrap_err();
    assert_eq!(codes(&e), [err::INVALID_SHIP_TYPE]);
    // A bundle without the lists lets the ship through.
    let bare = Table {
        dungeons: [(ROOM_A, room(ROOM_A))].into(),
        ..Table::default()
    };
    let reg = reg.with_content(content(bare));
    assert!(ship_allowed(&reg, Mode::Solo, ship(ALICE)));
}

#[test]
fn a_solo_deploy_pays_one_key_and_sends_the_pilot_in() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [1.0, 2.0, 3.0]);
    deploy_call(&mut reg, ALICE, Mode::Solo).expect("in");
    assert_eq!(cargo_of_key(&reg), Some(1), "one of two");
    let run = run_of(&reg, ALICE).expect("a run").clone();
    assert_eq!(run.mode, Mode::Solo);
    assert_eq!(
        (run.tier, run.rooms, run.filament),
        (KEY_TIER, ROOMS, KEY_TYPE)
    );
    assert!(run.entrance.is_none(), "a solo run has no gate");
    let p = run.pilot(ALICE).unwrap();
    assert_eq!((p.origin_system, p.origin_spot), (HOME, [1.0, 2.0, 3.0]));
    assert_eq!(run.pockets[0].system, ABY[0]);
    assert!(is_abyss_site(run.pockets[0].site_id));
    assert_eq!(run.content_id, run.pockets[0].site_id);
    assert!(reg.roster().get(ALICE).is_none(), "off");
    assert!(reg.jumps().contains(ALICE), "on a jump");
    // The origin is their logoff spot.
    let spot = crate::logoff_spot::get(&reg, ALICE).expect("a spot");
    assert_eq!((spot.system, spot.position), (HOME, [1.0, 2.0, 3.0]));
    // A last key is destroyed whole.
    let mut reg = world_with(G_CRUISER, 1);
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    deploy_call(&mut reg, ALICE, Mode::Solo).expect("in");
    assert_eq!(cargo_of_key(&reg), None);
}

#[test]
fn a_pilot_in_a_run_cannot_deploy_again() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    deploy_call(&mut reg, ALICE, Mode::Solo).expect("in");
    seat(&mut reg, ALICE);
    let e = deploy_call(&mut reg, ALICE, Mode::Solo).unwrap_err();
    assert!(codes(&e).contains(&err::ABYSS_CONTENT_ACTIVE));
    assert!(codes(&e).contains(&err::IN_ABYSS_SYSTEM));
    assert_eq!(cargo_of_key(&reg), Some(1));
}

#[test]
fn runs_never_share_a_system_and_a_live_one_is_never_reused() {
    let mut reg = world();
    for who in [ALICE, BOB, CAROL] {
        stand(&mut reg, who, HOME, [0.0; 3]);
        begin(&mut reg, who, Mode::Solo, 1, 0, 0, ROOMS, NOW).expect("in");
    }
    let mut seen = BTreeSet::new();
    for r in runs(&reg) {
        for p in &r.pockets {
            assert!(seen.insert(p.system), "system {} twice", p.system);
            assert!(VOID_SYSTEMS.contains(&p.system));
        }
    }
    assert_eq!(seen.len(), 3);
    // When the rooms are all taken, no run starts.
    let mut reg = world();
    for (i, id) in ABY.iter().enumerate() {
        let n = 100 + i as u64;
        reg.ext_mut::<AbyssState>().runs.insert(
            n,
            Run {
                id: n,
                creator: 1,
                mode: Mode::Solo,
                tier: 0,
                weather: 0,
                filament: 0,
                content_id: 0,
                rooms: 1,
                pilots: vec![],
                ever: vec![],
                pockets: vec![Pocket {
                    room: 1,
                    system: *id,
                    site_id: SITE_BASE + i as i64,
                    dungeon: ROOM_A,
                    entry: [0.0; 3],
                    exit_at: [0.0; 3],
                    gate: 0,
                    cleared: false,
                    seeded: false,
                    closed: false,
                }],
                entrance: None,
                started: false,
                ends_at: 0,
            },
        );
    }
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    assert!(
        begin(&mut reg, ALICE, Mode::Solo, 1, 0, 0, 1, NOW)
            .unwrap_err()
            .contains("no free")
    );
}

// ---- the rooms --------------------------------------------------------------

#[test]
fn the_pilot_is_told_once_per_room_they_arrive_in() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 4, 0, 0, ROOMS, NOW).unwrap();
    let _ = sent(&mut reg);
    let run = run_of(&reg, ALICE).unwrap().clone();
    // On the way: nothing.
    tick(&mut reg, Instant::now(), NOW + 1);
    assert!(sent(&mut reg).iter().all(|s| !s.0.starts_with("OnAbyssal")));
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 2);
    let out = sent(&mut reg);
    let got = |name: &str| out.iter().filter(|s| s.0 == name).count();
    assert_eq!(got("OnAbyssalLocationEntered"), 1);
    assert_eq!(got("OnAddExpiryDurationForAbyssalContent"), 1);
    let entered = out
        .iter()
        .find(|s| s.0 == "OnAbyssalLocationEntered")
        .unwrap();
    assert_eq!(entered.2, Some(vec![ALICE]));
    assert_eq!(
        crate::pyutil::dict_get(&entered.1[0], b"content_id"),
        Some(&PyValue::Int(run.content_id))
    );
    assert_eq!(
        crate::pyutil::dict_get(&entered.1[0], b"location_id"),
        Some(&PyValue::Int(1))
    );
    let expiry = out
        .iter()
        .find(|s| s.0 == "OnAddExpiryDurationForAbyssalContent")
        .unwrap();
    assert_eq!(expiry.1, vec![PyValue::Int(run.ends_at)]);
    tick(&mut reg, Instant::now(), NOW + 3);
    assert!(sent(&mut reg).iter().all(|s| !s.0.starts_with("OnAbyssal")));
}

#[test]
fn a_clear_room_has_its_gate_of_the_rooms_kind_on_the_exit_locator() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, ROOMS, NOW).unwrap();
    let p = pocket(&reg, ALICE);
    assert!(p.cleared, "no NPC is tagged to it");
    let inst = reg.ext::<Sites>().get(p.site_id).expect("an instance");
    let gate = inst
        .objects
        .iter()
        .find(|o| o.item_id == p.gate)
        .expect("the gate");
    assert_eq!(
        (gate.type_id, gate.position),
        (ENCOUNTER_GATE, [0.0, 0.0, 24.0])
    );
}

#[test]
fn a_room_with_a_living_npc_has_no_gate_until_it_is_dead() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    let run_id = begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).unwrap();
    seat(&mut reg, ALICE);
    let site_id = pocket(&reg, ALICE).site_id;
    // Make the room again with an NPC tagged to it: vacate, tag, ensure.
    let mut run = reg.ext::<AbyssState>().runs.get(&run_id).cloned().unwrap();
    assert!(site::vacate(&mut reg, site_id));
    run.pockets[0].cleared = false;
    run.pockets[0].gate = 0;
    let npc =
        crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, ABY[0], [0.0; 3]).expect("an npc");
    site::tag(
        &mut reg,
        npc,
        site::Tag {
            site_id,
            dun_object_id: 0,
            entity_group: None,
        },
    );
    assert!(!room_clear(&reg, site_id));
    let mut p = run.pockets[0].clone();
    assert!(ensure_pocket(&mut reg, &run, &mut p, NOW));
    assert!(!p.cleared);
    assert_eq!(p.gate, 0, "no gate while the NPC lives");
    // It dies; the stage places the gate.
    reg.npcs_mut().remove(npc);
    assert!(room_clear(&reg, site_id));
    run.pockets[0] = p;
    reg.ext_mut::<AbyssState>().runs.insert(run.id, run);
    tick(&mut reg, Instant::now(), NOW + 1);
    let p = pocket(&reg, ALICE);
    assert!(p.cleared);
    let inst = reg.ext::<Sites>().get(site_id).unwrap();
    assert!(
        inst.objects
            .iter()
            .any(|o| o.item_id == p.gate && o.type_id == EXIT_GATE)
    );
}

#[test]
fn the_gates_take_the_pilot_through_three_rooms_and_out() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    deploy_call(&mut reg, ALICE, Mode::Solo).unwrap();
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let _ = sent(&mut reg);
    let one = pocket(&reg, ALICE);
    // The exit method does not take a gate that leads on; a wrong id is no gate.
    assert!(gate_call(&mut reg, ALICE, "AbyssalEndGateActivation", one.gate).is_err());
    assert!(gate_call(&mut reg, ALICE, "AbyssalGateActivation", one.gate + 1).is_err());
    assert_eq!(pocket(&reg, ALICE).room, 1);
    // Room 1 to 2.
    gate_call(&mut reg, ALICE, "AbyssalGateActivation", one.gate).expect("on");
    assert!(reg.jumps().contains(ALICE));
    assert_eq!(pocket(&reg, ALICE).room, 2);
    assert_ne!(pocket(&reg, ALICE).system, one.system);
    assert!(
        reg.ext::<Sites>().get(one.site_id).is_none(),
        "nobody left in the first room"
    );
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 2);
    let out = sent(&mut reg);
    let entered = out
        .iter()
        .find(|s| s.0 == "OnAbyssalLocationEntered")
        .expect("told of room 2");
    assert_eq!(
        crate::pyutil::dict_get(&entered.1[0], b"location_id"),
        Some(&PyValue::Int(2))
    );
    // Room 2 to 3.
    let two = pocket(&reg, ALICE);
    gate_call(&mut reg, ALICE, "AbyssalGateActivation", two.gate).expect("on");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 3);
    let three = pocket(&reg, ALICE);
    assert_eq!(three.room, ROOMS);
    let inst = reg.ext::<Sites>().get(three.site_id).unwrap();
    assert!(
        inst.objects
            .iter()
            .any(|o| o.item_id == three.gate && o.type_id == EXIT_GATE)
    );
    // The last room's gate leads out, not on.
    assert!(gate_call(&mut reg, ALICE, "AbyssalGateActivation", three.gate).is_err());
    // Out.
    let _ = sent(&mut reg);
    gate_call(&mut reg, ALICE, "AbyssalEndGateActivation", three.gate).expect("out");
    assert!(run_of(&reg, ALICE).is_none());
    assert_eq!(runs(&reg).count(), 0);
    assert!(reg.ext::<Sites>().get(three.site_id).is_none());
    assert!(reg.jumps().contains(ALICE), "on the way home");
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_some(),
        "the origin row stays until the ship has landed"
    );
    // Landed at the origin (the bind took the arrival): the stage spends it.
    land(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    tick(&mut reg, Instant::now(), NOW + 4);
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_none(),
        "the origin row is cleared on landing"
    );
    assert!(
        sent(&mut reg)
            .iter()
            .any(|s| s.0 == "OnAbyssalContentFinished")
    );
}

#[test]
fn a_gate_that_is_not_yours_or_a_room_not_clear_takes_nobody() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    deploy_call(&mut reg, ALICE, Mode::Solo).unwrap();
    seat(&mut reg, ALICE);
    let gate = pocket(&reg, ALICE).gate;
    assert!(
        call(
            &mut reg,
            None,
            "AbyssalGateActivation",
            &[PyValue::Int(gate)]
        )
        .is_err()
    );
    assert!(gate_call(&mut reg, BOB, "AbyssalGateActivation", gate).is_err());
    assert!(gate_call(&mut reg, BOB, "AbyssalEndGateActivation", gate).is_err());
    // Not clear: no gate to name.
    reg.ext_mut::<AbyssState>()
        .runs
        .values_mut()
        .for_each(|r| r.pockets[0].gate = 0);
    assert!(gate_call(&mut reg, ALICE, "AbyssalGateActivation", 0).is_err());
    assert_eq!(pocket(&reg, ALICE).room, 1);
}

#[test]
fn a_room_the_site_stage_dropped_comes_back_with_its_gate() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, ROOMS, NOW).unwrap();
    let old = pocket(&reg, ALICE);
    assert!(old.seeded, "seeded when first made");
    assert!(site::vacate(&mut reg, old.site_id), "the cold sweep");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let p = pocket(&reg, ALICE);
    let inst = reg.ext::<Sites>().get(old.site_id).expect("made again");
    assert!(inst.objects.iter().any(|o| o.item_id == p.gate));
    assert_eq!(
        inst.objects
            .iter()
            .filter(|o| o.type_id == ENCOUNTER_GATE)
            .count(),
        1
    );
}

// ---- the clock --------------------------------------------------------------

#[test]
fn the_clock_starts_with_the_first_pilot_and_lasts_twenty_minutes() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, ROOMS, NOW).unwrap();
    let run = run_of(&reg, ALICE).unwrap();
    assert_eq!(run.ends_at, NOW + DURATION_S * T);
    assert_eq!(DURATION_S, 1200);
}

#[test]
fn a_pocket_out_of_time_kills_the_ship_and_the_pod() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    let ends = run_of(&reg, ALICE).unwrap().ends_at;
    tick(&mut reg, Instant::now(), ends - 1);
    assert!(run_of(&reg, ALICE).is_some());
    assert!(
        reg.store().get(ship(ALICE)).is_some(),
        "alive before the end"
    );
    let _ = sent(&mut reg);
    tick(&mut reg, Instant::now(), ends);
    assert!(run_of(&reg, ALICE).is_none(), "the run is over");
    assert_eq!(runs(&reg).count(), 0);
    // The hull went with the first death and the pod with the second: the
    // pilot is out of space in a new capsule, revived in their clone.
    assert_ne!(reg.character(ALICE).unwrap().ship_id, ship(ALICE));
    assert!(reg.roster().get(ALICE).is_none(), "the pod is gone too");
    assert!(
        sent(&mut reg)
            .iter()
            .any(|s| s.0 == "OnAbyssalContentFinished")
    );
    assert!(crate::logoff_spot::get(&reg, ALICE).is_none());
}

#[test]
fn a_pilot_still_on_the_jump_when_time_runs_out_is_waited_for_a_minute() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    let ends = run_of(&reg, ALICE).unwrap().ends_at;
    tick(&mut reg, Instant::now(), ends);
    assert!(run_of(&reg, ALICE).is_some(), "still on the way in");
    tick(&mut reg, Instant::now(), ends + (TRANSIT_GRACE_S + 1) * T);
    assert!(run_of(&reg, ALICE).is_none(), "the grace is over");
    // The jump lands in a room that is gone: the pilot is sent home.
    let sys = ABY[0];
    crate::jump::arm_arrival(&mut reg, ALICE, sys, [0.0; 3]);
    let _ = crate::jump::take_arrival(&mut reg, ALICE);
    stand(&mut reg, ALICE, sys, [0.0; 3]);
    assert!(!reg.jumps().contains(ALICE));
    tick(&mut reg, Instant::now(), ends + (TRANSIT_GRACE_S + 2) * T);
    assert!(reg.jumps().contains(ALICE), "on the way to the origin");
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_some(),
        "the row waits for the landing"
    );
    land(&mut reg, ALICE, HOME, [0.0; 3]);
    tick(&mut reg, Instant::now(), ends + (TRANSIT_GRACE_S + 3) * T);
    assert!(crate::logoff_spot::get(&reg, ALICE).is_none(), "row spent");
}

// ---- restart and logoff -----------------------------------------------------

#[test]
fn the_logoff_record_leaves_the_origin_while_the_pilot_is_in_a_run() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    crate::logoff_spot::record(&mut reg, ALICE);
    let spot = crate::logoff_spot::get(&reg, ALICE).expect("still the origin");
    assert_eq!((spot.system, spot.position), (HOME, [5.0, 6.0, 7.0]));
    // The same record outside a run writes where the ship is.
    stand(&mut reg, BOB, HOME, [1.0, 1.0, 1.0]);
    crate::logoff_spot::record(&mut reg, BOB);
    assert_eq!(crate::logoff_spot::get(&reg, BOB).unwrap().system, HOME);
}

#[test]
fn a_server_restart_leaves_the_origin_row_and_no_run() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    // The delta row is what the next process reads.
    let row = reg
        .deltas()
        .of(crate::logoff_spot::KIND)
        .get(&vec![ALICE.to_string()])
        .cloned()
        .expect("a row");
    assert_eq!(row[1], HOME.to_string());
    let after = world();
    assert!(runs(&after).next().is_none(), "the runs are not persisted");
}

#[test]
fn a_pilot_who_is_gone_leaves_the_run_and_the_origin_stays() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    let site_id = pocket(&reg, ALICE).site_id;
    // Off the roster and with no session: gone.
    reg.roster_mut().leave(ALICE);
    reg.session_closed(1);
    tick(&mut reg, Instant::now(), NOW + 5);
    assert!(run_of(&reg, ALICE).is_none());
    assert_eq!(runs(&reg).count(), 0, "nobody left, nothing to admit");
    assert!(reg.ext::<Sites>().get(site_id).is_none());
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_some(),
        "the origin stays"
    );
}

/// The real bind (`destiny::on_ballpark_bind`) for a pilot whose arrival is
/// armed at `system`/`at`, as the session change leaves it.
fn bind(reg: &mut Registry, who: i64) {
    crate::destiny::on_ballpark_bind(reg, Some(who));
}

#[test]
fn the_bind_of_a_pilot_going_in_lands_in_the_pocket_not_at_the_origin() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    let p = pocket(&reg, ALICE);
    // The session change has gone; the bind follows.
    crate::jump::arm_arrival(&mut reg, ALICE, p.system, p.entry);
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_some(),
        "the origin row is there"
    );
    bind(&mut reg, ALICE);
    let at = reg.roster().get(ALICE).copied().expect("seated");
    assert_eq!(at.solar_system_id, p.system, "in the pocket");
    assert!(!reg.jumps().contains(ALICE));
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_some(),
        "the origin row is kept for a restart"
    );
}

#[test]
fn a_jump_dropped_on_a_disconnect_still_restores_the_origin() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    assert!(reg.jumps().contains(ALICE));
    reg.session_closed(1);
    crate::jump::tick(&mut reg, Instant::now());
    assert!(
        !reg.jumps().contains(ALICE),
        "the jump went with the session"
    );
    let spot = crate::logoff_spot::take(&mut reg, ALICE).expect("the origin");
    assert_eq!((spot.system, spot.position), (HOME, [5.0, 6.0, 7.0]));
}

#[test]
fn a_login_after_a_restart_lands_at_the_origin_unharmed() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    let snap = reg.to_snapshot();
    // The next process: the saved world, restored, nobody in space.
    let mut after = world();
    after.roster_mut().leave(ALICE);
    after.apply_snapshot(snap);
    assert!(runs(&after).next().is_none(), "the runs are not persisted");
    assert!(after.roster().get(ALICE).is_none());
    bind(&mut after, ALICE);
    let at = after.roster().get(ALICE).copied().expect("seated");
    assert_eq!(at.solar_system_id, HOME, "the origin, not a pocket");
    assert_eq!(at.motion.position, [5.0, 6.0, 7.0]);
    assert_eq!(after.character(ALICE).unwrap().ship_id, ship(ALICE));
    assert!(
        after.store().get(ship(ALICE)).is_some(),
        "the hull is whole"
    );
    assert!(
        crate::logoff_spot::get(&after, ALICE).is_none(),
        "the row is spent"
    );
}

#[test]
fn a_pocket_is_closed_only_when_every_pilot_is_past_it() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    for who in [ALICE, BOB] {
        gate_call(&mut reg, who, "AbyssalEntranceGateActivation", e.gate).unwrap();
        seat(&mut reg, who);
    }
    tick(&mut reg, Instant::now(), NOW + 1);
    // Alice goes two rooms ahead; Bob stays in room 1.
    for _ in 0..2 {
        let here = pocket(&reg, ALICE);
        gate_call(&mut reg, ALICE, "AbyssalGateActivation", here.gate).unwrap();
        seat(&mut reg, ALICE);
        tick(&mut reg, Instant::now(), NOW + 2);
    }
    assert_eq!(pocket(&reg, ALICE).room, 3);
    let run = runs(&reg).next().unwrap().clone();
    let two = &run.pockets[1];
    assert!(!two.closed, "room 2 is still Bob's way");
    assert!(reg.ext::<Sites>().get(two.site_id).is_some());
    // Bob follows through room 2 and lands in his own room 2, not a shared
    // system with another run.
    let one = pocket(&reg, BOB);
    gate_call(&mut reg, BOB, "AbyssalGateActivation", one.gate).unwrap();
    assert_eq!(pocket(&reg, BOB).system, two.system, "the same room 2");
    seat(&mut reg, BOB);
    let taken: Vec<i64> = runs(&reg)
        .flat_map(|r| r.open_pockets().map(|p| p.system))
        .collect();
    let distinct: BTreeSet<i64> = taken.iter().copied().collect();
    assert_eq!(taken.len(), distinct.len(), "no system twice");
    // Now nobody is behind room 1 or 2 (the entrance no longer admits when
    // the seats are full or its life is over).
    reg.ext_mut::<AbyssState>()
        .runs
        .values_mut()
        .for_each(|r| r.entrance.as_mut().unwrap().expires = 1);
    tick(&mut reg, Instant::now(), NOW + 3);
    let run = runs(&reg).next().unwrap();
    assert!(run.pockets[0].closed, "everyone is past room 1");
    assert!(!run.pockets[1].closed, "bob is in room 2");
    assert!(!run.pockets[2].closed);
}

#[test]
fn a_relog_in_the_hold_keeps_the_origin_row() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    assert!(crate::crimewatch::logged_off(&mut reg, ALICE, NOW));
    assert!(crate::logoff_spot::get(&reg, ALICE).is_some());
    assert!(crate::crimewatch::reclaim(&mut reg, ALICE));
    let spot = crate::logoff_spot::get(&reg, ALICE).expect("the origin survives the relog");
    assert_eq!((spot.system, spot.position), (HOME, [5.0, 6.0, 7.0]));
    // And a hold released for a pilot who is back by another path.
    assert!(crate::crimewatch::logged_off(&mut reg, ALICE, NOW));
    let until = reg.combat().crimewatch().held_until(ALICE).expect("held");
    crate::crimewatch::release_holds(&mut reg, until);
    assert!(crate::logoff_spot::get(&reg, ALICE).is_some());
}

/// A pilot who logs off in the pocket: the connection closes, the ship is held.
fn drop_in_pocket(reg: &mut Registry, now: i64) {
    reg.session_closed(1);
    assert!(crate::crimewatch::logged_off(reg, ALICE, now));
}

#[test]
fn a_logoff_in_the_pocket_is_no_exit_the_clock_kills_ship_and_pod() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    let ends = run_of(&reg, ALICE).unwrap().ends_at;
    drop_in_pocket(&mut reg, NOW);
    // The hold's minimum is long over, well before the clock: the ship stays.
    let mid = NOW + 5 * crate::crimewatch::LOGOFF_MINIMUM;
    assert!(mid < ends);
    crate::crimewatch::release_holds(&mut reg, mid);
    tick(&mut reg, Instant::now(), mid);
    assert!(
        reg.roster().get(ALICE).is_some(),
        "the ship stays in the pocket"
    );
    assert!(run_of(&reg, ALICE).is_some(), "the run goes on");
    assert!(reg.store().get(ship(ALICE)).is_some());
    // The clock ends: ship and pod die, nobody lands at the origin.
    tick(&mut reg, Instant::now(), ends);
    crate::crimewatch::release_holds(&mut reg, ends);
    assert!(run_of(&reg, ALICE).is_none());
    assert_ne!(
        reg.character(ALICE).unwrap().ship_id,
        ship(ALICE),
        "hull died"
    );
    assert!(reg.roster().get(ALICE).is_none(), "the pod died too");
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_none(),
        "no origin to wake at"
    );
    assert!(reg.combat().crimewatch().held_until(ALICE).is_none());
}

#[test]
fn a_safe_logoff_in_the_pocket_leaves_the_ship_to_the_clock() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    crate::safe_logoff::start(&mut reg, Some(ALICE), NOW).expect("starts");
    let due = NOW + crate::safe_logoff::DELAY;
    crate::safe_logoff::tick(&mut reg, Instant::now(), due);
    assert!(reg.roster().get(ALICE).is_some(), "the ship did not leave");
    assert!(run_of(&reg, ALICE).is_some());
}

#[test]
fn a_relog_before_the_clock_stays_in_the_pocket() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    let pocket_system = pocket(&reg, ALICE).system;
    drop_in_pocket(&mut reg, NOW);
    let mid = NOW + 5 * crate::crimewatch::LOGOFF_MINIMUM;
    crate::crimewatch::release_holds(&mut reg, mid);
    assert!(crate::crimewatch::reclaim(&mut reg, ALICE), "still held");
    tick(&mut reg, Instant::now(), mid + 1);
    let at = reg.roster().get(ALICE).copied().expect("in space");
    assert_eq!(at.solar_system_id, pocket_system);
    assert!(run_of(&reg, ALICE).is_some());
}

#[test]
fn a_restart_after_a_pocket_logoff_still_lands_at_the_origin() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 5, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    drop_in_pocket(&mut reg, NOW);
    let snap = reg.to_snapshot();
    // The next process has no run and nobody in space, so the hold has nothing
    // to keep.
    let mut after = world();
    after.roster_mut().leave(ALICE);
    after.apply_snapshot(snap);
    assert!(runs(&after).next().is_none());
    bind(&mut after, ALICE);
    let at = after.roster().get(ALICE).copied().expect("seated");
    assert_eq!(at.solar_system_id, HOME);
    assert_eq!(at.motion.position, [5.0, 6.0, 7.0]);
}

#[test]
fn a_pilot_whose_pod_was_killed_is_dropped_from_the_run() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [5.0, 6.0, 7.0]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    seat(&mut reg, ALICE);
    // Killed in the pocket by someone else: docked at home, session open.
    reg.roster_mut().leave(ALICE);
    tick(&mut reg, Instant::now(), NOW + 5);
    assert!(run_of(&reg, ALICE).is_none(), "no longer in the run");
    assert_eq!(runs(&reg).count(), 0);
    assert!(
        crate::logoff_spot::get(&reg, ALICE).is_none(),
        "nothing to restore"
    );
    assert!(!in_run(&reg, ALICE));
}

#[test]
fn a_system_with_an_abandoned_ship_is_not_reused() {
    let mut reg = world();
    crate::pilotless::abandon(
        &mut reg,
        crate::pilotless::Abandoned {
            ship_id: 100_880_950,
            type_id: CRUISER,
            owner_id: BOB,
            corp_id: 0,
            alliance_id: None,
            system: ABY[0],
            abandoned_at: NOW,
        },
        Motion::arrived([0.0; 3], [0.0; 3]),
    );
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 3, 0, 0, ROOMS, NOW).unwrap();
    assert_ne!(pocket(&reg, ALICE).system, ABY[0], "left to its leavings");
}

// ---- the fleet modes --------------------------------------------------------

fn fleet_world() -> Registry {
    let mut reg = world_with(G_FRIGATE, 3);
    stand(&mut reg, ALICE, HOME, [3.0, 0.0, 0.0]);
    stand(&mut reg, BOB, HOME, [4.0, 0.0, 0.0]);
    stand(&mut reg, CAROL, HOME, [0.0; 3]);
    let f = reg.fleets_mut().create(ALICE).expect("created");
    reg.fleets_mut()
        .init(ALICE, f, Some(670), None, None)
        .expect("init");
    reg.fleets_mut()
        .invite(ALICE, f, BOB, None, None, None)
        .expect("invited");
    reg.fleets_mut()
        .accept_invite(BOB, f, Some(670), None, None)
        .expect("accepted");
    let _ = sent(&mut reg);
    reg
}

fn entrance_of(reg: &Registry) -> Entrance {
    runs(reg)
        .next()
        .and_then(|r| r.entrance.clone())
        .expect("an entrance")
}

#[test]
fn a_co_op_deploy_pays_three_keys_and_puts_a_gate_beside_the_ship() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).expect("deployed");
    assert_eq!(cargo_of_key(&reg), None, "the whole stack of three");
    let run = runs(&reg).next().unwrap().clone();
    assert!(run.pilots.is_empty(), "the deployer has not jumped");
    assert!(!run.started);
    assert_eq!(run.ends_at, 0);
    assert!(!reg.jumps().contains(ALICE));
    let e = entrance_of(&reg);
    assert_eq!(e.system, HOME);
    let inst = reg.ext::<Sites>().get(e.site_id).expect("the gate's site");
    assert!(
        inst.objects
            .iter()
            .any(|o| o.item_id == e.gate && o.type_id == ENTRANCE_GATE)
    );
    // Its slim carries the key, the mode and that it is open.
    let pairs = gate_pairs(&reg, e.gate);
    assert!(pairs.contains(&("abyssFilamentTypeID", PyValue::Int(KEY_TYPE))));
    assert!(pairs.contains(&("gameModeID", PyValue::Int(2))));
    assert!(pairs.contains(&("isAbyssGateOpen", PyValue::Bool(true))));
    assert!(gate_pairs(&reg, e.gate + 1).is_empty());
    // The same pairs ride on the site's extras for the gate.
    let extras = crate::site::slim_extras(&reg, &[e.gate]);
    assert!(extras[0].1.iter().any(|(k, _)| *k == "isAbyssGateOpen"));
    // The deployer may not deploy again while the gate stands.
    let again = deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap_err();
    assert!(codes(&again).contains(&err::ABYSS_CONTENT_ACTIVE));
}

#[test]
fn the_fleet_goes_in_by_the_gate_one_at_a_time_and_the_clock_starts_with_the_first() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    let enter =
        |reg: &mut Registry, who| gate_call(reg, who, "AbyssalEntranceGateActivation", e.gate);
    // Carol is not in the fleet.
    let refused = enter(&mut reg, CAROL).unwrap_err();
    assert!(codes(&refused).contains(&err::SHIP_NOT_IN_SAME_FLEET));
    assert!(runs(&reg).next().unwrap().pilots.is_empty());
    // Alice, then Bob.
    enter(&mut reg, ALICE).expect("alice in");
    let run = runs(&reg).next().unwrap().clone();
    assert!(run.started);
    assert_eq!(run.pilots.len(), 1);
    assert!(run.entrance.is_some(), "the gate stays for the rest");
    let ends = run.ends_at;
    enter(&mut reg, BOB).expect("bob in");
    let run = runs(&reg).next().unwrap().clone();
    assert_eq!(run.pilots.len(), 2);
    assert_eq!(run.ends_at, ends, "one clock");
    assert_eq!(run.pilots[0].origin_system, HOME, "from where they stood");
    // Both are in room 1: one pocket, one system.
    assert_eq!(run.pockets.len(), 1);
    // A pilot who is in cannot enter twice.
    let again = enter(&mut reg, ALICE).unwrap_err();
    assert!(codes(&again).contains(&err::ABYSS_CONTENT_ACTIVE));
}

#[test]
fn the_gate_refuses_a_pilot_across_the_system_a_wrong_gate_and_a_closed_gate() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    let enter =
        |reg: &mut Registry, who, gate| gate_call(reg, who, "AbyssalEntranceGateActivation", gate);
    // Bob far away.
    stand(&mut reg, BOB, HOME, [9.0e6, 0.0, 0.0]);
    let far = enter(&mut reg, BOB, e.gate).unwrap_err();
    assert_eq!(codes(&far), [err::PROXIMITY_ENTRANCE]);
    // No such gate.
    let missing = enter(&mut reg, BOB, e.gate + 5).unwrap_err();
    assert_eq!(codes(&missing), [err::ENTRANCE_MISSING]);
    // In another system: the gate is not near.
    stand(&mut reg, BOB, HIGH, [0.0; 3]);
    let other = enter(&mut reg, BOB, e.gate).unwrap_err();
    assert!(codes(&other).contains(&err::ENTRANCE_MISSING));
    // A docked pilot.
    reg.roster_mut().leave(BOB);
    let docked = enter(&mut reg, BOB, e.gate).unwrap_err();
    assert!(codes(&docked).contains(&err::SHIP_IS_DOCKED));
    // The gate's life is over.
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    let run_id = runs(&reg).next().unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&run_id)
        .unwrap()
        .entrance
        .as_mut()
        .unwrap()
        .expires = 1;
    let closed = enter(&mut reg, BOB, e.gate).unwrap_err();
    assert!(codes(&closed).contains(&err::ENTRANCE_CLOSED));
}

#[test]
fn a_gate_nobody_used_goes_with_its_keys_when_its_life_is_over() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    tick(&mut reg, Instant::now(), e.expires - 1);
    assert_eq!(runs(&reg).count(), 1);
    assert!(reg.ext::<Sites>().get(e.site_id).is_some());
    tick(&mut reg, Instant::now(), e.expires);
    assert_eq!(runs(&reg).count(), 0, "the run is closed");
    assert!(
        reg.ext::<Sites>().get(e.site_id).is_none(),
        "the gate is gone"
    );
    assert_eq!(cargo_of_key(&reg), None, "the keys are not given back");
    assert_eq!(UNENTERED_GATE_LIFETIME_S, 1800);
}

#[test]
fn the_gate_comes_back_when_the_site_stage_drops_it_and_someone_is_there() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    assert!(site::vacate(&mut reg, e.site_id));
    tick(&mut reg, Instant::now(), NOW);
    let again = entrance_of(&reg);
    let inst = reg.ext::<Sites>().get(again.site_id).expect("made again");
    assert!(inst.objects.iter().any(|o| o.item_id == again.gate));
}

#[test]
fn the_last_seat_taken_closes_the_gate() {
    // Two-player: two keys, two seats. Frigates are in list 233.
    let mut reg = world_with(G_FRIGATE, 2);
    for who in [ALICE, BOB] {
        stand(&mut reg, who, HOME, [0.0; 3]);
    }
    let f = reg.fleets_mut().create(ALICE).unwrap();
    reg.fleets_mut()
        .init(ALICE, f, Some(670), None, None)
        .unwrap();
    reg.fleets_mut()
        .invite(ALICE, f, BOB, None, None, None)
        .unwrap();
    reg.fleets_mut()
        .accept_invite(BOB, f, Some(670), None, None)
        .unwrap();
    deploy_call(&mut reg, ALICE, Mode::TwoPlayer).expect("deployed");
    assert_eq!(cargo_of_key(&reg), None);
    let e = entrance_of(&reg);
    gate_call(&mut reg, ALICE, "AbyssalEntranceGateActivation", e.gate).unwrap();
    assert!(runs(&reg).next().unwrap().entrance.is_some());
    gate_call(&mut reg, BOB, "AbyssalEntranceGateActivation", e.gate).unwrap();
    let run = runs(&reg).next().unwrap();
    assert!(run.entrance.is_none(), "both seats are taken");
    assert!(reg.ext::<Sites>().get(e.site_id).is_none());
    // A third pilot finds no gate.
    stand(&mut reg, CAROL, HOME, [0.0; 3]);
    let late = gate_call(&mut reg, CAROL, "AbyssalEntranceGateActivation", e.gate).unwrap_err();
    assert_eq!(codes(&late), [err::ENTRANCE_MISSING]);
}

#[test]
fn the_pilots_of_a_run_move_on_alone_into_one_shared_next_room() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    for who in [ALICE, BOB] {
        gate_call(&mut reg, who, "AbyssalEntranceGateActivation", e.gate).unwrap();
        seat(&mut reg, who);
    }
    tick(&mut reg, Instant::now(), NOW + 1);
    let one = pocket(&reg, ALICE);
    assert_eq!(one, pocket(&reg, BOB));
    gate_call(&mut reg, ALICE, "AbyssalGateActivation", one.gate).unwrap();
    assert_eq!(pocket(&reg, ALICE).room, 2);
    assert_eq!(pocket(&reg, BOB).room, 1, "bob has not moved");
    assert!(
        reg.ext::<Sites>().get(one.site_id).is_some(),
        "room 1 stays while bob is in it"
    );
    seat(&mut reg, ALICE);
    gate_call(&mut reg, BOB, "AbyssalGateActivation", one.gate).unwrap();
    assert_eq!(pocket(&reg, BOB), pocket(&reg, ALICE), "the same room 2");
    assert!(
        reg.ext::<Sites>().get(one.site_id).is_some(),
        "room 1 waits for a third who may still come in by the gate"
    );
    let run_id = runs(&reg).next().unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&run_id)
        .unwrap()
        .entrance
        .as_mut()
        .unwrap()
        .expires = 1;
    tick(&mut reg, Instant::now(), NOW + 2);
    assert!(reg.ext::<Sites>().get(one.site_id).is_none(), "now empty");
    let run = runs(&reg).next().unwrap();
    assert_eq!(run.pockets.len(), 2);
    let systems: BTreeSet<i64> = run.pockets.iter().map(|p| p.system).collect();
    assert_eq!(systems.len(), 2, "one system per room");
}

#[test]
fn when_one_leaves_the_run_goes_on_for_the_others() {
    let mut reg = fleet_world();
    deploy_call(&mut reg, ALICE, Mode::CoOp).unwrap();
    let e = entrance_of(&reg);
    for who in [ALICE, BOB] {
        gate_call(&mut reg, who, "AbyssalEntranceGateActivation", e.gate).unwrap();
        seat(&mut reg, who);
    }
    assert!(leave(&mut reg, ALICE));
    assert!(run_of(&reg, ALICE).is_none());
    assert!(run_of(&reg, BOB).is_some());
    assert_eq!(runs(&reg).count(), 1);
    assert!(leave(&mut reg, BOB));
    assert!(runs(&reg).all(|r| r.pilots.is_empty()));
}

// ---- the probe's door -------------------------------------------------------

#[test]
fn the_gm_door_is_a_one_room_solo_run_with_the_exit_gate_at_once() {
    let mut reg = world();
    assert!(
        enter(&mut reg, ALICE, 3, NOW)
            .unwrap_err()
            .contains("not in space")
    );
    stand(&mut reg, ALICE, HOME, [1.0, 2.0, 3.0]);
    assert!(enter(&mut reg, ALICE, 3, NOW).is_ok());
    assert!(
        enter(&mut reg, ALICE, 3, NOW)
            .unwrap_err()
            .contains("already")
    );
    let p = pocket(&reg, ALICE);
    assert_eq!(p.entry, [0.0, 0.0, -35.0]);
    let inst = reg.ext::<Sites>().get(p.site_id).unwrap();
    assert!(
        inst.objects
            .iter()
            .any(|o| o.item_id == p.gate && o.type_id == EXIT_GATE)
    );
    seat(&mut reg, ALICE);
    gate_call(&mut reg, ALICE, "AbyssalEndGateActivation", p.gate).expect("home");
    assert!(run_of(&reg, ALICE).is_none());
}

#[test]
fn the_service_knows_its_methods_and_nothing_else() {
    let mut reg = world();
    assert_eq!(
        call(&mut reg, Some(ALICE), "ClientIsReady", &[]).unwrap(),
        PyValue::None
    );
    for method in [
        "AbyssalGateActivation",
        "AbyssalEntranceDeployment",
        "AbyssalEntranceGateActivation",
        "AbyssalEndGateActivation",
    ] {
        assert!(
            call(&mut reg, None, method, &[]).is_err(),
            "{method}: no character"
        );
        assert!(
            call(&mut reg, Some(ALICE), method, &[]).is_err(),
            "{method}: no args"
        );
    }
    let other = call(&mut reg, Some(ALICE), "NoSuch", &[]);
    assert!(matches!(other, Err(ServiceError::NoSuchMethod { .. })));
}

#[test]
fn the_slash_command_has_its_three_words() {
    let mut reg = world();
    let text = |v: Result<PyValue, ServiceError>| {
        String::from_utf8_lossy(crate::pyutil::as_bytes(&v.unwrap()).unwrap()).into_owned()
    };
    assert!(text(slash(&mut reg, None, &["status"])).contains("no character"));
    assert!(text(slash(&mut reg, Some(ALICE), &[])).contains("usage"));
    assert!(text(slash(&mut reg, Some(ALICE), &["enter", "9"])).contains("usage"));
    assert!(text(slash(&mut reg, Some(ALICE), &["leave"])).contains("not in a pocket"));
    assert!(text(slash(&mut reg, Some(ALICE), &["status"])).contains("no pocket"));
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    assert!(text(slash(&mut reg, Some(ALICE), &["enter", "2"])).contains("into abyssal system"));
    assert!(text(slash(&mut reg, Some(ALICE), &["status"])).contains("tier 2"));
    assert!(text(slash(&mut reg, Some(ALICE), &["leave"])).contains("leaving"));
}

#[test]
fn a_living_npc_outside_the_targets_list_does_not_hold_the_gate() {
    let mut reg = world();
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).unwrap();
    let site_id = pocket(&reg, ALICE).site_id;
    let npc =
        crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, ABY[0], [0.0; 3]).expect("an npc");
    let tag = site::Tag {
        site_id,
        dun_object_id: 0,
        entity_group: None,
    };
    site::tag(&mut reg, npc, tag);
    assert!(!room_clear(&reg, site_id), "its group is on the list");
    let mut t = table(vec![room(ROOM_A), room(ROOM_B)]);
    t.typelists.insert(
        TARGETS_LIST,
        TypeList {
            inc_groups: vec![G_CRUISER],
            ..TypeList::default()
        },
    );
    let reg = reg.with_content(content(t));
    assert!(room_clear(&reg, site_id), "a swarmer is not a target");
}

#[test]
fn a_pve_room_does_not_place_the_pvp_gate() {
    let mut a = room(ROOM_A);
    a.objects.push(object(1, 15, PVP_GATE, [7.0, 7.0, 7.0]));
    let mut reg = world().with_content(content(table(vec![a])));
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).unwrap();
    let inst = reg
        .ext::<Sites>()
        .get(pocket(&reg, ALICE).site_id)
        .expect("an instance");
    assert!(inst.objects.iter().all(|o| o.type_id != PVP_GATE));
    assert!(inst.objects.iter().any(|o| o.type_id == OTHER_TYPE));
}
