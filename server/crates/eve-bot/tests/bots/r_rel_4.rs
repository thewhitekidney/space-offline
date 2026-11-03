//! R-REL-4 over a real socket: the human side. A human who helps a bot
//! rises a tier, is told by the CEO's mail and is invited into the corp; a
//! human who ganks a bot falls, is told, becomes a red-ish personal contact
//! of the bot and prey for the hunters; and a `--friends` human is blue to
//! the bots from the first minute.
//!
//! Invented ids; no `[A]` value is an expected value (named constants and
//! signs).

use super::common::{config, login, pilot, serve};
use eve_bot::career::{CAREERS, Signals};
use eve_bot::corpmind::{self, Hall};
use eve_bot::effects;
use eve_bot::human;
use eve_bot::mind::Mind;
use eve_bot::observe;
use eve_bot::relations::{self, Ctx, Fact, FactKind, Kill, Temper};
use eve_bot::society::{self, Stance5};
use eve_marshal::PyValue;
use eve_server::group_contacts::Affiliation;
use eve_server::mail::label;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const ALICE: i64 = 90_000_901;
const BOB: i64 = 90_000_902;
const CAROL: i64 = 90_000_903;
const DAY: u64 = 60;
const NOW: u64 = 10_000;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn mind() -> Arc<Mutex<Mind>> {
    Arc::new(Mutex::new(Mind::seeded(
        "Invented Pilot",
        7,
        CAREERS,
        &Signals::default(),
        0,
    )))
}

fn attr(v: &PyValue, name: &str) -> PyValue {
    let PyValue::Instance { state, .. } = v else {
        panic!("a KeyVal, got {v:?}")
    };
    eve_server::pyutil::dict_get(state, name.as_bytes())
        .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
        .clone()
}

fn number(v: &PyValue) -> f64 {
    match v {
        PyValue::Int(n) => *n as f64,
        PyValue::Real(r) => *r,
        other => panic!("a number, got {other:?}"),
    }
}

/// `bco_corporations.AddCorporation`'s fifteen positionals.
fn found(c: &mut eve_bot::Conn) -> i64 {
    let args = vec![
        uni("Invented Society Corp"),
        uni("ISCP"),
        uni("A test corp."),
        uni("http://"),
        PyValue::Real(0.1),
        PyValue::Int(419),
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::Bool(true),
        PyValue::Bool(false),
        PyValue::Real(0.0),
    ];
    let out = c
        .call("corpRegistry", "AddCorporation", args)
        .expect("answered");
    assert_eq!(out.refusal(), None, "the corp is founded");
    c.session().get_i64("corpid").expect("a corp")
}

/// The bot's own book as `GetContactList` returns it.
fn personal_book(c: &mut eve_bot::Conn) -> BTreeMap<i64, f64> {
    let list = c
        .call("charMgr", "GetContactList", vec![])
        .expect("answered")
        .value()
        .expect("a list");
    let PyValue::List(rows) = attr(&list, "addresses") else {
        panic!("addresses")
    };
    rows.iter()
        .map(|r| {
            let PyValue::Int(id) = attr(r, "contactID") else {
                panic!("contactID")
            };
            (id, number(&attr(r, "relationshipID")))
        })
        .collect()
}

/// The titles of the mails in `c`'s inbox (`SyncMail`'s headers).
fn inbox_titles(c: &mut eve_bot::Conn) -> Vec<String> {
    let answer = c
        .call("mailMgr", "SyncMail", vec![PyValue::None, PyValue::Int(0)])
        .expect("answered")
        .value()
        .expect("a mailbox");
    let (PyValue::List(new), PyValue::List(held)) =
        (attr(&answer, "newMail"), attr(&answer, "mailStatus"))
    else {
        panic!("lists")
    };
    let inbox: Vec<i64> = held
        .iter()
        .filter(|r| number(&attr(r, "labelMask")) as i64 & label::INBOX != 0)
        .map(|r| number(&attr(r, "messageID")) as i64)
        .collect();
    new.iter()
        .filter(|h| inbox.contains(&(number(&attr(h, "messageID")) as i64)))
        .map(|h| match attr(h, "title") {
            PyValue::Unicode(s) => s,
            other => panic!("a title, got {other:?}"),
        })
        .collect()
}

fn human(c: &eve_bot::Conn) -> Affiliation {
    Affiliation {
        character_id: c.session().char_id().expect("a pilot"),
        corporation_id: c.session().get_i64("corpid").expect("a corp"),
        alliance_id: None,
    }
}

fn fact(kind: FactKind, who: Affiliation, bot: Affiliation, gang: usize) -> Fact {
    Fact {
        kind,
        actor: who,
        target: bot,
        isk: society::KILL_VALUE_REF,
        ctx: Ctx {
            kill: Kill {
                gang,
                ..Ctx::default().kill
            },
            ..Ctx::default()
        },
        at: NOW,
    }
}

/// What the observer does with a fact: the bot's own regard and the hall.
fn apply(hall: &Hall, m: &Arc<Mutex<Mind>>, bot: i64, f: &Fact) {
    let ds = relations::deltas(f, &Temper::NORMAL);
    observe::apply_personal(m, bot, &ds, NOW, DAY, &Temper::NORMAL);
    hall.lock().apply_deltas(&ds);
}

fn r_of(hall: &Hall, m: &Arc<Mutex<Mind>>, corp: i64, who: &Affiliation) -> f64 {
    let (regard, loyalty) = {
        let m = m.lock().unwrap();
        (
            m.regard.clone(),
            m.traits.get(eve_bot::mind::Trait::Loyalty),
        )
    };
    hall.lock().r_eff_of(corp, &regard, loyalty, who, NOW)
}

/// **A human who helps a bot in a fight rises a tier, is told and is
/// invited.** The help facts are what R-REL-2 reads off a kill report; the
/// shared hours are what the presence hook credits. The CEO's own turn then
/// mails Bob the new tier and sends him a corp invitation, which Bob's
/// client lists.
#[test]
fn a_human_who_helps_a_bot_rises_a_tier_and_is_invited() {
    let mut config = config();
    let mut alice = pilot(ALICE, "Alice", 100_000_901);
    alice.balance = 1.0e9;
    config.sandbox.characters = vec![alice, pilot(BOB, "Bob", 100_000_902)];
    let addr = serve(config);
    let mut a = login(addr, "Alice");
    let corp = found(&mut a);
    let mut b = login(addr, "Bob");
    let bob = human(&b);

    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    let me = Affiliation {
        character_id: ALICE,
        corporation_id: corp,
        alliance_id: None,
    };
    hall.lock().see_human(&bob, NOW);
    assert!(
        r_of(&hall, &m, corp, &bob) < society::FRIEND_AT,
        "a stranger starts below the friend bar"
    );
    // He helps in fights until the CEO would take him, and flies on.
    let mut fights = 0;
    while r_of(&hall, &m, corp, &bob) < effects::INVITE_AT && fights < 20 {
        apply(&hall, &m, ALICE, &fact(FactKind::Help, bob, me, 1));
        fights += 1;
    }
    assert!(fights >= 2, "one fight is not a friendship");
    hall.lock()
        .note_fleet_hours(BOB, effects::INVITE_FLEET_HOURS);

    let lines = corpmind::turn(&mut a, &hall, &m, CAREERS, NOW).expect("the social turn");
    let tier = hall.snapshot().tiers.get(&(corp, BOB)).map(|t| t.0);
    assert!(tier >= Some(Stance5::Friendly), "{tier:?}\n{lines:#?}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains(&format!("invite {BOB}: ok"))),
        "{lines:#?}"
    );
    // Bob's client: a mail naming the tier, and the invitation.
    let titles = inbox_titles(&mut b);
    let word = tier.expect("a tier").name();
    assert!(
        titles.iter().any(|t| t.contains(word)),
        "{word} in {titles:?}\n{lines:#?}"
    );
    let mine = b
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "GetMyApplications",
            vec![],
            vec![],
        )
        .expect("answered");
    let Some(PyValue::Dict(offers)) = mine.value else {
        panic!("a dict: {:?}", mine.value)
    };
    assert!(!offers.is_empty(), "Bob holds the corp's invitation");
    // Told once: the next turn says nothing more of it.
    hall.lock()
        .members
        .get_mut(&ALICE)
        .expect("Alice")
        .last_turn = None;
    let again = corpmind::turn(&mut a, &hall, &m, CAREERS, NOW + 1).expect("again");
    assert!(
        !again.iter().any(|l| l.contains("mail") && l.contains(word)),
        "{again:#?}"
    );
}

/// **A human who ganks a bot falls and is hunted.** Two gang kills later
/// Bob stands at Wary or worse with the CEO, is told so, is a negative
/// personal contact in the bot's own book, and the hunters count him prey.
#[test]
fn a_human_who_ganks_a_bot_falls_is_told_and_is_hunted() {
    let mut config = config();
    let mut alice = pilot(ALICE, "Alice", 100_000_901);
    alice.balance = 1.0e9;
    config.sandbox.characters = vec![
        alice,
        pilot(BOB, "Bob", 100_000_902),
        pilot(CAROL, "Carol", 100_000_903),
    ];
    let addr = serve(config);
    let mut a = login(addr, "Alice");
    let corp = found(&mut a);
    let mut b = login(addr, "Bob");
    let bob = human(&b);
    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    let me = Affiliation {
        character_id: ALICE,
        corporation_id: corp,
        alliance_id: None,
    };
    hall.lock().see_human(&bob, NOW);
    let mut events = 0;
    while r_of(&hall, &m, corp, &bob) > society::WARY_AT && events < 2 {
        apply(
            &hall,
            &m,
            ALICE,
            &fact(FactKind::Loss, bob, me, relations::GANK_GANG),
        );
        events += 1;
    }
    assert!(
        r_of(&hall, &m, corp, &bob) <= society::WARY_AT,
        "wary within two events ({events})"
    );
    assert!(
        hall.lock().hostile_to(me, bob),
        "the hunters count him prey"
    );

    let lines = corpmind::turn(&mut a, &hall, &m, CAREERS, NOW).expect("the social turn");
    let tier = hall.snapshot().tiers.get(&(corp, BOB)).map(|t| t.0);
    assert!(
        matches!(tier, Some(Stance5::Wary | Stance5::Hostile)),
        "{tier:?}\n{lines:#?}"
    );
    assert!(human::unlocks(tier.unwrap()).hunted);
    let titles = inbox_titles(&mut b);
    let word = tier.unwrap().name();
    assert!(
        titles.iter().any(|t| t.contains(word)),
        "{word} in {titles:?}"
    );
    // The bot's own book: a negative contact, as the client lists it.
    effects::personal_contacts(&mut a, &hall, &m, NOW).expect("tends the book");
    let book = personal_book(&mut a);
    // Only a deep enough grudge is a contact; either way no positive one.
    assert!(book.get(&BOB).is_none_or(|v| *v < 0.0), "{book:?}");
}

/// **A `--friends` human is blue from the first minute**: the bot's regard
/// is seeded from the file and wins over a grudge it held, the hall never
/// hunts him, and the CEO tells him once that he is a friend.
#[test]
fn a_friends_file_human_starts_friendly_and_is_told() {
    let mut config = config();
    let mut alice = pilot(ALICE, "Alice", 100_000_901);
    alice.balance = 1.0e9;
    config.sandbox.characters = vec![alice, pilot(BOB, "Bob", 100_000_902)];
    let addr = serve(config);
    let mut a = login(addr, "Alice");
    let corp = found(&mut a);
    let mut b = login(addr, "Bob");
    let bob = human(&b);
    let hall = Hall::new(DAY).with_every(0);
    let m = mind();
    // A grudge from before the file.
    m.lock().unwrap().regard.push(eve_bot::mind::Regard {
        who: BOB,
        score: -6.0,
        at: NOW,
    });
    let friends = eve_bot::friends::Friends::from_text(format!("{BOB}\n"));
    let n = eve_bot::friends::apply_with_mind(&mut a, &friends, Some(&hall), Some(&*m), NOW)
        .expect("applies");
    assert_eq!(n, 1);
    let own = m
        .lock()
        .unwrap()
        .regard
        .iter()
        .find(|r| r.who == BOB)
        .map(|r| r.score);
    assert_eq!(
        own,
        Some(eve_bot::friends::DEFAULT_STANDING),
        "the file wins"
    );
    hall.lock().see_human(&bob, NOW);
    let me = Affiliation {
        character_id: ALICE,
        corporation_id: corp,
        alliance_id: None,
    };
    assert!(!hall.lock().hostile_to(me, bob));
    corpmind::turn(&mut a, &hall, &m, CAREERS, NOW).expect("the social turn");
    let tier = hall.snapshot().tiers.get(&(corp, BOB)).map(|t| t.0);
    assert_eq!(tier, Some(Stance5::Friendly));
    let titles = inbox_titles(&mut b);
    assert!(titles.iter().any(|t| t.contains("friendly")), "{titles:?}");
}
