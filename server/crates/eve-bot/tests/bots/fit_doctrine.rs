//! DOC-2 over a real socket: a pilot buys the parts of a doctrine fit from
//! asks (`acquire::run_order`), fits them in one `FitAuto`
//! (`fit_plan::fit_ids`), loads the round and fires the gun by the type's
//! default effect at another pilot's ship.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s frigate and autocannon and
//! an invented round. Alice sells, Bob buys, Carol is the target; they are
//! plain connections and nothing here is an `[A]` value.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::acquire::{self, Lacking, Order, Rounds};
use eve_bot::builder::SellLine;
use eve_bot::doctrine::{Profile, RoleFit};
use eve_bot::{Conn, fit_plan};
use eve_server::damage::{Damage, Profile as Tank};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_201;
const BOB: i64 = 90_000_202;
const CAROL: i64 = 90_000_203;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// An invented round for the fixture autocannon (group 83, size 1).
const ROUND: i64 = 990_980_001;
const WALLET: f64 = 5_000_000.0;
const ROUNDS: u32 = 10;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=5.0;128=1.0\tFixture Doctrine Round\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    let rich = |id, name: &str, ship| {
        let mut p = pilot(id, name, ship);
        p.balance = WALLET;
        p
    };
    config.sandbox.characters = vec![
        rich(ALICE, "Alice", 100_000_201),
        rich(BOB, "Bob", 100_000_202),
        rich(CAROL, "Carol", 100_000_203),
    ];
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

/// The doctrine fit on the fixture's frigate: one autocannon, its round.
fn role_fit() -> RoleFit {
    let l = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };
    RoleFit {
        hull: FRIGATE,
        modules: vec![(AUTOCANNON, 1)],
        charges: vec![(ROUND, ROUNDS)],
        skills: vec![],
        profile: Profile::blank(Tank {
            shield_capacity: 1.0,
            shield_recharge_rate: 0.0,
            armor_hp: 1.0,
            hp: 1.0,
            shield_resonance: l,
            armor_resonance: l,
            hull_resonance: l,
        }),
        cost: 1.0,
    }
}

/// Alice lists `quantity` of `type_id` at the home station at `price`, a
/// hull repackaged first as the market wants.
fn ask(c: &mut Conn, type_id: i64, quantity: i64, price: f64) {
    gm(c, &format!("/createitem {type_id} {quantity}"));
    let station = c.session().station_id().expect("docked");
    let hulls: Vec<eve_marshal::PyValue> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == type_id && r.category_id == acquire::CATEGORY_SHIP)
        .map(|r| {
            eve_marshal::PyValue::Tuple(vec![
                eve_marshal::PyValue::Int(r.item_id),
                eve_marshal::PyValue::Int(station),
            ])
        })
        .collect();
    if !hulls.is_empty() {
        let batch = eve_marshal::PyValue::Dict(vec![(
            eve_marshal::PyValue::List(hulls),
            eve_marshal::PyValue::Int(station),
        )]);
        let done = c
            .call("repackagingSvc", "RepackageItems", vec![batch])
            .expect("answered");
        assert_eq!(done.refusal(), None, "repackaged");
    }
    let lines: Vec<SellLine> = c
        .hangar_items()
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| SellLine {
            station_id: station,
            type_id,
            item_id: r.item_id,
            price,
            quantity: r.quantity,
        })
        .collect();
    c.sell(
        &lines,
        eve_bot::builder::ORDER_DAYS,
        eve_bot::builder::BROKER_FEE,
    )
    .expect("Alice lists");
}

/// **A doctrine fit, bought, fitted and fired.** Bob's order is the role
/// fit's: the hull, the gun, the round. Alice's asks fill it; `FitAuto`
/// takes the gun the fit names; the round loads; Bob undocks and fires the
/// gun, by its type's default effect, at Carol's ship.
#[test]
fn fit_doctrine_socket() {
    let (addr, content) = start();
    {
        let mut alice = login(addr, "Alice");
        ask(&mut alice, FRIGATE, 1, 1_000.0);
        ask(&mut alice, AUTOCANNON, 1, 100.0);
        ask(&mut alice, ROUND, i64::from(ROUNDS), 1.0);
    }
    let mut carol = login(addr, "Carol");
    gm(&mut carol, &format!("/createitem {FRIGATE}"));
    let target = carol
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate")
        .item_id;
    carol.board(target).expect("Carol boards");
    carol.undock().expect("Carol undocks");
    carol.enter_space(SYSTEM).expect("Carol's state");

    let role = role_fit();
    let order = Order::from_fit(&role);
    let mut bob = login(addr, "Bob");
    let got = acquire::run_order(
        &mut bob,
        &order,
        Lacking {
            ship: true,
            module: true,
            ammo: true,
            in_capsule: false,
        },
        None,
        &Rounds::default(),
    )
    .expect("the order is bought");
    assert!(got.missing.is_empty(), "{:?}", got.missing);
    // Every part came once, and what Bob holds now covers the order.
    let hangar = bob.hangar_items().expect("lists");
    assert!(hangar.iter().any(|r| r.type_id == AUTOCANNON));
    let held = acquire::holdings(&mut bob, &order).expect("holdings");
    assert!(
        order
            .short(&|t| held.get(&t).copied().unwrap_or(0))
            .is_empty(),
        "{held:?}"
    );
    // Nothing is bought twice.
    let again = acquire::buy_order(&mut bob, &order, None).expect("a second pass");
    assert!(again.bought.is_empty(), "{:?}", again.bought);

    // The hull is the doctrine's: board it, fit what the fit names.
    let hull = hangar
        .iter()
        .find(|r| r.type_id == FRIGATE && r.category_id == acquire::CATEGORY_SHIP)
        .expect("the hull is in the hangar");
    bob.board(hull.item_id).expect("Bob boards");
    let ids = fit_plan::fit_ids(&role, &bob.hangar_items().expect("lists"), &[]);
    assert_eq!(ids.len(), 1, "one gun to fit");
    let gun = bob
        .fit_auto(&ids)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the gun is fitted");
    if !bob.is_online(gun.item_id).expect("asks") {
        bob.set_online(gun.item_id).expect("onlines");
    }
    // Fitted: nothing more to fit.
    assert!(
        fit_plan::fit_ids(
            &role,
            &bob.hangar_items().expect("lists"),
            &bob.fitted().expect("fitted")
        )
        .is_empty()
    );
    let station = bob.session().station_id().expect("docked");
    let rounds: Vec<i64> = bob
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == ROUND)
        .map(|r| r.item_id)
        .collect();
    bob.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");
    bob.lock_target(target).expect("locks");
    bob.fire(gun.item_id, AUTOCANNON, Some(&content), target)
        .expect("fires");
    // The shot lands: Carol's ship takes damage or dies.
    let hit = until(Duration::from_secs(60), || {
        let _ = carol.collect_pushes(Duration::from_millis(100));
        carol.session().ship_id() != Some(target)
            || bob.active_effects(gun.item_id).is_ok_and(|e| !e.is_empty())
    });
    assert!(hit, "the gun runs");
}
