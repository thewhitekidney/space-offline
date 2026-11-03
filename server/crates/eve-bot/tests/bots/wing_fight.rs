//! FC-2 over real sockets: a wing bot shoots what its FC tags.
//!
//! The FC is a plain connection ("Pilot"): he lists a fleet, the wing bot
//! joins through the finder, follows him out of the station, and when he
//! deploys two rats beside himself and tags the **farther** one
//! (`CmdFleetTagTarget`, the client's own menu call), the bot's guns go to
//! the tagged rat first. The tag's path is the whole chain: the commander's
//! call, the server's `OnFleetStateChange` push, the driver's parse, the
//! wing's pick, the escort's fight.
//!
//! Same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is an
//! expected value**: the assertions are about which rat the bot orbits, locks
//! and fires on, never a range or a delay; the deadlines are ceilings.
//! "Anyone shooting the FC is shot" needs a second armed human and is covered
//! by the pure tests (`wing_fight::tests`), not here.

use super::common::{SYSTEM, config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_bot::travel::Map;
use eve_bot::wing::{ROLE, Seat};
use eve_bot::wing_hall::WingHall;
use eve_marshal::PyValue;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PILOT: i64 = 90_000_811;
const PILOT_SHIP: i64 = 100_000_811;
const WING: i64 = 90_000_812;
/// The no-bundle rat `/entity deploy` knows, invented (`npc::FIXTURE_RAT`).
const RAT: i64 = eve_server::npc::FIXTURE_RAT.type_id;
/// A Rifter **[V-data]**: the pilot's hull, so the rats have a ship to shoot.
const RIFTER: i64 = 587;
/// `fleet_finder::INVITE_PUBLIC`.
const PUBLIC: i64 = 8;

fn pump_until(c: &mut Conn, within: Duration, mut done: impl FnMut(&mut Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        let _ = c.pump(Duration::from_millis(100));
        if done(c) {
            return true;
        }
    }
    done(c)
}

fn rats(c: &Conn) -> Vec<(i64, f64)> {
    let w = c.world();
    let Some(ego) = w.ego else { return Vec::new() };
    let mut out: Vec<(i64, f64)> = w
        .balls()
        .filter(|b| b.type_id() == Some(RAT))
        .filter_map(|b| Some((b.ball_id, w.surface_distance(ego, b.ball_id)?)))
        .collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

/// **The wing shoots the rat its FC tagged, not the nearer one.**
#[test]
fn a_wing_bot_goes_for_the_rat_its_fc_tagged() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        eve_server::model::Character {
            ship_type_id: RIFTER,
            ..pilot(PILOT, "Pilot", PILOT_SHIP)
        },
        pilot(WING, "Wing1", 100_000_812),
    ];
    let addr = serve(cfg);

    let mut fc = login(addr, "Pilot");
    fc.create_fleet(RIFTER).expect("a fleet");
    let advert = PyValue::Dict(vec![
        (
            PyValue::Int(PUBLIC),
            PyValue::Bytes(b"inviteScope".to_vec()),
        ),
        (
            PyValue::Bool(false),
            PyValue::Bytes(b"joinNeedsApproval".to_vec()),
        ),
        (
            PyValue::Unicode("Wing".to_owned()),
            PyValue::Bytes(b"fleetName".to_vec()),
        ),
    ]);
    fc.call("fleetProxy", "AddFleetFinderAdvert", vec![advert])
        .expect("listed");

    let mut config = BotConfig::new("Wing1", addr, Pace::Fast).with_role(&ROLE);
    config.chat = false;
    config.policy.travel = Some(Arc::new(Map::new()));
    config.policy.quick = true;
    config.policy.accept_fleet = AcceptFleet::Fc(PILOT);
    config.policy.wing = Some(Seat::new(Arc::new(WingHall::new()), Some(PILOT)));
    let bot: Handle = driver::spawn(config).0;

    // He leaves the station far enough that the bot has to come to him.
    let joined = pump_until(&mut fc, Duration::from_secs(240), |c| {
        c.fleet_composition().is_ok_and(|rows| rows.len() == 2)
    });
    assert!(joined, "the bot joins the fleet\n{}", dump(&bot));
    fc.undock().expect("undocks");
    fc.enter_space(SYSTEM).expect("the state");
    gm(&mut fc, "/tp 1000");
    let _ = fc.collect_pushes(Duration::from_millis(500));
    let on_grid = pump_until(&mut fc, Duration::from_secs(240), |c| {
        let w = c.world_now();
        w.ship_of(WING).is_some()
    });
    assert!(on_grid, "the bot comes to his grid\n{}", dump(&bot));

    // Two rats; the farther one is tagged at once.
    gm(&mut fc, &format!("/entity deploy 2 {RAT}"));
    let mut tagged = None;
    let seen = pump_until(&mut fc, Duration::from_secs(30), |c| {
        let r = rats(c);
        if r.len() == 2 && tagged.is_none() {
            tagged = r.last().map(|x| x.0);
        }
        tagged.is_some()
    });
    assert!(seen, "two rats are deployed");
    let (tagged, near) = (tagged.expect("seen"), rats(&fc)[0].0);
    let mark = bot.log().len();
    fc.tag_target(tagged, Some("1")).expect("tags");

    // The bot's first act on a rat after the tag names the tagged one.
    let named = |h: &Handle| {
        h.log()[mark..]
            .iter()
            .find(|e| {
                matches!(e.kind, ActKind::Orbit | ActKind::Lock | ActKind::Fire)
                    && (e.ids.contains(&tagged) || e.ids.contains(&near))
            })
            .map(|e| e.ids.contains(&tagged))
    };
    let first = until(Duration::from_secs(150), || {
        let _ = fc.pump(Duration::from_millis(100));
        named(&bot).is_some()
    });
    let report = dump(&bot);
    assert!(first, "the bot engages a rat after the tag\n{report}");
    assert_eq!(named(&bot), Some(true), "the tagged rat first\n{report}");
    assert!(
        until(Duration::from_secs(150), || {
            let _ = fc.pump(Duration::from_millis(100));
            bot.log()
                .iter()
                .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&tagged))
        }),
        "its guns were on the tagged rat\n{}",
        dump(&bot)
    );
    bot.stop();
}
