//! R-REL-5 over a real socket: coalitions and wars.
//!
//! - two bot alliances whose members rate each other above the threshold
//!   court, set each other +10 and form a coalition the client lists;
//! - a bot alliance's executor declares war on a group its relation marks an
//!   enemy (the server's `DeclareWarAgainst`, the fee from the corp wallet),
//!   and once the war is hostile a member shoots a war target in an invented
//!   high-sec system at **green** safety, with no CONCORD and no criminal
//!   flag -- where the same shot before the war was refused;
//! - a hunter bot in a high-sec ground fires on a war target and leaves a
//!   bystander alone.
//!
//! Invented ids and rows throughout; no `[A]` value is an expected value
//! (thresholds are named, the assertions are about ids, signs and flags).

use super::common::{config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::brain::ActKind;
use eve_bot::career::{Band, CAREERS, Career, INCOME_REF, Signals, Support};
use eve_bot::coalition::{self, WarRow};
use eve_bot::corpmind::{self, CorpMind, Hall, Member};
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::hostile::Class;
use eve_bot::hunt::{self, Ground};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait, Traits};
use eve_bot::society::{self, Profile, Satisfaction};
use eve_marshal::PyValue;
use eve_server::group_contacts::{Books, StandingFlag, relationship, standing_flag};
use eve_server::model::Character;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ALICE: i64 = 90_000_951;
const BOB: i64 = 90_000_952;
const CAROL: i64 = 90_000_953;
const HUNTER: i64 = 90_000_954;
/// A game-day, seconds, and a start the clock's turns count from.
const DAY: u64 = 60;
const NOW: u64 = 10_000;
const A: i64 = 30_000_001;
const B: i64 = 30_000_003;
const GATE_A: i64 = 50_000_951;
const GATE_B: i64 = 50_000_952;
const GATE_TYPE: i64 = 990_029_001;
/// `typedata::FIXTURE`'s frigate and its autocannon, an invented round and an
/// invented wreck type (the same rows `crimewatch.rs` stands on).
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
const ROUND: i64 = 990_630_001;
const WRECK: i64 = 990_026_001;
/// `crimewatch/const.py` criminal timer **[V]**.
const TIMER_CRIMINAL: i64 = 303;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn mind(aggression: f64) -> Arc<Mutex<Mind>> {
    let m = Mind::seeded("Invented Pilot", 7, CAREERS, &Signals::default(), 0);
    let m = Arc::new(Mutex::new(m));
    m.lock().unwrap().traits.0[Trait::Aggression as usize] = aggression;
    m
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c.session().get_i64("corpid").expect("a corp");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

fn alliance_call(c: &mut Conn, alliance: i64, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let bind = PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]);
    c.moniker_call("allianceRegistry", bind, method, args, vec![])
        .unwrap_or_else(|e| panic!("allianceRegistry.{method}: {e}"))
}

/// A corp with a funded Master Wallet, and an alliance of it.
fn found(c: &mut Conn, name: &str, ticker: &str, alliance: Option<(&str, &str)>) -> (i64, i64) {
    gm(c, "/giveisk 2000000000");
    gm(c, "/giveisk 2000000000");
    let a = corp_call(
        c,
        "AddCorporation",
        vec![uni(name), uni(ticker), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp");
    let d = super::corp_wallet::deposit(c, corp, 3_000_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let al = match alliance {
        Some((n, t)) => {
            let a = corp_call(c, "CreateAlliance", vec![uni(n), uni(t), uni(""), uni("")]);
            assert_eq!(a.refusal(), None, "CreateAlliance refused");
            a.outcome
                .session_change
                .as_ref()
                .and_then(|s| s.new_i64("allianceid"))
                .expect("the founder's session gains allianceid")
        }
        None => 0,
    };
    (corp, al)
}

fn number(v: &PyValue) -> f64 {
    match v {
        PyValue::Int(n) => *n as f64,
        PyValue::Real(r) => *r,
        other => panic!("a number, got {other:?}"),
    }
}

fn attr(v: &PyValue, name: &str) -> PyValue {
    let PyValue::Instance { state, .. } = v else {
        panic!("a KeyVal, got {v:?}")
    };
    eve_server::pyutil::dict_get(state, name.as_bytes())
        .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
        .clone()
}

/// The alliance's book as the client's `GetAllianceContacts` returns it.
fn alliance_book(c: &mut Conn, alliance: i64) -> BTreeMap<i64, f64> {
    let got = alliance_call(c, alliance, "GetAllianceContacts", vec![]);
    let Some(PyValue::Dict(entries)) = got.value else {
        panic!("the alliance book is a dict: {:?}", got.value)
    };
    // Each entry carries its row in one half of the pair, whichever.
    entries
        .iter()
        .flat_map(|(x, y)| [x, y])
        .filter(|row| matches!(row, PyValue::Instance { .. }))
        .map(|row| {
            let PyValue::Int(id) = attr(row, "contactID") else {
                panic!("contactID")
            };
            (id, number(&attr(row, "relationshipID")))
        })
        .collect()
}

/// A turn of `c`'s bot on quarter-day `k`: the hall's own gate lifted.
fn turn(c: &mut Conn, hall: &Hall, m: &Arc<Mutex<Mind>>, who: i64, k: u64) -> Vec<String> {
    {
        let mut st = hall.lock();
        if let Some(me) = st.members.get_mut(&who) {
            me.last_turn = None;
        }
    }
    corpmind::turn(c, hall, m, CAREERS, NOW + k * DAY / 4).expect("the social turn")
}

// ---- coalitions ----------------------------------------------------------------------

/// **Two bot alliances above the threshold form a coalition the client
/// lists**: each executor's alliance book carries the other alliance at +10,
/// the client reads it as a high standing, and the hall derives one bloc.
#[test]
fn two_bot_alliances_above_the_threshold_form_a_coalition_the_client_lists() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_951),
        pilot(BOB, "Bob", 100_000_952),
    ];
    let addr = serve(cfg);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let (ca, a1) = found(
        &mut alice,
        "Alpha Invented",
        "ALPH",
        Some(("Alpha Pact", "ALPP")),
    );
    let (cb, a2) = found(
        &mut bob,
        "Bravo Invented",
        "BRAV",
        Some(("Bravo Pact", "BRVP")),
    );

    let hall = Hall::new(DAY).with_every(0);
    let (ma, mb) = (mind(0.5), mind(0.5));
    let mut lines = Vec::new();
    for k in 0..40 {
        {
            // What the facts left behind: each corp rates the other alliance
            // above the blue threshold.
            let mut st = hall.lock();
            st.relations.r.insert((ca, a2), society::ALLY_AT + 3.0);
            st.relations.r.insert((cb, a1), society::ALLY_AT + 3.0);
        }
        lines.extend(turn(&mut alice, &hall, &ma, ALICE, k));
        lines.extend(turn(&mut bob, &hall, &mb, BOB, k));
    }
    let book_a = alliance_book(&mut alice, a1);
    let book_b = alliance_book(&mut bob, a2);
    assert_eq!(
        book_a.get(&a2),
        Some(&society::BLUE_STANDING),
        "{book_a:?}\n{lines:#?}"
    );
    assert_eq!(
        book_b.get(&a1),
        Some(&society::BLUE_STANDING),
        "{book_b:?}\n{lines:#?}"
    );
    // The client's own reading of the member of the other alliance.
    let empty = BTreeMap::new();
    let seen = standing_flag(&relationship(
        Books {
            personal: &empty,
            corp: &empty,
            alliance: &book_a,
        },
        BOB,
        cb,
        Some(a2),
    ));
    assert_eq!(seen, StandingFlag::High);
    // One coalition of the two.
    let blocs = hall.blocs(NOW + 39 * DAY / 4);
    assert_eq!(blocs.len(), 1, "{blocs:?}\n{lines:#?}");
    assert!(blocs[0].contains(a1) && blocs[0].contains(a2));
}

// ---- wars ----------------------------------------------------------------------------

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n\
         type\t{WRECK}\t186\t990\t1\t1\t0.0\t1000.0\t10000.0\t14.0\t0\t\t\tFixture Wreck\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let universe = eve_server::celestial::Universe::parse(
        &[format!(
            "{B}\t19000001\t29000001\t\t\t0.9\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
        )],
        &[],
        &[],
    )
    .expect("the invented universe row parses");
    content.insert("universe", universe);
    content.insert("wrecks", eve_server::Wrecks::from_pairs([(FRIGATE, WRECK)]));
    Arc::new(content)
}

fn catalogue() -> eve_server::celestial::Catalogue {
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{A}\t20000001\t10000001\n\
         #system\t{B}\t20000001\t10000001\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t3000000000.0\t0.0\t0.0\t{A}\t{GATE_B}\t{B}\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-3000000000.0\t{B}\t{GATE_A}\t{A}\n"
    ))
}

fn ids_of(c: &mut Conn, type_id: i64) -> Vec<i64> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.item_id)
        .collect()
}

fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = *ids_of(c, FRIGATE)
        .last()
        .expect("the frigate is in the hangar");
    c.board(hull).expect("boards the frigate");
    hull
}

fn arm(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {AUTOCANNON}"));
    gm(c, &format!("/createitem {ROUND} 10"));
    let station = c.session().station_id().expect("docked");
    let spare = ids_of(c, AUTOCANNON);
    let gun = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !c.is_online(gun.item_id).expect("asks") {
        c.set_online(gun.item_id).expect("onlines");
    }
    let rounds = ids_of(c, ROUND);
    c.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    gun.item_id
}

fn jump_to_b(c: &mut Conn) {
    c.undock().expect("undocks");
    c.enter_space(A).expect("A's state");
    let ship = c.session().ship_id().expect("a ship");
    let outcome = c
        .bound_call(
            "CmdStargateJump",
            vec![
                PyValue::Int(GATE_A),
                PyValue::Int(GATE_B),
                PyValue::Int(ship),
            ],
        )
        .expect("the jump is answered");
    assert_eq!(outcome.refusal(), None, "the gate lets us through");
    let arrived = until(Duration::from_secs(15), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        c.session().solar_system_id() == Some(B)
    });
    assert!(arrived, "the jump's session change into B");
    c.bind_ballpark(B).expect("binds the new park");
}

/// The war ids `c`'s owner is in, with the declaring owner and the target.
fn wars_of(c: &mut Conn, owner: i64) -> Vec<(i64, i64, i64)> {
    let bind = PyValue::Tuple(vec![PyValue::Int(owner), PyValue::Int(1)]);
    let a = c
        .moniker_call(
            "warRegistry",
            bind,
            "GetWars",
            vec![PyValue::Int(owner)],
            vec![],
        )
        .expect("GetWars");
    let v = a.value.expect("a dict");
    v.dict_as_kv()
        .expect("GetWars answers a dict")
        .into_iter()
        .map(|(k, w)| {
            let PyValue::Int(id) = k else {
                panic!("war id")
            };
            let PyValue::Int(by) = attr(w, "declaredByID") else {
                panic!("by")
            };
            let PyValue::Int(against) = attr(w, "againstID") else {
                panic!("against")
            };
            (*id, by, against)
        })
        .collect()
}

/// **A bot alliance declares war on the group its relation marks an enemy,
/// and a member shoots a war target in high sec with no CONCORD.** Alice's
/// executor turn declares on Bob's corp (the fee out of the Master Wallet);
/// before the war is hostile the shot is refused at green safety; after it
/// the same shot goes, Bob's ship dies, Alice is no criminal and CONCORD
/// says nothing.
#[test]
fn a_bot_alliance_declares_on_a_hostile_group_and_its_member_shoots_in_high_sec_without_concord() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_951),
        pilot(BOB, "Bob", 100_000_952),
    ];
    cfg.sandbox.celestials = catalogue();
    let content = content();
    cfg.content = Some(content.clone());
    let addr = serve(cfg);
    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    let mut bob = login(addr, "Bob");
    let (ca, a1) = found(
        &mut alice,
        "Alpha Invented",
        "ALPH",
        Some(("Alpha Pact", "ALPP")),
    );
    let (cb, _) = found(&mut bob, "Bravo Invented", "BRAV", None);

    // The hall: wars on; Alice's corp rates Bob's corp as an enemy.
    let hall = Hall::new(DAY).with_every(0).with_wars(true, true);
    let ma = mind(1.0);
    hall.lock().relations.add(ca, cb, coalition::WAR_AT - 2.0);
    let mut lines = Vec::new();
    let mut declared = Vec::new();
    for k in 0..8 {
        lines.extend(turn(&mut alice, &hall, &ma, ALICE, k));
        declared = wars_of(&mut alice, a1);
        if !declared.is_empty() {
            break;
        }
    }
    assert_eq!(
        declared
            .iter()
            .map(|(_, by, ag)| (*by, *ag))
            .collect::<Vec<_>>(),
        vec![(a1, cb)],
        "the alliance declared on Bob's corp\n{lines:#?}"
    );
    let war_id = declared[0].0;
    assert!(
        lines.iter().any(|l| l.contains("declares on")),
        "{lines:#?}"
    );
    // Not again: one war a target.
    let again = turn(&mut alice, &hall, &ma, ALICE, 9);
    assert!(
        again.iter().all(|l| !l.contains("declares on")),
        "{again:#?}"
    );

    // Armed and in B while the war spools up: the shot is refused at green.
    board_frigate(&mut bob);
    let bob_ship = bob.session().ship_id().expect("Bob's ship");
    board_frigate(&mut alice);
    let gun = arm(&mut alice);
    jump_to_b(&mut bob);
    jump_to_b(&mut alice);
    alice.lock_target(bob_ship).expect("locks Bob's ship");
    let refused = alice
        .fire(gun, AUTOCANNON, Some(&content), bob_ship)
        .expect_err("green safety refuses an attack on a stranger");
    assert!(
        refused
            .to_string()
            .contains("ModuleActivationDeniedSafetyPreventsCriminal"),
        "{refused}"
    );
    let _ = alice.take_pushes();

    // The war turns hostile (the client's own GM menu item).
    let bind = PyValue::Tuple(vec![PyValue::Int(a1), PyValue::Int(1)]);
    let past = eve_server::pyutil::filetime_now() - 2 * eve_server::war::WAR_SPOOLUP;
    let r = alice
        .moniker_call(
            "warRegistry",
            bind,
            "GMSetDeclareTime",
            vec![PyValue::Int(war_id), PyValue::Int(past)],
            vec![],
        )
        .expect("GMSetDeclareTime");
    assert_eq!(r.refusal(), None);
    alice
        .fire(gun, AUTOCANNON, Some(&content), bob_ship)
        .expect("a war target may be shot at green");
    let dead = until(Duration::from_secs(30), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        bob.session().ship_id() != Some(bob_ship)
    });
    assert!(dead, "Bob's ship dies");
    // Nobody is a criminal, and CONCORD never speaks.
    let mut heard = Vec::new();
    let _ = until(Duration::from_secs(3), || {
        let _ = alice.collect_pushes(Duration::from_millis(100));
        heard.extend(alice.take_pushes());
        false
    });
    assert!(
        !heard.iter().any(|(e, a)| e == "OnCriminalTimerUpdate"
            && a.first() == Some(&PyValue::Int(TIMER_CRIMINAL))),
        "the war made the shot legal: {:?}",
        heard.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    assert!(
        !heard.iter().any(|(e, a)| e == "OnRemoteMessage"
            && a.first()
                == Some(&PyValue::Bytes(
                    eve_server::concord::WARNING_KEY.as_bytes().to_vec()
                ))),
        "CONCORD has nothing to say about a war"
    );
    assert_ne!(
        alice.session().ship_id(),
        Some(100_000_951),
        "Alice still flies her frigate"
    );
}

// ---- the hunter ----------------------------------------------------------------------

const CORP: i64 = 98_000_961;
const BOB_CORP: i64 = 98_000_962;
const CAROL_CORP: i64 = 98_000_963;
const HUNTER_SHIP: i64 = 100_000_954;
const BOB_SHIP: i64 = 100_000_952;
const CAROL_SHIP: i64 = 100_000_953;
const RIFTER: i64 = 587;
const HIGH_SEC: f64 = 0.9;

static TABLE: &[Career] = &[Career {
    name: "pirate",
    role: Some(&hunt::ROLE),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::Low,
    prior_isk_h: INCOME_REF,
}];

fn member(name: &str, corp: i64) -> Member {
    Member {
        name: name.to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "pirate".to_owned(),
        band: Band::Low,
        corp,
        alliance: None,
        last_turn: Some(driver::unix_now()),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

/// A hall of three bot corps, and a war the hunter's corp is in with Bob's.
fn war_hall() -> Arc<Hall> {
    let hall = Hall::new(60).with_every(1 << 40);
    {
        let mut st = hall.lock();
        for (corp, ceo, name) in [
            (CORP, HUNTER, "Hunter1"),
            (BOB_CORP, BOB, "Bob"),
            (CAROL_CORP, CAROL, "Carol"),
        ] {
            st.members.insert(ceo, member(name, corp));
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
        st.note_war(WarRow {
            id: 1,
            by: CORP,
            against: BOB_CORP,
            started: eve_server::pyutil::filetime_now() - 1_000,
            finished: None,
            mutual: false,
            open: false,
            allies: vec![],
            seen: 0,
        });
    }
    Arc::new(hall)
}

/// **A hunter in a high-sec ground fires on a war target and leaves a
/// bystander alone.** Nobody is prey in high sec but the pilot whose side is
/// at war with the hunter's.
#[test]
fn a_hunter_in_high_sec_fires_on_a_war_target_and_leaves_a_bystander() {
    let mut cfg = config();
    let rifter = |c: Character, corp| Character {
        corporation_id: corp,
        ship_type_id: RIFTER,
        ..c
    };
    cfg.sandbox.characters = vec![
        Character {
            corporation_id: CORP,
            ..pilot(HUNTER, "Hunter1", HUNTER_SHIP)
        },
        rifter(pilot(BOB, "Bob", BOB_SHIP), BOB_CORP),
        rifter(pilot(CAROL, "Carol", CAROL_SHIP), CAROL_CORP),
    ];
    let addr: SocketAddr = serve(cfg);
    let in_space = |name: &str| {
        let mut c = login(addr, name);
        c.undock().expect("undocks");
        c.enter_space(A).expect("the state");
        c
    };
    let mut bob = in_space("Bob");
    let mut carol = in_space("Carol");
    let hall = war_hall();

    let now = driver::unix_now();
    let mut m = Mind::seeded("Hunter1", 1, TABLE, &Signals::default(), now);
    m.career = "pirate".to_owned();
    m.since = now;
    m.traits.0 = [0.5; TRAIT_COUNT];
    m.traits.0[Trait::Aggression as usize] = 1.0;
    let mut soc = Society::new(m, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(&hall));
    let mut bot_config = BotConfig::new("Hunter1", addr, Pace::Fast).with_society(soc);
    bot_config.chat = false;
    bot_config.policy.pvp = Some(Arc::new(
        Ground::new()
            .with_security(A, HIGH_SEC)
            .with_class(RIFTER, Class::Small),
    ));
    let bot = driver::spawn(bot_config).0;

    let fired = until(Duration::from_secs(120), || {
        let _ = bob.collect_pushes(Duration::from_millis(25));
        let _ = carol.collect_pushes(Duration::from_millis(25));
        bot.log()
            .iter()
            .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&BOB_SHIP))
    });
    let log = bot.log();
    bot.stop();
    assert!(
        fired,
        "the hunter's guns go on the war target:\n{}",
        dump(&bot)
    );
    let touched: Vec<_> = log
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ActKind::Lock | ActKind::Orbit | ActKind::Fire | ActKind::Follow
            ) && e.ids.contains(&CAROL_SHIP)
        })
        .collect();
    assert!(touched.is_empty(), "the bystander was spared: {touched:?}");
}
