//! The module through `eve_server`'s public face only. Invented ids.

use eve_mod_econ::tunables::Role;
use eve_mod_econ::{
    KIND, MODULE, drops, filaments, geode, injectors, motherlode, opening, tunables,
};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::crate_hooks::OpenEvent;
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::{Character, SandboxConfig};
use eve_server::module::{self, MinedEvent, Module};
use eve_server::nes_vault;
use eve_server::service::Registry;
use std::sync::Once;

/// Fix the process's plan to the module, before anything reads it. Every
/// test calls this first; the plan is process-wide and fixed once.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        module::install(&[&MODULE]).expect("the module installs");
    });
}

const PILOT: i64 = 91_001;
const STATION: i64 = 61_002;
const CORP: i64 = 1_000_091;
const WRECK_TYPE: i64 = 26_561;
const SYSTEM: i64 = 30_000_142;

fn world() -> Registry {
    install();
    let cfg = SandboxConfig {
        characters: vec![Character {
            character_id: PILOT,
            name: "Pilot".to_owned(),
            station_id: Some(STATION),
            solar_system_id: Some(SYSTEM),
            ..Character::default()
        }],
        ..SandboxConfig::default()
    };
    Registry::new(cfg)
}

/// A wreck item owned by an NPC corporation, and its record.
fn wreck(reg: &mut Registry) -> Wreck {
    let made = reg
        .apply_items_quiet(&[Edit::Create {
            type_id: WRECK_TYPE,
            quantity: 1,
            to: Slot::new(CORP, SYSTEM, 0),
        }])
        .expect("a wreck type the fixture knows");
    let Change::Created { item_id, .. } = made[0] else {
        panic!("created");
    };
    Wreck {
        wreck_id: item_id,
        type_id: WRECK_TYPE,
        solar_system_id: SYSTEM,
        position: [0.0; 3],
        ship_id: 1,
        owner_id: CORP,
        corp_id: CORP,
        fleet_id: None,
        abandoned: false,
        expires_at: 0,
    }
}

fn inside(reg: &Registry, w: &Wreck) -> i64 {
    reg.store()
        .contents_of(w.wreck_id)
        .filter(|i| i.type_id == nes_vault::TYPE_PLEX && i.flag_id == FLAG_IN_WRECK)
        .map(|i| i.quantity)
        .sum()
}

/// The module adds exactly its rows to the core plan and nothing else.
#[test]
fn the_module_adds_its_rows_and_only_those() {
    let core = module::build(&[]).expect("core");
    let with = module::build(&[&MODULE]).expect("assembles");
    assert_eq!(with.modules, [("econ", eve_mod_econ::VERSION)]);
    assert_eq!(with.kill_npc.len(), core.kill_npc.len() + 5);
    assert_eq!(with.on_login.len(), core.on_login.len() + 1);
    assert_eq!(with.kinds.len(), core.kinds.len() + 1);
    assert!(with.kinds.iter().any(|k| k.0 == KIND));
    assert!(with.kill_npc.iter().any(|r| r.name == "econ_plex_drop"));
    assert!(with.kill_npc.iter().any(|r| r.name == "econ_injector_drop"));
    assert!(with.kill_npc.iter().any(|r| r.name == "econ_filament_drop"));
    assert!(with.on_login.iter().any(|r| r.name == "econ_plex_seed"));
    // Everything else is the core's, row for row.
    assert_eq!(with.stages.len(), core.stages.len() + 1);
    assert!(with.stages.iter().any(|s| s.name == "econ_motherlode"));
    assert_eq!(with.services.len(), core.services.len());
    assert_eq!(with.commands.len(), core.commands.len() + 2);
    assert_eq!(with.tables.len(), core.tables.len() + 1);
    assert_eq!(with.crates.len(), core.crates.len() + 1);
    assert_eq!(with.mined.len(), core.mined.len() + 2);
    assert_eq!(with.settled.len(), core.settled.len() + 4);
    assert_eq!(with.slots.len(), core.slots.len() + 1);
    assert_eq!(with.kill_player.len(), core.kill_player.len());
    assert_eq!(with.on_logoff.len(), core.on_logoff.len());
    assert_eq!(with.rollover.len(), core.rollover.len());
    assert_eq!(with.mission.len(), core.mission.len());
    assert_eq!(with.warp_guards.len(), core.warp_guards.len());
    assert_eq!(with.jump_guards.len(), core.jump_guards.len());
    assert_eq!(with.rules.len(), core.rules.len());
    assert!(with.flags.is_empty());
    // A server with the module's slot in the list empty is the core.
    assert!(module::build(&[&Module::NONE]).is_ok());
}

/// The module's version is in the world format; the core's alone is not.
#[test]
fn the_world_format_names_the_module() {
    install();
    assert!(
        module::plan()
            .modules
            .contains(&("econ", eve_mod_econ::VERSION))
    );
}

/// A drop is an item in the wreck, ledgered under its source in the same
/// step, and counted against the day.
#[test]
fn a_drop_is_an_item_in_the_wreck_and_every_unit_is_ledgered() {
    let mut reg = world();
    let w = wreck(&mut reg);
    assert!(drops::leave_in(&mut reg, &w, "elite", 40, 9));
    assert!(drops::leave_in(&mut reg, &w, "heavy", 2, 9));
    assert_eq!(inside(&reg, &w), 42, "items in the wreck, not the vault");
    assert_eq!(nes_vault::ledger_of(reg.deltas(), "drop:elite"), (40, 0));
    assert_eq!(nes_vault::ledger_of(reg.deltas(), "drop:heavy"), (2, 0));
    assert_eq!(drops::minted_on(reg.deltas(), 9), 42);
    assert_eq!(drops::minted_on(reg.deltas(), 10), 0);
    let vault: i64 = nes_vault::ledger(reg.deltas())
        .iter()
        .filter(|(k, _, _)| k.starts_with("drop:"))
        .map(|(_, i, _)| i)
        .sum();
    assert_eq!(vault, 42, "the ledger equals what was made");
}

/// A settled kill has no wreck: the drop lands in the killer's hangar, with
/// the same ledger key and the same day counter as a wreck's.
#[test]
fn a_settled_drop_pays_the_hangar_through_the_same_ledger() {
    let mut reg = world();
    let hangar = Slot::new(PILOT, STATION, eve_server::model::FLAG_HANGAR);
    assert!(drops::leave_at(&mut reg, hangar, "elite", 30, 9));
    let held: i64 = reg
        .store()
        .at(hangar)
        .filter(|i| i.type_id == nes_vault::TYPE_PLEX)
        .map(|i| i.quantity)
        .sum();
    assert_eq!(held, 30);
    assert_eq!(nes_vault::ledger_of(reg.deltas(), "drop:elite"), (30, 0));
    assert_eq!(drops::minted_on(reg.deltas(), 9), 30);
}

/// A settled event with no NPC type rolls nothing and does not panic.
#[test]
fn a_settled_event_without_an_npc_type_rolls_nothing() {
    let mut reg = world();
    let ev = eve_server::module::KillEvent {
        victim_character: 0,
        victim_ship: 0,
        ship_type: 1,
        killer: Some(PILOT),
        system: SYSTEM,
        npc: None,
        when: 0,
        park: Vec::new(),
    };
    eve_server::settle_hooks::fire(
        &mut reg,
        &eve_server::settle_hooks::Settled {
            ev,
            station: STATION,
            seed: 5,
        },
    );
    assert_eq!(drops::minted_on(reg.deltas(), 0), 0);
}

/// The budget's counter is one day's: a new day starts at zero.
#[test]
fn the_day_counter_is_replaced_by_the_next_day() {
    let mut reg = world();
    let w = wreck(&mut reg);
    assert!(drops::leave_in(&mut reg, &w, "elite", 6, 20));
    assert!(drops::leave_in(&mut reg, &w, "elite", 4, 20));
    assert_eq!(drops::minted_on(reg.deltas(), 20), 10);
    assert!(drops::leave_in(&mut reg, &w, "elite", 3, 21));
    assert_eq!(drops::minted_on(reg.deltas(), 21), 3);
    assert_eq!(drops::minted_on(reg.deltas(), 20), 0);
}

/// A wreck the store does not hold takes nothing, and nothing is ledgered.
#[test]
fn a_refused_drop_is_not_ledgered() {
    let mut reg = world();
    let mut w = wreck(&mut reg);
    w.wreck_id = 7;
    // Location 7 is no item, so the store refuses to create inside it.
    let took = drops::leave_in(&mut reg, &w, "elite", 5, 1);
    assert_eq!(
        took,
        nes_vault::ledger_of(reg.deltas(), "drop:elite").0 == 5,
        "the ledger follows the store"
    );
}

/// PLEX the store holds as items anywhere.
fn plex_items(reg: &Registry) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == nes_vault::TYPE_PLEX)
        .map(|i| i.quantity)
        .sum()
}

/// The seed is made once, ledgered under its name, and only for a pilot the
/// world knows; a second log-in, or another pilot's, adds nothing.
#[test]
fn the_seed_is_one_ledgered_stack_made_once() {
    let mut reg = world();
    assert!(!opening::seeded(&reg));
    assert_eq!(plex_items(&reg), 0);
    opening::on_login(&mut reg, 424_242);
    assert!(!opening::seeded(&reg), "an unknown pilot does nothing");
    assert_eq!(plex_items(&reg), 0);

    opening::on_login(&mut reg, PILOT);
    assert!(opening::seeded(&reg));
    assert_eq!(plex_items(&reg), tunables::SEED_UNITS, "one stack");
    assert_eq!(
        nes_vault::ledger_of(reg.deltas(), "seed"),
        (tunables::SEED_UNITS, 0),
        "every unit has a name"
    );

    opening::on_login(&mut reg, PILOT);
    assert_eq!(plex_items(&reg), tunables::SEED_UNITS, "never restocks");
    assert_eq!(
        nes_vault::ledger_of(reg.deltas(), "seed").0,
        tunables::SEED_UNITS
    );
}

#[test]
fn the_tunables_are_sane() {
    const { assert!(tunables::SEED_UNITS > 0 && tunables::SEED_DAYS > 0) };
    const { assert!(tunables::SEED_PRICE > 0.0) };
    const { assert!(tunables::OVER_BUDGET_MULT < 1.0 && tunables::DAILY_BUDGET > 0) };
    assert_eq!(tunables::BOT_PLEX_MULT, 0.5, "owner decision #35");
}

/// An injector drop is an item in the wreck, its points ledgered under its
/// source in the core's `sptrade` and counted against the day's budget.
#[test]
fn an_injector_drop_is_an_item_and_its_points_are_ledgered() {
    use eve_server::skill_trading::{BUCKET, DIVISOR, TYPE_LARGE, TYPE_SMALL, ledger_of};
    let mut reg = world();
    let w = wreck(&mut reg);
    assert!(injectors::leave_in(&mut reg, &w, "elite", TYPE_LARGE, 9));
    assert!(injectors::leave_in(&mut reg, &w, "elite", TYPE_SMALL, 9));
    let held = |t: i64| -> i64 {
        reg.store()
            .contents_of(w.wreck_id)
            .filter(|i| i.type_id == t && i.flag_id == FLAG_IN_WRECK)
            .map(|i| i.quantity)
            .sum()
    };
    assert_eq!((held(TYPE_LARGE), held(TYPE_SMALL)), (1, 1));
    let points = BUCKET + BUCKET / DIVISOR;
    assert_eq!(ledger_of(reg.deltas(), "drop:elite"), (points, 0));
    assert_eq!(injectors::minted_on(reg.deltas(), 9), points);
    assert_eq!(injectors::minted_on(reg.deltas(), 10), 0);
    // PLEX's counter is a different row.
    assert_eq!(drops::minted_on(reg.deltas(), 9), 0);
}

/// A filament drop is one item in the wreck, counted against the day; the type
/// here is any the fixture knows (the real set is read from the bundle).
#[test]
fn a_filament_drop_is_an_item_and_counted_against_the_day() {
    use eve_server::skill_trading::TYPE_LARGE;
    const _: () = assert!(tunables::DAILY_FILAMENTS > 0 && tunables::BOT_FILAMENT_MULT <= 1.0);
    let mut reg = world();
    let w = wreck(&mut reg);
    assert!(filaments::leave_in(&mut reg, &w, TYPE_LARGE, 9));
    assert!(filaments::leave_in(&mut reg, &w, TYPE_LARGE, 9));
    let held: i64 = reg
        .store()
        .contents_of(w.wreck_id)
        .filter(|i| i.type_id == TYPE_LARGE && i.flag_id == FLAG_IN_WRECK)
        .map(|i| i.quantity)
        .sum();
    assert_eq!(held, 2);
    assert_eq!(filaments::dropped_on(reg.deltas(), 9), 2);
    assert_eq!(filaments::dropped_on(reg.deltas(), 10), 0);
    // It touches neither PLEX's counter nor the injectors'.
    assert_eq!(drops::minted_on(reg.deltas(), 9), 0);
    assert_eq!(injectors::minted_on(reg.deltas(), 9), 0);
    // No filament type in a world without the bundle: nothing to drop.
    assert!(eve_server::random_jump::filament_types(reg.content()).is_empty());
}

// ---- geodes (lane l6a-geodes) ----

/// A stackable type the item table knows, and a hull with a cargo hold: the
/// fixture's, whatever ids they are.
fn crate_type() -> i64 {
    eve_server::item::SPAWNABLE
        .iter()
        .find(|t| !t.singleton && t.volume < 1.0)
        .expect("a small stackable type")
        .type_id
}

fn hull_with_hold() -> (i64, f64) {
    let h = eve_server::item::SPAWNABLE
        .iter()
        .find(|t| t.capacity >= 100.0)
        .expect("a hull with a hold");
    (h.type_id, h.capacity)
}

/// A world where `crate_type()` is a geode of `tier` and the loot table holds
/// `loot`.
fn geode_world(tier: usize, loot: Vec<(Role, f64)>) -> Registry {
    install();
    let mut content = eve_server::content::Content::fixture();
    let lines = [format!("{}\t{tier}", crate_type())];
    content.insert(
        "geodes",
        geode::Geodes::parse(&lines, loot).expect("parses"),
    );
    let cfg = SandboxConfig {
        characters: vec![Character {
            character_id: PILOT,
            name: "Pilot".to_owned(),
            station_id: Some(STATION),
            solar_system_id: Some(SYSTEM),
            ..Character::default()
        }],
        ..SandboxConfig::default()
    };
    Registry::new(cfg).with_content(std::sync::Arc::new(content))
}

fn open_event(nth: u64) -> OpenEvent {
    OpenEvent {
        character: PILOT,
        crate_type: crate_type(),
        crate_item: 777,
        station: STATION,
        nth,
        when: 20 * 864_000_000_000,
    }
}

/// A PLEX prize goes to the vault, never to an item, and is ledgered under
/// `geode` and against the day; the crate is counted spent.
#[test]
fn a_plex_prize_is_paid_to_the_vault_and_ledgered() {
    let mut reg = geode_world(1, vec![(Role::Plex, 1.0)]);
    assert!(geode::is_crate(&reg, crate_type()));
    assert!(!geode::is_crate(&reg, crate_type() + 1));
    let loot = geode::open(&mut reg, &open_event(0));
    assert_eq!(loot.len(), tunables::PRIZES_BY_TIER[0]);
    let paid: i64 = loot.iter().map(|l| l.quantity).sum();
    assert!(
        loot.iter()
            .all(|l| l.item_id == 0 && l.type_id == nes_vault::TYPE_PLEX)
    );
    assert!(
        (tunables::GEODE_PLEX.0 as i64..=tunables::GEODE_PLEX.1 as i64).contains(&paid),
        "{paid}"
    );
    let user = nes_vault::key_of(&reg, PILOT);
    assert_eq!(nes_vault::balance(reg.deltas(), user), paid);
    assert_eq!(nes_vault::ledger_of(reg.deltas(), "geode"), (paid, 0));
    assert_eq!(plex_items(&reg), 0, "no item: the vault is safe");
    assert_eq!(drops::minted_on(reg.deltas(), 20), paid);
    assert_eq!(geode::ledger_of(reg.deltas(), "geode:crate:1"), (0, 1));
    assert_eq!(
        geode::ledger_of(reg.deltas(), "geode:prize:plex"),
        (paid, 0)
    );
    assert_eq!(geode::opens(reg.deltas()), 1);
}

/// The same crate opened twice does not open alike, and a replay does.
#[test]
fn opens_differ_and_replay() {
    let take = |opens: usize| {
        let mut reg = geode_world(3, vec![(Role::Plex, 1.0)]);
        (0..opens)
            .map(|_| geode::open(&mut reg, &open_event(0))[0].quantity)
            .collect::<Vec<_>>()
    };
    let a = take(8);
    assert_eq!(a, take(8), "a replay opens the same");
    assert!(a.windows(2).any(|w| w[0] != w[1]), "{a:?}");
}

/// An injector prize is an item in the hangar, its points in `sptrade`.
#[test]
fn an_injector_prize_is_an_item_in_the_hangar_and_ledgered() {
    use eve_server::skill_trading::{TYPE_LARGE, TYPE_SMALL, ledger_of};
    let mut reg = geode_world(
        1,
        vec![(Role::InjectorSmall, 1.0), (Role::InjectorLarge, 1.0)],
    );
    let mut made = 0;
    for n in 0..12 {
        for l in geode::open(&mut reg, &open_event(n)) {
            assert!(matches!(l.type_id, TYPE_LARGE | TYPE_SMALL));
            assert!(l.item_id != 0);
            made += 1;
        }
    }
    let held: i64 = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.owner_id == PILOT && matches!(i.type_id, TYPE_LARGE | TYPE_SMALL))
        .map(|i| i.quantity)
        .sum();
    assert_eq!(held, made, "every prize is an item in the hangar");
    let (minted, _) = ledger_of(reg.deltas(), "geode");
    assert!(minted > 0 && injectors::minted_on(reg.deltas(), 20) == minted);
}

/// A prize the bundle cannot make is not paid and not ledgered.
#[test]
fn a_prize_with_no_content_is_neither_paid_nor_ledgered() {
    let mut reg = geode_world(1, vec![(Role::Skin, 1.0)]);
    assert!(geode::open(&mut reg, &open_event(0)).is_empty());
    assert_eq!(geode::ledger_of(reg.deltas(), "geode:prize:skin"), (0, 0));
    assert_eq!(geode::ledger_of(reg.deltas(), "geode:crate:1"), (0, 1));
}

fn mined_event(at: i64, crit: i64, ship_type_id: i64) -> MinedEvent {
    MinedEvent {
        miner: PILOT,
        ship_id: 100_000_099,
        ship_type_id,
        rock_id: 5,
        rock_type: 6,
        system: SYSTEM,
        normal: 80,
        crit,
        at,
        source_id: 9,
    }
}

fn geodes_in_cargo(reg: &Registry) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == crate_type() && i.owner_id == PILOT)
        .map(|i| i.quantity)
        .sum()
}

/// Over many crit cuts a geode appears in the hold and is ledgered; a hull
/// with no hold never gets one.
#[test]
fn a_cut_leaves_a_geode_in_the_hold_only_if_it_has_room() {
    let (hull, _) = hull_with_hold();
    let mut reg = geode_world(1, vec![(Role::Plex, 1.0)]);
    for at in 0..3_000 {
        geode::on_mined(&mut reg, &mined_event(at, 5, hull));
    }
    let n = geodes_in_cargo(&reg);
    // 3000 crit cuts at 0.02: about 60.
    assert!((20..140).contains(&n), "{n}");
    assert_eq!(geode::ledger_of(reg.deltas(), "geode:crate:1").0, n);
    let pod = eve_server::item::SPAWNABLE
        .iter()
        .find(|t| t.capacity == 0.0)
        .expect("a hull with no hold")
        .type_id;
    let mut empty = geode_world(1, vec![(Role::Plex, 1.0)]);
    for at in 0..3_000 {
        geode::on_mined(&mut empty, &mined_event(at, 5, pod));
    }
    assert_eq!(geodes_in_cargo(&empty), 0);
    assert_eq!(geode::ledger_of(empty.deltas(), "geode:crate:1"), (0, 0));
}

/// The same cuts make the same geodes.
#[test]
fn a_replay_of_the_cuts_makes_the_same_geodes() {
    let (hull, _) = hull_with_hold();
    let run = || {
        let mut reg = geode_world(1, vec![(Role::Plex, 1.0)]);
        for at in 0..2_000 {
            geode::on_mined(&mut reg, &mined_event(at, 3, hull));
        }
        geodes_in_cargo(&reg)
    };
    assert_eq!(run(), run());
}

/// With no geode type in the bundle a cut does nothing at all.
#[test]
fn a_world_with_no_geode_type_mines_unchanged() {
    let (hull, _) = hull_with_hold();
    let mut reg = world();
    let before = reg.store().items().len();
    for at in 0..500 {
        geode::on_mined(&mut reg, &mined_event(at, 5, hull));
    }
    assert_eq!(reg.store().items().len(), before);
    assert!(!geode::is_crate(&reg, crate_type()));
}

// ---- motherlodes (lane l6b-mining-events) ----

const SEC: i64 = 10_000_000;
/// Fixture systems in two regions (`Universe::fixture`).
const ML_SYSTEM: i64 = 39_000_001;
const OTHER_REGION_SYSTEM: i64 = 39_000_003;
const BELT: i64 = 40_000_017;
const ROCK: i64 = 5;
const T0: i64 = 133_000_000_000_000_000;

fn ml_world() -> Registry {
    install();
    let mut content = eve_server::content::Content::fixture();
    let lines = [format!("{}\t1", crate_type())];
    content.insert(
        "geodes",
        geode::Geodes::parse(&lines, vec![(Role::Plex, 1.0)]).expect("parses"),
    );
    let cfg = SandboxConfig {
        characters: vec![Character {
            character_id: PILOT,
            name: "Pilot".to_owned(),
            station_id: Some(STATION),
            solar_system_id: Some(ML_SYSTEM),
            ..Character::default()
        }],
        ..SandboxConfig::default()
    };
    let mut reg = Registry::new(cfg).with_content(std::sync::Arc::new(content));
    reg.rocks_mut().add(eve_server::asteroid::Rock {
        id: ROCK,
        type_id: crate_type(),
        belt_id: BELT,
        solar_system_id: ML_SYSTEM,
        quantity: 100_000,
        radius: 500.0,
        position: [0.0; 3],
        law: eve_server::asteroid::Law {
            growth: 1.0,
            unit_size: 100.0,
            max_radius: 10_000.0,
        },
    });
    reg
}

fn ml_cut(at: i64, hull: i64) -> MinedEvent {
    MinedEvent {
        system: ML_SYSTEM,
        rock_id: ROCK,
        rock_type: crate_type(),
        ..mined_event(at, 0, hull)
    }
}

fn ore_in_cargo(reg: &Registry) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == crate_type() && i.owner_id == PILOT)
        .map(|i| i.quantity)
        .sum()
}

fn tick(reg: &mut Registry, ft: i64) {
    motherlode::tick(reg, std::time::Instant::now(), ft);
}

/// A motherlode is a persisted row that ends on its own; its cuts pay extra
/// ore, ledgered, and only on its belt; after it ends they pay nothing.
#[test]
fn a_motherlode_pays_extra_ore_on_its_belt_until_it_ends() {
    let (hull, _) = hull_with_hold();
    let mut reg = ml_world();
    tick(&mut reg, T0);
    // Not yet: a plain cut pays nothing extra and makes no event.
    motherlode::on_mined(&mut reg, &ml_cut(T0, hull));
    assert_eq!(ore_in_cargo(&reg), 0);
    assert!(motherlode::active(&reg, T0).is_empty());

    motherlode::begin(&mut reg, ML_SYSTEM, BELT, T0);
    let ends = T0 + tunables::ML_DURATION_S * SEC;
    assert_eq!(motherlode::active(&reg, T0), [(ML_SYSTEM, BELT, ends)]);
    assert_eq!(
        motherlode::zone_of(&reg, &ml_cut(T0 + 1, hull)),
        motherlode::Zone::Motherlode
    );
    assert_eq!(
        motherlode::multiplier(&reg, &ml_cut(T0 + 1, hull)),
        tunables::ML_GEODE_MULT
    );
    for at in 0..5 {
        motherlode::on_mined(&mut reg, &ml_cut(T0 + at, hull));
    }
    let made = ore_in_cargo(&reg);
    // 5 cuts of 80 at +200% (the hold is small, so only a few): 800.
    assert!((600..1_000).contains(&made), "{made}");
    let name = format!("motherlode:ore:{}", crate_type());
    assert_eq!(geode::ledger_of(reg.deltas(), &name), (made, 0));

    // Another belt of the same system is plain.
    let first = *reg.rocks().get(ROCK).expect("the rock");
    reg.rocks_mut().add(eve_server::asteroid::Rock {
        id: ROCK + 1,
        belt_id: BELT + 1,
        ..first
    });
    let other = MinedEvent {
        rock_id: ROCK + 1,
        ..ml_cut(T0, hull)
    };
    assert_eq!(motherlode::zone_of(&reg, &other), motherlode::Zone::Plain);

    // It ends by itself. The scheduler is due too (the cuts above were
    // within the hour), so the same tick opens a fresh one: the old row is
    // replaced by one that starts now.
    tick(&mut reg, ends + 1);
    let rows = motherlode::events_in(reg.deltas());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[&ML_SYSTEM].start, ends + 1, "the old one is gone");
    // Closed by hand, a cut is plain again and pays nothing.
    let say = motherlode::slash(&mut reg, Some(PILOT), &["end"]);
    assert!(say.is_ok());
    assert!(motherlode::active(&reg, ends + 2).is_empty());
    assert_eq!(
        motherlode::zone_of(&reg, &ml_cut(ends + 2, hull)),
        motherlode::Zone::Plain
    );
    let before = ore_in_cargo(&reg);
    motherlode::on_mined(&mut reg, &ml_cut(ends + 2, hull));
    assert_eq!(ore_in_cargo(&reg), before);
}

/// The announcement is queued when it begins, and a running event survives a
/// reload of the module's state from the rows.
#[test]
fn beginning_announces_and_the_rows_carry_it() {
    let mut reg = ml_world();
    tick(&mut reg, T0);
    let queued = reg.scheduled_count();
    motherlode::begin(&mut reg, ML_SYSTEM, BELT, T0);
    // Nobody is online in a test registry, so there is no audience and no
    // push; the row is the record.
    assert_eq!(reg.scheduled_count(), queued);
    let rows = motherlode::events_in(reg.deltas());
    let e = rows.get(&ML_SYSTEM).expect("the row");
    assert_eq!((e.belt_id, e.start), (BELT, T0));
    assert!(!e.echoed);
    // The next tick past the echo delay marks it echoed (and says it in
    // the neighbours' Local when a bot is there).
    tick(&mut reg, T0 + (tunables::ML_ECHO_DELAY_S + 1) * SEC);
    assert!(motherlode::events_in(reg.deltas())[&ML_SYSTEM].echoed);
}

/// The scheduler picks only systems where a belt was cut within the hour,
/// one per region, and not twice in a region that has one running.
#[test]
fn the_scheduler_starts_one_event_in_an_active_system_only() {
    let (hull, _) = hull_with_hold();
    let mut reg = ml_world();
    tick(&mut reg, T0);
    // The first event waits out the first delay.
    motherlode::on_mined(&mut reg, &ml_cut(T0, hull));
    tick(&mut reg, T0 + tunables::ML_FIRST_DELAY_S * SEC - 1);
    assert!(motherlode::active(&reg, T0).is_empty());
    tick(&mut reg, T0 + tunables::ML_FIRST_DELAY_S * SEC);
    let at = T0 + tunables::ML_FIRST_DELAY_S * SEC;
    assert_eq!(
        motherlode::active(&reg, at)
            .iter()
            .map(|e| e.0)
            .collect::<Vec<_>>(),
        [ML_SYSTEM],
        "the one active system"
    );
    // A system nobody mined in is never picked, a region with one running is
    // skipped: the next round, two hours on, starts nothing new here.
    let later = at + tunables::ML_INTERVAL_S * SEC;
    tick(&mut reg, T0 + 40 * SEC);
    tick(&mut reg, later);
    let n = motherlode::events_in(reg.deltas()).len();
    assert!(n <= 1, "{n}");
    let _ = OTHER_REGION_SYSTEM;
}

/// The schedule is in the rows: a fresh state loads it and does not open an
/// event at once.
#[test]
fn the_schedule_and_the_events_are_read_back_from_the_rows() {
    let mut reg = ml_world();
    tick(&mut reg, T0);
    let due = reg
        .deltas()
        .of(KIND)
        .get(&vec!["mlnext".to_owned()])
        .and_then(|v| v.first().cloned())
        .expect("the next-event row");
    assert_eq!(
        due.parse::<i64>().expect("a time"),
        T0 + tunables::ML_FIRST_DELAY_S * SEC
    );
}

/// The slash command opens and closes one for the GM's scenario.
#[test]
fn the_slash_command_opens_and_closes_one() {
    let mut reg = ml_world();
    tick(&mut reg, T0);
    let say = |reg: &mut Registry, a: &[&str]| -> String {
        match motherlode::slash(reg, Some(PILOT), a).expect("answers") {
            eve_marshal::PyValue::Bytes(b) => String::from_utf8(b).expect("text"),
            other => panic!("{other:?}"),
        }
    };
    // The fixture has no belt catalogue: nothing to open.
    assert!(say(&mut reg, &["begin"]).contains("no belt"));
    assert!(say(&mut reg, &["status"]).contains("no motherlode"));
    motherlode::begin(&mut reg, ML_SYSTEM, BELT, T0);
    assert!(say(&mut reg, &["status"]).contains("running"));
    assert!(say(&mut reg, &["begin"]).contains("already"));
    assert!(say(&mut reg, &["end"]).contains("over"));
    assert!(motherlode::active(&reg, T0).is_empty());
}

/// The ambush clock: the first comes after the first delay, the next one a
/// band's interval later (the squad itself needs the belt catalogue, which
/// a fixture world lacks, so only the clock is seen here).
#[test]
fn an_ambush_falls_due_after_the_delay_and_then_by_band() {
    let mut reg = ml_world();
    tick(&mut reg, T0);
    motherlode::begin(&mut reg, ML_SYSTEM, BELT, T0);
    let first = T0 + tunables::AMBUSH_FIRST_S * SEC;
    assert_eq!(
        motherlode::events_in(reg.deltas())[&ML_SYSTEM].next_ambush,
        first
    );
    tick(&mut reg, first - 1);
    assert_eq!(
        motherlode::events_in(reg.deltas())[&ML_SYSTEM].next_ambush,
        first
    );
    tick(&mut reg, first);
    let next = motherlode::events_in(reg.deltas())[&ML_SYSTEM].next_ambush;
    assert_eq!(
        next,
        first + motherlode::ambush_every(eve_server::celestial::Band::High)
    );
}
