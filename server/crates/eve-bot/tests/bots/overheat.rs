//! K-OVERHEAT over a real socket: a module is overloaded and stopped, its rack
//! heats while it cycles and the pilot is told the way the client's heat
//! gauge needs; two guns are linked into a bank that runs as one.
//!
//! Alice flies the fixture frigate with two invented guns (the client's own
//! overload category 5 and slot effect 12; invented ids and numbers
//! otherwise). The same limitation as every test in this crate: both ends
//! marshal with `eve-marshal`, so this proves behaviour, not bytes -- the
//! client's reading of the shapes is `tools/client-ci/scenarios/
//! fci_k_overheat.py`.
//!
//! **Invented rows only.** No `[A]` value is an expected value: the heat
//! rate is this file's own fixture number and the tests wait for the
//! pushes, not for a number the server chose.

use super::common::{SYSTEM, config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::server::Server;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_701;
/// `typedata::FIXTURE`'s frigate.
const FRIGATE: i64 = 990_000_001;
/// Invented: the gun type, its two effects.
const GUN: i64 = 990_701_001;
const FIRE: (&str, i64) = ("fixtureFire", 990_701_011);
const OVERLOAD: i64 = 990_701_012;
/// `groupProjectileWeapon` [V]: a groupable group.
const WEAPON_GROUP: i64 = 55;
/// Invented: a propulsion module whose overload effect is the client's own
/// `overloadSelfSpeedBonus` [V-data]; it sits in the medium rack.
const PROP: i64 = 990_701_002;
const PROP_OVERLOAD: i64 = 3175;
/// `attributeHeatHi` [V].
const HEAT_HI: i64 = 1175;
/// Heat per second one module adds to its rack (a fixture number, large so
/// the rack warms inside the test's wait).
const HEAT_RATE: f64 = 5.0;

fn content() -> Arc<Content> {
    let rows = format!(
        "{}\
         type\t{GUN}\t{WEAPON_GROUP}\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t{}\t12,16,{},{OVERLOAD}\t9=40.0;73=2000.0;1180={HEAT_RATE};1211=0.5;1212=0.0\tFixture Hot Gun\n\
         effect\t{}\t1\t0\t73\t0\t0\t0\t0\t{}\teffects.Fixture\n\
         effect\t{OVERLOAD}\t5\t0\t0\t0\t0\t0\t0\tfixtureOverload\t\n\
         type\t{PROP}\t46\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t{}\t13,16,{},{PROP_OVERLOAD}\t9=40.0;73=2000.0;1180={HEAT_RATE};1211=0.5;1212=0.0\tFixture Prop\n\
         effect\t{PROP_OVERLOAD}\t5\t0\t0\t0\t0\t0\t0\toverloadSelfSpeedBonus\t\n",
        eve_server::typedata::FIXTURE,
        FIRE.1,
        FIRE.1,
        FIRE.1,
        FIRE.0,
        FIRE.1,
        FIRE.1,
    );
    let mut content = Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

type Seen = Vec<(String, Vec<PyValue>)>;

fn until_seen(c: &mut Conn, done: impl Fn(&Seen) -> bool) -> Seen {
    let mut seen: Seen = Vec::new();
    until(Duration::from_secs(20), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        seen.extend(c.take_pushes());
        done(&seen)
    });
    seen
}

/// Alice undocked in the frigate with two guns fitted and online; the
/// guns' ids and the world.
fn flying() -> (
    Conn,
    i64,
    Vec<i64>,
    Arc<RwLock<eve_server::service::Registry>>,
) {
    let (c, ship, guns, _prop, world) = flying_with_prop();
    (c, ship, guns, world)
}

/// [`flying`], and the propulsion module fitted to the medium rack.
fn flying_with_prop() -> (
    Conn,
    i64,
    Vec<i64>,
    i64,
    Arc<RwLock<eve_server::service::Registry>>,
) {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_701)];
    config.content = Some(content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    gm(&mut alice, &format!("/createitem {FRIGATE}"));
    let hull = alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == FRIGATE)
        .map(|r| r.item_id)
        .next_back()
        .expect("the frigate");
    alice.board(hull).expect("boards");
    gm(&mut alice, &format!("/createitem {GUN} 2"));
    let spare: Vec<i64> = alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == GUN)
        .map(|r| r.item_id)
        .collect();
    // One fitting takes one module off a stack: twice for two.
    let mut guns: Vec<i64> = Vec::new();
    for _ in 0..2 {
        guns.extend(
            alice
                .fit_auto(&spare)
                .expect("fits")
                .into_iter()
                .filter(|r| r.type_id == GUN)
                .map(|r| r.item_id),
        );
    }
    guns.sort_unstable();
    assert_eq!(guns.len(), 2, "both guns are fitted");
    gm(&mut alice, &format!("/createitem {PROP}"));
    let spare: Vec<i64> = alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == PROP)
        .map(|r| r.item_id)
        .collect();
    let prop = alice
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == PROP)
        .expect("the propulsion module is fitted")
        .item_id;
    for g in guns.iter().chain(std::iter::once(&prop)) {
        if !alice.is_online(*g).expect("asks") {
            alice.set_online(*g).expect("onlines");
        }
    }
    alice.undock().expect("undocks");
    alice.enter_space(SYSTEM).expect("the park");
    let ship = alice.session().ship_id().expect("a ship");
    let _ = alice.take_pushes();
    (alice, ship, guns, prop, world)
}

/// The ids of a list answer, ascending (the rack answers in slot order).
fn ids(v: &Option<PyValue>) -> Vec<i64> {
    let mut v = match v {
        Some(PyValue::List(l)) => l
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect::<Vec<i64>>(),
        other => panic!("a list of ids, not {other:?}"),
    };
    v.sort_unstable();
    v
}

/// `(module, effect, start)` of every `OnGodmaShipEffect` in `seen`.
fn effects(seen: &Seen) -> Vec<(i64, i64, bool)> {
    seen.iter()
        .filter(|(n, _)| n == "OnGodmaShipEffect")
        .filter_map(|(_, a)| match (a.first(), a.get(1), a.get(3)) {
            (Some(PyValue::Int(m)), Some(PyValue::Int(e)), Some(PyValue::Int(s))) => {
                Some((*m, *e, *s == 1))
            }
            _ => None,
        })
        .collect()
}

fn sources(seen: &Seen, name: &str) -> Vec<(i64, i64)> {
    seen.iter()
        .filter(|(n, _)| n == name)
        .filter_map(|(_, a)| match (a.first(), a.get(1)) {
            (Some(PyValue::Int(h)), Some(PyValue::Int(m))) => Some((*h, *m)),
            _ => None,
        })
        .collect()
}

/// Every `(item, attribute, new)` of every `OnModuleAttributeChanges`.
fn changes(seen: &Seen) -> Vec<(i64, i64, f64)> {
    let mut out = Vec::new();
    for (n, a) in seen {
        if n != "OnModuleAttributeChanges" {
            continue;
        }
        let Some(PyValue::List(list)) = a.first() else {
            continue;
        };
        for c in list {
            if let PyValue::Tuple(t) = c
                && let (Some(PyValue::Int(i)), Some(PyValue::Int(at)), Some(PyValue::Real(v))) =
                    (t.get(2), t.get(3), t.get(5))
            {
                out.push((*i, *at, *v));
            }
        }
    }
    out
}

/// **Overload on and off, and the rack's heat**, as the client's HUD needs
/// them: the overload effect lit and put out with `OnGodmaShipEffect`, the
/// heat gauge's value and `OnHeatAdded` / `OnHeatRemoved` while the module
/// cycles, and the rack calls answering exactly the modules they change.
#[test]
fn a_module_is_overloaded_heats_its_rack_and_is_stopped() {
    let (mut alice, ship, guns, _world) = flying();
    let (g1, g2) = (guns[0], guns[1]);

    // On: the effect lights, and the module's own row says so.
    alice
        .overload_module(g1, OVERLOAD, true)
        .expect("overloads");
    let seen = until_seen(&mut alice, |s| !effects(s).is_empty());
    assert_eq!(effects(&seen), [(g1, OVERLOAD, true)], "{seen:?}");
    assert!(alice.active_effects(g1).expect("a row").contains(&OVERLOAD));

    // Cycling: the rack's gauge value and the source the client adds.
    alice
        .activate(g1, FIRE, None, 1000)
        .expect("the gun runs hot");
    let seen = until_seen(&mut alice, |s| {
        !sources(s, "OnHeatAdded").is_empty()
            && changes(s)
                .iter()
                .any(|(i, a, v)| *i == ship && *a == HEAT_HI && *v > 0.0)
    });
    assert_eq!(sources(&seen, "OnHeatAdded"), [(HEAT_HI, g1)], "{seen:?}");
    let warm = changes(&seen)
        .into_iter()
        .filter(|(i, a, _)| *i == ship && *a == HEAT_HI)
        .map(|(_, _, v)| v)
        .fold(0.0, f64::max);
    assert!(warm > 0.0, "the high rack warmed: {seen:?}");

    // Off: the effect goes out and the source is taken off.
    alice.overload_module(g1, OVERLOAD, false).expect("stops");
    let seen = until_seen(&mut alice, |s| {
        !effects(s).is_empty() && !sources(s, "OnHeatRemoved").is_empty()
    });
    assert_eq!(effects(&seen), [(g1, OVERLOAD, false)], "{seen:?}");
    assert_eq!(sources(&seen, "OnHeatRemoved"), [(HEAT_HI, g1)]);
    assert!(!alice.active_effects(g1).expect("a row").contains(&OVERLOAD));

    // The rack calls answer, and tell, the modules they change.
    let on = alice
        .dogma_im("OverloadRack", vec![PyValue::Int(g2)])
        .expect("answers");
    assert_eq!(ids(&on.value), [g1, g2]);
    let seen = until_seen(&mut alice, |s| effects(s).len() >= 2);
    let mut told: Vec<_> = effects(&seen).into_iter().filter(|e| e.2).collect();
    told.sort_unstable();
    assert_eq!(
        told,
        [(g1, OVERLOAD, true), (g2, OVERLOAD, true)],
        "{seen:?}"
    );
    let off = alice
        .dogma_im("StopOverloadRack", vec![PyValue::Int(g1)])
        .expect("answers");
    assert_eq!(ids(&off.value), [g1, g2]);
    let again = alice
        .dogma_im("StopOverloadRack", vec![PyValue::Int(g1)])
        .expect("answers");
    assert_eq!(
        ids(&again.value),
        Vec::<i64>::new(),
        "nothing left to flash"
    );
}

/// **A bank**: two guns linked into one, the dict answered in the shape
/// `SetWeaponBanks` reads, the master's click running both, and the
/// peeled-off slave named by the unlink.
#[test]
fn two_guns_are_linked_into_a_bank_that_runs_as_one() {
    let (mut alice, ship, guns, world) = flying();
    let (g1, g2) = (guns[0], guns[1]);

    let linked = alice
        .dogma_im(
            "LinkWeapons",
            vec![PyValue::Int(ship), PyValue::Int(g1), PyValue::Int(g2)],
        )
        .expect("answers");
    assert_eq!(linked.outcome.refusal(), None, "{linked:?}");
    // One entry: master -> [slave].
    let Some(PyValue::Dict(entries)) = &linked.value else {
        panic!("a dict, not {:?}", linked.value)
    };
    assert_eq!(entries.len(), 1);
    let flat = format!("{entries:?}");
    assert!(
        flat.contains(&format!("Int({g1})")) && flat.contains(&format!("Int({g2})")),
        "{flat}"
    );
    assert_eq!(
        world
            .read()
            .expect("the world")
            .ext::<eve_server::weapon_bank::WeaponBanks>()
            .of(ship)
            .into_iter()
            .collect::<Vec<_>>(),
        [(g1, vec![g2])]
    );

    // The master's click runs both guns; only the master is told (one button,
    // one icon under the target).
    let _ = alice.take_pushes();
    alice.activate(g1, FIRE, None, 1000).expect("the bank runs");
    let seen = until_seen(&mut alice, |s| {
        effects(s).iter().filter(|e| e.1 == FIRE.1 && e.2).count() >= 1
    });
    let mut started: Vec<i64> = effects(&seen)
        .into_iter()
        .filter(|e| e.1 == FIRE.1 && e.2)
        .map(|e| e.0)
        .collect();
    started.sort_unstable();
    assert_eq!(started, [g1], "{seen:?}");
    {
        let w = world.read().expect("the world");
        assert!(w.modules().running(g1).is_some() && w.modules().running(g2).is_some());
    }

    // A linked gun cannot be linked again while it runs.
    let refused = alice
        .dogma_im(
            "LinkWeapons",
            vec![PyValue::Int(ship), PyValue::Int(g1), PyValue::Int(g2)],
        )
        .expect("answers");
    assert_eq!(
        refused.outcome.refusal().as_deref(),
        Some("CantLinkModuleActive")
    );

    // The master's stop stops both (at their cycles' ends).
    alice
        .deactivate(g1, FIRE.0)
        .expect("the bank is told to stop");
    assert!(until(Duration::from_secs(15), || {
        let w = world.read().expect("the world");
        w.modules().running(g1).is_none() && w.modules().running(g2).is_none()
    }));

    // Unlinking names the slave the client takes out of its copy.
    let peeled = alice
        .dogma_im("UnlinkModule", vec![PyValue::Int(ship), PyValue::Int(g1)])
        .expect("answers");
    assert_eq!(peeled.value, Some(PyValue::Int(g2)));
    assert!(
        world
            .read()
            .expect("the world")
            .ext::<eve_server::weapon_bank::WeaponBanks>()
            .of(ship)
            .is_empty()
    );
}

/// **A bot's fight**: the first shot overloads the propulsion module the
/// client's data lets overload, and not the guns; the cease-fire stops it.
#[test]
fn a_bots_engage_overloads_the_propulsion_module_and_not_the_guns() {
    let (mut alice, _ship, guns, prop, _world) = flying_with_prop();
    let content = content();
    let on = eve_bot::overload::engage(&mut alice, Some(&content)).expect("engages");
    assert_eq!(on, [prop], "the propulsion module only");
    assert!(
        alice
            .active_effects(prop)
            .expect("a row")
            .contains(&PROP_OVERLOAD)
    );
    for g in &guns {
        assert!(!alice.active_effects(*g).expect("a row").contains(&OVERLOAD));
    }
    // Idempotent: a second shot asks for nothing more.
    assert!(
        eve_bot::overload::engage(&mut alice, Some(&content))
            .expect("again")
            .is_empty()
    );
    let off = eve_bot::overload::disengage(&mut alice, Some(&content)).expect("disengages");
    assert_eq!(off, [prop]);
    assert!(
        !alice
            .active_effects(prop)
            .expect("a row")
            .contains(&PROP_OVERLOAD)
    );
    // No content, no data: nothing is run hot.
    assert!(
        eve_bot::overload::engage(&mut alice, None)
            .expect("quiet")
            .is_empty()
    );
}
