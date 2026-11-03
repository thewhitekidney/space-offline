//! **CO-FOUND over real sockets**: three corpless society bots pool their
//! ISK. The two poorer pledge to the most diligent with real `GiveCash`
//! transfers, it founds a corp with the pool, and the pledgers apply and
//! are accepted. A second pool that cannot found (no Corporation
//! Management) is refunded at its deadline, with real transfers.
//!
//! Names, ids and wallets are invented; the amounts come from the named
//! consts (`cofound::PLEDGE_*`, `society::FOUND_RESERVE`), no `[A]` value
//! is asserted.

use super::common::{config, dump, gm, login, pilot, serve, until};
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::cofound::{self, Kind};
use eve_bot::corpmind::{CORP_FEE, Hall};
use eve_bot::driver::{self, BotConfig, Handle, Pace, Society};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait};
use eve_bot::society;
use std::sync::Arc;
use std::time::Duration;

const ANN: i64 = 90_000_501;
const BEN: i64 = 90_000_502;
const CAS: i64 = 90_000_503;
/// A game-day of this many seconds.
const DAY: u64 = 2;

static TABLE: &[Career] = &[Career {
    name: "miner",
    role: Some(&eve_bot::roles::MINE),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::High,
    prior_isk_h: INCOME_REF,
}];

fn bot(
    name: &str,
    diligence: f64,
    hall: &Arc<Hall>,
    addr: std::net::SocketAddr,
) -> (Handle, std::thread::JoinHandle<()>) {
    let now = driver::unix_now();
    let mut mind = Mind::seeded(name, 1, TABLE, &Signals::default(), now);
    mind.career = "miner".to_owned();
    mind.since = now;
    for t in &mut mind.traits.0 {
        *t = 0.5;
    }
    mind.traits.0[Trait::Loyalty as usize] = 1.0;
    mind.traits.0[Trait::Sociability as usize] = (society::FOUND_SOCIAL + 1.0) / 2.0;
    mind.traits.0[Trait::Aggression as usize] = 1.0;
    mind.traits.0[Trait::Diligence as usize] = diligence;
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(hall));
    let mut config = BotConfig::new(name, addr, Pace::Fast).with_society(soc);
    config.chat = false;
    config.policy.gm_setup = false;
    driver::spawn(config)
}

fn server() -> std::net::SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_501),
        pilot(BEN, "Ben", 100_000_502),
        pilot(CAS, "Cas", 100_000_503),
    ];
    serve(config)
}

/// The founder's own part, and each pledger's: neither pledger can give
/// the whole rest alone (its excess is 60 % of it), two cover it.
fn fund(addr: std::net::SocketAddr, skill: bool) {
    let line = CORP_FEE + society::FOUND_RESERVE;
    let own = (line * 0.4).ceil() as i64;
    let rest = line - own as f64;
    let wallet = cofound::PLEDGE_FLOOR + (rest * 0.6).ceil();
    for (name, isk) in [("Ann", own), ("Ben", wallet as i64), ("Cas", wallet as i64)] {
        let mut c = login(addr, name);
        gm(&mut c, &format!("/giveisk {isk}"));
        if skill && name == "Ann" {
            gm(&mut c, "/giveskill Corporation Management 1");
        }
    }
}

#[test]
fn three_bots_pledge_one_founds_and_two_join() {
    let addr = server();
    fund(addr, true);
    let hall = Arc::new(Hall::new(DAY).with_every(0));
    let (ann, ta) = bot("Ann", 1.0, &hall, addr);
    // Ann is the only candidate until the others arrive: nobody pledges.
    // Wait until the hall has seen her: the others log in on their own
    // threads, and one that registered first would be chosen founder (the
    // pool keeps the founder once pledges stand), which is a different
    // test. The settling wait in `cofound::step` covers the same race for
    // real bots; this covers a login slower than it.
    assert!(
        until(Duration::from_secs(60), || hall
            .snapshot()
            .members
            .contains_key(&ANN)),
        "Ann never registered:\n{}",
        dump(&ann)
    );
    let (ben, tb) = bot("Ben", 0.9, &hall, addr);
    let (cas, tc) = bot("Cas", 0.8, &hall, addr);
    let done = until(Duration::from_secs(90), || {
        let st = hall.snapshot();
        let corp = st.members.get(&ANN).map(|m| m.corp);
        corp.is_some_and(society::is_player_corp)
            && st.pool.pledges.is_empty()
            && st.members.get(&BEN).map(|m| m.corp) == corp
            && st.members.get(&CAS).map(|m| m.corp) == corp
    });
    let st = hall.snapshot();
    for h in [&ann, &ben, &cas] {
        h.stop();
    }
    for t in [ta, tb, tc] {
        let _ = t.join();
    }
    assert!(
        done,
        "members {:?}\npool {:?}\nAnn:\n{}\nBen:\n{}\nCas:\n{}",
        st.members,
        st.pool,
        dump(&ann),
        dump(&ben),
        dump(&cas)
    );
    let pledges: Vec<_> = st
        .pool
        .journal
        .iter()
        .filter(|e| e.kind == Kind::Pledge)
        .collect();
    assert!(pledges.iter().all(|e| e.to == ANN && e.from != ANN));
    for pledger in [BEN, CAS] {
        assert!(
            pledges.iter().any(|e| e.from == pledger),
            "{:?}",
            st.pool.journal
        );
    }
    // Everything pledged was spent into the corp or paid back.
    let sum = |k: Kind| -> f64 {
        st.pool
            .journal
            .iter()
            .filter(|e| e.kind == k)
            .map(|e| e.amount)
            .sum()
    };
    assert_eq!(
        sum(Kind::Pledge),
        sum(Kind::Spent) + sum(Kind::Refund) + sum(Kind::RefundShort),
        "{:#?}",
        st.pool.journal
    );
    assert!(
        st.acts.iter().any(|a| a.contains("pledged")),
        "{:?}",
        st.acts
    );
}

#[test]
fn a_pool_that_cannot_found_is_refunded_at_the_deadline() {
    let addr = server();
    // No Corporation Management for Ann: the pool fills and cannot found.
    fund(addr, false);
    let hall = Arc::new(Hall::new(DAY).with_every(0));
    let (ann, ta) = bot("Ann", 1.0, &hall, addr);
    let (ben, tb) = bot("Ben", 0.9, &hall, addr);
    let (cas, tc) = bot("Cas", 0.8, &hall, addr);
    let refunded = until(Duration::from_secs(120), || {
        let st = hall.snapshot();
        st.pool.journal.iter().any(|e| e.kind == Kind::Pledge)
            && st.pool.pledges.is_empty()
            && st.pool.journal.iter().any(|e| e.kind == Kind::Refund)
    });
    let st = hall.snapshot();
    for h in [&ann, &ben, &cas] {
        h.stop();
    }
    for t in [ta, tb, tc] {
        let _ = t.join();
    }
    assert!(refunded, "pool {:?}\nAnn:\n{}", st.pool, dump(&ann));
    // Every pledge came back to the pledger it came from.
    for e in st.pool.journal.iter().filter(|e| e.kind == Kind::Pledge) {
        assert!(
            st.pool
                .journal
                .iter()
                .any(|r| r.kind == Kind::Refund && r.to == e.from && r.from == e.to),
            "{:?}",
            st.pool.journal
        );
    }
    assert!(st.bot_corps().is_empty());
}
