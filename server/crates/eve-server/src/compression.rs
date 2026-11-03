//! **In-space compression** (PROG-MINE): `inSpaceCompressionMgr`, and the
//! slim attribute that makes a ship a compression facility. Lane note
//! `docs/lanes/prog-mine.md`.
//!
//! What the client does, read from its own code **[V]**
//! (`itemcompression/client/*`, `menucheckers/itemCheckers.py`):
//!
//! - **The facility is a ship.** `GetFacilitiesInRange` walks the ballpark's
//!   slim items: the pilot's own ship (distance 0), and every other ball
//!   whose slim has `compression_facility_typelists` and whose `charID` is
//!   a fleet member. That attribute is a dict `{typeListID:
//!   activationRange}` (metres, `distance > activationRange` skips it);
//!   [`MAX_FACILITY_ACTIVATION_RANGE`] 250 km is the client's ceiling.
//! - **The call.** `sm.RemoteSvc('inSpaceCompressionMgr')
//!   .CompressItemInSpace(itemID, facilityBallID)` answers `None` for a
//!   refusal (the client then says `FailureToCompress`) or the six-tuple
//!   `(sourceItemID, sourceTypeID, sourceQuantity, outputItemID,
//!   outputTypeID, outputQuantity)`.
//! - **Enabled by default**: `globalConfig.IsMaterialCompressionEnabled`
//!   defaults to `True` and this server never sends the config.
//! - **A compressor module** is group 4174 **[V-data]**, effect 8364
//!   (`effectIndustrialItemCompression`), attribute 3255
//!   (`attributeCompressibleItemsTypeList`) names the typelist it takes
//!   (330 moon ore, 331 ice, 332 gas, 333 mercoxit, 334 asteroid ore) and
//!   attribute 54 (`maxRange`) is the facility range (44 km medium, 83 km
//!   large, 144 km capital) **[V-data]**; `canFitShipType1` (1302) names the
//!   hull (Porpoise, Orca, Rorqual) and the fitting validator enforces it.
//! - **The station and structure path** (`structureCompressionMgr`) needs an
//!   Upwell structure; this server has none, so a station compresses
//!   nothing. In space is the only compression there is.
//!
//! What is **[I]** (inferred from the data, no server code to read): a unit
//! of compressed ore replaces **one** unit of ore -- `types/compress.tsv`'s
//! compressed twin refines to exactly what the raw ore does per portion
//! (`types/materials.tsv`, both portion 100) and is a hundredth of the
//! volume per unit **[V-data]** (`moon.rs` header). So compression is
//! 1:1 in count and 100:1 in volume. Falsified by a client whose compressed
//! stack's refine yield is not the source's. The *ratio* of the volumes is
//! also what makes compression worth a trip: a Venture's 5,000 m3 ore hold
//! holds 100 times the ore once compressed.
//!
//! **[A]** the facility must have the compressor **fitted and online**
//! (a module that is not online does nothing); that the facility and the
//! pilot share a system and, unless it is the pilot's own ship, a fleet;
//! that the pilot is within the facility's range at the call. The client
//! checks the range itself, so the server's check only stops a modified
//! client. Falsified by a retail capture of a refusal the client does not
//! pre-empt.
//!
//! **Wiring (coordinator rows):** `lib.rs` `pub mod compression;`;
//! `service.rs` `DOMAINS` row `(compression::SERVICES,
//! compression::dispatch)`; `slim_extra::SOURCES` row
//! `crate::compression::slim_extras`.

use crate::item::{Edit, Slot};
use crate::service::{Registry, Request, ServiceError};
use crate::slim_extra::SlimExtras;
use eve_marshal::PyValue;

/// The one service this file answers.
pub const SERVICES: &[&str] = &["inSpaceCompressionMgr"];

/// `itemcompression.MAX_FACILITY_ACTIVATION_RANGE` **[V]**, metres.
pub const MAX_FACILITY_ACTIVATION_RANGE: f64 = 250_000.0;
/// `itemcompression.SLIM_ATTRIBUTE_COMPRESSION_FACILITY_TYPELISTS` **[V]**.
pub const SLIM_KEY: &str = "compression_facility_typelists";
/// The compressor modules' group **[V-data]**.
pub const GROUP_COMPRESSOR: i64 = 4174;
/// `effectIndustrialItemCompression` **[V]** (`dogma/const.py`).
pub const EFFECT_COMPRESSION: i64 = 8364;
/// `attributeCompressibleItemsTypeList` **[V]** (`dogma/const.py`).
pub const ATTR_TYPELIST: i64 = 3255;
/// `maxRange` **[V]**.
pub const ATTR_RANGE: i64 = 54;
/// The holds a pilot's ore may sit in **[V]**
/// (`compress_in_space_controller.HARVESTABLE_ORE_HOLDS`): cargo, general
/// mining hold, asteroid (ore) hold, ice hold, gas hold. Flag ids:
/// `flagCargo` 5, `flagGeneralMiningHold` 134, `flagSpecializedAsteroidHold`
/// 137, `flagSpecializedIceHold` 181, `flagSpecializedGasHold` 135.
pub const ORE_HOLD_FLAGS: [i64; 5] = [5, 134, 137, 181, 135];
/// Volume of a compressed unit over a raw unit **[V-data]** (a hundredth).
pub const VOLUME_RATIO: f64 = 0.01;

/// One compressor fitted to a ship: the typelist it takes and its range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facility {
    pub module_id: i64,
    pub type_list: i64,
    pub range: f64,
}

/// **The compressors fitted and online on `ship_id`**, from the type data
/// (`Installed`), none when the content has no type data.
pub fn facilities_on(w: &Registry, ship_id: i64) -> Vec<Facility> {
    let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
        return Vec::new();
    };
    crate::modules::slim_modules(w.store(), ship_id)
        .into_iter()
        .filter(|m| w.modules().is_online(m.item_id))
        .filter_map(|m| {
            let row = td.table().get(m.type_id)?;
            if !row.has_effect(EFFECT_COMPRESSION) {
                return None;
            }
            let list = row.attribute(ATTR_TYPELIST)? as i64;
            let range = row
                .attribute(ATTR_RANGE)
                .unwrap_or(0.0)
                .min(MAX_FACILITY_ACTIVATION_RANGE);
            Some(Facility {
                module_id: m.item_id,
                type_list: list,
                range,
            })
        })
        .collect()
}

/// **The slim-extra source** (`slim_extra::SOURCES`): a pilot's ship with a
/// compressor fitted and online carries `compression_facility_typelists`
/// `{typeListID: range}`, the larger range where two compressors take one
/// list.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> SlimExtras {
    ids.iter()
        .filter_map(|id| {
            w.roster().by_ship(*id)?;
            let mut lists: Vec<(i64, f64)> = Vec::new();
            for f in facilities_on(w, *id) {
                match lists.iter_mut().find(|(l, _)| *l == f.type_list) {
                    Some(e) => e.1 = e.1.max(f.range),
                    None => lists.push((f.type_list, f.range)),
                }
            }
            if lists.is_empty() {
                return None;
            }
            lists.sort_by_key(|(l, _)| *l);
            Some((
                *id,
                vec![(
                    SLIM_KEY,
                    PyValue::Dict(
                        lists
                            .into_iter()
                            .map(|(l, r)| (PyValue::Real(r), PyValue::Int(l)))
                            .collect(),
                    ),
                )],
            ))
        })
        .collect()
}

/// Why a compression was refused (the client sees `None` for every one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotInSpace,
    NoSuchItem,
    /// The item is not in the pilot's ship, not the pilot's, or in a flag
    /// that is not an ore hold.
    NotInHold,
    /// The type has no compressed twin.
    NotCompressible,
    NoFacility,
    /// The facility is not here, not the pilot's own and not a fleet mate.
    FacilityNotHere,
    /// No online compressor on the facility takes this type.
    NoCompressor,
    OutOfRange,
    /// The store refused the plan.
    Store,
}

/// The result tuple the client unpacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    pub source_item: i64,
    pub source_type: i64,
    pub source_quantity: i64,
    pub output_item: i64,
    pub output_type: i64,
    pub output_quantity: i64,
}

impl Done {
    pub fn to_pyvalue(self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.source_item),
            PyValue::Int(self.source_type),
            PyValue::Int(self.source_quantity),
            PyValue::Int(self.output_item),
            PyValue::Int(self.output_type),
            PyValue::Int(self.output_quantity),
        ])
    }
}

/// **Compress `item_id` in `actor`'s ship at the facility ball `ball`.**
pub fn compress(w: &mut Registry, actor: i64, item_id: i64, ball: i64) -> Result<Done, Refusal> {
    let me = w.roster().get(actor).copied().ok_or(Refusal::NotInSpace)?;
    let it = w.store().get(item_id).ok_or(Refusal::NoSuchItem)?.clone();
    if it.owner_id != actor || it.location_id != me.ship_id || it.singleton {
        return Err(Refusal::NotInHold);
    }
    if !ORE_HOLD_FLAGS.contains(&it.flag_id) {
        return Err(Refusal::NotInHold);
    }
    let twin = w
        .content()
        .try_get::<crate::reprocess::Table>()
        .and_then(|t| t.compressed_type(it.type_id))
        .ok_or(Refusal::NotCompressible)?;
    let fac = w
        .roster()
        .by_ship(ball)
        .copied()
        .ok_or(Refusal::NoFacility)?;
    let same_fleet = || {
        w.fleets().fleet_of(actor).is_some_and(|f| {
            f.members.contains_key(&actor) && f.members.contains_key(&fac.character_id)
        })
    };
    if fac.solar_system_id != me.solar_system_id || (fac.ship_id != me.ship_id && !same_fleet()) {
        return Err(Refusal::FacilityNotHere);
    }
    let lists = w.content().try_get::<crate::dungeon_data::Table>();
    let ok = facilities_on(w, ball).into_iter().filter(|f| {
        lists
            .and_then(|t| t.typelists.get(&f.type_list))
            .is_some_and(|l| l.contains(it.type_id, it.group_id, it.category_id))
    });
    let best = ok
        .map(|f| f.range)
        .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r))))
        .ok_or(Refusal::NoCompressor)?;
    if fac.ship_id != me.ship_id {
        w.advance_character(actor);
        w.advance_character(fac.character_id);
        let (a, b) = (
            w.roster()
                .get(actor)
                .map_or([0.0; 3], |p| p.motion.position),
            w.roster()
                .get(fac.character_id)
                .map_or([0.0; 3], |p| p.motion.position),
        );
        if crate::space_state::distance(a, b) > best {
            return Err(Refusal::OutOfRange);
        }
    }
    let slot = Slot::new(actor, me.ship_id, it.flag_id);
    let stack = w
        .store()
        .at(slot)
        .filter(|i| i.type_id == twin && !i.singleton)
        .map(|i| i.item_id)
        .min();
    let quantity = it.quantity;
    let edits = [
        Edit::Destroy { item_id },
        match stack {
            Some(item_id) => Edit::Grow { item_id, quantity },
            None => Edit::Create {
                type_id: twin,
                quantity,
                to: slot,
            },
        },
    ];
    w.apply_items(&edits).map_err(|_| Refusal::Store)?;
    let output_item = w
        .store()
        .at(slot)
        .filter(|i| i.type_id == twin && !i.singleton)
        .map(|i| i.item_id)
        .min()
        .unwrap_or(0);
    Ok(Done {
        source_item: item_id,
        source_type: it.type_id,
        source_quantity: quantity,
        output_item,
        output_type: twin,
        output_quantity: quantity,
    })
}

/// `inSpaceCompressionMgr.CompressItemInSpace(itemID, facilityBallID)`.
pub fn dispatch(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "CompressItemInSpace" => {
            let (Some(actor), [PyValue::Int(item), PyValue::Int(ball), ..]) = (req.actor, req.args)
            else {
                return Ok(PyValue::None);
            };
            Ok(compress(w, actor, *item, *ball).map_or(PyValue::None, Done::to_pyvalue))
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::reprocess::fixture::{ORE_MIXED, ORE_MIXED_COMPRESSED};
    use std::time::Duration;

    const ALICE: i64 = 1_591_001;
    const BOB: i64 = 1_591_002;
    const CAROL: i64 = 1_591_003;
    const ALICE_SHIP: i64 = 159_100_001;
    const BOB_SHIP: i64 = 159_100_002;
    const CAROL_SHIP: i64 = 159_100_003;
    /// An invented compressor type, its fitted item on Bob's ship and
    /// Alice's, and the typelist it takes.
    const COMPRESSOR_T: i64 = 990_641;
    const BOB_MODULE: i64 = 159_100_011;
    const ALICE_MODULE: i64 = 159_100_012;
    const LIST: i64 = 990_642;
    const ORE_GROUP: i64 = 99_001;
    const HULL_T: i64 = 990_643;
    const RANGE: f64 = 83_000.0;
    const HI_SLOT_0: i64 = 27;
    const STACK: i64 = 159_100_021;
    const STACK_QUANTITY: i64 = 5_000;

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Alice, Bob and Carol in one system. Alice and Bob are in one fleet,
    /// Bob 40 km out with an online compressor; Carol is in no fleet.
    /// Alice holds a stack of the fixture's compressible ore in cargo and
    /// has a compressor fitted **offline** (so her own ship is no facility
    /// until a test onlines it).
    fn world() -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let rows = format!(
            "type\t{ORE_MIXED}\t{ORE_GROUP}\t25\t1\t100\t0.35\t0.0\t0.0\t1.0\t0\t\t\tFixture Ore\n\
             type\t{ORE_MIXED_COMPRESSED}\t{ORE_GROUP}\t25\t1\t100\t0.0035\t0.0\t0.0\t1.0\t0\t\t\tFixture Compressed Ore\n\
             type\t{COMPRESSOR_T}\t4174\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12,16,{EFFECT_COMPRESSION}\t{ATTR_TYPELIST}={LIST}.0;{ATTR_RANGE}={RANGE:?}\tFixture Compressor\n\
             type\t{HULL_T}\t25\t6\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Hull\n"
        );
        let table = crate::typedata::Table::parse(&format!("{}{rows}", crate::typedata::FIXTURE))
            .expect("the rows parse");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut dungeons = crate::dungeon_data::Table::fixture();
        dungeons.typelists.insert(
            LIST,
            crate::dungeon_data::TypeList {
                inc_groups: vec![ORE_GROUP],
                ..Default::default()
            },
        );
        content.insert("dungeons", dungeons);
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
            ],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        let item = |item_id, type_id, owner, location, flag, quantity, group, category| {
            crate::item::Item {
                item_id,
                type_id,
                owner_id: owner,
                location_id: location,
                flag_id: flag,
                quantity,
                group_id: group,
                category_id: category,
                singleton: quantity == 1 && category != 25,
            }
        };
        let mut items = reg.store().items().to_vec();
        items.extend([
            item(
                BOB_MODULE,
                COMPRESSOR_T,
                BOB,
                BOB_SHIP,
                HI_SLOT_0,
                1,
                4174,
                7,
            ),
            item(
                ALICE_MODULE,
                COMPRESSOR_T,
                ALICE,
                ALICE_SHIP,
                HI_SLOT_0,
                1,
                4174,
                7,
            ),
            item(
                STACK,
                ORE_MIXED,
                ALICE,
                ALICE_SHIP,
                5,
                STACK_QUANTITY,
                ORE_GROUP,
                25,
            ),
        ]);
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        for c in [ALICE, BOB, CAROL] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [(ALICE, 0.0), (BOB, 40_000.0), (CAROL, 1_000.0)] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
            p.since = crate::space_state::now_seconds();
        }
        reg.modules_mut().set_online(ALICE_MODULE, false);
        let id = reg.fleets_mut().create(ALICE).expect("created");
        reg.fleets_mut()
            .init(ALICE, id, Some(HULL_T), None, None)
            .expect("init");
        reg.fleets_mut()
            .invite(ALICE, id, BOB, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(BOB, id, Some(HULL_T), None, None)
            .expect("accepted");
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    fn ore_of(reg: &Registry, ship: i64, type_id: i64) -> i64 {
        reg.store()
            .contents_of(ship)
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    #[test]
    fn a_fleet_mates_compressor_compresses_one_for_one() {
        let mut reg = world();
        let done = compress(&mut reg, ALICE, STACK, BOB_SHIP).expect("compresses");
        assert_eq!(
            (done.source_type, done.source_quantity),
            (ORE_MIXED, STACK_QUANTITY)
        );
        assert_eq!(
            (done.output_type, done.output_quantity),
            (ORE_MIXED_COMPRESSED, STACK_QUANTITY)
        );
        assert_eq!(ore_of(&reg, ALICE_SHIP, ORE_MIXED), 0);
        assert_eq!(
            ore_of(&reg, ALICE_SHIP, ORE_MIXED_COMPRESSED),
            STACK_QUANTITY
        );
        // The output lands in the stack's own hold, a hundredth the volume.
        let out = reg.store().get(done.output_item).expect("the output stack");
        assert_eq!((out.location_id, out.flag_id), (ALICE_SHIP, 5));
        let used = reg.store().used_volume_in(reg.content(), ALICE_SHIP, 5);
        assert!(
            (used - STACK_QUANTITY as f64 * 0.0035).abs() < 1e-6,
            "{used}"
        );
    }

    #[test]
    fn a_second_compression_grows_the_stack_instead_of_adding_one() {
        let mut reg = world();
        let first = compress(&mut reg, ALICE, STACK, BOB_SHIP).expect("compresses");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: 159_100_022,
            type_id: ORE_MIXED,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: 5,
            quantity: 100,
            group_id: ORE_GROUP,
            category_id: 25,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        let second = compress(&mut reg, ALICE, 159_100_022, BOB_SHIP).expect("again");
        assert_eq!(second.output_item, first.output_item);
        assert_eq!(second.output_quantity, 100);
        assert_eq!(
            ore_of(&reg, ALICE_SHIP, ORE_MIXED_COMPRESSED),
            STACK_QUANTITY + 100
        );
    }

    #[test]
    fn the_refusals_are_the_clients_none() {
        let mut reg = world();
        // Out of range: 40 km is inside 83 km; move Bob past it.
        reg.roster_mut().get_mut(BOB).expect("Bob").motion =
            crate::space_state::Motion::arrived([9.0e6, 0.0, 0.0], [0.0; 3]);
        assert_eq!(
            compress(&mut reg, ALICE, STACK, BOB_SHIP),
            Err(Refusal::OutOfRange)
        );
        // Her own compressor is fitted but offline.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, ALICE_SHIP),
            Err(Refusal::NoCompressor)
        );
        // Carol is in no fleet and has no compressor.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, CAROL_SHIP),
            Err(Refusal::FacilityNotHere)
        );
        // Not a facility ball at all.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, 42),
            Err(Refusal::NoFacility)
        );
        // Not hers, and not an ore.
        assert_eq!(
            compress(&mut reg, BOB, STACK, BOB_SHIP),
            Err(Refusal::NotInHold)
        );
        assert_eq!(
            compress(&mut reg, ALICE, ALICE_MODULE, BOB_SHIP),
            Err(Refusal::NotInHold)
        );
        assert_eq!(ore_of(&reg, ALICE_SHIP, ORE_MIXED), STACK_QUANTITY);
    }

    #[test]
    fn a_pilot_compresses_in_his_own_online_compressor_and_a_compressed_stack_does_not_compress_again()
     {
        let mut reg = world();
        reg.modules_mut().set_online(ALICE_MODULE, true);
        let done = compress(&mut reg, ALICE, STACK, ALICE_SHIP).expect("own facility");
        assert_eq!(
            compress(&mut reg, ALICE, done.output_item, ALICE_SHIP),
            Err(Refusal::NotCompressible)
        );
    }

    #[test]
    fn only_an_online_compressor_makes_a_slim_facility_with_its_range() {
        let mut reg = world();
        let lists = |reg: &Registry, ship: i64| slim_extras(reg, &[ship]);
        assert!(lists(&reg, ALICE_SHIP).is_empty(), "offline");
        assert!(lists(&reg, CAROL_SHIP).is_empty(), "no module");
        assert_eq!(
            lists(&reg, BOB_SHIP),
            vec![(
                BOB_SHIP,
                vec![(
                    SLIM_KEY,
                    PyValue::Dict(vec![(PyValue::Real(RANGE), PyValue::Int(LIST))])
                )]
            )]
        );
        reg.modules_mut().set_online(ALICE_MODULE, true);
        assert_eq!(lists(&reg, ALICE_SHIP).len(), 1);
    }

    #[test]
    fn the_service_answers_none_for_a_refusal_and_the_tuple_for_a_success() {
        let mut reg = world();
        let call = |reg: &mut Registry, item: i64, ball: i64| {
            let args = [PyValue::Int(item), PyValue::Int(ball)];
            let req = Request {
                service: "inSpaceCompressionMgr",
                method: "CompressItemInSpace",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            };
            dispatch(reg, &req).expect("answers")
        };
        assert_eq!(call(&mut reg, STACK, CAROL_SHIP), PyValue::None);
        let PyValue::Tuple(t) = call(&mut reg, STACK, BOB_SHIP) else {
            panic!("a tuple");
        };
        assert_eq!(t.len(), 6);
        assert_eq!(t[0], PyValue::Int(STACK));
        assert_eq!(t[5], PyValue::Int(STACK_QUANTITY));
    }

    #[test]
    fn the_result_is_the_six_tuple_the_client_unpacks() {
        let d = Done {
            source_item: 1,
            source_type: 2,
            source_quantity: 3,
            output_item: 4,
            output_type: 5,
            output_quantity: 6,
        };
        assert_eq!(
            d.to_pyvalue(),
            PyValue::Tuple((1..=6).map(PyValue::Int).collect())
        );
    }
}
