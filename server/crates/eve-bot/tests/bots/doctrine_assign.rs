//! DOC-5 over a real socket: **a member reships into the corp's doctrine fit
//! after a loss.** Bob sits in a capsule; the hall's doctrine for his corp
//! names a fit (the fixture frigate, one autocannon, an invented round) that
//! the hull table `HULLS` does not list. He reports he holds no hull, the hall
//! plans his role, the chooser buys the doctrine hull from Alice's ask, the
//! rest of the order follows, he boards and fits it in one `FitAuto`, and
//! reports `Staged`.
//!
//! **Invented rows and ids only** (`typedata::FIXTURE`, 990_xxx_xxx, the
//! corp id below).

use super::common::{config, gm, login, pilot, serve};
use eve_bot::acquire::{self, Lacking, Rounds};
use eve_bot::builder::SellLine;
use eve_bot::corpmind::{CorpMind, Hall, Member};
use eve_bot::doctrine::{
    Doctrine, Family, FleetSize, Layer, Profile, Range, Role, RoleFit, Stance,
};
use eve_bot::doctrine_hall::DoctrineWorld;
use eve_bot::doctrine_lead::{DoctrineBook, Fancy};
use eve_bot::doctrine_theatre::Ship;
use eve_bot::hull_choice::{self, Class, Need, Weapon};
use eve_bot::{Conn, doctrine_assign, fit_plan};
use eve_server::damage::{Damage, Profile as Tank};
use std::net::SocketAddr;
use std::sync::Arc;

const ALICE: i64 = 90_000_301;
const BOB: i64 = 90_000_302;
/// An invented bot corp (its CEO is Bob).
const CORP: i64 = 98_000_311;
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// An invented round for the fixture autocannon (group 83, size 1).
const ROUND: i64 = 990_980_011;
const WALLET: f64 = 5_000_000.0;
const ROUNDS: u32 = 10;
const KEY: &str = "invented/doctrine";
const DAY: u64 = 1_000;
const NOW: u64 = 10 * DAY;

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
        rich(ALICE, "Alice", 100_000_301),
        rich(BOB, "Bob", 100_000_302),
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

/// A hall whose only corp, Bob's, flies the fit in `KEY`.
fn hall() -> Hall {
    let doctrine = Doctrine {
        key: KEY.to_owned(),
        class: Class::Frigate,
        family: Family::Projectile,
        layer: Layer::Armour,
        active: false,
        range: Range::Short,
        stance: Stance::Brawl,
        roles: vec![(Role::Dps, 1.0, role_fit())],
        fleet: FleetSize {
            min: 1,
            target: 5,
            max_on_grid: 30,
        },
    };
    let world = DoctrineWorld::new(vec![doctrine], Default::default(), Default::default());
    let hall = Hall::new(DAY)
        .with_every(0)
        .with_doctrine(Arc::new(world), 2, 7);
    {
        let mut st = hall.lock();
        st.members.insert(
            BOB,
            Member {
                name: "Bob".to_owned(),
                traits: eve_bot::mind::Traits([0.5; eve_bot::mind::TRAIT_COUNT]),
                career: "ratter".to_owned(),
                band: eve_bot::career::Band::High,
                corp: CORP,
                alliance: None,
                last_turn: Some(NOW),
                satisfaction: Default::default(),
                applied: None,
                salt: 0,
            },
        );
        st.corps.insert(
            CORP,
            CorpMind {
                id: CORP,
                ceo: Some(BOB),
                profile: eve_bot::society::Profile::of("ratter", eve_bot::career::Band::High),
                founded: 0,
                alliance: None,
                name: "Invented Corp".to_owned(),
                applied_alliance: None,
                salt: 0,
            },
        );
        let mut book = DoctrineBook::new(Fancy::draw("Bob", 7));
        book.current = Some(KEY.to_owned());
        st.dbooks.insert(CORP, book);
    }
    hall
}

/// **A member reships into the corp's doctrine fit after a loss.**
#[test]
fn reship_into_the_doctrine_socket() {
    let (addr, _content) = start();
    {
        let mut alice = login(addr, "Alice");
        ask(&mut alice, FRIGATE, 1, 1_000.0);
        ask(&mut alice, AUTOCANNON, 1, 100.0);
        ask(&mut alice, ROUND, i64::from(ROUNDS), 1.0);
    }
    let hall = hall();
    let mut bob = login(addr, "Bob");

    // Lost his ship: a capsule, no doctrine hull. He reports it.
    doctrine_assign::report(&mut bob, &hall, BOB, NOW).expect("reports");
    assert_eq!(
        hall.snapshot().readiness[&BOB].ship,
        Some(Ship::Market),
        "no hull in hand"
    );

    // The hall plans his role: the doctrine's fighter fit.
    let plan = hall
        .plan_for(BOB)
        .expect("a fighter in a led corp has a plan");
    assert_eq!(plan.key, KEY);
    assert_eq!(plan.role, Role::Dps);
    assert_eq!(plan.fit.hull, FRIGATE);

    // The chooser (no `HULLS` row for this hull) buys the doctrine hull.
    let need = Need {
        weapon: Weapon::Projectile,
        drones: false,
        wallet: WALLET,
        risk: 0.5,
        bias: [0.0; hull_choice::CLASSES],
        taste: 3,
        avoid: &[],
        untrained: &[],
        strict: false,
        doctrine: Some(&plan.fit),
    };
    let got = hull_choice::obtain(&mut bob, &need, None, true).expect("obtains");
    assert_eq!(got.hull, Some(FRIGATE), "{}", got.note);

    // The rest of the order: the fit's gun and rounds (the hull is held).
    let order = fit_plan::fight_order(
        acquire::FIGHTER_KIT,
        None,
        Weapon::Projectile,
        Some(&plan.fit),
    );
    let done = acquire::run_order(
        &mut bob,
        &order,
        Lacking {
            ship: false,
            module: true,
            ammo: true,
            in_capsule: false,
        },
        None,
        &Rounds::default(),
    )
    .expect("the order is bought");
    assert!(done.missing.is_empty(), "{:?}", done.missing);

    // Board the doctrine hull and fit what the fit names.
    let hull = bob
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE && r.category_id == acquire::CATEGORY_SHIP)
        .expect("the hull is in the hangar");
    bob.board(hull.item_id).expect("boards");
    let note = doctrine_assign::outfit(&mut bob, &plan.fit).expect("fits");
    assert!(note.contains("1 modules fitted"), "{note}");
    assert!(
        bob.fitted()
            .expect("fitted")
            .iter()
            .any(|r| r.type_id == AUTOCANNON),
        "the doctrine's gun is on the hull"
    );
    assert_eq!(
        doctrine_assign::outfit(&mut bob, &plan.fit).expect("again"),
        "",
        "nothing left to fit"
    );

    // Back in a ship of the doctrine: he reports Staged.
    doctrine_assign::report(&mut bob, &hall, BOB, NOW + 60).expect("reports");
    assert_eq!(hall.snapshot().readiness[&BOB].ship, Some(Ship::Staged));
}
