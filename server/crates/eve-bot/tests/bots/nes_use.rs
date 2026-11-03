//! L7 BOT-ECON over a real socket (issue #34): the docked turn of the new
//! economy against the in-process server, with **invented rows only**: one
//! SKIN licence that fits the hull the pilot flies, one store with one offer
//! for it, no vault PLEX (nothing in the socket path credits a vault, so the
//! purchase itself is a pure-rule test in `nes_use`).
//!
//! What this proves is the wire: that the bot's `get_offers` reader parses
//! the server's own answer, that a licence in the hangar is activated and
//! applied through `shipCosmeticsMgr`, that a refused crate is noted and the
//! turn goes on, and that a bot with no PLEX buys nothing. Both ends marshal
//! with `eve-marshal`, so this is behaviour, not bytes.

use super::common::{config, gm, login, pilot, serve};
use eve_bot::geode_haul::Table;
use eve_bot::nes_use::{self, Ctx, State};
use eve_server::content::Content;
use eve_server::nes::{Catalogue, Category};
use eve_server::skins::Skins;
use std::net::SocketAddr;
use std::sync::Arc;

const PILOT: i64 = 90_000_911;
/// The hull the licence fits: `merlin`'s type, the fixture's frigate.
const HULL: i64 = 603;
const LICENCE: i64 = 990_911;
const SKIN: i64 = 9_911;
const OFFER: i64 = 9_911_001;
const PRICE: i64 = 40;

fn skins() -> Skins {
    let lic = [format!(
        "{LICENCE}\t991\t{SKIN}\t-1\t0\t0.01\tFixture Licence"
    )];
    let skn = [format!("{SKIN}\t1\t0\t{HULL}")];
    Skins::parse(&lic, &skn).expect("fixture rows")
}

fn catalogue() -> Catalogue {
    let offers = [format!(
        "skin_fix\t{OFFER}\tskins/frigate\tFixture Licence\tA licence.\t{PRICE}\t{PRICE}\t1\t{LICENCE}\t1\t"
    )];
    let types = [
        format!(
            "{}\t90001\t90002\t0\t0.0002\tFixture PLEX",
            eve_server::nes_vault::TYPE_PLEX
        ),
        format!("{LICENCE}\t991\t91\t0\t0.01\tFixture Licence"),
    ];
    let cats = vec![
        Category {
            path: "skins".into(),
            name: "SKINs".into(),
        },
        Category {
            path: "skins/frigate".into(),
            name: "Frigate SKINs".into(),
        },
    ];
    Catalogue::parse(&offers, &types, cats).expect("fixture rows")
}

fn start() -> SocketAddr {
    let mut cfg = config();
    cfg.sandbox.characters = vec![pilot(PILOT, "Skinner", 100_000_911)];
    let mut c = Content::fixture();
    c.insert("skins", skins());
    c.insert("nes", catalogue());
    cfg.content = Some(Arc::new(c));
    serve(cfg)
}

fn table() -> Table {
    Table::with_skins(skins())
}

/// A pilot flying a merlin, a licence for it in the hangar.
fn pilot_with_licence(addr: SocketAddr) -> eve_bot::Conn {
    let mut c = login(addr, "Skinner");
    gm(&mut c, "/createitem merlin");
    let ship = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == HULL)
        .expect("the merlin");
    c.board(ship.item_id).expect("boards");
    gm(&mut c, &format!("/createitem {LICENCE}"));
    c
}

fn run(c: &mut eve_bot::Conn, st: &mut State, table: &Table) -> String {
    let mut desk = eve_bot::price_memory::Desk::default();
    let mut cx = Ctx {
        table,
        content: None,
        map: None,
        trader: false,
        desk: &mut desk,
    };
    nes_use::turn(c, st, &mut cx).expect("the turn runs")
}

#[test]
fn a_licence_in_the_hangar_is_activated_and_worn_once() {
    let addr = start();
    let mut c = pilot_with_licence(addr);
    let mut st = State::default();
    let t = table();
    let note = run(&mut c, &mut st, &t);
    assert!(note.contains("wearing SKIN"), "{note}");
    assert_eq!(st.skinned.len(), 1);
    // The licence is spent: the next turn finds nothing to wear.
    let again = run(&mut c, &mut st, &t);
    assert!(!again.contains("wearing SKIN"), "{again}");
}

#[test]
fn the_stores_answer_is_read_and_a_bot_without_plex_buys_nothing() {
    let addr = start();
    let mut c = login(addr, "Skinner");
    let v = c
        .call(
            "storeManager",
            "get_offers",
            vec![eve_marshal::PyValue::Int(1)],
        )
        .expect("answered")
        .value()
        .expect("a value");
    let offers = nes_use::offers_of(&v);
    assert_eq!(offers.len(), 1, "{v:?}");
    assert_eq!((offers[0].price, offers[0].type_id), (PRICE, LICENCE));
    assert!(!offers[0].can_purchase, "the vault is empty");
    assert_eq!(nes_use::vault_balance(&mut c).expect("balance"), 0);
    assert!(nes_use::skin_offer(&table(), &offers, HULL, 0, 0).is_none());
}

#[test]
fn a_refused_crate_is_noted_and_the_turn_goes_on() {
    let addr = start();
    let mut c = login(addr, "Skinner");
    gm(&mut c, "/createitem merlin");
    // The merlin stands in for a geode type: the server says it is no crate.
    let t = Table::with_geodes(&[(HULL, 1)]);
    let mut st = State::default();
    let note = run(&mut c, &mut st, &t);
    assert!(note.contains("open geode"), "{note}");
    assert_eq!(st.opened, 0);
}
