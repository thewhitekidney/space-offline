//! K-BOOST over a real socket: a command burst raises a fleet member's
//! attribute, and it falls back after the buff's duration.
//!
//! Alice flies a frigate with an invented command burst module (the client's
//! own effect `moduleBonusWarfareLinkArmor`, 6732, and charge effect 6737 with
//! its two real modifier rows) holding an invented charge. Bob flies a frigate
//! and joins her fleet the player way; Carol is in the same system and in no
//! fleet. Alice runs the burst. Then:
//!
//! - **Bob is boosted**: his client is told `OnDbuffUpdated` for his own ship
//!   (a destiny action) naming the collection, and `OnModuleAttributeChange`
//!   for the attribute the collection moves, at the burst's percent;
//! - **Alice is boosted too** (the client's own `ApplyBuffToSourceShip`), and
//!   hears `OnFleetBoosted` with one other ship;
//! - **Carol is not**: she is told nothing;
//! - **it ends with the duration**: Bob is told the attribute back and an
//!   empty `dbuffState`, which resets the client's tracker.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s frigate, an invented module,
//! charge and collection (ids in the 99xxxx band). The effect ids, the
//! attribute ids of the buff slots and the shield capacity attribute are the
//! client's own [V]. No `[A]` value is an expected value: the percent and the
//! duration are this file's own fixture numbers.

use super::common::{SYSTEM, config, gm, login, pilot, until};
use eve_bot::burst::{Lack, Step};
use eve_bot::fleet::Director;
use eve_bot::{Conn, FleetInvite};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::dbuff_data::Dbuffs;
use eve_server::server::Server;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_321;
const BOB: i64 = 90_000_322;
const CAROL: i64 = 90_000_323;
/// `typedata::FIXTURE`'s frigate and its shield capacity (attribute 263).
const FRIGATE: i64 = 990_000_001;
const SHIELD_CAPACITY: i64 = 263;
const FRIGATE_SHIELD: f64 = 420.0;
/// Invented: the burst module and the collection.
const BURST: i64 = 990_591_101;
/// The charge: a type id the director probes ([`eve_bot::burst::CHARGES`]),
/// with an invented row.
const CHARGE: i64 = eve_bot::burst::CHARGES[13].0;
/// The charge group an armor burst takes (the director matches it, so it is
/// the client's own [V-data] and not invented).
const CHARGE_GROUP: i64 = eve_bot::burst::KINDS[1].0;
const COLLECTION: i64 = 990_591_104;
/// The buff's percent on the shield: the module's value (1.0) times the
/// charge's multiplier.
const PERCENT: f64 = 10.0;
/// How long the buff lasts, ms: short, so the test sees it end.
const BUFF_MS: f64 = 3_000.0;
/// The client's own effect [V]: name and id.
const BURST_EFFECT: (&str, i64) = ("moduleBonusWarfareLinkArmor", 6732);

fn content() -> Arc<Content> {
    let rows = format!(
        "{}\
         attribute\t{SHIELD_CAPACITY}\t0.0\tshieldCapacity\n\
         type\t{BURST}\t1770\t7\t1\t1\t60.0\t3.0\t0.0\t1.0\t6732\t12,16,6732\t6=0.0;30=1.0;50=1.0;54=15000.0;56=1.0;73=600000.0;604={CHARGE_GROUP}.0;2469=1.0;2535={BUFF_MS}\tFixture Command Burst\n\
         type\t{CHARGE}\t{CHARGE_GROUP}\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t6737\t6737\t2468={COLLECTION}.0;2596={PERCENT}\tFixture Burst Charge\n\
         effect\t6732\t1\t0\t73\t6\t54\t0\t0\t{}\teffects.WarfareLinkSphereArmor\n\
         effect\t6737\t0\t0\t0\t0\t0\t0\t0\tchargeBonusWarfareCharge\t\n\
         modifier\t6737\totherID\tItemModifier\t2468\t2468\t7\t\t\n\
         modifier\t6737\totherID\tItemModifier\t2469\t2596\t4\t\t\n",
        eve_server::typedata::FIXTURE,
        BURST_EFFECT.0,
    );
    let mut content = Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let collections = vec![format!(
        "{COLLECTION}\tPostPercent\tMaximum\tShowNormal\tFixture Boost\tfixture"
    )];
    let modifiers = vec![format!("{COLLECTION}\titem\t{SHIELD_CAPACITY}\t\t")];
    content.insert(
        "dbuffs",
        Dbuffs::parse(&collections, &modifiers).expect("the collection parses"),
    );
    Arc::new(content)
}

fn ids_of(c: &mut Conn, type_id: i64) -> Vec<i64> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.item_id)
        .collect()
}

/// Docked: create a fixture frigate and board it.
fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = *ids_of(c, FRIGATE)
        .last()
        .expect("the frigate is in the hangar");
    c.board(hull).expect("boards the frigate");
    hull
}

/// `OnDbuffUpdated` entries in the destiny batches of `seen`, as
/// `(ship, [collection ids])`, in order.
fn dbuff_states(seen: &[(String, Vec<PyValue>)]) -> Vec<(i64, Vec<i64>)> {
    let mut out = Vec::new();
    for (event, args) in seen {
        if event != "DoDestinyUpdate" {
            continue;
        }
        let Some(PyValue::List(batch)) = args.first() else {
            continue;
        };
        for entry in batch {
            let PyValue::Tuple(entry) = entry else {
                continue;
            };
            let Some(PyValue::Tuple(call)) = entry.get(1) else {
                continue;
            };
            if call.first() != Some(&PyValue::Bytes(b"OnDbuffUpdated".to_vec())) {
                continue;
            }
            let Some(PyValue::Tuple(a)) = call.get(1) else {
                continue;
            };
            let (Some(PyValue::Int(ship)), Some(PyValue::List(list))) = (a.first(), a.get(1))
            else {
                continue;
            };
            let ids = list
                .iter()
                .filter_map(|row| match row {
                    PyValue::Tuple(r) => match r.first() {
                        Some(PyValue::Int(c)) => Some(*c),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            out.push((*ship, ids));
        }
    }
    out
}

/// `(new, old)` of every `OnModuleAttributeChange` for `attribute` of `ship`.
fn changes(seen: &[(String, Vec<PyValue>)], ship: i64, attribute: i64) -> Vec<(f64, f64)> {
    seen.iter()
        .filter(|(e, a)| {
            e == "OnModuleAttributeChange"
                && a.get(1) == Some(&PyValue::Int(ship))
                && a.get(2) == Some(&PyValue::Int(attribute))
        })
        .filter_map(|(_, a)| match (a.get(4), a.get(5)) {
            (Some(PyValue::Real(n)), Some(PyValue::Real(o))) => Some((*n, *o)),
            _ => None,
        })
        .collect()
}

/// Read `c` until `done(seen)` holds or twenty seconds; everything read.
fn until_seen(
    c: &mut Conn,
    done: impl Fn(&[(String, Vec<PyValue>)]) -> bool,
) -> Vec<(String, Vec<PyValue>)> {
    let mut seen: Vec<(String, Vec<PyValue>)> = Vec::new();
    until(Duration::from_secs(20), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        seen.extend(c.take_pushes());
        done(&seen)
    });
    seen
}

/// **A burst boosts the fleet and ends with its duration.**
#[test]
fn a_command_burst_raises_a_fleet_members_shield_and_it_falls_back() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_321),
        pilot(BOB, "Bob", 100_000_322),
        pilot(CAROL, "Carol", 100_000_323),
    ];
    config.content = Some(content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_ship = board_frigate(&mut bob);
    let mut carol = login(addr, "Carol");
    carol.keep_pushes();
    let carol_ship = board_frigate(&mut carol);

    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    let alice_ship = board_frigate(&mut alice);
    gm(&mut alice, &format!("/createitem {BURST}"));
    let spare = ids_of(&mut alice, BURST);
    let module = alice
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == BURST)
        .expect("the burst is fitted");
    if !alice.is_online(module.item_id).expect("asks") {
        alice.set_online(module.item_id).expect("onlines");
    }
    gm(&mut alice, &format!("/createitem {CHARGE} 10"));
    let rounds = ids_of(&mut alice, CHARGE);
    let station = alice.session().station_id().expect("docked");
    alice
        .load_ammo(
            &[(module.item_id, module.flag_id)],
            &rounds,
            CHARGE,
            station,
        )
        .expect("the charge loads");

    for c in [&mut alice, &mut bob, &mut carol] {
        c.undock().expect("undocks");
        c.enter_space(SYSTEM).expect("the park");
    }

    // The fleet, the player way: Alice invites Bob, Bob accepts.
    let fleet = alice.create_fleet(FRIGATE).expect("the fleet");
    let seat = alice.fleet_seat(fleet, 0).expect("a seat");
    alice.fleet_invite(fleet, BOB, seat).expect("the invite");
    let heard = until_seen(&mut bob, |s| {
        s.iter()
            .any(|(n, a)| FleetInvite::from_push(n, a).is_some())
    });
    let invite = heard
        .iter()
        .find_map(|(n, a)| FleetInvite::from_push(n, a))
        .expect("Bob is invited");
    bob.accept_fleet_invite(invite.fleet_id, FRIGATE)
        .expect("Bob joins");

    // Bob and Carol a kilometre from Alice, at rest: inside the module's range.
    {
        let mut w = world.write().expect("the world");
        let a = w.roster().get(ALICE).expect("Alice").motion.position;
        for (who, dx) in [(ALICE, 0.0), (BOB, 1_000.0), (CAROL, -1_000.0)] {
            let p = w.roster_mut().get_mut(who).expect("in space");
            p.motion.velocity = [0.0; 3];
            p.motion.position = [a[0] + dx, a[1], a[2]];
        }
    }
    let _ = bob.take_pushes();
    let _ = carol.take_pushes();
    let _ = alice.take_pushes();

    alice
        .activate(module.item_id, BURST_EFFECT, None, 1000)
        .expect("the burst runs");

    let boosted = FRIGATE_SHIELD * (100.0 + PERCENT) / 100.0;
    // Bob: the buff bar's state, and the shield at the burst's percent.
    let mut bob_seen = until_seen(&mut bob, |s| {
        !changes(s, bob_ship, SHIELD_CAPACITY).is_empty()
            && dbuff_states(s).contains(&(bob_ship, vec![COLLECTION]))
    });
    assert_eq!(
        dbuff_states(&bob_seen),
        [(bob_ship, vec![COLLECTION])],
        "{:?}",
        bob_seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    assert_eq!(
        changes(&bob_seen, bob_ship, SHIELD_CAPACITY),
        [(boosted, FRIGATE_SHIELD)]
    );

    // Alice: boosted herself, and told how many others.
    let alice_seen = until_seen(&mut alice, |s| {
        s.iter().any(|(e, _)| e == "OnFleetBoosted")
            && dbuff_states(s).contains(&(alice_ship, vec![COLLECTION]))
    });
    let told = alice_seen
        .iter()
        .find(|(e, _)| e == "OnFleetBoosted")
        .expect("OnFleetBoosted");
    assert_eq!(told.1, [PyValue::Int(BURST), PyValue::Int(1)]);
    assert_eq!(
        changes(&alice_seen, alice_ship, SHIELD_CAPACITY).first(),
        Some(&(boosted, FRIGATE_SHIELD))
    );

    // It falls back when the buff ends: the shield, and an empty state.
    bob_seen.extend(until_seen(&mut bob, |s| {
        changes(s, bob_ship, SHIELD_CAPACITY).contains(&(FRIGATE_SHIELD, boosted))
            && dbuff_states(s).contains(&(bob_ship, vec![]))
    }));
    assert!(
        changes(&bob_seen, bob_ship, SHIELD_CAPACITY).contains(&(FRIGATE_SHIELD, boosted)),
        "the shield falls back"
    );
    assert!(
        dbuff_states(&bob_seen).contains(&(bob_ship, vec![])),
        "the tracker is reset"
    );

    // Carol, in range but in no fleet, was told nothing.
    let _ = carol.collect_pushes(Duration::from_millis(500));
    let carol_seen = carol.take_pushes();
    assert!(
        dbuff_states(&carol_seen).is_empty()
            && changes(&carol_seen, carol_ship, SHIELD_CAPACITY).is_empty(),
        "Carol is not in the fleet"
    );
}

/// **The director's burst**, step by step over the socket: it lacks the
/// module it does not own; given one (the test's GM setup, not the bot's),
/// it fits (online) and loads it docked, runs it in space, and leaves it
/// running -- and its own ship is boosted.
#[test]
fn a_director_fits_loads_and_runs_the_burst_it_owns() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_321)];
    config.content = Some(content());
    let addr = super::common::serve(config);

    let mut alice = login(addr, "Alice");
    board_frigate(&mut alice);
    let mut director = Director::new(alice, FRIGATE);

    // Nothing owned: it lacks the module, and does nothing else (no GM).
    assert_eq!(director.burst().expect("a plan"), Step::Lacks(Lack::Module));
    assert!(
        ids_of(&mut director.conn, BURST).is_empty(),
        "the bot made nothing"
    );

    // Owned: a module, but no charge yet.
    gm(&mut director.conn, &format!("/createitem {BURST}"));
    let spare = ids_of(&mut director.conn, BURST);
    assert_eq!(
        director.burst().expect("fits"),
        Step::Fit { item: spare[0] }
    );
    // A module fits online (`modules::on_fitted`), so the next step is the
    // charge it lacks.
    assert_eq!(
        director.burst().expect("a step"),
        Step::Lacks(Lack::Charge {
            group: CHARGE_GROUP
        }),
        "fitted and online, no charge in the hangar"
    );

    // Charges owned: it loads them, then is ready.
    gm(&mut director.conn, &format!("/createitem {CHARGE} 10"));
    assert!(
        matches!(director.burst().expect("loads"), Step::Load { type_id, .. } if type_id == CHARGE)
    );
    assert_eq!(director.burst().expect("ready"), Step::Ready);

    // In space it runs, then leaves it running.
    director.conn.undock().expect("undocks");
    director.conn.enter_space(SYSTEM).expect("the park");
    assert_eq!(
        director.burst().expect("runs"),
        Step::Run {
            module: spare_module(&mut director),
            effect: BURST_EFFECT
        }
    );
    assert_eq!(director.burst().expect("still"), Step::Running);
    let ship = director.conn.session().ship_id().expect("a ship");
    let seen = until_seen(&mut director.conn, |s| {
        dbuff_states(s).contains(&(ship, vec![COLLECTION]))
    });
    assert!(
        dbuff_states(&seen).contains(&(ship, vec![COLLECTION])),
        "the director is boosted by his own burst"
    );
}

/// The fitted burst's item id (read back from the ship).
fn spare_module(d: &mut Director) -> i64 {
    let ship = d.conn.session().ship_id().expect("a ship");
    d.conn
        .ship_items(ship, None)
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == BURST && r.is_fitted())
        .expect("the burst is fitted")
        .item_id
}
