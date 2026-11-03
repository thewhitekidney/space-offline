//! **PROG-MINE over a real socket**: the mining ladder's hull buy, a foreman's
//! boosted fleet, and compression at a fleet mate's compressor.
//!
//! - **A hull upgrade buy**: a miner with the skills and the ISK buys a T1
//!   barge and its Strip Miner I from a player's asks (the builders' product
//!   in the closed economy), assembles the hull and keeps the laser; one
//!   without Mining Barge buys neither. No GM from the bot ([`acquire`] is
//!   called as the bot's own acquire step is).
//! - **A boosted fleet**: a foreman in an invented command hull outfits itself
//!   (board, burst fitted and loaded, compressor fitted, undock), makes a
//!   fleet and invites the miner on the grid; the miner accepts; the burst
//!   runs and the miner's laser cycle shortens by the burst's percent.
//! - **Compression**: the miner's ore, offered to the foreman's compressor by
//!   [`eve_bot::ore::compress`], becomes its compressed twin, unit for unit.
//!
//! **Invented rows only**: the ids of the mining ladder (a Retriever, a Strip
//! Miner I, an Orca, the burst and its charge, a compressor) are the
//! client's [V-data] because the bot's tables name them; every row, price,
//! percent and range here is this file's own. The GM lines are a non-bot's
//! setup, named at the call site.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::acquire::{self, Lacking};
use eve_bot::brain::{Memory, Policy};
use eve_bot::builder::SellLine;
use eve_bot::mine_ladder::{ORCA, RETRIEVER, STRIP_MINER_I};
use eve_bot::{Conn, FleetInvite, ItemRow, foreman, ore};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::dbuff_data::Dbuffs;
use eve_server::reprocess::fixture::{ORE_MIXED, ORE_MIXED_COMPRESSED};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const MINER: i64 = 90_000_551;
const SELLER: i64 = 90_000_552;
const FOREMAN: i64 = 90_000_553;
const NO_BARGE: i64 = 90_000_554;
const ALICE: i64 = 90_000_555;
/// A Merlin [V-data]: on the ladder's table, so a foreman invites it.
const MERLIN: i64 = 603;
const WALLET: f64 = 200_000.0;
/// This file's own prices: the barge's and the laser's reference prices,
/// and the seller's asks a little over.
const HULL_BASE: f64 = 10_000.0;
const LASER_BASE: f64 = 2_000.0;
const HULL_ASK: f64 = 11_000.0;
const LASER_ASK: f64 = 2_200.0;
/// Invented module and group ids.
const LASER: i64 = 990_730_001;
const ORE_GROUP: i64 = 99_001;
const LIST: i64 = 990_730_010;
const COLLECTION: i64 = 990_730_020;
/// The burst's percent on the laser's duration: this file's own number.
const BURST_PCT: f64 = -20.0;
const BURST_MS: f64 = 600_000.0;
const COMPRESSOR_RANGE: f64 = 83_000.0;
const ORE_UNITS: i64 = 400;

/// The ladder ids the bot's tables name [V-data].
const BURST: i64 = foreman::BURST_MINING_I;
const CHARGE: i64 = foreman::CHARGE_OPTIMIZATION;
const COMPRESSOR: i64 = foreman::COMPRESSOR_LARGE;
const MINING: i64 = eve_bot::mine_ladder::SKILL_MINING;
const ASTROGEOLOGY: i64 = eve_bot::mine_ladder::SKILL_ASTROGEOLOGY;
const MINING_BARGE: i64 = eve_bot::mine_ladder::SKILL_MINING_BARGE;
/// The charge group a mining burst takes, and its effect [V].
const CHARGE_GROUP: i64 = eve_bot::burst::KINDS[5].0;
const BURST_EFFECT: (&str, i64) = ("moduleBonusWarfareLinkMining", 6736);

fn content() -> Arc<Content> {
    let hull = |id: i64, group: i64| {
        format!(
            "type\t{id}\t{group}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
             76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;479=600000.0;\
             482=260.0;552=40.0;564=640.0;600=5.0;1137=3.0\tInvented Hull {id}\n"
        )
    };
    let rows = format!(
        "{fixture}\
         effect\t67\t2\t0\t73\t6\t54\t0\t0\tminingLaser\teffects.Mining\n\
         effect\t{be}\t1\t0\t73\t6\t54\t0\t0\t{bn}\teffects.WarfareLinkSphereMining\n\
         effect\t6737\t0\t0\t0\t0\t0\t0\t0\tchargeBonusWarfareCharge\t\n\
         effect\t{ce}\t1\t0\t73\t6\t54\t0\t0\tindustrialItemCompression\teffects.IndustrialItemCompression\n\
         modifier\t6737\totherID\tItemModifier\t2468\t2468\t7\t\t\n\
         modifier\t6737\totherID\tItemModifier\t2469\t2596\t4\t\t\n\
         {h1}{h2}{h3}\
         type\t{BURST}\t1770\t7\t1\t1\t60.0\t3.0\t0.0\t1.0\t{be}\t12,16,{be}\t6=0.0;30=1.0;50=1.0;54=15000.0;56=1.0;73=600000.0;604={CHARGE_GROUP}.0;2469=1.0;2535={BURST_MS}\tInvented Mining Burst\n\
         type\t{CHARGE}\t{CHARGE_GROUP}\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t6737\t6737\t2468={COLLECTION}.0;2596={BURST_PCT}\tInvented Mining Charge\n\
         type\t{COMPRESSOR}\t4174\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{ce}\t12,16,{ce}\t6=0.0;30=1.0;50=1.0;54={COMPRESSOR_RANGE:?};73=60000.0;3255={LIST}.0\tInvented Compressor\n\
         type\t{LASER}\t54\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t6=1.0;30=1.0;50=1.0;54=15000.0;73=15000.0;77=10.0;182=3386.0;277=1.0\tInvented Laser\n\
         type\t{STRIP_MINER_I}\t464\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t6=1.0;30=1.0;50=1.0;54=15000.0;73=45000.0;77=150.0\tInvented Strip Miner\n\
         type\t{ORE_MIXED}\t{ORE_GROUP}\t25\t1\t100\t0.35\t0.0\t0.0\t1.0\t0\t\t\tInvented Ore\n\
         type\t{ORE_MIXED_COMPRESSED}\t{ORE_GROUP}\t25\t1\t100\t0.0035\t0.0\t0.0\t1.0\t0\t\t\tInvented Compressed Ore\n",
        fixture = eve_server::typedata::FIXTURE,
        be = BURST_EFFECT.1,
        bn = BURST_EFFECT.0,
        ce = eve_server::compression::EFFECT_COMPRESSION,
        h1 = hull(RETRIEVER, 463),
        h2 = hull(ORCA, 941),
        h3 = hull(MERLIN, 25),
    );
    let td = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{RETRIEVER}\t{HULL_BASE}\tbase\t0.0\t6\t463\t\t1"),
        format!("{STRIP_MINER_I}\t{LASER_BASE}\tbase\t0.0\t7\t464\t\t1"),
    ])
    .expect("the prices parse");
    let skills = eve_server::skill::Table::parse(&format!(
        "{}skill\t{ASTROGEOLOGY}\t990730100\t3\t164\t166\t0.01\t\tInvented Astrogeology\n\
         skill\t{MINING_BARGE}\t990730100\t4\t164\t166\t0.01\t\tInvented Mining Barge\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skill rows parse");
    let mut dungeons = eve_server::dungeon_data::Table::fixture();
    dungeons.typelists.insert(
        LIST,
        eve_server::dungeon_data::TypeList {
            inc_groups: vec![ORE_GROUP],
            ..Default::default()
        },
    );
    let collections = vec![format!(
        "{COLLECTION}\tPostPercent\tMinimum\tShowNormal\tInvented Mining Boost\tfixture"
    )];
    let modifiers = vec![format!("{COLLECTION}\tskill\t73\t\t{MINING}")];
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert(eve_server::skill::TABLE.name, skills);
    c.insert("dungeons", dungeons);
    c.insert(
        "dbuffs",
        Dbuffs::parse(&collections, &modifiers).expect("the collection parses"),
    );
    Arc::new(c)
}

fn pilots() -> Vec<eve_server::model::Character> {
    [
        (MINER, "Miner1", 100_000_551),
        (SELLER, "Seller", 100_000_552),
        (FOREMAN, "Foreman1", 100_000_553),
        (NO_BARGE, "Miner2", 100_000_554),
        (ALICE, "Alice", 100_000_555),
    ]
    .into_iter()
    .map(|(id, name, ship)| eve_server::model::Character {
        balance: WALLET,
        ..pilot(id, name, ship)
    })
    .collect()
}

fn start() -> (SocketAddr, eve_server::server::Server) {
    let mut config = config();
    config.sandbox.characters = pilots();
    config.content = Some(content());
    let server = eve_server::server::Server::bind(config).expect("bind");
    (server.local_addr().expect("local addr"), server)
}

fn serve_it() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = pilots();
    config.content = Some(content());
    serve(config)
}

fn hangar_of(c: &mut Conn, type_id: i64) -> Vec<ItemRow> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .collect()
}

/// The seller lists `quantity` of `type_id` at the home station at `price`,
/// a hull repackaged first as the market wants.
fn ask(c: &mut Conn, type_id: i64, quantity: i64, price: f64) {
    gm(c, &format!("/createitem {type_id} {quantity}"));
    let station = c.session().station_id().expect("docked");
    let ships: Vec<PyValue> = hangar_of(c, type_id)
        .iter()
        .filter(|r| r.category_id == acquire::CATEGORY_SHIP)
        .map(|r| PyValue::Tuple(vec![PyValue::Int(r.item_id), PyValue::Int(station)]))
        .collect();
    if !ships.is_empty() {
        let batch = PyValue::Dict(vec![(PyValue::List(ships), PyValue::Int(station))]);
        let done = c
            .call("repackagingSvc", "RepackageItems", vec![batch])
            .expect("answered");
        assert_eq!(done.refusal(), None, "repackaged");
    }
    let lines: Vec<SellLine> = hangar_of(c, type_id)
        .iter()
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
    .expect("lists");
}

fn lacking() -> Lacking {
    Lacking {
        ship: true,
        module: true,
        ammo: false,
        in_capsule: true,
    }
}

/// **A hull upgrade buy**: the barge and the Strip Miner I come from the
/// seller's asks for the pilot with the skills and the ISK; the pilot
/// without Mining Barge buys neither (and no GM is in the bot's path).
#[test]
fn a_miner_with_the_skills_buys_a_barge_and_its_strip_miner_and_one_without_does_not() {
    let addr = serve_it();
    let mut seller = login(addr, "Seller");
    ask(&mut seller, RETRIEVER, 2, HULL_ASK);
    ask(&mut seller, STRIP_MINER_I, 2, LASER_ASK);

    // The test's own setup (a human stand-in): trained skills.
    let mut skilled = login(addr, "Miner1");
    gm(&mut skilled, &format!("/giveskill {MINING} 4"));
    gm(&mut skilled, &format!("/giveskill {ASTROGEOLOGY} 3"));
    gm(&mut skilled, &format!("/giveskill {MINING_BARGE} 1"));
    let before = skilled.cash_balance().expect("a wallet");
    let got = acquire::run_with(&mut skilled, &acquire::MINER_KIT, lacking(), None)
        .expect("an acquire step");
    let barge = hangar_of(&mut skilled, RETRIEVER);
    assert_eq!(barge.len(), 1, "one barge bought: {}", got.summary());
    assert!(barge[0].singleton, "and assembled: {}", got.summary());
    let in_hold = skilled
        .cargo()
        .expect("the hold lists")
        .iter()
        .chain(hangar_of(&mut skilled, STRIP_MINER_I).iter())
        .any(|r| r.type_id == STRIP_MINER_I);
    assert!(
        in_hold,
        "the Strip Miner I, not a Miner I: {}",
        got.summary()
    );
    assert!(
        hangar_of(&mut skilled, eve_bot::brain::MINER_I).is_empty(),
        "no Miner I was bought for a barge"
    );
    let after = skilled.cash_balance().expect("a wallet");
    assert!(
        before - after >= HULL_ASK + LASER_ASK,
        "it paid the asks: {before} -> {after}"
    );

    // No Mining Barge: the ladder offers nothing, the legacy tail has no ask.
    let mut green = login(addr, "Miner2");
    gm(&mut green, &format!("/giveskill {MINING} 4"));
    gm(&mut green, &format!("/giveskill {ASTROGEOLOGY} 3"));
    let got = acquire::run_with(&mut green, &acquire::MINER_KIT, lacking(), None)
        .expect("an acquire step");
    assert!(
        hangar_of(&mut green, RETRIEVER).is_empty(),
        "no barge without the skill: {}",
        got.summary()
    );
}

/// A foreman outfitted and in space, a miner beside it, both in the same
/// system: `(server world, foreman, its memory, miner, the miner's laser)`.
struct Scene {
    foreman: Conn,
    memory: Memory,
    miner: Conn,
    laser: i64,
}

fn scene(ore_units: i64) -> Scene {
    let (addr, server) = start();
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    // The miner: a Merlin, a laser, in space.
    let mut miner = login(addr, "Miner1");
    miner.keep_pushes();
    gm(&mut miner, &format!("/createitem {MERLIN}"));
    gm(&mut miner, &format!("/giveskill {MINING} 1"));
    gm(&mut miner, &format!("/createitem {LASER}"));
    let hull = hangar_of(&mut miner, MERLIN)
        .last()
        .expect("a Merlin")
        .item_id;
    miner.board(hull).expect("boards");
    if ore_units > 0 {
        gm(&mut miner, &format!("/createitem {ORE_MIXED} {ore_units}"));
        let stack = hangar_of(&mut miner, ORE_MIXED)
            .last()
            .expect("the ore")
            .item_id;
        miner
            .load_cargo(stack, Some(ore_units))
            .expect("the ore goes into the hold");
    }
    let spare: Vec<i64> = hangar_of(&mut miner, LASER)
        .iter()
        .map(|r| r.item_id)
        .collect();
    let laser = miner
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == LASER)
        .expect("the laser is fitted")
        .item_id;
    if !miner.is_online(laser).expect("asks") {
        miner.set_online(laser).expect("onlines");
    }
    miner.undock().expect("undocks");
    miner.enter_space(SYSTEM).expect("the park");

    // The foreman: given the hull and the kit (a non-bot's setup), it
    // outfits itself with its own turns.
    let mut foreman = login(addr, "Foreman1");
    foreman.keep_pushes();
    for line in [
        format!("/createitem {ORCA}"),
        format!("/createitem {BURST}"),
        format!("/createitem {CHARGE} 10"),
        format!("/createitem {COMPRESSOR}"),
    ] {
        gm(&mut foreman, &line);
    }
    let mut memory = Memory::new("Foreman1", Policy::fast());
    let mut notes = Vec::new();
    for _ in 0..8 {
        let note = foreman::turn(&mut foreman, &mut memory).expect("a turn");
        notes.push(note.clone());
        if note == "undocked" {
            break;
        }
    }
    assert!(notes.iter().any(|n| n.starts_with("boarded")), "{notes:?}");
    assert!(notes.iter().any(|n| n.starts_with("burst:")), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.starts_with("fitted compressor")),
        "{notes:?}"
    );
    assert_eq!(
        notes.last().map(String::as_str),
        Some("undocked"),
        "{notes:?}"
    );
    foreman.enter_space(SYSTEM).expect("the park");

    // Both a kilometre apart, at rest: inside the burst's and the
    // compressor's reach.
    {
        let mut w = world.write().expect("the world");
        let a = w.roster().get(MINER).expect("the miner").motion.position;
        for (who, dx) in [(MINER, 0.0), (FOREMAN, 1_000.0)] {
            let p = w.roster_mut().get_mut(who).expect("in space");
            p.motion.velocity = [0.0; 3];
            p.motion.position = [a[0] + dx, a[1], a[2]];
        }
    }
    Scene {
        foreman,
        memory,
        miner,
        laser,
    }
}

/// Read `c` until `done` holds or twenty seconds.
fn pump_until(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> bool {
    until(Duration::from_secs(20), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        done(c)
    })
}

/// The foreman makes a fleet of the miner on the grid; the miner accepts.
fn fleet_up(s: &mut Scene) {
    assert!(
        pump_until(&mut s.foreman, |c| c.world_now().ship_of(MINER).is_some()),
        "the foreman sees the miner's ship"
    );
    let fleet = s.foreman.create_fleet(ORCA).expect("the fleet");
    let note = foreman::invite(&mut s.foreman, &mut s.memory, fleet).expect("invites");
    assert!(note.contains(&MINER.to_string()), "{note}");
    assert!(!note.contains(&FOREMAN.to_string()), "not himself: {note}");
    let mut invite = None;
    assert!(pump_until(&mut s.miner, |c| {
        invite = c
            .take_pushes()
            .iter()
            .find_map(|(n, a)| FleetInvite::from_push(n, a));
        invite.is_some()
    }));
    let invite = invite.expect("the miner is invited");
    s.miner
        .accept_fleet_invite(invite.fleet_id, MERLIN)
        .expect("the miner joins");
}

/// **A boosted fleet**: the foreman outfits itself, invites the miner, runs
/// the burst, and the miner's laser cycle is the burst's percent shorter.
#[test]
fn a_foreman_outfits_makes_a_fleet_and_its_burst_shortens_the_miners_laser_cycle() {
    let mut s = scene(0);
    let duration_ms = |c: &mut Conn, laser: i64| {
        c.module_row(laser)
            .expect("the module row")
            .attribute(73)
            .expect("a duration")
    };
    let plain = duration_ms(&mut s.miner, s.laser);
    fleet_up(&mut s);

    // The burst runs (the foreman's own step), and stays running.
    let step = eve_bot::burst::step(&mut s.foreman).expect("a step");
    assert!(matches!(step, eve_bot::burst::Step::Run { .. }), "{step:?}");
    assert_eq!(
        eve_bot::burst::step(&mut s.foreman).expect("still"),
        eve_bot::burst::Step::Running
    );

    let boosted = plain * (100.0 + BURST_PCT) / 100.0;
    let laser = s.laser;
    assert!(
        pump_until(&mut s.miner, |c| {
            (duration_ms(c, laser) - boosted).abs() < 1e-6
        }),
        "the laser cycle went {plain} -> {boosted}, reads {}",
        duration_ms(&mut s.miner, laser)
    );
}

/// **Compression**: the miner's ore, offered to the foreman's compressor,
/// is compressed unit for unit and the raw stack is gone.
#[test]
fn a_fleet_mates_compressor_compresses_the_miners_ore_over_the_socket() {
    let mut s = scene(ORE_UNITS);
    fleet_up(&mut s);
    let ship = s.miner.session().ship_id().expect("a ship");
    let held = |c: &mut Conn, t: i64| {
        c.ship_items(ship, None)
            .expect("lists")
            .iter()
            .filter(|r| r.type_id == t)
            .map(|r| r.quantity)
            .sum::<i64>()
    };
    assert_eq!(
        held(&mut s.miner, ORE_MIXED),
        ORE_UNITS,
        "the hold carries it"
    );
    let done = ore::compress(&mut s.miner).expect("compresses");
    assert_eq!(done, 1, "one stack");
    assert_eq!(held(&mut s.miner, ORE_MIXED), 0, "the raw ore is gone");
    assert_eq!(
        held(&mut s.miner, ORE_MIXED_COMPRESSED),
        ORE_UNITS,
        "unit for unit"
    );
}

/// **BOT-CHURN, the Orca**: a dev-roster foreman with a human in the park
/// undocks and stays out for a minute -- it did not (courtesy docked it at
/// the mouth 6 s after the undock, four times). Without the dev roster it
/// does dock, and its dock's note says courtesy: the control.
fn orca_bot(dev: bool) -> (SocketAddr, eve_bot::driver::Handle, Conn) {
    let addr = serve_it();
    // The kit: a non-bot's setup, on the bot's own character.
    {
        let mut c = login(addr, "Foreman1");
        for line in [
            format!("/createitem {ORCA}"),
            format!("/createitem {BURST}"),
            format!("/createitem {CHARGE} 10"),
            format!("/createitem {COMPRESSOR}"),
        ] {
            gm(&mut c, &line);
        }
    }
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    let mut config = eve_bot::driver::BotConfig::new("Foreman1", addr, eve_bot::driver::Pace::Fast)
        .with_role(&foreman::ROLE);
    config.policy.courtesy = true;
    config.policy.combat_drones = false;
    config.policy.dev = dev.then(super::bot_churn::dev_bot);
    config.content = Some(content());
    let _ = alice.pump(Duration::from_millis(100));
    (addr, eve_bot::driver::spawn(config).0, alice)
}

fn dock_notes(h: &eve_bot::driver::Handle) -> Vec<String> {
    h.log()
        .iter()
        .filter(|e| e.kind == eve_bot::brain::ActKind::Dock)
        .map(|e| e.what.clone())
        .collect()
}

#[test]
fn a_foreman_with_courtesy_docks_for_a_human_and_logs_why() {
    let (_, bot, mut alice) = orca_bot(false);
    let docked = until(Duration::from_secs(120), || {
        let _ = alice.pump(Duration::from_millis(100));
        dock_notes(&bot).iter().any(|w| w.contains("courtesy"))
    });
    bot.stop();
    assert!(docked, "{:?}", bot.log());
}

#[test]
fn a_dev_roster_foreman_with_a_human_in_the_park_undocks_and_stays_out() {
    let (_, bot, mut alice) = orca_bot(true);
    let out = until(Duration::from_secs(120), || {
        let _ = alice.pump(Duration::from_millis(100));
        bot.log()
            .iter()
            .any(|e| e.kind == eve_bot::brain::ActKind::Foreman && e.what.ends_with("undocked"))
    });
    assert!(out, "the foreman undocks: {:?}", bot.log());
    let from = std::time::Instant::now();
    while from.elapsed() < Duration::from_secs(60) {
        let _ = alice.pump(Duration::from_millis(200));
    }
    bot.stop();
    for note in dock_notes(&bot) {
        assert!(note.contains("HoldFull"), "{note}\n{:?}", bot.log());
    }
}
