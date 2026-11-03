//! **DEV-MODE over real sockets**: roster bots against an `eve-server --dev`
//! world. A roster bot gets every skill at V from `/devskills`, buys its
//! roster hull from the dev seller (100 ISK, no GM item), and is seated in a
//! corporation by its side: one side is one corporation (allied), another
//! side is not (hostile to the bots' rule), and a group's boss forms the
//! fleet its group joins.
//!
//! Invented rows only: one station, one fixture hull, ids of this file's own.

use super::common::{config, dump, pilot, until};
use eve_bot::brain::ActKind;
use eve_bot::dev;
use eve_bot::driver::{self, BotConfig, Pace};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::group_contacts::Affiliation;
use eve_server::server::Server;
use std::sync::Arc;
use std::time::Duration;

const RED1: i64 = 90_000_901;
const RED2: i64 = 90_000_902;
const BLUE1: i64 = 90_000_903;
/// The fixture frigate (`typedata::FIXTURE`), by id and by name.
const FRIGATE: i64 = 990_000_001;
const FRIGATE_NAME: &str = "Fixture Frigate";
/// `Character::default()`'s station, system and region.
const STATION: i64 = 60_012_526;
const SYSTEM: i64 = 30_000_001;
const REGION: i64 = 10_000_001;

fn content() -> Arc<Content> {
    let row = exchange::TypeRow {
        group_id: 25,
        category_id: 6,
        base_price: 100_000.0,
        meta_group_id: None,
        tech_level: None,
    };
    let station = exchange::Station {
        station_id: STATION,
        system_id: SYSTEM,
        region_id: REGION,
        owner_id: 1_999_001,
    };
    // No trade list: no NPC sells the hull, only the dev seller does.
    let table = exchange::Table::new(
        vec![station],
        &[],
        std::collections::BTreeMap::from([(FRIGATE, row)]),
        &[],
    )
    .expect("builds");
    let mut c = Content::fixture();
    c.insert("econ_exchange", table);
    Arc::new(c)
}

fn bot(
    addr: std::net::SocketAddr,
    e: &dev::Expanded,
) -> (driver::Handle, std::thread::JoinHandle<()>) {
    let mut config = BotConfig::new(&e.name, addr, Pace::Fast).with_role(e.role);
    config.policy.gm_setup = false;
    config.policy.closed = true;
    config.policy.dev = Some(Arc::clone(&e.dev));
    config.chat = false;
    driver::spawn(config)
}

fn done(h: &driver::Handle) -> bool {
    let log = h.log();
    log.iter()
        .any(|e| e.what.starts_with("/devcorp") && e.kind == ActKind::Slash)
        && log.iter().any(|e| e.kind == ActKind::Acquire)
}

#[test]
fn a_roster_bot_gets_all5_and_buys_its_hull_from_the_dev_market() {
    let c = content();
    let rows = dev::parse_roster(&format!("red\tred\tmine\t1\tall5\t{FRIGATE_NAME}\n")).unwrap();
    let bots = dev::expand(&rows, Some(&c)).expect("the hull's name resolves");
    assert_eq!(bots[0].dev.hull, Some(FRIGATE));

    let mut cfg = config();
    cfg.sandbox.dev = true;
    cfg.sandbox.characters = vec![pilot(RED1, "red1", 100_000_901)];
    cfg.content = Some(Arc::clone(&c));
    let server = Server::bind(cfg).expect("bind");
    let addr = server.local_addr().expect("addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let (h, t) = bot(addr, &bots[0]);
    let held = |w: &std::sync::RwLock<eve_server::service::Registry>| {
        w.read()
            .expect("world")
            .store()
            .owned_by(RED1)
            .any(|i| i.type_id == FRIGATE)
    };
    let got = until(Duration::from_secs(30), || held(&world));
    h.stop();
    t.join().ok();
    assert!(got, "the hull is in the world:\n{}", dump(&h));
    let log = h.log();
    assert!(
        log.iter()
            .filter(|e| e.kind == ActKind::Slash)
            .all(|e| e.what.starts_with("/dev")),
        "no GM item: {}",
        dump(&h)
    );
    let w = world.read().expect("world");
    let n = eve_server::skill::all_in(w.content()).count() as i64;
    let sum: i64 = w
        .skills_for(Some(RED1))
        .expect("skills")
        .entries()
        .map(|e| e.level)
        .sum();
    assert_eq!(sum, 5 * n, "every skill at V");
    let bal = w.character(RED1).expect("pilot").balance;
    assert!(
        bal <= 1_000_000_000.0 - exchange::DEV_PRICE,
        "paid 100 ISK: {bal}"
    );
}

#[test]
fn opposite_sides_are_hostile_the_same_side_is_allied_and_the_boss_forms_a_fleet() {
    let c = content();
    let rows = dev::parse_roster("red\tred\tescort\t2\tall5\t-\nblue\tblue\tescort\t1\tall5\t-\n")
        .unwrap();
    let bots = dev::expand(&rows, Some(&c)).expect("expands");
    let mut cfg = config();
    cfg.sandbox.dev = true;
    cfg.sandbox.characters = vec![
        pilot(RED1, "red1", 100_000_901),
        pilot(RED2, "red2", 100_000_902),
        pilot(BLUE1, "blue1", 100_000_903),
    ];
    cfg.content = Some(Arc::clone(&c));
    let server = Server::bind(cfg).expect("bind");
    let addr = server.local_addr().expect("addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let spawned: Vec<_> = bots.iter().map(|e| bot(addr, e)).collect();
    let ready = until(Duration::from_secs(40), || {
        spawned.iter().all(|(h, _)| done(h))
    });
    let fleet_said = until(Duration::from_secs(40), || {
        spawned[0]
            .0
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Fleet && e.what.starts_with("dev fleet: invited"))
    });
    for (h, _) in &spawned {
        h.stop();
    }
    for (_, t) in spawned {
        t.join().ok();
    }
    assert!(ready, "every bot joined its side");
    assert!(fleet_said, "the boss invited its group");
    let w = world.read().expect("world");
    let aff = |id: i64| {
        let c = w.character(id).expect("pilot");
        Affiliation {
            character_id: id,
            corporation_id: c.corporation_id,
            alliance_id: c.alliance_id,
        }
    };
    let none = |_: i64, _: i64| None;
    assert!(
        !eve_bot::society::hostile_to(aff(RED1), aff(RED2), none),
        "one side is friendly"
    );
    assert!(
        eve_bot::society::hostile_to(aff(RED1), aff(BLUE1), none),
        "the other side is hostile"
    );
    assert!(
        eve_bot::society::hostile_to(aff(BLUE1), aff(RED2), none),
        "both ways"
    );
}
