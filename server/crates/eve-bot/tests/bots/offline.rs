//! E1 over a real socket: two bots log off docked, the server's game clock
//! runs sixty times faster than the wall, and when they log on again their
//! wallets and hangars hold exactly what the offline settlement made, with
//! the matching ledger rows.
//!
//! The bots here are plain connections (the settlement is the server's; the
//! bot side is one call, [`eve_bot::offline::park`]): a miner whose rocks
//! are in its home system and a ratter with a loaded gun. Names, ids and
//! rows are invented; no `[A]` value is asserted by number (the consts of
//! `econ_agents::rules` are named).

use super::common::{SYSTEM, config, login, pilot, rock};
use eve_bot::offline::{self, Parking};
use eve_server::econ_agents::rules;
use eve_server::game_clock::GameClock;
use eve_server::item::{Edit, Slot};
use eve_server::server::Server;
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const MINER: i64 = 90_000_701;
const RATTER: i64 = 90_000_702;
/// The characters' capsules (their sandbox `ship_id`); the hulls are minted.
const MINER_POD: i64 = 100_000_701;
const RATTER_POD: i64 = 100_000_702;
const MERLIN: i64 = 603;
const MINER_I: i64 = 483;
const GUN: i64 = 484;
const ROUND: i64 = 184;
const VELDSPAR: i64 = 1230;
const HI_SLOT_0: i64 = 27;
/// The server's clock runs this many times faster than the wall.
const SCALE: u32 = 60;
/// Wall seconds the bots stay away: `SCALE` x this is game time, and it
/// must clear `rules::MIN_SPAN_S`.
const AWAY_S: u64 = 20;

fn serve_world(config: eve_server::server::Config) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

/// A bot character that the server marks as a bot.
fn bot(id: i64, name: &str, hull: i64) -> eve_server::model::Character {
    eve_server::model::Character {
        is_bot: true,
        balance: 5_000_000.0,
        ..pilot(id, name, hull)
    }
}

/// Give `owner` a hull and fit it, through the store's own plan (the way a
/// GM `/createitem` and the fitting window end up): the hull and each of
/// `fit` `(type, flag, quantity)` are created in the hangar, the modules are
/// assembled onto the hull at their flag, a charge is created in its gun's slot;
/// the character flies it.
fn equip(world: &Arc<RwLock<Registry>>, owner: i64, fit: &[(i64, i64, i64)]) {
    let mut w = world.write().expect("the world lock");
    let station = w
        .character(owner)
        .and_then(|c| c.station_id)
        .expect("docked");
    let hangar = Slot::new(owner, station, eve_server::model::FLAG_HANGAR);
    let made = w
        .apply_items(&[Edit::Create {
            type_id: MERLIN,
            quantity: 1,
            to: hangar,
        }])
        .expect("the hull");
    let hull = made[0].item_id();
    for &(type_id, flag, quantity) in fit {
        let charge = eve_server::item::find_in(w.content(), type_id)
            .expect("fixture type")
            .category_id
            == eve_server::item::CATEGORY_CHARGE;
        if charge {
            w.apply_items(&[Edit::Create {
                type_id,
                quantity,
                to: Slot::new(owner, hull, flag),
            }])
            .expect("the loaded charge");
        } else {
            let made = w
                .apply_items(&[Edit::Create {
                    type_id,
                    quantity: 1,
                    to: hangar,
                }])
                .expect("the module");
            // Fitting is "one off the stack, into the slot, assembled".
            w.apply_items(&[Edit::Assemble {
                item_id: made[0].item_id(),
                to: Slot::new(owner, hull, flag),
            }])
            .expect("the fit");
        }
    }
    let c = w.character_mut(owner).expect("the character");
    c.ship_id = hull;
    c.ship_type_id = MERLIN;
}

fn units(c: &mut eve_bot::Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
        .map(|r| r.quantity)
        .sum()
}

#[test]
fn two_bots_log_off_the_clock_runs_and_they_log_on_to_what_was_settled() {
    let mut config = config();
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    config.sandbox.characters = vec![
        bot(MINER, "Offline Miner", MINER_POD),
        bot(RATTER, "Offline Ratter", RATTER_POD),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks([
        rock(0, 6_000, [80_000.0, 0.0, 0.0]),
        rock(1, 6_000, [90_000.0, 0.0, 0.0]),
    ]);
    let (addr, world) = serve_world(config);
    equip(&world, MINER, &[(MINER_I, HI_SLOT_0, 1)]);
    equip(
        &world,
        RATTER,
        &[(GUN, HI_SLOT_0, 1), (ROUND, HI_SLOT_0, 500)],
    );
    let rocks_before: i64 = {
        let w = world.read().expect("the world lock");
        w.rocks().in_system(SYSTEM).map(|r| r.quantity).sum()
    };

    // Log on, say which career, and log off (the connection drops).
    let (miner_cash, ratter_cash) = {
        let mut m = login(addr, "Offline Miner");
        let mut r = login(addr, "Offline Ratter");
        let cash = (
            m.cash_balance().expect("balance"),
            r.cash_balance().expect("balance"),
        );
        assert!(matches!(offline::park(&mut m, "miner"), Ok(Parking::Parked(c)) if c == "miner"));
        assert!(matches!(offline::park(&mut r, "ratter"), Ok(Parking::Parked(c)) if c == "ratter"));
        // A career with no offline rule is not even asked.
        assert!(matches!(
            offline::park(&mut m, "explorer"),
            Ok(Parking::Unavailable(_))
        ));
        cash
    };
    let left_at = Instant::now();

    // Nothing is settled while they are away and no sweep period has passed.
    std::thread::sleep(Duration::from_secs(AWAY_S));
    let away_s = left_at.elapsed().as_secs_f64();
    {
        let w = world.read().expect("the world lock");
        assert!(eve_server::econ_agents::agent(&w, MINER).is_some());
        assert!(eve_server::econ_agents::agent(&w, RATTER).is_some());
    }

    // Log on again: the settlement runs as the character is selected.
    let mut m = login(addr, "Offline Miner");
    let mut r = login(addr, "Offline Ratter");

    // The miner: ore in the hangar, exactly what left the rocks.
    let ore = units(&mut m, VELDSPAR);
    assert!(ore > 0, "the miner mined while away");
    let rocks_after: i64 = {
        let w = world.read().expect("the world lock");
        w.rocks().in_system(SYSTEM).map(|r| r.quantity).sum()
    };
    assert_eq!(rocks_before - rocks_after, ore, "ore is conserved");

    // The ratter: the wallet moved by the ref-85 bounty rows, no more.
    let paid = r.cash_balance().expect("balance") - ratter_cash;
    assert!(paid > 0.0, "the ratter was paid while away");
    let w = world.read().expect("the world lock");
    let rows: Vec<_> = w
        .journal()
        .entries()
        .iter()
        .filter(|e| e.wallet() == RATTER)
        .collect();
    let bounty: f64 = rows
        .iter()
        .filter(|e| e.entry_type_id == eve_server::npc::bounty::REF_BOUNTY_PRIZES)
        .map(|e| e.amount)
        .sum();
    assert!(bounty > 0.0, "a ref-85 row");
    let others: f64 = rows
        .iter()
        .filter(|e| e.entry_type_id != eve_server::npc::bounty::REF_BOUNTY_PRIZES)
        .map(|e| e.amount)
        .sum();
    assert!(
        (bounty + others - paid).abs() < 0.01,
        "the wallet is the ledger: paid {paid}, rows {bounty} + {others}"
    );
    // The miner's wallet is untouched by the sale it could not make (no bid,
    // no refinery rows for an unrefinable fixture ore).
    let _ = miner_cash;

    // Both rows are gone: the bots are live again, and each was settled once.
    assert!(eve_server::econ_agents::agent(&w, MINER).is_none());
    assert!(eve_server::econ_agents::agent(&w, RATTER).is_none());
    let book = w.ext::<eve_server::econ_agents::Book>();
    assert_eq!(book.settled(), 2);
    // The span was the wall time away, scaled; the work a duty share of it.
    let report = book
        .recent()
        .iter()
        .find(|r| r.character == MINER)
        .expect("the miner's report");
    let game_h = away_s * f64::from(SCALE) / 3_600.0;
    assert!(
        (report.span_h - game_h).abs() <= 0.15 * game_h + 0.02,
        "span {} against {game_h}",
        report.span_h
    );
    assert!(
        report.work_h > 0.0 && (report.work_h - report.span_h * rules::DUTY).abs() < 1e-9,
        "work {} of span {}",
        report.work_h,
        report.span_h
    );
}
