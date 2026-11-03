//! SOC-C2 over a socket: a corporation's contact book, set by its CEO
//! through the moniker the client binds, read back the way
//! `addressbookService.GetContacts` reads it, and coloured the way
//! `GetRelationship` and the overview colour it. Invented ids throughout.

use super::common::{config, login, pilot, serve};
use eve_marshal::PyValue;
use eve_server::group_contacts::{Books, StandingFlag, relationship, standing_flag};
use std::collections::BTreeMap;

const ALICE: i64 = 90_000_201;
const BOB: i64 = 90_000_202;
/// Bob's corp on his slim item: an NPC corp id, invented.
const BOB_CORP: i64 = 1_000_001;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// `bco_corporations.AddCorporation`'s fifteen positionals.
fn found(c: &mut eve_bot::Conn) -> i64 {
    let args = vec![
        uni("Invented Socket Corp"),
        uni("ISCK"),
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
    c.session()
        .get_i64("corpid")
        .expect("a corp in the session")
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

#[test]
fn a_corp_contact_at_minus_ten_colours_its_target_horrible() {
    let mut config = config();
    let mut alice = pilot(ALICE, "Alice", 100_000_201);
    alice.balance = 1.0e9;
    config.sandbox.characters = vec![alice, pilot(BOB, "Bob", 100_000_202)];
    let addr = serve(config);
    let mut c = login(addr, "Alice");
    let corp = found(&mut c);

    let set = c
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "AddCorporateContact",
            vec![PyValue::Int(BOB), PyValue::Int(-10)],
            vec![],
        )
        .expect("answered");
    assert_eq!(set.outcome.refusal(), None);

    let got = c
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "GetCorporateContacts",
            vec![],
            vec![],
        )
        .expect("answered");
    let Some(PyValue::Dict(entries)) = got.value else {
        panic!("the corp book is a dict: {:?}", got.value)
    };
    let corp_book: BTreeMap<i64, f64> = entries
        .iter()
        .map(|(v, k)| {
            let PyValue::Int(id) = k else {
                panic!("keyed by contactID: {k:?}")
            };
            assert_eq!(attr(v, "contactID"), PyValue::Int(*id));
            (*id, number(&attr(v, "relationshipID")))
        })
        .collect();
    assert_eq!(corp_book.get(&BOB), Some(&-10.0));

    let empty = BTreeMap::new();
    let r = relationship(
        Books {
            personal: &empty,
            corp: &corp_book,
            alliance: &empty,
        },
        BOB,
        BOB_CORP,
        None,
    );
    assert_eq!(r.corp_to_pers, -10.0);
    assert!(r.has_relationship);
    assert_eq!(standing_flag(&r), StandingFlag::Horrible);
}

/// **REL-FIX: a personal contact makes a lone pilot allied.** Alice, a bot,
/// applies a friends list naming Bob (by name) and an invented id: Bob is in
/// her contact list at the default standing, the pure `allied_with` over
/// that list says allied for Bob (an NPC-corp pilot) and not for a stranger,
/// and the hall stops treating Bob as prey.
#[test]
fn a_friends_list_sets_personal_contacts_and_ends_the_hunt() {
    const STRANGER: i64 = 90_000_299;
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_201),
        pilot(BOB, "Bob", 100_000_202),
    ];
    let addr = serve(config);
    let mut c = login(addr, "Alice");
    let friends = eve_bot::friends::Friends::from_text("Bob\n".to_owned());
    let hall = eve_bot::corpmind::Hall::new(86_400);
    let set = eve_bot::friends::apply(&mut c, &friends, Some(&hall)).expect("applies");
    assert_eq!(set, 1);

    let list = c
        .call("charMgr", "GetContactList", vec![])
        .expect("answered")
        .value()
        .expect("a list");
    let addresses = attr(&list, "addresses");
    let PyValue::List(rows) = addresses else {
        panic!("addresses: {addresses:?}")
    };
    let personal: BTreeMap<i64, f64> = rows
        .iter()
        .map(|r| {
            let PyValue::Int(id) = attr(r, "contactID") else {
                panic!("contactID")
            };
            (id, number(&attr(r, "relationshipID")))
        })
        .collect();
    assert_eq!(
        personal.get(&BOB),
        Some(&eve_bot::friends::DEFAULT_STANDING)
    );

    use eve_server::group_contacts::Affiliation;
    let me = Affiliation {
        character_id: ALICE,
        corporation_id: BOB_CORP,
        alliance_id: None,
    };
    let lone = |id| Affiliation {
        character_id: id,
        corporation_id: BOB_CORP,
        alliance_id: None,
    };
    let mine = |id: i64| personal.get(&id).copied();
    let none = |_: i64, _: i64| None;
    assert!(eve_bot::society::allied_with(me, lone(BOB), none, mine));
    assert!(!eve_bot::society::allied_with(
        me,
        lone(STRANGER),
        none,
        mine
    ));
    assert!(!hall.hostile_to(me, lone(BOB)));
    assert!(hall.hostile_to(me, lone(STRANGER)));
}
