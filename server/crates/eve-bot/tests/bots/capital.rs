//! **CAP-L over a real socket: a capital pair.** Two plain connections stand
//! in for the pair's two bots and run the pair's own verbs (`eve_bot::capital`):
//! Alice (the alt) and Bob (the pilot) form a fleet; Bob's hangar is stocked
//! into his fuel bay by [`capital::stock_bay`]; Alice lights a cynosural field
//! (`dogmaIM.Activate`, [`capital::light`]); Bob is told of it
//! (`OnFleetJumpBeaconModuleChange`, folded into [`capital::Beacons`]) and runs
//! one real [`capital::turn`], which reads his drive, his fuel, the timers and
//! the distance, takes [`capital::cap_turn`]'s step and jumps
//! (`CmdJumpToFleetModuleBeacon`). The assertions are about ids and order:
//! which system Bob is in, what fuel he spent, whether his timer runs.
//!
//! **Every id is invented** (types 990,700,xxx, systems 30,700,xxx); the
//! client's constants appear by name. No `[A]` value is an expected value: the
//! drive's range, per-light-year burn and the distance are in the invented rows
//! the server reads, and the fuel spent is the server's own `fuel_required`.

use super::common::{config, login, pilot, until};
use eve_bot::brain::{Memory, Policy};
use eve_bot::capital::{self, Ctx};
use eve_server::server::{Config, Mode, Server};
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const ALICE_SHIP: i64 = 100_000_001;
const BOB_SHIP: i64 = 100_000_002;

/// Invented: the cyno module, its fuel (ozone), the isotope, the jump hull.
const CYNO: i64 = 990_700_001;
const OZONE: i64 = 990_700_002;
const ISOTOPE: i64 = 990_700_003;
const JUMPER: i64 = 990_700_004;
/// Groups: the fuel's is the client's `groupIceProduct` (the bay takes only
/// it, CAP-B); the hull's is the Dreadnought's (the pilot's role asks for a
/// jumper group); the module's is invented.
const GROUP_FUEL: i64 = 423;
const GROUP_JUMPER: i64 = 485;
const GROUP_CYNO: i64 = 990_700;
const LIST_PLAIN: i64 = 990_760;
const CYCLE_MS: i64 = 600_000;
const OZONE_PER_CYCLE: i64 = 500;
/// The client's names, which the server must agree on.
const EFFECT_ID: i64 = 2_857;
const EFFECT_NAME: &str = "cynosuralGeneration";
const TYPE_CYNOSURAL_FIELD: i64 = 21_094;
const HI_SLOT_0: i64 = 27;
const CARGO: i64 = 5;
const HANGAR: i64 = 4;

const HOME: i64 = 30_000_001;
const DEST: i64 = 30_700_002;
/// Invented drive and distance (never an integer product).
const RANGE: f64 = 5.0;
const PER_LY: f64 = 1_000.0;
const DEST_LY: f64 = 3.2504;
const BAY_CAPACITY: f64 = 50_000.0;
const SPAWN: f64 = 5_000.0;
/// Isotopes in Bob's hangar: enough for the jump and the margin.
const STOCK: i64 = 6_000;

fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{OZONE}\t{GROUP_FUEL}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n\
         type\t{ISOTOPE}\t{GROUP_FUEL}\t4\t1\t1\t0.15\t0.0\t0.0\t1.0\t0\t\t\tInvented Isotope\n\
         type\t{JUMPER}\t{GROUP_JUMPER}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=355.0;482=260.0;861=1.0;866={ISOTOPE}.0;867={RANGE};868={PER_LY};\
         1549={BAY_CAPACITY};3317={LIST_PLAIN}.0\t\
         Invented Jumper\n\
         type\t{CYNO}\t{GROUP_CYNO}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{EFFECT_ID}\t12,16,{EFFECT_ID}\t\
         30=1.0;50=1.0;73={CYCLE_MS}.0;713={OZONE}.0;714={OZONE_PER_CYCLE}.0;763=1.0;1245=1.0;\
         1334=500.0;2455={SPAWN}\t\
         Invented Cyno Generator\n\
         effect\t{EFFECT_ID}\t1\t0\t73\t0\t0\t0\t0\t{EFFECT_NAME}\t\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let sys = |id: i64, name: &str| {
        format!("{id}\t10700001\t20700001\t\t\t-0.45\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = eve_server::celestial::Universe::parse(
        &[sys(HOME, "Invented Home"), sys(DEST, "Invented Dest")],
        &["10700001\t\t\t\t\tInvented Region".to_owned()],
        &["20700001\t10700001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    let centers = eve_server::celestial::Centers::parse(&[
        format!("s\t{HOME}\t0.0\t0.0\t0.0"),
        format!(
            "s\t{DEST}\t{}\t0.0\t0.0",
            DEST_LY * eve_server::celestial::LIGHTYEAR
        ),
    ])
    .expect("the test centres parse");
    c.insert("centers", centers);
    let mut lists = eve_server::dungeon_data::Table::fixture();
    lists.typelists.insert(
        LIST_PLAIN,
        eve_server::dungeon_data::TypeList {
            inc_types: vec![TYPE_CYNOSURAL_FIELD],
            ..Default::default()
        },
    );
    c.insert("dungeon_data", lists);
    c
}

#[test]
fn a_pilot_stocks_its_bay_and_jumps_to_its_alts_cyno() {
    let mut cfg: Config = config();
    cfg.mode = Mode::Gps;
    cfg.sandbox.characters = vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
    ];
    let content = Arc::new(content());
    cfg.content = Some(content.clone());
    let server = Server::bind(cfg).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    let mut bob = login(addr, "Bob");
    bob.keep_pushes();

    // Items: Alice's module and its ozone; Bob's hull and the isotopes in his
    // hangar (the pilot buys nothing here: the hangar's own are bayed first).
    let (module_id, station) = {
        let mut w = world.write().expect("the world");
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut().filter(|i| i.item_id == BOB_SHIP) {
            i.type_id = JUMPER;
            i.group_id = GROUP_JUMPER;
        }
        w.apply_snapshot(snap);
        w.character_mut(BOB).expect("Bob").ship_type_id = JUMPER;
        let station = w.character(BOB).expect("Bob").station_id.expect("docked");
        w.apply_items(&[
            eve_server::item::Edit::Create {
                type_id: CYNO,
                quantity: 1,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_0),
            },
            eve_server::item::Edit::Create {
                type_id: OZONE,
                quantity: 5 * OZONE_PER_CYCLE,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, CARGO),
            },
            eve_server::item::Edit::Create {
                type_id: ISOTOPE,
                quantity: STOCK,
                to: eve_server::item::Slot::new(BOB, station, HANGAR),
            },
        ])
        .expect("the items are made");
        let module = w
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == CYNO && i.owner_id == ALICE)
            .expect("her module")
            .item_id;
        (module, station)
    };
    let _ = station;

    // **The fleet**: the alt commands (as the role does), the pilot accepts.
    let fleet = alice
        .create_fleet(super::common::FLEET_SHIP)
        .expect("the fleet");
    let seat = alice.fleet_seat(fleet, 0).expect("a seat");
    alice.fleet_invite(fleet, BOB, seat).expect("the invite");
    let mut invited = None;
    assert!(
        until(Duration::from_secs(10), || {
            let _ = bob.collect_pushes(Duration::from_millis(100));
            for (e, a) in bob.take_pushes() {
                if let Some(i) = eve_bot::FleetInvite::from_push(&e, &a) {
                    invited = Some(i.fleet_id);
                }
            }
            invited.is_some()
        }),
        "Bob is invited"
    );
    bob.accept_fleet_invite(fleet, super::common::FLEET_SHIP)
        .expect("accepts");

    // **Fuel logistics**: the hangar's isotopes go into the bay, the rest of
    // the want into the hold; the bay holds the lion's share.
    let ship = bob.session().ship_id().expect("Bob's ship");
    let fuel = capital::read_fuel(&mut bob, Some(&content), ship)
        .expect("the drive reads")
        .expect("a jump drive");
    assert_eq!(fuel.isotope, ISOTOPE);
    assert_eq!(fuel.range_ly, RANGE);
    assert_eq!(fuel.per_ly, PER_LY);
    let (bought, bayed, held) = capital::stock_bay(&mut bob, ship, ISOTOPE, STOCK).expect("stocks");
    assert_eq!(bought, 0, "the hangar had the isotopes");
    assert_eq!(
        bayed + held,
        STOCK,
        "all of it moved ({bayed} bay, {held} hold)"
    );
    assert!(bayed > 0, "the fuel bay took some");
    let in_bay = capital::held_at(&mut bob, ship, capital::FUEL_BAY, ISOTOPE).expect("reads");
    assert_eq!(in_bay, bayed);

    // **Alice lights** in the other system.
    alice.undock().expect("Alice undocks");
    capital::bind(&mut alice).expect("binds");
    {
        let mut w = world.write().expect("the world");
        let _ = w.roster_mut().leave(ALICE);
        w.roster_mut()
            .arrive(eve_server::space_state::Presence::arriving(
                ALICE,
                ALICE_SHIP,
                DEST,
                eve_server::space_state::Motion::arrived([1.0e7, 2.0e6, -3.0e6], [0.0; 3]),
            ));
    }
    capital::light(&mut alice, module_id).expect("the cyno lights");

    // **Bob is told**, and takes one real turn: docked, he reads his drive,
    // the timers and the distance, then undocks and jumps.
    let mut policy = Policy::fast();
    policy.role = &capital::ROLE;
    policy.partner = Some(ALICE);
    let mut memory = Memory::new("Bob", policy);
    assert!(
        until(Duration::from_secs(10), || {
            let _ = bob.collect_pushes(Duration::from_millis(100));
            for (e, a) in bob.take_pushes() {
                memory.capital.beacons.on_push(&e, &a);
            }
            memory.capital.beacons.of(ALICE).is_some()
        }),
        "Bob is told of the beacon"
    );
    let beacon = memory.capital.beacons.of(ALICE).expect("held");
    assert_eq!(beacon.system, DEST);
    assert_eq!(beacon.type_id, TYPE_CYNOSURAL_FIELD);

    let ctx = Ctx {
        map: None,
        floor: eve_bot::travel::SECURITY_FLOOR,
        content: Some(&content),
        alt: false,
    };
    let note = capital::turn(&mut bob, &mut memory, &ctx).expect("the turn");
    assert!(note.contains("jumped"), "{note}");
    assert_eq!(bob.session().solar_system_id(), Some(DEST), "{note}");

    // **What it cost**: the server's own floor of the product, out of the
    // bay first; and the pair's timers now run.
    let spent = eve_server::jump_drive::fuel_required(DEST_LY, PER_LY);
    let bay_after = capital::held_at(&mut bob, ship, capital::FUEL_BAY, ISOTOPE).expect("reads");
    let hold_after = capital::held_at(&mut bob, ship, capital::CARGO, ISOTOPE).expect("reads");
    assert_eq!(bay_after + hold_after, STOCK - spent);
    let timers = capital::read_timers(&mut bob).expect("timers");
    assert!(
        !timers
            .activation_wait(eve_server::pyutil::filetime_now())
            .is_zero(),
        "the activation timer runs after a jump"
    );
    assert_eq!(memory.capital.legs, 1);

    // Another turn at once: the field is in his own system now, so he waits,
    // and he does not jump again.
    let again = capital::turn(&mut bob, &mut memory, &ctx).expect("the turn");
    assert!(again.contains("waiting"), "{again}");
    capital::douse(&mut alice, module_id).expect("the cyno goes out");
}
