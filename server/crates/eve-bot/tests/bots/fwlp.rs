//! FU-FWLP over a real socket: an enlisted plexer clears the enemy
//! militia's complex, and its militia LP rises (`LPSvc.
//! GetAllMyCharacterWalletLPBalances`, the client's LP window).
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): the sandbox home is an
//! invented low-sec system of the content fixture's warzone side A, with
//! one invented Novice complex owned by A (a structure and one spawn of the
//! no-bundle rat, flown for A), and invented militia corporations for both
//! sides. The expected LP is `fw_lp::payout` by name, never a number.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Pace};
use eve_marshal::PyValue;
use eve_server::celestial::{Band, Universe};
use eve_server::dungeon_data::{self, Dungeon, Entry, Object, Room, Spawn};
use eve_server::npc_data::{self, Bounty, Faction, Needs, NpcTypeRow, Tier};
use eve_server::population::{Diffs, Row};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const PLEXER: i64 = 90_000_801;
const REGION: i64 = 19_080_001;
const CONSTELLATION: i64 = 29_080_001;
const DUNGEON: i64 = 999_801;
const STRUCTURE: i64 = 990_801;
/// Invented militia corporations of sides A and B.
const CORP_A: i64 = 1_999_801;
const CORP_B: i64 = 1_999_802;
/// Invented station-owning corporation of side B: the enlistment office is
/// at a station of the militia's faction (FW-4, #100).
const OFFICE_B: i64 = 1_999_812;
/// `dunArchetypeComplexNovice` [V], as the plexer names it.
const NOVICE: i64 = eve_bot::plexer::COMPLEX_ARCHETYPES[0];

/// The content fixture's warzone sides.
fn sides() -> [i64; 2] {
    eve_server::facwar::Table::fixture().warzones()[0].factions
}

fn universe() -> Universe {
    let a = sides()[0];
    Universe::parse(
        &[format!(
            "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t{a}\t\t0.3\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture front"
        )],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

/// One Novice complex owned by A: a structure and one never-respawning
/// spawn, so killing the squad completes it.
fn dungeons() -> dungeon_data::Table {
    let d = Dungeon {
        id: DUNGEON,
        archetype: NOVICE,
        faction: Some(sides()[0]),
        difficulty: Some(1),
        entry_type: Some(dungeon_data::ENTRY_ANOMALY),
        name_id: Some(DUNGEON + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![Object {
            room: 1,
            id: 1,
            type_id: STRUCTURE,
            pos: [0.0; 3],
            radius: Some(1_000.0),
            ..Object::default()
        }],
        spawns: vec![Spawn {
            room: 1,
            object: 1,
            entity: 71,
            ..Spawn::default()
        }],
        ..Dungeon::default()
    };
    dungeon_data::Table {
        dungeons: BTreeMap::from([(DUNGEON, d)]),
        ..dungeon_data::Table::default()
    }
}

/// Both militias with their corporations; A flies the no-bundle rat.
fn npcs() -> npc_data::Table {
    let [a, b] = sides();
    let rat = eve_server::npc::FIXTURE_RAT.type_id;
    let mut t = npc_data::Table::default();
    for (f, corp) in [(a, CORP_A), (b, CORP_B)] {
        t.factions.insert(
            f,
            Faction {
                faction_id: f,
                corporation_id: corp + 10,
                militia_corporation_id: Some(corp),
                ..Faction::default()
            },
        );
    }
    t.corporations.insert(
        OFFICE_B,
        npc_data::Corporation {
            corp_id: OFFICE_B,
            faction_id: Some(sides()[1]),
            ..npc_data::Corporation::default()
        },
    );
    t.types.insert(
        rat,
        NpcTypeRow {
            type_id: rat,
            group_id: eve_server::npc::FIXTURE_RAT.group_id,
            faction_id: a,
            context: eve_server::npc_gen::ASTEROID.to_owned(),
            class: "Frigate".to_owned(),
            tier: Tier::Base,
            alias_of: None,
            needs: Needs::TURRET,
            role: npc_data::Role::Dps,
        },
    );
    t.bounty.insert(
        rat,
        Bounty {
            bounty: 1.0,
            ..Bounty::default()
        },
    );
    t
}

/// One low-sec FW row: a Novice complex in the warzone system.
fn population() -> eve_server::population::Table {
    eve_server::population::Table::new(vec![Row {
        key: "fx_fw_low".to_owned(),
        band: Band::Low,
        archetypes: vec![NOVICE],
        entry: Entry::Anomaly,
        count: 1.0,
        difficulties: Diffs::In(vec![1]),
        respawn_s: 600,
        ttl_s: 172_800,
    }])
    .expect("one row")
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![eve_server::model::Character {
        station_owner_id: OFFICE_B,
        ..pilot(PLEXER, "Plexer", 100_000_801)
    }];
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", population());
    config.content = Some(Arc::new(content));
    serve(config)
}

/// The caller's LP with `corp`.
fn lp_with(c: &mut eve_bot::Conn, corp: i64) -> i64 {
    let v = c
        .call("LPSvc", "GetAllMyCharacterWalletLPBalances", vec![])
        .expect("answered")
        .value()
        .unwrap_or(PyValue::None);
    eve_bot::plexer::read_lp(&v, corp)
}

/// **The packet's test**: a pilot enlisted in B (a plain connection, the
/// enlistment window's call) has no LP. As a plexer bot it undocks, finds
/// A's complex on the overlay, warps in and kills its rat, which completes
/// the complex. Logged in again, its LP with B's militia corporation is one
/// pilot's capture payout.
#[test]
fn clearing_the_enemys_complex_raises_the_plexers_militia_lp() {
    let [_, b] = sides();
    let addr = start();

    let mut c = login(addr, "Plexer");
    let joined = c
        .call(
            "fwCharacterEnlistmentMgr",
            "CreateMyDirectEnlistment",
            vec![PyValue::Int(b)],
        )
        .expect("answered");
    assert!(joined.refusal().is_none(), "enlists in B");
    assert_eq!(lp_with(&mut c, CORP_B), 0, "no LP before the complex");
    drop(c);

    let bot = driver::spawn(
        BotConfig::new("Plexer", addr, Pace::Fast)
            .with_token(DEV_TOKEN)
            .with_role(&eve_bot::plexer::ROLE),
    )
    .0;
    assert!(
        until(Duration::from_secs(240), || {
            let log = bot.log();
            let plex = log.iter().position(|e| ok(e, ActKind::Plex));
            let fired = log.iter().rposition(|e| ok(e, ActKind::Fire));
            let looted = log.iter().rposition(|e| ok(e, ActKind::Loot));
            matches!((plex, fired, looted), (Some(p), Some(f), Some(l)) if p < f && f < l)
        }),
        "warp to the complex, fire on its rat, loot:\n{}",
        dump(&bot)
    );
    bot.stop();

    let want = eve_server::fw_lp::payout(
        NOVICE,
        eve_server::fw_lp::Side::Capture,
        0,
        eve_server::facwar::VP_THRESHOLD,
        1,
        // The fixture's one system has no gates: the back line.
        eve_server::facwar::Adjacency::Backline,
    );
    assert!(want > 0);
    let mut got = 0;
    let paid = until(Duration::from_secs(30), || {
        let mut c = login(addr, "Plexer");
        got = lp_with(&mut c, CORP_B);
        got == want
    });
    assert!(paid, "the plexer's LP with B's militia: {got}, want {want}");
}

// ---- X-LPSTORE: the plexer spends what it earned ------------------------------------

const SPENDER: i64 = 90_000_821;
const PRODUCT: i64 = 990_821;
const PRODUCT_GROUP: i64 = 99_821;
const TABLE: i64 = 9_982;
const OFFER: i64 = 99_820_000;

/// The militia's store: one offer for [`PRODUCT`], costing one capture's
/// LP and nothing else, whose product sells at twice the least rate a bot
/// redeems for ([`eve_bot::plexer::MIN_ISK_PER_LP`]).
fn store_content() -> eve_server::content::Content {
    let lp = eve_server::fw_lp::payout(
        NOVICE,
        eve_server::fw_lp::Side::Capture,
        0,
        eve_server::facwar::VP_THRESHOLD,
        1,
        // The fixture's one system has no gates: the back line.
        eve_server::facwar::Adjacency::Backline,
    );
    let base =
        eve_bot::plexer::MIN_ISK_PER_LP * 2.0 * lp as f64 / (1.0 - eve_bot::plexer::SALE_COST);
    let mut c = eve_server::content::Content::fixture();
    c.insert("universe", universe());
    c.insert("dungeon_data", dungeons());
    c.insert("npc_data", npcs());
    c.insert("population", population());
    let types = format!(
        "{}type\t{PRODUCT}\t{PRODUCT_GROUP}\t7\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Militia Product\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert(
        "econ_prices",
        eve_server::econ_prices::Table::parse(&[format!(
            "{PRODUCT}\t{base}\tbase\t{base}\t7\t{PRODUCT_GROUP}\t\t1"
        )])
        .expect("the price parses"),
    );
    let offer = format!("{TABLE}\t{OFFER}\t{PRODUCT}\t1\t{lp}\t0\t");
    let corp = format!("{CORP_B}\t\t\t\t\t\t0\t0\t0\t\t\t{TABLE}\tMIL\tMilitia");
    c.insert(
        "loyalty",
        eve_server::loyalty::Table::parse(&[offer], &[corp]).expect("the store parses"),
    );
    c
}

/// **The spend, over a socket**: the plexer of the first test, now with a
/// store at its militia corporation. After the capture pays its LP the
/// bot's next docked sortie redeems the offer (the LP is spent to zero) and
/// lists the product.
#[test]
fn the_plexer_redeems_its_militia_lp_and_lists_the_product() {
    let [_, b] = sides();
    let mut cfg = config();
    // A name of its own: the bots' plans are per process and name.
    // Listing an ask costs the broker's fee: the plexer needs ISK for it.
    // `is_bot`: the poll below logs in as the bot's own character, and a
    // human on a bot's character is not taken over (NET-TAKEOVER).
    cfg.sandbox.characters = vec![eve_server::model::Character {
        balance: 1_000_000.0,
        is_bot: true,
        station_owner_id: OFFICE_B,
        ..pilot(SPENDER, "Spender", 100_000_821)
    }];
    cfg.content = Some(Arc::new(store_content()));
    let addr = serve(cfg);

    let mut c = login(addr, "Spender");
    let joined = c
        .call(
            "fwCharacterEnlistmentMgr",
            "CreateMyDirectEnlistment",
            vec![PyValue::Int(b)],
        )
        .expect("answered");
    assert!(joined.refusal().is_none(), "enlists in B");
    drop(c);

    let bot = driver::spawn(
        BotConfig::new("Spender", addr, Pace::Fast)
            .with_token(DEV_TOKEN)
            .with_role(&eve_bot::plexer::ROLE),
    )
    .0;
    let mut listed = false;
    let done = until(Duration::from_secs(300), || {
        let mut c = login(addr, "Spender");
        listed = c
            .my_orders()
            .map(|o| o.iter().any(|x| !x.bid && x.type_id == PRODUCT))
            .unwrap_or(false);
        listed
    });
    bot.stop();
    assert!(done, "the product was listed:\n{}", dump(&bot));
    let mut c = login(addr, "Spender");
    assert_eq!(lp_with(&mut c, CORP_B), 0, "the LP was spent");
}
