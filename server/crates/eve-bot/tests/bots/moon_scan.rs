//! **MN-D over a real socket**: a pilot fits a Survey Probe Launcher, loads a
//! survey probe, points the ship at a moon and activates the launcher with the
//! calls the client's own HUD makes (`dogmaIM.Activate`); the server tells the
//! Moon Analysis window about the probe (`OnNewMoonProbe`) and, when its time
//! is up, what the moon is made of (`OnMoonScanComplete`) and
//! that the probe is gone (`OnRemoveMoonProbe`). The same launcher refuses a
//! pilot without the probe's skill.
//!
//! **Synthetic furniture only** (the no-client-data rule): the home system is
//! an invented null-sec row with one invented moon, the ores are invented
//! types in the client's own moon ore groups (1884 and 1923 [V],
//! `inventorycommon/const.py:1160-1164`), the launcher and probe are invented
//! rows in the client's groups 1226 and 492 [V] with the attributes the server
//! reads (`chargeGroup2` 605, `explosionDelay` 281, `maxScanRange` 2697), the
//! skill an invented row. **No `[A]` value is an expected value here**: the
//! assertions are that the ores named are the moon's table, that the shares
//! sum to one and that the order of pushes is the client's.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::moon::{GROUP_EXCEPTIONAL, GROUP_UBIQUITOUS};
use eve_server::moon_scan::{GROUP_SURVEY_LAUNCHER, GROUP_SURVEY_PROBE};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const SCANNER: i64 = 90_000_001;
const MOON: i64 = 40_000_777;
const ORE_A: i64 = 990_621;
const ORE_B: i64 = 990_622;
const LAUNCHER: i64 = 990_623;
const PROBE: i64 = 990_624;
const SKILL: i64 = 990_625;
const SKILL_GROUP: i64 = 990_020_100;
/// `typedata::FIXTURE`'s frigate: three high slots.
const FRIGATE: i64 = 990_000_001;
/// The probe's invented life, milliseconds: a flight runs on real time.
const DELAY_MS: i64 = 2_000;

fn universe() -> eve_server::celestial::Universe {
    let row = format!(
        "{SYSTEM}\t19062001\t29062001\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    eve_server::celestial::Universe::parse(
        &[row],
        &["19062001\t\t\t\t\tFixture region".to_owned()],
        &["29062001\t19062001\t\t\t\tFixture constellation".to_owned()],
    )
    .expect("the invented rows parse")
}

fn typedata() -> eve_server::typedata::Installed {
    let ore = |id: i64, group: i64| {
        format!(
            "type\t{id}\t{group}\t25\t1\t100\t10.0\t0.0\t4000.0\t1.0\t0\t\t\
             182=3386.0;1980=0.75;1981=600.0;2699=1.0;2711={id}.0;2727=5000.0\tFixture ore {id}\n"
        )
    };
    let text = format!(
        "{}\n{}{}\
         effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t\n\
         type\t{LAUNCHER}\t{GROUP_SURVEY_LAUNCHER}\t7\t1\t1\t5.0\t10.0\t0.0\t1.0\t101\t12,16,101\t51=2000.0;56=1.0;605={GROUP_SURVEY_PROBE}.0\tFixture Survey Launcher\n\
         type\t{PROBE}\t{GROUP_SURVEY_PROBE}\t8\t1\t1\t5.0\t0.0\t1.0\t30.0\t0\t\t182={SKILL}.0;277=1.0;37=30000.0;281={DELAY_MS}.0;2697=100000000.0\tFixture Survey Probe\n",
        eve_server::typedata::FIXTURE,
        ore(ORE_A, GROUP_UBIQUITOUS),
        ore(ORE_B, GROUP_EXCEPTIONAL),
    );
    eve_server::typedata::Installed::build(
        eve_server::typedata::Table::parse(&text).expect("parses"),
    )
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![pilot(SCANNER, "Scanner1", 100_000_001)];
    // A moon 3,000 km east of the station at the origin, 1,000 km across.
    c.sandbox.celestials = eve_server::celestial::Catalogue::parse(&format!(
        "moon\t{MOON}\t990021\t1000000.0\t3000000.0\t0.0\t0.0\t{SYSTEM}\n"
    ));
    let skills = eve_server::skill::Table::parse(&format!(
        "{}skill\t{SKILL}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tFixture Astrometrics\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skills parse");
    let mut content = Content::fixture();
    content.insert("universe", universe());
    content.insert("typedata", typedata());
    content.insert(eve_server::skill::TABLE.name, skills);
    c.content = Some(Arc::new(content));
    serve(c)
}

/// Outfitted and in space, heading for the moon: the launcher's item id.
fn ready(addr: SocketAddr, skill: bool) -> (Conn, i64) {
    let mut c = login(addr, "Scanner1");
    if skill {
        gm(&mut c, &format!("/giveskill {SKILL} 1"));
    }
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(&mut c, &format!("/createitem {PROBE} 2"));
    let station = c.session().station_id().expect("docked");
    let hangar = c.hangar_items().expect("the hangar lists");
    let find = |t: i64| {
        hangar
            .iter()
            .find(|r| r.type_id == t)
            .expect("made")
            .item_id
    };
    c.board(find(FRIGATE)).expect("boards");
    let launcher = c.fit_auto(&[find(LAUNCHER)]).expect("fits")[0];
    c.set_online(launcher.item_id).expect("onlines");
    c.load_ammo(
        &[(launcher.item_id, launcher.flag_id)],
        &[find(PROBE)],
        PROBE,
        station,
    )
    .expect("loads the probe");
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state");
    c.goto_direction([1.0, 0.0, 0.0]).expect("aims");
    c.set_speed_fraction(1.0).expect("speeds up");
    // Let the ship pick up speed along the aim, so its velocity names it.
    let _ = c.pump(Duration::from_secs(3));
    (c, launcher.item_id)
}

fn names(seen: &[(String, Vec<PyValue>)]) -> Vec<&str> {
    seen.iter().map(|(n, _)| n.as_str()).collect()
}

/// Pushes until `want` has been seen or the deadline passes.
fn until_seen(c: &mut Conn, want: &str, seen: &mut Vec<(String, Vec<PyValue>)>) -> bool {
    until(Duration::from_secs(60), || {
        let _ = c.pump(Duration::from_millis(300));
        seen.extend(c.take_pushes());
        seen.iter().any(|(n, _)| n == want)
    })
}

fn field(kv: &PyValue, name: &str) -> Option<PyValue> {
    let PyValue::Instance { state, .. } = kv else {
        return None;
    };
    eve_server::pyutil::dict_get(state, name.as_bytes()).cloned()
}

/// **The packet's test**: activate the launcher; the probe is announced with
/// the fields the client's tracker reads; and when its time is up the window
/// is told the moon's makeup, then that the probe is gone.
#[test]
fn a_probe_scans_a_moon_and_the_window_is_told_what_it_is_made_of() {
    let addr = start();
    let (mut c, launcher) = ready(addr, true);
    c.keep_pushes();
    c.activate(launcher, ("useMissiles", 101), None, 1)
        .expect("the launcher cycles");
    let mut seen = Vec::new();
    assert!(
        until_seen(&mut c, "OnMoonScanComplete", &mut seen),
        "the scan completes: {:?}",
        names(&seen)
    );
    // Give the removal its moment too.
    let _ = until_seen(&mut c, "OnRemoveMoonProbe", &mut seen);

    let probe = seen
        .iter()
        .find(|(n, _)| n == "OnNewMoonProbe")
        .map(|(_, a)| a[0].clone())
        .expect("the probe is announced");
    assert_eq!(field(&probe, "typeID"), Some(PyValue::Int(PROBE)));
    let Some(PyValue::Int(probe_id)) = field(&probe, "probeID") else {
        panic!("a probe id: {probe:?}");
    };
    assert_eq!(field(&probe, "delay"), Some(PyValue::Int(DELAY_MS)));

    let order = names(&seen);
    let at = |n: &str| order.iter().position(|x| *x == n);
    assert!(at("OnNewMoonProbe") < at("OnMoonScanComplete"), "{order:?}");
    assert!(
        at("OnMoonScanComplete") < at("OnRemoveMoonProbe"),
        "{order:?}"
    );

    let (_, done) = seen
        .iter()
        .find(|(n, _)| n == "OnMoonScanComplete")
        .expect("seen");
    assert_eq!(done[0], PyValue::Int(MOON), "the moon it flew at");
    let PyValue::Dict(rows) = &done[1] else {
        panic!("a {{typeID: fraction}} dict: {:?}", done[1]);
    };
    assert!(!rows.is_empty());
    let mut total = 0.0;
    for (v, k) in rows {
        assert!(
            matches!(k, PyValue::Int(t) if *t == ORE_A || *t == ORE_B),
            "an ore of the moon's table: {k:?}"
        );
        let PyValue::Real(f) = v else {
            panic!("a fraction: {v:?}")
        };
        assert!((0.0..=1.0).contains(f));
        total += f;
    }
    assert!((total - 1.0).abs() < 1e-9, "the shares sum to one: {total}");
    let (_, gone) = seen
        .iter()
        .find(|(n, _)| n == "OnRemoveMoonProbe")
        .expect("the probe is removed");
    assert_eq!(gone[0], PyValue::Int(probe_id));
}

/// The probe's skill is the server's to check: without it the launcher is
/// refused and no probe leaves.
#[test]
fn a_launcher_without_the_probes_skill_is_refused() {
    let addr = start();
    let (mut c, launcher) = ready(addr, false);
    c.keep_pushes();
    let refused = c.activate(launcher, ("useMissiles", 101), None, 1);
    assert!(refused.is_err(), "no skill, no probe: {refused:?}");
    let _ = c.pump(Duration::from_secs(2));
    let seen = c.take_pushes();
    assert!(
        !names(&seen).contains(&"OnNewMoonProbe"),
        "{:?}",
        names(&seen)
    );
}
