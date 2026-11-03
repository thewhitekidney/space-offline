//! Undock points: CCP's own `ballparkCommon/docking.py`, ported.
//!
//! A ship that undocks does not appear at the station's centre, and it does
//! not appear at a flat offset along one axis either. Every station model
//! carries one or more **docking mouths** as named locators in its own model
//! space, and some structures carry several -- a Keepstar has twenty-five,
//! sorted into four size classes. Which mouth a given ship gets, and which
//! way it is pointing when it arrives, is CCP's algorithm, and the client
//! ships it: `out/pyc/ballparkCommon/docking.pyc`, the same category of find
//! as `baseColony.pyc` for planetary interaction and `industry/` for
//! manufacturing. This module is that file, function for function.
//!
//! **It is the server's code, not only the client's.** `GetUndockVector` is
//! imported by `behaviors/utility/spawning.py`'s `EntitySpawnerWithUndock`,
//! which is NPC-behaviour code -- so the algorithm below is the one that
//! decides where CCP's own server puts a hull. **[V]**
//!
//! # The chain
//!
//! ```text
//! station typeID -> types.graphicID -> graphicids.graphicLocationID
//!                -> graphiclocations.directionalLocators
//! ```
//!
//! and each locator is `(category, name, position, direction)` with the two
//! vectors in the **model's own space**, in metres. The five category names
//! are `eve/common/lib/appConst`'s `*_UNDOCKPOINT_CATEGORY` constants
//! **[V]**; the ship's `groupID` picks one.
//!
//! `graphiclocations.fsdbinary` has no FSD schema, so its layout was derived:
//! stride 40, 68 rows, three 8-byte fields then a `uint32` presence bitfield,
//! and the two list fields hold **inline structs** rather than pointers. The
//! decoder is `tools/fsd.py`'s `graphic_locations`, the layout is in
//! `findings/pyd-schemas.tsv`, and the account is `docs/lanes/undock-points.md`.
//!
//! # The world transform, and why there is no rotation here
//!
//! A locator is in model space, so a world position needs the station's
//! position *and* its orientation -- and the orientation was the open
//! question. It is **settled, and the answer is that there is none to find**:
//!
//! - `GetUndockVector`'s only rotation input is
//!   `ballpark.GetSlimItemField(ballID, 'dunRotation')` -- a field of the
//!   **slim item the server itself sends**, a `(yaw, pitch, roll)` triple in
//!   degrees. When it is falsy the function applies no rotation at all and
//!   adds the model-space position straight to the ball's `x, y, z`. **[V]**
//! - Nothing in the client derives a station orientation from static data.
//!   `bin64/staticdata/mapObjects.db` has `x, y, z` and **no rotation
//!   column** in either `npcStations` or `celestials` **[V]**, and the one
//!   convention that does exist -- `spaceObject._ShouldFaceTheSun` -- is
//!   gated on `inventorycommon.const.TYPES_THAT_ALWAYS_FACE_THE_SUN`, which
//!   is **fifteen Triglavian and CONCORD stellar-harvester types and no
//!   station** **[V]**. It also only turns the *model*, never the undock
//!   vector: `GetUndockVector` does not consult it.
//!
//! So the transform is a pure translation unless the server declares a
//! rotation, and this server declares none. [`undock_vector`] keeps the
//! rotation branch anyway, because it is the ported function's shape and
//! because a future dungeon-placed structure is exactly what would need it.
//!
//! # What this module deliberately does not have
//!
//! - **CCP's locator geometry in the source.** The rows come from the
//!   content bundle's `universe/dock-locators.tsv` (CP3c,
//!   `docs/content/schema.md`), loaded into a [`Table`] that production
//!   reads through `reg.content()`. [`HAND_ROWS`] -- the one row this server
//!   docks characters in -- is only the no-bundle fallback, and what
//!   [`Table::fixture`] holds.
//! - **A random choice.** CCP picks a mouth with `random.choice`; this crate
//!   has no RNG and `server/deps.allow` is empty. [`undock_direction`] takes
//!   the index instead, and [`undock_point`] passes `0`.
//! - **Any reach into the world.** No `Registry`, no `Store`; the locator
//!   [`Table`] is an argument, and the only thing this module reads outside
//!   itself is [`crate::item::find_in`], for the ship's `groupID`.

use std::borrow::Cow;

use crate::content::Content;
use crate::item;

/// Where a ship appears when it undocks, and which way it is facing.
///
/// `position` is in world coordinates, metres; `direction` is a unit vector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Undock {
    pub position: [f64; 3],
    pub direction: [f64; 3],
}

/// One of `appConst`'s five `*_UNDOCKPOINT_CATEGORY` values. **[V]**
///
/// `eve/common/lib/appConst`:
///
/// ```text
/// DEFAULT_UNDOCKPOINT_CATEGORY      = 'undockPoint'
/// SMALL_UNDOCKPOINT_CATEGORY        = 'undockPointSmall'
/// LARGE_UNDOCKPOINT_CATEGORY        = 'undockPointLarge'
/// CAPITAL_UNDOCKPOINT_CATEGORY      = 'undockPointCapital'
/// SUPERCAPITAL_UNDOCKPOINT_CATEGORY = 'undockPointSupercapital'
/// ```
///
/// [`Default`] is the odd one out: no *ship* ever maps to it, because
/// [`DEFAULT_MAPPING`] is [`Small`](UndockCategory::Small). It is the category
/// the **data** uses for a station with a single undifferentiated mouth, which
/// is every pre-Upwell NPC station, and it is reached through the fallback arm
/// of [`undock_direction`] rather than through the group mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndockCategory {
    Default,
    Small,
    Large,
    Capital,
    Supercapital,
}

impl UndockCategory {
    /// The string the client's data and its code both use.
    ///
    /// ```
    /// use eve_server::docking::UndockCategory;
    /// assert_eq!(UndockCategory::Supercapital.name(), "undockPointSupercapital");
    /// ```
    pub const fn name(self) -> &'static str {
        match self {
            UndockCategory::Default => "undockPoint",
            UndockCategory::Small => "undockPointSmall",
            UndockCategory::Large => "undockPointLarge",
            UndockCategory::Capital => "undockPointCapital",
            UndockCategory::Supercapital => "undockPointSupercapital",
        }
    }

    /// The category whose [`name`](Self::name) is `name`, the way the
    /// bundle writes it; `None` for anything else.
    ///
    /// ```
    /// use eve_server::docking::UndockCategory;
    /// assert_eq!(UndockCategory::from_name("undockPointLarge"), Some(UndockCategory::Large));
    /// assert_eq!(UndockCategory::from_name("cloningService"), None);
    /// ```
    pub fn from_name(name: &str) -> Option<Self> {
        [
            UndockCategory::Default,
            UndockCategory::Small,
            UndockCategory::Large,
            UndockCategory::Capital,
            UndockCategory::Supercapital,
        ]
        .into_iter()
        .find(|c| c.name() == name)
    }
}

/// `docking.DEFAULT_MAPPING`. **[V]**
///
/// A ship whose group is not in [`GROUP_MAPPING`] -- and a type the item
/// table has never heard of -- undocks as a small ship. Note that this is
/// *not* [`UndockCategory::Default`]; CCP's default *mapping* is the small
/// category, and the `undockPoint` category is a property of the data.
pub const DEFAULT_MAPPING: UndockCategory = UndockCategory::Small;

/// `docking.DEFAULT_DIRECTION`: the station's own centre, facing `+Z`. **[V]**
///
/// CCP's last-resort answer when a station has no locators at all. It is a
/// safety net in their data and a bug in ours -- putting the ship at the
/// centre of a sixteen-kilometre model is the exact failure this module
/// exists to fix -- so [`undock_point`] returns `None` instead of reaching
/// for it. See that function.
pub const DEFAULT_DIRECTION: Undock = Undock {
    position: [0.0, 0.0, 0.0],
    direction: [0.0, 0.0, 1.0],
};

/// `docking.GROUP_MAPPING`: eighteen ship `groupID`s to a category. **[V]**
///
/// Read out of `ballparkCommon/docking.pyc` with `tools/pyc27.py`, with the
/// group ids resolved from `inventorycommon/const.pyc` the same way. Every
/// group CCP names is here and nothing else is; a group that is absent gets
/// [`DEFAULT_MAPPING`], which is the whole of frigates, cruisers,
/// battlecruisers, corvettes and capsules.
///
/// These eighteen integers are part of the ported *function*, not an extract
/// of CCP's type database: without them `GetUndockCategoryByType` has no
/// body.
pub const GROUP_MAPPING: &[(i64, UndockCategory)] = &[
    // LARGE_UNDOCKPOINT_CATEGORY
    (27, UndockCategory::Large),   // groupBattleship
    (28, UndockCategory::Large),   // groupIndustrial
    (380, UndockCategory::Large),  // groupTransportShip
    (381, UndockCategory::Large),  // groupEliteBattleship
    (513, UndockCategory::Large),  // groupFreighter
    (898, UndockCategory::Large),  // groupBlackOps
    (900, UndockCategory::Large),  // groupMarauders
    (902, UndockCategory::Large),  // groupJumpFreighter
    (941, UndockCategory::Large),  // groupIndustrialCommandShip
    (1202, UndockCategory::Large), // groupBlockadeRunner
    // CAPITAL_UNDOCKPOINT_CATEGORY
    (485, UndockCategory::Capital),  // groupDreadnought
    (547, UndockCategory::Capital),  // groupCarrier
    (883, UndockCategory::Capital),  // groupCapitalIndustrialShip
    (1538, UndockCategory::Capital), // groupForceAux
    (4594, UndockCategory::Capital), // groupLancerDreadnought
    (5120, UndockCategory::Capital), // groupCommandCarrier
    // SUPERCAPITAL_UNDOCKPOINT_CATEGORY
    (30, UndockCategory::Supercapital),  // groupTitan
    (659, UndockCategory::Supercapital), // groupSupercarrier
];

/// `docking.ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE`. **[V]**
///
/// A structure that will let a size class *out* but not back *in*. Only
/// consulted by [`can_dock`], and no type here is one this server has.
pub const ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE: &[(i64, &[UndockCategory])] = &[
    // typeEngineeringComplexRaitaru
    (
        35_825,
        &[UndockCategory::Capital, UndockCategory::Supercapital],
    ),
    // typeEngineeringComplexAzbel
    (
        35_826,
        &[UndockCategory::Capital, UndockCategory::Supercapital],
    ),
    // typeEngineeringComplexSotiyo
    (35_827, &[UndockCategory::Supercapital]),
];

/// `docking.EXCLUDED_DOCKS_BY_STRUCTURE_TYPE`. **[V]**
///
/// Ship *groups* a particular structure type refuses outright. Only
/// consulted by [`can_dock`].
pub const EXCLUDED_DOCKS_BY_STRUCTURE_TYPE: &[(i64, &[i64])] = &[
    // typeRefineryTatara: carriers, command carriers, force auxiliaries,
    // dreadnoughts, lancer dreadnoughts.
    (35_836, &[547, 5120, 1538, 485, 4594]),
    // typeInsurgencyFOBAngel: freighters and jump freighters.
    (78_260, &[513, 902]),
    // typeInsurgencyFOBGuristas: the same two.
    (79_172, &[513, 902]),
];

/// One docking mouth of one station graphic.
///
/// `position` and `direction` are in the model's own space and `f32` because
/// that is their width in `graphiclocations.fsdbinary` -- keeping them `f32`
/// rather than widening at generation time is what stops a test comparing
/// `-12_607.206_f64` against a number that can only be
/// `-12_607.206_054_687_5`.
///
/// The bundle widens each `f32` with `repr`, so narrowing it back with `as`
/// is exact (`docs/content/schema.md`, `dock-locators.tsv`).
#[derive(Debug, Clone, PartialEq)]
pub struct Locator {
    pub category: UndockCategory,
    /// CCP's own name for the mouth: `undockPoint`, `capital01`, `small07`.
    /// **Not unique within a graphic**: one graphic in the client repeats
    /// `undockPoint` for a dozen mouths, so nothing keys on it.
    pub name: Cow<'static, str>,
    pub position: [f32; 3],
    pub direction: [f32; 3],
}

/// One station type's docking geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct StationGraphic {
    pub station_type_id: i64,
    /// `types.graphicID`, kept so the chain can be re-walked by hand.
    pub graphic_id: i64,
    /// `graphicids.graphicLocationID`, the key into `graphiclocations`.
    pub graphic_location_id: i64,
    /// CCP's model name for the hull, e.g. `ms3`. Not used; it is the
    /// cheapest way for a reader to confirm a regenerated row is the same
    /// row.
    pub hull: Cow<'static, str>,
    /// In file order, which is the order [`undock_direction`]'s `pick`
    /// indexes.
    pub undock_points: Cow<'static, [Locator]>,
}

/// **The no-bundle fallback**: every station type this server can dock a
/// character in, hand-copied from the client before the bundle existed.
///
/// Production reads [`Table`], which holds the bundle's
/// `universe/dock-locators.tsv` when there is one; these rows are what it
/// holds when there is not ([`Table::hand`]), and what tests run on
/// ([`Table::fixture`]). The bundle never merges with them: a bundle is the
/// whole answer.
///
/// # Why one row and not sixty-eight
///
/// The selection rule is mechanical and testable: **a station graphic is in
/// this table exactly when some station type this server docks characters in
/// resolves to it.** Today that is the single type in
/// `model::Character::default().station_type_id`, and
/// `tests::every_station_this_server_docks_in_has_its_undock_geometry`
/// enforces it -- so adding a second station and not regenerating is a test
/// failure rather than a ship that silently reappears twenty kilometres along
/// `+X`. It is deliberately *not* keyed on [`crate::item::SPAWNABLE`], which
/// contains no station type at all: 60 types, categories 4, 6, 7, 8, 9, 17
/// and 25, and nothing in category 3.
///
/// `graphiclocations.fsdbinary` holds 68 rows and 235 directional locators;
/// 44 of those rows are reachable from a group-15 type. We ship the one we
/// use. **The interesting row we do not ship is graphicLocationID 45, hull
/// `cxl1_t1` -- the Keepstar -- with twenty-five mouths: three
/// supercapital, four capital, four large and fourteen small.** It is written
/// up in `docs/lanes/undock-points.md` because it is the case the whole
/// category mechanism exists for, and because a Keepstar is not something
/// this server has.
///
/// # Regeneration
///
/// ```text
/// tools/analysis.sh python3 -c "
/// import sys, struct; sys.path.insert(0, '/work/tools'); import fsd
/// c  = fsd.Client('/client', resfiles='/client-root/ResFiles')
/// ty = fsd.CfsdTable(c.path('res:/staticdata/types.fsdbinary'), 'types')
/// gi = fsd.CfsdTable(c.path('res:/staticdata/graphicids.fsdbinary'), 'graphicids')
/// gl = fsd.CfsdTable(c.path('res:/staticdata/graphiclocations.fsdbinary'), 'graphiclocations')
/// rows, locs = fsd.cfsd_rows(ty, 'types'), fsd.graphic_locations(gl)
/// for t in (2502,):
///     g = rows[t]['graphicID']
///     lid = struct.unpack_from('<i', gi.pl, dict(gi.records())[g] + 148)[0]
///     print(t, g, lid, locs[lid])
/// "
/// ```
///
/// prints, for build 24.01 / 3619879:
///
/// ```text
/// 2502 1138 11 {'hull': 'ms3', 'directionalLocators': [{'category': 'undockPoint',
///   'name': 'undockPoint', 'direction': (0.0, 5.960464477539063e-08, -1.0),
///   'position': (17.0, 10096.0, -12607.2060546875)}], 'locators': None}
/// ```
///
/// `locators` -- the non-directional set, service points such as
/// `cloningService` -- is `None` for this row: its presence bit is clear.
/// Nothing here needs it; it is the station-services geometry.
pub const HAND_ROWS: &[StationGraphic] = &[
    // 2502 "Minmatar Trade Post", the type of station 60012526, which is
    // where every character in this sandbox starts docked. Hull `ms3`, so
    // Minmatar -- `destiny.rs`'s "a Caldari station" comment on
    // `STATION_RADIUS` is wrong about the race and right about the number.
    //
    // One mouth, category `undockPoint`, which is how every pre-Upwell NPC
    // station is built: no size classes, so every hull uses it. Its distance
    // from the model's origin is 16,151 m against a `types.radius` of
    // 15,733 m, so the mouth is just outside the bounding sphere -- which is
    // the first thing to check on a regenerated row, and what
    // `tests::the_undock_point_clears_the_station_hull` pins.
    StationGraphic {
        station_type_id: 2502,
        graphic_id: 1138,
        graphic_location_id: 11,
        hull: Cow::Borrowed("ms3"),
        undock_points: Cow::Borrowed(&[Locator {
            category: UndockCategory::Default,
            name: Cow::Borrowed("undockPoint"),
            // `5.960_464_5e-8` is 2^-24: CCP's `y` is zero in all but name
            // and the mouth faces straight down `-Z`.
            //
            // `-12_607.206` is the f32 the file holds; written out in full it
            // is `-12_607.206_054_687_5`, which is the same f32 and which
            // `clippy::excessive_precision` rejects as redundant.
            position: [17.0, 10_096.0, -12_607.206],
            direction: [0.0, 5.960_464_5e-8, -1.0],
        }]),
    },
];

/// The bundle file this module reads (`docs/content/schema.md`).
pub const FILE: &str = "universe/dock-locators.tsv";

/// [`FILE`]'s header line, exactly as `schema.md` prints it.
pub const HEADER: &str = "#station_type_id\tgraphic_id\tgraphic_location_id\tcategory\tname\t\
                          px\tpy\tpz\tdx\tdy\tdz\thull";

/// Every station type's undock geometry, as one value
/// (`docs/content/plan.md` §1.5, ruling 3).
///
/// Production reads it through `reg.content().get::<docking::Table>()`;
/// the lookups below take `&Table`, so a test can hand them any rows.
///
/// ```
/// use eve_server::docking::{undock_point, Table};
/// let c = eve_server::content::Content::fixture();
/// let t = Table::parse(&["990002\t99901\t99\tundockPoint\tundockPoint\t\
///                         1.0\t2.0\t-3.0\t0.0\t0.0\t-1.0\txx1"]).unwrap();
/// let u = undock_point(&t, &c, 990002, [100.0, 0.0, 0.0], 587).unwrap();
/// assert_eq!(u.position, [101.0, 2.0, -3.0]);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// One entry per station type, in first-seen order.
    stations: Vec<StationGraphic>,
}

/// This module's registration row, one line in `TABLES` in
/// `content_tables.rs` (coordinator-only).
pub const LOCATORS: crate::content::TableSpec = crate::content::TableSpec {
    name: "docking",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// The bundle's [`FILE`] when the bundle has one; otherwise [`HAND_ROWS`],
/// reported as `baked` (CA4). A bundle that predates the file still docks.
fn boxed_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    match src.bundle.filter(|b| b.path(FILE).is_some()) {
        Some(bundle) => Ok(Box::new(Table::from_bundle(bundle)?)),
        None => {
            src.note(crate::content::Origin::Baked);
            Ok(Box::new(Table::hand()))
        }
    }
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

impl Table {
    /// [`HAND_ROWS`], the no-bundle fallback.
    pub fn hand() -> Table {
        Table {
            stations: HAND_ROWS.to_vec(),
        }
    }

    /// The rows tests and the no-bundle server run on: [`Table::hand`].
    pub fn fixture() -> Table {
        Table::hand()
    }

    /// The bundle's [`FILE`]. An absent file is an empty table; a wrong one
    /// is an error naming the file.
    pub fn from_bundle(bundle: &crate::content::Bundle) -> Result<Table, String> {
        let lines = bundle.data_lines(FILE, HEADER)?;
        Table::parse(&lines).map_err(|e| format!("{FILE}: {e}"))
    }

    /// [`FILE`]'s data lines, header and comments already removed.
    ///
    /// Rows of one station type need not be adjacent; their locators keep
    /// file order. A type whose rows disagree on `graphic_id`,
    /// `graphic_location_id` or `hull` is an error, because one type has one
    /// graphic. A repeated `name` is not: the client repeats them.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
        let mut stations: Vec<StationGraphic> = Vec::new();
        for (n, line) in lines.iter().enumerate() {
            let row = n + 1;
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            if f.len() != 12 {
                return Err(format!("data row {row}: {} columns, expected 12", f.len()));
            }
            let int = |i: usize| {
                f[i].parse::<i64>()
                    .map_err(|e| format!("data row {row}: column {}: '{}': {e}", i + 1, f[i]))
            };
            let float = |i: usize| -> Result<f32, String> {
                let v = f[i]
                    .parse::<f64>()
                    .map_err(|e| format!("data row {row}: column {}: '{}': {e}", i + 1, f[i]))?;
                if !v.is_finite() {
                    return Err(format!("data row {row}: column {}: not finite", i + 1));
                }
                // Exact: the writer widened an f32 (see [`Locator`]).
                Ok(v as f32)
            };
            let station_type_id = int(0)?;
            let graphic_id = int(1)?;
            let graphic_location_id = int(2)?;
            let category = UndockCategory::from_name(f[3]).ok_or_else(|| {
                format!("data row {row}: '{}' is not an undock-point category", f[3])
            })?;
            let locator = Locator {
                category,
                name: Cow::Owned(f[4].to_owned()),
                position: [float(5)?, float(6)?, float(7)?],
                direction: [float(8)?, float(9)?, float(10)?],
            };
            let hull = f[11];
            match stations
                .iter_mut()
                .find(|s| s.station_type_id == station_type_id)
            {
                Some(s) => {
                    if s.graphic_id != graphic_id
                        || s.graphic_location_id != graphic_location_id
                        || s.hull != hull
                    {
                        return Err(format!(
                            "data row {row}: station type {station_type_id} has a second \
                             graphic ({graphic_id}, {graphic_location_id}, {hull}); its first \
                             is ({}, {}, {})",
                            s.graphic_id, s.graphic_location_id, s.hull
                        ));
                    }
                    s.undock_points.to_mut().push(locator);
                }
                None => stations.push(StationGraphic {
                    station_type_id,
                    graphic_id,
                    graphic_location_id,
                    hull: Cow::Owned(hull.to_owned()),
                    undock_points: Cow::Owned(vec![locator]),
                }),
            }
        }
        Ok(Table { stations })
    }

    /// One station type's geometry.
    pub fn station(&self, station_type_id: i64) -> Option<&StationGraphic> {
        self.stations
            .iter()
            .find(|s| s.station_type_id == station_type_id)
    }

    /// Every station type, in first-seen order.
    pub fn stations(&self) -> &[StationGraphic] {
        &self.stations
    }
}

/// `docking.GetUndockCategoryByType`: the ship's size class.
///
/// CCP's body is exactly
///
/// ```python
/// try:    return GROUP_MAPPING[evetypes.GetGroupID(typeID)]
/// except (KeyError, evetypes.TypeNotFoundException):
///         return DEFAULT_MAPPING
/// ```
///
/// so both arms collapse to the same thing here: `item::find_in` returning
/// `None` *is* `TypeNotFoundException`, and a group missing from
/// [`GROUP_MAPPING`] is the `KeyError`. **The two are not equivalent in
/// scope**, and that is worth stating: CCP's `evetypes` knows all 53,288
/// types, while `item::SPAWNABLE` knows 60, so a real battleship that is not
/// in our item table takes the `TypeNotFoundException` arm here and the
/// `KeyError` arm there. Both yield [`DEFAULT_MAPPING`], which is why the
/// difference is invisible rather than merely small.
///
/// ```
/// use eve_server::content::Content;
/// use eve_server::docking::{undock_category_by_type, UndockCategory};
/// let c = Content::fixture();
/// // 641 Megathron, group 27 groupBattleship.
/// assert_eq!(undock_category_by_type(&c, 641), UndockCategory::Large);
/// // 587 Rifter, group 25, which CCP does not map.
/// assert_eq!(undock_category_by_type(&c, 587), UndockCategory::Small);
/// ```
pub fn undock_category_by_type(content: &Content, ship_type_id: i64) -> UndockCategory {
    let Some(t) = item::find_in(content, ship_type_id) else {
        return DEFAULT_MAPPING;
    };
    GROUP_MAPPING
        .iter()
        .find(|(group, _)| *group == t.group_id)
        .map_or(DEFAULT_MAPPING, |(_, category)| *category)
}

/// `docking.GetUndockDirections`, as far as this server can go.
///
/// CCP walks `typeID -> graphicID -> graphicLocationID` and buckets the
/// locators by category, with a second bucket under the key `None` holding
/// **every** locator; both lookups miss into `return {}`. [`Table`] is
/// that walk already done, so this is the table lookup, and `None` here is
/// CCP's empty dict.
///
/// The `None` bucket is not a field: it is every element of the returned
/// slice, which is what [`undock_direction`] falls back to.
pub fn undock_directions(table: &Table, station_type_id: i64) -> Option<&[Locator]> {
    table.station(station_type_id).map(|s| &*s.undock_points)
}

/// `docking.GetUndockDirection`: one mouth for one ship.
///
/// CCP's body, with `random.choice` replaced by `pick`:
///
/// ```python
/// directions = GetUndockDirections(typeID)
/// try:    return random.choice(directions[GetUndockCategoryByType(shipTypeID)])
/// except KeyError:
///         return random.choice(directions.get(None, [DEFAULT_DIRECTION]))
/// ```
///
/// **The `KeyError` arm is the one that carries every NPC station.** A
/// Rifter maps to `undockPointSmall`, graphicLocationID 11 has only an
/// `undockPoint`, so the category lookup misses and the fallback hands back
/// the station's single mouth. Without that arm this function returns nothing
/// for every pre-Upwell station in the game.
///
/// `pick` is taken modulo the number of candidates, so any index is valid.
/// It stands in for CCP's uniform random choice; a caller that wants ships
/// spread across a Keepstar's fourteen small mouths passes a counter.
pub fn undock_direction<'t>(
    table: &'t Table,
    content: &Content,
    station_type_id: i64,
    ship_type_id: i64,
    pick: usize,
) -> Option<&'t Locator> {
    let locators = undock_directions(table, station_type_id)?;
    if locators.is_empty() {
        return None;
    }
    let category = undock_category_by_type(content, ship_type_id);
    let matching = locators.iter().filter(|l| l.category == category).count();
    if matching == 0 {
        // CCP's `except KeyError` arm: `directions[None]`, every locator.
        return locators.get(pick % locators.len());
    }
    locators
        .iter()
        .filter(|l| l.category == category)
        .nth(pick % matching)
}

/// `docking.GetUndockVector`: the model-space mouth, placed in the world.
///
/// CCP's body, with the ballpark lookups replaced by arguments:
///
/// ```python
/// ballTypeID = ballpark.GetBallType(ballID)
/// position, direction = GetUndockDirection(ballTypeID, typeID)
/// rotation = ballpark.GetSlimItemField(ballID, 'dunRotation')
/// if rotation:
///     rotation = map(math.radians, rotation)
///     quaternion = geo2.QuaternionRotationSetYawPitchRoll(*rotation)
///     position = geo2.QuaternionTransformVector(quaternion, position)
///     direction = geo2.QuaternionTransformVector(quaternion, direction)
/// ball = ballpark.CheckBalls(ballID)
/// position = [ball.x + position[0], ball.y + position[1], ball.z + position[2]]
/// direction = geo2.Vec3Normalize(direction)
/// if IsFlexStructure(ballTypeID):
///     position = geo2.Vec3AddD(position, geo2.Vec3ScaleD(direction, ball.radius))
/// return tuple(position), direction
/// ```
///
/// `rotation_degrees` is the station's `dunRotation`, which this server never
/// sends -- see the module header for why there is nothing else it could be
/// derived from. `None` is the ordinary case and means "model axes are world
/// axes".
///
/// The `IsFlexStructure` tail is kept and is unreachable: see
/// [`is_flex_structure`].
pub fn undock_vector(
    table: &Table,
    content: &Content,
    station_type_id: i64,
    station_position: [f64; 3],
    ship_type_id: i64,
    rotation_degrees: Option<[f64; 3]>,
    pick: usize,
) -> Option<Undock> {
    let locator = undock_direction(table, content, station_type_id, ship_type_id, pick)?;
    let mut position = [
        f64::from(locator.position[0]),
        f64::from(locator.position[1]),
        f64::from(locator.position[2]),
    ];
    let mut direction = [
        f64::from(locator.direction[0]),
        f64::from(locator.direction[1]),
        f64::from(locator.direction[2]),
    ];
    // `if rotation:` in Python is false for both a missing field and an
    // all-zero triple, and `(0, 0, 0)` degrees is the identity anyway.
    if let Some([yaw, pitch, roll]) = rotation_degrees
        && [yaw, pitch, roll] != [0.0, 0.0, 0.0]
    {
        let q = quaternion_yaw_pitch_roll(yaw.to_radians(), pitch.to_radians(), roll.to_radians());
        position = quaternion_transform_vector(q, position);
        direction = quaternion_transform_vector(q, direction);
    }
    for (axis, offset) in position.iter_mut().zip(station_position) {
        *axis += offset;
    }
    let direction = normalize(direction)?;
    // **A deliberate refusal, not an oversight.** CCP's tail here is
    // `position += direction * ball.radius`, and the ball's radius is not an
    // argument of this function -- the brief's signature has no room for one
    // and inventing a radius would be worse than refusing. Unreachable
    // today: nothing in this server is a flex structure (the home hulls are
    // medium), and `tests::nothing_in_this_server_is_a_flex_structure` says so, so
    // whoever adds the first one is told to finish this arm rather than
    // getting a silently misplaced ship.
    if is_flex_structure(content, station_type_id) {
        return None;
    }
    Some(Undock {
        position,
        direction,
    })
}

/// Where a ship undocking from this station appears. The coordinator's entry
/// point.
///
/// `None` when this server has no locator geometry for `station_type_id` --
/// which is **not** what CCP does. Their `GetUndockDirection` falls through to
/// [`DEFAULT_DIRECTION`], a zero offset, putting the ship at the station's
/// centre; that is a safety net in data where every station has a mouth, and
/// in ours it would be the sixteen-kilometres-inside-the-model bug this
/// module exists to fix. `None` lets the caller keep a fallback that is at
/// least plainly outside, and makes a missing row visible.
///
/// ```
/// use eve_server::docking::{undock_point, Table};
/// let c = eve_server::content::Content::fixture();
/// let t = Table::fixture();
/// let u = undock_point(&t, &c, 2502, [1.0e9, 0.0, -2.0e9], 587).expect("station 2502");
/// // Model-space mouth, translated by the station's own position.
/// assert_eq!(u.position[0], 1.0e9 + 17.0);
/// // Facing out of the mouth, down the model's -Z.
/// assert!(u.direction[2] < -0.999);
/// assert!(undock_point(&t, &c, 1, [0.0; 3], 587).is_none());
/// ```
pub fn undock_point(
    table: &Table,
    content: &Content,
    station_type_id: i64,
    station_position: [f64; 3],
    ship_type_id: i64,
) -> Option<Undock> {
    undock_vector(
        table,
        content,
        station_type_id,
        station_position,
        ship_type_id,
        None,
        0,
    )
}

/// `docking.CanDock`. **Only meaningful for an Upwell structure.**
///
/// ```python
/// directions = GetUndockDirections(typeID)
/// undockCategory = GetUndockCategoryByType(shipTypeID)
/// if undockCategory not in directions:
///     return False
/// if evetypes.GetGroupID(shipTypeID) in EXCLUDED_DOCKS_BY_STRUCTURE_TYPE.get(typeID, []):
///     return False
/// return undockCategory not in ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE.get(typeID, [])
/// ```
///
/// The first test is on the *category* key, never the `None` bucket -- so for
/// a station whose only mouth is `undockPoint` this returns `false` for every
/// ship in the game, including the ones that dock there every day. That is
/// not a decode error and it is not a bug: the client only ever asks when you
/// are inside a structure. `eve/client/script/ui/station/undockQuestions.py`
/// guards the call with `if session.structureid:` and reads
/// `session.structureid`'s type, not `session.stationid`'s. **[V]**
///
/// It is ported anyway, with that finding attached, so that the next person
/// who reaches for a docking check does not wire it into the NPC-station path
/// and get a server that refuses every undock.
pub fn can_dock(table: &Table, content: &Content, station_type_id: i64, ship_type_id: i64) -> bool {
    let Some(locators) = undock_directions(table, station_type_id) else {
        // CCP: `directions` is `{}`, so the category is not in it.
        return false;
    };
    let category = undock_category_by_type(content, ship_type_id);
    if !locators.iter().any(|l| l.category == category) {
        return false;
    }
    if let Some(t) = item::find_in(content, ship_type_id) {
        let excluded = EXCLUDED_DOCKS_BY_STRUCTURE_TYPE
            .iter()
            .find(|(type_id, _)| *type_id == station_type_id)
            .map_or(&[][..], |(_, groups)| groups);
        if excluded.contains(&t.group_id) {
            return false;
        }
    }
    let one_way = ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE
        .iter()
        .find(|(type_id, _)| *type_id == station_type_id)
        .map_or(&[][..], |(_, categories)| categories);
    !one_way.contains(&category)
}

/// `structures.types.IsFlexStructure`.
///
/// ```python
/// if GetCategoryID(typeID) != invConst.categoryStructure:
///     return False
/// return GetStructureSize(typeID) == const.STRUCTURE_SIZE_FLEX
/// ```
///
/// **[V]**. `categoryStructure` is 65, and the size is
/// `structures.STRUCTURE_SIZE_BY_TYPE`, which [`crate::structure::size_of_type`]
/// ports (U4). Before U4 this was the category test alone, which was true for
/// every Upwell hull and made [`undock_vector`] refuse them all; only the
/// four flex types (Ansiblex, Pharolynx, Tenebrex, Metenox) are the ones whose
/// undock point is pushed out along its direction by the ball's radius, which
/// is what [`undock_vector`]'s unreachable tail does for them.
pub fn is_flex_structure(_content: &Content, type_id: i64) -> bool {
    crate::structure::size_of_type(type_id) == crate::structure::Size::Flex
}

/// `docking.ShipUndockDirection`: the per-ship jitter on the mouth's heading.
///
/// ```python
/// def ShipUndockDirection(direction, jitter = 0.25):
///     yaw   = random.uniform(-jitter, jitter)
///     pitch = random.uniform(-jitter, jitter)
///     roll  = random.uniform(-jitter, jitter)
///     quaternion = geo2.QuaternionRotationSetYawPitchRoll(yaw, pitch, roll)
///     return geo2.QuaternionTransformVector(quaternion, direction)
/// ```
///
/// The angles are **radians**, so [`JITTER`] is about 14 degrees either way on
/// each axis -- that is why two ships leaving the same mouth do not fly in
/// formation. The three draws are the caller's, because this crate has no
/// RNG; passing `0.0, 0.0, 0.0` is the identity and is what [`undock_point`]
/// effectively does by not calling this at all.
pub fn ship_undock_direction(direction: [f64; 3], yaw: f64, pitch: f64, roll: f64) -> [f64; 3] {
    quaternion_transform_vector(quaternion_yaw_pitch_roll(yaw, pitch, roll), direction)
}

/// `docking.ShipUndockDirection`'s default `jitter`, in radians. **[V]**
pub const JITTER: f64 = 0.25;

/// `docking.VELOCITY_MULTIPLIER`. **[V]**
pub const VELOCITY_MULTIPLIER: f64 = 1.75;

/// `docking.VELOCITY_MAXIMUM`, m/s. **[V]**
pub const VELOCITY_MAXIMUM: f64 = 700.0;

/// `docking.ShipUndockVelocity`: how fast the ship leaves.
///
/// ```python
/// speed = min(VELOCITY_MULTIPLIER * dogma.GetAttributeValue(shipID, attributeMaxVelocity),
///             VELOCITY_MAXIMUM)
/// return geo2.Vec3ScaleD(geo2.Vec3Normalize(direction), speed)
/// ```
///
/// **A ship undocks at 1.75x its own top speed, capped at 700 m/s** -- which
/// is why you coast out of a station well above what the ship can sustain.
/// `max_velocity` is attribute 37, the hull's own number; `destiny::hull`
/// already has it for every hull in this sandbox, and it is a parameter here
/// so that this module reads nothing but [`crate::item`].
///
/// ```
/// use eve_server::docking::ship_undock_velocity;
/// // A Rifter's 365 m/s: 1.75 * 365 = 638.75, under the cap.
/// let v = ship_undock_velocity([0.0, 0.0, -1.0], 365.0);
/// assert_eq!(v, [0.0, 0.0, -638.75]);
/// // The cap binds above 400 m/s, which no hull in this sandbox reaches.
/// let v = ship_undock_velocity([0.0, 0.0, -1.0], 1000.0);
/// assert_eq!(v, [0.0, 0.0, -700.0]);
/// ```
pub fn ship_undock_velocity(direction: [f64; 3], max_velocity: f64) -> [f64; 3] {
    let speed = (VELOCITY_MULTIPLIER * max_velocity).min(VELOCITY_MAXIMUM);
    let Some(unit) = normalize(direction) else {
        return [0.0, 0.0, 0.0];
    };
    [unit[0] * speed, unit[1] * speed, unit[2] * speed]
}

/// `geo2.Vec3Normalize`, refusing a zero vector.
///
/// CCP's native version returns `(nan, nan, nan)` for a zero vector and the
/// callers never pass one; returning `None` is the same refusal made visible.
pub fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    // `is_finite` first, so a NaN component is refused rather than compared.
    if !len.is_finite() || len <= 0.0 {
        return None;
    }
    Some([v[0] / len, v[1] / len, v[2] / len])
}

/// `geo2.QuaternionRotationSetYawPitchRoll`, as `(x, y, z, w)`.
///
/// **[V] on the axes and the units**, from `bin64/_geo2.dll`'s own docstring
/// for this function, which is CCP's and not a guess:
///
/// ```text
/// Builds a quaternion with the given yaw, pitch, and roll.
/// :param yaw: Yaw around the y-axis, in radians.
/// :param pitch: Pitch around the x-axis, in radians
/// :param roll: Roll around the z-axis, in radians
/// ```
///
/// **[I] on the formula**: that text is `D3DXQuaternionRotationYawPitchRoll`'s
/// documentation verbatim, so the body below is D3DX's composition
/// (`roll * pitch * yaw`, left-handed). `_geo2.dll` was not disassembled.
/// **The experiment that would settle it** is one line in the client's own
/// Python, through `tools/client-overrides/`:
/// `geo2.QuaternionRotationSetYawPitchRoll(0.3, 0.2, 0.1)`, and compare the
/// four components against `tests::the_quaternion_matches_d3dx`. Until then,
/// nothing in this server reaches this function: the only caller is
/// [`undock_vector`]'s `dunRotation` branch, and no station carries one.
pub fn quaternion_yaw_pitch_roll(yaw: f64, pitch: f64, roll: f64) -> [f64; 4] {
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let (sr, cr) = (roll * 0.5).sin_cos();
    [
        cy * sp * cr + sy * cp * sr,
        sy * cp * cr - cy * sp * sr,
        cy * cp * sr - sy * sp * cr,
        cy * cp * cr + sy * sp * sr,
    ]
}

/// `geo2.QuaternionTransformVector`: rotate `v` by `q`.
///
/// `v + 2w(u x v) + 2(u x (u x v))` with `u = (x, y, z)`, which is
/// `q v q*` written without the quaternion multiply. Exact for a unit `q`.
pub fn quaternion_transform_vector(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let uv = cross(u, v);
    let uuv = cross(u, uv);
    [
        v[0] + 2.0 * (w * uv[0] + uuv[0]),
        v[1] + 2.0 * (w * uv[1] + uuv[1]),
        v[2] + 2.0 * (w * uv[2] + uuv[2]),
    ]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(v: [f64; 3]) -> f64 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// Pins: the table covers every station this server docks characters in.
    ///
    /// The selection rule of [`HAND_ROWS`], and the reason adding a second
    /// station type is a test failure rather than a ship at the wrong place.
    /// `Character::default()` is the only place a station type is chosen.
    #[test]
    fn every_station_this_server_docks_in_has_its_undock_geometry() {
        let table = Table::fixture();
        let station = crate::model::Character::default().station_type_id;
        let points = undock_directions(&table, station)
            .unwrap_or_else(|| panic!("no undock geometry for station type {station}"));
        assert!(
            !points.is_empty(),
            "station type {station} has a row with no mouths"
        );
    }

    /// Pins: a mouth's heading is a unit vector.
    ///
    /// The property that falsifies a mis-decoded `graphiclocations` row. The
    /// two f32 vectors sit at struct `+16` and `+28`; reading them at the
    /// wrong offset, or reading `position` as `direction`, gives a length of
    /// thousands or of zero rather than one.
    #[test]
    fn every_mouths_direction_is_a_unit_vector() {
        for s in HAND_ROWS {
            for l in s.undock_points.iter() {
                let d = [
                    f64::from(l.direction[0]),
                    f64::from(l.direction[1]),
                    f64::from(l.direction[2]),
                ];
                assert!(
                    (len(d) - 1.0).abs() < 1.0e-5,
                    "{} mouth {} direction {:?} has length {}",
                    s.hull,
                    l.name,
                    l.direction,
                    len(d)
                );
            }
        }
    }

    /// Pins: the ship appears outside the station model, not inside it.
    ///
    /// The whole point of the module. `destiny::STATION_RADIUS` is 15,733 m
    /// for type 2502 and the mouth is 16,151 m out, so the check is against
    /// the radius the server already believes. A row regenerated from the
    /// wrong field, or a transform that dropped the model-space offset, puts
    /// this inside the hull.
    #[test]
    fn the_undock_point_clears_the_station_hull() {
        let c = Content::fixture();
        let table = Table::fixture();
        let station = crate::model::Character::default().station_type_id;
        let u = undock_point(&table, &c, station, [0.0, 0.0, 0.0], 587).expect("geometry");
        assert!(
            len(u.position) > f64::from(crate::destiny::STATION_RADIUS),
            "undock point {:?} is {} m out, inside the {} m hull",
            u.position,
            len(u.position),
            crate::destiny::STATION_RADIUS
        );
    }

    /// Pins: the mouth faces away from the station's centre.
    ///
    /// A ship that flies along `direction` must get farther from the station,
    /// not pass through it. True of every row we ship; **not** a property of
    /// CCP's whole corpus -- 37 of the 235 directional locators in the file
    /// have a negative dot product, all of them recessed bays on Upwell
    /// structures -- so this is scoped to [`HAND_ROWS`] on purpose.
    #[test]
    fn every_mouth_faces_out_of_the_station() {
        for s in HAND_ROWS {
            for l in s.undock_points.iter() {
                let p = [
                    f64::from(l.position[0]),
                    f64::from(l.position[1]),
                    f64::from(l.position[2]),
                ];
                let d = [
                    f64::from(l.direction[0]),
                    f64::from(l.direction[1]),
                    f64::from(l.direction[2]),
                ];
                assert!(
                    dot(p, d) > 0.0,
                    "{} mouth {} points back into the hull",
                    s.hull,
                    l.name
                );
            }
        }
    }

    /// Pins: the world transform is a pure translation by the station's
    /// position.
    ///
    /// This is the orientation finding, made executable. `GetUndockVector`
    /// rotates only when the slim item carries `dunRotation`, and this server
    /// sends none; if anyone later composes an orientation out of `orbitID`,
    /// a celestial, or a convention, this goes red and they have to say why.
    #[test]
    fn the_undock_point_only_translates_with_the_station() {
        let c = Content::fixture();
        let table = Table::fixture();
        let at_origin = undock_point(&table, &c, 2502, [0.0, 0.0, 0.0], 587).expect("geometry");
        let far = [1.234e11, -5.678e10, 9.012e10];
        let moved = undock_point(&table, &c, 2502, far, 587).expect("geometry");
        for ((got, base), offset) in moved.position.iter().zip(at_origin.position).zip(far) {
            assert_eq!(*got, base + offset);
        }
        assert_eq!(moved.direction, at_origin.direction);
    }

    /// Pins: a station with one undifferentiated mouth serves every ship.
    ///
    /// `GetUndockDirection`'s `except KeyError` arm. Our station's only mouth
    /// is category `undockPoint`, which no ship maps to, so without the
    /// fallback every pre-Upwell station answers nothing. A Rifter (Small)
    /// and a Megathron (Large) must both get it, and get the same one.
    #[test]
    fn a_ship_whose_size_class_has_no_mouth_still_gets_one() {
        let c = Content::fixture();
        let table = Table::fixture();
        assert_eq!(undock_category_by_type(&c, 587), UndockCategory::Small);
        assert_eq!(undock_category_by_type(&c, 641), UndockCategory::Large);
        let small = undock_direction(&table, &c, 2502, 587, 0).expect("small");
        let large = undock_direction(&table, &c, 2502, 641, 0).expect("large");
        assert_eq!(small.category, UndockCategory::Default);
        assert_eq!(small, large);
    }

    /// Pins: a type the item table does not know still undocks.
    ///
    /// `GetUndockCategoryByType`'s `TypeNotFoundException` arm. The ship type
    /// comes off a `Character`, which is not constrained to
    /// `item::SPAWNABLE`, so an unknown hull must fall back rather than
    /// refuse. `undock_point` returning `None` here would strand the
    /// character at the station's centre.
    #[test]
    fn an_unknown_ship_type_falls_back_to_the_default_category() {
        let c = Content::fixture();
        let table = Table::fixture();
        assert_eq!(undock_category_by_type(&c, 999_999_999), DEFAULT_MAPPING);
        assert!(undock_point(&table, &c, 2502, [0.0; 3], 999_999_999).is_some());
    }

    /// Pins: every hull in this sandbox gets a category, and six of them are
    /// not the default.
    ///
    /// The group mapping has to actually bite, or it is decoration. Three
    /// group-28 industrials and three group-27 battleships in
    /// `item::SPAWNABLE` are Large; everything else there is Small. Computed
    /// from `SPAWNABLE` rather than listed, so growing it is covered.
    #[test]
    fn the_group_mapping_bites_for_the_hulls_this_server_has() {
        let c = Content::fixture();
        let large: Vec<i64> = crate::item::SPAWNABLE
            .iter()
            .filter(|t| t.category_id == 6)
            .filter(|t| undock_category_by_type(&c, t.type_id) != DEFAULT_MAPPING)
            .map(|t| t.type_id)
            .collect();
        assert_eq!(
            large.len(),
            6,
            "expected six non-default hulls, got {large:?}"
        );
        for type_id in large {
            let t = crate::item::find_in(&c, type_id).expect("spawnable");
            assert_eq!(undock_category_by_type(&c, type_id), UndockCategory::Large);
            assert!(
                t.group_id == 27 || t.group_id == 28,
                "type {type_id} is group {} and should not be Large",
                t.group_id
            );
        }
    }

    /// Pins: `None` for a station we have no geometry for, never a zero
    /// offset.
    ///
    /// CCP's `DEFAULT_DIRECTION` would answer `(0, 0, 0)` and put the ship in
    /// the middle of the model. The caller must be able to tell "no data"
    /// from "the mouth is at the centre", because the fallback it keeps is
    /// not the same answer.
    #[test]
    fn a_station_with_no_geometry_answers_none_rather_than_the_centre() {
        let c = Content::fixture();
        let table = Table::fixture();
        assert!(undock_directions(&table, 2501).is_none());
        assert!(undock_point(&table, &c, 2501, [1.0, 2.0, 3.0], 587).is_none());
        assert_eq!(DEFAULT_DIRECTION.position, [0.0, 0.0, 0.0]);
    }

    /// Pins: `pick` selects among a station's mouths and never panics.
    ///
    /// The stand-in for `random.choice`. Any index must be valid, and for a
    /// one-mouth station every index must give that mouth -- which is the
    /// case a `%` on the wrong length gets wrong.
    #[test]
    fn any_pick_index_is_valid() {
        let c = Content::fixture();
        let table = Table::fixture();
        let only = undock_direction(&table, &c, 2502, 587, 0).expect("mouth");
        for pick in [0, 1, 7, usize::MAX] {
            assert_eq!(undock_direction(&table, &c, 2502, 587, pick), Some(only));
        }
    }

    /// Pins: `can_dock` is false for an NPC station, for every ship.
    ///
    /// Not a bug, and the test exists so nobody "fixes" it into the undock
    /// path: `CanDock` tests the *category* key and an NPC station has only
    /// the `undockPoint` key, so the client asks only when
    /// `session.structureid` is set. If this ever goes green for an NPC
    /// station, the port has drifted from CCP's function.
    #[test]
    fn can_dock_refuses_an_npc_station_because_ccps_function_does() {
        let c = Content::fixture();
        let table = Table::fixture();
        for t in crate::item::SPAWNABLE.iter().filter(|t| t.category_id == 6) {
            assert!(
                !can_dock(&table, &c, 2502, t.type_id),
                "can_dock said yes for {} at an undockPoint-only station",
                t.name
            );
        }
        // And the undock itself is unaffected: that is the whole point.
        assert!(undock_point(&table, &c, 2502, [0.0; 3], 587).is_some());
    }

    /// Pins: `can_dock`'s two exception tables are consulted, not decoration.
    ///
    /// Driven on a synthetic station whose mouths are the categories CCP's
    /// Tatara and Raitaru rows discriminate, so the arms are exercised
    /// without shipping an Upwell row. A Rifter is Small, a Megathron Large.
    #[test]
    fn the_exception_tables_are_the_ones_ccp_wrote() {
        // 35836 Tatara excludes five capital *groups*; 35825 Raitaru makes
        // capital and supercapital one-way; 35827 Sotiyo only supercapital.
        let tatara = EXCLUDED_DOCKS_BY_STRUCTURE_TYPE
            .iter()
            .find(|(t, _)| *t == 35_836)
            .expect("Tatara row");
        assert_eq!(tatara.1.len(), 5);
        assert!(tatara.1.contains(&547), "groupCarrier");
        let raitaru = ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE
            .iter()
            .find(|(t, _)| *t == 35_825)
            .expect("Raitaru row");
        assert_eq!(
            raitaru.1,
            &[UndockCategory::Capital, UndockCategory::Supercapital]
        );
        // Every id in both tables is a type id, not a group id: the smallest
        // group CCP maps is 27 and the smallest type here is 35,825.
        for (type_id, _) in ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE {
            assert!(*type_id > 10_000);
        }
        for (type_id, _) in EXCLUDED_DOCKS_BY_STRUCTURE_TYPE {
            assert!(*type_id > 10_000);
        }
    }

    /// Pins: undock speed is 1.75x the hull's own, capped at 700 m/s.
    ///
    /// Both arms of CCP's `min`, and that the result is along the heading.
    /// **The expected numbers are written out rather than recomputed from
    /// [`VELOCITY_MULTIPLIER`] and [`VELOCITY_MAXIMUM`]**, which is the whole
    /// point: `assert_eq!(v[2], -VELOCITY_MULTIPLIER * speed)` is true for
    /// any multiplier and pins nothing. 1.75 x 365 = 638.75 and the 700
    /// cap are CCP's, so changing either constant turns this red.
    #[test]
    fn undock_speed_is_the_hulls_own_times_the_multiplier_until_the_cap() {
        // A hull speed is an input here, not a table read (the hull table
        // is bundle content since CP58b), so both arms are driven
        // synthetically: below the knee, and either side of it.
        let v = ship_undock_velocity([0.0, 0.0, -1.0], 365.0);
        assert_eq!(v, [0.0, 0.0, -638.75]);
        assert_eq!(
            ship_undock_velocity([1.0, 0.0, 0.0], 399.0),
            [698.25, 0.0, 0.0]
        );
        assert_eq!(
            ship_undock_velocity([1.0, 0.0, 0.0], 400.0),
            [700.0, 0.0, 0.0]
        );
        assert_eq!(
            ship_undock_velocity([0.0, 0.0, -1.0], 10_000.0),
            [0.0, 0.0, -700.0]
        );
    }

    /// Pins: the jitter rotation preserves length and is the identity at zero.
    ///
    /// Convention-independent properties of a unit-quaternion rotation, so
    /// they hold whatever `_geo2.dll` composes. They are what catches a sign
    /// slip in `quaternion_transform_vector`, which is the half of the pair
    /// that is **not** `[I]`.
    #[test]
    fn a_jitter_rotation_turns_the_heading_without_stretching_it() {
        let d = [0.0, 0.0, -1.0];
        assert_eq!(ship_undock_direction(d, 0.0, 0.0, 0.0), d);
        for (yaw, pitch, roll) in [
            (JITTER, 0.0, 0.0),
            (0.0, JITTER, 0.0),
            (0.0, 0.0, JITTER),
            (-JITTER, JITTER, -JITTER),
            (0.1, -0.2, 0.3),
        ] {
            let out = ship_undock_direction(d, yaw, pitch, roll);
            assert!(
                (len(out) - 1.0).abs() < 1.0e-12,
                "{out:?} has length {}",
                len(out)
            );
            // Three axes of at most 0.25 rad cannot turn a vector by 90 deg.
            assert!(dot(out, d) > 0.0, "{out:?} points backwards");
        }
        // And a full turn about one axis comes back to where it started.
        let round = ship_undock_direction(d, std::f64::consts::TAU, 0.0, 0.0);
        for (got, want) in round.iter().zip(d) {
            assert!((got - want).abs() < 1.0e-12);
        }
    }

    /// Pins: the quaternion is D3DX's, which is the `[I]` in this module.
    ///
    /// `_geo2.dll`'s docstring gives the axes and the units **[V]**; the
    /// composition is inferred from that text being D3DX's verbatim. These
    /// four numbers are what to compare against
    /// `geo2.QuaternionRotationSetYawPitchRoll(0.3, 0.2, 0.1)` run in the
    /// client's own Python. **If they disagree, this module is wrong only in
    /// the `dunRotation` branch, which nothing reaches.**
    #[test]
    fn the_quaternion_matches_d3dx() {
        let q = quaternion_yaw_pitch_roll(0.3, 0.2, 0.1);
        let want = [
            0.106_020_511_1,
            0.143_572_175_0,
            0.034_270_798_6,
            0.983_347_443_3,
        ];
        for (i, (got, expected)) in q.iter().zip(want).enumerate() {
            assert!(
                (got - expected).abs() < 1.0e-9,
                "component {i}: {got} vs {expected}"
            );
        }
        // A quaternion from angles is a unit quaternion, whatever the order.
        let n: f64 = q.iter().map(|c| c * c).sum();
        assert!((n - 1.0).abs() < 1.0e-12);
        // Yaw is about +Y, and in a left-handed basis that sends +Z to +X.
        let turned = quaternion_transform_vector(
            quaternion_yaw_pitch_roll(std::f64::consts::FRAC_PI_2, 0.0, 0.0),
            [0.0, 0.0, 1.0],
        );
        assert!((turned[0] - 1.0).abs() < 1.0e-12, "{turned:?}");
    }

    /// Pins: `is_flex_structure` is false for everything this server has.
    ///
    /// The unreachable tail of `GetUndockVector`. If a category-65 type ever
    /// enters `item::SPAWNABLE`, this goes red and whoever added it has to
    /// finish the flex-structure arm rather than get a `None` undock.
    #[test]
    fn nothing_in_this_server_is_a_flex_structure() {
        let c = Content::fixture();
        assert!(!is_flex_structure(&c, 2502));
        for t in crate::item::SPAWNABLE {
            assert!(
                !is_flex_structure(&c, t.type_id),
                "{} is category 65",
                t.name
            );
        }
    }

    /// Pins: `normalize` refuses a zero vector instead of returning NaN.
    #[test]
    fn normalize_refuses_a_zero_vector() {
        assert!(normalize([0.0, 0.0, 0.0]).is_none());
        assert!(normalize([f64::NAN, 0.0, 0.0]).is_none());
        assert_eq!(normalize([0.0, 0.0, -3.0]), Some([0.0, 0.0, -1.0]));
    }

    // ---- CP3c: the bundle's `dock-locators.tsv` --------------------------

    /// A fresh directory under the system temp dir, removed on drop.
    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A bundle directory holding `rows` as [`FILE`] under [`HEADER`], or no
    /// [`FILE`] at all when `rows` is `None`. Invented ids only.
    fn bundle(rows: Option<&str>) -> (Tmp, crate::content::Bundle) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "eve-docking-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("universe")).unwrap();
        if let Some(rows) = rows {
            std::fs::write(dir.join(FILE), format!("{HEADER}\n{rows}")).unwrap();
        }
        let b = crate::content::Bundle { dir: dir.clone() };
        (Tmp(dir), b)
    }

    /// [`LOCATORS`]' `load`, as `content::load` calls it.
    fn load(bundle: Option<&crate::content::Bundle>) -> Result<Table, String> {
        let authored = crate::authored::Dir::compiled_only();
        let src = crate::content::Sources::new(bundle, &authored);
        let boxed = (LOCATORS.load)(&src)?;
        Ok(boxed
            .downcast_ref::<Table>()
            .expect("a docking::Table")
            .clone())
    }

    /// Two invented station types: 990002 with one mouth, 990003 with a
    /// small and a large one, its rows split by 990002's.
    const ROWS: &str = "990003\t99902\t98\tundockPointSmall\tsmall01\t5.0\t0.0\t0.0\t1.0\t0.0\t0.0\txx2\n\
                        990002\t99901\t99\tundockPoint\tundockPoint\t1.0\t2.0\t-3.0\t0.0\t0.0\t-1.0\txx1\n\
                        # a comment\n\
                        990003\t99902\t98\tundockPointLarge\tlarge01\t0.0\t7.0\t0.0\t0.0\t1.0\t0.0\txx2\n";

    /// Pins: a bundle locator gives the undock point and heading.
    ///
    /// The production path: `LOCATORS.load` over a bundle, then
    /// [`undock_point`] translating the model-space mouth by the station's
    /// position.
    #[test]
    fn a_bundle_locator_gives_the_undock_point_and_heading() {
        let c = Content::fixture();
        let (_tmp, b) = bundle(Some(ROWS));
        let table = load(Some(&b)).expect("loads");
        let u = undock_point(&table, &c, 990_002, [100.0, 200.0, 300.0], 587).expect("mouth");
        assert_eq!(u.position, [101.0, 202.0, 297.0]);
        assert_eq!(u.direction, [0.0, 0.0, -1.0]);
        let s = table.station(990_002).expect("row");
        assert_eq!((s.graphic_id, s.graphic_location_id), (99_901, 99));
        assert_eq!(s.hull, "xx1");
    }

    /// Pins: a type's rows group under it in file order, wherever they sit,
    /// and the ship's size class picks among them.
    #[test]
    fn a_stations_rows_group_in_file_order_and_the_class_picks() {
        let c = Content::fixture();
        let (_tmp, b) = bundle(Some(ROWS));
        let table = load(Some(&b)).expect("loads");
        let names: Vec<&str> = undock_directions(&table, 990_003)
            .expect("rows")
            .iter()
            .map(|l| &*l.name)
            .collect();
        assert_eq!(names, ["small01", "large01"]);
        // 587 Rifter is Small, 641 Megathron is Large (`GROUP_MAPPING`).
        let small = undock_direction(&table, &c, 990_003, 587, 0).expect("small");
        let large = undock_direction(&table, &c, 990_003, 641, 0).expect("large");
        assert_eq!(small.category, UndockCategory::Small);
        assert_eq!(large.category, UndockCategory::Large);
    }

    /// Pins: a bundle is the whole answer; the hand rows do not leak in.
    ///
    /// A station type the bundle lacks has no geometry, so the caller's
    /// visible fallback (`destiny::undock_motion`'s offset from the centre)
    /// takes over rather than a stale hand row.
    #[test]
    fn a_bundle_replaces_the_hand_rows() {
        let c = Content::fixture();
        let (_tmp, b) = bundle(Some(ROWS));
        let table = load(Some(&b)).expect("loads");
        assert_eq!(table.stations().len(), 2);
        for hand in HAND_ROWS {
            assert!(undock_directions(&table, hand.station_type_id).is_none());
            assert!(undock_point(&table, &c, hand.station_type_id, [0.0; 3], 587).is_none());
        }
    }

    /// Pins: with no bundle, or a bundle without [`FILE`], the hand rows
    /// are the table.
    #[test]
    fn no_bundle_or_no_file_falls_back_to_the_hand_rows() {
        assert_eq!(load(None).expect("loads"), Table::hand());
        let (_tmp, b) = bundle(None);
        assert_eq!(load(Some(&b)).expect("loads"), Table::hand());
        assert_eq!(Table::fixture(), Table::hand());
    }

    /// Pins: a wrong file is an error naming the file, never a panic.
    #[test]
    fn a_wrong_file_is_an_error_naming_it() {
        let bad = [
            // An unknown category.
            "990002\t99901\t99\tcloningService\tx\t1.0\t2.0\t3.0\t0.0\t0.0\t1.0\txx1\n",
            // Eleven columns.
            "990002\t99901\t99\tundockPoint\tx\t1.0\t2.0\t3.0\t0.0\t0.0\t1.0\n",
            // A non-number.
            "990002\t99901\t99\tundockPoint\tx\t1.0\tnope\t3.0\t0.0\t0.0\t1.0\txx1\n",
            // One type, two graphics.
            "990002\t99901\t99\tundockPoint\ta\t1.0\t2.0\t3.0\t0.0\t0.0\t1.0\txx1\n\
             990002\t99909\t99\tundockPoint\tb\t1.0\t2.0\t3.0\t0.0\t0.0\t1.0\txx1\n",
        ];
        for rows in bad {
            let (_tmp, b) = bundle(Some(rows));
            let e = load(Some(&b)).expect_err(rows);
            assert!(e.contains(FILE), "{e}");
        }
        // A wrong header is caught by `Bundle::data_lines`, naming the path.
        let (tmp, b) = bundle(None);
        std::fs::write(tmp.0.join(FILE), "#station_type_id\n").unwrap();
        assert!(load(Some(&b)).expect_err("header").contains(FILE));
    }

    /// Pins: a repeated mouth name is kept, not merged or refused.
    ///
    /// `schema.md` gives the key as `(station_type_id, name)`, but CCP's own
    /// data repeats `undockPoint` within one graphic, so both mouths must
    /// survive and `pick` must reach each.
    #[test]
    fn a_repeated_mouth_name_is_two_mouths() {
        let c = Content::fixture();
        let rows = "990002\t99901\t99\tundockPoint\tundockPoint\t1.0\t0.0\t0.0\t1.0\t0.0\t0.0\txx1\n\
                    990002\t99901\t99\tundockPoint\tundockPoint\t0.0\t1.0\t0.0\t0.0\t1.0\t0.0\txx1\n";
        let table = Table::parse(&rows.lines().collect::<Vec<_>>()).expect("parses");
        let a = undock_direction(&table, &c, 990_002, 587, 0).expect("first");
        let b = undock_direction(&table, &c, 990_002, 587, 1).expect("second");
        assert_ne!(a.position, b.position);
    }

    /// Pins: the bundle's number format round-trips the hand rows exactly.
    ///
    /// The writer widens each `f32` and prints it with `repr`; Rust's `{}`
    /// on an `f64` is the same shortest round-trip form. So a hand row
    /// written out that way and parsed back must equal itself, bit for bit
    /// -- the property that lets the bundle replace the hand rows without
    /// moving a mouth.
    #[test]
    fn the_hand_rows_round_trip_through_the_bundle_format() {
        let mut lines = Vec::new();
        for s in HAND_ROWS {
            for l in s.undock_points.iter() {
                let v = |x: f32| f64::from(x).to_string();
                lines.push(format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    s.station_type_id,
                    s.graphic_id,
                    s.graphic_location_id,
                    l.category.name(),
                    l.name,
                    v(l.position[0]),
                    v(l.position[1]),
                    v(l.position[2]),
                    v(l.direction[0]),
                    v(l.direction[1]),
                    v(l.direction[2]),
                    s.hull
                ));
            }
        }
        assert_eq!(Table::parse(&lines).expect("parses"), Table::hand());
    }

    /// Pins: every category name parses back to itself.
    #[test]
    fn every_category_name_parses_back() {
        for c in [
            UndockCategory::Default,
            UndockCategory::Small,
            UndockCategory::Large,
            UndockCategory::Capital,
            UndockCategory::Supercapital,
        ] {
            assert_eq!(UndockCategory::from_name(c.name()), Some(c));
        }
    }
}
