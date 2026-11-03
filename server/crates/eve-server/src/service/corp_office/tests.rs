//! CORP-O's service tests: the rental, the divisions, the deliveries hangar,
//! the rent coming due. Every id is invented; nothing here is a client value
//! and no assumed number is written into an assertion (the consts are named).

use super::*;
use crate::model::{Character, SandboxConfig};

const ALICE: i64 = 90_000_911;
const BOB: i64 = 90_000_912;
const CAROL: i64 = 90_000_913;
/// Not in the corporation.
const DAVE: i64 = 90_000_914;
/// `Character::default()`'s station: where all three are docked.
const STATION: i64 = 60_012_526;
const ORE: &str = "Veldspar";

fn key_of(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        return format!("{e:?}");
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        panic!("an exception header is (class, args)")
    };
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("UserError's args are a tuple")
    };
    let key = match &args[0] {
        PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => panic!("the key is a str: {other:?}"),
    };
    format!("{key} {:?}", args.get(1))
}

fn refused(e: &ServiceError, key: &str) -> bool {
    key_of(e).contains(key)
}

/// Alice founds a corp (CEO), Bob and Carol are members with no roles.
fn world() -> (Registry, i64) {
    let characters = [
        (ALICE, "Alice"),
        (BOB, "Bob"),
        (CAROL, "Carol"),
        (DAVE, "Dave"),
    ]
    .into_iter()
    .map(|(id, name)| Character {
        character_id: id,
        name: name.into(),
        ship_id: id + 1,
        balance: 5_000_000.0,
        ..Character::default()
    })
    .collect();
    let mut reg = Registry::new(SandboxConfig {
        characters,
        ..SandboxConfig::default()
    });
    let corp = found(&mut reg, ALICE, "Office Test", "OFFT");
    for who in [BOB, CAROL] {
        reg.corporations.enrol_for_test(who, corp, 0);
    }
    let _ = reg.take_session_changes();
    let _ = reg.take_pending();
    (reg, corp)
}

fn found(reg: &mut Registry, actor: i64, name: &str, ticker: &str) -> i64 {
    let u = |s: &str| PyValue::Unicode(s.to_owned());
    let args = vec![
        u(name),
        u(ticker),
        u("We mine."),
        u("http://"),
        PyValue::Real(0.0),
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
    reg.dispatch(&Request {
        service: "corpRegistry",
        method: "AddCorporation",
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: Some(actor),
    })
    .expect("founded");
    reg.character(actor).expect("a pilot").corporation_id
}

fn set_roles(reg: &mut Registry, who: i64, roles: i64) {
    reg.corporations.member_mut(who).expect("a member").roles = roles;
}

fn fund(reg: &mut Registry, corp: i64, isk: f64) {
    cw::adjust(reg.deltas_mut(), corp, cw::MASTER_KEY, cw::to_cents(isk)).expect("funded");
}

fn master(reg: &Registry, corp: i64) -> f64 {
    cw::to_isk(cw::balance(reg.deltas(), corp, cw::MASTER_KEY))
}

/// `officeManager`, bound on `station`.
fn om(
    reg: &mut Registry,
    who: i64,
    method: &'static str,
    args: Vec<PyValue>,
    station: i64,
) -> Result<PyValue, ServiceError> {
    let bind = PyValue::Int(station);
    reg.dispatch(&Request {
        service: "officeManager",
        method,
        args: &args,
        kwargs: &[],
        actor: Some(who),
        bind_params: Some(&bind),
    })
}

fn rent_ok(reg: &mut Registry, who: i64) -> i64 {
    om(
        reg,
        who,
        "RentOffice",
        vec![PyValue::Real(co::OFFICE_RENT_ISK)],
        STATION,
    )
    .expect("rented");
    co::all(reg.deltas())
        .last()
        .expect("an office row")
        .office_id
}

/// A call on a bound inventory: `GetInventoryFromId(container, 0)`'s object.
fn inv(
    reg: &mut Registry,
    who: i64,
    container: i64,
    method: &'static str,
    args: Vec<PyValue>,
    kwargs: Vec<(PyValue, PyValue)>,
) -> Result<PyValue, ServiceError> {
    let bind = PyValue::Tuple(vec![PyValue::Int(container), PyValue::Int(0)]);
    reg.dispatch(&Request {
        service: "invbroker::inventory",
        method,
        args: &args,
        kwargs: &kwargs,
        actor: Some(who),
        bind_params: Some(&bind),
    })
}

fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, crate::pyutil::bstr(name))
}

fn add(
    reg: &mut Registry,
    who: i64,
    container: i64,
    item: i64,
    source: i64,
    flag: i64,
    qty: Option<i64>,
) -> Result<PyValue, ServiceError> {
    inv(
        reg,
        who,
        container,
        "Add",
        vec![PyValue::Int(item), PyValue::Int(source)],
        vec![
            kw("qty", qty.map_or(PyValue::None, PyValue::Int)),
            kw("flag", PyValue::Int(flag)),
        ],
    )
}

fn listed(reg: &mut Registry, who: i64, container: i64, flag: Option<i64>) -> usize {
    match inv(
        reg,
        who,
        container,
        "List",
        vec![],
        vec![kw("flag", flag.map_or(PyValue::None, PyValue::Int))],
    )
    .expect("List answers")
    {
        PyValue::List(v) => v.len(),
        other => panic!("a list, got {other:?}"),
    }
}

fn ore(reg: &mut Registry, owner: i64, qty: i64) -> i64 {
    reg.spawn_items(ORE, qty, owner, STATION)
        .expect("granted")
        .0[0]
}

fn units_at(reg: &Registry, owner: i64, location: i64, flag: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        // Stacks only: the pilots' capsules are singletons in their hangars.
        .filter(|i| !i.singleton)
        .filter(|i| i.owner_id == owner && i.location_id == location && i.flag_id == flag)
        .map(|i| i.quantity)
        .sum()
}

/// A world with the corp's wallet funded and an office rented by Alice.
fn with_office() -> (Registry, i64, i64) {
    let (mut reg, corp) = world();
    fund(&mut reg, corp, co::OFFICE_RENT_ISK * 10.0);
    let office = rent_ok(&mut reg, ALICE);
    (reg, corp, office)
}

// ---------------------------------------------------------------------------
// The rental
// ---------------------------------------------------------------------------

#[test]
fn a_ceo_quotes_and_rents_an_office_paid_from_the_master_wallet() {
    let (mut reg, corp) = world();
    // The founding CEO holds the rent role (`CEO_ROLES`).
    assert_eq!(
        om(
            &mut reg,
            ALICE,
            "GetPriceQuote",
            vec![PyValue::Int(corp)],
            STATION
        )
        .unwrap(),
        PyValue::Real(co::OFFICE_RENT_ISK)
    );
    // A cost that is not the quote is refused with the real one.
    fund(&mut reg, corp, co::OFFICE_RENT_ISK * 3.0);
    let e = om(
        &mut reg,
        ALICE,
        "RentOffice",
        vec![PyValue::Real(1.0)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "RentingAnOfficeCostsMore"), "{e:?}");
    assert_eq!(master(&reg, corp), co::OFFICE_RENT_ISK * 3.0);

    let office = rent_ok(&mut reg, ALICE);
    assert_eq!(master(&reg, corp), co::OFFICE_RENT_ISK * 2.0);
    // The office is an item of the corporation's, in the station, and the
    // rental is a row.
    let item = reg.item(office).expect("the office item").clone();
    assert_eq!(
        (item.type_id, item.owner_id, item.location_id, item.flag_id),
        (co::TYPE_OFFICE, corp, STATION, co::FLAG_OFFICE)
    );
    let row = co::get(reg.deltas(), office).expect("the row");
    assert!(row.live() && row.paid_until > row.rented_at);
    // The journal names the fee and the station.
    let rows = cw::all_rows(reg.deltas(), corp);
    let fee = rows
        .iter()
        .find(|r| r.ref_type == REF_OFFICE_RENTAL_FEE)
        .expect("a rent row");
    assert_eq!(fee.amount, -cw::to_cents(co::OFFICE_RENT_ISK));
    assert_eq!(fee.reference, STATION);
    // The corp hears of it: the push names the corp and the office.
    let pushes = reg.take_pending();
    assert!(
        pushes
            .iter()
            .any(|(_, p)| p.to_push().0 == "OnOfficeRentalChange"
                && p.to_push().1 == vec![PyValue::Int(corp), PyValue::Int(office)]),
        "no OnOfficeRentalChange"
    );
}

#[test]
fn the_panels_reads_follow_the_rental() {
    let (mut reg, corp) = world();
    assert_eq!(
        om(&mut reg, BOB, "GetCorporationsWithOffices", vec![], STATION).unwrap(),
        PyValue::List(vec![])
    );
    assert_eq!(
        om(&mut reg, ALICE, "GetEmptyOfficeCount", vec![], STATION).unwrap(),
        PyValue::Int(co::OFFICES_PER_STATION as i64)
    );
    fund(&mut reg, corp, co::OFFICE_RENT_ISK);
    let office = rent_ok(&mut reg, ALICE);
    assert_eq!(
        om(&mut reg, BOB, "GetCorporationsWithOffices", vec![], STATION).unwrap(),
        PyValue::List(vec![PyValue::Int(corp)])
    );
    assert_eq!(
        om(&mut reg, ALICE, "GetEmptyOfficeCount", vec![], STATION).unwrap(),
        PyValue::Int(co::OFFICES_PER_STATION as i64 - 1)
    );
    // The corp's own list: a row with the attributes the client reads.
    let mine = om(&mut reg, BOB, "GetMyCorporationsOffices", vec![], 0).unwrap();
    let PyValue::List(rows) = mine else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    // An `eve_rowset::Row`: `(header names, line)`.
    let PyValue::ObjectEx { header, .. } = &rows[0] else {
        panic!("a row: {:?}", rows[0])
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        panic!("a header")
    };
    let PyValue::Tuple(h) = &h[1] else {
        panic!("row args")
    };
    let PyValue::List(line) = &h[1] else {
        panic!("a line")
    };
    assert_eq!(line[0], PyValue::Int(office));
    assert_eq!(line[1], PyValue::Int(STATION));
    // A pilot outside a player corp has none.
    assert_eq!(
        om(&mut reg, DAVE, "GetMyCorporationsOffices", vec![], 0).unwrap(),
        PyValue::List(vec![])
    );
}

#[test]
fn only_the_rent_role_quotes_and_rents_and_never_twice() {
    let (mut reg, corp) = world();
    fund(&mut reg, corp, co::OFFICE_RENT_ISK * 5.0);
    for method in ["GetPriceQuote", "RentOffice"] {
        let e = om(
            &mut reg,
            BOB,
            method,
            vec![PyValue::Real(co::OFFICE_RENT_ISK)],
            STATION,
        )
        .unwrap_err();
        assert!(
            refused(&e, "RentingOfficeQuotesOnlyGivenToActiveCEOsOrEquivale"),
            "{method}: {e:?}"
        );
    }
    // The role alone is enough; director is not.
    set_roles(&mut reg, BOB, co::ROLE_DIRECTOR);
    assert!(
        om(
            &mut reg,
            BOB,
            "GetPriceQuote",
            vec![PyValue::Int(corp)],
            STATION
        )
        .is_err()
    );
    set_roles(&mut reg, BOB, co::ROLE_CAN_RENT_OFFICE);
    om(
        &mut reg,
        BOB,
        "RentOffice",
        vec![PyValue::Real(co::OFFICE_RENT_ISK)],
        STATION,
    )
    .expect("a member with the role rents");
    // One per station.
    let e = om(
        &mut reg,
        ALICE,
        "GetPriceQuote",
        vec![PyValue::Int(corp)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "RentingYouHaveAnOfficeHere"), "{e:?}");
    let e = om(
        &mut reg,
        ALICE,
        "RentOffice",
        vec![PyValue::Real(co::OFFICE_RENT_ISK)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "RentingYouHaveAnOfficeHere"), "{e:?}");
    assert_eq!(master(&reg, corp), co::OFFICE_RENT_ISK * 4.0, "one payment");
}

#[test]
fn a_poor_corp_cannot_rent_and_nothing_is_minted() {
    let (mut reg, corp) = world();
    let before = reg.store().items().len();
    let e = om(
        &mut reg,
        ALICE,
        "RentOffice",
        vec![PyValue::Real(co::OFFICE_RENT_ISK)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "NotEnoughMoneyCorp"), "{e:?}");
    assert_eq!(reg.store().items().len(), before);
    assert!(co::all(reg.deltas()).is_empty());
    assert_eq!(master(&reg, corp), 0.0);
}

#[test]
fn a_full_station_has_no_office_to_rent() {
    let (mut reg, corp) = world();
    fund(&mut reg, corp, co::OFFICE_RENT_ISK);
    for i in 0..co::OFFICES_PER_STATION as i64 {
        co::put(
            reg.deltas_mut(),
            &Office {
                office_id: 100_000_900 + i,
                corp: 98_100_000 + i,
                station: STATION,
                system: 1,
                paid_until: i64::MAX,
                impounded_at: 0,
                rented_at: 1,
            },
        );
    }
    let e = om(
        &mut reg,
        ALICE,
        "GetPriceQuote",
        vec![PyValue::Int(corp)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "NoOfficesAreAvailableForRenting"), "{e:?}");
}

#[test]
fn renting_needs_the_pilot_docked_at_that_station() {
    let (mut reg, corp) = world();
    fund(&mut reg, corp, co::OFFICE_RENT_ISK);
    let e = om(
        &mut reg,
        ALICE,
        "GetPriceQuote",
        vec![PyValue::Int(corp)],
        STATION + 1,
    )
    .unwrap_err();
    assert!(refused(&e, "CustomNotify"), "{e:?}");
}

#[test]
fn the_rent_period_runs_on_the_game_clock() {
    let (mut reg, corp) = world();
    fund(&mut reg, corp, co::OFFICE_RENT_ISK * 2.0);
    crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(10).unwrap());
    let office = rent_ok(&mut reg, ALICE);
    let o = co::get(reg.deltas(), office).unwrap();
    assert_eq!(
        o.paid_until - o.rented_at,
        co::RENT_PERIOD_DAYS * co::DAY_TICKS / 10
    );
}

#[test]
fn a_director_gives_up_an_empty_office_and_it_goes() {
    let (mut reg, corp, office) = with_office();
    set_roles(&mut reg, BOB, co::ROLE_CAN_RENT_OFFICE);
    let e = om(&mut reg, BOB, "UnrentOffice", vec![], STATION).unwrap_err();
    assert!(
        refused(&e, "CrpAccessDenied"),
        "rent role is not unrent: {e:?}"
    );
    om(&mut reg, ALICE, "UnrentOffice", vec![], STATION).expect("a director unrents");
    assert!(reg.item(office).is_none());
    assert!(co::all(reg.deltas()).is_empty());
    assert_eq!(
        om(&mut reg, ALICE, "HasCorpImpoundedItems", vec![], STATION).unwrap(),
        PyValue::Bool(false)
    );
    // And a new one can be rented.
    let _ = corp;
    rent_ok(&mut reg, ALICE);
}

// ---------------------------------------------------------------------------
// The divisions
// ---------------------------------------------------------------------------

const DIV2: i64 = co::FLAG_SAG[1];
const DIV3: i64 = co::FLAG_SAG[2];

#[test]
fn a_member_with_the_take_role_puts_an_item_in_division_two_and_one_without_is_refused() {
    let (mut reg, corp, office) = with_office();
    set_roles(
        &mut reg,
        BOB,
        co::ROLE_HANGAR_TAKE[1] | co::ROLE_HANGAR_QUERY[1],
    );
    let bobs = ore(&mut reg, BOB, 100);
    let carols = ore(&mut reg, CAROL, 100);

    // Carol holds nothing: refused, and her ore stays where it was.
    let e = add(&mut reg, CAROL, office, carols, STATION, DIV2, None).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    assert_eq!(
        units_at(&reg, CAROL, STATION, crate::model::FLAG_HANGAR),
        100
    );

    // Bob holds division 2's bits, not division 3's.
    let e = add(&mut reg, BOB, office, bobs, STATION, DIV3, None).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    add(&mut reg, BOB, office, bobs, STATION, DIV2, None).expect("division 2");

    // The item is the corporation's, in the office, in division 2.
    let item = reg.item(bobs).expect("the item").clone();
    assert_eq!(
        (item.owner_id, item.location_id, item.flag_id),
        (corp, office, DIV2)
    );
    assert_eq!(units_at(&reg, BOB, STATION, crate::model::FLAG_HANGAR), 0);
    // The listing is by division and by query role.
    assert_eq!(listed(&mut reg, BOB, office, Some(DIV2)), 1);
    assert_eq!(listed(&mut reg, BOB, office, None), 1);
    assert_eq!(listed(&mut reg, BOB, office, Some(DIV3)), 0);
    assert_eq!(listed(&mut reg, CAROL, office, None), 0);
    assert_eq!(
        listed(&mut reg, ALICE, office, Some(DIV2)),
        1,
        "the CEO holds every bit"
    );
}

#[test]
fn a_part_of_a_stack_is_split_into_the_division() {
    let (mut reg, corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 100);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, Some(40)).expect("a split");
    assert_eq!(units_at(&reg, corp, office, DIV2), 40);
    assert_eq!(
        units_at(&reg, ALICE, STATION, crate::model::FLAG_HANGAR),
        60
    );
    let e = add(&mut reg, ALICE, office, stack, STATION, DIV2, Some(0)).unwrap_err();
    assert!(refused(&e, "CustomNotify"), "{e:?}");
}

#[test]
fn query_and_take_are_two_bits_and_a_taker_who_cannot_query_sees_nothing() {
    let (mut reg, _corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    // Query only: sees, cannot take.
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_QUERY[1]);
    assert_eq!(listed(&mut reg, BOB, office, None), 1);
    let e = add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        stack,
        office,
        crate::model::FLAG_HANGAR,
        None,
    )
    .unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    // Take only: cannot see it.
    set_roles(&mut reg, CAROL, co::ROLE_HANGAR_TAKE[1]);
    assert_eq!(listed(&mut reg, CAROL, office, None), 0);
}

#[test]
fn a_taker_moves_an_item_out_to_the_hangar_and_a_stranger_cannot() {
    let (mut reg, corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    // Bob has no role: refused, and the item stays.
    let e = add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        stack,
        office,
        crate::model::FLAG_HANGAR,
        None,
    )
    .unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    assert_eq!(units_at(&reg, corp, office, DIV2), 10);
    // Bob with the wrong division's take role: refused.
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_TAKE[2]);
    assert!(
        add(
            &mut reg,
            BOB,
            CONTAINER_HANGAR,
            stack,
            office,
            crate::model::FLAG_HANGAR,
            None
        )
        .is_err()
    );
    // The right one: part of it, then the rest.
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_TAKE[1]);
    add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        stack,
        office,
        crate::model::FLAG_HANGAR,
        Some(4),
    )
    .expect("take 4");
    assert_eq!(units_at(&reg, BOB, STATION, crate::model::FLAG_HANGAR), 4);
    assert_eq!(units_at(&reg, corp, office, DIV2), 6);
    add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        stack,
        office,
        crate::model::FLAG_HANGAR,
        None,
    )
    .expect("take the rest");
    assert_eq!(units_at(&reg, BOB, STATION, crate::model::FLAG_HANGAR), 10);
    assert_eq!(units_at(&reg, corp, office, DIV2), 0);
}

#[test]
fn an_item_cannot_go_into_a_ship_or_another_inventory_straight_from_a_division() {
    let (mut reg, _corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    let ship = reg.character(ALICE).unwrap().ship_id;
    let e = add(
        &mut reg,
        ALICE,
        ship,
        stack,
        office,
        crate::fitting::FLAG_CARGO,
        None,
    )
    .unwrap_err();
    assert!(refused(&e, "CustomNotify"), "{e:?}");
}

#[test]
fn the_active_ship_is_not_put_in_a_corporation_hangar() {
    let (mut reg, _corp, office) = with_office();
    let ship = reg.character(ALICE).unwrap().ship_id;
    let e = add(&mut reg, ALICE, office, ship, STATION, DIV2, None).unwrap_err();
    assert!(refused(&e, "CanNotPlaceActiveShipInAnotherHangar"), "{e:?}");
}

#[test]
fn another_corps_office_and_a_flag_that_is_no_division_are_refused() {
    let (mut reg, _corp, office) = with_office();
    let mine = ore(&mut reg, ALICE, 5);
    // Not a division.
    let e = add(&mut reg, ALICE, office, mine, STATION, 99, None).unwrap_err();
    assert!(refused(&e, "CustomNotify"), "{e:?}");
    // The projects' deliveries flag is not a division.
    assert!(add(&mut reg, ALICE, office, mine, STATION, 184, None).is_err());
    // An office of another corporation, asked by Alice.
    co::put(
        reg.deltas_mut(),
        &Office {
            office_id: office + 1,
            corp: 98_100_001,
            station: STATION,
            system: 1,
            paid_until: i64::MAX,
            impounded_at: 0,
            rented_at: 1,
        },
    );
    let e = add(&mut reg, ALICE, office + 1, mine, STATION, DIV2, None).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    assert_eq!(units_at(&reg, ALICE, STATION, crate::model::FLAG_HANGAR), 5);
}

#[test]
fn a_multi_drag_moves_what_it_may_and_raises_the_first_refusal() {
    let (mut reg, corp, office) = with_office();
    let a = ore(&mut reg, ALICE, 1);
    let b = ore(&mut reg, ALICE, 2);
    let r = inv(
        &mut reg,
        ALICE,
        office,
        "MultiAdd",
        vec![
            PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]),
            PyValue::Int(STATION),
        ],
        vec![kw("flag", PyValue::Int(DIV3))],
    );
    assert!(r.is_ok());
    assert_eq!(units_at(&reg, corp, office, DIV3), 3);
    // Bob, with no role, moves none and is told.
    let c = ore(&mut reg, BOB, 1);
    let e = inv(
        &mut reg,
        BOB,
        office,
        "MultiAdd",
        vec![PyValue::List(vec![PyValue::Int(c)]), PyValue::Int(STATION)],
        vec![kw("flag", PyValue::Int(DIV3))],
    )
    .unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
}

#[test]
fn a_division_moves_to_another_division_under_the_sources_role_too() {
    let (mut reg, corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    // Bob may take from division 3 only.
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_TAKE[2]);
    let e = add(&mut reg, BOB, office, stack, office, DIV3, None).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    set_roles(
        &mut reg,
        BOB,
        co::ROLE_HANGAR_TAKE[2] | co::ROLE_HANGAR_TAKE[1],
    );
    add(&mut reg, BOB, office, stack, office, DIV3, None).expect("both roles");
    assert_eq!(units_at(&reg, corp, office, DIV3), 10);
}

#[test]
fn the_office_asks_what_it_is_and_how_much_it_holds() {
    let (mut reg, _corp, office) = with_office();
    let row = inv(&mut reg, BOB, office, "GetSelfInvItem", vec![], vec![]).unwrap();
    let PyValue::DbRow(r) = &row else {
        panic!("an item row, got {row:?}")
    };
    let ix = r.descriptor().index_of(b"typeID").expect("typeID");
    assert_eq!(r.values()[ix], eve_marshal::DbValue::Int(co::TYPE_OFFICE));
    let cap = inv(&mut reg, BOB, office, "GetCapacity", vec![], vec![]).unwrap();
    assert!(matches!(cap, PyValue::Instance { .. }), "{cap:?}");
}

#[test]
fn a_division_change_tells_the_other_members_who_may_look() {
    let (mut reg, _corp, office) = with_office();
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_QUERY[1]);
    let stack = ore(&mut reg, ALICE, 10);
    let _ = reg.take_pending();
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    let pushes = reg.take_pending();
    let told: Vec<Vec<i64>> = pushes
        .iter()
        .filter(|(_, p)| p.to_push().0 == "OnInvalidateCacheForLocations")
        .filter_map(|(_, p)| p.audience())
        .collect();
    assert!(told.iter().any(|a| a.contains(&BOB)), "{told:?}");
    assert!(
        !told.iter().any(|a| a.contains(&CAROL)),
        "Carol has no bit for division 2: {told:?}"
    );
}

// ---------------------------------------------------------------------------
// The deliveries hangar
// ---------------------------------------------------------------------------

#[test]
fn the_deliveries_hangar_is_listed_for_accountants_and_traders_only() {
    let (mut reg, corp, _office) = with_office();
    let type_id = reg.spawn_items(ORE, 1, ALICE, STATION).unwrap().1.type_id;
    deliver_to_corp(&mut reg, corp, STATION, type_id, 500).expect("delivered");
    assert_eq!(units_at(&reg, corp, STATION, co::FLAG_CORP_DELIVERIES), 500);
    let market = co::CONTAINER_CORP_MARKET;
    assert_eq!(listed(&mut reg, BOB, market, None), 0, "no role");
    set_roles(&mut reg, BOB, co::ROLE_JUNIOR_ACCOUNTANT);
    assert_eq!(
        listed(&mut reg, BOB, market, None),
        1,
        "a junior accountant sees it"
    );
    set_roles(&mut reg, BOB, co::ROLE_TRADER);
    assert_eq!(listed(&mut reg, BOB, market, None), 1);
    assert_eq!(
        listed(&mut reg, ALICE, market, None),
        1,
        "the CEO holds the accountant role"
    );
}

#[test]
fn deliveries_are_taken_by_accountants_and_traders_not_a_junior() {
    let (mut reg, corp, _office) = with_office();
    let type_id = reg.spawn_items(ORE, 1, ALICE, STATION).unwrap().1.type_id;
    let id = deliver_to_corp(&mut reg, corp, STATION, type_id, 500).unwrap();
    set_roles(&mut reg, BOB, co::ROLE_JUNIOR_ACCOUNTANT);
    let e = add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        id,
        STATION,
        crate::model::FLAG_HANGAR,
        None,
    )
    .unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    set_roles(&mut reg, BOB, co::ROLE_TRADER);
    add(
        &mut reg,
        BOB,
        CONTAINER_HANGAR,
        id,
        STATION,
        crate::model::FLAG_HANGAR,
        Some(200),
    )
    .expect("a trader takes");
    assert_eq!(units_at(&reg, BOB, STATION, crate::model::FLAG_HANGAR), 200);
    assert_eq!(units_at(&reg, corp, STATION, co::FLAG_CORP_DELIVERIES), 300);
}

#[test]
fn deliver_to_corp_hangar_moves_deliveries_into_a_division() {
    let (mut reg, corp, office) = with_office();
    let type_id = reg.spawn_items(ORE, 1, ALICE, STATION).unwrap().1.type_id;
    let id = deliver_to_corp(&mut reg, corp, STATION, type_id, 80).unwrap();
    let call = |reg: &mut Registry, who: i64, flag: i64| {
        let bind = PyValue::Tuple(vec![PyValue::Int(STATION), PyValue::Int(15)]);
        let args = vec![
            PyValue::Int(STATION),
            PyValue::Int(STATION),
            PyValue::List(vec![PyValue::Int(id)]),
            PyValue::None,
            PyValue::Int(corp),
            PyValue::Int(flag),
        ];
        reg.dispatch(&Request {
            service: "invbroker",
            method: "DeliverToCorpHangar",
            args: &args,
            kwargs: &[],
            actor: Some(who),
            bind_params: Some(&bind),
        })
    };
    // Bob: the division's take role but not the deliveries one.
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_TAKE[3]);
    let e = call(&mut reg, BOB, co::FLAG_SAG[3]).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    set_roles(&mut reg, BOB, co::ROLE_HANGAR_TAKE[3] | co::ROLE_TRADER);
    call(&mut reg, BOB, co::FLAG_SAG[3]).expect("delivered to division 4");
    assert_eq!(units_at(&reg, corp, office, co::FLAG_SAG[3]), 80);
    assert_eq!(units_at(&reg, corp, STATION, co::FLAG_CORP_DELIVERIES), 0);
}

// ---------------------------------------------------------------------------
// The rent comes due
// ---------------------------------------------------------------------------

fn due_date(reg: &Registry, office: i64) -> i64 {
    co::get(reg.deltas(), office).expect("a row").paid_until
}

#[test]
fn a_due_office_is_billed_and_renewed_from_its_old_date() {
    let (mut reg, corp, office) = with_office();
    let before = master(&reg, corp);
    let due = due_date(&reg, office);
    // Not yet.
    assert_eq!(on_rollover(&mut reg, due - 1), 0);
    assert_eq!(master(&reg, corp), before);
    // On the day.
    assert_eq!(on_rollover(&mut reg, due), 1);
    assert_eq!(master(&reg, corp), before - co::OFFICE_RENT_ISK);
    assert_eq!(
        due_date(&reg, office),
        due + co::RENT_PERIOD_DAYS * co::DAY_TICKS
    );
    assert!(co::get(reg.deltas(), office).unwrap().live());
    // Billed once, not on the next rollover too.
    assert_eq!(on_rollover(&mut reg, due + 1), 0);
}

#[test]
fn an_empty_office_lapses_and_vanishes_when_the_wallet_cannot_pay() {
    let (mut reg, corp, office) = with_office();
    // Spend the wallet.
    let rest = master(&reg, corp);
    cw::adjust(reg.deltas_mut(), corp, cw::MASTER_KEY, -cw::to_cents(rest)).unwrap();
    let due = due_date(&reg, office);
    assert_eq!(on_rollover(&mut reg, due), 1);
    assert!(reg.item(office).is_none());
    assert!(co::all(reg.deltas()).is_empty());
    assert_eq!(
        om(&mut reg, ALICE, "GetMyCorporationsOffices", vec![], 0).unwrap(),
        PyValue::List(vec![])
    );
}

/// An office with 10 ore in division 2 whose rent has lapsed.
fn lapsed_with_items() -> (Registry, i64, i64, i64) {
    let (mut reg, corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    let rest = master(&reg, corp);
    cw::adjust(reg.deltas_mut(), corp, cw::MASTER_KEY, -cw::to_cents(rest)).unwrap();
    let due = due_date(&reg, office);
    assert_eq!(on_rollover(&mut reg, due), 1);
    (reg, corp, office, stack)
}

#[test]
fn a_lapsed_office_with_items_is_impounded_not_lost() {
    let (mut reg, corp, office, stack) = lapsed_with_items();
    let item = reg.item(office).expect("the office item stays").clone();
    assert_eq!(item.flag_id, co::FLAG_IMPOUNDED);
    let row = co::get(reg.deltas(), office).unwrap();
    assert!(!row.live());
    // No longer a live office.
    assert_eq!(
        om(&mut reg, ALICE, "GetMyCorporationsOffices", vec![], 0).unwrap(),
        PyValue::List(vec![])
    );
    assert_eq!(
        om(
            &mut reg,
            ALICE,
            "GetCorporationsWithOffices",
            vec![],
            STATION
        )
        .unwrap(),
        PyValue::List(vec![])
    );
    assert_eq!(
        om(&mut reg, ALICE, "HasCorpImpoundedItems", vec![], STATION).unwrap(),
        PyValue::Bool(true)
    );
    // Its contents are out of reach, listing and taking alike.
    assert_eq!(listed(&mut reg, ALICE, office, None), 0);
    assert!(
        add(
            &mut reg,
            ALICE,
            CONTAINER_HANGAR,
            stack,
            office,
            crate::model::FLAG_HANGAR,
            None
        )
        .is_err()
    );
    assert_eq!(units_at(&reg, corp, office, DIV2), 10);
    // And a new rental is allowed.
    fund(&mut reg, corp, co::OFFICE_RENT_ISK);
    rent_ok(&mut reg, ALICE);
}

#[test]
fn the_release_price_brings_the_items_to_a_directors_hangar() {
    let (mut reg, corp, office, _stack) = lapsed_with_items();
    let price = co::release_price();
    assert_eq!(
        om(&mut reg, ALICE, "GetImpoundReleasePrice", vec![], STATION).unwrap(),
        PyValue::Real(price)
    );
    // A short wallet, a wrong price, a non-director.
    let e = om(
        &mut reg,
        ALICE,
        "GetItemsFromImpound",
        vec![PyValue::Real(price)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "NotEnoughMoneyCorp"), "{e:?}");
    fund(&mut reg, corp, price * 2.0);
    let e = om(
        &mut reg,
        ALICE,
        "GetItemsFromImpound",
        vec![PyValue::Real(1.0)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "RentingAnOfficeCostsMore"), "{e:?}");
    let e = om(
        &mut reg,
        BOB,
        "GetItemsFromImpound",
        vec![PyValue::Real(price)],
        STATION,
    )
    .unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    om(
        &mut reg,
        ALICE,
        "GetItemsFromImpound",
        vec![PyValue::Real(price)],
        STATION,
    )
    .expect("released");
    assert_eq!(master(&reg, corp), price);
    assert_eq!(
        units_at(&reg, ALICE, STATION, crate::model::FLAG_HANGAR),
        10
    );
    assert!(reg.item(office).is_none());
    assert!(co::all(reg.deltas()).is_empty());
}

#[test]
fn trashing_the_impound_destroys_the_items_and_the_office() {
    let (mut reg, _corp, office, stack) = lapsed_with_items();
    let e = om(&mut reg, BOB, "TrashImpoundedOffice", vec![], STATION).unwrap_err();
    assert!(refused(&e, "CrpAccessDenied"), "{e:?}");
    om(&mut reg, ALICE, "TrashImpoundedOffice", vec![], STATION).expect("trashed");
    assert!(reg.item(stack).is_none());
    assert!(reg.item(office).is_none());
    assert!(co::all(reg.deltas()).is_empty());
}

#[test]
fn giving_up_an_office_with_items_impounds_it() {
    let (mut reg, corp, office) = with_office();
    let stack = ore(&mut reg, ALICE, 3);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    om(&mut reg, ALICE, "UnrentOffice", vec![], STATION).unwrap();
    assert_eq!(reg.item(office).unwrap().flag_id, co::FLAG_IMPOUNDED);
    assert!(!co::get(reg.deltas(), office).unwrap().live());
    let _ = corp;
}

#[test]
fn item_locking_answers_an_empty_container() {
    let (mut reg, _corp) = world();
    for method in ["GetItemsByLocation", "GetLockedItemLocations"] {
        let r = reg
            .dispatch(&Request {
                service: "itemLocking",
                method,
                args: &[PyValue::Int(STATION)],
                kwargs: &[],
                actor: Some(ALICE),
                bind_params: None,
            })
            .unwrap();
        assert_eq!(r, PyValue::List(vec![]));
    }
}

#[test]
fn the_world_file_keeps_the_rental() {
    let (reg, _corp, office) = with_office();
    let snapshot = reg.to_snapshot();
    let mut again = Registry::new(SandboxConfig::default());
    again.apply_snapshot(snapshot);
    assert!(co::get(again.deltas(), office).is_some());
    assert!(again.item(office).is_some());
}

// ---------------------------------------------------------------------------
// The Corporation > Assets tab
// ---------------------------------------------------------------------------

fn assets(reg: &Registry, who: i64, method: &str, args: Vec<PyValue>) -> eve_rowset::CRowset {
    let req = Request {
        service: "corpmgr",
        method,
        args: &args,
        kwargs: &[],
        actor: Some(who),
        bind_params: None,
    };
    if method == "GetAssetInventory" {
        asset_inventory(reg, &req)
    } else {
        asset_inventory_for_location(reg, &req)
    }
}

fn which(corp: i64, w: &str) -> Vec<PyValue> {
    vec![PyValue::Int(corp), crate::pyutil::bstr(w)]
}

#[test]
fn the_assets_tab_lists_the_corps_offices_and_deliveries_and_never_none() {
    let (mut reg, corp, office) = with_office();
    let offices = assets(&reg, ALICE, "GetAssetInventory", which(corp, "offices"));
    assert_eq!(offices.len(), 1, "the rented office, empty or not");
    let names: Vec<_> = offices
        .descriptor()
        .columns()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert!(names.contains(&b"locationID".to_vec()) && names.contains(&b"solarsystemID".to_vec()));
    assert!(!names.contains(&b"itemCount".to_vec()));
    // Another corp's member and a stranger see nothing.
    assert_eq!(
        assets(&reg, DAVE, "GetAssetInventory", which(corp, "offices")).len(),
        0
    );
    // Deliveries: counted per station, with the count column.
    let type_id = reg.spawn_items(ORE, 1, ALICE, STATION).unwrap().1.type_id;
    deliver_to_corp(&mut reg, corp, STATION, type_id, 500).expect("delivered");
    let d = assets(&reg, ALICE, "GetAssetInventory", which(corp, "deliveries"));
    assert_eq!(d.len(), 1);
    assert!(
        d.descriptor()
            .columns()
            .iter()
            .any(|c| c.name == b"itemCount")
    );
    // The tabs this server has nothing for answer a rowset, not None.
    for w in ["property", "capsuleerdeliveries", "assetwraps", "impounded"] {
        assert_eq!(
            assets(&reg, ALICE, "GetAssetInventory", which(corp, w)).len(),
            0,
            "{w}"
        );
    }
    // Opening the office group: the items in the divisions, by query bit.
    let stack = ore(&mut reg, ALICE, 10);
    add(&mut reg, ALICE, office, stack, STATION, DIV2, None).unwrap();
    let args = |w: &str| {
        vec![
            PyValue::Int(corp),
            PyValue::Int(STATION),
            crate::pyutil::bstr(w),
        ]
    };
    assert_eq!(
        assets(&reg, ALICE, "GetAssetInventoryForLocation", args("offices")).len(),
        1
    );
    assert_eq!(
        assets(&reg, BOB, "GetAssetInventoryForLocation", args("offices")).len(),
        0
    );
    assert_eq!(
        assets(
            &reg,
            ALICE,
            "GetAssetInventoryForLocation",
            args("deliveries")
        )
        .len(),
        1
    );
    assert_eq!(
        assets(
            &reg,
            ALICE,
            "GetAssetInventoryForLocation",
            args("impounded")
        )
        .len(),
        0
    );
}

#[test]
fn the_assets_tab_lists_an_impounded_office_and_its_items() {
    let (reg, corp, _office, _stack) = lapsed_with_items();
    assert_eq!(
        assets(&reg, ALICE, "GetAssetInventory", which(corp, "offices")).len(),
        0
    );
    assert_eq!(
        assets(&reg, ALICE, "GetAssetInventory", which(corp, "impounded")).len(),
        1
    );
    let args = vec![
        PyValue::Int(corp),
        PyValue::Int(STATION),
        crate::pyutil::bstr("impounded"),
    ];
    assert_eq!(
        assets(&reg, ALICE, "GetAssetInventoryForLocation", args).len(),
        1
    );
}

#[test]
fn the_votes_tab_question_is_answered_false_and_the_campaign_snapshot_names_the_callers_system() {
    let (mut reg, _corp, _office) = with_office();
    let call = |reg: &mut Registry, service: &'static str, method: &'static str, who: i64| {
        reg.dispatch(&Request {
            service,
            method,
            args: &[PyValue::Int(1)],
            kwargs: &[],
            actor: Some(who),
            bind_params: None,
        })
        .expect("answered")
    };
    assert_eq!(
        call(&mut reg, "voteManager", "CanViewVotes", ALICE),
        PyValue::Bool(false)
    );
    // The caller's session system, not the first configured character's.
    reg.character_mut(BOB).expect("bob").solar_system_id = Some(30_000_777);
    assert_eq!(
        call(
            &mut reg,
            "insurgencySolarsystem",
            "GetLocalCampaignClientSnapshot",
            BOB
        ),
        PyValue::Tuple(vec![PyValue::Int(30_000_777), PyValue::None])
    );
}
