//! X-ICE over a real socket: a system's ice belt is the ice of its security
//! band, read from the Agency the way the client's ice list is.
//!
//! Invented furniture only (the no-client-data rule): three ice dungeons
//! (ids 999 8xx), one per tier, each with an entry beacon and one rock of an
//! invented ice type that carries the client's ice group (465 [V]) and the
//! radius law's two numbers. The tiers are told apart by the **named rule
//! constants** (`dungeon_data::ICE_HIGH_TIER`, `ICE_LOW_TIER`, `ICE_FINER`),
//! never by a literal. The population has one `*` ice row per band, count 1.
//!
//! **No `[A]` value is an expected value here**: the assertion is which
//! dungeon the band draws, never a count, a place or a respawn.

use super::common::{SYSTEM, config, login, pilot, serve, until};
use eve_bot::ice;
use eve_bot::site_runner;
use eve_marshal::PyValue;
use eve_server::celestial::{Band, Universe};
use eve_server::content::Content;
use eve_server::dungeon_data::{
    self, Difficulties, Dungeon, ENTRY_ANOMALY, Entry, ICE_FINER, ICE_HIGH_TIER, ICE_LOW_TIER,
    Object, Resource, ResourceKind, Room, RoomRock,
};
use eve_server::population::{self, Diffs, Row};
use eve_server::typedata::{FIXTURE, Installed, Table};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const MINER: i64 = 90_000_001;
/// Invented: the three dungeons (basic, Glare Crust, finer), their beacon,
/// and the ice type of their one rock.
const HIGH_FIELD: i64 = 999_801;
const LOW_FIELD: i64 = 999_802;
const NULL_FIELD: i64 = 999_803;
const BEACON: i64 = 990_801;
const ICE_ROCK: i64 = 990_802;
/// `groupIce` [V], as the bot names it.
const GROUP_ICE: i64 = ice::GROUP_ICE;
/// `dunArchetypeIceBelt` [V], as the server names it.
const ICE_ARCHETYPE: i64 = eve_server::signal_tracker::ARCHETYPE_ICE;

fn universe(security: f64) -> Universe {
    let row = format!(
        "{SYSTEM}\t19055001\t29055001\t\t\t{security}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    Universe::parse(
        &[row],
        &["19055001\t\t\t\t\tFixture region".to_owned()],
        &["29055001\t19055001\t\t\t\tFixture constellation".to_owned()],
    )
    .expect("the invented rows parse")
}

/// The ice rock: 1,000 m3 a unit, the radius law's two numbers invented.
fn typedata() -> Installed {
    let rock = format!(
        "type\t{ICE_ROCK}\t{GROUP_ICE}\t25\t1\t100\t1000.0\t0.0\t4000.0\t1.0\t0\t\t\
         1980=0.75;1981=600.0\tFixture ice\n"
    );
    Installed::build(Table::parse(&format!("{FIXTURE}\n{rock}")).expect("parses"))
}

/// One ice field holding `ices`: a beacon, one rock.
fn field(id: i64, ices: &[i64]) -> Dungeon {
    let obj = |oid, type_id, x, radius| Object {
        room: 1,
        id: oid,
        type_id,
        pos: [x, 0.0, 0.0],
        radius,
        ..Object::default()
    };
    Dungeon {
        id,
        archetype: ICE_ARCHETYPE,
        difficulty: Some(2),
        entry_type: Some(ENTRY_ANOMALY),
        name_id: Some(id + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![
            obj(1, BEACON, 0.0, None),
            obj(2, ICE_ROCK, 4_000.0, Some(3_000.0)),
        ],
        room_rocks: vec![RoomRock {
            room: 1,
            type_id: ICE_ROCK,
            count: 1,
        }],
        resources: vec![Resource {
            kind: ResourceKind::Ice,
            type_ids: ices.to_vec(),
            ore_band: None,
        }],
        ..Dungeon::default()
    }
}

fn dungeons() -> dungeon_data::Table {
    let all = [
        field(HIGH_FIELD, &ICE_HIGH_TIER),
        field(LOW_FIELD, &[ICE_HIGH_TIER[0], ICE_LOW_TIER]),
        field(NULL_FIELD, &[ICE_LOW_TIER, ICE_FINER[0], ICE_FINER[1]]),
    ];
    dungeon_data::Table {
        dungeons: all
            .into_iter()
            .map(|d| (d.id, d))
            .collect::<BTreeMap<_, _>>(),
        ..dungeon_data::Table::default()
    }
}

/// One `*` ice anomaly a system, in `band`.
fn population(band: Band) -> population::Table {
    let row = Row {
        key: "x_ice".to_owned(),
        band,
        archetypes: vec![ICE_ARCHETYPE],
        entry: Entry::Anomaly,
        count: 1.0,
        difficulties: Diffs::Star,
        respawn_s: 600,
        ttl_s: 172_800,
    };
    population::Table::new(vec![row]).expect("one row")
}

fn start(security: f64, band: Band) -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(MINER, "Miner1", 100_000_001)];
    let mut content = Content::fixture();
    content.insert("universe", universe(security));
    content.insert("typedata", typedata());
    content.insert("dungeon_data", dungeons());
    content.insert("population", population(band));
    config.content = Some(Arc::new(content));
    serve(config)
}

/// The dungeons of the ice belts the home system's Agency lists.
fn ice_fields(addr: SocketAddr) -> Vec<Option<i64>> {
    let mut c = login(addr, "Miner1");
    let mut rows = Vec::new();
    until(Duration::from_secs(30), || {
        let v = c
            .call(ice::AGENCY, ice::ROWS_METHOD, vec![])
            .expect("the Agency answers")
            .value()
            .unwrap_or(PyValue::None);
        rows = site_runner::read_rows(&v, SYSTEM);
        !rows.is_empty()
    });
    let counts = c
        .call(ice::AGENCY, ice::COUNT_METHOD, vec![])
        .expect("the Agency counts")
        .value()
        .unwrap_or(PyValue::None);
    assert!(
        site_runner::read_counts(&counts)
            .get(&SYSTEM)
            .is_some_and(|n| *n as usize == rows.len()),
        "the count agrees with the rows"
    );
    rows.iter().map(|r| r.dungeon_id).collect()
}

/// **X-ICE.** A high-sec system's ice belt is the basic-ice field, a
/// low-sec system's the Glare Crust field and a null-sec system's the finer
/// ice, never a neighbour band's.
#[test]
fn a_systems_ice_belt_is_the_ice_of_its_band() {
    for (security, band, want) in [
        (0.9, Band::High, HIGH_FIELD),
        (0.3, Band::Low, LOW_FIELD),
        (-0.5, Band::Null, NULL_FIELD),
    ] {
        let fields = ice_fields(start(security, band));
        assert!(
            !fields.is_empty() && fields.iter().all(|d| *d == Some(want)),
            "{band:?}: want only dungeon {want}, got {fields:?}"
        );
    }
}

/// The unused-tier check, so a helper is not dead: the dungeons are told
/// apart by `Difficulties::Star`, the rule `population.tsv` rows use.
#[test]
fn the_fields_are_told_apart_by_star() {
    use eve_server::dungeon_data::{Exclusions, OreBand, Slot, candidates};
    let t = dungeons();
    let pick = |b| {
        let slot = Slot {
            archetypes: &[ICE_ARCHETYPE],
            entry: Entry::Anomaly,
            difficulties: Difficulties::Star(b),
            faction: None,
        };
        candidates(&t, &slot, &Exclusions::C3, &|_| None)
    };
    assert_eq!(pick(OreBand::High), [HIGH_FIELD]);
    assert_eq!(pick(OreBand::Low), [LOW_FIELD]);
    assert_eq!(pick(OreBand::Null), [NULL_FIELD]);
}
