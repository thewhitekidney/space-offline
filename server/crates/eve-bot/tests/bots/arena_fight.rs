//! DOC-9 over a real socket: **the arena**. Two fleets of two doctrines of
//! equal spend fight in the real combat engine, and the doctrine the
//! matrix names as the counter wins.
//!
//! The matchup is stacked on purpose: one invented hull whose three layers
//! resist EM to a fifth and nothing else; doctrine EM fires an EM round,
//! doctrine KIN a kinetic one, the same guns and the same spend. The paper
//! (`profile_of` over the same rows, `doctrine_exchange`) says KIN is the
//! counter of EM; the engine must agree on every seed, with the ISK ratio
//! above 1.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s autocannon, an invented
//! hull and two invented rounds. No `[A]` value is an expected value.

use eve_bot::arena::{self, Fleet, Rig, Side, Tally};
use eve_bot::doctrine::{Doctrine, Family, Layer, Makeable, Range, Role, Stance, fleet_size};
use eve_bot::doctrine_exchange::{Features, matrix};
use eve_bot::doctrine_fit::{Library, Limits, profile_of};
use eve_bot::hull_choice::Class;
use std::sync::Arc;
use std::time::Duration;

/// An invented autocannon: the fixture's, with a 10 km optimal so the
/// matrix's bands (3, 15 and 45 km) are inside its reach.
const GUN: i64 = 990_990_002;
const HULL: i64 = 990_990_001;
const KIN_ROUND: i64 = 990_990_011;
const EM_ROUND: i64 = 990_990_012;
const ROUNDS: u32 = 50;
const GUNS: u8 = 2;

fn rows() -> String {
    // The fixture frigate's numbers, with EM resonance 0.2 in all three
    // layers (the others 1.0).
    format!(
        "{}type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;76=21000.0;\
         101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;267=0.2;268=1.0;269=1.0;270=1.0;\
         271=0.2;272=1.0;273=1.0;274=1.0;113=0.2;479=600000.0;482=260.0;564=640.0;600=5.0;\
         1137=3.0;1281=1.0\tInvented Arena Hull\n\
         type\t{GUN}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
         6=0.0;30=2.0;50=4.0;51=2500.0;54=10000.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=83.0\tInvented Long Autocannon\n\
         type\t{KIN_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=40.0;128=1.0\tInvented Kinetic Round\n\
         type\t{EM_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t114=40.0;128=1.0\tInvented EM Round\n",
        eve_server::typedata::FIXTURE
    )
}

fn table() -> eve_server::typedata::Table {
    eve_server::typedata::Table::parse(&rows()).expect("the invented rows parse")
}

/// A one-role doctrine on the invented hull firing `round`, its profile
/// from the paper evaluation of the same rows.
fn doctrine(lib: &Library<'_>, key: &str, round: i64) -> Doctrine {
    let modules = vec![(GUN, GUNS)];
    let profile = profile_of(lib, HULL, &modules, Some(round), &[]).expect("the fit evaluates");
    println!(
        "{key}: dps={:?} delivery={:?} ehp={:?} speed={} sig={}",
        profile.dps,
        profile.delivery,
        profile.ehp(),
        profile.speed,
        profile.sig
    );
    assert!(profile.total_dps() > 0.0, "the guns do damage on paper");
    Doctrine {
        key: key.to_owned(),
        class: Class::Frigate,
        family: Family::Projectile,
        layer: Layer::Shield,
        active: false,
        range: Range::Short,
        stance: Stance::Brawl,
        roles: vec![(
            Role::Dps,
            1.0,
            eve_bot::doctrine::RoleFit {
                hull: HULL,
                modules,
                charges: vec![(round, ROUNDS)],
                skills: Vec::new(),
                profile,
                cost: 100.0,
            },
        )],
        fleet: fleet_size(Class::Frigate),
    }
}

#[test]
fn arena_fight_socket() {
    // The paper: KIN is the counter of EM.
    let inst = eve_server::typedata::Installed::build(table());
    let lib = Library::build(&inst, &Makeable::all(), &|_| 1.0, Limits::default());
    let cat = vec![
        doctrine(&lib, "em", EM_ROUND),
        doctrine(&lib, "kin", KIN_ROUND),
    ];
    let m = matrix(&cat, Features::default());
    let (counter, er) = arena::counter_of(&m, 0).expect("a counter for EM");
    assert_eq!(counter, 1, "the matrix names KIN against EM");
    assert!(er > 0.0, "and says it wins: {er}");

    // The engine: the same two fits, equal spend, over seeds.
    let mut content = eve_server::content::Content::fixture();
    content.insert("typedata", eve_server::typedata::Installed::build(table()));
    arena::set_trace(true);
    let mut rig = Rig::new(Arc::new(content));
    rig.sep_m = 3_000.0;
    rig.limit = Duration::from_secs(90);
    let (na, nb) = arena::fielded(&cat[0], &cat[1], 2);
    assert_eq!((na, nb), (2, 2), "equal spend, equal fleets");
    let outcomes: Vec<_> = std::thread::scope(|s| {
        let jobs: Vec<_> = (1..=2u64)
            .map(|seed| {
                let (rig, cat) = (&rig, &cat);
                s.spawn(move || {
                    arena::skirmish(
                        rig,
                        Fleet {
                            doctrine: &cat[0],
                            n: na,
                        },
                        Fleet {
                            doctrine: &cat[1],
                            n: nb,
                        },
                        seed,
                    )
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| j.join().expect("no panic").expect("the skirmish runs"))
            .collect()
    });
    for o in &outcomes {
        println!("{}", arena::skirmish_line("em", "kin", 0, o));
        assert!(!o.timed_out, "a small fight resolves: {o:?}");
        assert_eq!(o.winner, Some(Side::B), "the counter wins: {o:?}");
        assert_eq!(o.lost[0], 2, "EM's fleet is gone: {o:?}");
        assert!(o.isk_lost[0] > o.isk_lost[1], "{o:?}");
        assert!(o.secs > 0.0);
    }
    let t = Tally::of(&outcomes, Side::B);
    assert!(t.counter_works(), "{t:?}");
    assert_eq!(t.wins_predicted, 2);
    let line = arena::pair_line("em", "kin", er, true, &t);
    println!("{line}");
    assert!(
        line.contains("predicted=B") && line.contains("wins_predicted=2"),
        "{line}"
    );
}
