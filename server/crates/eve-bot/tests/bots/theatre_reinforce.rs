//! DOC-4 over a real socket: a theatre's call for backup goes out as the
//! client's own fleet broadcast, a blue's bot answers it, and the hall turns
//! the arrival into standing.
//!
//! Carol is the fleet commander of corp `C_ASK` (a hall member and CEO).
//! The hall queues a `NeedBackup` for her, her connection runs it
//! (`Director::run`, `fleet::orders_for`: `BroadcastToBubble`), and Botty,
//! the CEO of the blue corp `C_HELP`, warps to her (R-REL-3's path). The
//! test then reports Botty on Carol's grid; a battle tick settles the call:
//! both views of the pair rise. A second blue, `C_IDLE`, had the means and
//! the standing and never came: after `HELP_WAIT_S` Carol's view of it falls
//! and its own view of her does not.
//!
//! Invented ids; thresholds are the named consts, assertions are about signs.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::career::{Band, CAREERS, Signals};
use eve_bot::corpmind::{CorpMind, Hall, Member};
use eve_bot::doctrine_exchange::Features;
use eve_bot::doctrine_hall::{DoctrineWorld, Readiness};
use eve_bot::doctrine_sense::{Board, Lens};
use eve_bot::doctrine_theatre::{Action, HELP_AT, HELP_WAIT_S};
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::fleet::Director;
use eve_bot::mind::{Mind, TRAIT_COUNT, Traits};
use eve_bot::society::{self, Held5, Profile, Satisfaction, Stance5};
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const BOT: i64 = 90_000_905;
const CAROL: i64 = 90_000_903;
const IDLE: i64 = 90_000_907;
const C_ASK: i64 = 98_000_903;
const C_HELP: i64 = 98_000_905;
const C_IDLE: i64 = 98_000_907;
const RIFTER: i64 = 587;
const DAY: u64 = 60;

fn heard(h: &driver::Handle, who: i64) -> bool {
    h.log()
        .iter()
        .any(|e| e.kind == ActKind::Corp && e.what.starts_with(&format!("help {who}")))
}

fn member(name: &str, corp: i64, last_turn: u64) -> Member {
    Member {
        name: name.to_owned(),
        traits: Traits([0.7; TRAIT_COUNT]),
        career: "ratter".to_owned(),
        band: Band::High,
        corp,
        alliance: None,
        last_turn: Some(last_turn),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

fn corp(id: i64, ceo: i64) -> CorpMind {
    CorpMind {
        id,
        ceo: Some(ceo),
        profile: Profile::of("ratter", Band::High),
        founded: 0,
        alliance: None,
        name: format!("Invented {id}"),
        applied_alliance: None,
        salt: 0,
    }
}

#[test]
fn theatre_reinforce_socket() {
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(BOT, "Botty", 100_000_905),
        Character {
            ship_type_id: RIFTER,
            ..pilot(CAROL, "Carol", 100_000_903)
        },
    ];
    let addr: SocketAddr = serve(c);
    let t0 = driver::unix_now();

    // The hall: three bot corps, the doctrine layer awake with an empty
    // catalog (this test is about the call, not the choice).
    let hall = Arc::new(Hall::new(DAY).with_every(3_600_000).with_doctrine(
        Arc::new(DoctrineWorld::new(
            Vec::new(),
            Lens::default(),
            Features::default(),
        )),
        2,
        7,
    ));
    {
        let mut st = hall.lock();
        // The obedience dice are keyed by the battle tick, which comes from
        // the wall clock (`t0`); a level's p_obey below 1 made the idle
        // ally's `Avail` vanish on some ticks. Everyone obeys here.
        st.knobs.theatre.p_obey = 1.0;
        for (id, name, c) in [
            (CAROL, "Carol", C_ASK),
            (BOT, "Botty", C_HELP),
            (IDLE, "Idler", C_IDLE),
        ] {
            st.members.insert(id, member(name, c, t0));
            st.corps.insert(c, corp(c, id));
        }
        // Both blues are C_ASK's friends and view it as one.
        for blue in [C_HELP, C_IDLE] {
            st.stances.graded.insert(
                (C_ASK, blue),
                Held5 {
                    stance: Stance5::Friendly,
                    since: Some(0),
                },
            );
            st.relations.add(blue, C_ASK, HELP_AT + 1.0);
        }
        // The bot's own decision reads R_eff toward Carol (R-REL-3).
        st.friends.insert(CAROL, 5.0);
    }

    // Botty: a society escort, in Carol's fleet.
    let mut mind = Mind::seeded("Botty", 1, CAREERS, &Signals::default(), t0);
    mind.career = "ratter".to_owned();
    mind.since = t0;
    let mut soc = Society::new(mind, CAREERS, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(&hall));
    let mut cfg = BotConfig::new("Botty", addr, Pace::Fast).with_society(soc);
    cfg.chat = false;
    cfg.policy.accept_fleet = AcceptFleet::Any;
    let (bot, join) = driver::spawn(cfg);
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Undock))),
        "the bot sets itself up and undocks:\n{}",
        dump(&bot)
    );

    let mut boss = login(addr, "Carol");
    boss.undock().expect("Carol undocks");
    boss.enter_space(SYSTEM).expect("Carol's state");
    let mut director = Director::new(boss, RIFTER);
    assert!(
        director
            .form_fleet(&[BOT], Duration::from_secs(60))
            .expect("the fleet forms"),
        "the bot joined:\n{}",
        dump(&bot)
    );

    // The theatre asks for backup: the hall queues it for the commander ...
    hall.lock().enqueue(
        CAROL,
        C_ASK,
        SYSTEM,
        t0,
        Action::NeedBackup { system: SYSTEM },
    );
    let acts = hall.take_directives(CAROL);
    assert_eq!(acts, vec![Action::NeedBackup { system: SYSTEM }]);
    assert_eq!(hall.snapshot().calls.len(), 1, "and opens the call");
    // ... her connection sends it as the client's own broadcast ...
    director.run(&acts).expect("the broadcast is relayed");
    // ... and the blue's bot comes.
    let came = until(Duration::from_secs(20), || heard(&bot, CAROL));
    bot.stop();
    let _ = join.join();
    assert!(came, "a blue's call is answered:\n{}", dump(&bot));

    // Botty is on Carol's grid after the call: the next tick settles it.
    hall.report_readiness(
        BOT,
        Readiness {
            system: Some(SYSTEM),
            at: t0 + 5,
            ..Readiness::default()
        },
    );
    let board = Board::default();
    let early = hall.theatre_tick(t0 + 10, &board);
    assert!(
        early.iter().any(|l| l.contains("Reinforced")),
        "the arrival is recorded: {early:?}"
    );
    let st = hall.snapshot();
    assert!(
        st.relations.get(C_ASK, C_HELP) > 0.0,
        "the helped trusts the helper"
    );
    assert!(
        st.relations.get(C_HELP, C_ASK) > HELP_AT + 1.0,
        "and the helper the helped"
    );
    assert_eq!(st.relations.get(C_ASK, C_IDLE), 0.0, "the wait is not over");

    // The other blue never came: after the wait only the asker's view falls.
    let late = hall.theatre_tick(t0 + HELP_WAIT_S + 30, &board);
    assert!(
        late.iter().any(|l| l.contains("Abandoned")),
        "the absence is recorded: {late:?}"
    );
    let st = hall.snapshot();
    assert!(st.relations.get(C_ASK, C_IDLE) <= -society::K_ABANDON + 1e-9);
    assert_eq!(st.relations.get(C_IDLE, C_ASK), HELP_AT + 1.0);
}
