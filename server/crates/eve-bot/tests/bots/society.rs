//! SOC-C4 over real sockets: four society bots form two corps, the two
//! corps turn blue toward each other and one of them red toward a human,
//! the CEOs set those contacts at the real corp contact calls, and the two
//! corps merge into one alliance -- every act on the acting bot's own
//! connection.
//!
//! The fights that move the relation matrix are injected through
//! [`Hall::record`] (C5's and C7's seam): a real kill would take minutes of
//! combat. The game-day is a few seconds, so the once-a-day stance rule and
//! the days-blue merge rule run inside the test. Names, ids, careers and
//! traits are invented; no `[A]` value is asserted (the thresholds are
//! named).

use super::common::{config, dump, gm, login, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::corpmind::Hall;
use eve_bot::driver::{self, BotConfig, Handle, Pace, Society};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait};
use eve_bot::society::{self, Affiliation, Event};
use std::sync::Arc;
use std::time::Duration;

const ANN: i64 = 90_000_401;
const BEN: i64 = 90_000_402;
const CAS: i64 = 90_000_403;
const DEE: i64 = 90_000_404;
const MAL: i64 = 90_000_405;
/// A game-day of this many seconds.
const DAY: u64 = 2;

/// Two invented careers, both played by the miner role (which waits
/// docked without a ship): one in the gatherer family, one a ratter.
static TABLE: &[Career] = &[
    Career {
        name: "miner",
        role: Some(&eve_bot::roles::MINE),
        support: Support::Served,
        base: 1.0,
        affinity: [0.0; TRAIT_COUNT],
        band: Band::High,
        prior_isk_h: INCOME_REF,
    },
    Career {
        name: "ratter",
        role: Some(&eve_bot::roles::MINE),
        support: Support::Served,
        base: 1.0,
        affinity: [0.0; TRAIT_COUNT],
        band: Band::High,
        prior_isk_h: INCOME_REF,
    },
];

fn bot(
    name: &str,
    career: &str,
    traits: &[(Trait, f64)],
    hall: &Arc<Hall>,
    addr: std::net::SocketAddr,
) -> (Handle, std::thread::JoinHandle<()>) {
    let now = driver::unix_now();
    let mut mind = Mind::seeded(name, 1, TABLE, &Signals::default(), now);
    mind.career = career.to_owned();
    mind.since = now;
    for t in &mut mind.traits.0 {
        *t = 0.5;
    }
    mind.traits.0[Trait::Loyalty as usize] = 1.0;
    for &(t, v) in traits {
        mind.traits.0[t as usize] = v;
    }
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(hall));
    let mut config = BotConfig::new(name, addr, Pace::Fast).with_society(soc);
    config.chat = false;
    config.policy.gm_setup = false;
    driver::spawn(config)
}

fn corp_lines(h: &Handle) -> Vec<String> {
    h.log()
        .into_iter()
        .filter(|e| e.kind == ActKind::Corp)
        .map(|e| e.what)
        .collect()
}

#[test]
fn four_bots_form_two_corps_an_alliance_and_a_blue_and_a_red() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_401),
        pilot(BEN, "Ben", 100_000_402),
        pilot(CAS, "Cas", 100_000_403),
        pilot(DEE, "Dee", 100_000_404),
        pilot(MAL, "Mal", 100_000_405),
    ];
    let addr = serve(config);
    // The founders' setup, as a human's: the skill and the fees.
    for name in ["Ann", "Cas"] {
        let mut c = login(addr, name);
        gm(&mut c, "/giveisk 3000000000");
        gm(&mut c, "/giveskill Corporation Management 1");
    }

    let hall = Arc::new(Hall::new(DAY).with_every(0));
    let social = (society::FOUND_SOCIAL + 1.0) / 2.0;
    let joiner = (society::JOIN_SOCIAL + 1.0) / 2.0;
    // Driven enough to found, and Ann the more aggressive.
    let drive = [
        (Trait::Sociability, social),
        (Trait::Diligence, 1.0),
        (Trait::Aggression, 1.0),
    ];
    let cas_drive = [
        (Trait::Sociability, social),
        (Trait::Diligence, 1.0),
        (Trait::Aggression, society::FOUND_DRIVE / 2.0),
    ];
    let content = [
        (Trait::Sociability, joiner.min(social)),
        (Trait::Diligence, 0.0),
        (Trait::Aggression, 0.0),
    ];
    let (ann, ta) = bot("Ann", "miner", &drive, &hall, addr);
    let (cas, tc) = bot("Cas", "ratter", &cas_drive, &hall, addr);

    let founded = until(Duration::from_secs(60), || {
        hall.snapshot().bot_corps().len() == 2
    });
    assert!(
        founded,
        "two corps: {:?}\nAnn:\n{}\nCas:\n{}",
        hall.snapshot().corps,
        dump(&ann),
        dump(&cas)
    );
    let st = hall.snapshot();
    let c1 = st.members[&ANN].corp;
    let c2 = st.members[&CAS].corp;
    assert!(society::is_player_corp(c1) && society::is_player_corp(c2) && c1 != c2);

    // The joiners start once both corps exist, so each picks by fit.
    let (ben, tb) = bot("Ben", "miner", &content, &hall, addr);
    let (dee, td) = bot("Dee", "ratter", &content, &hall, addr);
    let joined = until(Duration::from_secs(60), || {
        let st = hall.snapshot();
        st.members.get(&BEN).map(|m| m.corp) == Some(c1)
            && st.members.get(&DEE).map(|m| m.corp) == Some(c2)
    });
    assert!(
        joined,
        "Ben in {c1}, Dee in {c2}: {:?}\nBen:\n{}\nDee:\n{}\nAnn:\n{}",
        hall.snapshot().members,
        dump(&ben),
        dump(&dee),
        dump(&ann)
    );

    // Fights side by side, and Mal killing Ann's corp's pilots, keep
    // coming while the rest happens.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feeder = {
        let (hall, stop) = (Arc::clone(&hall), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                hall.record(&Event::AllyFight { a: c1, b: c2 });
                hall.record(&Event::Kill {
                    victim: c1,
                    killer: MAL,
                    isk: 0.0,
                });
                std::thread::sleep(Duration::from_millis(300));
            }
        })
    };

    let done = until(Duration::from_secs(120), || {
        let st = hall.snapshot();
        let al = st.members.get(&CAS).and_then(|m| m.alliance);
        al.is_some()
            && st.members.get(&ANN).and_then(|m| m.alliance) == al
            && st.books.get(&(c1, c2)).is_some_and(|r| *r > 0.0)
            && st.books.get(&(c1, MAL)).is_some_and(|r| *r < 0.0)
    });
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = feeder.join();
    let st = hall.snapshot();
    for h in [&ann, &ben, &cas, &dee] {
        h.stop();
    }
    for t in [ta, tb, tc, td] {
        let _ = t.join();
    }
    assert!(
        done,
        "an alliance, a blue and a red: alliances {:?} books {:?} stances {:?}\nAnn:\n{:#?}\nCas:\n{:#?}",
        st.alliances,
        st.books,
        st.stances,
        corp_lines(&ann),
        corp_lines(&cas)
    );

    // The contacts were set at the real calls, by the CEOs.
    let ann_lines = corp_lines(&ann);
    assert!(
        ann_lines
            .iter()
            .any(|l| l.starts_with(&format!("contact {c1} -> {c2} +")) && l.ends_with("ok")),
        "{ann_lines:#?}"
    );
    assert!(
        ann_lines
            .iter()
            .any(|l| l.starts_with(&format!("contact {c1} -> {MAL} -")) && l.ends_with("ok")),
        "{ann_lines:#?}"
    );
    assert!(
        !corp_lines(&ben).iter().any(|l| l.starts_with("contact")),
        "only a CEO sets contacts"
    );
    // Both corps, both members each, one alliance.
    assert_eq!(st.size(c1), 2);
    assert_eq!(st.size(c2), 2);
    let al = st.members[&CAS].alliance.expect("in the alliance");
    assert!(society::PLAYER_ALLIANCES.contains(&al));
    assert_eq!(
        st.alliances[&al].executor,
        Some(c1),
        "Ann's corp founded it"
    );
    // Coalition: Ann does not hunt Dee (same alliance), she hunts Mal.
    let aff = |c: i64, corp: i64, al: Option<i64>| Affiliation {
        character_id: c,
        corporation_id: corp,
        alliance_id: al,
    };
    assert!(!hall.hostile_to(aff(ANN, c1, Some(al)), aff(DEE, c2, Some(al))));
    assert!(hall.hostile_to(aff(ANN, c1, Some(al)), aff(MAL, 1_000_044, None)));
    assert!(
        hall.summary().contains("corps 2 alliances 1"),
        "{}",
        hall.summary()
    );
}
