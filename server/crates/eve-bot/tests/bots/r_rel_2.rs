//! R-REL-2 over a real socket: what a bot sees through the client's calls
//! becomes relation facts. A kill by a pilot (with a second attacker on the
//! report) raises a grudge against him, his corp and the assister's corp; a
//! contract honoured improves regard on both sides.
//!
//! Invented rows only: `typedata::FIXTURE`'s frigate and autocannon, two
//! invented rounds and goods, invented corp ids. No `[A]` value is an expected
//! value: the assertions are about signs, ids and counts.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::career::{Band, CAREERS, Signals};
use eve_bot::contracts::{self, Offer};
use eve_bot::corpmind::{self, CorpMind, Hall, Member};
use eve_bot::mind::{Mind, TRAIT_COUNT, Traits};
use eve_bot::observe;
use eve_bot::society::{Profile, Satisfaction};
use eve_marshal::PyValue;
use eve_server::model::Character;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ALICE: i64 = 90_000_701;
const BOB: i64 = 90_000_702;
const CAROL: i64 = 90_000_703;
const ALICE_CORP: i64 = 98_000_701;
const BOB_CORP: i64 = 98_000_702;
const CAROL_CORP: i64 = 98_000_703;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
const ROUND: i64 = 990_630_011;
const WEAK_ROUND: i64 = 990_630_012;
const GOOD: i64 = 990_901_011;
/// Dear enough that a trade moves regard past the matrix's epsilon.
const UNIT: f64 = 60_000.0;
const UNITS: i64 = 100;
const WALLET: f64 = 10_000_000.0;
/// Game seconds for the observer's clock.
const NOW: u64 = 10_000;

fn in_corp(c: Character, corp: i64) -> Character {
    Character {
        corporation_id: corp,
        ..c
    }
}

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n\
         type\t{WEAK_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1.0;128=1.0\tFixture Weak Round\n\
         type\t{GOOD}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Contract Good\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let mut prices = eve_server::econ_prices::Table::fixture();
    prices.prices.extend(
        eve_server::econ_prices::Table::parse(&[format!(
            "{GOOD}\t{UNIT}\tbase\t{UNIT}\t6\t99010\t\t1"
        )])
        .expect("parses")
        .prices,
    );
    content.insert("econ_prices", prices);
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        in_corp(pilot(ALICE, "Alice", 100_000_701), ALICE_CORP),
        Character {
            balance: WALLET,
            ..in_corp(pilot(BOB, "Bob", 100_000_702), BOB_CORP)
        },
        in_corp(pilot(CAROL, "Carol", 100_000_703), CAROL_CORP),
    ];
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

fn member(corp: i64) -> Member {
    Member {
        name: "Invented".to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "pirate".to_owned(),
        band: Band::Low,
        corp,
        alliance: None,
        // A turn just taken: the hall's own social turn does nothing.
        last_turn: Some(u64::MAX / 2),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

/// A hall holding bot corps `(corp, ceo)`.
fn hall(corps: &[(i64, i64)]) -> Hall {
    let hall = Hall::new(60).with_every(0);
    {
        let mut st = hall.lock();
        for &(corp, ceo) in corps {
            st.members.insert(ceo, member(corp));
            st.corps.insert(
                corp,
                CorpMind {
                    id: corp,
                    ceo: Some(ceo),
                    profile: Profile::of("pirate", Band::Low),
                    founded: 0,
                    alliance: None,
                    name: String::new(),
                    applied_alliance: None,
                    salt: 0,
                },
            );
        }
    }
    hall
}

fn mind() -> Arc<Mutex<Mind>> {
    Arc::new(Mutex::new(Mind::seeded(
        "Invented Pilot",
        7,
        CAREERS,
        &Signals::default(),
        0,
    )))
}

fn regard(m: &Arc<Mutex<Mind>>, who: i64) -> f64 {
    m.lock()
        .unwrap()
        .regard
        .iter()
        .find(|r| r.who == who)
        .map_or(0.0, |r| r.score)
}

/// Docked: create a fixture frigate and board it.
fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate is in the hangar");
    c.board(hull.item_id).expect("boards the frigate");
    hull.item_id
}

/// Docked: board a fixture frigate, fit and load the autocannon with
/// `round`, undock into space. Answers the fitted gun.
fn arm(c: &mut Conn, round: i64) -> eve_bot::ItemRow {
    board_frigate(c);
    gm(c, &format!("/createitem {AUTOCANNON}"));
    gm(c, &format!("/createitem {round} 10"));
    let station = c.session().station_id().expect("docked");
    let spare: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == AUTOCANNON)
        .map(|r| r.item_id)
        .collect();
    let gun = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !c.is_online(gun.item_id).expect("asks") {
        c.set_online(gun.item_id).expect("onlines");
    }
    let rounds: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == round)
        .map(|r| r.item_id)
        .collect();
    c.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, round, station)
        .expect("loads");
    c.undock().expect("Alice undocks");
    c.enter_space(SYSTEM).expect("Alice's state");
    gun
}

/// Bob undocks in a fixture frigate; Carol hurts it with the weak round;
/// Alice undocks in one with the autocannon, locks Bob's ship and fires until
/// it is gone. Answers Alice's and Bob's connections.
fn fight() -> (Conn, Conn) {
    let (addr, content) = start();

    let mut bob = login(addr, "Bob");
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");

    let _carol = {
        let mut c = login(addr, "Carol");
        let gun = arm(&mut c, WEAK_ROUND);
        c.lock_target(bob_ship).expect("Carol locks");
        c.fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
            .expect("Carol fires");
        // One cycle later her volley is booked and Alice's will kill.
        std::thread::sleep(Duration::from_secs(5));
        c
    };
    let mut alice = login(addr, "Alice");
    let gun = arm(&mut alice, ROUND);

    alice.lock_target(bob_ship).expect("locks Bob's ship");
    alice
        .fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
        .expect("fires");

    let dead = until(Duration::from_secs(90), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        bob.session().ship_id() != Some(bob_ship)
    });
    assert!(dead, "Bob's ship dies");
    (alice, bob)
}

/// **A kill by a pilot raises a grudge against him and his corp.** Carol
/// grazes Bob's ship with the weak round first, Alice's volley kills it: Bob's
/// own social turn reads the report with both attackers. His regard of Alice
/// is negative (the character level) and his corp's view of Alice's corp is
/// down (the final blow's event). Carol's graze is on the report but under
/// the assist share, so neither she nor her corp is blamed (the assister's
/// own weight is the pure test `a_kill_by_a_pilot_raises_a_grudge_...`). A
/// second turn changes nothing.
#[test]
fn a_kill_by_a_pilot_raises_a_grudge_against_him_and_his_corp() {
    let (mut alice, mut bob) = fight();
    // The report does list both attackers.
    let rows = alice
        .call(
            "charMgr",
            "GetRecentShipKillsAndLosses",
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = rows else {
        panic!("a list")
    };
    assert_eq!(observe::seen_of(&rows)[0].hits.len(), 2);

    let h = hall(&[(BOB_CORP, BOB)]);
    let m = mind();
    corpmind::turn(&mut bob, &h, &m, CAREERS, NOW).expect("the social turn");
    assert!(regard(&m, ALICE) < 0.0, "a grudge against the killer");
    let view = |t: i64| h.lock().relations.get(BOB_CORP, t);
    assert!(
        view(ALICE_CORP) < 0.0,
        "his corp is red to the killer's corp"
    );
    assert_eq!(regard(&m, CAROL), 0.0, "a graze is no assist");
    assert_eq!(view(CAROL_CORP), 0.0);
    // Reading it all again moves nothing.
    let (relations, regards) = (h.snapshot().relations, m.lock().unwrap().regard.clone());
    observe::turn(&mut bob, &h, &m, NOW + 1).expect("the observer");
    assert_eq!(h.snapshot().relations, relations);
    assert_eq!(m.lock().unwrap().regard, regards);
}

/// **A honoured contract improves regard, on both sides.** Alice puts goods
/// up, Bob's trader takes them; each bot's observer then reads its finished
/// contracts and the other becomes a better-regarded pilot.
#[test]
fn a_honoured_contract_improves_regard_on_both_sides() {
    let (addr, _) = start();
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {GOOD} {UNITS}"));
    let station = alice.session().station_id().expect("docked");
    let item = alice
        .hangar_items()
        .expect("the hangar lists")
        .iter()
        .find(|r| r.type_id == GOOD)
        .expect("the stack is there")
        .item_id;
    let price = (UNIT * UNITS as f64 * contracts::UNDERPRICED_BELOW * 0.5).round();
    let id = contracts::create(
        &mut alice,
        station,
        &Offer {
            item_id: item,
            type_id: GOOD,
            quantity: UNITS,
            price,
        },
    )
    .expect("answered")
    .expect("created");
    let mut bob = login(addr, "Bob");
    let prices = BTreeMap::from([(GOOD, UNIT)]);
    let taken = contracts::take_underpriced_with(&mut bob, &prices).expect("accepts");
    assert_eq!(taken.contract_id, Some(id), "{}", taken.note);

    let h = hall(&[(ALICE_CORP, ALICE), (BOB_CORP, BOB)]);
    let (ma, mb) = (mind(), mind());
    let la = observe::turn(&mut alice, &h, &ma, NOW).expect("Alice's observer");
    let lb = observe::turn(&mut bob, &h, &mb, NOW).expect("Bob's observer");
    assert!(regard(&ma, BOB) > 0.0, "Alice trusts Bob more: {la:?}");
    assert!(regard(&mb, ALICE) > 0.0, "Bob trusts Alice more: {lb:?}");
    // The matrix heard the trade as the event it always was.
    assert!(h.lock().relations.get(ALICE_CORP, BOB_CORP) > 0.0);
    // Seen once: the same contract again adds nothing.
    let before = regard(&ma, BOB);
    observe::turn(&mut alice, &h, &ma, NOW + 24 * 3600).expect("again");
    assert_eq!(regard(&ma, BOB), before);
    let _ = PyValue::None;
}
