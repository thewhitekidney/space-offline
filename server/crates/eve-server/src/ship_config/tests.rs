//! CAP-H unit tests. **Every id and number is invented** (types 990,660,xxx,
//! items 100,660,xxx, pilots 90,660,xxx); the client's constants appear only
//! by name (`FLAG_*`, [`MAX_TRANSFER_DISTANCE`], [`CONTAINER_GROUPS`]). The
//! capacities are round test numbers, not the game's.

use super::*;
use crate::item::find_in;
use crate::service::Request;
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_660_001;
const BOB: i64 = 90_660_002;
const CAROL: i64 = 90_660_003;
const DAVE: i64 = 90_660_004;
const CORP_A: i64 = 98_660_001;
const CORP_B: i64 = 98_660_002;
const CORP_C: i64 = 98_660_003;
const SYSTEM: i64 = 30_660_001;
const STATION: i64 = 60_660_001;

const CARRIER: i64 = 990_660_001;
const HULL_S: i64 = 990_660_002;
const CAPSULE_T: i64 = 990_660_003;
const STUFF: i64 = 990_660_004;
const BOX: i64 = 990_660_005;
const MINI: i64 = 990_660_006;
const GROUP_HULL: i64 = 990_660;
const GROUP_STUFF: i64 = 990_661;
/// Invented: the carrier's two bays, in m3, and the unit volumes.
const SMB_M3: f64 = 100.0;
const HANGAR_M3: f64 = 50.0;
const STUFF_M3: f64 = 1.0;
const HULL_M3: f64 = 10.0;
/// Invented: a small hull's cargo hold.
const CARGO_M3: f64 = 1000.0;

fn ship(who: i64) -> i64 {
    100_660_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let hull = |id: i64, group: i64, vol: f64, extra: &str, name: &str| {
        format!(
            "type\t{id}\t{group}\t6\t1\t1\t{vol}\t{CARGO_M3}\t1000000.0\t30.0\t0\t\t9=1000.0{extra}\t{name}\n"
        )
    };
    let rows = format!(
        "{}{}{}{}{}\
         type\t{STUFF}\t{GROUP_STUFF}\t4\t1\t1\t{STUFF_M3}\t0.0\t0.0\t1.0\t0\t\t\tInvented Stuff\n\
         type\t{BOX}\t{}\t4\t1\t1\t{STUFF_M3}\t0.0\t0.0\t1.0\t0\t\t\tInvented Box\n",
        crate::typedata::FIXTURE,
        hull(
            CARRIER,
            GROUP_HULL,
            1000.0,
            &format!(";908={SMB_M3};912={HANGAR_M3}"),
            "Invented Carrier"
        ),
        hull(HULL_S, GROUP_HULL, HULL_M3, "", "Invented Hull"),
        hull(CAPSULE_T, GROUP_CAPSULE, 1.0, "", "Invented Capsule"),
        hull(
            MINI,
            GROUP_HULL,
            HULL_M3,
            ";908=1000",
            "Invented Mini Carrier"
        ),
        CONTAINER_GROUPS[0],
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

#[allow(clippy::too_many_arguments)]
fn make(
    c: &crate::content::Content,
    id: i64,
    type_id: i64,
    owner: i64,
    location: i64,
    flag: i64,
    quantity: i64,
    singleton: bool,
) -> Item {
    let t = find_in(c, type_id).expect("a test type");
    Item {
        item_id: id,
        type_id,
        owner_id: owner,
        location_id: location,
        flag_id: flag,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
    }
}

// Items every world has.
const A_STUFF: i64 = 100_660_101; // Alice's stuff, in her cargo hold
const B_STUFF: i64 = 100_660_102; // Bob's stuff, in his cargo hold
const A_HULL: i64 = 100_660_103; // an assembled small ship, hangar (docked world)
const A_PACKED: i64 = 100_660_104; // a packaged one
const A_CAPSULE: i64 = 100_660_105;
const A_BOX: i64 = 100_660_106; // a container (in Alice's fleet hangar when placed)
const STACK: i64 = 500;
/// `containerHangar` [V].
const CONTAINER_HANGAR: i64 = 10_004;

struct World {
    reg: Registry,
    content: std::sync::Arc<crate::content::Content>,
}

fn pilot(id: i64, name: &str, corp: i64) -> Character {
    Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        corporation_id: corp,
        ..Character::default()
    }
}

fn place(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        SYSTEM,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// Four pilots in space in one system, side by side: Alice flies the
/// carrier (her stuff in the hold), Bob a small hull (stuff in the hold),
/// Carol (Alice's corp) and Dave (nobody's) small hulls. Alice and Bob are
/// in one fleet. Every push drained.
fn space() -> World {
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", CORP_A),
            pilot(BOB, "Bob", CORP_B),
            pilot(CAROL, "Carol", CORP_A),
            pilot(DAVE, "Dave", CORP_C),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for who in [ALICE, BOB, CAROL, DAVE] {
        let ty = if who == ALICE { CARRIER } else { HULL_S };
        let t = find_in(&content, ty).expect("a hull");
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = ty;
            i.group_id = t.group_id;
            i.category_id = t.category_id;
            i.singleton = true;
            i.owner_id = who;
        }
        reg.character_mut(who).expect("a pilot").ship_type_id = ty;
    }
    items.push(make(
        &content,
        A_STUFF,
        STUFF,
        ALICE,
        ship(ALICE),
        FLAG_CARGO,
        STACK,
        false,
    ));
    items.push(make(
        &content,
        B_STUFF,
        STUFF,
        BOB,
        ship(BOB),
        FLAG_CARGO,
        STACK,
        false,
    ));
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_661_000,
        ..crate::store::Snapshot::default()
    });
    let spot = [1.0e7, 2.0e6, -3.0e6];
    for who in [ALICE, BOB, CAROL, DAVE] {
        place(&mut reg, who, spot);
    }
    let mut w = World { reg, content };
    w.form_fleet();
    w
}

fn call_fleet(
    reg: &mut Registry,
    actor: i64,
    method: &str,
    args: Vec<PyValue>,
    fleet: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let bind = fleet.map_or(PyValue::None, PyValue::Int);
    reg.dispatch(&Request {
        service: "fleetObjectHandler",
        method,
        args: &args,
        kwargs: &[],
        bind_params: Some(&bind),
        actor: Some(actor),
    })
}

impl World {
    /// Alice creates a fleet and invites Bob, who accepts.
    fn form_fleet(&mut self) {
        let r = &mut self.reg;
        call_fleet(r, ALICE, "CreateFleet", vec![], None).expect("create");
        call_fleet(r, ALICE, "Init", vec![PyValue::Int(670)], None).expect("init");
        let PyValue::Int(id) = call_fleet(r, ALICE, "GetFleetID", vec![], None).expect("id") else {
            panic!("an integer");
        };
        call_fleet(r, ALICE, "Invite", vec![PyValue::Int(BOB)], Some(id)).expect("invite");
        call_fleet(r, BOB, "AcceptInvite", vec![PyValue::Int(670)], Some(id)).expect("accept");
    }

    fn who(&self, id: i64) -> Character {
        self.reg.character(id).expect("a pilot").clone()
    }

    /// Plan and apply, the way `move_item` does, minus the roster refresh
    /// (the test ships do not move).
    fn mv(&mut self, who: i64, dest: Dest, item: i64, qty: Option<i64>) -> Result<(), BayRefusal> {
        let c = self.who(who);
        let edits = plan(&self.reg, &c, dest, item, qty).expect("a move that touches a bay")?;
        self.reg
            .apply_items(&edits)
            .expect("a plan the store takes");
        Ok(())
    }

    fn set(&mut self, who: i64, changes: &[(&str, bool)]) {
        let d = PyValue::Dict(crate::pyutil::pypairs(
            changes
                .iter()
                .map(|(k, v)| (*k, PyValue::Bool(*v)))
                .collect(),
        ));
        configure(&mut self.reg, Some(who), Some(&d)).expect("configured");
    }

    fn item(&self, id: i64) -> Item {
        self.reg.item(id).expect("an item").clone()
    }

    fn units(&self, ship_id: i64, flag: i64) -> i64 {
        self.reg
            .store()
            .items()
            .iter()
            .filter(|i| i.location_id == ship_id && i.flag_id == flag && i.type_id == STUFF)
            .map(|i| i.quantity)
            .sum()
    }
}

fn hangar_of(ship_id: i64) -> Dest {
    Dest::Hold {
        ship_id,
        flag: FLAG_FLEET_HANGAR,
    }
}

fn smb_of(ship_id: i64) -> Dest {
    Dest::Hold {
        ship_id,
        flag: FLAG_SMB,
    }
}

fn key_of(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let PyValue::Bytes(key) = &args[0] else {
        panic!("odd key")
    };
    String::from_utf8_lossy(key).into_owned()
}

fn dict_bool(v: &PyValue, key: &str) -> bool {
    match crate::pyutil::dict_get(v, key.as_bytes()) {
        Some(PyValue::Bool(b)) => *b,
        other => panic!("{key}: {other:?}"),
    }
}

// ---- the config ----

/// The answer carries all four keys the client indexes, bools, and a ship
/// nobody configured is the default.
#[test]
fn config_defaults() {
    let w = space();
    let v = configuration_value(&w.reg, ship(ALICE));
    for key in [
        KEY_FLEET_HANGAR_FLEET,
        KEY_FLEET_HANGAR_CORP,
        KEY_SMB_FLEET,
        KEY_SMB_CORP,
    ] {
        assert!(
            !dict_bool(&v, key),
            "{key} is the default {:?}",
            ShipConfig::DEFAULT
        );
    }
    assert_eq!(stored(&w.reg, ship(ALICE)), ShipConfig::DEFAULT);
    assert!(w.reg.deltas().of(KIND).is_empty(), "a default has no row");
}

/// `ConfigureShip` changes the **caller's active ship** only, one key at a
/// time, ignores keys the client never sends, and a ship back at the default
/// leaves no row.
#[test]
fn only_the_owner_configures() {
    let mut w = space();
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    let a = stored(&w.reg, ship(ALICE));
    assert!(a.fleet_hangar_fleet && !a.fleet_hangar_corp && !a.smb_fleet && !a.smb_corp);
    assert_eq!(
        stored(&w.reg, ship(BOB)),
        ShipConfig::DEFAULT,
        "Bob's ship is not touched"
    );
    // Bob's call configures Bob's ship, never Alice's.
    w.set(BOB, &[(KEY_SMB_CORP, true), ("NotAKey", true)]);
    assert!(stored(&w.reg, ship(BOB)).smb_corp);
    assert_eq!(stored(&w.reg, ship(ALICE)), a);
    // The wire: the answer and the arm.
    let v = w
        .reg
        .dispatch(&Request {
            service: "ship",
            method: "GetShipConfiguration",
            args: &[PyValue::Int(ship(ALICE))],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        })
        .expect("anyone may read it");
    assert!(dict_bool(&v, KEY_FLEET_HANGAR_FLEET));
    let off = PyValue::Dict(crate::pyutil::pypairs(vec![(
        KEY_FLEET_HANGAR_FLEET,
        PyValue::Bool(false),
    )]));
    w.reg
        .dispatch(&Request {
            service: "ship",
            method: "ConfigureShip",
            args: &[off],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("the arm");
    assert_eq!(stored(&w.reg, ship(ALICE)), ShipConfig::DEFAULT);
    assert!(
        !w.reg
            .deltas()
            .of(KIND)
            .contains_key(&vec![ship(ALICE).to_string()]),
        "back at the default, the row goes"
    );
    // A pilot who is not at the controls of his own ship has nothing to set.
    let d = PyValue::Dict(vec![]);
    w.reg.character_mut(DAVE).expect("Dave").ship_id = 1;
    assert!(configure(&mut w.reg, Some(DAVE), Some(&d)).is_err());
}

/// The row is what a restart reads back: the delta store holds the four
/// switches and nothing else.
#[test]
fn the_config_is_a_delta_row() {
    let mut w = space();
    w.set(
        ALICE,
        &[(KEY_SMB_FLEET, true), (KEY_FLEET_HANGAR_CORP, true)],
    );
    let row = w
        .reg
        .deltas()
        .of(KIND)
        .get(&vec![ship(ALICE).to_string()])
        .expect("a row")
        .clone();
    assert_eq!(ShipConfig::from_row(&row), stored(&w.reg, ship(ALICE)));
    assert!(crate::delta_kinds::is_registered(KIND));
}

// ---- the rule ----

/// Each bay reads its own two keys, the corp key needs the same corp and
/// the fleet key the same fleet; nothing else opens a bay.
#[test]
fn the_access_rule_is_per_bay_and_per_route() {
    let all = |f, c| ShipConfig {
        fleet_hangar_fleet: f,
        fleet_hangar_corp: c,
        smb_fleet: false,
        smb_corp: false,
    };
    // fleet route only
    assert!(all(true, false).allows(FLAG_FLEET_HANGAR, false, true));
    assert!(!all(true, false).allows(FLAG_FLEET_HANGAR, true, false));
    // corp route only
    assert!(all(false, true).allows(FLAG_FLEET_HANGAR, true, false));
    assert!(!all(false, true).allows(FLAG_FLEET_HANGAR, false, true));
    // the fleet hangar's keys do not open the maintenance bay
    assert!(!all(true, true).allows(FLAG_SMB, true, true));
    // no relation, no access
    assert!(!all(true, true).allows(FLAG_FLEET_HANGAR, false, false));
    // a flag that is not a bay is never allowed
    assert!(!all(true, true).allows(FLAG_CARGO, true, true));
}

/// A fleet member deposits into a carrier's fleet hangar only once the
/// owner allows it; what he put there is the ship owner's afterwards.
#[test]
fn fleet_member_opens_the_hangar_iff_allowed() {
    let mut w = space();
    let carrier = ship(ALICE);
    let denied = w.mv(BOB, hangar_of(carrier), B_STUFF, Some(10));
    assert!(
        matches!(denied, Err(BayRefusal::NotConfigured)),
        "{denied:?}"
    );
    assert_eq!(
        w.units(carrier, FLAG_FLEET_HANGAR),
        0,
        "a refusal moves nothing"
    );
    // The corp key does not help a fleet member of another corp.
    w.set(ALICE, &[(KEY_FLEET_HANGAR_CORP, true)]);
    assert!(matches!(
        w.mv(BOB, hangar_of(carrier), B_STUFF, Some(10)),
        Err(BayRefusal::NotConfigured)
    ));
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    w.mv(BOB, hangar_of(carrier), B_STUFF, Some(10))
        .expect("allowed now");
    assert_eq!(w.units(carrier, FLAG_FLEET_HANGAR), 10);
    assert_eq!(w.units(ship(BOB), FLAG_CARGO), STACK - 10);
    assert!(
        w.reg
            .store()
            .items()
            .iter()
            .filter(|i| i.location_id == carrier && i.flag_id == FLAG_FLEET_HANGAR)
            .all(|i| i.owner_id == ALICE),
        "a guest's deposit is the ship owner's"
    );
    // And Alice may close it again.
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, false)]);
    assert!(matches!(
        w.mv(BOB, hangar_of(carrier), B_STUFF, Some(10)),
        Err(BayRefusal::NotConfigured)
    ));
}

/// Somebody in neither the corp nor the fleet is refused whatever the
/// switches say; a corp mate outside the fleet needs the corp key.
#[test]
fn strangers_and_corp_mates() {
    let mut w = space();
    let carrier = ship(ALICE);
    // Carol and Dave hold stuff of their own.
    for (who, id) in [(CAROL, 100_660_201), (DAVE, 100_660_202)] {
        let mut items = w.reg.store().items().to_vec();
        items.push(make(
            &w.content,
            id,
            STUFF,
            who,
            ship(who),
            FLAG_CARGO,
            20,
            false,
        ));
        w.reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_662_000,
            ..crate::store::Snapshot::default()
        });
    }
    w.set(
        ALICE,
        &[
            (KEY_FLEET_HANGAR_FLEET, true),
            (KEY_FLEET_HANGAR_CORP, true),
            (KEY_SMB_FLEET, true),
            (KEY_SMB_CORP, true),
        ],
    );
    // Dave: no relation.
    assert!(matches!(
        w.mv(DAVE, hangar_of(carrier), 100_660_202, None),
        Err(BayRefusal::NoAccess)
    ));
    // Carol: same corp, not in the fleet: the corp key opens it.
    w.mv(CAROL, hangar_of(carrier), 100_660_201, None)
        .expect("corp route");
    // With only the fleet key on, Carol is refused: she is related, but not
    // by that route.
    w.set(ALICE, &[(KEY_FLEET_HANGAR_CORP, false)]);
    assert!(matches!(
        w.mv(CAROL, hangar_of(carrier), 100_660_201, None),
        Err(BayRefusal::NotConfigured) | Err(BayRefusal::Refused(_))
    ));
}

/// In space a guest must be inside the client's transfer distance (surface
/// to surface) of the ship.
#[test]
fn a_guest_must_be_within_range() {
    let mut w = space();
    let carrier = ship(ALICE);
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    // Hull radii are the fallback's (1 m each, the table has no radius for
    // an unknown hull): the surface distance is the centre distance less 2.
    let base = w.reg.roster().get(ALICE).expect("Alice").motion.position;
    let at = |d: f64| [base[0] + d, base[1], base[2]];
    let radii = 2.0;
    let near = MAX_TRANSFER_DISTANCE - 10.0 + radii;
    let far = MAX_TRANSFER_DISTANCE + crate::combat::LOOT_RANGE_SLACK + 100.0 + radii;
    place(&mut w.reg, BOB, at(far));
    let r = w.mv(BOB, hangar_of(carrier), B_STUFF, Some(5));
    assert!(matches!(r, Err(BayRefusal::TooFar)), "{r:?}");
    place(&mut w.reg, BOB, at(near));
    w.mv(BOB, hangar_of(carrier), B_STUFF, Some(5))
        .expect("in range");
    // A pilot in another system is out of reach however the rule reads.
    let _ = w.reg.roster_mut().leave(BOB);
    w.reg.roster_mut().arrive(Presence::arriving(
        BOB,
        ship(BOB),
        SYSTEM + 1,
        Motion::arrived(base, [0.0; 3]),
    ));
    assert!(matches!(
        w.mv(BOB, hangar_of(carrier), B_STUFF, Some(5)),
        Err(BayRefusal::TooFar)
    ));
}

/// A fleet hangar holds what the hull's `fleetHangarCapacity` says, by the
/// client's own arithmetic; a hull without one takes nothing.
#[test]
fn the_fleet_hangar_has_a_capacity() {
    let mut w = space();
    let carrier = ship(ALICE);
    let cap = (HANGAR_M3 / STUFF_M3) as i64;
    assert!(matches!(
        w.mv(ALICE, hangar_of(carrier), A_STUFF, Some(cap + 1)),
        Err(BayRefusal::Refused(Refusal::NoRoom { .. }))
    ));
    assert_eq!(w.units(carrier, FLAG_FLEET_HANGAR), 0);
    w.mv(ALICE, hangar_of(carrier), A_STUFF, Some(cap))
        .expect("exactly full");
    assert_eq!(w.units(carrier, FLAG_FLEET_HANGAR), cap);
    assert!(matches!(
        w.mv(ALICE, hangar_of(carrier), A_STUFF, Some(1)),
        Err(BayRefusal::Refused(Refusal::NoRoom { .. }))
    ));
    // Bob's hull has no hangar: even his own stuff has no room in it.
    assert!(matches!(
        w.mv(BOB, hangar_of(ship(BOB)), B_STUFF, Some(1)),
        Err(BayRefusal::Refused(Refusal::NoRoom { .. }))
    ));
}

/// Items come back out of the hangar into the owner's cargo (space), and a
/// guest may take out of a fleet hangar he has access to: they become his.
#[test]
fn things_come_out_of_the_fleet_hangar() {
    let mut w = space();
    let carrier = ship(ALICE);
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    w.mv(ALICE, hangar_of(carrier), A_STUFF, Some(30))
        .expect("in");
    let in_bay = w
        .reg
        .store()
        .items()
        .iter()
        .find(|i| i.location_id == carrier && i.flag_id == FLAG_FLEET_HANGAR)
        .expect("the stack")
        .item_id;
    // Alice takes 10 back into her hold, in space.
    w.mv(
        ALICE,
        Dest::Hold {
            ship_id: carrier,
            flag: FLAG_CARGO,
        },
        in_bay,
        Some(10),
    )
    .expect("out");
    assert_eq!(w.units(carrier, FLAG_FLEET_HANGAR), 20);
    // Bob takes the rest into his hold: now Bob's.
    w.mv(
        BOB,
        Dest::Hold {
            ship_id: ship(BOB),
            flag: FLAG_CARGO,
        },
        in_bay,
        None,
    )
    .expect("a guest takes");
    assert_eq!(w.units(carrier, FLAG_FLEET_HANGAR), 0);
    assert_eq!(w.item(in_bay).owner_id, BOB);
    // Once the owner closes it, he cannot.
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, false)]);
    let mut items = w.reg.store().items().to_vec();
    items.push(make(
        &w.content,
        100_660_301,
        STUFF,
        ALICE,
        carrier,
        FLAG_FLEET_HANGAR,
        5,
        false,
    ));
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
    assert!(matches!(
        w.mv(
            BOB,
            Dest::Hold {
                ship_id: ship(BOB),
                flag: FLAG_CARGO
            },
            100_660_301,
            None
        ),
        Err(BayRefusal::NotConfigured)
    ));
}

/// A container in a fleet hangar is moved by the captain alone [V].
#[test]
fn a_container_in_the_hangar_is_the_captains() {
    let mut w = space();
    let carrier = ship(ALICE);
    let mut items = w.reg.store().items().to_vec();
    items.push(make(
        &w.content,
        A_BOX,
        BOX,
        ALICE,
        carrier,
        FLAG_FLEET_HANGAR,
        1,
        true,
    ));
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    let bob_hold = Dest::Hold {
        ship_id: ship(BOB),
        flag: FLAG_CARGO,
    };
    let r = w.mv(BOB, bob_hold, A_BOX, None);
    assert!(matches!(r, Err(BayRefusal::ContainerNotYours)), "{r:?}");
    w.mv(
        ALICE,
        Dest::Hold {
            ship_id: carrier,
            flag: FLAG_CARGO,
        },
        A_BOX,
        None,
    )
    .expect("the captain moves it");
}

/// The keys are the client's: the access refusals, the container refusal,
/// the ship-type refusal.
#[test]
fn the_refusals_are_the_clients_keys() {
    for (r, key) in [
        (BayRefusal::NoAccess, "CannotAccessShipHangar"),
        (BayRefusal::NotConfigured, "FleetHangarAccessDenied"),
        (
            BayRefusal::ContainerNotYours,
            "CannotTakeContainerFromFleetHangar",
        ),
        (BayRefusal::NotAShip, "CannotAddShipTypeToMaintainanceBay"),
    ] {
        assert_eq!(key_of(&r.to_error()), key);
    }
}

// ---- the listing ----

/// The owner sees everything in his bay, an admitted guest sees it too, a
/// stranger sees an empty bay, and a flag that is not a bay is not ours.
#[test]
fn the_bay_listing_follows_the_rule() {
    let mut w = space();
    let carrier = ship(ALICE);
    w.mv(ALICE, hangar_of(carrier), A_STUFF, Some(10))
        .expect("in");
    fn count(w: &World, who: i64, carrier: i64, flag: Option<i64>) -> Option<usize> {
        bay_listing(&w.reg, who, carrier, flag).map(|v| v.len())
    }
    let n = |w: &World, who, flag| count(w, who, carrier, flag);
    assert_eq!(n(&w, ALICE, Some(FLAG_FLEET_HANGAR)), Some(1));
    assert_eq!(
        n(&w, BOB, Some(FLAG_FLEET_HANGAR)),
        Some(0),
        "not allowed yet"
    );
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    assert_eq!(n(&w, BOB, Some(FLAG_FLEET_HANGAR)), Some(1));
    assert_eq!(n(&w, DAVE, Some(FLAG_FLEET_HANGAR)), Some(0));
    assert_eq!(
        n(&w, BOB, Some(FLAG_SMB)),
        Some(0),
        "the other bay's keys are its own"
    );
    assert_eq!(n(&w, ALICE, Some(FLAG_CARGO)), None);
    assert_eq!(n(&w, ALICE, None), None);
    assert!(can_open_any(&w.reg, BOB, carrier));
    assert!(!can_open_any(&w.reg, DAVE, carrier));
    // The wire: `List(flag=155)` on the carrier, as Bob.
    let bind = PyValue::Tuple(vec![PyValue::Int(carrier), PyValue::Int(1)]);
    let kw = crate::pyutil::pypairs(vec![("flag", PyValue::Int(FLAG_FLEET_HANGAR))]);
    let rows = w
        .reg
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "List",
            args: &[],
            kwargs: &kw,
            bind_params: Some(&bind),
            actor: Some(BOB),
        })
        .expect("listed");
    let PyValue::List(rows) = rows else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
}

// ---- the maintenance bay, docked ----

/// Alice docked in the carrier with an assembled small ship, a packaged
/// one, a pod and some stuff in the hangar.
fn docked() -> World {
    let content = std::sync::Arc::new(content());
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        station_id: Some(STATION),
        corporation_id: CORP_A,
        ..Character::default()
    };
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![alice],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    items.push(make(
        &content,
        ship(ALICE),
        CARRIER,
        ALICE,
        STATION,
        FLAG_HANGAR,
        1,
        true,
    ));
    items.push(make(
        &content,
        A_HULL,
        HULL_S,
        ALICE,
        STATION,
        FLAG_HANGAR,
        1,
        true,
    ));
    items.push(make(
        &content,
        A_PACKED,
        HULL_S,
        ALICE,
        STATION,
        FLAG_HANGAR,
        1,
        false,
    ));
    items.push(make(
        &content,
        A_CAPSULE,
        CAPSULE_T,
        ALICE,
        STATION,
        FLAG_HANGAR,
        1,
        true,
    ));
    items.push(make(
        &content,
        A_STUFF,
        STUFF,
        ALICE,
        STATION,
        FLAG_HANGAR,
        STACK,
        false,
    ));
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_661_000,
        ..crate::store::Snapshot::default()
    });
    let c = reg.character_mut(ALICE).expect("Alice");
    c.ship_id = ship(ALICE);
    c.ship_type_id = CARRIER;
    World { reg, content }
}

/// An assembled ship goes into the active carrier's maintenance bay from the
/// hangar, stays its owner's, counts against the bay, and comes back out to
/// the hangar still assembled.
#[test]
fn an_assembled_ship_goes_in_and_out_of_the_maintenance_bay() {
    let mut w = docked();
    let carrier = ship(ALICE);
    w.mv(ALICE, smb_of(carrier), A_HULL, None).expect("in");
    let i = w.item(A_HULL);
    assert_eq!(
        (i.location_id, i.flag_id, i.owner_id, i.singleton),
        (carrier, FLAG_SMB, ALICE, true)
    );
    assert_eq!(
        w.reg
            .store()
            .used_volume_in(w.reg.content(), carrier, FLAG_SMB),
        HULL_M3
    );
    w.mv(ALICE, Dest::Hangar, A_HULL, None).expect("out");
    let i = w.item(A_HULL);
    assert_eq!(
        (i.location_id, i.flag_id, i.singleton),
        (STATION, FLAG_HANGAR, true)
    );
}

/// The bay is for ships: a pod and a plain item are refused with the
/// client's ship-type key, a packaged ship is refused with a word, and the
/// bay's capacity is the hull's.
#[test]
fn the_maintenance_bay_takes_assembled_ships_only() {
    let mut w = docked();
    let carrier = ship(ALICE);
    assert!(matches!(
        w.mv(ALICE, smb_of(carrier), A_CAPSULE, None),
        Err(BayRefusal::NotAShip)
    ));
    assert!(matches!(
        w.mv(ALICE, smb_of(carrier), A_STUFF, Some(1)),
        Err(BayRefusal::NotAShip)
    ));
    assert!(matches!(
        w.mv(ALICE, smb_of(carrier), A_PACKED, None),
        Err(BayRefusal::Refused(Refusal::Unsupported(_)))
    ));
    // The bay holds SMB_M3 / HULL_M3 hulls and not one more.
    let room = (SMB_M3 / HULL_M3) as i64;
    let mut items = w.reg.store().items().to_vec();
    for n in 0..room {
        items.push(make(
            &w.content,
            100_660_400 + n,
            HULL_S,
            ALICE,
            STATION,
            FLAG_HANGAR,
            1,
            true,
        ));
    }
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
    for n in 0..room {
        w.mv(ALICE, smb_of(carrier), 100_660_400 + n, None)
            .expect("fits");
    }
    assert!(matches!(
        w.mv(ALICE, smb_of(carrier), A_HULL, None),
        Err(BayRefusal::Refused(Refusal::NoRoom { .. }))
    ));
}

/// A ship cannot go into its own bay, nor a pair of carriers into each
/// other's; the active ship goes nowhere.
#[test]
fn a_ship_cannot_be_inside_itself() {
    let mut w = docked();
    let carrier = ship(ALICE);
    assert!(matches!(
        w.mv(ALICE, smb_of(carrier), carrier, None),
        Err(BayRefusal::Refused(Refusal::ActiveShip))
    ));
    // A second carrier goes into the first's bay; then the pilot sits in a
    // small hull and the first carrier, parked, cannot go into the second.
    let second = 100_660_500;
    let mut items = w.reg.store().items().to_vec();
    items.push(make(
        &w.content,
        second,
        MINI,
        ALICE,
        STATION,
        FLAG_HANGAR,
        1,
        true,
    ));
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
    w.mv(ALICE, smb_of(carrier), second, None).expect("in");
    w.reg.character_mut(ALICE).expect("Alice").ship_id = A_HULL;
    assert!(matches!(
        w.mv(ALICE, smb_of(second), carrier, None),
        Err(BayRefusal::Refused(Refusal::InsideItself))
    ));
}

/// Docked, the move goes through the arms: an `Add` with `flag=90` on the
/// carrier's inventory puts the ship in the bay and an `Add` with `flag=4`
/// on the hangar's takes it out.
#[test]
fn the_arms_route_the_bays() {
    let mut w = docked();
    let carrier = ship(ALICE);
    let bind = PyValue::Tuple(vec![PyValue::Int(carrier), PyValue::Int(1)]);
    let kw = crate::pyutil::pypairs(vec![
        ("qty", PyValue::None),
        ("flag", PyValue::Int(FLAG_SMB)),
    ]);
    w.reg
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "Add",
            args: &[PyValue::Int(A_HULL), PyValue::Int(STATION)],
            kwargs: &kw,
            bind_params: Some(&bind),
            actor: Some(ALICE),
        })
        .expect("into the bay");
    assert_eq!(w.item(A_HULL).flag_id, FLAG_SMB);
    let hangar = PyValue::Int(CONTAINER_HANGAR);
    let kw = crate::pyutil::pypairs(vec![
        ("qty", PyValue::None),
        ("flag", PyValue::Int(FLAG_HANGAR)),
    ]);
    w.reg
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "Add",
            args: &[PyValue::Int(A_HULL), PyValue::Int(carrier)],
            kwargs: &kw,
            bind_params: Some(&hangar),
            actor: Some(ALICE),
        })
        .expect("out of the bay");
    assert_eq!(w.item(A_HULL).flag_id, FLAG_HANGAR);
}

/// A guest's ship in the bay stays the guest's: the carrier's owner cannot
/// take it out [A], the guest can, and an assembled ship does not belong in
/// a fleet hangar.
#[test]
fn a_ferried_ship_stays_its_owners() {
    let mut w = space();
    let carrier = ship(ALICE);
    // Bob's assembled hull is in his own pilot seat (not movable); make a
    // second one in his hold? An assembled ship is not cargo, so it sits in
    // Bob's ship at a bay-less flag only in this test: place it directly.
    let guest_hull = 100_660_600;
    let mut items = w.reg.store().items().to_vec();
    items.push(make(
        &w.content, guest_hull, HULL_S, BOB, carrier, FLAG_SMB, 1, true,
    ));
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
    w.set(ALICE, &[(KEY_SMB_FLEET, true)]);
    // Alice (the carrier's owner) may not take Bob's ship out. In space the
    // way out is her cargo hold, which refuses assembled ships; the rule
    // under test is the owner check, which comes first.
    let alice_hold = Dest::Hold {
        ship_id: carrier,
        flag: FLAG_CARGO,
    };
    assert!(matches!(
        w.mv(ALICE, alice_hold, guest_hull, None),
        Err(BayRefusal::NoAccess)
    ));
    // Bob may reach it, and an assembled ship is refused cargo with the
    // client's own key.
    let bob_hold = Dest::Hold {
        ship_id: ship(BOB),
        flag: FLAG_CARGO,
    };
    let r = w.mv(BOB, bob_hold, guest_hull, None);
    assert!(
        matches!(r, Err(BayRefusal::Refused(Refusal::AssembledShip))),
        "{r:?}"
    );
    // And no assembled ship goes into a fleet hangar.
    w.set(ALICE, &[(KEY_FLEET_HANGAR_FLEET, true)]);
    assert!(matches!(
        w.mv(BOB, hangar_of(carrier), guest_hull, None),
        Err(BayRefusal::Refused(Refusal::AssembledShip))
    ));
}

/// A move that touches no bay is not this module's: the caller's own rules
/// apply.
#[test]
fn a_plain_move_is_left_alone() {
    let w = docked();
    let c = w.who(ALICE);
    let cargo = Dest::Hold {
        ship_id: ship(ALICE),
        flag: FLAG_CARGO,
    };
    assert!(plan(&w.reg, &c, cargo, A_STUFF, None).is_none());
    assert!(plan(&w.reg, &c, Dest::Hangar, A_STUFF, None).is_none());
    assert!(plan(&w.reg, &c, hangar_of(ship(ALICE)), 1, None).is_none());
}
