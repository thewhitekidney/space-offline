//! R-REL-3 over a real socket: the model's number becomes what the client
//! shows and what a bot does. A rival becomes a -10 contact in the corp book
//! and the personal book; a friend a +5; a red seller's contract is refused;
//! a blue's `NeedBackup` is answered.
//!
//! Invented ids and rows throughout; no `[A]` value is an expected value
//! (thresholds are named and the assertions are about signs, ids and
//! colours).

use super::common::{SYSTEM, config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::career::{Band, CAREERS, Career, INCOME_REF, Signals, Support};
use eve_bot::contracts::{self, Offer};
use eve_bot::corpmind::{self, Hall};
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::effects;
use eve_bot::fleet::Director;
use eve_bot::mind::{Mind, Regard, TRAIT_COUNT};
use eve_bot::society;
use eve_marshal::PyValue;
use eve_server::group_contacts::{Books, StandingFlag, relationship, standing_flag};
use eve_server::model::Character;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ALICE: i64 = 90_000_801;
const BOB: i64 = 90_000_802;
const CAROL: i64 = 90_000_803;
const DAVE: i64 = 90_000_804;
const BOT: i64 = 90_000_805;
/// Bob's corp on his slim item: an NPC corp id, invented.
const NPC_CORP: i64 = 1_000_001;
/// `typedata::FIXTURE`'s goods and a Rifter for the fleet boss.
const GOOD: i64 = 990_901_011;
const RIFTER: i64 = 587;
const UNIT: f64 = 60_000.0;
const UNITS: i64 = 100;
const NOW: u64 = 10_000;
/// A game-day, seconds.
const DAY: u64 = 60;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
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

fn regard_set(m: &Arc<Mutex<Mind>>, who: i64, score: f64) {
    let mut m = m.lock().unwrap();
    m.regard.retain(|r| r.who != who);
    m.regard.push(Regard {
        who,
        score,
        at: NOW,
    });
}

fn attr(v: &PyValue, name: &str) -> PyValue {
    let PyValue::Instance { state, .. } = v else {
        panic!("a KeyVal, got {v:?}")
    };
    eve_server::pyutil::dict_get(state, name.as_bytes())
        .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
        .clone()
}

fn number(v: &PyValue) -> f64 {
    match v {
        PyValue::Int(n) => *n as f64,
        PyValue::Real(r) => *r,
        other => panic!("a number, got {other:?}"),
    }
}

/// `bco_corporations.AddCorporation`'s fifteen positionals.
fn found(c: &mut eve_bot::Conn) -> i64 {
    let args = vec![
        uni("Invented Society Corp"),
        uni("ISCP"),
        uni("A test corp."),
        uni("http://"),
        PyValue::Real(0.1),
        PyValue::Int(419),
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::Bool(true),
        PyValue::Bool(false),
        PyValue::Real(0.0),
    ];
    let out = c
        .call("corpRegistry", "AddCorporation", args)
        .expect("answered");
    assert_eq!(out.refusal(), None, "the corp is founded");
    c.session().get_i64("corpid").expect("a corp")
}

/// The corp's book as the client's `GetCorporateContacts` returns it.
fn corp_book(c: &mut eve_bot::Conn, corp: i64) -> BTreeMap<i64, f64> {
    let got = c
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "GetCorporateContacts",
            vec![],
            vec![],
        )
        .expect("answered");
    let Some(PyValue::Dict(entries)) = got.value else {
        panic!("the corp book is a dict: {:?}", got.value)
    };
    entries
        .iter()
        .map(|(v, k)| {
            let PyValue::Int(id) = k else {
                panic!("keyed by contactID: {k:?}")
            };
            (*id, number(&attr(v, "relationshipID")))
        })
        .collect()
}

/// The bot's own book as `GetContactList` returns it.
fn personal_book(c: &mut eve_bot::Conn) -> BTreeMap<i64, f64> {
    let list = c
        .call("charMgr", "GetContactList", vec![])
        .expect("answered")
        .value()
        .expect("a list");
    let PyValue::List(rows) = attr(&list, "addresses") else {
        panic!("addresses")
    };
    rows.iter()
        .map(|r| {
            let PyValue::Int(id) = attr(r, "contactID") else {
                panic!("contactID")
            };
            (id, number(&attr(r, "relationshipID")))
        })
        .collect()
}

fn flag(book: &BTreeMap<i64, f64>, who: i64) -> StandingFlag {
    let empty = BTreeMap::new();
    standing_flag(&relationship(
        Books {
            personal: &empty,
            corp: book,
            alliance: &empty,
        },
        who,
        NPC_CORP,
        None,
    ))
}

/// **A red becomes a -10 contact the client lists** (the corp's book, set by
/// its CEO from the graded stance), a friend +5 and a wary pilot -5, each in
/// the colour the overview draws; the relation falls back and the contact
/// goes.
#[test]
fn a_red_becomes_a_minus_ten_contact_the_client_lists() {
    let mut config = config();
    let mut alice = pilot(ALICE, "Alice", 100_000_801);
    alice.balance = 1.0e9;
    config.sandbox.characters = vec![
        alice,
        pilot(BOB, "Bob", 100_000_802),
        pilot(CAROL, "Carol", 100_000_803),
        pilot(DAVE, "Dave", 100_000_804),
    ];
    let addr = serve(config);
    let mut c = login(addr, "Alice");
    let corp = found(&mut c);

    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    {
        let mut st = hall.lock();
        // What the facts left behind: a rival, a friend and a doubt.
        st.relations.add(corp, BOB, society::ENEMY_AT - 1.0);
        st.relations.add(corp, CAROL, society::FRIEND_AT + 0.5);
        st.relations.add(corp, DAVE, society::WARY_AT - 0.5);
    }
    let lines = corpmind::turn(&mut c, &hall, &m, CAREERS, NOW).expect("the social turn");
    let book = corp_book(&mut c, corp);
    assert_eq!(book.get(&BOB), Some(&-10.0), "{book:?}\n{lines:#?}");
    assert_eq!(book.get(&CAROL), Some(&5.0), "{book:?}\n{lines:#?}");
    assert_eq!(book.get(&DAVE), Some(&-5.0), "{book:?}\n{lines:#?}");
    // And the client colours them so.
    assert_eq!(flag(&book, BOB), StandingFlag::Horrible);
    assert_eq!(flag(&book, CAROL), StandingFlag::Good);
    assert_eq!(flag(&book, DAVE), StandingFlag::Bad);

    // The doubt fades: past the flip window and under the leave point the
    // contact is cleared (a bot-set one only).
    {
        let mut st = hall.lock();
        st.relations.r.remove(&(corp, DAVE));
        st.members.get_mut(&ALICE).expect("Alice").last_turn = None;
    }
    let later = NOW + 3 * DAY;
    corpmind::turn(&mut c, &hall, &m, CAREERS, later).expect("the next turn");
    let book = corp_book(&mut c, corp);
    assert!(!book.contains_key(&DAVE), "{book:?}");
    assert_eq!(book.get(&BOB), Some(&-10.0), "the rival stays");
}

/// **The bot's own book**: a grudge becomes a -10 personal contact, a
/// well-liked pilot +10, a protected friend is left, and a regard that faded
/// removes its contact.
#[test]
fn a_grudge_becomes_a_personal_red_contact_and_fading_clears_it() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_801),
        pilot(BOB, "Bob", 100_000_802),
        pilot(CAROL, "Carol", 100_000_803),
        pilot(DAVE, "Dave", 100_000_804),
    ];
    let addr = serve(config);
    let mut c = login(addr, "Alice");
    let hall = Hall::new(DAY).with_every(0);
    hall.befriend(DAVE, 3.0);
    // Dave is in the book from the friends file (applied elsewhere).
    c.call(
        "charMgr",
        "AddContact",
        vec![PyValue::Int(DAVE), PyValue::Real(3.0), PyValue::Bool(false)],
    )
    .expect("answered");
    let m = mind();
    regard_set(&m, BOB, -(society::ALLY_AT + 1.0));
    regard_set(&m, CAROL, society::ALLY_AT + 1.0);
    // A regard keyed by a corp is not a pilot and is never a contact.
    regard_set(&m, 98_000_888, -9.0);
    let lines = effects::personal_contacts(&mut c, &hall, &m, NOW).expect("tends the book");
    let book = personal_book(&mut c);
    assert_eq!(book.get(&BOB), Some(&-10.0), "{book:?}\n{lines:#?}");
    assert_eq!(book.get(&CAROL), Some(&10.0), "{book:?}\n{lines:#?}");
    assert_eq!(book.get(&DAVE), Some(&3.0), "the friends file's stays");
    assert!(!book.contains_key(&98_000_888));

    // Not again within the cadence; after it, a forgotten regard clears.
    regard_set(&m, BOB, 0.0);
    let soon = effects::personal_contacts(&mut c, &hall, &m, NOW + 1).expect("again");
    assert!(soon.is_empty(), "{soon:?}");
    let later = NOW + effects::PERSONAL_EVERY + 1;
    effects::personal_contacts(&mut c, &hall, &m, later).expect("later");
    let book = personal_book(&mut c);
    assert!(!book.contains_key(&BOB), "{book:?}");
    assert_eq!(book.get(&CAROL), Some(&10.0));
}

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{GOOD}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Contract Good\n",
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

/// **A red's contract is refused, a neutral's is taken**: Alice lists an
/// underpriced stack; Bob's trader holds a grudge against her and leaves it;
/// with the grudge gone he takes it.
#[test]
fn a_reds_contract_is_refused() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_801),
        Character {
            balance: 10_000_000.0,
            ..pilot(BOB, "Bob", 100_000_802)
        },
    ];
    config.content = Some(content());
    let addr = serve(config);
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
    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    // Bob holds a grudge: the contract stays on the board.
    regard_set(&m, ALICE, society::ENEMY_AT - 1.0);
    let refused = effects::take_underpriced_as(&mut bob, &hall, &m, NOW).expect("answered");
    assert_eq!(refused.contract_id, None, "{}", refused.note);
    // A doubt (wary) is not enough on a bargain this deep: the premium
    // narrows the bar, it does not shut it.
    regard_set(&m, ALICE, society::WARY_AT - 0.5);
    let wary = effects::take_underpriced_as(&mut bob, &hall, &m, NOW).expect("answered");
    assert_eq!(
        wary.contract_id,
        Some(id),
        "a deep bargain from a wary seller: {}",
        wary.note
    );
}

/// The same board, a wary seller whose price is only just under the normal
/// bar: not taken (it would have been from a stranger).
#[test]
fn a_wary_sellers_shallow_bargain_is_left() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_801),
        Character {
            balance: 10_000_000.0,
            ..pilot(BOB, "Bob", 100_000_802)
        },
    ];
    config.content = Some(content());
    let addr = serve(config);
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
    // Just under the normal bar: a stranger's, not a wary one's.
    let price = (UNIT * UNITS as f64 * contracts::UNDERPRICED_BELOW * 0.95).round();
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
    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    regard_set(&m, ALICE, society::WARY_AT - 0.5);
    let left = effects::take_underpriced_as(&mut bob, &hall, &m, NOW).expect("answered");
    assert_eq!(left.contract_id, None, "{}", left.note);
    regard_set(&m, ALICE, 0.0);
    let taken = effects::take_underpriced_as(&mut bob, &hall, &m, NOW).expect("answered");
    assert_eq!(taken.contract_id, Some(id), "{}", taken.note);
}

// ---- NeedBackup -------------------------------------------------------------------------

static TABLE: &[Career] = &[Career {
    name: "escort",
    role: Some(&eve_bot::roles::ESCORT),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::High,
    prior_isk_h: INCOME_REF,
}];

fn heard(h: &driver::Handle, who: i64) -> bool {
    h.log()
        .iter()
        .any(|e| e.kind == ActKind::Corp && e.what.starts_with(&format!("help {who}")))
}

/// **A blue's `NeedBackup` brings a bot; a stranger's does not.** The bot is
/// a society escort in Carol's fleet. Carol broadcasts `NeedBackup`; as a
/// stranger she is not answered, once the hall counts her a friend she is.
#[test]
fn a_blues_needbackup_brings_a_bot() {
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(BOT, "Botty", 100_000_805),
        Character {
            ship_type_id: RIFTER,
            ..pilot(CAROL, "Carol", 100_000_803)
        },
    ];
    let addr: SocketAddr = serve(c);

    let hall = Arc::new(Hall::new(DAY).with_every(3_600_000));
    let now = driver::unix_now();
    let mut mind = Mind::seeded("Botty", 1, TABLE, &Signals::default(), now);
    mind.career = "escort".to_owned();
    mind.since = now;
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(&hall));
    let mut cfg = BotConfig::new("Botty", addr, Pace::Fast).with_society(soc);
    cfg.chat = false;
    cfg.policy.accept_fleet = AcceptFleet::Any;
    let (bot, join) = driver::spawn(cfg);
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Undock))),
        "the bot sets itself up and undocks:\n{}",
        dump(&bot)
    );

    let mut boss = login(addr, "Carol");
    boss.undock().expect("Carol undocks");
    boss.enter_space(SYSTEM).expect("Carol's state");
    let mut director = Director::new(boss, RIFTER);
    assert!(
        director
            .form_fleet(&[BOT], Duration::from_secs(60))
            .expect("the fleet forms"),
        "the bot joined:\n{}",
        dump(&bot)
    );
    let ship = director.conn.session().ship_id().expect("Carol's ship");

    // A stranger's call: not answered.
    director
        .conn
        .fleet_broadcast("NeedBackup", ship, None)
        .expect("the broadcast is relayed");
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !heard(&bot, CAROL),
        "a stranger is not helped:\n{}",
        dump(&bot)
    );

    // Now a friend: the call is answered.
    hall.befriend(CAROL, 5.0);
    director
        .conn
        .fleet_broadcast("NeedBackup", ship, None)
        .expect("the broadcast is relayed");
    let came = until(Duration::from_secs(20), || heard(&bot, CAROL));
    bot.stop();
    let _ = join.join();
    assert!(came, "a friend's call is answered:\n{}", dump(&bot));
}
